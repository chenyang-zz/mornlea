mod abi;
mod abi_matrix;
mod bridge;
mod client_core;
mod feature_negotiation;
mod frame_decode;
#[cfg(test)]
mod input_map;
mod lifecycle;
mod mesh_worker;
mod pull_buffers;
mod quad_decode;
mod status_decode;
mod terrain_bridge;
mod terrain_budget;
mod terrain_resources;

use godot::prelude::*;

use crate::lifecycle::{deinitialize_godot_stage, initialize_godot_stage};

// These constants make the compiled binding identity reviewable without exposing
// gameplay or client-core data through the provisional extension.
pub const GODOT_API_MAJOR: u8 = 4;
pub const GODOT_API_MINOR: u8 = 7;
pub const GODOT_RUST_VERSION: &str = "0.5.5";

struct MornleaGodotExtension;

// Godot owns extension lifetime; the adapter validates ordered stage transitions and
// converts callback failures to engine errors instead of unwinding across the ABI.
#[gdextension]
unsafe impl ExtensionLibrary for MornleaGodotExtension {
    fn min_level() -> InitLevel {
        InitLevel::Scene
    }

    fn on_stage_init(stage: InitStage) {
        match initialize_godot_stage(stage) {
            Ok(Some(stage)) => godot_print!("[mornlea-lifecycle] rust-init={}", stage.label()),
            Ok(None) => {}
            Err(error) => godot_error!("mornlea_godot initialization failed: {error}"),
        }
    }

    fn on_stage_deinit(stage: InitStage) {
        match deinitialize_godot_stage(stage) {
            Ok(Some(stage)) => godot_print!("[mornlea-lifecycle] rust-deinit={}", stage.label()),
            Ok(None) => {}
            Err(error) => godot_error!("mornlea_godot deinitialization failed: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GODOT_API_MAJOR, GODOT_API_MINOR, GODOT_RUST_VERSION,
        bridge::{BRIDGE_CLASS_NAME, IDENTITY_METHODS, RUST_FACADE_METHODS},
        lifecycle::{BoundaryFailure, Lifecycle, Stage, catch_boundary, supports_godot_api},
    };

    #[test]
    fn dependency_identity_matches_the_pilot() {
        assert_eq!((GODOT_API_MAJOR, GODOT_API_MINOR), (4, 7));
        assert_eq!(GODOT_RUST_VERSION, "0.5.5");
    }

    #[test]
    fn lifecycle_registration_is_identity_only() {
        assert_eq!(BRIDGE_CLASS_NAME, "MornleaClientBridge");
        assert_eq!(
            IDENTITY_METHODS,
            [
                "client_core_abi_major",
                "client_core_abi_minor",
                "godot_api_major",
                "godot_api_minor",
                "godot_rust_version",
                "lifecycle_stage",
                "supports_godot_api",
            ]
        );
    }

    /// The rust-producer facade surface stays exactly the eight
    /// facade-contract methods: a renamed, added or dropped method fails
    /// here until the facade contract itself is consciously revised. The
    /// pilot `session_*` names stay outside this table on purpose.
    #[test]
    fn rust_facade_pins_the_eight_method_table() {
        assert_eq!(
            RUST_FACADE_METHODS,
            [
                "open_core",
                "connect",
                "submit_typed_input",
                "step",
                "pull_typed_frame",
                "family_table",
                "reset",
                "close",
            ]
        );
        // The pilot-only surface and the rust facade stay disjoint name
        // spaces: no pilot method name may shadow a facade entry.
        for pilot in ["session_create", "session_step", "session_close"] {
            assert!(
                !RUST_FACADE_METHODS.contains(&pilot),
                "the pilot name {pilot} is not a facade method"
            );
        }
    }

    #[test]
    fn lifecycle_rejects_wrong_versions_and_stage_order() {
        assert!(supports_godot_api(4, 7));
        assert!(!supports_godot_api(4, 6));
        assert!(!supports_godot_api(5, 0));

        let mut lifecycle = Lifecycle::default();
        assert!(lifecycle.initialize(Stage::MainLoop).is_err());
        lifecycle.initialize(Stage::Scene).unwrap();
        lifecycle.initialize(Stage::MainLoop).unwrap();
        assert!(lifecycle.initialize(Stage::MainLoop).is_err());
        assert!(lifecycle.deinitialize(Stage::Scene).is_err());
        lifecycle.deinitialize(Stage::MainLoop).unwrap();
        lifecycle.deinitialize(Stage::Scene).unwrap();
        assert_eq!(lifecycle.stage(), None);
    }

    #[test]
    fn lifecycle_repeats_one_hundred_clean_cycles() {
        let mut lifecycle = Lifecycle::default();
        for _ in 0..100 {
            lifecycle.initialize(Stage::Scene).unwrap();
            lifecycle.initialize(Stage::Editor).unwrap();
            lifecycle.initialize(Stage::MainLoop).unwrap();
            lifecycle.deinitialize(Stage::MainLoop).unwrap();
            lifecycle.deinitialize(Stage::Editor).unwrap();
            lifecycle.deinitialize(Stage::Scene).unwrap();
        }
        assert_eq!(lifecycle.stage(), None);
    }

    #[test]
    fn lifecycle_converts_rust_panics_to_boundary_failures() {
        let failure = catch_boundary(|| -> () { panic!("lifecycle probe") }).unwrap_err();
        assert_eq!(failure, BoundaryFailure::Panic);
    }
}
