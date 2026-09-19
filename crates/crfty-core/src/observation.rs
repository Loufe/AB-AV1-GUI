//! One immutable observation per terminal run, the unit of History fixed by
//! ADR-024. An observation is identified by its run identifier, names at
//! most one source by content key, and never changes once the run is
//! terminal: a retry, a changed source, or a later success is a new
//! observation, and a file's standing is derived from its observations on
//! request. Predictions (`SearchEvidence`) and measurements
//! (`EncodeEvidence`, `RemuxEvidence`) are distinct types so no consumer can
//! present one as the other, and an absent fact stays absent.
//!
//! History owns eligibility: the typed accessors on [`Observation`] yield a
//! fact only when the outcome, source assessment, and evidence qualify for
//! that consumer. Estimation owns weighting and never re-decides
//! eligibility. The full contract, including the facts required per outcome
//! and the eligibility table, is `docs/HISTORY.md`.
//!
//! Today the observation is a derived view over the run, record, and output
//! ledgers of [`DurableState`]; the same type becomes the stored unit when
//! History gains its own storage.

use serde::{Deserialize, Serialize};

use crate::{
    AnalysisAttempt, AnalysisResult, CompletionEvidence, ContentKey, ConversionRun, DecodeMode,
    DurableState, DurationMs, FailureFacts, ItemOutcome, JobPhase, Operation, PhaseSpan, RunId,
    SearchMeasurement, ToolRevisions, UnixMillis, VideoCodec, VmafTarget,
};

/// One terminal run. Skipped work and reservation-only failures decide
/// nothing and produce no observation; their reasons stay on the queue item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub run_id: RunId,
    pub operation: Operation,
    /// `None`: the run ended before its source was identified by content.
    pub source: Option<SourceFacts>,
    /// The toolchain the claimed execution profile named. Every run carries
    /// it, including runs that never searched, so estimation can weight by
    /// toolchain without reaching into the analysis.
    pub revisions: ToolRevisions,
    pub outcome: ObservedOutcome,
    pub started_at: Option<UnixMillis>,
    /// When the terminal outcome was decided. An unknown instant stays
    /// unknown; it is never filled in from another clock.
    pub finished_at: Option<UnixMillis>,
}

/// The source as identified by content, with the media facts cohorts are
/// built from. Width and height are post-rotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFacts {
    pub content_key: ContentKey,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub size_bytes: u64,
}

/// What the run concluded, carrying only the evidence that outcome can
/// honestly hold. Failed, stopped, and incomplete runs are browsable
/// evidence and feed no aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ObservedOutcome {
    Analyzed {
        search: SearchEvidence,
    },
    Converted {
        search: SearchEvidence,
        encode: EncodeEvidence,
    },
    Remuxed {
        remux: RemuxEvidence,
    },
    /// Quality-target fallback attempts are one of three things called an
    /// attempt (ADR-024): they belong to this run's search. The
    /// hardware-to-software decode retry is visible in
    /// [`EncodeMeasurement::Live`]'s decode mode; a queue retry is a new run
    /// and therefore a new observation.
    NotWorthwhile {
        requested: VmafTarget,
        floor: VmafTarget,
        attempts: Vec<AnalysisAttempt>,
    },
    Failed {
        facts: FailureFacts,
    },
    Stopped,
    Incomplete,
}

/// What a quality search claimed. Everything inside `analysis.measurement`
/// beyond the CRF and score is a prediction; nothing here was measured on a
/// finished encode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchEvidence {
    pub analysis: AnalysisResult,
    /// Wall time of the search phase in this run; absent when the analysis
    /// was reused rather than searched here.
    pub duration: Option<DurationMs>,
    pub assessment: Option<SourceAssessment>,
}

/// What a finished encode measured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncodeEvidence {
    pub measurement: EncodeMeasurement,
    /// Wall time of the encode phase; absent when the run recorded no span.
    pub duration: Option<DurationMs>,
    /// The settled output's content key; absent only when the transaction
    /// could not be settled to an artifact.
    pub output_content_key: Option<ContentKey>,
    pub assessment: Option<SourceAssessment>,
}

/// Sizes are verified by the settled output transaction or absent, never
/// estimated. A live encode always carries both; a crash-recovered success
/// carries them only when the promoted artifact identity is durable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum EncodeMeasurement {
    Live {
        sizes: MeasuredSizes,
        /// The decode mode the encode actually ran with, which diverges
        /// from the search profile after a hardware-to-software retry.
        decode: DecodeMode,
    },
    Recovered {
        sizes: Option<MeasuredSizes>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemuxEvidence {
    pub measurement: RemuxMeasurement,
    pub output_content_key: Option<ContentKey>,
    pub assessment: Option<SourceAssessment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RemuxMeasurement {
    Live { sizes: MeasuredSizes },
    Recovered { sizes: Option<MeasuredSizes> },
}

/// Both byte sizes of one produced output; representable only as a pair so
/// a half-known reduction cannot exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasuredSizes {
    pub input: u64,
    pub output: u64,
}

impl MeasuredSizes {
    /// Signed reduction: source minus produced-file logical size, negative
    /// when the output grew.
    #[must_use]
    pub fn reduction_bytes(self) -> i128 {
        i128::from(self.input) - i128::from(self.output)
    }
}

/// How the source compared across a phase's read interval, in the classes
/// the source-continuity contract defines. `None` on an observation means
/// the run was never assessed, which is every run recorded before the engine
/// produced assessments; the `Option` disappears once it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceAssessment {
    Matched,
    IdentityDiffers,
    PropertiesChanged,
    Absent,
    Unassessable,
}

