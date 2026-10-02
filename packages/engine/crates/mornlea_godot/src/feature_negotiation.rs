//! Client-core feature-family negotiation, mirroring the Go sole registry.
//!
//! The Go c-shared client core owns the single feature-family registry built
//! from the header constants; this module mirrors its table-validation and
//! version-negotiation semantics over the [`FamilyDescriptor`] records the
//! identity family reports, so the Rust consumer can reject an incompatible
//! producer before any world feature is instantiated. The registry reports
//! data-plane availability only; product assembly stays with the Godot-side
//! feature catalog.
//!
//! Negotiation semantics (identical to the Go registry):
//! - an unregistered family identifier is rejected;
//! - any ABI major mismatch is rejected, because the major only moves for
//!   incompatible layout or semantic changes;
//! - a request is accepted when the major matches and the requested minor is
//!   equal to or older than the registered family contract version, because
//!   family versions only rise additively.

// Like the abi mirror, this module is ahead of its non-test consumers: the
// Rust FFI and bridge calls that decide feature compatibility from a
// producer-reported descriptor table land with the later client-core tasks,
// and until then only the tests below call `validate_registry` and
// `negotiate`. Allow `dead_code` module-wide so the negotiation helper does
// not fail `cargo clippy --all-targets -- -D warnings` before those consumers
// exist; remove this allowance once production code consumes the module.
#![allow(dead_code)]

use crate::abi::{self, FamilyDescriptor};

/// Stable classification of one negotiation outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NegotiationReason {
    /// The requested family version is compatible with the registry.
    Compatible,
    /// The requested family identifier is not registered.
    UnknownFamily,
    /// The requested ABI major differs from the producer major.
    MajorMismatch,
    /// The requested minor is newer than the registered contract version.
    MinorTooNew,
}

/// Outcome of one feature-family version check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NegotiationDecision {
    /// Whether the consumer may use the requested family version.
    pub accepted: bool,
    /// Why the decision was made; `Compatible` whenever `accepted` is true.
    pub reason: NegotiationReason,
}

/// Why a descriptor list is not a valid registry table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// A descriptor carries a family identifier outside the pilot set.
    UnknownFamily { family: u32 },
    /// Two descriptors carry the same family identifier.
    DuplicateFamily { family: u32 },
    /// A family required by the pilot data plane is absent.
    MissingRequiredFamily { family: u32 },
    /// A descriptor carries contract version zero; versions start at one.
    ZeroVersion { family: u32 },
}

const fn descriptor(
    family: u32,
    version: u32,
    record_limit: u32,
    record_bytes: u32,
) -> FamilyDescriptor {
    FamilyDescriptor {
        family,
        version,
        record_limit,
        record_bytes,
        reserved: [0; 2],
    }
}

/// The pilot feature-family set the Go sole registry reports, mirrored from
/// the header constants. Every family is required by the pilot data plane;
/// all eight must be present for a producer identity to be usable. Field
/// conventions match the Go registry: `record_limit` is each family's bounded
/// batch capacity, which for byte-payload families (connection addresses) is
/// the byte bound, and `record_bytes` is the fixed per-record wire size where
/// zero marks a variable-length record whose layout lands with that family's
/// later export. The shared cross-language contract is the accept/reject
/// predicate; rejection-reason precedence is a per-language diagnostic detail.
pub const PILOT_FAMILIES: [FamilyDescriptor; abi::FAMILY_COUNT as usize] = [
    descriptor(
        abi::FAMILY_IDENTITY,
        abi::IDENTITY_VERSION,
        abi::FAMILY_COUNT,
        abi::FAMILY_DESCRIPTOR_BYTES as u32,
    ),
    descriptor(
        abi::FAMILY_CONNECTION,
        abi::CONNECTION_VERSION,
        abi::MAX_CONNECTION_ADDRESS_BYTES,
        0,
    ),
    descriptor(
        abi::FAMILY_INPUT,
        abi::INPUT_VERSION,
        abi::MAX_INPUT_EVENTS,
        0,
    ),
    descriptor(
        abi::FAMILY_STEP,
        abi::STEP_VERSION,
        1,
        abi::STEP_REQUEST_BYTES as u32,
    ),
    descriptor(
        abi::FAMILY_WORLD,
        abi::WORLD_VERSION,
        abi::MAX_WORLD_BATCH_OPERATIONS,
        0,
    ),
    descriptor(
        abi::FAMILY_FRAME,
        abi::FRAME_VERSION,
        abi::MAX_ENTITY_RECORDS,
        0,
    ),
    descriptor(
        abi::FAMILY_STATUS,
        abi::STATUS_VERSION,
        abi::MAX_STATUS_RECORDS,
        0,
    ),
    descriptor(
        abi::FAMILY_ENVIRONMENT,
        abi::ENVIRONMENT_VERSION,
        1,
        abi::ENVIRONMENT_BYTES as u32,
    ),
];

