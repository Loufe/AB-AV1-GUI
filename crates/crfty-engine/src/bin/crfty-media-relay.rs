//! Relay an unmodified CLI's bare media-tool invocation to the selected executable.

use std::{
    env,
    path::Path,
    process::{Command, ExitStatus},
};

fn main() {
    match relay() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("media tool relay failed: {error}");
            std::process::exit(1);
        }
    }
}

fn relay() -> Result<ExitStatus, Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let invocation = arguments.next().ok_or("missing invocation")?;
    let name = Path::new(&invocation)
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("invalid relay name")?;
    let variable = match name.to_ascii_lowercase().as_str() {
        "ffmpeg" => "CRFTY_RELAY_FFMPEG",
        "ffprobe" => "CRFTY_RELAY_FFPROBE",
        _ => return Err("unexpected relay invocation".into()),
    };
    let tool = env::var_os(variable).ok_or("missing relay target")?;
    if !Path::new(&tool).is_absolute() {
        return Err("relay target must be absolute".into());
    }
    let mut command = Command::new(tool);
    command.args(arguments);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.status().map_err(Into::into)
}
