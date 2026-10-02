use crfty_core::{
    AppState, DurableDelta, JournalEnvelope, JournalReplay, Observation, ObservationPaths,
    UnixMillis, compaction_quiescent, encode_record, encode_snapshot, observations, replay,
};

use crate::journal::DurabilityToken;

use super::{
    HistoryStore, Result, StoreError,
    database::{Database, Value},
    mapping, number, store,
};

#[derive(Debug)]
pub enum OperationalState {
    Ready(Box<JournalReplay>),
    Unavailable {
        reason: String,
        runtime_high_water: u64,
    },
}

impl HistoryStore {
    pub fn operations(&mut self) -> Result<OperationalState> {
        let transaction = self.database.transaction(false)?;
        let state = load(transaction.database)?;
        transaction.commit()?;
        Ok(state)
    }

    /// The candidate replays on request rather than owning a second mutable
    /// application state. Only COMMIT can mint the publication token.
    pub fn append_operations(
        &mut self,
        deltas: &[DurableDelta],
        record_paths: bool,
    ) -> Result<DurabilityToken> {
        self.append_operations_at(deltas, record_paths, |_| Ok(()))
    }

    #[cfg(feature = "contract-test-fixture")]
    pub fn append_operations_with_commit_hook(
        &mut self,
        deltas: &[DurableDelta],
        record_paths: bool,
        before_commit: impl FnOnce(),
    ) -> Result<DurabilityToken> {
        self.append_operations_at(deltas, record_paths, |_| {
            before_commit();
            Ok(())
        })
    }

    pub(super) fn append_operations_at(
        &mut self,
        deltas: &[DurableDelta],
        record_paths: bool,
        before_commit: impl FnOnce(&Database) -> Result<()>,
    ) -> Result<DurabilityToken> {
        let transaction = self.database.transaction(true)?;
        let OperationalState::Ready(previous) = load(transaction.database)? else {
            return Err(StoreError(
                "operational payload is unavailable; writes are stopped".to_owned(),
            ));
        };
        if deltas.is_empty() {
            transaction.commit()?;
            return Ok(DurabilityToken::new());
        }
        let record = encode_record(&JournalEnvelope {
            sequence: previous.next_sequence,
            deltas: deltas.to_vec(),
        })
        .map_err(|error| StoreError::context("encode operational batch", error))?;
        let mut bytes = encode_snapshot(
            env!("CARGO_PKG_VERSION"),
            UnixMillis(0),
            previous.next_sequence,
            &previous.state,
        )
        .map_err(|error| StoreError::context("validate operational prestate", error))?;
        bytes.extend_from_slice(&record);
        let after = replay(&bytes);
        if let Some(corruption) = after.corruption {
            return Err(StoreError::context(
                "invalid operational batch",
                corruption.reason,
            ));
        }
        if after.ignored_torn_tail {
            return Err(StoreError(
                "encoded operational batch has a torn tail".to_owned(),
            ));
        }
        transaction.database.execute(
            "INSERT INTO operational_log(seq,record) VALUES(?,?)",
            &[
                Value::Blob(number::unsigned(previous.next_sequence.0)),
                Value::Blob(record),
            ],
        )?;
        let mut terminal = observations(&after.state)
            .into_iter()
            .map(|observation| (observation.run_id, observation))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut inserted = false;
        for delta in deltas {
            if let DurableDelta::ItemFinished { run_id, .. } = delta
                && let Some(native) = terminal.remove(run_id)
            {
                let paths = after
                    .state
                    .conversion_runs
                    .get(run_id)
                    .filter(|_| record_paths)
                    .map(|run| ObservationPaths {
                        source: run.spec.input.clone(),
                        output: after
                            .state
                            .outputs
                            .get(run_id)
                            .filter(|output| output.settled_identity().is_some())
                            .map(|output| output.final_path.clone()),
                    });
                let observation = Observation::Native(Box::new(native));
                let packed = mapping::pack(&observation)?;
                store::insert(transaction.database, &observation, paths.as_ref(), packed)?;
                inserted = true;
            }
        }
        if inserted {
            store::advance_revision(transaction.database)?;
        }
        transaction.database.execute(
            "UPDATE history_meta SET runtime_high_water=? WHERE singleton=1",
            &[Value::Blob(number::unsigned(
                after.state.runtime_id_high_water,
            ))],
        )?;
        before_commit(transaction.database)?;
        transaction.commit()?;
        Ok(DurabilityToken::new())
    }

    /// The driver supplies its session state at the writer barrier. History
    /// rows and runtime-ID metadata are outside the compacted payload.
    pub fn compact_operations(
        &mut self,
        session: crfty_core::SessionState,
        at: UnixMillis,
    ) -> Result<()> {
        self.compact_operations_at(session, at, || {})
    }

    #[cfg(feature = "contract-test-fixture")]
    pub fn compact_operations_with_commit_hook(
        &mut self,
        session: crfty_core::SessionState,
        at: UnixMillis,
        before_commit: impl FnOnce(),
    ) -> Result<()> {
        self.compact_operations_at(session, at, before_commit)
    }

    fn compact_operations_at(
        &mut self,
        session: crfty_core::SessionState,
        at: UnixMillis,
        before_commit: impl FnOnce(),
    ) -> Result<()> {
        let transaction = self.database.transaction(true)?;
        let OperationalState::Ready(current) = load(transaction.database)? else {
            return Err(StoreError(
                "cannot compact unavailable operations".to_owned(),
            ));
        };
        let state = AppState {
            durable: current.state,
            session,
            ..AppState::default()
        };
        if !compaction_quiescent(&state) {
            return Err(StoreError(
                "operational compaction requires a quiescent writer".to_owned(),
            ));
        }
        let record = encode_snapshot(
            env!("CARGO_PKG_VERSION"),
            at,
            current.next_sequence,
            &state.durable,
        )
        .map_err(|error| StoreError::context("encode operational snapshot", error))?;
        transaction.database.execute("INSERT INTO operational_snapshot(singleton,record) VALUES(1,?) ON CONFLICT(singleton) DO UPDATE SET record=excluded.record",&[Value::Blob(record)])?;
        transaction
            .database
            .execute("DELETE FROM operational_log", &[])?;
        before_commit();
        transaction.commit()
    }
}

fn load(database: &Database) -> Result<OperationalState> {
    let mut bytes = Vec::new();
    for row in database.query(
        "SELECT record FROM operational_snapshot WHERE singleton=1",
        &[],
    )? {
        let [Value::Blob(record)] = row.as_slice() else {
            return Err(StoreError("invalid operational snapshot row".to_owned()));
        };
        bytes.extend_from_slice(record);
    }
    for row in database.query("SELECT record FROM operational_log ORDER BY seq ASC", &[])? {
        let [Value::Blob(record)] = row.as_slice() else {
            return Err(StoreError("invalid operational log row".to_owned()));
        };
        bytes.extend_from_slice(record);
    }
    let high_water = store::metadata(database, "runtime_high_water")?;
    let replay = replay(&bytes);
    let reason = match &replay.corruption {
        Some(corruption) => Some(corruption.reason.clone()),
        None if replay.ignored_torn_tail => {
            Some("incomplete operational record in a committed SQL row".to_owned())
        }
        None if replay.state.runtime_id_high_water != high_water => {
            Some("runtime-ID metadata disagrees with operational state".to_owned())
        }
        None => None,
    };
    Ok(match reason {
        Some(reason) => OperationalState::Unavailable {
            reason,
            runtime_high_water: high_water,
        },
        None => OperationalState::Ready(Box::new(replay)),
    })
}
