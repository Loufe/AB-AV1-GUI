use std::path::PathBuf;

use crfty_core::{
    ClaimId, DurableDelta, Observation, ObservationId, QueueItemId, RunId, SessionState, UnixMillis,
};

#[path = "../../tests/support/history_facts.rs"]
mod facts;
use facts::{prepared, stopped};

use super::{
    Candidate, Direction, HistoryStore, OperationalState, Order, PageRequest, Result, StoreError,
    database::Value, mapping, number, store,
};

fn history(
    candidate: Candidate,
) -> std::result::Result<(tempfile::TempDir, HistoryStore), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = HistoryStore::open(candidate, directory.path())?;
    Ok((directory, store))
}

fn ready(store: &mut HistoryStore) -> Result<crfty_core::JournalReplay> {
    match store.operations()? {
        OperationalState::Ready(replay) => Ok(*replay),
        OperationalState::Unavailable { reason, .. } => Err(StoreError(reason)),
    }
}

#[test]
fn terminal_batch_and_history_are_atomic_and_survive_compaction()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let (directory, mut store) = history(candidate)?;
        store.append_operations(&prepared(), true)?;
        assert!(
            store
                .compact_operations(SessionState::Idle, UnixMillis(100))
                .is_err()
        );
        assert!(
            store
                .append_operations_at(&[stopped()], true, |_| Err(StoreError(
                    "injected before COMMIT".to_owned()
                )))
                .is_err()
        );
        drop(store);
        let mut store = HistoryStore::open(candidate, directory.path())?;
        assert!(store.detail(&ObservationId::Native(RunId(2)))?.is_none());
        assert_eq!(
            ready(&mut store)?
                .state
                .conversion_runs
                .get(&RunId(2))
                .and_then(|run| run.outcome.as_ref()),
            None
        );
        store.append_operations(&[stopped()], true)?;
        let entry = store
            .detail(&ObservationId::Native(RunId(2)))?
            .ok_or("missing terminal observation")?;
        assert_eq!(entry.seq, 1);
        assert_eq!(
            entry.paths.as_ref().map(|paths| &paths.source),
            Some(&PathBuf::from("synthetic.mkv"))
        );
        let before = ready(&mut store)?;
        assert_eq!(before.state.runtime_id_high_water, 2);
        assert_eq!(
            crfty_core::observations(&before.state),
            vec![match &entry.observation {
                Observation::Native(native) => *native.clone(),
                _ => return Err("expected native".into()),
            }]
        );
        store.compact_operations(SessionState::Idle, UnixMillis(200))?;
        let compacted = ready(&mut store)?;
        assert_eq!(compacted.state, before.state);
        assert_eq!(compacted.next_sequence, before.next_sequence);
        drop(store);
        let mut store = HistoryStore::open(candidate, directory.path())?;
        let reopened = ready(&mut store)?;
        assert_eq!(reopened.state, before.state);
        assert_eq!(reopened.next_sequence, before.next_sequence);
        assert_eq!(store.detail(&entry.observation.id())?, Some(entry));
    }
    Ok(())
}

