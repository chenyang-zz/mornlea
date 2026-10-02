use godot::classes::image::Format as ImageFormat;
use godot::classes::rendering_server::TextureLayeredType;
use godot::classes::{
    FileAccess, INode, Json, Node, Node3D, RenderingServer, ResourceLoader, ShaderMaterial,
};
use godot::prelude::*;

use crate::abi;
use crate::client_core::{
    self, ClientHandle, CoreCalls, ProducerLoadFailure, PullFamily, producer_identity,
    production_core_calls,
};
use crate::feature_negotiation::PILOT_FAMILIES;
use crate::frame_decode::{TypedFrame, apply_environment_projection, decode_frame_record};
use crate::lifecycle;
use crate::pull_buffers::{PullBufferSet, pull_via_buffer};
use crate::status_decode::{TypedStatus, decode_status_record};
use crate::terrain_bridge::{
    BridgeTerrain, TerrainFrameSummary, TerrainIngestSummary, TerrainInputs,
};
use crate::terrain_budget::TerrainBudgetStage;
use crate::terrain_resources::{TerrainMaterials, TerrainRenderer, production_render_backend};

const GODOT_API_MAJOR: i64 = 4;
const GODOT_API_MINOR: i64 = 7;
const GODOT_RUST_VERSION: &str = "0.5.5";

// Pinned atlas identity the Rust-side loader validates before building the
// layered texture: the same fields the world feature's Python bind pins
// (schema version, pixel format, layer size, mip count, storage order, and
// pixel file name). The manifest's byte-level currency is enforced by the
// asset gate; this validation fails closed on identity drift only.
const ATLAS_MANIFEST_SCHEMA_VERSION: i64 = 1;
const ATLAS_FORMAT: &str = "RGBA8";
const ATLAS_LAYER_SIZE: i64 = 16;
const ATLAS_MIP_LEVELS: i64 = 5;
const ATLAS_STORAGE_ORDER: &str = "layer-major,mip-major";
const ATLAS_MANIFEST_PATH: &str = "res://assets/generated/manifest.json";
const ATLAS_PIXELS_PATH: &str = "res://assets/generated/atlas.rgba8";
const ATLAS_LAYER_NAME: &str = "atlas.rgba8";
/// Bytes of one atlas layer: the full 16x16 RGBA8 mip chain (256+64+16+4+1
/// texels, four bytes each).
const ATLAS_LAYER_BYTES: usize = 1364;

#[cfg(test)]
pub(crate) const BRIDGE_CLASS_NAME: &str = "MornleaClientBridge";
#[cfg(test)]
pub(crate) const IDENTITY_METHODS: [&str; 7] = [
    "client_core_abi_major",
    "client_core_abi_minor",
    "godot_api_major",
    "godot_api_minor",
    "godot_rust_version",
    "lifecycle_stage",
    "supports_godot_api",
];

/// The eight rust-producer facade methods in the frozen facade-contract
/// order. Every one marshals its Godot arguments into owned boundary copies
/// and calls the shared engine-independent routines in
/// `client_core::rust_core`; the old `session_*` surface above stays the
/// explicitly pilot-only Go path.
#[cfg(test)]
pub(crate) const RUST_FACADE_METHODS: [&str; 8] = [
    "open_core",
    "connect",
    "submit_typed_input",
    "step",
    "pull_typed_frame",
    "family_table",
    "reset",
    "close",
];

// The rust-producer marshalling half: conversions between Godot-owned
// variants and the engine-neutral boundary copies the shared adapter
// routines consume. This code compiles with the extension and only runs
// inside the engine; the engine-free adapter tests exercise the identical
// routines through `client_core::rust_core` directly, so no dictionary
// round-trip is claimed as adapter acceptance here.

/// Converts one Godot variant into the owned boundary copy. Typed leaves
/// keep their type: booleans never become integers, floats must be finite
/// to cross, and dictionaries and arrays become owned field lists and
/// vectors.
fn boundary_from_variant(variant: &Variant) -> Result<crate::abi::boundary::BoundaryValue, ()> {
    use crate::abi::boundary::BoundaryValue;
    use godot::builtin::VariantType;
    match variant.get_type() {
        VariantType::NIL => Ok(BoundaryValue::Null),
        VariantType::BOOL => variant
            .try_to::<bool>()
            .map(BoundaryValue::Bool)
            .map_err(|_| ()),
        VariantType::INT => variant
            .try_to::<i64>()
            .map(BoundaryValue::Int)
            .map_err(|_| ()),
        VariantType::FLOAT => {
            let value = variant.try_to::<f64>().map_err(|_| ())?;
            if value.is_finite() {
                Ok(BoundaryValue::Float(value))
            } else {
                Err(())
            }
        }
        VariantType::STRING => variant
            .try_to::<GString>()
            .map(|text| BoundaryValue::Text(text.to_string()))
            .map_err(|_| ()),
        VariantType::ARRAY => {
            let array = variant.try_to::<VarArray>().map_err(|_| ())?;
            let mut items = Vec::with_capacity(array.len());
            for item in array.iter_shared() {
                items.push(boundary_from_variant(&item)?);
            }
            Ok(BoundaryValue::List(items))
        }
        VariantType::DICTIONARY => {
            let dictionary = variant.try_to::<VarDictionary>().map_err(|_| ())?;
            let mut fields = Vec::with_capacity(dictionary.len());
            for (key, value) in dictionary.iter_shared() {
                let key = key.try_to::<GString>().map_err(|_| ())?;
                fields.push((key.to_string(), boundary_from_variant(&value)?));
            }
            Ok(BoundaryValue::Fields(fields))
        }
        _ => Err(()),
    }
}

/// Converts one owned boundary copy back into a Godot-owned variant.
fn variant_from_boundary(value: &crate::abi::boundary::BoundaryValue) -> Variant {
    use crate::abi::boundary::BoundaryValue;
    match value {
        BoundaryValue::Null => Variant::nil(),
        BoundaryValue::Bool(value) => Variant::from(*value),
        BoundaryValue::Int(value) => Variant::from(*value),
        BoundaryValue::Float(value) => Variant::from(*value),
        BoundaryValue::Text(value) => Variant::from(GString::from(value.as_str())),
        BoundaryValue::List(items) => {
            let mut array = VarArray::new();
            for item in items {
                array.push(&variant_from_boundary(item));
            }
            array.to_variant()
        }
        BoundaryValue::Fields(fields) => {
            let mut dictionary = VarDictionary::new();
            for (name, item) in fields {
                dictionary.set(name.as_str(), &variant_from_boundary(item));
            }
            dictionary.to_variant()
        }
    }
}

/// The Godot-independent lifecycle state of one bridge-held client session.
///
/// The bridge holds at most one producer handle plus one set of reusable
/// FFI-side pull buffers (see `crate::pull_buffers`): every pull still hands
/// Python an owned `Vec<u8>` copy, so nothing here needs destruction beyond
/// the idempotent handle release. Every method returns the producer status
/// vocabulary (or the local invalid-state word for calls made without a
/// session), mapping every status word without panicking.
pub(crate) struct BridgeSession {
    calls: Box<dyn CoreCalls>,
    handle: Option<ClientHandle>,
    buffers: PullBufferSet,
    /// The bridge-owned terrain pipeline, attached once by the world
    /// feature's activation. Survives producer-session closes (its `reset`
    /// rides every close) so re-entry reuses the same stage under a
    /// strictly greater stage epoch. The atlas texture RID stays with the
    /// node layer because freeing it is an engine call this Godot-free
    /// struct must never make.
    terrain: Option<BridgeTerrain>,
}

impl BridgeSession {
    pub(crate) fn new(calls: Box<dyn CoreCalls>) -> Self {
        Self {
            calls,
            handle: None,
            buffers: PullBufferSet::default(),
            terrain: None,
        }
    }

    /// Create one producer session requesting the pinned pilot families at
    /// their pinned contract versions. A bridge holds at most one session, so
    /// a second create without a close reports invalid state locally. The
    /// buffers' served identities reset on success because wire epochs are
    /// per producer session and a fresh session's epoch space restarts.
    pub(crate) fn create(&mut self) -> u32 {
        if self.handle.is_some() {
            return abi::STATUS_INVALID_STATE;
        }
        let mut requested = Vec::with_capacity(PILOT_FAMILIES.len());
        for descriptor in &PILOT_FAMILIES {
            requested.push((u64::from(descriptor.version) << 32) | u64::from(descriptor.family));
        }
        match self
            .calls
            .create_session(abi::ABI_MAJOR, abi::ABI_MINOR, &requested)
        {
            Ok(handle) => {
                self.handle = Some(handle);
                self.buffers.reset_served_identities();
                abi::STATUS_OK
            }
            Err(word) => word,
        }
    }

    /// Begin one asynchronous connection; the producer owns every address
    /// domain check and never blocks the calling thread.
    pub(crate) fn connect(&mut self, address: &str) -> u32 {
        let Some(handle) = self.handle else {
            return abi::STATUS_INVALID_STATE;
        };
        self.calls.connect_begin(handle, address.as_bytes())
    }

    /// Poll the connection phase: the status word plus the phase word, which
    /// is meaningful only on success.
    pub(crate) fn poll(&mut self) -> (u32, u32) {
        let Some(handle) = self.handle else {
            return (abi::STATUS_INVALID_STATE, 0);
        };
        match self.calls.connect_poll(handle) {
            Ok(phase) => (abi::STATUS_OK, phase),
            Err(word) => (word, 0),
        }
    }

    /// Submit one complete input-family batch; the batch bytes are passed
    /// through to the producer unchanged.
    pub(crate) fn submit(&mut self, batch: &[u8]) -> u32 {
        let Some(handle) = self.handle else {
            return abi::STATUS_INVALID_STATE;
        };
        self.calls.submit_input(handle, batch)
    }