/// What a decisive observation concluded about its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisiveKind {
    Converted,
    Remuxed,
    NotWorthwhile {
        requested: VmafTarget,
        floor: VmafTarget,
    },
}

/// An observation that decides the standing of its content. Aggregates that
/// count files dedupe these by content key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecisiveFact<'a> {
    pub content_key: &'a ContentKey,
    pub kind: DecisiveKind,
    pub finished_at: Option<UnixMillis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Converted,
    Remuxed,
}

/// A produced output with both sizes measured on a matched source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReductionFact<'a> {
    pub source: &'a SourceFacts,
    pub kind: OutputKind,
    pub sizes: MeasuredSizes,
    pub finished_at: Option<UnixMillis>,
    /// The search behind a conversion; a remux has none.
    pub search: Option<&'a AnalysisResult>,
}

/// Measured wall time of one phase over one source, from which estimation
/// derives a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateSample<'a> {
    pub source: &'a SourceFacts,
    pub elapsed: DurationMs,
}

/// A prediction and the measurement that tests it, from the same run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictionPair<'a> {
    pub source: &'a SourceFacts,
    pub predicted: &'a SearchMeasurement,
    pub measured: MeasuredSizes,
    pub encoding: DurationMs,
}

impl Observation {
    /// Consistency the type shape cannot express. Every observation derived
    /// from folded state satisfies this; deserialized fixtures prove which
    /// shapes the contract rejects.
    pub fn validate(&self) -> Result<(), &'static str> {
        if let (Some(started), Some(finished)) = (self.started_at, self.finished_at)
            && started > finished
        {
            return Err("observation finished before it started");
        }
        match &self.outcome {
            ObservedOutcome::Analyzed { search } => search.analysis.validate_consistent(),
            ObservedOutcome::Converted { search, encode } => {
                search.analysis.validate_consistent()?;
                if matches!(encode.measurement, EncodeMeasurement::Live { .. })
                    && encode.duration.is_none()
                {
                    return Err("live encode measurement requires an encode duration");
                }
                Ok(())
            }
            ObservedOutcome::Remuxed { .. }
            | ObservedOutcome::Failed { .. }
            | ObservedOutcome::Stopped
            | ObservedOutcome::Incomplete => Ok(()),
            ObservedOutcome::NotWorthwhile {
                requested,
                floor,
                attempts,
            } => {
                if floor > requested {
                    return Err("not-worthwhile floor exceeds the requested target");
                }
                if attempts.is_empty() {
                    return Err("not-worthwhile observation carries no attempts");
                }
                if attempts
                    .iter()
                    .any(|attempt| attempt.target > *requested || attempt.target < *floor)
                {
                    return Err("not-worthwhile attempt targets outside the fallback range");
                }
                attempts
                    .iter()
                    .filter_map(|attempt| attempt.last_measurement.as_ref())
                    .try_for_each(SearchMeasurement::validate)
            }
        }
    }

    /// The content this observation is about, when identified.
    #[must_use]
    pub fn content_key(&self) -> Option<&ContentKey> {
        self.source.as_ref().map(|source| &source.content_key)
    }

    /// Whether this observation decides its content's standing. Requires
    /// no assessment: the decision was made, whatever its evidence is now
    /// worth to an aggregate.
    #[must_use]
    pub fn decisive_fact(&self) -> Option<DecisiveFact<'_>> {
        let kind = match &self.outcome {
            ObservedOutcome::Converted { .. } => DecisiveKind::Converted,
            ObservedOutcome::Remuxed { .. } => DecisiveKind::Remuxed,
            ObservedOutcome::NotWorthwhile {
                requested, floor, ..
            } => DecisiveKind::NotWorthwhile {
                requested: *requested,
                floor: *floor,
            },
            ObservedOutcome::Analyzed { .. }
            | ObservedOutcome::Failed { .. }
            | ObservedOutcome::Stopped
            | ObservedOutcome::Incomplete => return None,
        };
        Some(DecisiveFact {
            content_key: self.content_key()?,
            kind,
            finished_at: self.finished_at,
        })
    }

    /// Eligible for output-size reduction aggregates: a produced output with
    /// both sizes measured whose source matched across the producing phase.
    #[must_use]
    pub fn reduction_fact(&self) -> Option<ReductionFact<'_>> {
        let source = self.source.as_ref()?;
        let (kind, sizes, assessment, search) = match &self.outcome {
            ObservedOutcome::Converted { search, encode } => (
                OutputKind::Converted,
                encode.measurement.sizes()?,
                encode.assessment,
                Some(&search.analysis),
            ),
            ObservedOutcome::Remuxed { remux } => (
                OutputKind::Remuxed,
                remux.measurement.sizes()?,
                remux.assessment,
                None,
            ),
            ObservedOutcome::Analyzed { .. }
            | ObservedOutcome::NotWorthwhile { .. }
            | ObservedOutcome::Failed { .. }
            | ObservedOutcome::Stopped
            | ObservedOutcome::Incomplete => return None,
        };
        matched(assessment)?;
        Some(ReductionFact {
            source,
            kind,
            sizes,
            finished_at: self.finished_at,
            search,
        })
    }

    /// Eligible for the time cohort of `operation`: the phase ran in this
    /// run against a source of known positive duration that matched across
    /// the phase. Analyze samples come from every outcome that searched;
    /// convert samples come from conversions only.
    #[must_use]
    pub fn rate_sample(&self, operation: Operation) -> Option<RateSample<'_>> {
        let source = self
            .source
            .as_ref()
            .filter(|source| source.duration_ms > 0)?;
        let (elapsed, assessment) = match (operation, &self.outcome) {
            (
                Operation::Analyze,
                ObservedOutcome::Analyzed { search } | ObservedOutcome::Converted { search, .. },
            ) => (search.duration?, search.assessment),
            (Operation::Convert, ObservedOutcome::Converted { encode, .. }) => {
                (encode.duration?, encode.assessment)
            }
            (
                Operation::Analyze,
                ObservedOutcome::Remuxed { .. }
                | ObservedOutcome::NotWorthwhile { .. }
                | ObservedOutcome::Failed { .. }
                | ObservedOutcome::Stopped
                | ObservedOutcome::Incomplete,
            )
            | (
                Operation::Convert,
                ObservedOutcome::Analyzed { .. }
                | ObservedOutcome::Remuxed { .. }
                | ObservedOutcome::NotWorthwhile { .. }
                | ObservedOutcome::Failed { .. }
                | ObservedOutcome::Stopped
                | ObservedOutcome::Incomplete,
            ) => return None,
        };
        if elapsed.0 == 0 {
            return None;
        }
        matched(assessment)?;
        Some(RateSample { source, elapsed })
    }

    /// Eligible for prediction backtests: a conversion whose search
    /// predicted and whose encode measured sizes and time, with both phases
    /// assessed as matched.
    #[must_use]
    pub fn prediction_pair(&self) -> Option<PredictionPair<'_>> {
        let source = self.source.as_ref()?;
        let ObservedOutcome::Converted { search, encode } = &self.outcome else {
            return None;
        };
        matched(search.assessment)?;
        matched(encode.assessment)?;
        Some(PredictionPair {
            source,
            predicted: &search.analysis.measurement,
            measured: encode.measurement.sizes()?,
            encoding: encode.duration?,
        })
    }
}