/// Every family identifier the pilot contract defines, in ascending order.
const PILOT_FAMILY_IDS: [u32; abi::FAMILY_COUNT as usize] = [
    abi::FAMILY_IDENTITY,
    abi::FAMILY_CONNECTION,
    abi::FAMILY_INPUT,
    abi::FAMILY_STEP,
    abi::FAMILY_WORLD,
    abi::FAMILY_FRAME,
    abi::FAMILY_STATUS,
    abi::FAMILY_ENVIRONMENT,
];

/// Validate a producer-reported descriptor list as a registry table: every
/// descriptor must name a known pilot family with a nonzero contract version,
/// identifiers must be unique, and all eight families are required. Unknown
/// identifiers are rejected before presence checks so a table that both
/// smuggles an unknown family and drops a required one reports the smuggled
/// family first.
pub fn validate_registry(descriptors: &[FamilyDescriptor]) -> Result<(), RegistryError> {
    for descriptor in descriptors {
        if !PILOT_FAMILY_IDS.contains(&descriptor.family) {
            return Err(RegistryError::UnknownFamily {
                family: descriptor.family,
            });
        }
    }
    for family in PILOT_FAMILY_IDS {
        let mut seen = 0;
        for descriptor in descriptors {
            if descriptor.family == family {
                if descriptor.version == 0 {
                    return Err(RegistryError::ZeroVersion { family });
                }
                seen += 1;
            }
        }
        if seen > 1 {
            return Err(RegistryError::DuplicateFamily { family });
        }
        if seen == 0 {
            return Err(RegistryError::MissingRequiredFamily { family });
        }
    }
    Ok(())
}

/// Decide whether the consumer may use `family` at the requested
/// `(major, minor)` against a validated descriptor table. The registry major
/// is the producer ABI major pinned by the header; the registry minor of a
/// family is that family's contract version, which only rises compatibly.
pub fn negotiate(
    descriptors: &[FamilyDescriptor],
    family: u32,
    major: u32,
    minor: u32,
) -> NegotiationDecision {
    let Some(descriptor) = descriptors
        .iter()
        .find(|descriptor| descriptor.family == family)
    else {
        return NegotiationDecision {
            accepted: false,
            reason: NegotiationReason::UnknownFamily,
        };
    };
    if major != abi::ABI_MAJOR {
        return NegotiationDecision {
            accepted: false,
            reason: NegotiationReason::MajorMismatch,
        };
    }
    if minor > descriptor.version {
        return NegotiationDecision {
            accepted: false,
            reason: NegotiationReason::MinorTooNew,
        };
    }
    NegotiationDecision {
        accepted: true,
        reason: NegotiationReason::Compatible,
    }
}

/// The producer identity of the Rust semantic client core. It scopes a
/// separate descriptor table: the pilot IDs 1..8 above are not the meanings
/// of the producer's numeric IDs, and the pilot path stays exactly as it is.
pub const RUST_PRODUCER_NAME: &str = "rust-client-core";

/// The per-family record limit, mirrored from the accepted client-core
/// contract value `ClientLimits::MAX_FAMILY_RECORDS` in
/// `mornlea_client_core/src/contracts.rs`: 4096 records per family per
/// frame, frozen from the measurements of the prerequisite capability
/// inventory. The Godot crate does not depend on the client-core crate, so
/// the mirror is a literal; the registry tests pin the value so the two
/// sides cannot drift silently.
pub const RUST_PRODUCER_FAMILY_RECORD_LIMIT: u64 = 4096;

/// One producer-scoped semantic family descriptor: the symbolic logical key
/// consumers resolve, the numeric ID the producer assigns, the family
/// contract version, and the bounded per-family record capacity. Unlike the
/// pilot [`FamilyDescriptor`], the contract version is per descriptor and the
/// consumer-facing identity is the symbolic logical name; a numeric ID is
/// only meaningful within one producer's table. `record_bytes` stays zero
/// because the semantic records are typed owned values whose per-record
/// layout lands with each family's later export, matching the pilot
/// variable-length convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProducerFamilyDescriptor {
    pub logical_name: &'static str,
    pub numeric_id: u16,
    pub major: u16,
    pub minor: u16,
    pub record_limit: u64,
    pub record_bytes: u64,
}

const fn producer_descriptor(
    logical_name: &'static str,
    numeric_id: u16,
) -> ProducerFamilyDescriptor {
    ProducerFamilyDescriptor {
        logical_name,
        numeric_id,
        major: 1,
        minor: 0,
        record_limit: RUST_PRODUCER_FAMILY_RECORD_LIMIT,
        record_bytes: 0,
    }
}