    /// Encode one complete semantic desktop intent into the producer-owned
    /// MCN1 record. Python supplies only typed device state; wire identity,
    /// event ordering, and numeric validation stay on this Rust boundary.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn submit_semantic(
        &mut self,
        move_x: i64,
        move_z: i64,
        jump: bool,
        mining: bool,
        eating: bool,
        sprinting: bool,
        sneaking: bool,
        yaw: f64,
        pitch: f64,
    ) -> u32 {
        if !(-1..=1).contains(&move_x) || !(-1..=1).contains(&move_z) {
            return abi::STATUS_INPUT_REJECTED;
        }
        let yaw = yaw as f32;
        let pitch = pitch as f32;
        if !yaw.is_finite()
            || !pitch.is_finite()
            || pitch.abs() > core::f32::consts::FRAC_PI_2 - 0.01
        {
            return abi::STATUS_INPUT_REJECTED;
        }
        let mut batch = vec![0u8; abi::INPUT_HEADER_BYTES + 7 * 16];
        batch[0..4].copy_from_slice(&abi::MAGIC_INPUT.to_le_bytes());
        batch[4..8].copy_from_slice(&abi::INPUT_VERSION.to_le_bytes());
        batch[8..12].copy_from_slice(&7u32.to_le_bytes());
        let mut event = |index: usize, action: u32, value: u32, aux: u32| {
            let start = abi::INPUT_HEADER_BYTES + index * 16;
            batch[start..start + 4].copy_from_slice(&action.to_le_bytes());
            batch[start + 4..start + 8].copy_from_slice(&value.to_le_bytes());
            batch[start + 8..start + 12].copy_from_slice(&aux.to_le_bytes());
        };
        event(0, 1, move_x as i32 as u32, move_z as i32 as u32);
        event(1, 2, yaw.to_bits(), pitch.to_bits());
        for (index, action, pressed) in [
            (2, 3, jump),
            (3, 4, mining),
            (4, 5, eating),
            (5, 6, sprinting),
            (6, 7, sneaking),
        ] {
            event(index, action, u32::from(pressed), 0);
        }
        self.submit(&batch)
    }

    /// Drive exactly one bounded step; the frozen request record is encoded
    /// by the client-core seam.
    pub(crate) fn step(&mut self, elapsed_ns: u64, message_budget: u32, mesh_budget: u32) -> u32 {
        let Some(handle) = self.handle else {
            return abi::STATUS_INVALID_STATE;
        };
        let request = client_core::encode_step_request(elapsed_ns, message_budget, mesh_budget);
        self.calls.step(handle, &request)
    }

    /// Pull one family record through the two-phase protocol into the
    /// family's reusable FFI-side buffer, returning an owned byte vector.
    pub(crate) fn pull(&mut self, family: PullFamily) -> Result<Vec<u8>, u32> {
        let Some(handle) = self.handle else {
            return Err(abi::STATUS_INVALID_STATE);
        };
        let calls = self.calls.as_ref();
        let buffer = self.buffers.for_family(family);
        pull_via_buffer(calls, family, handle, buffer)
    }

    /// Pull the status record set and decode it into its typed semantic
    /// fields. This is the Python-facing status surface: record decoding is
    /// Rust-owned (design decision 4 of the pilot migration), so consumers
    /// receive typed values and never record bytes. Every failure — a
    /// missing session, a producer error word, or a record set the decoder
    /// rejects — surfaces as the failing status word.
    pub(crate) fn status_typed(&mut self) -> Result<TypedStatus, u32> {
        self.pull(PullFamily::Status)
            .and_then(|record| decode_status_record(&record))
    }

    /// Pull and decode the retained frame into semantic camera and target
    /// values. Record parsing stays Rust-owned so Python never depends on the
    /// client-core wire layout or invents a value after a producer failure.
    pub(crate) fn frame_typed(&mut self) -> Result<TypedFrame, u32> {
        let mut frame = decode_frame_record(&self.pull(PullFamily::Frame)?)?;
        let environment = self.pull(PullFamily::Environment)?;
        apply_environment_projection(&mut frame, &environment)?;
        Ok(frame)
    }

    /// Release the session idempotently: the first close destroys the handle
    /// (which also cancels and joins an in-flight connection) and every later
    /// close is a local no-op that still reports success. The buffers' served
    /// identities reset so a later session's fresh epoch space cannot be
    /// mistaken for a stale one. The terrain stage resets on every close:
    /// world re-entry and teardown both ride this one path, and the stage's
    /// strictly monotonic epochs keep the retired session's in-flight work
    /// from ever resurfacing.
    pub(crate) fn close(&mut self) -> u32 {
        if let Some(terrain) = self.terrain.as_mut() {
            terrain.reset();
        }
        let Some(handle) = self.handle.take() else {
            return abi::STATUS_OK;
        };
        self.buffers.reset_served_identities();
        self.calls.destroy_session(handle)
    }

    /// Store an assembled terrain pipeline; the attach path (bridge node)
    /// owns the Godot-resource half and hands the finished stage over.
    pub(crate) fn attach_terrain(&mut self, terrain: BridgeTerrain) {
        self.terrain = Some(terrain);
    }

    /// The attached inputs, for idempotent re-attachment checks.
    pub(crate) fn terrain_inputs(&self) -> Option<&TerrainInputs> {
        self.terrain.as_ref().map(|terrain| &terrain.inputs)
    }

    /// Pull the pending world batch and feed it through the terrain
    /// pipeline. Fails with the local invalid-state word before any pull
    /// when no pipeline is attached, so an unattached session never
    /// consumes a batch it cannot render.
    pub(crate) fn terrain_ingest(&mut self) -> Result<TerrainIngestSummary, u32> {
        self.terrain.as_mut().ok_or(abi::STATUS_INVALID_STATE)?;
        let record = self.pull(PullFamily::World)?;
        self.terrain
            .as_mut()
            .expect("terrain checked above the pull")
            .ingest(&record)
    }

    /// Pull the frame snapshot and drive exactly one terrain frame from the
    /// derived camera. Like the ingest path, an unattached pipeline fails
    /// before any pull.
    pub(crate) fn terrain_frame(&mut self) -> Result<TerrainFrameSummary, u32> {
        self.terrain.as_mut().ok_or(abi::STATUS_INVALID_STATE)?;
        let record = self.pull(PullFamily::Frame)?;
        self.terrain
            .as_mut()
            .expect("terrain checked above the pull")
            .frame(&record)
    }

    /// The structural terrain summary JSON, or the local invalid-state
    /// marker when no pipeline is attached.
    pub(crate) fn terrain_summary(&self) -> Result<String, u32> {
        let terrain = self.terrain.as_ref().ok_or(abi::STATUS_INVALID_STATE)?;
        Ok(terrain.summary_json())
    }
}

impl Drop for BridgeSession {
    fn drop(&mut self) {
        // Godot frees the node during scene teardown; the handle release lands
        // here, before the boxed core-call table drops, so the producer
        // session is always destroyed before any library state goes away.
        self.close();
    }
}

/// One typed manifest-field reader: the engine's own JSON parse hands back
/// a Variant, and every field is read through the typed conversion so a
/// drifted manifest fails closed instead of decoding as zero.
fn variant_field<T: godot::meta::FromGodot>(dictionary: &VarDictionary, name: &str) -> Option<T> {
    dictionary
        .get(name)
        .and_then(|value| value.try_to::<T>().ok())
}

/// The project-owned native bridge: identity statics for qualification plus
/// one scene-held session surface whose every value is a Godot-owned typed
/// value (integers, strings, dictionaries, packed byte arrays). Python never
/// receives a pointer, a raw buffer it must free, or a producer status it
/// cannot classify.
///
/// Main-thread discipline: Godot dispatches node calls on the main thread and
/// the bridge guards every instance method with `godot::init::is_main_thread`
/// anyway, answering off-thread calls with the internal status word instead
/// of panicking, so the single session is never raced.
#[derive(GodotClass)]
#[class(base=Node)]
struct MornleaClientBridge {
    session: BridgeSession,
    /// The rust-producer core arena. It is disjoint from the pilot
    /// `session` above: rust mode never loads the Go producer and the pilot
    /// path never touches a core token.
    rust_cores: crate::client_core::rust_core::CoreArena,
    /// The layered atlas texture the terrain attach built. Bridge-owned and
    /// freed when the node leaves the tree, after the session close reset
    /// every table-owned RID; the borrowed material and scenario RIDs are
    /// never freed (the resource cache and the scene own them).
    terrain_atlas: Option<Rid>,
    #[base]
    base: Base<Node>,
}

#[godot_api]
impl INode for MornleaClientBridge {
    fn init(base: Base<Node>) -> Self {
        Self {
            session: BridgeSession::new(production_core_calls()),
            rust_cores: crate::client_core::rust_core::CoreArena::new(),
            terrain_atlas: None,
            base,
        }
    }

    fn exit_tree(&mut self) {
        // Node teardown is the one owner-side moment the atlas may be
        // freed: the session close inside has already reset the stage and
        // freed every table-owned mesh and instance RID, and the rendering
        // server still exists on this (main) thread.
        self.session.close();
        if let Some(atlas) = self.terrain_atlas.take() {
            RenderingServer::singleton().free_rid(atlas);
        }
    }
}

#[godot_api]
impl MornleaClientBridge {
    // Identity calls are deliberately allocation-free except for the Godot string
    // conversion and do not create a second path to engine or gameplay state.
    // The client-core identity comes from the header-pinned abi module, so a
    // header bump moves the Godot-visible identity with it.
    #[func]
    fn client_core_abi_major() -> i64 {
        i64::from(abi::ABI_MAJOR)
    }

    #[func]
    fn client_core_abi_minor() -> i64 {
        i64::from(abi::ABI_MINOR)
    }

    #[func]
    fn godot_api_major() -> i64 {
        GODOT_API_MAJOR
    }

    #[func]
    fn godot_api_minor() -> i64 {
        GODOT_API_MINOR
    }

    #[func]
    fn godot_rust_version() -> GString {
        GODOT_RUST_VERSION.into()
    }

    #[func]
    fn lifecycle_stage() -> GString {
        lifecycle::current_stage().into()
    }

    #[func]
    fn supports_godot_api(major: i64, minor: i64) -> bool {
        lifecycle::supports_godot_api(major, minor)
    }

    // -----------------------------------------------------------------
    // The rust-producer facade. Each method marshals its Godot arguments
    // into an owned boundary copy and calls the identical engine-free
    // routine the adapter tests drive; the result dictionary is the closed
    // ok/value/error envelope. These paths create the safe Rust core and
    // never the Go loader or its CoreCalls table.
    // -----------------------------------------------------------------

    /// Opens the safe Rust client core under a checked configuration and
    /// answers its private core token.
    #[func]
    fn open_core(&mut self, config: VarDictionary) -> VarDictionary {
        self.rust_call(
            boundary_from_variant(&config.to_variant()),
            |arena, config| {
                arena.open_core_routine(
                    config,
                    std::sync::Arc::new(mornlea_client_core::contracts::StdMonotonicClock),
                    std::sync::Arc::new(mornlea_client_core::contracts::ConnectorRegistry::new()),
                )
            },
        )
    }

    /// Begins one pending connection against a checked endpoint and
    /// identity; the answered epoch is admission-pending only.
    #[func]
    fn connect(
        &mut self,
        token: VarDictionary,
        endpoint: VarDictionary,
        identity: VarDictionary,
    ) -> VarDictionary {
        self.rust_call3(
            boundary_from_variant(&token.to_variant()),
            boundary_from_variant(&endpoint.to_variant()),
            boundary_from_variant(&identity.to_variant()),
            |arena, token, endpoint, identity| arena.connect_routine(token, endpoint, identity),
        )
    }

