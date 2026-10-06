//! Topic module for domain.command_inventory corpus cases.
//!
//! The eleven rules here are the Go producer's inventory, container and chat
//! command admissions. Each executor parses the case input, selects the Go
//! rejection rule from the raw values in the producer's own precedence, and
//! then proves the matching public Rust constructor agrees: a classified
//! acceptance must construct, and a classified rejection must return the
//! mapped `DomainError`. A rejection whose bad value cannot enter any Rust
//! type (an unknown container kind, an unknown view, or a reference carried
//! by a non-container view) is decided by the pre-construction classifier
//! alone, exactly as the adapter table names it.
//!
//! Every sequenced payload is wrapped in a `CommandEnvelope`. The producer
//! supplies only the sequence, so the tick, session and arrival index are
//! synthesized here; they are intake metadata that never reaches the
//! normalized projection. `chat-intent` is deliberately outside that stream:
//! it becomes a `CommandText` and then a `ChatIntent`, never a `Command`.
//!
//! The zero container reference never enters a domain type. Absence of a
//! container is the `StackView` variant itself; the all-zero reference the
//! normalized record shows for a non-container view is the wire's sentinel
//! form, reproduced here only as projection data.

use super::support::{
    DispatchError, ExecutedCase, JsonMap, assert_domain_normalized, execute_topic, input_object,
    invalid_case, normalize_u64, normalized_error, normalized_ok, optional_string, required_bool,
    required_i32, required_string, required_u8, required_u32, required_u64,
};
use crate::runtime_corpus::{CorpusConsumer, FrozenCase};
use mornlea_domain::{
    ChatIntent, ChunkPos, Command, CommandEnvelope, CommandEnvelopeParts, CommandText,
    ContainerKind, ContainerMove, ContainerRef, CraftingMove, DomainError, InventoryMove,
    PartialMove, StackSource, StackView,
};
use serde_json::Value;

pub const EXPECTED_COUNT: usize = 54;

const RULES: &[&str] = &[
    "move-inventory",
    "move-crafting",
    "move-container",
    "close-container",
    "drop-selected-item",
    "take-crafting-output",
    "equip-armor",
    "move-partial",
    "quick-move",
    "drop-stack",
    "chat-intent",
];

pub fn owns(case: &FrozenCase) -> bool {
    if case.consumer != CorpusConsumer::Domain || case.family != "domain.command_inventory" {
        return false;
    }
    let rule = match case
        .input_json
        .as_ref()
        .and_then(|v| v.get("rule"))
        .and_then(|v| v.as_str())
    {
        Some(r) => r,
        None => return false,
    };
    RULES.contains(&rule)
}

pub fn execute(case: &FrozenCase) -> Result<serde_json::Value, DispatchError> {
    let input = input_object(case)?;
    let rule = required_string(case, input, "rule")?;
    match rule {
        "move-inventory" => execute_move_inventory(case, input),
        "move-crafting" => execute_move_crafting(case, input),
        "move-container" => execute_move_container(case, input),
        "close-container" => execute_sequence_only(case, input, "close-container"),
        "drop-selected-item" => execute_sequence_only(case, input, "drop-selected-item"),
        "take-crafting-output" => execute_take_crafting_output(case, input),
        "equip-armor" => execute_sequence_only(case, input, "equip-armor"),
        "move-partial" => execute_move_partial(case, input),
        "quick-move" => execute_source_only(case, input, SourceRule::QuickMove),
        "drop-stack" => execute_source_only(case, input, SourceRule::DropStack),
        "chat-intent" => execute_chat_intent(case, input),
        unknown => Err(invalid_case(
            case,
            format!("unknown inventory rule '{unknown}'"),
        )),
    }
}

/// Executes one `move-inventory` case, whose payload bounds are the fixed
/// inventory range and the distinct-slot rule.
fn execute_move_inventory(
    case: &FrozenCase,
    input: &JsonMap,
) -> Result<serde_json::Value, DispatchError> {
    let sequence = required_u64(case, input, "sequence")?;
    let from = required_u8(case, input, "from")?;
    let to = required_u8(case, input, "to")?;

    match move_inventory_rule(from, to) {
        Some(rejection) => match InventoryMove::try_new(from, to) {
            Err(error) if Some(error) == rejection.error => normalized_error(
                case,
                rejection.category,
                rejection.rule,
                move_projection(sequence, from, to),
            ),
            _ => Err(classification_conflict(
                case,
                "move-inventory",
                rejection.rule,
            )),
        },
        None => match InventoryMove::try_new(from, to) {
            Ok(move_) => {
                let envelope = wrap(sequence, Command::MoveInventory(move_))
                    .map_err(|_| classification_conflict(case, "move-inventory", ""))?;
                let fields = move_projection(envelope.sequence(), move_.from(), move_.to());
                Ok(normalized_ok("move-inventory", fields))
            }
            Err(_) => Err(classification_conflict(case, "move-inventory", "")),
        },
    }
}

