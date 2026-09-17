//! Media tool contract (ADR-023): the durable engine starts, replays, and
//! serves non-media commands when no tools are located; startup recovery
//! defers unsettled output transactions instead of settling blind; discovery
//! honours its precedence and fails closed on explicit tiers; and the
//! session-start capability probe gates the first claim, records provenance,
//! and stays cancellable.
#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crfty_core::{
    AnalysisIntent, AnalysisProfile, AnalysisResult, AppState, ClaimId, Command, Crf, DurableDelta,
    EphemeralDelta, ExecutionSettings, ItemOutcome, LocatedTool, LocatedTools, MediaTool,
    Operation, OutputDelta, OutputTarget, OverwriteDecision, ProbeFailure, QueueAddRequest,
    QueueCommand, QueueItemId, QueueItemState, Replacement, Reply, RunId, SearchMeasurement,
    SessionCommand, SessionState, Settings, SettingsCommand, ToolAvailability, ToolCapability,
    ToolLocationFailure, ToolPathSettings, ToolRevisions, ToolSource, ToolVerification,
    ToolsCommand, UnixMillis, VmafScore, WorkerCommand, apply,
};

fn add_one(item_id: QueueItemId, input: PathBuf) -> QueueCommand {
    QueueCommand::AddMany {
        requests: vec![QueueAddRequest {
            item_id,
            input,
            path_hash: None,
            identity: None,
            timestamp_reliability: crfty_core::TimestampReliability::Unknown,
            operation: Operation::Convert,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Replace,
            overwrite: OverwriteDecision::FollowSettings,
        }],
    }
}
use crfty_engine::{
    ab_av1::AB_AV1_REVISION,
    coordinator::{EngineConfig, EngineRuntime, FixedTools, ToolsConfig},
    driver::{DriverEvent, DriverHandle},
    journal::JournalWriter,
    output::{FixtureByteInspector, OutputManager},
    tools::{
        MediaTools,
        discovery::{self, DiscoveryEnvironment},
    },
};

/// `AbAv1Runtime` is a process-wide singleton; engine-starting tests in this
/// file must not overlap.
static ENGINE_GUARD: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn execution() -> ExecutionSettings {
    let mut profile = AnalysisProfile::production();
    profile.ab_av1_revision = "fixture".to_owned();
    profile.ffmpeg_revision = "fixture".to_owned();
    profile.encoder_revision = "fixture".to_owned();
    ExecutionSettings::production(profile, false)
}

/// Real discovery over an empty environment: nothing is located.
fn missing_tools() -> ToolsConfig {
    ToolsConfig::Discover(DiscoveryEnvironment::default())
}

fn fixture_revisions() -> ToolRevisions {
    ToolRevisions {
        ab_av1: "fixture".to_owned(),
        ffmpeg: "fixture".to_owned(),
        encoder: "fixture".to_owned(),
    }
}

fn located(media: &MediaTools, source: ToolSource) -> LocatedTools {
    LocatedTools {
        ffmpeg: LocatedTool {
            source,
            path: media.ffmpeg.clone(),
        },
        ffprobe: LocatedTool {
            source,
            path: media.ffprobe.clone(),
        },
    }
}

