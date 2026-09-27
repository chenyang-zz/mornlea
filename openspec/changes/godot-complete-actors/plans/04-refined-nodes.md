# Refined node decisions

Unsplit nodes retain the exact files, APIs, algorithms, red/green cases, commands and rollback in the existing worker packet. The dependency register adds readiness requirements and does not grant edits to shared files.

## Source-bound actor behavior overrides

Read F3 [source mapping and supporting values](../../rust-client-core/plans/05-supporting-values.md) and its actor field table. These rules override invented attack/lure/task/hit fields in the earlier packet; they preserve the supported observable source behavior rather than add protocol payloads.

| Node | Exact additional scene file | Permitted observations and first failure |
| --- | --- | --- |
| 2.2 | `apps/mornlea-godot/features/companions/companions.tscn` | Companion identity/pose/name/reset only; task text may read world-UI TaskView as a separate ordered observation. Missing task stays absent; no fabricated companion death/action marker. Old-epoch pose cannot resurrect a removed instance. |
| 2.3 | `apps/mornlea-godot/features/hostile_mobs/hostile.tscn` | Archetype/health/pose/velocity; no actor-specific attack or hit marker from CombatHit. Health transition is visual state, not a new damage calculation. Test health change and remove exactly once; an unassociated combat cue never names an attacker. |
| 2.4 | `apps/mornlea-godot/features/passive_cows/passive.tscn` | Health/grazing and accepted Vanished/Died despawn reason; no lure marker. Test late graze after removal; separate drop publication cannot be causally attached to a cow without source identity. |
| 2.5a | existing exact packet scene | Spawn/position/remove; no projectile-specific hit reason. Test launch→state→remove and late state ignored, not an invented hit. |
| 2.6a | existing exact packet scene | Pose/movement/correction plus typed local semantic input records for cosmetic held-use animation; no inferred authoritative attack result. Confirmed mining progress comes only from accepted player-view MiningState projection, never raw PlayerState access. Release/reset cancels local animation; queued input never implies success. |
| 2.6b | existing exact packet scene | Tags only for remote-player display_name and companion name; absent names create no placeholder actor tag. Test missing name, invalid bounded text and remove. |
| 2.6c | `apps/mornlea-godot/features/effects/effects.tscn` | Source-bound confirmed cue/observation keys from F3 audio provenance, including absence of universal event ID. Test duplicate observation once, old epoch ignored and no fabricated actor/projectile association. Cosmetic effects own no authoritative outcome. |

Node 1.2 freezes and tests feature facade `apply_typed_frame(frame:dict)->None`, with the accepted G1-owned validated dictionary shape; features do not create/drive another session. P9 3.1 maps reserved `features/particles/feature.tres` to the stable particle capability's new effects scene and manifest, updates catalogs atomically, and removes duplicate reserved references. It owns `features/effects/feature.tres` and `features/particles/feature.tres`; workers edit neither. Exact family requirements include world-ui for companion task display and input/audio-cues for the corresponding viewmodel/effects consumers. Missing mandatory family fails before activation.

The controller registers new `actor_*_check.py` tests in `scripts/godot/entity-check.sh` at 1.2, which currently runs only pilot remote-player checks. Every per-kind script and Rust topic module is separately named in discovery; zero cases fail. Provider gates retain original commands and per-kind source oracles. Scene behavior uses confirmed source fields or explicitly cosmetic local intent; requesting new authoritative associations starts a separate upstream OpenSpec change, never worker invention.
