use std::path::PathBuf;

use proptest::prelude::*;

use crate::reducer::validate_terminal;

use crate::{
    AnalysisProfile, AnalysisResult, AppState, ArtifactIdentity, ClaimId, Command, ContentKey, Crf,
    DecodeMode, DecodePreference, DefaultOutputMode, DestructiveIdentity, DestructiveObservation,
    DurableDelta, Effect, Eligibility, EphemeralDelta, ExecutionSettings, FileRecord, FileStamp,
    FileSystemFacts, FileSystemId, ItemOutcome, JOURNAL_SCHEMA_VERSION, JobAction, JobPhase,
    JobProgress, JournalEnvelope, JournalSequence, MediaContainer, MediaObservation, Operation,
    OutputDelta, OutputRecoveryAction, OutputState, OutputTarget, OutputTransaction, PathBinding,
    PathHash, QueueCommand, QueueItemId, QueueItemState, Replacement, Reply, RunId,
    SearchMeasurement, SessionCommand, SessionState, Settings, SettingsCommand, SkipReason,
    SystemCommand, Telemetry, ToolAvailability, ToolRevisions, VideoCodec, VideoMeta, VmafScore,
    VmafTarget, WorkerCommand, apply, encode_record, evaluate_eligibility, recover_output, replay,
    select_analysis, select_job_action, settled_outcome,
};

fn execution() -> ExecutionSettings {
    ExecutionSettings::production(
        AnalysisProfile::production(ToolRevisions {
            ab_av1: "test-ab-av1".to_owned(),
            ffmpeg: "test-ffmpeg".to_owned(),
            encoder: "test-svt".to_owned(),
        }),
        false,
    )
}

fn analysis() -> AnalysisResult {
    let execution = execution();
    AnalysisResult {
        requested_target: execution.requested_target,
        successful_target: execution.requested_target,
        fallback_floor: execution.fallback_floor,
        fallback_step: execution.fallback_step,
        failed_attempts: Vec::new(),
        measurement: SearchMeasurement {
            crf: Crf(30_000),
            score: VmafScore(9_500),
            predicted_size: 1_000,
            predicted_percent_basis_points: 5_000,
            predicted_duration_ms: 60_000,
            from_cache: false,
        },
        profile: execution.profile,
    }
}

#[test]
fn settings_defaults_validate_and_inactive_values_remain_remembered() {
    let defaults = Settings::default();
    assert!(defaults.validate().is_ok());
    assert_eq!(defaults.scan_extensions.len(), 4);
    assert!(defaults.hardware_decode);
    assert!(!defaults.privacy.anonymize_logs);
    assert!(!defaults.privacy.anonymize_history);

    let mut remembered = defaults;
    remembered.output.suffix.clear();
    remembered.output.separate_folder = None;
    assert!(remembered.validate().is_ok());

    remembered.output.default_mode = DefaultOutputMode::Suffix;
    assert_eq!(
        remembered.validate(),
        Err("default output suffix must not be empty in suffix mode")
    );
    remembered.output.default_mode = DefaultOutputMode::SeparateFolder;
    assert_eq!(
        remembered.validate(),
        Err("default separate output folder is required in separate-folder mode")
    );
}

#[test]
fn settings_change_is_typed_config_state_with_a_write_effect() {
    let mut state = AppState::default();
    let mut settings = state.settings.clone();
    settings.output.overwrite_existing = true;
    settings.hardware_decode = false;
    let applied = apply(
        &mut state,
        Command::Settings(SettingsCommand::Set {
            settings: settings.clone(),
        }),
    );
    assert_eq!(applied.reply, Reply::Accepted);
    assert_eq!(applied.config.len(), 1);
    assert_eq!(
        applied.effects,
        vec![Effect::WriteSettings {
            settings: settings.clone(),
        }]
    );
    assert_eq!(state.settings, settings);

    let current = state.settings.clone();
    let unchanged = apply(
        &mut state,
        Command::Settings(SettingsCommand::Set { settings: current }),
    );
    assert!(unchanged.config.is_empty());
    assert_eq!(unchanged.effects.len(), 1);
}

#[test]
fn settings_control_job_overwrite_and_hardware_decode_policy() {
    let mut state = AppState::default();
    state.settings.output.overwrite_existing = true;
    state.settings.hardware_decode = false;
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _started = start_session(&mut state);
    let mut requested = execution();
    requested.profile.decode_mode = DecodeMode::Hardware(crate::HardwareDecoder::H264Qsv);
    let prepared = reserve_and_prepare(&mut state, ClaimId(2), RunId(3), requested);
    let Reply::Claimed(Some(job)) = prepared.reply else {
        panic!("expected claimed job");
    };
    assert!(job.spec.execution.overwrite_existing);
    assert_eq!(
        job.spec.execution.decode_preference,
        DecodePreference::SoftwareOnly
    );
    assert_eq!(job.spec.execution.profile.decode_mode, DecodeMode::Software);
}

