//! Replay cases for the inventory authority provider.
//!
//! Every expected value mirrors a frozen Go oracle row, cited at each case:
//! the 36-slot move primitives from `packages/shared/core/inventory.go`, the
//! half/single amount derivation from `stackSplitAmount` in
//! `packages/server/sim/entity/container.go`, quick-move regions from
//! `packages/server/sim/entity/quick_move.go`, equip and wear from
//! `packages/server/sim/entity/armor.go` plus `packages/shared/core/armor.go`,
//! and the runtime command rows from
//! `packages/server/sim/runtime/inventory_test.go` with the duplicate gate in
//! `packages/server/sim/runtime/engine_step.go`. No case chooses a value the
//! oracle does not pin.

use std::collections::BTreeMap;

use super::*;
use mornlea_domain::{
    Command, CommandEnvelope, CommandEnvelopeParts, HotbarSlot, InventoryMove, PartialMove,
    PlayerId, Season, StackSource, StackView, Weather, WorldStateParts,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    DamageCause, FixtureState, InventoryRecord, RulePhase, SessionKey, SleepState, TransportKind,
    WorkState,
};
use mornlea_server::rules::inventory as provider;
use mornlea_storage::ItemStack;

// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_STONE: u16 = 1;
const ITEM_DIRT: u16 = 2;
const ITEM_GRASS: u16 = 3;
const ITEM_STONE_SWORD: u16 = 48;
const ITEM_IRON_HELMET: u16 = 58;
const ITEM_IRON_CHESTPLATE: u16 = 59;
const ITEM_IRON_LEGGINGS: u16 = 60;
const ITEM_IRON_BOOTS: u16 = 61;

/// Mints one session identity through the real admission path. Session keys
/// are process-local nonzero ids, so a key minted on a throwaway authority is
/// a valid fixture identity for the replay authority.
fn player_session() -> SessionKey {
    let mut bytes = [0u8; 16];
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, "inventory", 8).expect("login start");
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

/// One wearable armor stack: armor pieces never stack above one
/// (`core.ItemStackLimit`, `packages/shared/core/item.go`).
fn armor_stack(item: u16, durability: u16) -> ItemStack {
    ItemStack {
        item,
        count: 1,
        durability,
    }
}

fn empty_armor() -> [ItemStack; 4] {
    [ItemStack::default(); 4]
}

/// A full iron set at each ceiling, the `intactIronArmor` fixture values
/// (`packages/server/sim/entity/armor_test.go`; the ceilings are the armor
/// domain constants in `packages/shared/core/armor.go`).
fn intact_iron_armor() -> [ItemStack; 4] {
    [
        armor_stack(ITEM_IRON_HELMET, 165),
        armor_stack(ITEM_IRON_CHESTPLATE, 240),
        armor_stack(ITEM_IRON_LEGGINGS, 225),
        armor_stack(ITEM_IRON_BOOTS, 195),
    ]
}

/// The zero-durability historical form of the full set, `brokenIronArmor`
/// (`packages/server/sim/entity/armor_test.go`): kept in place, zero points.
fn broken_iron_armor() -> [ItemStack; 4] {
    [
        armor_stack(ITEM_IRON_HELMET, 0),
        armor_stack(ITEM_IRON_CHESTPLATE, 0),
        armor_stack(ITEM_IRON_LEGGINGS, 0),
        armor_stack(ITEM_IRON_BOOTS, 0),
    ]
}

/// Builds one inventory record. Slots and armor hold raw storage triples so a
/// case can stage the zero-durability armor form the domain constructor
/// rejects outside the armor region.
fn record(selected: u8, slots: &[(usize, ItemStack)], armor: [ItemStack; 4]) -> InventoryRecord {
    let mut value = InventoryRecord::empty().with_selected(HotbarSlot::new(selected).unwrap());
    for (index, item) in slots {
        value.slots[*index] = *item;
    }
    value.armor = armor;
    value
}

fn envelope(session: SessionKey, sequence: u64, command: Command) -> CommandEnvelope {
    CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: session.get(),
        sequence,
        arrival_index: 0,
        command,
    })
    .expect("envelope")
}

fn command_call(envelope: &CommandEnvelope) -> RuleCall<'_> {
    RuleCall {
        phase: RulePhase::PlayerCommand,
        actor: None,
        command: Some(envelope),
        internal: None,
    }
}

