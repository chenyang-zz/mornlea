use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock};

use godot::prelude::InitStage;

const SUPPORTED_GODOT_API: (i64, i64) = (4, 7);
static RUNTIME_LIFECYCLE: OnceLock<Mutex<Lifecycle>> = OnceLock::new();

/// Ordered extension stages owned by the Mornlea adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Stage {
    Scene,
    Editor,
    MainLoop,
}

impl Stage {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Scene => "scene",
            Self::Editor => "editor",
            Self::MainLoop => "main-loop",
        }
    }

    fn from_godot(stage: InitStage) -> Option<Self> {
        match stage {
            InitStage::Scene => Some(Self::Scene),
            InitStage::Editor => Some(Self::Editor),
            InitStage::MainLoop => Some(Self::MainLoop),
            _ => None,
        }
    }
}

/// Stable failures returned at the native boundary instead of unwinding into Godot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundaryFailure {
    InvalidTransition,
    Panic,
    Poisoned,
}

impl std::fmt::Display for BoundaryFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTransition => formatter.write_str("invalid lifecycle transition"),
            Self::Panic => formatter.write_str("Rust panic at Godot boundary"),
            Self::Poisoned => formatter.write_str("lifecycle state is poisoned"),
        }
    }
}

/// Small deterministic state machine shared by unit tests and extension callbacks.
#[derive(Debug, Default)]
pub(crate) struct Lifecycle {
    active: Vec<Stage>,
}

impl Lifecycle {
    pub(crate) fn initialize(&mut self, stage: Stage) -> Result<(), BoundaryFailure> {
        let valid = matches!(
            (self.active.as_slice(), stage),
            ([], Stage::Scene)
                | ([Stage::Scene], Stage::Editor | Stage::MainLoop)
                | ([Stage::Scene, Stage::Editor], Stage::MainLoop)
        );
        if !valid {
            return Err(BoundaryFailure::InvalidTransition);
        }
        self.active.push(stage);
        Ok(())
    }

    pub(crate) fn deinitialize(&mut self, stage: Stage) -> Result<(), BoundaryFailure> {
        if self.active.last().copied() != Some(stage) {
            return Err(BoundaryFailure::InvalidTransition);
        }
        self.active.pop();
        Ok(())
    }

    pub(crate) fn stage(&self) -> Option<Stage> {
        self.active.last().copied()
    }
}

pub(crate) fn supports_godot_api(major: i64, minor: i64) -> bool {
    (major, minor) == SUPPORTED_GODOT_API
}

pub(crate) fn catch_boundary<T>(operation: impl FnOnce() -> T) -> Result<T, BoundaryFailure> {
    catch_unwind(AssertUnwindSafe(operation)).map_err(|_| BoundaryFailure::Panic)
}

pub(crate) fn initialize_godot_stage(stage: InitStage) -> Result<Option<Stage>, BoundaryFailure> {
    let Some(stage) = Stage::from_godot(stage) else {
        return Ok(None);
    };
    catch_boundary(|| {
        let mut lifecycle = runtime_lifecycle()
            .lock()
            .map_err(|_| BoundaryFailure::Poisoned)?;
        lifecycle.initialize(stage)?;
        Ok(Some(stage))
    })?
}

pub(crate) fn deinitialize_godot_stage(stage: InitStage) -> Result<Option<Stage>, BoundaryFailure> {
    let Some(stage) = Stage::from_godot(stage) else {
        return Ok(None);
    };
    catch_boundary(|| {
        let mut lifecycle = runtime_lifecycle()
            .lock()
            .map_err(|_| BoundaryFailure::Poisoned)?;
        lifecycle.deinitialize(stage)?;
        Ok(Some(stage))
    })?
}

pub(crate) fn current_stage() -> &'static str {
    runtime_lifecycle()
        .lock()
        .ok()
        .and_then(|lifecycle| lifecycle.stage())
        .map(Stage::label)
        .unwrap_or("inactive")
}

fn runtime_lifecycle() -> &'static Mutex<Lifecycle> {
    RUNTIME_LIFECYCLE.get_or_init(|| Mutex::new(Lifecycle::default()))
}

/// The code-level release ownership for the bridge's rust cores.
///
/// This module owns the engine-free half of the bridge lifecycle that the
/// Godot host performs across its feature consumers: the release order
/// (every consumer first, then the one native release), the late-callback
/// gate after close, the reset invalidation order, and the panic
/// containment of the routed facade calls. It composes the actual adapter
/// routines of [`crate::client_core::rust_core::CoreArena`] — there is no
/// second, test-only implementation of any release path — and the
/// `lifecycle_contract` target pins these contracts through it without the
/// engine. The bridge node's wiring of this ownership is the integration
/// that follows; the ordering and containment rules land here first.
#[allow(dead_code)]
pub(crate) mod ownership {
    use crate::abi::boundary::{self, BoundaryValue, CoreOpenSpec, CoreTokenValue};
    use crate::client_core::rust_core::CoreArena;
    use mornlea_client_core::contracts::{ClientEndpoint, ClientError};

