//! The v3 history import surface: strict parsing of the versioned import
//! file defined in `docs/HISTORY_IMPORT.md`, and the path normalization that
//! lets a parked record meet the real file it describes.
//!
//! The app knows nothing about any older history format. The standalone
//! converter script shipped in this repository (`tools/export_history_v3.py`)
//! does all source-format interpretation and emits this schema; anything malformed
//! here is a converter or version-skew problem, so parsing rejects the whole
//! file rather than salvaging records.

use std::path::Path;

use crfty_core::{
    FileTimeNs, ImportPath, ImportedHistoryRecord, MAX_VMAF_SCORE, ParkedStatus, UnixMillis,
    VMAF_SCORE_FIXED_SCALE, VideoCodec,
};
use serde::Deserialize;

/// The import-file schema version this build reads.
pub(crate) const IMPORT_SCHEMA_VERSION: u32 = 1;

/// Refuse to read import files larger than this; the schema describes
/// metadata, not media, and even six-figure record counts stay far below it.
pub(crate) const MAX_IMPORT_BYTES: u64 = 256 * 1024 * 1024;

const CONVERTER_HINT: &str =
    "produce the file with the bundled converter script (tools/export_history_v3.py)";

#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    Unreadable { detail: String },
    TooLarge { size: u64 },
    UnsupportedVersion { found: u32 },
    Malformed { detail: String },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable { detail } => {
                write!(formatter, "failed to read the import file: {detail}")
            }
            Self::TooLarge { size } => write!(
                formatter,
                "import file is {size} bytes, above the {MAX_IMPORT_BYTES}-byte limit"
            ),
            Self::UnsupportedVersion { found } => write!(
                formatter,
                "unsupported import file version {found} (this build reads version \
                 {IMPORT_SCHEMA_VERSION}); {CONVERTER_HINT}"
            ),
            Self::Malformed { detail } => {
                write!(
                    formatter,
                    "malformed import file: {detail}; {CONVERTER_HINT}"
                )
            }
        }
    }
}

impl std::error::Error for ImportError {}

/// Read and strictly parse an import file from disk, refusing files over
/// [`MAX_IMPORT_BYTES`] before reading them.
pub fn load_import_file(
    path: &Path,
    now: UnixMillis,
) -> Result<Vec<(ImportPath, ImportedHistoryRecord)>, ImportError> {
    load_import_file_capped(path, MAX_IMPORT_BYTES, now)
}

fn load_import_file_capped(
    path: &Path,
    cap: u64,
    now: UnixMillis,
) -> Result<Vec<(ImportPath, ImportedHistoryRecord)>, ImportError> {
    let metadata = std::fs::metadata(path).map_err(|error| ImportError::Unreadable {
        detail: error.to_string(),
    })?;
    if metadata.len() > cap {
        return Err(ImportError::TooLarge {
            size: metadata.len(),
        });
    }
    let bytes = std::fs::read(path).map_err(|error| ImportError::Unreadable {
        detail: error.to_string(),
    })?;
    parse_import(&bytes, now)
}

/// Normalize a source path into the key spelling both sides of the match
/// use. The rule is v3's own, applied identically at import time and to the
/// observed file's spellings at prepare time: strip Windows verbatim
/// prefixes (`\\?\UNC\server\share\…` → `\\server\share\…`, `\\?\C:\…` →
/// `C:\…`), turn backslashes into forward slashes, and lowercase (ASCII).
/// Keys therefore compare case-insensitively; on a case-sensitive filesystem
/// two paths differing only in ASCII case share a key, and the import's
/// dedup keeps the first.
#[must_use]
pub fn normalize_import_path(raw: &str) -> ImportPath {
    let stripped = strip_verbatim(raw.trim());
    let mut normalized = stripped.replace('\\', "/");
    normalized.make_ascii_lowercase();
    ImportPath(normalized)
}

fn strip_verbatim(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        raw.to_owned()
    }
}

