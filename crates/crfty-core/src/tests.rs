use std::path::PathBuf;

use proptest::prelude::*;

use crate::reducer::validate_terminal;

use crate::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, AppState, ArtifactIdentity, ClaimId, Command,
    CompletionEvidence, ConflictKind, ContentKey, Crf, DecodeMode, DecodePreference,
    DefaultOutputMode, DestructiveIdentity, DestructiveObservation, DurableDelta, DurationMs,
    Effect, Eligibility, EphemeralDelta, ExecutionSettings, FailureFacts, FailureKind, FileRecord,
    FileStamp, FileSystemFacts, FileSystemId, FileTimeNs, ItemOutcome, JOURNAL_SCHEMA_VERSION,
    JobAction, JobPhase, JobProgress, JournalEnvelope, JournalSequence, MediaContainer,
    MediaObservation, Operation, OutputDelta, OutputRecoveryAction, OutputState, OutputTarget,
    OutputTransaction, PathBinding, PathHash, PhaseSpan, QueueCommand, QueueItemId, QueueItemState,
    Replacement, Reply, RunId, SearchMeasurement, SessionCommand, SessionState, Settings,
    SettingsCommand, SkipReason, StreamByteSizes, SystemCommand, Telemetry, ToolAvailability,
    ToolRevisions, ToolSource, ToolsState, UnixMillis, VendorActivity, VendorCommand, VideoCodec,
    VideoMeta, VmafScore, VmafTarget, WorkerCommand, apply, encode_record, evaluate_eligibility,
    permitted_profiles, recover_output, replay, select_analysis, select_job_action,
};

fn revisions() -> ToolRevisions {
    ToolRevisions {
        ab_av1: "test-ab-av1".to_owned(),
        ffmpeg: "test-ffmpeg".to_owned(),
        encoder: "test-svt".to_owned(),
    }
}

fn available() -> ToolAvailability {
    ToolAvailability::Available {
        source: ToolSource::System,
        revisions: revisions(),
    }
}

fn execution() -> ExecutionSettings {
    let mut profile = AnalysisProfile::production();
    let revisions = revisions();
    profile.ab_av1_revision = revisions.ab_av1;
    profile.ffmpeg_revision = revisions.ffmpeg;
    profile.encoder_revision = revisions.encoder;
    ExecutionSettings::production(profile, false)
}

#[test]
fn base_profile_is_valid_only_until_revisions_are_required() {
    let base = ExecutionSettings::production(AnalysisProfile::production(), false);
    assert_eq!(base.validate_base(), Ok(()));
    assert_eq!(base.validate(), Err("tool revisions must not be empty"));
    assert_eq!(execution().validate(), Ok(()));
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
            availability: available(),
            update_available: false,
        }),
    );
    assert_eq!(discovered.reply, Reply::Accepted);
    assert_eq!(
        discovered.ephemeral,
        vec![EphemeralDelta::ToolsChanged(ToolsState {
            availability: available(),
            activity: VendorActivity::Idle,
            update_available: false,
        })]
    );
    assert_eq!(state.tools.availability, available());

    let unchanged = apply(
        &mut state,
        Command::System(SystemCommand::ToolsDiscovered {
            availability: available(),
            update_available: false,
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
            update_available: false,
        }),
    );
    assert_eq!(missing.reply, Reply::Accepted);
    assert!(matches!(
        state.tools.availability,
        ToolAvailability::Missing { .. }
    ));
}

/// Reports available tools, then starts the session. Nearly every session
/// test needs both because `AppState` defaults to tools-missing (fail-closed).
fn start_session(state: &mut AppState) -> crate::Applied {
    let discovered = apply(
        state,
        Command::System(SystemCommand::ToolsDiscovered {
            availability: available(),
            update_available: false,
        }),
    );
    assert_eq!(discovered.reply, Reply::Accepted);
    apply(state, Command::Session(SessionCommand::Start))
}

