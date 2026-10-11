# Shared authoritative interaction classification

Controller node2.1c0 lands one compile-ready classifier before the concurrent
container and later combat consumers. Main owns S/src/core/interaction.rs,
core/mod.rs, rules/projectiles.rs only extraction of its existing target_block,
tests/server_contract/interaction.rs and root registration, plus artifacts/guides.
No other implementation file changes. S=packages/engine/crates/mornlea_server.

Public core::interaction::target_block(view:&AuthorityReadView<'_>,
dimension:Dimension,pos:BlockPos,block:u16)->bool is a pure read of the caller's
already observed cell. Air0 and fluids27..34 are false. Door lower62..69 is
true for even/closed and false for odd/open. Upper70 queries the same dimension
at y-1: a recognized open lower is false; closed, missing, unready or a
non-lower form is true. Every other ID is true, including plants/glass and
unregistered scalar IDs, matching Go InteractionTarget (registration belongs
to chunk validation). Never reinterpret target as collision/support/harvestability.
The caller remains responsible for observing the original cell and its Ready
failure; the helper grants no mutation authority. Source Go core/raycast.go204
and entity/mining.go168; existing projectiles.rs target_block already matches.

Write REDs before implementation using a total inert true classifier: literal
source ID matrix0..89 plus unknown; all8 lower forms under upper; same-coordinate
Overworld/Depths isolation; unavailable/nonlower/bottom-of-world lower fallback.
Tests assert no world effects and callable results, not source text. Copy the
accepted projectile predicate into this module; switch projectiles to import it
and remove its duplicate without changing flight/missing-cell behavior. F1 DDA
walk remains with each provider; do not conflate projectile unknown-cell policy
with interaction refusal.

Run pinned server_contract interaction, server_replay projectiles, full server,
clippy/fmt, focused Go core Raycast and entity interaction ray tests (list actual
matches), strict OpenSpec/diff and independent review. Scoped commit freezes SHA.
Main then supplies exact container consumption update: its ray match skips any
observed !target_block rather than only air, and actual Ready submerged/open-door
rays must bind the target behind them. Core mutation, workbench and sleep
migration is a separate serial node2.1c1; tools may deliberately target water
for bucket collection and must retain its operation-specific predicate. Combat
consumes the same accepted classifier without copying gameplay rules. Main owns
all integration/rollback and reruns affected consumers. No wire/save changes.
