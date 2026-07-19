//! Isolated adapter for CRFty's pinned ab-av1 revision.

use std::{
    fmt,
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use ab_av1::{command, ffprobe};
use tokio::runtime;
use tokio_stream::StreamExt;

static JOB_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub input: PathBuf,
    pub target_vmaf: f32,
    pub max_encoded_percent: f32,
    pub preset: u8,
    pub samples: Option<u64>,
    pub sample_duration: Duration,
    pub thorough: bool,
}

#[derive(Debug, Clone)]
pub struct EncodeRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub crf: f32,
    pub preset: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchWork {
    Encode,
    Vmaf,
    Xpsnr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchTelemetry {
    pub crf_run: usize,
    pub crf: f32,
    pub work: SearchWork,
    pub fps: f32,
    pub progress: f32,
    pub sample: u64,
    pub samples: u64,
    pub full_pass: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EncodeTelemetry {
    pub frame: u64,
    pub fps: f32,
    pub position: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Telemetry {
    Search(SearchTelemetry),
    Encode(EncodeTelemetry),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    pub crf: f32,
    pub vmaf: f32,
    pub predicted_size: u64,
    pub predicted_percent: f64,
    pub predicted_duration: Duration,
    pub from_cache: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamSizes {
    pub video: u64,
    pub audio: u64,
    pub subtitle: u64,
    pub other: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeOutcome {
    pub output: PathBuf,
    pub input_size: u64,
    pub output_size: u64,
    pub stream_sizes: StreamSizes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    Busy,
    Cancelled,
    Start(String),
    AbAv1(String),
    WorkerPanicked,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter.write_str("an ab-av1 job is already active"),
            Self::Cancelled => formatter.write_str("the ab-av1 job was cancelled"),
            Self::Start(message) => write!(formatter, "failed to start ab-av1 job: {message}"),
            Self::AbAv1(message) => write!(formatter, "ab-av1 job failed: {message}"),
            Self::WorkerPanicked => formatter.write_str("the ab-av1 worker panicked"),
        }
    }
}

impl std::error::Error for AdapterError {}

pub struct JobHandle<T> {
    cancellation: tokio::sync::watch::Sender<bool>,
    result: mpsc::Receiver<Result<T, AdapterError>>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
    worker: Option<thread::JoinHandle<()>>,
    cancel_on_drop: bool,
}

impl<T> JobHandle<T> {
    #[must_use]
    pub fn latest_telemetry(&self) -> Option<Telemetry> {
        match self.telemetry.lock() {
            Ok(telemetry) => telemetry.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn cancel(&self) {
        let _result = self.cancellation.send(true);
    }

    pub fn wait(mut self) -> Result<T, AdapterError> {
        let result = self
            .result
            .recv()
            .map_err(|error| AdapterError::Start(error.to_string()))?;
        self.cancel_on_drop = false;
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            return Err(AdapterError::WorkerPanicked);
        }
        result
    }
}

impl<T> Drop for JobHandle<T> {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            let _result = self.cancellation.send(true);
        }
    }
}

pub fn start_search(request: SearchRequest) -> Result<JobHandle<SearchOutcome>, AdapterError> {
    spawn_job(move |cancellation, telemetry| run_search(request, cancellation, telemetry))
}

pub fn start_encode(request: EncodeRequest) -> Result<JobHandle<EncodeOutcome>, AdapterError> {
    spawn_job(move |cancellation, telemetry| run_encode(request, cancellation, telemetry))
}

fn spawn_job<T, F, Fut>(job: F) -> Result<JobHandle<T>, AdapterError>
where
    T: Send + 'static,
    F: FnOnce(tokio::sync::watch::Receiver<bool>, Arc<Mutex<Option<Telemetry>>>) -> Fut
        + Send
        + 'static,
    Fut: Future<Output = Result<T, AdapterError>> + 'static,
{
    let permit = JobPermit::acquire()?;
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let (result_tx, result_rx) = mpsc::channel();
    let telemetry = Arc::new(Mutex::new(None));
    let worker_telemetry = Arc::clone(&telemetry);

    let worker = thread::Builder::new()
        .name("crfty-ab-av1".into())
        .spawn(move || {
            let _permit = permit;
            let runtime = match runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _result = result_tx.send(Err(AdapterError::Start(error.to_string())));
                    return;
                }
            };

            let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let local = tokio::task::LocalSet::new();
                runtime.block_on(local.run_until(job(cancel_rx, worker_telemetry)))
            }));
            let result = match execution {
                Ok(result) => result,
                Err(_) => {
                    let _cleanup = runtime.block_on(ab_av1::cancel_job());
                    Err(AdapterError::WorkerPanicked)
                }
            };
            let _result = result_tx.send(result);
        })
        .map_err(|error| AdapterError::Start(error.to_string()))?;

    Ok(JobHandle {
        cancellation: cancel_tx,
        result: result_rx,
        telemetry,
        worker: Some(worker),
        cancel_on_drop: true,
    })
}