/// Located tools for driver-only tests, where no process ever runs.
fn fixture_available() -> ToolAvailability {
    ToolAvailability::Located {
        tools: located(
            &MediaTools {
                ffmpeg: PathBuf::from("fixture-ffmpeg"),
                ffprobe: PathBuf::from("fixture-ffprobe"),
            },
            ToolSource::SearchPath,
        ),
        verification: ToolVerification::Verified {
            revisions: fixture_revisions(),
        },
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    #[expect(clippy::expect_used, reason = "fixture setup")]
    fn new(name: &str) -> Self {
        let unique = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "crfty-{name}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _result = fs::remove_dir_all(&self.0);
    }
}

fn engine_config(directory: &TestDirectory, tools: ToolsConfig) -> EngineConfig {
    EngineConfig {
        journal_path: directory.path().join("state.jsonl"),
        config_path: directory.path().join("config.json"),
        tools,
        execution: execution(),
    }
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn startup_without_tools_replays_and_serves_non_media_commands() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("tool-free-startup");
    let journal_path = directory.path().join("state.jsonl");
    let config_path = directory.path().join("config.json");

    let settings = Settings {
        hardware_decode: false,
        ..Settings::default()
    };
    let seeder = DriverHandle::start(&journal_path, &config_path).expect("seeding driver");
    let _snapshot = seeder
        .events()
        .expect("event receiver")
        .recv()
        .expect("seed snapshot");
    assert_eq!(
        seeder
            .commands
            .submit(Command::Queue(add_one(
                QueueItemId(1),
                PathBuf::from("video.mkv"),
            )))
            .expect("seed add reply"),
        Reply::Accepted
    );
    assert_eq!(
        seeder
            .commands
            .submit(Command::Settings(SettingsCommand::Set {
                settings: settings.clone(),
            }))
            .expect("seed settings reply"),
        Reply::Accepted
    );
    seeder.shutdown().expect("seeding driver shutdown");

    let engine =
        EngineRuntime::start(engine_config(&directory, missing_tools())).expect("tool-free start");
    let DriverEvent::Snapshot(snapshot) = engine.events.recv().expect("startup snapshot") else {
        panic!("expected startup snapshot first");
    };
    assert_eq!(snapshot.durable.queue.len(), 1);
    assert_eq!(
        snapshot.durable.queue.first().expect("queued item").id,
        QueueItemId(1)
    );
    assert_eq!(snapshot.settings, settings);
    let availability = engine.events.recv().expect("availability event");
    let DriverEvent::Ephemeral(EphemeralDelta::ToolsChanged(ToolAvailability::Missing {
        failures,
    })) = availability
    else {
        panic!("expected missing-tools availability after the snapshot: {availability:?}");
    };
    assert_eq!(
        failures,
        vec![
            ToolLocationFailure::NotOnSearchPath {
                tool: MediaTool::Ffmpeg
            },
            ToolLocationFailure::NotOnSearchPath {
                tool: MediaTool::Ffprobe
            },
        ]
    );

    assert_eq!(
        engine
            .commands
            .submit_queue(add_one(QueueItemId(2), PathBuf::from("another.mkv")))
            .expect("tool-free add reply"),
        Reply::Accepted
    );
    let mut updated = settings.clone();
    updated.output.overwrite_existing = true;
    assert_eq!(
        engine
            .commands
            .submit_settings(SettingsCommand::Set { settings: updated })
            .expect("tool-free settings reply"),
        Reply::Accepted
    );
    let start = engine
        .commands
        .submit_session(SessionCommand::Start)
        .expect("session start reply");
    let Reply::Rejected { reason } = start else {
        panic!("expected tool-free session start rejection: {start:?}");
    };
    assert!(
        reason.starts_with("media tools are unavailable"),
        "{reason}"
    );
    for event in engine.events.try_iter() {
        assert!(
            !matches!(
                event,
                DriverEvent::Ephemeral(EphemeralDelta::SessionChanged(
                    crfty_core::SessionState::Running
                ))
            ),
            "session must not run without tools: {event:?}"
        );
    }
    engine.shutdown().expect("tool-free shutdown");
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn startup_recovery_without_ffprobe_defers_output_settlement() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("tool-free-recovery");
    let journal_path = directory.path().join("state.jsonl");
    let input = directory.path().join("input.mp4");
    let final_path = directory.path().join("input.mkv");
    fs::write(&input, b"input bytes").expect("input fixture");
    let manager = OutputManager::new(FixtureByteInspector);
    let transaction = manager
        .plan(
            RunId(2),
            &input,
            &final_path,
            Replacement::RetireOriginal,
            false,
        )
        .expect("plan transaction");
    let initial = manager
        .create_staging(&transaction)
        .expect("create staging");
    fs::write(&transaction.staging, b"crash-left partial bytes").expect("partial staging");

    let settings = execution();
    let mut state = AppState::default();
    let mut durable = Vec::new();
    for command in [
        Command::Queue(add_one(QueueItemId(1), input.clone())),
        Command::System(crfty_core::SystemCommand::ToolsDiscovered {
            availability: fixture_available(),
        }),
        Command::Session(SessionCommand::Start),
        Command::Worker(WorkerCommand::ReserveNext),
        Command::Worker(WorkerCommand::PrepareReserved {
            item_id: QueueItemId(1),
            claim_id: ClaimId(1),
            run_id: RunId(2),
            observation: None,
            import_paths: Vec::new(),
            execution: settings.clone(),
        }),
        Command::Worker(WorkerCommand::Started {
            item_id: QueueItemId(1),
            claim_id: ClaimId(1),
            run_id: RunId(2),
            at: UnixMillis(1_000),
        }),
        Command::Worker(WorkerCommand::RecordAnalysis {
            item_id: QueueItemId(1),
            claim_id: ClaimId(1),
            run_id: RunId(2),
            result: Box::new(AnalysisResult {
                requested_target: settings.requested_target,
                successful_target: settings.requested_target,
                fallback_floor: settings.fallback_floor,
                fallback_step: settings.fallback_step,
                failed_attempts: Vec::new(),
                measurement: SearchMeasurement {
                    crf: Crf(30_000),
                    score: VmafScore(9_500),
                    predicted_size: 1_000,
                    predicted_percent_basis_points: 5_000,
                    predicted_duration_ms: 60_000,
                    from_cache: false,
                },
                profile: settings.profile.clone(),
            }),
        }),
        Command::Worker(WorkerCommand::Output(OutputDelta::OutputStarted {
            transaction: Box::new(transaction.clone()),
        })),
        Command::Worker(WorkerCommand::Output(OutputDelta::StagingCreated {
            run_id: RunId(2),
            initial,
        })),
    ] {
        let applied = apply(&mut state, command);
        assert!(!matches!(applied.reply, Reply::Rejected { .. }));
        durable.extend(applied.durable);
    }
    let (mut writer, _replay) = JournalWriter::open(&journal_path).expect("journal writer");
    writer.append_batch(&durable).expect("crash fixture batch");
    drop(writer);

    let deferred =
        EngineRuntime::start(engine_config(&directory, missing_tools())).expect("tool-free start");
    let DriverEvent::Snapshot(snapshot) = deferred.events.recv().expect("deferred snapshot") else {
        panic!("expected deferred snapshot");
    };
    assert!(
        matches!(
            snapshot.durable.queue.first().expect("queued item").state,
            QueueItemState::Running { .. }
        ),
        "item must stay active while settlement is deferred: {snapshot:?}"
    );
    let deferred_transaction = snapshot
        .durable
        .outputs
        .get(&RunId(2))
        .expect("deferred transaction");
    assert!(!deferred_transaction.is_settled());
    assert!(transaction.staging.exists(), "staging must be untouched");
    deferred.shutdown().expect("deferred shutdown");

    let executable = std::env::current_exe().expect("test executable");
    let recovered = EngineRuntime::start(engine_config(
        &directory,
        ToolsConfig::Fixed(FixedTools {
            tools: located(
                &MediaTools {
                    ffmpeg: executable.clone(),
                    ffprobe: executable,
                },
                ToolSource::SearchPath,
            ),
            revisions: fixture_revisions(),
        }),
    ))
    .expect("recovery with tools");
    let DriverEvent::Snapshot(snapshot) = recovered.events.recv().expect("recovered snapshot")
    else {
        panic!("expected recovered snapshot");
    };
    assert!(
        matches!(
            snapshot.durable.queue.first().expect("queued item").state,
            QueueItemState::Finished(ItemOutcome::Incomplete)
        ),
        "deferred recovery must complete once tools exist: {snapshot:?}"
    );
    assert!(!transaction.staging.exists());
    recovered.shutdown().expect("recovered shutdown");
}

/// A PATH-style directory holding contract-fixture copies that answer the
/// ffprobe JSON version document and the synthetic capability probes.
#[expect(clippy::expect_used, reason = "fixture setup")]
fn fixture_path_directory(directory: &TestDirectory) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_crfty-contract-fixture"));
    let path_dir = directory.path().join("bin");
    fs::create_dir(&path_dir).expect("fixture PATH directory");
    fs::copy(&fixture, path_dir.join(tool_file_name("ffmpeg"))).expect("fixture ffmpeg");
    fs::copy(&fixture, path_dir.join(tool_file_name("ffprobe"))).expect("fixture ffprobe");
    path_dir
}