impl EncodeMeasurement {
    #[must_use]
    pub fn sizes(&self) -> Option<MeasuredSizes> {
        match self {
            Self::Live { sizes, .. } => Some(*sizes),
            Self::Recovered { sizes } => *sizes,
        }
    }
}

impl RemuxMeasurement {
    #[must_use]
    pub fn sizes(&self) -> Option<MeasuredSizes> {
        match self {
            Self::Live { sizes } => Some(*sizes),
            Self::Recovered { sizes } => *sizes,
        }
    }
}

fn matched(assessment: Option<SourceAssessment>) -> Option<()> {
    (assessment == Some(SourceAssessment::Matched)).then_some(())
}

/// Every terminal, non-skipped run as an observation, in run order.
#[must_use]
pub fn observations(state: &DurableState) -> Vec<Observation> {
    state
        .conversion_runs
        .values()
        .filter_map(|run| observe(state, run))
        .collect()
}

/// The latest decisive observation about `content_key`, or `None` when
/// nothing has decided its standing. Display standing only: queue
/// eligibility keeps reading the record's verdict.
#[must_use]
pub fn standing<'a>(
    observations: &'a [Observation],
    content_key: &ContentKey,
) -> Option<&'a Observation> {
    observations.iter().rev().find(|observation| {
        observation
            .decisive_fact()
            .is_some_and(|fact| fact.content_key == content_key)
    })
}

