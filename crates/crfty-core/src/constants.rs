use crate::VmafTarget;

pub const DEFAULT_VMAF_TARGET: VmafTarget = VmafTarget(95);
pub const MIN_VMAF_FALLBACK_TARGET: VmafTarget = VmafTarget(90);
pub const VMAF_FALLBACK_STEP: u8 = 1;
pub const DEFAULT_ENCODING_PRESET: u8 = 6;
pub const DEFAULT_MAX_ENCODED_PERCENT_BASIS_POINTS: u32 = 8_000;
pub const DEFAULT_SAMPLE_DURATION_MS: u64 = 20_000;

/// Import sources may have recorded modification times at seconds precision;
/// matching a parked record against a real file tolerates one second of
/// drift.
pub const IMPORT_MTIME_TOLERANCE_NS: u64 = 1_000_000_000;

pub const CRF_FIXED_SCALE: u32 = 1_000;
pub const VMAF_SCORE_FIXED_SCALE: u16 = 100;
pub const MAX_VMAF_SCORE: u16 = 100;
pub const PERCENT_BASIS_POINTS_SCALE: u32 = 100;
pub const MAX_PERCENT_BASIS_POINTS: u32 = 10_000;
pub const MAX_ENCODING_PRESET: u8 = 13;
