#!/usr/bin/env bash
# Fresh rerun of the seven inventory rows that the earlier 3.7 review left
# open. For each row it records the producer locations, the Rust tests that
# assert the event content, recipient and order, and the Go tests for the
# same behaviour. Every log starts with the exact command and source commit
# and ends with the exit code. Run from anywhere inside the repository.
set -u
ROOT=$(git rev-parse --show-toplevel)
EV="$ROOT/openspec/changes/rust-authoritative-server/evidence/3.7-rerun"
SHA=$(git -C "$ROOT" rev-parse HEAD)
LIB="core::state::owner_record_admission_tests"

record() {
  local out=$1
  shift
  mkdir -p "$(dirname "$EV/$out")"
  {
    echo "\$ $*"
    echo "# source commit: $SHA"
    echo "# started: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    (cd "$ROOT" && bash -c "$*") 2>&1
    echo "# exit: $?"
  } > "$EV/$out"
}

rust() {
  local out=$1 target=$2
  shift 2
  record "$out" "cd packages/engine && cargo test -p mornlea_server --locked $target -- --exact $*"
}

gotest() {
  local out=$1 pattern=$2
  record "$out" "go test ./packages/server/server -race -count=1 -v -run '^($pattern)\$'"
}

producer() {
  local out=$1 pattern=$2
  record "$out" "git grep -n -E '$pattern' -- packages/engine/crates/mornlea_server/src/core/publication_project.rs packages/engine/crates/mornlea_server/src/core/step.rs"
}

record environment.txt "git rev-parse HEAD; git status --porcelain -- packages; rustc --version; cargo --version; go version; uname -sm"
# Production call chain: AuthorityState::advance_tick -> step::reduce_tick ->
# project_source_publication -> prepare (despawns, forgets) / finish_tail
# (companions, remotes, chat).
record call-chain.txt "git grep -n -E 'project_source_publication\(|emit_despawns\(|emit_forgets\(|emit_companions\(|emit_remotes\(|emit_chat\(' -- packages/engine/crates/mornlea_server/src/core/step.rs packages/engine/crates/mornlea_server/src/core/publication_project.rs"

# event.domain.chat
producer chat/producer.txt 'Event::Chat\(|fn emit_chat'
rust chat/rust-lib.log --lib core::state::companion_chat_boundary_tests::restored_padded_terminal_fact_consumes_id_and_preserves_later_publication
rust chat/rust-replay.log "--test server_replay" publication_projection::projection_chat_accepted_broadcast_and_rejects_sender_only publication_projection::projection_chat_contract_inactive_admission_and_unknown publication_projection::projection_chat_contract_install_stop_runtime_fence publication_projection::projection_chat_contract_stop_rejections_and_take_once publication_projection::projection_chat_contract_fifo_capacity_two_slots
rust chat/rust-parity.log "--test local_remote_parity" integration::chat_publications_match_across_adapters
gotest chat/go.log 'TestChatCommandAddressesExactConfiguredCompanionAtTickBoundary|TestMalformedOrUnknownCompanionChatRejectsOnlySender|TestAcceptedCompanionChatBroadcastsInChannelOrder|TestCompanionChatMemoryTCPParity'

# event.domain.companion-spawn
producer companion-spawn/producer.txt 'Event::CompanionSpawn\(|fn emit_companions'
rust companion-spawn/rust-lib.log --lib $LIB::companion_admission_closed_owner_and_peer $LIB::companion_admission_departure_and_arrival $LIB::companion_admission_projection_then_queued $LIB::companion_admission_targeted_and_broadcast_ownership
rust companion-spawn/rust-replay.log "--test server_replay" publication_projection::projection_companion_lifecycle_spawn_states_despawn publication_projection::projection_actor_reset_companion_captured_once publication_projection::projection_batch_splitting_respects_wire_caps
rust companion-spawn/rust-parity.log "--test local_remote_parity" integration::companion_lifecycle_publications_match_across_adapters
gotest companion-spawn/go.log 'TestCompanionPublicationWaitsForFootChunkSnapshot|TestCompanionPublicationStatesAreSortedAndNewSpawnsSkipCurrentTick|TestEightPlayersAndFourCompanionsUseIndependentServerCapacity|TestCompanionPublicationRejectsUnknownDefinitionWithoutPartialVisibility'