/// Executes one `move-crafting` case, which adds the rule that both ends may
/// not sit inside the backpack region.
fn execute_move_crafting(
    case: &FrozenCase,
    input: &JsonMap,
) -> Result<serde_json::Value, DispatchError> {
    let sequence = required_u64(case, input, "sequence")?;
    let from = required_u8(case, input, "from")?;
    let to = required_u8(case, input, "to")?;

    match move_crafting_rule(from, to) {
        Some(rejection) => match CraftingMove::try_new(from, to) {
            Err(error) if Some(error) == rejection.error => normalized_error(
                case,
                rejection.category,
                rejection.rule,
                move_projection(sequence, from, to),
            ),
            _ => Err(classification_conflict(
                case,
                "move-crafting",
                rejection.rule,
            )),
        },
        None => match CraftingMove::try_new(from, to) {
            Ok(move_) => {
                let envelope = wrap(sequence, Command::MoveCrafting(move_))
                    .map_err(|_| classification_conflict(case, "move-crafting", ""))?;
                let fields = move_projection(envelope.sequence(), move_.from(), move_.to());
                Ok(normalized_ok("move-crafting", fields))
            }
            Err(_) => Err(classification_conflict(case, "move-crafting", "")),
        },
    }
}

/// Executes one `move-container` case.
///
/// The reference is decided before any slot is consulted, exactly as the Go
/// DTO does, so a malformed reference is never reported as a slot problem.
fn execute_move_container(
    case: &FrozenCase,
    input: &JsonMap,
) -> Result<serde_json::Value, DispatchError> {
    let sequence = required_u64(case, input, "sequence")?;
    let raw = parse_ref(case, input)?;
    let from = required_u8(case, input, "from")?;
    let to = required_u8(case, input, "to")?;

    let classified = match ref_parts(&raw) {
        Err(rejection) => Some(rejection),
        Ok(parts) => container_move_rule(&parts, from, to),
    };

    match classified {
        Some(rejection) => {
            verify_rejection(case, "move-container", &rejection, || {
                match ref_parts(&raw) {
                    // A classifier-only kind rejection has no constructible
                    // reference, so the mapped-error check never runs.
                    Err(_) => Ok(()),
                    Ok(parts) => ContainerMove::try_new(
                        parts.chunk,
                        parts.kind,
                        parts.slot,
                        parts.generation,
                        from,
                        to,
                    )
                    .map(|_| ()),
                }
            })?;
            let mut fields = raw_ref_fields(&raw);
            fields.insert("sequence".to_string(), normalize_u64(sequence));
            fields.insert("from".to_string(), Value::from(from));
            fields.insert("to".to_string(), Value::from(to));
            normalized_error(case, rejection.category, rejection.rule, fields)
        }
        None => {
            let parts =
                ref_parts(&raw).map_err(|_| classification_conflict(case, "move-container", ""))?;
            match ContainerMove::try_new(
                parts.chunk,
                parts.kind,
                parts.slot,
                parts.generation,
                from,
                to,
            ) {
                Ok(move_) => {
                    let envelope = wrap(sequence, Command::MoveContainer(move_))
                        .map_err(|_| classification_conflict(case, "move-container", ""))?;
                    let mut fields = JsonMap::new();
                    fields.insert("sequence".to_string(), normalize_u64(envelope.sequence()));
                    fields.insert("from".to_string(), Value::from(move_.from()));
                    fields.insert("to".to_string(), Value::from(move_.to()));
                    push_container_fields(&mut fields, &move_.container());
                    Ok(normalized_ok("move-container", fields))
                }
                Err(_) => Err(classification_conflict(case, "move-container", "")),
            }
        }
    }
}

/// Executes one command whose entire payload is the sequence. The protocol
/// publishes no further bound, so there is no rejection to classify.
fn execute_sequence_only(
    case: &FrozenCase,
    input: &JsonMap,
    subject: &'static str,
) -> Result<serde_json::Value, DispatchError> {
    let sequence = required_u64(case, input, "sequence")?;
    let command = match subject {
        "close-container" => Command::CloseContainer,
        "drop-selected-item" => Command::DropSelectedItem,
        "equip-armor" => Command::EquipArmor,
        _ => return Err(classification_conflict(case, subject, "")),
    };
    match wrap(sequence, command) {
        Ok(envelope) => {
            let mut fields = JsonMap::new();
            fields.insert("sequence".to_string(), normalize_u64(envelope.sequence()));
            Ok(normalized_ok(subject, fields))
        }
        Err(_) => Err(classification_conflict(case, subject, "")),
    }
}

