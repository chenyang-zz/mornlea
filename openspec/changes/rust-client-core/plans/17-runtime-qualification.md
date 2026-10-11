# Rust client runtime qualification plan

**Goal:** Finish the five original F3 qualification nodes through actual native
Godot/Python sessions, real F2 library providers, and source-bound closeout.
**Architecture:** Keep C1/C2/G1 unchanged. Repair native value conversion and
shutdown in their existing owners. Test-only F2 drivers feed the actual client
through the accepted common Memory/TCP adapters; no production server or default
client composition is added.
**Spec:** `../design.md`, the delta specifications, and packets 03/04/06.
**Baseline:** `4fc470aad7df1a9d1a5ed5cf771957599b3f5533` (product
`8e561c4122e179bd17d4bbf1cb2a730822fd4cd2`).

## Authorization and prerequisites

The user's 2026-10-11 request to finish the explicitly listed outstanding gates
authorizes their headless processes and disposable server fixtures. The earlier
code-only prohibition is historical for that batch. Foreground windows, default
switching, product features, production F2 changes, release deployment and remote
publication remain outside this round.

The normative refined dependency table requires F2 node 4.2 for F3 integration.
F2 4.2 is checked at its local closeout/merge `aab9b0f0c998924124567bdbe2d22003e1a55b78`;
joint S2 was accepted at `756937af0871527c93cc432059638a01a6103591`.
The older packet's phrase "F2 complete" means this declared provider/stage
prerequisite for F3, not acceptance of the separately pending whole F2 gameplay
executable. Actual current Memory/TCP providers are requalified here. F3 must
record that distinction and cannot check off another change's pending tasks.

No changed shared runtime interface is planned. Existing `ClientEndpoint`,
`CoreArena::open`, `ConnectorRegistry`, `LoginDriver` and transport contracts are
the stable boundaries. Qualification helpers are test-only. One controller owns
Cargo files, native integration, smoke scripts, inventory and status; isolated
workers own disjoint host repairs or fixture files. Discovery reports are
`/private/tmp/mornlea-f3-{native,f2}-discovery.md`.

Memory terminal ruling: F2 retires its in-process transport without encoding a
Disconnect packet. The test-only connector wraps the actual MemoryConnector and
associates each issued TransportTicket generation with its actual F2 ConnectionId.
Only actual close/common terminal phase marks that generation disconnected;
Connector::poll returns the existing Closed(Disconnected). Ordinary polls/sends
forward unchanged. Old closes and feeds are fenced by their generation and never
touch the new current peer. This uses the existing injected port, not a new
production API or forged packet. Native TCP observes actual socket retirement.

## Native host conversion and release (3.3b)

Editable: `apps/mornlea-godot/app/host/feature_host.py`,
`typing/py4godot/classes/core.pyi`, `tests/unit/test_rust_feature_host.py`,
`tests/scripts/rust_bridge_lifecycle_check.py`,
`tests/scenes/rust_bridge_lifecycle_check.tscn`, and tests-only fixture resources.
Read-only: frozen G1 packet and actual installed Py4Godot wrapper implementation.

Write regressions first using Dictionary/Array shells that reject built-in
containers in `Node.call`, matching installed Py4Godot. Assert native activation,
table negotiation, complete frame copy, same-frame fanout, reset/stale epoch,
invalid result preservation, consumer reference release before native close,
and refused-close retry without losing the token. Current shell-only tests are
insufficient. Retain all existing malformed-payload and accounting checks.

Add one private `_call_rust` conversion seam in this owner. Recursively construct
native Dictionary/Array for outbound plain structures and copy inbound native
structures to owned plain dictionaries/lists before the existing exact validators.
Preserve scalar type, null, float and canonical text; no JSON round trip or new
wire schema. Bound depth to 64 and traversal to 8 Mi nodes; bound individual and
total UTF-8 text to 64 MiB before copying. These native representation bounds
cover the accepted 8 MiB semantic frame, whose fixed keys and decimal scalar
spellings add conversion overhead. Reject invalid/cyclic/oversized trees before
fanout. No concrete feature identity or project-module import enters the host.
Use only the installed Dictionary.new0/set/keys/size and Array.new0/append/get/size.

Consumers clear held frames during `deactivate_feature`, then the host drops its
own frame before native close. A failed close retains the token, bridge and
shutdown ownership for an explicit subsequent teardown retry; no resumed tick or
activation can overwrite that retiring owner. Successful/idempotent close clears
all Rust state. Keep deferred node destruction after explicit consumer release.