    /// Submits one whole checked input batch of up to 128 semantic actions.
    #[func]
    fn submit_typed_input(
        &mut self,
        token: VarDictionary,
        epoch: GString,
        batch: VarDictionary,
    ) -> VarDictionary {
        let epoch = boundary_from_variant(&Variant::from(epoch));
        self.rust_call3(
            boundary_from_variant(&token.to_variant()),
            epoch,
            boundary_from_variant(&batch.to_variant()),
            |arena, token, epoch, batch| arena.submit_routine(token, epoch, batch),
        )
    }

    /// Drives exactly one bounded step and answers the checked report copy.
    #[func]
    fn step(&mut self, token: VarDictionary, epoch: GString, work: VarDictionary) -> VarDictionary {
        let epoch = boundary_from_variant(&Variant::from(epoch));
        self.rust_call3(
            boundary_from_variant(&token.to_variant()),
            epoch,
            boundary_from_variant(&work.to_variant()),
            |arena, token, epoch, work| arena.step_routine(token, epoch, work),
        )
    }

    /// Pulls, validates and answers the whole visible frame as one owned
    /// copy.
    #[func]
    fn pull_typed_frame(&mut self, token: VarDictionary, epoch: GString) -> VarDictionary {
        let epoch = boundary_from_variant(&Variant::from(epoch));
        self.rust_call2(
            boundary_from_variant(&token.to_variant()),
            epoch,
            |arena, token, epoch| arena.pull_frame_routine(token, epoch),
        )
    }

    /// Answers the frozen rust-client-core descriptor table.
    #[func]
    fn family_table(&mut self, token: VarDictionary) -> VarDictionary {
        self.rust_call(
            boundary_from_variant(&token.to_variant()),
            |arena, token| arena.family_table_routine(token),
        )
    }

    /// Resets the live session onto a fresh epoch after invalidation.
    #[func]
    fn reset(&mut self, token: VarDictionary, epoch: GString) -> VarDictionary {
        let epoch = boundary_from_variant(&Variant::from(epoch));
        self.rust_call2(
            boundary_from_variant(&token.to_variant()),
            epoch,
            |arena, token, epoch| arena.reset_routine(token, epoch),
        )
    }

    /// Releases the core token; a repeated close of the same issued token
    /// succeeds without another release.
    #[func]
    fn close(&mut self, token: VarDictionary) -> VarDictionary {
        self.rust_call(
            boundary_from_variant(&token.to_variant()),
            |arena, token| arena.close_routine(token),
        )
    }

    /// Renders one closed facade envelope as a Godot dictionary. The
    /// envelope value is always a field list, so the conversion is total.
    fn rust_envelope(&mut self, value: crate::abi::boundary::BoundaryValue) -> VarDictionary {
        match variant_from_boundary(&value).try_to::<VarDictionary>() {
            Ok(dictionary) => dictionary,
            Err(_) => VarDictionary::new(),
        }
    }

    /// One rust-facade call over one marshalled argument. An off-thread call
    /// or a marshalling refusal answers the closed envelope's `Internal` or
    /// `InvalidInput` failure instead of panicking across the engine
    /// boundary, and the shared routine runs only on the main thread.
    fn rust_call(
        &mut self,
        argument: Result<crate::abi::boundary::BoundaryValue, ()>,
        call: impl FnOnce(
            &mut crate::client_core::rust_core::CoreArena,
            &crate::abi::boundary::BoundaryValue,
        ) -> crate::abi::boundary::BoundaryValue,
    ) -> VarDictionary {
        use crate::abi::boundary;
        use mornlea_client_core::contracts::ClientError;
        let value = match argument {
            Ok(argument) if Self::on_main_thread() => call(&mut self.rust_cores, &argument),
            Ok(_) => boundary::outcome(Err(ClientError::Internal)),
            Err(()) => boundary::outcome(Err(ClientError::InvalidInput)),
        };
        self.rust_envelope(value)
    }

    /// The closed refusal envelope for a marshalling failure: no routine
    /// runs and no panic can cross the engine boundary, whatever argument
    /// position refused.
    fn rust_marshalling_refusal(&mut self) -> VarDictionary {
        use crate::abi::boundary;
        use mornlea_client_core::contracts::ClientError;
        let value = boundary::outcome(Err(ClientError::InvalidInput));
        self.rust_envelope(value)
    }

    /// One rust-facade call over two marshalled arguments. The admission is
    /// the shared engine-free join: a refusal in any position — including a
    /// late one behind an already-marshalled first argument — answers the
    /// closed envelope without dispatching.
    fn rust_call2(
        &mut self,
        first: Result<crate::abi::boundary::BoundaryValue, ()>,
        second: Result<crate::abi::boundary::BoundaryValue, ()>,
        call: impl FnOnce(
            &mut crate::client_core::rust_core::CoreArena,
            &crate::abi::boundary::BoundaryValue,
            &crate::abi::boundary::BoundaryValue,
        ) -> crate::abi::boundary::BoundaryValue,
    ) -> VarDictionary {
        match crate::abi::boundary::join_arguments2(first, second) {
            Ok((first, second)) => {
                self.rust_call(Ok(first), move |arena, first| call(arena, first, &second))
            }
            Err(()) => self.rust_marshalling_refusal(),
        }
    }

    /// One rust-facade call over three marshalled arguments, with the same
    /// any-position admission rule as [`Self::rust_call2`].
    fn rust_call3(
        &mut self,
        first: Result<crate::abi::boundary::BoundaryValue, ()>,
        second: Result<crate::abi::boundary::BoundaryValue, ()>,
        third: Result<crate::abi::boundary::BoundaryValue, ()>,
        call: impl FnOnce(
            &mut crate::client_core::rust_core::CoreArena,
            &crate::abi::boundary::BoundaryValue,
            &crate::abi::boundary::BoundaryValue,
            &crate::abi::boundary::BoundaryValue,
        ) -> crate::abi::boundary::BoundaryValue,
    ) -> VarDictionary {
        match crate::abi::boundary::join_arguments3(first, second, third) {
            Ok((first, second, third)) => self.rust_call(Ok(first), move |arena, first| {
                call(arena, first, &second, &third)
            }),
            Err(()) => self.rust_marshalling_refusal(),
        }
    }