/// Executes one `take-crafting-output` case, the one command in this family
/// whose sequence may not be zero because it takes part in acknowledgement.
fn execute_take_crafting_output(
    case: &FrozenCase,
    input: &JsonMap,
) -> Result<serde_json::Value, DispatchError> {
    let sequence = required_u64(case, input, "sequence")?;

    let rejection = if sequence == 0 {
        Some(Rejection {
            category: "invalid-value",
            rule: "take_crafting_output.zero_sequence",
            error: Some(DomainError::InvalidSequence),
        })
    } else {
        None
    };

    match rejection {
        Some(rejection) => match wrap(sequence, Command::TakeCraftingOutput) {
            Err(error) if Some(error) == rejection.error => {
                let mut fields = JsonMap::new();
                fields.insert("sequence".to_string(), normalize_u64(sequence));
                normalized_error(case, rejection.category, rejection.rule, fields)
            }
            _ => Err(classification_conflict(
                case,
                "take-crafting-output",
                rejection.rule,
            )),
        },
        None => match wrap(sequence, Command::TakeCraftingOutput) {
            Ok(envelope) => {
                let mut fields = JsonMap::new();
                fields.insert("sequence".to_string(), normalize_u64(envelope.sequence()));
                Ok(normalized_ok("take-crafting-output", fields))
            }
            Err(_) => Err(classification_conflict(case, "take-crafting-output", "")),
        },
    }
}

/// Executes one `move-partial` case.
///
/// The shared view and reference bounds apply, plus the distinct-slot rule.
/// Neither the crafting both-ends rule nor the furnace output rule applies,
/// because the protocol publishes neither for this command.
fn execute_move_partial(
    case: &FrozenCase,
    input: &JsonMap,
) -> Result<serde_json::Value, DispatchError> {
    let sequence = required_u64(case, input, "sequence")?;
    let view = required_u8(case, input, "view")?;
    let raw = parse_ref(case, input)?;
    let from = required_u8(case, input, "from")?;
    let to = required_u8(case, input, "to")?;
    let single = required_bool(case, input, "single")?;

    let shape = split_shape(view, &raw);
    let classified = match &shape {
        Err(rejection) => Some(rejection.clone()),
        Ok(shape) => partial_move_rule(shape, from, to),
    };

    match classified {
        Some(rejection) => {
            verify_rejection(case, "move-partial", &rejection, || {
                shape.as_ref().map_or(Ok(()), |shape| {
                    stack_view_checked(shape)
                        .and_then(|view| PartialMove::try_new(view, from, to, single).map(|_| ()))
                })
            })?;
            let mut fields = split_projection(view, &raw);
            fields.insert("sequence".to_string(), normalize_u64(sequence));
            fields.insert("from".to_string(), Value::from(from));
            fields.insert("to".to_string(), Value::from(to));
            fields.insert("single".to_string(), Value::Bool(single));
            normalized_error(case, rejection.category, rejection.rule, fields)
        }
        None => {
            let shape = shape.map_err(|_| classification_conflict(case, "move-partial", ""))?;
            let stack_view = stack_view_of(&shape)
                .ok_or_else(|| classification_conflict(case, "move-partial", ""))?;
            match PartialMove::try_new(stack_view, from, to, single) {
                Ok(partial) => {
                    let envelope = wrap(sequence, Command::MovePartial(partial))
                        .map_err(|_| classification_conflict(case, "move-partial", ""))?;
                    let mut fields = JsonMap::new();
                    fields.insert("sequence".to_string(), normalize_u64(envelope.sequence()));
                    fields.insert("from".to_string(), Value::from(partial.from()));
                    fields.insert("to".to_string(), Value::from(partial.to()));
                    fields.insert("single".to_string(), Value::Bool(partial.single()));
                    push_view_fields(&mut fields, view, partial.view());
                    Ok(normalized_ok("move-partial", fields))
                }
                Err(_) => Err(classification_conflict(case, "move-partial", "")),
            }
        }
    }
}

/// The two whole-stack commands that address one source slot and let the
/// authority derive the destination. They share the partial view and
/// reference bounds and carry no same-slot rule, because there is no target.
#[derive(Clone, Copy)]
enum SourceRule {
    QuickMove,
    DropStack,
}

impl SourceRule {
    /// The normalized category, which is also the input key's rule name.
    fn category(self) -> &'static str {
        match self {
            SourceRule::QuickMove => "quick-move",
            SourceRule::DropStack => "drop-stack",
        }
    }

    /// The input key the source slot is named by. The quick-move case names
    /// it `from` on the wire, while the normalized record always calls it
    /// `slot`.
    fn input_key(self) -> &'static str {
        match self {
            SourceRule::QuickMove => "from",
            SourceRule::DropStack => "slot",
        }
    }

    fn command(self, source: StackSource) -> Command {
        match self {
            SourceRule::QuickMove => Command::QuickMove(source),
            SourceRule::DropStack => Command::DropStack(source),
        }
    }
}

