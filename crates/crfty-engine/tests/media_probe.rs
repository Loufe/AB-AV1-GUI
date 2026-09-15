//! Real-process contract for supervised media probes: a hung ffprobe ends at
//! its deadline or on cancellation with its descendants dead, verification
//! verdicts and rejection diagnostics are typed, and a Force Stop reaches a
//! probe that hangs at claim time or during output verification.
#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use crfty_core::{
    AnalysisIntent, AnalysisProfile, DurableDelta, EphemeralDelta, ExecutionSettings, ItemOutcome,
    Operation, OutputTarget, OverwriteDecision, QueueAddRequest, QueueCommand, QueueItemId, Reply,
    SessionCommand, SessionState, ToolRevisions, ToolSource, VideoCodec,
};
use crfty_engine::{
    coordinator::{EngineConfig, EngineRuntime, FixedTools, ToolsConfig},
    driver::DriverEvent,
    media::{MediaError, MediaInspector},
    process_supervisor::ProcessCancellation,
};

const EVENT_TIMEOUT: Duration = Duration::from_secs(15);
const SHORT_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const MINIMUM_OUTPUT_BYTES: u64 = 1024;
/// `AbAv1Runtime` is a process-wide singleton; engine-starting tests in this
/// file must not overlap.
static ENGINE_TEST_GATE: Mutex<()> = Mutex::new(());

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn hung_probe_ends_at_the_deadline_with_its_descendants_dead() {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
    let input = fixture.path().join("hang-claim.mkv");
    fs::write(&input, vec![1_u8; 8192]).expect("input fixture");
    let inspector = MediaInspector::new(ffprobe).with_probe_timeout(SHORT_PROBE_TIMEOUT);

    let started = Instant::now();
    let error = inspector
        .observe(&input, &ProcessCancellation::new())
        .expect_err("a hung probe must not observe");
    assert!(
        started.elapsed() < EVENT_TIMEOUT,
        "the deadline must bound the probe"
    );
    let MediaError::TimedOut { timeout, .. } = error else {
        panic!("expected a typed timeout: {error}");
    };
    assert_eq!(timeout, SHORT_PROBE_TIMEOUT);
    assert_heartbeat_stopped(&input.with_extension("heartbeat"));
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn cancellation_interrupts_a_hung_probe_before_its_deadline() {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
    let input = fixture.path().join("hang-claim.mkv");
    fs::write(&input, vec![1_u8; 8192]).expect("input fixture");
    let heartbeat = input.with_extension("heartbeat");
    let inspector = MediaInspector::new(ffprobe);
    let cancellation = ProcessCancellation::new();
    let canceller = {
        let cancellation = cancellation.clone();
        let heartbeat = heartbeat.clone();
        thread::spawn(move || {
            wait_for_file(&heartbeat);
            cancellation.cancel();
        })
    };

    let started = Instant::now();
    let error = inspector
        .observe(&input, &cancellation)
        .expect_err("a cancelled probe must not observe");
    assert!(matches!(error, MediaError::Cancelled), "{error}");
    assert!(
        started.elapsed() < EVENT_TIMEOUT,
        "cancellation must not wait for the default deadline"
    );
    canceller.join().expect("canceller thread");
    assert_heartbeat_stopped(&heartbeat);
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn verification_returns_the_identity_or_a_typed_verdict() {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
    let inspector = MediaInspector::new(ffprobe);
    let cancellation = ProcessCancellation::new();

    let av1 = fixture.path().join("already-av1.mp4");
    fs::write(&av1, vec![1_u8; 8192]).expect("av1 fixture");
    let identity = inspector
        .verify_av1(&av1, MINIMUM_OUTPUT_BYTES, &cancellation)
        .expect("an AV1 output verifies");
    assert_eq!(identity.destructive.size, 8192);

    let h264 = fixture.path().join("plain.mkv");
    fs::write(&h264, vec![2_u8; 8192]).expect("h264 fixture");
    let error = inspector
        .verify_av1(&h264, MINIMUM_OUTPUT_BYTES, &cancellation)
        .expect_err("a non-AV1 output must not verify");
    assert!(
        matches!(
            error,
            MediaError::OutputNotAv1 {
                codec: VideoCodec::H264
            }
        ),
        "{error}"
    );

    let tiny = fixture.path().join("tiny-already-av1.mp4");
    fs::write(&tiny, vec![3_u8; 16]).expect("tiny fixture");
    let error = inspector
        .verify_av1(&tiny, MINIMUM_OUTPUT_BYTES, &cancellation)
        .expect_err("a truncated output must not verify");
    assert!(
        matches!(
            error,
            MediaError::OutputTooSmall {
                size_bytes: 16,
                minimum_bytes: MINIMUM_OUTPUT_BYTES
            }
        ),
        "{error}"
    );
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn rejected_probe_carries_the_tool_diagnostic() {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
    let input = fixture.path().join("reject-secret.mkv");
    fs::write(&input, vec![1_u8; 8192]).expect("input fixture");
    let inspector = MediaInspector::new(ffprobe);

    let error = inspector
        .observe(&input, &ProcessCancellation::new())
        .expect_err("a rejected probe must not observe");
    let MediaError::Rejected { diagnostic } = &error else {
        panic!("expected a typed rejection: {error}");
    };
    assert!(
        String::from_utf8_lossy(diagnostic.as_bytes()).contains("fixture rejected"),
        "diagnostic must carry the tool's stderr"
    );
    assert!(error.diagnostic().is_some());
    assert!(!error.is_cancelled());
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn force_stop_reaches_a_hung_claim_time_probe() {
    let _gate = ENGINE_TEST_GATE.lock().expect("engine test gate");
    let fixture = tempfile::tempdir().expect("fixture directory");
    let media_root = fixture.path().join("media");
    fs::create_dir(&media_root).expect("media directory");
    let input = media_root.join("hang-claim.mkv");
    fs::write(&input, vec![1_u8; 8192]).expect("input fixture");
    let heartbeat = input.with_extension("heartbeat");
    let engine = start_engine(fixture.path());

    assert_eq!(
        engine
            .commands
            .submit_queue(add_one(QueueItemId(1), &input))
            .expect("queue add reply"),
        Reply::Accepted
    );
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("session start reply"),
        Reply::Accepted
    );
    wait_for_file(&heartbeat);
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::ForceStop)
            .expect("force stop reply"),
        Reply::Accepted
    );
    wait_for_event(&engine, "stopped item", |event| {
        matches!(
            event,
            DriverEvent::Durable(DurableDelta::ItemFinished {
                outcome: ItemOutcome::Stopped,
                ..
            })
        )
    });
    wait_for_event(&engine, "idle session", |event| {
        matches!(
            event,
            DriverEvent::Ephemeral(EphemeralDelta::SessionChanged(SessionState::Idle))
        )
    });
    assert_heartbeat_stopped(&heartbeat);
    engine.shutdown().expect("engine shutdown");
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn force_stop_reaches_a_hung_output_verification_probe() {
    let _gate = ENGINE_TEST_GATE.lock().expect("engine test gate");
    let fixture = tempfile::tempdir().expect("fixture directory");
    let media_root = fixture.path().join("media");
    fs::create_dir(&media_root).expect("media directory");
    let input = media_root.join("hang-verify-already-av1.mp4");
    fs::write(&input, vec![1_u8; 8192]).expect("input fixture");
    let engine = start_engine(fixture.path());

    assert_eq!(
        engine
            .commands
            .submit_queue(add_one(QueueItemId(1), &input))
            .expect("queue add reply"),
        Reply::Accepted
    );
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::Start)
            .expect("session start reply"),
        Reply::Accepted
    );
    let heartbeat = wait_for_heartbeat_in(&media_root);
    assert!(
        heartbeat.to_string_lossy().contains(".part."),
        "the hung probe must be the staging verification: {}",
        heartbeat.display()
    );
    assert_eq!(
        engine
            .commands
            .submit_session(SessionCommand::ForceStop)
            .expect("force stop reply"),
        Reply::Accepted
    );
    wait_for_event(&engine, "stopped item", |event| {
        matches!(
            event,
            DriverEvent::Durable(DurableDelta::ItemFinished {
                outcome: ItemOutcome::Stopped,
                ..
            })
        )
    });
    wait_for_event(&engine, "idle session", |event| {
        matches!(
            event,
            DriverEvent::Ephemeral(EphemeralDelta::SessionChanged(SessionState::Idle))
        )
    });
    assert_heartbeat_stopped(&heartbeat);
    assert!(
        !media_root
            .join("hang-verify-already-av1_remuxed.mkv")
            .exists()
    );
    let staging_remains = fs::read_dir(&media_root)
        .expect("media directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_none_or(|extension| extension != "heartbeat")
        })
        .any(|path| path.to_string_lossy().contains(".part."));
    assert!(!staging_remains, "the abandoned staging must be removed");
    engine.shutdown().expect("engine shutdown");
}

