//! Executable History scenarios (`docs/design/history-storage-workload.md`).
//! Each JSON file under `tests/fixtures/history/scenarios/` is a
//! hand-maintained sequence of steps against an empty History, and every
//! query step carries its expected result.
//!
//! The in-memory store and browse evaluator below are the self-check of
//! those hand-maintained expectations: they prove the expected results are
//! consistent with the browse semantics before any storage engine exists.
//! They are test code only. The selected storage runs the same scenarios
//! and replaces this evaluator.
//!
//! Recording sequences, revisions, and page cursors are storage-assigned, so
//! they exist here and not in production code. Sequences start at 1 and
//! increase by one per inserted observation, in file order within an
//! import. The revision starts at 0 and increases once per committed
//! record, once per import that inserts, and once per scrub that deletes a
//! path row. A page cursor is opaque to the scenarios: it is the order-key
//! value and recording sequence of the page's last row, so a continued page
//! stays deterministic even when that row's value or filter membership has
//! since changed. A `Page` step with `continues` resumes from the cursor the
//! previous `Page` step returned.
#![forbid(unsafe_code)]

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use crfty_core::{
    ImportCandidate, ImportReport, NativeObservation, Observation, ObservationId, ObservationPaths,
    ObservedOutcome, TranslatedId, TranslatedObservation, TranslatedOutcome, plan_import,
};
use serde::{Deserialize, Serialize};

const SCENARIOS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/history/scenarios"
);
const MAX_LIMIT: usize = 200;
const MAX_PAIRS_LIMIT: usize = 500;
const CHANGE_SCALE: i128 = 10_000;

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
    matched: usize,
    more: bool,
    revision: u64,
}

/// Prediction-pair evidence in recording order; `next` is the sequence to
/// continue after, absent when no more rows follow.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum Order {
    Date,
    File,
    Before,
    After,
    Change,
    Quality,
    Took,
    Recorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum Direction {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
enum OutcomeClass {
    Converted,
    Remuxed,
    NotWorthwhile,
    Analyzed,
    Failed,
    Stopped,
    Incomplete,
}

const DEFAULT_OUTCOMES: [OutcomeClass; 4] = [
    OutcomeClass::Converted,
    OutcomeClass::Remuxed,
    OutcomeClass::NotWorthwhile,
    OutcomeClass::Analyzed,
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SortValue {
    Number(i128),
    Text(String),
}

/// One row's position under an order: its value, then its sequence.
#[derive(Debug, Clone)]
struct Position {
    value: Option<SortValue>,
    seq: u64,
}

struct Cursor {
    order: Order,
    direction: Direction,
    after: Position,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    seq: u64,
    observation: Observation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Store {
    revision: u64,
    /// In recording order; observations are never deleted.
    entries: Vec<Entry>,
    /// The separate path relation, keyed by recording sequence.
    paths: BTreeMap<u64, ObservationPaths>,
}

impl Store {
    fn insert(&mut self, observation: Observation, paths: Option<ObservationPaths>) {
        let id = observation.id();
        assert!(self.find(&id).is_none(), "{id:?} is already recorded");
        let seq = self.entries.last().map_or(1, |last| last.seq + 1);
        self.entries.push(Entry { seq, observation });
        if let Some(paths) = paths {
            self.paths.insert(seq, paths);
        }
    }

    fn find(&self, id: &ObservationId) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.observation.id() == *id)
    }

    fn translated(&self, id: &TranslatedId) -> Option<&TranslatedObservation> {
        self.entries
            .iter()
            .find_map(|entry| match &entry.observation {
                Observation::Translated(translated) if translated.id() == *id => Some(translated),
                Observation::Native(_) | Observation::Translated(_) => None,
            })
    }

    fn file_name(&self, seq: u64) -> Option<String> {
        let source = self.paths.get(&seq)?.source.to_string_lossy().into_owned();
        source
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_ascii_lowercase)
    }

    fn position(&self, entry: &Entry, order: Order) -> Position {
        let number = |value: Option<u64>| value.map(|value| SortValue::Number(i128::from(value)));
        let value = match order {
            Order::Date => number(entry.observation.dated_at().map(|at| at.0)),
            Order::File => self.file_name(entry.seq).map(SortValue::Text),
            Order::Before => number(before(&entry.observation)),
            Order::After => number(after(&entry.observation)),
            Order::Change => measured_pair(&entry.observation)
                .filter(|(before, _)| *before > 0)
                .map(|(before, after)| {
                    let (before, after) = (i128::from(before), i128::from(after));
                    SortValue::Number((before - after) * CHANGE_SCALE / before)
                }),
            Order::Quality => number(quality(&entry.observation)),
            Order::Took => number(took(&entry.observation)),
            Order::Recorded => number(Some(entry.seq)),
        };
        Position {
            value,
            seq: entry.seq,
        }
    }

    fn matches(
        &self,
        entry: &Entry,
        outcomes: &BTreeSet<OutcomeClass>,
        search: Option<&str>,
    ) -> bool {
        outcomes.contains(&class(&entry.observation))
            && search.is_none_or(|needle| {
                self.file_name(entry.seq)
                    .is_some_and(|name| name.contains(&needle.to_ascii_lowercase()))
            })
    }
}