#[test]
fn vendor_install_requires_a_fully_idle_engine() {
    // Running session refuses an install.
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _started = start_session(&mut state);
    assert_eq!(state.session, SessionState::Running);
    let refused = apply(&mut state, Command::Vendor(VendorCommand::Install));
    assert_eq!(
        refused.reply,
        Reply::Rejected {
            reason: "vendor install requires an idle session".to_owned(),
        }
    );
    assert!(refused.effects.is_empty());
    assert_eq!(state.tools.activity, VendorActivity::Idle);

    // Idle session but a crash-recovered active item still refuses.
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _started = start_session(&mut state);
    let _reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    state.session = SessionState::Idle;
    let refused = apply(&mut state, Command::Vendor(VendorCommand::Install));
    assert_eq!(
        refused.reply,
        Reply::Rejected {
            reason: "vendor install cannot start while a queue item is active".to_owned(),
        }
    );

    // Fully idle accepts: activity transitions and the effect is emitted.
    let mut state = AppState::default();
    let accepted = apply(&mut state, Command::Vendor(VendorCommand::Install));
    assert_eq!(accepted.reply, Reply::Accepted);
    assert_eq!(accepted.effects, vec![Effect::VendorInstall]);
    assert_eq!(
        state.tools.activity,
        VendorActivity::Downloading {
            received: 0,
            total: None,
        }
    );

    // A second install while the first is in flight is refused.
    let refused = apply(&mut state, Command::Vendor(VendorCommand::Install));
    assert_eq!(
        refused.reply,
        Reply::Rejected {
            reason: "a vendor operation is already in progress".to_owned(),
        }
    );
    assert!(refused.effects.is_empty());
}

#[test]
fn vendor_activity_serializes_workers_and_failed_is_restartable() {
    let mut state = AppState::default();
    let checking = apply(&mut state, Command::Vendor(VendorCommand::Check));
    assert_eq!(checking.reply, Reply::Accepted);
    assert_eq!(checking.effects, vec![Effect::VendorCheck]);
    assert_eq!(state.tools.activity, VendorActivity::Checking);

    // Checking blocks both vendor commands: at most one worker exists.
    for command in [VendorCommand::Install, VendorCommand::Check] {
        let refused = apply(&mut state, Command::Vendor(command));
        assert_eq!(
            refused.reply,
            Reply::Rejected {
                reason: "a vendor operation is already in progress".to_owned(),
            }
        );
    }

    // A failed operation is terminal for the worker, so both restart paths
    // reopen from it.
    let failed = apply(
        &mut state,
        Command::System(SystemCommand::VendorProgress {
            activity: VendorActivity::Failed {
                detail: "checksum mismatch".to_owned(),
            },
        }),
    );
    assert_eq!(failed.reply, Reply::Accepted);
    assert!(matches!(
        state.tools.activity,
        VendorActivity::Failed { .. }
    ));
    let retried = apply(&mut state, Command::Vendor(VendorCommand::Install));
    assert_eq!(retried.reply, Reply::Accepted);
    assert_eq!(retried.effects, vec![Effect::VendorInstall]);
}

#[test]
fn session_start_is_refused_while_the_vendor_swaps_tools() {
    for activity in [
        VendorActivity::Downloading {
            received: 5,
            total: Some(10),
        },
        VendorActivity::Installing,
    ] {
        let mut state = AppState::default();
        let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
        let discovered = apply(
            &mut state,
            Command::System(SystemCommand::ToolsDiscovered {
                availability: available(),
                update_available: false,
            }),
        );
        assert_eq!(discovered.reply, Reply::Accepted);
        let progressed = apply(
            &mut state,
            Command::System(SystemCommand::VendorProgress {
                activity: activity.clone(),
            }),
        );
        assert_eq!(progressed.reply, Reply::Accepted);
        let refused = apply(&mut state, Command::Session(SessionCommand::Start));
        assert_eq!(
            refused.reply,
            Reply::Rejected {
                reason: "a vendor install is in progress".to_owned(),
            },
            "start accepted during {activity:?}"
        );
        assert_eq!(state.session, SessionState::Idle);
    }

    // Checking does not block a start: it swaps no binaries.
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _discovered = apply(
        &mut state,
        Command::System(SystemCommand::ToolsDiscovered {
            availability: available(),
            update_available: false,
        }),
    );
    let _checking = apply(
        &mut state,
        Command::System(SystemCommand::VendorProgress {
            activity: VendorActivity::Checking,
        }),
    );
    let started = apply(&mut state, Command::Session(SessionCommand::Start));
    assert_eq!(started.reply, Reply::Accepted);
}

