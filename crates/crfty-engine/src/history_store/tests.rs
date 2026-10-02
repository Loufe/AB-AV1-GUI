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

fn media(rotation: i16) -> crfty_core::MediaObservation {
    crfty_core::MediaObservation {
        path_hash: crfty_core::PathHash("synthetic-path-hash".to_owned()),
        binding: crfty_core::PathBinding {
            identity: crfty_core::DestructiveIdentity {
                file_id: crfty_core::FileSystemId::Unix {
                    device: 1,
                    inode: 1,
                },
                size: 1000,
                modified_ns: None,
            },
            content_key: crfty_core::ContentKey("synthetic-content-key".to_owned()),
        },
        metadata: crfty_core::VideoMeta {
            codec: crfty_core::VideoCodec::Hevc,
            container: crfty_core::MediaContainer::Matroska,
            width: 1920,
            height: 1080,
            rotation_degrees: rotation,
            duration_ms: 1000,
            size_bytes: 1000,
            audio: Vec::new(),
            subtitle_count: 0,
        },
    }
}

fn prepared_source() -> Vec<DurableDelta> {
    let mut deltas = prepared();
    let observation = media(0);
    if let Some(DurableDelta::ItemPrepared { spec }) = deltas.last_mut() {
        spec.source = Some(crfty_core::SourceFacts::from_media(
            observation.binding.content_key.clone(),
            &observation.metadata,
        ));
    }
    deltas.insert(
        2,
        DurableDelta::MediaObserved {
            observation: Box::new(observation),
        },
    );
    deltas
}

#[test]
fn backward_wall_clock_terminal_preserves_exact_stamps_and_monotonic_spans_after_restart()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let (directory, mut store) = history(candidate)?;
        let mut deltas = prepared();
        deltas.push(DurableDelta::ItemRunning {
            item_id: QueueItemId(1),
            claim_id: ClaimId(1),
            run_id: RunId(2),
            at: UnixMillis(200),
        });
        store.append_operations(&deltas, true)?;
        let mut terminal = stopped();
        let spans = vec![crfty_core::PhaseSpan {
            phase: crfty_core::JobPhase::Encoding,
            duration: crfty_core::DurationMs(777),
        }];
        if let DurableDelta::ItemFinished { phase_spans, .. } = &mut terminal {
            *phase_spans = spans.clone();
        }
        store.append_operations(&[terminal], true)?;
        store.compact_operations(SessionState::Idle, UnixMillis(100))?;
        drop(store);
        let mut store = HistoryStore::open(candidate, directory.path())?;
        let entry = store
            .detail(&ObservationId::Native(RunId(2)))?
            .ok_or("missing clock-adjusted terminal")?;
        let Observation::Native(native) = entry.observation else {
            return Err("expected native".into());
        };
        assert_eq!(native.started_at, Some(UnixMillis(200)));
        assert_eq!(native.finished_at, Some(UnixMillis(123)));
        assert_eq!(native.validate(), Ok(()));
        assert_eq!(
            ready(&mut store)?
                .state
                .conversion_runs
                .get(&RunId(2))
                .map(|run| &run.phase_spans),
            Some(&spans)
        );
        assert_eq!(store::metadata(&store.database, "revision")?, 1);
    }
    Ok(())
}

#[test]
fn scans_on_both_sides_of_terminal_and_batch_partitioning_preserve_prepared_facts()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        for scan_before in [false, true] {
            let (_first, mut separate) = history(candidate)?;
            let (second, mut together) = history(candidate)?;
            separate.append_operations(&prepared_source(), true)?;
            together.append_operations(&prepared_source(), true)?;
            let mut deltas = Vec::new();
            if scan_before {
                deltas.push(DurableDelta::MediaObserved {
                    observation: Box::new(media(90)),
                });
            }
            deltas.extend([
                stopped(),
                DurableDelta::MediaObserved {
                    observation: Box::new(media(180)),
                },
            ]);
            for delta in &deltas {
                separate.append_operations(std::slice::from_ref(delta), true)?;
            }
            together.append_operations(&deltas, true)?;
            let id = ObservationId::Native(RunId(2));
            let entry = together.detail(&id)?.ok_or("missing terminal")?;
            assert_eq!(separate.detail(&id)?, Some(entry.clone()));
            let Observation::Native(native) = &entry.observation else {
                return Err("expected native".into());
            };
            assert_eq!(
                native
                    .source
                    .as_ref()
                    .map(|source| (source.width, source.height)),
                Some((1920, 1080))
            );
            together.compact_operations(SessionState::Idle, UnixMillis(200))?;
            drop(together);
            let mut reopened = HistoryStore::open(candidate, second.path())?;
            assert_eq!(reopened.detail(&id)?, Some(entry));
        }
    }
    Ok(())
}

#[test]
fn multiple_terminals_share_one_revision_and_invalid_batches_commit_nothing()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for candidate in [Candidate::Rusqlite, Candidate::Turso] {
        let (_directory, mut store) = history(candidate)?;
        store.append_operations(&prepared(), false)?;
        let before = ready(&mut store)?;
        assert!(
            store
                .append_operations(&[stopped(), stopped()], false)
                .is_err()
        );
        assert_eq!(ready(&mut store)?, before);
        assert!(store.detail(&ObservationId::Native(RunId(2)))?.is_none());
        assert_eq!(store::metadata(&store.database, "recording_high_water")?, 0);
        assert_eq!(store::metadata(&store.database, "revision")?, 0);
        let mut second = prepared();
        for delta in &mut second {
            match delta {
                DurableDelta::QueueAdded { item } => item.id = QueueItemId(3),
                DurableDelta::ItemReserved { job } => {
                    job.item_id = QueueItemId(3);
                    job.claim_id = ClaimId(3);
                    job.run_id = RunId(4);
                }
                DurableDelta::ItemPrepared { spec } => {
                    spec.item_id = QueueItemId(3);
                    spec.claim_id = ClaimId(3);
                    spec.run_id = RunId(4);
                }
                _ => return Err("unexpected preparation delta".into()),
            }
        }
        let mut last = stopped();
        if let DurableDelta::ItemFinished {
            item_id,
            claim_id,
            run_id,
            ..
        } = &mut last
        {
            *item_id = QueueItemId(3);
            *claim_id = ClaimId(3);
            *run_id = RunId(4);
        }
        let mut deltas = vec![stopped()];
        deltas.extend(second);
        deltas.push(last);
        store.append_operations(&deltas, false)?;
        for (run, seq) in [(2, 1), (4, 2)] {
            let entry = store
                .detail(&ObservationId::Native(RunId(run)))?
                .ok_or("missing batch terminal")?;
            assert_eq!(entry.seq, seq);
            assert_eq!(entry.paths, None);
        }
        assert_eq!(store::metadata(&store.database, "revision")?, 1);
        assert_eq!(store::metadata(&store.database, "runtime_high_water")?, 4);
    }
    Ok(())
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

#[test]
fn windows_path_decoding_preserves_surrogates_and_rejects_incomplete_units() -> Result<()> {
    assert_eq!(
        store::decode_windows_units(&[2, 0, 0xD8, 0x73, 0])?,
        vec![0xD800, 0x73]
    );
    for malformed in [&[][..], &[1][..], &[2, 0][..]] {
        assert!(store::decode_windows_units(malformed).is_err());
    }
    Ok(())
}
