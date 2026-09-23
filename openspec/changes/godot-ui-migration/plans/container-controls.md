# Godot container controls implementation plan

> For agentic workers: use Superpowers subagent-driven development. This packet carries execution detail; only the parent change's `tasks.md` records status. The controller freezes and integrates contracts, and workers implement only their assigned nodes.

**Goal:** Present personal inventory/crafting, workbench, chest, and furnace with Godot Control and embedded Python over a Rust-owned semantic view, while preserving server-authoritative interaction.

**Architecture:** `mornlea_client_core` owns confirmed slots, view identity, selected source, recipe selection, command routing, and bounded intent validation. `mornlea_godot` projects one typed `container_ui` value per coherent frame and accepts typed intent. The reserved Godot `containers` feature renders fixed Control trees and never constructs protocol messages or predicts counts.

**Tech stack:** Rust 1.97.1 client-core and Godot 4.7.2 GDExtension, pinned Py4Godot embedded Python, Godot Control scenes, Go 1.26 deterministic asset generator, source-level Python `unittest` and headless Godot gates.

**Spec:** [P10 delta](../specs/godot-ui-migration/spec.md), [P10 design](../design.md), [F3 producer design](../../rust-client-core/design.md), [target architecture](../../../../docs/architecture-target.md). The old [container behavior spec](../../../../openspec/specs/container-ui-presentation/spec.md) and current Go implementation are offline behavior oracles, not target ownership.

## Scope and dispatch gate

This is the container slice of P10 task 2.3. It does not complete menus, character panels, HUD, P12 visual handoff, or P10 closeout. All C1–C6 nodes stay pending until F1, F2 and F3 acceptance is recorded with source SHA, corpus identity, nonzero tests, rollback, and compatible typed family in their ledgers. The `mornlea_client_core` crate does not exist at this planning baseline, so commands naming it are prospective, never evidence of a passing implementation. If the accepted F3 public API or directory layout differs, the controller reconciles this packet and both changes before any Worker is sent.

The current protocol completion checkout is separate and dirty. Execution uses an isolated worktree based on an accepted prerequisite SHA. A Worker edits only the paths assigned below and does not clean, reset, or commit user-owned files. The controller records evidence and checks off `tasks.md` after each independently verified, scoped commit.

## Frozen shared contract

### Publication and ownership

`mornlea_client_core::ui::containers` produces `ContainerViewV1` once per immutable frame. The selected Rust producer and `mornlea_godot` adapter publish it at `frame["container_ui"]`, with the same positive `epoch` and monotonically increasing `revision` as the containing typed frame. An accepted local source/recipe/cancel/close transition republishes one whole coherent frame with a strictly greater revision even if the server sends nothing; the inventory mirror and its confirmed counts are unchanged. The bridge projects a Godot Dictionary with these exact snake-case keys:

| Key | Type and bound | Meaning |
| --- | --- | --- |
| `schema_major`, `schema_minor` | integers `1, 0` | Reject a different major or a newer minor. |
| `epoch`, `revision` | positive signed 64-bit integers | Equal the containing frame identity; a lower revision in the same epoch is stale. |
| `token` | integer `1..9007199254740991` | Changes whenever panel identity, session epoch, or input-capture identity changes; never reused within an epoch. |
| `kind` | `none/inventory/character/workbench/chest/furnace` | `none` and `character` hide this feature's panels. |
| `confirmed`, `cursor_free` | booleans | Only confirmed open views permit slot intent; focus can still suppress callbacks. |
| `inventory`, `grid`, `chest`, `furnace` | fixed arrays of 36, 9, 27, 3 `SlotV1` records | Unused arrays contain empty slots; the current kind alone decides visible addresses. |
| `grid_size` | integer 2 or 3 | Inventory uses 2 and workbench uses 3; active cells are row-major indexes `0..grid_size²-1`. |
| `output` | one `SlotV1` | Crafting result; only `take-output` can activate it. Furnace result is `furnace[2]`. |
| `progress`, `burn` | finite ratios `0..1` | Rust calculates furnace progress; zero for non-furnace views. |
| `recipes` | fixed array of 10 `RecipeV1` records | Each has `name` (UTF-8, at most 64 Unicode scalars), `size=3`, nine `SlotV1` cells and one `output` slot. |
| `recipe_index` | integer `-1..9` | Selection is Rust-owned. |
| `source` | null or `{"area": string, "index": integer}` | Rust-owned two-click source; Godot highlights only this published value. |

`SlotV1` contains `item_id: u16`, `count: u8`, `name: UTF-8 string` (at most 64 Unicode scalars), and `durability: finite f32 in [0,1]`. Empty is exactly `(0,0,"",0)`; a nonempty slot has registered `item_id`, `count in 1..64`, and its authoritative Chinese display name. The Rust producer validates item-specific stack limits and derives durability exactly once. Python rejects an invalid record before changing any visible slot. An absent nonempty icon is a bounded asset qualification failure, not permission to invent one.

The planned Rust public surface is exact at the type boundary; private storage may differ:

