//! Replay cases for the tick-driven furnace provider.
//!
//! Every expected value mirrors a frozen Go oracle row, cited at each case:
//! the ignition and completion rows in `advanceFurnace` plus the `canSmelt`
//! pause rows in `packages/server/sim/entity/furnace.go`, the burn and smelt
//! timers `FurnaceBurnTicks` 1600 and `FurnaceSmeltTicks` 200 in
//! `packages/shared/core/furnace.go`, the smelting rows 6->7, 18->23, 27->24
//! and 53->54 in `packages/shared/core/smelting.go`, the runtime rows in
//! `packages/server/sim/runtime/furnace_test.go`
//! (`TestFurnaceMaterialsLightFuelAndAdvanceSameTick`,
//! `TestFurnaceProducesMaterialsAtTwoHundredTicks`,
//! `TestOneCoalYieldsExactlyEightIngots`,
//! `TestFurnaceMaterialsPauseWithoutWastingFuel`,
//! `TestFurnaceMaterialsResumeFromStoredValues`,
//! `TestFurnaceAdvancesOnceWithOverlappingViewers`,
//! `TestFurnaceChunkRevisionRisesOnce`), the restart row
//! `TestFurnaceRestartRestoresTimersWithoutCatchUp` in
//! `packages/server/server/furnace_publication_test.go`, and the burn-1400
//! checkpoint in `packages/server/server/material_processing_integration_test.go`.
//! No case chooses a value the oracle does not pin.

use super::*;
use mornlea_domain::{ChunkPos, ContainerKind, ContainerRef, PlayerId};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ContainerRecord, ContainerSlots, EnvironmentState, RuleEffect, RulePhase, RuleTunables,
    SessionKey, TransportKind,
};
use mornlea_server::rules::furnaces as provider;
use mornlea_storage::ItemStack;

// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_STONE: u16 = 1; // `core.ItemStone`
const ITEM_COAL: u16 = 5; // `core.ItemCoal`
const ITEM_RAW_IRON: u16 = 6; // `core.ItemRawIron`
const ITEM_IRON_INGOT: u16 = 7; // `core.ItemIronIngot`
const ITEM_GLASS: u16 = 23; // `core.ItemGlass`

// The frozen furnace timers (`core.FurnaceBurnTicks`, `core.FurnaceSmeltTicks`
// in `packages/shared/core/furnace.go`).
const FURNACE_BURN_TICKS: u16 = 1600;
const FURNACE_SMELT_TICKS: u32 = 200;

/// Mints one session identity through the real admission path. Session keys
/// are process-local nonzero ids, so a key minted on a throwaway authority is
/// a valid fixture identity for the replay authority.
fn player_session(tag: u8, name: &str) -> SessionKey {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    let login = admit_login(inbound).expect("admitted");
    let mut mint = AuthorityState::try_new(limits(), 0).expect("authority");
    mint.admit(login, TransportKind::Memory).expect("session")
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}

fn empty() -> ItemStack {
    ItemStack::default()
}

fn furnace_ref(slot: u8, generation: u32) -> ContainerRef {
    ContainerRef::try_new(
        ChunkPos::new(0, 0),
        ContainerKind::Furnace,
        slot,
        generation,
    )
    .expect("furnace reference")
}

fn chest_ref(generation: u32) -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, generation)
        .expect("chest reference")
}

fn furnace_record(
    slot: u8,
    generation: u32,
    input: ItemStack,
    fuel_cell: ItemStack,
    output: ItemStack,
    burn: u32,
    progress: u32,
) -> ContainerRecord {
    ContainerRecord {
        reference: furnace_ref(slot, generation),
        revision: 1,
        slots: ContainerSlots::Furnace {
            slots: [input, fuel_cell, output],
            fuel: burn,
            progress,
        },
    }
}

/// Destructures one staged record into its three cells and the two timers.
fn furnace_parts(record: &ContainerRecord) -> ([ItemStack; 3], u32, u32) {
    match &record.slots {
        ContainerSlots::Furnace {
            slots,
            fuel,
            progress,
        } => (*slots, *fuel, *progress),
        ContainerSlots::Chest(_) => panic!("chest record in a furnace case"),
    }
}

/// The one batch call shape the provider owns: the phase alone, with no
/// actor, command or internal payload.
fn batch_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::FurnaceStep,
        actor: None,
        command: None,
        internal: None,
    }
}