async fn run_search(
    request: SearchRequest,
    mut cancellation: tokio::sync::watch::Receiver<bool>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
) -> Result<SearchOutcome, AdapterError> {
    let probe = Arc::new(ffprobe::probe(&request.input));
    let mut args = search_args(&request)?;
    args.sample
        .set_extension_from_input(&request.input, &args.args.encoder, &probe);
    args.validate().map_err(ab_error)?;

    let result = {
        let mut updates = std::pin::pin!(command::crf_search::run(args, probe));
        let mut outcome = None;
        loop {
            tokio::select! {
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        break Err(AdapterError::Cancelled);
                    }
                }
                update = updates.next() => match update {
                    Some(Ok(command::crf_search::Update::Status { crf_run, crf, sample })) => {
                        set_telemetry(&telemetry, Telemetry::Search(SearchTelemetry {
                            crf_run,
                            crf,
                            work: map_work(sample.work),
                            fps: sample.fps,
                            progress: sample.progress,
                            sample: sample.sample,
                            samples: sample.samples,
                            full_pass: sample.full_pass,
                        }));
                    }
                    Some(Ok(command::crf_search::Update::Done(best))) => {
                        let Some(vmaf) = best.enc.vmaf_score else {
                            break Err(AdapterError::AbAv1(
                                "VMAF search completed without a VMAF score".into(),
                            ));
                        };
                        outcome = Some(SearchOutcome {
                            crf: best.crf,
                            vmaf,
                            predicted_size: best.enc.predicted_encode_size,
                            predicted_percent: best.enc.encode_percent,
                            predicted_duration: best.enc.predicted_encode_time,
                            from_cache: best.enc.from_cache,
                        });
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => break Err(ab_error(error)),
                    None => break outcome.ok_or_else(|| {
                        AdapterError::AbAv1("quality search ended without a result".into())
                    }),
                }
            }
        }
    };

    finalize_job(result).await
}

async fn run_encode(
    request: EncodeRequest,
    mut cancellation: tokio::sync::watch::Receiver<bool>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
) -> Result<EncodeOutcome, AdapterError> {
    let probe = Arc::new(ffprobe::probe(&request.input));
    let args = encode_args(&request)?;
    let result = {
        let mut updates = std::pin::pin!(command::encode::run(args, probe));
        let mut sizes = StreamSizes::default();
        loop {
            tokio::select! {
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        break Err(AdapterError::Cancelled);
                    }
                }
                update = updates.next() => match update {
                    Some(Ok(command::encode::Update::Progress { frame, fps, time })) => {
                        set_telemetry(&telemetry, Telemetry::Encode(EncodeTelemetry {
                            frame,
                            fps,
                            position: time,
                        }));
                    }
                    Some(Ok(command::encode::Update::StreamSizes {
                        video,
                        audio,
                        subtitle,
                        other,
                    })) => sizes = StreamSizes { video, audio, subtitle, other },
                    Some(Ok(command::encode::Update::Done {
                        output,
                        input_size,
                        output_size,
                    })) => break Ok(EncodeOutcome {
                        output,
                        input_size,
                        output_size,
                        stream_sizes: sizes,
                    }),
                    Some(Err(error)) => break Err(ab_error(error)),
                    None => break Err(AdapterError::AbAv1(
                        "encode ended without a result".into(),
                    )),
                }
            }
        }
    };

    finalize_job(result).await
}

async fn finalize_job<T>(result: Result<T, AdapterError>) -> Result<T, AdapterError> {
    let cleanup = match result {
        Ok(_) => ab_av1::finish_job(false).await,
        Err(_) => ab_av1::cancel_job().await,
    };
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(ab_error(cleanup)),
        (Err(error), Err(cleanup)) => Err(AdapterError::AbAv1(format!(
            "{error}; cleanup failed: {cleanup}"
        ))),
    }
}

fn search_args(request: &SearchRequest) -> Result<command::crf_search::Args, AdapterError> {
    Ok(command::crf_search::Args {
        args: common_encode_args(request.input.clone(), request.preset)?,
        min_vmaf: Some(request.target_vmaf),
        min_xpsnr: None,
        max_encoded_percent: request.max_encoded_percent,
        min_crf: None,
        max_crf: None,
        thorough: request.thorough,
        crf_increment: None,
        high_crf_means_hq: None,
        cache: true,
        sample: command::args::Sample {
            samples: request.samples,
            sample_every: Duration::from_secs(12 * 60),
            min_samples: None,
            sample_duration: request.sample_duration,
            keep: false,
            temp_dir: None,
            extension: None,
        },
        vmaf: command::args::Vmaf::default(),
        score: command::args::ScoreArgs {
            reference_vfilter: None,
        },
        xpsnr: command::args::Xpsnr {
            xpsnr_fps: 60.0,
            xpsnr_pix_format: None,
        },
        verbose: Default::default(),
    })
}

