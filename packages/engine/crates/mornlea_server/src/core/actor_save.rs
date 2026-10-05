//! Bounded latest actor values and immutable selected save targets.
//!
//! Eligibility and pinning belong to source lifecycle callers. This owner
//! performs no I/O, captures no actors and allocates no tickets; the scheduler
//! retains retry timing while exact selected targets stay charged here.

use std::collections::BTreeMap;

use mornlea_domain::PlayerId;
use mornlea_storage::{
    companions_encoded_len, hostile_mobs_encoded_len, passive_mobs_encoded_len, player_encoded_len,
};

use super::contracts::{
    OwnedSnapshot, Resource, SaveBudget, SaveKey, SaveMode, SaveStats, SaveUrgency, SaveValue,
    ServerError, ServerPhase,
};

const PLAYER_KEYS: usize = 16;
const INVALID: ServerError = ServerError::InvalidInput {
    field: "actor_save",
};
const IDENTITY: ServerError = ServerError::Internal {
    invariant: "actor save identity",
};

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum Key {
    Player([u8; 16]),
    Companions,
    Hostiles,
    Passives,
}

impl Key {
    fn from_save(key: &SaveKey) -> Option<Self> {
        match key {
            SaveKey::Player(id) => Some(Self::Player(id.bytes())),
            SaveKey::Companions => Some(Self::Companions),
            SaveKey::Hostiles => Some(Self::Hostiles),
            SaveKey::Passives => Some(Self::Passives),
            _ => None,
        }
    }

    fn save(self) -> SaveKey {
        match self {
            Self::Player(bytes) => {
                SaveKey::Player(PlayerId::try_from_bytes(bytes).expect("validated actor save key"))
            }
            Self::Companions => SaveKey::Companions,
            Self::Hostiles => SaveKey::Hostiles,
            Self::Passives => SaveKey::Passives,
        }
    }
}

struct Entry {
    current: SaveValue,
    estimate: usize,
    persisted: u64,
    dirty: bool,
    eligible: bool,
    pinned: bool,
    force: bool,
    flight: Option<OwnedSnapshot>,
    /// A distinct latest target is charged beside the selected immutable owner.
    different_from_flight: bool,
}

/// Retains at most sixteen player keys and the three actor aggregate keys.
///
/// Each key has one latest value and at most one immutable flight. Schema
/// validation bounds their bodies; mailbox and worker copies have separate
/// limits. A passing ledger recipe does not qualify automatic capture or disk.
#[derive(Default)]
pub struct ActorSaveLedger {
    entries: BTreeMap<Key, Entry>,
    frozen: bool,
}

impl ActorSaveLedger {
    /// Retains a loaded value or an explicitly unsaved initial value.
    ///
    /// Positive durable revisions must agree with the value. An unsaved value
    /// uses envelope one and persisted zero. Only the caller decides whether
    /// a missing player has been confirmed and whether a prepared/live entry
    /// is pinned. Eligible unsaved admission requires explicit initial dirty
    /// intent; quiet absent baselines remain ineligible until source promotion.
    /// Invalid admission never evicts existing ownership.
    pub fn retain(
        &mut self,
        value: SaveValue,
        persisted_revision: u64,
        needs_rewrite: bool,
        eligible: bool,
        pinned: bool,
    ) -> Result<(), ServerError> {
        self.check_open()?;
        let (key, envelope, value, estimate) = canonical(value)?;
        if envelope != persisted_revision.max(1)
            || (persisted_revision == 0 && eligible && !needs_rewrite)
            || self.entries.contains_key(&key)
        {
            return Err(INVALID);
        }
        if matches!(key, Key::Player(_))
            && self
                .entries
                .keys()
                .filter(|key| matches!(key, Key::Player(_)))
                .count()
                == PLAYER_KEYS
        {
            let evictable: Vec<_> = self
                .entries
                .iter()
                .filter_map(|(key, entry)| {
                    (matches!(key, Key::Player(_))
                        && !entry.pinned
                        && !entry.dirty
                        && entry.flight.is_none())
                    .then_some(*key)
                })
                .collect();
            if evictable.is_empty() {
                return Err(ServerError::Capacity {
                    resource: Resource::Players,
                    limit: PLAYER_KEYS,
                    observed: PLAYER_KEYS + 1,
                });
            }
            for key in evictable {
                self.entries.remove(&key);
            }
        }
        self.entries.insert(
            key,
            Entry {
                current: value,
                estimate,
                persisted: persisted_revision,
                dirty: needs_rewrite,
                eligible,
                pinned,
                force: false,
                flight: None,
                different_from_flight: false,
            },
        );
        Ok(())
    }

