use std::path::PathBuf;

use proptest::prelude::*;

use crate::reducer::validate_terminal;

use crate::{
    AnalysisActivity, AnalysisCommand, AnalysisDisplayText, AnalysisEntryKind,
    AnalysisGenerationId, AnalysisIntent, AnalysisProfile, AnalysisResult, AnalysisRow,
    AnalysisRowId, AppState, ArtifactIdentity, COMPACTION_HARD_LIMIT_BYTES,
    COMPACTION_IDLE_MIN_JOURNAL_BYTES, COMPACTION_IDLE_MIN_RATIO, ClaimId, Command,
    CompletionEvidence, ConflictKind, ContentKey, Crf, DecodeMode, DecodePreference,
    DefaultOutputMode, DestructiveIdentity, DestructiveObservation, DurableDelta, DurationMs,
    Effect, Eligibility, EphemeralDelta, ExecutionSettings, FailureFacts, FailureKind, FileRecord,
    FileSystemFacts, FileSystemId, FileTimeNs, HistoryCommand, ImportPath, ImportedHistoryRecord,
    ImportedProvenance, ItemOutcome, JOURNAL_SCHEMA_VERSION, JobAction, JobPhase, JobProgress,
    JournalEnvelope, JournalSequence, MediaContainer, MediaObservation, Operation, OutputDelta,
    OutputRecoveryAction, OutputState, OutputTarget, OutputTransaction, OverwriteDecision,
    ParkedResolution, ParkedStatus, PathBinding, PathHash, PhaseSpan, ProjectionCommand,
    QueueAddRequest, QueueCommand, QueueItemEdit, QueueItemId, QueueItemState, Replacement, Reply,
    RunId, SearchMeasurement, SessionAggregates, SessionCommand, SessionState, Settings,
    SettingsCommand, SkipReason, StreamByteSizes, SystemCommand, Telemetry, ToolAvailability,
    ToolRevisions, ToolSource, ToolsState, UnixMillis, VendorActivity, VendorCommand, VideoCodec,
    VideoMeta, VmafScore, VmafTarget, WorkerCommand, apply, compaction_due, compaction_quiescent,
    corruption_signature, encode_record, encode_snapshot, evaluate_eligibility, permitted_profiles,
    recover_output, replay, resolve_parked, select_analysis, select_job_action,
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
fn analysis_commands_allocate_generations_and_reject_late_same_generation_work() {
    let mut state = AppState::default();
    let root = |name: &str| AnalysisDisplayText {
        text: name.to_owned(),
        lossy: false,
    };
    let started = apply(
        &mut state,
        Command::Analysis(AnalysisCommand::Begin {
            root: root("first"),
        }),
    );
    assert_eq!(
        started.reply,
        Reply::AnalysisStarted {
            generation: AnalysisGenerationId(1),
        }
    );
    let row = AnalysisRow {
        id: AnalysisRowId(1),
        parent: None,
        kind: AnalysisEntryKind::File,
        display_name: root("movie.mkv"),
        display_path: root("first/movie.mkv"),
    };
    let inserted = apply(
        &mut state,
        Command::Analysis(AnalysisCommand::UpsertRows {
            generation: AnalysisGenerationId(1),
            rows: vec![row],
        }),
    );
    assert_eq!(inserted.reply, Reply::Accepted);
    assert_eq!(
        state
            .analysis
            .current
            .as_ref()
            .expect("current generation")
            .rows
            .len(),
        1
    );
    assert_eq!(
        apply(
            &mut state,
            Command::Analysis(AnalysisCommand::SetActivity {
                generation: AnalysisGenerationId(1),
                activity: AnalysisActivity::Discovered,
            }),
        )
        .reply,
        Reply::Accepted
    );

    let late = apply(
        &mut state,
        Command::Analysis(AnalysisCommand::UpsertRows {
            generation: AnalysisGenerationId(1),
            rows: Vec::new(),
        }),
    );
    assert!(matches!(late.reply, Reply::Rejected { .. }));

    let next = apply(
        &mut state,
        Command::Analysis(AnalysisCommand::Begin {
            root: root("second"),
        }),
    );
    assert_eq!(
        next.reply,
        Reply::AnalysisStarted {
            generation: AnalysisGenerationId(2),
        }
    );
    let stale_completion = apply(
        &mut state,
        Command::Analysis(AnalysisCommand::SetActivity {
            generation: AnalysisGenerationId(1),
            activity: AnalysisActivity::Ready,
        }),
    );
    assert!(matches!(stale_completion.reply, Reply::Rejected { .. }));
    assert_eq!(
        state
            .analysis
            .current
            .as_ref()
            .map(|generation| generation.id),
        Some(AnalysisGenerationId(2))
    );
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
    Command::Queue(QueueCommand::AddMany {
        requests: vec![add_request(item_id, input)],
    })
}

/// A convert add with no enqueue facts: absent facts fail open, so the item
/// always enqueues (unless its path is already queued).
fn add_request(item_id: QueueItemId, input: impl Into<PathBuf>) -> QueueAddRequest {
    QueueAddRequest {
        item_id,
        input: input.into(),
        path_hash: None,
        identity: None,
        timestamp_reliability: crate::TimestampReliability::Unknown,
        operation: Operation::Convert,
        intent: AnalysisIntent::ReuseIfFresh,
        output_target: OutputTarget::Replace,
        overwrite: OverwriteDecision::FollowSettings,
    }
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
            identity: DestructiveIdentity {
                modified_ns: Some(FileTimeNs(1)),
                ..destructive(content, 10_000)
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
fn moved_or_duplicated_paths_join_the_probable_content_record_after_observation() {
    let first = media_observation("shared-content");
    let mut second = first.clone();
    second.path_hash = PathHash("path-shared-content-copy".to_owned());
    second.binding.identity = DestructiveIdentity {
        file_id: FileSystemId::Unix {
            device: 1,
            inode: 999,
        },
        ..second.binding.identity
    };
    let mut durable = crate::DurableState::default();
    for observation in [first, second] {
        crate::fold(
            &mut durable,
            &DurableDelta::MediaObserved {
                observation: Box::new(observation),
            },
        );
    }

    assert_eq!(durable.paths.len(), 2);
    assert_eq!(durable.records.len(), 1);
    assert!(
        durable
            .paths
            .values()
            .all(|binding| binding.content_key == ContentKey("shared-content".to_owned()))
    );
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
                fps_centi: None,
                eta_ms: None,
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
            import_paths: Vec::new(),
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
            import_paths: Vec::new(),
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
        let added = apply(
            &mut state,
            add_command(QueueItemId(id), format!("video-{id}.mkv")),
        );
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
        let added = apply(
            &mut state,
            add_command(QueueItemId(id), format!("video-{id}.mkv")),
        );
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
            import_paths: Vec::new(),
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
            import_paths: Vec::new(),
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
fn session_aggregates_absorb_counts_every_outcome_and_live_evidence() {
    let mut aggregates = SessionAggregates::default();
    aggregates.absorb(&ItemOutcome::Analyzed, &[]);
    aggregates.absorb(
        &ItemOutcome::Converted(CompletionEvidence::LiveEncode {
            input_size: 1_000,
            output_size: 400,
            stream_sizes: StreamByteSizes {
                video: 300,
                audio: 80,
                subtitle: 15,
                other: 5,
            },
            encode_decode: DecodeMode::Software,
        }),
        &[
            PhaseSpan {
                phase: JobPhase::Analyzing,
                duration: DurationMs(30_000),
            },
            PhaseSpan {
                phase: JobPhase::Encoding,
                duration: DurationMs(90_000),
            },
        ],
    );
    aggregates.absorb(
        &ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
            input_size: 500,
            output_size: 480,
        }),
        &[],
    );
    // A crash-recovered success counts the item but measured no bytes.
    aggregates.absorb(
        &ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
        &[],
    );
    aggregates.absorb(
        &ItemOutcome::NotWorthwhile {
            attempts: Vec::new(),
        },
        &[],
    );
    aggregates.absorb(&ItemOutcome::Stopped, &[]);
    aggregates.absorb(
        &ItemOutcome::Skipped {
            reason: SkipReason::OutputExists,
        },
        &[],
    );
    aggregates.absorb(
        &ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
        &[],
    );
    assert_eq!(
        aggregates,
        SessionAggregates {
            completed: 2,
            failed: 1,
            skipped: 1,
            stopped: 1,
            not_worthwhile: 1,
            analyzed: 1,
            remuxed: 1,
            input_bytes: 1_500,
            output_bytes: 880,
            encode_duration_ms: 90_000,
        }
    );
}

#[test]
fn session_start_zeroes_the_aggregates_for_the_new_run() {
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    // Leftover totals from a previous session are display state, not history.
    state.aggregates.completed = 5;
    let started = start_session(&mut state);
    assert_eq!(started.reply, Reply::Accepted);
    assert_eq!(
        started.ephemeral,
        vec![
            EphemeralDelta::SessionChanged(SessionState::Running),
            EphemeralDelta::SessionAggregates(SessionAggregates::default()),
        ]
    );
    assert_eq!(state.aggregates, SessionAggregates::default());
}

#[test]
fn terminal_and_abandonment_emit_the_updated_aggregates() {
    let mut state = active_state();
    let terminal = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
            at: UnixMillis(2_000),
            phase_spans: vec![PhaseSpan {
                phase: JobPhase::Encoding,
                duration: DurationMs(1_500),
            }],
            final_telemetry: None,
        }),
    );
    assert_eq!(terminal.reply, Reply::Accepted);
    let expected = SessionAggregates {
        failed: 1,
        encode_duration_ms: 1_500,
        ..SessionAggregates::default()
    };
    assert_eq!(
        terminal.ephemeral.last(),
        Some(&EphemeralDelta::SessionAggregates(expected))
    );
    assert_eq!(state.aggregates, expected);

    // An abandoned reservation finishes as Stopped and counts as one.
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
    let expected = SessionAggregates {
        stopped: 1,
        ..SessionAggregates::default()
    };
    assert_eq!(
        stopped.ephemeral,
        vec![EphemeralDelta::SessionAggregates(expected)]
    );
    assert_eq!(state.aggregates, expected);
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
            import_paths: Vec::new(),
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
            import_paths: Vec::new(),
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
            overwrite: OverwriteDecision::FollowSettings,
            state: QueueItemState::Queued,
        },
    };
    let first = JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![delta.clone()],
    };
    let second = JournalEnvelope {
        sequence: JournalSequence(1),
        deltas: vec![delta],
    };
    let mut bytes = encode_record(&first).expect("first record");
    let intact_len = bytes.len();
    let mut torn = encode_record(&second).expect("second record");
    assert_eq!(torn.pop(), Some(b'\n'));
    bytes.extend(torn);
    let replayed = replay(&bytes);
    assert!(replayed.ignored_torn_tail);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state.queue.len(), 1);
    assert_eq!(replayed.next_sequence, JournalSequence(1));
    assert_eq!(replayed.valid_prefix_len, intact_len);
}