#[test]
fn session_start_requires_discovered_tools_and_discovery_is_idempotent() {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let blocked = apply(&mut state, Command::Session(SessionCommand::Start));
    let Reply::Rejected { reason } = blocked.reply else {
        panic!("expected fail-closed session start");
    };
    assert!(
        reason.starts_with("media tools are unavailable"),
        "{reason}"
    );
    assert_eq!(state.session, SessionState::Idle);

    let discovered = apply(
        &mut state,
        Command::System(SystemCommand::ToolsDiscovered {
            availability: ToolAvailability::Available,
        }),
    );
    assert_eq!(discovered.reply, Reply::Accepted);
    assert_eq!(
        discovered.ephemeral,
        vec![EphemeralDelta::ToolsChanged(ToolAvailability::Available)]
    );
    assert_eq!(state.tools, ToolAvailability::Available);

    let unchanged = apply(
        &mut state,
        Command::System(SystemCommand::ToolsDiscovered {
            availability: ToolAvailability::Available,
        }),
    );
    assert_eq!(unchanged.reply, Reply::Accepted);
    assert!(unchanged.ephemeral.is_empty());

    let started = apply(&mut state, Command::Session(SessionCommand::Start));
    assert_eq!(started.reply, Reply::Accepted);
    assert_eq!(state.session, SessionState::Running);

    let missing = apply(
        &mut state,
        Command::System(SystemCommand::ToolsDiscovered {
            availability: ToolAvailability::Missing {
                missing: vec![crate::MediaTool::Ffmpeg],
                detail: "ffmpeg was not found".to_owned(),
            },
        }),
    );
    assert_eq!(missing.reply, Reply::Accepted);
    assert!(matches!(state.tools, ToolAvailability::Missing { .. }));
}

/// Reports available tools, then starts the session. Nearly every session
/// test needs both because `AppState` defaults to tools-missing (fail-closed).
fn start_session(state: &mut AppState) -> crate::Applied {
    let discovered = apply(
        state,
        Command::System(SystemCommand::ToolsDiscovered {
            availability: ToolAvailability::Available,
        }),
    );
    assert_eq!(discovered.reply, Reply::Accepted);
    apply(state, Command::Session(SessionCommand::Start))
}

fn add_command(item_id: QueueItemId, input: impl Into<PathBuf>) -> Command {
    Command::Queue(QueueCommand::Add {
        item_id,
        input: input.into(),
        operation: Operation::Convert,
        output_target: OutputTarget::Replace,
    })
}

fn identity(name: &str, size: u64) -> ArtifactIdentity {
    ArtifactIdentity {
        content_key: ContentKey(name.to_owned()),
        destructive: destructive(name, size),
    }
}

fn destructive(name: &str, size: u64) -> DestructiveIdentity {
    DestructiveIdentity {
        file_id: FileSystemId::Unix {
            device: 1,
            inode: u64::from(name.as_bytes().first().copied().unwrap_or_default()),
        },
        size,
        modified_ns: Some(u128::from(size)),
    }
}

fn transaction(state: OutputState, replacement: Replacement) -> OutputTransaction {
    OutputTransaction {
        run_id: RunId(9),
        input: PathBuf::from("input.mkv"),
        input_identity: destructive("input", 10),
        staging: PathBuf::from(".output.mkv.crfty-9.part"),
        final_path: PathBuf::from("output.mkv"),
        final_preimage: None,
        replacement,
        state,
    }
}

fn media_observation(content: &str) -> MediaObservation {
    MediaObservation {
        path_hash: PathHash(format!("path-{content}")),
        binding: PathBinding {
            stamp: FileStamp {
                size: 10_000,
                modified_ns: Some(1),
            },
            content_key: ContentKey(content.to_owned()),
        },
        metadata: VideoMeta {
            codec: VideoCodec::H264,
            container: MediaContainer::Matroska,
            width: 1_280,
            height: 720,
            rotation_degrees: 0,
            duration_ms: 60_000,
        },
    }
}

#[test]
fn reducer_enforces_session_claim_and_terminal_ordering() {
    let mut state = AppState::default();
    let add = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    assert_eq!(add.reply, Reply::Accepted);
    let start = start_session(&mut state);
    assert_eq!(start.effects, vec![Effect::StartWorker]);
    let claim = reserve_and_prepare(&mut state, ClaimId(2), RunId(3), execution());
    assert!(matches!(claim.reply, Reply::Claimed(Some(_))));
    let stale = apply(
        &mut state,
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(99),
            run_id: RunId(3),
        }),
    );
    assert!(matches!(stale.reply, Reply::Rejected { .. }));
    assert!(matches!(
        state.durable.queue.first().expect("queue item").state,
        QueueItemState::Claimed { .. }
    ));
    let terminal = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed {
                message: "fixture".to_owned(),
            },
            final_telemetry: Some(Telemetry {
                run_id: RunId(3),
                sequence: 20,
                phase: JobPhase::Encoding,
                progress: JobProgress::OutputPositionMs(100),
            }),
        }),
    );
    assert_eq!(terminal.reply, Reply::Accepted);
    assert_eq!(state.session, SessionState::Running);
    let finished = apply(&mut state, Command::Worker(WorkerCommand::Finished));
    assert_eq!(finished.reply, Reply::Accepted);
    assert_eq!(state.session, SessionState::Idle);
    assert!(!state.telemetry.contains_key(&RunId(3)));
}

