//! Pure configuration migration prepares a complete v5 result before caller-owned durability.
use super::*;
use std::collections::BTreeMap;

/// Separates invalid save data from the caller's unchanged entropy failure.
#[derive(Debug)]
pub enum CompanionMergeError<E> {
    /// Codec, configuration, revision, or generated-identity admission failed.
    Storage(StorageError),
    /// The caller's identity provider failed; its original cause remains owned here.
    Identity(E),
}

impl<E> From<StorageError> for CompanionMergeError<E> {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl<E: std::fmt::Display> std::fmt::Display for CompanionMergeError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::Identity(error) => write!(f, "companion identity generation failed: {error}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CompanionMergeError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Identity(error) => Some(error),
        }
    }
}

#[derive(Clone, Copy)]
enum PendingIdentity {
    None,
    Mirror,
    Tombstone,
}

/// Merges configured identities while preserving stored bodies and independent memory revisions.
/// Entropy belongs to the caller and is consumed in canonical order; errors return no partial save.
/// With no generator, explicitly choose an error type, such as `std::convert::Infallible`.
pub fn merge_companions_v5<E>(
    loaded: &StoredCompanions,
    active: &[CompanionBody],
    generate: Option<&mut dyn FnMut() -> Result<PlayerId, E>>,
) -> Result<(StoredCompanions, bool), CompanionMergeError<E>> {
    let input = canonical_input(loaded)?;
    if active.len() > MAX_ACTIVE {
        return Err(corrupt("companion active count", "exceeds limit").into());
    }
    let mut active_by_id = BTreeMap::new();
    for body in active {
        validate_body(body).map_err(|detail| corrupt("companion active body", detail))?;
        if active_by_id.insert(body.id.to_bytes(), body).is_some() {
            return Err(corrupt("companion active bodies", "duplicate ID").into());
        }
    }
    let mut bodies: BTreeMap<_, _> = input.records.iter().map(|b| (b.id.to_bytes(), b)).collect();
    for (&id, &body) in &active_by_id {
        bodies.entry(id).or_insert(body);
    }
    if bodies.len() > MAX_STORED {
        return Err(corrupt("companion count", "active and inactive union exceeds limit").into());
    }
    let old_queues: BTreeMap<_, _> = input.queues.iter().map(|q| (q.id.to_bytes(), q)).collect();
    let old_lifecycles: BTreeMap<_, _> = input
        .lifecycles
        .iter()
        .map(|l| (l.id.to_bytes(), l))
        .collect();
    let mut changed = input.source_schema != CURRENT_SCHEMA;
    let mut lifecycles = Vec::with_capacity(bodies.len());
    let mut queues = Vec::new();
    let mut pending = Vec::with_capacity(bodies.len());
    for (&id, &body) in &bodies {
        let should_be_active = active_by_id.contains_key(&id);
        let old = old_lifecycles.get(&id).copied();
        let mut identity = PendingIdentity::None;
        let next;
        if input.source_schema != CURRENT_SCHEMA {
            let mut lifecycle = zero_lifecycle(body.id, should_be_active, 1);
            if should_be_active {
                if let Some(&old_queue) = old_queues.get(&id) {
                    let mut queue = old_queue.clone();
                    if !queue.summary.is_empty() {
                        lifecycle.memory_revision = 1;
                        lifecycle.summary = std::mem::take(&mut queue.summary);
                        identity = PendingIdentity::Mirror;
                    }
                    if queue.has_current || !queue.pending.is_empty() {
                        queues.push(queue);
                    }
                }
            } else {
                identity = PendingIdentity::Tombstone;
            }
            next = lifecycle;
        } else if let Some(old) = old {
            if old.active == should_be_active {
                next = old.clone();
                if should_be_active && let Some(&queue) = old_queues.get(&id) {
                    queues.push(queue.clone());
                }
            } else {
                changed = true;
                let epoch = old
                    .memory_epoch
                    .checked_add(1)
                    .ok_or_else(|| corrupt("companion memory epoch", "overflow"))?;
                next = zero_lifecycle(body.id, should_be_active, epoch);
                if !should_be_active {
                    identity = PendingIdentity::Tombstone;
                }
            }
        } else {
            changed = true;
            next = zero_lifecycle(body.id, true, 1);
        }
        lifecycles.push(next);
        pending.push(identity);
    }
    // An unchanged v5 value needs neither a new aggregate revision nor an entropy provider.
    if !changed {
        return Ok((input, false));
    }
    let revision = input
        .revision
        .checked_add(1)
        .ok_or_else(|| corrupt("companion aggregate revision", "overflow"))?;
    let generate = generate.ok_or_else(|| corrupt("companion identity generator", "missing"))?;
    let mut namespace = input.agent_namespace_id;
    if !namespace.is_valid() {
        namespace = generated_identity(generate, "namespace")?;
    }
    for (lifecycle, identity) in lifecycles.iter_mut().zip(pending) {
        match identity {
            PendingIdentity::None => {}
            PendingIdentity::Mirror => {
                lifecycle.memory_operation_id = generated_identity(generate, "memory operation")?
            }
            PendingIdentity::Tombstone => {
                lifecycle.tombstone_operation_id =
                    generated_identity(generate, "tombstone operation")?
            }
        }
    }
    let records: Vec<_> = bodies.into_values().cloned().collect();
    let (records, lifecycles, queues) =
        canonical_v5_parts_borrowed(revision, namespace, &records, &lifecycles, &queues)?;
    Ok((
        StoredCompanions {
            source_schema: CURRENT_SCHEMA,
            revision,
            agent_namespace_id: namespace,
            records,
            lifecycles,
            queues,
        },
        true,
    ))
}