Controller serial files: `mornlea_godot/src/{bridge,client_core}.rs`, existing
`tests/rust_producer.rs`, Cargo feature declaration and a qualification-only native
probe. Add explicit arena close-all for bridge exit; a failed close cannot retire
the token. Run the actual native guard with injected panic in a separate
qualification-only class, never an environment-controlled production fault.
Default artifacts retain exactly the eight-method facade. Native harness asserts
Internal, valid owned prior values, stale token refusal, duplicate-close success,
and actual release order. Test-only probe builds are explicitly identified.

Validation: host unittest discovery/run, locked Python check, Godot crate tests,
rebuilt `rust_bridge_lifecycle_check.tscn` headless with a bounded timeout.
Expected baseline native rejection is unsupported built-in dict or unreadable
native envelope; green must include actual native values, not shell substitution.
Commit: `fix(godot): qualify native client host ownership`.

## Actual F2 driver and complete integration (3.4 provider preparation)

Controller owns `mornlea_godot/Cargo.toml`, workspace lockfile and registration.
Worker editable: new `mornlea_godot/tests/support/rust_server.rs`,
`tests/rust_server_integration.rs`, `examples/rust_session_server.rs`.
Dev-only server/storage/serde_json dependencies are permitted in this qualification
target; the extension and client production dependency graph is unchanged.
Read-only: all F2 production code and its existing local_remote_parity fixtures.

Use actual `AuthorityState`, `LoginDriver::bind`, `MemoryTransport` and
`TcpTransport`, with immediate deterministic player loads as the declared store
substitution. Use the accepted existing ground/container/entity fixture values
from local_remote_parity/integration.rs; stage through TickContext between ticks.
Do not synthesize server packets as the ordinary observation producer. Run the
real reducer, encode through real outboxes, and consume through actual
`ClientCore` installed in CoreArena. Memory moves complete outbound frames to
MemoryTransport::send and delivers received controls/play frames to MemoryPeer;
acknowledge only delivered controls and retain any undelivered suffix. Fence
reset generations by replacing the peer/connection; late old peer data cannot
reach the new registry entry. Physical terminal semantics use actual server
retirement through the test-only generation wrapper above; no production Memory
callback or Disconnect packet is invented.

The predicted-audio integration adds one test-only snow-covered ground variant
using the current registered snow kinds and actual authoritative chunk facts.
The accepted grass parity fixture stays unchanged; predicted provenance must
match the real client stride source and correction, not a synthesized cue.

TCP uses the existing numeric connector and actual server accept/pump/poll/
forward/flush. Pump with bounded iterations and a 10-second test deadline, not
sleeps as protocol semantics. Match source packet facts to C2 records and rendered
G1 values, including exact IDs, optional fields, headers, and source ticks. Compare
normalized Memory/TCP semantic transcripts only where the contract permits local
epoch/frame-index differences.

Cases: real login and all ten families; all six actor kinds/upsert/removal; terrain
snapshot/delta/forget plus native prepared resource; inventory/hotbar/chest/
crafting/furnace; environment/survival/chat/task/prompt; sequenced input and
authoritative correction/replay; predicted/confirmed audio; disconnect/reset/
reconnect and stale old epochs; capacity+one and invalid mixed input unchanged
sequence; malformed frame preserves G1 visible frame; existing actual failed-save
and stale-Agent rejection evidence with exact source bindings. Distinguish injected
fault evidence from ordinary real-provider output. Empty families alone do not
accept the inventory. No additional gameplay or numerical registry change.

The example accepts `--listen 127.0.0.1:0 --ready-file PATH --stop-file PATH
--report-file PATH --max-seconds N`. It serves the same real TCP fixture, emits its
actual numeric address to ready-file after bind, pumps actual login/tick/outboxes,
and terminates on owned stop-file/deadline. It reports actual session/packet/tick
counts and errors. Its world/state is disposable and in-memory. No production
server CLI or activation manifest is repurposed. Shutdown retains failures.

Validation: `cargo test -p mornlea_godot --test rust_server_integration --locked`
with nonzero discovery; example compilation and owned readiness/shutdown probe;
actual server local_remote_parity target. Controller independently reviews fixture
semantics before accepting the helper SHA and native consumer. Commit:
`test(client-core): integrate real rust server transports`.

