//! Regenerates the checked-in golden Analysis fixtures replayed by the
//! frontend (`ui/src/lib/store/analysis-fold.test.ts`), proving the
//! TypeScript Analysis fold mirrors `crfty_core::fold_analysis` over the
//! deltas the reducer actually publishes: scan results, statuses projected
//! from the claim policy, and the refreshes that follow tool, execution,
//! and job facts. Every scenario drives the real reducer through `apply`;
//! its `deltas` are the Analysis deltas it published, in order, and
//! `expected` is the reducer's own standing Analysis snapshot afterwards.
//!
//! CI verifies freshness with
//! `git diff --exit-code -- ui/src/lib/store/analysis-fixtures.json`
//! after the test suite runs, so a stale file fails the build (same contract
//! as `export_bindings` and `export_fold_fixtures`).
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use crfty_core::{
    AnalysisActivity, AnalysisCommand, AnalysisDelta, AnalysisDisplayText, AnalysisFileScan,
    AnalysisGenerationId, AnalysisIntent, AnalysisProfile, AnalysisResult, AnalysisRow,
    AnalysisRowEntry, AnalysisRowId, AnalysisScanFailure, AnalysisSnapshot, AppState,
    ArtifactIdentity, AudioCodec, AudioStreamMeta, ClaimId, Command, CompletionEvidence,
    ContentKey, Crf, CurrentFileIdentity, DecodeMode, DestructiveIdentity, EphemeralDelta,
    ExecutionSettings, FileSystemId, FileTimeNs, HardwareDecoder, HistoryCommand, ImportPath,
    ImportedHistoryRecord, ItemOutcome, LocatedTool, LocatedTools, MediaContainer,
    MediaObservation, Operation, OutputDelta, OutputState, OutputTarget, OutputTransaction,
    OverwriteDecision, ParkedStatus, PathBinding, PathHash, QueueAddRequest, QueueCommand,
    QueueItemId, Replacement, Reply, RunId, SearchMeasurement, SessionCommand, SettingsCommand,
    SystemCommand, TimestampReliability, ToolAvailability, ToolRevisions, ToolSource,
    ToolVerification, UnixMillis, VideoCodec, VideoMeta, VmafScore, WorkerCommand, apply,
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
    deltas: Vec<AnalysisDelta>,
    expected: AnalysisSnapshot,
}

