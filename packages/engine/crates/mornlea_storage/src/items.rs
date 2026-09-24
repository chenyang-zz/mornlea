//! Item and inventory value rules shared by the entity save families.
//!
//! Ordinary stack, limit, and durability rules come from `mornlea_domain`.
//! The numeric constants that remain name furnace wire fields or fixed slot
//! counts. Player armor is not an ordinary stack and is not admitted here.

/// Stable global item number. Numbering is protocol-stable and append-only.
pub type ItemId = u16;

/// Empty slot and list terminator.
pub const ITEM_NONE: ItemId = 0;
/// Stone and the stone pickaxe are named because chunk slot tests spell those
/// wire item numbers. The limit and durability tables do not live here.
#[cfg(test)]
pub(crate) const ITEM_STONE: ItemId = 1;
#[cfg(test)]
pub(crate) const ITEM_STONE_PICKAXE: ItemId = 10;
/// Furnace fuel and the empty-fuel sentinel used by the chunk slot layout.
pub(crate) const ITEM_COAL: ItemId = 5;
pub(crate) const ITEM_RAW_IRON: ItemId = 6;
pub(crate) const ITEM_IRON_INGOT: ItemId = 7;
pub(crate) const ITEM_SAND: ItemId = 18;
pub(crate) const ITEM_GLASS: ItemId = 23;
pub(crate) const ITEM_BRICK: ItemId = 24;
pub(crate) const ITEM_CLAY: ItemId = 27;
pub(crate) const ITEM_RAW_BEEF: ItemId = 53;
pub(crate) const ITEM_COOKED_BEEF: ItemId = 54;

/// Exclusive upper bound of the legal item numbering.
pub const ITEM_ID_MAX: ItemId = 66;

/// Fixed number of hotbar slots.
pub const HOTBAR_SLOTS: usize = 9;
/// Fixed number of backpack slots beyond the hotbar.
pub const BACKPACK_SLOTS: usize = 27;
/// Maximum stack size for a stackable item.
pub const MAX_STACK_COUNT: u8 = 64;

/// One hotbar or backpack slot. The zero value is an empty slot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ItemStack {
    pub item: ItemId,
    pub count: u8,
    /// Only meaningful for tools; other items keep it at zero.
    pub durability: u16,
}

impl ItemStack {
    /// Reports whether the slot value is canonical: an empty slot carries no
    /// item, count, or durability, and a non-empty slot is a registered item
    /// within its stack limit with durability inside the tool range.
    pub fn is_valid(&self) -> bool {
        mornlea_domain::ItemStack::try_new(self.item, self.count, self.durability).is_ok()
    }
}

/// Converts one ordinary save slot into a domain stack.
///
/// Player armor does not use this conversion. Armor keeps raw triples that
/// ordinary stack admission rejects.
pub fn checked_item_stack(
    raw: ItemStack,
) -> crate::error::StorageResult<mornlea_domain::ItemStack> {
    mornlea_domain::ItemStack::try_new(raw.item, raw.count, raw.durability)
        .map_err(|_| crate::error::corrupt("item stack", "not an ordinary stack"))
}

/// The fixed-capacity hotbar. `selected` must be inside `0..HOTBAR_SLOTS`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Hotbar {
    pub selected: u8,
    pub slots: [ItemStack; HOTBAR_SLOTS],
}

impl Hotbar {
    /// Reports whether the selection index and every slot value are canonical.
    pub fn is_valid(&self) -> bool {
        self.selected < HOTBAR_SLOTS as u8 && self.slots.iter().all(ItemStack::is_valid)
    }
}

/// The complete inventory state: hotbar plus the fixed-size backpack.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Inventory {
    pub hotbar: Hotbar,
    pub backpack: [ItemStack; BACKPACK_SLOTS],
}

impl Inventory {
    /// Reports whether the hotbar and every backpack slot are canonical.
    pub fn is_valid(&self) -> bool {
        self.hotbar.is_valid() && self.backpack.iter().all(ItemStack::is_valid)
    }
}

/// Returns the per-slot stack limit for `item`, or `None` for an unknown item.
///
/// The table lives in `mornlea_domain`. This name stays so save codecs can ask
/// for a format limit without owning a second registry.
pub fn item_stack_limit(item: ItemId) -> Option<u8> {
    mornlea_domain::item_stack_limit(item)
}

/// Returns the durability ceiling for `item`, or `None` when it has none.
pub fn item_max_durability(item: ItemId) -> Option<u16> {
    mornlea_domain::durability_max(item)
}

#[cfg(test)]
mod tests {
    use super::{BACKPACK_SLOTS, HOTBAR_SLOTS, ITEM_NONE, Inventory, ItemStack, MAX_STACK_COUNT};

    #[test]
    fn empty_slot_is_the_only_valid_zero_value() {
        assert!(ItemStack::default().is_valid());
        assert!(
            !ItemStack {
                item: ITEM_NONE,
                count: 1,
                durability: 0,
            }
            .is_valid()
        );
        assert!(
            !ItemStack {
                item: ITEM_NONE,
                count: 0,
                durability: 1,
            }
            .is_valid()
        );
    }

    #[test]
    fn stackable_and_tool_slots_use_their_own_bounds() {
        assert!(
            ItemStack {
                item: 1,
                count: MAX_STACK_COUNT,
                durability: 0,
            }
            .is_valid()
        );
        assert!(
            !ItemStack {
                item: 1,
                count: MAX_STACK_COUNT + 1,
                durability: 0,
            }
            .is_valid()
        );
        let tool = ItemStack {
            item: 10,
            count: 1,
            durability: 131,
        };
        assert!(tool.is_valid());
        assert!(
            !ItemStack {
                durability: 0,
                ..tool
            }
            .is_valid()
        );
        assert!(
            !ItemStack {
                durability: 132,
                ..tool
            }
            .is_valid()
        );
    }

    #[test]
    fn unknown_items_are_rejected() {
        assert!(
            !ItemStack {
                item: super::ITEM_ID_MAX,
                count: 1,
                durability: 0,
            }
            .is_valid()
        );
        assert!(
            !ItemStack {
                item: 4_000,
                count: 1,
                durability: 0,
            }
            .is_valid()
        );
    }

    #[test]
    fn inventory_covers_the_fixed_slot_counts() {
        assert_eq!(HOTBAR_SLOTS, 9);
        assert_eq!(BACKPACK_SLOTS, 27);
        assert!(Inventory::default().is_valid());
        assert!(
            !Inventory {
                hotbar: super::Hotbar {
                    selected: 9,
                    slots: [ItemStack::default(); HOTBAR_SLOTS],
                },
                ..Inventory::default()
            }
            .is_valid()
        );
    }
}