    /// Replaces only the latest current value, never a selected retry target.
    ///
    /// Eligibility promotes monotonically; an unchanged first confirmed
    /// value still becomes dirty. Force during a flight belongs to the next
    /// target. The result describes content change rather than dirty state.
    pub fn observe(
        &mut self,
        value: SaveValue,
        persistable: bool,
        force: bool,
    ) -> Result<bool, ServerError> {
        self.check_open()?;
        let (key, _, value, estimate) = canonical(value)?;
        let entry = self.entries.get_mut(&key).ok_or(INVALID)?;
        let changed = entry.current != value;
        let promoted = persistable && !entry.eligible;
        entry.eligible |= persistable;
        if changed {
            entry.different_from_flight = entry.flight.as_ref().is_some_and(|flight| {
                let mut comparable = flight.value.clone();
                stamp(&mut comparable, 1);
                comparable != value
            });
            entry.current = value;
            entry.estimate = estimate;
        }
        if entry.eligible {
            entry.dirty |= changed || promoted;
            entry.force |= force;
        }
        Ok(changed)
    }

    /// Pins caller-owned prepared/live/cache obligations independently of dirty state.
    pub fn set_pinned(&mut self, key: &SaveKey, pinned: bool) -> Result<(), ServerError> {
        self.check_open()?;
        let key = Key::from_save(key).ok_or(INVALID)?;
        self.entries.get_mut(&key).ok_or(INVALID)?.pinned = pinned;
        Ok(())
    }

    /// Borrows the normalized current body and its separate durable revision.
    /// The body's envelope one is comparison state, never a disk receipt.
    pub fn current(&self, key: &SaveKey) -> Option<(u64, &SaveValue)> {
        self.entries
            .get(&Key::from_save(key)?)
            .map(|entry| (entry.persisted, &entry.current))
    }

    /// Captures a whole ordered prefix before changing any selected ownership.
    ///
    /// The first whole target may exceed the count/byte budget. Subsequent
    /// targets must fit; checked revision overflow leaves all flights intact.
    /// Callers transfer these targets to the existing immutable mailbox.
    pub fn select(
        &mut self,
        mode: SaveMode,
        budget: SaveBudget,
    ) -> Result<Vec<OwnedSnapshot>, ServerError> {
        self.check_open()?;
        let mut selected = Vec::new();
        let mut bytes = 0usize;
        for (key, entry) in &self.entries {
            if !entry.eligible
                || !entry.dirty
                || entry.flight.is_some()
                || (mode == SaveMode::Urgent && !entry.force)
            {
                continue;
            }
            if !selected.is_empty()
                && (selected.len() >= budget.chunks
                    || bytes.saturating_add(entry.estimate) > budget.estimated_bytes)
            {
                break;
            }
            let revision = entry
                .persisted
                .checked_add(1)
                .ok_or(ServerError::Internal {
                    invariant: "actor save revision space",
                })?;
            let mut value = entry.current.clone();
            stamp(&mut value, revision);
            let snapshot = OwnedSnapshot::try_new(
                key.save(),
                revision,
                entry.estimate,
                if entry.force {
                    SaveUrgency::Unload
                } else {
                    SaveUrgency::Autosave
                },
                value,
            )?;
            bytes = bytes.saturating_add(entry.estimate);
            selected.push(snapshot);
        }
        // All fallible preparation precedes ownership changes, avoiding orphan
        // flights if a later key exhausts revision space.
        for snapshot in &selected {
            let entry = self
                .entries
                .get_mut(&Key::from_save(&snapshot.key).expect("actor target key"))
                .expect("selected actor entry");
            entry.flight = Some(snapshot.clone());
            entry.different_from_flight = false;
            entry.force = false;
        }
        Ok(selected)
    }

    /// Reports whether an exact key/revision currently has a selected owner.
    pub fn has_target(&self, key: &SaveKey, revision: u64) -> bool {
        Key::from_save(key)
            .and_then(|key| self.entries.get(&key))
            .and_then(|entry| entry.flight.as_ref())
            .is_some_and(|flight| flight.revision == revision)
    }

