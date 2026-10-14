use std::{collections::BTreeSet, path::PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_OUTPUT_SUFFIX: &str = "_av1";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum VideoExtension {
    Mp4,
    Mkv,
    Avi,
    Wmv,
}

impl VideoExtension {
    /// The file extension this variant matches: lowercase, no dot. Matching
    /// is the scanner's job and is case-insensitive.
    #[must_use]
    pub const fn as_extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Avi => "avi",
            Self::Wmv => "wmv",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DefaultOutputMode {
    Replace,
    Suffix,
    SeparateFolder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct OutputSettings {
    pub default_mode: DefaultOutputMode,
    pub suffix: String,
    pub separate_folder: Option<PathBuf>,
    pub overwrite_existing: bool,
}

impl OutputSettings {
    /// Resolve the committed default for an enqueue, retaining the originating
    /// folder only when the output should mirror a discovered source tree.
    pub fn target(
        &self,
        source_root: Option<PathBuf>,
    ) -> Result<crate::OutputTarget, &'static str> {
        match self.default_mode {
            DefaultOutputMode::Replace => Ok(crate::OutputTarget::Replace),
            DefaultOutputMode::Suffix => Ok(crate::OutputTarget::Suffix {
                suffix: self.suffix.clone(),
            }),
            DefaultOutputMode::SeparateFolder => Ok(crate::OutputTarget::SeparateFolder {
                directory: self
                    .separate_folder
                    .clone()
                    .ok_or("default separate output folder is required")?,
                source_root,
            }),
        }
    }
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            default_mode: DefaultOutputMode::Replace,
            suffix: DEFAULT_OUTPUT_SUFFIX.to_owned(),
            separate_folder: None,
            overwrite_existing: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct PrivacySettings {
    pub anonymize_logs: bool,
    pub anonymize_history: bool,
}

/// Operator-configured locations of the external media tools. A configured
/// path is an explicit choice: discovery uses it as given and fails closed
/// when it does not name a file, never falling through to the search path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct ToolPathSettings {
    pub ab_av1: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
}

impl ToolPathSettings {
    fn validate(&self) -> Result<(), &'static str> {
        if self
            .ab_av1
            .as_deref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err("configured ab-av1 path must be absolute");
        }
        if self
            .ffmpeg
            .as_deref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err("configured ffmpeg path must be absolute");
        }
        if self
            .ffprobe
            .as_deref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err("configured ffprobe path must be absolute");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub last_input_folder: Option<PathBuf>,
    pub scan_extensions: BTreeSet<VideoExtension>,
    pub output: OutputSettings,
    pub hardware_decode: bool,
    pub privacy: PrivacySettings,
    pub log_folder: Option<PathBuf>,
    pub tools: ToolPathSettings,
}

impl Settings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.output.default_mode == DefaultOutputMode::Suffix
            && self.output.suffix.trim().is_empty()
        {
            return Err("default output suffix must not be empty in suffix mode");
        }
        if self.output.default_mode == DefaultOutputMode::SeparateFolder
            && self
                .output
                .separate_folder
                .as_ref()
                .is_none_or(|path| path.as_os_str().is_empty())
        {
            return Err("default separate output folder is required in separate-folder mode");
        }
        self.tools.validate()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_input_folder: None,
            scan_extensions: [
                VideoExtension::Mp4,
                VideoExtension::Mkv,
                VideoExtension::Avi,
                VideoExtension::Wmv,
            ]
            .into_iter()
            .collect(),
            output: OutputSettings::default(),
            hardware_decode: true,
            privacy: PrivacySettings::default(),
            log_folder: None,
            tools: ToolPathSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputTarget;

    #[test]
    fn enqueue_target_uses_only_the_selected_default_and_source_root() -> Result<(), &'static str> {
        let source = PathBuf::from("source");
        let mut output = OutputSettings::default();
        assert_eq!(output.target(Some(source.clone()))?, OutputTarget::Replace);
        output.default_mode = DefaultOutputMode::Suffix;
        output.suffix = "_encoded".to_owned();
        assert_eq!(
            output.target(Some(source.clone()))?,
            OutputTarget::Suffix {
                suffix: "_encoded".to_owned()
            }
        );
        output.default_mode = DefaultOutputMode::SeparateFolder;
        assert!(output.target(None).is_err());
        output.separate_folder = Some("destination".into());
        for root in [None, Some(source)] {
            assert_eq!(
                output.target(root.clone())?,
                OutputTarget::SeparateFolder {
                    directory: "destination".into(),
                    source_root: root,
                }
            );
        }
        Ok(())
    }
}