#[test]
fn journal_degrades_on_nonfinal_corruption() {
    let first = JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![DurableDelta::QueueAdded {
            item: crate::QueueItem {
                id: QueueItemId(1),
                input: PathBuf::from("one.mkv"),
                operation: Operation::Convert,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                overwrite: OverwriteDecision::FollowSettings,
                state: QueueItemState::Queued,
            },
        }],
    };
    let mut bytes = encode_record(&first).expect("first record");
    let intact_len = bytes.len();
    bytes.extend(b"not-json\n");
    bytes.extend(encode_record(&first).expect("following record"));
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_some());
    assert!(!replayed.ignored_torn_tail);
    assert_eq!(replayed.state.queue.len(), 1);
    assert_eq!(replayed.valid_prefix_len, intact_len);
}

#[test]
fn acknowledge_corruption_never_applies_through_the_reducer() {
    let mut state = AppState::default();
    let applied = apply(
        &mut state,
        Command::System(SystemCommand::AcknowledgeCorruption {
            signature: corruption_signature(b"unreadable"),
        }),
    );
    assert!(matches!(applied.reply, Reply::Rejected { .. }));
    assert!(applied.durable.is_empty());
    assert!(applied.config.is_empty());
    assert!(applied.effects.is_empty());
    assert_eq!(state, AppState::default());
}

#[test]
fn corruption_signature_covers_the_whole_unreadable_suffix() {
    let mut bytes = encode_record(&JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![queue_added_delta(1, "one.mkv")],
    })
    .expect("head record");
    let intact_len = bytes.len();
    // Corrupt record followed by a torn fragment: the signature must span
    // both, because acknowledgement discards everything past the valid prefix.
    let suffix: &[u8] = b"not-json\n{\"torn";
    bytes.extend(suffix);
    let replayed = replay(&bytes);
    let corruption = replayed.corruption.expect("corrupt journal");
    assert_eq!(corruption.offset, intact_len);
    assert_eq!(corruption.signature, corruption_signature(suffix));
    assert_eq!(corruption.signature.tail_len, suffix.len() as u64);
    assert_eq!(corruption.signature.digest.len(), 128);
    assert!(
        corruption
            .signature
            .digest
            .bytes()
            .all(|c| c.is_ascii_hexdigit())
    );
}

#[test]
fn different_corrupt_tails_yield_different_signatures() {
    let head = encode_record(&JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![queue_added_delta(1, "one.mkv")],
    })
    .expect("head record");
    let mut first = head.clone();
    first.extend(b"garbage-one\n");
    let mut second = head;
    second.extend(b"garbage-two\n");
    let first_signature = replay(&first).corruption.expect("first corrupt").signature;
    let second_signature = replay(&second)
        .corruption
        .expect("second corrupt")
        .signature;
    assert_ne!(first_signature, second_signature);
}

#[test]
fn the_same_corrupt_tail_at_different_offsets_shares_a_signature() {
    let suffix: &[u8] = b"not-json\n";
    let mut short = encode_record(&JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![queue_added_delta(1, "one.mkv")],
    })
    .expect("head record");
    let mut long = short.clone();
    long.extend(
        encode_record(&JournalEnvelope {
            sequence: JournalSequence(1),
            deltas: vec![queue_added_delta(2, "two.mkv")],
        })
        .expect("second record"),
    );
    short.extend(suffix);
    long.extend(suffix);
    let short_corruption = replay(&short).corruption.expect("short corrupt");
    let long_corruption = replay(&long).corruption.expect("long corrupt");
    // Identity is content-based: the same unreadable bytes sign identically
    // wherever they sit, while the reports remain distinct via the offset.
    assert_eq!(short_corruption.signature, long_corruption.signature);
    assert_ne!(short_corruption, long_corruption);
}

fn queue_added_delta(id: u64, name: &str) -> DurableDelta {
    DurableDelta::QueueAdded {
        item: crate::QueueItem {
            id: QueueItemId(id),
            input: PathBuf::from(name),
            operation: Operation::Convert,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Replace,
            overwrite: OverwriteDecision::FollowSettings,
            state: QueueItemState::Queued,
        },
    }
}

#[test]
fn snapshot_head_seeds_state_and_continues_sequence_numbering() {
    let mut compacted = crate::DurableState::default();
    crate::fold(&mut compacted, &queue_added_delta(1, "one.mkv"));
    let mut bytes = encode_snapshot("1.2.3", UnixMillis(1_000), JournalSequence(7), &compacted)
        .expect("snapshot");
    bytes.extend(
        encode_record(&JournalEnvelope {
            sequence: JournalSequence(7),
            deltas: vec![queue_added_delta(2, "two.mkv")],
        })
        .expect("tail record"),
    );
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert!(!replayed.ignored_torn_tail);
    assert_eq!(replayed.state.queue.len(), 2);
    assert_eq!(replayed.next_sequence, JournalSequence(8));
    assert_eq!(replayed.valid_prefix_len, bytes.len());
}

#[test]
fn replay_rejects_a_snapshot_after_the_journal_head() {
    let mut bytes = encode_record(&JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![queue_added_delta(1, "one.mkv")],
    })
    .expect("head record");
    let intact_len = bytes.len();
    bytes.extend(
        encode_snapshot(
            "1.2.3",
            UnixMillis(1_000),
            JournalSequence(1),
            &crate::DurableState::default(),
        )
        .expect("snapshot"),
    );
    let replayed = replay(&bytes);
    let corruption = replayed.corruption.expect("snapshot after head");
    assert_eq!(corruption.offset, intact_len);
    assert!(corruption.reason.contains("snapshot"));
    assert_eq!(replayed.state.queue.len(), 1);
    assert_eq!(replayed.valid_prefix_len, intact_len);
}

#[test]
fn replay_rejects_a_mismatched_sequence_after_a_snapshot() {
    let mut bytes = encode_snapshot(
        "1.2.3",
        UnixMillis(1_000),
        JournalSequence(7),
        &crate::DurableState::default(),
    )
    .expect("snapshot");
    let intact_len = bytes.len();
    bytes.extend(
        encode_record(&JournalEnvelope {
            sequence: JournalSequence(0),
            deltas: vec![queue_added_delta(1, "one.mkv")],
        })
        .expect("stale record"),
    );
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_some());
    assert_eq!(replayed.next_sequence, JournalSequence(7));
    assert_eq!(replayed.valid_prefix_len, intact_len);
}

#[test]
fn replay_reports_old_and_future_schema_versions_before_decoding_the_payload() {
    for unsupported in [JOURNAL_SCHEMA_VERSION - 1, JOURNAL_SCHEMA_VERSION + 1] {
        let line =
            format!("{{\"schema_version\":{unsupported},\"record\":{{\"Unknown\":null}}}}\n");
        let replayed = replay(line.as_bytes());
        let corruption = replayed.corruption.expect("unsupported schema");
        assert_eq!(
            corruption.reason,
            format!("unsupported journal schema {unsupported}")
        );
    }
}

#[test]
fn torn_tail_after_a_snapshot_still_seeds_the_snapshot_state() {
    let mut state = crate::DurableState::default();
    crate::fold(&mut state, &queue_added_delta(1, "one.mkv"));
    let mut bytes =
        encode_snapshot("1.2.3", UnixMillis(1_000), JournalSequence(3), &state).expect("snapshot");
    let intact_len = bytes.len();
    let mut torn = encode_record(&JournalEnvelope {
        sequence: JournalSequence(3),
        deltas: vec![queue_added_delta(2, "two.mkv")],
    })
    .expect("torn record");
    assert_eq!(torn.pop(), Some(b'\n'));
    bytes.extend(torn);
    let replayed = replay(&bytes);
    assert!(replayed.ignored_torn_tail);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, state);
    assert_eq!(replayed.next_sequence, JournalSequence(3));
    assert_eq!(replayed.valid_prefix_len, intact_len);
}

#[test]
fn compacting_a_replayed_journal_preserves_state_and_sequence() {
    let mut bytes = Vec::new();
    for sequence in 0..3_u64 {
        bytes.extend(
            encode_record(&JournalEnvelope {
                sequence: JournalSequence(sequence),
                deltas: vec![queue_added_delta(sequence + 1, &format!("{sequence}.mkv"))],
            })
            .expect("journal record"),
        );
    }
    let original = replay(&bytes);
    assert!(original.corruption.is_none());
    let mut compacted = encode_snapshot(
        "1.2.3",
        UnixMillis(1_000),
        original.next_sequence,
        &original.state,
    )
    .expect("snapshot");
    let tail = encode_record(&JournalEnvelope {
        sequence: original.next_sequence,
        deltas: vec![queue_added_delta(9, "after.mkv")],
    })
    .expect("tail record");
    bytes.extend(tail.clone());
    compacted.extend(tail);
    let replayed_original = replay(&bytes);
    let replayed_compacted = replay(&compacted);
    assert!(replayed_original.corruption.is_none());
    assert!(replayed_compacted.corruption.is_none());
    assert_eq!(replayed_compacted.state, replayed_original.state);
    assert_eq!(
        replayed_compacted.next_sequence,
        replayed_original.next_sequence
    );
}

