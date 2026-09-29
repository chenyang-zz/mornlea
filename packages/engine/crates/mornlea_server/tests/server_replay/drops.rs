//! Replay of stored drop aging and pickup through the accepted slot transaction.

use super::*;
use mornlea_domain::{
    ChunkPos, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts, PlayerId, Season,
    SurvivalState, SurvivalStateParts, Weather, WorldStateParts,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorBody, ActorKey, ActorLifecycle, ActorRecord, ChunkKey, CloseReason, DropBatch, DropSource,
    EnvironmentState, FixtureState, InventoryRecord, RuleEffect, RulePhase, RuleTunables,
    SessionKey, TransportKind,
};
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::rules::drops as provider;
use mornlea_storage::{
    Chunk, ContainerSnapshot, ItemStack, PlayerLocation, PlayerSave, StorageKind,
};

fn key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    }
}
fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}
fn chunk() -> Chunk {
    Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}
fn world() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .unwrap()
}
fn environment(tunables: RuleTunables) -> EnvironmentState {
    EnvironmentState {
        seed: 1,
        next_tick: 1,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables,
    }
}
fn sessions(tags: &[u8]) -> Vec<SessionKey> {
    let mint_limits = mornlea_server::contracts::ServerLimits::try_new(8, 1, 1, 1, 1, 1).unwrap();
    let mut authority = AuthorityState::try_new(mint_limits, 0).unwrap();
    let mut minted = Vec::new();
    for &tag in tags {
        if minted.len() == 8 {
            authority
                .close_session(minted[0], CloseReason::PeerGone)
                .unwrap();
        }
        let mut bytes = [0; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let id = PlayerId::try_from_bytes(bytes).unwrap();
        let start = LoginStart::new(id, "Tester", 8).unwrap();
        let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
        minted.push(
            authority
                .admit(admit_login(inbound).unwrap(), TransportKind::Memory)
                .unwrap(),
        );
    }
    minted
}
fn session(tag: u8) -> SessionKey {
    sessions(&[tag])[0]
}
fn player(session: SessionKey, position: [f32; 3], lifecycle: ActorLifecycle) -> ActorRecord {
    let mut id = [0; 16];
    id[0] = 1;
    id[6] = 0x40;
    id[8] = 0x80;
    ActorRecord::try_new(
        ActorKey::Player(session),
        lifecycle,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).unwrap(),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap(),
        ActorBody::Player(PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(id),
            revision: 1,
            display_name: "Tester".to_owned(),
            current: PlayerLocation {
                dimension: 0,
                position,
            },
            yaw: 0.0,
            pitch: 0.0,
            safe: None,
            inventory: mornlea_storage::Inventory::default(),
            health: 20,
            hunger: 20,
            saturation_milli: 5000,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
        }),
    )
    .unwrap()
}
fn seeded(delay: u8, count: u8, players: &[(SessionKey, [f32; 3])]) -> FixtureState {
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(), 1, 1, chunk()).unwrap());
    ctx.stage(RuleEffect::Drops(
        DropBatch::try_new(
            DropSource::Panel {
                session: session(99),
                sequence: 1,
            },
            Dimension::OVERWORLD,
            FiniteVec3::try_new([0.5, 0.5, 0.5]).unwrap(),
            vec![stack(1, count)],
            delay,
        )
        .unwrap(),
    ))
    .unwrap();
    let mut initial = ctx.snapshot_state(world());
    for (session, position) in players {
        initial
            .actors
            .push(player(*session, *position, ActorLifecycle::Active));
        initial
            .inventories
            .push((ActorKey::Player(*session), InventoryRecord::empty()));
    }
    initial
}
fn context<'a>(authority: &'a mut AuthorityState, initial: &FixtureState) -> TickContext<'a> {
    let mut ctx = TickContext::from_fixture(authority, initial, TickBudget::full());
    ctx.stage(RuleEffect::Environment(environment(
        RuleTunables::source_defaults(),
    )))
    .unwrap();
    ctx
}

#[test]
fn delay_forty_then_atomic_pickup_conserves_stack() {
    let session = session(1);
    let initial = seeded(40, 3, &[(session, [0.5, 0.5, 0.5])]);
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    for remaining in (1..40).rev() {
        let report = provider::advance(&mut ctx, &[key()]).unwrap();
        assert_eq!((report.examined, report.applied), (1, 0));
        let view = ctx.read();
        let drop = &view.drops(key())[0];
        assert_eq!(drop.pickup_delay, remaining);
        assert_eq!(drop.stack.count, 3);
        assert_eq!(
            ctx.read()
                .inventory(ActorKey::Player(session))
                .unwrap()
                .slots[0]
                .count,
            0
        );
    }
    let report = provider::advance(&mut ctx, &[key()]).unwrap();
    assert_eq!((report.examined, report.applied), (1, 1));
    assert!(ctx.read().drops(key()).is_empty());
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(session))
            .unwrap()
            .slots[0],
        stack(1, 3)
    );
}

