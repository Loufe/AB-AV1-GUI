//! Regenerates the checked-in golden fold fixtures replayed by the frontend
//! (`ui/src/lib/store/fold.test.ts`), proving the TypeScript fold mirrors
//! `crfty_core::fold` (#33 §14). Every scenario's `initial` and `expected`
//! states are computed by the Rust fold itself — the oracle — and serialized
//! with the same serde shapes the wire uses.
//!
//! CI verifies freshness with
//! `git diff --exit-code -- ui/src/lib/store/fold-fixtures.json` after the
//! test suite runs, so a stale file fails the build (same contract as
//! `export_bindings`).
#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

use crfty_core::{
    AnalysisAttempt, AnalysisProfile, AnalysisResult, ArtifactIdentity, ClaimId, ContentKey, Crf,
    DecodeMode, DecodePreference, DestructiveIdentity, DurableDelta, DurableState,
    ExecutionSettings, FileStamp, FileSystemId, ItemOutcome, JobAction, JobSpec, MediaContainer,
    MediaObservation, Operation, OutputDelta, OutputState, OutputTarget, OutputTransaction,
    PathBinding, PathHash, QueueItem, QueueItemId, QueueItemState, Replacement, ReservedJob, RunId,
    SearchMeasurement, SkipReason, VideoCodec, VideoMeta, VmafScore, VmafTarget, fold,
};
use serde::Serialize;

#[derive(Serialize)]
struct Fixtures {
    _source: &'static str,
    scenarios: Vec<Scenario>,
}

#[derive(Serialize)]
struct Scenario {
    name: &'static str,
    initial: DurableState,
    deltas: Vec<DurableDelta>,
    expected: DurableState,
}

fn scenario(name: &'static str, prelude: Vec<DurableDelta>, deltas: Vec<DurableDelta>) -> Scenario {
    let mut initial = DurableState::default();
    for delta in &prelude {
        fold(&mut initial, delta);
    }
    let mut expected = initial.clone();
    for delta in &deltas {
        fold(&mut expected, delta);
    }
    Scenario {
        name,
        initial,
        deltas,
        expected,
    }
}

fn added(id: u64) -> DurableDelta {
    DurableDelta::QueueAdded {
        item: QueueItem {
            id: QueueItemId(id),
            input: PathBuf::from(format!("videos/input-{id}.mp4")),
            operation: Operation::Convert,
            output_target: OutputTarget::Replace,
            state: QueueItemState::Queued,
        },
    }
}

fn removed(id: u64) -> DurableDelta {
    DurableDelta::QueueRemoved {
        item_id: QueueItemId(id),
    }
}

fn moved(id: u64, before: Option<u64>) -> DurableDelta {
    DurableDelta::QueueMoved {
        item_id: QueueItemId(id),
        before: before.map(QueueItemId),
    }
}

fn reserved(item: u64, claim: u64, run: u64) -> DurableDelta {
    DurableDelta::ItemReserved {
        job: Box::new(ReservedJob {
            item_id: QueueItemId(item),
            claim_id: ClaimId(claim),
            run_id: RunId(run),
            input: PathBuf::from(format!("videos/input-{item}.mp4")),
            operation: Operation::Convert,
            output_target: OutputTarget::Replace,
        }),
    }
}

fn running(item: u64, claim: u64, run: u64) -> DurableDelta {
    DurableDelta::ItemRunning {
        item_id: QueueItemId(item),
        claim_id: ClaimId(claim),
        run_id: RunId(run),
    }
}

fn finished(item: u64, claim: u64, run: u64, outcome: ItemOutcome) -> DurableDelta {
    DurableDelta::ItemFinished {
        item_id: QueueItemId(item),
        claim_id: ClaimId(claim),
        run_id: RunId(run),
        outcome,
    }
}

fn video_meta(duration_ms: u64) -> VideoMeta {
    VideoMeta {
        codec: VideoCodec::H264,
        container: MediaContainer::Other("mp4".to_owned()),
        width: 1920,
        height: 1080,
        rotation_degrees: 0,
        duration_ms,
    }
}

fn observed(path: &str, key: &str, duration_ms: u64) -> DurableDelta {
    DurableDelta::MediaObserved {
        observation: Box::new(MediaObservation {
            path_hash: PathHash(path.to_owned()),
            binding: PathBinding {
                stamp: FileStamp {
                    size: 3_000_000,
                    modified_ns: Some(1_000_000),
                },
                content_key: ContentKey(key.to_owned()),
            },
            metadata: video_meta(duration_ms),
        }),
    }
}

