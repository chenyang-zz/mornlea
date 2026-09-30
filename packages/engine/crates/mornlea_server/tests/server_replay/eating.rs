//! Atomic eating progression replay.
//!
//! Every scene below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Hold ticks and the `(slot, item)` progress key from `advanceEating`
//!   (`packages/server/sim/entity/eating.go`): the starting tick counts as 1,
//!   an unchanged key increments once per tick, a changed key restarts at 1,
//!   and the zero progress value is the empty state
//!   (`TestEatingSettlesExactlyAtEatingTicksWithFixedValues` pins the tick
//!   positionality; `TestEatingSlotSwitchRestartsAndConsumesNeitherSlot` pins
//!   the key).
//! - Interrupt precedence from the same function: release, position reset,
//!   non-food, full hunger, an open container view and an unready view all
//!   clear before the settlement branch, so an interrupt landing on the
//!   settlement tick consumes nothing
//!   (`TestEatingContainerOpenOnSettlementTickDoesNotSettle`).
//! - Settlement arithmetic from the same function: one item consumed through
//!   the `Hotbar.Consume` normalization (`packages/shared/core/item.go`),
//!   hunger `min(20, old+gain)` and saturation `min(new_hunger*1000, old+gain)`
//!   over wide intermediates, with saturation clamped against the updated
//!   hunger and progress reset in the same atomic step.
//! - The food table from `FoodValue` (`packages/shared/core/hunger.go`):
//!   bread 5/6000, potato 1/600, carrot 3/3600, poisonous potato 2/1200,
//!   rotten flesh 4/0, raw beef 3/1800, cooked beef 8/12800. The poisonous
//!   potato settles as plain food while the status-effect system is absent,
//!   exactly like the Go table.
//! - Damage interruption itself is owned by the accepted survival provider
//!   (`applyDamage` in `packages/server/sim/entity/player.go` clears eating on
//!   actual health loss only); these cases drive the eating side of that lane
//!   and never reimplement the damage entry.
//!
//! No case chooses a value the oracle does not pin.

use mornlea_domain::{
    ChunkPos, ContainerKind, ContainerRef, Dimension, FiniteVec3, HeldActions, HotbarSlot,
    LookAngles, MotionState, MotionStateParts, Movement, PlayerControl, PlayerControlParts,
    SurvivalState, SurvivalStateParts,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::*;
use mornlea_server::rules::eating as provider;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_BREAD: u16 = 36; // `core.ItemBread`
const ITEM_POTATO: u16 = 40; // `core.ItemPotato`
const ITEM_CARROT: u16 = 41; // `core.ItemCarrot`
const ITEM_POISONOUS_POTATO: u16 = 42; // `core.ItemPoisonousPotato`
const ITEM_ROTTEN_FLESH: u16 = 45; // `core.ItemRottenFlesh`
const ITEM_RAW_BEEF: u16 = 53; // `core.ItemRawBeef`
const ITEM_COOKED_BEEF: u16 = 54; // `core.ItemCookedBeef`

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority")
}

fn admitted(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    admit_login(inbound).expect("admitted login")
}

fn harness_context(authority: &mut AuthorityState) -> TickContext<'_> {
    TickContext::harness(authority, TickBudget::full())
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}

fn slot(index: u8) -> HotbarSlot {
    HotbarSlot::new(index).expect("hotbar slot")
}

fn player_actor(session: SessionKey, hunger: u8, saturation_zero: bool) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([0.5, 64.0, 0.5]).expect("position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger,
            saturation_zero,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(player_body()),
    )
    .expect("player actor")
}

fn player_body() -> PlayerSave {
    PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
        revision: 1,
        display_name: "Tester".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [0.5, 64.0, 0.5],
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: mornlea_storage::Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0, 0.0, 0.0],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

/// Held control with only the eating bit pressed, the motion-owned intake the
/// provider reads back through the runtime lane.
fn hold_control(held: bool) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 0,
            jump: false,
        },
        look: LookAngles::try_new(0.0, 0.0).expect("look"),
        actions: HeldActions {
            primary: false,
            eating: held,
            sprinting: false,
            sneaking: false,
        },
    })
}

