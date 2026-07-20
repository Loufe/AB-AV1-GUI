use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    AnalysisProfile, AnalysisResult, ContentKey, Crf, DurationMs, RunId, UnixMillis, VmafScore,
    VmafTarget,
};

const HALF_ROTATION_DEGREES: i16 = 180;
const QUARTER_ROTATION_DEGREES: i16 = 90;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct PathHash(pub String);

/// The normalized source-path key an imported history record is parked
/// under. Normalization is v3's own documented rule (verbatim prefixes
/// stripped, backslashes to slashes, lowercased) applied by the engine at
/// import and at prepare time, so both sides of the match meet on the same
/// spelling. Cleartext PII — a phase-5 scrub target.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct ImportPath(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FileStamp {
    #[specta(type = crate::JsNumber)]
    pub size: u64,
    pub modified_ns: Option<crate::FileTimeNs>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PathBinding {
    pub stamp: FileStamp,
    pub content_key: ContentKey,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub enum VideoCodec {
    Av1,
    H264,
    Hevc,
    Vp9,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum MediaContainer {
    Matroska,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum AudioCodec {
    Aac,
    Ac3,
    Eac3,
    Dts,
    Opus,
    Flac,
    Mp3,
    Other(String),
}

/// One audio stream of the inspected file. Consumers are remux-eligibility
/// policy and remux reporting, not the current view designs, so this stays a
/// summary rather than a full stream description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AudioStreamMeta {
    pub codec: AudioCodec,
    pub channels: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct VideoMeta {
    pub codec: VideoCodec,
    pub container: MediaContainer,
    pub width: u32,
    pub height: u32,
    pub rotation_degrees: i16,
    #[specta(type = crate::JsNumber)]
    pub duration_ms: u64,
    /// Byte size of the inspected file — a content fact and the authority for
    /// input size in views. `FileStamp.size` remains the freshness probe.
    /// Bitrate is derived in views, never stored.
    #[specta(type = crate::JsNumber)]
    pub size_bytes: u64,
    pub audio: Vec<AudioStreamMeta>,
    pub subtitle_count: u32,
}

impl VideoMeta {
    #[must_use]
    pub fn post_rotation_dimensions(&self) -> (u32, u32) {
        if self.rotation_degrees.rem_euclid(HALF_ROTATION_DEGREES) == QUARTER_ROTATION_DEGREES {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    #[must_use]
    pub fn post_rotation_pixels(&self) -> u64 {
        let (width, height) = self.post_rotation_dimensions();
        u64::from(width).saturating_mul(u64::from(height))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct MediaObservation {
    pub path_hash: PathHash,
    pub binding: PathBinding,
    pub metadata: VideoMeta,
}

/// The record's standing judgment about this content: what the latest
/// decisive run concluded. Distinct from `ItemOutcome` (a run-level fact) —
/// only Converted, Remuxed, and NotWorthwhile outcomes decide anything about
/// the content itself. Analyzed results live in `analyses` (facts, not
/// verdicts); Stopped, Skipped, and Failed decide nothing.
///
/// Lineage is derived, never stored: the runs that concern this content are
/// `conversion_runs` filtered by `spec.content_key`, ordered by the monotonic
/// `RunId`. `source_run` links the verdict into that chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Verdict {
    pub kind: VerdictKind,
    /// `None`: decided before this app existed — the verdict was adopted
    /// from a history import and no conversion run backs it.
    pub source_run: Option<RunId>,
    pub decided_at: UnixMillis,
}

/// Summary fields only: the full attempt list stays on the run outcome, and
/// the produced artifact's full identity stays on the settled output
/// transaction (`outputs[source_run]`). The measured summary (sizes, encode
/// duration, achieved quality) is absorbed here at fold time so consumers
/// never branch on provenance; every summary field is nullable because a
/// history import — and a crash-recovered native success — cannot honestly
/// supply it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub enum VerdictKind {
    Converted {
        /// `None`: adopted from a history import — the produced output was
        /// never content-hashed.
        output_content_key: Option<ContentKey>,
        #[specta(type = Option<crate::JsNumber>)]
        input_size: Option<u64>,
        #[specta(type = Option<crate::JsNumber>)]
        output_size: Option<u64>,
        encoding_time: Option<DurationMs>,
        crf: Option<Crf>,
        vmaf: Option<VmafScore>,
        target: Option<VmafTarget>,
    },
    Remuxed {
        /// Required: imports never carry remuxes, so every remux verdict
        /// names a settled, content-hashed output.
        output_content_key: ContentKey,
        #[specta(type = Option<crate::JsNumber>)]
        input_size: Option<u64>,
        #[specta(type = Option<crate::JsNumber>)]
        output_size: Option<u64>,
    },
    NotWorthwhile {
        requested: VmafTarget,
        floor: VmafTarget,
    },
}

/// The profile-keyed index crosses serde as an entry list: JSON map keys must
/// be strings, and `AnalysisProfile` is a multi-field struct.
pub type AnalysisIndexEntries = Vec<(AnalysisProfile, BTreeMap<VmafTarget, AnalysisResult>)>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FileRecord {
    pub metadata: VideoMeta,
    #[serde(with = "analysis_index")]
    #[specta(type = AnalysisIndexEntries)]
    pub analyses: BTreeMap<AnalysisProfile, BTreeMap<VmafTarget, AnalysisResult>>,
    pub verdict: Option<Verdict>,
    /// Provenance of a history-import adoption, and the re-import guard:
    /// keys recorded here (or still parked) are skipped by later imports.
    pub imported: Option<ImportPath>,
}

mod analysis_index {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serializer};

    use super::{AnalysisIndexEntries, AnalysisProfile, AnalysisResult, VmafTarget};

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<AnalysisProfile, BTreeMap<VmafTarget, AnalysisResult>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(map.iter())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<AnalysisProfile, BTreeMap<VmafTarget, AnalysisResult>>, D::Error> {
        Ok(AnalysisIndexEntries::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

impl FileRecord {
    #[must_use]
    pub fn new(metadata: VideoMeta) -> Self {
        Self {
            metadata,
            analyses: BTreeMap::new(),
            verdict: None,
            imported: None,
        }
    }

    pub(crate) fn record_analysis(&mut self, result: AnalysisResult) {
        self.analyses
            .entry(result.profile.clone())
            .or_default()
            .insert(result.successful_target, result);
    }
}

/// The decisive standing of an imported history record.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
pub enum ParkedStatus {
    Scanned,
    Analyzed,
    NotWorthwhile,
    Converted,
}

/// An imported history record waiting to be matched against a real file.
/// Every field the import schema marks optional is nullable — the import
/// carries what the source record actually said, nothing synthesized.
/// Fixed-point integers only: durable state derives `Eq`, so any float
/// representation is the converter script's problem, never this crate's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ParkedRecord {
    pub status: ParkedStatus,
    /// Byte size of the source file when the record was decided: both the
    /// freshness-stamp probe and the input size for a converted record's
    /// summary.
    #[specta(type = Option<crate::JsNumber>)]
    pub size: Option<u64>,
    pub modified_ns: Option<crate::FileTimeNs>,
    pub video_codec: Option<VideoCodec>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[specta(type = Option<crate::JsNumber>)]
    pub duration_ms: Option<u64>,
    #[specta(type = Option<crate::JsNumber>)]
    pub output_size: Option<u64>,
    pub encoding_time: Option<DurationMs>,
    pub crf: Option<Crf>,
    pub vmaf: Option<VmafScore>,
    pub target: Option<VmafTarget>,
    pub requested_target: Option<VmafTarget>,
    pub floor_target: Option<VmafTarget>,
    /// When the source record was decided, falling back to the import
    /// instant when the source carried no timestamp.
    pub decided_at: UnixMillis,
}
