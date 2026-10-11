# Source interaction ray math and consumers

Required execution skill: executing-plans. Main used brainstorming/writing-plans;
status remains tasks.md. Goal: all existing authority interaction rays consume
one accepted source classifier and ray math, with source-specific water pickup
and unavailable-cell policies retained. No new gameplay scope or schema version.
S=packages/engine/crates/mornlea_server. Source authority: shared/core/raycast.go
and server/sim/entity/{command,mining,door,container,sleep,bucket}.go. Refer to
../design.md and plans/18-interaction-target-contract.md.

## Small shared math landing (main)

Main owns S/src/core/interaction.rs, S/tests/server_contract/interaction.rs and
artifacts/guides. Existing target_block contract5f8325b2 remains unchanged.
Add public look_direction(yaw:f32,pitch:f32)->[f32;3], preserving source f64 trig
then f32 cast BEFORE component multiplication. Inputs normally come from checked
LookAngles; nonfinite raw scalar results receive no authority and downstream
normalization rejects them. Add public normalized_direction(direction:[f32;3])
->Option<[f32;3]>: reject any nonfinite component or f64 hypot length<1e-6;
length=hypot(hypot(x,y),z), inverse=(1.0/length) as f32; result=input*inverse.
Do not use f32 squared sums, per-component division or normalize look before
source AABB tests. These pure helpers do not grant Ready/target authority.

Test-first inert helpers produce behavioral REDs. Exact independently generated
Go math.Float32bits KATs (source operation order, no Rust-derived expectations):
look(0,0)=[80000000,00000000,bf800000]; look(.1,.2)=[bdc8621f,3e4b6ff9,bf79a4c4];
look(1.234,-.456)=[bf58ede4,bee176ea,be97e8e0];
look(3.1415927,1.5707964)=[a789aded,3f800000,b33bbd2e].
normalize[1,2,3]=[3e88d677,3f08d677,3f4d41b2]; [3,4,0]=[3f19999a,3f4ccccd,0];
[1e30;3]=[3f13cd3a;3]; [-2.8,.123,9.5]=[be90bce4,3c4b75c8,3f758996].
Zero, subthreshold, NaN and +/-infinity refuse; exactly1e-6 as f32 is below
1e-6 f64 and refuses per source, next f32 above accepts. Caller array unchanged.
Run pinned server_contract interaction, full server, clippy/fmt/diff and scoped
commit. Record accepted SHA before any dependent task uses new helpers.

## Existing consumer migration (isolated worker)

Editable S/src/core/mutation.rs; S/src/rules/{mining,world_mutation,crafting,
sleep,tools,containers}.rs, only their private ray/look helpers, imports and
obsolete ray comments/constants. Editable matching S/tests/server_contract/
mutation.rs and S/tests/server_replay/{mining,world_mutation,crafting,sleep,
tools,containers}.rs, only ray consumer regressions and source-invalid assertions.
All other files readonly. Main owns guides/artifacts/integration/rollback.
Baseline includes accepted math SHA from landing and classifier5f8325b2;
container migration998a6be1 is also required. No new shared surface needed.
No source-hashed/generated consumer covers these Rust files. No new directory.

Enumerated existing NativeRaycast producers: core/mutation; rules/mining,
world_mutation,crafting,sleep,tools,containers,projectiles. Projectiles already
uses classifier and source segment normalization, and is READONLY: do not change
its ordinary f32 segment length, unavailable-cell no-collision policy or bounded
512-cell flight traversal. Combat and hostile LOS are separately owned future
consumers; their accepted packet demands the same source math.

Remove private look_direction duplicates and import accepted math. Preserve
caller error/None shapes when normalized_direction returns None. All existing
walkers still own traversal and Ready errors; replace AIR-only hit predicates
with !target_block(view,dimension,cell,observed.block). Tools removes its duplicate
door/fluid classifier: ray_target is `(collect && block==27) || target_block(...)`;
keep is_fluid where block mutation still needs it. Tools must normalize its look
before NativeRaycast, which it previously omitted. No new error policy, leases,
crafting lifecycle, output, reach or selected-item behavior in this node.

Source door executeInteractDoor uses blockRaycastSampler even for a door command.
Thus an open lower or its upper is transparent; its old replay assertion that
an upper over open lower closes the door is wrong. Replace that exact case with
source passthrough: put a second closed valid door behind, assert only that pair
opens and the front open pair stays unchanged. Missing cells after a transparent
cell still refuse; do not reinterpret a passed open door as a fallback target.
This is a controller ruling from Go door.go178, not worker discretion.

Concrete RED/GREEN acceptance, using existing topic fixtures and compact Ready
chunks when testing end-to-end authority:
1. mutation:: resolve_mine and resolve_place select the solid BEHIND water27,
   flowing34 and open lower63/upper70. Resolve mine target exactly; placement
   still respects occupied water destination policy, never skips debit checks.
   Source-invalid water placement is allowed to refuse separately from correct
   target selection. Closed lower62 blocks and remains the selected target.
2. mining progress with the same Ready corridor pins target behind water/open
   doors and completes that target at its required ticks; the front cells remain
   unchanged. Tracker and accepted completion resolver must agree, with tool
   wear/output once. Missing traversed observation clears progress safely.
3. workbench Open through water and open door sets Workbench; closed door refuses
   unchanged inventory. Bed night entry through water/open door records actual
   foot anchor; closed door refuses/no bed record. This node does not accept
   missing anchor lifecycle or actual Ready open ownership in crafting.
4. door source correction above and closed target behind water; unavailable
   observation after water refuses unchanged pair/inventory/events.
5. tools source27 pickup with empty bucket succeeds and flowing water remains
   transparent toward later source; hoe/bone meal pass water/open doors, closed
   doors block. Existing bucket full-source/output-capacity tests remain green.
6. existing actual Ready container regression remains green; add diagonal look
   with source math if needed to exercise integrated ray setup, not text scan.

Run focused nonzero tests BEFORE changes and record real behavior REDs. After
migration run pinned server_contract mutation and interaction, replay mining,
world_mutation,crafting,sleep,tools,containers plus projectile regression, full
server, clippy all-targets -D warnings/fmt/diff. make rust before focused Go
shared/core Raycast|InteractionTarget and sim/entity ray/door/bucket tests; list
actual source matches. Independently review consumers and source-corrected
assertion, then scoped commit. Main integrates and owns final combined evidence.
No benchmark/performance claims or actual reducer acceptance follow this node.