/// Absent values sort last in both directions, and every tie breaks by
/// recording sequence descending.
fn compare(left: &Position, right: &Position, direction: Direction) -> Ordering {
    let by_value = match (&left.value, &right.value) {
        (Some(left), Some(right)) => match direction {
            Direction::Ascending => left.cmp(right),
            Direction::Descending => right.cmp(left),
        },
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    by_value.then(right.seq.cmp(&left.seq))
}

/// Both sizes of a produced output as one measured pair: native measured
/// sizes, or a translated record's source and output size when it held
/// both.
fn measured_pair(observation: &Observation) -> Option<(u64, u64)> {
    match observation {
        Observation::Native(native) => match &native.outcome {
            ObservedOutcome::Converted { encode, .. } => encode.measurement.sizes(),
            ObservedOutcome::Remuxed { remux } => remux.measurement.sizes(),
            ObservedOutcome::Analyzed { .. }
            | ObservedOutcome::NotWorthwhile { .. }
            | ObservedOutcome::Failed { .. }
            | ObservedOutcome::Stopped
            | ObservedOutcome::Incomplete => None,
        }
        .map(|sizes| (sizes.input, sizes.output)),
        Observation::Translated(translated) => match &translated.outcome {
            TranslatedOutcome::Converted { output_size, .. } => {
                translated.source.size_bytes.zip(*output_size)
            }
            TranslatedOutcome::Analyzed { .. } | TranslatedOutcome::NotWorthwhile { .. } => None,
        },
    }
}

fn before(observation: &Observation) -> Option<u64> {
    let inspected = match observation {
        Observation::Native(native) => native.source.as_ref().map(|source| source.size_bytes),
        Observation::Translated(translated) => translated.source.size_bytes,
    };
    measured_pair(observation)
        .map(|(before, _)| before)
        .or(inspected)
}

fn after(observation: &Observation) -> Option<u64> {
    match observation {
        Observation::Native(_) => measured_pair(observation).map(|(_, after)| after),
        Observation::Translated(translated) => match &translated.outcome {
            TranslatedOutcome::Converted { output_size, .. } => *output_size,
            TranslatedOutcome::Analyzed { .. } | TranslatedOutcome::NotWorthwhile { .. } => None,
        },
    }
}

fn quality(observation: &Observation) -> Option<u64> {
    match observation {
        Observation::Native(native) => match &native.outcome {
            ObservedOutcome::Analyzed { search } | ObservedOutcome::Converted { search, .. } => {
                Some(search.analysis.measurement.crf.0)
            }
            ObservedOutcome::Remuxed { .. }
            | ObservedOutcome::NotWorthwhile { .. }
            | ObservedOutcome::Failed { .. }
            | ObservedOutcome::Stopped
            | ObservedOutcome::Incomplete => None,
        },
        Observation::Translated(translated) => match &translated.outcome {
            TranslatedOutcome::Analyzed { quality }
            | TranslatedOutcome::Converted { quality, .. } => quality.crf.map(|crf| crf.0),
            TranslatedOutcome::NotWorthwhile { .. } => None,
        },
    }
    .map(u64::from)
}

fn took(observation: &Observation) -> Option<u64> {
    match observation {
        Observation::Native(native) => match &native.outcome {
            ObservedOutcome::Converted { encode, .. } => encode.duration,
            ObservedOutcome::Analyzed { .. }
            | ObservedOutcome::Remuxed { .. }
            | ObservedOutcome::NotWorthwhile { .. }
            | ObservedOutcome::Failed { .. }
            | ObservedOutcome::Stopped
            | ObservedOutcome::Incomplete => None,
        },
        Observation::Translated(translated) => match &translated.outcome {
            TranslatedOutcome::Converted {
                encode_duration, ..
            } => *encode_duration,
            TranslatedOutcome::Analyzed { .. } | TranslatedOutcome::NotWorthwhile { .. } => None,
        },
    }
    .map(|duration| duration.0)
}

fn class(observation: &Observation) -> OutcomeClass {
    match observation {
        Observation::Native(native) => match &native.outcome {
            ObservedOutcome::Analyzed { .. } => OutcomeClass::Analyzed,
            ObservedOutcome::Converted { .. } => OutcomeClass::Converted,
            ObservedOutcome::Remuxed { .. } => OutcomeClass::Remuxed,
            ObservedOutcome::NotWorthwhile { .. } => OutcomeClass::NotWorthwhile,
            ObservedOutcome::Failed { .. } => OutcomeClass::Failed,
            ObservedOutcome::Stopped => OutcomeClass::Stopped,
            ObservedOutcome::Incomplete => OutcomeClass::Incomplete,
        },
        Observation::Translated(translated) => match &translated.outcome {
            TranslatedOutcome::Analyzed { .. } => OutcomeClass::Analyzed,
            TranslatedOutcome::Converted { .. } => OutcomeClass::Converted,
            TranslatedOutcome::NotWorthwhile { .. } => OutcomeClass::NotWorthwhile,
        },
    }
}

struct PageQuery<'a> {
    order: Order,
    direction: Direction,
    outcomes: BTreeSet<OutcomeClass>,
    search: Option<&'a str>,
    after: Option<&'a Position>,
    limit: usize,
}