#[test]
fn reservation_is_atomic_and_uses_current_queue_order() {
    let mut state = AppState::default();
    let _first = apply(&mut state, add_command(QueueItemId(1), "first.mkv"));
    let _second = apply(&mut state, add_command(QueueItemId(2), "second.mkv"));
    let moved = apply(
        &mut state,
        Command::Queue(QueueCommand::Move {
            item_id: QueueItemId(2),
            before: Some(QueueItemId(1)),
        }),
    );
    assert_eq!(moved.reply, Reply::Accepted);
    let _started = start_session(&mut state);
    let reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(10),
            run_id: RunId(11),
        }),
    );
    let Reply::Reserved(Some(job)) = reserved.reply else {
        panic!("expected an atomic reservation");
    };
    assert_eq!(job.item_id, QueueItemId(2));

    let competing = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(12),
            run_id: RunId(13),
        }),
    );
    assert!(matches!(competing.reply, Reply::Rejected { .. }));
}

#[test]
fn durable_analysis_is_selected_for_the_same_content_and_profile() {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "first.mkv"));
    let _started = start_session(&mut state);
    let reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    assert!(matches!(reserved.reply, Reply::Reserved(Some(_))));
    let mut moved_observation = media_observation("same-content");
    moved_observation.path_hash = PathHash("moved-path".to_owned());
    let prepared = apply(
        &mut state,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: Some(Box::new(moved_observation)),
            execution: execution(),
        }),
    );
    assert!(matches!(prepared.reply, Reply::Claimed(Some(_))));
    let recorded = apply(
        &mut state,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(analysis()),
        }),
    );
    assert_eq!(recorded.reply, Reply::Accepted);
    let failed = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed {
                message: "fixture boundary".to_owned(),
            },
            final_telemetry: None,
        }),
    );
    assert_eq!(failed.reply, Reply::Accepted);
    let _second = apply(&mut state, add_command(QueueItemId(4), "moved.mkv"));
    let reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(5),
            run_id: RunId(6),
        }),
    );
    assert!(matches!(reserved.reply, Reply::Reserved(Some(_))));
    let prepared = apply(
        &mut state,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(4),
            claim_id: ClaimId(5),
            run_id: RunId(6),
            observation: Some(Box::new(media_observation("same-content"))),
            execution: execution(),
        }),
    );
    let Reply::Claimed(Some(job)) = prepared.reply else {
        panic!("expected prepared reused job");
    };
    assert_eq!(job.spec.action.selected_analysis(), Some(&analysis()));
}

/// Builds `[1 Finished, 2 Claimed (active), 3 Queued, 4 Queued, 5 Queued]`.
fn reorder_fixture() -> AppState {
    let mut state = AppState::default();
    for id in 1..=5 {
        let added = apply(&mut state, add_command(QueueItemId(id), "video.mkv"));
        assert_eq!(added.reply, Reply::Accepted);
    }
    let start = start_session(&mut state);
    assert_eq!(start.reply, Reply::Accepted);
    let first = reserve_and_prepare(&mut state, ClaimId(20), RunId(21), execution());
    assert!(matches!(first.reply, Reply::Claimed(Some(_))));
    let finished = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(20),
            run_id: RunId(21),
            outcome: ItemOutcome::Failed {
                message: "fixture".to_owned(),
            },
            final_telemetry: None,
        }),
    );
    assert_eq!(finished.reply, Reply::Accepted);
    let second = reserve_and_prepare(&mut state, ClaimId(22), RunId(23), execution());
    assert!(matches!(second.reply, Reply::Claimed(Some(_))));
    assert_queue_shape(&state, &[1, 2, 3, 4, 5]);
    state
}

fn queue_order(state: &AppState) -> Vec<u64> {
    state.durable.queue.iter().map(|item| item.id.0).collect()
}

/// Asserts the id order and the frozen-prefix invariant: every finished item
/// precedes the active item, which precedes every queued item.
fn assert_queue_shape(state: &AppState, expected: &[u64]) {
    assert_eq!(queue_order(state), expected);
    let mut frontier = 0;
    for item in &state.durable.queue {
        let class = match item.state {
            QueueItemState::Finished(_) => 0,
            QueueItemState::Reserved { .. }
            | QueueItemState::Claimed { .. }
            | QueueItemState::Running { .. } => 1,
            QueueItemState::Queued => 2,
        };
        assert!(class >= frontier, "queue prefix invariant violated");
        frontier = class;
    }
}

fn move_command(item_id: u64, before: Option<u64>) -> Command {
    Command::Queue(QueueCommand::Move {
        item_id: QueueItemId(item_id),
        before: before.map(QueueItemId),
    })
}

