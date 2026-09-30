//! Translated observations: History evidence imported from another
//! application's records rather than observed by a native run (ADR-025).
//!
//! A translated observation is identified by its import origin and that
//! origin's own record key, never by a readable path, and claims only what
//! the origin record held. It has no content key, operation, tool
//! revisions, or start instant, and every fact it carries may be absent; an
//! absent fact is never filled in, least of all a missing update instant
//! from the import clock. Scanned records decided nothing and do not
//! translate, and the origin never recorded failed or stopped work.
//!
//! Import is planned here and committed by storage. A batch is validated
//! whole before anything is planned, and planning is deterministic in file
//! order: a new identity inserts, an identity already present with equal
//! facts is a duplicate that writes nothing, and one present with different
//! facts is a conflict in which the present observation stands.

use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Crf, DurationMs, UnixMillis, VideoCodec, VmafScore, VmafTarget};

/// Where translated evidence came from. Identity is the pair of origin and
/// record key, so a later origin cannot collide with this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ImportOrigin {
    V2History,
}

const RECORD_KEY_LENGTH: usize = 16;

/// The origin record's own primary key: for V2 its path hash, exactly 16
/// lowercase hexadecimal characters. Any other shape is refused on
/// construction and on deserialization, so a readable path can never become
/// an identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RecordKey(String);

impl RecordKey {
    pub fn new(key: String) -> Result<Self, &'static str> {
        if key.len() == RECORD_KEY_LENGTH
            && key
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(key))
        } else {
            Err("record key must be 16 lowercase hexadecimal characters")
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RecordKey {
    type Error = &'static str;

    fn try_from(key: String) -> Result<Self, Self::Error> {
        Self::new(key)
    }
}

impl From<RecordKey> for String {
    fn from(key: RecordKey) -> Self {
        key.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranslatedId {
    pub origin: ImportOrigin,
    pub record_key: RecordKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranslatedObservation {
    pub origin: ImportOrigin,
    pub record_key: RecordKey,
    pub source: TranslatedSource,
    pub outcome: TranslatedOutcome,
    /// The origin's last update to the record, which may postdate its
    /// decision because V2 rewrote records on rescans; `None` when the
    /// record did not say. Never the import instant.
    pub updated_at: Option<UnixMillis>,
}

/// The source facts the origin record held. There is no content key: the
/// origin never computed one and a translated observation never gains one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranslatedSource {
    pub codec: Option<VideoCodec>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TranslatedOutcome {
    Analyzed {
        quality: TranslatedQuality,
    },
    Converted {
        quality: TranslatedQuality,
        output_size: Option<u64>,
        encode_duration: Option<DurationMs>,
    },
    NotWorthwhile {
        requested: Option<VmafTarget>,
        floor: Option<VmafTarget>,
    },
}

/// The search result the origin recorded. The origin kept no profile or
/// predictions, so this is display and prior evidence, never a reusable
/// analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranslatedQuality {
    pub crf: Option<Crf>,
    pub score: Option<VmafScore>,
    /// The target the result satisfied.
    pub target: Option<VmafTarget>,
}

impl TranslatedObservation {
    #[must_use]
    pub fn id(&self) -> TranslatedId {
        TranslatedId {
            origin: self.origin,
            record_key: self.record_key.clone(),
        }
    }

    /// Rejects the facts native validation rejects, wherever the fact is
    /// present.
    pub fn validate(&self) -> Result<(), &'static str> {
        match &self.outcome {
            TranslatedOutcome::Analyzed { quality }
            | TranslatedOutcome::Converted { quality, .. } => quality.validate(),
            TranslatedOutcome::NotWorthwhile { requested, floor } => {
                requested.map_or(Ok(()), VmafTarget::validate)?;
                floor.map_or(Ok(()), VmafTarget::validate)?;
                if let (Some(requested), Some(floor)) = (requested, floor)
                    && floor > requested
                {
                    return Err("not-worthwhile floor exceeds the requested target");
                }
                Ok(())
            }
        }
    }
}

impl TranslatedQuality {
    fn validate(&self) -> Result<(), &'static str> {
        self.score.map_or(Ok(()), VmafScore::validate)?;
        self.target.map_or(Ok(()), VmafTarget::validate)
    }
}

/// One record of an import batch with its readable source path, when the
/// origin still held one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportCandidate {
    pub observation: TranslatedObservation,
    pub path: Option<PathBuf>,
}

/// What storage commits for one accepted batch, as one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    /// New observations in file order, each with its path.
    pub inserts: Vec<ImportCandidate>,
    pub report: ImportReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportReport {
    pub inserted: usize,
    pub duplicates: usize,
    /// Incoming records dropped because a different observation already
    /// holds their identity, in file order.
    pub conflicts: Vec<TranslatedId>,
}

