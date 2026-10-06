//! Inventory authority: hotbar selection, whole and partial stack moves,
//! quick-move region credit, armor equip and armor durability settlement.
//!
//! Every table and branch mirrors a frozen Go row, cited at each site: the
//! 36-slot move primitives from `packages/shared/core/inventory.go`, the
//! armor piece table from `packages/shared/core/armor.go`, equip and wear
//! from `packages/server/sim/entity/armor.go`, quick-move regions from
//! `packages/server/sim/entity/quick_move.go`, the command rows from
//! `packages/server/sim/entity/tick.go` and `container.go`, and the runtime
//! duplicate gate from `packages/server/sim/runtime/engine_step.go`.
//!
//! This provider settles ordinary inventory and armor commands in the
//! player-command phase. Partial and quick moves that address the crafting
//! or container views belong to the crafting and container providers, so this
//! module refuses those views without effect instead of settling a view it
//! does not own. Duplicate delivery is stopped upstream by the ordering layer
//! (`engine_step.go` silently skips a sequence at or below the session's last
//! admitted one), so this provider sees each admitted command at most once;
//! the duplicate-visible rows at this boundary are the idempotent re-select
//! and the source-empty refusal.

use mornlea_domain::{Command, CommandEnvelope, RejectReason, StackView};
use mornlea_storage::ItemStack;

use crate::contracts::{
    ActorKey, ActorLifecycle, DamageCause, InventoryPatch, InventoryRecord, PhaseReport, RuleCall,
    RuleEffect, RulePhase, ServerError, SessionKey,
};
use crate::core::command_outcome::{CommandDisposition, CommandResult};
use crate::state::TickContext;

/// Hotbar length inside the unified inventory (`core.HotbarSlots`,
/// `packages/shared/core/item.go`).
const HOTBAR_SLOTS: usize = 9;

/// Unified inventory length: hotbar 0..8 plus backpack 9..35
/// (`core.InventorySlots`, `packages/shared/core/inventory.go`).
const INVENTORY_SLOTS: usize = 36;

/// The absent item number (`core.ItemNone`).
const ITEM_NONE: u16 = 0;

/// Armor region length: head, chest, legs, feet
/// (`core.ArmorSlotCount`, `packages/shared/core/armor.go`).
const ARMOR_SLOTS: usize = 4;

/// Truncation ceiling of the four armor point sums
/// (`core.MaxArmorPoints`, `packages/shared/core/armor.go`).
const MAX_ARMOR_POINTS: u16 = 20;

// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_IRON_HELMET: u16 = 58;
const ITEM_IRON_CHESTPLATE: u16 = 59;
const ITEM_IRON_LEGGINGS: u16 = 60;
const ITEM_IRON_BOOTS: u16 = 61;

/// One armor piece's region attributes: its only legal equip slot and its
/// armor points, the exact `armorPieceOf` table
/// (`packages/shared/core/armor.go`). The durability ceilings behind the
/// points come from `mornlea_domain::durability_max`, the same single Rust
/// registry the Go `core.ItemMaxDurability` table feeds: helmet 165,
/// chestplate 240, leggings 225, boots 195.
fn armor_piece(item: u16) -> Option<(usize, u8)> {
    match item {
        ITEM_IRON_HELMET => Some((0, 2)),
        ITEM_IRON_CHESTPLATE => Some((1, 6)),
        ITEM_IRON_LEGGINGS => Some((2, 5)),
        ITEM_IRON_BOOTS => Some((3, 2)),
        _ => None,
    }
}

/// Sums the intact armor points over the four worn slots and truncates to
/// [`MAX_ARMOR_POINTS`], the exact `core.ArmorPoints` rule
/// (`packages/shared/core/armor.go`): a piece counts when it holds at least
/// one copy and its durability sits inside `1..=ceiling`. The zero-durability
/// historical form stays in place and contributes zero points, and the worn
/// slot itself is not re-checked against the piece mapping here, matching the
/// Go row whose registration boundary (`armorRestoreValid`) owns that check.
pub fn armor_points(worn: &[ItemStack; ARMOR_SLOTS]) -> u8 {
    let mut total: u16 = 0;
    for stack in worn {
        let Some((_, points)) = armor_piece(stack.item) else {
            continue;
        };
        let intact = mornlea_domain::durability_max(stack.item).is_some_and(|max| {
            stack.count >= 1 && stack.durability >= 1 && stack.durability <= max
        });
        if intact {
            total += u16::from(points);
        }
    }
    (total.min(MAX_ARMOR_POINTS)) as u8
}

