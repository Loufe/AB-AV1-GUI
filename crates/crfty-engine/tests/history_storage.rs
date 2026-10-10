#![cfg(feature = "history-storage-spike")]

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::Path,
};

use crfty_core::{
    ImportCandidate, ImportReport, NativeObservation, Observation, ObservationId, ObservationPaths,
};
use crfty_engine::history_store::{
    Candidate, Cursor, Direction, HistoryStore, ImportError, Order, OutcomeClass, PageRequest,
};
use serde::Deserialize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    description: String,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum Step {
    Record {
        observation: NativeObservation,
        paths: Option<ObservationPaths>,
    },
    AbortedRecord {
        observation: NativeObservation,
        paths: Option<ObservationPaths>,
    },
    Import {
        batch: Vec<ImportCandidate>,
        expect: ImportExpectation,
    },
    Scrub,
    Restart,
    SetPathRecording {
        enabled: bool,
    },
    Page {
        order: Order,
        direction: Direction,
        outcomes: Option<BTreeSet<OutcomeClass>>,
        search: Option<String>,
        #[serde(default)]
        continues: bool,
        limit: usize,
        expect: PageExpectation,
    },
    Pairs {
        after: Option<u64>,
        limit: usize,
        expect: PairsExpectation,
    },
    Detail {
        id: ObservationId,
        expect: DetailExpectation,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum ImportExpectation {
    Report(ImportReport),
    Rejection { index: usize, reason: String },
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageExpectation {
    ids: Vec<ObservationId>,
    matched: u64,
    more: bool,
    revision: u64,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairsExpectation {
    ids: Vec<ObservationId>,
    next: Option<u64>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
enum DetailExpectation {
    Found {
        seq: u64,
        paths: Option<ObservationPaths>,
    },
    NotFound,
}

fn run(candidate: Candidate, path: &Path) -> Result<()> {
    let scenario: Scenario = serde_json::from_slice(&fs::read(path)?)?;
    assert!(!scenario.description.trim().is_empty());
    let directory = tempfile::tempdir()?;
    let mut store = HistoryStore::open(candidate, directory.path())?;
    let mut committed = BTreeMap::new();
    let mut record_paths = true;
    let mut cursor: Option<Cursor> = None;
    for (index, step) in scenario.steps.into_iter().enumerate() {
        let at = format!("{candidate:?} {} step {index}", path.display());
        match step {
            Step::Record { observation, paths } => {
                let observation = Observation::Native(Box::new(observation));
                committed.insert(observation.id(), observation.clone());
                store.record(observation, paths.filter(|_| record_paths), false)?;
            }
            Step::AbortedRecord { observation, paths } => {
                store.record(
                    Observation::Native(Box::new(observation)),
                    paths.filter(|_| record_paths),
                    true,
                )?;
            }
            Step::Import { batch, expect } => {
                let planned = crfty_core::plan_import(batch.clone(), |id| {
                    match committed.get(&ObservationId::Translated(id.clone())) {
                        Some(Observation::Translated(observation)) => Some(observation),
                        _ => None,
                    }
                });
                match (store.import(batch, record_paths), expect) {
                    (Ok(report), ImportExpectation::Report(expected)) => {
                        assert_eq!(report, expected, "{at}");
                        let plan = planned.map_err(|_| "expected valid import plan")?;
                        for candidate in plan.inserts {
                            let observation = Observation::Translated(candidate.observation);
                            committed.insert(observation.id(), observation);
                        }
                    }
                    (
                        Err(ImportError::Rejected(rejection)),
                        ImportExpectation::Rejection { index, reason },
                    ) => {
                        assert_eq!(
                            (rejection.index, rejection.reason),
                            (index, reason.as_str()),
                            "{at}"
                        );
                    }
                    (Err(ImportError::Storage(error)), _) => return Err(Box::new(error)),
                    (actual, _) => panic!("{at}: unexpected import result {actual:?}"),
                }
            }
            Step::Scrub => store.scrub()?,
            Step::Restart => {
                drop(store);
                store = HistoryStore::open(candidate, directory.path())?;
            }
            Step::SetPathRecording { enabled } => record_paths = enabled,
            Step::Page {
                order,
                direction,
                outcomes,
                search,
                continues,
                limit,
                expect,
            } => {
                assert!(!continues || cursor.is_some(), "{at}: missing cursor");
                let page = store.page(PageRequest {
                    order,
                    direction,
                    outcomes,
                    search,
                    after: if continues { cursor.as_ref() } else { None },
                    limit,
                })?;
                assert_eq!(
                    PageExpectation {
                        ids: page.ids,
                        matched: page.matched,
                        more: page.more,
                        revision: page.revision
                    },
                    expect,
                    "{at}"
                );
                cursor = page.next;
            }
            Step::Pairs {
                after,
                limit,
                expect,
            } => {
                let page = store.pairs(after, limit)?;
                assert_eq!(
                    PairsExpectation {
                        ids: page.ids,
                        next: page.next
                    },
                    expect,
                    "{at}"
                );
            }
            Step::Detail { id, expect } => {
                let actual = match store.detail(&id)? {
                    None => DetailExpectation::NotFound,
                    Some(entry) => {
                        assert_eq!(
                            Some(&entry.observation),
                            committed.get(&id),
                            "{at}: facts changed"
                        );
                        DetailExpectation::Found {
                            seq: entry.seq,
                            paths: entry.paths,
                        }
                    }
                };
                assert_eq!(actual, expect, "{at}");
            }
        }
    }
    for (id, observation) in committed {
        let entry = store
            .detail(&id)?
            .ok_or("committed observation disappeared")?;
        assert_eq!(
            entry.observation,
            observation,
            "{candidate:?} {} final detail",
            path.display()
        );
    }
    Ok(())
}

fn scenarios(candidate: Candidate) -> Result<()> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../crfty-core/tests/fixtures/history/scenarios");
    let mut files = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    files.sort();
    assert!(!files.is_empty());
    for path in files {
        run(candidate, &path)?;
    }
    Ok(())
}

#[test]
fn rusqlite_reproduces_all_history_scenarios() -> Result<()> {
    scenarios(Candidate::Rusqlite)
}

#[test]
fn turso_reproduces_all_history_scenarios() -> Result<()> {
    scenarios(Candidate::Turso)
}

#[test]
fn data_lock_precedes_database_access() -> Result<()> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let directory = tempfile::tempdir()?;
        let held = HistoryStore::open(candidate, directory.path())?;
        assert!(HistoryStore::open(candidate, directory.path()).is_err());
        drop(held);
        HistoryStore::open(candidate, directory.path())?;
    }
    Ok(())
}