/// The normalized spellings under which the file at `path` could have been
/// imported: the canonical spelling (drive letters resolved to UNC, symlinks
/// followed) and the merely-absolute one (canonicalization can fail on some
/// network mounts, and import files may carry unresolved spellings). At most
/// two, deduplicated.
#[must_use]
pub(crate) fn import_path_candidates(path: &Path) -> Vec<ImportPath> {
    let mut candidates = Vec::new();
    if let Ok(canonical) = std::fs::canonicalize(path) {
        candidates.push(normalize_import_path(&canonical.to_string_lossy()));
    }
    if let Ok(absolute) = std::path::absolute(path) {
        let candidate = normalize_import_path(&absolute.to_string_lossy());
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// The import file: `{ "import_version": 1, "records": [ … ] }`. Unknown
/// fields are version skew and reject the file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportFile {
    import_version: u32,
    records: Vec<ImportRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRecord {
    path: String,
    status: ImportStatus,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    modified_ns: Option<FileTimeNs>,
    #[serde(default)]
    video_codec: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    output_size: Option<u64>,
    #[serde(default)]
    encoding_time_ms: Option<u64>,
    #[serde(default)]
    crf_thousandths: Option<u32>,
    #[serde(default)]
    vmaf_hundredths: Option<u16>,
    #[serde(default)]
    target: Option<u8>,
    #[serde(default)]
    requested_target: Option<u8>,
    #[serde(default)]
    floor_target: Option<u8>,
    #[serde(default)]
    decided_at_ms: Option<u64>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ImportStatus {
    Scanned,
    Analyzed,
    NotWorthwhile,
    Converted,
}

impl From<ImportStatus> for ParkedStatus {
    fn from(status: ImportStatus) -> Self {
        match status {
            ImportStatus::Scanned => Self::Scanned,
            ImportStatus::Analyzed => Self::Analyzed,
            ImportStatus::NotWorthwhile => Self::NotWorthwhile,
            ImportStatus::Converted => Self::Converted,
        }
    }
}

/// Parse an import file into parked records keyed by normalized path.
/// Duplicate keys pass through — the reducer is the single dedup authority
/// and counts them as skipped. `now` backfills records without a decision
/// timestamp.
pub(crate) fn parse_import(
    bytes: &[u8],
    now: UnixMillis,
) -> Result<Vec<(ImportPath, ImportedHistoryRecord)>, ImportError> {
    let file: ImportFile =
        serde_json::from_slice(bytes).map_err(|error| ImportError::Malformed {
            detail: error.to_string(),
        })?;
    if file.import_version != IMPORT_SCHEMA_VERSION {
        return Err(ImportError::UnsupportedVersion {
            found: file.import_version,
        });
    }
    file.records
        .into_iter()
        .map(|record| convert_record(record, now))
        .collect()
}

fn convert_record(
    record: ImportRecord,
    now: UnixMillis,
) -> Result<(ImportPath, ImportedHistoryRecord), ImportError> {
    let key = normalize_import_path(&record.path);
    if key.0.is_empty() {
        return Err(ImportError::Malformed {
            detail: "record path is empty".to_owned(),
        });
    }
    let vmaf_ceiling = MAX_VMAF_SCORE.saturating_mul(VMAF_SCORE_FIXED_SCALE);
    if record
        .vmaf_hundredths
        .is_some_and(|vmaf| vmaf > vmaf_ceiling)
    {
        return Err(ImportError::Malformed {
            detail: format!("vmaf_hundredths exceeds {vmaf_ceiling} for {:?}", key.0),
        });
    }
    for target in [record.target, record.requested_target, record.floor_target]
        .into_iter()
        .flatten()
    {
        if u16::from(target) > MAX_VMAF_SCORE {
            return Err(ImportError::Malformed {
                detail: format!(
                    "VMAF target {target} exceeds {MAX_VMAF_SCORE} for {:?}",
                    key.0
                ),
            });
        }
    }
    let parked = ImportedHistoryRecord {
        status: record.status.into(),
        size: record.size,
        modified_ns: record.modified_ns,
        video_codec: record.video_codec.as_deref().map(video_codec),
        width: record.width,
        height: record.height,
        duration_ms: record.duration_ms,
        output_size: record.output_size,
        encoding_time: record.encoding_time_ms.map(crfty_core::DurationMs),
        crf: record.crf_thousandths.map(crfty_core::Crf),
        vmaf: record.vmaf_hundredths.map(crfty_core::VmafScore),
        target: record.target.map(crfty_core::VmafTarget),
        requested_target: record.requested_target.map(crfty_core::VmafTarget),
        floor_target: record.floor_target.map(crfty_core::VmafTarget),
        decided_at: record.decided_at_ms.map_or(now, UnixMillis),
    };
    Ok((key, parked))
}

fn video_codec(raw: &str) -> VideoCodec {
    match raw {
        "av1" => VideoCodec::Av1,
        "h264" => VideoCodec::H264,
        "hevc" => VideoCodec::Hevc,
        "vp9" => VideoCodec::Vp9,
        other => VideoCodec::Other(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_strips_verbatim_prefixes_and_folds_case() {
        assert_eq!(
            normalize_import_path(r"\\?\UNC\Server\Share\Videos\A.MKV").0,
            "//server/share/videos/a.mkv"
        );
        assert_eq!(
            normalize_import_path(r"\\?\C:\Videos\Movie.mkv").0,
            "c:/videos/movie.mkv"
        );
        assert_eq!(
            normalize_import_path(r"C:\Videos\Movie.mkv").0,
            "c:/videos/movie.mkv"
        );
        assert_eq!(
            normalize_import_path("/mnt/Media/Movie.mkv").0,
            "/mnt/media/movie.mkv"
        );
        assert_eq!(
            normalize_import_path("  C:\\Videos\\padded.mkv  ").0,
            "c:/videos/padded.mkv"
        );
    }

    #[test]
    fn candidates_deduplicate_canonical_and_absolute_spellings() {
        let file = tempfile::NamedTempFile::new().expect("create temp file");
        let candidates = import_path_candidates(file.path());
        assert!(!candidates.is_empty());
        assert!(candidates.len() <= 2);
        let unique: std::collections::BTreeSet<_> = candidates.iter().collect();
        assert_eq!(unique.len(), candidates.len());
        // A path that cannot canonicalize still yields the absolute spelling.
        let missing = file.path().with_extension("does-not-exist");
        let fallback = import_path_candidates(&missing);
        assert_eq!(fallback.len(), 1);
    }

    fn full_record_json() -> serde_json::Value {
        serde_json::json!({
            "path": "C:\\Videos\\Movie.mkv",
            "status": "converted",
            "size": 3_000_000_u64,
            "modified_ns": "1752871234567890123",
            "video_codec": "h264",
            "width": 1920,
            "height": 1080,
            "duration_ms": 120_000_u64,
            "output_size": 1_000_000_u64,
            "encoding_time_ms": 88_000_u64,
            "crf_thousandths": 30_000_u32,
            "vmaf_hundredths": 9_550_u16,
            "target": 95,
            "requested_target": 95,
            "floor_target": 90,
            "decided_at_ms": 1_700_000_000_000_u64,
        })
    }

    fn import_file(records: Vec<serde_json::Value>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "import_version": 1,
            "records": records,
        }))
        .expect("serialize import file")
    }

    #[test]
    fn parses_full_and_sparse_records() {
        let bytes = import_file(vec![
            full_record_json(),
            serde_json::json!({ "path": "/mnt/media/b.mkv", "status": "scanned" }),
        ]);
        let records = parse_import(&bytes, UnixMillis(42)).expect("parse import");
        assert_eq!(records.len(), 2);
        let (key, parked) = &records[0];
        assert_eq!(key.0, "c:/videos/movie.mkv");
        assert_eq!(parked.status, ParkedStatus::Converted);
        assert_eq!(parked.size, Some(3_000_000));
        assert_eq!(
            parked.modified_ns,
            Some(FileTimeNs(1_752_871_234_567_890_123))
        );
        assert_eq!(parked.video_codec, Some(VideoCodec::H264));
        assert_eq!(parked.crf, Some(crfty_core::Crf(30_000)));
        assert_eq!(parked.vmaf, Some(crfty_core::VmafScore(9_550)));
        assert_eq!(parked.decided_at, UnixMillis(1_700_000_000_000));
        let (_, sparse) = &records[1];
        assert_eq!(sparse.status, ParkedStatus::Scanned);
        assert_eq!(sparse.size, None);
        // A record without a decision timestamp adopts the import instant.
        assert_eq!(sparse.decided_at, UnixMillis(42));
    }

    #[test]
    fn rejects_wrong_version_naming_the_converter() {
        let bytes = serde_json::to_vec(&serde_json::json!({ "import_version": 2, "records": [] }))
            .expect("serialize");
        let error = parse_import(&bytes, UnixMillis(0)).expect_err("must reject");
        assert_eq!(error, ImportError::UnsupportedVersion { found: 2 });
        assert!(error.to_string().contains("export_history_v3.py"));
    }

    #[test]
    fn rejects_files_that_are_not_the_import_schema() {
        // A raw V2-style history document is not an import file.
        let foreign = serde_json::to_vec(&serde_json::json!({
            "schema_version": 2,
            "records": { "abc123": { "status": "completed" } },
        }))
        .expect("serialize");
        assert!(matches!(
            parse_import(&foreign, UnixMillis(0)),
            Err(ImportError::Malformed { .. })
        ));
        // Unknown fields are version skew, not tolerated noise.
        let mut extra = full_record_json();
        extra["surprise"] = serde_json::json!(true);
        assert!(matches!(
            parse_import(&import_file(vec![extra]), UnixMillis(0)),
            Err(ImportError::Malformed { .. })
        ));
        assert!(matches!(
            parse_import(b"not json", UnixMillis(0)),
            Err(ImportError::Malformed { .. })
        ));
    }

    #[test]
    fn rejects_out_of_range_values_and_empty_paths() {
        let mut empty_path = full_record_json();
        empty_path["path"] = serde_json::json!("   ");
        assert!(matches!(
            parse_import(&import_file(vec![empty_path]), UnixMillis(0)),
            Err(ImportError::Malformed { .. })
        ));
        let mut vmaf = full_record_json();
        vmaf["vmaf_hundredths"] = serde_json::json!(10_001);
        assert!(matches!(
            parse_import(&import_file(vec![vmaf]), UnixMillis(0)),
            Err(ImportError::Malformed { .. })
        ));
        let mut target = full_record_json();
        target["target"] = serde_json::json!(101);
        assert!(matches!(
            parse_import(&import_file(vec![target]), UnixMillis(0)),
            Err(ImportError::Malformed { .. })
        ));
    }

    #[test]
    fn duplicate_keys_pass_through_for_the_reducer_to_dedup() {
        let bytes = import_file(vec![full_record_json(), full_record_json()]);
        let records = parse_import(&bytes, UnixMillis(0)).expect("parse import");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0, records[1].0);
    }

    #[test]
    fn load_enforces_the_size_cap_and_reports_unreadable_files() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().expect("create temp file");
        file.write_all(&import_file(vec![full_record_json()]))
            .expect("write import file");
        file.flush().expect("flush import file");
        let records = load_import_file(file.path(), UnixMillis(0)).expect("load import file");
        assert_eq!(records.len(), 1);
        assert!(matches!(
            load_import_file_capped(file.path(), 4, UnixMillis(0)),
            Err(ImportError::TooLarge { .. })
        ));
        let missing = file.path().with_extension("does-not-exist");
        assert!(matches!(
            load_import_file(&missing, UnixMillis(0)),
            Err(ImportError::Unreadable { .. })
        ));
    }

    #[test]
    fn unknown_codec_strings_are_preserved_not_dropped() {
        let mut record = full_record_json();
        record["video_codec"] = serde_json::json!("mpeg2video");
        let records =
            parse_import(&import_file(vec![record]), UnixMillis(0)).expect("parse import");
        assert_eq!(
            records[0].1.video_codec,
            Some(VideoCodec::Other("mpeg2video".to_owned()))
        );
    }
}