#[test]
fn vendor_progress_and_discovery_compose_without_clobbering_each_other() {
    let mut state = AppState::default();
    let _install = apply(&mut state, Command::Vendor(VendorCommand::Install));
    let progressed = apply(
        &mut state,
        Command::System(SystemCommand::VendorProgress {
            activity: VendorActivity::Downloading {
                received: 1_024,
                total: Some(4_096),
            },
        }),
    );
    assert_eq!(
        progressed.ephemeral,
        vec![EphemeralDelta::ToolsChanged(ToolsState {
            availability: ToolAvailability::default(),
            activity: VendorActivity::Downloading {
                received: 1_024,
                total: Some(4_096),
            },
            update_available: false,
        })]
    );

    // Discovery mid-flight (the post-install report) preserves the activity.
    let discovered = apply(
        &mut state,
        Command::System(SystemCommand::ToolsDiscovered {
            availability: ToolAvailability::Available {
                source: ToolSource::Managed,
                revisions: revisions(),
            },
            update_available: false,
        }),
    );
    assert_eq!(discovered.reply, Reply::Accepted);
    assert_eq!(
        state.tools.activity,
        VendorActivity::Downloading {
            received: 1_024,
            total: Some(4_096),
        }
    );
    assert_eq!(
        state.tools.availability,
        ToolAvailability::Available {
            source: ToolSource::Managed,
            revisions: revisions(),
        }
    );

    // Identical progress re-report emits nothing.
    let unchanged = apply(
        &mut state,
        Command::System(SystemCommand::VendorProgress {
            activity: VendorActivity::Downloading {
                received: 1_024,
                total: Some(4_096),
            },
        }),
    );
    assert!(unchanged.ephemeral.is_empty());
}

fn add_command(item_id: QueueItemId, input: impl Into<PathBuf>) -> Command {
    Command::Queue(QueueCommand::Add {
        item_id,
        input: input.into(),
        operation: Operation::Convert,
        intent: AnalysisIntent::ReuseIfFresh,
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
        modified_ns: Some(FileTimeNs(size)),
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
                modified_ns: Some(FileTimeNs(1)),
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
            size_bytes: 10_000,
            audio: vec![crate::AudioStreamMeta {
                codec: crate::AudioCodec::Aac,
                channels: 2,
            }],
            subtitle_count: 0,
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
            at: UnixMillis(1_000),
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
            outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
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
            outcome: ItemOutcome::Failed(FailureFacts::new(
                FailureKind::Internal,
                "fixture boundary",
            )),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
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
            outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
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
            outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
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
            AnalysisIntent::ReuseIfFresh,
            &execution()
        ),
        JobAction::Remux
    );
    assert!(matches!(
        select_job_action(
            Some(&metadata),
            Some(&record),
            Operation::Analyze,
            AnalysisIntent::ReuseIfFresh,
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
            outcome: ItemOutcome::Failed(FailureFacts::new(
                FailureKind::Internal,
                "fixture boundary",
            )),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
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
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
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
            outcome: ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
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
                kind: ConflictKind::InspectionFailed,
                detail: "fixture conflict".to_owned(),
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
            outcome: ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
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
    let running = apply(
        &mut state,
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            at: UnixMillis(1_000),
        }),
    );
    assert_eq!(running.reply, Reply::Accepted);
    let run = state
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("prepared remux run");
    assert_eq!(run.spec.action, JobAction::Remux);
    assert_eq!(run.started_at, Some(UnixMillis(1_000)));
    let mut output = transaction(
        OutputState::Committed {
            final_identity: identity("remux-output", 20),
        },
        Replacement::KeepOriginal,
    );
    output.run_id = RunId(3);
    let live_remux = ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
        input_size: 10_000,
        output_size: 20,
    });
    assert!(validate_terminal(run, Some(&output), &live_remux).is_ok());
    assert!(
        validate_terminal(
            run,
            Some(&output),
            &ItemOutcome::Remuxed(CompletionEvidence::RecoveredAtStartup),
        )
        .is_ok()
    );
    assert!(
        validate_terminal(
            run,
            Some(&output),
            &ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
        )
        .is_err()
    );
    // Evidence provenance must match the outcome kind: a remux cannot carry
    // adapter encode facts and vice versa.
    assert!(
        validate_terminal(
            run,
            Some(&output),
            &ItemOutcome::Remuxed(CompletionEvidence::LiveEncode {
                input_size: 10_000,
                output_size: 20,
                stream_sizes: StreamByteSizes {
                    video: 10,
                    audio: 5,
                    subtitle: 0,
                    other: 5,
                },
                encode_decode: DecodeMode::Software,
            }),
        )
        .is_err()
    );
}