fn tool_file_name(binary: &str) -> String {
    if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_owned()
    }
}

fn fixture_media(path_dir: &Path) -> MediaTools {
    MediaTools {
        ffmpeg: path_dir.join(tool_file_name("ffmpeg")),
        ffprobe: path_dir.join(tool_file_name("ffprobe")),
    }
}

fn search_path_environment(path_dir: &Path) -> DiscoveryEnvironment {
    DiscoveryEnvironment {
        ffmpeg_override: None,
        ffprobe_override: None,
        search_path: Some(path_dir.as_os_str().to_owned()),
    }
}

/// Directs the fixture's capability probe: see `fake_capability_probe`.
#[expect(clippy::expect_used, reason = "fixture setup")]
fn write_probe_marker(path_dir: &Path, directive: &str) {
    fs::write(path_dir.join("crfty-fixture-probe"), directive).expect("probe marker");
}

#[test]
fn discovery_reports_every_tool_missing_when_no_tier_provides_one() {
    let availability = discovery::discover(
        &DiscoveryEnvironment::default(),
        &ToolPathSettings::default(),
    );
    assert_eq!(
        availability,
        ToolAvailability::Missing {
            failures: vec![
                ToolLocationFailure::NotOnSearchPath {
                    tool: MediaTool::Ffmpeg
                },
                ToolLocationFailure::NotOnSearchPath {
                    tool: MediaTool::Ffprobe
                },
            ],
        }
    );
}