fn fixture(actor: ActorKey, start: InventoryRecord) -> Fixture {
    Fixture {
        seed: 0,
        initial: FixtureState {
            runtime: Vec::new(),
            actors: Vec::new(),
            chunks: Vec::new(),
            inventories: vec![(actor, start)],
            containers: Vec::new(),
            work: WorkState::default(),
            sleep: SleepState {
                beds: Vec::new(),
                day_phase_offset: 0,
                pending_offset: None,
            },
            projectiles: Vec::new(),
            drops: Vec::new(),
            world: WorldState::try_new(WorldStateParts {
                day_phase_offset: 0,
                world_time_ticks: 0,
                weather: Weather::Clear,
                season: Season::Spring,
                season_progress: 0,
                temperature: 0,
            })
            .expect("world"),
        },
        schedule: Vec::new(),
        expected: Expected {
            events: Vec::new(),
            state_sha256: [0; 32],
            class: None,
            counters: TickCounters::default(),
        },
        budgets: Vec::new(),
    }
}

/// Settles one call over a fixture-held inventory and returns the outcome with
/// the resulting record, so cases can pin exact slot values.
fn settle_call(
    actor: ActorKey,
    start: InventoryRecord,
    call: RuleCall<'_>,
) -> (Result<PhaseReport, ServerError>, InventoryRecord) {
    let mut authority = AuthorityState::try_new(limits(), 0).expect("authority");
    let source = fixture(actor, start);
    let mut context =
        TickContext::from_fixture(&mut authority, &source.initial, TickBudget::full());
    let outcome = provider::run(&mut context, call);
    let after = context
        .read()
        .inventory(actor)
        .copied()
        .expect("inventory remains staged");
    (outcome, after)
}

fn settle(
    actor: ActorKey,
    start: InventoryRecord,
    envelope: &CommandEnvelope,
) -> (Result<PhaseReport, ServerError>, InventoryRecord) {
    settle_call(actor, start, command_call(envelope))
}

/// A no-settlement observation for the same fixture state, the baseline
/// `assert_no_effect` compares a refusal against.
fn baseline_observed(state: &FixtureState) -> Observed {
    Observed {
        events: Vec::new(),
        counters: TickCounters::default(),
        state_sha256: canonical_state_sha256(state),
        class: None,
    }
}

/// The conservation invariant: the multiset of item counts keyed by item and
/// durability form over slots plus armor. A settlement may regroup stacks but
/// never mints or destroys a count.
fn item_totals(value: &InventoryRecord) -> Vec<((u16, u16), u64)> {
    let mut totals: BTreeMap<(u16, u16), u64> = BTreeMap::new();
    for held in value.slots.iter().chain(value.armor.iter()) {
        if held.item != 0 {
            *totals.entry((held.item, held.durability)).or_insert(0) += u64::from(held.count);
        }
    }
    totals.into_iter().collect()
}