#[test]
fn reorder_destinations_around_active_pending_and_finished_items() {
    struct Case {
        name: &'static str,
        item: u64,
        before: Option<u64>,
        expected_delta: Option<(u64, Option<u64>)>,
        expected_order: [u64; 5],
    }
    let accepted = [
        Case {
            name: "queued before queued",
            item: 4,
            before: Some(3),
            expected_delta: Some((4, Some(3))),
            expected_order: [1, 2, 4, 3, 5],
        },
        Case {
            name: "drop above active clamps to first pending",
            item: 4,
            before: Some(2),
            expected_delta: Some((4, Some(3))),
            expected_order: [1, 2, 4, 3, 5],
        },
        Case {
            name: "drop above finished clamps to first pending",
            item: 4,
            before: Some(1),
            expected_delta: Some((4, Some(3))),
            expected_order: [1, 2, 4, 3, 5],
        },
        Case {
            name: "first pending above active is an identity no-op",
            item: 3,
            before: Some(2),
            expected_delta: None,
            expected_order: [1, 2, 3, 4, 5],
        },
        Case {
            name: "first pending above finished is an identity no-op",
            item: 3,
            before: Some(1),
            expected_delta: None,
            expected_order: [1, 2, 3, 4, 5],
        },
        Case {
            name: "move to end",
            item: 4,
            before: None,
            expected_delta: Some((4, None)),
            expected_order: [1, 2, 3, 5, 4],
        },
        Case {
            name: "self destination is an identity no-op",
            item: 4,
            before: Some(4),
            expected_delta: None,
            expected_order: [1, 2, 3, 4, 5],
        },
    ];
    for case in accepted {
        let mut state = reorder_fixture();
        let applied = apply(&mut state, move_command(case.item, case.before));
        assert_eq!(applied.reply, Reply::Accepted, "{}", case.name);
        let expected_durable = case
            .expected_delta
            .map(|(item_id, before)| DurableDelta::QueueMoved {
                item_id: QueueItemId(item_id),
                before: before.map(QueueItemId),
            })
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(applied.durable, expected_durable, "{}", case.name);
        assert_queue_shape(&state, &case.expected_order);
    }

    let rejected = [
        (
            "missing destination",
            4,
            Some(99),
            "queue destination does not exist",
        ),
        (
            "active item cannot move",
            2,
            Some(5),
            "only queued items can be reordered",
        ),
        (
            "finished item cannot move",
            1,
            None,
            "only queued items can be reordered",
        ),
        ("missing item", 99, Some(3), "queue item does not exist"),
    ];
    for (name, item, before, reason) in rejected {
        let mut state = reorder_fixture();
        let applied = apply(&mut state, move_command(item, before));
        assert_eq!(
            applied.reply,
            Reply::Rejected {
                reason: reason.to_owned(),
            },
            "{name}",
        );
        assert!(applied.durable.is_empty(), "{name}");
        assert_queue_shape(&state, &[1, 2, 3, 4, 5]);
    }
}

#[test]
fn reorder_of_the_only_pending_item_above_active_is_a_no_op() {
    let mut state = AppState::default();
    for id in 1..=3 {
        let added = apply(&mut state, add_command(QueueItemId(id), "video.mkv"));
        assert_eq!(added.reply, Reply::Accepted);
    }
    let start = start_session(&mut state);
    assert_eq!(start.reply, Reply::Accepted);
    let first = reserve_and_prepare(&mut state, ClaimId(20), RunId(21), execution());
    assert!(matches!(first.reply, Reply::Claimed(Some(_))));
    let finished = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(20),
            run_id: RunId(21),
            outcome: ItemOutcome::Failed {
                message: "fixture".to_owned(),
            },
            final_telemetry: None,
        }),
    );
    assert_eq!(finished.reply, Reply::Accepted);
    let second = reserve_and_prepare(&mut state, ClaimId(22), RunId(23), execution());
    assert!(matches!(second.reply, Reply::Claimed(Some(_))));

    let applied = apply(&mut state, move_command(3, Some(2)));
    assert_eq!(applied.reply, Reply::Accepted);
    assert!(applied.durable.is_empty());
    assert_queue_shape(&state, &[1, 2, 3]);
}

#[test]
fn policy_uses_post_rotation_pixel_floor_and_typed_av1_decisions() {
    let mut metadata = media_observation("policy").metadata;
    metadata.width = 640;
    metadata.height = 1_000;
    metadata.rotation_degrees = 90;
    assert_eq!(
        evaluate_eligibility(&metadata, Operation::Convert),
        Eligibility::Skip(SkipReason::LowResolution {
            pixels: 640_000,
            minimum: crate::MIN_VIDEO_PIXELS,
        })
    );

    metadata.width = 1_280;
    metadata.height = 720;
    metadata.codec = VideoCodec::Av1;
    metadata.container = MediaContainer::Matroska;
    assert_eq!(
        evaluate_eligibility(&metadata, Operation::Convert),
        Eligibility::Skip(SkipReason::AlreadyAv1Matroska)
    );
    metadata.container = MediaContainer::Other("mov,mp4".to_owned());
    assert_eq!(
        evaluate_eligibility(&metadata, Operation::Convert),
        Eligibility::Remux
    );
    let record = FileRecord::new(metadata.clone());
    assert_eq!(
        select_job_action(
            Some(&metadata),
            Some(&record),
            Operation::Convert,
            &execution()
        ),
        JobAction::Remux
    );
    assert!(matches!(
        select_job_action(
            Some(&metadata),
            Some(&record),
            Operation::Analyze,
            &execution()
        ),
        JobAction::Analyze { .. }
    ));
}