/// Motion-owned runtime record with a ready view, the held eating control and
/// the saturation lane under test; eating progress starts empty.
fn hold_runtime(actor: ActorKey, saturation_milli: u32, held: bool) -> ActorRuntime {
    ActorRuntime {
        key: actor,
        controls: Some(hold_control(held)),
        has_view: true,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 64.0,
        exhaustion_milli: 0,
        saturation_milli,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Player {
            respawn: None,
            workbench: None,
        },
    }
}

fn inventory_with(slots: &[(u8, ItemStack)], selected: u8) -> InventoryRecord {
    let mut inventory = InventoryRecord::empty();
    for (index, item) in slots {
        inventory.slots[usize::from(*index)] = *item;
    }
    inventory.with_selected(slot(selected))
}

/// One eating scene: an active player with the given hunger lanes, a hotbar of
/// `slots` with `selected` held, and the ready-view runtime carrying the
/// pressed eating control.
fn eating_scene(
    context: &mut TickContext<'_>,
    session: SessionKey,
    hunger: u8,
    saturation_milli: u32,
    slots: &[(u8, ItemStack)],
    selected: u8,
) -> ActorKey {
    context
        .stage(RuleEffect::Environment(eating_environment(32)))
        .expect("eating snapshot");
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            hunger,
            saturation_milli == 0,
        )))
        .expect("actor");
    context
        .stage(RuleEffect::Runtime(hold_runtime(
            actor,
            saturation_milli,
            true,
        )))
        .expect("runtime");
    context.preload_inventory(actor, inventory_with(slots, selected));
    actor
}

fn eating_environment(ticks: u16) -> EnvironmentState {
    let defaults = RuleTunables::source_defaults();
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: mornlea_domain::Weather::Clear,
        weather_remaining: 100,
        difficulty: 0,
        tunables: RuleTunables::try_new(
            defaults.physics(),
            100,
            40,
            20,
            80,
            18,
            4000,
            ticks,
            1600,
            200,
            5,
            3,
            50,
            6.0,
            1.62,
            10,
            40,
            6000,
            1.25,
        )
        .unwrap(),
    }
}

#[test]
fn configured_eating_duration_settles_at_exact_tick() {
    for configured in [8, 32, 0] {
        let mut state = authority();
        let session = state
            .admit(admitted(1, "Ada"), TransportKind::Memory)
            .unwrap();
        let mut context = harness_context(&mut state);
        let actor = eating_scene(
            &mut context,
            session,
            10,
            0,
            &[(0, stack(ITEM_BREAD, 2))],
            0,
        );
        context
            .stage(RuleEffect::Environment(eating_environment(configured)))
            .unwrap();
        for tick in 1..configured.max(1) {
            provider::run(&mut context, eating_call(actor)).unwrap();
            assert_eq!(held_progress(&context, actor).unwrap().ticks, tick);
            assert_eq!(context.read().inventory(actor).unwrap().slots[0].count, 2);
            assert_eq!(context.read().actor(actor).unwrap().survival.hunger(), 10);
            assert_eq!(context.read().runtime(actor).unwrap().saturation_milli, 0);
        }
        provider::run(&mut context, eating_call(actor)).unwrap();
        assert_eq!(
            context.read().inventory(actor).unwrap().slots[0].count,
            1,
            "duration {configured}"
        );
        assert_eq!(context.read().actor(actor).unwrap().survival.hunger(), 15);
        assert_eq!(
            context.read().runtime(actor).unwrap().saturation_milli,
            6000
        );
        assert_eq!(held_progress(&context, actor), None);
    }
}

