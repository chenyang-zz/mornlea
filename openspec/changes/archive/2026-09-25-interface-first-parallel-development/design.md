## Context

The project already requires controller-owned exact APIs, file sets, task DAGs, and linked briefs. Its active Rust numerical closure illustrates a usable interface landing: `rust-native-numerical-closure` plans shared typed contracts and test doubles before eleven providers, with `ffi.rs` and corpus registration serialized. Read-only source inspection confirms that landing is planned rather than implemented; it is an example of the dependency shape, not acceptance evidence. The existing provider-aware orchestration policy controls whether an agent is delegated. This change controls whether implementation tasks are ready to run independently.

## Goals / Non-Goals

**Goals:** Give every multi-task plan a decidable interface-first test, a minimum contract packet, an accepted landing, a dependency/ownership graph, separate conformance and integration evidence, and a change-control path. Make those rules usable by human or agent implementers across Go, Rust, Python, Godot and cross-language contracts.

**Non-Goals:** A universal runtime interface framework, one trait per task, automatic parallel dispatch, an agent count change, or implementation of the numerical closure.

## Decisions

### Select the boundary, not an interface layer for every task

The controller first lists producer/consumer edges and marks each boundary as existing-stable, new/changed-shared, or private. Only a new/changed boundary consumed by at least two independently reviewable tasks needs a separate contract landing. A single-provider change can carry its API with its implementation node. The contract may be a Rust trait, Go interface, function signature, schema, message, fixture, or semantic record; dynamic dispatch is introduced only when a runtime consumer genuinely needs it. This avoids an abstract plugin framework that would obscure ownership and add public surface without enabling real concurrency. The controller records why a shared landing is unnecessary when a plan has no eligible boundary.

The contract owner is the lowest stable domain owner that all consumers may depend on without a cycle. Its public surface contains semantic values and operations, never provider-private storage, byte parser state, network handles, or mutable global state. For each operation the controller freezes signature and field map; valid and invalid ranges; units, coordinate systems and ordering; ownership, lifetime and mutability; result publication; error precedence and capacity units; cancellation/concurrency and work limits; compatibility/version behavior; deterministic success, failure and boundary examples. Existing cross-language formats retain their established version authority. Shared declarations and registries have one editor.

### Land a compile-ready contract before implementation lanes

The contract landing is one independently reviewed task. It creates the minimal public declarations, validated constructors or parsers needed for real fixtures, topic test registration where required, at least one deterministic consumer double per operation shape, and tests that compile and execute success and typed failure paths. It publishes a commit SHA in the change ledger. A production operation must not return a fabricated success or panic as a placeholder. If a language needs module files to compile before providers exist, create empty compiling modules and reserve their exact files for provider owners. No worker starts from prose alone or from an unaccepted landing.

The landing deliberately freezes observable behavior rather than private algorithms. Provider workers may organize local helpers inside their exclusive files; they cannot edit the shared contract. Consumer work depends only on the public contract, and double-based tests are labeled consumer tests. The contract landing itself does not prove provider behavior.

### Represent parallelism as a DAG plus integration locks

Use the graph `contract landing → independent providers/consumers → shared adapter or registry integration → real end-to-end gate`. An edge exists for actual data, file, version, or behavioral dependence; an exclusive edit lock serializes a common file without inventing a functional dependency between otherwise independent providers. Each task packet records baseline contract SHA, direct predecessors, exact editable and read-only files, output interface, test cases and command, exclusion, integration owner and rollback unit. Different worktrees or exclusive file sets can implement ready nodes from the same accepted SHA. A task is ready only after all predecessor evidence is accepted, the file sets do not overlap, and a concrete failing/green test route exists.

Shared ABI adapters, save/protocol migrations, manifest or corpus registries, generated outputs, and final acceptance remain controller-owned serial points. The controller merges reviewed results one at a time and reruns affected downstream tests on the merged SHA. Concurrent task readiness does not change the project's agent delegation ceiling or justify delegation solely for speed.

### Separate three kinds of evidence

Contract evidence proves the surface compiles, validation examples run and a consumer can use a double. Provider evidence proves the real implementation meets the same success/error/boundary cases and resource limits. Integration evidence executes the real producer and consumer together, including an independent oracle or compatibility corpus where relevant, on one recorded SHA. A double, a compile-only call, or an inventory label cannot substitute for the latter two. The ledger records the case set and identity for each gate; `tasks.md` remains the sole status source.

### Resolve drift centrally

A worker sends the controller the verified source, failing case and affected signature or semantic rule, then pauses only dependent work. The controller decides whether source or contract is wrong, updates `design.md`, delta specs when behavior changes, all affected briefs and contract tests, and lands a new contract SHA. Affected workers rebase and rerun their gates; independent work unaffected by the changed surface may continue. No provider may silently broaden an interface to make its local test pass. Versioned wire/save/ABI changes follow their existing migration and exclusivity gates.

### Placement and validation

Root `AGENTS.md` states the mandatory trigger and points to the detailed bilingual guide. `openspec/config.yaml` and the synchronized orchestration skill reference make dispatch readiness explicit. `docs/interface-first-parallel-development.md` and its Chinese counterpart hold the decision table, contract packet template, DAG and acceptance checklist. `docs/development-process*` and `docs/README*` link to this authority without copying it. `docs/documentation-manifest.json` tracks the new pair and revised paired document revisions. This governance change uses static artifact coherence, documentation/audit gates and OpenSpec strict validation; no runtime test or benchmark is claimed.

## Risks / Trade-offs

- **Premature public surface** → Freeze only shared semantics needed by multiple tasks and permit private provider choices.
- **False parallelism from disjoint files** → Check data, version and integration dependencies as well as file ownership.
- **Doubles mistaken for completion** → Require distinct provider and real integration evidence with recorded SHA.
- **Contract drift after dispatch** → Central ruling, new SHA, affected brief updates and revalidation.
- **Governance duplication** → Keep the full method in one bilingual guide; other entry points link or state only their scope-specific trigger.

## Migration and rollback

Apply this rule to future or materially revised multi-task plans. Do not retroactively reinterpret completed task evidence. The planned Rust numerical contract landing can be used as a readiness example; its implementation remains a separate OpenSpec change. Revert this governance unit and its mirrors together if the rule is withdrawn; no runtime data migration is involved.
