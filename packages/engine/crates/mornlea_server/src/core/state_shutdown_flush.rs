//! Single-key save selection for the shutdown flush barrier.
//!
//! The shutdown machine flushes one save family at a time; the store asks
//! the authority for exactly that key's pending snapshot and waits for it to
//! become durable before the next family starts. Selection keeps the same
//! ownership rules as the autosave `select`: a chosen snapshot moves to its
//! in-flight owner, and its completion or `return_dirty` settles it.
use super::*;

impl AuthorityState {
    /// Selects the pending snapshot of exactly one save key.
    ///
    /// A failed or closed authority selects nothing, like `select`. Metadata
    /// is never selected here: its frozen target comes from
    /// `try_metadata_snapshot`. A key with nothing dirty, or whose chunk
    /// flight capacity is momentarily full, selects nothing; the caller
    /// keeps waiting while [`Self::has_pending_save`] reports work.
    pub fn select_family(&mut self, family: &SaveKey) -> Vec<OwnedSnapshot> {
        if self.tick_failure.is_some() || self.phase == ServerPhase::Closed {
            return Vec::new();
        }
        if !self.acquisition.enabled() {
            let (chosen, rest): (Vec<_>, Vec<_>) = self
                .dirty
                .drain(..)
                .partition(|snapshot| snapshot.key == *family);
            self.dirty = rest;
            self.in_flight.extend(chosen.iter().cloned());
            return chosen;
        }
        match family {
            SaveKey::Chunk(key) => self.select_live_chunk(*key),
            SaveKey::Metadata => Vec::new(),
            SaveKey::Player(_) | SaveKey::Companions | SaveKey::Hostiles | SaveKey::Passives => {
                let Some(ledger) = self.actor_saves.as_mut() else {
                    return Vec::new();
                };
                match ledger.select_key(family, SaveMode::All) {
                    Ok(selected) => selected,
                    Err(error) => {
                        self.fail_tick(error);
                        Vec::new()
                    }
                }
            }
        }
    }

    /// One live chunk's target under the same flight cap and identity
    /// checks as the autosave chunk selection.
    fn select_live_chunk(&mut self, key: ChunkKey) -> Vec<OwnedSnapshot> {
        if self.acquisition.stats().in_flight >= 8 {
            return Vec::new();
        }
        let Some((_, urgency)) = self.acquisition.save_target(key) else {
            return Vec::new();
        };
        let Some(snapshot) = self.capture_chunk_snapshot(key, urgency) else {
            self.acquisition.save_error(
                key,
                ServerError::Internal {
                    invariant: "chunk payload estimate",
                },
            );
            return Vec::new();
        };
        if let Err(error) = self.acquisition.selected(snapshot.clone()) {
            self.acquisition.save_error(key, error);
            return Vec::new();
        }
        vec![snapshot]
    }

    /// Whether one save key still has dirty or in-flight work: the same key
    /// set the frozen authority reports.
    pub fn has_pending_save(&self, key: &SaveKey) -> bool {
        self.pending_save_keys().contains(key)
    }

    /// Unique dirty and in-flight save keys across live chunks, the actor
    /// ledger and the fixture lists, in that order.
    pub(super) fn pending_save_keys(&self) -> Vec<SaveKey> {
        let mut save_keys = if self.acquisition.enabled() {
            self.acquisition.save_keys()
        } else {
            Vec::new()
        };
        // Closing preserves ledger selection until the final flush consumes it.
        if let Some(ledger) = &self.actor_saves {
            for key in ledger.save_keys() {
                if !save_keys.contains(&key) {
                    save_keys.push(key);
                }
            }
        }
        for snapshot in self.dirty.iter().chain(self.in_flight.iter()) {
            if !save_keys.contains(&snapshot.key) {
                save_keys.push(snapshot.key.clone());
            }
        }
        save_keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_storage::{HostileMobsSave, PassiveMobsSave};

    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }

    fn snapshot(key: SaveKey) -> OwnedSnapshot {
        let value = match key {
            SaveKey::Hostiles => SaveValue::Hostiles(HostileMobsSave {
                revision: 2,
                records: vec![],
            }),
            _ => SaveValue::Passives(PassiveMobsSave {
                revision: 2,
                records: vec![],
            }),
        };
        OwnedSnapshot::try_new(key, 2, 64, SaveUrgency::Autosave, value).unwrap()
    }

    #[test]
    fn select_family_takes_only_the_requested_key() {
        let mut a = authority();
        a.dirty.push(snapshot(SaveKey::Hostiles));
        a.dirty.push(snapshot(SaveKey::Passives));
        assert!(a.select_family(&SaveKey::Companions).is_empty());
        let chosen = a.select_family(&SaveKey::Passives);
        assert_eq!(chosen, vec![snapshot(SaveKey::Passives)]);
        assert_eq!(a.dirty, vec![snapshot(SaveKey::Hostiles)]);
        assert_eq!(a.in_flight, vec![snapshot(SaveKey::Passives)]);
        // In flight is still pending until its completion settles it.
        assert!(a.has_pending_save(&SaveKey::Passives));
        assert!(a.has_pending_save(&SaveKey::Hostiles));
        assert!(!a.has_pending_save(&SaveKey::Companions));
        assert!(a.select_family(&SaveKey::Passives).is_empty());
        assert!(a.select_family(&SaveKey::Metadata).is_empty());
        assert_eq!(a.freeze().save_keys, a.pending_save_keys());
    }

    #[test]
    fn select_family_refuses_a_closed_or_failed_authority() {
        let mut closed = authority();
        closed.dirty.push(snapshot(SaveKey::Hostiles));
        closed.mark_closed();
        assert!(closed.select_family(&SaveKey::Hostiles).is_empty());
        let mut failed = authority();
        failed.dirty.push(snapshot(SaveKey::Hostiles));
        let _ = failed.fail_tick(ServerError::Internal { invariant: "test" });
        assert!(failed.select_family(&SaveKey::Hostiles).is_empty());
        assert_eq!(failed.dirty.len(), 1);
    }

    #[test]
    fn shutdown_flush_core_names_no_store_type() {
        // The shutdown flush reaches the store only through core ports: the
        // phase machine, its port contracts, family selection and the actor
        // ledger never name a store item. Older core files that already do
        // are outside this boundary and are not widened by it.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core");
        let mut offenders = Vec::new();
        for file in [
            "shutdown.rs",
            "contracts.rs",
            "state_shutdown_flush.rs",
            "actor_save.rs",
        ] {
            let text = std::fs::read_to_string(root.join(file)).unwrap();
            for (index, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.starts_with("//") || line.contains("\"crate::") {
                    continue;
                }
                if line.contains("crate::store") || line.contains("super::super::store") {
                    offenders.push(format!("{file}:{}: {line}", index + 1));
                }
            }
        }
        assert!(offenders.is_empty(), "{}", offenders.join("\n"));
    }
}