#[test]
fn eating_duration_changes_and_interrupts_use_current_snapshot() {
    for interruption in ["none", "release", "reset", "view"] {
        let mut state = authority();
        let session = state
            .admit(admitted(1, "Ada"), TransportKind::Memory)
            .unwrap();
        let mut context = harness_context(&mut state);
        let actor = eating_scene(
            &mut context,
            session,
            10,
            0,
            &[(0, stack(ITEM_BREAD, 2))],
            0,
        );
        for _ in 0..7 {
            provider::run(&mut context, eating_call(actor)).unwrap();
        }
        context
            .stage(RuleEffect::Environment(eating_environment(8)))
            .unwrap();
        match interruption {
            "release" | "reset" => {
                let mut runtime = context.read().runtime(actor).unwrap().clone();
                if interruption == "release" {
                    runtime.controls = Some(hold_control(false));
                } else {
                    runtime.reset = true;
                }
                context.stage(RuleEffect::Runtime(runtime)).unwrap();
            }
            "view" => {
                context
                    .stage(RuleEffect::Viewer {
                        session,
                        view: Some(ViewLease::new(session, chest_reference())),
                    })
                    .unwrap();
            }
            _ => {}
        }
        provider::run(&mut context, eating_call(actor)).unwrap();
        assert_eq!(
            context.read().inventory(actor).unwrap().slots[0].count,
            if interruption == "none" { 1 } else { 2 }
        );
        assert_eq!(
            context.read().actor(actor).unwrap().survival.hunger(),
            if interruption == "none" { 15 } else { 10 }
        );
        assert_eq!(held_progress(&context, actor), None);
    }
}

#[test]
fn eligible_eating_without_environment_refuses_before_effects() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .unwrap();
    let mut context = harness_context(&mut state);
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Actor(player_actor(session, 10, true)))
        .unwrap();
    context
        .stage(RuleEffect::Runtime(hold_runtime(actor, 0, true)))
        .unwrap();
    context.preload_inventory(actor, inventory_with(&[(0, stack(ITEM_BREAD, 2))], 0));
    let before = context.resident_snapshot();
    let events = context.events().to_vec();
    assert_eq!(
        provider::run(&mut context, eating_call(actor)),
        Err(ServerError::Internal {
            invariant: "eating snapshot"
        })
    );
    let after = context.resident_snapshot();
    assert_eq!(after.actors, before.actors);
    assert_eq!(after.runtimes, before.runtimes);
    assert_eq!(after.inventories, before.inventories);
    assert_eq!(after.projectiles, before.projectiles);
    assert_eq!(context.events(), events);
}

fn eating_call(actor: ActorKey) -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::Eating,
        actor: Some(actor),
        command: None,
        internal: None,
    }
}

fn chest_reference() -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1)
        .expect("container reference")
}

/// Progress on the staged runtime lane after one call.
fn held_progress(context: &TickContext<'_>, actor: ActorKey) -> Option<EatingProgress> {
    context.read().runtime(actor).expect("runtime").eating
}

/// Hold progress from `advanceEating`: 31 ticks of bread leave the stack, the
/// hunger lanes and the running count untouched at tick 31, and tick 32
/// settles atomically — one bread gone, hunger 15, saturation 6000, progress
/// reset to the empty state.
#[test]
fn tick_31_32_atomic() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = eating_scene(
        &mut context,
        session,
        10,
        0,
        &[(0, stack(ITEM_BREAD, 2))],
        0,
    );
    let call = eating_call(actor);
    for tick in 1..=31u16 {
        provider::run(&mut context, call).expect("hold");
        assert_eq!(
            held_progress(&context, actor),
            Some(EatingProgress {
                slot: slot(0),
                item: ITEM_BREAD,
                ticks: tick,
            }),
            "tick {tick} advances the hold by exactly one"
        );
        let view = context.read();
        assert_eq!(
            view.inventory(actor).expect("inventory").slots[0].count,
            2,
            "tick {tick} consumes nothing"
        );
        assert_eq!(
            view.actor(actor).expect("actor").survival.hunger(),
            10,
            "tick {tick} leaves hunger untouched"
        );
        assert_eq!(
            view.runtime(actor).expect("runtime").saturation_milli,
            0,
            "tick {tick} leaves saturation untouched"
        );
    }
    provider::run(&mut context, call).expect("settle");
    let view = context.read();
    assert_eq!(
        view.inventory(actor).expect("inventory").slots[0],
        stack(ITEM_BREAD, 1),
        "tick 32 consumes exactly one item"
    );
    assert_eq!(
        view.actor(actor).expect("actor").survival.hunger(),
        15,
        "tick 32 settles the bread hunger gain"
    );
    let runtime = view.runtime(actor).expect("runtime");
    assert_eq!(runtime.saturation_milli, 6000, "tick 32 settles saturation");
    assert_eq!(runtime.eating, None, "tick 32 resets the progress");
}

