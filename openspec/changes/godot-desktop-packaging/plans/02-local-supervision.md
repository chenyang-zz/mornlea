# P13 local desktop supervision contract

This is the controller-owned compile-ready seam for P13 1.2. The new Rust client core, F2 opt-in server binary/lease, F3 TCP endpoint and P13 release manifest are prerequisites. The types below are planned declarations until 1.2 commits compiling code and a behavioral failure/success double. Workers 2.1a–2.1c receive that accepted SHA and implement only their named module. `mornlea_client_core` never becomes the world writer.

## Shared process and readiness types

```rust
pub struct ChildSpec {
    pub binary: CanonicalPackagePath,
    pub binary_sha256: [u8; 32],
    pub release_id: ReleaseId,
    pub world: CanonicalWritableWorldPath,
    pub bind: LoopbackSocketAddr,
    pub activation_manifest_sha256: [u8; 32],
    pub readiness_nonce: [u8; 32],
}
pub struct GroupHandle(NonZeroU64); // private process-local identity
pub struct ReleaseId([u8; 32]);
pub struct ChildIdentity {
    pub group: GroupHandle,
    pub pid: NonZeroU32,
    pub launch_generation: NonZeroU64,
    pub binary_sha256: [u8; 32],
}
pub struct LeaseProof {
    pub world_identity: [u8; 32],
    pub writer_generation: NonZeroU64,
    pub activation_manifest_sha256: [u8; 32],
}
pub struct LaunchTicket(NonZeroU64);
pub struct MonotonicDeadline(Instant);
pub enum FlushState { NotRequested, Pending, Durable, Failed }
pub enum StopCause { Closed, Cancelled, ReadyTimeout, ChildExited, WrongIdentity, ForcedKill, Io }
pub struct StopReport {
    pub child: ChildIdentity,
    pub direct_child_reaped: bool,
    pub live_group_members: u32,
    pub flush: FlushState,
    pub lease_released: bool,
    pub cause: StopCause,
}
pub enum LaunchState {
    Starting(Option<ChildIdentity>),
    Ready { child: ChildIdentity, proof: ReadyProof },
    Stopping { child: ChildIdentity, flush: FlushState },
    Reaped(StopReport),
    Failed { error: LaunchError, report: Option<StopReport> },
}
pub struct ProcessObservation {
    pub direct_child_reaped: bool,
    pub live_group_members: u32,
}
pub trait ProcessGroup {
    fn spawn(&mut self, spec: ChildSpec) -> Result<ChildIdentity, LaunchError>;
    fn request_stop(&mut self, group: GroupHandle) -> Result<(), LaunchError>;
    fn force_kill(&mut self, group: GroupHandle) -> Result<(), LaunchError>;
    fn poll_reaped(&mut self, group: GroupHandle) -> Result<ProcessObservation, LaunchError>;
}
pub struct ReadyProof {
    pub nonce: [u8; 32],
    pub release_id: ReleaseId,
    pub server_binary_sha256: [u8; 32],
    pub protocol_version: u32,
    pub world_lease: LeaseProof,
    pub bind: LoopbackSocketAddr,
}
pub enum SupervisorState { Idle, Starting, Ready, Stopping, Reaped, Failed }
```

`CanonicalPackagePath`, `CanonicalWritableWorldPath` and `LoopbackSocketAddr` have private fields and checked constructors. The package path must resolve inside the selected hashed release package without symlink escape; the writable world path is outside the package, names one existing world/backup pair and is never silently created after a read error. `LoopbackSocketAddr` accepts only `127.0.0.1` or `::1` and a nonzero port. The world lease proof projects accepted F2 S3 ownership into an owned local record; it is not a client-held writable lease or an imported server type. The server acquires its lease before mutable read and publishes its proof before F3 connect. A second active or not-yet-reaped group for the same world is `LeaseBusy`; a stale ticket or reused process identity is `WrongIdentity`. The launcher cannot start a second writer.

`ReleaseId::try_new([u8;32])` rejects the zero package digest. `LaunchTicket` and `GroupHandle` are private monotonic nonzero identities with checked exhaustion; they never equal an OS PID. `MonotonicDeadline::try_after(now:Instant, budget:Duration)` accepts budgets in `(0,30s]` and uses checked addition. Start defaults to 10 s, stop to 5 s; platform and fake providers receive the same injected clock. `ChildIdentity` is tied to its process-group handle and launch generation, so a reused PID cannot authorize a stop. `LaunchState` owns its records and no borrowed process handles. Clean stop requires reaped direct child, zero live group members, Durable flush and released lease; every other result remains explicitly failed or forced. A timeout with descendants still alive retains the slot and blocks retry.