/// Reduces one raw physical damage by armor points, the exact
/// `core.ReducedDamage` row (`packages/shared/core/armor.go`): each point
/// removes 4% of the raw damage, nonpositive damage has no game meaning and
/// returns zero, and any nonzero settlement keeps at least one damage. The
/// wide intermediate mirrors the Go `int64` arithmetic; the narrowing
/// conversion cannot truncate because the multiplier stays inside `20..=100`,
/// so the scaled value never exceeds the raw damage.
pub fn reduced_damage(damage: i32, points: u8) -> i32 {
    if damage <= 0 {
        return 0;
    }
    let scaled = i64::from(damage) * (100 - 4 * i64::from(points)) / 100;
    if scaled < 1 {
        return 1;
    }
    i32::try_from(scaled).unwrap_or(damage)
}

/// Wears each intact armor piece by exactly one durability point in place,
/// the exact `consumeArmorDurability` row
/// (`packages/server/sim/entity/armor.go`): a piece participates when its
/// item maps to this slot, it holds exactly one copy, and its durability
/// sits inside `1..=ceiling`. Durability one becomes the in-place
/// zero-durability historical form; armor has no broken item number.
/// Reports whether any slot changed.
pub fn consume_armor_durability(worn: &mut [ItemStack; ARMOR_SLOTS]) -> bool {
    let mut changed = false;
    for (slot, stack) in worn.iter_mut().enumerate() {
        let mapped = armor_piece(stack.item).is_some_and(|(expected, _)| expected == slot);
        let intact = mornlea_domain::durability_max(stack.item)
            .is_some_and(|max| stack.durability >= 1 && stack.durability <= max);
        if !mapped || stack.count != 1 || !intact {
            continue;
        }
        stack.durability -= 1;
        changed = true;
    }
    changed
}

/// Settles the armor row of one damage intent over the worn slots and
/// returns the effective damage. Reduction is attached only to the melee and
/// projectile settle points (`settleCombatIntent` in
/// `packages/server/sim/entity/combat.go`,
/// `settleProjectileEntityHit` in `projectile.go`); fall, drowning, hunger
/// and fire keep the raw damage and never touch durability, exactly like the
/// Go paths that bypass the reduction settle point (`applyFallDamage`,
/// `TestFallDamageNotReduced`). When the reduction actually happened, every
/// intact piece wears exactly once in the same settlement, gated on the
/// frozen pre-hit points and the effective-versus-raw comparison.
pub fn settle_damage(
    cause: DamageCause,
    damage: i32,
    points: u8,
    worn: &mut [ItemStack; ARMOR_SLOTS],
) -> i32 {
    if !matches!(cause, DamageCause::Melee | DamageCause::Projectile) {
        return damage;
    }
    let effective = reduced_damage(damage, points);
    if points > 0 && effective < damage {
        consume_armor_durability(worn);
    }
    effective
}

/// Whole-stack move over the unified slots, the exact
/// `core.Inventory.MoveStack` row (`packages/shared/core/inventory.go`):
/// an empty target receives the whole stack, a same-item target merges up to
/// the stack cap with the remainder kept at the source, an unlike target
/// swaps, and an empty source or a same-item target already at the cap
/// refuses with zero change.
fn move_whole(record: &mut InventoryRecord, from: usize, to: usize) -> bool {
    let mut source = record.slots[from];
    if source.item == ITEM_NONE {
        return false;
    }
    let target = record.slots[to];
    if target.item == ITEM_NONE {
        record.slots[to] = source;
        record.slots[from] = ItemStack::default();
        return true;
    }
    if target.item == source.item {
        let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
            return false;
        };
        let space = limit.saturating_sub(target.count);
        if space == 0 {
            return false;
        }
        let moved = space.min(source.count);
        let mut merged = target;
        merged.count += moved;
        source.count -= moved;
        record.slots[to] = merged;
        record.slots[from] = if source.count == 0 {
            ItemStack::default()
        } else {
            source
        };
        return true;
    }
    record.slots[to] = source;
    record.slots[from] = target;
    true
}

