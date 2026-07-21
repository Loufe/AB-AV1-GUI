use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::Metadata,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use blake2::{
    Blake2bVar,
    digest::{Update, VariableOutput},
};
use crfty_core::{
    ArtifactIdentity, AudioCodec, AudioStreamMeta, ContentKey, DecodeMode, DecodePreference,
    DestructiveIdentity, FileSystemId, FileTimeNs, HardwareDecoder, MediaContainer,
    MediaObservation, ObservationStability, PathBinding, PathHash, TimestampReliability,
    VideoCodec, VideoMeta, observation_stability,
};
use serde::Deserialize;

use crate::process;

const CONTENT_KEY_SCHEMA: &[u8] = b"ck1";
const CONTENT_KEY_TEXT_PREFIX: &str = "ck1:";
const PATH_HASH_SCHEMA: &[u8] = b"ph2";
const PATH_HASH_TEXT_PREFIX: &str = "ph2:";
const DIGEST_BYTES: usize = 16;
const HEX_CHARACTERS_PER_BYTE: usize = 2;
const SAMPLE_ALIGNMENT_BYTES: u64 = 4 * 1024;
const WHOLE_FILE_LIMIT: u64 = 4 * 1024 * 1024;
const EDGE_SAMPLE: usize = 256 * 1024;
const MIDDLE_SAMPLE: usize = 64 * 1024;
const QUARTER_SAMPLE_NUMERATORS: [u64; 3] = [1, 2, 3];
const SAMPLE_QUARTERS: u64 = 4;
const MILLISECONDS_PER_SECOND: f64 = 1_000.0;
const FULL_ROTATION_DEGREES: i16 = 360;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;
const RECENT_MTIME_WINDOW_NS: u64 = 2 * NANOSECONDS_PER_SECOND;

#[derive(Debug, Clone)]
pub struct MediaInspector {
    ffprobe: PathBuf,
}

pub struct DecodeResolver {
    ffmpeg: PathBuf,
    availability: BTreeMap<HardwareDecoder, bool>,
}