    /// One host-side consumer of a live rust core, in the host's own
    /// callback vocabulary. The defaults are deliberate no-ops: a consumer
    /// that does not use a callback cannot be broken by one.
    pub(crate) trait CoreConsumer {
        /// One whole rendered frame of the live epoch, in registration
        /// order (providers before consumers).
        fn apply(&mut self, _frame: &BoundaryValue) {}
        /// The queued-state invalidation of a successful reset, in
        /// registration order, before anything can be applied onto the
        /// fresh epoch.
        fn invalidate(&mut self) {}
        /// The release callback of a close, in the exact reverse of
        /// registration order, strictly before the native release.
        fn release(&mut self) {}
    }

    /// The registration identity of one consumer: a nonzero slot plus the
    /// core generation the registration belongs to. A released consumer's
    /// handle is retired with the registry and never reissued.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct ConsumerHandle {
        slot: u32,
        generation: u64,
    }

    struct ConsumerEntry {
        handle: ConsumerHandle,
        consumer: Box<dyn CoreConsumer>,
    }

    /// The bridge-side release-order authority over one rust-core arena.
    ///
    /// Ownership model: one live core token, one canonical session-epoch
    /// text, one retained frame copy, and the registered consumers of the
    /// live generation. The token is the single native release obligation —
    /// `close` releases it exactly once per issue and drops it even when
    /// the envelope refuses, so a released token can never be replayed as
    /// if it were live. Panic policy: every routed call goes through the
    /// adapter's guarded facade routines, so a native panic answers the
    /// closed `Internal` envelope instead of unwinding across the boundary
    /// while the prior visible state survives.
    pub(crate) struct CoreOwnership {
        arena: CoreArena,
        token: Option<CoreTokenValue>,
        epoch: String,
        retained_frame: Option<BoundaryValue>,
        consumers: Vec<ConsumerEntry>,
        next_slot: u32,
    }

    impl CoreOwnership {
        pub(crate) fn new(arena: CoreArena) -> Self {
            Self {
                arena,
                token: None,
                epoch: String::new(),
                retained_frame: None,
                consumers: Vec::new(),
                next_slot: 0,
            }
        }

        /// The engine-free substitution open over the same slot,
        /// generation and lifecycle code as the production path: a wrapper
        /// around a real C1 endpoint may be installed for tracing and
        /// panic injection, but the adapter routines themselves are never
        /// replaced.
        pub(crate) fn open_with_endpoint(
            &mut self,
            spec: &CoreOpenSpec,
            endpoint: Box<dyn ClientEndpoint>,
        ) -> Result<CoreTokenValue, ClientError> {
            let token = self.arena.open_with_endpoint(spec, endpoint)?;
            self.token = Some(token);
            Ok(token)
        }

        /// Begins one connection on the owned token and adopts the answered
        /// canonical epoch text; an ownership without a live token answers
        /// the typed refusal without any core call.
        pub(crate) fn connect(
            &mut self,
            endpoint: &BoundaryValue,
            identity: &BoundaryValue,
        ) -> BoundaryValue {
            let Some(token) = self.token else {
                return boundary::outcome(Err(ClientError::InvalidState));
            };
            let answer = self
                .arena
                .connect_routine(&token.to_boundary(), endpoint, identity);
            if let Some(text) = success_text(&answer) {
                self.epoch = text;
            }
            answer
        }

        /// `submit_typed_input` in the bridge facade shape: the caller owns
        /// the token argument, so a late or forged token earns the
        /// adapter's own typed rejection before any core dereference —
        /// zero native calls behind the refusal.
        pub(crate) fn submit_input(
            &mut self,
            token: &BoundaryValue,
            epoch: &BoundaryValue,
            batch: &BoundaryValue,
        ) -> BoundaryValue {
            self.arena.submit_routine(token, epoch, batch)
        }

        /// One host-owned tick: step, then pull, then apply the frame to
        /// every consumer in registration order. A failed step skips the
        /// pull and every apply; a failed pull keeps the prior retained
        /// copy. Answers whether a live core was dispatched at all: a
        /// closed or never-connected ownership dispatches nothing, so a
        /// late tick is an ignored stale callback rather than a call on a
        /// dead token.
        pub(crate) fn tick(&mut self, work: &BoundaryValue) -> bool {
            let Some(token) = self.token else {
                return false;
            };
            if self.epoch.is_empty() {
                return false;
            }
            let epoch = BoundaryValue::Text(self.epoch.clone());
            let stepped = self.arena.step_routine(&token.to_boundary(), &epoch, work);
            if !is_success(&stepped) {
                return true;
            }
            let pulled = self.arena.pull_frame_routine(&token.to_boundary(), &epoch);
            let Some(frame) = success_value(&pulled) else {
                return true;
            };
            self.retained_frame = Some(frame.clone());
            for entry in &mut self.consumers {
                entry.consumer.apply(&frame);
            }
            true
        }

        /// Reset mirrors the accepted host routine: the native reset runs
        /// first, and a refusal keeps the live epoch and every consumer's
        /// queued state rather than splitting consumers from their session
        /// epoch. On success the fresh epoch is adopted, the retained frame
        /// copy of the retired epoch is dropped — only a validated frame of
        /// the fresh epoch can reappear — and every consumer's queued state
        /// is invalidated in registration order, providers before
        /// consumers, before the next tick can apply anything.
        pub(crate) fn reset(&mut self) -> BoundaryValue {
            let Some(token) = self.token else {
                return boundary::outcome(Err(ClientError::InvalidState));
            };
            if self.epoch.is_empty() {
                return boundary::outcome(Err(ClientError::InvalidState));
            }
            let epoch = BoundaryValue::Text(self.epoch.clone());
            let answer = self.arena.reset_routine(&token.to_boundary(), &epoch);
            if let Some(fresh) = success_text(&answer) {
                self.epoch = fresh;
                self.retained_frame = None;
                for entry in &mut self.consumers {
                    entry.consumer.invalidate();
                }
            }
            answer
        }

        /// Releases every consumer first — in the exact reverse of
        /// registration order, so consumers always outlive their providers
        /// — and only then performs the single native release of the owned
        /// token. The owned state drops even when the envelope refuses: a
        /// token this ownership decided to release is never reused, and a
        /// repeated close dispatches nothing. A panic inside the native
        /// release is contained by the adapter's guard into the `Internal`
        /// envelope while the already-released consumers stay released.
        pub(crate) fn close(&mut self) -> BoundaryValue {
            for mut entry in self.consumers.drain(..).rev() {
                entry.consumer.release();
            }
            let answer = match self.token.take() {
                Some(token) => self.arena.close_routine(&token.to_boundary()),
                None => boundary::outcome(Ok(BoundaryValue::Null)),
            };
            self.epoch.clear();
            self.retained_frame = None;
            answer
        }

        /// Registers one consumer against the live generation; an
        /// ownership without a live token accepts none.
        pub(crate) fn register_consumer(
            &mut self,
            consumer: Box<dyn CoreConsumer>,
        ) -> Option<ConsumerHandle> {
            let token = self.token?;
            self.next_slot += 1;
            let handle = ConsumerHandle {
                slot: self.next_slot,
                generation: token.generation(),
            };
            self.consumers.push(ConsumerEntry { handle, consumer });
            Some(handle)
        }

        /// The number of retained consumer registrations; zero after close.
        pub(crate) fn retained_consumers(&self) -> usize {
            self.consumers.len()
        }

        /// Whether one consumer handle still addresses a live registration.
        pub(crate) fn is_registered(&self, handle: ConsumerHandle) -> bool {
            self.consumers.iter().any(|entry| entry.handle == handle)
        }

        /// The canonical session-epoch text, empty when none is owned.
        pub(crate) fn epoch(&self) -> &str {
            &self.epoch
        }

        /// The ownership's retained frame copy of the live epoch.
        pub(crate) fn retained_frame(&self) -> Option<&BoundaryValue> {
            self.retained_frame.as_ref()
        }

        /// The adapter's own slot-visible copy, for pinning that a
        /// contained panic preserved the prior visible state on both sides.
        pub(crate) fn visible_frame(&self) -> Option<&BoundaryValue> {
            let token = self.token?;
            self.arena.visible_frame(token).ok()
        }

        /// The arena-level release count: one per issued token regardless
        /// of how many times that token is closed.
        pub(crate) fn releases(&self) -> usize {
            self.arena.releases()
        }
    }

    fn is_success(answer: &BoundaryValue) -> bool {
        answer.field("ok").and_then(|flag| flag.as_bool().ok()) == Some(true)
    }

    fn success_value(answer: &BoundaryValue) -> Option<BoundaryValue> {
        if is_success(answer) {
            answer.field("value").cloned()
        } else {
            None
        }
    }

    fn success_text(answer: &BoundaryValue) -> Option<String> {
        if is_success(answer) {
            answer.field("value")?.as_text().ok().map(str::to_string)
        } else {
            None
        }
    }
}
