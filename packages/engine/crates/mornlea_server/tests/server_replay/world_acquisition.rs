//! Chunk acquisition and stale-generation rejection replay.
//!
//! Every gate below mirrors the accepted Go runtime rather than choosing a
//! local policy:
//!
//! - `packages/server/sim/runtime/engine_subscription.go` (`applyAcquired`,
//!   `applyGenerated`): a completion installs only while its key is still
//!   wanted and still in the expected in-flight state; a completion for a
//!   forgotten key is dropped or unloaded and never publishes.
//! - `packages/server/sim/runtime/engine_step.go` (`StepWithTunables`): the
//!   acquire work runs after companion actions and before physics, the seam
//!   position `RulePhase::Acquire` occupies in the frozen phase order.
//! - `packages/server/sim/runtime/persistence_lifecycle_test.go`
//!   (`TestEngineForgottenCleanAcquiredHitIsDeleted`,
//!   `TestPersistenceLifecycleRetainsLateGeneratedChunkForSaving`): a
//!   forgotten key's late hit publishes nothing and never becomes authority.
//! - `packages/server/sim/realm/state.go` (`BeginLoading`, `MarkGenerating`,
//!   `ApplyLoaded`, `ApplyGenerated`, `MarkLoadFailed`): the record state
//!   machine silently skips every completion whose record left the expected
//!   state. The frozen `ChunkResult` request/generation identity sharpens
//!   that skip into the counted stale-generation refusal this rule owns.
//!
//! The pending-result ceiling and the admission counters are the accepted
//! mailbox's (`core/mailbox.rs`): cancellation and duplicate discards are
//! counted by `AuthorityState::chunk_discard_counts` at admission, while the
//! provider-level refusals report through `PhaseReport.rejected` because the
//! state exposes no per-reason counter for them.

use mornlea_domain::{ChunkPos, Dimension, Season, Weather, WorldState, WorldStateParts};
use mornlea_server::contracts::{
    ChunkKey, ChunkRequestId, ChunkResult, Expected, FixtureState, Observed, PhaseReport, RuleCall,
    RulePhase, RuleReject, ServerError, TickBudget, TickCounters,
};
use mornlea_server::core::mailbox::{self, ChunkAdmission};
use mornlea_server::rules::world_acquisition::{self, AcquireOutcome, AcquireRefusal, ChunkWants};
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::Chunk;

use super::{assert_expected, canonical_state_sha256, limits};

/// Server-local chunk key in the overworld at one column.
fn key(x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, z),
    }
}

fn request_id(raw: u64) -> ChunkRequestId {
    ChunkRequestId::try_new(raw).expect("request id")
}

/// A ready completion carrying the smallest legal payload: an empty chunk.
fn completion(request: u64, generation: u64, item: ChunkKey) -> ChunkResult {
    ChunkResult {
        request: request_id(request),
        key: item,
        generation,
        result: Ok(Chunk {
            sections: Vec::new(),
            drops: Vec::new(),
            furnaces: Vec::new(),
            chests: Vec::new(),
        }),
    }
}

/// A completion whose producer failed; nothing may install from it.
fn failed_completion(request: u64, generation: u64, item: ChunkKey) -> ChunkResult {
    ChunkResult {
        request: request_id(request),
        key: item,
        generation,
        result: Err(ServerError::Cancelled),
    }
}

/// Canonical fixture state for the world record this suite hashes.
fn fixture_state() -> FixtureState {
    FixtureState {
        runtime: Vec::new(),
        actors: Vec::new(),
        chunks: Vec::new(),
        inventories: Vec::new(),
        containers: Vec::new(),
        work: mornlea_server::contracts::WorkState::default(),
        sleep: mornlea_server::contracts::SleepState {
            beds: Vec::new(),
            day_phase_offset: 0,
            pending_offset: None,
        },
        projectiles: Vec::new(),
        drops: Vec::new(),
        world: world(),
    }
}