#[test]
fn successful_outcomes_require_a_started_run_and_matching_evidence() {
    // Claimed but never Started: a live success is impossible from this state,
    // and recovery derives Stopped for it — Converted must be rejected.
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _started = start_session(&mut state);
    let _claimed = reserve_and_prepare(&mut state, ClaimId(2), RunId(3), execution());
    let _analysis = apply(
        &mut state,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(analysis()),
        }),
    );
    let run = state
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("claimed run");
    assert_eq!(run.started_at, None);
    let mut output = transaction(
        OutputState::Committed {
            final_identity: identity("encoded", 20),
        },
        Replacement::KeepOriginal,
    );
    output.run_id = RunId(3);
    assert!(
        validate_terminal(
            run,
            Some(&output),
            &ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
        )
        .is_err()
    );
    // Converted with remux-shaped evidence is a provenance lie even when the
    // run is otherwise valid.
    let started_state = active_state();
    let started_run = started_state
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("started run");
    assert!(started_run.started_at.is_some());
    assert!(
        validate_terminal(
            started_run,
            Some(&output),
            &ItemOutcome::Converted(CompletionEvidence::LiveRemux {
                input_size: 10_000,
                output_size: 20,
            }),
        )
        .is_err()
    );
    assert!(
        validate_terminal(
            started_run,
            Some(&output),
            &ItemOutcome::Converted(CompletionEvidence::LiveEncode {
                input_size: 10_000,
                output_size: 20,
                stream_sizes: StreamByteSizes {
                    video: 15,
                    audio: 3,
                    subtitle: 1,
                    other: 1,
                },
                encode_decode: DecodeMode::Software,
            }),
        )
        .is_ok()
    );
}

#[test]
fn run_facts_fold_start_finish_instants_and_phase_spans() {
    let mut state = active_state();
    let spans = vec![
        PhaseSpan {
            phase: JobPhase::Preparing,
            duration: DurationMs(40),
        },
        PhaseSpan {
            phase: JobPhase::Analyzing,
            duration: DurationMs(65_000),
        },
    ];
    let terminal = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
            at: UnixMillis(2_000),
            phase_spans: spans.clone(),
            final_telemetry: None,
        }),
    );
    assert_eq!(terminal.reply, Reply::Accepted);
    let run = state
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("finished run");
    assert_eq!(run.started_at, Some(UnixMillis(1_000)));
    assert_eq!(run.finished_at, Some(UnixMillis(2_000)));
    assert_eq!(run.phase_spans, spans);
}