/// Executes one `quick-move` or `drop-stack` case.
fn execute_source_only(
    case: &FrozenCase,
    input: &JsonMap,
    rule: SourceRule,
) -> Result<serde_json::Value, DispatchError> {
    let sequence = required_u64(case, input, "sequence")?;
    let view = required_u8(case, input, "view")?;
    let raw = parse_ref(case, input)?;
    let slot = required_u8(case, input, rule.input_key())?;

    let shape = split_shape(view, &raw);
    let classified = match &shape {
        Err(rejection) => Some(rejection.clone()),
        Ok(shape) => split_rule(shape, slot, slot),
    };

    match classified {
        Some(rejection) => {
            verify_rejection(case, rule.category(), &rejection, || {
                shape.as_ref().map_or(Ok(()), |shape| {
                    stack_view_checked(shape)
                        .and_then(|view| StackSource::try_new(view, slot).map(|_| ()))
                })
            })?;
            let mut fields = split_projection(view, &raw);
            fields.insert("sequence".to_string(), normalize_u64(sequence));
            fields.insert("slot".to_string(), Value::from(slot));
            normalized_error(case, rejection.category, rejection.rule, fields)
        }
        None => {
            let shape = shape.map_err(|_| classification_conflict(case, rule.category(), ""))?;
            let stack_view = stack_view_of(&shape)
                .ok_or_else(|| classification_conflict(case, rule.category(), ""))?;
            match StackSource::try_new(stack_view, slot) {
                Ok(source) => {
                    let envelope = wrap(sequence, rule.command(source))
                        .map_err(|_| classification_conflict(case, rule.category(), ""))?;
                    let mut fields = JsonMap::new();
                    fields.insert("sequence".to_string(), normalize_u64(envelope.sequence()));
                    fields.insert("slot".to_string(), Value::from(source.slot()));
                    push_view_fields(&mut fields, view, source.view());
                    Ok(normalized_ok(rule.category(), fields))
                }
                Err(_) => Err(classification_conflict(case, rule.category(), "")),
            }
        }
    }
}

/// Executes one `chat-intent` case.
///
/// The text becomes a `CommandText` and then a `ChatIntent`; it never enters
/// a `Command` variant or a `CommandEnvelope`, because chat travels on its
/// own bounded channel outside the sequenced command stream.
fn execute_chat_intent(
    case: &FrozenCase,
    input: &JsonMap,
) -> Result<serde_json::Value, DispatchError> {
    let text = required_string(case, input, "text")?;

    let rejection = chat_rule(text);
    let constructed = CommandText::try_from_canonical(text.to_string());

    match rejection {
        Some(rejection) => match constructed {
            Err(error) if Some(error) == rejection.error => {
                let mut fields = JsonMap::new();
                fields.insert("text".to_string(), Value::String(text.to_string()));
                normalized_error(case, rejection.category, rejection.rule, fields)
            }
            _ => Err(classification_conflict(case, "chat-intent", rejection.rule)),
        },
        None => match constructed {
            Ok(text_value) => {
                let intent = ChatIntent::try_new(text_value).unwrap();
                let mut fields = JsonMap::new();
                fields.insert(
                    "text".to_string(),
                    Value::String(intent.text().as_str().to_string()),
                );
                Ok(normalized_ok("chat-intent", fields))
            }
            Err(_) => Err(classification_conflict(case, "chat-intent", "")),
        },
    }
}

/// One classified rejection: its normalized category, its exact Go rule name,
/// and the `DomainError` the matching constructor must return.
///
/// `error` is `None` for a classifier-only rejection whose bad value cannot
/// enter any Rust type, so no constructor verdict exists to check.
#[derive(Clone)]
struct Rejection {
    category: &'static str,
    rule: &'static str,
    error: Option<DomainError>,
}

impl Rejection {
    /// A rejection only the classifier can decide, because the rejected raw
    /// value has no constructible Rust representation.
    fn classifier(category: &'static str, rule: &'static str) -> Self {
        Self {
            category,
            rule,
            error: None,
        }
    }
}

/// Wraps one payload in an envelope with the synthesized intake metadata.
///
/// The producer names only the sequence, so the tick, session and arrival
/// index are the fixed zeros this adapter records. None of the three reaches
/// the normalized projection.
fn wrap(sequence: u64, command: Command) -> Result<CommandEnvelope, DomainError> {
    CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: 0,
        sequence,
        arrival_index: 0,
        command,
    })
}

/// Fixed player inventory slots, from the Go `core.InventorySlots`.
const INVENTORY_SLOTS: u8 = 36;

/// Fixed crafting grid cells, from the Go `core.CraftingGridSlots`.
const CRAFTING_GRID_SLOTS: u8 = 9;

/// Unified crafting view slots: grid `0..8` plus backpack `9..44`, from the
/// Go `GridCraftingViewSlots`.
const CRAFTING_VIEW_SLOTS: u8 = 45;

/// Unified furnace view slots: inventory `0..35`, input `36`, fuel `37` and
/// output `38`, from the Go `core.FurnaceViewSlots`.
const FURNACE_VIEW_SLOTS: u8 = 39;

/// Unified chest view slots: inventory `0..35` plus chest `36..62`, from the
/// Go `core.ChestViewSlots`.
const CHEST_VIEW_SLOTS: u8 = 63;

/// Furnace output slot, from the Go `core.FurnaceOutputSlot`.
const FURNACE_OUTPUT_SLOT: u8 = 38;

/// Fixed furnace array size of one chunk, from the Go
/// `core.FurnacesPerChunk`.
const FURNACES_PER_CHUNK: u8 = 32;

