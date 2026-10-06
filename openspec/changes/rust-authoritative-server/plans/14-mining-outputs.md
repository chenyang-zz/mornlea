# Human mining output completion

Node2.8b3c consumes accepted atomic world outputs9c3ab40e and pure harvest
samplers f087d018. Main froze source algorithms after the independent audit.
This node completes the existing resolver/progression output branches; it does
not accept the real reducer or implement Q/panel/death/environment producers.

Worker edits only S/src/core/mutation.rs, src/rules/mining.rs,
tests/server_contract/mutation.rs, tests/server_replay/mining.rs. Read-only
state/contracts, harvest.rs, all other rule/test files, registries, guides and
plans. It runs in an isolated checkout; main integrates and owns rollback,
review, status and final commit. Worker is not alone and preserves others.

## Shared local policy and resolution

Move the exact `mining_rule(block:u16,held:u16)->(u16,bool)` table from mining.rs
to a pub(crate) function in core/mutation.rs and import it in mining.rs. This
is a single node's internal dependency, not a worker-designed shared interface.
Correct the existing mining.rs crop predicate: exact37..44 or46..61 excludes
Workbench45; workbench must take15 ticks and be companion-mineable. Do not widen
source mineability (torch remains required0 in Go miningRule despite BlockDrop).
Keep frozen required ticks/tool table, source selected slot and current actor
basis. Resolver refuses required0 as ProtectedBlock; completion provider clears
progress on unmineable/missing target as before. It does not reimplement progress
inside resolver or accept a client harvestable flag.

resolve_mine derives inventory/tool preimage before output capacity, determines
harvestability from the same selected stack/table, then builds the full private
BlockTxn. Reuse existing wear_selected_tool, including sword/crop-hoe/grass/
sapling exemptions; success wears once, refusal never wears. The progression
provider removes its stalled-short-grass branch and completes through resolver
normally. Existing human refusal clears progress; companion capacity retains
saturation. Preserve bow/command suppression gates.

Source Go entity/mining.go:697-1145 determines output order:

- Door62..70: derive lower from struck half (upper70 is one above lower), require
  both current observations and valid world Y, clear lower then upper in one
  transaction. If harvestable, one ItemDoor43 at LOWER exact integer cell,
  including an upper-half hit. Missing counterpart or stale preimage refuses
  both and tool/drop outputs. Do not require a matching counterpart form beyond
  existing container-ownership guards; source clears chosen coordinates.
- Bed76..83: derive foot/head from source orientation0South/1West/2North/3East;
  head forms are foot+4. Clear foot/head atomically; one ItemBed46 anchored at
  STRUCK half if harvestable. Cross-chunk unavailable/MAX/stale refuses whole.
- Furnace/chest: capture actual fixed slot from accepted container_at. Body
  stack only when harvestable, then furnace input/fuel/output or27 chest cells
  in slot order regardless of harvestability. Empty entries may be omitted.
  Entire batch/capture/block/tool settle once. Wrong tool with empty contents
  needs no drop capacity, and wrong tool must not destroy nonempty contents.
- ShortGrass84: use harvest::short_grass(seed,dim,target). Hit outputs one
  wheat seed34; miss outputs none, bypassing all capacity gates. Both clear,
  no tool wear. Same position retries cannot reroll by completion tick.
- Snow85..88: keep existing no-drop clear and tool rules.
- Immature wheat37..43→one seed34, potato46..52→one potato40,
  carrot54..60→one carrot41 when harvestable. Mature wheat44 uses wheat tuple
  in wheat35 then seed34 order; mature potato53 uses potato count then optional
  poison potato42x1; mature carrot61 uses carrot count. Calls use environment
  seed and view.tick() completion tick; no mutable RNG or reroll at commit.
- Leaves19 when harvestable: one leaf22, then optional sapling57 using
  harvest::leaf_sapling. Companion keeps the source generic body only.
- Other registered mineable blocks: body from existing BlockDrop iff harvestable.
  Wrong-tool clears without a drop-capacity preflight. Air/protected still refuse.

Nonempty stacks create DropBatch using target/center policy above and existing
pickup delay; exact check_drop_batch then BlockTxn revalidation at commit.
Empty output uses drops=None (do not feed an empty checked batch). All changed
observations and captured containers remain in one accepted mutation. Companion
restrictions remain crop/farmland/torch/grass/snow/fluid refusal. Companion tool
wear is a known deferred seam: complete it now by first calculating credit on
inventory copy, then wearing the selected tool slot with the same
exemption predicate before one patch. Source credit uses its accepted AddStack
order. More precisely, Go entity/mining.go:485,512,590 credits first and then
wears the selected slot in that credited copy; preserve this even if a formerly
empty selected slot received a durable container item. If credit fails, no wear.
Do not introduce a protective exception absent from Go. The companion's body
output also respects the original selected-tool harvestability; wrong-tool
ordinary blocks clear with no credit, container contents still transfer. Bed
mining clears both halves with one inventory credit, matching Go:460-490. Door
companion mining remains the source's generic single-cell branch; do not widen
it to the separate human structural branch. Report conflicts with these exact
source policies before choosing another rule.

## Concrete RED and validation

Add real Ready tests for wrong-tool light/iron body suppression with full32 drops;
wrong-tool nonempty furnace/chest output contents without body, all-or-none capacity;
shortgrass hit/miss/stable retry with full capacity and one-tick provider completion;
last-durability exemption; leaf hit/miss and second-output capacity rollback;
all immature crops and mature wheat/potato(+poison)/carrot source counts; no extra
companion sapling roll; companion crop/grass refusal and no world drop; companion
credit+wear, newly credited selected tool source behavior, wrong-tool body
suppression, atomic bed pair and sapling exemption. Add workbench15-tick and companion mining
regression to catch the current45 crop misclassification.

Structural tests hit upper door and either bed half, prove exactly two changes
and one correctly anchored output; cross-chunk missing/stale/MAX counterpart
leaves block/inventory/tool/slots byte-identical. Existing large-coordinate
integer-drop cell and fixed container-generation tests must remain green.
Old tests that asserted wrong-tool body or stalled grass must be rewritten to
verified Go outcomes, not retained as a competing rule. Expected vectors come
from source fixtures or executed Go probes, never derive goldens from Rust.

Run pinned server_contract mutation, server_replay mining, full crate,
clippy/fmt, make rust and Go `go test ./packages/server/sim/entity -run
'Mining|ShortGrass|Leaves|CompanionMining' -count=1`, plus relevant runtime Door/
Bed mining tests. Record nonzero counts, behavioral REDs, exact source evidence
and scoped commit. Main obtains independent review before integration; rollback
reverts only this node and invalidates dependent evidence. No default runtime,
wire/save schema or new item/geometry change.

## Source suspension reconciliation

The earlier output acceptance preserved existing bow-progress/command suppression only. Packet127 separately qualifies source runtime reset/subscription, current container view and either selected bow form at the sole human mining provider; it leaves the accepted harvest/output tables unchanged.
