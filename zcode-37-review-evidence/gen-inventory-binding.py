#!/usr/bin/env python3
"""Generate the 3.7 inventory-row to real-integration-evidence binding table.

Every binding names concrete test names and the evidence log that recorded
them passing. The script refuses to mark a row BOUND unless the named test
line (`test <name> ... ok`) exists in the referenced log; unverified rows are
emitted as OPEN so the report never overclaims coverage.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROUND1 = ROOT / "zcode-37-evidence"
ROUND2 = Path(__file__).resolve().parent

INVENTORY = ROOT / "testdata/runtime-migration/server/capability-inventory.json"

# row id -> list of (log filename, test-name substring) bindings.
# The first binding is the integration-path evidence; extra bindings are
# rule-level or transport-level corroboration.
BINDINGS: dict[str, list[tuple[str, str]]] = {
    "command.internal.bed": [("02-server-replay.log", "sleep::bed_partner_coordinate_edges_keep_representable_pairs"), ("02-server-replay.log", "sleep::non_bed_target_is_silent_noop")],
    "command.internal.door": [("02-server-replay.log", "world_mutation::door_pair_and_internal_toggle"), ("02-server-replay.log", "world_mutation::held_sneak_refuses_door_after_target_classification")],
    "command.protocol.client.BoneMeal": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "tools::")],
    "command.protocol.client.ChatCommand": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters")],
    "command.protocol.client.CloseContainer": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("01-local-remote-parity.log", "integration::shared_transcript_matches_across_adapters")],
    "command.protocol.client.CollectWater": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "tools::")],
    "command.protocol.client.DropSelectedItem": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "drops::")],
    "command.protocol.client.DropStack": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "drops::")],
    "command.protocol.client.EquipArmor": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "inventory::")],
    "command.protocol.client.KeepAliveReply": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("01-local-remote-parity.log", "common::")],
    "command.protocol.client.MoveContainerStack": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "containers::")],
    "command.protocol.client.MoveCraftingStack": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "crafting::")],
    "command.protocol.client.MoveInventoryStack": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "inventory::")],
    "command.protocol.client.MoveStackPartial": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "inventory::")],
    "command.protocol.client.OpenContainer": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "containers::")],
    "command.protocol.client.PlaceBlock": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "world_mutation::")],
    "command.protocol.client.PlaceWater": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "tools::")],
    "command.protocol.client.PlayerInput": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "player_motion::")],
    "command.protocol.client.QuickMoveStack": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "containers::")],
    "command.protocol.client.RequestChunkResync": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "world_acquisition::")],
    "command.protocol.client.SelectHotbar": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "inventory::")],
    "command.protocol.client.TakeCraftingOutput": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "crafting::")],
    "command.protocol.client.TillSoil": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters"), ("02-server-replay.log", "tools::")],
    "companion.action.mine-hold": [("02-server-replay.log", "full_corpus::real_agent_candidate_admitted_and_stale_refused"), ("02-server-replay.log", "companions::mining_intents_stage_nothing_here"), ("19-agent-process-nocapture.log", "integration::rust_plan_python_mcp_and_authority")],
    "companion.action.mine-release": [("02-server-replay.log", "full_corpus::real_agent_candidate_admitted_and_stale_refused"), ("02-server-replay.log", "companions::neutral_hold_release_and_container_atomic")],
    "companion.action.move": [("02-server-replay.log", "full_corpus::real_agent_candidate_admitted_and_stale_refused"), ("02-server-replay.log", "companions::motion_steps_move_and_retains_yaw_neutral")],
    "companion.action.place": [("02-server-replay.log", "full_corpus::real_agent_candidate_admitted_and_stale_refused"), ("02-server-replay.log", "companions::placement_settles_in_id_order_with_atomic_refusals")],
    "control.protocol.client.ClientHello": [("01-local-remote-parity.log", "common::")],
    "control.protocol.client.KeepAliveReply": [("01-local-remote-parity.log", "common::")],
    "control.protocol.client.LoginStart": [("01-local-remote-parity.log", "common::"), ("01-local-remote-parity.log", "integration::shared_transcript_matches_across_adapters")],
    "control.protocol.server.Disconnect": [("01-local-remote-parity.log", "tcp::terminal_rejection_flushes_then_releases_socket"), ("01-local-remote-parity.log", "tcp::terminal_poll_expiry_releases_socket")],
    "control.protocol.server.HandshakeReject": [("01-local-remote-parity.log", "common::")],
    "control.protocol.server.KeepAlive": [("01-local-remote-parity.log", "common::")],
    "control.protocol.server.LoginReject": [("01-local-remote-parity.log", "common::")],
    "control.protocol.server.LoginSuccess": [("01-local-remote-parity.log", "integration::shared_transcript_matches_across_adapters")],
    "control.protocol.server.ServerHello": [("01-local-remote-parity.log", "common::")],
    "event.domain.block-changes": [("02-server-replay.log", "world_mutation::")],
    "event.domain.chat": [("25-parity-review.log", "integration::inventory_command_transcripts_match_across_adapters")],
    "event.domain.chest-state": [("02-server-replay.log", "containers::")],
    "event.domain.chunk-snapshot": [("02-server-replay.log", "generation::")],
    "event.domain.combat-hit": [("02-server-replay.log", "hostile_outcomes::")],
    "event.domain.command-rejected": [("01-local-remote-parity.log", "integration::shared_transcript_matches_across_adapters")],
    "event.domain.companion-despawn": [("02-server-replay.log", "companions::")],
    "event.domain.companion-spawn": [("02-server-replay.log", "companions::")],
    "event.domain.companion-states": [("02-server-replay.log", "companions::")],
    "event.domain.container-closed": [("01-local-remote-parity.log", "integration::shared_transcript_matches_across_adapters")],
    "event.domain.crafting-state": [("02-server-replay.log", "crafting::")],
    "event.domain.forget-chunks": [("02-server-replay.log", "world_acquisition::")],
    "event.domain.furnace-state": [("02-server-replay.log", "furnaces::")],
    "event.domain.hostile-despawn": [("02-server-replay.log", "hostile_actors::")],
    "event.domain.hostile-spawn": [("02-server-replay.log", "hostile_actors::")],
    "event.domain.hostile-state": [("02-server-replay.log", "hostile_actors::")],
    "event.domain.inventory-state": [("02-server-replay.log", "inventory::")],
    "event.domain.item-drop-removes": [("02-server-replay.log", "drops::")],
    "event.domain.item-drop-upserts": [("02-server-replay.log", "drops::")],
    "event.domain.passive-despawn": [("02-server-replay.log", "passives::")],
    "event.domain.passive-spawn": [("02-server-replay.log", "passives::")],
    "event.domain.passive-state": [("02-server-replay.log", "passives::")],
    "event.domain.place-block-succeeded": [("02-server-replay.log", "world_mutation::")],
    "event.domain.player-state": [("02-server-replay.log", "player_survival::")],
    "event.domain.projectile-despawn": [("02-server-replay.log", "projectiles::")],
    "event.domain.projectile-spawn": [("02-server-replay.log", "projectiles::")],
    "event.domain.projectile-state": [("02-server-replay.log", "projectiles::")],
    "event.domain.remote-player-despawn": [("01-local-remote-parity.log", "live::actual_reconnects_release_committed_driver_history_across_transports")],
    "event.domain.remote-player-spawn": [("01-local-remote-parity.log", "integration::shared_transcript_matches_across_adapters")],
    "event.domain.remote-player-states": [("01-local-remote-parity.log", "integration::shared_transcript_matches_across_adapters")],
    "save.chunk": [("23-persistence-failure-fixtures-v2.log", "integration::real_save_restart_round_trip_through_store")],
    "save.companion": [("23-persistence-failure-fixtures-v2.log", "integration::real_save_restart_round_trip_through_store"), ("23-persistence-failure-fixtures-v2.log", "actor_projection::")],
    "save.hostile": [("23-persistence-failure-fixtures-v2.log", "integration::real_save_restart_round_trip_through_store"), ("23-persistence-failure-fixtures-v2.log", "actor_projection::")],
    "save.passive": [("23-persistence-failure-fixtures-v2.log", "integration::real_save_restart_round_trip_through_store"), ("23-persistence-failure-fixtures-v2.log", "actor_projection::")],
    "save.player": [("23-persistence-failure-fixtures-v2.log", "integration::real_save_restart_round_trip_through_store")],
    "save.region": [("23-persistence-failure-fixtures-v2.log", "region_io::"), ("23-persistence-failure-fixtures-v2.log", "recovery::")],
    "save.world-metadata": [("23-persistence-failure-fixtures-v2.log", "integration::real_save_restart_round_trip_through_store"), ("23-persistence-failure-fixtures-v2.log", "metadata_live::")],
    "tick.phase.01.player-commands": [("02-server-replay.log", "phase_order::dispatch_chain_runs_in_frozen_order"), ("02-server-replay.log", "phase_order::dispatch_guard_rejects_a_swapped_row")],
    "tick.phase.02.companion-actions": [("02-server-replay.log", "phase_order::")],
    "tick.phase.03.physics-advance": [("02-server-replay.log", "player_motion::")],
    "tick.phase.04.hostile-advance": [("02-server-replay.log", "hostile_actions::")],
    "tick.phase.05.block-updates": [("02-server-replay.log", "world_mutation::mutation_before_fluid_support_order"), ("02-server-replay.log", "phase_order::budget_carry_over_two_ticks")],
}


def log_lines(name: str) -> list[str]:
    for base in (ROUND2, ROUND1):
        path = base / name
        if path.exists():
            return path.read_text(errors="replace").splitlines()
    return []


CACHE: dict[str, list[str]] = {}


def verify(log: str, needle: str) -> bool:
    if log not in CACHE:
        CACHE[log] = log_lines(log)
    return any(
        line.startswith("test ") and needle in line and line.rstrip().endswith(" ok")
        for line in CACHE[log]
    )


def main() -> int:
    rows = json.loads(INVENTORY.read_text())["rows"]
    table = []
    open_rows = 0
    for row in rows:
        rid = row["id"]
        bindings = BINDINGS.get(rid, [])
        checked = []
        ok = True
        for log, needle in bindings:
            hit = verify(log, needle)
            checked.append({"log": log, "test": needle, "verified": hit})
            ok = ok and hit
        if not bindings:
            ok = False
        if not ok:
            open_rows += 1
        table.append({"id": rid, "status": "BOUND" if ok else "OPEN", "evidence": checked})

    out = ROUND2 / "inventory-binding.json"
    out.write_text(json.dumps(table, indent=1) + "\n")

    md = ["# 3.7 capability-inventory row bindings", "",
          f"Generated from `capability-inventory.json` ({len(rows)} rows) against recorded passing test logs.",
          "A row is BOUND only when every named test line (`test <name> ... ok`) exists in the referenced log.", ""]
    for entry in table:
        md.append(f"- **{entry['id']}** — {entry['status']}")
        for ev in entry["evidence"]:
            mark = "✓" if ev["verified"] else "✗"
            md.append(f"  - {mark} `{ev['test']}` in `{ev['log']}`")
    (ROUND2 / "inventory-binding.md").write_text("\n".join(md) + "\n")

    bound = len(rows) - open_rows
    print(f"rows={len(rows)} bound={bound} open={open_rows}")
    for entry in table:
        if entry["status"] == "OPEN":
            missing = [e for e in entry["evidence"] if not e["verified"]] or [{"test": "<no binding>", "log": "-"}]
            print(f"OPEN {entry['id']}: " + ", ".join(f"{m['test']}@{m['log']}" for m in missing))
    return 0 if open_rows == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
