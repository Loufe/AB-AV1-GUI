#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use crfty_core::{
    AnalysisActivity, AnalysisDelta, AnalysisFileScan, AnalysisProfile, AnalysisRowEntry,
    AnalysisSnapshot, EphemeralDelta, ExecutionSettings, ToolRevisions, ToolSource, VideoExtension,
    fold_analysis,
};
use crfty_engine::{
    coordinator::{EngineConfig, EngineRuntime, ToolsConfig},
    driver::DriverEvent,
    vendor::discovery::{CurrentTools, DiscoveredTools, MediaTools},
};

const FILE_COUNT: usize = 40;
const EVENT_TIMEOUT: Duration = Duration::from_secs(15);
static ENGINE_TEST_GATE: Mutex<()> = Mutex::new(());

#[test]
fn basic_scan_streams_results_with_a_bounded_parallel_probe_pool() {
    let _gate = ENGINE_TEST_GATE.lock().expect("engine test gate");
    let fixture = tempfile::tempdir().expect("fixture directory");
    let media_root = fixture.path().join("media");
    fs::create_dir(&media_root).expect("media directory");
    for index in 0..FILE_COUNT {
        fs::write(
            media_root.join(format!("slow-{index:03}.mkv")),
            vec![index as u8; 8192],
        )
        .expect("media fixture");
    }
    let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
    let ffmpeg = copy_as_tool(fixture.path(), "ffmpeg");
    let engine = EngineRuntime::start(config(fixture.path(), ffmpeg, ffprobe)).expect("engine");
    let generation = engine
        .commands
        .begin_analysis_discovery(
            vec![media_root.clone()],
            BTreeSet::from([VideoExtension::Mkv]),
        )
        .expect("begin discovery");
    let mut snapshot = AnalysisSnapshot::default();
    wait_for_activity(
        &engine,
        &mut snapshot,
        generation,
        AnalysisActivity::Discovered,
    );

    engine
        .commands
        .begin_analysis_basic_scan(generation)
        .expect("begin Basic Scan");
    let mut scanned_batches = 0_usize;
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        let event = match engine.events.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("Basic Scan event stream disconnected")
            }
        };
        if let DriverEvent::Ephemeral(EphemeralDelta::Analysis(delta)) = event {
            if matches!(delta, AnalysisDelta::RowsUpserted { .. }) {
                scanned_batches += 1;
            }
            fold_analysis(&mut snapshot, &delta);
        }
        if snapshot.current.as_ref().is_some_and(|current| {
            current.id == generation && current.activity == AnalysisActivity::Ready
        }) {
            break;
        }
    }
    let current = snapshot.current.as_ref().expect("Analysis generation");
    assert_eq!(current.activity, AnalysisActivity::Ready);
    assert_eq!(
        current
            .rows
            .values()
            .filter(|row| matches!(
                row.entry,
                AnalysisRowEntry::File {
                    scan: AnalysisFileScan::Scanned { .. }
                }
            ))
            .count(),
        FILE_COUNT
    );
    assert!(
        scanned_batches > 1,
        "results must be published incrementally"
    );

    let observed: Vec<usize> = fs::read_to_string(media_root.join("scan-concurrency.log"))
        .expect("concurrency log")
        .lines()
        .filter_map(|line| line.trim().parse::<usize>().ok())
        .collect();
    assert!(
        observed
            .iter()
            .copied()
            .max()
            .is_some_and(|value| value >= 2)
    );
    assert!(
        observed.iter().all(|value| *value <= 8),
        "observed concurrency: {observed:?}"
    );
    engine.shutdown().expect("engine shutdown");
}

#[test]
fn cancelling_basic_scan_drops_queued_work_and_joins_running_probes() {
    let _gate = ENGINE_TEST_GATE.lock().expect("engine test gate");
    let fixture = tempfile::tempdir().expect("fixture directory");
    let media_root = fixture.path().join("media");
    fs::create_dir(&media_root).expect("media directory");
    for index in 0..FILE_COUNT {
        fs::write(
            media_root.join(format!("slow-{index:03}.mkv")),
            vec![index as u8; 8192],
        )
        .expect("media fixture");
    }
    let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
    let ffmpeg = copy_as_tool(fixture.path(), "ffmpeg");
    let engine = EngineRuntime::start(config(fixture.path(), ffmpeg, ffprobe)).expect("engine");
    let generation = engine
        .commands
        .begin_analysis_discovery(
            vec![media_root.clone()],
            BTreeSet::from([VideoExtension::Mkv]),
        )
        .expect("begin discovery");
    let mut snapshot = AnalysisSnapshot::default();
    wait_for_activity(
        &engine,
        &mut snapshot,
        generation,
        AnalysisActivity::Discovered,
    );
    engine
        .commands
        .begin_analysis_basic_scan(generation)
        .expect("begin Basic Scan");
    wait_for_probe_marker(&media_root);
    engine
        .commands
        .cancel_analysis()
        .expect("cancel Basic Scan");
    wait_for_activity(
        &engine,
        &mut snapshot,
        generation,
        AnalysisActivity::Cancelled,
    );
    engine.shutdown().expect("engine shutdown");
    let completed = fs::read_to_string(media_root.join("scan-concurrency.log"))
        .expect("concurrency log")
        .lines()
        .count();
    assert!(completed < FILE_COUNT, "queued work must be discarded");
}