#[test]
fn compaction_size_policy_requires_floor_and_ratio_or_hard_cap() {
    assert!(!compaction_due(COMPACTION_IDLE_MIN_JOURNAL_BYTES - 1, 0));
    assert!(compaction_due(
        COMPACTION_IDLE_MIN_JOURNAL_BYTES,
        COMPACTION_IDLE_MIN_JOURNAL_BYTES / COMPACTION_IDLE_MIN_RATIO
    ));
    assert!(!compaction_due(
        COMPACTION_IDLE_MIN_JOURNAL_BYTES,
        COMPACTION_IDLE_MIN_JOURNAL_BYTES
    ));
    assert!(compaction_due(COMPACTION_HARD_LIMIT_BYTES, u64::MAX));
}

#[test]
fn compaction_waits_for_an_idle_session_and_settled_queue() {
    let mut settled = AppState::default();
    crate::fold(&mut settled.durable, &queue_added_delta(1, "one.mkv"));
    assert!(compaction_quiescent(&settled));
    assert!(!compaction_quiescent(&active_state()));
}

#[test]
fn replay_rejects_semantically_impossible_durable_transition() {
    let envelope = JournalEnvelope {
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
        sequence: JournalSequence(0),
        deltas: vec![
            DurableDelta::QueueAdded {
                item: crate::QueueItem {
                    id: QueueItemId(1),
                    input: PathBuf::from("video.mkv"),
                    operation: Operation::Convert,
                    intent: AnalysisIntent::ReuseIfFresh,
                    output_target: OutputTarget::Replace,
                    overwrite: OverwriteDecision::FollowSettings,
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
fn replay_rejects_adoption_when_carried_facts_do_not_match_parked_record() {
    let import_path = ImportPath("c:/videos/imported.mkv".to_owned());
    let parked = parked_record(ParkedStatus::Converted);
    let mut mismatched = parked.clone();
    mismatched.output_size = Some(123);
    let observation = media_observation("imported-content");
    let envelope = JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![
            DurableDelta::HistoryImported {
                records: vec![(import_path.clone(), parked)],
            },
            DurableDelta::MediaObserved {
                observation: Box::new(observation.clone()),
            },
            DurableDelta::ParkedAdopted {
                import_path,
                content_key: observation.binding.content_key,
                imported: mismatched,
                verdict: None,
            },
        ],
    };

    let replayed = replay(&encode_record(&envelope).expect("journal record"));
    assert!(replayed.corruption.is_some());
    assert_eq!(replayed.state, crate::DurableState::default());
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
            import_paths: Vec::new(),
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
                output_content_key: Some(ContentKey("ck-out".to_owned())),
                input_size: None,
                output_size: None,
                encoding_time: None,
                crf: None,
                vmaf: None,
                target: None,
            },
            source_run: Some(RunId(3)),
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
            source_run: Some(RunId(3)),
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
    assert_eq!(verdict.source_run, Some(RunId(9)));
    assert!(matches!(
        verdict.kind,
        crate::VerdictKind::NotWorthwhile { .. }
    ));
}

#[test]
fn verdict_freshness_is_a_full_identity_match_against_the_settled_output() {
    let converted = crate::Verdict {
        kind: crate::VerdictKind::Converted {
            output_content_key: Some(ContentKey("ck-out".to_owned())),
            input_size: None,
            output_size: None,
            encoding_time: None,
            crf: None,
            vmaf: None,
            target: None,
        },
        source_run: Some(RunId(3)),
        decided_at: UnixMillis(5_000),
    };
    let settled = destructive("out", 42);
    assert!(crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&settled),
        crate::TimestampReliability::Reliable,
    ));
    assert!(!crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&settled),
        crate::TimestampReliability::CoarseOrRecent,
    ));
    // Changed size or modification time: the file is no longer the output.
    assert!(!crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&DestructiveIdentity {
            size: 43,
            ..settled.clone()
        }),
        crate::TimestampReliability::Reliable,
    ));
    assert!(!crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&DestructiveIdentity {
            modified_ns: Some(FileTimeNs(7)),
            ..settled.clone()
        }),
        crate::TimestampReliability::Reliable,
    ));
    // One side missing its modification time is not a match.
    assert!(!crate::verdict_applies(
        &converted,
        Some(&settled),
        Some(&DestructiveIdentity {
            modified_ns: None,
            ..settled.clone()
        }),
        crate::TimestampReliability::Reliable,
    ));
    // Even full file-id/size equality is conservative when mtime is unknown.
    let unstamped = DestructiveIdentity {
        modified_ns: None,
        ..destructive("out", 42)
    };
    assert!(!crate::verdict_applies(
        &converted,
        Some(&unstamped),
        Some(&unstamped),
        crate::TimestampReliability::Reliable,
    ));
    // Missing file or pruned/unsettled transaction: no answer, not fresh.
    assert!(!crate::verdict_applies(
        &converted,
        Some(&settled),
        None,
        crate::TimestampReliability::Reliable,
    ));
    assert!(!crate::verdict_applies(
        &converted,
        None,
        Some(&settled),
        crate::TimestampReliability::Reliable,
    ));

    // Not-worthwhile is content-keyed; path state is irrelevant.
    let not_worthwhile = crate::Verdict {
        kind: crate::VerdictKind::NotWorthwhile {
            requested: VmafTarget(95),
            floor: VmafTarget(90),
        },
        source_run: Some(RunId(3)),
        decided_at: UnixMillis(5_000),
    };
    assert!(crate::verdict_applies(
        &not_worthwhile,
        None,
        None,
        crate::TimestampReliability::Unknown,
    ));
}

/// A durable state with one observed path/record, optionally carrying a
/// standing verdict. Returns the bound path hash and its full destructive
/// identity (the "file is unchanged" probe).
fn enqueue_fixture(
    verdict: Option<crate::Verdict>,
) -> (crate::DurableState, PathHash, DestructiveIdentity) {
    let observation = media_observation("enqueue-content");
    let path_hash = observation.path_hash.clone();
    let identity = observation.binding.identity.clone();
    let content_key = observation.binding.content_key.clone();
    let mut durable = crate::DurableState::default();
    crate::fold(
        &mut durable,
        &DurableDelta::MediaObserved {
            observation: Box::new(observation),
        },
    );
    if let Some(verdict) = verdict {
        durable
            .records
            .get_mut(&content_key)
            .expect("observed record")
            .verdict = Some(verdict);
    }
    (durable, path_hash, identity)
}

fn converted_verdict(source_run: RunId) -> crate::Verdict {
    crate::Verdict {
        kind: crate::VerdictKind::Converted {
            output_content_key: Some(ContentKey("ck-out".to_owned())),
            input_size: None,
            output_size: None,
            encoding_time: None,
            crf: None,
            vmaf: None,
            target: None,
        },
        source_run: Some(source_run),
        decided_at: UnixMillis(5_000),
    }
}

fn not_worthwhile_verdict(source_run: RunId) -> crate::Verdict {
    let execution = execution();
    crate::Verdict {
        kind: crate::VerdictKind::NotWorthwhile {
            requested: execution.requested_target,
            floor: execution.fallback_floor,
        },
        source_run: Some(source_run),
        decided_at: UnixMillis(5_000),
    }
}

#[test]
fn enqueue_disposition_is_verdict_aware_and_identity_gated() {
    // Unknown path: nothing cached, nothing to skip on.
    assert_eq!(
        crate::evaluate_enqueue(
            &crate::DurableState::default(),
            &PathHash("path-unknown".to_owned()),
            None,
            crate::TimestampReliability::Unknown,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        None
    );

    let (durable, path_hash, identity) = enqueue_fixture(Some(not_worthwhile_verdict(RunId(9))));
    let cases: [(
        Option<&DestructiveIdentity>,
        Operation,
        AnalysisIntent,
        Option<SkipReason>,
    ); 4] = [
        (
            Some(&identity),
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
            Some(SkipReason::NotWorthwhile {
                source_run: Some(RunId(9)),
            }),
        ),
        // Analyze adds are never verdict-filtered.
        (
            Some(&identity),
            Operation::Analyze,
            AnalysisIntent::ReuseIfFresh,
            None,
        ),
        // Refresh is the explicit escape hatch.
        (
            Some(&identity),
            Operation::Convert,
            AnalysisIntent::Refresh,
            None,
        ),
        // A missing identity cannot establish the binding still describes the
        // file, so no verdict-based skip fires.
        (None, Operation::Convert, AnalysisIntent::ReuseIfFresh, None),
    ];
    for (identity, operation, intent, expected) in cases {
        assert_eq!(
            crate::evaluate_enqueue(
                &durable,
                &path_hash,
                identity,
                crate::TimestampReliability::Reliable,
                operation,
                intent,
            ),
            expected,
            "operation {operation:?} intent {intent:?}"
        );
    }

    // A changed file at the known path (stale identity) is re-queueable.
    let changed = DestructiveIdentity {
        size: identity.size + 1,
        ..identity.clone()
    };
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&changed),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        None
    );

    // Fresh binding + Converted verdict: the content itself was already
    // processed, so re-converting is duplicate work.
    let (durable, path_hash, identity) = enqueue_fixture(Some(converted_verdict(RunId(9))));
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&identity),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        Some(SkipReason::ProbableDuplicate {
            source_run: Some(RunId(9)),
        })
    );
    for reliability in [
        crate::TimestampReliability::Unknown,
        crate::TimestampReliability::CoarseOrRecent,
    ] {
        assert_eq!(
            crate::evaluate_enqueue(
                &durable,
                &path_hash,
                Some(&identity),
                reliability,
                Operation::Convert,
                AnalysisIntent::ReuseIfFresh,
            ),
            None
        );
    }
}

