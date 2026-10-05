//! Source player lifecycle owns cache leases; the actor ledger exclusively owns bodies and flights.
use super::super::actor_projection::project_player;
use super::*;

const CACHE_PLAYERS: usize = 16;
const CACHE_IDENTITY: ServerError = ServerError::Internal {
    invariant: "source player cache identity",
};
#[derive(Default)]
pub(super) struct PlayerPersistence {
    entries: BTreeMap<PlayerId, CachedPlayer>,
}
struct CachedPlayer {
    pending_session: Option<SessionKey>,
    loading: bool,
    active: bool,
    missing: bool,
    confirmed: bool,
    observed: bool,
}

impl AuthorityState {
    /// Enables the source cache before any login; current bodies remain in the actor ledger.
    pub fn enable_player_persistence(&mut self) -> Result<(), ServerError> {
        self.require_live_chunks(true)?;
        if self.source_player_radius.is_none() || self.actor_saves.is_none() {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        if self.player_persistence.is_some() {
            return Ok(());
        }
        if self.next_tick != 0
            || !self.sessions.is_empty()
            || self.actor_saves.as_ref().unwrap().retained_player_count() != 0
        {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        self.player_persistence = Some(PlayerPersistence::default());
        Ok(())
    }
    /// Signals whether the concrete login driver must use cache-before-load admission.
    pub fn player_persistence_enabled(&self) -> bool {
        self.player_persistence.is_some()
    }

    /// Reserves one cache lease. True installs retained facts; false owns a new load placeholder.
    /// A loading lease cannot be started twice, and the cache bound is checked before disk admission.
    pub fn prepare_player_cache(&mut self, session: SessionKey) -> Result<bool, ServerError> {
        self.require_live_chunks(true)?;
        let record = self
            .sessions
            .get(&session)
            .filter(|r| r.phase == SessionPhase::Prepared)
            .ok_or(ServerError::StaleSession { session })?;
        let player = record.player_id;
        let cache = self
            .player_persistence
            .as_ref()
            .ok_or(ServerError::InvalidState { phase: self.phase })?;
        if let Some(entry) = cache.entries.get(&player) {
            if entry.loading || entry.active || entry.pending_session.is_some() {
                return Err(CACHE_IDENTITY);
            }
            let (persisted, value) = self
                .actor_saves
                .as_ref()
                .unwrap()
                .current(&SaveKey::Player(player))
                .ok_or(CACHE_IDENTITY)?;
            let SaveValue::Player(value) = value else {
                return Err(CACHE_IDENTITY);
            };
            let mut body = value.clone();
            body.revision = persisted.max(1);
            let loaded_current = !(entry.missing && persisted == 0) || entry.observed;
            self.actor_saves
                .as_mut()
                .unwrap()
                .set_pinned(&SaveKey::Player(player), true)?;
            let entry = self
                .player_persistence
                .as_mut()
                .unwrap()
                .entries
                .get_mut(&player)
                .unwrap();
            // A positive ledger receipt proves the formerly missing identity is now stored.
            entry.missing &= persisted == 0;
            entry.confirmed &= entry.missing;
            entry.pending_session = Some(session);
            let record = self.sessions.get_mut(&session).unwrap();
            record.body = Some(body);
            record.loaded_current = loaded_current;
            return Ok(true);
        }
        self.evict_clean_player_cache()?;
        let cache = self.player_persistence.as_mut().unwrap();
        if cache.entries.len() == CACHE_PLAYERS {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: CACHE_PLAYERS,
                observed: CACHE_PLAYERS + 1,
            });
        }
        cache.entries.insert(
            player,
            CachedPlayer {
                pending_session: Some(session),
                loading: true,
                active: false,
                missing: false,
                confirmed: false,
                observed: false,
            },
        );
        Ok(false)
    }
    fn evict_clean_player_cache(&mut self) -> Result<(), ServerError> {
        let candidates: Vec<_> = self
            .player_persistence
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .filter_map(|(id, e)| {
                (!e.active && !e.loading && e.pending_session.is_none()).then_some(*id)
            })
            .collect();
        for player in candidates {
            if self.actor_saves.as_mut().unwrap().release_player(player)? {
                self.player_persistence
                    .as_mut()
                    .unwrap()
                    .entries
                    .remove(&player);
            }
        }
        Ok(())
    }
    pub(super) fn install_player_cache(
        &mut self,
        session: SessionKey,
        loaded: Option<StoredPlayer>,
    ) -> Result<(), ServerError> {
        self.require_live_chunks(true)?;
        let record = self
            .sessions
            .get(&session)
            .filter(|r| r.phase == SessionPhase::Prepared)
            .ok_or(ServerError::StaleSession { session })?;
        let player = record.player_id;
        let name = record.display_name.clone();
        if !self
            .player_persistence
            .as_ref()
            .unwrap()
            .entries
            .get(&player)
            .is_some_and(|e| e.loading && e.pending_session == Some(session))
        {
            return Err(CACHE_IDENTITY);
        }
        let missing = loaded.is_none();
        let (body, persisted, rewrite) = match loaded {
            Some(stored) => {
                if stored.player_id.to_bytes() != player.bytes() {
                    return Err(ServerError::InvalidInput { field: "player_id" });
                }
                let persisted = stored.revision;
                let rewrite = stored.needs_rewrite;
                (save_from_stored(stored)?, persisted, rewrite)
            }
            None => {
                let mut body = canonical_player(player, &name)?;
                body.current = PlayerLocation {
                    dimension: self.metadata.spawn_dimension,
                    position: [
                        self.metadata.spawn_anchor.x as f32 * 16.0 + 0.5,
                        321.0,
                        self.metadata.spawn_anchor.z as f32 * 16.0 + 0.5,
                    ],
                };
                body.health = 0;
                (body, 0, false)
            }
        };
        self.actor_saves.as_mut().unwrap().retain(
            SaveValue::Player(body.clone()),
            persisted,
            rewrite,
            !missing,
            true,
        )?;
        let entry = self
            .player_persistence
            .as_mut()
            .unwrap()
            .entries
            .get_mut(&player)
            .unwrap();
        entry.loading = false;
        entry.missing = missing;
        entry.observed = !missing;
        let record = self.sessions.get_mut(&session).unwrap();
        record.body = Some(body);
        record.loaded_current = !missing;
        Ok(())
    }
    pub(super) fn confirm_player_cache(&mut self, session: SessionKey) -> Result<(), ServerError> {
        if self.player_persistence.is_none() {
            return Ok(());
        }
        let record = self
            .sessions
            .get(&session)
            .ok_or(ServerError::StaleSession { session })?;
        let player = record.player_id;
        if !self
            .player_persistence
            .as_ref()
            .unwrap()
            .entries
            .get(&player)
            .is_some_and(|e| !e.loading && !e.active && e.pending_session == Some(session))
        {
            return Err(CACHE_IDENTITY);
        }
        let (_, value) = self
            .actor_saves
            .as_ref()
            .unwrap()
            .current(&SaveKey::Player(player))
            .ok_or(CACHE_IDENTITY)?;
        let SaveValue::Player(value) = value else {
            return Err(CACHE_IDENTITY);
        };
        let mut confirmed = value.clone();
        confirmed.display_name = record.display_name.clone();
        self.actor_saves
            .as_mut()
            .unwrap()
            .observe(SaveValue::Player(confirmed), true, false)?;
        let entry = self
            .player_persistence
            .as_mut()
            .unwrap()
            .entries
            .get_mut(&player)
            .unwrap();
        entry.confirmed |= entry.missing;
        entry.active = true;
        entry.pending_session = None;
        Ok(())
    }
    fn player_observation(
        &self,
        session: SessionKey,
        inventory: Option<&InventoryRecord>,
    ) -> Result<Option<PlayerSave>, ServerError> {
        let record = self
            .sessions
            .get(&session)
            .filter(|r| r.phase == SessionPhase::Active)
            .ok_or(CACHE_IDENTITY)?;
        let slot = self
            .residents
            .player_slots
            .get(&session)
            .ok_or(CACHE_IDENTITY)?;
        let actor = self
            .residents
            .actors
            .get(*slot)
            .filter(|a| a.key == ActorKey::Player(session))
            .ok_or(CACHE_IDENTITY)?;
        let source = self
            .source_players
            .entries
            .get(&session)
            .ok_or(CACHE_IDENTITY)?;
        if actor.lifecycle != ActorLifecycle::Active && !source.ever_spawned {
            return Ok(None);
        }
        let key = actor.key;
        let mut body = project_player(
            actor,
            inventory.or_else(|| self.residents.inventories.get(&key)),
            self.residents.runtimes.get(&key),
            1,
        )?;
        if body.player_id.to_bytes() != record.player_id.bytes() {
            return Err(CACHE_IDENTITY);
        }
        body.display_name = record.display_name.clone();
        Ok(Some(body))
    }
    /// Captures only the current indexed players after settled ordinary or final execution.
    pub(crate) fn capture_player_saves(&mut self) -> Result<(), ServerError> {
        if self.player_persistence.is_none() {
            return Ok(());
        }
        self.require_live_chunks(false)?;
        let mut observations = Vec::new();
        for &session in self.residents.player_slots.keys() {
            if let Some(body) = self.player_observation(session, None)? {
                observations.push(body);
            }
        }
        for body in observations {
            let player =
                PlayerId::try_from_bytes(body.player_id.to_bytes()).map_err(|_| CACHE_IDENTITY)?;
            self.actor_saves
                .as_mut()
                .unwrap()
                .observe(SaveValue::Player(body), true, false)?;
            self.player_persistence
                .as_mut()
                .unwrap()
                .entries
                .get_mut(&player)
                .ok_or(CACHE_IDENTITY)?
                .observed = true;
        }
        Ok(())
    }
    pub(super) fn retire_player_cache(&mut self, session: SessionKey) -> Result<(), ServerError> {
        if self.player_persistence.is_none() {
            return Ok(());
        }
        self.require_live_chunks(false)?;
        let record = self
            .sessions
            .get(&session)
            .filter(|r| r.phase != SessionPhase::Retired)
            .ok_or(ServerError::StaleSession { session })?;
        let player = record.player_id;
        let active = record.phase == SessionPhase::Active;
        let Some(entry) = self
            .player_persistence
            .as_ref()
            .unwrap()
            .entries
            .get(&player)
        else {
            return Ok(());
        };
        if (!active && entry.pending_session != Some(session)) || (active && !entry.active) {
            return Err(CACHE_IDENTITY);
        }
        if entry.loading {
            self.player_persistence
                .as_mut()
                .unwrap()
                .entries
                .remove(&player);
            return Ok(());
        }
        let key = ActorKey::Player(session);
        let repacked = if active {
            let inventory = self
                .residents
                .inventories
                .get(&key)
                .copied()
                .ok_or(CACHE_IDENTITY)?;
            Some(
                crate::rules::crafting::repack_all(inventory).ok_or(ServerError::Internal {
                    invariant: "source player exit repack",
                })?,
            )
        } else {
            None
        };
        if active && let Some(body) = self.player_observation(session, repacked.as_ref())? {
            self.actor_saves
                .as_mut()
                .unwrap()
                .observe(SaveValue::Player(body), true, true)?;
            self.player_persistence
                .as_mut()
                .unwrap()
                .entries
                .get_mut(&player)
                .unwrap()
                .observed = true;
        }
        self.actor_saves
            .as_mut()
            .unwrap()
            .set_pinned(&SaveKey::Player(player), false)?;
        let entry = self
            .player_persistence
            .as_mut()
            .unwrap()
            .entries
            .get_mut(&player)
            .unwrap();
        entry.active = false;
        entry.pending_session = None;
        if self.actor_saves.as_mut().unwrap().release_player(player)? {
            self.player_persistence
                .as_mut()
                .unwrap()
                .entries
                .remove(&player);
        }
        if active {
            // Providers order by identity; only player slots retain physical vector positions.
            let slot = *self
                .residents
                .player_slots
                .get(&session)
                .ok_or(CACHE_IDENTITY)?;
            self.residents.actors.swap_remove(slot);
            if let Some(moved) = self.residents.actors.get(slot)
                && let ActorKey::Player(other) = moved.key
            {
                self.residents.player_slots.insert(other, slot);
            }
            self.residents.inventories.remove(&key);
            self.residents.runtimes.remove(&key);
            self.residents.mining.remove(&key);
            self.views.remove(&session);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::step::AuthoritativeFinalReducer;
    use mornlea_protocol::{LoginStart, admit_login};

    fn fixture() -> AuthorityState {
        let mut state = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap();
        state.enable_source_player_restoration(1).unwrap();
        state.enable_live_chunks().unwrap();
        state.enable_actor_saves().unwrap();
        state.enable_player_persistence().unwrap();
        state
    }
    fn add(state: &mut AuthorityState, tag: u8) -> (PlayerId, SessionKey) {
        let mut bytes = [0; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let player = PlayerId::try_from_bytes(bytes).unwrap();
        let login = admit_login(
            LoginStart::decode_inbound(
                &LoginStart::new(player, "Ada", 8).unwrap().encode().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let session = state.prepare(login, TransportKind::Memory).unwrap();
        assert!(!state.prepare_player_cache(session).unwrap());
        state.install(session, None).unwrap();
        state.activate(session).unwrap();
        (player, session)
    }
    fn body(state: &AuthorityState, player: PlayerId) -> &PlayerSave {
        let (_, SaveValue::Player(body)) =
            state.actor_save_current(&SaveKey::Player(player)).unwrap()
        else {
            panic!("player body")
        };
        body
    }
    // The source lifecycle bit is a prepared control, not an executed death producer.
    #[test]
    fn post_death_pending_final_capture_keeps_empty_inventory_and_current_pose() {
        let mut state = fixture();
        let (player, session) = add(&mut state, 1);
        let mut previous = body(&state, player).clone();
        previous.inventory.hotbar.slots[0] = mornlea_storage::ItemStack {
            item: 1,
            count: 7,
            durability: 0,
        };
        state
            .observe_actor_save(SaveValue::Player(previous), true, false)
            .unwrap();
        state
            .source_players
            .entries
            .get_mut(&session)
            .unwrap()
            .ever_spawned = true;
        let slot = state.residents.player_slots[&session];
        let actor = &mut state.residents.actors[slot];
        actor.motion = mornlea_domain::MotionState::new(mornlea_domain::MotionStateParts {
            position: mornlea_domain::FiniteVec3::try_new([9.5, 65.0, 8.5]).unwrap(),
            velocity: mornlea_domain::FiniteVec3::try_new([0.; 3]).unwrap(),
            on_ground: false,
        });
        assert_eq!(actor.lifecycle, ActorLifecycle::Pending);
        state.begin_close();
        assert_eq!(state.run_final(&mut AuthoritativeFinalReducer).unwrap(), 0);
        assert_eq!(
            body(&state, player).inventory,
            mornlea_storage::Inventory::default()
        );
        assert_eq!(body(&state, player).current.position, [9.5, 65.0, 8.5]);
        assert_eq!(state.select(SaveMode::All, SaveBudget::default()).len(), 1);
        assert!(
            state
                .observe_actor_save(SaveValue::Player(body(&state, player).clone()), true, false)
                .is_err()
        );
    }
    #[test]
    fn retirement_repairs_moved_player_slot_and_ordinary_capture_reads_current_owner() {
        let mut state = fixture();
        let (_, first) = add(&mut state, 1);
        let (player, second) = add(&mut state, 2);
        assert_eq!(state.residents.player_slots[&second], 1);
        state.retire(first, CloseReason::PeerGone).unwrap();
        assert_eq!(state.residents.player_slots[&second], 0);
        assert_eq!(state.residents.actors.len(), 1);
        state
            .source_players
            .entries
            .get_mut(&second)
            .unwrap()
            .ever_spawned = true;
        state
            .residents
            .inventories
            .get_mut(&ActorKey::Player(second))
            .unwrap()
            .selected = mornlea_domain::HotbarSlot::new(6).unwrap();
        state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(body(&state, player).inventory.hotbar.selected, 6);
        assert_eq!(state.residents.actors[0].key, ActorKey::Player(second));
    }
}
