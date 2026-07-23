//! Live-rate smoothing for job telemetry: fps over a ~3 s sliding window and
//! a progress-velocity ETA that is simply absent during warm-up (#33 §11).
//! One implementation serves the search, encode, and remux phases. Elapsed
//! time enters as an argument — the math owns no clock — so every rule here
//! is testable without processes.

use std::{collections::VecDeque, time::Duration};

/// Sliding-window span for rate smoothing (#33 §11).
const SMOOTHING_WINDOW: Duration = Duration::from_secs(3);
/// ETA stays absent this long after a phase starts: early velocity readings
/// swing wildly while encoder pipelines fill (#33 §17 progress hygiene).
const ETA_WARMUP: Duration = Duration::from_secs(4);
/// The wire's `fps_centi` field is hundredths of a frame per second.
const CENTI_PER_UNIT: f64 = 100.0;
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// One adapter progress update, normalized for rate tracking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RateSample {
    /// Monotonic frames-encoded counter, when the phase reports one (encode).
    pub frames: Option<u64>,
    /// Adapter-reported instantaneous fps. Fallback evidence: a windowed mean
    /// of gauge readings smooths phases with no frame counter (search) and
    /// covers the first samples before a counter slope exists.
    pub fps_gauge: Option<f32>,
    /// Monotonic progress toward the tracker's total, in caller-chosen units
    /// (normalized fraction for search, output position ms for encode/remux).
    pub work_done: f64,
}

/// Sliding-window fps and ETA for one adapter run. Create one per spawned
/// process: a retry or fallback attempt restarts progress, so it restarts the
/// window and the ETA warm-up with it.
#[derive(Debug)]
pub(crate) struct RateTracker {
    total_work: Option<f64>,
    frames: Window,
    gauge: Window,
    work: Window,
}

impl RateTracker {
    /// `total_work` is the value `work_done` reaches at completion, in the
    /// same units the caller feeds to [`Self::record`]; `None` means the
    /// total is unknown and the ETA stays absent.
    pub fn new(total_work: Option<f64>) -> Self {
        Self {
            total_work: total_work.filter(|total| total.is_finite() && *total > 0.0),
            frames: Window::new(),
            gauge: Window::new(),
            work: Window::new(),
        }
    }

    /// Records one adapter update observed `elapsed` after the run started.
    /// Push each distinct update once: repeating the latest value at later
    /// instants would drag the window's slope toward zero between updates.
    pub fn record(&mut self, elapsed: Duration, sample: &RateSample) {
        if let Some(frames) = sample.frames {
            self.frames.push(elapsed, frames as f64);
        }
        if let Some(gauge) = sample.fps_gauge
            && gauge.is_finite()
            && gauge >= 0.0
        {
            self.gauge.push(elapsed, f64::from(gauge));
        }
        if sample.work_done.is_finite() {
            self.work.push(elapsed, sample.work_done);
        }
    }

    /// Smoothed fps in hundredths of a frame per second. The frame-counter
    /// slope is the honest measurement and wins when the window spans one;
    /// otherwise the windowed gauge mean stands in.
    pub fn fps_centi(&self) -> Option<u32> {
        let fps = self
            .frames
            .slope_per_second()
            .or_else(|| self.gauge.mean())?;
        Some((fps.max(0.0) * CENTI_PER_UNIT).round() as u32)
    }

    /// Milliseconds until the tracked work reaches its total, from the
    /// window's progress velocity. Absent during warm-up, without a known
    /// total, and until the velocity is positive.
    pub fn eta_ms(&self, elapsed: Duration) -> Option<u64> {
        if elapsed < ETA_WARMUP {
            return None;
        }
        let total = self.total_work?;
        let velocity = self.work.slope_per_second().filter(|slope| *slope > 0.0)?;
        let remaining = (total - self.work.latest()?).max(0.0);
        Some((remaining / velocity * MILLIS_PER_SECOND).round() as u64)
    }
}

/// Timestamped samples no older than [`SMOOTHING_WINDOW`] behind the newest.
#[derive(Debug)]
struct Window {
    samples: VecDeque<(Duration, f64)>,
}

impl Window {
    fn new() -> Self {
        Self {
            samples: VecDeque::new(),
        }
    }

    fn push(&mut self, at: Duration, value: f64) {
        self.samples.push_back((at, value));
        let cutoff = at.saturating_sub(SMOOTHING_WINDOW);
        while self
            .samples
            .front()
            .is_some_and(|(taken, _)| *taken < cutoff)
        {
            self.samples.pop_front();
        }
    }