#[test]
fn enqueue_recognizes_the_replace_mode_output_without_a_fresh_binding() {
    // Replace-mode aftermath: the binding still names the ORIGINAL content
    // (source identity size 10_000), but the file now at the path is the settled output of
    // run 9 (size 42). Recognition rides on the output identity alone.
    let (mut durable, path_hash, _identity) = enqueue_fixture(Some(converted_verdict(RunId(9))));
    durable.outputs.insert(
        RunId(9),
        transaction(
            OutputState::Retired {
                final_identity: identity("out", 42),
            },
            Replacement::RetireOriginal,
        ),
    );
    let output_identity = destructive("out", 42);
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&output_identity),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        Some(SkipReason::AlreadyConverted {
            source_run: Some(RunId(9)),
        })
    );
    for reliability in [
        crate::TimestampReliability::Unknown,
        crate::TimestampReliability::CoarseOrRecent,
    ] {
        assert_eq!(
            crate::evaluate_enqueue(
                &durable,
                &path_hash,
                Some(&output_identity),
                reliability,
                Operation::Convert,
                AnalysisIntent::ReuseIfFresh,
            ),
            None
        );
    }
    // Refresh bypasses even output recognition.
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&output_identity),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::Refresh,
        ),
        None
    );
    // A file that matches neither the output nor the original binding is new
    // content at a known path: accept.
    let unrelated = destructive("unrelated", 43);
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&unrelated),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        None
    );
    // An unsettled or pruned transaction has no artifact to answer for.
    durable.outputs.insert(
        RunId(9),
        transaction(
            OutputState::Ready {
                staging_identity: identity("out", 42),
            },
            Replacement::RetireOriginal,
        ),
    );
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&output_identity),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        None
    );
}

#[test]
fn enqueue_filters_ineligible_cached_metadata_only_when_fresh() {
    let (mut durable, path_hash, identity) = enqueue_fixture(None);
    {
        let record = durable
            .records
            .get_mut(&ContentKey("enqueue-content".to_owned()))
            .expect("observed record");
        record.metadata.codec = VideoCodec::Av1;
        record.metadata.container = MediaContainer::Matroska;
    }
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&identity),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        Some(SkipReason::AlreadyAv1Matroska)
    );
    // Media-fact skips ignore intent: refreshing cannot make a file eligible.
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&identity),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::Refresh,
        ),
        Some(SkipReason::AlreadyAv1Matroska)
    );
    // Stale identity: the cached metadata no longer answers for the file.
    let changed = DestructiveIdentity {
        size: identity.size + 1,
        ..identity.clone()
    };
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&changed),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        None
    );
    // Av1 outside Matroska is remux-eligible work, not a skip.
    {
        let record = durable
            .records
            .get_mut(&ContentKey("enqueue-content".to_owned()))
            .expect("observed record");
        record.metadata.container = MediaContainer::Other("mp4".to_owned());
    }
    assert_eq!(
        crate::evaluate_enqueue(
            &durable,
            &path_hash,
            Some(&identity),
            crate::TimestampReliability::Reliable,
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
        ),
        None
    );
}

#[test]
fn claim_short_circuits_content_with_a_decisive_verdict() {
    let metadata = media_observation("claim-content").metadata;
    let mut record = FileRecord::new(metadata.clone());
    record.verdict = Some(converted_verdict(RunId(4)));

    assert_eq!(
        select_job_action(
            Some(&metadata),
            Some(&record),
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
            &execution(),
        ),
        JobAction::Skip {
            reason: SkipReason::ProbableDuplicate {
                source_run: Some(RunId(4)),
            },
        }
    );
    // Refresh and Analyze both proceed past the verdict.
    assert_eq!(
        select_job_action(
            Some(&metadata),
            Some(&record),
            Operation::Convert,
            AnalysisIntent::Refresh,
            &execution(),
        ),
        JobAction::Encode {
            selected_analysis: None,
        }
    );
    assert_eq!(
        select_job_action(
            Some(&metadata),
            Some(&record),
            Operation::Analyze,
            AnalysisIntent::ReuseIfFresh,
            &execution(),
        ),
        JobAction::Analyze {
            selected_analysis: None,
        }
    );
    // A media-fact skip outranks the verdict.
    let mut small = metadata.clone();
    small.width = 640;
    small.height = 480;
    assert_eq!(
        select_job_action(
            Some(&small),
            Some(&record),
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
            &execution(),
        ),
        JobAction::Skip {
            reason: SkipReason::LowResolution {
                pixels: 640 * 480,
                minimum: crate::MIN_VIDEO_PIXELS,
            },
        }
    );
    // The verdict outranks the remux branch: re-remuxing already-processed
    // content is still duplicate work.
    let mut av1_mp4 = metadata.clone();
    av1_mp4.codec = VideoCodec::Av1;
    av1_mp4.container = MediaContainer::Other("mp4".to_owned());
    assert_eq!(
        select_job_action(
            Some(&av1_mp4),
            Some(&record),
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
            &execution(),
        ),
        JobAction::Skip {
            reason: SkipReason::ProbableDuplicate {
                source_run: Some(RunId(4)),
            },
        }
    );
    record.verdict = None;
    assert_eq!(
        select_job_action(
            Some(&av1_mp4),
            Some(&record),
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
            &execution(),
        ),
        JobAction::Remux
    );
}

#[test]
fn claim_not_worthwhile_skip_respects_the_fallback_floor() {
    let metadata = media_observation("claim-floor").metadata;
    let mut record = FileRecord::new(metadata.clone());
    record.verdict = Some(not_worthwhile_verdict(RunId(4)));
    let execution = execution();
    assert_eq!(
        select_job_action(
            Some(&metadata),
            Some(&record),
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
            &execution,
        ),
        JobAction::Skip {
            reason: SkipReason::NotWorthwhile {
                source_run: Some(RunId(4)),
            },
        }
    );
    // A ladder reaching below the decided floor is untried ground.
    let mut deeper = execution.clone();
    deeper.fallback_floor = VmafTarget(execution.fallback_floor.0 - 1);
    assert_eq!(
        select_job_action(
            Some(&metadata),
            Some(&record),
            Operation::Convert,
            AnalysisIntent::ReuseIfFresh,
            &deeper,
        ),
        JobAction::Encode {
            selected_analysis: None,
        }
    );
}

#[test]
fn enqueue_time_skip_reasons_are_never_terminal_outcomes() {
    let spec = crate::JobSpec {
        item_id: QueueItemId(1),
        claim_id: ClaimId(2),
        run_id: RunId(3),
        input: PathBuf::from("video.mkv"),
        content_key: None,
        operation: Operation::Convert,
        intent: AnalysisIntent::ReuseIfFresh,
        output_target: OutputTarget::Replace,
        execution: execution(),
        action: JobAction::Encode {
            selected_analysis: None,
        },
    };
    let run = crate::ConversionRun {
        spec,
        analysis: None,
        output_content_key: None,
        outcome: None,
        started_at: None,
        finished_at: None,
        phase_spans: Vec::new(),
    };
    for reason in [
        SkipReason::AlreadyQueued,
        SkipReason::AlreadyConverted {
            source_run: Some(RunId(9)),
        },
    ] {
        assert!(
            validate_terminal(
                &run,
                None,
                &ItemOutcome::Skipped {
                    reason: reason.clone()
                }
            )
            .is_err(),
            "{reason:?} must be rejected as a terminal outcome"
        );
    }
}

#[test]
fn add_many_judges_each_request_and_reports_one_summary() {
    // A fresh-bound path whose content carries a Converted verdict — the
    // enqueue tier's ProbableDuplicate case.
    let (durable, path_hash, identity) = enqueue_fixture(Some(converted_verdict(RunId(9))));
    let mut state = AppState {
        durable,
        ..AppState::default()
    };
    let batch = Command::Queue(QueueCommand::AddMany {
        requests: vec![
            // Fresh binding + decisive verdict: filtered at add.
            QueueAddRequest {
                path_hash: Some(path_hash.clone()),
                identity: Some(identity.clone()),
                timestamp_reliability: crate::TimestampReliability::Reliable,
                ..add_request(QueueItemId(1), "known.mkv")
            },
            // No enqueue facts: fails open and enqueues.
            add_request(QueueItemId(2), "new.mkv"),
            // Same path as a request accepted earlier in this batch.
            add_request(QueueItemId(3), "new.mkv"),
            // Refresh bypasses the verdict skip even on the known content.
            QueueAddRequest {
                path_hash: Some(path_hash),
                identity: Some(identity),
                timestamp_reliability: crate::TimestampReliability::Reliable,
                intent: AnalysisIntent::Refresh,
                ..add_request(QueueItemId(4), "known-again.mkv")
            },
        ],
    });
    let applied = apply(&mut state, batch);
    assert_eq!(applied.reply, Reply::Accepted);
    let added_ids: Vec<QueueItemId> = state.durable.queue.iter().map(|item| item.id).collect();
    assert_eq!(added_ids, vec![QueueItemId(2), QueueItemId(4)]);
    assert_eq!(
        applied.ephemeral,
        vec![EphemeralDelta::QueueAddSummary {
            added: 2,
            skipped: vec![
                (
                    SkipReason::ProbableDuplicate {
                        source_run: Some(RunId(9))
                    },
                    1
                ),
                (SkipReason::AlreadyQueued, 1),
            ],
        }]
    );
}

#[test]
fn add_many_skips_a_queued_path_until_it_finishes() {
    let mut state = AppState::default();
    let first = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    assert_eq!(first.reply, Reply::Accepted);
    // The path is pending: a re-add counts into the summary and adds nothing.
    let readd = apply(&mut state, add_command(QueueItemId(2), "video.mkv"));
    assert_eq!(readd.reply, Reply::Accepted);
    assert!(readd.durable.is_empty());
    assert_eq!(
        readd.ephemeral,
        vec![EphemeralDelta::QueueAddSummary {
            added: 0,
            skipped: vec![(SkipReason::AlreadyQueued, 1)],
        }]
    );
    // Once the standing item is finished, the same path enqueues again.
    state.durable.queue[0].state = QueueItemState::Finished(ItemOutcome::Stopped);
    let after_finish = apply(&mut state, add_command(QueueItemId(3), "video.mkv"));
    assert_eq!(after_finish.durable.len(), 1);
    assert_eq!(state.durable.queue.len(), 2);
}