/// Fixed chest array size of one chunk, from the Go `core.ChestsPerChunk`.
const CHESTS_PER_CHUNK: u8 = 16;

/// Maximum UTF-8 byte length of chat command text, from the Go
/// `companion.MaxPlanCommandBytes` bound the protocol shares.
const CHAT_TEXT_MAX_BYTES: usize = 1024;

/// The parsed container-reference shape one case names.
#[derive(Clone)]
enum RawRef {
    /// No `kind` field: the case names no reference at all. On the wire this
    /// is the all-zero reference, which normalizes as the furnace sentinel.
    Absent,
    /// A reference whose kind text is one of the two published kinds.
    Known {
        kind: ContainerKind,
        chunk: ChunkPos,
        slot: u8,
        generation: u32,
    },
    /// A reference whose kind text is neither published kind, so no Rust
    /// value can represent it and only the classifier decides.
    UnknownKind {
        chunk: ChunkPos,
        slot: u8,
        generation: u32,
    },
}

impl RawRef {
    /// Reports whether the parsed reference equals the all-zero wire form,
    /// which a non-container view has to carry and which the Go classifier
    /// therefore treats as carrying no container at all.
    fn is_zero(&self) -> bool {
        match self {
            RawRef::Absent => true,
            RawRef::Known {
                kind,
                chunk,
                slot,
                generation,
            } => {
                *kind == ContainerKind::Furnace
                    && chunk.x() == 0
                    && chunk.z() == 0
                    && *slot == 0
                    && *generation == 0
            }
            RawRef::UnknownKind { .. } => false,
        }
    }
}

/// Parses the raw container-reference fields one case names.
///
/// A reference is absent only when `kind` is absent; a present `kind`
/// requires both chunk coordinates, the array slot and the generation, and
/// their absence is a hard schema failure rather than a normalized one.
fn parse_ref(case: &FrozenCase, input: &JsonMap) -> Result<RawRef, DispatchError> {
    let kind = optional_string(case, input, "kind")?;
    let Some(kind) = kind else {
        return Ok(RawRef::Absent);
    };
    let chunk_x = required_i32(case, input, "chunk_x")?;
    let chunk_z = required_i32(case, input, "chunk_z")?;
    let slot = required_u8(case, input, "container_slot")?;
    let generation = required_u32(case, input, "generation")?;
    let chunk = ChunkPos::new(chunk_x, chunk_z);
    match kind {
        "furnace" => Ok(RawRef::Known {
            kind: ContainerKind::Furnace,
            chunk,
            slot,
            generation,
        }),
        "chest" => Ok(RawRef::Known {
            kind: ContainerKind::Chest,
            chunk,
            slot,
            generation,
        }),
        _ => Ok(RawRef::UnknownKind {
            chunk,
            slot,
            generation,
        }),
    }
}

/// The constructible reference fields behind a parsed shape.
struct RefParts {
    kind: ContainerKind,
    chunk: ChunkPos,
    slot: u8,
    generation: u32,
}

/// Resolves the constructible reference fields, or the classifier-only kind
/// rejection.
///
/// A case that names no reference at all carries the all-zero wire form,
/// which is a furnace reference with a zero generation and is rejected by
/// the reference rule before any slot is consulted.
fn ref_parts(raw: &RawRef) -> Result<RefParts, Rejection> {
    match raw {
        RawRef::Absent => Ok(RefParts {
            kind: ContainerKind::Furnace,
            chunk: ChunkPos::new(0, 0),
            slot: 0,
            generation: 0,
        }),
        RawRef::UnknownKind { .. } => {
            Err(Rejection::classifier("invalid-enum", "container_ref.kind"))
        }
        RawRef::Known {
            kind,
            chunk,
            slot,
            generation,
        } => Ok(RefParts {
            kind: *kind,
            chunk: *chunk,
            slot: *slot,
            generation: *generation,
        }),
    }
}

/// Selects the first rule a container reference breaks: the array slot has
/// to fit the kind's fixed per-chunk array and the generation may not be
/// zero. The dimension arm of the Go rule is unreachable here because the
/// input carries no dimension.
fn ref_rule(parts: &RefParts) -> Option<Rejection> {
    let bound = match parts.kind {
        ContainerKind::Furnace => FURNACES_PER_CHUNK,
        ContainerKind::Chest => CHESTS_PER_CHUNK,
    };
    if parts.slot >= bound {
        return Some(Rejection {
            category: "invalid-value",
            rule: "container_ref.slot_range",
            error: Some(DomainError::InvalidContainerSlot),
        });
    }
    if parts.generation == 0 {
        return Some(Rejection {
            category: "invalid-value",
            rule: "container_ref.generation",
            error: Some(DomainError::InvalidContainerGeneration),
        });
    }
    None
}

