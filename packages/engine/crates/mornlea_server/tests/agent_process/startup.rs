//! Configured companion startup against the actual Python Agent gateway: the
//! runtime's own lease controller acquires the namespace lease on its control
//! worker while the authority already holds both configured companions.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mornlea_server::agent::lease::ControlPhase;
use mornlea_server::contracts::{Deadline, DiskBackend, ServerError, ServerLimits};
use mornlea_server::runtime::RuntimeConfig;
use mornlea_server::runtime::companion::{
    CompanionStartupPorts, SystemClock, start_companions, system_identity,
};
use mornlea_server::state::AuthorityState;
use mornlea_server::store::disk::{DiskOptions, DiskStore};

use super::integration::HELPER_SERIAL;
use super::process::{TempDir, spawn_helper};

fn options() -> DiskOptions {
    DiskOptions {
        region_handle_cap: 1,
        create: mornlea_storage::Metadata {
            format_version: mornlea_storage::METADATA_CURRENT_VERSION,
            seed: 42,
            spawn_dimension: 0,
            spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
            world_time_ticks: 1000,
            day_phase_offset: 0,
            weather_kind: 0,
            weather_ticks_remaining: 0,
            depths_spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
            depths_seed_salt: 0,
            difficulty: 0,
        },
    }
}

#[test]
fn configured_startup_acquires_real_namespace_lease() {
    let _serial = HELPER_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let token = "startup-child-token";
    let mut child = spawn_helper("startup", token);
    let world = TempDir::new("startup-world");
    let root = world.join("world");
    std::fs::create_dir(&root).expect("world dir");
    let mut disk = DiskStore::open(&root, options()).expect("disk store");
    let mut state = AuthorityState::try_new_with_metadata(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        disk.metadata().clone(),
    )
    .unwrap();
    state.enable_source_player_restoration(1).unwrap();
    state.enable_live_chunks().unwrap();
    state.enable_actor_saves().unwrap();
    state.enable_player_persistence().unwrap();
    // `apiKeyEnv` names `PATH` so Go's non-empty check passes; the injected
    // lookup supplies the helper token instead of the variable's value.
    let config = RuntimeConfig::decode(
        format!(
            r#"{{"ai":{{"agentService":{{"endpoint":"{}","apiKeyEnv":"PATH"}},"companions":[{{"id":"3f2b8c1e-4a5d-4e6f-8a7b-9c0d1e2f3a4b","name":"Mira"}},{{"id":"7d9e1f20-3b4c-4d5e-9f60-718293a4b5c6","name":"Tove"}}]}}}}"#,
            child.endpoint()
        )
        .as_bytes(),
    )
    .expect("config decodes");
    let credential = |_: &str| Some(token.to_owned());
    let mut identity = || -> Result<[u8; 16], ServerError> { system_identity() };
    let mut runtime = start_companions(
        config.ai(),
        &mut disk,
        &mut state,
        CompanionStartupPorts {
            clock: Arc::new(SystemClock),
            credential: &credential,
            identity: &mut identity,
        },
    )
    .expect("startup succeeds")
    .expect("configured runtime");
    assert!(state.companion_persistence_enabled());
    assert_eq!(state.residents().actors.len(), 2);
    assert_eq!(runtime.task_timeout_minutes(), 10);

    let until = Instant::now() + Duration::from_secs(15);
    while runtime.agent().current_lease().is_none() {
        assert!(
            Instant::now() < until,
            "control worker never acquired the namespace lease: phase={:?}",
            runtime.agent().lease_phase()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(runtime.agent().lease_phase(), ControlPhase::Active);
    assert!(runtime.agent().lease_worker_running());

    runtime
        .agent_mut()
        .close_until(Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap())
        .expect("agent services close");
    assert_eq!(runtime.agent().lease_phase(), ControlPhase::Closed);
    disk.close().expect("disk closes");
    child.shutdown("startup");
}
