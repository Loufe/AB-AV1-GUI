use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crfty_core::{
    HistoryBrowseFacts, ImportCandidate, ImportRejection, ImportReport, Observation, ObservationId,
    ObservationPaths, plan_import,
};

use crate::lock::DataLock;

use super::{
    Candidate, Page, PageRequest, Result, StoreError,
    database::{Database, Value},
    mapping::{self, Children, Facts},
    number,
    query::{OutcomeClass, SqlQuery, outcome_text},
    schema,
};

/// A candidate owns its connection and the exclusive data-directory lock.
/// Connection fields drop before the lock, including on failed startup.
pub struct HistoryStore {
    pub(super) database: Database,
    _lock: DataLock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub seq: u64,
    pub observation: Observation,
    pub paths: Option<ObservationPaths>,
}

#[derive(Debug)]
pub enum ImportError {
    Rejected(ImportRejection),
    Storage(StoreError),
}

impl From<StoreError> for ImportError {
    fn from(error: StoreError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Debug)]
pub struct PairPage {
    pub ids: Vec<ObservationId>,
    pub next: Option<u64>,
}

impl HistoryStore {
    pub fn open(candidate: Candidate, directory: &Path) -> Result<Self> {
        let lock = DataLock::acquire(directory)
            .map_err(|error| StoreError::context("acquire History data lock", error))?;
        let mut database = Database::open(candidate, &directory.join("history.db"))?;
        schema::initialize(&mut database)?;
        Ok(Self {
            database,
            _lock: lock,
        })
    }

    /// Storage-only recording for the workload. Operational terminal batches
    /// use the same insert inside their larger transaction.
    pub fn record(
        &mut self,
        observation: Observation,
        paths: Option<ObservationPaths>,
        abort: bool,
    ) -> Result<()> {
        let packed = mapping::pack(&observation)?;
        let transaction = self.database.transaction(true)?;
        insert(transaction.database, &observation, paths.as_ref(), packed)?;
        advance_revision(transaction.database)?;
        if !abort {
            transaction.commit()?;
        }
        Ok(())
    }

    pub fn import(
        &mut self,
        batch: Vec<ImportCandidate>,
        record_paths: bool,
    ) -> std::result::Result<ImportReport, ImportError> {
        self.import_at(batch, record_paths, || {})
    }

    #[cfg(feature = "contract-test-fixture")]
    pub fn import_with_commit_hook(
        &mut self,
        batch: Vec<ImportCandidate>,
        record_paths: bool,
        before_commit: impl FnOnce(),
    ) -> std::result::Result<ImportReport, ImportError> {
        self.import_at(batch, record_paths, before_commit)
    }

    fn import_at(
        &mut self,
        batch: Vec<ImportCandidate>,
        record_paths: bool,
        before_commit: impl FnOnce(),
    ) -> std::result::Result<ImportReport, ImportError> {
        // Validation precedes any reads or writes, including batch duplicates.
        plan_import(batch.clone(), |_| None).map_err(ImportError::Rejected)?;
        let transaction = self.database.transaction(true)?;
        let mut existing = BTreeMap::new();
        for candidate in &batch {
            let id = candidate.observation.id();
            if !existing.contains_key(&id)
                && let Some(entry) =
                    detail(transaction.database, &ObservationId::Translated(id.clone()))?
                && let Observation::Translated(observation) = entry.observation
            {
                existing.insert(id, observation);
            }
        }
        let plan = plan_import(batch, |id| existing.get(id)).map_err(ImportError::Rejected)?;
        for candidate in plan.inserts {
            let observation = Observation::Translated(candidate.observation);
            let packed = mapping::pack(&observation)?;
            let paths = candidate
                .path
                .filter(|_| record_paths)
                .map(|source| ObservationPaths {
                    source,
                    output: None,
                });
            insert(transaction.database, &observation, paths.as_ref(), packed)?;
        }
        if plan.report.inserted > 0 {
            advance_revision(transaction.database)?;
        }
        before_commit();
        transaction.commit()?;
        Ok(plan.report)
    }