fn add_one(item_id: QueueItemId, input: &Path) -> QueueCommand {
    QueueCommand::AddMany {
        requests: vec![QueueAddRequest {
            item_id,
            input: input.to_path_buf(),
            path_hash: None,
            identity: None,
            timestamp_reliability: crfty_core::TimestampReliability::Unknown,
            operation: Operation::Convert,
            intent: AnalysisIntent::ReuseIfFresh,
            output_target: OutputTarget::Suffix {
                suffix: "_remuxed".to_owned(),
            },
            overwrite: OverwriteDecision::FollowSettings,
        }],
    }
}

#[expect(clippy::expect_used, reason = "fixture setup")]
fn start_engine(directory: &Path) -> EngineRuntime {
    let ffprobe = copy_as_tool(directory, "ffprobe");
    let ffmpeg = copy_as_tool(directory, "ffmpeg");
    let engine = EngineRuntime::start(config(directory, ffmpeg, ffprobe)).expect("engine");
    let DriverEvent::Snapshot(_) = engine.events.recv().expect("startup snapshot") else {
        panic!("expected the startup snapshot first");
    };
    engine
}

fn wait_for_event(
    engine: &EngineRuntime,
    label: &str,
    mut matches: impl FnMut(&DriverEvent) -> bool,
) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        let event = match engine.events.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("engine event stream disconnected before {label}")
            }
        };
        if let DriverEvent::Fatal { message } = &event {
            panic!("engine failed before {label}: {message}");
        }
        if matches(&event) {
            return;
        }
    }
    panic!("engine never reported {label}");
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("the hung probe never started its heartbeat");
}

