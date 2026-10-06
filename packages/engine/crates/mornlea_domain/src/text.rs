//! Owned canonical text values shared by the semantic records.
//!
//! Each wrapper owns its `String` and keeps the field private, so a text value
//! can only enter the crate through a checked constructor. The rules pin the
//! Go authority behavior with explicit character ranges instead of a Unicode
//! table crate: the whitespace and control sets are the ones the Go baseline
//! accepts and rejects today, and pinning the ranges keeps a language
//! Unicode-version bump from silently widening or narrowing the domain.

use crate::identity::DomainError;

/// Maximum scalar count of a display name, from the Go
/// `core.NormalizeDisplayName` length rule.
const DISPLAY_NAME_MAX_SCALARS: usize = 32;

/// Maximum UTF-8 byte length of a display name, from the Go
/// `core.NormalizeDisplayName` length rule.
const DISPLAY_NAME_MAX_BYTES: usize = 128;

/// Maximum UTF-8 byte length of a player command text.
///
/// The bound is the Go `companion.MaxPlanCommandBytes` snapshot instruction
/// limit, which the network chat command shares so a command that reaches the
/// planner always fits the wire slot.
const COMMAND_TEXT_MAX_BYTES: usize = 1024;

/// Maximum UTF-8 byte length of a companion speech text.
///
/// The bound is the Go `companion.MaxDialogueLineBytes` single dialogue line
/// limit, which is tighter than the command bound because speech is a bounded
/// model-generated expression rather than a player input channel.
const SPEECH_TEXT_MAX_BYTES: usize = 256;

/// Reports whether `ch` is Unicode whitespace, pinned to the set the Go
/// `unicode.IsSpace` rule trims: U+0009..000D, 0020, 0085, 00A0, 1680,
/// 2000..200A, 2028, 2029, 202F, 205F and 3000. The ranges are written out
/// rather than delegated so the domain behavior is frozen against Unicode
/// table drift.
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

/// Reports whether `ch` is a Unicode control character, pinned to the two
/// ranges the Go `unicode.IsControl` rule rejects: U+0000..001F and
/// U+007F..009F. Format characters outside those ranges stay accepted, which
/// is what the Go baseline does.
fn is_pinned_control(ch: char) -> bool {
    matches!(ch, '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}')
}

/// Reports whether the text is already in canonical form: no leading or
/// trailing whitespace, because the Go normalizers trim before they publish.
fn has_surrounding_whitespace(text: &str) -> bool {
    text.chars().next().is_some_and(is_pinned_whitespace)
        || text.chars().next_back().is_some_and(is_pinned_whitespace)
}

/// Trims the pinned whitespace set from both ends and borrows the remainder.
///
/// The Go admission path trims a raw name before it validates the result, so
/// the trim set and the validation set have to be the one this crate already
/// pins. Delegating to `str::trim` instead would read the standard library's
/// Unicode tables, whose contents can move with a Unicode-version bump, so
/// the trim runs over the same explicit ranges as `is_pinned_whitespace` and
/// returns a borrow of the caller's text. Callers that then construct a
/// canonical value feed the trimmed slice back through the same crate, so
/// one lexical rule decides both steps.
pub fn trim_pinned_whitespace(text: &str) -> &str {
    text.trim_matches(is_pinned_whitespace)
}

/// Applies the Go display-name admission rule to a canonical text: 1..=32
/// scalars, at most 128 bytes, no surrounding whitespace, and no control
/// character. Normalization is deliberately not performed here; the caller
/// trims first and this rule decides whether the result is publishable.
///
/// The scalar inspection is routed through the private visitor seam so a test
/// can observe how many scalars a rejection actually inspects. `on_scalar` is
/// invoked once per inspected scalar; the admitted set is unchanged.
fn is_canonical_display_name(text: &str) -> bool {
    is_canonical_display_name_with_visit(text, |_| {})
}