#[test]
fn half_5_to_3_remainder_2() {
    let session = player_session();
    let actor = ActorKey::Player(session);

    // Half of 5 is the ceiling half (count + 1) / 2 = 3, leaving 2 behind.
    // The same derivation is pinned for 7 -> 4/3 by
    // `TestStackSplitInventoryHalfDerivesCeilingAmount`
    // (`stack_split_inventory_test.go`); `stackSplitAmount` in
    // `container.go` is the single derivation site.
    let start = record(0, &[(0, stack(ITEM_STONE, 5))], empty_armor());
    let (outcome, after) = settle(
        actor,
        start,
        &envelope(
            session,
            2,
            Command::MovePartial(PartialMove::try_new(StackView::Inventory, 0, 4, false).unwrap()),
        ),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.slots[0], stack(ITEM_STONE, 2));
    assert_eq!(after.slots[4], stack(ITEM_STONE, 3));
    assert_eq!(item_totals(&after), item_totals(&start));

    // The single flag moves exactly one item
    // (`TestStackSplitInventorySingleAndHalfMergeIntoNearlyFull`, single arm).
    let single_start = record(0, &[(0, stack(ITEM_STONE, 5))], empty_armor());
    let (outcome, after) = settle(
        actor,
        single_start,
        &envelope(
            session,
            3,
            Command::MovePartial(PartialMove::try_new(StackView::Inventory, 0, 4, true).unwrap()),
        ),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.slots[0], stack(ITEM_STONE, 4));
    assert_eq!(after.slots[4], stack(ITEM_STONE, 1));
    assert_eq!(item_totals(&after), item_totals(&single_start));

    // A same-item target already at the 64 stack limit cannot absorb: the
    // movable amount is zero and the whole request refuses unchanged
    // (`core.Inventory.MoveStackAmount`, zero-movable refusal).
    let full_start = record(
        0,
        &[(0, stack(ITEM_DIRT, 5)), (4, stack(ITEM_DIRT, 64))],
        empty_armor(),
    );
    let (outcome, after) = settle(
        actor,
        full_start,
        &envelope(
            session,
            4,
            Command::MovePartial(PartialMove::try_new(StackView::Inventory, 0, 4, false).unwrap()),
        ),
    );
    assert!(outcome.is_err());
    assert_eq!(after, full_start);

    // A half into a nearly-full same item truncates to the remaining capacity
    // and keeps the remainder at the source (the nearly-full arm of the same
    // oracle test: 7 into 63/64 moves exactly 1).
    let tight_start = record(
        0,
        &[(0, stack(ITEM_DIRT, 7)), (4, stack(ITEM_DIRT, 63))],
        empty_armor(),
    );
    let (outcome, after) = settle(
        actor,
        tight_start,
        &envelope(
            session,
            5,
            Command::MovePartial(PartialMove::try_new(StackView::Inventory, 0, 4, false).unwrap()),
        ),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.slots[0], stack(ITEM_DIRT, 6));
    assert_eq!(after.slots[4], stack(ITEM_DIRT, 64));
    assert_eq!(item_totals(&after), item_totals(&tight_start));

    // An unlike non-empty target refuses the whole partial move: a partial
    // move never swaps (`TestStackSplitInventoryRejectsDifferentItemTarget`,
    // both the half and single arms).
    for single in [false, true] {
        let unlike_start = record(
            0,
            &[(0, stack(ITEM_STONE, 5)), (4, stack(ITEM_DIRT, 10))],
            empty_armor(),
        );
        let (outcome, after) = settle(
            actor,
            unlike_start,
            &envelope(
                session,
                6,
                Command::MovePartial(
                    PartialMove::try_new(StackView::Inventory, 0, 4, single).unwrap(),
                ),
            ),
        );
        assert!(outcome.is_err());
        assert_eq!(after, unlike_start);
    }

    // An empty source derives amount zero and refuses with zero change
    // (`TestStackSplitInventoryRejectsEmptySource`).
    let empty_source = record(0, &[], empty_armor());
    let (outcome, after) = settle(
        actor,
        empty_source,
        &envelope(
            session,
            7,
            Command::MovePartial(PartialMove::try_new(StackView::Inventory, 0, 4, false).unwrap()),
        ),
    );
    assert!(outcome.is_err());
    assert_eq!(after, empty_source);
}