impl DecodeResolver {
    pub fn new(ffmpeg: PathBuf) -> Self {
        Self {
            ffmpeg,
            availability: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn resolve(&mut self, preference: DecodePreference, codec: &VideoCodec) -> DecodeMode {
        if preference == DecodePreference::SoftwareOnly {
            return DecodeMode::Software;
        }
        for decoder in decoder_candidates(codec) {
            let available = if let Some(available) = self.availability.get(decoder) {
                *available
            } else {
                let available = decoder_is_available(&self.ffmpeg, decoder);
                self.availability.insert(*decoder, available);
                available
            };
            if available {
                return DecodeMode::Hardware(*decoder);
            }
        }
        DecodeMode::Software
    }
}

impl MediaInspector {
    pub fn new(ffprobe: PathBuf) -> Self {
        Self { ffprobe }
    }

    pub fn observe(&self, path: &Path) -> io::Result<MediaObservation> {
        let (metadata, identity) = self.inspect(path)?;
        Ok(MediaObservation {
            path_hash: path_hash(path)?,
            binding: PathBinding {
                identity: identity.destructive,
                content_key: identity.content_key,
            },
            metadata,
        })
    }

    pub fn inspect_artifact(&self, path: &Path) -> io::Result<ArtifactIdentity> {
        self.inspect(path).map(|(_, identity)| identity)
    }

    pub fn verify_av1(&self, path: &Path, minimum_size: u64) -> io::Result<ArtifactIdentity> {
        let metadata = std::fs::metadata(path)?;
        if metadata.len() < minimum_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "output is too small to be a valid video",
            ));
        }
        let (video, identity) = self.inspect(path)?;
        if video.codec != VideoCodec::Av1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "output video codec is not AV1",
            ));
        }
        Ok(identity)
    }

    fn inspect(&self, path: &Path) -> io::Result<(VideoMeta, ArtifactIdentity)> {
        let before_probe = destructive_identity(path)?;
        let metadata = self.probe(path, before_probe.size)?;
        let after_probe = destructive_identity(path)?;
        let identity = sampled_identity(path, &metadata)?;
        if observation_stability(&before_probe, &after_probe, &identity.destructive)
            != ObservationStability::Stable
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "artifact changed while it was probed and identified",
            ));
        }
        Ok((metadata, identity))
    }

    fn probe(&self, path: &Path, size_bytes: u64) -> io::Result<VideoMeta> {
        let mut command = Command::new(&self.ffprobe);
        command
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,codec_name,width,height,channels:stream_tags=rotate:stream_side_data=rotation:format=duration,format_name",
                "-of",
                "json",
            ])
            .arg(path);
        let output = process::output(&mut command)?;
        if !output.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ffprobe rejected media artifact",
            ));
        }
        let probe: ProbeDocument = serde_json::from_slice(&output.stdout)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        // All streams arrive in one probe; classify by codec_type. The first
        // video stream matches the previous `-select_streams v:0` behavior.
        let mut video = None;
        let mut audio = Vec::new();
        let mut subtitle_count = 0_u32;
        for stream in probe.streams {
            match stream.codec_type.as_str() {
                "video" if video.is_none() => video = Some(stream),
                "audio" => audio.push(AudioStreamMeta {
                    codec: audio_codec(&stream.codec_name),
                    channels: stream.channels,
                }),
                "subtitle" => subtitle_count = subtitle_count.saturating_add(1),
                _ => {}
            }
        }
        let stream = video.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "media has no video stream")
        })?;
        let format = probe
            .format
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "media format is missing"))?;
        let duration = format
            .duration
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "duration is missing"))?
            .parse::<f64>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if !duration.is_finite() || duration <= 0.0 || stream.width == 0 || stream.height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "media dimensions and duration must be positive and finite",
            ));
        }
        let rotation = stream
            .side_data_list
            .iter()
            .find_map(|data| data.rotation)
            .or_else(|| stream.tags.and_then(|tags| tags.rotate))
            .unwrap_or_default()
            .rem_euclid(FULL_ROTATION_DEGREES);
        let codec_name = stream.codec_name.to_ascii_lowercase();
        let codec = match codec_name.as_str() {
            "av1" => VideoCodec::Av1,
            "h264" => VideoCodec::H264,
            "hevc" | "h265" => VideoCodec::Hevc,
            "vp9" => VideoCodec::Vp9,
            _ => VideoCodec::Other(codec_name),
        };
        let container = if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(OsStr::new("mkv")))
        {
            MediaContainer::Matroska
        } else {
            MediaContainer::Other(format.format_name.unwrap_or_default())
        };
        Ok(VideoMeta {
            codec,
            container,
            width: stream.width,
            height: stream.height,
            rotation_degrees: rotation,
            duration_ms: (duration * MILLISECONDS_PER_SECOND).round() as u64,
            size_bytes,
            audio,
            subtitle_count,
        })
    }
}

fn audio_codec(codec_name: &str) -> AudioCodec {
    let codec_name = codec_name.to_ascii_lowercase();
    match codec_name.as_str() {
        "aac" => AudioCodec::Aac,
        "ac3" => AudioCodec::Ac3,
        "eac3" => AudioCodec::Eac3,
        "dts" => AudioCodec::Dts,
        "opus" => AudioCodec::Opus,
        "flac" => AudioCodec::Flac,
        "mp3" => AudioCodec::Mp3,
        _ => AudioCodec::Other(codec_name),
    }
}

#[must_use]
pub const fn decoder_name(decoder: HardwareDecoder) -> &'static str {
    match decoder {
        HardwareDecoder::H264Cuvid => "h264_cuvid",
        HardwareDecoder::H264Qsv => "h264_qsv",
        HardwareDecoder::HevcCuvid => "hevc_cuvid",
        HardwareDecoder::HevcQsv => "hevc_qsv",
        HardwareDecoder::Vp9Cuvid => "vp9_cuvid",
        HardwareDecoder::Vp9Qsv => "vp9_qsv",
        HardwareDecoder::Av1Cuvid => "av1_cuvid",
        HardwareDecoder::Av1Qsv => "av1_qsv",
    }
}

fn decoder_candidates(codec: &VideoCodec) -> &'static [HardwareDecoder] {
    match codec {
        VideoCodec::H264 => &[HardwareDecoder::H264Cuvid, HardwareDecoder::H264Qsv],
        VideoCodec::Hevc => &[HardwareDecoder::HevcCuvid, HardwareDecoder::HevcQsv],
        VideoCodec::Vp9 => &[HardwareDecoder::Vp9Cuvid, HardwareDecoder::Vp9Qsv],
        VideoCodec::Av1 => &[HardwareDecoder::Av1Cuvid, HardwareDecoder::Av1Qsv],
        VideoCodec::Other(_) => &[],
    }
}

