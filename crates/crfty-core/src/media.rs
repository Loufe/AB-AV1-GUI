use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AnalysisProfile, AnalysisResult, ContentKey, VmafTarget};

const HALF_ROTATION_DEGREES: i16 = 180;
const QUARTER_ROTATION_DEGREES: i16 = 90;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
pub struct PathHash(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FileStamp {
    #[specta(type = crate::JsNumber)]
    pub size: u64,
    #[specta(type = Option<crate::JsNumber>)]
    pub modified_ns: Option<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PathBinding {
    pub stamp: FileStamp,
    pub content_key: ContentKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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
pub struct VideoMeta {
    pub codec: VideoCodec,
    pub container: MediaContainer,
    pub width: u32,
    pub height: u32,
    pub rotation_degrees: i16,
    #[specta(type = crate::JsNumber)]
    pub duration_ms: u64,
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

/// The profile-keyed index crosses serde as an entry list: JSON map keys must
/// be strings, and `AnalysisProfile` is a multi-field struct.
pub type AnalysisIndexEntries = Vec<(AnalysisProfile, BTreeMap<VmafTarget, AnalysisResult>)>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FileRecord {
    pub metadata: VideoMeta,
    #[serde(with = "analysis_index")]
    #[specta(type = AnalysisIndexEntries)]
    pub analyses: BTreeMap<AnalysisProfile, BTreeMap<VmafTarget, AnalysisResult>>,
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
        }
    }

    pub(crate) fn record_analysis(&mut self, result: AnalysisResult) {
        self.analyses
            .entry(result.profile.clone())
            .or_default()
            .insert(result.successful_target, result);
    }
}