```rust
pub enum ContainerKindV1 { None, Inventory, Character, Workbench, Chest, Furnace }
pub enum SlotAreaV1 { Inventory, Crafting, Chest, Furnace }
pub struct SlotAddressV1 { pub area: SlotAreaV1, pub index: u8 }
pub struct SlotV1 { pub item_id: u16, pub count: u8, pub name: String, pub durability: f32 }
pub struct RecipeV1 { pub name: String, pub size: u8, pub slots: [SlotV1; 9], pub output: SlotV1 }
pub struct ContainerViewV1 {
    pub epoch: u64, pub revision: u64, pub token: u64,
    pub kind: ContainerKindV1, pub confirmed: bool, pub cursor_free: bool,
    pub inventory: [SlotV1; 36], pub grid: [SlotV1; 9], pub grid_size: u8,
    pub output: SlotV1, pub chest: [SlotV1; 27], pub furnace: [SlotV1; 3],
    pub progress: f32, pub burn: f32, pub recipes: [RecipeV1; 10],
    pub recipe_index: i8, pub source: Option<SlotAddressV1>,
}
pub enum ContainerIntentV1 {
    Close { epoch: u64, token: u64 },
    Recipe { epoch: u64, token: u64, index: u8 },
    TakeOutput { epoch: u64, token: u64 },
    Slot { epoch: u64, token: u64, address: SlotAddressV1, button: SlotButtonV1, shift: bool },
    DragMove { epoch: u64, token: u64, from: SlotAddressV1, to: SlotAddressV1 },
    Drop { epoch: u64, token: u64, address: SlotAddressV1 },
}
pub enum SlotButtonV1 { Left, Right }
pub enum IntentResultV1 { Accepted = 0, Stale = 1, Unavailable = 2, Invalid = 3, Capacity = 4 }
```

The adapter lowercases enum labels for the Godot Dictionary and maps `DragMove` to `op="drag-move"`; it never serializes a Rust enum discriminant as a game protocol value. `u64` frame values must be within the signed Godot integer domain, and the token must also fit the legacy safe-integer range above. The Rust producer rejects publication outside those bounds rather than truncating.

The only accepted semantic slot addresses are `inventory:0..35`, `crafting:0..3` for inventory or `0..8` for workbench, `chest:0..26` for chest, and `furnace:0..2` for furnace. For personal crafting commands, the unified crafting view uses grid `0..8` and player slots `9..44`; an inventory-only command uses raw player `0..35`. A chest or furnace command uses player `0..35` and container slots `36..62` or `36..38`. Python never sees or calculates these unified numbers.

### Intent and result

`MornleaClientBridge.submit_container_intent_v1(intent: Dictionary) -> int` is the only panel command seam. Each Dictionary contains exact keys `epoch`, `token`, `op` and the operation's fields; extras, wrong types (including booleans masquerading as integers), invalid addresses and non-finite values reject the entire intent. The return codes are `0=accepted`, `1=stale`, `2=unavailable`, `3=invalid`, `4=capacity`. Accepted means a local Rust UI transition occurred or one server command was queued; it does not mean a server move was confirmed. Neither Godot nor Rust changes confirmed slot counts until the server publishes a newer confirmed mirror. No local retry or replay follows rejection.

| Operation | Required fields | Result |
| --- | --- | --- |
| `close` | none | Personal inventory close is a local Rust UI transition. Workbench, chest and furnace close queue exactly one ordered `CloseContainer` request before hiding the panel and invalidating its token. No Python-only close. |
| `recipe` | `index: 0..9` | Select read-only recipe in Rust. |
| `take-output` | none | Send one crafting-output request in inventory/workbench when nonempty. |
| `slot` | `area`, `index`, `button: left/right`, `shift: bool` | First click selects any valid slot, including an empty one, as Rust source; second left moves whole stack, second right moves half, second Shift+right moves one. Shift+left on any valid slot sends one quick-move request and clears previous source; the server decides whether an empty-source request is rejected. |
| `drag-move` | `from_area`, `from_index`, `to_area`, `to_index` | Same whole-stack command as two left clicks; same-slot clears source without command. |
| `drop` | `area`, `index` | Send one whole-stack drop request; the server chooses count/placement. |

The Rust handler checks in order: exact schema/field types and operation shape (`invalid`); session epoch and token (`stale`); play phase, current panel, cursor capture and server session (`unavailable`); confirmed mirror for every operation except `close` (`unavailable`); current-kind address and semantic admissibility (`invalid` for an impossible address, `unavailable` for empty crafting output on `take-output`); then queue capacity only if the operation would emit a server command (`capacity`). `close` is valid on an open panel even when its inventory mirror is unconfirmed. A first slot click, recipe selection, same-slot cancellation and personal-inventory close are local Rust UI transitions: they return `accepted`, emit zero commands and remain available even when the server-command queue is full. Workbench/chest/furnace close must first enqueue one ordered `CloseContainer` in that same bounded gameplay-command queue; only on successful enqueue may Rust hide the panel and invalidate its token. If the queue is full or send fails, it returns `capacity` or `unavailable` and keeps the panel, token and confirmed mirrors unchanged. If the server later rejects the close, Rust republishes the still-confirmed container mirror with a new token, preserving its slots; workbench grid cells 4..8 are returned only by the authoritative server's close result. Every rejection occurs before source/recipe mutation. A full bounded command queue never silently drops a command. Capacity is at most 64 commands per frame with a fixed upper bound of 1 MiB for the entire encoded batch; the producer must reject overflow without partial append. Legacy parity permits empty slots to be recorded as a source and permits empty-slot quick-move or drop requests; the authoritative server may reject the eventual command. The source remains selected if a furnace output is clicked as a destination; output may be a source but never a destination. Clicking the selected slot clears source with no network command.

When accepted, Rust maps personal inventory-only moves to inventory view, any move touching the personal crafting grid to crafting view, and chest/furnace moves to the appropriate container view and server-issued container identity. The second right click's `shift` bit selects single versus half; the server derives the count. An intent that queues a server command changes only Rust UI source selection and queue state. A server rejection or correction leaves visible slots at the last confirmed values; the next frame carries the accepted mirror and new revision. A panel identity change clears source and recipe selection and issues a new token.

