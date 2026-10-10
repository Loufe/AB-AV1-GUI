#![forbid(unsafe_code)]

use std::{collections::BTreeSet, fs, path::Path, time::Duration};

use crfty_core::{
    AnalysisActivity, AnalysisDelta, AnalysisQueueRequest, AnalysisQueueSelection,
    AnalysisRowEntry, AnalysisSnapshot, DefaultOutputMode, DurableDelta, EphemeralDelta,
    ExecutionSettings, LocatedTool, LocatedTools, Operation, OutputTarget, QueueAddRequest,
    QueueCommand, Reply, Settings, SettingsCommand, ToolRevisions, ToolSource, VideoExtension,
    fold_analysis,
};
use crfty_engine::{
    coordinator::{EngineConfig, EngineRuntime, FixedTools, ToolsConfig},
    driver::DriverEvent,
};

fn config(root: &Path) -> Result<EngineConfig, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let tool = LocatedTool {
        path: executable,
        source: ToolSource::Environment,
    };
    Ok(EngineConfig {
        journal_path: root.join("journal.jsonl"),
        config_path: root.join("config.json"),
        execution: ExecutionSettings::default(),
        tools: ToolsConfig::Fixed(FixedTools {
            tools: LocatedTools {
                ffmpeg: tool.clone(),
                ffprobe: tool,
            },
            revisions: ToolRevisions {
                ab_av1: "fixture".to_owned(),
                ffmpeg: "fixture".to_owned(),
                encoder: "fixture".to_owned(),
            },
            hardware_decoders: BTreeSet::new(),
        }),
    })
}

fn discover(
    engine: &EngineRuntime,
    root: &Path,
) -> Result<AnalysisSnapshot, Box<dyn std::error::Error>> {
    let generation = engine.commands.begin_analysis_discovery(
        vec![root.to_path_buf()],
        BTreeSet::from([VideoExtension::Mkv]),
    )?;
    let mut snapshot = AnalysisSnapshot::default();
    loop {
        if let DriverEvent::Ephemeral(EphemeralDelta::Analysis(delta)) =
            engine.events.recv_timeout(Duration::from_secs(5))?
        {
            fold_analysis(&mut snapshot, &delta);
            if matches!(delta, AnalysisDelta::ActivityChanged { generation: id, activity: AnalysisActivity::Discovered } if id == generation)
            {
                return Ok(snapshot);
            }
        }
    }
}

#[test]
fn captured_native_paths_committed_settings_and_concurrent_ids_survive_restart()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let media = fixture.path().join("media");
    fs::create_dir(&media)?;
    for id in 1..=4 {
        fs::write(media.join(format!("movie-{id}.mkv")), b"fixture")?;
    }
    let engine = EngineRuntime::start(config(fixture.path())?)?;
    let snapshot = discover(&engine, &media)?;
    let generation = snapshot.current.ok_or("missing generation")?;
    let files: Vec<_> = generation
        .rows
        .values()
        .filter(|row| matches!(row.entry, AnalysisRowEntry::File { .. }))
        .map(|row| row.id)
        .collect();
    assert_eq!(files.len(), 4);
    let mut settings = Settings::default();
    settings.output.default_mode = DefaultOutputMode::Suffix;
    settings.output.suffix = "_current".to_owned();
    assert_eq!(
        engine
            .commands
            .submit_settings(SettingsCommand::Set { settings })?,
        Reply::Accepted
    );
    fs::remove_dir_all(&media)?;
    let mut workers = Vec::new();
    for id in files {
        let commands = engine.commands.clone();
        workers.push(std::thread::spawn(move || {
            commands.enqueue_analysis(AnalysisQueueRequest {
                generation: generation.id,
                selection: AnalysisQueueSelection::Selected { row_ids: vec![id] },
                operation: Operation::Analyze,
            })
        }));
    }
    let commands = engine.commands.clone();
    let direct = std::thread::spawn(move || {
        commands.submit_queue(QueueCommand::AddMany {
            requests: vec![QueueAddRequest {
                input: "direct-file.mkv".into(),
                path_hash: None,
                identity: None,
                timestamp_reliability: crfty_core::TimestampReliability::Unknown,
                operation: Operation::Convert,
                intent: crfty_core::AnalysisIntent::ReuseIfFresh,
                output_target: OutputTarget::Replace,
                overwrite: crfty_core::OverwriteDecision::FollowSettings,
            }],
        })
    });
    for worker in workers {
        let summary = worker.join().map_err(|_| "enqueue worker panicked")??;
        assert_eq!(summary.added, 1);
        assert!(summary.excluded.is_empty());
    }
    assert_eq!(
        direct
            .join()
            .map_err(|_| "direct enqueue worker panicked")??,
        Reply::Accepted
    );
    let mut added = Vec::new();
    while added.len() < 5 {
        if let DriverEvent::Durable(DurableDelta::QueueAdded { item }) =
            engine.events.recv_timeout(Duration::from_secs(5))?
        {
            if item.operation == Operation::Analyze {
                assert!(item.input.starts_with(&media));
                assert_eq!(
                    item.output_target,
                    OutputTarget::Suffix {
                        suffix: "_current".to_owned()
                    }
                );
            }
            added.push(item.id.0);
        }
    }
    assert_eq!(added, vec![1, 2, 3, 4, 5]);
    fs::create_dir(&media)?;
    fs::write(media.join("new.mkv"), b"new")?;
    discover(&engine, &media)?;
    let stale = engine.commands.enqueue_analysis(AnalysisQueueRequest {
        generation: generation.id,
        selection: AnalysisQueueSelection::AllFiles,
        operation: Operation::Convert,
    });
    assert!(
        matches!(stale, Err(crfty_engine::analysis::AnalysisError::Rejected(reason)) if reason.contains("stale"))
    );
    assert_eq!(
        engine.commands.submit_queue(QueueCommand::Clear)?,
        Reply::Accepted
    );
    engine.shutdown().map_err(|_| "engine shutdown failed")?;

    let engine = EngineRuntime::start(config(fixture.path())?)?;
    let DriverEvent::Snapshot(restored) = engine.events.recv_timeout(Duration::from_secs(5))?
    else {
        return Err("startup snapshot is missing".into());
    };
    assert!(restored.durable.queue.is_empty());
    assert_eq!(restored.durable.queue_item_id_high_water, 5);
    let snapshot = discover(&engine, &media)?;
    let generation = snapshot.current.ok_or("missing restarted generation")?;
    let summary = engine.commands.enqueue_analysis(AnalysisQueueRequest {
        generation: generation.id,
        selection: AnalysisQueueSelection::AllFiles,
        operation: Operation::Convert,
    })?;
    assert_eq!(summary.added, 1);
    loop {
        if let DriverEvent::Durable(DurableDelta::QueueAdded { item }) =
            engine.events.recv_timeout(Duration::from_secs(5))?
        {
            assert_eq!(item.id.0, 6);
            break;
        }
    }
    engine
        .shutdown()
        .map_err(|_| "restarted engine shutdown failed")?;
    Ok(())
}
