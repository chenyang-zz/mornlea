//! Bounded, borrowed projection of settled fields into existing save records.
//!
//! These helpers choose neither persistability nor revision ownership. The
//! caller supplies one actor and its fixed overlays; inputs stay untouched on
//! every refusal. Safe observations, immutable identity and omitted transients
//! remain with their existing owners.

use super::contracts::{
    ActorAux, ActorBody, ActorKey, ActorRecord, ActorRuntime, InventoryRecord, ServerError,
};
use mornlea_storage::{
    CompanionBody, HostileMob, HostileMobsSave, Inventory, PassiveMob, PassiveMobsSave, PlayerSave,
    hostile_mobs_encoded_len, passive_mobs_encoded_len, player_encoded_len,
};

const INVALID: ServerError = ServerError::InvalidInput {
    field: "actor_save",
};

/// Projects current player pose, survival and optional fixed overlays.
///
/// Body identity/name/safe facts remain immutable here. Absent overlays keep
/// stored fallback values; a present runtime with no respawn explicitly clears
/// that value. The caller supplies the durable revision, including its policy.
pub fn project_player(
    actor: &ActorRecord,
    inventory: Option<&InventoryRecord>,
    runtime: Option<&ActorRuntime>,
    revision: u64,
) -> Result<PlayerSave, ServerError> {
    let (ActorKey::Player(_), ActorBody::Player(body)) = (actor.key, &actor.body) else {
        return Err(INVALID);
    };
    if revision == 0 {
        return Err(INVALID);
    }
    let mut output = body.clone();
    output.revision = revision;
    output.current.dimension = i32::from(actor.dimension.get());
    output.current.position = actor.motion.position().get();
    output.yaw = actor.look.yaw();
    output.pitch = actor.look.pitch();
    output.health = actor.survival.health();
    output.hunger = actor.survival.hunger();
    if let Some(inventory) = inventory {
        project_inventory(&mut output.inventory, inventory);
        // Armor is raw slot fidelity in the storage contract, not equipment policy.
        output.armor = inventory.armor;
    }
    if let Some(runtime) = runtime {
        if runtime.key != actor.key {
            return Err(INVALID);
        }
        let ActorAux::Player { respawn, .. } = &runtime.aux else {
            return Err(INVALID);
        };
        output.saturation_milli = u16::try_from(runtime.saturation_milli).map_err(|_| INVALID)?;
        output.exhaustion_milli = u16::try_from(runtime.exhaustion_milli).map_err(|_| INVALID)?;
        if let Some((dimension, position)) = respawn {
            output.respawn_present = true;
            output.respawn_dimension = i32::from(dimension.get());
            // Source respawn conversion stores each integer block coordinate as f32.
            output.respawn_position = [
                position.x() as f32,
                position.y() as f32,
                position.z() as f32,
            ];
        } else {
            output.respawn_present = false;
            output.respawn_dimension = 0;
            output.respawn_position = [0.0; 3];
        }
    }
    player_encoded_len(&output).map_err(|_| INVALID)?;
    Ok(output)
}

/// Projects one companion body without constructing lifecycle/memory aggregates.
/// Only current pose and optional inventory belong to this format; the UUID
/// must already agree with the actor key.
pub fn project_companion(
    actor: &ActorRecord,
    inventory: Option<&InventoryRecord>,
) -> Result<CompanionBody, ServerError> {
    let (ActorKey::Companion(key), ActorBody::Companion(body)) = (actor.key, &actor.body) else {
        return Err(INVALID);
    };
    if key.bytes() != body.id.to_bytes() {
        return Err(INVALID);
    }
    let mut output = body.clone();
    output.dimension = i32::from(actor.dimension.get());
    output.position = actor.motion.position().get();
    output.yaw = actor.look.yaw();
    output.pitch = actor.look.pitch();
    if let Some(inventory) = inventory {
        project_inventory(&mut output.inventory, inventory);
    }
    validate_companion_body(&output)?;
    Ok(output)
}