### Godot resource and lifecycle rules

The existing host samples one typed frame and calls each active feature's `apply_typed_frame(frame)`. The `containers` root implements `bind_host(services_path: str) -> str`, `activate_feature(epoch: int) -> str`, `reset_feature(epoch: int) -> None`, `deactivate_feature() -> None`, and `apply_typed_frame(frame: Dictionary) -> str`; child panels implement the same four lifecycle/apply methods and `set_slot_intent_sink(services_path: str) -> str`. Scripts communicate through Godot scene-path `.call`, not project-Python sibling imports. The root may acquire the same bridge node through `get_node_or_null(services_path)`; it must test `submit_container_intent_v1`, `container_ui_schema_version() == "1.0"`, and `client_core_producer_kind() == "rust"` before activation. The C1 schema method returns an empty string when the selected producer lacks the UI family and has no Go fallback.

Create 36 player slots, 9 crafting slots, one crafting output and 10 recipe controls in the personal panel; create another 36 player slots, 27 chest slots and 3 furnace slots in the storage panel. Both scenes stay instantiated, for a fixed upper bound of 122 interactive slot/recipe Controls; only one panel is visible and input-enabled. The repeated 36-slot player view is a bounded presentation cache, not a second gameplay mirror. No slot node or texture is allocated per frame. The Python validation pass inspects at most 76 slot records (36+9+27+3+1), 10 recipe records and their 100 nested slot records, then applies the view atomically to the active fixed nodes. It rejects overlong/short arrays, unknown kind, wrong revision, non-finite progress, and malformed source, hides the panel, disables intent and returns a stable error string. Reset/deactivate clears source highlight, tooltip, hover/focus, cached token and queued callback references. Focus loss disables intent and hides tooltip without mutating Rust gameplay state. At zero viewport size the panel hides; narrow viewports use scroll containment so all slots remain keyboard reachable. Godot Control's slot rect is the sole hit target; tooltip uses mouse-ignore and viewport-clamped coordinates.

The initial Godot theme freezes these tokens from the current [UI token source](../../../../packages/engine/crates/mornlea_client/frontend/src/tokens.css) and [container layout source](../../../../packages/engine/crates/mornlea_client/frontend/src/ui/game.css); C2 stores them once in `shared/panel_theme.tres`, and C4/C5 consume the same resource instead of choosing separate styling. The font is the generated `res://assets/generated/fonts/NotoSansCJKsc-Regular.otf`. P12 still judges declared renderer/font differences case by case; matching numeric tokens does not grant pixel handoff.

| Token | Frozen Godot value |
| --- | --- |
| Panel surface / slot paper / dark edge | `#fff5df` / `#f8ead0` / `#4a3826` |
| Primary / secondary text | `#3d2e20` / `#7a6449` |
| Sage focus / wheat progress | `#7e9c63` / `#d9a94e` |
| Scrim / panel shadow | `#4a3826` at 32% / 28% opacity |
| Panel width / compact width / max height | 740 / 520 px / 88% of viewport height |
| Panel padding / major gap / minor gap | 16 / 12 / 6 px |
| Slot side / narrow minimum / border | 40 / 24 / 2 px |
| Panel / control corner radius | 24 / 12 px |
| Title / subtitle / small text | 26 / 16 / 12 px |
| Recipe rail / recipe row | 180 / 32 px |

The panel is centered with a 6 px viewport inset and vertical scroll when contents exceed its maximum height. The 9-column slot grid shrinks each cell toward 24 px before horizontal scrolling; it never changes semantic order or expands an invisible hit region. Initial semantic/visual candidate identities are `panel-inventory`, `panel-inventory-empty`, `panel-inventory-full`, `panel-workbench`, `panel-chest`, `panel-furnace` at 1280×720 and `panel-inventory-narrow` at 360×640, matching the existing fixture names. Include a 640×360 interaction geometry case, a zero-size hide case, focus loss and reset; C6 records any rendering difference rather than silently changing the shared theme.

The shared drag router owns one pointer gesture; panels own no independent drag state. A confirmed nonempty slot's primary press arms it with the current epoch/token, semantic source and down position. The gesture activates only when squared pointer movement reaches 9 px², matching the current 3 px threshold. A release below threshold follows the ordinary `slot(left,shift)` click path exactly once. On an active drag, release over another slot emits one `drag-move`, over the same slot cancels with zero intents, over panel whitespace cancels with zero intents, and outside the panel emits one `drop` from the original semantic address. The target slot handles its release and marks the event handled; panel whitespace calls `release_on_blank`; only the overlay's unhandled release calls `release_outside`. A static ghost Control follows the pointer while active, ignores mouse input and never becomes a hit target. Right-click during drag, Escape, pointer cancellation, focus loss, token change, reset and deactivate cancel without a command. The router clears the gesture before invoking the bridge so reentrant signals cannot emit twice.

Every nonempty `item_id` resolves to `res://assets/generated/ui-icons/%03d.png`; no Python per-frame base64 decode, raw protocol parse or `ImageTexture` creation. The Go generator copies pixels from `assets.Registry.ItemIconRGBA` into deterministic 16×16 RGBA PNGs and the existing generated manifest records their digests. Empty slots use no icon resource. Resource loading is cached by `item_id` for the feature lifetime and bounded by registered items; failed loads reject feature activation. This is a generated derivative of current authorized assets, with retained provenance.

## Dependency and exclusive ownership

