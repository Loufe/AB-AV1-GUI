use serde::{Deserialize, Serialize};

use crate::{DurableDelta, DurableState, JournalSequence, fold};

pub const JOURNAL_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEnvelope {
    pub schema_version: u32,
    pub sequence: JournalSequence,
    pub delta: DurableDelta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalCorruption {
    pub offset: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalReplay {
    pub state: DurableState,
    pub next_sequence: JournalSequence,
    pub corruption: Option<JournalCorruption>,
    pub ignored_torn_tail: bool,
}

pub fn encode_record(envelope: &JournalEnvelope) -> Result<Vec<u8>, serde_json::Error> {
    let mut encoded = serde_json::to_vec(envelope)?;
    encoded.push(b'\n');
    Ok(encoded)
}

#[must_use]
pub fn replay(bytes: &[u8]) -> JournalReplay {
    let mut state = DurableState::default();
    let mut expected = 0_u64;
    let mut offset = 0_usize;
    let mut corruption = None;
    let mut ignored_torn_tail = false;

    for segment in bytes.split_inclusive(|byte| *byte == b'\n') {
        let complete = segment.last() == Some(&b'\n');
        if !complete {
            ignored_torn_tail = true;
            break;
        }
        let line = segment.strip_suffix(b"\n").unwrap_or(segment);
        if line.is_empty() {
            if complete {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: "empty journal record".to_owned(),
                });
            } else {
                ignored_torn_tail = true;
            }
            break;
        }
        let envelope = match serde_json::from_slice::<JournalEnvelope>(line) {
            Ok(envelope) => envelope,
            Err(error) => {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: format!("invalid journal record: {error}"),
                });
                break;
            }
        };
        if envelope.schema_version != JOURNAL_SCHEMA_VERSION {
            corruption = Some(JournalCorruption {
                offset,
                reason: format!("unsupported journal schema {}", envelope.schema_version),
            });
            break;
        }
        if envelope.sequence != JournalSequence(expected) {
            corruption = Some(JournalCorruption {
                offset,
                reason: format!(
                    "expected journal sequence {expected}, found {}",
                    envelope.sequence.0
                ),
            });
            break;
        }
        fold(&mut state, &envelope.delta);
        expected = match expected.checked_add(1) {
            Some(next) => next,
            None => {
                corruption = Some(JournalCorruption {
                    offset,
                    reason: "journal sequence overflow".to_owned(),
                });
                break;
            }
        };
        offset += segment.len();
    }

    JournalReplay {
        state,
        next_sequence: JournalSequence(expected),
        corruption,
        ignored_torn_tail,
    }
}
