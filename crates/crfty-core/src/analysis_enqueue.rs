//! Plan queue actions from one standing generation without filesystem reads
//! or historical estimates. Native paths are supplied by the engine registry.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::{
    AnalysisFileScan, AnalysisGenerationId, AnalysisIntent, AnalysisRowEntry, AnalysisRowId,
    AppState, DefaultOutputMode, Operation, OutputTarget, OverwriteDecision, QueueAddRequest,
    RowEligibility, SkipReason, TimestampReliability,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum AnalysisQueueSelection {
    AllFiles,
    Selected { row_ids: Vec<AnalysisRowId> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AnalysisQueueRequest {
    pub generation: AnalysisGenerationId,
    pub selection: AnalysisQueueSelection,
    pub operation: Operation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub enum AnalysisQueueExclusion {
    Folder,
    DuplicateIdentity,
    ScanFailed,
    Policy { reason: SkipReason },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
pub struct AnalysisQueueSummary {
    pub added: u32,
    pub excluded: Vec<(AnalysisQueueExclusion, u32)>,
}

/// Operational spellings never come from Analysis display strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisQueueFile {
    pub row_id: AnalysisRowId,
    pub input: PathBuf,
    pub source_root: PathBuf,
}

pub(crate) struct AnalysisQueuePlan {
    pub requests: Vec<QueueAddRequest>,
    pub summary: AnalysisQueueSummary,
}

impl AnalysisQueueSummary {
    pub(crate) fn exclude(&mut self, reason: AnalysisQueueExclusion) {
        self.exclude_many(reason, 1);
    }

    pub(crate) fn exclude_many(&mut self, reason: AnalysisQueueExclusion, count: u32) {
        if let Some((_, existing)) = self.excluded.iter_mut().find(|(key, _)| *key == reason) {
            *existing = existing.saturating_add(count);
        } else {
            self.excluded.push((reason, count));
        }
    }
}

pub(crate) fn plan_analysis_enqueue(
    state: &AppState,
    request: AnalysisQueueRequest,
    files: Vec<AnalysisQueueFile>,
) -> Result<AnalysisQueuePlan, &'static str> {
    let current = state
        .analysis
        .current
        .as_ref()
        .filter(|current| current.id == request.generation)
        .ok_or("queue action names a stale Analysis generation")?;
    if !current.activity.is_live() {
        return Err("queue action names an inactive Analysis generation");
    }
    let output = &state.settings.output;
    let output_template = match output.default_mode {
        DefaultOutputMode::Replace => OutputTarget::Replace,
        DefaultOutputMode::Suffix => OutputTarget::Suffix {
            suffix: output.suffix.clone(),
        },
        DefaultOutputMode::SeparateFolder => OutputTarget::SeparateFolder {
            directory: output
                .separate_folder
                .clone()
                .ok_or("default separate output folder is required")?,
            source_root: None,
        },
    };
    let mut summary = AnalysisQueueSummary::default();
    let selected = match request.selection {
        AnalysisQueueSelection::AllFiles => None,
        AnalysisQueueSelection::Selected { row_ids } => {
            let mut selected = BTreeSet::new();
            for id in row_ids {
                if !current.rows.contains_key(&id) {
                    return Err("queue action names an unknown Analysis row");
                }
                if !selected.insert(id) {
                    summary.exclude(AnalysisQueueExclusion::DuplicateIdentity);
                }
            }
            Some(selected)
        }
    };
    let files: BTreeMap<_, _> = files.into_iter().map(|file| (file.row_id, file)).collect();
    let mut paths = BTreeSet::new();
    let mut content = BTreeSet::new();
    let mut requests = Vec::new();
    for row in current.rows.values() {
        if selected.as_ref().is_some_and(|ids| !ids.contains(&row.id)) {
            continue;
        }
        let AnalysisRowEntry::File { scan } = &row.entry else {
            if selected.is_some() {
                summary.exclude(AnalysisQueueExclusion::Folder);
            }
            continue;
        };
        if matches!(
            scan,
            AnalysisFileScan::Failed { .. }
                | AnalysisFileScan::Scanned {
                    refresh_failure: Some(_),
                    ..
                }
                | AnalysisFileScan::SettledOutput {
                    refresh_failure: Some(_),
                    ..
                }
        ) {
            summary.exclude(AnalysisQueueExclusion::ScanFailed);
            continue;
        }
        let file = files
            .get(&row.id)
            .ok_or("Analysis native path is unavailable")?;
        let key = match scan {
            AnalysisFileScan::Scanned { content_key, .. } => Some(content_key),
            AnalysisFileScan::SettledOutput {
                output_content_key, ..
            } => Some(output_content_key),
            AnalysisFileScan::Discovered | AnalysisFileScan::Failed { .. } => None,
        };
        if !paths.insert(&file.input) || key.is_some_and(|key| !content.insert(key)) {
            summary.exclude(AnalysisQueueExclusion::DuplicateIdentity);
            continue;
        }
        if let Some(status) = scan.status() {
            let eligibility = match request.operation {
                Operation::Analyze => &status.analyze,
                Operation::Convert => &status.convert,
            };
            if let RowEligibility::Skip { reason } = eligibility {
                summary.exclude(AnalysisQueueExclusion::Policy {
                    reason: reason.clone(),
                });
                continue;
            }
        }
        let mut output_target = output_template.clone();
        if let OutputTarget::SeparateFolder { source_root, .. } = &mut output_target {
            *source_root = Some(file.source_root.clone());
        }
        requests.push(QueueAddRequest {
            input: file.input.clone(),
            path_hash: None,
            identity: None,
            timestamp_reliability: TimestampReliability::Unknown,
            operation: request.operation,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target,
            overwrite: OverwriteDecision::FollowSettings,
        });
    }
    Ok(AnalysisQueuePlan { requests, summary })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnalysisActivity, AnalysisCommand, AnalysisDisplayText, AnalysisGeneration, AnalysisRow,
        AnalysisRowStatus, AnalysisScanFailure, AnalysisSnapshot, ApplicableLevel, Command,
        ContentKey, EphemeralDelta, HistoricalLevel, JournalSequence, QueueCommand, QueueItemId,
        Reply, ReuseStanding, SettingsCommand, UnixMillis, VideoCodec, apply, encode_record,
        encode_snapshot, replay,
    };

    fn file(id: u64) -> AnalysisQueueFile {
        AnalysisQueueFile {
            row_id: AnalysisRowId(id),
            input: PathBuf::from(format!("native/movie-{id}.mkv")),
            source_root: PathBuf::from("native"),
        }
    }

    fn row(id: u64, scan: AnalysisFileScan) -> AnalysisRow {
        AnalysisRow {
            id: AnalysisRowId(id),
            parent: None,
            entry: AnalysisRowEntry::File { scan },
            display_name: AnalysisDisplayText {
                text: "display only".to_owned(),
                lossy: true,
            },
            display_path: AnalysisDisplayText {
                text: "never/use/this.mkv".to_owned(),
                lossy: true,
            },
        }
    }

    fn status(historical: HistoricalLevel, eligibility: RowEligibility) -> Box<AnalysisRowStatus> {
        Box::new(AnalysisRowStatus {
            applicable: ApplicableLevel::Scanned {
                reuse: ReuseStanding::Unanalyzed,
            },
            historical,
            analyze: eligibility.clone(),
            convert: eligibility,
        })
    }

    fn scanned(
        key: &str,
        historical: HistoricalLevel,
        eligibility: RowEligibility,
    ) -> AnalysisFileScan {
        AnalysisFileScan::Scanned {
            content_key: ContentKey(key.to_owned()),
            metadata: crate::test_support::meta(VideoCodec::H264, 100),
            refresh_failure: None,
            status: status(historical, eligibility),
        }
    }

    fn settled(output_key: &str) -> AnalysisFileScan {
        AnalysisFileScan::SettledOutput {
            source_content_key: ContentKey("source".to_owned()),
            output_content_key: ContentKey(output_key.to_owned()),
            metadata: None,
            refresh_failure: None,
            status: status(HistoricalLevel::Converted, RowEligibility::Eligible),
        }
    }

    fn state(rows: Vec<AnalysisRow>) -> AppState {
        AppState {
            analysis: AnalysisSnapshot {
                current: Some(AnalysisGeneration {
                    id: AnalysisGenerationId(1),
                    roots: vec![AnalysisDisplayText {
                        text: "root".to_owned(),
                        lossy: false,
                    }],
                    activity: AnalysisActivity::Discovering,
                    rows: rows.into_iter().map(|row| (row.id, row)).collect(),
                }),
            },
            ..AppState::default()
        }
    }

    fn request(selection: AnalysisQueueSelection, operation: Operation) -> AnalysisQueueRequest {
        AnalysisQueueRequest {
            generation: AnalysisGenerationId(1),
            selection,
            operation,
        }
    }

    fn enqueue(
        state: &mut AppState,
        selection: AnalysisQueueSelection,
        files: Vec<AnalysisQueueFile>,
    ) -> crate::Applied {
        apply(
            state,
            Command::Analysis(AnalysisCommand::Enqueue {
                request: request(selection, Operation::Convert),
                files,
            }),
        )
    }

    #[test]
    fn empty_selected_and_bulk_actions_are_noops_even_with_history_only_inputs() {
        for selection in [
            AnalysisQueueSelection::AllFiles,
            AnalysisQueueSelection::Selected {
                row_ids: Vec::new(),
            },
        ] {
            let mut state = state(Vec::new());
            state.durable.records.insert(
                ContentKey("history-only".to_owned()),
                crate::FileRecord::new(crate::test_support::meta(VideoCodec::H264, 100)),
            );
            let applied = enqueue(&mut state, selection, Vec::new());
            assert_eq!(
                applied.reply,
                Reply::AnalysisQueued(AnalysisQueueSummary::default())
            );
            assert!(applied.durable.is_empty());
            assert_eq!(state.durable.queue_item_id_high_water, 0);
        }
    }

    #[test]
    fn partial_generations_enqueue_only_captured_files_in_row_order() {
        let rows = vec![
            row(3, AnalysisFileScan::Discovered),
            row(1, AnalysisFileScan::Discovered),
        ];
        let mut state = state(rows);
        let files = vec![file(3), file(2), file(1)];
        let applied = enqueue(&mut state, AnalysisQueueSelection::AllFiles, files);
        assert_eq!(
            applied.reply,
            Reply::AnalysisQueued(AnalysisQueueSummary {
                added: 2,
                excluded: Vec::new()
            })
        );
        assert_eq!(
            state
                .durable
                .queue
                .iter()
                .map(|item| (&item.input, item.id))
                .collect::<Vec<_>>(),
            vec![
                (&file(1).input, QueueItemId(1)),
                (&file(3).input, QueueItemId(2))
            ]
        );
    }

    #[test]
    fn selected_order_and_duplicates_do_not_change_request_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state(vec![
            row(1, AnalysisFileScan::Discovered),
            row(2, AnalysisFileScan::Discovered),
        ]);
        let selection = |ids: Vec<u64>| AnalysisQueueSelection::Selected {
            row_ids: ids.into_iter().map(AnalysisRowId).collect(),
        };
        let first = plan_analysis_enqueue(
            &state,
            request(selection(vec![2, 1, 2]), Operation::Analyze),
            vec![file(2), file(1)],
        )?;
        let second = plan_analysis_enqueue(
            &state,
            request(selection(vec![1, 2, 2]), Operation::Analyze),
            vec![file(1), file(2)],
        )?;
        assert_eq!(first.requests, second.requests);
        assert_eq!(first.summary, second.summary);
        assert_eq!(
            first.summary.excluded,
            vec![(AnalysisQueueExclusion::DuplicateIdentity, 1)]
        );
        Ok(())
    }

    #[test]
    fn native_paths_and_observed_content_deduplicate_aliases() {
        let mut state = state(vec![
            row(
                1,
                scanned("same", HistoricalLevel::Scanned, RowEligibility::Eligible),
            ),
            row(
                2,
                scanned("same", HistoricalLevel::Scanned, RowEligibility::Eligible),
            ),
            row(3, AnalysisFileScan::Discovered),
            row(4, AnalysisFileScan::Discovered),
        ]);
        let mut alias = file(4);
        alias.input = file(3).input;
        let applied = enqueue(
            &mut state,
            AnalysisQueueSelection::AllFiles,
            vec![file(1), file(2), file(3), alias],
        );
        assert_eq!(
            applied.reply,
            Reply::AnalysisQueued(AnalysisQueueSummary {
                added: 2,
                excluded: vec![(AnalysisQueueExclusion::DuplicateIdentity, 2)],
            })
        );
    }

    #[test]
    fn exclusions_preserve_operational_policy_reasons() {
        let skip = SkipReason::AlreadyAv1Matroska;
        let mut folder = row(1, AnalysisFileScan::Discovered);
        folder.entry = AnalysisRowEntry::Folder { failure: None };
        let mut state = state(vec![
            folder,
            row(
                2,
                AnalysisFileScan::Failed {
                    failure: AnalysisScanFailure::Missing,
                },
            ),
            row(
                3,
                scanned(
                    "skip",
                    HistoricalLevel::Scanned,
                    RowEligibility::Skip {
                        reason: skip.clone(),
                    },
                ),
            ),
            row(4, AnalysisFileScan::Discovered),
        ]);
        enqueue(
            &mut state,
            AnalysisQueueSelection::Selected {
                row_ids: vec![AnalysisRowId(4)],
            },
            vec![file(4)],
        );
        let applied = enqueue(
            &mut state,
            AnalysisQueueSelection::Selected {
                row_ids: (1..=4).map(AnalysisRowId).collect(),
            },
            vec![file(3), file(4)],
        );
        assert_eq!(
            applied.reply,
            Reply::AnalysisQueued(AnalysisQueueSummary {
                added: 0,
                excluded: vec![
                    (AnalysisQueueExclusion::Folder, 1),
                    (AnalysisQueueExclusion::ScanFailed, 1),
                    (AnalysisQueueExclusion::Policy { reason: skip }, 1),
                    (
                        AnalysisQueueExclusion::Policy {
                            reason: SkipReason::AlreadyQueued
                        },
                        1
                    )
                ],
            })
        );
    }

    #[test]
    fn historical_level_never_prevents_analyze_or_convert() {
        for operation in [Operation::Analyze, Operation::Convert] {
            for historical in [
                HistoricalLevel::Scanned,
                HistoricalLevel::Analyzed,
                HistoricalLevel::Converted,
            ] {
                let mut state = state(vec![row(
                    1,
                    scanned("history", historical, RowEligibility::Eligible),
                )]);
                let applied = apply(
                    &mut state,
                    Command::Analysis(AnalysisCommand::Enqueue {
                        request: request(AnalysisQueueSelection::AllFiles, operation),
                        files: vec![file(1)],
                    }),
                );
                assert_eq!(
                    applied.reply,
                    Reply::AnalysisQueued(AnalysisQueueSummary {
                        added: 1,
                        excluded: Vec::new()
                    })
                );
                assert_eq!(
                    state.durable.queue.first().map(|item| item.operation),
                    Some(operation)
                );
            }
        }
    }

    #[test]
    fn stale_unknown_and_superseded_actions_do_not_mutate_the_queue() {
        let mut state = state(vec![row(1, AnalysisFileScan::Discovered)]);
        let before = state.durable.clone();
        let unknown = enqueue(
            &mut state,
            AnalysisQueueSelection::Selected {
                row_ids: vec![AnalysisRowId(9)],
            },
            vec![file(1)],
        );
        assert!(matches!(unknown.reply, Reply::Rejected { .. }));
        let captured = Command::Analysis(AnalysisCommand::Enqueue {
            request: request(AnalysisQueueSelection::AllFiles, Operation::Analyze),
            files: vec![file(1)],
        });
        apply(
            &mut state,
            Command::Analysis(AnalysisCommand::Begin {
                roots: vec![AnalysisDisplayText {
                    text: "new root".to_owned(),
                    lossy: false,
                }],
            }),
        );
        let stale = apply(&mut state, captured);
        assert_eq!(
            stale.reply,
            Reply::Rejected {
                reason: "queue action names a stale Analysis generation".to_owned()
            }
        );
        assert!(stale.durable.is_empty());
        assert_eq!(state.durable, before);
    }

    #[test]
    fn ended_generations_cannot_enqueue_frozen_operational_statuses() {
        for activity in [
            AnalysisActivity::Cancelled,
            AnalysisActivity::Failed {
                detail: "fixture".to_owned(),
            },
        ] {
            let mut state = state(vec![row(1, AnalysisFileScan::Discovered)]);
            if let Some(current) = &mut state.analysis.current {
                current.activity = activity;
            }
            let applied = enqueue(&mut state, AnalysisQueueSelection::AllFiles, vec![file(1)]);
            assert_eq!(
                applied.reply,
                Reply::Rejected {
                    reason: "queue action names an inactive Analysis generation".to_owned()
                }
            );
            assert!(applied.durable.is_empty());
        }
    }

    #[test]
    fn committed_output_settings_are_resolved_at_enqueue() {
        let mut state = state(vec![row(1, AnalysisFileScan::Discovered)]);
        let mut settings = state.settings.clone();
        settings.output.default_mode = DefaultOutputMode::SeparateFolder;
        settings.output.separate_folder = Some(PathBuf::from("output"));
        let changed = apply(
            &mut state,
            Command::Settings(SettingsCommand::Set { settings }),
        );
        assert_eq!(changed.reply, Reply::Accepted);
        enqueue(&mut state, AnalysisQueueSelection::AllFiles, vec![file(1)]);
        assert_eq!(
            state.durable.queue.first().map(|item| &item.output_target),
            Some(&OutputTarget::SeparateFolder {
                directory: PathBuf::from("output"),
                source_root: Some(PathBuf::from("native")),
            })
        );
    }

    #[test]
    fn removed_ids_survive_replay_and_snapshot_compaction() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut state = state(vec![row(1, AnalysisFileScan::Discovered)]);
        let added = enqueue(&mut state, AnalysisQueueSelection::AllFiles, vec![file(1)]);
        let removed = apply(&mut state, Command::Queue(QueueCommand::Clear));
        let mut bytes = encode_record(&crate::JournalEnvelope {
            sequence: JournalSequence(0),
            deltas: added.durable,
        })?;
        bytes.extend(encode_record(&crate::JournalEnvelope {
            sequence: JournalSequence(1),
            deltas: removed.durable,
        })?);
        let journal = replay(&bytes);
        assert!(journal.corruption.is_none());
        assert!(journal.state.queue.is_empty());
        assert_eq!(journal.state.queue_item_id_high_water, 1);
        let compacted = encode_snapshot("test", UnixMillis(0), JournalSequence(2), &journal.state)?;
        state.durable = replay(&compacted).state;
        let next = enqueue(&mut state, AnalysisQueueSelection::AllFiles, vec![file(1)]);
        assert_eq!(
            state.durable.queue.first().map(|item| item.id),
            Some(QueueItemId(2))
        );
        assert!(matches!(next.reply, Reply::AnalysisQueued(_)));
        Ok(())
    }

    #[test]
    fn id_exhaustion_rejects_the_entire_action() {
        let mut state = state(vec![
            row(1, AnalysisFileScan::Discovered),
            row(2, AnalysisFileScan::Discovered),
        ]);
        state.durable.queue_item_id_high_water = crate::DurableState::MAX_RUNTIME_ID - 1;
        let before = state.durable.clone();
        let applied = enqueue(
            &mut state,
            AnalysisQueueSelection::AllFiles,
            vec![file(1), file(2)],
        );
        assert_eq!(
            applied.reply,
            Reply::Rejected {
                reason: "queue item id space is exhausted".to_owned()
            }
        );
        assert!(applied.durable.is_empty());
        assert!(
            !applied
                .ephemeral
                .iter()
                .any(|delta| matches!(delta, EphemeralDelta::QueueAddSummary { .. }))
        );
        assert_eq!(state.durable, before);
    }

    #[test]
    fn replay_rejects_reused_gapped_zero_and_inexact_queue_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut state = state(vec![row(1, AnalysisFileScan::Discovered)]);
        let added = enqueue(&mut state, AnalysisQueueSelection::AllFiles, vec![file(1)]);
        apply(&mut state, Command::Queue(QueueCommand::Clear));
        let Some(crate::DurableDelta::QueueAdded { item }) = added.durable.first() else {
            return Err("expected an added item".into());
        };
        let snapshot = encode_snapshot("test", UnixMillis(0), JournalSequence(0), &state.durable)?;
        for id in [0, 1, 3, crate::DurableState::MAX_RUNTIME_ID + 1] {
            let mut item = item.clone();
            item.id = QueueItemId(id);
            let mut bytes = snapshot.clone();
            bytes.extend(encode_record(&crate::JournalEnvelope {
                sequence: JournalSequence(0),
                deltas: vec![crate::DurableDelta::QueueAdded { item }],
            })?);
            let recovered = replay(&bytes);
            assert!(recovered.corruption.is_some(), "accepted invalid ID {id}");
            assert_eq!(recovered.state, state.durable);
        }
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn paths_that_cannot_round_trip_through_json_are_counted() {
        let mut state = state(vec![row(1, AnalysisFileScan::Discovered)]);
        let mut native = file(1);
        #[cfg(unix)]
        let unsupported = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![0xff])
        };
        #[cfg(windows)]
        let unsupported = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0xd800])
        };
        native.input = unsupported.into();
        let applied = enqueue(&mut state, AnalysisQueueSelection::AllFiles, vec![native]);
        assert_eq!(
            applied.reply,
            Reply::AnalysisQueued(AnalysisQueueSummary {
                added: 0,
                excluded: vec![(
                    AnalysisQueueExclusion::Policy {
                        reason: SkipReason::PathNotPersistable
                    },
                    1
                )],
            })
        );
        assert!(applied.durable.is_empty());
    }

    #[test]
    fn settled_outputs_deduplicate_by_output_content() {
        let mut state = state(vec![
            row(1, settled("output")),
            row(2, settled("output")),
            row(3, settled("other")),
        ]);
        let applied = enqueue(
            &mut state,
            AnalysisQueueSelection::AllFiles,
            vec![file(1), file(2), file(3)],
        );
        assert_eq!(
            applied.reply,
            Reply::AnalysisQueued(AnalysisQueueSummary {
                added: 2,
                excluded: vec![(AnalysisQueueExclusion::DuplicateIdentity, 1)],
            })
        );
    }

    #[test]
    fn refresh_failures_count_as_scan_failures() {
        let failure = Some(Box::new(AnalysisScanFailure::Missing));
        let mut stale_scan = scanned("stale", HistoricalLevel::Scanned, RowEligibility::Eligible);
        let mut stale_output = settled("output");
        for scan in [&mut stale_scan, &mut stale_output] {
            if let AnalysisFileScan::Scanned {
                refresh_failure, ..
            }
            | AnalysisFileScan::SettledOutput {
                refresh_failure, ..
            } = scan
            {
                *refresh_failure = failure.clone();
            }
        }
        let mut state = state(vec![row(1, stale_scan), row(2, stale_output)]);
        let applied = enqueue(
            &mut state,
            AnalysisQueueSelection::AllFiles,
            vec![file(1), file(2)],
        );
        assert_eq!(
            applied.reply,
            Reply::AnalysisQueued(AnalysisQueueSummary {
                added: 0,
                excluded: vec![(AnalysisQueueExclusion::ScanFailed, 2)],
            })
        );
    }

    #[test]
    fn bulk_actions_skip_folders_without_counting_them() {
        let mut folder = row(1, AnalysisFileScan::Discovered);
        folder.entry = AnalysisRowEntry::Folder { failure: None };
        let mut state = state(vec![folder, row(2, AnalysisFileScan::Discovered)]);
        let applied = enqueue(&mut state, AnalysisQueueSelection::AllFiles, vec![file(2)]);
        assert_eq!(
            applied.reply,
            Reply::AnalysisQueued(AnalysisQueueSummary {
                added: 1,
                excluded: Vec::new(),
            })
        );
    }

    #[test]
    fn separate_folder_mode_without_a_folder_rejects_before_planning_rows() {
        let mut state = state(Vec::new());
        state.settings.output.default_mode = DefaultOutputMode::SeparateFolder;
        state.settings.output.separate_folder = None;
        let applied = enqueue(&mut state, AnalysisQueueSelection::AllFiles, Vec::new());
        assert_eq!(
            applied.reply,
            Reply::Rejected {
                reason: "default separate output folder is required".to_owned()
            }
        );
    }

    #[test]
    fn the_typed_reply_is_the_only_summary() {
        let mut state = state(vec![row(1, AnalysisFileScan::Discovered)]);
        let applied = enqueue(&mut state, AnalysisQueueSelection::AllFiles, vec![file(1)]);
        assert!(matches!(applied.reply, Reply::AnalysisQueued(_)));
        assert!(
            !applied
                .ephemeral
                .iter()
                .any(|delta| matches!(delta, EphemeralDelta::QueueAddSummary { .. }))
        );
    }
}
