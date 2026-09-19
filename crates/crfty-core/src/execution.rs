//! Claim-time execution composition. The reducer owns the only composition
//! of the base execution settings, the probed tool facts, and Settings into
//! the `ExecutionSettings` a claim freezes into its `JobSpec`. Every consumer
//! that asks what a claim would run with goes through [`compose_execution`],
//! so no projection can claim a result a claim would not reproduce.

use std::collections::BTreeSet;

use crate::{
    DecodeMode, DecodePreference, ExecutionSettings, HardwareDecoder, OverwriteDecision, Settings,
    ToolAvailability, ToolVerification, VideoCodec,
};

/// Codec-appropriate hardware decoders in preference order: CUVID before
/// QSV. Codecs without a hardware path have no candidates.
#[must_use]
pub fn decoder_candidates(codec: &VideoCodec) -> &'static [HardwareDecoder] {
    match codec {
        VideoCodec::H264 => &[HardwareDecoder::H264Cuvid, HardwareDecoder::H264Qsv],
        VideoCodec::Hevc => &[HardwareDecoder::HevcCuvid, HardwareDecoder::HevcQsv],
        VideoCodec::Vp9 => &[HardwareDecoder::Vp9Cuvid, HardwareDecoder::Vp9Qsv],
        VideoCodec::Av1 => &[HardwareDecoder::Av1Cuvid, HardwareDecoder::Av1Qsv],
        VideoCodec::Other(_) => &[],
    }
}

/// Why no claim can be composed from the current tool picture. Path-free by
/// construction so it can travel in rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionUnavailable {
    ToolsMissing,
    ToolsPending,
    ToolsFailed,
}

impl ExecutionUnavailable {
    #[must_use]
    pub const fn summary(self) -> &'static str {
        match self {
            Self::ToolsMissing => "media tools are not located",
            Self::ToolsPending => "media tools have not been verified",
            Self::ToolsFailed => "media tools failed verification",
        }
    }
}

/// Compose the execution a claim runs with. The verified revisions become
/// the profile's provenance, the item's overwrite decision resolves against
/// Settings, and the decode mode is the first codec-appropriate decoder the
/// probe found when hardware decode is enabled. `codec: None` (no observed
/// media facts) composes software decode.
pub fn compose_execution(
    base: &ExecutionSettings,
    tools: &ToolAvailability,
    settings: &Settings,
    overwrite: OverwriteDecision,
    codec: Option<&VideoCodec>,
) -> Result<ExecutionSettings, ExecutionUnavailable> {
    let (revisions, hardware_decoders) = match tools {
        ToolAvailability::Missing { .. } => return Err(ExecutionUnavailable::ToolsMissing),
        ToolAvailability::Located { verification, .. } => match verification {
            ToolVerification::Pending => return Err(ExecutionUnavailable::ToolsPending),
            ToolVerification::Failed(_) => return Err(ExecutionUnavailable::ToolsFailed),
            ToolVerification::Verified {
                revisions,
                hardware_decoders,
            } => (revisions, hardware_decoders),
        },
    };
    let mut execution = base.clone();
    execution.profile.ab_av1_revision = revisions.ab_av1.clone();
    execution.profile.ffmpeg_revision = revisions.ffmpeg.clone();
    execution.profile.encoder_revision = revisions.encoder.clone();
    execution.overwrite_existing = match overwrite {
        OverwriteDecision::FollowSettings => settings.output.overwrite_existing,
        OverwriteDecision::Allow => true,
        OverwriteDecision::Deny => false,
    };
    (execution.decode_preference, execution.profile.decode_mode) = if settings.hardware_decode {
        let decoder = select_hardware_decoder(codec, hardware_decoders);
        (
            DecodePreference::HardwarePreferred,
            decoder.map_or(DecodeMode::Software, DecodeMode::Hardware),
        )
    } else {
        (DecodePreference::SoftwareOnly, DecodeMode::Software)
    };
    Ok(execution)
}