/// Partial move of at most `amount` items, the exact
/// `core.Inventory.MoveStackAmount` row
/// (`packages/shared/core/inventory.go`): an empty target inherits the
/// source's item and durability and then shares the capacity-truncated move,
/// a same-item target merges up to the remaining cap with the remainder kept
/// at the source, and an unlike non-empty target refuses because a partial
/// move never swaps — swapping is whole-move semantics.
fn move_amount(record: &mut InventoryRecord, from: usize, to: usize, amount: u8) -> bool {
    if amount == 0 {
        return false;
    }
    let mut source = record.slots[from];
    if source.item == ITEM_NONE {
        return false;
    }
    let mut target = record.slots[to];
    if target.item != ITEM_NONE && target.item != source.item {
        return false;
    }
    if target.item == ITEM_NONE {
        target = source;
        target.count = 0;
    }
    let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
        return false;
    };
    let moved = amount
        .min(source.count)
        .min(limit.saturating_sub(target.count));
    if moved == 0 {
        return false;
    }
    target.count += moved;
    source.count -= moved;
    record.slots[to] = target;
    record.slots[from] = if source.count == 0 {
        ItemStack::default()
    } else {
        source
    };
    true
}

/// Derives the partial amount from the settlement-time source stack, the
/// exact `stackSplitAmount` row (`packages/server/sim/entity/container.go`):
/// a single move takes one, a half move takes the ceiling half
/// `(count + 1) / 2`, and an empty source refuses. The amount is authority
/// derived; no client count exists on the wire.
fn split_amount(source: ItemStack, single: bool) -> Option<u8> {
    if source.item == ITEM_NONE || source.count == 0 {
        return None;
    }
    Some(if single {
        1
    } else {
        // Overflow-free form of `(count + 1) / 2`; valid counts never leave
        // the 1..=64 stack domain so both forms agree.
        source.count / 2 + source.count % 2
    })
}

/// Credits one source stack into one slot region with the region-restricted
/// two-phase order, the exact `quickMoveInsertRegion` row
/// (`packages/server/sim/entity/quick_move.go`): first the same-item slots
/// with remaining capacity in ascending slot order, then the empty slots in
/// ascending slot order; an empty-slot landing inherits the source's
/// durability. The two phases are the region projection of the four-phase
/// `core.Inventory.AddStack` credit order (hotbar merge, hotbar empty,
/// backpack merge, backpack empty) and never cross regions. Returns the
/// unabsorbed remainder.
fn quick_move_insert_region(
    slots: &mut [ItemStack; INVENTORY_SLOTS],
    mut source: ItemStack,
    first: usize,
    last: usize,
) -> ItemStack {
    let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
        return source;
    };
    for merge in [true, false] {
        for current in slots.iter_mut().take(last + 1).skip(first) {
            if source.count == 0 {
                return ItemStack::default();
            }
            let held = *current;
            if merge {
                if held.item != source.item || held.count >= limit {
                    continue;
                }
            } else if held.item != ITEM_NONE {
                continue;
            }
            let base = if merge { held.count } else { 0 };
            let moved = (limit - base).min(source.count);
            *current = ItemStack {
                item: source.item,
                count: base + moved,
                durability: if merge {
                    held.durability
                } else {
                    source.durability
                },
            };
            source.count -= moved;
        }
    }
    source
}