#[test]
fn analysis_selection_prefers_exact_then_lowest_qualifying_target() {
    let settings = execution();
    let mut record = FileRecord::new(media_observation("selection").metadata);
    let mut fallback = analysis();
    fallback.successful_target = VmafTarget(93);
    fallback.requested_target = VmafTarget(95);
    fallback.failed_attempts = vec![crate::AnalysisAttempt {
        target: VmafTarget(95),
        last_measurement: None,
    }];
    let mut exact = analysis();
    exact.successful_target = VmafTarget(95);
    let mut higher = analysis();
    higher.successful_target = VmafTarget(96);
    higher.requested_target = VmafTarget(96);
    record.record_analysis(fallback);
    record.record_analysis(higher);
    record.record_analysis(exact.clone());
    assert_eq!(select_analysis(&record, &settings), Some(exact));

    let mut lower_request = settings;
    lower_request.requested_target = VmafTarget(94);
    let selected = select_analysis(&record, &lower_request).expect("qualifying analysis");
    assert_eq!(selected.successful_target, VmafTarget(95));
}

#[test]
fn media_record_and_analysis_checkpoint_replay_as_one_state() {
    let mut state = AppState::default();
    let mut bytes = Vec::new();
    let mut sequence = 0_u64;
    let _added = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        add_command(QueueItemId(1), "video.mkv"),
    );
    let _started = start_session(&mut state);
    let _reserved = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    let _prepared = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: Some(Box::new(media_observation("durable-content"))),
            execution: execution(),
        }),
    );
    let _recorded = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(analysis()),
        }),
    );
    let _terminal = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed {
                message: "fixture boundary".to_owned(),
            },
            final_telemetry: None,
        }),
    );

    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, state.durable);
    assert_eq!(
        replayed
            .state
            .records
            .get(&ContentKey("durable-content".to_owned()))
            .expect("content record")
            .analyses
            .len(),
        1
    );
}

#[test]
fn stop_after_current_does_not_kill_but_force_stop_does() {
    let mut state = active_state();
    let graceful = apply(
        &mut state,
        Command::Session(SessionCommand::StopAfterCurrent),
    );
    assert!(graceful.effects.is_empty());
    assert_eq!(state.session, SessionState::StopAfterCurrent);
    let forced = apply(&mut state, Command::Session(SessionCommand::ForceStop));
    assert_eq!(
        forced.effects,
        vec![Effect::KillActiveRun { run_id: RunId(3) }]
    );
}

#[test]
fn terminal_is_rejected_until_output_ledger_is_settled() {
    let mut state = active_state();
    let mut started = transaction(OutputState::Started, Replacement::KeepOriginal);
    started.run_id = RunId(3);
    let output = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputStarted {
            transaction: Box::new(started),
        })),
    );
    assert_eq!(output.reply, Reply::Accepted);
    let terminal = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Stopped,
            final_telemetry: None,
        }),
    );
    assert!(matches!(terminal.reply, Reply::Rejected { .. }));
}

#[test]
fn converted_requires_a_successfully_committed_output() {
    let mut state = active_state();
    let without_output = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Converted,
            final_telemetry: None,
        }),
    );
    assert!(matches!(without_output.reply, Reply::Rejected { .. }));

    let mut started = transaction(OutputState::Started, Replacement::KeepOriginal);
    started.run_id = RunId(3);
    assert_eq!(
        apply(
            &mut state,
            Command::Worker(WorkerCommand::Output(OutputDelta::OutputStarted {
                transaction: Box::new(started),
            })),
        )
        .reply,
        Reply::Accepted
    );
    assert_eq!(
        apply(
            &mut state,
            Command::Worker(WorkerCommand::Output(OutputDelta::Conflict {
                run_id: RunId(3),
                reason: "fixture conflict".to_owned(),
            })),
        )
        .reply,
        Reply::Accepted
    );
    let conflicted = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Converted,
            final_telemetry: None,
        }),
    );
    assert!(matches!(conflicted.reply, Reply::Rejected { .. }));
}

#[test]
fn remuxed_requires_a_remux_action_and_committed_output() {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mp4"));
    let _started = start_session(&mut state);
    let _reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    let mut observation = media_observation("remux-content");
    observation.metadata.codec = VideoCodec::Av1;
    observation.metadata.container = MediaContainer::Other("mov,mp4".to_owned());
    let prepared = apply(
        &mut state,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: Some(Box::new(observation)),
            execution: execution(),
        }),
    );
    assert!(matches!(prepared.reply, Reply::Claimed(Some(_))));
    let run = state
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("prepared remux run");
    assert_eq!(run.spec.action, JobAction::Remux);
    let mut output = transaction(
        OutputState::Committed {
            final_identity: identity("remux-output", 20),
        },
        Replacement::KeepOriginal,
    );
    output.run_id = RunId(3);
    assert!(validate_terminal(run, Some(&output), &ItemOutcome::Remuxed).is_ok());
    assert!(validate_terminal(run, Some(&output), &ItemOutcome::Converted).is_err());
}

