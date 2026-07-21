use serde::{Deserialize, Serialize};

use crate::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, DEFAULT_VMAF_TARGET, DecodeMode,
    DestructiveIdentity, DurableState, ExecutionSettings, FileRecord, FileStamp,
    IMPORT_MTIME_TOLERANCE_NS, ImportedHistoryRecord, JobAction, MIN_VMAF_FALLBACK_TARGET,
    MediaContainer, MediaObservation, Operation, ParkedStatus, PathHash, RunId,
    TimestampReliability, Verdict, VerdictKind, VideoCodec, VideoMeta,
};

pub const MIN_VIDEO_PIXELS: u64 = 921_600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum SkipReason {
    LowResolution {
        #[specta(type = crate::JsNumber)]
        pixels: u64,
        #[specta(type = crate::JsNumber)]
        minimum: u64,
    },
    AlreadyAv1Matroska,
    OutputExists,
    /// Enqueue-time only: the file at the candidate path is recognized by
    /// full destructive identity as the settled output of `source_run` (replace-mode output at
    /// the input path).
    AlreadyConverted {
        source_run: Option<RunId>,
    },
    /// The content's standing verdict says a search already bottomed out at
    /// (or below) the floor this request would try. `source_run: None`: the
    /// verdict was adopted from a history import.
    NotWorthwhile {
        source_run: Option<RunId>,
    },
    /// Claim-time: the observed content carries a Converted/Remuxed verdict —
    /// this file is bytewise identical to content that was already processed,
    /// wherever it now lives. `source_run: None`: the verdict was adopted
    /// from a history import.
    ProbableDuplicate {
        source_run: Option<RunId>,
    },
    /// Add-summary only: the path is already queued and not finished. Never a
    /// terminal outcome.
    AlreadyQueued,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    Process,
    Remux,
    Skip(SkipReason),
}

#[must_use]
pub fn evaluate_eligibility(metadata: &VideoMeta, operation: Operation) -> Eligibility {
    let pixels = metadata.post_rotation_pixels();
    if pixels < MIN_VIDEO_PIXELS {
        return Eligibility::Skip(SkipReason::LowResolution {
            pixels,
            minimum: MIN_VIDEO_PIXELS,
        });
    }
    if metadata.codec == VideoCodec::Av1 {
        if metadata.container == MediaContainer::Matroska {
            return Eligibility::Skip(SkipReason::AlreadyAv1Matroska);
        }
        if operation == Operation::Convert {
            return Eligibility::Remux;
        }
    }
    Eligibility::Process
}

#[must_use]
pub fn select_analysis(
    record: &FileRecord,
    execution: &ExecutionSettings,
) -> Option<AnalysisResult> {
    let analyses = record.analyses.get(&execution.profile)?;
    analyses
        .range(execution.requested_target..)
        .find_map(|(_, result)| {
            result
                .validate_reusable_for(execution)
                .is_ok()
                .then(|| result.clone())
        })
        .or_else(|| {
            analyses.values().rev().find_map(|result| {
                let same_fallback_request = result.requested_target == execution.requested_target
                    && result.fallback_floor == execution.fallback_floor
                    && result.fallback_step == execution.fallback_step;
                (same_fallback_request && result.validate_reusable_for(execution).is_ok())
                    .then(|| result.clone())
            })
        })
}