/// Drives one reducer and records every Analysis delta it publishes.
struct Recorder {
    state: AppState,
    deltas: Vec<AnalysisDelta>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            state: AppState::default(),
            deltas: Vec::new(),
        }
    }

    fn apply(&mut self, command: Command) -> Reply {
        let applied = apply(&mut self.state, command);
        for delta in applied.ephemeral {
            if let EphemeralDelta::Analysis(delta) = delta {
                self.deltas.push(delta);
            }
        }
        applied.reply
    }

    fn accept(&mut self, command: Command) {
        let reply = self.apply(command);
        assert!(
            matches!(
                reply,
                Reply::Accepted
                    | Reply::AnalysisStarted { .. }
                    | Reply::BasicScan(_)
                    | Reply::Reserved(Some(_))
                    | Reply::Claimed(Some(_))
                    | Reply::Imported { .. }
            ),
            "fixture command was not accepted: {reply:?}"
        );
    }

    fn finish(self, name: &'static str) -> Scenario {
        Scenario {
            name,
            deltas: self.deltas,
            expected: self.state.analysis,
        }
    }

    fn tools(&mut self, verification: ToolVerification) {
        self.accept(Command::System(SystemCommand::ToolsDiscovered {
            availability: ToolAvailability::Located {
                tools: located_tools(),
                verification,
            },
        }));
    }

    /// One generation with the given discovered file rows, taken through
    /// discovery into Basic Scan.
    fn discover(&mut self, names: &[&str]) {
        self.accept(Command::Analysis(AnalysisCommand::Begin {
            roots: vec![display("/videos")],
        }));
        let rows = names
            .iter()
            .enumerate()
            .map(|(index, name)| AnalysisRow {
                id: AnalysisRowId(index as u64 + 1),
                parent: None,
                entry: AnalysisRowEntry::File {
                    scan: AnalysisFileScan::Discovered,
                },
                display_name: display(name),
                display_path: display(&format!("/videos/{name}")),
            })
            .collect();
        self.accept(Command::Analysis(AnalysisCommand::UpsertRows {
            generation: GENERATION,
            rows,
        }));
        self.accept(Command::Analysis(AnalysisCommand::SetActivity {
            generation: GENERATION,
            activity: AnalysisActivity::Discovered,
        }));
        self.accept(Command::Analysis(AnalysisCommand::BeginBasicScan {
            generation: GENERATION,
        }));
    }

    fn observe(&mut self, row: u64, observation: &MediaObservation, import_paths: &[&str]) {
        self.accept(Command::Analysis(AnalysisCommand::ObserveFile {
            generation: GENERATION,
            row_id: AnalysisRowId(row),
            observation: Box::new(observation.clone()),
            import_paths: import_paths
                .iter()
                .map(|path| ImportPath((*path).to_owned()))
                .collect(),
        }));
    }

    fn ready(&mut self) {
        self.accept(Command::Analysis(AnalysisCommand::FinishBasicScan {
            generation: GENERATION,
        }));
    }

    /// Queue, claim, and start one job over `observation`, returning the
    /// claim and run ids the reducer allocated.
    fn start_job(
        &mut self,
        item: u64,
        operation: Operation,
        observation: &MediaObservation,
    ) -> (ClaimId, RunId) {
        self.accept(Command::Queue(QueueCommand::AddMany {
            requests: vec![QueueAddRequest {
                item_id: QueueItemId(item),
                input: PathBuf::from(format!("/videos/{item}.mkv")),
                path_hash: None,
                identity: None,
                timestamp_reliability: TimestampReliability::Unknown,
                operation,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                overwrite: OverwriteDecision::FollowSettings,
            }],
        }));
        if self.state.session == crfty_core::SessionState::Idle {
            self.accept(Command::Session(SessionCommand::Start));
        }
        let reserved = self.apply(Command::Worker(WorkerCommand::ReserveNext));
        let Reply::Reserved(Some(job)) = reserved else {
            panic!("fixture reservation failed: {reserved:?}");
        };
        let (claim_id, run_id) = (job.claim_id, job.run_id);
        self.accept(Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(item),
            claim_id,
            run_id,
            observation: Some(Box::new(observation.clone())),
            import_paths: Vec::new(),
        }));
        self.accept(Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(item),
            claim_id,
            run_id,
            at: UnixMillis(1_000),
        }));
        (claim_id, run_id)
    }

    fn record_analysis(&mut self, item: u64, ids: (ClaimId, RunId), result: AnalysisResult) {
        self.accept(Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(item),
            claim_id: ids.0,
            run_id: ids.1,
            result: Box::new(result),
        }));
    }

    fn finish_job(&mut self, item: u64, ids: (ClaimId, RunId), outcome: ItemOutcome) {
        self.accept(Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(item),
            claim_id: ids.0,
            run_id: ids.1,
            outcome,
            at: UnixMillis(2_000),
            phase_spans: Vec::new(),
            final_telemetry: None,
        }));
    }

    /// The composed execution a claim over `codec` would run with now.
    fn composed(&self, codec: &VideoCodec) -> ExecutionSettings {
        match crfty_core::compose_execution(
            &self.state.execution,
            &self.state.tools,
            &self.state.settings,
            OverwriteDecision::FollowSettings,
            Some(codec),
        ) {
            Ok(execution) => execution,
            Err(reason) => panic!("fixture tools must compose: {reason:?}"),
        }
    }
}

const GENERATION: AnalysisGenerationId = AnalysisGenerationId(1);

fn display(text: &str) -> AnalysisDisplayText {
    AnalysisDisplayText {
        text: text.to_owned(),
        lossy: false,
    }
}

fn located_tools() -> LocatedTools {
    LocatedTools {
        ffmpeg: LocatedTool {
            source: ToolSource::SearchPath,
            path: PathBuf::from("/usr/bin/ffmpeg"),
        },
        ffprobe: LocatedTool {
            source: ToolSource::SearchPath,
            path: PathBuf::from("/usr/bin/ffprobe"),
        },
    }
}