fn profile(preset: u8) -> AnalysisProfile {
    AnalysisProfile {
        preset,
        max_encoded_percent_basis_points: 8_000,
        samples: None,
        sample_duration_ms: 20_000,
        thorough: false,
        decode_mode: DecodeMode::Software,
        ab_av1_revision: "ab-av1-test".to_owned(),
        ffmpeg_revision: "ffmpeg-test".to_owned(),
        encoder_revision: "svt-av1-test".to_owned(),
    }
}

fn measurement(crf: u32) -> SearchMeasurement {
    SearchMeasurement {
        crf: Crf(crf),
        score: VmafScore(9_550),
        predicted_size: 1_000_000,
        predicted_percent_basis_points: 4_200,
        predicted_duration_ms: 60_000,
        from_cache: false,
    }
}

fn analysis(preset: u8, successful_target: u8) -> AnalysisResult {
    AnalysisResult {
        requested_target: VmafTarget(95),
        successful_target: VmafTarget(successful_target),
        fallback_floor: VmafTarget(90),
        fallback_step: 1,
        failed_attempts: Vec::new(),
        measurement: measurement(30),
        profile: profile(preset),
    }
}

fn prepared(item: u64, claim: u64, run: u64, key: Option<&str>, action: JobAction) -> DurableDelta {
    DurableDelta::ItemPrepared {
        spec: Box::new(JobSpec {
            item_id: QueueItemId(item),
            claim_id: ClaimId(claim),
            run_id: RunId(run),
            input: PathBuf::from(format!("videos/input-{item}.mp4")),
            content_key: key.map(|key| ContentKey(key.to_owned())),
            operation: Operation::Convert,
            output_target: OutputTarget::Replace,
            execution: ExecutionSettings {
                requested_target: VmafTarget(95),
                fallback_floor: VmafTarget(90),
                fallback_step: 1,
                overwrite_existing: false,
                decode_preference: DecodePreference::HardwarePreferred,
                profile: profile(6),
            },
            action,
        }),
    }
}

fn recorded(run: u64, result: AnalysisResult) -> DurableDelta {
    DurableDelta::AnalysisRecorded {
        run_id: RunId(run),
        result: Box::new(result),
    }
}

fn destructive(size: u64) -> DestructiveIdentity {
    DestructiveIdentity {
        file_id: FileSystemId::Unix {
            device: 1,
            inode: 42,
        },
        size,
        modified_ns: Some(2_000_000),
    }
}

fn artifact(key: &str) -> ArtifactIdentity {
    ArtifactIdentity {
        content_key: ContentKey(key.to_owned()),
        destructive: destructive(1_000_000),
    }
}

fn output_started(run: u64) -> DurableDelta {
    DurableDelta::Output(OutputDelta::OutputStarted {
        transaction: Box::new(OutputTransaction {
            run_id: RunId(run),
            input: PathBuf::from("videos/input-1.mp4"),
            input_identity: destructive(3_000_000),
            staging: PathBuf::from("videos/.input-1.crfty-staging.mkv"),
            final_path: PathBuf::from("videos/input-1.mkv"),
            final_preimage: None,
            replacement: Replacement::RetireOriginal,
            state: OutputState::Started,
        }),
    })
}

fn output_staging_created(run: u64) -> DurableDelta {
    DurableDelta::Output(OutputDelta::StagingCreated {
        run_id: RunId(run),
        initial: destructive(0),
    })
}

fn output_ready(run: u64, key: &str) -> DurableDelta {
    DurableDelta::Output(OutputDelta::OutputReady {
        run_id: RunId(run),
        staging_identity: artifact(key),
    })
}

fn output_committed(run: u64, key: &str) -> DurableDelta {
    DurableDelta::Output(OutputDelta::OutputCommitted {
        run_id: RunId(run),
        final_identity: artifact(key),
    })
}

fn convert_prelude(key: &str) -> Vec<DurableDelta> {
    vec![
        added(1),
        observed("path-1", key, 120_000),
        reserved(1, 10, 100),
        prepared(
            1,
            10,
            100,
            Some(key),
            JobAction::Encode {
                selected_analysis: None,
            },
        ),
        running(1, 10, 100),
    ]
}