fn encode_args(request: &EncodeRequest) -> Result<command::encode::Args, AdapterError> {
    Ok(command::encode::Args {
        args: common_encode_args(request.input.clone(), request.preset)?,
        crf: request.crf,
        encode: command::args::EncodeToOutput {
            output: Some(request.output.clone()),
            audio_codec: None,
            downmix_to_stereo: false,
            video_only: false,
            overwrite_input: false,
        },
    })
}

fn common_encode_args(input: PathBuf, preset: u8) -> Result<command::args::Encode, AdapterError> {
    Ok(command::args::Encode {
        encoder: command::args::Encoder::from_str("libsvtav1").map_err(ab_error)?,
        input,
        vfilter: None,
        pix_format: None,
        preset: Some(preset.to_string().into()),
        keyint: None,
        scd: None,
        svt_args: Vec::new(),
        enc_args: Vec::new(),
        enc_input_args: Vec::new(),
    })
}

fn map_work(work: command::sample_encode::Work) -> SearchWork {
    match work {
        command::sample_encode::Work::Encode => SearchWork::Encode,
        command::sample_encode::Work::Score(command::sample_encode::ScoreKind::Vmaf) => {
            SearchWork::Vmaf
        }
        command::sample_encode::Work::Score(command::sample_encode::ScoreKind::Xpsnr) => {
            SearchWork::Xpsnr
        }
    }
}

fn set_telemetry(telemetry: &Mutex<Option<Telemetry>>, update: Telemetry) {
    match telemetry.lock() {
        Ok(mut slot) => *slot = Some(update),
        Err(poisoned) => *poisoned.into_inner() = Some(update),
    }
}

fn ab_error(error: impl fmt::Display) -> AdapterError {
    AdapterError::AbAv1(error.to_string())
}

#[derive(Debug)]
struct JobPermit;

impl JobPermit {
    fn acquire() -> Result<Self, AdapterError> {
        JOB_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| AdapterError::Busy)
    }
}

impl Drop for JobPermit {
    fn drop(&mut self) {
        JOB_ACTIVE.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdapterError, EncodeRequest, JobPermit, SearchRequest, Telemetry, encode_args, search_args,
        set_telemetry,
    };
    use std::{path::PathBuf, sync::Mutex, time::Duration};

    #[test]
    fn only_one_ab_av1_job_can_hold_the_permit() {
        let permit = JobPermit::acquire().expect("first permit");
        assert_eq!(
            JobPermit::acquire().expect_err("second permit"),
            AdapterError::Busy
        );
        drop(permit);
        JobPermit::acquire().expect("permit after release");
    }

    #[test]
    fn telemetry_keeps_only_the_latest_value() {
        let slot = Mutex::new(None);
        let first = Telemetry::Encode(super::EncodeTelemetry {
            frame: 1,
            fps: 2.0,
            position: Duration::from_secs(3),
        });
        let second = Telemetry::Encode(super::EncodeTelemetry {
            frame: 4,
            fps: 5.0,
            position: Duration::from_secs(6),
        });
        set_telemetry(&slot, first);
        set_telemetry(&slot, second.clone());
        assert_eq!(*slot.lock().expect("telemetry slot"), Some(second));
    }

    #[test]
    fn search_request_maps_to_typed_ab_av1_arguments() {
        let request = SearchRequest {
            input: PathBuf::from("input.mkv"),
            target_vmaf: 96.0,
            max_encoded_percent: 70.0,
            preset: 6,
            samples: Some(4),
            sample_duration: Duration::from_secs(12),
            thorough: true,
        };

        let args = search_args(&request).expect("search arguments");
        assert_eq!(args.args.input, request.input);
        assert_eq!(args.args.encoder.as_str(), "libsvtav1");
        assert_eq!(args.args.preset.as_deref(), Some("6"));
        assert_eq!(args.min_vmaf, Some(96.0));
        assert_eq!(args.max_encoded_percent, 70.0);
        assert_eq!(args.sample.samples, Some(4));
        assert_eq!(args.sample.sample_duration, Duration::from_secs(12));
        assert!(args.thorough);
    }

    #[test]
    fn encode_request_maps_output_and_quality() {
        let request = EncodeRequest {
            input: PathBuf::from("input.mkv"),
            output: PathBuf::from("output.mkv"),
            crf: 31.5,
            preset: 7,
        };

        let args = encode_args(&request).expect("encode arguments");
        assert_eq!(args.args.input, request.input);
        assert_eq!(args.crf, 31.5);
        assert_eq!(args.encode.output, Some(request.output));
        assert_eq!(args.args.preset.as_deref(), Some("7"));
    }
}