#[test]
fn add_many_rejects_the_whole_batch_on_an_item_id_collision() {
    let mut state = AppState::default();
    let _first = apply(&mut state, add_command(QueueItemId(1), "one.mkv"));
    for requests in [
        vec![add_request(QueueItemId(1), "two.mkv")],
        vec![
            add_request(QueueItemId(2), "two.mkv"),
            add_request(QueueItemId(2), "three.mkv"),
        ],
    ] {
        let applied = apply(
            &mut state,
            Command::Queue(QueueCommand::AddMany { requests }),
        );
        assert!(matches!(applied.reply, Reply::Rejected { .. }));
        assert!(applied.durable.is_empty());
    }
    assert_eq!(state.durable.queue.len(), 1);
}

#[test]
fn add_many_is_allowed_while_a_session_runs() {
    let mut state = active_state();
    assert_eq!(state.session, SessionState::Running);
    let applied = apply(&mut state, add_command(QueueItemId(9), "late.mkv"));
    assert_eq!(applied.reply, Reply::Accepted);
    assert_eq!(applied.durable.len(), 1);
}

#[test]
fn prepare_resolves_overwrite_from_the_item_not_the_settings() {
    let cases = [
        (OverwriteDecision::FollowSettings, true, true),
        (OverwriteDecision::FollowSettings, false, false),
        (OverwriteDecision::Allow, false, true),
        (OverwriteDecision::Deny, true, false),
    ];
    for (decision, settings_overwrite, expected) in cases {
        let mut state = AppState::default();
        state.settings.output.overwrite_existing = settings_overwrite;
        let added = apply(
            &mut state,
            Command::Queue(QueueCommand::AddMany {
                requests: vec![QueueAddRequest {
                    overwrite: decision,
                    ..add_request(QueueItemId(1), "video.mkv")
                }],
            }),
        );
        assert_eq!(added.reply, Reply::Accepted);
        let _started = start_session(&mut state);
        let claimed = reserve_and_prepare(&mut state, ClaimId(2), RunId(3), execution());
        let Reply::Claimed(Some(job)) = claimed.reply else {
            panic!("expected a claim for {decision:?}");
        };
        assert_eq!(
            job.spec.execution.overwrite_existing, expected,
            "{decision:?} under overwrite_existing={settings_overwrite}"
        );
    }
}

/// A settled queue for the admin-command decision tables: items 1 (Failed)
/// and 2 (Stopped) are finished, item 3 takes the given state, items 4 and 5
/// are queued. Item and session states are set directly — the command flows
/// that produce them are exercised by the lifecycle tests.
fn admin_fixture(third: QueueItemState, session: SessionState) -> AppState {
    let mut state = AppState::default();
    for id in 1..=5 {
        let added = apply(
            &mut state,
            add_command(QueueItemId(id), format!("clip-{id}.mkv")),
        );
        assert_eq!(added.reply, Reply::Accepted);
    }
    state.durable.queue[0].state = QueueItemState::Finished(ItemOutcome::Failed(
        FailureFacts::new(FailureKind::Internal, "fixture"),
    ));
    state.durable.queue[1].state = QueueItemState::Finished(ItemOutcome::Stopped);
    state.durable.queue[2].state = third;
    state.session = session;
    state
}

fn claimed_state() -> QueueItemState {
    QueueItemState::Claimed {
        claim_id: ClaimId(30),
        run_id: RunId(31),
    }
}

#[test]
fn clear_is_refused_while_a_session_is_active() {
    for session in [
        SessionState::Running,
        SessionState::StopAfterCurrent,
        SessionState::ForceStopping,
    ] {
        let mut state = admin_fixture(claimed_state(), session.clone());
        let applied = apply(&mut state, Command::Queue(QueueCommand::Clear));
        assert!(
            matches!(applied.reply, Reply::Rejected { .. }),
            "{session:?}"
        );
        assert!(applied.durable.is_empty());
        assert_eq!(state.durable.queue.len(), 5, "{session:?}");
    }
}

#[test]
fn clear_removes_pending_and_finished_items_but_never_an_active_one() {
    // A crash-recovered claim can outlive its session; clear leaves it alone.
    let mut state = admin_fixture(claimed_state(), SessionState::Idle);
    let applied = apply(&mut state, Command::Queue(QueueCommand::Clear));
    assert_eq!(applied.reply, Reply::Accepted);
    assert_eq!(applied.durable.len(), 4);
    assert_eq!(queue_order(&state), vec![3]);
    // A settled queue clears to empty, and an empty clear is an accepted no-op.
    let mut state = admin_fixture(QueueItemState::Queued, SessionState::Idle);
    let applied = apply(&mut state, Command::Queue(QueueCommand::Clear));
    assert_eq!(applied.reply, Reply::Accepted);
    assert_eq!(applied.durable.len(), 5);
    assert!(state.durable.queue.is_empty());
    let empty = apply(&mut state, Command::Queue(QueueCommand::Clear));
    assert_eq!(empty.reply, Reply::Accepted);
    assert!(empty.durable.is_empty());
}

#[test]
fn clear_completed_keeps_failed_items_and_runs_in_any_session_state() {
    for session in [
        SessionState::Idle,
        SessionState::Running,
        SessionState::StopAfterCurrent,
        SessionState::ForceStopping,
    ] {
        let mut state = admin_fixture(claimed_state(), session.clone());
        let applied = apply(&mut state, Command::Queue(QueueCommand::ClearCompleted));
        assert_eq!(applied.reply, Reply::Accepted, "{session:?}");
        assert_eq!(
            applied.durable,
            vec![DurableDelta::QueueRemoved {
                item_id: QueueItemId(2)
            }],
            "{session:?}"
        );
        assert_eq!(queue_order(&state), vec![1, 3, 4, 5], "{session:?}");
    }
}

#[test]
fn clear_completed_removes_every_terminal_outcome_except_failed() {
    let outcomes = [
        ItemOutcome::Analyzed,
        ItemOutcome::Converted(CompletionEvidence::RecoveredAtStartup),
        ItemOutcome::Remuxed(CompletionEvidence::RecoveredAtStartup),
        ItemOutcome::NotWorthwhile {
            attempts: Vec::new(),
        },
        ItemOutcome::Stopped,
        ItemOutcome::Skipped {
            reason: SkipReason::AlreadyAv1Matroska,
        },
        ItemOutcome::Failed(FailureFacts::new(FailureKind::Internal, "fixture")),
    ];
    let mut state = AppState::default();
    for (index, outcome) in outcomes.into_iter().enumerate() {
        let id = u64::try_from(index).expect("small index") + 1;
        let added = apply(
            &mut state,
            add_command(QueueItemId(id), format!("clip-{id}.mkv")),
        );
        assert_eq!(added.reply, Reply::Accepted);
        state.durable.queue[index].state = QueueItemState::Finished(outcome);
    }
    let applied = apply(&mut state, Command::Queue(QueueCommand::ClearCompleted));
    assert_eq!(applied.reply, Reply::Accepted);
    assert_eq!(applied.durable.len(), 6);
    assert_eq!(queue_order(&state), vec![7]);
}

#[test]
fn retry_requeues_a_finished_item_to_the_end_in_any_session_state() {
    for session in [
        SessionState::Idle,
        SessionState::Running,
        SessionState::StopAfterCurrent,
        SessionState::ForceStopping,
    ] {
        let mut state = admin_fixture(claimed_state(), session.clone());
        let applied = apply(
            &mut state,
            Command::Queue(QueueCommand::Retry {
                item_id: QueueItemId(1),
            }),
        );
        assert_eq!(applied.reply, Reply::Accepted, "{session:?}");
        assert_eq!(
            applied.durable,
            vec![DurableDelta::QueueRequeued {
                item_id: QueueItemId(1)
            }],
            "{session:?}"
        );
        assert_queue_shape(&state, &[2, 3, 4, 5, 1]);
        let retried = state.durable.queue.last().expect("requeued item");
        assert_eq!(retried.state, QueueItemState::Queued, "{session:?}");
    }
}

#[test]
fn retry_is_refused_for_missing_active_or_pending_items() {
    let active_states = [
        QueueItemState::Reserved {
            claim_id: ClaimId(30),
            run_id: RunId(31),
        },
        claimed_state(),
        QueueItemState::Running {
            claim_id: ClaimId(30),
            run_id: RunId(31),
        },
    ];
    for active in active_states {
        let mut state = admin_fixture(active.clone(), SessionState::Running);
        // Item 3 is active, item 4 is queued, item 9 does not exist.
        for target in [QueueItemId(3), QueueItemId(4), QueueItemId(9)] {
            let applied = apply(
                &mut state,
                Command::Queue(QueueCommand::Retry { item_id: target }),
            );
            assert!(
                matches!(applied.reply, Reply::Rejected { .. }),
                "{active:?} -> {target:?}"
            );
            assert!(applied.durable.is_empty());
        }
        assert_queue_shape(&state, &[1, 2, 3, 4, 5]);
    }
}

#[test]
fn retry_flows_through_the_next_reservation_and_replays() {
    let mut state = AppState::default();
    let mut bytes = Vec::new();
    let mut sequence = 0;
    for (id, input) in [(1, "first.mkv"), (2, "second.mkv")] {
        let added = apply_and_journal(
            &mut state,
            &mut bytes,
            &mut sequence,
            add_command(QueueItemId(id), input),
        );
        assert_eq!(added.reply, Reply::Accepted);
    }
    let started = start_session(&mut state);
    assert_eq!(started.reply, Reply::Accepted);
    let reserved = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(20),
            run_id: RunId(21),
        }),
    );
    assert!(matches!(reserved.reply, Reply::Reserved(Some(_))));
    let prepared = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(20),
            run_id: RunId(21),
            observation: None,
            import_paths: Vec::new(),
            execution: execution(),
        }),
    );
    assert!(matches!(prepared.reply, Reply::Claimed(Some(_))));
    let failed = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
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
    assert_eq!(failed.reply, Reply::Accepted);
    // Retry sends the failure behind the remaining pending item.
    let retried = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Queue(QueueCommand::Retry {
            item_id: QueueItemId(1),
        }),
    );
    assert_eq!(retried.reply, Reply::Accepted);
    assert_queue_shape(&state, &[2, 1]);
    // The next reservation claims item 2 — the retried item waits its turn —
    // and the old run's lineage still records the failure.
    let next = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(22),
            run_id: RunId(23),
        }),
    );
    let Reply::Reserved(Some(job)) = next.reply else {
        panic!("expected a reservation for item 2");
    };
    assert_eq!(job.item_id, QueueItemId(2));
    assert!(
        state
            .durable
            .conversion_runs
            .get(&RunId(21))
            .is_some_and(|run| run.outcome.is_some())
    );
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, state.durable);
}