```text
accepted F1 + F2 + F3 → C1 Rust view/intent → C2 shared slot/harness
                                    C2 → C3 item icons     ┐
                                    C2 → C4 personal panel ├→ C6 integration/parity → P12 evidence
                                    C2 → C5 storage panel  ┘
```

Each C-node runs one Luna Worker at `max` reasoning after its predecessors pass; C3, C4 and C5 may run concurrently in separate isolated worktrees, with at most three subagents active. Each node gets an independent Sol review after its focused tests. The controller alone edits `tasks.md`, `design.md`, `ledger.md`, cross-change producer contracts and integration rulings. A reviewer uses `gpt-6-sol` at `high` reasoning; no Astra agent is dispatched. If this task runs under Cursor Superpowers, use project `superpowers-implementer` and `superpowers-reviewer` cards for the respective roles.

The current branch's protocol files, Go real-time UI controller, React/WebView source, existing HUD, tracked visual images, and P12 producer records are read-only for every C-node. C1 may read the Go code and fixture corpus; it does not port or expand the Go client. Workers report any contract conflict with file/line and a failing case to the controller; they do not select another API.

## Review focus

1. Token reuse across chest reopen or epoch reset must never submit into the new panel (C1, C6).
2. Shift+right on the second click must request a single item, while right without Shift requests half; neither changes local counts (C1, C4, C5).
3. Furnace output must be a valid source and an invalid destination, preserving an existing source on rejected destination (C1, C5).
4. A malformed 63-slot chest view must fail atomically before any node changes, even if the first 62 slots are valid (C2, C5).
5. Focus loss, zero-size viewport and narrow keyboard navigation must not leave an invisible live hit target or stale tooltip (C2, C6).

### Fixture and expected-result table

The C1 fixture helper `container_case(id)` owns these immutable starting views and expected semantic commands; C2's Python fixture helper `godot_container_frame(id)` projects the same named views into Dictionaries. C1 creates `testdata/runtime-migration/client/ui-containers.json` with these case IDs and the exact fields above; C2 reads that file, not a second hand-authored inventory. Nonmentioned slots are exact empty records, and every case has `schema=1.0`, `epoch=7`, `revision=40`, `confirmed=true`, `cursor_free=true`, `recipe_index=-1`, `source=null`, `progress=0`, and `burn=0` unless overridden here.

| Case ID | View and stimulus | Expected result / unchanged data |
| --- | --- | --- |
| `personal-left` | `kind=inventory`, `grid_size=2`, `token=101`, `inventory[0]=(stone,8)`, `inventory[1]=empty`; `slot(inventory,0,left,false)` then `slot(inventory,1,left,false)` | First returns accepted with source `inventory:0`, no command; second returns accepted with one `MoveInventoryStack(0,1)`, clears source. Both confirmed counts remain 8/0 until server publication. |
| `personal-half` | Same view, token 102; source click `inventory:0`, then `slot(inventory,1,right,false)` | Exactly one `MoveStackPartial(view=inventory,from=0,to=1,single=false)`, no local count change. |
| `personal-single` | Same view, token 103; source click `inventory:0`, then `slot(inventory,1,right,true)` | Exactly one `MoveStackPartial(view=inventory,from=0,to=1,single=true)`, no local count change. |
| `empty-source` | `kind=inventory`, token 105, `grid[1]=empty`, `inventory[0]=empty`; click `crafting:1` then `inventory:0` | First click records `crafting:1` even though empty; second emits one `MoveCraftingStack(from=1,to=9)` and clears source. The server may reject it; confirmed counts stay empty. |
| `empty-direct` | Same empty personal view; Shift+left `inventory:0`, then `drop(crafting:1)` in separate input turns | Each emits one valid semantic `QuickMoveStack` or `DropStack` request despite empty source; server rejection leaves confirmed slots empty. |
| `workbench-grid` | `kind=workbench`, `grid_size=3`, token 104, `inventory[0]=(stone,8)`; source `inventory:0`, target `crafting:8` | Exactly one `MoveCraftingStack(from=9,to=8)`; nine grid hit targets, no local count change. |
| `chest-reopen` | `kind=chest`, token 202, `inventory[0]=(stone,8)`, `chest[0]=empty`, private container identity B; prior closed view used token 201/identity A | An intent with token 201 is stale, no command or source change; source+target on token 202 emits one `MoveContainerStack(container=B,from=0,to=36)`. |
| `furnace-source` | `kind=furnace`, token 301, `furnace[2]=(iron,4)`, `inventory[0]=empty`, `progress=0.375`, `burn=0.25` | Source `furnace:2` then destination `inventory:0` emits one `MoveContainerStack(from=38,to=0)`; bars show 0.375/0.25. |
| `furnace-target` | Same view with `inventory[0]=(stone,8)` and selected source `inventory:0` | `slot(furnace,2,left,false)` returns unavailable, source remains `inventory:0`, no command and no count change. |
| `personal-close` | Open personal inventory token 401, no container mirror | `close` returns accepted, emits no server command, hides panel and issues token 402 in a higher frame revision. |
| `storage-close` | Open workbench/chest/furnace token 501, confirmed container mirror | `close` queues exactly one `CloseContainer` before hiding and invalidating token; no crafting grid or storage slots are changed locally. A full queue prevents closure, and a later server rejection republishes the same confirmed mirror with a fresh token. |
| `capacity` | Any confirmed open case with 64 server commands already queued for its frame | The next command-producing intent returns capacity, queue length stays 64 and all confirmed slots/source remain unchanged. A `recipe` selection or first slot click on the same full queue returns accepted with no new command and publishes a higher coherent frame revision. |
| `invalid-last` | A complete view whose final `output` slot has `count=65` | Godot validator returns a stable error, panel is hidden, no Control apply or bridge intent occurs. |