fn decoder_is_available(ffmpeg: &Path, decoder: &HardwareDecoder) -> bool {
    let mut command = Command::new(ffmpeg);
    command
        .args(["-v", "error", "-hide_banner", "-h"])
        .arg(format!("decoder={}", decoder_name(*decoder)));
    process::status(&mut command).is_ok_and(|status| status.success())
}

pub(crate) fn destructive_identity(path: &Path) -> io::Result<DestructiveIdentity> {
    let metadata = std::fs::metadata(path)?;
    identity_from_metadata(path, &metadata)
}

/// Conservative metadata-cache judgment. Missing timestamps are unknown;
/// exact-second values are treated as a coarse-filesystem signature; and a
/// timestamp within the write-race window (including a future timestamp) is
/// recent. False negatives only cause re-observation, never stale reuse.
pub(crate) fn timestamp_reliability(
    identity: &DestructiveIdentity,
    now: SystemTime,
) -> TimestampReliability {
    let Some(modified) = identity.modified_ns else {
        return TimestampReliability::Unknown;
    };
    let Some(now_ns) = now
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
    else {
        return TimestampReliability::Unknown;
    };
    if modified.0.is_multiple_of(NANOSECONDS_PER_SECOND)
        || modified.0 >= now_ns.saturating_sub(RECENT_MTIME_WINDOW_NS)
    {
        TimestampReliability::CoarseOrRecent
    } else {
        TimestampReliability::Reliable
    }
}

fn sampled_identity(path: &Path, header: &VideoMeta) -> io::Result<ArtifactIdentity> {
    let before = std::fs::metadata(path)?;
    let before_identity = identity_from_metadata(path, &before)?;
    let mut file = std::fs::File::open(path)?;
    let mut digest = Blake2bVar::new(DIGEST_BYTES)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    digest.update(CONTENT_KEY_SCHEMA);
    digest.update(&before.len().to_le_bytes());
    digest.update(&header.duration_ms.to_le_bytes());
    let codec = codec_header(&header.codec);
    digest.update(&(codec.len() as u64).to_le_bytes());
    digest.update(codec.as_bytes());
    digest.update(&header.width.to_le_bytes());
    digest.update(&header.height.to_le_bytes());

    if before.len() <= WHOLE_FILE_LIMIT {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        digest.update(&bytes);
    } else {
        hash_region(&mut digest, &mut file, 0, EDGE_SAMPLE)?;
        for numerator in QUARTER_SAMPLE_NUMERATORS {
            let raw = before.len().saturating_mul(numerator) / SAMPLE_QUARTERS;
            let offset = raw / SAMPLE_ALIGNMENT_BYTES * SAMPLE_ALIGNMENT_BYTES;
            hash_region(&mut digest, &mut file, offset, MIDDLE_SAMPLE)?;
        }
        hash_region(
            &mut digest,
            &mut file,
            before.len().saturating_sub(EDGE_SAMPLE as u64),
            EDGE_SAMPLE,
        )?;
    }
    let after = std::fs::metadata(path)?;
    let after_identity = identity_from_metadata(path, &after)?;
    if observation_stability(&before_identity, &before_identity, &after_identity)
        != ObservationStability::Stable
    {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "artifact changed while its identity was computed",
        ));
    }
    Ok(ArtifactIdentity {
        content_key: ContentKey(finalize_hex(digest, CONTENT_KEY_TEXT_PREFIX)?),
        destructive: after_identity,
    })
}

fn codec_header(codec: &VideoCodec) -> &str {
    match codec {
        VideoCodec::Av1 => "av1",
        VideoCodec::H264 => "h264",
        VideoCodec::Hevc => "hevc",
        VideoCodec::Vp9 => "vp9",
        VideoCodec::Other(name) => name,
    }
}

/// Hashes a canonical path's native representation. Display conversion and
/// case rewriting are deliberately absent: Unix hashes `OsStr` bytes and
/// Windows hashes wide units. Every `PathHash` in the system must come from
/// here so enqueue facts and claim-time observations agree.
pub fn path_hash(path: &Path) -> io::Result<PathHash> {
    let canonical = std::fs::canonicalize(path)?;
    hash_native_path(&canonical)
}

fn hash_native_path(path: &Path) -> io::Result<PathHash> {
    let mut digest = Blake2bVar::new(DIGEST_BYTES)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    digest.update(PATH_HASH_SCHEMA);
    update_native_path(&mut digest, path);
    Ok(PathHash(finalize_hex(digest, PATH_HASH_TEXT_PREFIX)?))
}