#[test]
fn discovery_locates_search_path_tools_without_verifying_them() {
    let directory = TestDirectory::new("discovery-search-path");
    let path_dir = fixture_path_directory(&directory);
    let availability = discovery::discover(
        &search_path_environment(&path_dir),
        &ToolPathSettings::default(),
    );
    assert_eq!(
        availability,
        ToolAvailability::Located {
            tools: located(&fixture_media(&path_dir), ToolSource::SearchPath),
            verification: ToolVerification::Pending,
        }
    );
}

#[test]
fn settings_paths_win_over_the_search_path_and_fail_closed() {
    let directory = TestDirectory::new("discovery-settings");
    let path_dir = fixture_path_directory(&directory);
    let media = fixture_media(&path_dir);

    let configured = ToolPathSettings {
        ffmpeg: Some(media.ffmpeg.clone()),
        ffprobe: Some(media.ffprobe.clone()),
    };
    assert_eq!(
        discovery::discover(&DiscoveryEnvironment::default(), &configured),
        ToolAvailability::Located {
            tools: located(&media, ToolSource::Settings),
            verification: ToolVerification::Pending,
        }
    );

    // A configured path that names nothing is a defect to report, not a
    // reason to fall through to whatever PATH holds.
    let dangling = directory.path().join("missing-ffmpeg");
    let half_configured = ToolPathSettings {
        ffmpeg: Some(dangling.clone()),
        ffprobe: None,
    };
    assert_eq!(
        discovery::discover(&search_path_environment(&path_dir), &half_configured),
        ToolAvailability::Missing {
            failures: vec![ToolLocationFailure::SettingsPathIsNotAFile {
                tool: MediaTool::Ffmpeg,
                path: dangling,
            }],
        }
    );
}

#[test]
fn environment_overrides_win_over_settings_and_fail_closed() {
    let directory = TestDirectory::new("discovery-environment");
    let path_dir = fixture_path_directory(&directory);
    let media = fixture_media(&path_dir);
    let dangling = directory.path().join("missing-ffprobe");

    // Settings name files that do not exist, yet the override wins outright.
    let stale_settings = ToolPathSettings {
        ffmpeg: Some(dangling.clone()),
        ffprobe: Some(dangling.clone()),
    };
    let overrides = DiscoveryEnvironment {
        ffmpeg_override: Some(media.ffmpeg.clone().into_os_string()),
        ffprobe_override: Some(media.ffprobe.clone().into_os_string()),
        search_path: None,
    };
    assert_eq!(
        discovery::discover(&overrides, &stale_settings),
        ToolAvailability::Located {
            tools: located(&media, ToolSource::Environment),
            verification: ToolVerification::Pending,
        }
    );

    let broken_override = DiscoveryEnvironment {
        ffmpeg_override: None,
        ffprobe_override: Some(dangling.clone().into_os_string()),
        search_path: Some(path_dir.as_os_str().to_owned()),
    };
    assert_eq!(
        discovery::discover(&broken_override, &ToolPathSettings::default()),
        ToolAvailability::Missing {
            failures: vec![ToolLocationFailure::EnvironmentPathIsNotAFile {
                tool: MediaTool::Ffprobe,
                path: dangling,
            }],
        }
    );
}