/// Projects hostile motion and the runtime-owned burn/distance counters.
/// Combat health, attack/hurt, target, repath and kind stay canonical in the
/// body provider; neutral runtime fields must not erase those persisted facts.
pub fn project_hostile(
    actor: &ActorRecord,
    runtime: Option<&ActorRuntime>,
) -> Result<HostileMob, ServerError> {
    let (ActorKey::Hostile(key), ActorBody::Hostile(body)) = (actor.key, &actor.body) else {
        return Err(INVALID);
    };
    if key.get() != body.id {
        return Err(INVALID);
    }
    let mut output = body.clone();
    output.dimension = i32::from(actor.dimension.get());
    output.position = actor.motion.position().get();
    output.velocity = actor.motion.velocity().get();
    output.on_ground = actor.motion.on_ground();
    output.yaw = actor.look.yaw();
    if let Some(runtime) = runtime {
        if runtime.key != actor.key {
            return Err(INVALID);
        }
        let ActorAux::Hostile { distant_ticks, .. } = &runtime.aux else {
            return Err(INVALID);
        };
        output.burn_cooldown = u8::try_from(runtime.burn_cooldown).map_err(|_| INVALID)?;
        output.distant_ticks = *distant_ticks;
    }
    // This one-record aggregate only invokes the actual codec validation;
    // revision 1 is not an allocated save target or aggregate history.
    let mut validation = HostileMobsSave {
        revision: 1,
        records: vec![output],
    };
    hostile_mobs_encoded_len(&validation).map_err(|_| INVALID)?;
    Ok(validation.records.pop().expect("one validation record"))
}

/// Projects passive motion while preserving the damage provider's body health.
/// Lifecycle membership and the runtime-only home/flee/graze lanes stay with
/// the later aggregate owner.
pub fn project_passive(actor: &ActorRecord) -> Result<PassiveMob, ServerError> {
    let (ActorKey::Passive(key), ActorBody::Passive(body)) = (actor.key, &actor.body) else {
        return Err(INVALID);
    };
    if key.get() != body.id {
        return Err(INVALID);
    }
    let mut output = body.clone();
    output.dimension = i32::from(actor.dimension.get());
    output.position = actor.motion.position().get();
    output.velocity = actor.motion.velocity().get();
    output.on_ground = actor.motion.on_ground();
    output.yaw = actor.look.yaw();
    let mut validation = PassiveMobsSave {
        revision: 1,
        records: vec![output],
    };
    passive_mobs_encoded_len(&validation).map_err(|_| INVALID)?;
    Ok(validation.records.pop().expect("one validation record"))
}

fn project_inventory(output: &mut Inventory, inventory: &InventoryRecord) {
    output.hotbar.slots.copy_from_slice(&inventory.slots[..9]);
    output.backpack.copy_from_slice(&inventory.slots[9..36]);
    output.hotbar.selected = inventory.selected.get();
}