    /// Matches body, estimate and urgency as well as key/revision. Immutable
    /// identity uses float bits, independently of source semantic equality.
    pub fn matches(&self, snapshot: &OwnedSnapshot) -> bool {
        Key::from_save(&snapshot.key)
            .and_then(|key| self.entries.get(&key))
            .and_then(|entry| entry.flight.as_ref())
            .is_some_and(|flight| {
                flight.key == snapshot.key
                    && flight.revision == snapshot.revision
                    && flight.estimated_bytes == snapshot.estimated_bytes
                    && flight.urgency == snapshot.urgency
                    && exact_value(&flight.value, &snapshot.value)
            })
    }

    /// Validates an echoed selected preimage before any cohort ACK mutation.
    /// Stale revisions are ignored; an altered current target is a hard error.
    pub fn validate_saved(&self, snapshot: &OwnedSnapshot) -> Result<(), ServerError> {
        if self.has_target(&snapshot.key, snapshot.revision) && !self.matches(snapshot) {
            return Err(IDENTITY);
        }
        Ok(())
    }

    /// Acknowledges only the exact immutable target, retaining any later current.
    /// Failed writes do not call this operation: their original flight remains
    /// charged until the scheduler's same-target retry succeeds.
    pub fn saved(&mut self, snapshot: &OwnedSnapshot) -> Result<bool, ServerError> {
        self.validate_saved(snapshot)?;
        if !self.matches(snapshot) {
            return Ok(false);
        }
        let entry = self
            .entries
            .get_mut(&Key::from_save(&snapshot.key).expect("matched actor key"))
            .expect("matched actor entry");
        entry.persisted = snapshot.revision;
        entry.flight = None;
        // Codec-valid absent respawn fields may contain NaN. The immutable
        // target remains reflexive, while source IEEE comparison keeps such
        // current content semantically dirty even after its exact ACK.
        let mut comparable = snapshot.value.clone();
        stamp(&mut comparable, 1);
        entry.dirty = entry.current != comparable;
        entry.different_from_flight = false;
        if !entry.dirty {
            entry.force = false;
        }
        Ok(true)
    }

    /// Returns an exactly refused target to selection, restoring consumed force.
    /// Admitted write failures instead retain the flight for immutable retry.
    pub fn return_dirty(&mut self, snapshot: &OwnedSnapshot) -> bool {
        if !self.matches(snapshot) {
            return false;
        }
        let entry = self
            .entries
            .get_mut(&Key::from_save(&snapshot.key).expect("matched actor key"))
            .expect("matched actor entry");
        entry.flight = None;
        entry.different_from_flight = false;
        entry.dirty = true;
        entry.force |= snapshot.urgency == SaveUrgency::Unload;
        true
    }

    /// Counts unsaved current targets beside distinct selected targets without
    /// scanning bodies or cloning payloads. These are ledger-local estimates.
    pub fn stats(&self) -> SaveStats {
        let mut stats = SaveStats::default();
        for entry in self.entries.values() {
            if entry.eligible
                && entry.dirty
                && (entry.flight.is_none() || entry.different_from_flight)
            {
                stats.dirty += 1;
                stats.estimated_unsaved_bytes =
                    stats.estimated_unsaved_bytes.saturating_add(entry.estimate);
            }
            if let Some(flight) = &entry.flight {
                stats.in_flight += 1;
                stats.estimated_unsaved_bytes = stats
                    .estimated_unsaved_bytes
                    .saturating_add(flight.estimated_bytes);
            }
        }
        stats
    }

    /// Enumerates unique outstanding actor keys for an off-tick freeze report.
    pub fn save_keys(&self) -> Vec<SaveKey> {
        self.entries
            .iter()
            .filter(|(_, entry)| (entry.eligible && entry.dirty) || entry.flight.is_some())
            .map(|(key, _)| key.save())
            .collect()
    }

    /// Fences new captures while leaving existing acknowledgment ownership usable.
    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    fn check_open(&self) -> Result<(), ServerError> {
        if self.frozen {
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing,
            })
        } else {
            Ok(())
        }
    }
}