fn verified(decoders: &[HardwareDecoder]) -> ToolVerification {
    ToolVerification::Verified {
        revisions: ToolRevisions {
            ab_av1: "ab-av1 0.10.0".to_owned(),
            ffmpeg: "7.1".to_owned(),
            encoder: "7.1".to_owned(),
        },
        hardware_decoders: decoders.iter().copied().collect::<BTreeSet<_>>(),
    }
}

fn destructive(inode: u64, size: u64) -> DestructiveIdentity {
    DestructiveIdentity {
        file_id: FileSystemId::Unix { device: 1, inode },
        size,
        modified_ns: Some(FileTimeNs(1_700_000_000_000_000_000)),
    }
}

fn meta(codec: VideoCodec, container: MediaContainer, width: u32, height: u32) -> VideoMeta {
    VideoMeta {
        codec,
        container,
        width,
        height,
        rotation_degrees: 0,
        duration_ms: 5_400_000,
        size_bytes: 4_000_000_000,
        audio: vec![AudioStreamMeta {
            codec: AudioCodec::Aac,
            channels: 6,
        }],
        subtitle_count: 1,
    }
}

fn observation(name: &str, inode: u64, metadata: VideoMeta) -> MediaObservation {
    MediaObservation {
        path_hash: PathHash(format!("path:{name}")),
        binding: PathBinding {
            identity: destructive(inode, metadata.size_bytes),
            content_key: ContentKey(format!("content:{name}")),
        },
        metadata,
    }
}

fn h264_1080p(name: &str, inode: u64) -> MediaObservation {
    observation(
        name,
        inode,
        meta(VideoCodec::H264, MediaContainer::Matroska, 1_920, 1_080),
    )
}

fn result_for(execution: &ExecutionSettings) -> AnalysisResult {
    AnalysisResult {
        requested_target: execution.requested_target,
        successful_target: execution.requested_target,
        fallback_floor: execution.fallback_floor,
        fallback_step: execution.fallback_step,
        failed_attempts: Vec::new(),
        measurement: SearchMeasurement {
            crf: Crf(31_000),
            score: VmafScore(9_530),
            predicted_size: 1_600_000_000,
            predicted_percent_basis_points: 4_000,
            predicted_duration_ms: 2_700_000,
            from_cache: false,
        },
        profile: execution.profile.clone(),
    }
}

fn scan_before_verification_then_probe() -> Scenario {
    let mut recorder = Recorder::new();
    recorder.tools(ToolVerification::Pending);
    recorder.discover(&["movie.mkv"]);
    recorder.observe(1, &h264_1080p("movie", 10), &[]);
    recorder.ready();
    recorder.accept(Command::System(SystemCommand::ToolsProbed {
        tools: located_tools(),
        verification: verified(&[]),
    }));
    recorder.finish("scan_before_verification_then_probe")
}

fn analyze_job_refreshes_a_ready_row() -> Scenario {
    let mut recorder = Recorder::new();
    recorder.tools(verified(&[]));
    recorder.discover(&["movie.mkv"]);
    let movie = h264_1080p("movie", 10);
    recorder.observe(1, &movie, &[]);
    recorder.ready();
    let ids = recorder.start_job(1, Operation::Analyze, &movie);
    let execution = recorder.composed(&VideoCodec::H264);
    recorder.record_analysis(1, ids, result_for(&execution));
    recorder.finish_job(1, ids, ItemOutcome::Analyzed);
    recorder.finish("analyze_job_refreshes_a_ready_row")
}

