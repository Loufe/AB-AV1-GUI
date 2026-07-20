//! Regenerates the checked-in golden projection fixtures replayed by the
//! frontend (`ui/src/lib/projection/history-rows.test.ts`), proving the
//! TypeScript history-row mirror matches `crfty_core::history_rows` (#40).
//! Every scenario's `expected_rows` are computed by the Rust projection
//! itself — the oracle — and serialized with the same serde shapes the
//! snapshot wire uses for `DurableState`.
//!
//! CI verifies freshness with
//! `git diff --exit-code -- ui/src/lib/projection/projection-fixtures.json`
//! after the test suite runs, so a stale file fails the build (same contract
//! as `export_bindings` and `export_fold_fixtures`).
#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use crfty_core::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, ArtifactIdentity, AudioCodec, AudioStreamMeta,
    ClaimId, CompletionEvidence, ContentKey, ConversionRun, Crf, DestructiveIdentity, DurableState,
    DurationMs, ExecutionSettings, FailureFacts, FailureKind, FileRecord, FileSystemId, HistoryRow,
    ItemOutcome, JobAction, JobPhase, JobSpec, MediaContainer, Operation, OutputState,
    OutputTarget, OutputTransaction, PhaseSpan, QueueItemId, Replacement, RunId, SearchMeasurement,
    StreamByteSizes, UnixMillis, Verdict, VerdictKind, VideoCodec, VideoMeta, VmafScore,
    VmafTarget, history_rows,
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
    state: DurableState,
    expected_rows: Vec<HistoryRow>,
}

fn scenario(name: &'static str, state: DurableState) -> Scenario {
    let expected_rows = history_rows(&state);
    Scenario {
        name,
        state,
        expected_rows,
    }
}

const DAY_MS: u64 = 86_400_000;
const FINISHED_AT: UnixMillis = UnixMillis(DAY_MS * 20_000 + 3_600_000);

fn key(name: &str) -> ContentKey {
    ContentKey(name.to_owned())
}

fn meta(codec: VideoCodec, size_bytes: u64) -> VideoMeta {
    VideoMeta {
        codec,
        container: MediaContainer::Matroska,
        width: 1920,
        height: 1080,
        rotation_degrees: 0,
        duration_ms: 600_000,
        size_bytes,
        audio: vec![
            AudioStreamMeta {
                codec: AudioCodec::Aac,
                channels: 6,
            },
            AudioStreamMeta {
                codec: AudioCodec::Other("wmav2".to_owned()),
                channels: 2,
            },
        ],
        subtitle_count: 1,
    }
}

fn analysis(crf_milli: u32, score_centi: u16, target: u8) -> AnalysisResult {
    AnalysisResult {
        requested_target: VmafTarget(95),
        successful_target: VmafTarget(target),
        fallback_floor: VmafTarget(90),
        fallback_step: 1,
        failed_attempts: Vec::new(),
        measurement: SearchMeasurement {
            crf: Crf(crf_milli),
            score: VmafScore(score_centi),
            predicted_size: 1_000_000,
            predicted_percent_basis_points: 4_200,
            predicted_duration_ms: 60_000,
            from_cache: false,
        },
        profile: AnalysisProfile::production(),
    }
}

fn spec(run: u64, content_key: &ContentKey) -> JobSpec {
    JobSpec {
        item_id: QueueItemId(run),
        claim_id: ClaimId(run),
        run_id: RunId(run),
        input: PathBuf::from(format!("videos/input-{run}.mkv")),
        content_key: Some(content_key.clone()),
        operation: Operation::Convert,
        intent: AnalysisIntent::ReuseIfFresh,
        output_target: OutputTarget::Replace,
        execution: ExecutionSettings::production(AnalysisProfile::production(), false),
        action: JobAction::Encode {
            selected_analysis: None,
        },
    }
}

fn finished_run(run: u64, content_key: &ContentKey, outcome: ItemOutcome) -> ConversionRun {
    ConversionRun {
        spec: spec(run, content_key),
        analysis: Some(analysis(24_000, 9_512, 95)),
        output_content_key: None,
        outcome: Some(outcome),
        started_at: Some(UnixMillis(FINISHED_AT.0 - 300_000)),
        finished_at: Some(FINISHED_AT),
        phase_spans: vec![
            PhaseSpan {
                phase: JobPhase::Analyzing,
                duration: DurationMs(60_000),
            },
            PhaseSpan {
                phase: JobPhase::Encoding,
                duration: DurationMs(240_000),
            },
        ],
    }
}