#[test]
fn one_probe_failure_is_typed_scrubbed_and_does_not_abort_other_files() {
    let _gate = ENGINE_TEST_GATE.lock().expect("engine test gate");
    let fixture = tempfile::tempdir().expect("fixture directory");
    let media_root = fixture.path().join("private-media");
    fs::create_dir(&media_root).expect("media directory");
    fs::write(media_root.join("good.mkv"), vec![1_u8; 8192]).expect("good media");
    fs::write(media_root.join("reject-secret-name.mkv"), vec![2_u8; 8192]).expect("rejected media");
    let ffprobe = copy_as_tool(fixture.path(), "ffprobe");
    let ffmpeg = copy_as_tool(fixture.path(), "ffmpeg");
    let engine = EngineRuntime::start(config(fixture.path(), ffmpeg, ffprobe)).expect("engine");
    let generation = engine
        .commands
        .begin_analysis_discovery(
            vec![media_root.clone()],
            BTreeSet::from([VideoExtension::Mkv]),
        )
        .expect("begin discovery");
    let mut snapshot = AnalysisSnapshot::default();
    wait_for_activity(
        &engine,
        &mut snapshot,
        generation,
        AnalysisActivity::Discovered,
    );
    engine
        .commands
        .begin_analysis_basic_scan(generation)
        .expect("begin Basic Scan");
    wait_for_activity(&engine, &mut snapshot, generation, AnalysisActivity::Ready);
    let current = snapshot.current.as_ref().expect("Analysis generation");
    let good = current
        .rows
        .values()
        .find(|row| row.display_name.text == "good.mkv")
        .expect("good row");
    assert!(matches!(
        good.entry,
        AnalysisRowEntry::File {
            scan: AnalysisFileScan::Scanned { .. }
        }
    ));
    let rejected = current
        .rows
        .values()
        .find(|row| row.display_name.text == "reject-secret-name.mkv")
        .expect("rejected row");
    let diagnostic = match &rejected.entry {
        AnalysisRowEntry::File {
            scan:
                AnalysisFileScan::Failed {
                    failure: crfty_core::AnalysisScanFailure::Rejected { diagnostic },
                },
        } => diagnostic,
        other => panic!("unexpected rejected row state: {other:?}"),
    };
    assert!(diagnostic.text.contains("[input]"));
    assert!(!diagnostic.text.contains("private-media"));
    assert!(!diagnostic.text.contains("reject-secret-name.mkv"));
    engine.shutdown().expect("engine shutdown");
}

fn wait_for_activity(
    engine: &EngineRuntime,
    snapshot: &mut AnalysisSnapshot,
    generation: crfty_core::AnalysisGenerationId,
    activity: AnalysisActivity,
) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        let event = match engine.events.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("Analysis event stream disconnected")
            }
        };
        if let DriverEvent::Ephemeral(EphemeralDelta::Analysis(delta)) = event {
            fold_analysis(snapshot, &delta);
        }
        if snapshot
            .current
            .as_ref()
            .is_some_and(|current| current.id == generation && current.activity == activity)
        {
            return;
        }
    }
    panic!("Analysis activity did not reach {activity:?}");
}

fn wait_for_probe_marker(directory: &Path) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while Instant::now() < deadline {
        if fs::read_dir(directory).is_ok_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".scan-active-")
            })
        }) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("ffprobe fixture never started");
}

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
        ab_av1: "analysis-test".to_owned(),
        ffmpeg: "analysis-test".to_owned(),
        encoder: "analysis-test".to_owned(),
    };
    let mut profile = AnalysisProfile::production();
    profile.ab_av1_revision = revisions.ab_av1.clone();
    profile.ffmpeg_revision = revisions.ffmpeg.clone();
    profile.encoder_revision = revisions.encoder.clone();
    EngineConfig {
        journal_path: directory.join("journal.jsonl"),
        config_path: directory.join("config.json"),
        vendor_root: directory.join("vendor"),
        tools: ToolsConfig::Fixed(DiscoveredTools::Available(CurrentTools {
            media: MediaTools { ffmpeg, ffprobe },
            source: ToolSource::Explicit,
            revisions,
        })),
        execution: ExecutionSettings::production(profile, false),
    }
}