#[test]
fn exact_pickup_radius_and_age_expiry_precedes_pickup() {
    let session = session(1);
    for (distance, picked) in [(1.25, true), (1.251, false)] {
        let initial = seeded(0, 2, &[(session, [0.5 + distance, 0.5, 0.5])]);
        let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
        let mut ctx = context(&mut authority, &initial);
        let report = provider::advance(&mut ctx, &[key()]).unwrap();
        assert_eq!(report.applied, usize::from(picked));
        assert_eq!(ctx.read().drops(key()).is_empty(), picked);
    }
    let mut initial = seeded(0, 2, &[(session, [0.5, 0.5, 0.5])]);
    initial.chunks[0].3.drops[0].age_ticks = 5999;
    initial.drops[0].age = 5999;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    assert_eq!(provider::advance(&mut ctx, &[key()]).unwrap().applied, 1);
    assert!(ctx.read().drops(key()).is_empty());
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(session))
            .unwrap()
            .slots[0]
            .count,
        0
    );
}

#[test]
fn inactive_keys_unknown_chunks_and_wrapping_age() {
    let mut initial = seeded(2, 1, &[]);
    initial.chunks[0].3.drops[0].age_ticks = u32::MAX;
    initial.drops[0].age = u32::MAX;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    assert_eq!(provider::advance(&mut ctx, &[]).unwrap().examined, 0);
    let unknown = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(1, 0),
    };
    assert_eq!(provider::advance(&mut ctx, &[unknown]).unwrap().examined, 0);
    assert_eq!(ctx.read().drops(key())[0].age, u32::MAX);
    let report = provider::advance(&mut ctx, &[key()]).unwrap();
    assert_eq!((report.examined, report.applied), (1, 0));
    assert_eq!(ctx.read().drops(key())[0].age, 0);
    assert_eq!(ctx.read().drops(key())[0].pickup_delay, 1);
    assert_eq!(ctx.snapshot_state(world()).chunks[0].2, 2);
}

#[test]
fn session_order_partial_remainder_and_repack_veto() {
    let [first, second] = sessions(&[1, 2]).try_into().unwrap();
    let mut initial = seeded(0, 3, &[(second, [0.5; 3]), (first, [0.5; 3])]);
    let mut first_pack = InventoryRecord::empty();
    first_pack.slots.fill(stack(1, 64));
    first_pack.slots[0] = stack(1, 63);
    initial
        .inventories
        .iter_mut()
        .find(|(key, _)| *key == ActorKey::Player(first))
        .unwrap()
        .1 = first_pack;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    assert_eq!(provider::advance(&mut ctx, &[key()]).unwrap().applied, 1);
    assert_eq!(
        ctx.read().inventory(ActorKey::Player(first)).unwrap().slots[0].count,
        64
    );
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(second))
            .unwrap()
            .slots[0]
            .count,
        2
    );
    assert!(ctx.read().drops(key()).is_empty());

    let mut initial = seeded(0, 3, &[(first, [0.5; 3]), (second, [0.5; 3])]);
    let mut veto = InventoryRecord::empty();
    veto.slots.fill(stack(1, 64));
    veto.slots[0] = stack(1, 63);
    veto.crafting[0] = stack(1, 1);
    initial.inventories[0] = (ActorKey::Player(first), veto);
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    assert_eq!(provider::advance(&mut ctx, &[key()]).unwrap().applied, 1);
    assert_eq!(
        ctx.read().inventory(ActorKey::Player(first)).unwrap(),
        &veto
    );
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(second))
            .unwrap()
            .slots[0]
            .count,
        3
    );
}

#[test]
fn full_pack_and_exhausted_revision_preserve_items() {
    let session = session(1);
    let mut initial = seeded(0, 3, &[(session, [0.5; 3])]);
    let mut full = InventoryRecord::empty();
    full.slots.fill(stack(1, 64));
    initial.inventories[0].1 = full;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    let report = provider::advance(&mut ctx, &[key()]).unwrap();
    assert_eq!((report.examined, report.applied), (1, 0));
    assert_eq!(ctx.read().drops(key())[0].stack.count, 3);
    assert_eq!(
        ctx.read().inventory(ActorKey::Player(session)).unwrap(),
        &full
    );

    let mut initial = seeded(0, 3, &[(session, [0.5; 3])]);
    initial.chunks[0].2 = u64::MAX;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    assert_eq!(
        provider::advance(&mut ctx, &[key()]),
        Err(ServerError::InvalidInput { field: "drop_step" })
    );
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(session))
            .unwrap()
            .slots[0]
            .count,
        0
    );
    assert_eq!(ctx.read().drops(key())[0].stack.count, 3);
}