/// The frozen ignition and completion rows. The first tick after lighting
/// consumes the coal, sets the burn to 1600 and then applies the same-tick
/// burn-minus-one and progress-plus-one, so it reads burn 1599 with progress 1
/// and an untouched input (`TestFurnaceMaterialsLightFuelAndAdvanceSameTick`).
/// The 200th tick resets the progress, consumes one input and mints one
/// product in that same tick, leaving burn 1400
/// (`TestFurnaceProducesMaterialsAtTwoHundredTicks` and the burn-1400
/// checkpoint in `material_processing_integration_test.go`).
#[test]
fn fuel_1600_smelt_200() {
    let reference = furnace_ref(0, 1);
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    context.preload_container(furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 4),
        stack(ITEM_COAL, 1),
        empty(),
        0,
        0,
    ));

    let report = provider::advance(&mut context, &[reference]).expect("first tick");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 1);
    let (slots, burn, progress) =
        furnace_parts(&context.read().container(reference).expect("staged"));
    assert_eq!(
        slots[0],
        stack(ITEM_RAW_IRON, 4),
        "ignition keeps the input"
    );
    assert_eq!(slots[1], empty(), "one coal is consumed");
    assert_eq!(slots[2], empty());
    assert_eq!(burn, 1599);
    assert_eq!(progress, 1);
    assert!(context.events().is_empty());

    for _ in 1..FURNACE_SMELT_TICKS {
        let tick = provider::advance(&mut context, &[reference]).expect("tick");
        assert_eq!(tick.applied, 1);
    }
    let (slots, burn, progress) =
        furnace_parts(&context.read().container(reference).expect("staged"));
    assert_eq!(
        slots[0],
        stack(ITEM_RAW_IRON, 3),
        "completion consumes one input"
    );
    assert_eq!(slots[1], empty());
    assert_eq!(slots[2], stack(ITEM_IRON_INGOT, 1));
    assert_eq!(burn, 1400);
    assert_eq!(progress, 0);
}

/// The pause and resume rows. A full output, an output holding a different
/// product and an input with no smelting row all freeze both timers with
/// nothing consumed (`TestFurnaceMaterialsPauseWithoutWastingFuel`). Freeing
/// the output resumes exactly from the stored 17/199 timers, and a
/// context-restored record advances from its stored values without any
/// wall-clock catch-up (`TestFurnaceMaterialsResumeFromStoredValues`,
/// `TestFurnaceRestartRestoresTimersWithoutCatchUp`).
#[test]
fn full_output_pauses_both() {
    // Full same-product output at burn 17 / progress 199: both unchanged.
    let reference = furnace_ref(0, 1);
    let paused = furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 2),
        stack(ITEM_COAL, 1),
        stack(ITEM_IRON_INGOT, 64),
        17,
        199,
    );
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    context.preload_container(paused.clone());
    let report = provider::advance(&mut context, &[reference]).expect("paused tick");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 0);
    assert_eq!(
        context.read().container(reference).expect("staged"),
        paused,
        "a full output freezes both timers"
    );
    assert!(context.events().is_empty());

    // Freeing the output resumes exactly: the stored timers advance to 16 and
    // the completion at 200 in one tick.
    context.preload_container(furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 2),
        stack(ITEM_COAL, 1),
        empty(),
        17,
        199,
    ));
    let report = provider::advance(&mut context, &[reference]).expect("resumed tick");
    assert_eq!(report.applied, 1);
    let (slots, burn, progress) =
        furnace_parts(&context.read().container(reference).expect("staged"));
    assert_eq!(slots[0], stack(ITEM_RAW_IRON, 1));
    assert_eq!(slots[1], stack(ITEM_COAL, 1));
    assert_eq!(slots[2], stack(ITEM_IRON_INGOT, 1));
    assert_eq!(burn, 16);
    assert_eq!(progress, 0);

    // A conflicting output and an unusable input pause the same way in one
    // batch: the burn stays frozen at 17 and the 137/1463 pair survives.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    let conflict = furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 2),
        stack(ITEM_COAL, 1),
        stack(ITEM_GLASS, 1),
        17,
        199,
    );
    let unusable = furnace_record(
        1,
        1,
        stack(ITEM_STONE, 2),
        stack(ITEM_COAL, 1),
        empty(),
        1463,
        137,
    );
    context.preload_container(conflict.clone());
    context.preload_container(unusable.clone());
    let report = provider::advance(&mut context, &[furnace_ref(0, 1), furnace_ref(1, 1)])
        .expect("paused batch");
    assert_eq!(report.examined, 2);
    assert_eq!(report.applied, 0);
    assert_eq!(
        context.read().container(furnace_ref(0, 1)).expect("staged"),
        conflict
    );
    assert_eq!(
        context.read().container(furnace_ref(1, 1)).expect("staged"),
        unusable
    );

    // A context-restored record resumes from the stored 137/1463 pair without
    // completing and without any catch-up.
    let restored = furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 4),
        stack(ITEM_COAL, 1),
        empty(),
        1463,
        137,
    );
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    context.preload_container(restored);
    let report = provider::advance(&mut context, &[furnace_ref(0, 1)]).expect("restored tick");
    assert_eq!(report.applied, 1);
    let (slots, burn, progress) =
        furnace_parts(&context.read().container(furnace_ref(0, 1)).expect("staged"));
    assert_eq!(slots[0], stack(ITEM_RAW_IRON, 4));
    assert_eq!(slots[2], empty());
    assert_eq!(burn, 1462);
    assert_eq!(progress, 138);
}