#[test]
fn reservation_only_ids_are_independent_of_history_and_unknown_operations_remain_browsable()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let (directory, mut store) = history(candidate)?;
        let mut deltas = prepared();
        deltas.pop();
        deltas.push(DurableDelta::ReservationReleased {
            item_id: QueueItemId(1),
            claim_id: ClaimId(1),
            run_id: RunId(2),
        });
        store.append_operations(&deltas, true)?;
        assert!(store.detail(&ObservationId::Native(RunId(2)))?.is_none());
        assert_eq!(ready(&mut store)?.state.runtime_id_high_water, 2);
        let imported: crfty_core::TranslatedObservation = serde_json::from_str(include_str!(
            "../../../crfty-core/tests/fixtures/observations/translated/valid/converted_without_sizes.json"
        ))?;
        store
            .import(
                vec![crfty_core::ImportCandidate {
                    observation: imported.clone(),
                    path: None,
                }],
                false,
            )
            .map_err(|error| format!("import: {error:?}"))?;
        store.compact_operations(SessionState::Idle, UnixMillis(10))?;
        store.database.execute(
            "UPDATE operational_snapshot SET record=?",
            &[Value::Blob(
                b"{\"schema_version\":999,\"record\":{}}\n".to_vec(),
            )],
        )?;
        drop(store);
        let mut store = HistoryStore::open(candidate, directory.path())?;
        assert!(matches!(
            store.operations()?,
            OperationalState::Unavailable {
                runtime_high_water: 2,
                ..
            }
        ));
        assert!(store.append_operations(&[], false).is_err());
        assert_eq!(
            store
                .detail(&ObservationId::Translated(imported.id()))?
                .map(|entry| entry.observation),
            Some(Observation::Translated(imported))
        );
        assert_eq!(
            store
                .page(PageRequest {
                    order: Order::Date,
                    direction: Direction::Descending,
                    outcomes: None,
                    search: None,
                    after: None,
                    limit: 100
                })?
                .matched,
            1
        );
    }
    Ok(())
}

#[test]
fn row_constraints_reject_missing_and_inactive_fields()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let (_directory, mut store) = history(candidate)?;
        let observation: crfty_core::NativeObservation = serde_json::from_str(include_str!(
            "../../../crfty-core/tests/fixtures/observations/native/valid/stopped.json"
        ))?;
        store.record(Observation::Native(Box::new(observation)), None, false)?;
        for statement in [
            "UPDATE observations SET fact_tag=NULL",
            "UPDATE observations SET fact_native_source_present=NULL",
            "UPDATE observations SET fact_native_outcome_tag='Converted'",
            "UPDATE observations SET fact_translated_record_key='0123456789abcdef'",
            "UPDATE observations SET fact_native_run_id=X'ff'",
        ] {
            assert!(
                store.database.execute(statement, &[]).is_err(),
                "{candidate:?}: {statement}"
            );
        }
        assert!(
            store
                .database
                .execute(
                    "INSERT INTO observation_paths(owner,source) VALUES(?,?)",
                    &[
                        Value::Blob(number::unsigned(999)),
                        Value::Blob(b"synthetic-orphan.mkv".to_vec())
                    ]
                )
                .is_err()
        );
        assert_eq!(store::metadata(&store.database, "revision")?, 1);
    }
    Ok(())
}

#[test]
fn relational_mapping_round_trips_every_valid_observation_fixture()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crfty-core/tests/fixtures/observations");
    for kind in ["native", "translated"] {
        for entry in std::fs::read_dir(root.join(kind).join("valid"))? {
            let bytes = std::fs::read(entry?.path())?;
            let observation = if kind == "native" {
                Observation::Native(Box::new(serde_json::from_slice(&bytes)?))
            } else {
                Observation::Translated(serde_json::from_slice(&bytes)?)
            };
            let packed = mapping::pack(&observation)?;
            assert_eq!(
                mapping::unpack(&packed.facts, &packed.children)?,
                observation
            );
        }
    }
    Ok(())
}