# event.domain.companion-despawn
producer companion-despawn/producer.txt 'Event::CompanionDespawn\(|fn emit_despawns'
rust companion-despawn/rust-lib.log --lib $LIB::companion_admission_departure_and_arrival $LIB::companion_admission_despawn_preflight_retry
rust companion-despawn/rust-replay.log "--test server_replay" publication_projection::projection_companion_lifecycle_spawn_states_despawn publication_projection::projection_despawn_families_emit_once
rust companion-despawn/rust-parity.log "--test local_remote_parity" integration::companion_lifecycle_publications_match_across_adapters
gotest companion-despawn/go.log 'TestCompanionPublicationDespawnsOnInterestExit'

# event.domain.forget-chunks
producer forget-chunks/producer.txt 'Event::ForgetChunks\(|fn emit_forgets'
rust forget-chunks/rust-lib.log --lib $LIB::source_delta_validation_classified_prefix
rust forget-chunks/rust-replay.log "--test server_replay" publication_projection::projection_chunk_snapshot_then_block_changes_then_forget
rust forget-chunks/rust-parity.log "--test local_remote_parity" integration::chunk_snapshot_block_changes_forget_match_across_adapters
gotest forget-chunks/go.log 'TestSessionRegistrySnapshotsDeltasAndForgetStayWithTarget|TestPlayerStatePublicationOrder'

# event.domain.remote-player-spawn
producer remote-player-spawn/producer.txt 'Event::RemotePlayerSpawn\(|fn emit_remotes'
rust remote-player-spawn/rust-lib.log --lib $LIB::remote_admission_closed_owner_and_peer $LIB::remote_admission_projection_then_queued $LIB::remote_admission_same_uuid_replacement
rust remote-player-spawn/rust-replay.log "--test server_replay" publication_projection::projection_remote_lifecycle_spawn_states_despawn_order publication_projection::projection_actor_reset_remote_captured_once publication_projection::projection_despawn_families_emit_once
rust remote-player-spawn/rust-parity.log "--test local_remote_parity" integration::remote_player_lifecycle_publications_match_across_adapters
gotest remote-player-spawn/go.log 'TestRemotePlayerInterestMatrix|TestRemotePlayerOutsideInterestJoinAndLeaveAreSilent|TestRemotePlayerNegativeFootChunkUsesFloor|TestRemotePlayerPublicationOrder|TestRemotePlayerGenerationReplacementDespawnsBeforeSpawn'

# event.domain.remote-player-despawn
producer remote-player-despawn/producer.txt 'Event::RemotePlayerDespawn\(|fn emit_despawns'
rust remote-player-despawn/rust-lib.log --lib $LIB::remote_admission_despawn_preflight_retry $LIB::remote_admission_same_uuid_replacement $LIB::remote_admission_unavailable_observer_preflight
rust remote-player-despawn/rust-replay.log "--test server_replay" publication_projection::projection_remote_lifecycle_spawn_states_despawn_order publication_projection::projection_despawn_families_emit_once publication_projection::projection_remote_reconnect_same_uuid_replaces_visible_incarnation publication_projection::projection_remote_reconnect_pending_gap_control
rust remote-player-despawn/rust-parity.log "--test local_remote_parity" integration::remote_player_lifecycle_publications_match_across_adapters
gotest remote-player-despawn/go.log 'TestRemotePlayerInterestMatrix|TestRemotePlayerPublicationOrder|TestRemotePlayerGenerationReplacementDespawnsBeforeSpawn'

# event.domain.remote-player-states
producer remote-player-states/producer.txt 'RemotePlayerStates::try_new|Event::RemotePlayerStates\('
rust remote-player-states/rust-lib.log --lib $LIB::remote_admission_closed_owner_and_peer $LIB::remote_admission_projection_then_queued
rust remote-player-states/rust-replay.log "--test server_replay" publication_projection::projection_remote_lifecycle_spawn_states_despawn_order publication_projection::projection_actor_reset_remote_captured_once
rust remote-player-states/rust-parity.log "--test local_remote_parity" integration::remote_player_lifecycle_publications_match_across_adapters
gotest remote-player-states/go.log 'TestRemotePlayerStatesAreSortedAndNewSpawnsSkipCurrentTick|TestRemotePlayerEightSessionsPublishSevenStatesWith296BytePayload'