fn observe(state: &DurableState, run: &ConversionRun) -> Option<Observation> {
    let outcome = match run.outcome.as_ref()? {
        ItemOutcome::Skipped { .. } => return None,
        ItemOutcome::Analyzed => ObservedOutcome::Analyzed {
            search: search_evidence(run)?,
        },
        ItemOutcome::Converted(evidence) => ObservedOutcome::Converted {
            search: search_evidence(run)?,
            encode: EncodeEvidence {
                measurement: match evidence {
                    CompletionEvidence::LiveEncode {
                        input_size,
                        output_size,
                        encode_decode,
                    } => EncodeMeasurement::Live {
                        sizes: MeasuredSizes {
                            input: *input_size,
                            output: *output_size,
                        },
                        decode: *encode_decode,
                    },
                    CompletionEvidence::LiveRemux { .. } => return None,
                    CompletionEvidence::RecoveredAtStartup => EncodeMeasurement::Recovered {
                        sizes: recovered_sizes(state, run.spec.run_id),
                    },
                },
                duration: phase_duration(&run.phase_spans, JobPhase::Encoding),
                output_content_key: run.output_content_key.clone(),
                assessment: None,
            },
        },
        ItemOutcome::Remuxed(evidence) => ObservedOutcome::Remuxed {
            remux: RemuxEvidence {
                measurement: match evidence {
                    CompletionEvidence::LiveRemux {
                        input_size,
                        output_size,
                    } => RemuxMeasurement::Live {
                        sizes: MeasuredSizes {
                            input: *input_size,
                            output: *output_size,
                        },
                    },
                    CompletionEvidence::LiveEncode { .. } => return None,
                    CompletionEvidence::RecoveredAtStartup => RemuxMeasurement::Recovered {
                        sizes: recovered_sizes(state, run.spec.run_id),
                    },
                },
                output_content_key: run.output_content_key.clone(),
                assessment: None,
            },
        },
        ItemOutcome::NotWorthwhile { attempts } => ObservedOutcome::NotWorthwhile {
            requested: run.spec.execution.requested_target,
            floor: run.spec.execution.fallback_floor,
            attempts: attempts.clone(),
        },
        ItemOutcome::Failed(facts) => ObservedOutcome::Failed {
            facts: facts.clone(),
        },
        ItemOutcome::Stopped => ObservedOutcome::Stopped,
        ItemOutcome::Incomplete => ObservedOutcome::Incomplete,
    };
    let source = run.spec.content_key.as_ref().and_then(|content_key| {
        let metadata = &state.records.get(content_key)?.metadata;
        let (width, height) = metadata.post_rotation_dimensions();
        Some(SourceFacts {
            content_key: content_key.clone(),
            codec: metadata.codec.clone(),
            width,
            height,
            duration_ms: metadata.duration_ms,
            size_bytes: metadata.size_bytes,
        })
    });
    let profile = &run.spec.execution.profile;
    Some(Observation {
        run_id: run.spec.run_id,
        operation: run.spec.operation,
        source,
        revisions: ToolRevisions {
            ab_av1: profile.ab_av1_revision.clone(),
            ffmpeg: profile.ffmpeg_revision.clone(),
            encoder: profile.encoder_revision.clone(),
        },
        outcome,
        started_at: run.started_at,
        finished_at: run.finished_at,
    })
}

fn search_evidence(run: &ConversionRun) -> Option<SearchEvidence> {
    Some(SearchEvidence {
        analysis: run.analysis.clone()?,
        duration: phase_duration(&run.phase_spans, JobPhase::Analyzing),
        assessment: None,
    })
}

/// A crash-recovered success measured nothing itself; the promoted artifact
/// identity in the output transaction is the only verified size source.
fn recovered_sizes(state: &DurableState, run_id: RunId) -> Option<MeasuredSizes> {
    let transaction = state.outputs.get(&run_id)?;
    let artifact = transaction.promoted_identity()?;
    Some(MeasuredSizes {
        input: transaction.input_identity.size,
        output: artifact.destructive.size,
    })
}