#[test]
fn armor_swap_and_zero() {
    let session = player_session();
    let actor = ActorKey::Player(session);

    // Empty-slot wear: the selected single helmet stack lands in the
    // piece-derived head slot and the hotbar slot empties
    // (`TestEquipArmorSwapsHotbarAndArmorSlot`, empty-slot arm; the derived
    // slot comes from `core.ArmorSlotOf`).
    let start = record(0, &[(0, armor_stack(ITEM_IRON_HELMET, 165))], empty_armor());
    let (outcome, after) = settle(actor, start, &envelope(session, 7, Command::EquipArmor));
    assert!(outcome.is_ok());
    assert_eq!(after.armor[0], armor_stack(ITEM_IRON_HELMET, 165));
    assert_eq!(after.slots[0], ItemStack::default());
    assert_eq!(item_totals(&after), item_totals(&start));

    // Occupied swap: the worn piece returns to the selected hotbar slot as
    // the raw storage triple (occupied-swap arm of the same oracle test).
    let worn_first = armor_stack(ITEM_IRON_HELMET, 100);
    let swap_start = record(
        0,
        &[(0, armor_stack(ITEM_IRON_HELMET, 165))],
        [
            worn_first,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
        ],
    );
    let (outcome, after) = settle(
        actor,
        swap_start,
        &envelope(session, 8, Command::EquipArmor),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.armor[0], armor_stack(ITEM_IRON_HELMET, 165));
    assert_eq!(after.slots[0], worn_first);
    assert_eq!(item_totals(&after), item_totals(&swap_start));

    // Only the derived slot changes: equipping a chestplate leaves the worn
    // helmet untouched and the points projection reads 2 + 6
    // (cross-slot arm; point values are the `armorPieceOf` table).
    let cross_start = record(
        0,
        &[(0, armor_stack(ITEM_IRON_CHESTPLATE, 240))],
        [
            worn_first,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
        ],
    );
    let (outcome, after) = settle(
        actor,
        cross_start,
        &envelope(session, 9, Command::EquipArmor),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.armor[1], armor_stack(ITEM_IRON_CHESTPLATE, 240));
    assert_eq!(after.armor[0], worn_first);
    assert_eq!(after.slots[0], ItemStack::default());
    assert_eq!(provider::armor_points(&after.armor), 8);

    // A non-armor selected stack refuses with every slot and armor cell
    // unchanged (`RejectNotArmor`, non-armor arm of the same oracle test).
    let refused = record(
        0,
        &[(
            0,
            ItemStack {
                item: ITEM_STONE_SWORD,
                count: 1,
                durability: 131,
            },
        )],
        intact_iron_armor(),
    );
    let (outcome, after) = settle(actor, refused, &envelope(session, 10, Command::EquipArmor));
    assert!(outcome.is_err());
    assert_eq!(after, refused);

    // The zero-durability historical form is wearable, stays in place and
    // contributes zero points
    // (`TestEquipBrokenArmorFormIsWearable`; `armorRestoreValid` admits
    // durability zero only in the armor region).
    let zero_start = record(0, &[(0, armor_stack(ITEM_IRON_HELMET, 0))], empty_armor());
    let (outcome, after) = settle(
        actor,
        zero_start,
        &envelope(session, 11, Command::EquipArmor),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.armor[0], armor_stack(ITEM_IRON_HELMET, 0));
    assert_eq!(after.slots[0], ItemStack::default());
    assert_eq!(provider::armor_points(&after.armor), 0);

    // Physical damage over a full intact set: 15 points turn raw 3 into
    // max(1, 3 * (100 - 60) / 100) = 1 and every piece wears exactly one
    // (`TestHostileMeleeReducedByArmor`, `TestPvPMeleeReducedByArmor`;
    // formula is `core.ReducedDamage`).
    let mut melee_worn = intact_iron_armor();
    let points = provider::armor_points(&melee_worn);
    assert_eq!(points, 15);
    let effective = provider::settle_damage(DamageCause::Melee, 3, points, &mut melee_worn);
    assert_eq!(effective, 1);
    assert_eq!(melee_worn[0].durability, 164);
    assert_eq!(melee_worn[1].durability, 239);
    assert_eq!(melee_worn[2].durability, 224);
    assert_eq!(melee_worn[3].durability, 194);

    // The projectile cause shares the melee reduction and wear gate
    // (`settleProjectileEntityHit` in
    // `packages/server/sim/entity/projectile.go`).
    let mut shot_worn = intact_iron_armor();
    let effective = provider::settle_damage(DamageCause::Projectile, 5, 15, &mut shot_worn);
    assert_eq!(effective, 2);
    assert_eq!(shot_worn[0].durability, 164);

    // A hit held up by the floor of one reduces nothing and wears nothing
    // (`TestArmorDurabilityConsumesOnlyOnReduction`, floor arm).
    let mut floored = intact_iron_armor();
    let effective = provider::settle_damage(DamageCause::Melee, 1, 15, &mut floored);
    assert_eq!(effective, 1);
    assert_eq!(floored, intact_iron_armor());

    // A zero-point set takes raw damage and stays unworn (broken-form arm of
    // the same oracle test).
    let mut broken = broken_iron_armor();
    assert_eq!(provider::armor_points(&broken), 0);
    let effective = provider::settle_damage(DamageCause::Melee, 3, 0, &mut broken);
    assert_eq!(effective, 3);
    assert_eq!(broken, broken_iron_armor());

    // Durability one becomes the in-place zero form on an actual reduction
    // (durability-one arm of the same oracle test).
    let mut last_point = [ItemStack::default(); 4];
    last_point[0] = armor_stack(ITEM_IRON_HELMET, 1);
    let effective = provider::settle_damage(DamageCause::Melee, 5, 2, &mut last_point);
    assert_eq!(effective, 4);
    assert_eq!(last_point[0], armor_stack(ITEM_IRON_HELMET, 0));

    // Fall damage bypasses armor: the fall cause keeps the raw damage and
    // never wears (`TestFallDamageNotReduced`; the Go fall path calls
    // `applyFallDamage` without the reduction settle point).
    let mut fallen = intact_iron_armor();
    let effective = provider::settle_damage(DamageCause::Fall, 3, 15, &mut fallen);
    assert_eq!(effective, 3);
    assert_eq!(fallen, intact_iron_armor());

    // The four piece point values 2/6/5/2 through the frozen formula
    // `max(1, damage * (100 - 4 * points) / 100)` with damage 10: 92%, 76%,
    // 80% and 92% of ten truncate to 9/7/8/9 (`armorPieceOf`,
    // `core.ReducedDamage`).
    for (points, expected) in [(2u8, 9i32), (6, 7), (5, 8), (2, 9)] {
        assert_eq!(provider::reduced_damage(10, points), expected);
    }
    // Every point combination keeps at least one damage
    // (`core.ReducedDamage` floor).
    assert_eq!(provider::reduced_damage(1, 15), 1);
}