/// Publication world record the harness hash folds.
fn world() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .expect("fixture world")
}

/// The expected outcome for a phase call that must not move anything.
fn expected_initial() -> Expected {
    Expected {
        events: Vec::new(),
        state_sha256: canonical_state_sha256(&fixture_state()),
        class: None,
        counters: TickCounters::default(),
    }
}

/// Builds the authority and context exactly like the harness `run_phase` and
/// invokes the acquisition provider once, classifying a failed call the same
/// way the harness does.
fn run_phase(call: RuleCall<'_>) -> (Result<PhaseReport, ServerError>, Observed) {
    let mut authority = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context =
        TickContext::from_fixture(&mut authority, &fixture_state(), TickBudget::full());
    let report = world_acquisition::run(&mut context, call);
    let failed = report.is_err();
    let state = context.snapshot_state(world());
    (
        report,
        Observed {
            events: context.events().to_vec(),
            counters: TickCounters {
                executed_tick: context.read().tick(),
                ..TickCounters::default()
            },
            state_sha256: canonical_state_sha256(&state),
            class: if failed {
                Some(RuleReject::StaleObservation)
            } else {
                None
            },
        },
    )
}

fn acquire_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::Acquire,
        actor: None,
        command: None,
        internal: None,
    }
}

/// An authority at the caller's ready-result ceiling.
fn authority(ready_chunk_results: usize) -> AuthorityState {
    AuthorityState::try_new(
        mornlea_server::contracts::ServerLimits::try_new(
            1,
            4096,
            512,
            ready_chunk_results,
            64,
            1_048_576,
        )
        .expect("limits"),
        7,
    )
    .expect("authority")
}

/// The post-run hash: a fresh fixture overlay hashes the initial world, and
/// the mailbox plus the pure acquisition gate never touch that overlay.
fn overlay_hash(authority: &mut AuthorityState) -> [u8; 32] {
    let context = TickContext::from_fixture(authority, &fixture_state(), TickBudget::full());
    canonical_state_sha256(&context.snapshot_state(world()))
}

/// Drains everything the mailbox queued and hands the batch to the
/// acquisition rule, the flow the serial tick reducer owns later.
fn apply_drained(wants: &mut ChunkWants, authority: &mut AuthorityState) -> PhaseReport {
    let drained = authority.drain_chunks(64);
    world_acquisition::apply_drained(wants, &drained)
}

