use serde::{Deserialize, Serialize};

/// Durable byte bound for diagnostic tails. Enforced by the only constructor
/// and re-enforced during journal replay validation, so a hand-edited journal
/// cannot smuggle an unbounded blob into durable state.
pub const DIAGNOSTIC_TAIL_MAX_BYTES: usize = 4096;

/// Bounded diagnostic text journaled with a failure — typically the tail of a
/// process's stderr. Producers must substitute run paths with placeholders
/// before construction; the durable model never sees a real path here.
///
/// Deliberately no `Display` impl: the tail is payload for inspection surfaces,
/// not prose to be interpolated into messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(transparent)]
pub struct DiagnosticTail(String);

impl DiagnosticTail {
    /// Keeps the trailing bytes of `text` within the durable bound, advancing
    /// the cut to a character boundary so the result stays valid UTF-8.
    #[must_use]
    pub fn truncated(text: &str) -> Self {
        if text.len() <= DIAGNOSTIC_TAIL_MAX_BYTES {
            return Self(text.to_owned());
        }
        let mut start = text.len() - DIAGNOSTIC_TAIL_MAX_BYTES;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        Self(text.get(start..).unwrap_or_default().to_owned())
    }

    #[must_use]
    pub fn empty() -> Self {
        Self(String::new())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Replay-side re-enforcement of the construction bound: serde's
    /// transparent deserialization bypasses [`DiagnosticTail::truncated`].
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.0.len() > DIAGNOSTIC_TAIL_MAX_BYTES {
            return Err("failure diagnostic exceeds the durable byte bound");
        }
        Ok(())
    }
}

/// Which class of work failed. One variant per production failure site class
/// in the engine coordinator; `Internal` covers protocol and channel errors —
/// there is no open-ended escape variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum FailureKind {
    SearchStart,
    SearchRun,
    EncodeStart,
    EncodeRun,
    RemuxStart,
    RemuxRun,
    AdapterPanicked { cleanup_failed: bool },
    OutputPrepare,
    OutputPromote,
    OutputConflict,
    Internal,
}

/// The durable description of a failed run: a stable kind for policy and
/// display grouping, a user-facing message, and a bounded diagnostic tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FailureFacts {
    pub kind: FailureKind,
    pub message: String,
    pub diagnostic: DiagnosticTail,
}

impl FailureFacts {
    #[must_use]
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            diagnostic: DiagnosticTail::empty(),
        }
    }

    #[must_use]
    pub fn with_diagnostic(mut self, diagnostic: DiagnosticTail) -> Self {
        self.diagnostic = diagnostic;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{DIAGNOSTIC_TAIL_MAX_BYTES, DiagnosticTail};

    #[test]
    fn truncation_keeps_the_tail_within_the_bound() {
        let text = "a".repeat(DIAGNOSTIC_TAIL_MAX_BYTES + 10);
        let tail = DiagnosticTail::truncated(&text);
        assert_eq!(tail.as_str().len(), DIAGNOSTIC_TAIL_MAX_BYTES);
        assert!(tail.validate().is_ok());
    }

    #[test]
    fn truncation_lands_on_a_char_boundary() {
        // é is two bytes; an odd prefix forces the cut inside a character.
        let text = format!("x{}", "é".repeat(DIAGNOSTIC_TAIL_MAX_BYTES / 2));
        let tail = DiagnosticTail::truncated(&text);
        assert!(tail.as_str().len() <= DIAGNOSTIC_TAIL_MAX_BYTES);
        assert!(tail.as_str().chars().all(|character| character == 'é'));
    }

    #[test]
    fn truncation_preserves_short_text_verbatim() {
        let tail = DiagnosticTail::truncated("stderr: boom");
        assert_eq!(tail.as_str(), "stderr: boom");
    }

    #[test]
    fn replay_validation_rejects_an_oversized_deserialized_tail() {
        let oversized = format!("\"{}\"", "a".repeat(DIAGNOSTIC_TAIL_MAX_BYTES + 1));
        let tail: DiagnosticTail =
            serde_json::from_str(&oversized).expect("transparent deserialization succeeds");
        assert!(tail.validate().is_err());
    }
}