/// Whether a decided verdict still describes the file that would be enqueued,
/// without ffprobe.
///
/// After a replace-mode conversion settles, `state.paths` still holds the OLD
/// input binding (only `MediaObserved` writes path bindings, and core cannot
/// hash paths), so recognition of the output-at-the-input-path CANNOT come
/// from the binding. It comes from the settled output identity instead:
/// `settled_output` is the final identity from
/// `outputs[verdict.source_run].state` (Committed or Retired), and
/// `current_identity` is a cheap stat-derived identity of the file now at the
/// candidate path.
///
/// - `Converted`/`Remuxed` apply while the file on disk IS the produced
///   output: engine-confirmed reliable time plus exact file id, size, and
///   known modification time equality against the settled identity. A
///   missing file, a changed file, or
///   a pruned/unsettled transaction means the verdict no longer answers for
///   the path.
/// - `NotWorthwhile` is a judgment about input content. Callers resolve the
///   record by `ContentKey`, so content match is already established; the
///   verdict applies regardless of path state. Whether its targets satisfy a
///   NEW request is `select_analysis`/target policy, not freshness.
///
/// An adopted verdict (`source_run: None`) has no transaction to resolve, so
/// callers pass `settled_output: None`: its Converted form never applies
/// (the imported output was never content-hashed, so the file on disk cannot
/// be proven to be it), while NotWorthwhile applies by content identity as
/// usual.
#[must_use]
pub fn verdict_applies(
    verdict: &Verdict,
    settled_output: Option<&DestructiveIdentity>,
    current_identity: Option<&DestructiveIdentity>,
    timestamp_reliability: TimestampReliability,
) -> bool {
    match &verdict.kind {
        VerdictKind::Converted { .. } | VerdictKind::Remuxed { .. } => {
            if timestamp_reliability != TimestampReliability::Reliable {
                return false;
            }
            let (Some(settled), Some(current)) = (settled_output, current_identity) else {
                return false;
            };
            current.modified_ns.is_some() && current == settled
        }
        VerdictKind::NotWorthwhile { .. } => true,
    }
}

/// What becomes of a parked import record once its file is actually
/// observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParkedResolution {
    /// The record describes this content. `verdict: None` for records that
    /// decided nothing (scanned/analyzed) — the file still gains import
    /// provenance and the parked entry retires.
    Adopt { verdict: Option<Verdict> },
    /// The record no longer describes the file at its path.
    Retire,
}

/// Decide a parked import record against a fresh observation of the file at
/// the path the record was keyed under:
///
/// - Stamp match (equal size, mtime within [`IMPORT_MTIME_TOLERANCE_NS`];
///   a record missing either stamp half never matches): the record describes
///   the observed content — decisive statuses adopt their verdict, scanned/
///   analyzed adopt provenance only.
/// - Stamp mismatch, status Converted, observed codec AV1: the replace-mode
///   case — the file now at the path IS the conversion's output, so the
///   Converted verdict adopts onto the observed (output) content.
/// - Anything else: the content changed; the imported claim retires.
///
/// Adopted verdicts carry `source_run: None` and whatever summary fields the
/// imported record supplied. Missing NotWorthwhile targets fall back to
/// [`DEFAULT_VMAF_TARGET`] / [`MIN_VMAF_FALLBACK_TARGET`].
#[must_use]
pub fn resolve_parked(
    parked: &ImportedHistoryRecord,
    observation: &MediaObservation,
) -> ParkedResolution {
    if parked_stamp_matches(parked, &observation.binding.stamp()) {
        let verdict = match parked.status {
            ParkedStatus::Converted => Some(converted_verdict(parked)),
            ParkedStatus::NotWorthwhile => Some(not_worthwhile_verdict(parked)),
            ParkedStatus::Scanned | ParkedStatus::Analyzed => None,
        };
        return ParkedResolution::Adopt { verdict };
    }
    if parked.status == ParkedStatus::Converted && observation.metadata.codec == VideoCodec::Av1 {
        return ParkedResolution::Adopt {
            verdict: Some(converted_verdict(parked)),
        };
    }
    ParkedResolution::Retire
}

fn parked_stamp_matches(parked: &ImportedHistoryRecord, current: &FileStamp) -> bool {
    let (Some(size), Some(modified)) = (parked.size, parked.modified_ns) else {
        return false;
    };
    if size != current.size {
        return false;
    }
    current
        .modified_ns
        .is_some_and(|now| now.0.abs_diff(modified.0) <= IMPORT_MTIME_TOLERANCE_NS)
}

