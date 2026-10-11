# Checked hostile geometry repair

**Goal:** Storage-valid finite hostile records must never panic, alias to a different integer cell, or partially publish a hostile batch when geometry is unrepresentable.
**Spec:** ../specs/rust-authoritative-server/spec.md and ../design.md; verified review-entities.md E9 and source audit.
**Owner:** One isolated implementer for node3.9q9; controller integrates, records acceptance and rolls back its exact files.

## Scope and readiness

Baseline9ebc39ee. Existing ActorRecord/ActorRuntime/RuleEffect/AuthorityReadView and numerical contracts remain read-only. No public boundary or schema change; no parallel shared-contract landing qualifies. Editable only src/rules/hostile_actors.rs, src/rules/hostile_actions.rs, tests/server_replay/hostile_actors.rs and tests/server_replay/hostile_actions.rs beneath packages/engine/crates/mornlea_server. Read root/engine/server guides first. Exclude path cadence/target identity, core/state, contracts, shot spread and other providers.

## Controller decisions and algorithms

Existing InvalidInput{field:"actor"} is the sole refusal for unrepresentable actor-cell geometry; preserve source float order for admitted records. Finite X/Z beyond signed cell domain are not mapped to0 or MIN. No narrower global stored-coordinate contract, wrapping, clamping to a different world cell or wire reason is introduced.

- Replace private floor conversion with checked finite f64.floor in i32::MIN..=MAX returning Result<i32,ServerError>; propagate every call. Existing checked_floor/fluid_upper are the reference. Ceiling upper cells check finite f64.ceil, checked_sub(1), max(lower). This covers block_pos_of, burn column/body top and sweep-prism endpoints.
- Before constructing path or light arrays, form each endpoint via checked_sub/checked_add with existing radii16/4 and14. Derive iteration origins and chase clamps from validated endpoints, not duplicate unchecked arithmetic. Grid offsets may use addition only after endpoint proof; signed Y distance must widen to i64 before subtract/abs. Cell counts use checked or bounded i64 arithmetic before allocation. Do not change numerical grid dimensions or path algorithm.
- Spawn candidate x/z uses checked_add of signed radius24..48; unrepresentable candidate returns the same actor refusal before spawn effects. Ordinary candidate predicates/hash/axis order remain unchanged. A fully checked light window precedes scan.
- Huge finite velocity must fail checked sweep/cell budget before allocation or numerical call; it cannot generate a pathological loop or alias.
- Before a hostile action ray uses NativeRaycast, validate checked floor of all three hostile eye and target eye components. The private target-eye helper must propagate refusal. Keep existing normalized_direction/line-of-sight semantics after admission, actual executing-tick spread untouched.
- Whole HostileMotion and HostileBurnDistant are atomic for failure. Calculate up to64 HostileEntry copies (including optional spawn) and every fallible record before staging; do not stage fresh flags or an early actor while later geometry can refuse. Produce Actor/Runtime pairs for dirty entries, then one RuleEffect::Compound bounded by128 arms. advance_movement returns its private changed result without calling entry.stage. On success applied retains prior per-actor count; on failure exact context preimages remain. Burn follows the same final compound. Existing read-only hostile_actions::plan already preflights geometry before apply; do not add a second action authority.

## Concrete red/green evidence

Use actual providers and existing observed-room/helper records, not copied physics math.

1. Active hostile at [i32::MIN as f32,40,0.5] with ordinary live target refuses without panic; mirror Z, positive i32::MAX as f32 (=2147483648), finite beyond-domain values and huge finite velocity. Assert all actors/runtimes/blocks/events exact before/after on refusal.
2. A valid low-ID actor followed by invalid later actor and the reverse order both refuse without staging; include fresh valid entry to pin once-only fresh flag preservation. Before fix the MIN path window panics or an earlier actor stages.
3. Night spawn uses an extreme active-player anchor and a seed/worldtime whose hash axis pushes beyond representable cells; refuse before candidate/body/runtime effects. Cover X and Z directions. Day burn at an out-of-domain coordinate refuses without changing health, burn/distant counters or any prior actor.
4. Hurler eye and target eye independently outside signed cell domain refuse read-only plan; no projectile/cooldown/effect change. Existing actual clear shot and ordinary negative-cell shot remain admitted.
5. Near-boundary representable grid windows and ordinary negative coordinates remain valid; missing Ready observations preserve source defer behavior. Keep existing fluid upper-body, burn, liveness, ordinary chase and shot-clock known answers green.

Record compile setup separately from actual behavioral RED. If an advertised source boundary cannot be reached with existing public constructors/helpers, report exact evidence and escalate rather than fabricate unreachable acceptance or expand scope. Do not write comments containing task IDs.

## Gates and closure

Use source /workspace/.mornlea-env/env.sh, pinned Rust1.97.1, own worktree CARGO_TARGET_DIR, cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay --locked with filters hostile_actors/hostile_actions/hostile_outcomes. Nonzero tests and actual red/green required. Run four owned rustfmt files, cargo clippy -p mornlea_server --all-targets --locked -- -D warnings, git diff --check. Enumerate derived source scanners/corpus pins; no Go source or corpus-case changes authorized. Return exact commands/counts/results, full diff and a scoped fix(server): reject unrepresentable hostile geometry commit. Controller independently verifies and marks status. Production executable acceptance remains open.