/// Selects the first rule an inventory move breaks, in the same order the
/// domain checks them: the slot range, then the distinct-slot rule.
fn move_inventory_rule(from: u8, to: u8) -> Option<Rejection> {
    if from >= INVENTORY_SLOTS || to >= INVENTORY_SLOTS {
        return Some(Rejection {
            category: "invalid-value",
            rule: "move_inventory.slot_range",
            error: Some(DomainError::InvalidSlot),
        });
    }
    if from == to {
        return Some(Rejection {
            category: "invalid-value",
            rule: "move_inventory.same_slot",
            error: Some(DomainError::SourceEqualsTarget),
        });
    }
    None
}

/// Selects the first rule a crafting move breaks. The both-ends-in-inventory
/// rule is the one the partial move does not publish, so it is pinned here
/// and only here.
fn move_crafting_rule(from: u8, to: u8) -> Option<Rejection> {
    if from >= CRAFTING_VIEW_SLOTS || to >= CRAFTING_VIEW_SLOTS {
        return Some(Rejection {
            category: "invalid-value",
            rule: "move_crafting.slot_range",
            error: Some(DomainError::InvalidSlot),
        });
    }
    if from == to {
        return Some(Rejection {
            category: "invalid-value",
            rule: "move_crafting.same_slot",
            error: Some(DomainError::SourceEqualsTarget),
        });
    }
    if from >= CRAFTING_GRID_SLOTS && to >= CRAFTING_GRID_SLOTS {
        return Some(Rejection {
            category: "invalid-value",
            rule: "move_crafting.both_ends_in_inventory",
            error: Some(DomainError::CraftingMoveInsideInventory),
        });
    }
    None
}

/// Selects the first rule a container move breaks. The reference is checked
/// first, exactly as the domain does, so a malformed reference is never
/// reported as a slot problem.
fn container_move_rule(parts: &RefParts, from: u8, to: u8) -> Option<Rejection> {
    if let Some(rejection) = ref_rule(parts) {
        return Some(rejection);
    }
    if from == to {
        return Some(Rejection {
            category: "invalid-value",
            rule: "move_container.same_slot",
            error: Some(DomainError::SourceEqualsTarget),
        });
    }
    if from >= view_bound(parts.kind) || to >= view_bound(parts.kind) {
        return Some(Rejection {
            category: "invalid-value",
            rule: "move_container.slot_range",
            error: Some(DomainError::InvalidSlot),
        });
    }
    if parts.kind == ContainerKind::Furnace && to == FURNACE_OUTPUT_SLOT {
        return Some(Rejection {
            category: "invalid-value",
            rule: "move_container.furnace_output_target",
            error: Some(DomainError::FurnaceOutputAsTarget),
        });
    }
    None
}

/// The unified view bound a container kind addresses.
fn view_bound(kind: ContainerKind) -> u8 {
    match kind {
        ContainerKind::Furnace => FURNACE_VIEW_SLOTS,
        ContainerKind::Chest => CHEST_VIEW_SLOTS,
    }
}

/// The constructible view shape behind a raw view number and reference.
enum SplitShape {
    Inventory,
    Crafting,
    Container(RefParts),
}

/// Classifies the raw view/reference shape in the Go split precedence.
///
/// `Err` carries the pre-construction rejections: an unknown view number, a
/// non-container view that carries a reference, or a container view whose
/// reference names an unknown kind. Those are classifier-only because no
/// Rust value can represent the rejected shape.
fn split_shape(view: u8, raw: &RawRef) -> Result<SplitShape, Rejection> {
    match view {
        0 => {
            if raw.is_zero() {
                Ok(SplitShape::Inventory)
            } else {
                Err(Rejection::classifier(
                    "invalid-value",
                    "stack_split.inventory_view_carries_container",
                ))
            }
        }
        1 => {
            if raw.is_zero() {
                Ok(SplitShape::Crafting)
            } else {
                Err(Rejection::classifier(
                    "invalid-value",
                    "stack_split.crafting_view_carries_container",
                ))
            }
        }
        2 => match raw {
            RawRef::Absent => Ok(SplitShape::Container(RefParts {
                kind: ContainerKind::Furnace,
                chunk: ChunkPos::new(0, 0),
                slot: 0,
                generation: 0,
            })),
            RawRef::UnknownKind { .. } => {
                Err(Rejection::classifier("invalid-enum", "container_ref.kind"))
            }
            RawRef::Known {
                kind,
                chunk,
                slot,
                generation,
            } => Ok(SplitShape::Container(RefParts {
                kind: *kind,
                chunk: *chunk,
                slot: *slot,
                generation: *generation,
            })),
        },
        _ => Err(Rejection::classifier("invalid-enum", "stack_split.view")),
    }
}

/// Selects the first rule one split command breaks.
///
/// It is the shared static bound of the partial, quick-move and drop-stack
/// commands: the view domain, the reference that has to match it, and the
/// index bound the view dispatches. It deliberately omits both stricter
/// rules, because the protocol does not publish them for these commands.
fn split_rule(shape: &SplitShape, from: u8, to: u8) -> Option<Rejection> {
    match shape {
        SplitShape::Inventory => split_bound_rule(from, to, INVENTORY_SLOTS),
        SplitShape::Crafting => split_bound_rule(from, to, CRAFTING_VIEW_SLOTS),
        SplitShape::Container(parts) => {
            if let Some(rejection) = ref_rule(parts) {
                return Some(rejection);
            }
            split_bound_rule(from, to, view_bound(parts.kind))
        }
    }
}

