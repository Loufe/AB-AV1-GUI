use crate::{
    ContentKey, DurableDelta, DurableState, FileRecord, ImportPath, ImportedHistoryRecord,
    ImportedProvenance, MediaObservation, ParkedResolution, resolve_parked,
};

/// Parked-import effects computed while a reserved job is prepared. The
/// reducer owns when these deltas are emitted; this module owns which parked
/// records win and what record the job-action policy must observe.
pub(crate) struct PreparedImportAdoptions {
    pub(crate) deltas: Vec<DurableDelta>,
    pub(crate) effective_record: Option<FileRecord>,
}

pub(crate) fn prepare_import_adoptions(
    durable: &DurableState,
    observation: Option<&MediaObservation>,
    import_paths: &[ImportPath],
    adoption_content_key: Option<&ContentKey>,
    record: Option<&FileRecord>,
) -> PreparedImportAdoptions {
    let (Some(observation), Some(adoption_content_key)) = (observation, adoption_content_key)
    else {
        return PreparedImportAdoptions {
            deltas: Vec::new(),
            effective_record: None,
        };
    };

    // A native verdict outranks every imported verdict. Imported provenance
    // still lands so the inbox drains and re-import protection remains exact.
    let native_verdict_stands = record
        .and_then(|known| known.verdict.as_ref())
        .is_some_and(|existing| existing.source_run.is_some());
    let resolutions: Vec<(ImportPath, ImportedHistoryRecord, ParkedResolution)> = import_paths
        .iter()
        .filter_map(|key| durable.parked.get(key).map(|parked| (key, parked)))
        .map(|(key, parked)| {
            (
                key.clone(),
                parked.clone(),
                resolve_parked(parked, observation),
            )
        })
        .collect();

    let mut selected_import = record.and_then(|known| known.imported.clone());
    for (import_path, imported, resolution) in &resolutions {
        if !matches!(resolution, ParkedResolution::Adopt { .. }) {
            continue;
        }
        let candidate = ImportedProvenance {
            import_path: import_path.clone(),
            record: imported.clone(),
        };
        if selected_import
            .as_ref()
            .is_none_or(|current| candidate.outranks(current))
        {
            selected_import = Some(candidate);
        }
    }
    let selected_path = selected_import
        .as_ref()
        .map(|selected| &selected.import_path);
    let deltas: Vec<DurableDelta> = resolutions
        .into_iter()
        .map(|(import_path, imported, resolution)| match resolution {
            ParkedResolution::Adopt { verdict } => DurableDelta::ParkedAdopted {
                verdict: if !native_verdict_stands && selected_path == Some(&import_path) {
                    verdict
                } else {
                    None
                },
                import_path,
                content_key: adoption_content_key.clone(),
                imported,
            },
            ParkedResolution::Retire => DurableDelta::ParkedRetired { import_path },
        })
        .collect();

    let adopted_verdict = deltas.iter().rev().find_map(|delta| match delta {
        DurableDelta::ParkedAdopted {
            verdict: Some(verdict),
            ..
        } => Some(verdict.clone()),
        _ => None,
    });
    // The effective record is the record as it will exist once MediaObserved
    // and the adoption deltas fold. Job-action selection must use this view.
    let effective_record = match (&adopted_verdict, record) {
        (Some(verdict), Some(known)) => {
            let mut known = known.clone();
            known.verdict = Some(verdict.clone());
            Some(known)
        }
        (Some(verdict), None) if adoption_content_key == &observation.binding.content_key => {
            let mut fresh = FileRecord::new(observation.metadata.clone());
            fresh.verdict = Some(verdict.clone());
            Some(fresh)
        }
        (Some(_), None) | (None, _) => None,
    };

    PreparedImportAdoptions {
        deltas,
        effective_record,
    }
}

#[cfg(test)]
mod tests {
    use super::prepare_import_adoptions;
    use crate::{
        ContentKey, Crf, DestructiveIdentity, DurableDelta, DurableState, FileSystemId, FileTimeNs,
        ImportPath, ImportedHistoryRecord, MediaContainer, MediaObservation, ParkedStatus,
        PathBinding, PathHash, UnixMillis, VideoCodec, VideoMeta, VmafScore, VmafTarget,
    };

    fn observation() -> MediaObservation {
        MediaObservation {
            path_hash: PathHash("path".to_owned()),
            binding: PathBinding {
                identity: DestructiveIdentity {
                    file_id: FileSystemId::Unix {
                        device: 1,
                        inode: 2,
                    },
                    size: 10_000,
                    modified_ns: Some(FileTimeNs(1)),
                },
                content_key: ContentKey("content".to_owned()),
            },
            metadata: VideoMeta {
                codec: VideoCodec::H264,
                container: MediaContainer::Matroska,
                width: 1_280,
                height: 720,
                rotation_degrees: 0,
                duration_ms: 60_000,
                size_bytes: 10_000,
                audio: Vec::new(),
                subtitle_count: 0,
            },
        }
    }

    fn imported(status: ParkedStatus, decided_at: u64) -> ImportedHistoryRecord {
        ImportedHistoryRecord {
            status,
            size: Some(10_000),
            modified_ns: Some(FileTimeNs(1)),
            video_codec: Some(VideoCodec::H264),
            width: Some(1_280),
            height: Some(720),
            duration_ms: Some(60_000),
            output_size: Some(4_000),
            encoding_time: None,
            crf: Some(Crf(30_000)),
            vmaf: Some(VmafScore(9_500)),
            target: Some(VmafTarget(95)),
            requested_target: Some(VmafTarget(95)),
            floor_target: Some(VmafTarget(90)),
            decided_at: UnixMillis(decided_at),
        }
    }

    #[test]
    fn strongest_adoption_supplies_the_effective_verdict() {
        let observation = observation();
        let older = ImportPath("older".to_owned());
        let newer = ImportPath("newer".to_owned());
        let stale = ImportPath("stale".to_owned());
        let mut stale_record = imported(ParkedStatus::Scanned, 3_000);
        stale_record.size = Some(99_999);
        let mut durable = DurableState::default();
        durable
            .parked
            .insert(older.clone(), imported(ParkedStatus::Converted, 1_000));
        durable
            .parked
            .insert(newer.clone(), imported(ParkedStatus::NotWorthwhile, 2_000));
        durable.parked.insert(stale.clone(), stale_record);

        let prepared = prepare_import_adoptions(
            &durable,
            Some(&observation),
            &[older, stale.clone(), newer.clone()],
            Some(&observation.binding.content_key),
            None,
        );

        assert_eq!(prepared.deltas.len(), 3);
        assert!(prepared.deltas.iter().any(
            |delta| matches!(delta, DurableDelta::ParkedRetired { import_path } if import_path == &stale)
        ));
        assert!(prepared.deltas.iter().any(|delta| matches!(
            delta,
            DurableDelta::ParkedAdopted {
                import_path,
                verdict: Some(_),
                ..
            } if import_path == &newer
        )));
        assert!(prepared.effective_record.is_some_and(|record| {
            record
                .verdict
                .is_some_and(|verdict| verdict.decided_at == UnixMillis(2_000))
        }));
    }

    #[test]
    fn preparation_without_an_observation_has_no_import_effects() {
        let durable = DurableState::default();
        let prepared = prepare_import_adoptions(&durable, None, &[], None, None);

        assert!(prepared.deltas.is_empty());
        assert!(prepared.effective_record.is_none());
    }
}