/// Interrupt precedence from `advanceEating` via
/// `TestEatingContainerOpenOnSettlementTickDoesNotSettle`: the container view
/// opening on the settlement tick short-circuits before the settlement
/// branch, so the would-complete hold consumes nothing and pays no hunger.
#[test]
fn interrupt_on_completion() {
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = eating_scene(
        &mut context,
        session,
        12,
        0,
        &[(0, stack(ITEM_BREAD, 2))],
        0,
    );
    let call = eating_call(actor);
    for tick in 1..=31u16 {
        provider::run(&mut context, call).expect("hold");
        assert_eq!(
            held_progress(&context, actor),
            Some(EatingProgress {
                slot: slot(0),
                item: ITEM_BREAD,
                ticks: tick,
            }),
            "fixture self-check: the hold must reach tick 31"
        );
    }
    // The container view opens while the eating input stays pressed, the same
    // moment the Go fixture expresses with `viewContainer = true`.
    context
        .stage(RuleEffect::Viewer {
            session,
            view: Some(ViewLease::new(session, chest_reference())),
        })
        .expect("viewer");
    provider::run(&mut context, call).expect("interrupted tick");
    let view = context.read();
    assert_eq!(
        view.inventory(actor).expect("inventory").slots[0],
        stack(ITEM_BREAD, 2),
        "the interrupt on the settlement tick consumes nothing"
    );
    let actor_record = view.actor(actor).expect("actor");
    assert_eq!(actor_record.survival.hunger(), 12, "no hunger gain");
    let runtime = view.runtime(actor).expect("runtime");
    assert_eq!(runtime.saturation_milli, 0, "no saturation gain");
    assert_eq!(runtime.eating, None, "the interrupt clears the progress");
}

/// Slot-switch restart from `TestEatingSlotSwitchRestartsAndConsumesNeitherSlot`
/// plus the restore leg: switching the selected slot restarts the hold at 1
/// without touching either stack, restoring the original slot restarts from
/// zero (the banked ticks are gone), and the restored hold still settles
/// atomically on its own 32nd tick.
#[test]
fn slot_change_and_restore() {
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cate"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = eating_scene(
        &mut context,
        session,
        12,
        0,
        &[(0, stack(ITEM_BREAD, 2)), (1, stack(ITEM_BREAD, 3))],
        0,
    );
    let call = eating_call(actor);
    for _ in 1..=17u16 {
        provider::run(&mut context, call).expect("hold");
    }
    assert_eq!(
        held_progress(&context, actor),
        Some(EatingProgress {
            slot: slot(0),
            item: ITEM_BREAD,
            ticks: 17,
        }),
        "fixture self-check: 17 banked ticks on slot 0"
    );
    // Switch to the other food slot with the input still held: the changed
    // slot restarts at 1 and neither stack pays.
    context.preload_inventory(
        actor,
        inventory_with(&[(0, stack(ITEM_BREAD, 2)), (1, stack(ITEM_BREAD, 3))], 1),
    );
    provider::run(&mut context, call).expect("switch tick");
    assert_eq!(
        held_progress(&context, actor),
        Some(EatingProgress {
            slot: slot(1),
            item: ITEM_BREAD,
            ticks: 1,
        }),
        "the slot switch restarts the hold at 1"
    );
    let view = context.read();
    let inventory = view.inventory(actor).expect("inventory");
    assert_eq!(inventory.slots[0].count, 2, "the old slot keeps its stack");
    assert_eq!(inventory.slots[1].count, 3, "the new slot keeps its stack");
    // Restore the original slot: the recorded key no longer matches, so the
    // hold restarts from zero instead of resuming the banked ticks.
    context.preload_inventory(
        actor,
        inventory_with(&[(0, stack(ITEM_BREAD, 2)), (1, stack(ITEM_BREAD, 3))], 0),
    );
    provider::run(&mut context, call).expect("restore tick");
    assert_eq!(
        held_progress(&context, actor),
        Some(EatingProgress {
            slot: slot(0),
            item: ITEM_BREAD,
            ticks: 1,
        }),
        "restoring the original slot restarts from zero"
    );
    // Ticks 2..=31 after the restart must not settle: had the banked 17 ticks
    // survived the switch away, tick 31 would already consume.
    for tick in 2..=31u16 {
        provider::run(&mut context, call).expect("restored hold");
        assert_eq!(
            held_progress(&context, actor),
            Some(EatingProgress {
                slot: slot(0),
                item: ITEM_BREAD,
                ticks: tick,
            }),
            "restored hold tick {tick} stays unsettled"
        );
    }
    let view = context.read();
    assert_eq!(
        view.inventory(actor).expect("inventory").slots[0].count,
        2,
        "tick 31 after the restart keeps the stack"
    );
    provider::run(&mut context, call).expect("restored settle");
    let view = context.read();
    assert_eq!(
        view.inventory(actor).expect("inventory").slots[0],
        stack(ITEM_BREAD, 1),
        "the restored hold settles one item on its own 32nd tick"
    );
    assert_eq!(view.actor(actor).expect("actor").survival.hunger(), 17);
    let runtime = view.runtime(actor).expect("runtime");
    assert_eq!(runtime.saturation_milli, 6000);
    assert_eq!(runtime.eating, None);
}

