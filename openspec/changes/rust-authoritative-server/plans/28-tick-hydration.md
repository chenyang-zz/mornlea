# Tick hydration for live adapter flows

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: unblock node 3.7 by hydrating live ticks that adapter flows can
actually run: session saves become tick actors, drained chunk completions
become Ready overlay. New node 3.7a; original 3.7 keeps its number and
follows. No save-format, transport, or provider-behavior changes.

## Provenance ruling (controller)

A read-only scout proved production ticks run empty overlays: session
`PlayerSave` bodies are write-only after install, drained chunk results sit
in `chunk_results` forever (`world_acquisition::run` is a deliberate no-op
awaiting a drain caller), and `TickContext::for_tick` starts actors/ready
empty. Fixtures bypass all of it via `from_fixture`/`preload_*`. Nothing
here re-litigates accepted providers; this node adds the missing production
assembly only.

## Ownership

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/src/core/hydrate.rs (new): pure mapping plus the tick-start driver.
- S/tests/server_replay/hydration.rs (new replay topic) plus index
  registration following existing topic patterns.
- S/src/core/step.rs: call the driver at tick start (before row 1).
- One doc-comment line on `preload_ready_chunk` noting production use.
All other files read-only, including saves/transport/providers/ports and
guides. Main owns guides, artifacts, integration and rollback. No new
phases, effects, wire/save changes, Go edits, or world scans. No new
directory beyond the two files.

## Frozen mapping rule (no invented defaults)

Every `ActorRecord` field copies 1:1 from its save: players from
`PlayerSave` (position/motion/survival/inventory/armor/crafting), hostiles
from hostile-mob saves, passives from passive saves, companions from
companion saves; runtimes take existing neutral defaults (cite the
hostile-actor neutral staging and sleep fresh-synthesis conventions).
Chunk drain: `drain_chunks` results feed `apply_drained` with wants built
from active sessions' radius-2 interest (the reducer's active-key
derivation, reused); Ready lands through `preload_ready_chunk`. Order:
hydrate actors first, then chunks, both before row 1. Any save field
without an obvious record counterpart, or any family whose mapping needs a
judgment call, stops the whole node with evidence — partial hydration is
worse than none.

Acceptance is overlay assertions (actors present with exact fields, Ready
present with exact revisions), not new counters: hydration returns counts
the reducer logs nowhere, recorded here so 4.1 can reconcile counter
fidelity if it chooses. No behavior change to any provider.

Out of scope with pointers: subscriber despawn/drop projection (absent
diff derivation; follow-up blocks 3.8, not 3.7a); save cadence (scheduler);
agent install feed (endpoint); TransportAuthority production impl (3.3
test-doubles stand; real-adapter selection is 3.8 activation scope).

## RED/GREEN and acceptance

RED: live-shaped tick (sessions installed, chunk results admitted, no
fixture preloads) runs empty — actors absent, Ready absent, providers
skip; the new replay tests pin the absence first. GREEN: same setup
hydrates exactly (field-by-field asserts, revision asserts), providers
then run (motion advances a hydrated player; mining sees a hydrated
Ready chunk). Run pinned focused hydration replay, full server, clippy
all-targets -D warnings, fmt, diff. Independent review covers mapping
exactness (no defaults), drain ordering, and the documented exclusions.
Worker scoped English commits allowed (mapping, then driver), main
integrates, updates guide/tasks/ledger and reruns integration gates.
Refusal/conflict returns evidence to main; no invented policy. Main owns
rollback and revalidation.