/// The first invalid record of a rejected batch; nothing was planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportRejection {
    pub index: usize,
    pub reason: &'static str,
}

/// Plans one batch against the translated observations already stored.
/// Paths never take part in the comparison, so a duplicate writes nothing,
/// and a re-import after a scrub restores no readable path.
pub fn plan_import<'a>(
    batch: Vec<ImportCandidate>,
    existing: impl Fn(&TranslatedId) -> Option<&'a TranslatedObservation>,
) -> Result<ImportPlan, ImportRejection> {
    if let Some(rejection) = batch.iter().enumerate().find_map(|(index, candidate)| {
        candidate
            .observation
            .validate()
            .err()
            .map(|reason| ImportRejection { index, reason })
    }) {
        return Err(rejection);
    }
    let mut planned: BTreeMap<TranslatedId, usize> = BTreeMap::new();
    let mut inserts: Vec<ImportCandidate> = Vec::new();
    let mut duplicates = 0;
    let mut conflicts = Vec::new();
    for candidate in batch {
        let id = candidate.observation.id();
        let present = match planned.get(&id) {
            Some(&index) => inserts.get(index).map(|earlier| &earlier.observation),
            None => existing(&id),
        };
        match present.map(|present| *present == candidate.observation) {
            None => {
                planned.insert(id, inserts.len());
                inserts.push(candidate);
            }
            Some(true) => duplicates += 1,
            Some(false) => conflicts.push(id),
        }
    }
    Ok(ImportPlan {
        report: ImportReport {
            inserted: inserts.len(),
            duplicates,
            conflicts,
        },
        inserts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> RecordKey {
        RecordKey::new(text.to_owned()).unwrap_or_else(|error| panic!("{text}: {error}"))
    }

    fn sparse_source() -> TranslatedSource {
        TranslatedSource {
            codec: None,
            width: None,
            height: None,
            duration_ms: None,
            size_bytes: None,
        }
    }

    fn converted(record_key: &str, crf: u32) -> TranslatedObservation {
        TranslatedObservation {
            origin: ImportOrigin::V2History,
            record_key: key(record_key),
            source: sparse_source(),
            outcome: TranslatedOutcome::Converted {
                quality: TranslatedQuality {
                    crf: Some(Crf(crf)),
                    score: None,
                    target: None,
                },
                output_size: None,
                encode_duration: None,
            },
            updated_at: None,
        }
    }

    fn candidate(observation: TranslatedObservation, path: Option<&str>) -> ImportCandidate {
        ImportCandidate {
            observation,
            path: path.map(PathBuf::from),
        }
    }

    fn empty_store(_: &TranslatedId) -> Option<&'static TranslatedObservation> {
        None
    }

    #[test]
    fn record_keys_are_exactly_sixteen_lowercase_hex_characters() {
        assert_eq!(key("0123456789abcdef").as_str(), "0123456789abcdef");
        for refused in [
            "",
            "0123456789abcde",
            "0123456789abcdef0",
            "0123456789ABCDEF",
            "0123456789abcdeg",
            "/videos/film.mkv",
            "c:\\videos\\a.mkv",
            "0123456789abcdé",
        ] {
            assert_eq!(
                RecordKey::new(refused.to_owned()),
                Err("record key must be 16 lowercase hexadecimal characters"),
                "{refused}"
            );
            let encoded = format!("\"{}\"", refused.replace('\\', "\\\\"));
            assert!(
                serde_json::from_str::<RecordKey>(&encoded).is_err(),
                "{refused}"
            );
        }
        let decoded: Result<RecordKey, _> = serde_json::from_str("\"00000000000000ff\"");
        assert_eq!(decoded.ok(), Some(key("00000000000000ff")));
    }

    #[test]
    fn validation_rejects_present_facts_native_validation_rejects() {
        let sparse = converted("00000000000000a1", 30_000);
        assert_eq!(sparse.validate(), Ok(()));

        let mut high_score = sparse.clone();
        high_score.outcome = TranslatedOutcome::Analyzed {
            quality: TranslatedQuality {
                crf: None,
                score: Some(VmafScore(10_001)),
                target: None,
            },
        };
        assert_eq!(
            high_score.validate(),
            Err("VMAF score is outside the supported range")
        );

        let mut high_target = sparse.clone();
        high_target.outcome = TranslatedOutcome::Converted {
            quality: TranslatedQuality {
                crf: None,
                score: Some(VmafScore(10_000)),
                target: Some(VmafTarget(101)),
            },
            output_size: None,
            encode_duration: None,
        };
        assert_eq!(
            high_target.validate(),
            Err("VMAF targets must be in 0..=100")
        );

        let declined = |requested, floor| TranslatedObservation {
            outcome: TranslatedOutcome::NotWorthwhile { requested, floor },
            ..sparse.clone()
        };
        assert_eq!(declined(None, None).validate(), Ok(()));
        assert_eq!(declined(None, Some(VmafTarget(99))).validate(), Ok(()));
        assert_eq!(
            declined(Some(VmafTarget(95)), Some(VmafTarget(95))).validate(),
            Ok(())
        );
        assert_eq!(
            declined(Some(VmafTarget(90)), Some(VmafTarget(95))).validate(),
            Err("not-worthwhile floor exceeds the requested target")
        );
        assert_eq!(
            declined(Some(VmafTarget(101)), None).validate(),
            Err("VMAF targets must be in 0..=100")
        );
    }

    #[test]
    fn one_invalid_record_rejects_the_whole_batch_at_the_first_failure() {
        let mut invalid = converted("00000000000000a2", 30_000);
        invalid.outcome = TranslatedOutcome::NotWorthwhile {
            requested: Some(VmafTarget(90)),
            floor: Some(VmafTarget(95)),
        };
        let mut also_invalid = converted("00000000000000a3", 30_000);
        also_invalid.outcome = TranslatedOutcome::Analyzed {
            quality: TranslatedQuality {
                crf: None,
                score: Some(VmafScore(10_001)),
                target: None,
            },
        };
        let batch = vec![
            candidate(converted("00000000000000a1", 30_000), Some("a.mkv")),
            candidate(invalid, None),
            candidate(also_invalid, None),
        ];
        assert_eq!(
            plan_import(batch, empty_store),
            Err(ImportRejection {
                index: 1,
                reason: "not-worthwhile floor exceeds the requested target",
            })
        );
    }

    #[test]
    fn new_identities_insert_in_file_order_with_their_paths() {
        let batch = vec![
            candidate(converted("00000000000000b2", 30_000), Some("b.mkv")),
            candidate(converted("00000000000000b1", 31_000), None),
        ];
        let Ok(plan) = plan_import(batch.clone(), empty_store) else {
            panic!("valid batch plans");
        };
        assert_eq!(plan.inserts, batch);
        assert_eq!(
            plan.report,
            ImportReport {
                inserted: 2,
                duplicates: 0,
                conflicts: vec![],
            }
        );
    }

    #[test]
    fn stored_identities_are_duplicates_or_conflicts_and_the_stored_one_stands() {
        let stored = [
            converted("00000000000000c1", 30_000),
            converted("00000000000000c2", 30_000),
        ];
        let existing = |id: &TranslatedId| stored.iter().find(|stored| stored.id() == *id);
        let batch = vec![
            candidate(converted("00000000000000c1", 30_000), Some("renamed.mkv")),
            candidate(converted("00000000000000c2", 32_000), Some("c2.mkv")),
            candidate(converted("00000000000000c3", 30_000), Some("c3.mkv")),
        ];
        let Ok(plan) = plan_import(batch, existing) else {
            panic!("valid batch plans");
        };
        assert_eq!(
            plan.inserts,
            vec![candidate(
                converted("00000000000000c3", 30_000),
                Some("c3.mkv")
            )]
        );
        assert_eq!(
            plan.report,
            ImportReport {
                inserted: 1,
                duplicates: 1,
                conflicts: vec![converted("00000000000000c2", 32_000).id()],
            }
        );
    }

    #[test]
    fn earlier_records_in_the_same_batch_count_as_present() {
        let batch = vec![
            candidate(converted("00000000000000d1", 30_000), Some("first.mkv")),
            candidate(converted("00000000000000d1", 30_000), Some("second.mkv")),
            candidate(converted("00000000000000d1", 33_000), None),
        ];
        let Ok(plan) = plan_import(batch, empty_store) else {
            panic!("valid batch plans");
        };
        assert_eq!(
            plan.inserts,
            vec![candidate(
                converted("00000000000000d1", 30_000),
                Some("first.mkv")
            )]
        );
        assert_eq!(
            plan.report,
            ImportReport {
                inserted: 1,
                duplicates: 1,
                conflicts: vec![converted("00000000000000d1", 30_000).id()],
            }
        );
    }

    #[test]
    fn an_empty_batch_plans_nothing() {
        assert_eq!(
            plan_import(Vec::new(), empty_store),
            Ok(ImportPlan {
                inserts: Vec::new(),
                report: ImportReport {
                    inserted: 0,
                    duplicates: 0,
                    conflicts: Vec::new(),
                },
            })
        );
    }
}