/// The frozen `rust-client-core` family table: ten semantic families in
/// contract order with numeric IDs 1..10, every family at major 1 / minor 0,
/// and the accepted per-family record limit. Consumers resolve the symbolic
/// logical names before any feature is instantiated; the numeric IDs are
/// producer-internal values reported by the later `family_table` facade.
pub const RUST_PRODUCER_FAMILIES: [ProducerFamilyDescriptor; 10] = [
    producer_descriptor("session", 1),
    producer_descriptor("input", 2),
    producer_descriptor("terrain", 3),
    producer_descriptor("actors", 4),
    producer_descriptor("player-view", 5),
    producer_descriptor("inventory-ui", 6),
    producer_descriptor("world-ui", 7),
    producer_descriptor("audio-cues", 8),
    producer_descriptor("lifecycle", 9),
    producer_descriptor("diagnostics", 10),
];

/// Why a producer descriptor list is not a valid registry table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProducerRegistryError {
    /// A descriptor carries a logical name outside the producer's keys.
    UnknownFamilyKey { logical_name: &'static str },
    /// A descriptor carries a numeric ID outside the producer's ID space.
    UnknownFamilyId { numeric_id: u16 },
    /// Two descriptors carry the same logical name.
    DuplicateFamilyKey { logical_name: &'static str },
    /// Two descriptors carry the same numeric ID within the producer.
    DuplicateFamilyId { numeric_id: u16 },
    /// A family required by the producer data plane is absent.
    MissingRequiredFamily { logical_name: &'static str },
}

/// Validate a producer-reported descriptor list as a registry table: every
/// descriptor must name a known producer key with a numeric ID inside the
/// producer's ID space, keys and IDs must be unique, and all ten families are
/// required. Unknown identities are rejected before presence checks so a
/// table that both smuggles an unknown family and drops a required one
/// reports the smuggled identity first; key checks run before ID checks.
pub fn validate_producer_registry(
    descriptors: &[ProducerFamilyDescriptor],
) -> Result<(), ProducerRegistryError> {
    let id_space = RUST_PRODUCER_FAMILIES.len() as u16;
    for descriptor in descriptors {
        if !RUST_PRODUCER_FAMILIES
            .iter()
            .any(|known| known.logical_name == descriptor.logical_name)
        {
            return Err(ProducerRegistryError::UnknownFamilyKey {
                logical_name: descriptor.logical_name,
            });
        }
        if descriptor.numeric_id == 0 || descriptor.numeric_id > id_space {
            return Err(ProducerRegistryError::UnknownFamilyId {
                numeric_id: descriptor.numeric_id,
            });
        }
    }
    for known in RUST_PRODUCER_FAMILIES {
        let mut seen = 0;
        for descriptor in descriptors {
            if descriptor.logical_name == known.logical_name {
                seen += 1;
            }
        }
        if seen > 1 {
            return Err(ProducerRegistryError::DuplicateFamilyKey {
                logical_name: known.logical_name,
            });
        }
        if seen == 0 {
            return Err(ProducerRegistryError::MissingRequiredFamily {
                logical_name: known.logical_name,
            });
        }
    }
    for id in 1..=id_space {
        let mut seen = 0;
        for descriptor in descriptors {
            if descriptor.numeric_id == id {
                seen += 1;
            }
        }
        if seen > 1 {
            return Err(ProducerRegistryError::DuplicateFamilyId { numeric_id: id });
        }
    }
    Ok(())
}

/// Decide whether the consumer may resolve `logical_name` at the requested
/// `(major, minor)` against a validated producer table. Symbolic resolution
/// is the consumer-facing identity check: an unregistered logical name, a
/// major mismatch, or a minor newer than the registered family contract
/// version fails closed before any feature is instantiated.
pub fn negotiate_producer(
    descriptors: &[ProducerFamilyDescriptor],
    logical_name: &str,
    major: u16,
    minor: u16,
) -> NegotiationDecision {
    let Some(descriptor) = descriptors
        .iter()
        .find(|descriptor| descriptor.logical_name == logical_name)
    else {
        return NegotiationDecision {
            accepted: false,
            reason: NegotiationReason::UnknownFamily,
        };
    };
    if major != descriptor.major {
        return NegotiationDecision {
            accepted: false,
            reason: NegotiationReason::MajorMismatch,
        };
    }
    if minor > descriptor.minor {
        return NegotiationDecision {
            accepted: false,
            reason: NegotiationReason::MinorTooNew,
        };
    }
    NegotiationDecision {
        accepted: true,
        reason: NegotiationReason::Compatible,
    }
}

#[cfg(test)]
mod tests {
    use super::{NegotiationReason, PILOT_FAMILIES, RegistryError, negotiate, validate_registry};
    use crate::abi::{ABI_MAJOR, FAMILY_COUNT, FamilyDescriptor};

    /// Rebuild the pilot table from the header constants for mutation tests;
    /// unlike the pinned literals, this follows a reviewed header bump.
    fn pilot_table() -> Vec<FamilyDescriptor> {
        PILOT_FAMILIES.to_vec()
    }

    #[test]
    fn feature_negotiation_pilot_families_match_header_constants() {
        // The pinned literals are the no-silent-change gate: any family ID,
        // contract version, or limit that changes in the header or in this
        // mirror fails here until the pin is consciously updated as part of a
        // reviewed contract-version bump.
        let pinned: [(u32, u32, u32, u32); 7] = [
            (1, 1, 8, 24),
            (2, 1, 256, 0),
            (3, 1, 128, 0),
            (4, 1, 1, 24),
            (5, 1, 4096, 0),
            (6, 1, 7, 0),
            (7, 1, 64, 0),
        ];
        assert_eq!(PILOT_FAMILIES.len(), FAMILY_COUNT as usize);
        for (descriptor, (family, version, record_limit, record_bytes)) in
            PILOT_FAMILIES.iter().zip(pinned)
        {
            assert_eq!(
                (
                    descriptor.family,
                    descriptor.version,
                    descriptor.record_limit,
                    descriptor.record_bytes
                ),
                (family, version, record_limit, record_bytes),
                "family {family} descriptor"
            );
            assert_eq!(descriptor.reserved, [0, 0]);
        }
        assert_eq!(validate_registry(&PILOT_FAMILIES), Ok(()));
    }

    #[test]
    fn feature_negotiation_rejects_unknown_family() {
        for unknown in [0, FAMILY_COUNT + 1, 99] {
            let mut table = pilot_table();
            table[0].family = unknown;
            assert_eq!(
                validate_registry(&table),
                Err(RegistryError::UnknownFamily { family: unknown }),
                "table with family {unknown}"
            );
            let decision = negotiate(&PILOT_FAMILIES, unknown, ABI_MAJOR, 1);
            assert!(!decision.accepted);
            assert_eq!(decision.reason, NegotiationReason::UnknownFamily);
        }
    }

    #[test]
    fn feature_negotiation_rejects_duplicate_family_ids() {
        let mut table = pilot_table();
        table.push(table[0]);
        assert_eq!(
            validate_registry(&table),
            Err(RegistryError::DuplicateFamily {
                family: table[0].family
            })
        );
    }

    #[test]
    fn feature_negotiation_rejects_missing_required_family() {
        for dropped in 0..pilot_table().len() {
            let mut table = pilot_table();
            let missing = table[dropped].family;
            table.remove(dropped);
            assert_eq!(
                validate_registry(&table),
                Err(RegistryError::MissingRequiredFamily { family: missing }),
                "table without family {missing}"
            );
        }
    }

    #[test]
    fn feature_negotiation_rejects_zero_contract_version() {
        let mut table = pilot_table();
        table[0].version = 0;
        assert_eq!(
            validate_registry(&table),
            Err(RegistryError::ZeroVersion {
                family: table[0].family
            })
        );
    }

    #[test]
    fn feature_negotiation_accepts_compatible_minor() {
        for descriptor in &PILOT_FAMILIES {
            let mut requested = [descriptor.version, 0];
            if descriptor.version > 1 {
                requested[1] = descriptor.version - 1;
            }
            for minor in requested {
                let decision = negotiate(&PILOT_FAMILIES, descriptor.family, ABI_MAJOR, minor);
                assert!(
                    decision.accepted,
                    "family {} minor {minor}",
                    descriptor.family
                );
                assert_eq!(decision.reason, NegotiationReason::Compatible);
            }
        }
    }

    #[test]
    fn feature_negotiation_rejects_incompatible_major() {
        for descriptor in &PILOT_FAMILIES {
            for major in [ABI_MAJOR + 1, ABI_MAJOR - 1] {
                let decision = negotiate(
                    &PILOT_FAMILIES,
                    descriptor.family,
                    major,
                    descriptor.version,
                );
                assert!(
                    !decision.accepted,
                    "family {} major {major}",
                    descriptor.family
                );
                assert_eq!(decision.reason, NegotiationReason::MajorMismatch);
            }
        }
    }

    #[test]
    fn feature_negotiation_rejects_newer_minor_than_registry() {
        for descriptor in &PILOT_FAMILIES {
            let decision = negotiate(
                &PILOT_FAMILIES,
                descriptor.family,
                ABI_MAJOR,
                descriptor.version + 1,
            );
            assert!(!decision.accepted, "family {}", descriptor.family);
            assert_eq!(decision.reason, NegotiationReason::MinorTooNew);
        }
    }
}