#[test]
fn edit_resolves_partial_patches_against_the_current_tuple() {
    let mut state = AppState::default();
    let mut bytes = Vec::new();
    let mut sequence = 0;
    let added = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        add_command(QueueItemId(1), "video.mkv"),
    );
    assert_eq!(added.reply, Reply::Accepted);
    let applied = apply_and_journal(
        &mut state,
        &mut bytes,
        &mut sequence,
        Command::Queue(QueueCommand::Edit {
            item_id: QueueItemId(1),
            patch: QueueItemEdit {
                operation: Some(Operation::Analyze),
                overwrite: Some(OverwriteDecision::Deny),
                ..QueueItemEdit::default()
            },
        }),
    );
    assert_eq!(applied.reply, Reply::Accepted);
    // The journaled delta carries the resolved tuple: patched fields plus the
    // item's unchanged intent and output target.
    assert_eq!(
        applied.durable,
        vec![DurableDelta::QueueEdited {
            item_id: QueueItemId(1),
            operation: Operation::Analyze,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Replace,
            overwrite: OverwriteDecision::Deny,
        }]
    );
    assert_eq!(state.durable.queue[0].operation, Operation::Analyze);
    assert_eq!(state.durable.queue[0].overwrite, OverwriteDecision::Deny);
    // Patches that change nothing are accepted no-ops with no durable record.
    for patch in [
        QueueItemEdit::default(),
        QueueItemEdit {
            operation: Some(Operation::Analyze),
            ..QueueItemEdit::default()
        },
    ] {
        let noop = apply_and_journal(
            &mut state,
            &mut bytes,
            &mut sequence,
            Command::Queue(QueueCommand::Edit {
                item_id: QueueItemId(1),
                patch,
            }),
        );
        assert_eq!(noop.reply, Reply::Accepted);
        assert!(noop.durable.is_empty());
    }
    let replayed = replay(&bytes);
    assert!(replayed.corruption.is_none());
    assert_eq!(replayed.state, state.durable);
}

#[test]
fn edit_is_idle_only_and_targets_queued_items() {
    // Session gate: a non-idle session refuses edits even of queued items —
    // the running session's rules are frozen.
    for session in [
        SessionState::Running,
        SessionState::StopAfterCurrent,
        SessionState::ForceStopping,
    ] {
        let mut state = admin_fixture(claimed_state(), session.clone());
        let applied = apply(
            &mut state,
            Command::Queue(QueueCommand::Edit {
                item_id: QueueItemId(4),
                patch: QueueItemEdit {
                    operation: Some(Operation::Analyze),
                    ..QueueItemEdit::default()
                },
            }),
        );
        assert!(
            matches!(applied.reply, Reply::Rejected { .. }),
            "{session:?}"
        );
        assert!(applied.durable.is_empty());
    }
    // Item gate while idle: finished, crash-recovered active, and missing
    // items all refuse.
    let mut state = admin_fixture(claimed_state(), SessionState::Idle);
    for target in [QueueItemId(1), QueueItemId(3), QueueItemId(9)] {
        let applied = apply(
            &mut state,
            Command::Queue(QueueCommand::Edit {
                item_id: target,
                patch: QueueItemEdit {
                    operation: Some(Operation::Analyze),
                    ..QueueItemEdit::default()
                },
            }),
        );
        assert!(
            matches!(applied.reply, Reply::Rejected { .. }),
            "{target:?}"
        );
        assert!(applied.durable.is_empty());
    }
    assert_queue_shape(&state, &[1, 2, 3, 4, 5]);
}

#[test]
fn replay_rejects_requeues_of_pending_and_edits_of_finished_items() {
    // A queued item cannot be requeued.
    let mut bytes = encode_record(&JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![queue_added_delta(1, "one.mkv")],
    })
    .expect("head record");
    bytes.extend(
        encode_record(&JournalEnvelope {
            sequence: JournalSequence(1),
            deltas: vec![DurableDelta::QueueRequeued {
                item_id: QueueItemId(1),
            }],
        })
        .expect("requeue record"),
    );
    let replayed = replay(&bytes);
    let corruption = replayed.corruption.expect("requeue of a queued item");
    assert!(corruption.reason.contains("requeued item"));

    // A finished item cannot be edited.
    let mut bytes = encode_record(&JournalEnvelope {
        sequence: JournalSequence(0),
        deltas: vec![
            queue_added_delta(1, "one.mkv"),
            DurableDelta::ItemReserved {
                job: Box::new(crate::ReservedJob {
                    item_id: QueueItemId(1),
                    claim_id: ClaimId(2),
                    run_id: RunId(3),
                    input: PathBuf::from("one.mkv"),
                    operation: Operation::Convert,
                    intent: AnalysisIntent::ReuseIfFresh,
                    output_target: OutputTarget::Replace,
                }),
            },
            DurableDelta::ItemFinished {
                item_id: QueueItemId(1),
                claim_id: ClaimId(2),
                run_id: RunId(3),
                outcome: ItemOutcome::Stopped,
                at: UnixMillis(1_000),
                phase_spans: Vec::new(),
            },
        ],
    })
    .expect("head record");
    bytes.extend(
        encode_record(&JournalEnvelope {
            sequence: JournalSequence(1),
            deltas: vec![DurableDelta::QueueEdited {
                item_id: QueueItemId(1),
                operation: Operation::Analyze,
                intent: AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                overwrite: OverwriteDecision::FollowSettings,
            }],
        })
        .expect("edit record"),
    );
    let replayed = replay(&bytes);
    let corruption = replayed.corruption.expect("edit of a finished item");
    assert!(corruption.reason.contains("edited item"));
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
            import_paths: Vec::new(),
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
        Command::Queue(QueueCommand::AddMany {
            requests: vec![QueueAddRequest {
                intent: AnalysisIntent::Refresh,
                ..add_request(QueueItemId(4), "video.mkv")
            }],
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
            import_paths: Vec::new(),
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
            import_paths: Vec::new(),
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
    assert!(matches!(
        rejected.ephemeral.as_slice(),
        [EphemeralDelta::CommandRejected { .. }]
    ));
}

#[test]
fn worker_rejections_pair_the_reply_with_a_command_rejected_delta() {
    // Every rejection surfaces both ways: the worker's `Reply` and a
    // `CommandRejected` ephemeral for the stream. The in-closure rejection
    // paths must uphold the same contract as `Applied::rejected`.
    let mut state = AppState::default();
    let _added = apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    let _started = start_session(&mut state);
    let _claimed = reserve_and_prepare(&mut state, ClaimId(2), RunId(3), execution());
    let record = |run_id| {
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id,
            result: Box::new(analysis()),
        })
    };
    let recorded = apply(&mut state, record(RunId(3)));
    assert_eq!(recorded.reply, Reply::Accepted);

    let duplicate = apply(&mut state, record(RunId(3)));
    let reason = "analysis is already recorded".to_owned();
    assert_eq!(
        duplicate.reply,
        Reply::Rejected {
            reason: reason.clone(),
        }
    );
    assert_eq!(
        duplicate.ephemeral,
        vec![EphemeralDelta::CommandRejected { reason }]
    );

    let terminal = apply(
        &mut state,
        Command::Worker(WorkerCommand::Terminal {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Analyzed,
            at: UnixMillis(1_000),
            phase_spans: Vec::new(),
            final_telemetry: None,
        }),
    );
    let reason = "analyzed outcome is incompatible with the run state".to_owned();
    assert_eq!(
        terminal.reply,
        Reply::Rejected {
            reason: reason.clone(),
        }
    );
    assert_eq!(
        terminal.ephemeral,
        vec![EphemeralDelta::CommandRejected { reason }]
    );
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
            import_paths: Vec::new(),
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
            input_size: Some(10_000),
            output_size: Some(9_500),
        },
        source_run: Some(RunId(11)),
        decided_at: UnixMillis(4_000),
    });
    // serde_json rejects non-string map keys, so the profile-keyed index must
    // serialize as an entry list; this fails if that representation regresses.
    let encoded = serde_json::to_string(&record).expect("serialize file record");
    let decoded: FileRecord = serde_json::from_str(&encoded).expect("deserialize file record");
    assert_eq!(decoded, record);
}

#[test]
fn statistics_request_answers_on_the_stream_without_touching_state() {
    let mut state = AppState::default();
    let applied = apply(
        &mut state,
        Command::Projection(ProjectionCommand::RequestStatistics {
            utc_offset_minutes: 60,
        }),
    );
    assert_eq!(applied.reply, Reply::Accepted);
    assert!(applied.durable.is_empty());
    assert!(applied.config.is_empty());
    assert!(applied.effects.is_empty());
    let [EphemeralDelta::Statistics(payload)] = applied.ephemeral.as_slice() else {
        panic!("expected exactly one statistics delta");
    };
    assert_eq!(payload.utc_offset_minutes, 60);
    assert_eq!(payload.converted_files, 0);
    assert_eq!(state, AppState::default());
}