#[test]
fn capacity_and_shape_refuse_before_aging() {
    let first = session(1);
    let initial = seeded(3, 1, &[(first, [0.5; 3])]);
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    assert_eq!(
        provider::advance(&mut ctx, &vec![key(); 201]),
        Err(ServerError::Capacity {
            resource: mornlea_server::contracts::Resource::ChunkResults,
            limit: 200,
            observed: 201,
        })
    );
    assert_eq!(ctx.read().drops(key())[0].age, 0);
    let call = RuleCall {
        phase: RulePhase::DropStep,
        actor: None,
        command: None,
        internal: None,
    };
    assert_eq!(provider::run(&mut ctx, call).unwrap().examined, 0);
    assert_eq!(
        provider::run(
            &mut ctx,
            RuleCall {
                actor: Some(ActorKey::Player(first)),
                ..call
            }
        ),
        Err(ServerError::InvalidInput { field: "phase" })
    );
    assert_eq!(ctx.read().drops(key())[0].age, 0);

    let mut initial = initial;
    for session in sessions(&(1..=9).collect::<Vec<_>>()).into_iter().skip(1) {
        initial
            .actors
            .push(player(session, [0.5; 3], ActorLifecycle::Active));
        initial
            .inventories
            .push((ActorKey::Player(session), InventoryRecord::empty()));
    }
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    assert_eq!(
        provider::advance(&mut ctx, &[key()]),
        Err(ServerError::Capacity {
            resource: mornlea_server::contracts::Resource::Players,
            limit: 8,
            observed: 9,
        })
    );
    assert_eq!(ctx.read().drops(key())[0].age, 0);
}

#[test]
fn inactive_player_and_wrong_dimension_do_not_pickup() {
    let [one, two] = sessions(&[1, 2]).try_into().unwrap();
    let mut initial = seeded(0, 2, &[(one, [0.5; 3]), (two, [0.5; 3])]);
    initial.actors[0].lifecycle = ActorLifecycle::Dead;
    initial.actors[1].dimension = Dimension::DEPTHS;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    assert_eq!(provider::advance(&mut ctx, &[key()]).unwrap().applied, 0);
    assert_eq!(ctx.read().drops(key())[0].stack.count, 2);
}

#[test]
fn inactive_delay_and_restart_uses_stored_counters() {
    let mut initial = seeded(40, 1, &[]);
    initial.chunks[0].3.drops[5].generation = 7;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    for _ in 0..10 {
        provider::advance(&mut ctx, &[key()]).unwrap();
    }
    assert_eq!(
        (
            ctx.read().drops(key())[0].pickup_delay,
            ctx.read().drops(key())[0].age
        ),
        (30, 10)
    );
    for _ in 0..60 {
        provider::advance(&mut ctx, &[]).unwrap();
    }
    let mut saved = ctx.snapshot_state(world());
    assert_eq!(
        (
            saved.chunks[0].3.drops[0].pickup_delay_ticks,
            saved.chunks[0].3.drops[0].age_ticks
        ),
        (30, 10)
    );
    let save = mornlea_storage::ChunkSave {
        key: mornlea_storage::ChunkKey {
            dimension: 0,
            x: 0,
            z: 0,
        },
        revision: saved.chunks[0].2,
        chunk: saved.chunks[0].3.clone(),
    };
    let encoded = mornlea_storage::encode_chunk(&save).unwrap();
    let decoded = mornlea_storage::decode_chunk(save.key, save.revision, &encoded).unwrap();
    saved.chunks[0].3 = decoded.chunk;
    saved.drops.clear();
    let mut restored_authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut restored = context(&mut restored_authority, &saved);
    assert_eq!(
        provider::advance(&mut restored, &[key()]).unwrap().applied,
        0
    );
    assert_eq!(
        (
            restored.read().drops(key())[0].pickup_delay,
            restored.read().drops(key())[0].age
        ),
        (29, 11)
    );
    assert_eq!(
        restored.snapshot_state(world()).chunks[0].3.drops[0].generation,
        1
    );
    assert_eq!(
        restored.snapshot_state(world()).chunks[0].3.drops[5].generation,
        7
    );
}