    /// The pinned pilot feature-family table as JSON, one object per family
    /// with `family`, `version`, `record_limit`, and `record_bytes`, in
    /// ascending family order. This is the Rust mirror the bridge negotiates
    /// with; the live producer table is cross-checked through
    /// `pull_identity`.
    #[func]
    fn feature_families_json() -> GString {
        let mut json = String::from("[");
        for (index, descriptor) in PILOT_FAMILIES.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                "{{\"family\":{},\"record_bytes\":{},\"record_limit\":{},\"version\":{}}}",
                descriptor.family,
                descriptor.record_bytes,
                descriptor.record_limit,
                descriptor.version
            ));
        }
        json.push(']');
        json.as_str().into()
    }

    /// The live producer identity as JSON: `available`, and on success the
    /// producer-reported `major` and `minor` (or `reason` when the library
    /// beside the extension could not be loaded). This is the only method that
    /// observes the real producer before any session exists.
    #[func]
    fn producer_identity_json() -> GString {
        let identity = producer_identity();
        match identity.packed_version {
            Some(packed) => {
                let json = format!(
                    "{{\"available\":true,\"major\":{},\"minor\":{}}}",
                    packed >> 32,
                    packed & 0xFFFF_FFFF
                );
                json.as_str().into()
            }
            None => {
                let reason = identity
                    .failure
                    .map(ProducerLoadFailure::reason)
                    .unwrap_or("producer identity is unavailable");
                format!("{{\"available\":false,\"reason\":\"{reason}\"}}")
                    .as_str()
                    .into()
            }
        }
    }

    /// Create one session over the pinned pilot families; returns the
    /// producer status word (0 means the bridge now holds a live session).
    #[func]
    fn session_create(&mut self) -> i64 {
        if !Self::on_main_thread() {
            return i64::from(abi::STATUS_INTERNAL);
        }
        i64::from(self.session.create())
    }

    /// Begin one asynchronous connection to a "host:port" address; returns
    /// the producer status word. The call never blocks the main thread.
    #[func]
    fn session_connect(&mut self, address: GString) -> i64 {
        if !Self::on_main_thread() {
            return i64::from(abi::STATUS_INTERNAL);
        }
        i64::from(self.session.connect(&address.to_string()))
    }

    /// Poll the connection once: a dictionary with `status` and, when the
    /// status is 0, the producer `phase` word (otherwise 0).
    #[func]
    fn session_poll(&mut self) -> VarDictionary {
        let polled = if Self::on_main_thread() {
            self.session.poll()
        } else {
            (abi::STATUS_INTERNAL, 0)
        };
        let mut result = VarDictionary::new();
        result.set("status", i64::from(polled.0));
        result.set("phase", i64::from(polled.1));
        result
    }

    /// Submit one complete input-family batch (the packed wire record);
    /// returns the producer status word.
    #[func]
    fn session_submit(&mut self, batch: PackedByteArray) -> i64 {
        if !Self::on_main_thread() {
            return i64::from(abi::STATUS_INTERNAL);
        }
        i64::from(self.session.submit(&batch.to_vec()))
    }

    /// Submit typed desktop intent without exposing MCN1 to Python.
    #[allow(clippy::too_many_arguments)]
    #[func]
    fn session_submit_semantic(
        &mut self,
        move_x: i64,
        move_z: i64,
        jump: bool,
        mining: bool,
        eating: bool,
        sprinting: bool,
        sneaking: bool,
        yaw: f64,
        pitch: f64,
    ) -> i64 {
        if !Self::on_main_thread() {
            return i64::from(abi::STATUS_INTERNAL);
        }
        i64::from(self.session.submit_semantic(
            move_x, move_z, jump, mining, eating, sprinting, sneaking, yaw, pitch,
        ))
    }

    /// Drive exactly one bounded step. Negative or oversized arguments wrap
    /// into the producer's rejection domains instead of panicking on the
    /// integer conversion.
    #[func]
    fn session_step(&mut self, elapsed_ns: i64, message_budget: i64, mesh_budget: i64) -> i64 {
        if !Self::on_main_thread() {
            return i64::from(abi::STATUS_INTERNAL);
        }
        let elapsed = elapsed_ns as u64;
        let messages = u32::try_from(message_budget).unwrap_or(u32::MAX);
        let meshes = u32::try_from(mesh_budget).unwrap_or(u32::MAX);
        i64::from(self.session.step(elapsed, messages, meshes))
    }

    /// Drain the retained world batch: a dictionary with `status` and
    /// `record` (a Godot-owned packed byte array; empty when no batch is
    /// pending).
    #[func]
    fn pull_world(&mut self) -> VarDictionary {
        self.pull_dictionary(PullFamily::World)
    }

    /// Read the per-step frame snapshot: the same dictionary shape; the
    /// record is empty until a step published a frame.
    #[func]
    fn pull_frame(&mut self) -> VarDictionary {
        self.pull_dictionary(PullFamily::Frame)
    }

    /// Read the status and metrics record set: the same dictionary shape.
    #[func]
    fn pull_status(&mut self) -> VarDictionary {
        self.pull_dictionary(PullFamily::Status)
    }

    /// The status family's typed semantic view as one dictionary with
    /// `status` plus `phase`, `terminal_cause`, `steps_completed`, and
    /// `messages_processed` (the record kinds 1..4 of the status family,
    /// decoded Rust-side per the pilot's typed-values-only boundary for
    /// Python). The semantic fields are meaningful only when `status` is
    /// zero; every failure reports the status word with the fields zeroed,
    /// so a consumer can never mistake a failure for fabricated state.
    #[func]
    fn session_status_typed(&mut self) -> VarDictionary {
        let decoded = if Self::on_main_thread() {
            self.session.status_typed()
        } else {
            Err(abi::STATUS_INTERNAL)
        };
        let mut result = VarDictionary::new();
        match decoded {
            Ok(typed) => {
                result.set("status", i64::from(abi::STATUS_OK));
                result.set("phase", i64::from(typed.phase));
                result.set("terminal_cause", i64::from(typed.terminal_cause));
                // The counters are bounded by the step budgets in practice;
                // the saturation guard keeps the u64-to-i64 narrowing honest
                // if that bound is ever exceeded.
                result.set(
                    "steps_completed",
                    i64::try_from(typed.steps_completed).unwrap_or(i64::MAX),
                );
                result.set(
                    "messages_processed",
                    i64::try_from(typed.messages_processed).unwrap_or(i64::MAX),
                );
            }
            Err(word) => {
                result.set("status", i64::from(word));
                result.set("phase", 0);
                result.set("terminal_cause", 0);
                result.set("steps_completed", 0);
                result.set("messages_processed", 0);
            }
        }
        result
    }

    /// Read the retained frame's typed camera, target, phase, and revision
    /// values. A failed pull or decode returns only its status word and zeroed
    /// fields, preserving the atomic presentation boundary.
    #[func]
    fn session_frame_typed(&mut self) -> VarDictionary {
        let decoded = if Self::on_main_thread() {
            self.session.frame_typed()
        } else {
            Err(abi::STATUS_INTERNAL)
        };
        let mut result = VarDictionary::new();
        match decoded {
            Ok(frame) => {
                Self::put_typed_frame(&mut result, &frame, abi::STATUS_OK);
            }
            Err(word) => {
                result.set("status", i64::from(word));
                Self::put_typed_frame_defaults(&mut result);
            }
        }
        result
    }

    /// Read the producer identity record: the same dictionary shape.
    #[func]
    fn pull_identity(&mut self) -> VarDictionary {
        self.pull_dictionary(PullFamily::Identity)
    }

    /// Release the session idempotently; repeated closes and the node's
    /// scene-teardown drop are all safe and report success.
    #[func]
    fn session_close(&mut self) -> i64 {
        if !Self::on_main_thread() {
            return i64::from(abi::STATUS_INTERNAL);
        }
        i64::from(self.session.close())
    }

    /// Attach the native terrain renderer. Loads the three pinned
    /// ShaderMaterials and validates they are shader materials, resolves
    /// the scenario RID from the scenario-source Node3D's 3D world, builds
    /// the layered atlas texture from the generated pixels after manifest
    /// validation and assigns it to each material's `atlas` uniform, then
    /// assembles the budget stage over the production `RenderingServer`
    /// backend. Returns a dictionary with `status` (0 means attached) and,
    /// on failure, a stable English `detail`. Re-attaching with the same
    /// paths is an idempotent success; different paths fail closed with
    /// the invalid-state word.
    #[func]
    fn terrain_attach(
        &mut self,
        opaque: GString,
        cutout: GString,
        water: GString,
        scenario_source: GString,
    ) -> VarDictionary {
        let fail = |status: u32, detail: &str| {
            let mut result = VarDictionary::new();
            result.set("status", i64::from(status));
            result.set("detail", &GString::from(detail));
            result
        };
        if !Self::on_main_thread() {
            return fail(
                abi::STATUS_INTERNAL,
                "the terrain attach ran off the main thread",
            );
        }
        let inputs = TerrainInputs {
            opaque: opaque.to_string(),
            cutout: cutout.to_string(),
            water: water.to_string(),
            scenario: scenario_source.to_string(),
        };
        if let Some(existing) = self.session.terrain_inputs() {
            if *existing == inputs {
                let mut result = VarDictionary::new();
                result.set("status", i64::from(abi::STATUS_OK));
                result.set("detail", &GString::from(""));
                return result;
            }
            return fail(
                abi::STATUS_INVALID_STATE,
                "the terrain renderer is already attached with different resources",
            );
        }
        match self.attach_terrain_resources(&inputs) {
            Ok((terrain, atlas)) => {
                self.terrain_atlas = Some(atlas);
                self.session.attach_terrain(terrain);
                let mut result = VarDictionary::new();
                result.set("status", i64::from(abi::STATUS_OK));
                result.set("detail", &GString::from(""));
                result
            }
            Err((status, detail)) => fail(status, &detail),
        }
    }

    /// Pull the pending world batch once and feed it through the terrain
    /// pipeline (bounded upserts and drops; see the terrain module for the
    /// retention and drop bounds). Returns a dictionary with `status` plus
    /// the typed ingest counters; without an attached pipeline the call
    /// fails closed with the invalid-state word and consumes nothing.
    #[func]
    fn terrain_ingest_world(&mut self) -> VarDictionary {
        let ingested = if Self::on_main_thread() {
            self.session.terrain_ingest()
        } else {
            Err(abi::STATUS_INTERNAL)
        };
        let mut result = VarDictionary::new();
        match ingested {
            Ok(summary) => {
                result.set("status", i64::from(abi::STATUS_OK));
                result.set("operations", summary.operations as i64);
                result.set("record_quad_bytes", summary.record_quad_bytes as i64);
                result.set("upserts_submitted", summary.upserts_submitted as i64);
                result.set("upserts_deferred", summary.upserts_deferred as i64);
                result.set("upserts_replayed", summary.upserts_replayed as i64);
                result.set("drops_applied", summary.drops_applied as i64);
                result.set("drops_refused", summary.drops_refused as i64);
                result.set("drops_deferred", summary.drops_deferred as i64);
                result.set("drops_replayed", summary.drops_replayed as i64);
                result.set("deferred_pending", summary.deferred_pending as i64);
            }
            Err(word) => {
                result.set("status", i64::from(word));
            }
        }
        result
    }

    /// Drive exactly one terrain frame from the pulled frame snapshot's
    /// camera (floor of the camera position into section X/Z, the server's
    /// own streaming rule). The world feature's responsibility is to call
    /// this exactly once per `_process`; the summary dictionary carries the
    /// derived camera and the stage's frame report.
    #[func]
    fn terrain_frame(&mut self) -> VarDictionary {
        let driven = if Self::on_main_thread() {
            self.session.terrain_frame()
        } else {
            Err(abi::STATUS_INTERNAL)
        };
        let mut result = VarDictionary::new();
        match driven {
            Ok(summary) => {
                result.set("status", i64::from(abi::STATUS_OK));
                result.set("camera_dimension", i64::from(summary.camera.dimension));
                result.set("camera_x", i64::from(summary.camera.x));
                result.set("camera_z", i64::from(summary.camera.z));
                result.set(
                    "reclaimed_sections",
                    summary.report.reclaimed_sections as i64,
                );
                result.set(
                    "reclamation_pending",
                    i64::from(summary.report.reclamation_pending),
                );
                result.set("drained_results", summary.report.drained_results as i64);
                result.set("uploads_applied", summary.report.uploads_applied as i64);
                result.set("uploads_failed", summary.report.uploads_failed as i64);
                result.set("uploads_discarded", summary.report.uploads_discarded as i64);
            }
            Err(word) => {
                result.set("status", i64::from(word));
            }
        }
        result
    }

    /// The structural terrain summary as JSON: the live section inventory
    /// (dimension, section coordinates, revision, surface count) plus the
    /// stage facts. This is the no-stale-section proof surface the headless
    /// terrain check asserts against; without an attached pipeline the JSON
    /// carries only the invalid-state status word.
    #[func]
    fn terrain_sections_json(&mut self) -> GString {
        let summary = if Self::on_main_thread() {
            self.session.terrain_summary()
        } else {
            Err(abi::STATUS_INTERNAL)
        };
        match summary {
            Ok(json) => GString::from(json.as_str()),
            Err(word) => GString::from(format!("{{\"status\":{}}}", word).as_str()),
        }
    }

    /// One pull as the typed dictionary every pull shares: the producer
    /// status word and a Godot-owned copy of the record bytes. The record is
    /// never a retained native buffer.
    fn pull_dictionary(&mut self, family: PullFamily) -> VarDictionary {
        let outcome = if Self::on_main_thread() {
            self.session.pull(family)
        } else {
            Err(abi::STATUS_INTERNAL)
        };
        let mut result = VarDictionary::new();
        match outcome {
            Ok(record) => {
                let record_array = PackedByteArray::from(record);
                result.set("status", i64::from(abi::STATUS_OK));
                result.set("record", &record_array);
            }
            Err(word) => {
                let empty = PackedByteArray::new();
                result.set("status", i64::from(word));
                result.set("record", &empty);
            }
        }
        result
    }

    fn put_typed_frame_defaults(result: &mut VarDictionary) {
        result.set("hud_ready", false);
        result.set("health", 0i64);
        result.set("hunger", 0i64);
        result.set("oxygen", 0i64);
        result.set("environment_ready", false);
        result.set("environment_server_tick", 0i64);
        result.set("world_time_ticks", 0i64);
        result.set("day_phase_offset", 0i64);
        result.set("weather", 0i64);
        result.set("season", 0i64);
        result.set("season_progress", 0i64);
        result.set("temperature", 0i64);
        result.set("daylight", 0.0f64);
        result.set("sky_r", 0.0f64);
        result.set("sky_g", 0.0f64);
        result.set("sky_b", 0.0f64);
        result.set("revision", 0i64);
        result.set("epoch", 0i64);
        result.set("camera_ready", false);
        result.set("position_x", 0.0f64);
        result.set("position_y", 0.0f64);
        result.set("position_z", 0.0f64);
        result.set("yaw", 0.0f64);
        result.set("pitch", 0.0f64);
        result.set("fov_y", 0.0f64);
        result.set("aspect", 0.0f64);
        result.set("near", 0.0f64);
        result.set("far", 0.0f64);
        result.set("entity_server_tick", 0i64);
        result.set("entities", &VarArray::new());
        result.set("target_visible", false);
        result.set("target_x", 0i64);
        result.set("target_y", 0i64);
        result.set("target_z", 0i64);
        result.set("target_name", String::new());
        result.set("phase", 0i64);
        result.set("error", 0i64);
    }

    fn put_typed_frame(result: &mut VarDictionary, frame: &TypedFrame, status: u32) {
        result.set("hud_ready", frame.hud_ready);
        result.set("health", i64::from(frame.health));
        result.set("hunger", i64::from(frame.hunger));
        result.set("oxygen", i64::from(frame.oxygen));
        result.set("environment_ready", frame.environment_ready);
        result.set(
            "environment_server_tick",
            i64::try_from(frame.environment_server_tick).unwrap_or(i64::MAX),
        );
        result.set(
            "world_time_ticks",
            i64::try_from(frame.world_time_ticks).unwrap_or(i64::MAX),
        );
        result.set("day_phase_offset", i64::from(frame.day_phase_offset));
        result.set("weather", i64::from(frame.weather));
        result.set("season", i64::from(frame.season));
        result.set("season_progress", i64::from(frame.season_progress));
        result.set("temperature", i64::from(frame.temperature));
        result.set("daylight", f64::from(frame.daylight));
        result.set("sky_r", f64::from(frame.sky_color[0]));
        result.set("sky_g", f64::from(frame.sky_color[1]));
        result.set("sky_b", f64::from(frame.sky_color[2]));
        result.set("status", i64::from(status));
        result.set(
            "revision",
            i64::try_from(frame.revision).unwrap_or(i64::MAX),
        );
        result.set("epoch", i64::try_from(frame.epoch).unwrap_or(i64::MAX));
        result.set("camera_ready", frame.camera_ready);
        result.set("position_x", f64::from(frame.position[0]));
        result.set("position_y", f64::from(frame.position[1]));
        result.set("position_z", f64::from(frame.position[2]));
        result.set("yaw", f64::from(frame.yaw));
        result.set("pitch", f64::from(frame.pitch));
        result.set("fov_y", f64::from(frame.fov_y));
        result.set("aspect", f64::from(frame.aspect));
        result.set("near", f64::from(frame.near));
        result.set("far", f64::from(frame.far));
        result.set(
            "entity_server_tick",
            i64::try_from(frame.entity_server_tick).unwrap_or(i64::MAX),
        );
        let mut entities = VarArray::new();
        for entity in &frame.entities {
            let mut typed = VarDictionary::new();
            typed.set("kind", i64::from(entity.kind));
            typed.set("player_id", entity.player_id_text());
            typed.set("dimension", i64::from(entity.dimension));
            typed.set("position_x", f64::from(entity.position[0]));
            typed.set("position_y", f64::from(entity.position[1]));
            typed.set("position_z", f64::from(entity.position[2]));
            typed.set("yaw", f64::from(entity.yaw));
            typed.set("pitch", f64::from(entity.pitch));
            entities.push(&typed.to_variant());
        }
        result.set("entities", &entities);
        result.set("target_visible", frame.target_visible);
        result.set("target_x", i64::from(frame.target_position[0]));
        result.set("target_y", i64::from(frame.target_position[1]));
        result.set("target_z", i64::from(frame.target_position[2]));
        result.set("target_name", frame.target_name.clone());
        result.set("phase", i64::from(frame.phase));
        result.set("error", i64::from(frame.error));
    }

    /// The resource half of the terrain attach, fail-closed with a status
    /// word plus a stable English detail: load the three materials, resolve
    /// the scenario RID, build and validate the layered atlas texture, and
    /// assemble the budget stage over the production render backend. Every
    /// borrowed RID (materials, scenario) is validated valid; the atlas RID
    /// is the one resource the bridge itself owns.
    fn attach_terrain_resources(
        &self,
        inputs: &TerrainInputs,
    ) -> Result<(BridgeTerrain, Rid), (u32, String)> {
        let rejected = |detail: String| (abi::STATUS_INPUT_REJECTED, detail);
        let mut loader = ResourceLoader::singleton();
        let mut load_material = |path: &str, surface: &str| -> Result<Rid, (u32, String)> {
            let loaded = loader.load(path).ok_or_else(|| {
                rejected(format!(
                    "the {surface} terrain material could not be loaded"
                ))
            })?;
            let material = loaded.try_cast::<ShaderMaterial>().map_err(|_| {
                rejected(format!(
                    "the {surface} terrain material is not a ShaderMaterial"
                ))
            })?;
            let rid = material.get_rid();
            if rid.is_invalid() {
                return Err(rejected(format!(
                    "the {surface} terrain material has no rendering-server RID"
                )));
            }
            Ok(rid)
        };
        let opaque_rid = load_material(&inputs.opaque, "opaque")?;
        let cutout_rid = load_material(&inputs.cutout, "cutout")?;
        let water_rid = load_material(&inputs.water, "water")?;

        let scenario_node = self
            .to_gd()
            .try_get_node_as::<Node3D>(inputs.scenario.as_str())
            .ok_or_else(|| rejected("the terrain scenario node is missing".to_string()))?;
        let world = scenario_node.get_world_3d().ok_or_else(|| {
            rejected("the terrain scenario node is outside a 3D world".to_string())
        })?;
        let scenario_rid = world.get_scenario();
        if scenario_rid.is_invalid() {
            return Err(rejected(
                "the terrain scenario node has no rendering scenario".to_string(),
            ));
        }

        let atlas = Self::build_atlas_texture()?;
        let mut server = RenderingServer::singleton();
        for material in [opaque_rid, cutout_rid, water_rid] {
            server.material_set_param(material, "atlas", &Variant::from(atlas));
        }

        let renderer = TerrainRenderer::new(
            production_render_backend(),
            scenario_rid,
            TerrainMaterials {
                opaque: opaque_rid,
                cutout: cutout_rid,
                water: water_rid,
            },
        );
        // Ownership totality: the bridge owns the atlas RID from the moment
        // the texture exists, so the one failure arm after that point (mesh
        // worker spawn) frees the RID before returning instead of leaking it
        // into engine shutdown. Every earlier failure arm precedes the
        // texture build and owns nothing.
        let stage = match TerrainBudgetStage::new(renderer) {
            Ok(stage) => stage,
            Err(word) => {
                server.free_rid(atlas);
                return Err((word, "the terrain mesh worker could not start".to_string()));
            }
        };
        Ok((BridgeTerrain::assemble(stage, inputs.clone()), atlas))
    }

    /// Build the layered atlas texture from the generated pixel file after
    /// manifest validation. The texture is a plain (non-sRGB) RGBA8 2D array
    /// with the manifest's five precomputed mip levels per layer in the
    /// manifest's layer-major, mip-major storage order: the pinned terrain
    /// shaders sample `atlas` without a color hint and decode with their own
    /// pow(2.2), so an sRGB-tagged upload would double-decode every texel.
    fn build_atlas_texture() -> Result<Rid, (u32, String)> {
        let rejected = |detail: String| (abi::STATUS_INPUT_REJECTED, detail) as (u32, String);
        let manifest_text = FileAccess::get_file_as_string(ATLAS_MANIFEST_PATH);
        let parsed = Json::parse_string(&manifest_text);
        let dictionary = parsed
            .try_to::<VarDictionary>()
            .map_err(|_| rejected("the atlas manifest is not a JSON object".to_string()))?;
        // Godot's JSON parse produces float variants for every JSON number,
        // so numeric manifest fields are read as f64 and compared by value.
        if variant_field::<f64>(&dictionary, "schema_version")
            != Some(ATLAS_MANIFEST_SCHEMA_VERSION as f64)
        {
            return Err(rejected(
                "the atlas manifest schema version is not supported".to_string(),
            ));
        }
        let atlas = dictionary
            .get("atlas")
            .and_then(|value| value.try_to::<VarDictionary>().ok())
            .ok_or_else(|| rejected("the atlas manifest has no atlas identity".to_string()))?;
        for (name, expected, description) in [
            ("format", ATLAS_FORMAT, "pixel format"),
            ("storage_order", ATLAS_STORAGE_ORDER, "storage order"),
            ("path", ATLAS_LAYER_NAME, "pixel file name"),
        ] {
            if variant_field::<GString>(&atlas, name)
                .map(|value| value.to_string())
                .as_deref()
                != Some(expected)
            {
                return Err(rejected(format!(
                    "the atlas manifest {description} is not {expected}"
                )));
            }
        }
        for (name, expected, description) in [
            ("width", ATLAS_LAYER_SIZE, "layer width"),
            ("height", ATLAS_LAYER_SIZE, "layer height"),
            ("mip_levels", ATLAS_MIP_LEVELS, "mip level count"),
        ] {
            if variant_field::<f64>(&atlas, name) != Some(expected as f64) {
                return Err(rejected(format!(
                    "the atlas manifest {description} is not {expected}"
                )));
            }
        }
        let layers = variant_field::<f64>(&atlas, "layers")
            .ok_or_else(|| rejected("the atlas manifest reports no layer count".to_string()))?;
        if !(layers >= 1.0 && layers.fract() == 0.0) {
            return Err(rejected(
                "the atlas manifest reports no atlas layers".to_string(),
            ));
        }
        let pixels = FileAccess::get_file_as_bytes(ATLAS_PIXELS_PATH);
        let pixel_bytes = pixels.as_slice();
        // Fail closed on the layer count before any image is built: the
        // pixel file's own byte length divided by the per-layer mip-chain
        // size is the only reachable layer count, so a corrupted huge
        // integral float (or any mismatch) is a manifest violation instead
        // of a build loop the caller cannot bound.
        let layers_usize = pixel_bytes.len() / ATLAS_LAYER_BYTES;
        if layers_usize < 1 || layers_usize as f64 != layers {
            return Err(rejected(format!(
                "the atlas manifest layer count {} disagrees with the pixel file's {} bytes ({} full layers)",
                layers,
                pixel_bytes.len(),
                layers_usize
            )));
        }
        if pixel_bytes.len() != layers_usize * ATLAS_LAYER_BYTES {
            return Err(rejected(format!(
                "the atlas pixel file carries {} bytes, want {} layers of {}",
                pixel_bytes.len(),
                layers,
                ATLAS_LAYER_BYTES
            )));
        }
        let mut images = Array::<Gd<godot::classes::Image>>::new();
        for layer in 0..layers_usize {
            let start = layer * ATLAS_LAYER_BYTES;
            let data = PackedByteArray::from_iter(
                pixel_bytes[start..start + ATLAS_LAYER_BYTES]
                    .iter()
                    .copied(),
            );
            let image = godot::classes::Image::create_from_data(
                ATLAS_LAYER_SIZE as i32,
                ATLAS_LAYER_SIZE as i32,
                true,
                ImageFormat::RGBA8,
                &data,
            )
            .ok_or_else(|| rejected("an atlas layer could not be decoded".to_string()))?;
            images.push(&image);
        }
        let atlas = RenderingServer::singleton()
            .texture_2d_layered_create(&images, TextureLayeredType::LAYERED_2D_ARRAY);
        if atlas.is_invalid() {
            return Err(rejected(
                "the layered atlas texture could not be created".to_string(),
            ));
        }
        Ok(atlas)
    }

    /// The crate's main-thread check; instance methods answer off-thread
    /// calls with the internal status word instead of panicking.
    fn on_main_thread() -> bool {
        godot::init::is_main_thread()
    }
}