#[test]
fn failed_terminal_invariants_check_conflict_state_and_diagnostic_bound() {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
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
            observation: Some(Box::new(media_observation("failed-content"))),
            execution: execution(),
        }),
    );
    assert!(matches!(prepared.reply, Reply::Claimed(Some(_))));
    let run = state
        .durable
        .conversion_runs
        .get(&RunId(3))
        .expect("prepared run");

    let conflict_failure = ItemOutcome::Failed(FailureFacts::new(
        FailureKind::OutputConflict,
        "output settled as a conflict",
    ));
    // OutputConflict asserts a conflicted transaction; without one it lies.
    assert!(validate_terminal(run, None, &conflict_failure).is_err());
    let mut conflicted = transaction(
        OutputState::Conflict {
            kind: ConflictKind::InspectionFailed,
            detail: "fixture".to_owned(),
        },
        Replacement::KeepOriginal,
    );
    conflicted.run_id = RunId(3);
    assert!(validate_terminal(run, Some(&conflicted), &conflict_failure).is_ok());
    // The converse is NOT an invariant: a conflicted settlement followed by a
    // Stopped terminal stays legal (cancellation racing a settlement failure).
    assert!(validate_terminal(run, Some(&conflicted), &ItemOutcome::Stopped).is_ok());
    // Other failure kinds carry no output requirement.
    let plain_failure = ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture"));
    assert!(validate_terminal(run, None, &plain_failure).is_ok());

    // Replay-side bound enforcement: transparent deserialization can produce
    // an oversized tail, which the terminal validation must reject.
    let oversized = format!("\"{}\"", "a".repeat(crate::DIAGNOSTIC_TAIL_MAX_BYTES + 1));
    let tail: crate::DiagnosticTail =
        serde_json::from_str(&oversized).expect("transparent deserialization succeeds");
    let unbounded = ItemOutcome::Failed(
        FailureFacts::new(FailureKind::Internal, "fixture").with_diagnostic(tail),
    );
    assert!(validate_terminal(run, None, &unbounded).is_err());
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
            at: UnixMillis(1_000),
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
    // A repeated StagingCreated is legal: it is the encode retry's restage
    // (the pin moves to the recreated staging artifact).
    let repeated_staging = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial: destructive("staging", 0),
        })),
    );
    assert_eq!(repeated_staging.reply, Reply::Accepted);
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
            intent: AnalysisIntent::ReuseIfFresh,
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
                intent: AnalysisIntent::ReuseIfFresh,
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
            at: UnixMillis(1_000),
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
                    intent: AnalysisIntent::ReuseIfFresh,
                    output_target: OutputTarget::Replace,
                    state: QueueItemState::Queued,
                },
            },
            DurableDelta::ItemRunning {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(3),
                at: UnixMillis(1_000),
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
            modified_ns: Some(FileTimeNs(456)),
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
            at: UnixMillis(1_000),
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

/// Fold-level verdict fixture: media observed, run prepared against the
/// content, and an output transaction inserted directly in the given state
/// (the structural fold does not validate transitions).
fn verdict_fixture(
    run: u64,
    key: Option<&str>,
    output: Option<OutputState>,
) -> crate::DurableState {
    let mut state = crate::DurableState::default();
    let deltas = [
        Some(DurableDelta::MediaObserved {
            observation: Box::new(media_observation("verdict-content")),
        }),
        Some(DurableDelta::ItemPrepared {
            spec: Box::new(crate::JobSpec {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(run),
                input: PathBuf::from("video.mkv"),
                content_key: key.map(|key| ContentKey(key.to_owned())),
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                execution: execution(),
                action: JobAction::Encode {
                    selected_analysis: None,
                },
            }),
        }),
        output.map(|state| {
            let mut transaction = transaction(state, Replacement::KeepOriginal);
            transaction.run_id = RunId(run);
            DurableDelta::Output(OutputDelta::OutputStarted {
                transaction: Box::new(transaction),
            })
        }),
    ];
    for delta in deltas.into_iter().flatten() {
        crate::fold(&mut state, &delta);
    }
    state
}

fn fold_finished(state: &mut crate::DurableState, run: u64, outcome: ItemOutcome, at: u64) {
    crate::fold(
        state,
        &DurableDelta::ItemFinished {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(run),
            outcome,
            at: UnixMillis(at),
            phase_spans: Vec::new(),
        },
    );
}

#[test]
fn decisive_outcomes_upsert_the_record_verdict() {
    let key = "verdict-content";
    let record_verdict = |state: &crate::DurableState| {
        state
            .records
            .get(&ContentKey(key.to_owned()))
            .expect("content record")
            .verdict
            .clone()
    };

    // A settled success names its output content and decides the record.
    let mut state = verdict_fixture(
        3,
        Some(key),
        Some(OutputState::Committed {
            final_identity: identity("ck-out", 20),
        }),
    );
    fold_finished(
        &mut state,
        3,
        ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
        5_000,
    );
    assert_eq!(
        record_verdict(&state),
        Some(crate::Verdict {
            kind: crate::VerdictKind::Converted {
                output_content_key: ContentKey("ck-out".to_owned()),
            },
            source_run: RunId(3),
            decided_at: UnixMillis(5_000),
        })
    );

    // An unsettled success cannot name its artifact: no verdict.
    let mut unsettled = verdict_fixture(3, Some(key), Some(OutputState::Started));
    fold_finished(
        &mut unsettled,
        3,
        ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
        5_000,
    );
    assert_eq!(record_verdict(&unsettled), None);

    // Not-worthwhile records the summary targets from the run's execution.
    let mut skipped = verdict_fixture(3, Some(key), None);
    fold_finished(
        &mut skipped,
        3,
        ItemOutcome::NotWorthwhile {
            attempts: Vec::new(),
        },
        6_000,
    );
    assert_eq!(
        record_verdict(&skipped),
        Some(crate::Verdict {
            kind: crate::VerdictKind::NotWorthwhile {
                requested: execution().requested_target,
                floor: execution().fallback_floor,
            },
            source_run: RunId(3),
            decided_at: UnixMillis(6_000),
        })
    );

    // Indecisive outcomes leave the standing verdict untouched.
    fold_finished(
        &mut skipped,
        3,
        ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
        7_000,
    );
    assert_eq!(
        record_verdict(&skipped).map(|verdict| verdict.decided_at),
        Some(UnixMillis(6_000))
    );

    // Without a content key there is nothing to decide about.
    let mut keyless = verdict_fixture(3, None, None);
    fold_finished(
        &mut keyless,
        3,
        ItemOutcome::NotWorthwhile {
            attempts: Vec::new(),
        },
        6_000,
    );
    assert_eq!(record_verdict(&keyless), None);
}

#[test]
fn latest_decisive_run_wins_the_verdict() {
    let key = "verdict-content";
    let mut state = verdict_fixture(
        3,
        Some(key),
        Some(OutputState::Committed {
            final_identity: identity("ck-out", 20),
        }),
    );
    fold_finished(
        &mut state,
        3,
        ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
        5_000,
    );
    // A later run against the same content re-decides it.
    crate::fold(
        &mut state,
        &DurableDelta::ItemPrepared {
            spec: Box::new(crate::JobSpec {
                item_id: QueueItemId(2),
                claim_id: ClaimId(8),
                run_id: RunId(9),
                input: PathBuf::from("video.mkv"),
                content_key: Some(ContentKey(key.to_owned())),
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                execution: execution(),
                action: JobAction::Encode {
                    selected_analysis: None,
                },
            }),
        },
    );
    crate::fold(
        &mut state,
        &DurableDelta::ItemFinished {
            item_id: QueueItemId(2),
            claim_id: ClaimId(8),
            run_id: RunId(9),
            outcome: ItemOutcome::NotWorthwhile {
                attempts: Vec::new(),
            },
            at: UnixMillis(9_000),
            phase_spans: Vec::new(),
        },
    );
    let verdict = state
        .records
        .get(&ContentKey(key.to_owned()))
        .expect("content record")
        .verdict
        .clone()
        .expect("standing verdict");
    assert_eq!(verdict.source_run, RunId(9));
    assert!(matches!(
        verdict.kind,
        crate::VerdictKind::NotWorthwhile { .. }
    ));
}

#[test]
fn verdict_freshness_is_a_stamp_match_against_the_settled_output() {
    let converted = crate::Verdict {
        kind: crate::VerdictKind::Converted {
            output_content_key: ContentKey("ck-out".to_owned()),
        },
        source_run: RunId(3),
        decided_at: UnixMillis(5_000),
    };
    let settled = destructive("out", 42);
    let matching = FileStamp {
        size: 42,
        modified_ns: settled.modified_ns,
    };
    assert!(crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&matching)
    ));
    // Changed size or modification time: the file is no longer the output.
    assert!(!crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&FileStamp {
            size: 43,
            modified_ns: settled.modified_ns,
        })
    ));
    assert!(!crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&FileStamp {
            size: 42,
            modified_ns: Some(FileTimeNs(7)),
        })
    ));
    // One side missing its modification time is not a match.
    assert!(!crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&FileStamp {
            size: 42,
            modified_ns: None,
        })
    ));
    // Both sides unknown degrade to a size-only match.
    let unstamped = DestructiveIdentity {
        modified_ns: None,
        ..destructive("out", 42)
    };
    assert!(crate::verdict_applies(
        &converted,
        Some(&unstamped),
        Some(&FileStamp {
            size: 42,
            modified_ns: None,
        })
    ));
    // Missing file or pruned/unsettled transaction: no answer, not fresh.
    assert!(!crate::verdict_applies(&converted, Some(&settled), None));
    assert!(!crate::verdict_applies(&converted, None, Some(&matching)));

    // Not-worthwhile is content-keyed; path state is irrelevant.
    let not_worthwhile = crate::Verdict {
        kind: crate::VerdictKind::NotWorthwhile {
            requested: VmafTarget(95),
            floor: VmafTarget(90),
        },
        source_run: RunId(3),
        decided_at: UnixMillis(5_000),
    };
    assert!(crate::verdict_applies(&not_worthwhile, None, None));
}

