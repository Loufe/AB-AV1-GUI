//! The official ab-av1 NDJSON search contract. Human output is diagnostic only.

use super::{JobFailure, SearchOutcome};
use serde::Deserialize;

const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Message {
    #[serde(rename = "sample-encode-done")]
    Trial(ResultFields),
    #[serde(rename = "crf-search-done")]
    Done(ResultFields),
    /// ab-av1 emits this only when no CRF satisfies the target, after at least
    /// one attempt; every other failure exits non-zero without a terminal
    /// message. Its fixed human text is never retained.
    #[serde(rename = "crf-search-error")]
    NoGoodCrf {},
}

#[derive(Deserialize)]
struct ResultFields {
    crf: f32,
    vmaf: f32,
    from_cache: bool,
    predicted_encode_size: u64,
    predicted_encode_percent: f64,
    predicted_encode_seconds: f64,
}

impl ResultFields {
    fn outcome(self) -> Result<SearchOutcome, String> {
        if !self.crf.is_finite()
            || !(0.0..=70.0).contains(&self.crf)
            || !self.vmaf.is_finite()
            || !(0.0..=100.0).contains(&self.vmaf)
            || !self.predicted_encode_percent.is_finite()
            || self.predicted_encode_percent < 0.0
        {
            return Err("ab-av1 returned invalid quality-search values".into());
        }
        let predicted_duration =
            std::time::Duration::try_from_secs_f64(self.predicted_encode_seconds)
                .map_err(|_| "ab-av1 returned an invalid encode duration")?;
        if predicted_duration.as_millis() > u128::from(u64::MAX) {
            return Err("ab-av1 encode duration is too large to persist".into());
        }
        Ok(SearchOutcome {
            crf: self.crf,
            vmaf: self.vmaf,
            from_cache: self.from_cache,
            predicted_size: self.predicted_encode_size,
            predicted_percent: self.predicted_encode_percent,
            predicted_duration,
        })
    }
}

#[derive(Default)]
pub(super) struct SearchProtocol {
    pending: Vec<u8>,
    pub last: Option<SearchOutcome>,
    pub terminal: Option<Result<SearchOutcome, JobFailure>>,
    pub trials: usize,
}

impl SearchProtocol {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        for &byte in bytes {
            if byte == b'\n' {
                self.line()?;
            } else if self.pending.len() < MAX_LINE_BYTES {
                self.pending.push(byte);
            } else {
                return Err("ab-av1 JSON line exceeds the protocol limit".into());
            }
        }
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<(), String> {
        if !self.pending.is_empty() {
            self.line()?;
        }
        Ok(())
    }

    fn line(&mut self) -> Result<(), String> {
        if self.terminal.is_some() {
            return Err("ab-av1 wrote a message after its terminal result".into());
        }
        let message: Message = serde_json::from_slice(&self.pending)
            .map_err(|error| format!("invalid ab-av1 JSON message: {error}"))?;
        self.pending.clear();
        match message {
            Message::Trial(fields) => {
                self.last = Some(fields.outcome()?);
                self.trials = self.trials.saturating_add(1);
            }
            Message::Done(fields) => self.terminal = Some(Ok(fields.outcome()?)),
            Message::NoGoodCrf {} => {
                let last = self
                    .last
                    .clone()
                    .ok_or("ab-av1 reported no suitable CRF before any attempt")?;
                self.terminal = Some(Err(JobFailure::no_good_crf(last)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIAL: &[u8] = br#"{"type":"sample-encode-done","crf":34.25,"vmaf":95.5,"from_cache":false,"predicted_encode_size":123,"predicted_encode_percent":41.25,"predicted_encode_seconds":1.5}"#;

    #[test]
    fn fragmented_trial_then_final_result_preserves_fractional_values() -> Result<(), String> {
        let mut protocol = SearchProtocol::default();
        for byte in TRIAL {
            protocol.push(&[*byte])?;
        }
        protocol.push(b"\n")?;
        let done = String::from_utf8_lossy(TRIAL).replace("sample-encode-done", "crf-search-done");
        protocol.push(done.as_bytes())?;
        protocol.finish()?;
        assert_eq!(protocol.trials, 1);
        let outcome = protocol
            .terminal
            .clone()
            .ok_or("missing terminal")?
            .map_err(|e| e.message)?;
        assert_eq!(outcome.crf, 34.25);
        assert_eq!(outcome.predicted_duration.as_millis(), 1500);
        assert!(protocol.push(b"{}\n").is_err());
        Ok(())
    }

    #[test]
    fn rejection_retains_last_attempt_and_oversized_lines_fail() -> Result<(), String> {
        let mut protocol = SearchProtocol::default();
        protocol.push(TRIAL)?;
        protocol.push(b"\n{\"type\":\"crf-search-error\",\"message\":\"no good CRF\"}\n")?;
        assert!(matches!(
            protocol.terminal,
            Some(Err(JobFailure {
                kind: super::super::JobFailureKind::NoGoodCrf { .. },
                ..
            }))
        ));
        assert!(
            SearchProtocol::default()
                .push(&vec![b'x'; MAX_LINE_BYTES + 1])
                .is_err()
        );
        assert!(SearchProtocol::default().push(b"human output\n").is_err());
        assert!(
            SearchProtocol::default()
                .push(b"{\"type\":\"crf-search-error\",\"message\":\"/videos/private.mkv\"}\n")
                .is_err()
        );
        Ok(())
    }
}
