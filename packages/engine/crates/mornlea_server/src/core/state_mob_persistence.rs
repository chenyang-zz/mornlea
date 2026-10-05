//! Complete source mob startup and settled rosters share the existing actor ledger.
use super::*;
use crate::core::actor_projection::{project_hostile, project_passive};
use mornlea_domain::{HostileId, SurvivalState, SurvivalStateParts};
use mornlea_storage::{
    HostileMobs, HostileMobsSave, PassiveMobs, PassiveMobsSave, hostile_mobs_encoded_len,
    passive_mobs_encoded_len,
};
const INVALID: ServerError = ServerError::InvalidInput {
    field: "mob_startup",
};
const MAX_ACTORS: usize = 110;
const OWNERSHIP: ServerError = ServerError::Internal {
    invariant: "complete mob roster ownership",
};

impl AuthorityState {
    /// Restores both complete families before ticks or sessions; this is not a reload operation.
    pub fn enable_mob_persistence(
        &mut self,
        mut hostiles: HostileMobs,
        mut passives: PassiveMobs,
    ) -> Result<(), ServerError> {
        self.require_live_chunks(true)?;
        if self.actor_saves.is_none()
            || !self.player_persistence_enabled()
            || self.mob_persistence
            || self.next_tick != 0
            || !self.sessions.is_empty()
        {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        if self.residents.actors.len() > 4 {
            return Err(ServerError::Capacity {
                resource: Resource::Actors,
                limit: 4,
                observed: self.residents.actors.len(),
            });
        }
        for actor in &self.residents.actors {
            let ActorKey::Companion(id) = actor.key else {
                return Err(INVALID);
            };
            if !self.source_companions.entries.contains_key(&id) {
                return Err(INVALID);
            }
        }
        if self
            .residents
            .runtimes
            .keys()
            .any(|k| matches!(k, ActorKey::Hostile(_) | ActorKey::Passive(_)))
            || self
                .actor_saves
                .as_ref()
                .unwrap()
                .current(&SaveKey::Hostiles)
                .is_some()
            || self
                .actor_saves
                .as_ref()
                .unwrap()
                .current(&SaveKey::Passives)
                .is_some()
        {
            return Err(INVALID);
        }
        if (hostiles.revision == 0 && !hostiles.records.is_empty())
            || (passives.revision == 0 && !passives.records.is_empty())
        {
            return Err(INVALID);
        }
        hostiles.records.sort_by_key(|m| m.id);
        passives.records.sort_by_key(|m| m.id);
        let hostile_persisted = hostiles.revision;
        let passive_persisted = passives.revision;
        let hostile_save = HostileMobsSave {
            revision: hostile_persisted.max(1),
            records: hostiles.records,
        };
        let passive_save = PassiveMobsSave {
            revision: passive_persisted.max(1),
            records: passives.records,
        };
        hostile_mobs_encoded_len(&hostile_save).map_err(|_| INVALID)?;
        passive_mobs_encoded_len(&passive_save).map_err(|_| INVALID)?;
        let mut actors =
            Vec::with_capacity(hostile_save.records.len() + passive_save.records.len());
        let mut runtimes = BTreeMap::new();
        for body in &hostile_save.records {
            let key = ActorKey::Hostile(HostileId::try_new(body.id).map_err(|_| INVALID)?);
            actors.push(restored_actor(
                key,
                body.position,
                body.velocity,
                body.on_ground,
                body.yaw,
                body.health,
                ActorBody::Hostile(body.clone()),
            )?);
            let mut runtime = neutral_runtime(
                key,
                ActorAux::Hostile {
                    distant_ticks: body.distant_ticks,
                    shoot_cooldown: 0,
                    fresh: false,
                },
            );
            runtime.burn_cooldown = u32::from(body.burn_cooldown);
            runtimes.insert(key, runtime);
        }
        for body in &passive_save.records {
            let key = ActorKey::Passive(PassiveId::try_new(body.id).map_err(|_| INVALID)?);
            actors.push(restored_actor(
                key,
                body.position,
                body.velocity,
                body.on_ground,
                body.yaw,
                body.health,
                ActorBody::Passive(body.clone()),
            )?);
            // Keep the existing provider's saturating conversion for codec-valid finite extremes.
            let home = BlockPos::new(
                body.position[0].floor() as i32,
                body.position[1].floor() as i32,
                body.position[2].floor() as i32,
            );
            runtimes.insert(
                key,
                neutral_runtime(
                    key,
                    ActorAux::Passive {
                        home,
                        flee_ticks: 0,
                        flee_from: None,
                        graze_ticks: 0,
                        graze_at: None,
                        fresh: false,
                    },
                ),
            );
        }
        // All fallible construction and both ledger admissions precede resident transfer.
        self.actor_saves.as_mut().unwrap().retain_mob_pair(
            hostile_save,
            hostile_persisted,
            passive_save,
            passive_persisted,
        )?;
        self.residents.actors.extend(actors);
        self.residents.runtimes.append(&mut runtimes);
        self.mob_persistence = true;
        Ok(())
    }
    /// Identifies the opt-in complete roster owner used by tick admission and capture.
    pub fn mob_persistence_enabled(&self) -> bool {
        self.mob_persistence
    }

    /// Refuses unbounded trusted fixtures before moving any tick-owned state.
    pub(crate) fn preflight_mob_persistence(&self) -> Result<(), ServerError> {
        if !self.mob_persistence {
            return Ok(());
        }
        self.require_live_chunks(false)?;
        if self.residents.actors.len() > MAX_ACTORS {
            return Err(ServerError::Capacity {
                resource: Resource::Actors,
                limit: MAX_ACTORS,
                observed: self.residents.actors.len(),
            });
        }
        let mut keys = BTreeSet::new();
        let (mut players, mut companions, mut hostiles, mut passives) = (0, 0, 0, 0);
        for (slot, actor) in self.residents.actors.iter().enumerate() {
            if !keys.insert(actor.key) {
                return Err(OWNERSHIP);
            }
            match actor.key {
                ActorKey::Player(session) => {
                    players += 1;
                    if self.residents.player_slots.get(&session) != Some(&slot)
                        || !self.source_players.entries.contains_key(&session)
                        || !self.sessions.contains_key(&session)
                    {
                        return Err(OWNERSHIP);
                    }
                }
                ActorKey::Companion(id) => {
                    companions += 1;
                    if !self.source_companions.entries.contains_key(&id) {
                        return Err(OWNERSHIP);
                    }
                }
                ActorKey::Hostile(_) | ActorKey::Passive(_) => {
                    if !matches!(
                        actor.lifecycle,
                        ActorLifecycle::Active | ActorLifecycle::Dead
                    ) {
                        return Err(OWNERSHIP);
                    }
                    if actor.lifecycle == ActorLifecycle::Active {
                        match actor.key {
                            ActorKey::Hostile(_) => hostiles += 1,
                            ActorKey::Passive(_) => passives += 1,
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }
        if players > 8 || companions > 4 || hostiles > 64 || passives > 32 {
            return Err(OWNERSHIP);
        }
        Ok(())
    }

    /// Prepares both complete bodies before observation or physical terminal erasure.
    pub(crate) fn capture_mob_saves(&mut self) -> Result<(), ServerError> {
        if !self.mob_persistence {
            return Ok(());
        }
        self.preflight_mob_persistence()?;
        let mut hostiles = HostileMobsSave {
            revision: 1,
            records: Vec::new(),
        };
        let mut passives = PassiveMobsSave {
            revision: 1,
            records: Vec::new(),
        };
        for actor in &self.residents.actors {
            if !matches!(actor.key, ActorKey::Hostile(_) | ActorKey::Passive(_)) {
                continue;
            }
            let runtime = self.residents.runtimes.get(&actor.key).ok_or(OWNERSHIP)?;
            if runtime.key != actor.key {
                return Err(OWNERSHIP);
            }
            match (actor.key, &actor.body, &runtime.aux) {
                (ActorKey::Hostile(id), ActorBody::Hostile(body), ActorAux::Hostile { .. })
                    if id.get() == body.id =>
                {
                    if actor.lifecycle == ActorLifecycle::Active {
                        hostiles
                            .records
                            .push(project_hostile(actor, Some(runtime))?);
                    }
                }
                (ActorKey::Passive(id), ActorBody::Passive(body), ActorAux::Passive { .. })
                    if id.get() == body.id =>
                {
                    if actor.lifecycle == ActorLifecycle::Active {
                        passives.records.push(project_passive(actor)?);
                    }
                }
                _ => return Err(OWNERSHIP),
            }
        }
        hostiles.records.sort_by_key(|body| body.id);
        passives.records.sort_by_key(|body| body.id);
        hostile_mobs_encoded_len(&hostiles).map_err(|_| OWNERSHIP)?;
        passive_mobs_encoded_len(&passives).map_err(|_| OWNERSHIP)?;
        let ledger = self.actor_saves.as_mut().ok_or(OWNERSHIP)?;
        let hostile_persisted = ledger.current(&SaveKey::Hostiles).ok_or(OWNERSHIP)?.0;
        let passive_persisted = ledger.current(&SaveKey::Passives).ok_or(OWNERSHIP)?.0;
        let hostile_eligible = !hostiles.records.is_empty() || hostile_persisted > 0;
        let passive_eligible = !passives.records.is_empty() || passive_persisted > 0;
        // Both keys and codec-valid canonical values are prepared before either mutation.
        ledger.observe(SaveValue::Hostiles(hostiles), hostile_eligible, false)?;
        ledger.observe(SaveValue::Passives(passives), passive_eligible, false)?;
        let mut slot = 0;
        while slot < self.residents.actors.len() {
            let actor = &self.residents.actors[slot];
            if actor.lifecycle != ActorLifecycle::Dead
                || !matches!(actor.key, ActorKey::Hostile(_) | ActorKey::Passive(_))
            {
                slot += 1;
                continue;
            }
            let key = actor.key;
            self.residents.actors.swap_remove(slot);
            if let Some(moved) = self.residents.actors.get(slot)
                && let ActorKey::Player(session) = moved.key
            {
                self.residents.player_slots.insert(session, slot);
            }
            self.residents.runtimes.remove(&key);
            self.residents.inventories.remove(&key);
            self.residents.mining.remove(&key);
        }
        Ok(())
    }
}

impl TickContext<'_> {
    /// Ordered compound siblings share the same active-family and retained-record ceiling.
    pub(super) fn admit_mob_actor(&self, record: &ActorRecord) -> Result<(), RuleReject> {
        if !self.authority.mob_persistence {
            return Ok(());
        }
        if self.actors.len() > MAX_ACTORS {
            return Err(RuleReject::ResourceFull(Resource::Actors));
        }
        let previous = self.actors.iter().find(|actor| actor.key == record.key);
        if previous.is_none() && self.actors.len() == MAX_ACTORS {
            return Err(RuleReject::ResourceFull(Resource::Actors));
        }
        let limit = match record.key {
            ActorKey::Hostile(_) => 64,
            ActorKey::Passive(_) => 32,
            _ => return Ok(()),
        };
        if !matches!(
            record.lifecycle,
            ActorLifecycle::Active | ActorLifecycle::Dead
        ) {
            return Err(RuleReject::StaleObservation);
        }
        if record.lifecycle == ActorLifecycle::Active
            && !previous.is_some_and(|actor| actor.lifecycle == ActorLifecycle::Active)
        {
            let occupied = self
                .actors
                .iter()
                .filter(|actor| {
                    actor.lifecycle == ActorLifecycle::Active
                        && matches!(
                            (record.key, actor.key),
                            (ActorKey::Hostile(_), ActorKey::Hostile(_))
                                | (ActorKey::Passive(_), ActorKey::Passive(_))
                        )
                })
                .count();
            if occupied >= limit {
                return Err(RuleReject::ResourceFull(Resource::Actors));
            }
        }
        Ok(())
    }
}
#[allow(clippy::too_many_arguments)]
fn restored_actor(
    key: ActorKey,
    position: [f32; 3],
    velocity: [f32; 3],
    on_ground: bool,
    yaw: f32,
    health: u8,
    body: ActorBody,
) -> Result<ActorRecord, ServerError> {
    ActorRecord::try_new(
        key,
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(mornlea_domain::MotionStateParts {
            position: FiniteVec3::try_new(position).map_err(|_| INVALID)?,
            velocity: FiniteVec3::try_new(velocity).map_err(|_| INVALID)?,
            on_ground,
        }),
        LookAngles::try_new(yaw, 0.0).map_err(|_| INVALID)?,
        SurvivalState::try_new(SurvivalStateParts {
            health,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .map_err(|_| INVALID)?,
        body,
    )
}
fn neutral_runtime(key: ActorKey, aux: ActorAux) -> ActorRuntime {
    ActorRuntime {
        key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 0,
        peak_y: 0.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::step::AuthoritativeFinalReducer;
    use mornlea_storage::{HostileMob, PassiveMob};
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
            .enable_mob_persistence(
                HostileMobs {
                    revision: 9,
                    records: vec![HostileMob {
                        id: 1,
                        dimension: 0,
                        position: [8.5, 65.0, 8.5],
                        velocity: [0.0; 3],
                        on_ground: true,
                        yaw: 0.0,
                        health: 20,
                        attack_cooldown: 0,
                        hurt_cooldown: 0,
                        burn_cooldown: 20,
                        has_target: false,
                        player_id: mornlea_storage::PlayerId::from_bytes([0; 16]),
                        next_repath_ticks: 0,
                        distant_ticks: 0,
                        kind: 0,
                    }],
                },
                PassiveMobs {
                    revision: 5,
                    records: vec![PassiveMob {
                        id: 1,
                        dimension: 0,
                        position: [9.5, 65.0, 8.5],
                        velocity: [0.0; 3],
                        on_ground: true,
                        yaw: 0.0,
                        health: 20,
                    }],
                },
            )
            .unwrap();
        state
    }
    #[test]
    fn bounded_terminal_control_accepts_110_and_checks_111_before_census() {
        let mut state = fixture();
        let template = state.residents.actors[0].clone();
        let runtime = state.residents.runtimes[&template.key].clone();
        state.residents.actors.clear();
        state.residents.runtimes.clear();
        for id in 1..=110 {
            let mut actor = template.clone();
            actor.key = ActorKey::Hostile(HostileId::try_new(id).unwrap());
            actor.lifecycle = ActorLifecycle::Dead;
            let ActorBody::Hostile(body) = &mut actor.body else {
                unreachable!()
            };
            body.id = id;
            let mut runtime = runtime.clone();
            runtime.key = actor.key;
            state.residents.runtimes.insert(actor.key, runtime);
            state.residents.actors.push(actor);
        }
        state.preflight_mob_persistence().unwrap();
        // Duplicate identity would fail census if the constant-time size guard were absent.
        state.residents.actors.push(template);
        assert_eq!(
            state.preflight_mob_persistence(),
            Err(ServerError::Capacity {
                resource: Resource::Actors,
                limit: 110,
                observed: 111,
            })
        );
        state.residents.actors.pop();
        state.capture_mob_saves().unwrap();
        assert!(state.residents.actors.is_empty());
        assert!(state.residents.runtimes.is_empty());
    }
    #[test]
    fn invalid_second_runtime_keeps_both_current_values_and_terminal_diagnostics() {
        let mut state = fixture();
        state.residents.actors[0].lifecycle = ActorLifecycle::Dead;
        let passive = state.residents.actors[1].key;
        state.residents.runtimes.get_mut(&passive).unwrap().aux = ActorAux::Hostile {
            distant_ticks: 0,
            shoot_cooldown: 0,
            fresh: false,
        };
        let actors = state.residents.actors.clone();
        let stats = state.save_stats();
        let hostiles = state
            .actor_save_current(&SaveKey::Hostiles)
            .unwrap()
            .1
            .clone();
        let passives = state
            .actor_save_current(&SaveKey::Passives)
            .unwrap()
            .1
            .clone();
        let error = state.capture_mob_saves().unwrap_err();
        assert_eq!(error, OWNERSHIP);
        assert_eq!(state.residents.actors, actors);
        assert_eq!(state.save_stats(), stats);
        assert_eq!(
            state.actor_save_current(&SaveKey::Hostiles).unwrap().1,
            &hostiles
        );
        assert_eq!(
            state.actor_save_current(&SaveKey::Passives).unwrap().1,
            &passives
        );
        state.fail_tick(error);
        assert_eq!(state.advance_tick(TickBudget::full()), Err(error));
        assert_eq!(state.run_final(&mut AuthoritativeFinalReducer), Err(error));
        assert_eq!(state.residents.actors, actors);
        assert_eq!(state.next_tick(), 0);
    }
    #[test]
    fn disabled_context_keeps_legacy_fixture_family_admission() {
        let mut state = fixture();
        state.mob_persistence = false;
        let template = state.residents.actors[0].clone();
        let mut context = TickContext::restage(&mut state, TickBudget::full());
        for id in 2..=65 {
            let mut actor = template.clone();
            actor.key = ActorKey::Hostile(HostileId::try_new(id).unwrap());
            let ActorBody::Hostile(body) = &mut actor.body else {
                unreachable!()
            };
            body.id = id;
            context.stage(RuleEffect::Actor(actor)).unwrap();
        }
        assert_eq!(
            context
                .actors
                .iter()
                .filter(|a| matches!(a.key, ActorKey::Hostile(_)))
                .count(),
            65
        );
    }
}