#[expect(clippy::expect_used, reason = "test assertion")]
fn wait_for_heartbeat_in(directory: &Path) -> PathBuf {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        let heartbeat = fs::read_dir(directory)
            .expect("fixture directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "heartbeat")
            });
        if let Some(heartbeat) = heartbeat {
            return heartbeat;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("the hung probe never started its heartbeat");
}

/// The heartbeat descendant rewrites its marker every 20 ms while alive, so
/// an unchanged marker across a longer window proves the process group died.
#[expect(clippy::expect_used, reason = "test assertion")]
fn assert_heartbeat_stopped(heartbeat: &Path) {
    let before = fs::read(heartbeat).expect("heartbeat marker");
    thread::sleep(Duration::from_millis(200));
    let after = fs::read(heartbeat).expect("heartbeat marker");
    assert_eq!(before, after, "the heartbeat descendant outlived ffprobe");
}

#[expect(clippy::expect_used, reason = "fixture setup")]
fn copy_as_tool(directory: &Path, name: &str) -> PathBuf {
    let extension = std::env::consts::EXE_EXTENSION;
    let file_name = if extension.is_empty() {
        name.to_owned()
    } else {
        format!("{name}.{extension}")
    };
    let destination = directory.join(file_name);
    fs::copy(
        PathBuf::from(env!("CARGO_BIN_EXE_crfty-contract-fixture")),
        &destination,
    )
    .expect("copy fake tool");
    destination
}