fn live_encode(input_size: u64, output_size: u64) -> CompletionEvidence {
    CompletionEvidence::LiveEncode {
        input_size,
        output_size,
        stream_sizes: StreamByteSizes {
            video: output_size,
            audio: 0,
            subtitle: 0,
            other: 0,
        },
        encode_decode: crfty_core::DecodeMode::Software,
    }
}

fn verdict(kind: VerdictKind, run: u64) -> Verdict {
    Verdict {
        kind,
        source_run: Some(RunId(run)),
        decided_at: FINISHED_AT,
    }
}

fn converted_verdict(run: u64) -> Verdict {
    verdict(
        VerdictKind::Converted {
            output_content_key: Some(key(&format!("output-{run:04}"))),
            input_size: None,
            output_size: None,
            encoding_time: None,
            crf: None,
            vmaf: None,
            target: None,
        },
        run,
    )
}

fn record(codec: VideoCodec, size_bytes: u64, verdict: Option<Verdict>) -> FileRecord {
    let mut record = FileRecord::new(meta(codec, size_bytes));
    record.verdict = verdict;
    record
}

fn identity(size: u64) -> DestructiveIdentity {
    DestructiveIdentity {
        file_id: FileSystemId::Unix {
            device: 1,
            inode: size,
        },
        size,
        modified_ns: None,
    }
}

fn committed_transaction(run: u64, input_size: u64, output_size: u64) -> OutputTransaction {
    OutputTransaction {
        run_id: RunId(run),
        input: PathBuf::from(format!("videos/input-{run}.mkv")),
        input_identity: identity(input_size),
        staging: PathBuf::from("videos/.staging.mkv"),
        final_path: PathBuf::from("videos/final.mkv"),
        final_preimage: None,
        replacement: Replacement::RetireOriginal,
        state: OutputState::Committed {
            final_identity: ArtifactIdentity {
                content_key: key(&format!("output-{run:04}")),
                destructive: identity(output_size),
            },
        },
    }
}

fn converted_state(input_size: u64, output_size: u64) -> DurableState {
    let mut state = DurableState::default();
    let content_key = key("content-0001");
    state.records.insert(
        content_key.clone(),
        record(VideoCodec::Hevc, input_size, Some(converted_verdict(1))),
    );
    state.conversion_runs.insert(
        RunId(1),
        finished_run(
            1,
            &content_key,
            ItemOutcome::Converted(live_encode(input_size, output_size)),
        ),
    );
    state
}