#[test]
fn refresh_intent_forces_a_new_search_over_a_qualifying_cached_analysis() {
    let mut record = FileRecord::new(media_observation("refresh-content").metadata);
    record.record_analysis(analysis());
    assert!(matches!(
        select_job_action(
            None,
            Some(&record),
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
            &execution()
        ),
        JobAction::Encode {
            selected_analysis: Some(_)
        }
    ));
    assert_eq!(
        select_job_action(
            None,
            Some(&record),
            Operation::Convert,
            AnalysisIntent::Refresh,
            &execution()
        ),
        JobAction::Encode {
            selected_analysis: None
        }
    );

    // End to end: a Refresh item prepared against cached content journals a
    // spec without reuse, and replay recomputes the same intent-aware action.
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
            observation: Some(Box::new(media_observation("refresh-content"))),
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
    let _finished = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
            final_telemetry: None,
        }),
    );
    let _refresh_added = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Queue(QueueCommand::Add {
            item_id: QueueItemId(4),
            input: PathBuf::from("video.mkv"),
            operation: Operation::Convert,
            intent: AnalysisIntent::Refresh,
            output_target: OutputTarget::Replace,
        }),
    );
    let _reserved = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(5),
            run_id: RunId(6),
        }),
    );
    let prepared = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(4),
            claim_id: ClaimId(5),
            run_id: RunId(6),
            observation: Some(Box::new(media_observation("refresh-content"))),
            execution: execution(),
        }),
    );
    let Reply::Claimed(Some(job)) = prepared.reply else {
        panic!("expected refreshed claim");
    };
    assert_eq!(job.spec.intent, AnalysisIntent::Refresh);
    assert_eq!(
        job.spec.action,
        JobAction::Encode {
            selected_analysis: None
        }
    );
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, state.durable);
}