    pub fn scrub(&mut self) -> Result<()> {
        let transaction = self.database.transaction(true)?;
        if transaction
            .database
            .execute("DELETE FROM observation_paths", &[])?
            > 0
        {
            advance_revision(transaction.database)?;
        }
        transaction.commit()
    }

    pub fn detail(&mut self, id: &ObservationId) -> Result<Option<Entry>> {
        let transaction = self.database.transaction(false)?;
        let entry = detail(transaction.database, id)?;
        transaction.commit()?;
        Ok(entry)
    }

    pub fn page(&mut self, request: PageRequest<'_>) -> Result<Page> {
        let query = SqlQuery::build(&request)?;
        let transaction = self.database.transaction(false)?;
        let revision = metadata(transaction.database, "revision")?;
        let count = transaction.database.query(&format!(
            "SELECT COUNT(*) FROM observations o LEFT JOIN observation_paths p ON p.owner=o.seq WHERE {}",query.filter), &query.parameters)?;
        let matched = match count.first().and_then(|row| row.first()) {
            Some(Value::Integer(count)) => u64::try_from(*count)
                .map_err(|error| StoreError::context("History count", error))?,
            _ => return Err(StoreError("missing History count".to_owned())),
        };
        let mut parameters = query.following_parameters.clone();
        parameters.push(Value::Integer(
            i64::try_from(request.limit + 1)
                .map_err(|error| StoreError::context("page limit", error))?,
        ));
        let mut rows = transaction.database.query(&format!(
            "SELECT o.identity,o.seq,{} FROM observations o LEFT JOIN observation_paths p ON p.owner=o.seq
             WHERE ({}) AND ({}) ORDER BY {} LIMIT ?", query.column,query.filter,query.following,query.ordering), &parameters)?;
        let more = rows.len() > request.limit;
        rows.truncate(request.limit);
        let mut ids = Vec::new();
        let mut next = None;
        for row in rows {
            let [Value::Text(identity), Value::Blob(seq), value] = row.as_slice() else {
                return Err(StoreError("invalid History page row".to_owned()));
            };
            ids.push(parse_identity(identity)?);
            let seq = number::read_unsigned(seq)?;
            next = more.then(|| query.cursor(value.clone(), seq));
        }
        transaction.commit()?;
        Ok(Page {
            ids,
            matched,
            more,
            revision,
            next,
        })
    }

    pub fn pairs(&mut self, after: Option<u64>, limit: usize) -> Result<PairPage> {
        if !(1..=500).contains(&limit) {
            return Err(StoreError(
                "prediction-pair limit must be in 1..=500".to_owned(),
            ));
        }
        let transaction = self.database.transaction(false)?;
        let rows = transaction.database.query(
            "SELECT identity,seq FROM observations WHERE pair_eligible=1 AND seq>? ORDER BY seq ASC LIMIT ?",
            &[Value::Blob(number::unsigned(after.unwrap_or(0))),Value::Integer(i64::try_from(limit+1).map_err(|error| StoreError::context("pair limit",error))?)])?;
        let more = rows.len() > limit;
        let mut ids = Vec::new();
        let mut next = None;
        for row in rows.into_iter().take(limit) {
            let [Value::Text(identity), Value::Blob(seq)] = row.as_slice() else {
                return Err(StoreError("invalid prediction-pair row".to_owned()));
            };
            ids.push(parse_identity(identity)?);
            next = more.then_some(number::read_unsigned(seq)?);
        }
        transaction.commit()?;
        Ok(PairPage { ids, next })
    }
}

pub(super) fn insert(
    database: &Database,
    observation: &Observation,
    paths: Option<&ObservationPaths>,
    packed: mapping::Packed,
) -> Result<()> {
    let seq = metadata(database, "recording_high_water")?
        .checked_add(1)
        .ok_or_else(|| StoreError("History recording sequence exhausted".to_owned()))?;
    let keys = HistoryBrowseFacts::of(observation);
    let mut facts = packed.facts;
    facts.extend([
        ("seq".to_owned(), Value::Blob(number::unsigned(seq))),
        (
            "identity".to_owned(),
            Value::Text(identity(&observation.id())?),
        ),
        (
            "outcome".to_owned(),
            Value::Text(outcome_text(OutcomeClass::of(observation)).to_owned()),
        ),
        (
            "dated".to_owned(),
            unsigned(observation.dated_at().map(|at| at.0)),
        ),
        ("before_size".to_owned(), unsigned(keys.before)),
        ("after_size".to_owned(), unsigned(keys.after)),
        (
            "change".to_owned(),
            keys.change
                .map_or(Value::Null, |change| Value::Blob(number::signed(change))),
        ),
        ("quality".to_owned(), unsigned(keys.quality)),
        ("took".to_owned(), unsigned(keys.took)),
        (
            "pair_eligible".to_owned(),
            Value::Integer(i64::from(observation.prediction_pair().is_some())),
        ),
    ]);
    insert_facts(database, "observations", &facts)?;
    for (list, attempts) in packed.children {
        for (ordinal, mut facts) in attempts.into_iter().enumerate() {
            facts.insert("owner".to_owned(), Value::Blob(number::unsigned(seq)));
            facts.insert("list".to_owned(), Value::Text(list.clone()));
            facts.insert(
                "ordinal".to_owned(),
                Value::Integer(
                    i64::try_from(ordinal)
                        .map_err(|error| StoreError::context("attempt ordinal", error))?,
                ),
            );
            insert_facts(database, "history_attempts", &facts)?;
        }
    }
    if let Some(paths) = paths {
        let source = paths.source.to_string_lossy();
        let file = source
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_ascii_lowercase);
        database.execute(
            "INSERT INTO observation_paths(owner,source,output,file_name) VALUES(?,?,?,?)",
            &[
                Value::Blob(number::unsigned(seq)),
                Value::Blob(encode_path(&paths.source)),
                paths
                    .output
                    .as_ref()
                    .map_or(Value::Null, |path| Value::Blob(encode_path(path))),
                file.map_or(Value::Null, Value::Text),
            ],
        )?;
    }
    database.execute(
        "UPDATE history_meta SET recording_high_water=? WHERE singleton=1",
        &[Value::Blob(number::unsigned(seq))],
    )?;
    Ok(())
}

fn insert_facts(database: &Database, table: &str, facts: &Facts) -> Result<()> {
    let columns = facts.keys().cloned().collect::<Vec<_>>().join(",");
    let placeholders = vec!["?"; facts.len()].join(",");
    database.execute(
        &format!("INSERT INTO {table}({columns}) VALUES({placeholders})"),
        &facts.values().cloned().collect::<Vec<_>>(),
    )?;
    Ok(())
}

pub(super) fn metadata(database: &Database, column: &str) -> Result<u64> {
    let rows = database.query(
        &format!("SELECT {column} FROM history_meta WHERE singleton=1"),
        &[],
    )?;
    match rows.first().and_then(|row| row.first()) {
        Some(Value::Blob(bytes)) => number::read_unsigned(bytes),
        _ => Err(StoreError(format!("missing History metadata {column}"))),
    }
}

pub(super) fn advance_revision(database: &Database) -> Result<()> {
    let next = metadata(database, "revision")?
        .checked_add(1)
        .ok_or_else(|| StoreError("History revision exhausted".to_owned()))?;
    database.execute(
        "UPDATE history_meta SET revision=? WHERE singleton=1",
        &[Value::Blob(number::unsigned(next))],
    )?;
    Ok(())
}

fn detail(database: &Database, id: &ObservationId) -> Result<Option<Entry>> {
    let columns: Vec<_> = mapping::columns(false).into_keys().collect();
    let rows = database.query(
        &format!(
            "SELECT seq,{} FROM observations WHERE identity=?",
            columns.join(",")
        ),
        &[Value::Text(identity(id)?)],
    )?;
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let mut row = row.into_iter();
    let Some(Value::Blob(seq)) = row.next() else {
        return Err(StoreError("invalid History detail sequence".to_owned()));
    };
    let seq = number::read_unsigned(&seq)?;
    let facts = columns.into_iter().zip(row).collect();
    let columns: Vec<_> = mapping::columns(true).into_keys().collect();
    let rows = database.query(
        &format!(
            "SELECT list,ordinal,{} FROM history_attempts WHERE owner=? ORDER BY list,ordinal",
            columns.join(",")
        ),
        &[Value::Blob(number::unsigned(seq))],
    )?;
    let mut children = Children::new();
    for row in rows {
        let mut row = row.into_iter();
        let (Some(Value::Text(list)), Some(Value::Integer(ordinal))) = (row.next(), row.next())
        else {
            return Err(StoreError("invalid History attempt position".to_owned()));
        };
        let entries = children.entry(list).or_default();
        if usize::try_from(ordinal).ok() != Some(entries.len()) {
            return Err(StoreError("History attempt list has a gap".to_owned()));
        }
        entries.push(columns.iter().cloned().zip(row).collect());
    }
    let observation = mapping::unpack(&facts, &children)?;
    if observation.id() != *id {
        return Err(StoreError(
            "History identity disagrees with its facts".to_owned(),
        ));
    }
    let rows = database.query(
        "SELECT source,output FROM observation_paths WHERE owner=?",
        &[Value::Blob(number::unsigned(seq))],
    )?;
    let paths = rows
        .first()
        .map(|row| match row.as_slice() {
            [Value::Blob(source), output] => Ok(ObservationPaths {
                source: decode_path(source)?,
                output: match output {
                    Value::Null => None,
                    Value::Blob(bytes) => Some(decode_path(bytes)?),
                    _ => return Err(StoreError("invalid output path".to_owned())),
                },
            }),
            _ => Err(StoreError("invalid History path row".to_owned())),
        })
        .transpose()?;
    Ok(Some(Entry {
        seq,
        observation,
        paths,
    }))
}

fn unsigned(value: Option<u64>) -> Value {
    value.map_or(Value::Null, |value| Value::Blob(number::unsigned(value)))
}

fn identity(id: &ObservationId) -> Result<String> {
    serde_json::to_string(id).map_err(|error| StoreError::context("encode History identity", error))
}

fn parse_identity(text: &str) -> Result<ObservationId> {
    serde_json::from_str(text)
        .map_err(|error| StoreError::context("decode History identity", error))
}

#[cfg(unix)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let mut bytes = vec![1];
    bytes.extend_from_slice(path.as_os_str().as_bytes());
    bytes
}

#[cfg(unix)]
fn decode_path(bytes: &[u8]) -> Result<PathBuf> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let Some((1, bytes)) = bytes.split_first() else {
        return Err(StoreError(
            "History path has an unsupported platform encoding".to_owned(),
        ));
    };
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(windows)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    let mut bytes = vec![2];
    bytes.extend(path.as_os_str().encode_wide().flat_map(u16::to_le_bytes));
    bytes
}

#[cfg(windows)]
fn decode_path(bytes: &[u8]) -> Result<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let Some((2, bytes)) = bytes.split_first() else {
        return Err(StoreError(
            "History path has an unsupported platform encoding".to_owned(),
        ));
    };
    if !bytes.len().is_multiple_of(2) {
        return Err(StoreError("invalid Windows path encoding".to_owned()));
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| <[u8; 2]>::try_from(pair).map(u16::from_le_bytes))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| StoreError::context("Windows path encoding", error))?;
    Ok(PathBuf::from(OsString::from_wide(&units)))
}