/// Visitor variant of the display-name rule: identical admission result, with
/// `on_scalar` invoked once per scalar the rule inspects.
///
/// The empty and byte checks run before any `chars()` iterator is built, so an
/// oversized input costs one length compare instead of a scan; the scalar
/// count, surrounding-whitespace and control checks then share one bounded
/// scan because the byte bound caps the input length.
fn is_canonical_display_name_with_visit(text: &str, mut on_scalar: impl FnMut(char)) -> bool {
    if text.is_empty() || text.len() > DISPLAY_NAME_MAX_BYTES {
        return false;
    }
    let mut scalars = 0usize;
    let mut first_scalar = None;
    let mut last_scalar = None;
    let mut has_control = false;
    for ch in text.chars() {
        on_scalar(ch);
        if first_scalar.is_none() {
            first_scalar = Some(ch);
        }
        last_scalar = Some(ch);
        if is_pinned_control(ch) {
            has_control = true;
        }
        scalars += 1;
    }
    let surrounded = first_scalar.is_some_and(is_pinned_whitespace)
        || last_scalar.is_some_and(is_pinned_whitespace);
    (1..=DISPLAY_NAME_MAX_SCALARS).contains(&scalars) && !surrounded && !has_control
}

/// Applies the Go bounded-text admission rule to a canonical text: at least
/// one byte, at most `max_bytes` bytes, no surrounding whitespace, and no
/// control character. The bound comes from the caller so the command and
/// speech slots cannot drift apart.
fn is_canonical_bounded_text(text: &str, max_bytes: usize) -> bool {
    !text.is_empty()
        && text.len() <= max_bytes
        && !has_surrounding_whitespace(text)
        && !text.chars().any(is_pinned_control)
}

/// Canonical player display name.
///
/// The rule is the Go `core.NormalizeDisplayName` result: admission trims the
/// raw name and then hands the trimmed form to `try_from_canonical`, so a name
/// the authority would have to trim is rejected instead of normalized here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisplayName(String);