#[cfg(test)]
mod tests {
    use super::{BridgeSession, GODOT_API_MAJOR, GODOT_API_MINOR, GODOT_RUST_VERSION};
    use crate::abi::{
        self, ABI_MAJOR, ABI_MINOR, STATUS_ABI_MISMATCH, STATUS_INPUT_REJECTED,
        STATUS_INSUFFICIENT_CAPACITY, STATUS_INTERNAL, STATUS_INVALID_HANDLE, STATUS_INVALID_STATE,
        STATUS_OK,
    };
    use crate::client_core::{
        ClientHandle, CoreCalls, PullFamily, PullOutcome, encode_step_request,
        production_core_calls,
    };
    use crate::feature_negotiation::PILOT_FAMILIES;
    use crate::status_decode::{TypedStatus, test_status_record};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn bridge_identity_matches_pinned_dependencies() {
        assert_eq!((i64::from(ABI_MAJOR), i64::from(ABI_MINOR)), (1, 1));
        assert_eq!((GODOT_API_MAJOR, GODOT_API_MINOR), (4, 7));
        assert_eq!(GODOT_RUST_VERSION, "0.5.5");
    }

    /// The mutable half of the scripted core-call table.
    struct ScriptState {
        next_handle_word: u64,
        live: Vec<ClientHandle>,
        retired: Vec<ClientHandle>,
        create_status: u32,
        connect_status: u32,
        poll_result: Result<u32, u32>,
        submit_status: u32,
        step_status: u32,
        pull_outcomes: std::collections::VecDeque<PullOutcome>,
        pull_bytes: Vec<u8>,
        create_requests: Vec<(u32, u32, Vec<u64>)>,
        connect_addresses: Vec<Vec<u8>>,
        submitted_batches: Vec<Vec<u8>>,
        step_requests: Vec<[u8; abi::STEP_REQUEST_BYTES]>,
        destroy_calls: usize,
        pull_calls: usize,
        poll_calls: usize,
        pull_views: Vec<(usize, usize)>,
    }