#[test]
fn full_range_numbers_sort_and_continue_across_signed_integer_boundary()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let (_directory, mut store) = history(candidate)?;
        store.database.execute(
            "UPDATE history_meta SET recording_high_water=?",
            &[Value::Blob(number::unsigned((1_u64 << 63) - 2))],
        )?;
        let template: crfty_core::TranslatedObservation = serde_json::from_str(include_str!(
            "../../../crfty-core/tests/fixtures/observations/translated/valid/converted_with_sizes_and_quality.json"
        ))?;
        let values = [
            0,
            1,
            (1_u64 << 53) + 1,
            (1_u64 << 63) - 1,
            1_u64 << 63,
            u64::MAX,
        ];
        let mut ids = Vec::new();
        for (index, value) in values.into_iter().enumerate() {
            let mut observation = template.clone();
            observation.record_key = crfty_core::RecordKey::new(format!("{index:016x}"))
                .map_err(|error| StoreError(error.to_owned()))?;
            observation.updated_at = Some(UnixMillis(value));
            observation.source.size_bytes = Some(1);
            observation.source.duration_ms = Some(u64::MAX);
            if let crfty_core::TranslatedOutcome::Converted {
                output_size,
                encode_duration,
                ..
            } = &mut observation.outcome
            {
                *output_size = Some(value);
                *encode_duration = Some(crfty_core::DurationMs(value));
            }
            ids.push(ObservationId::Translated(observation.id()));
            store.record(Observation::Translated(observation), None, false)?;
        }
        for order in [
            Order::Date,
            Order::After,
            Order::Took,
            Order::Recorded,
            Order::Change,
        ] {
            for direction in [Direction::Ascending, Direction::Descending] {
                let mut expected = ids.clone();
                if (direction == Direction::Descending) != (order == Order::Change) {
                    expected.reverse();
                }
                let mut cursor = None;
                let mut actual = Vec::new();
                loop {
                    let page = store.page(PageRequest {
                        order,
                        direction,
                        outcomes: None,
                        search: None,
                        after: cursor.as_ref(),
                        limit: 2,
                    })?;
                    actual.extend(page.ids);
                    cursor = page.next;
                    if cursor.is_none() {
                        break;
                    }
                }
                assert_eq!(actual, expected, "{candidate:?}: {order:?} {direction:?}");
            }
        }
        for id in ids {
            assert_eq!(
                store
                    .detail(&id)?
                    .map(|entry| entry.observation)
                    .and_then(|observation| match observation {
                        Observation::Translated(translated) => translated.source.duration_ms,
                        _ => None,
                    }),
                Some(u64::MAX)
            );
        }
    }
    Ok(())
}

#[test]
fn last_unsigned_sequence_can_commit_and_the_next_record_rolls_back()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let (_directory, mut store) = history(candidate)?;
        store.database.execute(
            "UPDATE history_meta SET recording_high_water=?",
            &[Value::Blob(number::unsigned(u64::MAX - 1))],
        )?;
        let mut observation: crfty_core::TranslatedObservation = serde_json::from_str(
            include_str!(
                "../../../crfty-core/tests/fixtures/observations/translated/valid/converted_without_sizes.json"
            ),
        )?;
        let id = ObservationId::Translated(observation.id());
        store.record(Observation::Translated(observation.clone()), None, false)?;
        assert_eq!(store.detail(&id)?.map(|entry| entry.seq), Some(u64::MAX));
        observation.record_key = crfty_core::RecordKey::new("000000000000abcd".to_owned())?;
        assert!(
            store
                .record(Observation::Translated(observation), None, false)
                .is_err()
        );
        assert_eq!(store::metadata(&store.database, "revision")?, 1);
        assert_eq!(
            store::metadata(&store.database, "recording_high_water")?,
            u64::MAX
        );
    }
    Ok(())
}

#[test]
fn platform_native_path_units_round_trip_without_text_conversion()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    let path = {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        PathBuf::from(OsString::from_vec(vec![b's', 0xff, b'.', b'm', b'k', b'v']))
    };
    #[cfg(windows)]
    let path = {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt};
        PathBuf::from(OsString::from_wide(&[0xD800, 0x73, 0x2e, 0x6d, 0x6b, 0x76]))
    };
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let (_directory, mut store) = history(candidate)?;
        let observation: crfty_core::NativeObservation = serde_json::from_str(include_str!(
            "../../../crfty-core/tests/fixtures/observations/native/valid/stopped.json"
        ))?;
        let id = ObservationId::Native(observation.run_id);
        let paths = crfty_core::ObservationPaths {
            source: path.clone(),
            output: Some(path.clone()),
        };
        store.record(
            Observation::Native(Box::new(observation)),
            Some(paths.clone()),
            false,
        )?;
        assert_eq!(
            store.detail(&id)?.and_then(|entry| entry.paths),
            Some(paths)
        );
    }
    Ok(())
}