Other named tests vary one field of these cases: `epoch=8` with token 201 checks reset staleness; `revision=39` checks out-of-order publication; `confirmed=false` disables every slot operation; missing `container_ui` or wrong schema rejects feature activation; a zero-size viewport hides slot hit rectangles. The C1 and C2 helpers must assert every required field is present, so an added producer field cannot silently shift the oracle.

## C1: Rust container contract

**Deliverable and predecessors:** P10 task 2.3a; accepted F1/F2/F3 producer and compatible immutable frame identity. Reviewable Rust view/intent contract, bridge projection and real semantic replay. No Godot Control implementation.

**Editable files:** create `packages/engine/crates/mornlea_client_core/src/ui/containers.rs`; modify that crate's `src/ui/mod.rs` and `tests/ui_contract.rs`; create `packages/engine/crates/mornlea_godot/src/container_ui.rs`; modify its `src/lib.rs` and `src/bridge.rs`; extend `testdata/runtime-migration/client/ui-containers.json` (new, controller-approved offline characterization). Read-only: current `packages/client/cmd/mornlea/app/app_game_ui.go`, `packages/client/client/ui_game_bridge.go`, current specs and F3 accepted crate guide. If F3 creates a different public module or bridge entry, stop for controller reconciliation before editing.

**Interface produced:** Rust `ContainerViewV1`, `SlotV1`, `RecipeV1`, `SlotAddressV1`, `ContainerIntentV1`, `IntentResultV1`; `ClientSession::container_view(&self) -> ContainerViewV1`; `ClientSession::submit_container_intent(&mut self, intent: ContainerIntentV1) -> IntentResultV1`; bridge `submit_container_intent_v1(Dictionary) -> i32`, `container_ui_schema_version() -> String`, `client_core_producer_kind() -> String` (exactly `"rust"` or `"go-pilot"`), and `frame["container_ui"]`. All producer structs use owned or immutable borrowed data; no Rust reference escapes a published frame.

**Test first:** Add named tests `container_view_has_fixed_shapes_and_frame_identity`, `local_selection_advances_frame_revision_without_server_message`, `stale_token_cannot_move_new_chest`, `second_right_click_selects_half_or_single_without_prediction`, `empty_slot_can_be_selected_and_submitted`, `empty_quick_move_and_drop_reach_server_boundary`, `local_selection_succeeds_with_full_command_queue`, `close_container_is_ordered_and_capacity_bounded`, `rejected_close_reopens_confirmed_container`, `furnace_output_destination_preserves_source`, `invalid_area_and_capacity_reject_atomically`, `rejection_keeps_confirmed_counts`, and bridge tests `container_projection_matches_frame_identity`, `container_intent_rejects_wrong_field_types`. Use the recorded legacy trace for one inventory, workbench, chest and furnace command and assert exact Rust semantic command (view, address, single flag) plus unchanged confirmed slots. Red must be a failed behavioral assertion against an existing method, not merely an unresolved import; register target and test case first.

**Implementation sequence:** (1) Record the exact characterization input/expected command table in the JSON fixture, including container identity, panel token, 36/27/3 slots and rejection. (2) Define the bounded structs and deterministic projection; check all lengths and scalar domains before publishing. (3) Implement `submit_container_intent` with the precedence above and the address map `personal inventory i → crafting i+9; personal grid i → crafting i; storage inventory i → container i; chest i → container i+36; furnace i → container i+36`. (4) Apply local source/recipe/personal-close transitions without a gameplay command; queue exactly one typed command for each command-producing operation, including workbench/chest/furnace `CloseContainer`, leaving mirror slots unchanged until confirmation. (5) Project to Godot Dictionary and parse exact input keys at the GDExtension boundary; catch panics into a stable unavailable result. (6) Add concise English ownership, lifecycle and compatibility comments.

