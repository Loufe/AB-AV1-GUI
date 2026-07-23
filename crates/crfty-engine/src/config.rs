use std::{
    ffi::OsString,
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crfty_core::Settings;
use tempfile::NamedTempFile;

use crate::filesystem::{parent_directory, sync_parent};

#[derive(Debug)]
pub(crate) struct ConfigError {
    context: &'static str,
    source: io::Error,
}

impl ConfigError {
    fn new(context: &'static str, source: io::Error) -> Self {
        Self { context, source }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedConfig {
    pub settings: Settings,
    pub quarantined: Option<PathBuf>,
}

pub(crate) struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<LoadedConfig, ConfigError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LoadedConfig {
                    settings: Settings::default(),
                    quarantined: None,
                });
            }
            Err(error) => return Err(ConfigError::new("failed to read config", error)),
        };
        let parsed = serde_json::from_slice::<Settings>(&bytes)
            .map_err(|error| error.to_string())
            .and_then(|settings| {
                settings
                    .validate()
                    .map(|()| settings)
                    .map_err(str::to_owned)
            });
        match parsed {
            Ok(settings) => Ok(LoadedConfig {
                settings,
                quarantined: None,
            }),
            Err(_reason) => {
                let quarantined = invalid_path(&self.path)?;
                fs::rename(&self.path, &quarantined).map_err(|error| {
                    ConfigError::new("failed to quarantine invalid config", error)
                })?;
                sync_parent(&self.path).map_err(|error| {
                    ConfigError::new("failed to synchronize quarantined config", error)
                })?;
                Ok(LoadedConfig {
                    settings: Settings::default(),
                    quarantined: Some(quarantined),
                })
            }
        }
    }

    pub fn write(&self, settings: &Settings) -> Result<(), ConfigError> {
        settings.validate().map_err(|reason| {
            ConfigError::new(
                "refused to write invalid config",
                io::Error::new(io::ErrorKind::InvalidInput, reason),
            )
        })?;
        let parent = parent_directory(&self.path);
        fs::create_dir_all(parent)
            .map_err(|error| ConfigError::new("failed to create config directory", error))?;
        let mut encoded = serde_json::to_vec_pretty(settings).map_err(|error| {
            ConfigError::new(
                "failed to encode config",
                io::Error::new(io::ErrorKind::InvalidData, error),
            )
        })?;
        encoded.push(b'\n');
        let mut temporary = NamedTempFile::new_in(parent)
            .map_err(|error| ConfigError::new("failed to create temporary config", error))?;
        temporary
            .write_all(&encoded)
            .map_err(|error| ConfigError::new("failed to write temporary config", error))?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|error| ConfigError::new("failed to synchronize temporary config", error))?;
        temporary.persist(&self.path).map_err(|error| {
            ConfigError::new("failed to replace config atomically", error.error)
        })?;
        sync_parent(&self.path)
            .map_err(|error| ConfigError::new("failed to synchronize config directory", error))
    }
}

fn invalid_path(path: &Path) -> Result<PathBuf, ConfigError> {
    let file_name = path.file_name().ok_or_else(|| {
        ConfigError::new(
            "config path has no file name",
            io::Error::new(io::ErrorKind::InvalidInput, "missing file name"),
        )
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            ConfigError::new(
                "system clock is before the Unix epoch",
                io::Error::other(error),
            )
        })?
        .as_nanos();
    let mut invalid_name = OsString::from(file_name);
    invalid_name.push(format!(".invalid-{timestamp}"));
    Ok(path.with_file_name(invalid_name))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crfty_core::{DefaultOutputMode, Settings};
    use tempfile::tempdir;

    use super::ConfigStore;

    #[test]
    fn missing_config_uses_typed_defaults_and_valid_config_round_trips() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.json");
        let store = ConfigStore::new(&path);
        assert_eq!(
            store.load().expect("missing config").settings,
            Settings::default()
        );

        let mut settings = Settings::default();
        settings.output.default_mode = DefaultOutputMode::Suffix;
        settings.output.suffix = "_small".to_owned();
        settings.hardware_decode = false;
        store.write(&settings).expect("atomic config write");
        assert_eq!(store.load().expect("written config").settings, settings);
    }

    #[test]
    fn unknown_or_invalid_config_is_quarantined_whole() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.json");
        fs::write(&path, br#"{"unknown":true}"#).expect("invalid config fixture");
        let loaded = ConfigStore::new(&path).load().expect("quarantine config");
        assert_eq!(loaded.settings, Settings::default());
        let quarantined = loaded.quarantined.expect("quarantined path");
        assert!(!path.exists());
        assert!(quarantined.exists());
        assert!(
            quarantined
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("config.json.invalid-"))
        );
    }

    #[test]
    fn atomic_replacement_never_merges_old_fields() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("config.json");
        let store = ConfigStore::new(&path);
        let first = Settings {
            last_input_folder: Some(directory.path().join("first")),
            ..Settings::default()
        };
        store.write(&first).expect("first write");

        let mut second = Settings::default();
        second.scan_extensions.clear();
        store.write(&second).expect("replacement write");
        assert_eq!(store.load().expect("replacement config").settings, second);
    }
}
