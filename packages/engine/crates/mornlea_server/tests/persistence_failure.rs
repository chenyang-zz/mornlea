#[path = "persistence_failure/activation.rs"]
mod activation;
#[path = "persistence_failure/atomic_file.rs"]
mod atomic_file;
#[path = "persistence_failure/integration.rs"]
mod integration;
#[path = "persistence_failure/mailbox.rs"]
mod mailbox;
#[path = "persistence_failure/metadata_live.rs"]
mod metadata_live;
#[path = "persistence_failure/recovery.rs"]
mod recovery;
#[path = "persistence_failure/region_io.rs"]
mod region_io;
#[path = "persistence_failure/scheduler.rs"]
mod scheduler;

#[path = "persistence_failure/io.rs"]
mod io;

#[path = "persistence_failure/background.rs"]
mod background;

#[path = "persistence_failure/loads.rs"]
mod loads;

#[path = "persistence_failure/chunk_view.rs"]
mod chunk_view;

#[path = "persistence_failure/live_acquisition.rs"]
mod live_acquisition;

#[path = "persistence_failure/chunk_driver.rs"]
mod chunk_driver;

#[path = "persistence_failure/live_chunk_saves.rs"]
mod live_chunk_saves;

#[path = "persistence_failure/chunk_encoding.rs"]
mod chunk_encoding;

#[path = "persistence_failure/chunk_retirement.rs"]
mod chunk_retirement;

#[path = "persistence_failure/actor_projection.rs"]
mod actor_projection;

#[path = "persistence_failure/source_player_restore.rs"]
mod source_player_restore;

#[path = "persistence_failure/actor_save.rs"]
mod actor_save;

#[path = "persistence_failure/authority_actor_saves.rs"]
mod authority_actor_saves;

#[path = "persistence_failure/source_player_persistence.rs"]
mod source_player_persistence;

#[path = "persistence_failure/source_mob_persistence.rs"]
mod source_mob_persistence;

#[path = "persistence_failure/source_companion_persistence.rs"]
mod source_companion_persistence;