#[test]
fn software_fallback_analysis_is_permitted_under_a_hardware_spec() {
    // The pure gate: a software-prepared run has no wider ladder; a
    // hardware-prepared run also accepts its software-decode variant.
    let software = execution();
    assert_eq!(
        permitted_profiles(&software),
        vec![software.profile.clone()]
    );
    let mut hardware = execution();
    hardware.profile.decode_mode = DecodeMode::Hardware(crate::HardwareDecoder::H264Cuvid);
    let mut fallback_profile = hardware.profile.clone();
    fallback_profile.decode_mode = DecodeMode::Software;
    assert_eq!(
        permitted_profiles(&hardware),
        vec![hardware.profile.clone(), fallback_profile.clone()]
    );

    // Reducer + replay: the software-fallback result is recorded under the
    // hardware spec; the journal replays it through the same widened gate.
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
    let prepared = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: Some(Box::new(media_observation("ladder-content"))),
            execution: hardware.clone(),
        }),
    );
    let Reply::Claimed(Some(job)) = prepared.reply else {
        panic!("expected hardware-prepared claim");
    };
    assert_eq!(
        job.spec.execution.profile.decode_mode,
        DecodeMode::Hardware(crate::HardwareDecoder::H264Cuvid)
    );
    // An unrelated profile is still rejected.
    let mut wrong = analysis();
    wrong.profile = hardware.profile.clone();
    wrong.profile.preset = wrong.profile.preset.saturating_add(1);
    let rejected = apply(
        &mut state,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(wrong),
        }),
    );
    assert!(matches!(rejected.reply, Reply::Rejected { .. }));
    let mut fallback = analysis();
    fallback.profile = fallback_profile;
    let recorded = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(fallback),
        }),
    );
    assert_eq!(recorded.reply, Reply::Accepted);
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, state.durable);

    // The ladder is one-directional: a software-prepared run never records a
    // hardware result.
    let _finished = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
            final_telemetry: None,
        }),
    );
    let _added = apply(&mut state, add_command(QueueItemId(4), "again.mkv"));
    let _claimed = reserve_and_prepare(&mut state, ClaimId(5), RunId(6), software);
    let mut hardware_result = analysis();
    hardware_result.profile.decode_mode = DecodeMode::Hardware(crate::HardwareDecoder::H264Cuvid);
    let rejected = apply(
        &mut state,
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(4),
            claim_id: ClaimId(5),
            run_id: RunId(6),
            result: Box::new(hardware_result),
        }),
    );
    assert!(matches!(rejected.reply, Reply::Rejected { .. }));
}