impl DisplayName {
    pub fn try_from_canonical(canonical: String) -> Result<Self, DomainError> {
        if !is_canonical_display_name(&canonical) {
            return Err(DomainError::InvalidText);
        }
        Ok(Self(canonical))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Canonical companion name.
///
/// The rule is the Go `companion.ValidateName`: the display-name rule plus a
/// rejection of any Unicode whitespace, so a companion name never carries an
/// embedded space while a player display name may.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompanionName(String);

impl CompanionName {
    pub fn try_from_canonical(canonical: String) -> Result<Self, DomainError> {
        if !is_canonical_display_name(&canonical) || canonical.chars().any(is_pinned_whitespace) {
            return Err(DomainError::InvalidText);
        }
        Ok(Self(canonical))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Authoritative command text, bounded by `COMMAND_TEXT_MAX_BYTES`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandText(String);

impl CommandText {
    /// Restores source-valid saved text without trimming away persisted instruction bytes.
    /// Live admission continues to use the separate strict canonical constructor.
    pub fn try_from_persisted(persisted: String) -> Result<Self, DomainError> {
        if persisted.len() > COMMAND_TEXT_MAX_BYTES
            || trim_pinned_whitespace(&persisted).is_empty()
            || persisted.chars().any(is_pinned_control)
        {
            return Err(DomainError::InvalidText);
        }
        Ok(Self(persisted))
    }

    pub fn try_from_canonical(canonical: String) -> Result<Self, DomainError> {
        if !is_canonical_bounded_text(&canonical, COMMAND_TEXT_MAX_BYTES) {
            return Err(DomainError::InvalidText);
        }
        Ok(Self(canonical))
    }

    pub(crate) fn is_canonical(&self) -> bool {
        !has_surrounding_whitespace(&self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Canonical companion speech text, bounded by `SPEECH_TEXT_MAX_BYTES`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeechText(String);

impl SpeechText {
    pub fn try_from_canonical(canonical: String) -> Result<Self, DomainError> {
        if !is_canonical_bounded_text(&canonical, SPEECH_TEXT_MAX_BYTES) {
            return Err(DomainError::InvalidText);
        }
        Ok(Self(canonical))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An oversized display name must be rejected by the byte bound before a
    /// single scalar is inspected, even when the text also carries a control
    /// scalar a later rule would reject. The visitor count is the observable:
    /// zero means the scan never ran.
    #[test]
    fn oversized_display_name_skips_scalar_scan() {
        let oversized = format!("{}a\u{0001}", "a".repeat(128));
        assert!(oversized.len() > DISPLAY_NAME_MAX_BYTES);
        let visited = core::cell::Cell::new(0usize);
        let admitted = is_canonical_display_name_with_visit(&oversized, |ch| {
            visited.set(visited.get() + 1);
            let _ = ch;
        });
        assert!(
            !admitted,
            "an oversized name is not a canonical display name"
        );
        assert_eq!(
            visited.get(),
            0,
            "the byte bound must reject before any scalar is inspected"
        );
    }

    /// The trim helper borrows its input and trims exactly the pinned Go
    /// whitespace set: the whole set at both ends, nothing else. A format
    /// character the Go baseline keeps stays in the returned slice, which is
    /// what keeps the trim from widening with a Unicode-version bump.
    #[test]
    fn trim_pinned_whitespace_trims_only_the_pinned_set() {
        assert_eq!(trim_pinned_whitespace("\u{0009}n\u{000A}"), "n");
        assert_eq!(trim_pinned_whitespace("\u{00A0}n\u{3000}"), "n");
        assert_eq!(trim_pinned_whitespace("\u{200B}n"), "\u{200B}n");
        assert_eq!(trim_pinned_whitespace(""), "");
        assert_eq!(trim_pinned_whitespace("plain"), "plain");
    }
}

#[cfg(test)]
mod persisted_command_tests {
    use super::*;

    #[test]
    fn persisted_command_preserves_source_whitespace_without_relaxing_live_admission() {
        for text in [" work ", "\u{00a0}工作\u{3000}", "\u{200b}"] {
            let saved = CommandText::try_from_persisted(text.to_owned()).unwrap();
            assert_eq!(saved.as_str(), text);
            assert_eq!(
                CommandText::try_from_canonical(text.to_owned()).is_ok(),
                text == "\u{200b}"
            );
        }
    }

    #[test]
    fn persisted_command_keeps_byte_ceiling_and_control_refusals() {
        let text = "x".repeat(COMMAND_TEXT_MAX_BYTES);
        assert_eq!(
            CommandText::try_from_persisted(text.clone())
                .unwrap()
                .as_str(),
            text
        );
        for text in [
            String::new(),
            " \u{00a0}".into(),
            "a\nb".into(),
            "a\u{0085}b".into(),
            "x".repeat(COMMAND_TEXT_MAX_BYTES + 1),
        ] {
            assert!(CommandText::try_from_persisted(text).is_err());
        }
        assert!(CommandText::try_from_persisted("界".repeat(342)).is_err());
    }
}

#[cfg(test)]
mod restored_chat_event_tests {
    use super::*;
    use crate::{
        ChatBody, ChatEvent, ChatEventParts, CompanionId, CompanionSpeaker, PlayerId, TaskState,
    };

    fn parts(body: ChatBody) -> ChatEventParts {
        let mut raw = [0; 16];
        raw[6] = 0x40;
        raw[8] = 0x80;
        ChatEventParts {
            event_id: 1,
            player_id: PlayerId::try_from_bytes(raw).unwrap(),
            player_name: DisplayName::try_from_canonical("Ada".into()).unwrap(),
            body,
        }
    }
    fn companion() -> CompanionSpeaker {
        let mut raw = [0; 16];
        raw[6] = 0x40;
        raw[8] = 0x80;
        CompanionSpeaker::new(
            CompanionId::try_from_bytes(raw).unwrap(),
            CompanionName::try_from_canonical("Nova".into()).unwrap(),
        )
    }
    #[test]
    fn restored_padded_commands_cannot_escape_as_immutable_chat_events() {
        for text in [" work ", "\u{00a0}work\u{3000}"] {
            let command = CommandText::try_from_persisted(text.into()).unwrap();
            let companion = companion();
            for body in [
                ChatBody::Accepted {
                    companion: companion.clone(),
                    command: command.clone(),
                },
                ChatBody::QueueFull {
                    companion: companion.clone(),
                    command: command.clone(),
                },
                ChatBody::NotFollowing {
                    companion: companion.clone(),
                    command: command.clone(),
                },
                ChatBody::Task {
                    companion,
                    command: command.clone(),
                    state: TaskState::Completed,
                },
            ] {
                assert_eq!(
                    ChatEvent::try_new(parts(body)),
                    Err(DomainError::InvalidText)
                );
            }
        }
        let body = ChatBody::Task {
            companion: companion(),
            command: CommandText::try_from_persisted("work".into()).unwrap(),
            state: TaskState::Completed,
        };
        assert!(ChatEvent::try_new(parts(body)).is_ok());
    }
    #[test]
    fn zero_event_identity_precedes_restored_command_refusal() {
        let mut parts = parts(ChatBody::Task {
            companion: companion(),
            command: CommandText::try_from_persisted(" work ".into()).unwrap(),
            state: TaskState::Completed,
        });
        parts.event_id = 0;
        assert_eq!(ChatEvent::try_new(parts), Err(DomainError::InvalidIdentity));
    }
}
