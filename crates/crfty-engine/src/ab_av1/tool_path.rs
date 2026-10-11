//! Keep explicit tool selection when the official CLI invokes bare tool names.

use crate::tools::MediaTools;
use std::{path::Path, process::Command};

pub(super) fn configure(
    command: &mut Command,
    tools: &MediaTools,
    directory: &Path,
) -> Result<(), String> {
    for (name, tool) in [("ffmpeg", &tools.ffmpeg), ("ffprobe", &tools.ffprobe)] {
        #[cfg(unix)]
        std::os::unix::fs::symlink(tool, directory.join(name)).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            check_windows_precedence(&tools.ab_av1, name, tool)?;
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            let parent = executable.parent().ok_or("missing executable directory")?;
            let relay = if parent.file_name().is_some_and(|name| name == "deps") {
                parent
                    .parent()
                    .ok_or("missing Cargo binary directory")?
                    .join("crfty-media-relay.exe")
            } else {
                parent.join("crfty-media-relay.exe")
            };
            std::fs::copy(relay, directory.join(format!("{name}.exe")))
                .map_err(|e| format!("cannot prepare media relay: {e}"))?;
        }
    }
    let paths = std::iter::once(directory.to_path_buf()).chain(
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>()),
    );
    command
        .env(
            "PATH",
            std::env::join_paths(paths).map_err(|e| e.to_string())?,
        )
        .env("CRFTY_RELAY_FFMPEG", &tools.ffmpeg)
        .env("CRFTY_RELAY_FFPROBE", &tools.ffprobe);
    Ok(())
}

#[cfg(windows)]
fn check_windows_precedence(ab: &Path, name: &str, selected: &Path) -> Result<(), String> {
    let root = std::env::var_os("SystemRoot").ok_or("Windows system directory is unavailable")?;
    let root = std::path::PathBuf::from(root);
    let directories = [
        ab.parent().ok_or("missing ab-av1 directory")?.to_path_buf(),
        root.join("System32"),
        root.join("SysWOW64"),
        root,
    ];
    let selected = selected.canonicalize().map_err(|e| e.to_string())?;
    for directory in directories {
        let candidate = directory.join(format!("{name}.exe"));
        if candidate.is_file() && candidate.canonicalize().map_err(|e| e.to_string())? != selected {
            return Err(format!(
                "ab-av1 would select another {name} before PATH; remove that conflicting installation or select its executable in Settings"
            ));
        }
    }
    Ok(())
}