/// The frozen row end to end: the generation-3 / request-1 want applies
/// exactly once, while a cancellation tombstone, an unsubscribe-before-result
/// and an old generation-2 completion each fail to install with the world
/// hash unchanged and one counted reason.
#[test]
fn wanted_cancel_duplicate() {
    // Shape gate first: the batch phase refuses a wrong-phase call with the
    // zero report, and the registered Acquire shape itself examines nothing
    // through the context because completions reach the rule only through
    // the drained mailbox batch.
    let (report, observed) = run_phase(RuleCall {
        phase: RulePhase::Publish,
        ..acquire_call()
    });
    assert_eq!(
        report,
        Ok(PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0
        })
    );
    assert_eq!(observed.class, None);
    assert_expected(&observed, &expected_initial());
    let (report, observed) = run_phase(acquire_call());
    assert_eq!(
        report,
        Ok(PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0
        })
    );
    assert_expected(&observed, &expected_initial());

    // Wanted acquisition applies exactly once before physics: the matching
    // generation-3 completion installs, and replaying the same drained batch
    // is a counted refusal instead of a second install.
    let mut authority = authority(64);
    let before = canonical_state_sha256(&fixture_state());
    let mut wants = ChunkWants::new();
    let item = key(2, -4);
    wants.want(item, 3, request_id(1));
    assert_eq!(
        mailbox::admit_chunk_result(&mut authority, completion(1, 3, item)),
        Ok(ChunkAdmission::Admitted)
    );
    let drained = authority.drain_chunks(64);
    assert_eq!(drained.len(), 1);
    assert_eq!(
        world_acquisition::apply(&mut wants, drained.first().expect("record")),
        AcquireOutcome::Installed,
        "the matching generation-3 completion installs exactly once"
    );
    let replayed = [completion(1, 3, item)];
    assert_eq!(
        world_acquisition::apply_drained(&mut wants, &replayed),
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 1
        },
        "the same completion never installs twice"
    );
    assert_eq!(
        world_acquisition::apply(&mut wants, replayed.first().expect("record")),
        AcquireOutcome::Refused(AcquireRefusal::AlreadyConsumed)
    );

    // Cancellation: the tombstone discards the late completion at admission,
    // counted on the mailbox's cancellation surface, and nothing is queued.
    wants.want(key(4, -4), 3, request_id(2));
    mailbox::cancel_chunk_request(&mut authority, request_id(2));
    assert_eq!(
        wants.forget(key(4, -4)),
        Some(request_id(2)),
        "unsubscribe returns the issued request for the tombstone"
    );
    assert_eq!(
        mailbox::admit_chunk_result(&mut authority, completion(2, 3, key(4, -4))),
        Ok(ChunkAdmission::CancelledDiscarded)
    );
    assert_eq!(authority.chunk_discard_counts(), (1, 0));
    assert!(
        authority.drain_chunks(64).is_empty(),
        "a cancelled completion is never queued for install"
    );

    // Unsubscribe-before-result without a tombstone: the mailbox re-admits
    // the completion under its fresh request identity, and the install gate
    // refuses it as not wanted, mirroring the Go forgotten-key paths that
    // publish nothing and retain no authority.
    wants.want(key(6, -4), 3, request_id(3));
    assert_eq!(wants.forget(key(6, -4)), Some(request_id(3)));
    assert_eq!(
        mailbox::admit_chunk_result(&mut authority, completion(3, 3, key(6, -4))),
        Ok(ChunkAdmission::Admitted)
    );
    assert_eq!(
        apply_drained(&mut wants, &mut authority),
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 1
        }
    );
    let orphan = [completion(3, 3, key(6, -4))];
    assert_eq!(
        world_acquisition::apply(&mut wants, orphan.first().expect("record")),
        AcquireOutcome::Refused(AcquireRefusal::NotWanted)
    );

    // Old generation: a fresh request at generation 2 cannot install over the
    // generation-3 want. The mailbox keys on request identity alone and
    // admits the record; the stale-generation gate is the only protection.
    assert_eq!(
        mailbox::admit_chunk_result(&mut authority, completion(4, 2, item)),
        Ok(ChunkAdmission::Admitted)
    );
    assert_eq!(
        apply_drained(&mut wants, &mut authority),
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 1
        }
    );
    let stale = [completion(4, 2, item)];
    assert_eq!(
        world_acquisition::apply(&mut wants, stale.first().expect("record")),
        AcquireOutcome::Refused(AcquireRefusal::StaleGeneration {
            wanted: 3,
            found: 2
        })
    );
    assert_eq!(
        overlay_hash(&mut authority),
        before,
        "every refusal path leaves the world hash unchanged"
    );
}

/// The accepted ready-result ceiling is the frozen pending-result limit: 64
/// completions are admitted, the 65th is refused whole with its owned record
/// returned untouched, and nothing about it installs.
#[test]
fn ready_result_cap_plus_one() {
    let mut authority = authority(64);
    let before = canonical_state_sha256(&fixture_state());
    for request in 1..=64u64 {
        assert_eq!(
            mailbox::admit_chunk_result(
                &mut authority,
                completion(request, 1, key(request as i32, 0))
            ),
            Ok(ChunkAdmission::Admitted),
            "the first 64 pending results are admitted"
        );
    }
    let plus_one = completion(65, 1, key(65, 0));
    let returned = match mailbox::admit_chunk_result(&mut authority, plus_one.clone()) {
        Err(owned) => owned,
        Ok(_) => panic!("the 65th pending result must be refused whole"),
    };
    assert_eq!(
        returned, plus_one,
        "refusal returns the producer's owned record unchanged"
    );
    let drained = authority.drain_chunks(64);
    assert_eq!(drained.len(), 64, "exactly the cap is queued");
    assert!(
        drained.iter().all(|record| record.request.get() != 65),
        "the refused completion never reaches the install queue"
    );
    assert_eq!(
        overlay_hash(&mut authority),
        before,
        "the cap refusal leaves the world hash unchanged with no partial install"
    );
}

