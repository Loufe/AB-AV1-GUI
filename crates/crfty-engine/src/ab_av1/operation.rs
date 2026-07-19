use std::{
    fmt,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use ab_av1::{command, ffprobe};
use tokio_stream::StreamExt;

use super::types::{
    CancelMode, EncodeOutcome, EncodeRequest, EncodeTelemetry, JobFailure, JobTerminal, MediaTools,
    SearchOutcome, SearchRequest, SearchTelemetry, SearchWork, StreamSizes, Telemetry,
};

const DEFAULT_SAMPLE_EVERY: Duration = Duration::from_secs(12 * 60);
const DEFAULT_XPSNR_FPS: f32 = 60.0;

#[cfg(feature = "contract-test-fixture")]
use super::runtime::FaultInjection;

pub(crate) async fn run_search(
    tools: MediaTools,
    request: SearchRequest,
    cancellation: tokio::sync::watch::Receiver<Option<CancelMode>>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
) -> JobTerminal<SearchOutcome> {
    let tools = ab_av1::ToolPaths {
        ffmpeg: tools.ffmpeg,
        ffprobe: tools.ffprobe,
    };
    ab_av1::with_tool_paths(tools, async move {
        let result = search(request, cancellation, telemetry).await;
        finalize(result).await
    })
    .await
}

pub(crate) async fn run_encode(
    tools: MediaTools,
    request: EncodeRequest,
    cancellation: tokio::sync::watch::Receiver<Option<CancelMode>>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
    #[cfg(feature = "contract-test-fixture")] fault: FaultInjection,
) -> JobTerminal<EncodeOutcome> {
    let tools = ab_av1::ToolPaths {
        ffmpeg: tools.ffmpeg,
        ffprobe: tools.ffprobe,
    };
    ab_av1::with_tool_paths(tools, async move {
        let result = encode(
            request,
            cancellation,
            telemetry,
            #[cfg(feature = "contract-test-fixture")]
            fault,
        )
        .await;
        finalize(result).await
    })
    .await
}

enum OperationError {
    Cancelled,
    Failed(JobFailure),
}

async fn search(
    request: SearchRequest,
    mut cancellation: tokio::sync::watch::Receiver<Option<CancelMode>>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
) -> Result<SearchOutcome, OperationError> {
    if cancellation.borrow().is_some() {
        return Err(OperationError::Cancelled);
    }
    let probe = Arc::new(ffprobe::probe(&request.input));
    let mut args = search_args(&request).map_err(OperationError::Failed)?;
    args.sample
        .set_extension_from_input(&request.input, &args.args.encoder, &probe);
    args.validate()
        .map_err(failure)
        .map_err(OperationError::Failed)?;

    let mut updates = std::pin::pin!(command::crf_search::run(args, probe));
    let mut outcome = None;
    loop {
        tokio::select! {
            changed = cancellation.changed() => {
                if changed.is_err() || cancellation.borrow().is_some() {
                    return Err(OperationError::Cancelled);
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
                        return Err(OperationError::Failed(JobFailure::new(
                            "VMAF search completed without a VMAF score",
                        )));
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
                Some(Err(command::crf_search::Error::NoGoodCrf { last })) => {
                    let Some(vmaf) = last.enc.vmaf_score else {
                        return Err(OperationError::Failed(JobFailure::new(
                            "failed CRF search omitted its final VMAF score",
                        )));
                    };
                    return Err(OperationError::Failed(JobFailure::no_good_crf(SearchOutcome {
                        crf: last.crf,
                        vmaf,
                        predicted_size: last.enc.predicted_encode_size,
                        predicted_percent: last.enc.encode_percent,
                        predicted_duration: last.enc.predicted_encode_time,
                        from_cache: last.enc.from_cache,
                    })));
                }
                Some(Err(error)) => return Err(OperationError::Failed(failure(error))),
                None => return outcome.ok_or_else(|| OperationError::Failed(JobFailure::new(
                    "quality search ended without a result",
                ))),
            }
        }
    }
}

async fn encode(
    request: EncodeRequest,
    mut cancellation: tokio::sync::watch::Receiver<Option<CancelMode>>,
    telemetry: Arc<Mutex<Option<Telemetry>>>,
    #[cfg(feature = "contract-test-fixture")] fault: FaultInjection,
) -> Result<EncodeOutcome, OperationError> {
    if cancellation.borrow().is_some() {
        return Err(OperationError::Cancelled);
    }
    let probe = Arc::new(ffprobe::probe(&request.input));
    let args = encode_args(&request).map_err(OperationError::Failed)?;
    let mut updates = std::pin::pin!(command::encode::run(args, probe));
    let mut sizes = StreamSizes::default();
    loop {
        tokio::select! {
            changed = cancellation.changed() => {
                if changed.is_err() || cancellation.borrow().is_some() {
                    return Err(OperationError::Cancelled);
                }
            }
            update = updates.next() => match update {
                Some(Ok(command::encode::Update::Progress { frame, fps, time })) => {
                    set_telemetry(&telemetry, Telemetry::Encode(EncodeTelemetry {
                        frame,
                        fps,
                        position: time,
                    }));
                    #[cfg(feature = "contract-test-fixture")]
                    if fault == FaultInjection::PanicAfterFirstProgress {
                        panic!("contract fault after first encode progress");
                    }
                }
                Some(Ok(command::encode::Update::StreamSizes { video, audio, subtitle, other })) => {
                    sizes = StreamSizes { video, audio, subtitle, other };
                }
                Some(Ok(command::encode::Update::Done { output, input_size, output_size })) => {
                    return Ok(EncodeOutcome {
                        output,
                        input_size,
                        output_size,
                        stream_sizes: sizes,
                    });
                }
                Some(Err(error)) => return Err(OperationError::Failed(failure(error))),
                None => return Err(OperationError::Failed(JobFailure::new(
                    "encode ended without a result",
                ))),
            }
        }
    }
}

async fn finalize<T>(result: Result<T, OperationError>) -> JobTerminal<T> {
    let cleanup = match &result {
        Ok(_) => ab_av1::finish_job(false).await,
        Err(_) => ab_av1::cancel_job().await,
    };
    match (result, cleanup) {
        (Ok(value), Ok(())) => JobTerminal::Completed(value),
        (Err(OperationError::Cancelled), Ok(())) => JobTerminal::Cancelled,
        (Err(OperationError::Failed(error)), Ok(())) => JobTerminal::Failed(error),
        (Ok(_), Err(cleanup)) => JobTerminal::Failed(failure(cleanup)),
        (Err(OperationError::Cancelled), Err(cleanup)) => JobTerminal::Failed(JobFailure::new(
            format!("job was cancelled; cleanup failed: {cleanup}"),
        )),
        (Err(OperationError::Failed(error)), Err(cleanup)) => JobTerminal::Failed(JobFailure::new(
            format!("{}; cleanup failed: {cleanup}", error.message),
        )),
    }
}

pub(crate) fn search_args(
    request: &SearchRequest,
) -> Result<command::crf_search::Args, JobFailure> {
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
            sample_every: DEFAULT_SAMPLE_EVERY,
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
            xpsnr_fps: DEFAULT_XPSNR_FPS,
            xpsnr_pix_format: None,
        },
        verbose: Default::default(),
    })
}