#[test]
fn statistics_request_rejects_an_implausible_offset() {
    let mut state = AppState::default();
    for offset in [1_441, -1_441] {
        let applied = apply(
            &mut state,
            Command::Projection(ProjectionCommand::RequestStatistics {
                utc_offset_minutes: offset,
            }),
        );
        assert!(matches!(applied.reply, Reply::Rejected { .. }));
        assert!(matches!(
            applied.ephemeral.as_slice(),
            [EphemeralDelta::CommandRejected { .. }]
        ));
    }
}
/// A parked record whose stamp matches `media_observation` (size 10_000,
/// mtime 1 ns) with a full summary.
fn parked_record(status: ParkedStatus) -> ImportedHistoryRecord {
    ImportedHistoryRecord {
        status,
        size: Some(10_000),
        modified_ns: Some(FileTimeNs(1)),
        video_codec: Some(VideoCodec::H264),
        width: Some(1_280),
        height: Some(720),
        duration_ms: Some(60_000),
        output_size: Some(4_000),
        encoding_time: Some(DurationMs(120_000)),
        crf: Some(Crf(30_000)),
        vmaf: Some(VmafScore(9_512)),
        target: Some(VmafTarget(95)),
        requested_target: Some(VmafTarget(94)),
        floor_target: Some(VmafTarget(91)),
        decided_at: UnixMillis(1_000),
    }
}

#[test]
fn resolve_parked_adopts_on_stamp_match_by_status() {
    let observation = media_observation("adopt-content");

    // Decisive statuses adopt their verdict with an import-shaped summary.
    let converted = resolve_parked(&parked_record(ParkedStatus::Converted), &observation);
    let ParkedResolution::Adopt {
        verdict: Some(verdict),
    } = converted
    else {
        panic!("converted stamp match must adopt a verdict");
    };
    assert_eq!(verdict.source_run, None);
    assert_eq!(verdict.decided_at, UnixMillis(1_000));
    assert_eq!(
        verdict.kind,
        crate::VerdictKind::Converted {
            output_content_key: None,
            input_size: Some(10_000),
            output_size: Some(4_000),
            encoding_time: Some(DurationMs(120_000)),
            crf: Some(Crf(30_000)),
            vmaf: Some(VmafScore(9_512)),
            target: Some(VmafTarget(95)),
        }
    );

    let not_worthwhile = resolve_parked(&parked_record(ParkedStatus::NotWorthwhile), &observation);
    let ParkedResolution::Adopt {
        verdict: Some(verdict),
    } = not_worthwhile
    else {
        panic!("not-worthwhile stamp match must adopt a verdict");
    };
    assert_eq!(
        verdict.kind,
        crate::VerdictKind::NotWorthwhile {
            requested: VmafTarget(94),
            floor: VmafTarget(91),
        }
    );

    // Indecisive statuses adopt provenance only.
    for status in [ParkedStatus::Scanned, ParkedStatus::Analyzed] {
        assert_eq!(
            resolve_parked(&parked_record(status), &observation),
            ParkedResolution::Adopt { verdict: None }
        );
    }

    // Missing targets fall back to the default target constants.
    let mut bare = parked_record(ParkedStatus::NotWorthwhile);
    bare.requested_target = None;
    bare.floor_target = None;
    let ParkedResolution::Adopt {
        verdict: Some(verdict),
    } = resolve_parked(&bare, &observation)
    else {
        panic!("missing targets still adopt");
    };
    assert_eq!(
        verdict.kind,
        crate::VerdictKind::NotWorthwhile {
            requested: crate::DEFAULT_VMAF_TARGET,
            floor: crate::MIN_VMAF_FALLBACK_TARGET,
        }
    );
}

#[test]
fn resolve_parked_stamp_tolerance_and_retirement() {
    let observation = media_observation("adopt-content");

    // Modification time within one second still matches.
    let mut drifted = parked_record(ParkedStatus::Scanned);
    drifted.modified_ns = Some(FileTimeNs(1 + crate::IMPORT_MTIME_TOLERANCE_NS));
    assert_eq!(
        resolve_parked(&drifted, &observation),
        ParkedResolution::Adopt { verdict: None }
    );
    let mut beyond = parked_record(ParkedStatus::Scanned);
    beyond.modified_ns = Some(FileTimeNs(2 + crate::IMPORT_MTIME_TOLERANCE_NS));
    assert_eq!(
        resolve_parked(&beyond, &observation),
        ParkedResolution::Retire
    );

    // A record missing either stamp half never matches.
    let mut sizeless = parked_record(ParkedStatus::Scanned);
    sizeless.size = None;
    assert_eq!(
        resolve_parked(&sizeless, &observation),
        ParkedResolution::Retire
    );
    let mut timeless = parked_record(ParkedStatus::Scanned);
    timeless.modified_ns = None;
    assert_eq!(
        resolve_parked(&timeless, &observation),
        ParkedResolution::Retire
    );

    // Replace-mode: a converted record whose stamp mismatches still adopts
    // when the file now at the path is AV1 — it IS the conversion's output.
    let mut replaced = parked_record(ParkedStatus::Converted);
    replaced.size = Some(99_999);
    let mut av1_observation = media_observation("adopt-content");
    av1_observation.metadata.codec = VideoCodec::Av1;
    let ParkedResolution::Adopt {
        verdict: Some(verdict),
    } = resolve_parked(&replaced, &av1_observation)
    else {
        panic!("replace-mode output must adopt the converted verdict");
    };
    assert!(matches!(verdict.kind, crate::VerdictKind::Converted { .. }));
    // The same mismatch against a non-AV1 file retires the claim.
    assert_eq!(
        resolve_parked(&replaced, &observation),
        ParkedResolution::Retire
    );
}

#[test]
fn imported_provenance_priority_is_total_and_deterministic() {
    let current = ImportedProvenance {
        import_path: ImportPath("c:/videos/z.mkv".to_owned()),
        record: parked_record(ParkedStatus::Analyzed),
    };
    let stronger = ImportedProvenance {
        import_path: ImportPath("c:/videos/y.mkv".to_owned()),
        record: parked_record(ParkedStatus::Converted),
    };
    assert!(stronger.outranks(&current));

    let smaller_path = ImportedProvenance {
        import_path: ImportPath("c:/videos/a.mkv".to_owned()),
        record: parked_record(ParkedStatus::Converted),
    };
    assert!(smaller_path.outranks(&stronger));

    let mut newest = parked_record(ParkedStatus::Scanned);
    newest.decided_at = UnixMillis(1_001);
    let newer = ImportedProvenance {
        import_path: ImportPath("c:/videos/zz.mkv".to_owned()),
        record: newest,
    };
    assert!(newer.outranks(&smaller_path));
}

#[test]
fn history_deltas_fold_park_adopt_and_retire() {
    let mut state = crate::DurableState::default();
    let key_a = ImportPath("c:/videos/a.mkv".to_owned());
    let key_b = ImportPath("c:/videos/b.mkv".to_owned());
    crate::fold(
        &mut state,
        &DurableDelta::HistoryImported {
            records: vec![
                (key_a.clone(), parked_record(ParkedStatus::Converted)),
                (key_b.clone(), parked_record(ParkedStatus::Scanned)),
            ],
        },
    );
    assert_eq!(state.parked.len(), 2);

    let observation = media_observation("adopt-content");
    crate::fold(
        &mut state,
        &DurableDelta::MediaObserved {
            observation: Box::new(observation.clone()),
        },
    );
    let adopted_verdict = crate::Verdict {
        kind: crate::VerdictKind::NotWorthwhile {
            requested: VmafTarget(95),
            floor: VmafTarget(90),
        },
        source_run: None,
        decided_at: UnixMillis(1_000),
    };
    crate::fold(
        &mut state,
        &DurableDelta::ParkedAdopted {
            import_path: key_a.clone(),
            content_key: observation.binding.content_key.clone(),
            imported: parked_record(ParkedStatus::Converted),
            verdict: Some(adopted_verdict.clone()),
        },
    );
    let record = state
        .records
        .get(&observation.binding.content_key)
        .expect("content record");
    assert_eq!(
        record
            .imported
            .as_ref()
            .map(|imported| &imported.import_path),
        Some(&key_a)
    );
    assert_eq!(record.verdict, Some(adopted_verdict.clone()));
    assert!(!state.parked.contains_key(&key_a));

    // Adoption without a verdict keeps the standing one.
    crate::fold(
        &mut state,
        &DurableDelta::ParkedAdopted {
            import_path: key_b.clone(),
            content_key: observation.binding.content_key.clone(),
            imported: parked_record(ParkedStatus::Scanned),
            verdict: None,
        },
    );
    let record = state
        .records
        .get(&observation.binding.content_key)
        .expect("content record");
    assert_eq!(
        record
            .imported
            .as_ref()
            .map(|imported| &imported.import_path),
        Some(&key_a)
    );
    assert_eq!(record.verdict, Some(adopted_verdict));
    assert!(state.parked.is_empty());
    assert_eq!(state.adopted_imports.len(), 2);
    assert!(state.adopted_imports.contains(&key_a));
    assert!(state.adopted_imports.contains(&key_b));

    let encoded = serde_json::to_string(&state).expect("serialize adopted import guards");
    let restarted: crate::DurableState =
        serde_json::from_str(&encoded).expect("deserialize adopted import guards");
    assert_eq!(restarted.adopted_imports, state.adopted_imports);

    // Retirement just drops the parked entry.
    let key_c = ImportPath("c:/videos/c.mkv".to_owned());
    crate::fold(
        &mut state,
        &DurableDelta::HistoryImported {
            records: vec![(key_c.clone(), parked_record(ParkedStatus::Scanned))],
        },
    );
    crate::fold(
        &mut state,
        &DurableDelta::ParkedRetired { import_path: key_c },
    );
    assert!(state.parked.is_empty());
}