## Rebuilt 100-process session qualification (3.3c then 3.4 acceptance)

Controller editable: `scripts/godot/smoke.sh`, a bounded external runner under
`scripts/godot/`, tests-only `rust_session_smoke_check.py`/scene/catalog/consumer,
scoped guides and source-bound evidence. Add exact `--core rust --exercise-session`
flags. The default pilot startup test stays intact. Rebuild native bridge/probe
and the accepted fixture example; bind full source/contract, build features, native
library, Python loader/runtime and Godot executable SHA256 to a retained report.

Materialization must verify the canonical hardened Python payload before native
imports. Runtime imports then legitimately create `__pycache__` entries. Retain
the initial/final complete installed-tree digests and cache paths; freeze and
recheck the entire static tree, excluding only those generated cache entries,
plus the loader, CPython executable and shared-library identities. Noncache drift
refuses qualification. Canonical payload verification and runtime cache creation
are distinct recorded facts; no cache deletion or identity exemption is used.

Each process runs actual open/login, submits input with queued preparation,
retains an owned frame, resets/relogs/reconnects, rejects old epoch/token and late
consumer work, closes twice and tears down feature references before bridge.
Require all ten negotiated families and meaningful admitted frame records,
actual current diagnostic SHA, lifecycle markers and a per-process success report.
Fail stale/Go producer, startup-only, missing record, escaped panic, incomplete
cycle, process residue, crash report, error/leak or identity mismatch. Preserve
per-iteration logs, even on failure. Run exactly 100 separate processes.

Keep Python ambient poisoning and empty PATH/HOME isolation. Default startup keeps
the existing all-network deny. For the new real TCP fixture on this macOS host,
a dedicated sandbox profile denies all network except the one owned loopback
address/port. Native SBPL rejects a numeric host in `remote tcp`; the qualified
matcher is `remote tcp4 "localhost:PORT"`. Actual probes permit the owned numeric
127.0.0.1 endpoint and refuse IPv6 ::1 and 127.0.0.2 at that same port, unrelated
loopback ports and external access. Evidence for the native matcher is retained
under `/private/tmp/mornlea-policy-qualified.k4the4sc/`; the runner repeats the
semantic probes. Never remove the deny from the default gate. Other hosts
must refuse this new mode until an equivalent policy is qualified.

Validation: `scripts/godot/smoke.sh --iterations 100 --isolated-python --core rust
--exercise-session`, full client lifecycle suite, real native harness and complete
F2/C1/C2/G1 integration. Commit: `test(godot): qualify rust session process lifecycle`.

## Inventory and stage closeout (4.1/4.2)

### Integration-discovered acknowledgment repair

The frozen input-family table requires correlatable authoritative acknowledgments.
Real-provider discovery found that the live composition retired journal entries
without publishing Confirmed receipts. Repair this in the existing serial owners,
without a C1/C2/G1 schema change. Controller-owned files are `src/input.rs`,
`src/session/{login,publication}.rs`, `src/presentation/assembly.rs` and
`tests/session_replay/live.rs` in `mornlea_client_core`.

Write real public-endpoint regressions before implementation: cumulative PlayerState
acknowledgment publishes exact sequence/kind/tick once; duplicate/backward packets
and chat never invent confirmations; actual PlaceBlockSucceeded confirms its held
placement sequence with no invented source tick and is not repeated by later
PlayerState. Keep a private bounded session confirmation queue, sourced from the
existing journal before retirement. Reserve capacity before changing prediction or
journal; cap all pending confirmations plus input metadata by `family_records`. Reuse
one private combined-count check before admission, rejection facts and confirmation
reservation; refused publication cannot let subsequent ingress overfill the family.
Mark separately confirmed journal entries so repeated source receipts cannot emit
again. Rebase their headers to the complete frame revision, attest each receipt
against the consumed real acknowledgment observation, and clear only after successful
whole-frame publication. Reset/close follows the existing epoch owner. No receipt is
created for unissued/rejected/chat sequences. Run full core/Godot suites and real
Memory/TCP provider tests after integration; commit this coherent fix separately.

### Integration-discovered native NIL repair

Actual admitted-frame fanout exposed a defect in the pinned upstream runtime:
`Variant::construct_Nil` constructs INT zero and `construct_Nil_native_ptr` passes
32-bit storage to the 64-bit INT constructor. None therefore becomes an integer,
including uninitialized upper bits. This is a runtime correctness defect, not a
facade schema exception. Preserve the failed native log and add exact Dictionary
and Array None round-trip assertions before implementation.