#[test]
fn duplicate_interest_and_missing_environment() {
    let mut initial = seeded(3, 1, &[]);
    let unknown = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(1, 0),
    };
    initial
        .chunks
        .push((unknown, 1, 2, initial.chunks[0].3.clone()));
    let mut authority_a = AuthorityState::try_new(limits(), 0).unwrap();
    let mut a = context(&mut authority_a, &initial);
    let report_a = provider::advance(&mut a, &[unknown, key(), key()]).unwrap();
    let state_a = a.snapshot_state(world());
    let mut authority_b = AuthorityState::try_new(limits(), 0).unwrap();
    let mut b = context(&mut authority_b, &initial);
    let report_b = provider::advance(&mut b, &[key(), unknown]).unwrap();
    assert_eq!(report_a.examined, 2);
    assert_eq!(report_a, report_b);
    assert_eq!(state_a, b.snapshot_state(world()));
    let mut authority_c = AuthorityState::try_new(limits(), 0).unwrap();
    let mut c = TickContext::from_fixture(&mut authority_c, &initial, TickBudget::full());
    assert_eq!(provider::advance(&mut c, &[]).unwrap().examined, 0);
    assert_eq!(
        provider::advance(&mut c, &[key()]),
        Err(ServerError::InvalidInput { field: "drop_step" })
    );
}

#[test]
fn nondefault_lifetime_and_range_are_used() {
    let player = session(1);
    let initial = seeded(0, 1, &[(player, [2.5, 0.5, 0.5])]);
    let base = RuleTunables::source_defaults();
    let tunables = RuleTunables::try_new(
        base.physics(),
        100,
        40,
        20,
        80,
        18,
        4000,
        32,
        1600,
        200,
        5,
        3,
        50,
        6.0,
        1.62,
        10,
        40,
        2,
        2.0,
    )
    .unwrap();
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = TickContext::from_fixture(&mut authority, &initial, TickBudget::full());
    ctx.stage(RuleEffect::Environment(environment(tunables)))
        .unwrap();
    assert_eq!(provider::advance(&mut ctx, &[key()]).unwrap().applied, 1);
    assert!(ctx.read().drops(key()).is_empty());
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(player))
            .unwrap()
            .slots[0]
            .count,
        1
    );
    let mut initial = seeded(0, 1, &[(player, [2.5, 0.5, 0.5])]);
    initial.chunks[0].3.drops[0].age_ticks = 1;
    initial.drops[0].age = 1;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = TickContext::from_fixture(&mut authority, &initial, TickBudget::full());
    ctx.stage(RuleEffect::Environment(environment(tunables)))
        .unwrap();
    assert_eq!(provider::advance(&mut ctx, &[key()]).unwrap().applied, 1);
    assert_eq!(
        ctx.read()
            .inventory(ActorKey::Player(player))
            .unwrap()
            .slots[0]
            .count,
        0
    );
}

#[test]
fn pickup_preserves_tool_durability_and_unrelated_inventory() {
    let player = session(1);
    let mut initial = seeded(0, 1, &[(player, [0.5; 3])]);
    let tool = ItemStack {
        item: 10,
        count: 1,
        durability: 42,
    };
    initial.chunks[0].3.drops[0].stack = tool;
    initial.drops[0].stack = tool;
    let mut inventory = InventoryRecord::empty();
    inventory.armor[0] = ItemStack {
        item: 58,
        count: 1,
        durability: 7,
    };
    inventory.crafting[0] = stack(2, 1);
    initial.inventories[0].1 = inventory;
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = context(&mut authority, &initial);
    provider::advance(&mut ctx, &[key()]).unwrap();
    let after = ctx
        .read()
        .inventory(ActorKey::Player(player))
        .copied()
        .unwrap();
    assert_eq!(after.slots[0], tool);
    assert_eq!(after.armor, inventory.armor);
    assert_eq!(after.crafting, inventory.crafting);
    assert_eq!(after.selected, inventory.selected);
    assert_eq!(after.crafting_size, inventory.crafting_size);
}

#[test]
fn four_phase_pack_credit_prefers_hotbar_then_backpack() {
    let player = session(1);
    for (target, initial_stack) in [
        (0, stack(1, 63)),
        (2, ItemStack::default()),
        (9, stack(1, 63)),
        (10, ItemStack::default()),
    ] {
        let mut initial = seeded(0, 1, &[(player, [0.5; 3])]);
        let mut inventory = InventoryRecord::empty();
        inventory.slots.fill(stack(2, 64));
        inventory.slots[target] = initial_stack;
        initial.inventories[0].1 = inventory;
        let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
        let mut ctx = context(&mut authority, &initial);
        assert_eq!(provider::advance(&mut ctx, &[key()]).unwrap().applied, 1);
        assert_eq!(
            ctx.read()
                .inventory(ActorKey::Player(player))
                .unwrap()
                .slots[target],
            stack(1, initial_stack.count + 1)
        );
        assert!(ctx.read().drops(key()).is_empty());
        assert!(ctx.events().is_empty());
    }
}