#[cfg(unix)]
fn update_native_path(digest: &mut Blake2bVar, path: &Path) {
    use std::os::unix::ffi::OsStrExt as _;

    digest.update(b"unix\0");
    digest.update(path.as_os_str().as_bytes());
}

#[cfg(windows)]
fn update_native_path(digest: &mut Blake2bVar, path: &Path) {
    use std::os::windows::ffi::OsStrExt as _;

    digest.update(b"windows\0");
    for unit in path.as_os_str().encode_wide() {
        digest.update(&unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
fn update_native_path(digest: &mut Blake2bVar, path: &Path) {
    digest.update(b"other\0");
    digest.update(path.as_os_str().as_encoded_bytes());
}

fn finalize_hex(digest: Blake2bVar, prefix: &str) -> io::Result<String> {
    let mut bytes = [0_u8; DIGEST_BYTES];
    digest
        .finalize_variable(&mut bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut encoded = String::with_capacity(prefix.len() + bytes.len() * HEX_CHARACTERS_PER_BYTE);
    encoded.push_str(prefix);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").map_err(|error| io::Error::other(error.to_string()))?;
    }
    Ok(encoded)
}

fn hash_region(
    digest: &mut Blake2bVar,
    file: &mut std::fs::File,
    offset: u64,
    length: usize,
) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)?;
    digest.update(&bytes);
    Ok(())
}

fn modified_ns(metadata: &Metadata) -> Option<FileTimeNs> {
    // Epoch-nanoseconds fit u64 until the year 2554; a clock past that (or
    // before the epoch) degrades to "no modification time" rather than lying.
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
        .map(FileTimeNs)
}

fn identity_from_metadata(path: &Path, metadata: &Metadata) -> io::Result<DestructiveIdentity> {
    let file_id = match file_id::get_file_id(path)? {
        file_id::FileId::Inode {
            device_id,
            inode_number,
        } => FileSystemId::Unix {
            device: device_id,
            inode: inode_number,
        },
        file_id::FileId::LowRes {
            volume_serial_number,
            file_index,
        } => FileSystemId::WindowsLowResolution {
            volume_serial: volume_serial_number,
            file_index,
        },
        file_id::FileId::HighRes {
            volume_serial_number,
            file_id,
        } => FileSystemId::WindowsHighResolution {
            volume_serial: volume_serial_number,
            file_id,
        },
    };
    Ok(DestructiveIdentity {
        file_id,
        size: metadata.len(),
        modified_ns: modified_ns(metadata),
    })
}

#[derive(Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    #[serde(default)]
    codec_type: String,
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    channels: u16,
    tags: Option<ProbeTags>,
    #[serde(default)]
    side_data_list: Vec<ProbeSideData>,
}

#[derive(Deserialize)]
struct ProbeTags {
    #[serde(default, deserialize_with = "deserialize_optional_i16")]
    rotate: Option<i16>,
}

#[derive(Deserialize)]
struct ProbeSideData {
    rotation: Option<i16>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
    format_name: Option<String>,
}

