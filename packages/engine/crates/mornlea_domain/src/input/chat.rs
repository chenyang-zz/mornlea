//! Bounded chat intent payload.
//!
//! The chat channel carries the player's own text and nothing else. Parsing a
//! `/warp` namespace, resolving an `@name` companion address, deciding whether
//! a task stops, and queueing the result are authority decisions over the
//! session and the companion set, so none of them belongs to a payload the
//! client constructs. Keeping them out is what lets the text be retained
//! verbatim: a mention that the authority later rejects still has to reach it
//! exactly as typed.

use crate::identity::DomainError;
use crate::text::CommandText;

/// One chat intent: the player's bounded command text, retained verbatim.
///
/// Live chat requires canonical text even when a source-valid saved task
/// instruction is available. Construction checks that context without trimming.
/// Chat carries no sequence and remains independent of sequenced world actions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatIntent {
    text: CommandText,
}

impl ChatIntent {
    pub fn try_new(text: CommandText) -> Result<Self, DomainError> {
        if !text.is_canonical() {
            return Err(DomainError::InvalidText);
        }
        Ok(Self { text })
    }

    pub fn text(&self) -> &CommandText {
        &self.text
    }
}

#[cfg(test)]
mod restored_input_tests {
    use super::*;
    #[test]
    fn stored_task_text_does_not_bypass_canonical_human_chat_admission() {
        let stored = CommandText::try_from_persisted(" work ".into()).unwrap();
        assert_eq!(
            ChatIntent::try_new(stored),
            Err(crate::DomainError::InvalidText)
        );
        let canonical = CommandText::try_from_persisted("work".into()).unwrap();
        assert_eq!(
            ChatIntent::try_new(canonical).unwrap().text().as_str(),
            "work"
        );
    }
}