#[test]
fn settled_outcome_derives_encode_success_conflict_and_stopped_terminals() {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mp4"));
    let _started = start_session(&mut state);
    let _reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    let prepared = apply(
        &mut state,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: None,
            execution: execution(),
        }),
    );
    assert!(matches!(prepared.reply, Reply::Claimed(Some(_))));
    let recorded = apply(
        &mut state,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(analysis()),
        }),
    );
    assert_eq!(recorded.reply, Reply::Accepted);
    let run = state
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("prepared encode run");

    let committed = transaction(
        OutputState::Committed {
            final_identity: identity("output", 20),
        },
        Replacement::KeepOriginal,
    );
    let retired = transaction(
        OutputState::Retired {
            final_identity: identity("output", 20),
        },
        Replacement::RetireOriginal,
    );
    let conflicted = transaction(
        OutputState::Conflict {
            reason: "identity drifted".to_owned(),
        },
        Replacement::RetireOriginal,
    );
    let abandoned = transaction(OutputState::Abandoned, Replacement::KeepOriginal);

    assert_eq!(settled_outcome(run, &committed), ItemOutcome::Converted);
    assert_eq!(settled_outcome(run, &retired), ItemOutcome::Converted);
    assert_eq!(
        settled_outcome(run, &conflicted),
        ItemOutcome::Failed {
            message: "identity drifted".to_owned(),
        }
    );
    assert_eq!(settled_outcome(run, &abandoned), ItemOutcome::Stopped);
    for settled in [&committed, &retired, &conflicted, &abandoned] {
        assert!(validate_terminal(run, Some(settled), &settled_outcome(run, settled)).is_ok());
    }
}

#[test]
fn settled_outcome_labels_settled_remux_success_as_remuxed() {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mp4"));
    let _started = start_session(&mut state);
    let _reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    let mut observation = media_observation("remux-content");
    observation.metadata.codec = VideoCodec::Av1;
    observation.metadata.container = MediaContainer::Other("mov,mp4".to_owned());
    let prepared = apply(
        &mut state,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: Some(Box::new(observation)),
            execution: execution(),
        }),
    );
    assert!(matches!(prepared.reply, Reply::Claimed(Some(_))));
    let run = state
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("prepared remux run");
    assert_eq!(run.spec.action, JobAction::Remux);

    let committed = transaction(
        OutputState::Committed {
            final_identity: identity("remux-output", 20),
        },
        Replacement::KeepOriginal,
    );
    let retired = transaction(
        OutputState::Retired {
            final_identity: identity("remux-output", 20),
        },
        Replacement::RetireOriginal,
    );
    assert_eq!(settled_outcome(run, &committed), ItemOutcome::Remuxed);
    assert_eq!(settled_outcome(run, &retired), ItemOutcome::Remuxed);
    for settled in [&committed, &retired] {
        assert!(validate_terminal(run, Some(settled), &settled_outcome(run, settled)).is_ok());
    }
}

#[test]
fn preparation_rejects_invalid_execution_settings() {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _started = start_session(&mut state);
    let mut invalid = execution();
    invalid.fallback_step = 0;
    let reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    assert!(matches!(reserved.reply, Reply::Reserved(Some(_))));
    let prepared = apply(
        &mut state,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: None,
            execution: invalid,
        }),
    );
    assert!(matches!(prepared.reply, Reply::Rejected { .. }));
}

#[test]
fn reserved_item_can_be_durably_stopped_before_preparation() {
    let mut state = AppState::default();
    let mut durable = Vec::new();
    durable.extend(apply(&mut state, add_command(QueueItemId(1), "video.mkv")).durable);
    let _started = start_session(&mut state);
    durable.extend(
        apply(
            &mut state,
            Command::Worker(WorkerCommand::ReserveNext {
                claim_id: ClaimId(2),
                run_id: RunId(3),
            }),
        )
        .durable,
    );
    let stopped = apply(
        &mut state,
        Command::Worker(WorkerCommand::AbandonReservation {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    assert_eq!(stopped.reply, Reply::Accepted);
    durable.extend(stopped.durable);
    assert!(matches!(
        state.durable.queue.first().expect("queue item").state,
        QueueItemState::Finished(ItemOutcome::Stopped)
    ));

    let envelope = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(0),
        deltas: durable,
    };
    let replayed = replay(&encode_record(&envelope).expect("reservation journal"));
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, state.durable);
}

#[test]
fn output_ledger_rejects_skipped_and_mismatched_transitions() {
    let mut state = active_state();
    let skipped = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputCommitted {
            run_id: RunId(3),
            final_identity: identity("encoded", 8),
        })),
    );
    assert!(matches!(skipped.reply, Reply::Rejected { .. }));

    let mut started = transaction(OutputState::Started, Replacement::KeepOriginal);
    started.run_id = RunId(3);
    let accepted = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputStarted {
            transaction: Box::new(started),
        })),
    );
    assert_eq!(accepted.reply, Reply::Accepted);
    let ready_without_staging = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputReady {
            run_id: RunId(3),
            staging_identity: identity("staging", 8),
        })),
    );
    assert!(matches!(
        ready_without_staging.reply,
        Reply::Rejected { .. }
    ));
    let staging_created = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial: destructive("staging", 0),
        })),
    );
    assert_eq!(staging_created.reply, Reply::Accepted);
    let repeated_staging = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial: destructive("staging", 0),
        })),
    );
    assert!(matches!(repeated_staging.reply, Reply::Rejected { .. }));
    let empty_ready = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputReady {
            run_id: RunId(3),
            staging_identity: identity("staging", 0),
        })),
    );
    assert!(matches!(empty_ready.reply, Reply::Rejected { .. }));
    let foreign_ready = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputReady {
            run_id: RunId(3),
            staging_identity: identity("other", 8),
        })),
    );
    assert!(matches!(foreign_ready.reply, Reply::Rejected { .. }));
    let ready = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputReady {
            run_id: RunId(3),
            staging_identity: identity("staging", 8),
        })),
    );
    assert_eq!(ready.reply, Reply::Accepted);
}

