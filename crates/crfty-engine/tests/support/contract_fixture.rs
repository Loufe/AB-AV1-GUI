#![forbid(unsafe_code)]

use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process, thread,
    time::Duration,
};

use crfty_engine::ab_av1::{AdapterError, EncodeRequest, JobHandle, start_encode};

fn main() {
    if let Err(error) = dispatch() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn dispatch() -> Result<(), Box<dyn Error>> {
    let executable = env::current_exe()?;
    let stem = executable
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if stem.eq_ignore_ascii_case("ffprobe") {
        return fake_ffprobe();
    }
    if stem.eq_ignore_ascii_case("ffmpeg") {
        return fake_ffmpeg();
    }

    let mut arguments = env::args_os();
    let _executable = arguments.next();
    match arguments.next().as_deref() {
        Some(argument) if argument == "run" => run_contract(arguments),
        _ => Err("expected: crfty-contract-fixture run INPUT OUTPUT-DIR".into()),
    }
}

fn fake_ffprobe() -> Result<(), Box<dyn Error>> {
    const PROBE: &str = r#"{"streams":[{"codec_type":"video","avg_frame_rate":"30/1","r_frame_rate":"30/1","width":16,"height":16,"pix_fmt":"yuv420p"},{"codec_type":"audio","channels":2}],"format":{"duration":"10"}}"#;
    let mut stdout = io::stdout().lock();
    stdout.write_all(PROBE.as_bytes())?;
    stdout.write_all(b"\n")?;
    Ok(())
}

fn fake_ffmpeg() -> Result<(), Box<dyn Error>> {
    let output = env::args_os()
        .next_back()
        .map(PathBuf::from)
        .ok_or("ffmpeg output argument is missing")?;
    fs::write(&output, vec![0_u8; 4096])?;
    eprint!(
        "frame=    1 fps= 2 q=40.0 size=       1kB time=00:00:01.00 bitrate=8.0kbits/s speed=1x    \r"
    );
    io::stderr().flush()?;

    if output
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|stem| stem.contains("cancel"))
    {
        thread::sleep(Duration::from_secs(30));
    }

    eprintln!(
        "video:1kB audio:2kB subtitle:0kB other streams:1kB global headers:0kB muxing overhead: 0.0%"
    );
    Ok(())
}

fn run_contract(mut arguments: impl Iterator<Item = OsString>) -> Result<(), Box<dyn Error>> {
    let input = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("input path is missing")?;
    let output_dir = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("output directory is missing")?;

    let first = output_dir.join("first.mkv");
    let first_result = start_encode(request(&input, &first))?.wait()?;
    if first_result.output_size != 4096 || first_result.stream_sizes.audio != 2048 {
        return Err("successful encode did not preserve typed completion data".into());
    }

    let cancelled = output_dir.join("cancel.mkv");
    let cancelled_job = start_encode(request(&input, &cancelled))?;
    wait_for_telemetry(&cancelled_job)?;
    cancelled_job.cancel();
    if cancelled_job.wait() != Err(AdapterError::Cancelled) {
        return Err("cancelled encode did not return the cancellation terminal result".into());
    }
    if cancelled.exists() {
        return Err("cancelled encode left its temporary output behind".into());
    }

    let second = output_dir.join("second.mkv");
    let second_result = start_encode(request(&input, &second))?.wait()?;
    if second_result.output_size != 4096 {
        return Err("a second encode could not run after cancellation".into());
    }
    Ok(())
}

fn request(input: &Path, output: &Path) -> EncodeRequest {
    EncodeRequest {
        input: input.to_owned(),
        output: output.to_owned(),
        crf: 30.0,
        preset: 8,
    }
}

fn wait_for_telemetry<T>(job: &JobHandle<T>) -> Result<(), Box<dyn Error>> {
    for _attempt in 0..200 {
        if job.latest_telemetry().is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err("encode produced no telemetry before cancellation".into())
}