fn scenarios() -> Vec<Scenario> {
    let mut list = Vec::new();

    list.push(scenario("empty", DurableState::default()));

    list.push(scenario(
        "converted_with_live_evidence",
        converted_state(10_000_000, 4_000_000),
    ));

    // Crash-window recovery: the outcome carries no sizes, so the settled
    // output transaction supplies both.
    let mut recovered = converted_state(0, 0);
    if let Some(run) = recovered.conversion_runs.get_mut(&RunId(1)) {
        run.outcome = Some(ItemOutcome::Converted(
            CompletionEvidence::RecoveredAtStartup,
        ));
    }
    recovered
        .outputs
        .insert(RunId(1), committed_transaction(1, 8_000_000, 3_000_000));
    list.push(scenario("converted_recovered_at_startup", recovered));

    // A verdict whose run is gone (the parked-record stand-in): input size
    // falls back to metadata, output stays unknown, date to decided_at.
    let mut adopted = DurableState::default();
    adopted.records.insert(
        key("adopted"),
        record(VideoCodec::H264, 5_000_000, Some(converted_verdict(77))),
    );
    list.push(scenario("converted_verdict_without_run", adopted));

    let mut remuxed = DurableState::default();
    let remux_key = key("remuxed");
    remuxed.records.insert(
        remux_key.clone(),
        record(
            VideoCodec::Av1,
            9_000_000,
            Some(verdict(
                VerdictKind::Remuxed {
                    output_content_key: key("remuxed-out"),
                    input_size: None,
                    output_size: None,
                },
                50,
            )),
        ),
    );
    let mut remux_run = finished_run(
        50,
        &remux_key,
        ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
            input_size: 9_000_000,
            output_size: 8_900_000,
        }),
    );
    remux_run.analysis = None;
    remuxed.conversion_runs.insert(RunId(50), remux_run);
    list.push(scenario("remuxed", remuxed));

    let mut declined = DurableState::default();
    let declined_key = key("declined");
    declined.records.insert(
        declined_key.clone(),
        record(
            VideoCodec::H264,
            5_000_000,
            Some(verdict(
                VerdictKind::NotWorthwhile {
                    requested: VmafTarget(95),
                    floor: VmafTarget(90),
                },
                9,
            )),
        ),
    );
    declined.conversion_runs.insert(
        RunId(9),
        finished_run(
            9,
            &declined_key,
            ItemOutcome::NotWorthwhile {
                attempts: Vec::new(),
            },
        ),
    );
    list.push(scenario("not_worthwhile", declined));

    // Verdict precedence: a converted content's later failed retry does not
    // demote its row, while fresh content with only a failed run reports
    // Failed, and a second fresh content with only a stopped run reports
    // Stopped.
    let mut interrupted = converted_state(10_000_000, 4_000_000);
    let converted_key = key("content-0001");
    let mut failed_retry = finished_run(
        2,
        &converted_key,
        ItemOutcome::Failed(FailureFacts::new(FailureKind::EncodeRun, "encoder crashed")),
    );
    failed_retry.analysis = None;
    interrupted.conversion_runs.insert(RunId(2), failed_retry);
    let fresh_key = key("fresh-failed");
    interrupted
        .records
        .insert(fresh_key.clone(), record(VideoCodec::Vp9, 7_000_000, None));
    let mut failed_fresh = finished_run(
        3,
        &fresh_key,
        ItemOutcome::Failed(FailureFacts::new(FailureKind::SearchRun, "probe failed")),
    );
    failed_fresh.analysis = None;
    interrupted.conversion_runs.insert(RunId(3), failed_fresh);
    let stopped_key = key("fresh-stopped");
    interrupted.records.insert(
        stopped_key.clone(),
        record(VideoCodec::H264, 6_000_000, None),
    );
    let mut stopped_fresh = finished_run(4, &stopped_key, ItemOutcome::Stopped);
    stopped_fresh.analysis = None;
    interrupted.conversion_runs.insert(RunId(4), stopped_fresh);
    list.push(scenario("verdict_wins_over_interruptions", interrupted));

    // Two analysis runs against one content: the row reports the latest
    // run's measurement (higher run id wins), not the first.
    let mut studied = DurableState::default();
    let studied_key = key("studied");
    studied.records.insert(
        studied_key.clone(),
        record(VideoCodec::Hevc, 5_000_000, None),
    );
    let mut first = finished_run(4, &studied_key, ItemOutcome::Analyzed);
    first.analysis = Some(analysis(30_000, 9_400, 95));
    studied.conversion_runs.insert(RunId(4), first);
    let mut second = finished_run(5, &studied_key, ItemOutcome::Analyzed);
    second.analysis = Some(analysis(26_000, 9_600, 95));
    studied.conversion_runs.insert(RunId(5), second);
    list.push(scenario("analyzed_latest_run_wins", studied));

    // Analyses on the record with no surviving run (future legacy adoption):
    // the deterministic last index entry stands in and no run is attributed.
    let mut orphaned = DurableState::default();
    let mut orphan_record = record(VideoCodec::H264, 4_000_000, None);
    let mut by_target = BTreeMap::new();
    by_target.insert(VmafTarget(93), analysis(28_000, 9_350, 93));
    by_target.insert(VmafTarget(95), analysis(25_000, 9_520, 95));
    orphan_record
        .analyses
        .insert(AnalysisProfile::production(), by_target);
    orphaned.records.insert(key("orphaned"), orphan_record);
    list.push(scenario("analyzed_record_only", orphaned));

    // Scanned-only content has nothing to report; rotated portrait metadata
    // presents post-rotation dimensions on the row it does get.
    let mut mixed = DurableState::default();
    mixed
        .records
        .insert(key("seen-only"), record(VideoCodec::H264, 1_000_000, None));
    let mut portrait_meta = meta(VideoCodec::H264, 2_000_000);
    portrait_meta.rotation_degrees = 90;
    let mut portrait = FileRecord::new(portrait_meta);
    portrait.verdict = Some(verdict(
        VerdictKind::NotWorthwhile {
            requested: VmafTarget(95),
            floor: VmafTarget(90),
        },
        1,
    ));
    mixed.records.insert(key("portrait"), portrait);
    list.push(scenario("scanned_only_skipped_and_rotation", mixed));

    list
}

#[test]
fn export_projection_fixtures() {
    let fixtures = Fixtures {
        _source: "Generated by crates/crfty-core/tests/export_projection_fixtures.rs; regenerate with `cargo test -p crfty-core --test export_projection_fixtures`. Do not edit.",
        scenarios: scenarios(),
    };
    let mut encoded =
        serde_json::to_string_pretty(&fixtures).expect("serialize projection fixtures to JSON");
    encoded.push('\n');
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/src/lib/projection/projection-fixtures.json"
    );
    std::fs::write(path, encoded).expect("write projection fixtures");
}
