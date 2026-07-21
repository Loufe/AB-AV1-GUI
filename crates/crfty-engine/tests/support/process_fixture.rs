#![forbid(unsafe_code)]

use std::{
    env,
    error::Error,
    fs,
    io::{self, Write},
    path::Path,
    process::{self, Command},
    thread,
    time::Duration,
};

const CHUNK_BYTES: usize = 4 * 1024;

fn main() {
    if let Err(error) = dispatch() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn dispatch() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os();
    let _executable = arguments.next();
    match arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
    {
        Some(command) if command == "emit" => {
            let stdout_bytes = parse_usize(arguments.next(), "stdout byte count")?;
            let stderr_bytes = parse_usize(arguments.next(), "stderr byte count")?;
            let exit_code = parse_i32(arguments.next(), "exit code")?;
            emit(stdout_bytes, stderr_bytes)?;
            process::exit(exit_code);
        }
        Some(command) if command == "sleep" => loop {
            thread::sleep(Duration::from_secs(1));
        },
        Some(command) if command == "spawn-heartbeat" => {
            let marker = arguments.next().ok_or("heartbeat path is missing")?;
            let _child = Command::new(env::current_exe()?)
                .arg("heartbeat")
                .arg(marker)
                .spawn()?;
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        Some(command) if command == "orphan-heartbeat" => {
            let marker = arguments.next().ok_or("heartbeat path is missing")?;
            let _child = Command::new(env::current_exe()?)
                .arg("heartbeat")
                .arg(&marker)
                .spawn()?;
            wait_for_file(Path::new(&marker))
        }
        Some(command) if command == "orphan-closed-pipes" => {
            let marker = arguments.next().ok_or("heartbeat path is missing")?;
            let _child = Command::new(env::current_exe()?)
                .arg("heartbeat")
                .arg(&marker)
                .stdin(process::Stdio::null())
                .stdout(process::Stdio::null())
                .stderr(process::Stdio::null())
                .spawn()?;
            wait_for_file(Path::new(&marker))?;
            Ok(())
        }
        Some(command) if command == "heartbeat" => {
            let marker = arguments.next().ok_or("heartbeat path is missing")?;
            heartbeat(Path::new(&marker))
        }
        _ => Err("unknown process fixture command".into()),
    }
}

fn parse_usize(value: Option<std::ffi::OsString>, label: &str) -> Result<usize, Box<dyn Error>> {
    value
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| format!("{label} is missing"))?
        .parse()
        .map_err(Into::into)
}

fn parse_i32(value: Option<std::ffi::OsString>, label: &str) -> Result<i32, Box<dyn Error>> {
    value
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| format!("{label} is missing"))?
        .parse()
        .map_err(Into::into)
}

fn emit(stdout_bytes: usize, stderr_bytes: usize) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let mut stdout_offset = 0_usize;
    let mut stderr_offset = 0_usize;
    while stdout_offset < stdout_bytes || stderr_offset < stderr_bytes {
        let stdout_count = (stdout_bytes - stdout_offset).min(CHUNK_BYTES);
        if stdout_count > 0 {
            write_pattern(&mut stdout, stdout_offset, stdout_count, b'a')?;
            stdout.flush()?;
            stdout_offset += stdout_count;
        }
        let stderr_count = (stderr_bytes - stderr_offset).min(CHUNK_BYTES);
        if stderr_count > 0 {
            write_pattern(&mut stderr, stderr_offset, stderr_count, b'A')?;
            stderr.flush()?;
            stderr_offset += stderr_count;
        }
    }
    Ok(())
}

fn write_pattern(writer: &mut impl Write, offset: usize, count: usize, base: u8) -> io::Result<()> {
    let bytes: Vec<u8> = (offset..offset + count)
        .map(|position| base + u8::try_from(position % 26).unwrap_or_default())
        .collect();
    writer.write_all(&bytes)
}

fn heartbeat(path: &Path) -> Result<(), Box<dyn Error>> {
    let mut counter = 0_u64;
    loop {
        fs::write(path, counter.to_string())?;
        counter = counter.wrapping_add(1);
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_file(path: &Path) -> Result<(), Box<dyn Error>> {
    for _attempt in 0..500 {
        if path.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(format!("heartbeat fixture did not create {}", path.display()).into())
}