/// The ledger-carried pin: a completion repeated after its predecessor was
/// drained is re-admitted by the mailbox, and the stale-generation rejection
/// at install closes it — one counted refusal, no second install, hash
/// unchanged. A failed payload consumes its request without installing while
/// the want survives for the reconcile retry.
#[test]
fn drained_repeat_not_reinstalled() {
    let mut authority = authority(64);
    let before = canonical_state_sha256(&fixture_state());
    let mut wants = ChunkWants::new();
    let item = key(0, 0);
    wants.want(item, 3, request_id(1));

    assert_eq!(
        mailbox::admit_chunk_result(&mut authority, completion(1, 3, item)),
        Ok(ChunkAdmission::Admitted)
    );
    let drained = authority.drain_chunks(64);
    assert_eq!(drained.len(), 1);
    assert_eq!(
        world_acquisition::apply(&mut wants, drained.first().expect("record")),
        AcquireOutcome::Installed
    );
    assert!(
        authority.drain_chunks(64).is_empty(),
        "the predecessor completion was drained"
    );

    // The documented mailbox gap: with the request gone from the queue and no
    // tombstone armed, the replayed completion is admitted again. The install
    // gate is what refuses it.
    assert_eq!(
        mailbox::admit_chunk_result(&mut authority, completion(1, 3, item)),
        Ok(ChunkAdmission::Admitted),
        "the mailbox re-admission is the gap this install gate pins"
    );
    let drained = authority.drain_chunks(64);
    assert_eq!(drained.len(), 1);
    assert_eq!(
        world_acquisition::apply_drained(&mut wants, &drained),
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 1
        },
        "the drained replay never installs a second time"
    );
    assert_eq!(
        world_acquisition::apply(&mut wants, drained.first().expect("record")),
        AcquireOutcome::Refused(AcquireRefusal::AlreadyConsumed)
    );
    assert_eq!(overlay_hash(&mut authority), before);

    // A failed payload installs nothing and consumes its request, while the
    // want survives so the reconcile path can re-issue load work; the repeat
    // of the failed request is the same consumed refusal.
    let retry_key = key(2, 0);
    wants.want(retry_key, 5, request_id(9));
    assert_eq!(
        mailbox::admit_chunk_result(&mut authority, failed_completion(9, 5, retry_key)),
        Ok(ChunkAdmission::Admitted)
    );
    let drained = authority.drain_chunks(64);
    assert_eq!(
        world_acquisition::apply(&mut wants, drained.first().expect("record")),
        AcquireOutcome::Refused(AcquireRefusal::Failed)
    );
    assert_eq!(
        wants.want_at(retry_key),
        Some((5, request_id(9))),
        "a failed completion keeps the want for the retry"
    );
    assert_eq!(
        mailbox::admit_chunk_result(&mut authority, failed_completion(9, 5, retry_key)),
        Ok(ChunkAdmission::Admitted)
    );
    let drained = authority.drain_chunks(64);
    assert_eq!(
        world_acquisition::apply(&mut wants, drained.first().expect("record")),
        AcquireOutcome::Refused(AcquireRefusal::AlreadyConsumed)
    );
    assert_eq!(
        overlay_hash(&mut authority),
        before,
        "failed and repeated completions leave the world hash unchanged"
    );
}