/// Drains events until `predicate` accepts one, returning everything seen up
/// to and including it; panics on stream close or after ten seconds.
fn drain_until(
    events: &std::sync::mpsc::Receiver<DriverEvent>,
    predicate: impl Fn(&DriverEvent) -> bool,
    what: &str,
) -> Vec<DriverEvent> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut seen = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .unwrap_or_else(|| panic!("timed out waiting for {what}"));
        let event = events
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("stream ended waiting for {what}: {error}"));
        let matched = predicate(&event);
        seen.push(event);
        if matched {
            return seen;
        }
    }
}

fn wait_for_tools(
    events: &std::sync::mpsc::Receiver<DriverEvent>,
    predicate: impl Fn(&ToolAvailability) -> bool,
    what: &str,
) -> ToolAvailability {
    let seen = drain_until(
        events,
        |event| {
            matches!(
                event,
                DriverEvent::Ephemeral(EphemeralDelta::ToolsChanged(tools)) if predicate(tools)
            )
        },
        what,
    );
    match seen.into_iter().next_back() {
        Some(DriverEvent::Ephemeral(EphemeralDelta::ToolsChanged(tools))) => tools,
        other => panic!("drained past the matching tools event: {other:?}"),
    }
}

fn wait_for_session(events: &std::sync::mpsc::Receiver<DriverEvent>, expected: SessionState) {
    let _seen = drain_until(
        events,
        |event| {
            matches!(
                event,
                DriverEvent::Ephemeral(EphemeralDelta::SessionChanged(session)) if *session == expected
            )
        },
        "session state change",
    );
}

fn is_verified(tools: &ToolAvailability) -> bool {
    matches!(
        tools,
        ToolAvailability::Located {
            verification: ToolVerification::Verified { .. },
            ..
        }
    )
}

