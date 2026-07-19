use std::{fmt, path::PathBuf, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

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

#[derive(Debug, Clone, PartialEq)]
pub enum JobFailureKind {
    NoGoodCrf { last: SearchOutcome },
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobFailure {
    pub kind: JobFailureKind,
    pub message: String,
}

impl JobFailure {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            kind: JobFailureKind::Other,
            message: message.into(),
        }
    }

    pub(crate) fn no_good_crf(last: SearchOutcome) -> Self {
        Self {
            kind: JobFailureKind::NoGoodCrf { last },
            message: "failed to find a suitable CRF".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelMode {
    Graceful,
    Force,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobTerminal<T> {
    Completed(T),
    Failed(JobFailure),
    Cancelled,
    Panicked { cleanup_failure: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobReport<T> {
    pub terminal: JobTerminal<T>,
    pub final_telemetry: Option<Telemetry>,
}

macro_rules! message_error {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(pub(crate) String);

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl std::error::Error for $name {}
    };
}

message_error!(RuntimeStartError);
message_error!(ShutdownError);
message_error!(WaitError);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartJobError {
    Busy,
    ShuttingDown,
    InvalidTool { name: &'static str, path: PathBuf },
}

impl fmt::Display for StartJobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter.write_str("an ab-av1 job is already active"),
            Self::ShuttingDown => formatter.write_str("the ab-av1 runtime is shutting down"),
            Self::InvalidTool { name, path } => {
                write!(
                    formatter,
                    "{name} is not an absolute executable file: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for StartJobError {}