The two constructors must invoke actual `variant_new_nil` on their respective
native storage. Rebuild the owning `main.dylib` from the exact verified upstream
source and generated C++/Cython API headers; keep release wrappers only after
generated IDs, capsule signatures and actual native marshalling prove compatibility.
No JSON coercion, sentinel, raw-pointer Python, binary patch, symbol interposition
or DYLD bypass is accepted. The existing loader build alone cannot repair these
exported main-library functions.

Worker owns `scripts/godot/py4godot/build-main.py`, its focused test, a third minimal
patch and patch series. Its helper consumes verified/patched source and staged
Python roots, writes an owned external output, generates only required C++ and
nine API headers with offline pinned Cython 3.3.0, compiles deterministically under
the pinned compiler/SDK with at most four jobs, and retains failures. It never
installs. Controller owns builder integration, new Darwin derivative identity,
actual runtime installation/verification, native tests and rollback. Calibrate
new main/loader/full-payload digests from the reproducible candidate before
installation; existing verification cannot be disabled to install unknown bytes.
Other platforms require their own accepted rebuilt main identity before claiming
this derivative. Record platform refusal honestly; no Linux runtime acceptance is
inferred from macOS.

### Stage acceptance

### Integration-discovered TCP retirement repair

Native100 at round-five source `908a099282a1ad35736d0ce7ba17abcb089299a8`
failed in process12 after11 complete cycles: reset's new epoch was rejected as
the same player still online. The TCP connector returned close success after
setting a cancellation flag, while its detached worker still held the active
socket until a bounded read returned. Engine-free TCP reset qualification also
closed the server peer explicitly, hiding the actual client retirement race.

The controller retains the frozen nonblocking C1 close/reset contracts. Reuse
the same single OS socket through an `Arc<TcpStream>`: the worker owns read/write
waits, and its generation's private shared owner holds only a shutdown reference.
No cloned descriptor, worker join, reconnect delay, retry of a real rejection,
player-ID replacement, authority change or new public interface is accepted.
Worker binding and close acquire the socket-control mutex before the state mutex;
close retains the connector-inner mutex until retirement. Before mutating queues,
flags or ticket ownership, close performs `shutdown(Both)` on an established
socket. `NotConnected` means already retired; every other shutdown error is typed
Io and preserves retry ownership. Successful shutdown drops the shared reference,
clears the old state, removes its ticket and wakes its worker. Binding after a
close or connector drop refuses and drops its freshly dialed stream before any
handshake publication. Worker I/O remains outside every mutex. Drop cancels and
shuts down established sockets without joining workers.

One isolated worker owns only client-core `src/session/io.rs`, its focused tests
and the crate guide if materially needed. It writes actual socket-owner tests
before the minimum repair: a held data-side reference cannot prevent peer EOF
after close; a closed generation cannot bind/publish a late connection; released
and foreign tickets retain their typed errors. Native100 is the existing actual
behavioral red; no synthetic frame is substituted. Validate pinned core session
tests, all-target core Clippy and formatting; commit this coherent repair.
Controller owns the provider fixture/test files, integration and rollback: TCP
reset must not call the server-side close helper, while Memory keeps its declared
transport pairing. Add repeated same-player actual TCP reset/admission coverage
with old-epoch refusal and sequence restart, retaining normal admission rejection.
Run actual-provider39 plus the additive regression, fresh independent review,
same-source native100 and complete stage gates. Preserve both failed rounds and
their original criteria. Scope still excludes production F2 or default changes.

### Final stage acceptance

Reconcile every existing capability-inventory row to actual nonempty provider and
integration cases; preserve fixtures, IDs, versions and test definitions. Record
explicit rollback selection of the pilot, without default change or shadow writer.
Update guides and append ledger evidence/status only after the corresponding
full acceptance passes. Independent review binds the final immutable source.
Run `make rust`, `make rust-check`, `make dev-check`, all-six-module `make test-race`,
uncached audit and strict OpenSpec with the established qualified Python/temp/
previous-runtime environment. Reuse evidence only for byte-identical same-source
owners; failures retain their actual exit status. Evidence-only advancement needs
an independent diff review. Architecture skill promotion considers only verified
durable findings. Product F2/P8-P14 status remains separately owned.