// Validate before canonical sorting or mutation. Envelope normalization affects
// semantic comparison alone; no source field or queue/plan order is discarded.
fn canonical(mut value: SaveValue) -> Result<(Key, u64, SaveValue, usize), ServerError> {
    let (key, revision, estimate) = match &value {
        SaveValue::Player(player) => (
            Key::Player(
                PlayerId::try_from_bytes(player.player_id.to_bytes())
                    .map_err(|_| INVALID)?
                    .bytes(),
            ),
            player.revision,
            player_encoded_len(player).map_err(|_| INVALID)?,
        ),
        SaveValue::Companions(value) => (
            Key::Companions,
            value.revision,
            companions_encoded_len(value).map_err(|_| INVALID)?,
        ),
        SaveValue::Hostiles(value) => (
            Key::Hostiles,
            value.revision,
            hostile_mobs_encoded_len(value).map_err(|_| INVALID)?,
        ),
        SaveValue::Passives(value) => (
            Key::Passives,
            value.revision,
            passive_mobs_encoded_len(value).map_err(|_| INVALID)?,
        ),
        _ => return Err(INVALID),
    };
    match &mut value {
        SaveValue::Companions(value) => {
            value.records.sort_by_key(|record| record.id.to_bytes());
            value.lifecycles.sort_by_key(|record| record.id.to_bytes());
            value.queues.sort_by_key(|record| record.id.to_bytes());
        }
        SaveValue::Hostiles(value) => value.records.sort_by_key(|record| record.id),
        SaveValue::Passives(value) => value.records.sort_by_key(|record| record.id),
        _ => {}
    }
    stamp(&mut value, 1);
    Ok((key, revision, value, estimate))
}

fn stamp(value: &mut SaveValue, revision: u64) {
    match value {
        SaveValue::Player(value) => value.revision = revision,
        SaveValue::Companions(value) => value.revision = revision,
        SaveValue::Hostiles(value) => value.revision = revision,
        SaveValue::Passives(value) => value.revision = revision,
        _ => unreachable!("validated actor save value"),
    }
}

// Derived PartialEq is the source semantic comparator, not immutable identity:
// signed zero differs in encoded bodies, and valid absent respawn NaN is nonreflexive.
fn exact_value(left: &SaveValue, right: &SaveValue) -> bool {
    match (left, right) {
        (SaveValue::Player(a), SaveValue::Player(b)) => {
            a.player_id == b.player_id
                && a.revision == b.revision
                && a.display_name == b.display_name
                && exact_location(&a.current, &b.current)
                && a.yaw.to_bits() == b.yaw.to_bits()
                && a.pitch.to_bits() == b.pitch.to_bits()
                && match (&a.safe, &b.safe) {
                    (Some(a), Some(b)) => exact_location(a, b),
                    (None, None) => true,
                    _ => false,
                }
                && a.inventory == b.inventory
                && a.health == b.health
                && a.hunger == b.hunger
                && a.saturation_milli == b.saturation_milli
                && a.exhaustion_milli == b.exhaustion_milli
                && a.respawn_present == b.respawn_present
                && exact_vector(&a.respawn_position, &b.respawn_position)
                && a.respawn_dimension == b.respawn_dimension
                && a.armor == b.armor
        }
        (SaveValue::Companions(a), SaveValue::Companions(b)) => {
            a == b
                && a.records.iter().zip(&b.records).all(|(a, b)| {
                    exact_vector(&a.position, &b.position)
                        && a.yaw.to_bits() == b.yaw.to_bits()
                        && a.pitch.to_bits() == b.pitch.to_bits()
                })
        }
        (SaveValue::Hostiles(a), SaveValue::Hostiles(b)) => {
            a == b
                && a.records.iter().zip(&b.records).all(|(a, b)| {
                    exact_vector(&a.position, &b.position)
                        && exact_vector(&a.velocity, &b.velocity)
                        && a.yaw.to_bits() == b.yaw.to_bits()
                })
        }
        (SaveValue::Passives(a), SaveValue::Passives(b)) => {
            a == b
                && a.records.iter().zip(&b.records).all(|(a, b)| {
                    exact_vector(&a.position, &b.position)
                        && exact_vector(&a.velocity, &b.velocity)
                        && a.yaw.to_bits() == b.yaw.to_bits()
                })
        }
        _ => false,
    }
}

fn exact_location(
    a: &mornlea_storage::PlayerLocation,
    b: &mornlea_storage::PlayerLocation,
) -> bool {
    a.dimension == b.dimension && exact_vector(&a.position, &b.position)
}

fn exact_vector<const N: usize>(a: &[f32; N], b: &[f32; N]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits())
}