/// The shape and interest gates. Every non-batch call shape is refused with
/// nothing staged, a bare batch call reports zero work because the serial
/// reducer owns the interest enumeration, duplicate references advance one
/// furnace exactly once (`TestFurnaceAdvancesOnceWithOverlappingViewers`),
/// chunk-mates advance together in one call
/// (`TestFurnaceChunkRevisionRisesOnce`), a reference with no staged record
/// pauses without effect, and a chest reference in the interest set is a hard
/// shape error that stages nothing.
#[test]
fn shape_and_interest_gates() {
    let reference = furnace_ref(0, 1);
    let ignition = furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 4),
        stack(ITEM_COAL, 1),
        empty(),
        0,
        0,
    );

    // The wrong phase, and the batch shape carrying a payload, both refuse
    // without touching the staged record.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    context.preload_container(ignition.clone());
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::ContainerMove,
                actor: None,
                command: None,
                internal: None,
            },
        )
        .is_err()
    );
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::FurnaceStep,
                actor: Some(ActorKey::Player(player_session(3, "furnace-shape"))),
                command: None,
                internal: None,
            },
        )
        .is_err()
    );
    assert_eq!(
        context.read().container(reference).expect("staged"),
        ignition,
        "refused shapes stage nothing"
    );

    // A bare batch call is the frozen phase entry; the interest enumeration
    // belongs to the serial reducer, so it reports zero work and stages
    // nothing.
    let report = provider::run(&mut context, batch_call()).expect("batch entry");
    assert_eq!(report.examined, 0);
    assert_eq!(report.applied, 0);
    assert_eq!(
        context.read().container(reference).expect("staged"),
        ignition
    );

    // Duplicate references advance one furnace exactly once.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    context.preload_container(ignition.clone());
    let report = provider::advance(&mut context, &[reference, reference]).expect("batch");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 1);
    let (_, _, progress) = furnace_parts(&context.read().container(reference).expect("staged"));
    assert_eq!(progress, 1, "overlapping interest advances once");

    // Two chunk-mates advance together in one call.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    context.preload_container(furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 4),
        stack(ITEM_COAL, 1),
        empty(),
        0,
        0,
    ));
    context.preload_container(furnace_record(
        1,
        1,
        stack(ITEM_RAW_IRON, 4),
        stack(ITEM_COAL, 1),
        empty(),
        0,
        0,
    ));
    let report =
        provider::advance(&mut context, &[furnace_ref(0, 1), furnace_ref(1, 1)]).expect("batch");
    assert_eq!(report.examined, 2);
    assert_eq!(report.applied, 2);
    for slot in [0u8, 1] {
        let (_, _, progress) = furnace_parts(
            &context
                .read()
                .container(furnace_ref(slot, 1))
                .expect("staged"),
        );
        assert_eq!(progress, 1);
    }

    // A reference with no staged record is interest churn: it counts as
    // examined, stages nothing and pauses the absent record without effect.
    let missing = furnace_ref(2, 1);
    let report = provider::advance(&mut context, &[missing, furnace_ref(0, 1)]).expect("batch");
    assert_eq!(report.examined, 2);
    assert_eq!(report.applied, 1);
    assert!(context.read().container(missing).is_none());

    // A chest reference in the furnace interest set is a hard shape error
    // with nothing staged.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    context.preload_container(ignition.clone());
    context.preload_container(ContainerRecord {
        reference: chest_ref(1),
        revision: 1,
        slots: ContainerSlots::Chest([ItemStack::default(); 27]),
    });
    assert!(provider::advance(&mut context, &[chest_ref(1), reference]).is_err());
    assert_eq!(
        context.read().container(reference).expect("staged"),
        ignition,
        "the refused batch stages nothing"
    );
    assert!(context.events().is_empty());
}