/// The shared index bound one split rule checks.
fn split_bound_rule(from: u8, to: u8, bound: u8) -> Option<Rejection> {
    if from >= bound || to >= bound {
        return Some(Rejection {
            category: "invalid-value",
            rule: "stack_split.slot_range",
            error: Some(DomainError::InvalidSlot),
        });
    }
    None
}

/// Names the first rule a partial move breaks: the shared split bound plus
/// the distinct-slot rule the partial command adds on top of it.
fn partial_move_rule(shape: &SplitShape, from: u8, to: u8) -> Option<Rejection> {
    if let Some(rejection) = split_rule(shape, from, to) {
        return Some(rejection);
    }
    if from == to {
        Some(Rejection {
            category: "invalid-value",
            rule: "move_stack_partial.same_slot",
            error: Some(DomainError::SourceEqualsTarget),
        })
    } else {
        None
    }
}

/// Builds the constructible `StackView` behind one classified shape, so the
/// domain constructor verdict can be checked against the classifier.
fn stack_view_checked(shape: &SplitShape) -> Result<StackView, DomainError> {
    match shape {
        SplitShape::Inventory => Ok(StackView::Inventory),
        SplitShape::Crafting => Ok(StackView::Crafting),
        SplitShape::Container(parts) => {
            ContainerRef::try_new(parts.chunk, parts.kind, parts.slot, parts.generation)
                .map(StackView::Container)
        }
    }
}

/// The constructible `StackView` behind one classified shape, discarding the
/// constructor error for the accepted path where the classifier already
/// proved the shape admits construction.
fn stack_view_of(shape: &SplitShape) -> Option<StackView> {
    stack_view_checked(shape).ok()
}

/// Selects the first rule the chat command text breaks, in the same order
/// the protocol validator checks them: the byte range and the surrounding
/// whitespace, then the control characters. Invalid UTF-8 cannot occur in a
/// Rust `String` or in frozen JSON, so the byte range and the whitespace
/// share the bounds rule exactly as the Go classifier publishes them.
fn chat_rule(text: &str) -> Option<Rejection> {
    let bounds_broken = text.is_empty()
        || text.len() > CHAT_TEXT_MAX_BYTES
        || text.chars().next().is_some_and(is_pinned_whitespace)
        || text.chars().next_back().is_some_and(is_pinned_whitespace);
    if bounds_broken {
        return Some(Rejection {
            category: "invalid-value",
            rule: "chat_command.text_bounds",
            error: Some(DomainError::InvalidText),
        });
    }
    if text.chars().any(is_pinned_control) {
        return Some(Rejection {
            category: "invalid-value",
            rule: "chat_command.control_character",
            error: Some(DomainError::InvalidText),
        });
    }
    None
}

/// Reports whether `ch` is Unicode whitespace, pinned to the same explicit
/// ranges the domain's text rule uses, so the classifier cannot drift with a
/// Unicode table update.
fn is_pinned_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{0085}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

/// Reports whether `ch` is a Unicode control character, pinned to the same
/// two ranges the domain's text rule rejects.
fn is_pinned_control(ch: char) -> bool {
    matches!(ch, '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}')
}

/// Verifies the constructor agreement for one classified rejection.
///
/// When the rejection carries a mapped `DomainError`, the constructed
/// constructor chain must fail with exactly that error; a disagreement is a
/// contract conflict reported as `InvalidCase`. A classifier-only rejection
/// has no constructible Rust value, so there is nothing to check.
fn verify_rejection(
    case: &FrozenCase,
    subject: &str,
    rejection: &Rejection,
    constructed: impl FnOnce() -> Result<(), DomainError>,
) -> Result<(), DispatchError> {
    let Some(expected) = rejection.error else {
        return Ok(());
    };
    match constructed() {
        Err(error) if error == expected => Ok(()),
        _ => Err(classification_conflict(case, subject, rejection.rule)),
    }
}

/// Builds the rejected two-slot move projection from the parsed input,
/// because a rejected Rust value has no getters to read.
fn move_projection(sequence: u64, from: u8, to: u8) -> JsonMap {
    let mut fields = JsonMap::new();
    fields.insert("sequence".to_string(), normalize_u64(sequence));
    fields.insert("from".to_string(), Value::from(from));
    fields.insert("to".to_string(), Value::from(to));
    fields
}

/// Builds the rejected view-bearing projection's common fields: the view
/// number and the sentinel or raw reference the case names.
fn split_projection(view: u8, raw: &RawRef) -> JsonMap {
    let mut fields = raw_ref_fields(raw);
    fields.insert("view".to_string(), Value::from(view));
    fields
}