#[test]
fn quick_regions_and_no_fit() {
    let session = player_session();
    let actor = ActorKey::Player(session);

    // Hotbar to backpack: same-item merges in ascending slot order run before
    // empty slots, the remainder lands in the first empty backpack slot and
    // hotbar slots never absorb a hotbar source
    // (`TestQuickMoveInventoryHotbarToBackpack`, merge-before-empty arm; the
    // region order is the region-restricted projection of the four-phase
    // `core.Inventory.AddStack` credit order).
    let start = record(
        0,
        &[
            (0, stack(ITEM_STONE, 64)),
            (2, stack(ITEM_DIRT, 5)),
            (9, stack(ITEM_STONE, 63)),
            (10, stack(ITEM_DIRT, 10)),
        ],
        empty_armor(),
    );
    let (outcome, after) = settle(
        actor,
        start,
        &envelope(
            session,
            2,
            Command::QuickMove(StackSource::try_new(StackView::Inventory, 0).unwrap()),
        ),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.slots[9], stack(ITEM_STONE, 64));
    assert_eq!(after.slots[11], stack(ITEM_STONE, 63));
    assert_eq!(after.slots[0], ItemStack::default());
    assert_eq!(after.slots[1], ItemStack::default());
    assert_eq!(after.slots[2], stack(ITEM_DIRT, 5));
    assert_eq!(after.slots[10], stack(ITEM_DIRT, 10));
    assert_eq!(item_totals(&after), item_totals(&start));

    // Partial absorption keeps the remainder at the source instead of
    // refusing (partial-absorption arm of the same oracle test).
    let mut partial_slots: Vec<(usize, ItemStack)> =
        vec![(0, stack(ITEM_STONE, 64)), (9, stack(ITEM_STONE, 62))];
    for slot in 1..36 {
        if slot != 9 {
            partial_slots.push((slot, stack(ITEM_DIRT, 64)));
        }
    }
    let partial_start = record(0, &partial_slots, empty_armor());
    let (outcome, after) = settle(
        actor,
        partial_start,
        &envelope(
            session,
            3,
            Command::QuickMove(StackSource::try_new(StackView::Inventory, 0).unwrap()),
        ),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.slots[9], stack(ITEM_STONE, 64));
    assert_eq!(after.slots[0], stack(ITEM_STONE, 62));
    assert_eq!(item_totals(&after), item_totals(&partial_start));

    // Backpack to hotbar: the same region order runs in reverse, and backpack
    // slots never absorb a backpack source
    // (`TestQuickMoveInventoryBackpackToHotbar`).
    let reverse_start = record(
        0,
        &[
            (0, stack(ITEM_STONE, 30)),
            (1, stack(ITEM_DIRT, 3)),
            (9, stack(ITEM_STONE, 64)),
        ],
        empty_armor(),
    );
    let (outcome, after) = settle(
        actor,
        reverse_start,
        &envelope(
            session,
            4,
            Command::QuickMove(StackSource::try_new(StackView::Inventory, 9).unwrap()),
        ),
    );
    assert!(outcome.is_ok());
    assert_eq!(after.slots[0], stack(ITEM_STONE, 64));
    assert_eq!(after.slots[2], stack(ITEM_STONE, 30));
    assert_eq!(after.slots[9], ItemStack::default());
    assert_eq!(after.slots[10], ItemStack::default());
    assert_eq!(item_totals(&after), item_totals(&reverse_start));

    // An opposite region with no fit refuses the whole request unchanged and
    // mints no count (`TestQuickMoveInventoryRejectsWhenNoFit`).
    let mut blocked_slots: Vec<(usize, ItemStack)> = vec![(0, stack(ITEM_STONE, 64))];
    for slot in 1..36 {
        blocked_slots.push((slot, stack(ITEM_DIRT, 64)));
    }
    let blocked_start = record(0, &blocked_slots, empty_armor());
    let (outcome, after) = settle(
        actor,
        blocked_start,
        &envelope(
            session,
            5,
            Command::QuickMove(StackSource::try_new(StackView::Inventory, 0).unwrap()),
        ),
    );
    assert!(outcome.is_err());
    assert_eq!(after, blocked_start);
    assert_eq!(item_totals(&after), item_totals(&blocked_start));

    // The refused request also leaves the canonical replay hash untouched.
    let blocked_fixture = fixture(actor, blocked_start);
    let before = baseline_observed(&blocked_fixture.initial);
    let observed = run_phase(
        &blocked_fixture,
        provider::run,
        command_call(&envelope(
            session,
            6,
            Command::QuickMove(StackSource::try_new(StackView::Inventory, 0).unwrap()),
        )),
        TickBudget::full(),
    );
    assert_no_effect(&before, &observed, RuleReject::StaleObservation);

    // An empty source quick move refuses unchanged
    // (`TestQuickMoveInventoryRejectsEmptySource`).
    let empty_source = record(0, &[], empty_armor());
    let (outcome, after) = settle(
        actor,
        empty_source,
        &envelope(
            session,
            7,
            Command::QuickMove(StackSource::try_new(StackView::Inventory, 0).unwrap()),
        ),
    );
    assert!(outcome.is_err());
    assert_eq!(after, empty_source);
}