fn conversion_converts_the_row_and_recognizes_its_output() -> Scenario {
    let mut recorder = Recorder::new();
    recorder.tools(verified(&[]));
    recorder.discover(&["movie.mkv", "movie-av1.mkv"]);
    let movie = h264_1080p("movie", 10);
    recorder.observe(1, &movie, &[]);
    recorder.ready();
    let ids = recorder.start_job(1, Operation::Convert, &movie);
    let execution = recorder.composed(&VideoCodec::H264);
    recorder.record_analysis(1, ids, result_for(&execution));
    let output = ArtifactIdentity {
        content_key: ContentKey("content:movie-av1".to_owned()),
        destructive: destructive(20, 1_600_000_000),
    };
    recorder.accept(Command::Worker(WorkerCommand::Output(
        OutputDelta::OutputStarted {
            transaction: Box::new(OutputTransaction {
                run_id: ids.1,
                input: PathBuf::from("/videos/1.mkv"),
                input_identity: movie.binding.identity.clone(),
                staging: PathBuf::from("/videos/.1.mkv.crfty-staging"),
                final_path: PathBuf::from("/videos/movie-av1.mkv"),
                final_preimage: None,
                replacement: Replacement::KeepOriginal,
                state: OutputState::Started,
            }),
        },
    )));
    recorder.accept(Command::Worker(WorkerCommand::Output(
        OutputDelta::StagingCreated {
            run_id: ids.1,
            initial: destructive(20, 0),
        },
    )));
    recorder.accept(Command::Worker(WorkerCommand::Output(
        OutputDelta::OutputReady {
            run_id: ids.1,
            staging_identity: output.clone(),
        },
    )));
    recorder.accept(Command::Worker(WorkerCommand::Output(
        OutputDelta::OutputCommitted {
            run_id: ids.1,
            final_identity: output.clone(),
        },
    )));
    recorder.finish_job(
        1,
        ids,
        ItemOutcome::Converted(CompletionEvidence::LiveEncode {
            input_size: movie.metadata.size_bytes,
            output_size: output.destructive.size,
            encode_decode: DecodeMode::Software,
        }),
    );
    // A later scan finds the produced file: its row reports the source
    // conversion, with the output's own (already AV1) eligibility.
    recorder.accept(Command::Analysis(AnalysisCommand::BeginBasicScan {
        generation: GENERATION,
    }));
    recorder.accept(Command::Analysis(AnalysisCommand::InspectFile {
        generation: GENERATION,
        row_id: AnalysisRowId(2),
        path_hash: PathHash("path:movie-av1".to_owned()),
        current: CurrentFileIdentity::Present(output.destructive.clone()),
        timestamp_reliability: TimestampReliability::Reliable,
        import_paths: Vec::new(),
    }));
    recorder.ready();
    recorder.finish("conversion_converts_the_row_and_recognizes_its_output")
}

fn base_execution_decides_whether_a_search_applies() -> Scenario {
    let mut recorder = Recorder::new();
    recorder.tools(verified(&[]));
    recorder.accept(Command::System(SystemCommand::ConfigureExecution {
        base: ExecutionSettings {
            profile: AnalysisProfile {
                preset: 4,
                ..AnalysisProfile::production()
            },
            ..ExecutionSettings::default()
        },
    }));
    recorder.discover(&["movie.mkv"]);
    let movie = h264_1080p("movie", 10);
    recorder.observe(1, &movie, &[]);
    recorder.ready();
    let ids = recorder.start_job(1, Operation::Analyze, &movie);
    let execution = recorder.composed(&VideoCodec::H264);
    recorder.record_analysis(1, ids, result_for(&execution));
    recorder.finish_job(1, ids, ItemOutcome::Analyzed);
    recorder.accept(Command::System(SystemCommand::ConfigureExecution {
        base: ExecutionSettings::default(),
    }));
    recorder.finish("base_execution_decides_whether_a_search_applies")
}

fn hardware_decode_setting_refreshes_every_row() -> Scenario {
    let mut recorder = Recorder::new();
    recorder.tools(verified(&[HardwareDecoder::H264Cuvid]));
    recorder.discover(&["movie.mkv"]);
    let movie = h264_1080p("movie", 10);
    recorder.observe(1, &movie, &[]);
    recorder.ready();
    let ids = recorder.start_job(1, Operation::Analyze, &movie);
    let execution = recorder.composed(&VideoCodec::H264);
    assert_eq!(
        execution.profile.decode_mode,
        DecodeMode::Hardware(HardwareDecoder::H264Cuvid)
    );
    recorder.record_analysis(1, ids, result_for(&execution));
    recorder.finish_job(1, ids, ItemOutcome::Analyzed);
    let mut settings = recorder.state.settings.clone();
    settings.hardware_decode = false;
    recorder.accept(Command::Settings(SettingsCommand::Set { settings }));
    recorder.finish("hardware_decode_setting_refreshes_every_row")
}