/// One page and the cursor to continue after it, absent when no more rows
/// follow.
fn page(store: &Store, query: &PageQuery<'_>) -> (PageExpectation, Option<Position>) {
    assert!(
        (1..=MAX_LIMIT).contains(&query.limit),
        "limit {} is outside 1..={MAX_LIMIT}",
        query.limit
    );
    let matching: Vec<(&Entry, Position)> = store
        .entries
        .iter()
        .filter(|entry| store.matches(entry, &query.outcomes, query.search))
        .map(|entry| (entry, store.position(entry, query.order)))
        .collect();
    let matched = matching.len();
    let mut following: Vec<(&Entry, Position)> = matching
        .into_iter()
        .filter(|(_, position)| {
            query
                .after
                .is_none_or(|after| compare(position, after, query.direction) == Ordering::Greater)
        })
        .collect();
    following.sort_by(|(_, left), (_, right)| compare(left, right, query.direction));
    let more = following.len() > query.limit;
    following.truncate(query.limit);
    let next = following
        .last()
        .filter(|_| more)
        .map(|(_, position)| position.clone());
    let expectation = PageExpectation {
        ids: following
            .iter()
            .map(|(entry, _)| entry.observation.id())
            .collect(),
        matched,
        more,
        revision: store.revision,
    };
    (expectation, next)
}

fn pairs(store: &Store, after: Option<u64>, limit: usize) -> PairsExpectation {
    assert!(
        (1..=MAX_PAIRS_LIMIT).contains(&limit),
        "limit {limit} is outside 1..={MAX_PAIRS_LIMIT}"
    );
    let eligible: Vec<&Entry> = store
        .entries
        .iter()
        .filter(|entry| after.is_none_or(|after| entry.seq > after))
        .filter(|entry| entry.observation.prediction_pair().is_some())
        .collect();
    let more = eligible.len() > limit;
    let taken: Vec<&Entry> = eligible.into_iter().take(limit).collect();
    PairsExpectation {
        ids: taken.iter().map(|entry| entry.observation.id()).collect(),
        next: taken.last().filter(|_| more).map(|entry| entry.seq),
    }
}

fn detail(store: &Store, id: &ObservationId) -> DetailExpectation {
    store.find(id).map_or(DetailExpectation::NotFound, |entry| {
        assert_eq!(entry.observation.id(), *id);
        DetailExpectation::Found {
            seq: entry.seq,
            paths: store.paths.get(&entry.seq).cloned(),
        }
    })
}

