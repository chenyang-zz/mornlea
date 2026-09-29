//! Save-to-actor login staging.
//!
//! Pure save-to-actor mapping for tick-start login seeding. These functions
//! read save records only and never touch authority state; the serial reducer
//! owns session iteration and overlay staging.

use mornlea_domain::{
    CraftingSize, Dimension, FiniteVec3, HotbarSlot, LookAngles, MotionState, MotionStateParts,
    SurvivalState, SurvivalStateParts,
};
use mornlea_storage::{BACKPACK_SLOTS, HOTBAR_SLOTS, ItemStack, PlayerSave};

use super::contracts::{
    ActorBody, ActorKey, ActorLifecycle, ActorRecord, InventoryRecord, ServerError, SessionKey,
};

/// Full health for saves that carry none (`core.MaxHealth`, the
/// registration fallback in `packages/server/sim/entity/player.go`).
const FULL_HEALTH: u8 = 20;

/// Full oxygen at login. Oxygen never reaches saves, so every login starts
/// full (`core.MaxOxygenTicks`, same source).
const FULL_OXYGEN: u16 = 300;

/// Seeded tick state for one login: the initial actor plus its inventory.
/// Cooldown, mining, eating, and bow lanes stay unstaged; their zeroed
/// initial condition is the absence of a record, and providers derive
/// first-tick transients from the actor body instead.
pub struct SeededPlayer {
    pub actor: ActorRecord,
    pub inventory: InventoryRecord,
}

/// Maps one save body to its initial actor and inventory.
///
/// The pose carries the save position with a zeroed velocity and a false
/// ground bit, the look carries the restore yaw and pitch, health falls back
/// to full when the save carries none, hunger carries the save triple with
/// the saturation hint derived from its quantity, crafting is always the
/// empty personal grid (grids never persist), the unified slots join the
/// save hotbar and backpack under the save selection, armor copies slot-wise,
/// and the actor runs Active in the save dimension. Every default follows the
/// Go registration path (`RegisterPlayer` in
/// `packages/server/sim/entity/player.go`, the hunger reset in
/// `packages/server/sim/entity/hunger.go`, the grid sizes in
/// `packages/server/sim/entity/crafting.go`, and the unified inventory length
/// in the crafting provider).
pub fn seed_player(session: SessionKey, save: &PlayerSave) -> Result<SeededPlayer, ServerError> {
    let dimension = Dimension::new(
        u8::try_from(save.current.dimension)
            .map_err(|_| ServerError::InvalidInput { field: "dimension" })?,
    )
    .map_err(|_| ServerError::InvalidInput { field: "dimension" })?;
    let position = FiniteVec3::try_new(save.current.position)
        .map_err(|_| ServerError::InvalidInput { field: "position" })?;
    let velocity = FiniteVec3::try_new([0.0; 3])
        .map_err(|_| ServerError::InvalidInput { field: "velocity" })?;
    let look = LookAngles::try_new(save.yaw, save.pitch)
        .map_err(|_| ServerError::InvalidInput { field: "look" })?;
    let health = if save.health == 0 {
        FULL_HEALTH
    } else {
        save.health
    };
    let survival = SurvivalState::try_new(SurvivalStateParts {
        health,
        oxygen: FULL_OXYGEN,
        hunger: save.hunger,
        saturation_zero: save.saturation_milli == 0,
        armor_points: 0,
    })
    .map_err(|_| ServerError::InvalidInput { field: "survival" })?;
    let actor = ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        dimension,
        MotionState::new(MotionStateParts {
            position,
            velocity,
            on_ground: false,
        }),
        look,
        survival,
        ActorBody::Player(save.clone()),
    )?;
    let mut slots = [ItemStack::default(); 36];
    slots[0..HOTBAR_SLOTS].copy_from_slice(&save.inventory.hotbar.slots);
    slots[HOTBAR_SLOTS..HOTBAR_SLOTS + BACKPACK_SLOTS].copy_from_slice(&save.inventory.backpack);
    let selected = HotbarSlot::new(save.inventory.hotbar.selected)
        .map_err(|_| ServerError::InvalidInput { field: "inventory" })?;
    let inventory = InventoryRecord::try_new(
        slots,
        selected,
        save.armor,
        [ItemStack::default(); 9],
        CraftingSize::Personal,
    )?;
    Ok(SeededPlayer { actor, inventory })
}
