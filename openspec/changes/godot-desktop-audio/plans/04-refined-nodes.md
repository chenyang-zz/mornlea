# Refined node decisions

Unsplit nodes retain the exact files, APIs, algorithms, red/green cases, commands and rollback in the existing worker packet. The dependency register adds readiness requirements and does not grant edits to shared files.

## Independent actual-host qualification

Parent 3.2 is replaced by three independently rejected/accepted reports. 3.2a owns `testdata/runtime-migration/client/desktop-macos-cases.json`; 3.2b owns `desktop-windows-cases.json`; 3.2c owns `desktop-linux-cases.json` in that same directory. Each provider writes its immutable evidence under its own `build/desktop-evidence/<target>` directory; the controller alone appends ledger acceptance. No two workers edit the ledger/catalog or another target's manifest.

Each node requires 3.1 plus accepted P13 wrapper routing2.4a and its matching target Godot/embedded-Python/native-G1 preparation SHA (Windows2.3b1–2.3b3, Linux2.3c1–2.3c3; macOS qualified retained Darwin payload and manifest). Candidate device tests bind those exact artifact hashes, never a host-default Python. The P13 phase-2.4a gate precedes P11 without a cycle: P13 waits for P11 full acceptance only at3.5. Each node and runs actual host Rust `desktop_contract`, `scripts/godot/audio-check.sh`, `make godot-input-check`. Report `{target,actual_os,arch,source_sha,core_sha,bridge_sha,runtime_sha256,assets_sha256,cases,executed_count,failures}` and bind each cue/input/focus/reset/no-device case from the accepted inventory. Wrong host, missing case, empty discovery or copied report first fails the harness; real fake/no-device behavior then passes. Windows/Linux cannot close on macOS or cross compilation. Missing hardware is a separate playback status and does not remove required no-device/semantic coverage; foreground real-device acceptance requires the user's separate manual-testing instruction.

Rollback removes only the target's evidence/profile, leaving peers' results intact. All three reports are required by 4.1. Device callbacks follow accepted F3 lifecycle generation; P13 launch progress does not own device teardown, and shared close-order/catalog integration is controller-serialized.
