use std::{collections::BTreeSet, path::PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_OUTPUT_SUFFIX: &str = "_av1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoExtension {
    Mp4,
    Mkv,
    Avi,
    Wmv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultOutputMode {
    Replace,
    Suffix,
    SeparateFolder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSettings {
    pub default_mode: DefaultOutputMode,
    pub suffix: String,
    pub separate_folder: Option<PathBuf>,
    pub overwrite_existing: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacySettings {
    pub anonymize_logs: bool,
    pub anonymize_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub last_input_folder: Option<PathBuf>,
    pub scan_extensions: BTreeSet<VideoExtension>,
    pub output: OutputSettings,
    pub hardware_decode: bool,
    pub privacy: PrivacySettings,
    pub log_folder: Option<PathBuf>,
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
        Ok(())
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
        }
    }
}