#[test]
fn journal_ignores_only_an_unterminated_final_record() {
    let delta = DurableDelta::QueueAdded {
        item: crate::QueueItem {
            id: QueueItemId(1),
            input: PathBuf::from("one.mkv"),
            operation: Operation::Convert,
            output_target: OutputTarget::Replace,
            state: QueueItemState::Queued,
        },
    };
    let first = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(0),
        deltas: vec![delta.clone()],
    };
    let second = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(1),
        deltas: vec![delta],
    };
    let mut bytes = encode_record(&first).expect("first record");
    let mut torn = encode_record(&second).expect("second record");
    assert_eq!(torn.pop(), Some(b'\n'));
    bytes.extend(torn);
    let replayed = replay(&bytes);
    assert!(replayed.ignored_torn_tail);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state.queue.len(), 1);
    assert_eq!(replayed.next_sequence, JournalSequence(1));
}

#[test]
fn journal_degrades_on_nonfinal_corruption() {
    let first = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(0),
        deltas: vec![DurableDelta::QueueAdded {
            item: crate::QueueItem {
                id: QueueItemId(1),
                input: PathBuf::from("one.mkv"),
                operation: Operation::Convert,
                output_target: OutputTarget::Replace,
                state: QueueItemState::Queued,
            },
        }],
    };
    let mut bytes = encode_record(&first).expect("first record");
    bytes.extend(b"not-json\n");
    bytes.extend(encode_record(&first).expect("following record"));
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_some());
    assert!(!replayed.ignored_torn_tail);
    assert_eq!(replayed.state.queue.len(), 1);
}

#[test]
fn replay_rejects_semantically_impossible_durable_transition() {
    let envelope = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(0),
        deltas: vec![DurableDelta::ItemRunning {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }],
    };
    let replayed = replay(&encode_record(&envelope).expect("journal record"));
    assert!(replayed.corruption.is_some());
    assert!(replayed.state.queue.is_empty());
}

#[test]
fn replay_rejects_an_entire_semantically_invalid_batch() {
    let envelope = JournalEnvelope {
        schema_version: JOURNAL_SCHEMA_VERSION,
        sequence: JournalSequence(0),
        deltas: vec![
            DurableDelta::QueueAdded {
                item: crate::QueueItem {
                    id: QueueItemId(1),
                    input: PathBuf::from("video.mkv"),
                    operation: Operation::Convert,
                    output_target: OutputTarget::Replace,
                    state: QueueItemState::Queued,
                },
            },
            DurableDelta::ItemRunning {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(3),
            },
        ],
    };
    let replayed = replay(&encode_record(&envelope).expect("journal record"));
    assert!(replayed.corruption.is_some());
    assert!(replayed.state.queue.is_empty());
}

#[test]
fn recovery_covers_every_destructive_boundary() {
    let started = transaction(OutputState::Started, Replacement::KeepOriginal);
    let started_facts = FileSystemFacts {
        staging: DestructiveObservation::Present(destructive("empty", 0)),
        final_path: DestructiveObservation::Absent,
        original: DestructiveObservation::Present(destructive("input", 10)),
        staging_artifact: None,
        final_artifact: None,
    };
    assert!(matches!(
        recover_output(&started, &started_facts),
        OutputRecoveryAction::Append(OutputDelta::AbandonStagingIntent { .. })
    ));
    let changed_partial = FileSystemFacts {
        staging: DestructiveObservation::Present(DestructiveIdentity {
            size: 123,
            modified_ns: Some(456),
            ..destructive("empty", 0)
        }),
        ..started_facts.clone()
    };
    assert!(matches!(
        recover_output(&started, &changed_partial),
        OutputRecoveryAction::Append(OutputDelta::AbandonStagingIntent { .. })
    ));
    let absent_staging = FileSystemFacts {
        staging: DestructiveObservation::Absent,
        ..started_facts.clone()
    };
    assert!(matches!(
        recover_output(&started, &absent_staging),
        OutputRecoveryAction::Append(OutputDelta::Abandoned { .. })
    ));

    let staging_created = transaction(
        OutputState::StagingCreated {
            initial: destructive("empty", 0),
        },
        Replacement::KeepOriginal,
    );
    assert!(matches!(
        recover_output(&staging_created, &changed_partial),
        OutputRecoveryAction::Append(OutputDelta::AbandonStagingIntent { .. })
    ));
    assert!(matches!(
        recover_output(&staging_created, &absent_staging),
        OutputRecoveryAction::Append(OutputDelta::Abandoned { .. })
    ));
    let foreign_staging = FileSystemFacts {
        staging: DestructiveObservation::Present(destructive("foreign", 5)),
        ..started_facts.clone()
    };
    assert!(matches!(
        recover_output(&staging_created, &foreign_staging),
        OutputRecoveryAction::Conflict(_)
    ));

    let ready_identity = identity("encoded", 7);
    let ready = transaction(
        OutputState::Ready {
            staging_identity: ready_identity.clone(),
        },
        Replacement::KeepOriginal,
    );
    let before_rename = FileSystemFacts {
        staging: DestructiveObservation::Present(ready_identity.destructive.clone()),
        final_path: DestructiveObservation::Absent,
        original: DestructiveObservation::Present(destructive("input", 10)),
        staging_artifact: Some(ready_identity.clone()),
        final_artifact: None,
    };
    assert!(matches!(
        recover_output(&ready, &before_rename),
        OutputRecoveryAction::Promote { .. }
    ));
    let after_rename = FileSystemFacts {
        staging: DestructiveObservation::Absent,
        final_path: DestructiveObservation::Present(ready_identity.destructive.clone()),
        original: DestructiveObservation::Present(destructive("input", 10)),
        staging_artifact: None,
        final_artifact: Some(ready_identity.clone()),
    };
    assert!(matches!(
        recover_output(&ready, &after_rename),
        OutputRecoveryAction::Append(OutputDelta::OutputCommitted { .. })
    ));

    let retire = transaction(
        OutputState::RetireIntent {
            final_identity: ready_identity.clone(),
        },
        Replacement::RetireOriginal,
    );
    assert!(matches!(
        recover_output(&retire, &after_rename),
        OutputRecoveryAction::DeleteOriginal { .. }
    ));
    let after_delete = FileSystemFacts {
        original: DestructiveObservation::Absent,
        ..after_rename
    };
    assert!(matches!(
        recover_output(&retire, &after_delete),
        OutputRecoveryAction::Append(OutputDelta::OriginalRetired { .. })
    ));
}