/// Exact mirror of private mornlea_storage::companion::validate_body. There is
/// no standalone public body codec, and fabricated aggregate state would add
/// unrelated lifecycle/namespace/entropy requirements to this borrowed helper.
fn validate_companion_body(body: &CompanionBody) -> Result<(), ServerError> {
    if !body.id.is_valid()
        || body.dimension != 0
        || !body.position.iter().all(|value| value.is_finite())
        || !body.yaw.is_finite()
        || !body.pitch.is_finite()
        || body.pitch < -std::f32::consts::FRAC_PI_2
        || body.pitch > std::f32::consts::FRAC_PI_2
        || !body.inventory.is_valid()
    {
        return Err(INVALID);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::*;
    use mornlea_domain::{
        BlockPos, CompanionId, Dimension, FiniteVec3, HostileId, HotbarSlot, LookAngles,
        MotionState, MotionStateParts, PassiveId, SurvivalState, SurvivalStateParts,
    };
    use mornlea_storage::{
        CompanionBody, HostileMob, Inventory, ItemStack, PassiveMob, PlayerId, PlayerLocation,
        PlayerSave,
    };

    fn uuid(tag: u8) -> [u8; 16] {
        let mut bytes = [0; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        bytes
    }
    fn player_body() -> PlayerSave {
        PlayerSave {
            player_id: PlayerId::from_bytes(uuid(1)),
            revision: 9,
            display_name: "Ada".into(),
            current: PlayerLocation {
                dimension: 0,
                position: [1.0, 65.0, 2.0],
            },
            yaw: 0.1,
            pitch: 0.2,
            safe: Some(PlayerLocation {
                dimension: 1,
                position: [3.0, 64.0, 4.0],
            }),
            inventory: Inventory::default(),
            health: 20,
            hunger: 20,
            saturation_milli: 5000,
            exhaustion_milli: 250,
            respawn_present: true,
            respawn_dimension: 0,
            respawn_position: [2.0, 64.0, 3.0],
            armor: [ItemStack::default(); 4],
        }
    }
    fn actor(key: ActorKey, body: ActorBody) -> ActorRecord {
        ActorRecord::try_new(
            key,
            ActorLifecycle::Active,
            Dimension::OVERWORLD,
            MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([10.5, 66.0, -3.25]).unwrap(),
                velocity: FiniteVec3::try_new([0.25, -0.5, 0.75]).unwrap(),
                on_ground: true,
            }),
            LookAngles::try_new(1.2, 0.3).unwrap(),
            SurvivalState::try_new(SurvivalStateParts {
                health: 15,
                oxygen: 111,
                hunger: 12,
                saturation_zero: false,
                armor_points: 0,
            })
            .unwrap(),
            body,
        )
        .unwrap()
    }
    fn player() -> ActorRecord {
        actor(
            ActorKey::Player(SessionKey::from_raw(1).unwrap()),
            ActorBody::Player(player_body()),
        )
    }
    fn coal(count: u8) -> ItemStack {
        ItemStack {
            item: 5,
            count,
            durability: 0,
        }
    }
    fn inventory() -> InventoryRecord {
        let mut v = InventoryRecord::empty();
        v.slots[2] = coal(7);
        v.slots[9] = coal(64);
        v.slots[35] = coal(1);
        v.selected = HotbarSlot::new(5).unwrap();
        // Storage preserves raw armor even when simulation would refuse its item.
        v.armor[1] = ItemStack {
            item: u16::MAX,
            count: u8::MAX,
            durability: u16::MAX,
        };
        v.crafting[0] = coal(2);
        v
    }
    fn runtime(key: ActorKey, aux: ActorAux) -> ActorRuntime {
        ActorRuntime {
            key,
            controls: None,
            has_view: true,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 17,
            oxygen: 111,
            peak_y: 77.0,
            exhaustion_milli: 4321,
            saturation_milli: 1234,
            since_damage_ticks: 19,
            drown_ticks: 20,
            starvation_ticks: 21,
            eating: None,
            bow: None,
            path: None,
            aux,
        }
    }
    fn player_runtime(key: ActorKey) -> ActorRuntime {
        runtime(
            key,
            ActorAux::Player {
                respawn: Some((Dimension::DEPTHS, BlockPos::new(-7, 64, 8))),
                workbench: Some(BlockPos::new(1, 2, 3)),
            },
        )
    }
    fn companion() -> ActorRecord {
        actor(
            ActorKey::Companion(CompanionId::try_from_bytes(uuid(2)).unwrap()),
            ActorBody::Companion(CompanionBody {
                id: PlayerId::from_bytes(uuid(2)),
                dimension: 0,
                position: [1.0, 65.0, 2.0],
                yaw: 0.1,
                pitch: 0.2,
                inventory: Inventory::default(),
            }),
        )
    }
    fn hostile() -> ActorRecord {
        let mut v = actor(
            ActorKey::Hostile(HostileId::try_new(3).unwrap()),
            ActorBody::Hostile(HostileMob {
                id: 3,
                dimension: 0,
                position: [1.0, 65.0, 2.0],
                velocity: [0.0; 3],
                on_ground: false,
                yaw: 0.1,
                health: 13,
                attack_cooldown: 7,
                hurt_cooldown: 9,
                burn_cooldown: 11,
                has_target: true,
                player_id: PlayerId::from_bytes(uuid(1)),
                next_repath_ticks: 123456,
                distant_ticks: 12,
                kind: 1,
            }),
        );
        v.survival = SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap();
        v
    }
    fn hostile_runtime(key: ActorKey) -> ActorRuntime {
        runtime(
            key,
            ActorAux::Hostile {
                distant_ticks: 33,
                shoot_cooldown: 123,
                fresh: true,
            },
        )
    }
    fn passive() -> ActorRecord {
        actor(
            ActorKey::Passive(PassiveId::try_new(4).unwrap()),
            ActorBody::Passive(PassiveMob {
                id: 4,
                dimension: 0,
                position: [1.0, 65.0, 2.0],
                velocity: [0.0; 3],
                on_ground: false,
                yaw: 0.1,
                health: 13,
            }),
        )
    }
    fn invalid<T: std::fmt::Debug>(actual: Result<T, ServerError>) {
        assert!(
            matches!(
                actual,
                Err(ServerError::InvalidInput {
                    field: "actor_save"
                })
            ),
            "{actual:?}"
        );
    }
    #[test]
    fn player_projects_every_current_field_and_retains_immutable_body_facts() {
        let a = player();
        let i = inventory();
        let r = player_runtime(a.key);
        let before = (a.clone(), i, r.clone());
        let out = project_player(&a, Some(&i), Some(&r), 17).unwrap();
        let mut expected = player_body();
        expected.revision = 17;
        expected.current = PlayerLocation {
            dimension: 0,
            position: [10.5, 66.0, -3.25],
        };
        expected.yaw = 1.2;
        expected.pitch = 0.3;
        expected.health = 15;
        expected.hunger = 12;
        expected
            .inventory
            .hotbar
            .slots
            .copy_from_slice(&i.slots[..9]);
        expected.inventory.backpack.copy_from_slice(&i.slots[9..]);
        expected.inventory.hotbar.selected = 5;
        expected.armor = i.armor;
        expected.saturation_milli = 1234;
        expected.exhaustion_milli = 4321;
        expected.respawn_present = true;
        expected.respawn_dimension = 1;
        expected.respawn_position = [-7.0, 64.0, 8.0];
        assert_eq!(out, expected);
        assert_eq!((a, i, r), before);
    }
    #[test]
    fn player_optional_fallbacks_and_respawn_clear_are_exact() {
        let a = player();
        let out = project_player(&a, None, None, 17).unwrap();
        let body = player_body();
        assert_eq!(out.inventory, body.inventory);
        assert_eq!(out.armor, body.armor);
        assert_eq!((out.saturation_milli, out.exhaustion_milli), (5000, 250));
        assert_eq!(
            (
                out.respawn_present,
                out.respawn_dimension,
                out.respawn_position
            ),
            (true, 0, [2.0, 64.0, 3.0])
        );
        let mut r = player_runtime(a.key);
        r.aux = ActorAux::Player {
            respawn: None,
            workbench: None,
        };
        let out = project_player(&a, None, Some(&r), 17).unwrap();
        assert_eq!(
            (
                out.respawn_present,
                out.respawn_dimension,
                out.respawn_position
            ),
            (false, 0, [0.0; 3])
        );
        let mut a = a;
        a.dimension = Dimension::DEPTHS;
        assert_eq!(
            project_player(&a, None, None, 17)
                .unwrap()
                .current
                .dimension,
            1
        );
    }
    #[test]
    fn companion_projects_pose_and_fixed_inventory_with_exact_fallback() {
        let a = companion();
        let i = inventory();
        let before = (a.clone(), i);
        let mut expected = match &a.body {
            ActorBody::Companion(b) => b.clone(),
            _ => unreachable!(),
        };
        expected.position = [10.5, 66.0, -3.25];
        expected.yaw = 1.2;
        expected.pitch = 0.3;
        let fallback = project_companion(&a, None).unwrap();
        assert_eq!(fallback, expected);
        expected
            .inventory
            .hotbar
            .slots
            .copy_from_slice(&i.slots[..9]);
        expected.inventory.backpack.copy_from_slice(&i.slots[9..]);
        expected.inventory.hotbar.selected = 5;
        assert_eq!(project_companion(&a, Some(&i)).unwrap(), expected);
        assert_eq!((a, i), before);
    }
    #[test]
    fn hostile_retains_body_combat_and_projects_outer_motion_and_runtime_burn() {
        let a = hostile();
        let r = hostile_runtime(a.key);
        let before = (a.clone(), r.clone());
        let mut expected = match &a.body {
            ActorBody::Hostile(b) => b.clone(),
            _ => unreachable!(),
        };
        expected.position = [10.5, 66.0, -3.25];
        expected.velocity = [0.25, -0.5, 0.75];
        expected.on_ground = true;
        expected.yaw = 1.2;
        assert_eq!(project_hostile(&a, None).unwrap(), expected);
        expected.burn_cooldown = 17;
        expected.distant_ticks = 33;
        assert_eq!(project_hostile(&a, Some(&r)).unwrap(), expected);
        assert_eq!((a, r), before);
    }
    #[test]
    fn passive_retains_body_health_and_projects_outer_motion() {
        let a = passive();
        let before = a.clone();
        let mut expected = match &a.body {
            ActorBody::Passive(b) => b.clone(),
            _ => unreachable!(),
        };
        expected.position = [10.5, 66.0, -3.25];
        expected.velocity = [0.25, -0.5, 0.75];
        expected.on_ground = true;
        expected.yaw = 1.2;
        assert_eq!(project_passive(&a).unwrap(), expected);
        assert_eq!(a, before);
    }
    #[test]
    fn all_families_refuse_wrong_key_body_or_body_identity_atomically() {
        let mut p = player();
        p.key = companion().key;
        let before = p.clone();
        invalid(project_player(&p, None, None, 17));
        assert_eq!(p, before);
        let mut p = player();
        p.body = companion().body;
        invalid(project_player(&p, None, None, 17));
        invalid(project_companion(&player(), None));
        invalid(project_hostile(&player(), None));
        invalid(project_passive(&player()));
        let mut c = companion();
        c.key = ActorKey::Companion(CompanionId::try_from_bytes(uuid(3)).unwrap());
        invalid(project_companion(&c, None));
        let mut h = hostile();
        h.key = ActorKey::Hostile(HostileId::try_new(4).unwrap());
        invalid(project_hostile(&h, None));
        let mut p = passive();
        p.key = ActorKey::Passive(PassiveId::try_new(5).unwrap());
        invalid(project_passive(&p));
        for body in [hostile().body, passive().body, player().body] {
            let mut c = companion();
            c.body = body;
            invalid(project_companion(&c, None));
        }
        let mut h = hostile();
        h.body = passive().body;
        invalid(project_hostile(&h, None));
        let mut p = passive();
        p.body = hostile().body;
        invalid(project_passive(&p));
    }
    #[test]
    fn supplied_runtime_family_key_and_narrowing_refuse_without_mutation() {
        let p = player();
        invalid(project_player(&p, None, None, 0));
        let mut r = player_runtime(p.key);
        r.key = companion().key;
        invalid(project_player(&p, None, Some(&r), 17));
        r = hostile_runtime(p.key);
        invalid(project_player(&p, None, Some(&r), 17));
        for saturation in [true, false] {
            r = player_runtime(p.key);
            if saturation {
                r.saturation_milli = 65536
            } else {
                r.exhaustion_milli = 65536
            };
            let before = r.clone();
            invalid(project_player(&p, None, Some(&r), 17));
            assert_eq!(r, before);
        }
        let h = hostile();
        r = hostile_runtime(h.key);
        r.key = p.key;
        invalid(project_hostile(&h, Some(&r)));
        r = player_runtime(h.key);
        invalid(project_hostile(&h, Some(&r)));
        r = hostile_runtime(h.key);
        r.burn_cooldown = 256;
        let before = r.clone();
        invalid(project_hostile(&h, Some(&r)));
        assert_eq!(r, before);
    }
    #[test]
    fn codec_limits_refuse_actual_invalid_final_shapes() {
        let mut p = player();
        if let ActorBody::Player(b) = &mut p.body {
            b.player_id = PlayerId::from_bytes([0; 16])
        }
        invalid(project_player(&p, None, None, 17));
        let mut p = player();
        if let ActorBody::Player(b) = &mut p.body {
            b.display_name = " bad ".into()
        }
        invalid(project_player(&p, None, None, 17));
        let mut p = player();
        if let ActorBody::Player(b) = &mut p.body {
            b.safe.as_mut().unwrap().position[0] = f32::NAN
        }
        invalid(project_player(&p, None, None, 17));
        let p = player();
        let mut r = player_runtime(p.key);
        r.saturation_milli = 12001;
        invalid(project_player(&p, None, Some(&r), 17));
        let mut i = inventory();
        i.slots[0] = ItemStack {
            item: u16::MAX,
            count: 1,
            durability: 0,
        };
        invalid(project_player(&p, Some(&i), None, 17));
        invalid(project_companion(&companion(), Some(&i)));
        let mut p = player();
        p.look = LookAngles::try_new(0.0, 2.0).unwrap();
        invalid(project_player(&p, None, None, 17));
        let mut c = companion();
        c.dimension = Dimension::DEPTHS;
        invalid(project_companion(&c, None));
        let mut h = hostile();
        h.dimension = Dimension::DEPTHS;
        invalid(project_hostile(&h, None));
        let mut p = passive();
        p.dimension = Dimension::DEPTHS;
        invalid(project_passive(&p));
        for health in [0, 21] {
            let mut h = hostile();
            if let ActorBody::Hostile(b) = &mut h.body {
                b.health = health
            }
            invalid(project_hostile(&h, None));
            let mut p = passive();
            if let ActorBody::Passive(b) = &mut p.body {
                b.health = health
            }
            invalid(project_passive(&p));
        }
        let mut h = hostile();
        if let ActorBody::Hostile(b) = &mut h.body {
            b.id = 0
        }
        invalid(project_hostile(&h, None));
        let mut p = passive();
        if let ActorBody::Passive(b) = &mut p.body {
            b.id = 0
        }
        invalid(project_passive(&p));
        for y in [-64.25, 320.0] {
            let mut h = hostile();
            h.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([0.0, y, 0.0]).unwrap(),
                velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                on_ground: false,
            });
            invalid(project_hostile(&h, None));
            let mut p = passive();
            p.motion = h.motion;
            invalid(project_passive(&p));
        }
        let mut h = hostile();
        if let ActorBody::Hostile(b) = &mut h.body {
            b.attack_cooldown = 21
        }
        invalid(project_hostile(&h, None));
        let mut h = hostile();
        if let ActorBody::Hostile(b) = &mut h.body {
            b.kind = 2
        }
        invalid(project_hostile(&h, None));
        let mut r = hostile_runtime(h.key);
        r.aux = ActorAux::Hostile {
            distant_ticks: 601,
            shoot_cooldown: 0,
            fresh: false,
        };
        invalid(project_hostile(&hostile(), Some(&r)));
    }
    #[test]
    fn companion_private_validator_matches_codec_checks_and_inclusive_pitch() {
        let body = match companion().body {
            ActorBody::Companion(b) => b,
            _ => unreachable!(),
        };
        for pitch in [-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2] {
            let mut v = body.clone();
            v.pitch = pitch;
            assert!(validate_companion_body(&v).is_ok());
        }
        let mut invalids = Vec::new();
        let mut v = body.clone();
        v.id = PlayerId::from_bytes([0; 16]);
        invalids.push(v);
        let mut v = body.clone();
        v.dimension = 1;
        invalids.push(v);
        for index in 0..3 {
            let mut v = body.clone();
            v.position[index] = f32::NAN;
            invalids.push(v);
        }
        let mut v = body.clone();
        v.yaw = f32::INFINITY;
        invalids.push(v);
        for pitch in [f32::NAN, -2.0, 2.0] {
            let mut v = body.clone();
            v.pitch = pitch;
            invalids.push(v);
        }
        let mut v = body;
        v.inventory.hotbar.selected = 9;
        invalids.push(v);
        for v in invalids {
            let before = v.clone();
            invalid(validate_companion_body(&v));
            assert_eq!(v.id, before.id);
        }
    }
    #[test]
    fn projection_does_not_choose_actor_lifecycle_policy() {
        for lifecycle in [
            ActorLifecycle::Pending,
            ActorLifecycle::Respawning,
            ActorLifecycle::Dead,
        ] {
            let mut p = player();
            p.lifecycle = lifecycle;
            assert!(project_player(&p, None, None, 17).is_ok());
            let mut c = companion();
            c.lifecycle = lifecycle;
            assert!(project_companion(&c, None).is_ok());
            let mut h = hostile();
            h.lifecycle = lifecycle;
            assert!(project_hostile(&h, None).is_ok());
            let mut p = passive();
            p.lifecycle = lifecycle;
            assert!(project_passive(&p).is_ok());
        }
    }
}