**Red/green and gate:** `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test ui_contract --locked -- --list` must discover all named cases. Run the same command without `-- --list`; expected red is specific behavior mismatch, green is all named cases passing. Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked` and `rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all --check`. Reviewer inspects all producer/consumer field matches and rejected queues. Commit `feat(ui): publish rust container views and intents` after controller verification. Rollback selects the previous adapter/disabled feature without changing server or save data.

## C2: Shared slot Control and harness

**Deliverable and predecessors:** P10 task 2.3b after C1 contract/review. Fixed reusable slot Control, tooltip, atomic view parsing helper and nonzero test runner. It does not render a complete panel.

**Editable files:** create `apps/mornlea-godot/features/containers/AGENTS.md`, `shared/slot_control.py`, `shared/slot.tscn`, `shared/drag_router.py`, `shared/drag_router.tscn`, `shared/view_validation.py`, `shared/view_validator.tscn`, `shared/panel_theme.tres` under that feature; create `scripts/godot/container-check.sh` and `scripts/godot/container_shared_check.py`. Read-only: C1 contract, current HUD and host examples, root/engine/scripts guides. The directory guide maps the feature's Rust-to-Python boundary, scene-path method calls, fixed nodes and focused checks; child folders inherit it.

**Interface produced:** `slot_control.apply_slot(slot: Dictionary, selected: bool) -> str`, `slot_control.set_intent_sink(services_path: str, drag_router_path: str, epoch: int, token: int, area: str, index: int) -> str`, `slot_control.reset_slot() -> None`, `slot_control.set_input_enabled(enabled: bool) -> None`; `drag_router.bind_sink(services_path: str, epoch: int, token: int) -> str`, `arm_primary(epoch: int, token: int, area: str, index: int, slot: Dictionary, x: float, y: float) -> str`, `move_pointer(x: float, y: float) -> None`, `release_on_slot(area: str, index: int) -> int`, `release_on_blank() -> None`, `release_outside() -> int`, `cancel_drag() -> None`, and `is_active() -> bool`; `view_validation.validate_container_frame(frame: Dictionary, epoch: int, previous_revision: int) -> Dictionary` with exact keys `ok: bool`, `view: Dictionary` (empty on failure), and `error: str` (empty on success), plus `view_validation.validate_icon_catalog(manifest_path: str) -> str`. `bind_sink` reacquires the native bridge by scene path, requires `submit_container_intent_v1`, and replaces the cached epoch/token after cancelling an old gesture; C4/C5 call it before binding their slots and again on token change. `release_on_slot` and `release_outside` return 0 after a handled active gesture and 1 when no active gesture was present, allowing ordinary click dispatch only in the latter case; they invoke the bridge internally and do not return its result. `validate_icon_catalog` reads the existing generated manifest once per feature activation, checks each listed `ui-icons/*.png` path via ResourceLoader and returns a stable error for a missing or malformed file. Because project Python sibling imports are forbidden, validator and router are scene children called through `.call`; returned views contain only primitive Godot Dictionary/Array values. The slot scene owns one icon TextureRect, count label, durability bar, focus style and mouse-ignore tooltip; the router owns one mouse-ignore ghost.

**Test first:** `container_shared_check.py` uses `unittest` fake Godot nodes and has `test_slot_forwards_exact_semantic_address_once`, `test_slot_does_not_mutate_count_on_accepted_or_rejected_intent`, `test_invalid_76th_slot_fails_before_first_apply`, `test_tooltip_ignores_pointer_and_clamps_to_viewport`, `test_reset_clears_token_focus_hover_and_icon`, `test_zero_viewport_hides_hit_rect`, `test_drag_threshold_keeps_short_press_as_one_click`, `test_drag_cross_area_emits_one_complete_intent`, `test_drag_release_blank_cancels_but_outside_drops`, and `test_drag_interruption_cancels_without_intent`. The cross-area case arms `inventory:0`, moves from `(10,10)` to `(13,10)`, releases on `chest:0`, and expects exactly `{"epoch":7,"token":202,"op":"drag-move","from_area":"inventory","from_index":0,"to_area":"chest","to_index":0}`; the identical release at `(12,10)` remains an ordinary click. The 76th record case sets `output.count=65` after all prior slots are valid and expects validation error, zero `set_text` calls, and no intent call. Register `container-check.sh --group shared|personal|storage|integration` to run only the named Python test file for that group and fail if zero tests are discovered.

**Implementation sequence:** (1) Create tests/runner and observe the targeted failures. (2) Build the slot scene with direct signal handlers for left/right and route primary pointer gestures through one router child; only a nonempty confirmed primary press arms drag, while every valid slot retains ordinary click semantics. (3) Implement the 3 px activation, cross-area release, same-slot/whitespace/outside settlement and interruption cancellation above; clear router state before bridge submission. (4) Validate all view arrays and nested recipes into a temporary bounded projection, then apply a complete projection; a malformed record produces no partial UI mutation. (5) Cache icons by item ID, load only from the generated path, and cap cached entries to the current registered catalog size. (6) Clear transient visual state on reset/deactivate/focus loss. Keep all new first-party comments in English.

**Gate:** `scripts/godot/container-check.sh --group shared` (at least ten named cases discovered), `make godot-project-check`, `git diff --check`. Expected red is a named behavioral assertion; expected green includes no project-Python sibling imports. Reviewer checks that generated icon absence fails activation rather than masking a populated slot. Commit `feat(ui): add bounded godot container slot control`. Rollback leaves the reserved containers feature disabled.

## C3: Deterministic item icons

**Deliverable and predecessors:** P10 task 2.3c after C1/C2 field and resource contract; may run in parallel with C4/C5 after C2 review. Every registered nonempty item gets an exact 16×16 RGBA PNG derived from the current registry, with manifest digests and provenance.

**Editable files:** `packages/client/cmd/mornlea-godot-assets/main.go`, create `packages/client/cmd/mornlea-godot-assets/main_test.go` and `packages/client/cmd/mornlea-godot-assets/AGENTS.md`, regenerate only `apps/mornlea-godot/assets/generated/manifest.json` and `apps/mornlea-godot/assets/generated/ui-icons/*.png`. Read-only: `packages/client/assets/item_icons.go`, `packages/shared/core/item.go`, `scripts/godot/sync-assets.sh` and the existing asset notices. The new guide records the offline generator's registry, staging, manifest and focused test boundaries. Do not edit atlas pixels or existing source art.

**Interface produced:** exactly `res://assets/generated/ui-icons/%03d.png` for each `core.RegisteredItem(id)` with `1 <= id < core.ItemIDMax`; no file for item 0 or unregistered IDs. PNG row order and RGBA bytes exactly equal `Registry.ItemIconRGBA(id)`. The existing manifest's `outputs` array includes every icon path, length and SHA-256 and remains sorted.

**Test first:** `TestItemIconsCoverRegisteredItems` enumerates `ItemIDMax`, decodes every generated PNG, compares all 1024 RGBA bytes with the registry, rejects extra/missing item files, and checks deterministic rerun. `TestItemIconsRejectMissingRegistryPixelData` injects an absent icon through a small generator helper and expects a hard error without replacing the generated tree. Both fail on the current atlas-only output.

**Implementation sequence:** (1) Add test helper that builds to a temp `generated` directory and invokes `materialize`. (2) Iterate registered IDs in ascending order, construct `image.NRGBA` with 16×16 stride 64, copy the read-only registry pixels, and encode with `png.Encode` to zero-padded filenames. (3) Keep the staged tree's atomic comparison/replacement logic and manifest collection unchanged so `--check` detects drift. (4) Run the generator once for the tracked derived output; inspect asset notices and unexpected files.

**Gate:** from `packages/client`, `go test ./cmd/mornlea-godot-assets -count=1`; from root, `scripts/godot/sync-assets.sh --check` and `git diff --check`. Reviewer checks every new binary is derived from current registered art and is listed by the manifest. Commit `feat(assets): generate godot item icons`. Rollback restores the previous generated tree as one scoped commit while the container feature remains disabled.

## C4: Personal inventory and workbench

**Deliverable and predecessors:** P10 task 2.3d after C1/C2. Two Godot Control panel layouts using the shared slot methods; no root catalog changes.

**Editable files:** create `apps/mornlea-godot/features/containers/personal/personal_panel.py`, `personal/personal_panel.tscn`, `scripts/godot/container_personal_check.py`. Read-only: C1 field contract, C2 shared slot, old behavior fixture, current P11 input contract. C4 cannot edit C5 storage files, generated assets, bridge or feature root.

**Interface produced:** `personal_panel.apply_container_view(view: Dictionary) -> str`, `reset_feature(epoch: int) -> None`, `deactivate_feature() -> None`, `set_slot_intent_sink(services_path: str) -> str`, `set_input_enabled(enabled: bool) -> None`, `release_on_panel_blank() -> None`, `release_outside() -> int`, and `cancel_drag() -> None`. It renders 36 player slots as three inventory rows plus nine hotbar cells, 4 active crafting cells for inventory or 9 for workbench, one output, ten read-only recipe rows and one selected recipe preview. It instantiates exactly one C2 `DragRouter` child; each shared slot receives that router's scene path and its semantic `area` and zero-based `index`.

**Test first:** `test_inventory_has_36_player_4_grid_10_recipe_targets`, `test_workbench_has_9_grid_targets`, `test_recipe_selection_sends_index_without_crafting_prediction`, `test_take_output_requires_confirmed_nonempty_result`, `test_personal_right_click_preserves_counts_until_confirmation`, and `test_grid_size_mismatch_hides_panel`. The tests feed a complete C1 fixture, assert exact node counts and bridge intent dictionaries, and assert no local `count` write after `accepted` or `stale`.

**Implementation sequence:** (1) Create fixed nodes and the tests. (2) In `set_slot_intent_sink`, call the router's `bind_sink` before binding every slot; on a new epoch/token, bind it again and cancel the old gesture. Route every clicked slot through C2's sink/router; the panel never adds nine to inventory indexes. Panel whitespace calls `release_on_panel_blank`, while the feature overlay alone calls `release_outside` for an unhandled release. (3) Render recipe names/material preview from the published ten records; recipe selection sends one `recipe` intent and waits for `recipe_index` in a later frame. (4) Render crafting output but only its button emits `take-output`; no output-as-target slot. (5) Hide inactive 2×2/3×3 cells and cancel drag on malformed/unconfirmed view.

**Gate:** `scripts/godot/container-check.sh --group personal` must discover six cases and pass; run `scripts/godot/container-check.sh --group shared` and the Rust `ui_contract` target's personal cases. Reviewer checks the 36/4/9/10 address map and no optimistic count. Commit `feat(ui): render personal and workbench controls`. Rollback removes the isolated panel files while catalog still excludes the feature.

## C5: Chest and furnace

**Deliverable and predecessors:** P10 task 2.3e after C1/C2; may run concurrently with C4 because files and tests are disjoint.

**Editable files:** create `apps/mornlea-godot/features/containers/storage/storage_panel.py`, `storage/storage_panel.tscn`, `scripts/godot/container_storage_check.py`. Read-only: C1 contract, C2 slot scene, old chest/furnace behavior fixture. C5 cannot edit C4 personal files, generated assets, bridge or feature root.

**Interface produced:** `storage_panel.apply_container_view(view: Dictionary) -> str`, `reset_feature(epoch: int) -> None`, `deactivate_feature() -> None`, `set_slot_intent_sink(services_path: str) -> str`, `set_input_enabled(enabled: bool) -> None`, `release_on_panel_blank() -> None`, `release_outside() -> int`, and `cancel_drag() -> None`. Chest shows 27 chest +36 player slots; furnace shows input/fuel/output +36 player slots, progress and burn bars. It instantiates exactly one C2 `DragRouter` child; output slot can be selected as source but has no destination hit action.

**Test first:** `test_chest_has_63_semantic_addresses`, `test_furnace_has_39_semantic_addresses`, `test_furnace_output_is_not_move_destination`, `test_furnace_progress_is_published_value_only`, `test_chest_second_right_and_shift_right_emit_one_intent_each`, and `test_chest_malformed_last_slot_is_atomic`. A furnace fixture with `progress=0.375` and `burn=0.25` must render those exact ratios; advancing a local timer must not alter them.

**Implementation sequence:** (1) Create scene and failing named cases. (2) Assign `inventory`, `chest`, and `furnace` addresses directly to fixed nodes, without unified-index arithmetic in Python; `set_slot_intent_sink` binds the router's service path/epoch/token first and rebinds on identity change. (3) Display published ratios in fixed TextureProgressBar/ProgressBar nodes and never extrapolate. (4) Use C2's source highlight, tooltip and exact intent sink/router; panel whitespace cancels drag, while an unhandled outside release is passed to `release_outside` by the feature overlay. Block target affordance on furnace output while still showing it as a possible source. (5) Hide controls and cancel drag on malformed view, reset or zero viewport.

**Gate:** `scripts/godot/container-check.sh --group storage` must discover six cases and pass; run the shared suite and named Rust chest/furnace cases. Reviewer checks 63/39 mapping, output restriction and no local progress prediction. Commit `feat(ui): render chest and furnace controls`. Rollback removes the isolated panel files while catalog still excludes the feature.

## C6: Integration and parity

**Deliverable and predecessors:** P10 task 2.3f after accepted C1–C5 commits, their Sol reviews and controller interface check. Complete disabled-by-default integration evidence; enabling requires the explicit parity gate below.

**Editable files:** create `apps/mornlea-godot/AGENTS.md`, `apps/mornlea-godot/features/containers/containers_feature.py`, `apps/mornlea-godot/features/containers/feature_root.tscn`, `apps/mornlea-godot/features/containers/feature-required.tres`, `apps/mornlea-godot/config/container_candidate_catalog.tres`, and `scripts/godot/container_integration_check.py`. The root Godot guide records app/host, features, platform, generated assets, profile selection and focused validation without duplicating the child container guide. Keep the reserved `apps/mornlea-godot/features/containers/feature.tres`, current `config/feature_catalog.tres`, and `app/host/app_root.py` read-only; the pilot/default product selection remains unchanged. The controller alone updates this change's `ledger.md` and `tasks.md`. Read-only: the three completed worker trees, F3 producer identity, P12 visual producer records. No edits to the HUD coordinator or P11 input feature.

**Interface produced:** host-compatible `containers_feature` lifecycle/apply methods defined above. The candidate manifest uses `dependencies = PackedStringArray("session")`, `required_bridge_families = PackedStringArray("6@1.0")`, `metadata/required = true`, `metadata/enabled = true`, `metadata/entry_scene_path = "res://features/containers/feature_root.tscn"`, and binding-time `container_ui_schema_version() == "1.0"`, `client_core_producer_kind() == "rust"`, and `validate_icon_catalog("res://assets/generated/manifest.json") == ""` checks. `container_candidate_catalog.tres` lists exactly `res://features/session/feature.tres` followed by `res://features/containers/feature-required.tres`; the host orders their dependency. Missing Rust producer, schema, embedded runtime, icons, or scene makes activation return `PlanResult(ok=false)` and tear down the partial feature tree; it cannot be reported as an optional disable. The current pilot catalog and its disabled reserved manifest remain unchanged.

**Test first:** `test_required_candidate_catalog_fails_startup_without_rust_schema`, `test_required_candidate_catalog_fails_startup_without_embedded_runtime`, `test_frame_epoch_revision_mismatch_hides_all_panels`, `test_reopen_same_chest_rejects_old_token`, `test_focus_loss_and_resize_clear_hit_targets_and_tooltip`, `test_reset_deactivate_and_reactivate_clear_old_callbacks`, and `test_view_intent_replay_matches_four_legacy_scenarios`. Test the `feature_host.activate_catalog` result as `ok=false`, not merely a disabled ID. Run a real headless Godot scene lifecycle check under the pinned embedded runtime in addition to fake-node unit cases; startup-only smoke is insufficient. Any missing embedded runtime is a blocker, not a passing substitute.

**Implementation sequence:** (1) Wire two panel scenes and validator scene as children; bind by service scene path and reject missing method/family. (2) Apply one complete `container_ui` view to exactly one panel after checking frame epoch/revision and schema. (3) Route focus/resize/reset to both panels and cancel both routers; panel whitespace handles its own release, and the overlay's unhandled primary release calls only the active panel's `release_outside` exactly once. No stale callback may retain an old token. (4) Compare ordered semantic intent/view transcript to the frozen Rust characterization for inventory, workbench, chest and furnace, including stale/rejected cases. (5) Only after all gates, create the required candidate manifest/catalog; leave the current pilot catalog unchanged. (6) Record rollback by selecting the current pilot catalog/previous release producer explicitly; candidate activation failure is fatal for that candidate profile, not an optional disable.

**Gate:** `scripts/godot/container-check.sh`, `make godot-python-check`, `make godot-project-check`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test ui_contract --locked`, and the F3 Rust-session headless smoke mode from its accepted ledger. The controller records discovered/executed cases, fixture digests, tested SHA, Sol review findings, and rollback result. Commit `feat(ui): integrate godot container feature` only after focused gates. P12 candidate visual evidence and explicit human producer handoff remain P10 task 3.2, outside C6's acceptance.

## Controller readiness and completion review

Before each dispatch, the controller rereads proposal, delta spec, design, tasks, this packet and all scoped guides; rechecks the accepted prerequisite SHA; inventories hashed/generated/embedded consumers of each editable file; verifies no current Worker owns an overlapping path; and sends only the node's contract, paths, tests, exclusions and acceptance. A producer conflict or newly discovered consumer returns to the controller for artifact revision, never to a Worker's discretion.

After C1, compare Rust structs, Godot Dictionary keys and Python validator fields byte-for-byte and confirm the slot/intent address table. After C2, verify C4/C5 can consume the exact same slot interface. After C3–C5, run both panel groups against one integrated frame and all negative cases. After C6, review each requirement of the P10 delta against `tasks.md`: container semantics may pass while menus, character, HUD parity, P12 evidence and full closeout stay open. Record `Architecture skill: no change` unless a verified cross-task rule qualifies for promotion. A green planning validation or empty test target cannot close an implementation node.