    /// Endpoint slope across the window; needs two samples with time between
    /// them, so a single stale reading never fabricates a rate.
    fn slope_per_second(&self) -> Option<f64> {
        let (first_at, first_value) = self.samples.front()?;
        let (last_at, last_value) = self.samples.back()?;
        let seconds = last_at.saturating_sub(*first_at).as_secs_f64();
        if seconds <= 0.0 {
            return None;
        }
        Some((last_value - first_value) / seconds)
    }

    fn mean(&self) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: f64 = self.samples.iter().map(|(_, value)| value).sum();
        Some(sum / self.samples.len() as f64)
    }

    fn latest(&self) -> Option<f64> {
        self.samples.back().map(|(_, value)| *value)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{RateSample, RateTracker};

    fn frames(count: u64) -> RateSample {
        RateSample {
            frames: Some(count),
            fps_gauge: None,
            work_done: count as f64,
        }
    }

    fn gauge(fps: f32) -> RateSample {
        RateSample {
            frames: None,
            fps_gauge: Some(fps),
            work_done: 0.0,
        }
    }

    fn work(done: f64) -> RateSample {
        RateSample {
            frames: None,
            fps_gauge: None,
            work_done: done,
        }
    }

    #[test]
    fn fps_comes_from_the_frame_counter_slope_over_the_window() {
        let mut rates = RateTracker::new(None);
        for second in 0..=5_u64 {
            rates.record(Duration::from_secs(second), &frames(second * 60));
        }
        // Samples older than the 3 s window are evicted, so the slope spans
        // t=2..=5 only: (300 - 120) frames over 3 s.
        assert_eq!(rates.fps_centi(), Some(6_000));
    }

    #[test]
    fn fps_falls_back_to_the_windowed_gauge_mean() {
        let mut rates = RateTracker::new(None);
        rates.record(Duration::from_secs(0), &gauge(10.0));
        assert_eq!(rates.fps_centi(), Some(1_000), "one gauge reading suffices");
        rates.record(Duration::from_secs(1), &gauge(20.0));
        assert_eq!(rates.fps_centi(), Some(1_500));
    }

    #[test]
    fn frame_slope_outranks_the_gauge_and_bad_gauges_are_ignored() {
        let mut rates = RateTracker::new(None);
        let mixed = |count: u64, fps: f32| RateSample {
            frames: Some(count),
            fps_gauge: Some(fps),
            work_done: 0.0,
        };
        rates.record(Duration::from_secs(0), &mixed(0, 99.0));
        rates.record(Duration::from_secs(2), &mixed(100, 99.0));
        assert_eq!(rates.fps_centi(), Some(5_000), "slope wins over the gauge");

        let mut poisoned = RateTracker::new(None);
        poisoned.record(Duration::from_secs(0), &gauge(f32::NAN));
        poisoned.record(Duration::from_secs(1), &gauge(-3.0));
        assert_eq!(poisoned.fps_centi(), None);
    }

    #[test]
    fn eta_is_absent_during_warmup_and_appears_after_it() {
        let mut rates = RateTracker::new(Some(100.0));
        for second in 0..=3_u64 {
            rates.record(Duration::from_secs(second), &work(second as f64 * 10.0));
        }
        assert_eq!(rates.eta_ms(Duration::from_millis(3_999)), None);

        rates.record(Duration::from_secs(4), &work(40.0));
        // Velocity 10 units/s over the window, 60 units remaining.
        assert_eq!(rates.eta_ms(Duration::from_secs(4)), Some(6_000));
    }

    #[test]
    fn eta_requires_a_total_and_a_positive_velocity() {
        let mut unknown_total = RateTracker::new(None);
        let mut stalled = RateTracker::new(Some(100.0));
        for second in 0..=5_u64 {
            unknown_total.record(Duration::from_secs(second), &work(second as f64 * 10.0));
            stalled.record(Duration::from_secs(second), &work(25.0));
        }
        assert_eq!(unknown_total.eta_ms(Duration::from_secs(5)), None);
        assert_eq!(stalled.eta_ms(Duration::from_secs(5)), None, "flat window");
        assert_eq!(
            RateTracker::new(Some(100.0)).eta_ms(Duration::from_secs(5)),
            None,
            "no samples"
        );
    }

    #[test]
    fn eta_clamps_overshoot_to_zero_remaining() {
        let mut rates = RateTracker::new(Some(100.0));
        rates.record(Duration::from_secs(3), &work(90.0));
        rates.record(Duration::from_secs(5), &work(110.0));
        assert_eq!(rates.eta_ms(Duration::from_secs(5)), Some(0));
    }
}