fn converted_verdict(parked: &ImportedHistoryRecord) -> Verdict {
    Verdict {
        kind: VerdictKind::Converted {
            output_content_key: None,
            input_size: parked.size,
            output_size: parked.output_size,
            encoding_time: parked.encoding_time,
            crf: parked.crf,
            vmaf: parked.vmaf,
            target: parked.target,
        },
        source_run: None,
        decided_at: parked.decided_at,
    }
}

fn not_worthwhile_verdict(parked: &ImportedHistoryRecord) -> Verdict {
    Verdict {
        kind: VerdictKind::NotWorthwhile {
            requested: parked.requested_target.unwrap_or(DEFAULT_VMAF_TARGET),
            floor: parked.floor_target.unwrap_or(MIN_VMAF_FALLBACK_TARGET),
        },
        source_run: None,
        decided_at: parked.decided_at,
    }
}

/// The final artifact identity of `run_id`'s successfully settled output
/// transaction, if any. `Committed` covers keep-original settlements,
/// `Retired` covers replace-mode; every other state means the verdict has no
/// artifact to answer for.
#[must_use]
pub fn settled_output_identity(
    durable: &DurableState,
    run_id: RunId,
) -> Option<&DestructiveIdentity> {
    match &durable.outputs.get(&run_id)?.state {
        crate::OutputState::Committed { final_identity }
        | crate::OutputState::Retired { final_identity } => Some(&final_identity.destructive),
        _ => None,
    }
}

/// Disposition of one add request, decided before a queue item exists.
/// `None` accepts; `Some(reason)` counts into the add summary and never
/// creates an item. All I/O facts (`path_hash`, `identity`, timestamp
/// reliability) arrive as command payload — core cannot stat, hash paths, or
/// consult a clock.
///
/// Deliberate divergences from the V2 app (ADR-013):
/// - Analyze adds are never filtered on cached analyses: claim-time reuse
///   makes a redundant Analyze near-instant, and enqueue-time profile
///   matching would duplicate `select_analysis` on weaker facts.
/// - `AnalysisIntent::Refresh` bypasses every verdict-based skip — the
///   explicit "do it again" escape hatch. Media-fact skips (resolution,
///   already-AV1) still apply: refreshing cannot make a file eligible.
#[must_use]
pub fn evaluate_enqueue(
    durable: &DurableState,
    path_hash: &PathHash,
    identity: Option<&DestructiveIdentity>,
    timestamp_reliability: TimestampReliability,
    operation: Operation,
    intent: AnalysisIntent,
) -> Option<SkipReason> {
    let binding = durable.paths.get(path_hash)?;
    let record = durable.records.get(&binding.content_key)?;

    // Recognize the replace-mode output at the input path first. The binding
    // is stale by construction there — it still names the ORIGINAL content —
    // so this check must not require binding freshness; `verdict_applies`
    // matches the live destructive identity against the settled output instead.
    if intent == AnalysisIntent::ReuseIfFresh
        && let Some(verdict) = &record.verdict
        && matches!(
            verdict.kind,
            VerdictKind::Converted { .. } | VerdictKind::Remuxed { .. }
        )
        && verdict_applies(
            verdict,
            verdict
                .source_run
                .and_then(|run| settled_output_identity(durable, run)),
            identity,
            timestamp_reliability,
        )
    {
        return Some(SkipReason::AlreadyConverted {
            source_run: verdict.source_run,
        });
    }

    // Every remaining judgment rides on cached facts about the path's
    // content, so it needs a fresh binding: the destructive identity taken now
    // must equal the identity under which the content key was recorded.
    if timestamp_reliability != TimestampReliability::Reliable
        || !identity
            .is_some_and(|current| current.modified_ns.is_some() && current == &binding.identity)
    {
        return None;
    }
    if intent == AnalysisIntent::ReuseIfFresh
        && operation == Operation::Convert
        && let Some(verdict) = &record.verdict
    {
        match verdict.kind {
            // A fresh binding establishes content identity, so a decisive
            // verdict about this content stands: converting it again is
            // duplicate work wherever the produced artifact lives now.
            VerdictKind::Converted { .. } | VerdictKind::Remuxed { .. } => {
                return Some(SkipReason::ProbableDuplicate {
                    source_run: verdict.source_run,
                });
            }
            VerdictKind::NotWorthwhile { .. } => {
                // Execution targets are injected at claim, not add, so the
                // enqueue tier cannot compare fallback floors; it defers to
                // the standing judgment and leaves `Refresh` as the override.
                return Some(SkipReason::NotWorthwhile {
                    source_run: verdict.source_run,
                });
            }
        }
    }
    match evaluate_eligibility(&record.metadata, operation) {
        Eligibility::Skip(reason) => Some(reason),
        Eligibility::Process | Eligibility::Remux => None,
    }
}