    /// A scripted core-call table shared with the test through an `Rc`, so the
    /// bridge can own its boxed copy while the test keeps inspecting and
    /// scripting the same state. It mirrors the producer's handle vocabulary
    /// exactly (live handle, retired tombstone with idempotent destroy,
    /// anything else invalid) while each family call is scripted by the test.
    #[derive(Clone)]
    struct ScriptedCore {
        state: Rc<RefCell<ScriptState>>,
    }

    impl ScriptedCore {
        fn new() -> Self {
            Self {
                state: Rc::new(RefCell::new(ScriptState {
                    next_handle_word: 1,
                    live: Vec::new(),
                    retired: Vec::new(),
                    create_status: STATUS_OK,
                    connect_status: STATUS_OK,
                    poll_result: Ok(1),
                    submit_status: STATUS_INVALID_STATE,
                    step_status: STATUS_INVALID_STATE,
                    pull_outcomes: std::collections::VecDeque::new(),
                    pull_bytes: Vec::new(),
                    create_requests: Vec::new(),
                    connect_addresses: Vec::new(),
                    submitted_batches: Vec::new(),
                    step_requests: Vec::new(),
                    destroy_calls: 0,
                    pull_calls: 0,
                    poll_calls: 0,
                    pull_views: Vec::new(),
                })),
            }
        }

        fn state(&self) -> std::cell::RefMut<'_, ScriptState> {
            self.state.borrow_mut()
        }