fn scenarios() -> Vec<Scenario> {
    vec![
        scenario("queue_added", vec![], vec![added(1), added(2)]),
        scenario(
            "queue_removed",
            vec![added(1), added(2), added(3)],
            vec![removed(2)],
        ),
        scenario("queue_removed_missing", vec![added(1)], vec![removed(9)]),
        scenario(
            "queue_moved_before",
            vec![added(1), added(2), added(3)],
            vec![moved(3, Some(1))],
        ),
        scenario(
            "queue_moved_to_end",
            vec![added(1), added(2), added(3)],
            vec![moved(1, None)],
        ),
        scenario(
            "queue_moved_before_missing",
            vec![added(1), added(2), added(3)],
            vec![moved(1, Some(9))],
        ),
        scenario(
            "queue_moved_missing_item",
            vec![added(1), added(2)],
            vec![moved(9, Some(1))],
        ),
        scenario(
            "media_observed_new",
            vec![],
            vec![observed("path-1", "ck-1", 120_000)],
        ),
        scenario(
            "media_observed_update_and_alias",
            vec![observed("path-1", "ck-1", 120_000)],
            // A second path binds the same content; the metadata refresh
            // replaces the record's metadata without touching analyses.
            vec![observed("path-2", "ck-1", 121_000)],
        ),
        scenario(
            "item_reserved_then_running",
            vec![added(1)],
            vec![reserved(1, 10, 100), running(1, 10, 100)],
        ),
        scenario(
            "item_prepared_inserts_run",
            vec![added(1)],
            vec![prepared(
                1,
                10,
                100,
                None,
                JobAction::Analyze {
                    selected_analysis: None,
                },
            )],
        ),
        scenario(
            "item_prepared_with_selected_analysis",
            vec![added(1)],
            vec![prepared(
                1,
                10,
                100,
                Some("ck-1"),
                JobAction::Encode {
                    selected_analysis: Some(Box::new(analysis(6, 95))),
                },
            )],
        ),
        scenario(
            "analysis_recorded_updates_run_and_record",
            vec![
                added(1),
                observed("path-1", "ck-1", 120_000),
                prepared(
                    1,
                    10,
                    100,
                    Some("ck-1"),
                    JobAction::Analyze {
                        selected_analysis: None,
                    },
                ),
            ],
            vec![recorded(100, analysis(6, 95))],
        ),
        scenario(
            "analysis_recorded_unknown_run",
            vec![],
            vec![recorded(9, analysis(6, 95))],
        ),
        scenario(
            "analysis_recorded_second_profile_and_target",
            // Two runs against the same record: a second profile lands as a
            // new index entry (Rust sorts entries by profile Ord; the TS
            // mirror appends — the replay test compares order-insensitively),
            // and a second target under an existing profile extends its map.
            vec![
                added(1),
                added(2),
                observed("path-1", "ck-1", 120_000),
                prepared(
                    1,
                    10,
                    100,
                    Some("ck-1"),
                    JobAction::Analyze {
                        selected_analysis: None,
                    },
                ),
                prepared(
                    2,
                    11,
                    101,
                    Some("ck-1"),
                    JobAction::Analyze {
                        selected_analysis: None,
                    },
                ),
                recorded(100, analysis(8, 95)),
            ],
            vec![
                recorded(101, analysis(4, 95)),
                recorded(100, analysis(8, 93)),
            ],
        ),
        scenario(
            "item_finished_analyzed",
            vec![
                added(1),
                reserved(1, 10, 100),
                prepared(
                    1,
                    10,
                    100,
                    None,
                    JobAction::Analyze {
                        selected_analysis: None,
                    },
                ),
                running(1, 10, 100),
            ],
            vec![finished(1, 10, 100, ItemOutcome::Analyzed)],
        ),
        scenario(
            "item_finished_converted_committed",
            [
                convert_prelude("ck-1"),
                vec![
                    output_started(100),
                    output_staging_created(100),
                    output_ready(100, "ck-out"),
                    output_committed(100, "ck-out"),
                ],
            ]
            .concat(),
            vec![finished(1, 10, 100, ItemOutcome::Converted)],
        ),
        scenario(
            "item_finished_remuxed_retired",
            [
                convert_prelude("ck-1"),
                vec![
                    output_started(100),
                    output_staging_created(100),
                    output_ready(100, "ck-out"),
                    output_committed(100, "ck-out"),
                    DurableDelta::Output(OutputDelta::RetireOriginalIntent { run_id: RunId(100) }),
                    DurableDelta::Output(OutputDelta::OriginalRetired { run_id: RunId(100) }),
                ],
            ]
            .concat(),
            vec![finished(1, 10, 100, ItemOutcome::Remuxed)],
        ),
        scenario(
            "item_finished_converted_without_commit",
            [
                convert_prelude("ck-1"),
                vec![
                    output_started(100),
                    output_staging_created(100),
                    output_ready(100, "ck-out"),
                ],
            ]
            .concat(),
            vec![finished(1, 10, 100, ItemOutcome::Converted)],
        ),
        scenario(
            "item_finished_failed",
            convert_prelude("ck-1"),
            vec![finished(
                1,
                10,
                100,
                ItemOutcome::Failed {
                    message: "encoder exited with status 1".to_owned(),
                },
            )],
        ),
        scenario(
            "item_finished_not_worthwhile",
            convert_prelude("ck-1"),
            vec![finished(
                1,
                10,
                100,
                ItemOutcome::NotWorthwhile {
                    attempts: vec![
                        AnalysisAttempt {
                            target: VmafTarget(95),
                            last_measurement: Some(measurement(22)),
                        },
                        AnalysisAttempt {
                            target: VmafTarget(90),
                            last_measurement: None,
                        },
                    ],
                },
            )],
        ),
        scenario(
            "item_finished_skipped_low_resolution",
            convert_prelude("ck-1"),
            vec![finished(
                1,
                10,
                100,
                ItemOutcome::Skipped {
                    reason: SkipReason::LowResolution {
                        pixels: 307_200,
                        minimum: 921_600,
                    },
                },
            )],
        ),
        scenario(
            "item_finished_stopped",
            convert_prelude("ck-1"),
            vec![finished(1, 10, 100, ItemOutcome::Stopped)],
        ),
        scenario(
            "output_retire_intent_requires_commit",
            [convert_prelude("ck-1"), vec![output_started(100)]].concat(),
            vec![DurableDelta::Output(OutputDelta::RetireOriginalIntent {
                run_id: RunId(100),
            })],
        ),
        scenario(
            "output_retired_requires_intent",
            [
                convert_prelude("ck-1"),
                vec![
                    output_started(100),
                    output_staging_created(100),
                    output_ready(100, "ck-out"),
                    output_committed(100, "ck-out"),
                ],
            ]
            .concat(),
            vec![DurableDelta::Output(OutputDelta::OriginalRetired {
                run_id: RunId(100),
            })],
        ),
        scenario(
            "output_staging_created",
            [convert_prelude("ck-1"), vec![output_started(100)]].concat(),
            vec![output_staging_created(100)],
        ),
        scenario(
            "output_abandoned",
            [convert_prelude("ck-1"), vec![output_started(100)]].concat(),
            vec![
                DurableDelta::Output(OutputDelta::AbandonStagingIntent {
                    run_id: RunId(100),
                    staging_identity: destructive(500_000),
                }),
                DurableDelta::Output(OutputDelta::Abandoned { run_id: RunId(100) }),
            ],
        ),
        scenario(
            "output_conflict",
            [convert_prelude("ck-1"), vec![output_started(100)]].concat(),
            vec![DurableDelta::Output(OutputDelta::Conflict {
                run_id: RunId(100),
                reason: "final path changed since EncodeStarted".to_owned(),
            })],
        ),
        scenario(
            "output_ready_unknown_run",
            vec![],
            vec![output_ready(9, "ck-out")],
        ),
    ]
}

#[test]
fn export_fold_fixtures() {
    let fixtures = Fixtures {
        _source: "Generated by crates/crfty-core/tests/export_fold_fixtures.rs; regenerate with `cargo test -p crfty-core --test export_fold_fixtures`. Do not edit.",
        scenarios: scenarios(),
    };
    let mut encoded =
        serde_json::to_string_pretty(&fixtures).expect("serialize fold fixtures to JSON");
    encoded.push('\n');
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/src/lib/store/fold-fixtures.json"
    );
    std::fs::write(path, encoded).expect("write fold fixtures");
}