/// Total wall time of one phase; `None` when the run recorded no span of
/// that phase, which is distinct from a zero-length phase.
pub(crate) fn phase_duration(spans: &[PhaseSpan], phase: JobPhase) -> Option<DurationMs> {
    let mut measured = false;
    let mut total: u64 = 0;
    for span in spans.iter().filter(|span| span.phase == phase) {
        measured = true;
        total = total.saturating_add(span.duration.0);
    }
    measured.then_some(DurationMs(total))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use proptest::prelude::*;

    use super::*;
    use crate::test_support::{analysis, finished_run, identity, key, live, meta};
    use crate::{
        ArtifactIdentity, ConversionRun, DurableState, FailureKind, FileRecord, JobAction,
        OutputState, OutputTransaction, Replacement, SkipReason, VideoCodec, VmafScore,
    };

    const FINISHED: UnixMillis = UnixMillis(1_700_000_000_000);

    fn state_with(runs: Vec<ConversionRun>) -> DurableState {
        let mut state = DurableState::default();
        for run in runs {
            if let Some(content_key) = &run.spec.content_key
                && !state.records.contains_key(content_key)
            {
                state.records.insert(
                    content_key.clone(),
                    FileRecord::new(meta(VideoCodec::Hevc, 8_000)),
                );
            }
            state.conversion_runs.insert(run.spec.run_id, run);
        }
        state
    }

    fn run(id: u64, content: &str, outcome: ItemOutcome) -> ConversionRun {
        let mut run = finished_run(id, &key(content), outcome, UnixMillis(FINISHED.0 + id));
        match &run.outcome {
            Some(ItemOutcome::Remuxed(_)) => {
                run.analysis = None;
                run.spec.action = JobAction::Remux;
                run.phase_spans.clear();
            }
            Some(ItemOutcome::NotWorthwhile { .. }) => {
                run.analysis = None;
                run.phase_spans.truncate(1);
            }
            Some(ItemOutcome::Analyzed) => {
                run.spec.operation = Operation::Analyze;
                run.spec.action = JobAction::Analyze {
                    selected_analysis: None,
                };
                run.phase_spans.truncate(1);
            }
            _ => {}
        }
        run
    }

    fn not_worthwhile() -> ItemOutcome {
        ItemOutcome::NotWorthwhile {
            attempts: vec![AnalysisAttempt {
                target: VmafTarget(95),
                last_measurement: None,
            }],
        }
    }

    fn committed_output(run_id: u64, input: u64, output: u64) -> OutputTransaction {
        OutputTransaction {
            run_id: RunId(run_id),
            input: PathBuf::from(format!("input-{run_id}.mkv")),
            input_identity: identity(input),
            staging: PathBuf::from("staging"),
            final_path: PathBuf::from("final.mkv"),
            final_preimage: None,
            replacement: Replacement::KeepOriginal,
            state: OutputState::Committed {
                final_identity: ArtifactIdentity {
                    content_key: key("output"),
                    destructive: identity(output),
                },
            },
        }
    }

    fn assessed(mut observation: Observation, assessment: Option<SourceAssessment>) -> Observation {
        match &mut observation.outcome {
            ObservedOutcome::Analyzed { search } => search.assessment = assessment,
            ObservedOutcome::Converted { search, encode } => {
                search.assessment = assessment;
                encode.assessment = assessment;
            }
            ObservedOutcome::Remuxed { remux } => remux.assessment = assessment,
            ObservedOutcome::NotWorthwhile { .. }
            | ObservedOutcome::Failed { .. }
            | ObservedOutcome::Stopped
            | ObservedOutcome::Incomplete => {}
        }
        observation
    }

    #[expect(clippy::expect_used, reason = "test assertion")]
    fn only(state: &DurableState) -> Observation {
        let mut found = observations(state);
        assert_eq!(found.len(), 1, "exactly one observation");
        found.pop().expect("length checked")
    }

    #[test]
    fn every_terminal_outcome_except_skipped_becomes_one_observation() {
        let state = state_with(vec![
            run(1, "a", ItemOutcome::Analyzed),
            run(2, "a", ItemOutcome::Converted(live(8_000, 3_000))),
            run(
                3,
                "b",
                ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
                    input_size: 8_000,
                    output_size: 7_900,
                }),
            ),
            run(4, "c", not_worthwhile()),
            run(
                5,
                "d",
                ItemOutcome::Failed(FailureFacts::new(FailureKind::EncodeRun, "boom")),
            ),
            run(6, "d", ItemOutcome::Stopped),
            run(7, "d", ItemOutcome::Incomplete),
            run(
                8,
                "e",
                ItemOutcome::Skipped {
                    reason: SkipReason::AlreadyAv1Matroska,
                },
            ),
            ConversionRun {
                outcome: None,
                ..run(9, "f", ItemOutcome::Stopped)
            },
        ]);
        let found = observations(&state);
        let ids: Vec<u64> = found
            .iter()
            .map(|observation| observation.run_id.0)
            .collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6, 7]);
        for observation in &found {
            assert_eq!(observation.validate(), Ok(()));
            assert_eq!(
                observation.finished_at,
                Some(UnixMillis(FINISHED.0 + observation.run_id.0))
            );
        }
        let outcomes: Vec<&ObservedOutcome> = found.iter().map(|found| &found.outcome).collect();
        let [
            analyzed,
            converted,
            remuxed,
            not_worthwhile,
            failed,
            stopped,
            incomplete,
        ] = outcomes.as_slice()
        else {
            panic!("seven observations");
        };
        assert!(matches!(analyzed, ObservedOutcome::Analyzed { .. }));
        assert!(matches!(converted, ObservedOutcome::Converted { .. }));
        assert!(matches!(remuxed, ObservedOutcome::Remuxed { .. }));
        assert!(matches!(
            not_worthwhile,
            ObservedOutcome::NotWorthwhile {
                requested: VmafTarget(95),
                floor: VmafTarget(90),
                ..
            }
        ));
        assert!(matches!(failed, ObservedOutcome::Failed { facts } if facts.message == "boom"));
        assert_eq!(*stopped, &ObservedOutcome::Stopped);
        assert_eq!(*incomplete, &ObservedOutcome::Incomplete);
    }

    #[test]
    fn live_conversion_carries_predictions_and_measurements_apart() {
        let state = state_with(vec![run(
            1,
            "a",
            ItemOutcome::Converted(live(8_000, 3_000)),
        )]);
        let observation = only(&state);
        let ObservedOutcome::Converted { search, encode } = &observation.outcome else {
            panic!("converted observation");
        };
        assert_eq!(search.analysis, analysis(24_000, 9_512));
        assert_eq!(search.duration, Some(DurationMs(60_000)));
        assert_eq!(
            encode.measurement,
            EncodeMeasurement::Live {
                sizes: MeasuredSizes {
                    input: 8_000,
                    output: 3_000
                },
                decode: DecodeMode::Software,
            }
        );
        assert_eq!(encode.duration, Some(DurationMs(240_000)));
        assert_eq!(search.assessment, None);
        assert_eq!(encode.assessment, None);
        let Some(source) = &observation.source else {
            panic!("identified source");
        };
        assert_eq!(source.content_key, key("a"));
        assert_eq!((source.width, source.height), (1920, 1080));
        assert_eq!(source.duration_ms, 600_000);
    }

    #[test]
    fn recovered_success_takes_sizes_only_from_the_promoted_artifact() {
        let mut state = state_with(vec![
            run(
                1,
                "a",
                ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
            ),
            run(
                2,
                "b",
                ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
            ),
        ]);
        state
            .outputs
            .insert(RunId(1), committed_output(1, 8_000, 3_000));
        let found = observations(&state);
        let measurements: Vec<&EncodeMeasurement> = found
            .iter()
            .filter_map(|observation| match &observation.outcome {
                ObservedOutcome::Converted { encode, .. } => Some(&encode.measurement),
                _ => None,
            })
            .collect();
        assert_eq!(
            measurements,
            vec![
                &EncodeMeasurement::Recovered {
                    sizes: Some(MeasuredSizes {
                        input: 8_000,
                        output: 3_000
                    })
                },
                &EncodeMeasurement::Recovered { sizes: None },
            ]
        );
    }

    #[test]
    fn phase_durations_are_absent_without_a_span_and_sum_repeated_spans() {
        let spans = vec![
            PhaseSpan {
                phase: JobPhase::Encoding,
                duration: DurationMs(10),
            },
            PhaseSpan {
                phase: JobPhase::Encoding,
                duration: DurationMs(5),
            },
        ];
        assert_eq!(
            phase_duration(&spans, JobPhase::Encoding),
            Some(DurationMs(15))
        );
        assert_eq!(phase_duration(&spans, JobPhase::Analyzing), None);
        assert_eq!(phase_duration(&[], JobPhase::Encoding), None);
    }

    #[test]
    fn a_run_without_a_record_has_no_source_and_no_eligible_facts() {
        let mut state = state_with(vec![run(
            1,
            "a",
            ItemOutcome::Converted(live(8_000, 3_000)),
        )]);
        state.records.clear();
        let observation = assessed(only(&state), Some(SourceAssessment::Matched));
        assert_eq!(observation.source, None);
        assert!(observation.decisive_fact().is_none());
        assert!(observation.reduction_fact().is_none());
        assert!(observation.rate_sample(Operation::Convert).is_none());
        assert!(observation.prediction_pair().is_none());
    }

    #[test]
    fn standing_is_the_latest_decisive_observation_per_content() {
        let state = state_with(vec![
            run(1, "a", ItemOutcome::Converted(live(8_000, 3_000))),
            run(
                2,
                "a",
                ItemOutcome::Failed(FailureFacts::new(FailureKind::EncodeRun, "x")),
            ),
            run(3, "a", ItemOutcome::Analyzed),
            run(4, "b", not_worthwhile()),
            run(5, "b", ItemOutcome::Converted(live(8_000, 3_000))),
            run(6, "c", ItemOutcome::Stopped),
        ]);
        let found = observations(&state);
        let standing_run = |name: &str| standing(&found, &key(name)).map(|found| found.run_id);
        assert_eq!(standing_run("a"), Some(RunId(1)));
        assert_eq!(standing_run("b"), Some(RunId(5)));
        assert_eq!(standing_run("c"), None);
        assert_eq!(standing_run("missing"), None);
    }

    #[test]
    fn decisive_facts_need_no_assessment_but_aggregates_need_a_matched_source() {
        let state = state_with(vec![run(
            1,
            "a",
            ItemOutcome::Converted(live(8_000, 3_000)),
        )]);
        let unassessed = only(&state);
        assert!(unassessed.decisive_fact().is_some());
        assert!(unassessed.reduction_fact().is_none());
        assert!(unassessed.rate_sample(Operation::Analyze).is_none());
        assert!(unassessed.rate_sample(Operation::Convert).is_none());
        assert!(unassessed.prediction_pair().is_none());

        let matched = assessed(unassessed.clone(), Some(SourceAssessment::Matched));
        let Some(reduction) = matched.reduction_fact() else {
            panic!("matched conversion is a reduction fact");
        };
        assert_eq!(reduction.kind, OutputKind::Converted);
        assert_eq!(reduction.sizes.reduction_bytes(), 5_000);
        assert_eq!(
            reduction.search.map(|found| found.measurement.score),
            Some(VmafScore(9_512))
        );
        assert_eq!(
            matched
                .rate_sample(Operation::Analyze)
                .map(|sample| sample.elapsed),
            Some(DurationMs(60_000))
        );
        assert_eq!(
            matched
                .rate_sample(Operation::Convert)
                .map(|sample| sample.elapsed),
            Some(DurationMs(240_000))
        );
        let Some(pair) = matched.prediction_pair() else {
            panic!("matched conversion is a prediction pair");
        };
        assert_eq!(pair.predicted.predicted_size, 1_000);
        assert_eq!(pair.measured.output, 3_000);
        assert_eq!(pair.encoding, DurationMs(240_000));

        for assessment in [
            SourceAssessment::IdentityDiffers,
            SourceAssessment::PropertiesChanged,
            SourceAssessment::Absent,
            SourceAssessment::Unassessable,
        ] {
            let changed = assessed(unassessed.clone(), Some(assessment));
            assert!(changed.decisive_fact().is_some());
            assert!(changed.reduction_fact().is_none());
            assert!(changed.rate_sample(Operation::Convert).is_none());
            assert!(changed.prediction_pair().is_none());
        }
    }

    #[test]
    fn recovered_conversion_without_sizes_is_decisive_but_not_a_reduction_or_pair() {
        let state = state_with(vec![run(
            1,
            "a",
            ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
        )]);
        let observation = assessed(only(&state), Some(SourceAssessment::Matched));
        assert!(observation.decisive_fact().is_some());
        assert!(observation.reduction_fact().is_none());
        assert!(observation.prediction_pair().is_none());
        assert!(observation.rate_sample(Operation::Convert).is_some());
    }

    #[test]
    fn remux_and_not_worthwhile_feed_only_the_facts_they_can_support() {
        let state = state_with(vec![
            run(
                1,
                "a",
                ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
                    input_size: 8_000,
                    output_size: 7_900,
                }),
            ),
            run(2, "b", not_worthwhile()),
            run(3, "c", ItemOutcome::Analyzed),
        ]);
        let found: Vec<Observation> = observations(&state)
            .into_iter()
            .map(|observation| assessed(observation, Some(SourceAssessment::Matched)))
            .collect();
        let [remuxed, declined, analyzed] = found.as_slice() else {
            panic!("three observations");
        };

        let Some(remux) = remuxed.reduction_fact() else {
            panic!("live remux is a reduction fact");
        };
        assert_eq!(remux.kind, OutputKind::Remuxed);
        assert_eq!(remux.search, None);
        assert!(remuxed.rate_sample(Operation::Analyze).is_none());
        assert!(remuxed.rate_sample(Operation::Convert).is_none());
        assert!(remuxed.prediction_pair().is_none());

        assert!(matches!(
            declined.decisive_fact().map(|fact| fact.kind),
            Some(DecisiveKind::NotWorthwhile { .. })
        ));
        assert!(declined.reduction_fact().is_none());
        assert!(declined.rate_sample(Operation::Analyze).is_none());

        assert!(analyzed.decisive_fact().is_none());
        assert_eq!(
            analyzed
                .rate_sample(Operation::Analyze)
                .map(|sample| sample.elapsed),
            Some(DurationMs(60_000))
        );
        assert!(analyzed.rate_sample(Operation::Convert).is_none());
    }

    #[test]
    fn interrupted_and_failed_runs_are_browsable_only() {
        let state = state_with(vec![
            run(
                1,
                "a",
                ItemOutcome::Failed(FailureFacts::new(FailureKind::SearchRun, "x")),
            ),
            run(2, "a", ItemOutcome::Stopped),
            run(3, "a", ItemOutcome::Incomplete),
        ]);
        for observation in observations(&state) {
            let observation = assessed(observation, Some(SourceAssessment::Matched));
            assert!(observation.source.is_some());
            assert!(observation.decisive_fact().is_none());
            assert!(observation.reduction_fact().is_none());
            assert!(observation.rate_sample(Operation::Analyze).is_none());
            assert!(observation.rate_sample(Operation::Convert).is_none());
            assert!(observation.prediction_pair().is_none());
        }
    }

    #[test]
    fn rate_samples_need_positive_source_and_phase_durations() {
        let mut state = state_with(vec![run(
            1,
            "a",
            ItemOutcome::Converted(live(8_000, 3_000)),
        )]);
        let mut zero_length = assessed(only(&state), Some(SourceAssessment::Matched));
        if let Some(source) = &mut zero_length.source {
            source.duration_ms = 0;
        }
        assert!(zero_length.rate_sample(Operation::Convert).is_none());

        if let Some(run) = state.conversion_runs.get_mut(&RunId(1)) {
            run.phase_spans = vec![PhaseSpan {
                phase: JobPhase::Encoding,
                duration: DurationMs(0),
            }];
        }
        let unmeasured = assessed(only(&state), Some(SourceAssessment::Matched));
        assert!(unmeasured.rate_sample(Operation::Analyze).is_none());
        assert!(unmeasured.rate_sample(Operation::Convert).is_none());
    }

    #[test]
    fn validation_rejects_shapes_the_types_cannot_exclude() {
        let state = state_with(vec![
            run(1, "a", ItemOutcome::Converted(live(8_000, 3_000))),
            run(2, "b", not_worthwhile()),
        ]);
        let found = observations(&state);
        let [converted, declined] = found.as_slice() else {
            panic!("two observations");
        };

        let mut reversed = converted.clone();
        reversed.started_at = Some(UnixMillis(FINISHED.0 + 10));
        assert_eq!(
            reversed.validate(),
            Err("observation finished before it started")
        );

        let mut unmeasured = converted.clone();
        if let ObservedOutcome::Converted { encode, .. } = &mut unmeasured.outcome {
            encode.duration = None;
        }
        assert_eq!(
            unmeasured.validate(),
            Err("live encode measurement requires an encode duration")
        );

        let mut inconsistent = converted.clone();
        if let ObservedOutcome::Converted { search, .. } = &mut inconsistent.outcome {
            search.analysis.successful_target = VmafTarget(99);
        }
        assert_eq!(
            inconsistent.validate(),
            Err("successful VMAF target is outside the requested fallback range")
        );

        let mut empty = declined.clone();
        if let ObservedOutcome::NotWorthwhile { attempts, .. } = &mut empty.outcome {
            attempts.clear();
        }
        assert_eq!(
            empty.validate(),
            Err("not-worthwhile observation carries no attempts")
        );

        let mut below_floor = declined.clone();
        if let ObservedOutcome::NotWorthwhile { attempts, .. } = &mut below_floor.outcome {
            for attempt in attempts.iter_mut() {
                attempt.target = VmafTarget(80);
            }
        }
        assert_eq!(
            below_floor.validate(),
            Err("not-worthwhile attempt targets outside the fallback range")
        );

        let mut inverted = declined.clone();
        if let ObservedOutcome::NotWorthwhile { floor, .. } = &mut inverted.outcome {
            *floor = VmafTarget(99);
        }
        assert_eq!(
            inverted.validate(),
            Err("not-worthwhile floor exceeds the requested target")
        );
    }

    #[test]
    #[expect(clippy::expect_used, reason = "test assertion")]
    fn observations_round_trip_through_json() {
        let state = state_with(vec![
            run(1, "a", ItemOutcome::Converted(live(8_000, 3_000))),
            run(2, "b", not_worthwhile()),
            run(
                3,
                "c",
                ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "x")),
            ),
        ]);
        for observation in observations(&state) {
            let encoded = serde_json::to_string(&observation).expect("serialize observation");
            let decoded: Observation =
                serde_json::from_str(&encoded).expect("deserialize observation");
            assert_eq!(decoded, observation);
        }
    }

    fn outcome_strategy() -> impl Strategy<Value = ItemOutcome> {
        prop_oneof![
            Just(ItemOutcome::Analyzed),
            (1u64..1_000_000, 1u64..1_000_000)
                .prop_map(|(input, output)| ItemOutcome::Converted(live(input, output))),
            Just(ItemOutcome::Converted(
                CompletionEvidence::RecoveredAtStartup
            )),
            (1u64..1_000_000, 1u64..1_000_000).prop_map(|(input_size, output_size)| {
                ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
                    input_size,
                    output_size,
                })
            }),
            Just(not_worthwhile()),
            Just(ItemOutcome::Failed(FailureFacts::new(
                FailureKind::EncodeRun,
                "x"
            ))),
            Just(ItemOutcome::Stopped),
            Just(ItemOutcome::Incomplete),
            Just(ItemOutcome::Skipped {
                reason: SkipReason::AlreadyAv1Matroska
            }),
        ]
    }

    proptest! {
        #[test]
        fn derived_observations_validate_and_count_terminal_unskipped_runs(
            outcomes in proptest::collection::vec(outcome_strategy(), 0..24)
        ) {
            let runs: Vec<ConversionRun> = outcomes
                .iter()
                .enumerate()
                .map(|(index, outcome)| {
                    let id = index as u64 + 1;
                    run(id, &format!("content-{}", id % 5), outcome.clone())
                })
                .collect();
            let state = state_with(runs);
            let found = observations(&state);
            let expected = outcomes
                .iter()
                .filter(|outcome| !matches!(outcome, ItemOutcome::Skipped { .. }))
                .count();
            prop_assert_eq!(found.len(), expected);
            prop_assert!(found.is_sorted_by(|earlier, later| earlier.run_id < later.run_id));
            for observation in &found {
                prop_assert_eq!(observation.validate(), Ok(()));
                prop_assert_eq!(
                    observation.decisive_fact().is_some(),
                    matches!(
                        observation.outcome,
                        ObservedOutcome::Converted { .. }
                            | ObservedOutcome::Remuxed { .. }
                            | ObservedOutcome::NotWorthwhile { .. }
                    )
                );
            }
        }
    }
}