fn run(name: &str, scenario: Scenario) {
    assert!(
        !scenario.description.trim().is_empty(),
        "{name}: a scenario states what it covers"
    );
    let mut store = Store::default();
    let mut cursor: Option<Cursor> = None;
    for (index, step) in scenario.steps.into_iter().enumerate() {
        let at = format!("{name} step {index}");
        match step {
            Step::Record { observation, paths } => {
                assert_eq!(observation.validate(), Ok(()), "{at}");
                store.insert(Observation::Native(Box::new(observation)), paths);
                store.revision += 1;
            }
            Step::AbortedRecord { observation, paths } => {
                assert_eq!(observation.validate(), Ok(()), "{at}");
                let mut rolled_back = store.clone();
                rolled_back.insert(Observation::Native(Box::new(observation)), paths);
            }
            Step::Import { batch, expect } => {
                let planned = plan_import(batch, |id| store.translated(id));
                match (planned, expect) {
                    (Ok(plan), ImportExpectation::Report(expected)) => {
                        assert_eq!(plan.report, expected, "{at}");
                        if !plan.inserts.is_empty() {
                            for candidate in plan.inserts {
                                let paths = candidate.path.map(|source| ObservationPaths {
                                    source,
                                    output: None,
                                });
                                store.insert(Observation::Translated(candidate.observation), paths);
                            }
                            store.revision += 1;
                        }
                    }
                    (Err(rejection), ImportExpectation::Rejection { index, reason }) => {
                        assert_eq!(
                            (rejection.index, rejection.reason),
                            (index, reason.as_str()),
                            "{at}"
                        );
                    }
                    (Ok(plan), ImportExpectation::Rejection { .. }) => {
                        panic!("{at}: expected a rejection, planned {:?}", plan.report)
                    }
                    (Err(rejection), ImportExpectation::Report(_)) => {
                        panic!("{at}: expected a report, rejected {rejection:?}")
                    }
                }
            }
            Step::Scrub => {
                if !store.paths.is_empty() {
                    store.paths.clear();
                    store.revision += 1;
                }
            }
            Step::Restart => {
                let encoded = serde_json::to_string(&store)
                    .unwrap_or_else(|error| panic!("{at}: close: {error}"));
                let reopened: Store = serde_json::from_str(&encoded)
                    .unwrap_or_else(|error| panic!("{at}: reopen: {error}"));
                assert_eq!(reopened, store, "{at}: restart changed the store");
                store = reopened;
            }
            Step::Page {
                order,
                direction,
                outcomes,
                search,
                continues,
                limit,
                expect,
            } => {
                let after = if continues {
                    let Some(previous) = &cursor else {
                        panic!("{at}: no previous page left a cursor to continue");
                    };
                    assert_eq!(
                        (previous.order, previous.direction),
                        (order, direction),
                        "{at}: a cursor continues only its own order"
                    );
                    Some(&previous.after)
                } else {
                    None
                };
                let query = PageQuery {
                    order,
                    direction,
                    outcomes: outcomes.unwrap_or_else(|| DEFAULT_OUTCOMES.into_iter().collect()),
                    search: search.as_deref().filter(|search| !search.is_empty()),
                    after,
                    limit,
                };
                let (found, next) = page(&store, &query);
                assert_eq!(found, expect, "{at}");
                cursor = next.map(|after| Cursor {
                    order,
                    direction,
                    after,
                });
            }
            Step::Pairs {
                after,
                limit,
                expect,
            } => {
                assert_eq!(pairs(&store, after, limit), expect, "{at}");
            }
            Step::Detail { id, expect } => {
                assert_eq!(detail(&store, &id), expect, "{at}");
            }
        }
    }
}

#[test]
fn every_scenario_reproduces_its_expected_results() {
    let directory = Path::new(SCENARIOS);
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("read entry in {}: {error}", directory.display()))
                .path()
        })
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no scenarios under {}",
        directory.display()
    );
    for path in files {
        let name = path.display().to_string();
        let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {name}: {error}"));
        let scenario: Scenario = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{name}: deserialize: {error}"));
        run(&name, scenario);
    }
}