/// The analysis profiles a run may durably record: the prepared profile
/// itself, plus its software-decode variant when the prepared profile decodes
/// in hardware. This is the gate behind the hardware→software retry ladder —
/// a search that fails under hardware decode retries once with software and
/// records under the profile it actually ran with, while the requested
/// `JobSpec` is never rewritten. A software-prepared run has no wider ladder.
#[must_use]
pub fn permitted_profiles(execution: &ExecutionSettings) -> Vec<AnalysisProfile> {
    let mut profiles = vec![execution.profile.clone()];
    if matches!(execution.profile.decode_mode, DecodeMode::Hardware(_)) {
        let mut software = execution.profile.clone();
        software.decode_mode = DecodeMode::Software;
        profiles.push(software);
    }
    profiles
}

#[must_use]
pub fn select_job_action(
    metadata: Option<&VideoMeta>,
    record: Option<&FileRecord>,
    operation: Operation,
    intent: AnalysisIntent,
    execution: &ExecutionSettings,
) -> JobAction {
    let eligibility = metadata.map(|metadata| evaluate_eligibility(metadata, operation));
    if let Some(Eligibility::Skip(reason)) = eligibility {
        return JobAction::Skip { reason };
    }

    // Content identity is authoritative at claim: the record was resolved by
    // the live observation's ContentKey, so a decisive verdict about this
    // content stands regardless of where the bytes now live. This catches
    // cross-path content copies and verdicts that became decisive after
    // enqueue; `Refresh` is the explicit escape hatch. It outranks the remux
    // branch — re-remuxing already-processed content is still duplicate work.
    if intent == AnalysisIntent::ReuseIfFresh
        && operation == Operation::Convert
        && let Some(verdict) = record.and_then(|known| known.verdict.as_ref())
    {
        match &verdict.kind {
            VerdictKind::Converted { .. } | VerdictKind::Remuxed { .. } => {
                return JobAction::Skip {
                    reason: SkipReason::ProbableDuplicate {
                        source_run: verdict.source_run,
                    },
                };
            }
            VerdictKind::NotWorthwhile { floor, .. } => {
                // Failure at the floor implies failure at every higher
                // target (higher VMAF → larger file), so the verdict decides
                // any request whose ladder stops at or above it. A lower
                // floor is untried ground and proceeds.
                if execution.fallback_floor >= *floor {
                    return JobAction::Skip {
                        reason: SkipReason::NotWorthwhile {
                            source_run: verdict.source_run,
                        },
                    };
                }
            }
        }
    }

    if matches!(eligibility, Some(Eligibility::Remux)) {
        return JobAction::Remux;
    }

    let selected_analysis = match intent {
        AnalysisIntent::ReuseIfFresh => record
            .and_then(|known| select_analysis(known, execution))
            .map(Box::new),
        AnalysisIntent::Refresh => None,
    };
    match operation {
        Operation::Analyze => JobAction::Analyze { selected_analysis },
        Operation::Convert => JobAction::Encode { selected_analysis },
    }
}