fn is_pending(tools: &ToolAvailability) -> bool {
    matches!(
        tools,
        ToolAvailability::Located {
            verification: ToolVerification::Pending,
            ..
        }
    )
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn session_start_probes_located_tools_and_records_their_revisions() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("probe-verifies");
    let path_dir = fixture_path_directory(&directory);
    let input = directory.path().join("video.mkv");
    fs::write(&input, vec![7_u8; 8192]).expect("input media");
    let engine = EngineRuntime::start(engine_config(
        &directory,
        ToolsConfig::Discover(search_path_environment(&path_dir)),
    ))
    .expect("engine start");
    let pending = wait_for_tools(&engine.events, is_pending, "pending tools after startup");
    assert_eq!(
        pending,
        ToolAvailability::Located {
            tools: located(&fixture_media(&path_dir), ToolSource::SearchPath),
            verification: ToolVerification::Pending,
        }
    );

    assert_eq!(
        engine
            .commands
            .submit_queue(add_one(QueueItemId(1), input))
            .expect("add reply"),
        Reply::Accepted
    );
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("start reply"),
        Reply::Accepted
    );
    let verified = wait_for_tools(&engine.events, is_verified, "verified tools");
    let ToolAvailability::Located {
        verification: ToolVerification::Verified { revisions },
        ..
    } = verified
    else {
        unreachable!();
    };
    assert_eq!(revisions.ab_av1, AB_AV1_REVISION);
    assert_eq!(revisions.ffmpeg, "fixture-8.1.2");
    assert_eq!(revisions.encoder, "fixture-8.1.2");

    // The probed revisions are the provenance frozen into the claim.
    let seen = drain_until(
        &engine.events,
        |event| {
            matches!(
                event,
                DriverEvent::Durable(DurableDelta::ItemPrepared { .. })
            )
        },
        "prepared claim",
    );
    let Some(DriverEvent::Durable(DurableDelta::ItemPrepared { spec })) = seen.last() else {
        unreachable!();
    };
    assert_eq!(spec.execution.profile.ab_av1_revision, AB_AV1_REVISION);
    assert_eq!(spec.execution.profile.ffmpeg_revision, "fixture-8.1.2");
    assert_eq!(spec.execution.profile.encoder_revision, "fixture-8.1.2");
    engine.shutdown().expect("engine shutdown");
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn incapable_tools_fail_verification_and_never_reserve_an_item() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("probe-incapable");
    let path_dir = fixture_path_directory(&directory);
    write_probe_marker(&path_dir, "missing=libsvtav1");
    let input = directory.path().join("video.mkv");
    fs::write(&input, vec![7_u8; 8192]).expect("input media");
    let engine = EngineRuntime::start(engine_config(
        &directory,
        ToolsConfig::Discover(search_path_environment(&path_dir)),
    ))
    .expect("engine start");
    let _pending = wait_for_tools(&engine.events, is_pending, "pending tools after startup");
    assert_eq!(
        engine
            .commands
            .submit_queue(add_one(QueueItemId(1), input))
            .expect("add reply"),
        Reply::Accepted
    );
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("start reply"),
        Reply::Accepted
    );
    let failed = wait_for_tools(
        &engine.events,
        |tools| {
            matches!(
                tools,
                ToolAvailability::Located {
                    verification: ToolVerification::Failed(_),
                    ..
                }
            )
        },
        "failed verification",
    );
    let ToolAvailability::Located {
        verification:
            ToolVerification::Failed(ProbeFailure::Unsupported {
                capability,
                diagnostic,
            }),
        ..
    } = failed
    else {
        panic!("expected an unsupported-capability failure: {failed:?}");
    };
    assert_eq!(capability, ToolCapability::Svtav1Encoder);
    assert!(diagnostic.contains("libsvtav1"), "{diagnostic}");

    // The session ends without a claim: the item is still queued.
    let seen = drain_until(
        &engine.events,
        |event| {
            matches!(
                event,
                DriverEvent::Ephemeral(EphemeralDelta::SessionChanged(SessionState::Idle))
            )
        },
        "idle after failed verification",
    );
    assert!(
        !seen.iter().any(|event| matches!(
            event,
            DriverEvent::Durable(DurableDelta::ItemReserved { .. })
        )),
        "an unverified session must not reserve: {seen:?}"
    );
    engine.shutdown().expect("engine shutdown");
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn shutdown_terminates_a_hanging_probe_promptly() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("probe-hang");
    let path_dir = fixture_path_directory(&directory);
    write_probe_marker(&path_dir, "hang");
    let engine = EngineRuntime::start(engine_config(
        &directory,
        ToolsConfig::Discover(search_path_environment(&path_dir)),
    ))
    .expect("engine start");
    let _pending = wait_for_tools(&engine.events, is_pending, "pending tools after startup");
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("start reply"),
        Reply::Accepted
    );
    wait_for_session(&engine.events, SessionState::Running);
    // Let the worker reach the hanging encoder step before pulling the plug.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let started = std::time::Instant::now();
    engine.shutdown().expect("engine shutdown");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "shutdown waited on the hanging probe for {:?}",
        started.elapsed()
    );
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn rediscovery_and_changed_tool_paths_replace_the_located_tools() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("rediscover");
    let path_dir = fixture_path_directory(&directory);
    let media = fixture_media(&path_dir);
    let engine = EngineRuntime::start(engine_config(
        &directory,
        ToolsConfig::Discover(search_path_environment(&path_dir)),
    ))
    .expect("engine start");
    let _pending = wait_for_tools(&engine.events, is_pending, "pending tools after startup");
    // An empty queue: the session only probes, then finishes.
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("start reply"),
        Reply::Accepted
    );
    let _verified = wait_for_tools(&engine.events, is_verified, "verified tools");
    wait_for_session(&engine.events, SessionState::Idle);

    // Rediscovery starts verification over on the same tools.
    assert_eq!(
        engine
            .commands
            .submit_tools(ToolsCommand::Rediscover)
            .expect("rediscover reply"),
        Reply::Accepted
    );
    let republished = wait_for_tools(&engine.events, is_pending, "rediscovered tools");
    assert_eq!(
        republished,
        ToolAvailability::Located {
            tools: located(&media, ToolSource::SearchPath),
            verification: ToolVerification::Pending,
        }
    );

    // A changed Settings path rediscovers on its own, and a dangling one
    // fails closed even though PATH still holds working tools.
    let dangling = directory.path().join("missing-ffmpeg");
    let mut settings = Settings::default();
    settings.tools.ffmpeg = Some(dangling.clone());
    assert_eq!(
        engine
            .commands
            .submit_settings(SettingsCommand::Set {
                settings: settings.clone(),
            })
            .expect("settings reply"),
        Reply::Accepted
    );
    let missing = wait_for_tools(
        &engine.events,
        |tools| matches!(tools, ToolAvailability::Missing { .. }),
        "missing after dangling settings path",
    );
    assert_eq!(
        missing,
        ToolAvailability::Missing {
            failures: vec![ToolLocationFailure::SettingsPathIsNotAFile {
                tool: MediaTool::Ffmpeg,
                path: dangling.clone(),
            }],
        }
    );
    let start = engine
        .commands
        .submit_session(SessionCommand::Start)
        .expect("start reply");
    let Reply::Rejected { reason } = start else {
        panic!("expected a fail-closed session start: {start:?}");
    };
    assert!(reason.contains("configured ffmpeg path"), "{reason}");
    assert!(
        !reason.contains(&dangling.to_string_lossy().into_owned()),
        "rejection reasons never carry paths: {reason}"
    );

    settings.tools = ToolPathSettings {
        ffmpeg: Some(media.ffmpeg.clone()),
        ffprobe: Some(media.ffprobe.clone()),
    };
    assert_eq!(
        engine
            .commands
            .submit_settings(SettingsCommand::Set { settings })
            .expect("settings reply"),
        Reply::Accepted
    );
    let configured = wait_for_tools(
        &engine.events,
        |tools| matches!(tools, ToolAvailability::Located { .. }),
        "tools from settings paths",
    );
    assert_eq!(
        configured,
        ToolAvailability::Located {
            tools: located(&media, ToolSource::Settings),
            verification: ToolVerification::Pending,
        }
    );
    engine.shutdown().expect("engine shutdown");
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn force_stop_cancels_the_session_probe_before_a_run_exists_and_allows_restart() {
    let _serial = ENGINE_GUARD.lock().expect("engine guard");
    let directory = TestDirectory::new("probe-force-stop");
    let path_dir = fixture_path_directory(&directory);
    write_probe_marker(&path_dir, "hang");
    let engine = EngineRuntime::start(engine_config(
        &directory,
        ToolsConfig::Discover(search_path_environment(&path_dir)),
    ))
    .expect("engine");
    let _pending = wait_for_tools(&engine.events, is_pending, "pending tools");
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("start"),
        Reply::Accepted
    );
    let heartbeat = path_dir.join("crfty-fixture-probe.heartbeat");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !heartbeat.exists() {
        assert!(std::time::Instant::now() < deadline, "probe did not start");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::ForceStop)
            .expect("stop"),
        Reply::Accepted
    );
    wait_for_session(&engine.events, SessionState::Idle);
    loop {
        let before = fs::read(&heartbeat).expect("heartbeat");
        std::thread::sleep(std::time::Duration::from_millis(200));
        if !before.is_empty() && before == fs::read(&heartbeat).expect("heartbeat") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "force stop did not settle probe descendants"
        );
    }
    write_probe_marker(&path_dir, "");
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("restart"),
        Reply::Accepted
    );
    let _verified = wait_for_tools(
        &engine.events,
        |tools| {
            matches!(
                tools,
                ToolAvailability::Located {
                    verification: ToolVerification::Verified { .. },
                    ..
                }
            )
        },
        "verified tools after restart",
    );
    wait_for_session(&engine.events, SessionState::Idle);
    engine.shutdown().expect("shutdown");
}