fn deserialize_optional_i16<'de, D>(deserializer: D) -> Result<Option<i16>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|text| text.parse::<i16>().map_err(serde::de::Error::custom))
        .transpose()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, UNIX_EPOCH},
    };

    use crfty_core::{
        DestructiveIdentity, FileSystemId, FileTimeNs, MediaContainer, TimestampReliability,
        VideoCodec, VideoMeta,
    };

    use super::{hash_native_path, path_hash, sampled_identity, timestamp_reliability};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn timestamped_identity(modified_ns: Option<u64>) -> DestructiveIdentity {
        DestructiveIdentity {
            file_id: FileSystemId::Unix {
                device: 1,
                inode: 2,
            },
            size: 3,
            modified_ns: modified_ns.map(FileTimeNs),
        }
    }

    #[test]
    fn timestamp_reliability_is_conservative_about_unknown_coarse_and_recent_values() {
        let now = UNIX_EPOCH + Duration::from_secs(10);
        assert_eq!(
            timestamp_reliability(&timestamped_identity(None), now),
            TimestampReliability::Unknown
        );
        assert_eq!(
            timestamp_reliability(&timestamped_identity(Some(7_000_000_000)), now),
            TimestampReliability::CoarseOrRecent
        );
        assert_eq!(
            timestamp_reliability(&timestamped_identity(Some(8_500_000_001)), now),
            TimestampReliability::CoarseOrRecent
        );
        assert_eq!(
            timestamp_reliability(&timestamped_identity(Some(11_000_000_001)), now),
            TimestampReliability::CoarseOrRecent
        );
        assert_eq!(
            timestamp_reliability(&timestamped_identity(Some(7_000_000_001)), now),
            TimestampReliability::Reliable
        );
    }

    #[cfg(unix)]
    #[test]
    fn ph2_distinguishes_non_unicode_paths_that_render_identically() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt as _, path::PathBuf};

        let first = PathBuf::from(OsString::from_vec(b"/videos/movie-\x80.mkv".to_vec()));
        let second = PathBuf::from(OsString::from_vec(b"/videos/movie-\x81.mkv".to_vec()));
        assert_eq!(first.to_string_lossy(), second.to_string_lossy());

        let first_hash = hash_native_path(&first).expect("first native path hash");
        let second_hash = hash_native_path(&second).expect("second native path hash");
        assert!(first_hash.0.starts_with("ph2:"));
        assert_ne!(first_hash, second_hash);
    }

    #[cfg(unix)]
    #[test]
    fn path_hash_preserves_non_unicode_identity_and_canonicalizes_symlinks() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt as _, os::unix::fs::symlink};

        let directory = test_directory("ph2-native");
        let first = directory.join(OsString::from_vec(b"movie-\x80.mkv".to_vec()));
        let second = directory.join(OsString::from_vec(b"movie-\x81.mkv".to_vec()));
        fs::write(&first, b"first").expect("first non-Unicode file");
        fs::write(&second, b"second").expect("second non-Unicode file");
        let link = directory.join("movie-link.mkv");
        symlink(&first, &link).expect("file symlink");

        let first_hash = path_hash(&first).expect("first path hash");
        assert_ne!(first_hash, path_hash(&second).expect("second path hash"));
        assert_eq!(first_hash, path_hash(&link).expect("symlink path hash"));
        fs::remove_dir_all(directory).expect("remove fixture directory");
    }

    #[cfg(windows)]
    #[test]
    fn ph2_hashes_windows_wide_units_without_lossy_conversion() {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt as _, path::PathBuf};

        let first = PathBuf::from(OsString::from_wide(&[0x0043, 0x003a, 0x005c, 0xd800]));
        let second = PathBuf::from(OsString::from_wide(&[0x0043, 0x003a, 0x005c, 0xd801]));
        assert_eq!(first.to_string_lossy(), second.to_string_lossy());
        assert_ne!(
            hash_native_path(&first).expect("first native path hash"),
            hash_native_path(&second).expect("second native path hash")
        );
    }

    #[test]
    fn ck1_matches_independent_golden_fixtures() {
        let directory = test_directory("ck1-golden");
        let small = directory.join("small.bin");
        fs::write(&small, b"test").expect("small fixture");
        let small_identity = sampled_identity(
            &small,
            &VideoMeta {
                codec: VideoCodec::Av1,
                container: MediaContainer::Other(String::new()),
                width: 16,
                height: 9,
                rotation_degrees: 0,
                duration_ms: 1_000,
                size_bytes: 4,
                audio: Vec::new(),
                subtitle_count: 0,
            },
        )
        .expect("small content key");
        assert_eq!(
            small_identity.content_key.0,
            "ck1:23ba2a7ea690c5618f0d5e2ef5413c3e"
        );

        let large = directory.join("large.bin");
        let bytes: Vec<_> = (0..(4 * 1024 * 1024 + 12_345))
            .map(|index| u8::try_from(index % 251).expect("bounded byte"))
            .collect();
        fs::write(&large, bytes).expect("large fixture");
        let large_identity = sampled_identity(
            &large,
            &VideoMeta {
                codec: VideoCodec::H264,
                container: MediaContainer::Other(String::new()),
                width: 1_920,
                height: 1_080,
                rotation_degrees: 0,
                duration_ms: 98_765,
                size_bytes: 4 * 1024 * 1024 + 12_345,
                audio: Vec::new(),
                subtitle_count: 0,
            },
        )
        .expect("large content key");
        assert_eq!(
            large_identity.content_key.0,
            "ck1:58736d4906c208fb16d7e4e3febba397"
        );
        fs::remove_dir_all(directory).expect("remove fixture directory");
    }

    fn test_directory(label: &str) -> std::path::PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crfty-{label}-{}-{sequence}", std::process::id()));
        fs::create_dir(&path).expect("fixture directory");
        path
    }
}