#[test]
fn conservation_and_duplicate_command() {
    let session = player_session();
    let actor = ActorKey::Player(session);

    // Every successful path conserves the item multiset across slots and
    // armor. The whole-move rows are the runtime oracle
    // `TestMoveStackIntoBackpackPublishesOnce` and
    // `TestMoveStackMergesAndSwaps` (`runtime/inventory_test.go`) over the
    // `core.Inventory.MoveStack` primitive.
    let cases: Vec<(InventoryRecord, Command)> = vec![
        (
            record(0, &[(1, stack(ITEM_STONE, 7))], empty_armor()),
            Command::MoveInventory(InventoryMove::try_new(1, 13).unwrap()),
        ),
        (
            record(
                0,
                &[(0, stack(ITEM_STONE, 10)), (9, stack(ITEM_STONE, 60))],
                empty_armor(),
            ),
            Command::MoveInventory(InventoryMove::try_new(0, 9).unwrap()),
        ),
        (
            record(
                0,
                &[(2, stack(ITEM_GRASS, 3)), (10, stack(ITEM_DIRT, 4))],
                empty_armor(),
            ),
            Command::MoveInventory(InventoryMove::try_new(2, 10).unwrap()),
        ),
        (
            record(0, &[(0, stack(ITEM_STONE, 5))], empty_armor()),
            Command::MovePartial(PartialMove::try_new(StackView::Inventory, 0, 4, false).unwrap()),
        ),
        (
            record(
                0,
                &[(0, stack(ITEM_STONE, 64)), (9, stack(ITEM_STONE, 63))],
                empty_armor(),
            ),
            Command::QuickMove(StackSource::try_new(StackView::Inventory, 0).unwrap()),
        ),
        (
            record(0, &[(0, armor_stack(ITEM_IRON_BOOTS, 195))], empty_armor()),
            Command::EquipArmor,
        ),
    ];
    for (index, (start, command)) in cases.iter().enumerate() {
        let (outcome, after) = settle(actor, *start, &envelope(session, 20, *command));
        assert!(outcome.is_ok(), "case {index} must settle");
        assert_ne!(&after, start, "case {index} must change the record");
        assert_eq!(
            item_totals(&after),
            item_totals(start),
            "case {index} must conserve the item multiset"
        );
    }

    // A select that changes the hotbar stages one inventory effect the
    // canonical replay hash observes, and the expected hash is minted from
    // the fixture state carrying the selected slot (`CommandSelectHotbar`
    // settlement in `packages/server/sim/entity/tick.go`).
    let start = record(0, &[(0, stack(ITEM_STONE, 5))], empty_armor());
    let selected = start.with_selected(HotbarSlot::new(4).unwrap());
    let mut expected_state = fixture(actor, start).initial;
    expected_state.inventories[0].1 = selected;
    let expected = Expected {
        events: Vec::new(),
        state_sha256: canonical_state_sha256(&expected_state),
        class: None,
        counters: TickCounters::default(),
    };
    let observed = run_phase(
        &fixture(actor, start),
        provider::run,
        command_call(&envelope(
            session,
            21,
            Command::SelectHotbar(HotbarSlot::new(4).unwrap()),
        )),
        TickBudget::full(),
    );
    assert_expected(&observed, &expected);
    let (_, after) = settle(
        actor,
        start,
        &envelope(
            session,
            22,
            Command::SelectHotbar(HotbarSlot::new(4).unwrap()),
        ),
    );
    assert_eq!(after, selected);

    // A duplicate select of the already-selected slot settles idempotently:
    // the Go settlement writes and publishes nothing when the slot is
    // unchanged, and never rejects (`tick.go` select settlement guards the
    // write with `Selected != Slot`).
    let (outcome, after) = settle(
        actor,
        start,
        &envelope(
            session,
            23,
            Command::SelectHotbar(HotbarSlot::new(0).unwrap()),
        ),
    );
    assert!(outcome.is_ok());
    assert_eq!(after, start);

    // A duplicate whole move never double-applies: duplicate delivery is
    // dropped by the ordering layer before any provider sees it
    // (`engine_step.go` silently skips `Sequence <= lastSequence`), and the
    // provider-visible remainder of that policy is the source-empty refusal
    // of `core.Inventory.MoveStack` — replaying the identical envelope after
    // the first settlement refuses with the record unchanged.
    let moved_once = record(1, &[(1, stack(ITEM_STONE, 7))], empty_armor());
    let duplicate = envelope(
        session,
        24,
        Command::MoveInventory(InventoryMove::try_new(1, 13).unwrap()),
    );
    let (first, once) = settle(actor, moved_once, &duplicate);
    assert!(first.is_ok());
    assert_eq!(once.slots[13], stack(ITEM_STONE, 7));
    assert_eq!(once.slots[1], ItemStack::default());
    let (second, twice) = settle(actor, once, &duplicate);
    assert!(second.is_err());
    assert_eq!(twice, once);

    // The provider owns only the player-command phase and requires a command:
    // any other call shape refuses without effect.
    let shape_start = record(0, &[(0, stack(ITEM_STONE, 5))], empty_armor());
    let shape_envelope = envelope(
        session,
        25,
        Command::MoveInventory(InventoryMove::try_new(0, 4).unwrap()),
    );
    let wrong_phase = RuleCall {
        phase: RulePhase::CompanionIntent,
        actor: None,
        command: Some(&shape_envelope),
        internal: None,
    };
    let (outcome, after) = settle_call(actor, shape_start, wrong_phase);
    assert!(outcome.is_err());
    assert_eq!(after, shape_start);
    let missing_command = RuleCall {
        phase: RulePhase::PlayerCommand,
        actor: Some(actor),
        command: None,
        internal: None,
    };
    let (outcome, after) = settle_call(actor, shape_start, missing_command);
    assert!(outcome.is_err());
    assert_eq!(after, shape_start);

    // Crafting and container stack views belong to the crafting and container
    // providers; this provider refuses them without effect (routing table:
    // partial and quick moves split by view family).
    for command in [
        Command::MovePartial(PartialMove::try_new(StackView::Crafting, 0, 4, false).unwrap()),
        Command::QuickMove(StackSource::try_new(StackView::Crafting, 0).unwrap()),
    ] {
        let (outcome, after) = settle(actor, shape_start, &envelope(session, 26, command));
        assert!(outcome.is_err());
        assert_eq!(after, shape_start);
    }
}