/// One coal burns exactly 1600 ticks and yields exactly eight ingots, then
/// the furnace pauses with the fuel cell empty and does not relight
/// (`TestOneCoalYieldsExactlyEightIngots`).
#[test]
fn one_coal_yields_eight_ingots() {
    let reference = furnace_ref(0, 1);
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = furnace_context(&mut state);
    context.preload_container(furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 64),
        stack(ITEM_COAL, 1),
        empty(),
        0,
        0,
    ));

    for _ in 0..u32::from(FURNACE_BURN_TICKS) {
        provider::advance(&mut context, &[reference]).expect("tick");
    }
    let (slots, burn, progress) =
        furnace_parts(&context.read().container(reference).expect("staged"));
    assert_eq!(slots[0], stack(ITEM_RAW_IRON, 56));
    assert_eq!(slots[1], empty());
    assert_eq!(slots[2], stack(ITEM_IRON_INGOT, 8));
    assert_eq!(burn, 0);
    assert_eq!(progress, 0);

    provider::advance(&mut context, &[reference]).expect("tick after exhaustion");
    let (slots, burn, progress) =
        furnace_parts(&context.read().container(reference).expect("staged"));
    assert_eq!(slots[0], stack(ITEM_RAW_IRON, 56), "no input without fuel");
    assert_eq!(
        slots[2],
        stack(ITEM_IRON_INGOT, 8),
        "no relight without fuel"
    );
    assert_eq!(burn, 0);
    assert_eq!(progress, 0);
}

fn furnace_environment(burn: u16, smelt: u8) -> EnvironmentState {
    let defaults = RuleTunables::source_defaults();
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: mornlea_domain::Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::try_new(
            defaults.physics(),
            100,
            40,
            20,
            80,
            18,
            4000,
            32,
            burn,
            smelt,
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

fn furnace_context(state: &mut AuthorityState) -> TickContext<'_> {
    let mut context = TickContext::harness(state, TickBudget::full());
    context
        .stage(RuleEffect::Environment(furnace_environment(1600, 200)))
        .unwrap();
    context
}

#[test]
fn configured_furnace_timing_uses_current_batch_snapshot() {
    let mut state = AuthorityState::try_new(limits(), 7).unwrap();
    let mut context = furnace_context(&mut state);
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 4),
        stack(ITEM_COAL, 1),
        empty(),
        0,
        0,
    ));
    context
        .stage(RuleEffect::Environment(furnace_environment(9, 3)))
        .unwrap();
    let snapshot = context.read().environment().unwrap().clone();
    for (burn, progress, inputs, outputs) in [(8, 1, 4, 0), (7, 2, 4, 0), (6, 0, 3, 1)] {
        assert_eq!(
            provider::advance(&mut context, &[reference])
                .unwrap()
                .applied,
            1
        );
        let (slots, actual_burn, actual_progress) =
            furnace_parts(&context.read().container(reference).unwrap());
        assert_eq!(
            (actual_burn, actual_progress, slots[0].count, slots[2].count),
            (burn, progress, inputs, outputs)
        );
        assert_eq!(slots[1], empty());
        assert_eq!(context.read().environment(), Some(&snapshot));
    }
    context.preload_container(furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 4),
        empty(),
        empty(),
        20,
        5,
    ));
    context
        .stage(RuleEffect::Environment(furnace_environment(9, 2)))
        .unwrap();
    provider::advance(&mut context, &[reference]).unwrap();
    let (slots, burn, progress) = furnace_parts(&context.read().container(reference).unwrap());
    assert_eq!(
        (burn, progress, slots[0].count, slots[2].count),
        (19, 0, 3, 1)
    );
    assert!(context.events().is_empty());
}

#[test]
fn configured_furnace_zero_timing_normalizes_at_consumption() {
    let mut state = AuthorityState::try_new(limits(), 7).unwrap();
    let mut context = furnace_context(&mut state);
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 4),
        stack(ITEM_COAL, 1),
        empty(),
        0,
        0,
    ));
    context
        .stage(RuleEffect::Environment(furnace_environment(0, 0)))
        .unwrap();
    provider::advance(&mut context, &[reference]).unwrap();
    let (slots, burn, progress) = furnace_parts(&context.read().container(reference).unwrap());
    assert_eq!(
        (burn, progress, slots[0].count, slots[2].count),
        (0, 0, 3, 1)
    );
    assert_eq!(slots[1], empty());
}

#[test]
fn active_furnace_missing_snapshot_refuses_before_material_mutation() {
    let mut state = AuthorityState::try_new(limits(), 7).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let reference = furnace_ref(0, 1);
    assert_eq!(provider::advance(&mut context, &[]).unwrap().applied, 0);
    assert_eq!(
        provider::advance(&mut context, &[reference])
            .unwrap()
            .applied,
        0
    );
    let before = furnace_record(
        0,
        1,
        stack(ITEM_RAW_IRON, 4),
        stack(ITEM_COAL, 1),
        empty(),
        0,
        0,
    );
    context.preload_container(before.clone());
    assert_eq!(
        provider::advance(&mut context, &[reference]),
        Err(ServerError::Internal {
            invariant: "furnace snapshot"
        })
    );
    assert_eq!(context.read().container(reference), Some(before));
    assert!(context.changed_blocks().is_empty());
    assert!(context.events().is_empty());
}
