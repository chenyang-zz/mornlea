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
pub trait ProcessGroup {
    fn spawn(&mut self, spec: ChildSpec) -> Result<GroupHandle, LaunchError>;
    fn request_stop(&mut self, group: GroupHandle) -> Result<(), LaunchError>;
    fn force_kill(&mut self, group: GroupHandle) -> Result<(), LaunchError>;
    fn poll_reaped(&mut self, group: GroupHandle) -> Result<bool, LaunchError>;
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

`CanonicalPackagePath`, `CanonicalWritableWorldPath` and `LoopbackSocketAddr` have private fields and checked constructors. The package path must resolve inside the selected hashed release package without symlink escape; the writable world path is outside the package, names one existing world/backup pair and is never silently created after a read error. `LoopbackSocketAddr` accepts only `127.0.0.1` or `::1` and a nonzero port. The world lease proof is issued by accepted F2 S3, names world identity and writer generation, and is verified before mutable read or F3 connect. A second active or not-yet-reaped group for the same lease is `Capacity`/`InvalidState`; the launcher cannot start a second writer.

The child sends `ReadyProof` through the versioned local control channel after it has acquired the F2 lease and bound loopback TCP. The supervisor compares every field, including the unpredictable nonce and actual package/binary hash, then changes `Starting→Ready` once. Stdout/stderr are diagnostics only. A wrong proof, timeout, child exit or broken control channel yields a typed error and triggers group teardown; a late proof cannot revive a stopped launch. The F3 client connects only after authenticated readiness and then uses the ordinary S2 login path. Memory mode remains a separate F2/F3 parity test, not the local desktop launcher path.

`Supervisor::start(ChildSpec, MonotonicDeadline) -> Result<LaunchTicket, LaunchError>` reserves one group/lease and returns without waiting for network or disk. `Supervisor::poll(LaunchTicket, Instant) -> Result<LaunchState, LaunchError>` advances one bounded state-machine step. `Supervisor::cancel(LaunchTicket, MonotonicDeadline) -> Result<StopReport, LaunchError>` requests F2 graceful flush once, checks progress without blocking the Godot frame, then forces entire group termination at the injected monotonic deadline. `StopReport` names direct-child exit, observed live group/descendant count, flush status, lease release and forced-kill reason; it cannot report clean exit while a group member or lease remains. Parent exit uses the same group-kill ownership via OS lifecycle primitives. Unix uses a dedicated process group, signals/kills the group, verifies no live group members remain and reaps its **direct child**; it cannot claim to reap grandchildren owned by the OS. Windows uses a kill-on-close Job Object and closes/reaps owned handles. Platform modules do not implement readiness or lease policy a second time.

`LaunchError` is closed to `InvalidManifest`, `WrongArchitecture`, `InvalidPath`, `NonLoopback`, `LeaseBusy`, `WrongIdentity`, `ReadyTimeout`, `ChildExited`, `Cancelled`, `ForcedKill`, `Io`, `Internal`. Validation order is manifest/hash/path/target, lease reservation, group spawn, proof validation, client connect. An error before spawn leaves no child or lease; an error after spawn returns a report with teardown/reap state. A failed F2 flush is a hard release failure and preserves the named recoverable backup; forced kill is reported even if reaping succeeds.

The 1.2 double must behaviorally red on an orphaned grandchild, mismatched nonce and a second writer, then green with complete teardown and no lease. The 2.1a generic tests run against this fake. The Unix and Windows providers run actual child/grandchild tests on the corresponding host; cross-compiled code or a fake alone cannot qualify the target. P13 2.4 binds the actual `StopReport`, package hash and backup identity into each target release report. P14 uses those same identities in each independently built release cycle.