fn config(directory: &Path, ffmpeg: PathBuf, ffprobe: PathBuf) -> EngineConfig {
    let revisions = ToolRevisions {
        ab_av1: "probe-test".to_owned(),
        ffmpeg: "probe-test".to_owned(),
        encoder: "probe-test".to_owned(),
    };
    let mut profile = AnalysisProfile::production();
    profile.ab_av1_revision = revisions.ab_av1.clone();
    profile.ffmpeg_revision = revisions.ffmpeg.clone();
    profile.encoder_revision = revisions.encoder.clone();
    EngineConfig {
        journal_path: directory.join("journal.jsonl"),
        config_path: directory.join("config.json"),
        tools: ToolsConfig::Fixed(FixedTools {
            tools: crfty_core::LocatedTools {
                ffmpeg: crfty_core::LocatedTool {
                    path: ffmpeg,
                    source: ToolSource::Environment,
                },
                ffprobe: crfty_core::LocatedTool {
                    path: ffprobe,
                    source: ToolSource::Environment,
                },
            },
            revisions,
        }),
        execution: ExecutionSettings::production(profile, false),
    }
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn finalization_cancellation_preserves_the_correct_files_across_restart() {
    let _gate = ENGINE_TEST_GATE.lock().expect("engine test gate");
    for (name, replace, should_abandon) in [
        ("hang-ready-already-av1.mp4", false, true),
        ("hang-promoted-already-av1.mp4", false, false),
        ("hang-retirement-already-av1.mp4", true, false),
    ] {
        let fixture = tempfile::tempdir().expect("fixture directory");
        let media = fixture.path().join("media");
        fs::create_dir(&media).expect("media directory");
        let input = media.join(name);
        let original = vec![1_u8; 8192];
        fs::write(&input, &original).expect("input");
        let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
        let ffmpeg = copy_as_tool(fixture.path(), "ffmpeg");
        let configuration = config(fixture.path(), ffmpeg, ffprobe);
        let engine = EngineRuntime::start(configuration.clone()).expect("engine");
        let DriverEvent::Snapshot(mut snapshot) = engine.events.recv().expect("snapshot") else {
            panic!("snapshot");
        };
        let mut request = add_one(QueueItemId(1), &input);
        if let QueueCommand::AddMany { requests } = &mut request {
            for request in requests {
                if replace {
                    request.output_target = OutputTarget::Replace;
                }
            }
        }
        assert_eq!(
            engine.commands.submit_queue(request).expect("add"),
            Reply::Accepted
        );
        assert_eq!(
            engine
                .commands
                .submit_session(SessionCommand::Start)
                .expect("start"),
            Reply::Accepted
        );
        let heartbeat = wait_for_heartbeat_in(&media);
        assert_eq!(
            heartbeat.to_string_lossy().contains(".part."),
            should_abandon
        );
        assert_eq!(
            engine
                .commands
                .submit_session(SessionCommand::ForceStop)
                .expect("stop"),
            Reply::Accepted
        );
        let mut terminal = None;
        let deadline = Instant::now() + EVENT_TIMEOUT;
        loop {
            let event = engine
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("settlement event");
            match event {
                DriverEvent::Durable(delta) => {
                    if let DurableDelta::ItemFinished { outcome, .. } = &delta {
                        terminal = Some(outcome.clone());
                    }
                    crfty_core::fold(&mut snapshot.durable, &delta);
                }
                DriverEvent::Ephemeral(EphemeralDelta::SessionChanged(SessionState::Idle)) => break,
                DriverEvent::Fatal { message } => panic!("engine failed: {message}"),
                _ => {}
            }
        }
        let transaction = snapshot
            .durable
            .outputs
            .values()
            .next()
            .expect("output transaction");
        if should_abandon {
            assert_eq!(terminal, Some(ItemOutcome::Stopped));
            assert_eq!(transaction.state, crfty_core::OutputState::Abandoned);
            assert!(!transaction.final_path.exists());
        } else {
            let Some(ItemOutcome::Failed(facts)) = terminal else {
                panic!("expected visible conflict");
            };
            assert_eq!(facts.kind, crfty_core::FailureKind::OutputConflict);
            assert!(facts.message.contains("cancelled"));
            assert!(matches!(
                transaction.state,
                crfty_core::OutputState::Conflict { .. }
            ));
            assert!(transaction.final_path.exists());
        }
        assert!(!transaction.staging.exists());
        assert_eq!(fs::read(&input).expect("preserved input"), original);
        assert_heartbeat_stopped(&heartbeat);
        engine.shutdown().expect("shutdown");
        let restarted = EngineRuntime::start(configuration).expect("restart");
        let DriverEvent::Snapshot(recovered) = restarted.events.recv().expect("restart snapshot")
        else {
            panic!("snapshot");
        };
        assert_eq!(recovered.durable.outputs, snapshot.durable.outputs);
        assert_eq!(recovered.durable.queue, snapshot.durable.queue);
        assert_eq!(fs::read(&input).expect("input after restart"), original);
        assert_eq!(transaction.final_path.exists(), !should_abandon);
        restarted.shutdown().expect("restart shutdown");
    }
}

#[test]
#[expect(clippy::expect_used, reason = "test assertion")]
fn output_verification_deadline_settles_descendants_and_returns_a_typed_timeout() {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
    let staging = fixture.path().join("hang-claim-output.mkv");
    fs::write(&staging, vec![1_u8; 8192]).expect("staging");
    let inspector = MediaInspector::new(ffprobe).with_probe_timeout(SHORT_PROBE_TIMEOUT);
    let started = Instant::now();
    let error = inspector
        .verify_av1(&staging, MINIMUM_OUTPUT_BYTES, &ProcessCancellation::new())
        .expect_err("deadline");
    assert!(
        matches!(error, MediaError::TimedOut { timeout, .. } if timeout == SHORT_PROBE_TIMEOUT)
    );
    assert!(started.elapsed() < EVENT_TIMEOUT);
    assert_heartbeat_stopped(&staging.with_extension("heartbeat"));
}