/// Quick move of one unified slot into its opposite region, the exact
/// `applyQuickMoveInventory` row (`packages/server/sim/entity/quick_move.go`):
/// a hotbar source credits the backpack region and a backpack source the
/// hotbar region; the source slot sits outside the scanned region, so writing
/// the remainder back cannot clobber absorbed counts. Zero absorption or an
/// empty source refuses the whole request with zero change.
fn quick_move_inventory(record: &mut InventoryRecord, from: usize) -> bool {
    let source = record.slots[from];
    if source.item == ITEM_NONE {
        return false;
    }
    let (first, last) = if from < HOTBAR_SLOTS {
        (HOTBAR_SLOTS, INVENTORY_SLOTS - 1)
    } else {
        (0, HOTBAR_SLOTS - 1)
    };
    let mut slots = record.slots;
    let leftover = quick_move_insert_region(&mut slots, source, first, last);
    if leftover.count == source.count {
        return false;
    }
    slots[from] = leftover;
    record.slots = slots;
    true
}

/// Equip swap of the selected hotbar stack into its piece-derived armor slot,
/// the exact `executeEquipArmor` row (`packages/server/sim/entity/armor.go`):
/// exactly one armor piece is wearable — empty slots, non-armor items and
/// multi-piece stacks all take the not-armor refusal with zero change. The
/// durability field is not consulted, so the zero-durability historical form
/// stays wearable (`armorRestoreValid` admits it only inside the armor
/// region); the worn piece returns to the hand as the raw storage triple, and
/// the client never chooses the destination slot.
fn equip_armor(record: &mut InventoryRecord) -> bool {
    let selected = usize::from(record.selected.get());
    let stack = record.slots[selected];
    let Some((slot, _points)) = armor_piece(stack.item) else {
        return false;
    };
    if stack.count != 1 {
        return false;
    }
    let worn = record.armor[slot];
    record.armor[slot] = stack;
    record.slots[selected] = worn;
    true
}

/// Owns immediate inventory families at live admission. Raw provider callers
/// may hold inventory without an actor; live admission requires an active owner.
pub(crate) fn admit_command(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> CommandResult {
    let owned = match envelope.command() {
        Command::MoveInventory(_) | Command::EquipArmor => true,
        Command::MovePartial(partial) => partial.view() == StackView::Inventory,
        Command::QuickMove(source) => source.view() == StackView::Inventory,
        _ => false,
    };
    if !owned {
        return Ok(CommandDisposition::Unowned);
    }
    let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
        invariant: "inventory admission session",
    })?;
    let actor = ActorKey::Player(session);
    if !ctx
        .read()
        .actor(actor)
        .is_some_and(|record| record.lifecycle == ActorLifecycle::Active)
    {
        return Ok(CommandDisposition::Refused(RejectReason::PlayerNotReady));
    }
    let before = *ctx.read().inventory(actor).ok_or(ServerError::Internal {
        invariant: "inventory admission owner",
    })?;
    settle_owned(ctx, envelope, session, before)
}

/// A patch failure describes trusted ownership, not a client move refusal.
fn stage_inventory_change(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    before: InventoryRecord,
    after: InventoryRecord,
) -> Result<(), ServerError> {
    let patch =
        InventoryPatch::try_new(actor, before, after).map_err(|_| ServerError::Internal {
            invariant: "inventory patch",
        })?;
    ctx.stage(RuleEffect::Inventory(patch))
        .map_err(|_| ServerError::Internal {
            invariant: "inventory staging",
        })
}

/// Settles one inventory or armor player command and stages exactly one
/// whole-record inventory effect for every state change. The envelope's
/// session is the sole identity source for the player-command phase; a
/// refused settlement stages nothing, so the canonical state hash is
/// untouched.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::PlayerCommand {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    let envelope = call
        .command
        .ok_or(ServerError::InvalidInput { field: "command" })?;
    let session = SessionKey::from_raw(envelope.session())
        .ok_or(ServerError::InvalidInput { field: "session" })?;
    let actor = ActorKey::Player(session);
    let before = *ctx
        .read()
        .inventory(actor)
        .ok_or(ServerError::InvalidInput { field: "session" })?;
    match settle_owned(ctx, envelope, session, before)? {
        CommandDisposition::Settled(report) => Ok(report),
        CommandDisposition::Refused(_) => Err(ServerError::InvalidInput { field: "inventory" }),
        CommandDisposition::Unowned => Err(ServerError::InvalidInput { field: "command" }),
    }
}