fn zero_lifecycle(id: PlayerId, active: bool, memory_epoch: u64) -> StoredCompanionLifecycle {
    StoredCompanionLifecycle {
        id,
        active,
        memory_epoch,
        memory_revision: 0,
        memory_operation_id: Identity::default(),
        summary: String::new(),
        tombstone_operation_id: Identity::default(),
    }
}
fn generated_identity<E>(
    generate: &mut dyn FnMut() -> Result<Identity, E>,
    purpose: &str,
) -> Result<Identity, CompanionMergeError<E>> {
    let value = generate().map_err(CompanionMergeError::Identity)?;
    if !value.is_valid() {
        return Err(corrupt("companion generated identity", purpose).into());
    }
    Ok(value)
}

fn canonical_input(loaded: &StoredCompanions) -> StorageResult<StoredCompanions> {
    match loaded.source_schema {
        0 => {
            if loaded.revision != 0
                || !loaded.agent_namespace_id.is_zero()
                || !loaded.records.is_empty()
                || !loaded.lifecycles.is_empty()
                || !loaded.queues.is_empty()
            {
                return Err(corrupt("companion missing aggregate", "has data"));
            }
            Ok(StoredCompanions::default())
        }
        SCHEMA_V1..=SCHEMA_V4 => {
            if loaded.revision == 0
                || !loaded.agent_namespace_id.is_zero()
                || !loaded.lifecycles.is_empty()
            {
                return Err(corrupt("companion legacy metadata", "invalid"));
            }
            if loaded.records.len() > MAX_STORED || loaded.queues.len() > MAX_ACTIVE {
                return Err(corrupt("companion legacy count", "exceeds limit"));
            }
            let mut records: Vec<_> = loaded.records.iter().collect();
            records.sort_by_key(|b| b.id.to_bytes());
            for (index, body) in records.iter().enumerate() {
                validate_body(body).map_err(|detail| corrupt("companion legacy body", detail))?;
                if index > 0 && records[index - 1].id == body.id {
                    return Err(corrupt("companion legacy body", "duplicate ID"));
                }
            }
            if loaded.source_schema == SCHEMA_V1 && !loaded.queues.is_empty() {
                return Err(corrupt("companion legacy queues", "schema one has queues"));
            }
            validate_queues(&loaded.queues, &loaded.records, loaded.source_schema)
                .map_err(|detail| corrupt("companion legacy queues", detail))?;
            if loaded.source_schema < SCHEMA_V4
                && loaded.queues.iter().any(|q| !q.summary.is_empty())
            {
                return Err(corrupt("companion legacy summary", "before schema four"));
            }
            let mut queues = loaded.queues.clone();
            queues.sort_by_key(|q| q.id.to_bytes());
            Ok(StoredCompanions {
                source_schema: loaded.source_schema,
                revision: loaded.revision,
                agent_namespace_id: Identity::default(),
                records: records.into_iter().cloned().collect(),
                lifecycles: vec![],
                queues,
            })
        }
        CURRENT_SCHEMA => {
            let (records, lifecycles, queues) = canonical_v5_parts_borrowed(
                loaded.revision,
                loaded.agent_namespace_id,
                &loaded.records,
                &loaded.lifecycles,
                &loaded.queues,
            )?;
            Ok(StoredCompanions {
                source_schema: CURRENT_SCHEMA,
                revision: loaded.revision,
                agent_namespace_id: loaded.agent_namespace_id,
                records,
                lifecycles,
                queues,
            })
        }
        other => Err(future_version("companion schema", other)),
    }
}