fn select_hardware_decoder(
    codec: Option<&VideoCodec>,
    available: &BTreeSet<HardwareDecoder>,
) -> Option<HardwareDecoder> {
    codec.and_then(|codec| {
        decoder_candidates(codec)
            .iter()
            .copied()
            .find(|decoder| available.contains(decoder))
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        AnalysisProfile, LocatedTool, LocatedTools, ProbeFailure, ToolCapability, ToolRevisions,
        ToolSource,
    };

    fn located(verification: ToolVerification) -> ToolAvailability {
        ToolAvailability::Located {
            tools: LocatedTools {
                ffmpeg: LocatedTool {
                    source: ToolSource::SearchPath,
                    path: PathBuf::from("/usr/bin/ffmpeg"),
                },
                ffprobe: LocatedTool {
                    source: ToolSource::SearchPath,
                    path: PathBuf::from("/usr/bin/ffprobe"),
                },
            },
            verification,
        }
    }

    fn verified(decoders: &[HardwareDecoder]) -> ToolAvailability {
        located(ToolVerification::Verified {
            revisions: ToolRevisions {
                ab_av1: "ab".to_owned(),
                ffmpeg: "ff".to_owned(),
                encoder: "enc".to_owned(),
            },
            hardware_decoders: decoders.iter().copied().collect(),
        })
    }

    fn base() -> ExecutionSettings {
        ExecutionSettings::production(AnalysisProfile::production(), false)
    }

    #[test]
    fn unverified_tools_cannot_compose_a_claim() {
        let settings = Settings::default();
        let cases = [
            (
                ToolAvailability::default(),
                ExecutionUnavailable::ToolsMissing,
            ),
            (
                located(ToolVerification::Pending),
                ExecutionUnavailable::ToolsPending,
            ),
            (
                located(ToolVerification::Failed(ProbeFailure::TimedOut {
                    capability: ToolCapability::VmafFilter,
                })),
                ExecutionUnavailable::ToolsFailed,
            ),
        ];
        for (tools, expected) in cases {
            assert_eq!(
                compose_execution(
                    &base(),
                    &tools,
                    &settings,
                    OverwriteDecision::FollowSettings,
                    Some(&VideoCodec::H264)
                ),
                Err(expected)
            );
        }
    }

    #[test]
    fn verified_revisions_become_the_profile_provenance() {
        let composed = compose_execution(
            &base(),
            &verified(&[]),
            &Settings::default(),
            OverwriteDecision::FollowSettings,
            None,
        )
        .ok();
        let Some(composed) = composed else {
            panic!("expected composed execution");
        };
        assert_eq!(composed.profile.ab_av1_revision, "ab");
        assert_eq!(composed.profile.ffmpeg_revision, "ff");
        assert_eq!(composed.profile.encoder_revision, "enc");
        assert_eq!(composed.validate(), Ok(()));
    }

    #[test]
    fn overwrite_resolves_from_the_item_before_settings() {
        let mut settings = Settings::default();
        settings.output.overwrite_existing = true;
        let cases = [
            (OverwriteDecision::FollowSettings, true, true),
            (OverwriteDecision::FollowSettings, false, false),
            (OverwriteDecision::Allow, false, true),
            (OverwriteDecision::Deny, true, false),
        ];
        for (overwrite, from_settings, expected) in cases {
            settings.output.overwrite_existing = from_settings;
            let composed =
                compose_execution(&base(), &verified(&[]), &settings, overwrite, None).ok();
            assert_eq!(
                composed.map(|execution| execution.overwrite_existing),
                Some(expected)
            );
        }
    }

    #[test]
    fn hardware_decode_picks_the_first_probed_candidate_for_the_codec() {
        let settings = Settings {
            hardware_decode: true,
            ..Settings::default()
        };
        let cases: [(&[HardwareDecoder], Option<&VideoCodec>, DecodeMode); 6] = [
            (
                &[HardwareDecoder::H264Qsv, HardwareDecoder::H264Cuvid],
                Some(&VideoCodec::H264),
                DecodeMode::Hardware(HardwareDecoder::H264Cuvid),
            ),
            (
                &[HardwareDecoder::H264Qsv],
                Some(&VideoCodec::H264),
                DecodeMode::Hardware(HardwareDecoder::H264Qsv),
            ),
            (
                &[HardwareDecoder::H264Cuvid],
                Some(&VideoCodec::Hevc),
                DecodeMode::Software,
            ),
            (&[], Some(&VideoCodec::H264), DecodeMode::Software),
            (
                &[HardwareDecoder::H264Cuvid],
                Some(&VideoCodec::Other("prores".to_owned())),
                DecodeMode::Software,
            ),
            (&[HardwareDecoder::H264Cuvid], None, DecodeMode::Software),
        ];
        for (decoders, codec, expected) in cases {
            let composed = compose_execution(
                &base(),
                &verified(decoders),
                &settings,
                OverwriteDecision::FollowSettings,
                codec,
            )
            .ok();
            assert_eq!(
                composed
                    .as_ref()
                    .map(|execution| execution.decode_preference),
                Some(DecodePreference::HardwarePreferred)
            );
            assert_eq!(
                composed.map(|execution| execution.profile.decode_mode),
                Some(expected)
            );
        }
    }

    #[test]
    fn software_only_ignores_probed_decoders() {
        let settings = Settings {
            hardware_decode: false,
            ..Settings::default()
        };
        let composed = compose_execution(
            &base(),
            &verified(&[HardwareDecoder::H264Cuvid]),
            &settings,
            OverwriteDecision::FollowSettings,
            Some(&VideoCodec::H264),
        )
        .ok();
        assert_eq!(
            composed
                .as_ref()
                .map(|execution| execution.decode_preference),
            Some(DecodePreference::SoftwareOnly)
        );
        assert_eq!(
            composed.map(|execution| execution.profile.decode_mode),
            Some(DecodeMode::Software)
        );
    }
}