The child sends `ReadyProof` through the versioned local control channel after it has acquired the F2 lease and bound loopback TCP. The supervisor compares every field, including the unpredictable nonce and actual package/binary hash, then changes `Starting→Ready` once. Stdout/stderr are diagnostics only. A wrong proof, timeout, child exit or broken control channel yields a typed error and triggers group teardown; a late proof cannot revive a stopped launch. The F3 client connects only after authenticated readiness and then uses the ordinary S2 login path. Memory mode remains a separate F2/F3 parity test, not the local desktop launcher path.

The serial launcher integration owns both ends of the prospective local-control adapter: server binary, `mornlea_server/src/activation/local_control.rs` and its activation test, plus client `local/control.rs` and its test. No client-core dependency on server/storage is introduced. The child inherits one dedicated control handle; the planned binary options are `--supervision-control <handle>` and `--readiness-nonce <64-hex>`. Ordinary opt-in launch without these options is unchanged. The channel uses version 1, a four-byte little-endian length and exactly one UTF-8 JSON object of at most 4096 bytes per record; unknown/duplicate fields, excess bytes and wrong version/nonce fail closed. Closed variants are `Ready(ReadyProof)`, `Stop {nonce}`, and `Stopped {nonce,flush,lease_released}`. Hex digests encode exactly 32 bytes; world/generation/manifest values come from the acquired F2 lease and activation manifest. The server reports Durable/released only after real successful F2 shutdown. Each poll processes at most one complete record; the bounded I/O worker owns reading/writing and the core frame never waits. `ControlPoll` is `Pending|Record(ControlRecord)|Closed`; `LocalControl::try_send(ControlRecord)->Result<(),LaunchError>` and `poll()->Result<ControlPoll,LaunchError>` have one pending inbound/outbound record each, and full admission returns `Io` with no partial record. Contract examples pin exact encode/decode bytes for Ready, Stop and Stopped, cap+1, wrong nonce, duplicate readiness and failed flush. Accepted golden bytes and both real adapters are required before target qualification.

`Supervisor::start(ChildSpec, MonotonicDeadline) -> Result<LaunchTicket, LaunchError>` reserves one launch and submits bounded spawn work; package/path hashing and manifest validation run before this call in the preparation worker. It returns without waiting for network or disk. `Supervisor::poll(LaunchTicket, Instant) -> Result<LaunchState, LaunchError>` advances one bounded state-machine step. `Supervisor::cancel(LaunchTicket, MonotonicDeadline) -> Result<LaunchState, LaunchError>` requests F2 graceful flush once and returns Stopping; subsequent polls check progress and force entire group termination at the injected deadline, finally returning a StopReport in Reaped/Failed. Cancellation never blocks until a report exists. `StopReport` names direct-child exit, observed live group/descendant count, flush status, lease release and forced-kill reason; it cannot report clean exit while a group member or lease remains. Parent exit uses the same group-kill ownership via OS lifecycle primitives. Unix uses a dedicated process group, signals/kills the group, verifies no live group members remain and reaps its **direct child**; it cannot claim to reap grandchildren owned by the OS. Windows uses a kill-on-close Job Object and closes/reaps owned handles. Platform modules do not implement readiness or lease policy a second time.

`LaunchError` is closed to `InvalidManifest`, `WrongArchitecture`, `InvalidPath`, `NonLoopback`, `LeaseBusy`, `WrongIdentity`, `ReadyTimeout`, `ChildExited`, `Cancelled`, `ForcedKill`, `Io`, `Internal`. Validation order is manifest/hash/path/target, lease reservation, group spawn, proof validation, client connect. An error before spawn leaves no child or lease; an error after spawn returns a report with teardown/reap state. A failed F2 flush is a hard release failure and preserves the named recoverable backup; forced kill is reported even if reaping succeeds.

Starting(None) represents an accepted spawn request whose worker has not returned a child yet. Cancel in that state invalidates the request generation; any late child result is immediately torn down and cannot become Ready. Once a ticket exists, child/control failures are published as Failed with the retained StopReport during poll; the launch slot is not freed until group and lease absence are verified. Only malformed calls or a stale ticket return an immediate method error. The dedicated Unix control handle is close-on-exec for every unrelated descendant. The serial server local-control adapter owns an independent native EOF watchdog: lost parent/control EOF requests normal F2 shutdown and, after the same injected stop deadline, kills its entire dedicated process group even if the core cannot finish flushing. It never claims a clean flush on forced exit. Ordinary parent teardown uses the provider's stop/kill/reap path; after unexpected parent death the OS owns reaping, and actual-host tests observe zero surviving group members from an independent process. Unix process-group creation alone does not prove parent-death cleanup. Windows retains the kill-on-close Job Object path. Real lost-parent tests are mandatory before each platform report.

The 1.2 double must behaviorally red on an orphaned grandchild, mismatched nonce and a second writer, then green with complete teardown and no lease. The 2.1a generic tests run against this fake. The Unix and Windows providers run actual child/grandchild tests on the corresponding host; cross-compiled code or a fake alone cannot qualify the target. P13 2.4 binds the actual `StopReport`, package hash and backup identity into each target release report. P14 uses those same identities in each independently built release cycle.
