//! Fixed-field reset mapping for future serial recovery and death consumers.

use super::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, ServerError,
};
use super::pending_restore::PendingRestore;
use mornlea_domain::{
    FiniteVec3, MotionState, MotionStateParts, SurvivalState, SurvivalStateParts,
};

/// Maps a prepared Active player pair in place; callers own restart and transient cleanup.
/// All checked construction precedes mutation. Body, path and live aux ownership stay
/// untouched, so work and allocation do not depend on public String or Vec capacities.
#[allow(dead_code)] // Retained until the separately qualified serial recovery consumer lands.
pub(crate) fn begin_reset(
    actor: &mut ActorRecord,
    runtime: &mut ActorRuntime,
    restore: &PendingRestore,
) -> Result<(), ServerError> {
    if !matches!(actor.key, ActorKey::Player(_))
        || !matches!(actor.body, ActorBody::Player(_))
        || runtime.key != actor.key
        || !matches!(runtime.aux, ActorAux::Player { .. })
        || actor.lifecycle != ActorLifecycle::Active
    {
        return Err(ServerError::InvalidInput {
            field: "source_player_reset",
        });
    }
    let anchor = restore.player_reset_anchor()?;
    let position = FiniteVec3::try_new([
        (anchor.x() as f32) * 16.0 + 0.5,
        321.0,
        (anchor.z() as f32) * 16.0 + 0.5,
    ])
    .map_err(|_| ServerError::Internal {
        invariant: "source player reset",
    })?;
    let velocity = FiniteVec3::try_new([0.0; 3]).map_err(|_| ServerError::Internal {
        invariant: "source player reset",
    })?;
    let survival = SurvivalState::try_new(SurvivalStateParts {
        health: actor.survival.health(),
        oxygen: 300,
        hunger: actor.survival.hunger(),
        saturation_zero: actor.survival.saturation_zero(),
        armor_points: actor.survival.armor_points(),
    })
    .map_err(|_| ServerError::Internal {
        invariant: "source player reset",
    })?;
    let motion = MotionState::new(MotionStateParts {
        position,
        velocity,
        on_ground: false,
    });

    actor.lifecycle = ActorLifecycle::Pending;
    actor.motion = motion;
    actor.survival = survival;
    runtime.controls = None;
    runtime.reset = false;
    runtime.attack_cooldown = 0;
    runtime.hurt_cooldown = 0;
    runtime.oxygen = 300;
    runtime.peak_y = 321.0;
    runtime.drown_ticks = 0;
    runtime.eating = None;
    runtime.bow = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::actor_placement::{PlacementWorld, RestoreCandidate};
    use crate::core::contracts::*;
    use crate::core::login_seed::seed_player;
    use crate::core::pending_restore::{RestoreActivation, RestoreKind, RestoreProgress};
    use mornlea_domain::{
        BlockPos, ChunkPos, Dimension, FiniteVec3, HeldActions, HotbarSlot, LookAngles,
        MotionState, MotionStateParts, Movement, PassiveId, PlayerControl, PlayerControlParts,
        SurvivalState, SurvivalStateParts,
    };
    use mornlea_storage::{Inventory, ItemStack, PassiveMob, PlayerId, PlayerLocation, PlayerSave};

    // This AIR/Ready fixture qualifies the prepared-owner consumer, not a live world.
    struct AirReady;
    impl PlacementWorld for AirReady {
        fn ready_revision(&self, key: ChunkKey) -> Option<u64> {
            (key == ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, 0),
            })
            .then_some(9)
        }
        fn block_at(&self, dimension: Dimension, position: BlockPos) -> Option<u16> {
            self.ready_revision(ChunkKey {
                dimension,
                pos: ChunkPos::new(position.x() >> 4, position.z() >> 4),
            })?;
            Some(0)
        }
    }
    fn scan(kind: RestoreKind, anchor: ChunkPos, radius: u8, complete: bool) -> PendingRestore {
        let mut restore = PendingRestore::try_new(
            kind,
            Dimension::DEPTHS,
            anchor,
            radius,
            vec![RestoreCandidate {
                dimension: Dimension::OVERWORLD,
                position: [8.5, 65.0, 8.5],
                require_support: false,
            }],
        )
        .unwrap();
        if complete {
            assert_eq!(
                restore.advance(&AirReady, 1.62),
                Ok(RestoreProgress::Activated(RestoreActivation {
                    dimension: Dimension::OVERWORLD,
                    position: [8.5, 65.0, 8.5],
                    on_ground: false,
                }))
            );
        }
        restore
    }
    fn pair() -> (ActorRecord, ActorRuntime, InventoryRecord) {
        let mut saved_inventory = Inventory::default();
        saved_inventory.hotbar.selected = 2;
        saved_inventory.hotbar.slots[2] = ItemStack {
            item: 5,
            count: 7,
            durability: 0,
        };
        let save = PlayerSave {
            player_id: PlayerId::from_bytes([1; 16]),
            revision: 9,
            display_name: "Ada".into(),
            current: PlayerLocation {
                dimension: 1,
                position: [100.0, 64.0, 100.0],
            },
            safe: Some(PlayerLocation {
                dimension: 0,
                position: [3.0, 70.0, 4.0],
            }),
            yaw: 0.1,
            pitch: 0.2,
            inventory: saved_inventory,
            health: 7,
            hunger: 9,
            saturation_milli: 9000,
            exhaustion_milli: 250,
            respawn_present: true,
            respawn_dimension: 0,
            respawn_position: [10.0, 64.0, 11.0],
            armor: [ItemStack::default(); 4],
        };
        let seeded = seed_player(SessionKey::from_raw(1).unwrap(), &save).unwrap();
        let mut actor = seeded.actor;
        actor.dimension = Dimension::OVERWORLD;
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([6.0, 90.0, 7.0]).unwrap(),
            velocity: FiniteVec3::try_new([0.25, -0.5, 0.75]).unwrap(),
            on_ground: true,
        });
        actor.survival = survival(7, 9, 3);
        let runtime = ActorRuntime {
            key: actor.key,
            controls: Some(PlayerControl::new(PlayerControlParts {
                movement: Movement {
                    move_x: 1,
                    move_z: -1,
                    jump: true,
                },
                look: LookAngles::try_new(0.1, 0.2).unwrap(),
                actions: HeldActions {
                    primary: true,
                    eating: true,
                    sprinting: true,
                    sneaking: true,
                },
            })),
            has_view: true,
            reset: true,
            attack_cooldown: 11,
            hurt_cooldown: 12,
            burn_cooldown: 13,
            oxygen: 3,
            peak_y: 90.0,
            exhaustion_milli: 250,
            saturation_milli: 9000,
            since_damage_ticks: 17,
            drown_ticks: 18,
            starvation_ticks: 19,
            eating: Some(EatingProgress {
                slot: HotbarSlot::new(2).unwrap(),
                item: 5,
                ticks: 6,
            }),
            bow: Some(BowProgress {
                slot: HotbarSlot::new(3).unwrap(),
                ticks: 8,
            }),
            path: None,
            aux: ActorAux::Player {
                respawn: Some((Dimension::DEPTHS, BlockPos::new(-1, 2, 0))),
                workbench: Some(BlockPos::new(2, 3, 4)),
            },
        };
        let mut inventory = seeded.inventory;
        inventory.crafting[0] = ItemStack {
            item: 5,
            count: 2,
            durability: 0,
        };
        (actor, runtime, inventory)
    }
    fn survival(health: u8, hunger: u8, oxygen: u16) -> SurvivalState {
        SurvivalState::try_new(SurvivalStateParts {
            health,
            oxygen,
            hunger,
            saturation_zero: false,
            armor_points: 4,
        })
        .unwrap()
    }
    fn expected(
        mut actor: ActorRecord,
        mut runtime: ActorRuntime,
        position: [f32; 3],
    ) -> (ActorRecord, ActorRuntime) {
        actor.lifecycle = ActorLifecycle::Pending;
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: false,
        });
        actor.survival = survival(actor.survival.health(), actor.survival.hunger(), 300);
        runtime.controls = None;
        runtime.reset = false;
        runtime.attack_cooldown = 0;
        runtime.hurt_cooldown = 0;
        runtime.oxygen = 300;
        runtime.peak_y = 321.0;
        runtime.drown_ticks = 0;
        runtime.eating = None;
        runtime.bow = None;
        (actor, runtime)
    }
    #[test]
    fn recovery_prepared_owner_double_maps_every_field() {
        let (mut actor, mut runtime, inventory) = pair();
        let inventory_before = inventory;
        let raw_ack = 3;
        let ever_spawned = true;
        let want = expected(actor.clone(), runtime.clone(), [-31.5, 321.0, 48.5]);
        let restore = scan(RestoreKind::Player, ChunkPos::new(-2, 3), 1, true);
        assert_eq!(begin_reset(&mut actor, &mut runtime, &restore), Ok(()));
        assert_eq!((actor, runtime), want);
        assert_eq!(inventory, inventory_before);
        assert_eq!(raw_ack, 3);
        assert!(ever_spawned);
    }
    #[test]
    fn death_prepared_owner_double_reuses_mapping_after_full_survival_fill() {
        let (mut actor, mut runtime, _) = pair();
        // Caller preparation precedes the common map; no drop or bed validation runs here.
        actor.survival = survival(20, 20, 3);
        runtime.saturation_milli = 5000;
        runtime.exhaustion_milli = 0;
        runtime.since_damage_ticks = 0;
        runtime.starvation_ticks = 0;
        let want = expected(actor.clone(), runtime.clone(), [-31.5, 321.0, 48.5]);
        let restore = scan(RestoreKind::Player, ChunkPos::new(-2, 3), 1, true);
        begin_reset(&mut actor, &mut runtime, &restore).unwrap();
        assert_eq!((actor, runtime), want);
    }
    #[test]
    fn preserves_arbitrarily_capacious_path_and_body_allocations() {
        let (mut actor, mut runtime, _) = pair();
        let ActorBody::Player(body) = &mut actor.body else {
            unreachable!()
        };
        let mut name = String::with_capacity(65536);
        name.push_str("Ada");
        body.display_name = name;
        let name_allocation = (
            body.display_name.as_ptr(),
            body.display_name.len(),
            body.display_name.capacity(),
        );
        let mut revisions = Vec::with_capacity(65536);
        revisions.extend((0..9).map(|n| {
            (
                ChunkKey {
                    dimension: Dimension::DEPTHS,
                    pos: ChunkPos::new(n, -n),
                },
                n as u64,
            )
        }));
        let mut waypoints = Vec::with_capacity(65536);
        waypoints.extend((0..17).map(|n| BlockPos::new(n, 64, -n)));
        let allocations = (
            revisions.as_ptr(),
            revisions.len(),
            revisions.capacity(),
            waypoints.as_ptr(),
            waypoints.len(),
            waypoints.capacity(),
        );
        runtime.path = Some(PathState {
            generation: 7,
            target: BlockPos::new(1, 2, 3),
            revisions,
            waypoints,
            cursor: 2,
            next_repath_tick: 900,
        });
        // Snapshots allocate outside the helper; pointer checks qualify owner preservation.
        let want = expected(actor.clone(), runtime.clone(), [-31.5, 321.0, 48.5]);
        let restore = scan(RestoreKind::Player, ChunkPos::new(-2, 3), 1, true);
        begin_reset(&mut actor, &mut runtime, &restore).unwrap();
        assert_eq!((&actor, &runtime), (&want.0, &want.1));
        let ActorBody::Player(body) = &actor.body else {
            unreachable!()
        };
        assert_eq!(
            (
                body.display_name.as_ptr(),
                body.display_name.len(),
                body.display_name.capacity()
            ),
            name_allocation
        );
        let path = runtime.path.as_ref().unwrap();
        assert_eq!(
            (
                path.revisions.as_ptr(),
                path.revisions.len(),
                path.revisions.capacity(),
                path.waypoints.as_ptr(),
                path.waypoints.len(),
                path.waypoints.capacity()
            ),
            allocations
        );
    }
    #[test]
    fn invalid_pair_table_refuses_atomically_before_scan_readiness() {
        let restore = scan(RestoreKind::Player, ChunkPos::new(-2, 3), 1, false);
        for case in 0..7 {
            let (mut actor, mut runtime, _) = pair();
            match case {
                0 => actor.key = ActorKey::Passive(PassiveId::try_new(1).unwrap()),
                1 => {
                    actor.body = ActorBody::Passive(PassiveMob {
                        id: 1,
                        dimension: 0,
                        position: [0.0, 64.0, 0.0],
                        velocity: [0.0; 3],
                        on_ground: true,
                        yaw: 0.0,
                        health: 7,
                    })
                }
                2 => runtime.key = ActorKey::Player(SessionKey::from_raw(2).unwrap()),
                3 => {
                    runtime.aux = ActorAux::Hostile {
                        distant_ticks: 1,
                        shoot_cooldown: 2,
                        fresh: true,
                    }
                }
                4 => actor.lifecycle = ActorLifecycle::Pending,
                5 => actor.lifecycle = ActorLifecycle::Respawning,
                6 => actor.lifecycle = ActorLifecycle::Dead,
                _ => unreachable!(),
            }
            let before = (actor.clone(), runtime.clone());
            assert_eq!(
                begin_reset(&mut actor, &mut runtime, &restore),
                Err(ServerError::InvalidInput {
                    field: "source_player_reset"
                }),
                "case {case}"
            );
            assert_eq!((actor, runtime), before, "case {case}");
        }
    }
    #[test]
    fn incomplete_player_and_completed_companion_refuse_unchanged() {
        for restore in [
            scan(RestoreKind::Player, ChunkPos::new(-2, 3), 1, false),
            scan(RestoreKind::Companion, ChunkPos::new(-2, 3), 16, true),
        ] {
            let (mut actor, mut runtime, _) = pair();
            let before = (actor.clone(), runtime.clone());
            assert_eq!(
                begin_reset(&mut actor, &mut runtime, &restore),
                Err(ServerError::InvalidInput {
                    field: "restore_restart"
                })
            );
            assert_eq!((actor, runtime), before);
        }
    }
    #[test]
    fn checked_anchor_table_uses_source_float_arithmetic() {
        for (anchor, radius, position) in [
            (ChunkPos::new(-2, 3), 1, [-31.5, 321.0, 48.5]),
            (ChunkPos::new(2, -1), 1, [32.5, 321.0, -15.5]),
            (
                ChunkPos::new(134217723, -134217724),
                64,
                [
                    (134217723_i32 as f32) * 16.0 + 0.5,
                    321.0,
                    (-134217724_i32 as f32) * 16.0 + 0.5,
                ],
            ),
        ] {
            let (mut actor, mut runtime, _) = pair();
            let want = expected(actor.clone(), runtime.clone(), position);
            let restore = scan(RestoreKind::Player, anchor, radius, true);
            begin_reset(&mut actor, &mut runtime, &restore).unwrap();
            assert_eq!((actor, runtime), want);
        }
    }
    #[test]
    fn actor_dimension_survives_different_completed_scan_dimension() {
        let (mut actor, mut runtime, _) = pair();
        actor.dimension = Dimension::DEPTHS;
        // The successful AIR candidate completed in Overworld, independent of constructor dimension.
        let restore = scan(RestoreKind::Player, ChunkPos::new(-2, 3), 1, true);
        let want = expected(actor.clone(), runtime.clone(), [-31.5, 321.0, 48.5]);
        begin_reset(&mut actor, &mut runtime, &restore).unwrap();
        assert_eq!((actor, runtime), want);
    }
}