/// Every `FoodValue` row settles exactly, hunger clamps at the ceiling and
/// saturation clamps against the updated hunger, each over the same 32-tick
/// atomic settlement (`TestEatingSettlesExactlyAtEatingTicksWithFixedValues`
/// and `TestFoodValueCoversExactlySevenFoods`).
#[test]
fn all_food_and_clamp() {
    let rows: [(&str, u16, u8, u32, u8, u32); 9] = [
        ("bread", ITEM_BREAD, 10, 0, 15, 6000),
        ("potato", ITEM_POTATO, 10, 0, 11, 600),
        ("carrot", ITEM_CARROT, 10, 0, 13, 3600),
        ("poisonous potato", ITEM_POISONOUS_POTATO, 10, 0, 12, 1200),
        ("rotten flesh", ITEM_ROTTEN_FLESH, 10, 0, 14, 0),
        ("raw beef", ITEM_RAW_BEEF, 10, 0, 13, 1800),
        ("cooked beef", ITEM_COOKED_BEEF, 10, 0, 18, 12800),
        // Hunger 17 plus 5 must clamp at the ceiling, carrying saturation 6000
        // (`core.MaxHunger` clamp row of the Go table test).
        ("bread clamps hunger", ITEM_BREAD, 17, 0, 20, 6000),
        // The saturation bound is the updated hunger: 12000 plus 6000 clamps
        // to 17000, not 18000 (the `min` against the new hunger row).
        (
            "bread clamps saturation",
            ITEM_BREAD,
            12,
            12_000,
            17,
            17_000,
        ),
    ];
    for (name, item, hunger, saturation, want_hunger, want_saturation) in rows {
        let mut state = authority();
        let session = state
            .admit(admitted(4, "Dora"), TransportKind::Memory)
            .expect("session");
        let mut context = harness_context(&mut state);
        let actor = eating_scene(
            &mut context,
            session,
            hunger,
            saturation,
            &[(0, stack(item, 2))],
            0,
        );
        let call = eating_call(actor);
        for tick in 1..=32u16 {
            provider::run(&mut context, call).expect("hold");
            if tick < 32 {
                assert_eq!(
                    view_inventory_count(&context, actor),
                    2,
                    "{name} tick {tick} consumes nothing"
                );
            }
        }
        let view = context.read();
        assert_eq!(
            view.inventory(actor).expect("inventory").slots[0],
            stack(item, 1),
            "{name} settles exactly one item"
        );
        let actor_record = view.actor(actor).expect("actor");
        assert_eq!(
            actor_record.survival.hunger(),
            want_hunger,
            "{name} settles hunger exactly"
        );
        let runtime = view.runtime(actor).expect("runtime");
        assert_eq!(
            runtime.saturation_milli, want_saturation,
            "{name} settles saturation exactly"
        );
        assert_eq!(runtime.eating, None, "{name} resets the progress");
        assert_eq!(
            actor_record.survival.saturation_zero(),
            want_saturation == 0,
            "{name} keeps the zero-saturation hint aligned"
        );
    }
}

fn view_inventory_count(context: &TickContext<'_>, actor: ActorKey) -> u8 {
    context.read().inventory(actor).expect("inventory").slots[0].count
}
