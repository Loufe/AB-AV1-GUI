use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AnalysisProfile, AnalysisResult, ContentKey, VmafTarget};

const HALF_ROTATION_DEGREES: i16 = 180;
const QUARTER_ROTATION_DEGREES: i16 = 90;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PathHash(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub size: u64,
    pub modified_ns: Option<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathBinding {
    pub stamp: FileStamp,
    pub content_key: ContentKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    Av1,
    H264,
    Hevc,
    Vp9,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaContainer {
    Matroska,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoMeta {
    pub codec: VideoCodec,
    pub container: MediaContainer,
    pub width: u32,
    pub height: u32,
    pub rotation_degrees: i16,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaObservation {
    pub path_hash: PathHash,
    pub binding: PathBinding,
    pub metadata: VideoMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub metadata: VideoMeta,
    pub analyses: BTreeMap<AnalysisProfile, BTreeMap<VmafTarget, AnalysisResult>>,
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