fn ineligible_media_reports_skips_and_remux() -> Scenario {
    let mut recorder = Recorder::new();
    recorder.tools(verified(&[]));
    recorder.discover(&["phone.mp4", "done.mkv", "av1.mp4"]);
    recorder.observe(
        1,
        &observation(
            "phone",
            10,
            meta(
                VideoCodec::H264,
                MediaContainer::Other("mov,mp4,m4a,3gp,3g2,mj2".to_owned()),
                640,
                480,
            ),
        ),
        &[],
    );
    recorder.observe(
        2,
        &observation(
            "done",
            11,
            meta(VideoCodec::Av1, MediaContainer::Matroska, 1_920, 1_080),
        ),
        &[],
    );
    recorder.observe(
        3,
        &observation(
            "av1",
            12,
            meta(
                VideoCodec::Av1,
                MediaContainer::Other("mov,mp4,m4a,3gp,3g2,mj2".to_owned()),
                3_840,
                2_160,
            ),
        ),
        &[],
    );
    recorder.ready();
    recorder.finish("ineligible_media_reports_skips_and_remux")
}

fn imported_history_raises_only_the_historical_level() -> Scenario {
    let mut recorder = Recorder::new();
    recorder.tools(verified(&[]));
    let analyzed = h264_1080p("analyzed", 10);
    let rejected = h264_1080p("rejected", 11);
    let imported = |status: ParkedStatus, observation: &MediaObservation| ImportedHistoryRecord {
        status,
        size: Some(observation.metadata.size_bytes),
        modified_ns: observation.binding.identity.modified_ns,
        video_codec: Some(VideoCodec::H264),
        width: Some(1_920),
        height: Some(1_080),
        duration_ms: Some(observation.metadata.duration_ms),
        output_size: None,
        encoding_time: None,
        crf: None,
        vmaf: None,
        target: None,
        requested_target: None,
        floor_target: None,
        decided_at: UnixMillis(1_600_000_000_000),
    };
    recorder.accept(Command::History(HistoryCommand::Import {
        records: vec![
            (
                ImportPath("/videos/analyzed.mkv".to_owned()),
                imported(ParkedStatus::Analyzed, &analyzed),
            ),
            (
                ImportPath("/videos/rejected.mkv".to_owned()),
                imported(ParkedStatus::NotWorthwhile, &rejected),
            ),
        ],
    }));
    recorder.discover(&["analyzed.mkv", "rejected.mkv"]);
    recorder.observe(1, &analyzed, &["/videos/analyzed.mkv"]);
    recorder.observe(2, &rejected, &["/videos/rejected.mkv"]);
    recorder.ready();
    recorder.finish("imported_history_raises_only_the_historical_level")
}

fn scan_failures_keep_the_last_status() -> Scenario {
    let mut recorder = Recorder::new();
    recorder.tools(verified(&[]));
    recorder.discover(&["gone.mkv", "flaky.mkv"]);
    recorder.accept(Command::Analysis(AnalysisCommand::FailFile {
        generation: GENERATION,
        row_id: AnalysisRowId(1),
        failure: AnalysisScanFailure::Missing,
    }));
    recorder.observe(2, &h264_1080p("flaky", 11), &[]);
    recorder.accept(Command::Analysis(AnalysisCommand::FailFile {
        generation: GENERATION,
        row_id: AnalysisRowId(2),
        failure: AnalysisScanFailure::ChangedDuringSampling,
    }));
    recorder.ready();
    recorder.finish("scan_failures_keep_the_last_status")
}

fn scenarios() -> Vec<Scenario> {
    vec![
        scan_before_verification_then_probe(),
        analyze_job_refreshes_a_ready_row(),
        conversion_converts_the_row_and_recognizes_its_output(),
        base_execution_decides_whether_a_search_applies(),
        hardware_decode_setting_refreshes_every_row(),
        ineligible_media_reports_skips_and_remux(),
        imported_history_raises_only_the_historical_level(),
        scan_failures_keep_the_last_status(),
    ]
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn export_analysis_fixtures() {
    let fixtures = Fixtures {
        _source: "Generated by crates/crfty-core/tests/export_analysis_fixtures.rs; regenerate with `cargo test -p crfty-core --test export_analysis_fixtures`. Do not edit.",
        scenarios: scenarios(),
    };
    let mut encoded =
        serde_json::to_string_pretty(&fixtures).expect("serialize Analysis fixtures to JSON");
    encoded.push('\n');
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ui/src/lib/store/analysis-fixtures.json"
    );
    std::fs::write(path, encoded).expect("write Analysis fixtures");
}
