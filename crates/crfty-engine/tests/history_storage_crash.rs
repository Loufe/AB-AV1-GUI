#![cfg(all(feature = "history-storage-spike", feature = "contract-test-fixture"))]

use std::{
    error::Error,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

use crfty_core::{ImportCandidate, ItemOutcome, ObservationId, RunId};
use crfty_engine::history_store::{
    Candidate, Direction, HistoryStore, OperationalState, Order, PageRequest,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn crash(
    candidate: Candidate,
    action: &str,
    cut: &str,
) -> Result<(tempfile::TempDir, HistoryStore)> {
    let directory = tempfile::tempdir()?;
    let output = Command::new(env!("CARGO_BIN_EXE_crfty-history-fixture"))
        .arg(match candidate {
            Candidate::Rusqlite => "rusqlite",
            Candidate::Turso => "turso",
        })
        .arg(directory.path())
        .args([action, cut])
        .stderr(Stdio::piped())
        .output()?;
    assert!(
        !output.status.success(),
        "crash fixture unexpectedly returned success"
    );
    assert_eq!(
        String::from_utf8(output.stdout)?.trim(),
        cut,
        "{candidate:?} {action} {cut}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let store = HistoryStore::open(candidate, directory.path())?;
    Ok((directory, store))
}

fn page(store: &mut HistoryStore) -> Result<crfty_engine::history_store::Page> {
    Ok(store.page(PageRequest {
        order: Order::Recorded,
        direction: Direction::Ascending,
        outcomes: Some(
            [
                crfty_engine::history_store::OutcomeClass::Stopped,
                crfty_engine::history_store::OutcomeClass::Converted,
            ]
            .into_iter()
            .collect(),
        ),
        search: None,
        after: None,
        limit: 100,
    })?)
}

#[test]
fn process_abort_keeps_terminal_and_history_all_or_none() -> Result<()> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        for cut in ["before", "after"] {
            let (_directory, mut store) = crash(candidate, "terminal", cut)?;
            let OperationalState::Ready(replay) = store.operations()? else {
                return Err("operations unavailable after abort".into());
            };
            let terminal = replay
                .state
                .conversion_runs
                .get(&RunId(2))
                .and_then(|run| run.outcome.as_ref());
            let observation = store.detail(&ObservationId::Native(RunId(2)))?;
            assert_eq!(
                terminal,
                if cut == "after" {
                    Some(&ItemOutcome::Stopped)
                } else {
                    None
                }
            );
            assert_eq!(observation.is_some(), cut == "after");
            assert_eq!(replay.state.runtime_id_high_water, 2);
            let page = page(&mut store)?;
            assert_eq!(page.revision, u64::from(cut == "after"));
            assert_eq!(page.matched, u64::from(cut == "after"));
            if let Some(entry) = observation {
                assert_eq!(entry.seq, 1);
                assert!(entry.paths.is_some());
            }
        }
    }
    Ok(())
}

#[test]
fn process_abort_allows_idempotent_retry_of_uncertain_import() -> Result<()> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        for cut in ["before", "after"] {
            let (_directory, mut store) = crash(candidate, "import", cut)?;
            assert_eq!(page(&mut store)?.matched, 2 * u64::from(cut == "after"));
            let observation: crfty_core::TranslatedObservation = serde_json::from_str(
                include_str!(
                    "../../crfty-core/tests/fixtures/observations/translated/valid/converted_without_sizes.json"
                ),
            )?;
            let mut second = observation.clone();
            second.record_key = crfty_core::RecordKey::new("000000000000abcd".to_owned())?;
            let report = store
                .import(
                    vec![
                        ImportCandidate {
                            observation,
                            path: Some("synthetic-retry.mkv".into()),
                        },
                        ImportCandidate {
                            observation: second,
                            path: Some("synthetic-retry.mkv".into()),
                        },
                    ],
                    true,
                )
                .map_err(|error| format!("retry: {error:?}"))?;
            assert_eq!(report.inserted, 2 * usize::from(cut == "before"));
            assert_eq!(report.duplicates, 2 * usize::from(cut == "after"));
            let page = page(&mut store)?;
            assert_eq!(page.matched, 2);
            assert_eq!(page.revision, 1);
            for (index, id) in page.ids.iter().enumerate() {
                let entry = store.detail(id)?.ok_or("missing import detail")?;
                assert_eq!(entry.seq, u64::try_from(index)? + 1);
                assert_eq!(
                    entry.paths.map(|paths| paths.source),
                    Some(if cut == "before" {
                        "synthetic-retry.mkv".into()
                    } else {
                        "synthetic-import.mkv".into()
                    })
                );
            }
        }
    }
    Ok(())
}

#[test]
fn process_abort_during_compaction_preserves_operations_and_history() -> Result<()> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        for cut in ["before", "after"] {
            let (_directory, mut store) = crash(candidate, "compact", cut)?;
            let OperationalState::Ready(replay) = store.operations()? else {
                return Err("operations unavailable after compaction abort".into());
            };
            assert_eq!(replay.state.runtime_id_high_water, 2);
            assert_eq!(
                replay
                    .state
                    .conversion_runs
                    .get(&RunId(2))
                    .and_then(|run| run.outcome.as_ref()),
                Some(&ItemOutcome::Stopped)
            );
            assert_eq!(replay.next_sequence.0, 2);
            let page = page(&mut store)?;
            assert_eq!((page.matched, page.revision), (1, 1));
            assert_eq!(
                store
                    .detail(&ObservationId::Native(RunId(2)))?
                    .map(|entry| entry.seq),
                Some(1)
            );
        }
    }
    Ok(())
}

#[test]
fn another_process_cannot_open_the_store_and_a_crash_releases_the_lock() -> Result<()> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let directory = tempfile::tempdir()?;
        let mut child = Command::new(env!("CARGO_BIN_EXE_crfty-history-fixture"))
            .arg(match candidate {
                Candidate::Rusqlite => "rusqlite",
                Candidate::Turso => "turso",
            })
            .arg(directory.path())
            .args(["lock", "held"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().ok_or("missing lock fixture output")?;
        let mut output = BufReader::new(stdout);
        let mut marker = String::new();
        output.read_line(&mut marker)?;
        assert_eq!(marker.trim(), "locked");
        let refused = HistoryStore::open(candidate, directory.path()).is_err();
        child
            .stdin
            .take()
            .ok_or("missing lock fixture input")?
            .write_all(b"abort\n")?;
        assert!(!child.wait()?.success());
        assert!(refused, "second process acquired the data lock");
        HistoryStore::open(candidate, directory.path())?;
    }
    Ok(())
}