pub(crate) fn encode_args(request: &EncodeRequest) -> Result<command::encode::Args, JobFailure> {
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

fn common_encode_args(input: PathBuf, preset: u8) -> Result<command::args::Encode, JobFailure> {
    Ok(command::args::Encode {
        encoder: command::args::Encoder::from_str("libsvtav1").map_err(failure)?,
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

pub(crate) fn set_telemetry(telemetry: &Mutex<Option<Telemetry>>, update: Telemetry) {
    match telemetry.lock() {
        Ok(mut slot) => *slot = Some(update),
        Err(poisoned) => *poisoned.into_inner() = Some(update),
    }
}

fn failure(error: impl fmt::Display) -> JobFailure {
    JobFailure::new(format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{encode_args, search_args, set_telemetry};
    use crate::ab_av1::{EncodeRequest, EncodeTelemetry, SearchRequest, Telemetry};
    use std::{path::PathBuf, sync::Mutex, time::Duration};

    #[test]
    fn telemetry_keeps_only_the_latest_value_under_pressure() {
        let slot = Mutex::new(None);
        for frame in 0..100_000 {
            set_telemetry(
                &slot,
                Telemetry::Encode(EncodeTelemetry {
                    frame,
                    fps: 5.0,
                    position: Duration::from_secs(frame),
                }),
            );
        }
        assert_eq!(
            *slot.lock().expect("telemetry slot"),
            Some(Telemetry::Encode(EncodeTelemetry {
                frame: 99_999,
                fps: 5.0,
                position: Duration::from_secs(99_999),
            }))
        );
    }

    #[test]
    fn requests_map_to_typed_ab_av1_arguments() {
        let search = SearchRequest {
            input: PathBuf::from("input.mkv"),
            target_vmaf: 96.0,
            max_encoded_percent: 70.0,
            preset: 6,
            samples: Some(4),
            sample_duration: Duration::from_secs(12),
            thorough: true,
        };
        let args = search_args(&search).expect("search arguments");
        assert_eq!(args.args.input, search.input);
        assert_eq!(args.min_vmaf, Some(96.0));
        assert_eq!(args.sample.samples, Some(4));

        let encode = EncodeRequest {
            input: PathBuf::from("input.mkv"),
            output: PathBuf::from("output.mkv"),
            crf: 31.5,
            preset: 7,
        };
        let args = encode_args(&encode).expect("encode arguments");
        assert_eq!(args.crf, 31.5);
        assert_eq!(args.encode.output, Some(encode.output));
    }
}