#[test]
fn analyses_recorded_under_hardware_decode_are_not_reused_elsewhere() {
    // The decode mode is part of the analysis identity (ADR-007): hardware
    // and software measurements are not interchangeable, and the pin is
    // decoder-granular — a Cuvid analysis re-searches under Qsv.
    let mut hardware = execution();
    hardware.profile.decode_mode = DecodeMode::Hardware(crate::HardwareDecoder::H264Cuvid);
    let mut result = analysis();
    result.profile = hardware.profile.clone();
    let mut record = FileRecord::new(media_observation("pin-content").metadata);
    record.record_analysis(result.clone());
    assert_eq!(select_analysis(&record, &execution()), None);
    let mut qsv = hardware.clone();
    qsv.profile.decode_mode = DecodeMode::Hardware(crate::HardwareDecoder::H264Qsv);
    assert_eq!(select_analysis(&record, &qsv), None);
    assert_eq!(select_analysis(&record, &hardware), Some(result));
}

#[test]
fn restage_moves_the_staging_pin_and_is_refused_after_abandonment() {
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
    for command in [
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: None,
            execution: execution(),
        }),
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            at: UnixMillis(1_000),
        }),
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            result: Box::new(analysis()),
        }),
    ] {
        let applied = apply_and_journal(&mut state, &mut bytes, &mut sequence, command);
        assert!(!matches!(applied.reply, Reply::Rejected { .. }));
    }
    let mut started = transaction(OutputState::Started, Replacement::KeepOriginal);
    started.run_id = RunId(3);
    let _output = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputStarted {
            transaction: Box::new(started),
        })),
    );
    let _created = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial: destructive("empty", 0),
        })),
    );
    // The ready pin holds: an artifact on a different file id is rejected.
    let stale = apply(
        &mut state,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputReady {
            run_id: RunId(3),
            staging_identity: identity("wrong", 5),
        })),
    );
    assert!(matches!(stale.reply, Reply::Rejected { .. }));
    // The encode retry's restage: a repeated StagingCreated moves the pin to
    // the recreated staging artifact.
    let restaged = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial: destructive("fresh", 0),
        })),
    );
    assert_eq!(restaged.reply, Reply::Accepted);
    assert_eq!(
        state
            .durable
            .outputs
            .get(&RunId(3))
            .expect("restaged transaction")
            .state,
        OutputState::StagingCreated {
            initial: destructive("fresh", 0),
        }
    );
    // The ready pin follows the recreated staging file.
    let ready = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputReady {
            run_id: RunId(3),
            staging_identity: ArtifactIdentity {
                content_key: ContentKey("ck-retry".to_owned()),
                destructive: destructive("fresh", 9),
            },
        })),
    );
    assert_eq!(ready.reply, Reply::Accepted);
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, state.durable);

    // Retry-after-abandonment is unrepresentable: a settled transaction
    // refuses to restage.
    let mut abandoned_state = active_state();
    let mut abandoned = transaction(OutputState::Started, Replacement::KeepOriginal);
    abandoned.run_id = RunId(3);
    for delta in [
        OutputDelta::OutputStarted {
            transaction: Box::new(abandoned),
        },
        OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial: destructive("empty", 0),
        },
        OutputDelta::AbandonStagingIntent {
            run_id: RunId(3),
            staging_identity: destructive("empty", 0),
        },
        OutputDelta::Abandoned { run_id: RunId(3) },
    ] {
        let applied = apply(
            &mut abandoned_state,
            Command::Worker(WorkerCommand::Output(delta)),
        );
        assert_eq!(applied.reply, Reply::Accepted);
    }
    let refused = apply(
        &mut abandoned_state,
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(3),
            initial: destructive("fresh", 0),
        })),
    );
    assert!(matches!(refused.reply, Reply::Rejected { .. }));
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
        size_bytes: 10_000,
        audio: vec![crate::AudioStreamMeta {
            codec: crate::AudioCodec::Eac3,
            channels: 6,
        }],
        subtitle_count: 2,
    });
    record.record_analysis(analysis());
    record.verdict = Some(crate::Verdict {
        kind: crate::VerdictKind::Remuxed {
            output_content_key: ContentKey("ck-out".to_owned()),
        },
        source_run: RunId(11),
        decided_at: UnixMillis(4_000),
    });
    // serde_json rejects non-string map keys, so the profile-keyed index must
    // serialize as an entry list; this fails if that representation regresses.
    let encoded = serde_json::to_string(&record).expect("serialize file record");
    let decoded: FileRecord = serde_json::from_str(&encoded).expect("deserialize file record");
    assert_eq!(decoded, record);
}