#[test]
fn import_parks_fresh_records_and_skips_known_keys() {
    let mut state = AppState::default();
    let key_a = ImportPath("c:/videos/a.mkv".to_owned());
    let key_b = ImportPath("c:/videos/b.mkv".to_owned());
    let batch = vec![
        (key_a.clone(), parked_record(ParkedStatus::Converted)),
        (key_b.clone(), parked_record(ParkedStatus::Scanned)),
        // A duplicate inside one batch is skipped, not double-parked.
        (key_a.clone(), parked_record(ParkedStatus::Scanned)),
    ];
    let imported = apply(
        &mut state,
        Command::History(HistoryCommand::Import {
            records: batch.clone(),
        }),
    );
    assert_eq!(
        imported.reply,
        Reply::Imported {
            parked: 2,
            skipped: 1
        }
    );
    assert_eq!(imported.durable.len(), 1);
    assert_eq!(state.durable.parked.len(), 2);

    // A full re-import is a counted no-op with no durable write.
    let again = apply(
        &mut state,
        Command::History(HistoryCommand::Import { records: batch }),
    );
    assert_eq!(
        again.reply,
        Reply::Imported {
            parked: 0,
            skipped: 3
        }
    );
    assert!(again.durable.is_empty());

    // An adopted key stays skipped even after its parked entry is gone.
    let observation = media_observation("adopt-content");
    crate::fold(
        &mut state.durable,
        &DurableDelta::MediaObserved {
            observation: Box::new(observation.clone()),
        },
    );
    crate::fold(
        &mut state.durable,
        &DurableDelta::ParkedAdopted {
            import_path: key_a.clone(),
            content_key: observation.binding.content_key,
            imported: parked_record(ParkedStatus::Converted),
            verdict: None,
        },
    );
    let after_adoption = apply(
        &mut state,
        Command::History(HistoryCommand::Import {
            records: vec![(key_a, parked_record(ParkedStatus::Converted))],
        }),
    );
    assert_eq!(
        after_adoption.reply,
        Reply::Imported {
            parked: 0,
            skipped: 1
        }
    );
}

#[test]
fn prepare_resolves_parked_records_after_the_observation() {
    let mut state = AppState::default();
    let matching = ImportPath("c:/videos/match.mkv".to_owned());
    let weaker_match = ImportPath("c:/videos/also-match.mkv".to_owned());
    let stale = ImportPath("c:/videos/stale.mkv".to_owned());
    let mut stale_record = parked_record(ParkedStatus::Scanned);
    stale_record.size = Some(99_999);
    let imported = apply(
        &mut state,
        Command::History(HistoryCommand::Import {
            records: vec![
                (matching.clone(), parked_record(ParkedStatus::Converted)),
                (weaker_match.clone(), parked_record(ParkedStatus::Analyzed)),
                (stale.clone(), stale_record),
            ],
        }),
    );
    assert!(matches!(imported.reply, Reply::Imported { parked: 3, .. }));

    apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    start_session(&mut state);
    let reserved = apply(
        &mut state,
        Command::Worker(WorkerCommand::ReserveNext {
            claim_id: ClaimId(2),
            run_id: RunId(3),
        }),
    );
    assert!(matches!(reserved.reply, Reply::Reserved(Some(_))));
    let observation = media_observation("adopt-content");
    let prepared = apply(
        &mut state,
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            observation: Some(Box::new(observation.clone())),
            import_paths: vec![weaker_match.clone(), stale.clone(), matching.clone()],
            execution: execution(),
        }),
    );
    assert!(matches!(prepared.reply, Reply::Claimed(Some(_))));
    // The observation folds before any adoption delta.
    let observed_at = prepared
        .durable
        .iter()
        .position(|delta| matches!(delta, DurableDelta::MediaObserved { .. }))
        .expect("media observed");
    let adopted_at = prepared
        .durable
        .iter()
        .position(|delta| matches!(delta, DurableDelta::ParkedAdopted { .. }))
        .expect("parked adopted");
    assert!(observed_at < adopted_at);
    assert!(
        prepared
            .durable
            .iter()
            .any(|delta| matches!(delta, DurableDelta::ParkedRetired { .. }))
    );

    assert!(state.durable.parked.is_empty());
    assert!(state.durable.adopted_imports.contains(&matching));
    assert!(state.durable.adopted_imports.contains(&weaker_match));
    let record = state
        .durable
        .records
        .get(&observation.binding.content_key)
        .expect("content record");
    assert_eq!(
        record
            .imported
            .as_ref()
            .map(|imported| &imported.import_path),
        Some(&matching)
    );
    let verdict = record.verdict.clone().expect("adopted verdict");
    assert_eq!(verdict.source_run, None);
    assert!(matches!(
        verdict.kind,
        crate::VerdictKind::Converted {
            output_content_key: None,
            ..
        }
    ));
}

#[test]
fn adoption_never_overwrites_a_native_verdict() {
    let mut state = AppState::default();
    let key = ImportPath("c:/videos/native.mkv".to_owned());
    apply(
        &mut state,
        Command::History(HistoryCommand::Import {
            records: vec![(key.clone(), parked_record(ParkedStatus::Converted))],
        }),
    );
    // The content is already natively decided.
    let observation = media_observation("adopt-content");
    crate::fold(
        &mut state.durable,
        &DurableDelta::MediaObserved {
            observation: Box::new(observation.clone()),
        },
    );
    let native = crate::Verdict {
        kind: crate::VerdictKind::NotWorthwhile {
            requested: VmafTarget(95),
            floor: VmafTarget(90),
        },
        source_run: Some(RunId(7)),
        decided_at: UnixMillis(9_000),
    };
    state
        .durable
        .records
        .get_mut(&observation.binding.content_key)
        .expect("content record")
        .verdict = Some(native.clone());

    apply(&mut state, add_command(QueueItemId(1), "video.mkv"));
    start_session(&mut state);
    apply(
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
            observation: Some(Box::new(observation.clone())),
            import_paths: vec![key.clone()],
            execution: execution(),
        }),
    );
    assert!(matches!(prepared.reply, Reply::Claimed(Some(_))));
    let record = state
        .durable
        .records
        .get(&observation.binding.content_key)
        .expect("content record");
    // Provenance lands, the native verdict stands, the inbox empties.
    assert_eq!(
        record
            .imported
            .as_ref()
            .map(|imported| &imported.import_path),
        Some(&key)
    );
    assert_eq!(record.verdict, Some(native));
    assert!(state.durable.parked.is_empty());
}

#[test]
fn finished_verdict_absorbs_the_measured_summary() {
    let key = "verdict-content";
    let mut state = verdict_fixture(
        3,
        Some(key),
        Some(OutputState::Committed {
            final_identity: identity("ck-out", 20),
        }),
    );
    crate::fold(
        &mut state,
        &DurableDelta::AnalysisRecorded {
            run_id: RunId(3),
            result: Box::new(analysis()),
        },
    );
    crate::fold(
        &mut state,
        &DurableDelta::ItemFinished {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Converted(CompletionEvidence::LiveEncode {
                input_size: 10_000,
                output_size: 4_000,
                stream_sizes: StreamByteSizes {
                    video: 3_000,
                    audio: 900,
                    subtitle: 50,
                    other: 50,
                },
                encode_decode: DecodeMode::Software,
            }),
            at: UnixMillis(5_000),
            phase_spans: vec![
                PhaseSpan {
                    phase: JobPhase::Analyzing,
                    duration: DurationMs(30_000),
                },
                PhaseSpan {
                    phase: JobPhase::Encoding,
                    duration: DurationMs(100_000),
                },
                PhaseSpan {
                    phase: JobPhase::Encoding,
                    duration: DurationMs(20_000),
                },
                PhaseSpan {
                    phase: JobPhase::Verifying,
                    duration: DurationMs(5_000),
                },
            ],
        },
    );
    let verdict = state
        .records
        .get(&ContentKey(key.to_owned()))
        .expect("content record")
        .verdict
        .clone()
        .expect("standing verdict");
    assert_eq!(
        verdict.kind,
        crate::VerdictKind::Converted {
            output_content_key: Some(ContentKey("ck-out".to_owned())),
            input_size: Some(10_000),
            output_size: Some(4_000),
            encoding_time: Some(DurationMs(120_000)),
            crf: Some(Crf(30_000)),
            vmaf: Some(VmafScore(9_500)),
            target: Some(VmafTarget(95)),
        }
    );

    // A remux carries its sizes and nothing analysis-shaped.
    let mut remuxed = verdict_fixture(
        3,
        Some(key),
        Some(OutputState::Committed {
            final_identity: identity("ck-out", 20),
        }),
    );
    crate::fold(
        &mut remuxed,
        &DurableDelta::ItemFinished {
            item_id: QueueItemId(1),
            claim_id: ClaimId(2),
            run_id: RunId(3),
            outcome: ItemOutcome::Remuxed(CompletionEvidence::LiveRemux {
                input_size: 10_000,
                output_size: 9_800,
            }),
            at: UnixMillis(5_000),
            phase_spans: Vec::new(),
        },
    );
    let verdict = remuxed
        .records
        .get(&ContentKey(key.to_owned()))
        .expect("content record")
        .verdict
        .clone()
        .expect("standing verdict");
    assert_eq!(
        verdict.kind,
        crate::VerdictKind::Remuxed {
            output_content_key: ContentKey("ck-out".to_owned()),
            input_size: Some(10_000),
            output_size: Some(9_800),
        }
    );
}

#[test]
fn adopted_verdicts_apply_only_by_content_identity() {
    let adopted_converted = crate::Verdict {
        kind: crate::VerdictKind::Converted {
            output_content_key: None,
            input_size: Some(10_000),
            output_size: Some(4_000),
            encoding_time: None,
            crf: None,
            vmaf: None,
            target: None,
        },
        source_run: None,
        decided_at: UnixMillis(1_000),
    };
    // No transaction can ever be resolved for an adopted conversion, so it
    // never vouches for the file at a path.
    assert!(!crate::verdict_applies(
        &adopted_converted,
        None,
        Some(&destructive("adopted-output", 4_000)),
        crate::TimestampReliability::Reliable,
    ));
    let adopted_not_worthwhile = crate::Verdict {
        kind: crate::VerdictKind::NotWorthwhile {
            requested: VmafTarget(95),
            floor: VmafTarget(90),
        },
        source_run: None,
        decided_at: UnixMillis(1_000),
    };
    assert!(crate::verdict_applies(
        &adopted_not_worthwhile,
        None,
        None,
        crate::TimestampReliability::Unknown,
    ));
}
