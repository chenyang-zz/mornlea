//! Player identity shared by the entity save families.
//!
//! The raw sixteen bytes stay a format value, including the all-zero sentinel
//! hostile saves use for an absent target. Current UUIDv4 admission is owned
//! by `mornlea_domain` and is reached only through the checked conversions.

use crate::error::{StorageResult, corrupt};

/// A stable UUIDv4 player identifier.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlayerId([u8; 16]);

impl PlayerId {
    /// Wraps sixteen raw wire bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Borrows the raw wire bytes.
    pub const fn to_bytes(&self) -> [u8; 16] {
        self.0
    }

    /// Reports whether every byte is zero.
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 16]
    }

    /// Reports whether the domain player constructor accepts these bytes.
    ///
    /// The version and variant masks live in `mornlea_domain`. This wrapper
    /// does not repeat them, and it does not invent an absent domain identity
    /// for the all-zero format sentinel.
    pub fn is_valid(&self) -> bool {
        mornlea_domain::PlayerId::try_from_bytes(self.0).is_ok()
    }
}

/// Converts a raw save identity into a domain player identity.
pub fn checked_player_id(raw: PlayerId) -> StorageResult<mornlea_domain::PlayerId> {
    mornlea_domain::PlayerId::try_from_bytes(raw.to_bytes())
        .map_err(|_| corrupt("player ID", "not UUIDv4"))
}

/// Converts a raw save identity into a domain companion identity.
pub fn checked_companion_id(raw: PlayerId) -> StorageResult<mornlea_domain::CompanionId> {
    mornlea_domain::CompanionId::try_from_bytes(raw.to_bytes())
        .map_err(|_| corrupt("companion ID", "not UUIDv4"))
}

#[cfg(test)]
mod tests {
    use super::PlayerId;

    const TARGET: [u8; 16] = [
        0x6f, 0xce, 0x82, 0x77, 0xa9, 0x33, 0x46, 0xcb, 0x9a, 0x1f, 0xda, 0x13, 0xb7, 0xee, 0x56,
        0x44,
    ];

    #[test]
    fn zero_id_is_not_valid() {
        assert!(PlayerId::from_bytes([0u8; 16]).is_zero());
        assert!(!PlayerId::from_bytes(TARGET).is_zero());
    }

    #[test]
    fn uuidv4_nibbles_decide_validity() {
        assert!(PlayerId::from_bytes(TARGET).is_valid());

        let mut wrong_version = TARGET;
        wrong_version[6] = 0x5c;
        assert!(!PlayerId::from_bytes(wrong_version).is_valid());

        let mut wrong_variant = TARGET;
        wrong_variant[8] = 0x1f;
        assert!(!PlayerId::from_bytes(wrong_variant).is_valid());
    }
}