/// Shared settlement keeps semantic refusal separate from patch ownership and
/// preserves dirty intent, including an accepted equal armor swap.
fn settle_owned(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
    session: SessionKey,
    before: InventoryRecord,
) -> CommandResult {
    let actor = ActorKey::Player(session);
    let mut after = before;
    let settled = match envelope.command() {
        // The select settlement writes only on a real change and never
        // rejects: re-selecting the held slot is the idempotent no-op row
        // (`CommandSelectHotbar` in `packages/server/sim/entity/tick.go`).
        Command::SelectHotbar(slot) => {
            after.selected = slot;
            true
        }
        Command::MoveInventory(movement) => move_whole(
            &mut after,
            usize::from(movement.from()),
            usize::from(movement.to()),
        ),
        Command::EquipArmor => equip_armor(&mut after),
        Command::MovePartial(partial) if partial.view() == StackView::Inventory => {
            let from = usize::from(partial.from());
            match split_amount(after.slots[from], partial.single()) {
                Some(amount) => move_amount(&mut after, from, usize::from(partial.to()), amount),
                None => false,
            }
        }
        Command::QuickMove(source) if source.view() == StackView::Inventory => {
            quick_move_inventory(&mut after, usize::from(source.slot()))
        }
        // Every other command kind, and the crafting and container stack
        // views, belong to their own providers: refuse without effect.
        _ => return Ok(CommandDisposition::Unowned),
    };
    if !settled {
        return Ok(CommandDisposition::Refused(
            if matches!(envelope.command(), Command::EquipArmor) {
                RejectReason::NotArmor
            } else {
                RejectReason::InvalidInput
            },
        ));
    }
    if after != before {
        stage_inventory_change(ctx, actor, before, after)?;
    }
    // The owner publication lane follows the Go tick row: every accepted
    // settlement that staged a changed patch marks the owner dirty, and an
    // accepted `EquipArmor` may mark it even when the swap is equal
    // (`entity/armor.go` accepts unconditionally). Refused settlements
    // return above this point and never mark; the equal `SelectHotbar`
    // falls through and does not mark.
    if after != before || matches!(envelope.command(), Command::EquipArmor) {
        ctx.record_inventory_publication_dirty(session);
    }
    Ok(CommandDisposition::Settled(PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{ServerLimits, TickBudget};
    use crate::state::AuthorityState;
    use mornlea_domain::{HostileId, HotbarSlot};

    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }

    #[test]
    fn inventory_stale_patch_remains_hard_and_preserves_current_owner() {
        let mut authority = authority();
        let mut context = TickContext::harness(&mut authority, TickBudget::full());
        let actor = ActorKey::Player(SessionKey::from_raw(1).unwrap());
        let before = InventoryRecord::empty();
        let current = before.with_selected(HotbarSlot::new(2).unwrap());
        let after = before.with_selected(HotbarSlot::new(1).unwrap());
        context.preload_inventory(actor, current);
        assert_eq!(
            stage_inventory_change(&mut context, actor, before, after),
            Err(ServerError::Internal {
                invariant: "inventory staging"
            })
        );
        assert_eq!(context.read().inventory(actor), Some(&current));
        assert!(context.events().is_empty());
    }

    #[test]
    fn inventory_non_owner_patch_remains_hard_before_staging() {
        let mut authority = authority();
        let mut context = TickContext::harness(&mut authority, TickBudget::full());
        let actor = ActorKey::Hostile(HostileId::try_new(1).unwrap());
        let before = InventoryRecord::empty();
        let after = before.with_selected(HotbarSlot::new(1).unwrap());
        assert_eq!(
            stage_inventory_change(&mut context, actor, before, after),
            Err(ServerError::Internal {
                invariant: "inventory patch"
            })
        );
        assert_eq!(context.read().inventory(actor), None);
        assert!(context.events().is_empty());
    }
}