        fn script_pulls(&self, outcomes: &[PullOutcome], bytes: &[u8]) {
            let mut state = self.state();
            state.pull_outcomes = outcomes.iter().copied().collect();
            state.pull_bytes = bytes.to_vec();
        }
    }

    impl CoreCalls for ScriptedCore {
        fn abi_version(&self) -> Option<u64> {
            Some((u64::from(ABI_MAJOR) << 32) | u64::from(ABI_MINOR))
        }

        fn create_session(
            &self,
            abi_major: u32,
            abi_minor: u32,
            requested_families: &[u64],
        ) -> Result<ClientHandle, u32> {
            let mut state = self.state();
            state
                .create_requests
                .push((abi_major, abi_minor, requested_families.to_vec()));
            if state.create_status != STATUS_OK {
                return Err(state.create_status);
            }
            let handle = ClientHandle::from_word(state.next_handle_word);
            state.next_handle_word += 1;
            state.live.push(handle);
            Ok(handle)
        }

        fn destroy_session(&self, handle: ClientHandle) -> u32 {
            let mut state = self.state();
            state.destroy_calls += 1;
            if let Some(index) = state.live.iter().position(|live| *live == handle) {
                state.live.remove(index);
                state.retired.push(handle);
                return STATUS_OK;
            }
            if state.retired.contains(&handle) {
                return STATUS_OK;
            }
            STATUS_INVALID_HANDLE
        }

        fn connect_begin(&self, handle: ClientHandle, address: &[u8]) -> u32 {
            let mut state = self.state();
            if let Some(status) = require_live(&state, handle) {
                return status;
            }
            state.connect_addresses.push(address.to_vec());
            state.connect_status
        }

        fn connect_poll(&self, handle: ClientHandle) -> Result<u32, u32> {
            let mut state = self.state();
            if let Some(status) = require_live(&state, handle) {
                return Err(status);
            }
            state.poll_calls += 1;
            state.poll_result
        }

        fn disconnect_session(&self, handle: ClientHandle) -> u32 {
            let state = self.state();
            if let Some(status) = require_live(&state, handle) {
                return status;
            }
            STATUS_OK
        }

        fn submit_input(&self, handle: ClientHandle, batch: &[u8]) -> u32 {
            let mut state = self.state();
            if let Some(status) = require_live(&state, handle) {
                return status;
            }
            state.submitted_batches.push(batch.to_vec());
            state.submit_status
        }

        fn step(&self, handle: ClientHandle, request: &[u8; abi::STEP_REQUEST_BYTES]) -> u32 {
            let mut state = self.state();
            if let Some(status) = require_live(&state, handle) {
                return status;
            }
            state.step_requests.push(*request);
            state.step_status
        }

        fn world_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
            self.scripted_pull(handle, buffer)
        }

        fn frame_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
            self.scripted_pull(handle, buffer)
        }

        fn status_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
            self.scripted_pull(handle, buffer)
        }

        fn identity_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
            self.scripted_pull(handle, buffer)
        }
    }

    impl ScriptedCore {
        /// The scripted primitive pull shared by every family: pop the next
        /// scripted outcome, record the caller buffer's address and span (the
        /// reuse-accounting evidence for the buffer tests), and fill the
        /// caller buffer from the scripted bytes on a completed write.
        fn scripted_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
            let mut state = self.state();
            if let Some(status) = require_live(&state, handle) {
                return PullOutcome::Status(status);
            }
            state.pull_calls += 1;
            state
                .pull_views
                .push((buffer.as_ptr() as usize, buffer.len()));
            let outcome = state
                .pull_outcomes
                .pop_front()
                .unwrap_or(PullOutcome::Status(STATUS_INTERNAL));
            if let PullOutcome::Complete { written } = outcome {
                for index in 0..buffer.len().min(written as usize) {
                    buffer[index] = state.pull_bytes.get(index).copied().unwrap_or(0);
                }
            }
            outcome
        }
    }

    /// The producer's handle ruling as a helper: a retired tombstone reports
    /// invalid state, an unknown value invalid handle, and a live handle
    /// nothing.
    fn require_live(state: &ScriptState, handle: ClientHandle) -> Option<u32> {
        if state.retired.contains(&handle) {
            return Some(STATUS_INVALID_STATE);
        }
        if !state.live.contains(&handle) {
            return Some(STATUS_INVALID_HANDLE);
        }
        None
    }

    /// A bridge session over a scripted table, with the table handle kept for
    /// scripting and inspection.
    fn scripted_session() -> (BridgeSession, ScriptedCore) {
        let core = ScriptedCore::new();
        let session = BridgeSession::new(Box::new(core.clone()));
        (session, core)
    }

    #[test]
    fn bridge_lifecycle_unavailable_production_table_fails_closed() {
        // The cargo-test production wiring is the fail-closed unavailable
        // table (no test may load the producer library), so creation reports
        // the internal word and no handle ever exists: every later call stays
        // the local invalid state and close stays clean.
        let mut session = BridgeSession::new(production_core_calls());
        assert_eq!(session.create(), STATUS_INTERNAL);
        assert_eq!(session.poll(), (STATUS_INVALID_STATE, 0));
        assert_eq!(session.connect("127.0.0.1:9"), STATUS_INVALID_STATE);
        assert_eq!(session.submit(&[]), STATUS_INVALID_STATE);
        assert_eq!(session.step(0, 0, 0), STATUS_INVALID_STATE);
        assert_eq!(session.pull(PullFamily::World), Err(STATUS_INVALID_STATE));
        assert_eq!(session.status_typed(), Err(STATUS_INVALID_STATE));
        assert_eq!(session.close(), STATUS_OK);
    }

    #[test]
    fn bridge_lifecycle_offline_sequence_closes_idempotently() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        // Offline states: a connecting-or-idle session rejects submission and
        // stepping with the producer's invalid-state word.
        assert_eq!(session.poll(), (STATUS_OK, 1));
        assert_eq!(
            session.submit(&[77, 67, 78, 49, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            STATUS_INVALID_STATE
        );
        assert_eq!(session.step(16_666_667, 64, 32), STATUS_INVALID_STATE);
        assert_eq!(session.close(), STATUS_OK);
        assert_eq!(session.close(), STATUS_OK);
        // The second close is a no-op: exactly one producer destroy ran.
        assert_eq!(core.state().destroy_calls, 1);
    }

    #[test]
    fn bridge_lifecycle_drop_after_close_and_drop_live_both_destroy_once() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        assert_eq!(session.close(), STATUS_OK);
        drop(session);
        assert_eq!(core.state().destroy_calls, 1);

        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        drop(session);
        assert_eq!(core.state().destroy_calls, 1);
    }

    #[test]
    fn bridge_lifecycle_failed_create_keeps_no_handle_and_closes_cleanly() {
        let (mut session, core) = scripted_session();
        core.state().create_status = STATUS_ABI_MISMATCH;
        assert_eq!(session.create(), STATUS_ABI_MISMATCH);
        core.state().create_status = STATUS_INPUT_REJECTED;
        assert_eq!(session.create(), STATUS_INPUT_REJECTED);
        // Close after a failed create must not destroy a handle that was never
        // issued.
        assert_eq!(session.close(), STATUS_OK);
        assert_eq!(core.state().destroy_calls, 0);
    }

    #[test]
    fn bridge_lifecycle_second_create_is_rejected_and_keeps_the_first_handle() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let first = core.state().live[0];
        assert_eq!(session.create(), STATUS_INVALID_STATE);
        assert_eq!(core.state().create_requests.len(), 1);
        assert_eq!(core.state().live, vec![first]);
    }

    #[test]
    fn bridge_lifecycle_create_requests_the_pinned_pilot_families() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let requests = core.state().create_requests.clone();
        assert_eq!(requests.len(), 1);
        let (major, minor, words) = &requests[0];
        assert_eq!((*major, *minor), (ABI_MAJOR, ABI_MINOR));
        let pinned: Vec<u64> = PILOT_FAMILIES
            .iter()
            .map(|descriptor| (u64::from(descriptor.version) << 32) | u64::from(descriptor.family))
            .collect();
        assert_eq!(words, &pinned);
    }

    #[test]
    fn bridge_lifecycle_destroyed_and_unknown_handles_follow_the_producer_vocabulary() {
        let core = ScriptedCore::new();
        assert_eq!(
            core.create_session(ABI_MAJOR, ABI_MINOR, &[1]),
            Ok(ClientHandle::from_word(1))
        );
        let issued = core.state().live[0];
        assert_eq!(core.destroy_session(issued), STATUS_OK);
        // Idempotent destroy of the same tombstone.
        assert_eq!(core.destroy_session(issued), STATUS_OK);
        // Every other export rejects the tombstone with invalid state.
        assert_eq!(core.connect_poll(issued), Err(STATUS_INVALID_STATE));
        assert_eq!(
            core.identity_pull(issued, &mut []),
            PullOutcome::Status(STATUS_INVALID_STATE)
        );
        // A never-issued value is invalid everywhere.
        let unknown = ClientHandle::from_word(999);
        assert_eq!(core.destroy_session(unknown), STATUS_INVALID_HANDLE);
        assert_eq!(core.connect_poll(unknown), Err(STATUS_INVALID_HANDLE));
    }

    #[test]
    fn bridge_lifecycle_post_close_calls_stay_local() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        assert_eq!(session.close(), STATUS_OK);
        let polls = core.state().poll_calls;
        let pulls = core.state().pull_calls;
        assert_eq!(session.poll(), (STATUS_INVALID_STATE, 0));
        assert_eq!(session.pull(PullFamily::World), Err(STATUS_INVALID_STATE));
        assert_eq!(session.status_typed(), Err(STATUS_INVALID_STATE));
        assert_eq!(session.connect("127.0.0.1:9"), STATUS_INVALID_STATE);
        assert_eq!(session.submit(&[]), STATUS_INVALID_STATE);
        assert_eq!(session.step(0, 0, 0), STATUS_INVALID_STATE);
        // No producer call ran after the bridge retired its handle.
        assert_eq!(core.state().poll_calls, polls);
        assert_eq!(core.state().pull_calls, pulls);
    }

    #[test]
    fn bridge_lifecycle_two_phase_pull_queries_then_exact_writes() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let record: Vec<u8> = (0..192u32).map(|byte| (byte % 251) as u8).collect();
        core.script_pulls(
            &[
                PullOutcome::Capacity { required: 192 },
                PullOutcome::Complete { written: 192 },
            ],
            &record,
        );
        assert_eq!(session.pull(PullFamily::Identity), Ok(record));
        assert_eq!(core.state().pull_calls, 2);
    }

    #[test]
    fn bridge_lifecycle_pull_no_batch_returns_an_empty_owned_record() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        core.script_pulls(&[PullOutcome::Complete { written: 0 }], &[]);
        assert_eq!(session.pull(PullFamily::World), Ok(Vec::new()));
        core.script_pulls(&[PullOutcome::Complete { written: 0 }], &[]);
        assert_eq!(session.pull(PullFamily::Frame), Ok(Vec::new()));
    }

    #[test]
    fn bridge_lifecycle_pull_retries_one_capacity_signal_then_fails_closed() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let fresh: Vec<u8> = (0..120u32).map(|byte| byte as u8).collect();
        // A batch that grew between the query and the write allows exactly one
        // retry with the fresh required size.
        core.script_pulls(
            &[
                PullOutcome::Capacity { required: 100 },
                PullOutcome::Capacity { required: 120 },
                PullOutcome::Complete { written: 120 },
            ],
            &fresh,
        );
        assert_eq!(session.pull(PullFamily::World), Ok(fresh.clone()));
        assert_eq!(core.state().pull_calls, 3);

        // A second capacity signal after the one retry is a producer
        // contradiction and fails closed with the capacity status word.
        core.script_pulls(
            &[
                PullOutcome::Capacity { required: 100 },
                PullOutcome::Capacity { required: 120 },
                PullOutcome::Capacity { required: 200 },
            ],
            &[],
        );
        assert_eq!(
            session.pull(PullFamily::World),
            Err(STATUS_INSUFFICIENT_CAPACITY)
        );

        // An error status on the write phase surfaces the raw word.
        core.script_pulls(
            &[
                PullOutcome::Capacity { required: 100 },
                PullOutcome::Status(STATUS_INTERNAL),
            ],
            &[],
        );
        assert_eq!(session.pull(PullFamily::World), Err(STATUS_INTERNAL));
    }

    #[test]
    fn bridge_lifecycle_pull_contradictory_query_outcomes_fail_closed() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        // A completed write into a zero-capacity query and a zero required
        // size both contradict the two-phase protocol; the driver fails closed
        // instead of guessing.
        core.script_pulls(&[PullOutcome::Complete { written: 40 }], &[0; 40]);
        assert_eq!(session.pull(PullFamily::World), Err(STATUS_INTERNAL));
        core.script_pulls(&[PullOutcome::Capacity { required: 0 }], &[]);
        assert_eq!(session.pull(PullFamily::World), Err(STATUS_INTERNAL));
    }

    #[test]
    fn bridge_lifecycle_pulled_records_are_owned_copies() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let record = vec![7u8; 64];
        core.script_pulls(
            &[
                PullOutcome::Capacity { required: 64 },
                PullOutcome::Complete { written: 64 },
            ],
            &record,
        );
        let mut pulled = session.pull(PullFamily::Frame).expect("record");
        assert_eq!(pulled, record);
        // Mutating the returned buffer must not corrupt the next pull.
        pulled.iter_mut().for_each(|byte| *byte = 0);
        core.script_pulls(
            &[
                PullOutcome::Capacity { required: 64 },
                PullOutcome::Complete { written: 64 },
            ],
            &record,
        );
        assert_eq!(session.pull(PullFamily::Frame), Ok(record));
    }

    #[test]
    fn bridge_lifecycle_pull_without_a_session_reports_invalid_state() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.pull(PullFamily::Status), Err(STATUS_INVALID_STATE));
        assert_eq!(session.status_typed(), Err(STATUS_INVALID_STATE));
        assert_eq!(core.state().pull_calls, 0);
    }

    #[test]
    fn bridge_lifecycle_status_typed_decodes_the_pulled_record_set() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let record = test_status_record(5, 1, 7, 900);
        core.script_pulls(
            &[
                PullOutcome::Capacity {
                    required: record.len() as u32,
                },
                PullOutcome::Complete {
                    written: record.len() as u32,
                },
            ],
            &record,
        );
        // One two-phase pull, then the pure Rust-side decode: Python would
        // receive the typed dictionary, never these record bytes.
        assert_eq!(
            session.status_typed(),
            Ok(TypedStatus {
                phase: 5,
                terminal_cause: 1,
                steps_completed: 7,
                messages_processed: 900,
            })
        );
        assert_eq!(core.state().pull_calls, 2);
    }

    #[test]
    fn bridge_lifecycle_status_typed_fails_closed_on_drift_and_errors() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);

        // A record set the decoder rejects reports the internal word.
        let mut drifted = test_status_record(5, 1, 7, 900);
        drifted[0..4].copy_from_slice(&abi::MAGIC_FRAME.to_le_bytes());
        core.script_pulls(
            &[
                PullOutcome::Capacity {
                    required: drifted.len() as u32,
                },
                PullOutcome::Complete {
                    written: drifted.len() as u32,
                },
            ],
            &drifted,
        );
        assert_eq!(session.status_typed(), Err(STATUS_INTERNAL));

        // An empty record (a no-content producer outcome) cannot carry the
        // phase record the family always promises; it fails closed too.
        core.script_pulls(&[PullOutcome::Complete { written: 0 }], &[]);
        assert_eq!(session.status_typed(), Err(STATUS_INTERNAL));

        // A producer error word surfaces unchanged through the typed path.
        core.script_pulls(
            &[
                PullOutcome::Capacity { required: 8 },
                PullOutcome::Status(STATUS_INVALID_STATE),
            ],
            &[],
        );
        assert_eq!(session.status_typed(), Err(STATUS_INVALID_STATE));
    }

    #[test]
    fn bridge_lifecycle_submit_and_step_pass_exact_wire_bytes() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        core.state().submit_status = STATUS_OK;
        core.state().step_status = STATUS_OK;
        let batch: Vec<u8> = (0..16u32).map(|byte| byte as u8).collect();
        assert_eq!(session.submit(&batch), STATUS_OK);
        assert_eq!(core.state().submitted_batches, vec![batch]);

        assert_eq!(session.step(16_666_667, 64, 32), STATUS_OK);
        let request = encode_step_request(16_666_667, 64, 32);
        assert_eq!(core.state().step_requests, vec![request]);
        // The frozen record: magic "MCS1", layout 1, little-endian elapsed and
        // budgets.
        assert_eq!(request[0..4], abi::MAGIC_STEP.to_le_bytes());
        assert_eq!(request[4..8], abi::STEP_VERSION.to_le_bytes());
        assert_eq!(request[8..16], 16_666_667u64.to_le_bytes());
        assert_eq!(request[16..20], 64u32.to_le_bytes());
        assert_eq!(request[20..24], 32u32.to_le_bytes());
    }

    #[test]
    fn bridge_lifecycle_semantic_submit_encodes_the_complete_desktop_intent() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        core.state().submit_status = STATUS_OK;
        assert_eq!(
            session.submit_semantic(1, -1, true, true, false, true, false, 0.5, -0.2),
            STATUS_OK
        );
        let batch = &core.state().submitted_batches[0];
        assert_eq!(batch[0..4], abi::MAGIC_INPUT.to_le_bytes());
        assert_eq!(batch[4..8], abi::INPUT_VERSION.to_le_bytes());
        assert_eq!(u32::from_le_bytes(batch[8..12].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(batch[16..20].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(batch[20..24].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_le_bytes(batch[24..28].try_into().unwrap()),
            u32::MAX
        );
        assert_eq!(u32::from_le_bytes(batch[48..52].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(batch[52..56].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(batch[64..68].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(batch[68..72].try_into().unwrap()), 1);
        assert_eq!(
            session.submit_semantic(2, 0, false, false, false, false, false, 0.0, 0.0),
            STATUS_INPUT_REJECTED
        );
    }

    #[test]
    fn bridge_lifecycle_connect_passes_address_bytes_and_requires_a_session() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.connect("127.0.0.1:9"), STATUS_INVALID_STATE);
        assert!(core.state().connect_addresses.is_empty());
        assert_eq!(session.create(), STATUS_OK);
        assert_eq!(session.connect("127.0.0.1:9"), STATUS_OK);
        assert_eq!(
            core.state().connect_addresses,
            vec![b"127.0.0.1:9".to_vec()]
        );
        // A producer refusal (for example a second begin on a begun session)
        // surfaces unchanged.
        core.state().connect_status = STATUS_INVALID_STATE;
        assert_eq!(session.connect("127.0.0.1:10"), STATUS_INVALID_STATE);
    }

    #[test]
    fn bridge_lifecycle_every_status_word_maps_without_panicking() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let words = [
            STATUS_OK,
            1,
            STATUS_ABI_MISMATCH,
            STATUS_INPUT_REJECTED,
            STATUS_INSUFFICIENT_CAPACITY,
            5,
            STATUS_INVALID_STATE,
            7,
            STATUS_INTERNAL,
            9,
            10,
            11,
            100,
            u32::MAX,
        ];
        for word in words {
            core.state().submit_status = word;
            core.state().step_status = word;
            core.state().connect_status = word;
            core.state().poll_result = Err(word);
            core.script_pulls(
                &[
                    PullOutcome::Capacity { required: 8 },
                    PullOutcome::Status(word),
                ],
                &[],
            );
            assert_eq!(session.submit(&[]), word, "submit word {word}");
            assert_eq!(session.step(1, 1, 1), word, "step word {word}");
            assert_eq!(session.connect("127.0.0.1:9"), word, "connect word {word}");
            assert_eq!(session.poll(), (word, 0), "poll word {word}");
            assert_eq!(
                session.pull(PullFamily::Status),
                Err(word),
                "pull word {word}"
            );
            // The typed surface scripts its own two-phase pair: the query
            // fails with the same word before any decode runs.
            core.script_pulls(
                &[
                    PullOutcome::Capacity { required: 8 },
                    PullOutcome::Status(word),
                ],
                &[],
            );
            assert_eq!(
                session.status_typed(),
                Err(word),
                "typed status word {word}"
            );
        }
    }

    #[test]
    fn bridge_lifecycle_step_request_encoder_pins_the_wire_record() {
        let request = encode_step_request(u64::MAX, 4096, 4096);
        assert_eq!(
            request,
            [
                0x4D, 0x43, 0x53, 0x31, 0x01, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
                0xFF, 0xFF, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00,
            ]
        );
        assert_eq!(encode_step_request(0, 0, 0)[0..4], [0x4D, 0x43, 0x53, 0x31]);
    }

    #[test]
    fn bridge_lifecycle_pull_buffers_reuse_one_backing_across_session_pulls() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let record: Vec<u8> = (0..192u32).map(|byte| (byte % 251) as u8).collect();
        for _ in 0..3 {
            core.script_pulls(
                &[
                    PullOutcome::Capacity { required: 192 },
                    PullOutcome::Complete { written: 192 },
                ],
                &record,
            );
            // The Python-facing value stays an owned copy: mutating it cannot
            // corrupt the next pull even though the FFI-side backing is
            // shared across pulls.
            let mut pulled = session.pull(PullFamily::Identity).expect("record");
            assert_eq!(pulled, record);
            pulled.iter_mut().for_each(|byte| *byte = 0);
        }
        // Three two-phase pulls made six producer calls, and the three exact
        // writes landed at one stable FFI-side backing address: no per-pull
        // buffer allocation in the steady state.
        let views = core.state().pull_views.clone();
        assert_eq!(views.len(), 6);
        let writes: Vec<(usize, usize)> = views
            .iter()
            .copied()
            .filter(|(_, length)| *length > 0)
            .collect();
        assert_eq!(writes.len(), 3);
        assert!(
            writes.iter().all(|view| view.0 == writes[0].0),
            "the session reuses one FFI backing address"
        );
    }

    #[test]
    fn bridge_lifecycle_pull_buffers_reset_identities_on_session_recreate() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let first = crate::pull_buffers::test_world_record(9, 1, 1, 0);
        core.script_pulls(
            &[
                PullOutcome::Capacity {
                    required: first.len() as u32,
                },
                PullOutcome::Complete {
                    written: first.len() as u32,
                },
            ],
            &first,
        );
        assert_eq!(session.pull(PullFamily::World), Ok(first));

        // Close and create again: wire epochs are per producer session, so a
        // fresh session's epoch space legitimately restarts below the old
        // one and must not be refused as stale.
        assert_eq!(session.close(), STATUS_OK);
        assert_eq!(session.create(), STATUS_OK);
        let second = crate::pull_buffers::test_world_record(1, 1, 1, 0);
        core.script_pulls(
            &[
                PullOutcome::Capacity {
                    required: second.len() as u32,
                },
                PullOutcome::Complete {
                    written: second.len() as u32,
                },
            ],
            &second,
        );
        assert_eq!(session.pull(PullFamily::World), Ok(second));
    }

    use crate::mesh_worker::SectionId;
    use crate::quad_decode::TestQuad;
    use crate::terrain_bridge::test_records::{frame_record_with_camera, world_record};
    use crate::terrain_bridge::{BridgeTerrain, TerrainInputs};
    use crate::terrain_budget::TerrainBudgetStage;
    use crate::terrain_resources::render_script::ScriptedBackend;
    use std::thread;
    use std::time::{Duration, Instant};

    const TEST_TIMEOUT: Duration = Duration::from_secs(20);
    const POLL_STEP: Duration = Duration::from_millis(1);

    fn attached_session() -> (BridgeSession, ScriptedCore) {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        let (renderer, _render_script) = ScriptedBackend::scripted_renderer();
        let stage = TerrainBudgetStage::new(renderer).expect("stage starts");
        let inputs = TerrainInputs {
            opaque: "res://test/opaque.tres".into(),
            cutout: "res://test/cutout.tres".into(),
            water: "res://test/water.tres".into(),
            scenario: "/root/ScenarioNode".into(),
        };
        session.attach_terrain(BridgeTerrain::assemble(stage, inputs));
        (session, core)
    }

    fn section(x: i32, y: i32, z: i32, revision: u64) -> SectionId {
        SectionId {
            dimension: 0,
            x,
            y,
            z,
            revision,
        }
    }

    /// Serve one world record through the scripted two-phase pull.
    fn script_world(core: &ScriptedCore, record: &[u8]) {
        core.script_pulls(
            &[
                PullOutcome::Capacity {
                    required: record.len() as u32,
                },
                PullOutcome::Complete {
                    written: record.len() as u32,
                },
            ],
            record,
        );
    }

    #[test]
    fn bridge_terrain_surface_ingests_frames_and_resets_on_close() {
        let (mut session, core) = attached_session();

        // One world batch through the real pull path: an upsert plus a
        // producer drop of an absent section.
        let record = world_record(
            1,
            1,
            &[(section(0, 0, 0, 1), vec![TestQuad::default().pack()])],
            &[section(4, 0, 0, 1)],
        );
        script_world(&core, &record);
        let summary = session
            .terrain_ingest()
            .expect("ingest through the pull path");
        assert_eq!(summary.operations, 2);
        assert_eq!(summary.upserts_submitted, 1);
        assert_eq!(summary.drops_applied, 1);

        // Drive frames until the section is live; each frame pull serves
        // the same non-consuming frame record.
        let frame = frame_record_with_camera(true, [8.0, -32.0, 8.0]);
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            script_world(&core, &frame);
            session.terrain_frame().expect("frame drives");
            if session
                .terrain_summary()
                .expect("summary")
                .contains("\"live_sections\":1")
            {
                break;
            }
            assert!(Instant::now() < deadline, "the section never went live");
            thread::sleep(POLL_STEP);
        }
        let summary = session.terrain_summary().expect("summary");
        assert!(summary.contains("\"revision\":1"));
        assert!(summary.contains("\"x\":0"));
        assert!(summary.contains("\"z\":0"));

        // Close rides the terrain reset: strictly monotonic epoch, no live
        // section, and the deferred remainder cleared.
        assert_eq!(session.close(), STATUS_OK);
        let json = session.terrain_summary().expect("summary survives close");
        assert!(json.contains("\"live_sections\":0"));
        assert!(json.contains("\"resets\":1"));
        assert!(json.contains("\"epoch\":2"));

        // After close the pull fails first, so the ingest reports the local
        // invalid-state word without touching the stage.
        assert_eq!(session.terrain_ingest(), Err(STATUS_INVALID_STATE));
        assert_eq!(session.terrain_frame(), Err(STATUS_INVALID_STATE));

        // Re-entry: a fresh producer session under the same stage ingests a
        // lower revision legally because the reset RID table holds nothing.
        assert_eq!(session.create(), STATUS_OK);
        let reentry = world_record(
            1,
            1,
            &[(section(0, 0, 0, 1), vec![TestQuad::default().pack()])],
            &[],
        );
        script_world(&core, &reentry);
        let summary = session.terrain_ingest().expect("re-entry ingest");
        assert_eq!(summary.upserts_submitted, 1);
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            script_world(&core, &frame);
            session.terrain_frame().expect("re-entry frame drives");
            if session
                .terrain_summary()
                .expect("summary")
                .contains("\"live_sections\":1")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the re-entry section never went live"
            );
            thread::sleep(POLL_STEP);
        }
        assert_eq!(session.close(), STATUS_OK);
    }

    #[test]
    fn bridge_terrain_surface_requires_attachment_before_pulling() {
        let (mut session, core) = scripted_session();
        assert_eq!(session.create(), STATUS_OK);
        // Without an attached pipeline both typed paths fail with the local
        // invalid-state word and consume no batch: the scripted pull queue
        // below stays untouched.
        assert_eq!(session.terrain_ingest(), Err(STATUS_INVALID_STATE));
        assert_eq!(session.terrain_frame(), Err(STATUS_INVALID_STATE));
        assert_eq!(core.state().pull_calls, 0);
        assert_eq!(session.close(), STATUS_OK);
    }
}