proptest! {
    #[test]
    fn replay_equals_live_fold(operations in prop::collection::vec(any::<bool>(), 0..80)) {
        let mut live = AppState::default();
        let mut emitted = Vec::new();
        for (index, remove) in operations.into_iter().enumerate() {
            let item_id = QueueItemId(u64::try_from(index).expect("small generated index"));
            let added = apply(
                &mut live,
                add_command(item_id, PathBuf::from(format!("video-{index}.mkv"))),
            );
            emitted.extend(added.durable);
            if remove {
                let removed = apply(
                    &mut live,
                    Command::Queue(QueueCommand::Remove { item_id }),
                );
                emitted.extend(removed.durable);
            }
        }
        let mut bytes = Vec::new();
        for (sequence, delta) in emitted.into_iter().enumerate() {
            let envelope = JournalEnvelope {
                schema_version: JOURNAL_SCHEMA_VERSION,
                sequence: JournalSequence(u64::try_from(sequence).expect("small sequence")),
                deltas: vec![delta],
            };
            bytes.extend(encode_record(&envelope).expect("serializable delta"));
        }
        let replayed = replay(&bytes);
        prop_assert!(replayed.corruption.is_none());
        prop_assert_eq!(replayed.state, live.durable);
    }
}

fn active_state() -> AppState {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _started = start_session(&mut state);
    let _claimed = reserve_and_prepare(&mut state, ClaimId(2), RunId(3), execution());
    let _running = apply(
        &mut state,
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    let _analysis = apply(
        &mut state,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(analysis()),
        }),
    );
    state
}

fn reserve_and_prepare(
    state: &mut AppState,
    claim_id: ClaimId,
    run_id: RunId,
    execution: ExecutionSettings,
) -> crate::Applied {
    let reserved = apply(
        state,
        Command::Worker(WorkerCommand::ReserveNext { claim_id, run_id }),
    );
    let Reply::Reserved(Some(job)) = reserved.reply else {
        return reserved;
    };
    apply(
        state,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: job.item_id,
            claim_id,
            run_id,
            observation: None,
            execution,
        }),
    )
}

fn apply_and_journal(
    state: &mut AppState,
    bytes: &mut Vec<u8>,
    sequence: &mut u64,
    command: Command,
) -> crate::Applied {
    let applied = apply(state, command);
    if !applied.durable.is_empty() {
        let envelope = JournalEnvelope {
            schema_version: JOURNAL_SCHEMA_VERSION,
            sequence: JournalSequence(*sequence),
            deltas: applied.durable.clone(),
        };
        bytes.extend(encode_record(&envelope).expect("journal delta"));
        *sequence = sequence.saturating_add(1);
    }
    applied
}

#[test]
fn file_record_round_trips_through_json() {
    let mut record = FileRecord::new(VideoMeta {
        codec: VideoCodec::H264,
        container: MediaContainer::Matroska,
        width: 1_280,
        height: 720,
        rotation_degrees: 0,
        duration_ms: 60_000,
    });
    record.record_analysis(analysis());
    // serde_json rejects non-string map keys, so the profile-keyed index must
    // serialize as an entry list; this fails if that representation regresses.
    let encoded = serde_json::to_string(&record).expect("serialize file record");
    let decoded: FileRecord = serde_json::from_str(&encoded).expect("deserialize file record");
    assert_eq!(decoded, record);
}