/// Renders one parsed reference as its raw wire fields.
///
/// An absent or all-zero reference records the furnace sentinel form, which
/// is what the wire encoder writes for a non-container view; an unknown kind
/// records the stable `unknown` text the normalized vocabulary publishes.
/// The dimension is always the overworld zero because the reference shape is
/// inherently overworld and the input carries no dimension field.
fn raw_ref_fields(raw: &RawRef) -> JsonMap {
    let (chunk_x, chunk_z, slot, generation, kind_text) = match raw {
        RawRef::Absent => (0, 0, 0, 0, "furnace"),
        RawRef::Known {
            kind,
            chunk,
            slot,
            generation,
        } => (chunk.x(), chunk.z(), *slot, *generation, kind_text(*kind)),
        RawRef::UnknownKind {
            chunk,
            slot,
            generation,
        } => (chunk.x(), chunk.z(), *slot, *generation, "unknown"),
    };
    let mut fields = JsonMap::new();
    fields.insert("chunk_x".to_string(), Value::from(chunk_x));
    fields.insert("chunk_z".to_string(), Value::from(chunk_z));
    fields.insert("dimension".to_string(), Value::from(0));
    fields.insert("kind".to_string(), Value::from(kind_text));
    fields.insert("container_slot".to_string(), Value::from(slot));
    fields.insert("generation".to_string(), Value::from(generation));
    fields
}

/// Renders one constructed view into the accepted projection: the view
/// number beside the sentinel for a non-container view or the constructed
/// reference's getters for a container view.
fn push_view_fields(fields: &mut JsonMap, view_raw: u8, view: StackView) {
    fields.insert("view".to_string(), Value::from(view_raw));
    match view.container() {
        None => {
            fields.insert("chunk_x".to_string(), Value::from(0));
            fields.insert("chunk_z".to_string(), Value::from(0));
            fields.insert("dimension".to_string(), Value::from(0));
            fields.insert("kind".to_string(), Value::from("furnace"));
            fields.insert("container_slot".to_string(), Value::from(0));
            fields.insert("generation".to_string(), Value::from(0));
        }
        Some(container) => push_container_fields(fields, &container),
    }
}

/// Renders one constructed container reference into the accepted projection.
fn push_container_fields(fields: &mut JsonMap, container: &ContainerRef) {
    let chunk = container.chunk();
    fields.insert("chunk_x".to_string(), Value::from(chunk.x()));
    fields.insert("chunk_z".to_string(), Value::from(chunk.z()));
    fields.insert("dimension".to_string(), Value::from(0));
    fields.insert("kind".to_string(), Value::from(kind_text(container.kind())));
    fields.insert("container_slot".to_string(), Value::from(container.slot()));
    fields.insert(
        "generation".to_string(),
        Value::from(container.generation()),
    );
}

/// Renders one container kind as the stable text the normalized corpus
/// vocabulary uses.
fn kind_text(kind: ContainerKind) -> &'static str {
    match kind {
        ContainerKind::Furnace => "furnace",
        ContainerKind::Chest => "chest",
    }
}

/// Fails closed when the Go-rule classification and the Rust constructor
/// disagree about one case input.
///
/// A disagreement is a contract conflict rather than a corpus expectation: it
/// means one side's rule moved, so reporting `InvalidCase` stops the topic
/// instead of publishing whichever side happened to be consulted first.
fn classification_conflict(case: &FrozenCase, subject: &str, rule: &str) -> DispatchError {
    let classified = if rule.is_empty() { "accept" } else { rule };
    invalid_case(
        case,
        format!("{subject} classification '{classified}' disagrees with the Rust constructor"),
    )
}

#[test]
fn command_inventory_execute_54_cases() {
    let executed = execute_topic(EXPECTED_COUNT, owns, execute).expect("execute topic");
    for case in &executed {
        assert_domain_normalized(case);
    }
}

/// Proves the shared comparator actually compares semantic fields: the
/// produced accepted result matches its frozen expectation, while the same
/// actual value no longer matches once one normalized field is mutated.
#[test]
fn command_inventory_comparator_detects_semantic_mutation() {
    let executed = execute_topic(EXPECTED_COUNT, owns, execute).expect("execute topic");
    let accepted = executed
        .into_iter()
        .find(|case| {
            case.actual.get("kind").and_then(|kind| kind.as_str()) == Some("ok")
                && case
                    .actual
                    .get("category")
                    .and_then(|category| category.as_str())
                    == Some("move-inventory")
        })
        .expect("one accepted move-inventory case");

    assert_domain_normalized(&accepted);

    let mut mutated_case = accepted.case;
    let fields = mutated_case
        .normalized
        .get_mut("fields")
        .and_then(|fields| fields.as_object_mut())
        .expect("normalized fields object");
    fields.insert("from".to_string(), Value::from(999));

    let mutated = ExecutedCase {
        case: mutated_case,
        actual: accepted.actual,
    };
    let outcome = std::panic::catch_unwind(|| assert_domain_normalized(&mutated));
    assert!(
        outcome.is_err(),
        "a semantic mutation must fail the normalized comparison"
    );
}
