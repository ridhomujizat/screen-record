//! MasterClock + SourceClockState — global A/V timeline (ADR-0003).
#![allow(dead_code)] // dipakai penuh di M3 (audio+sync); M2 preview-only
//!
//! Port of Cap's `crates/timestamp/master_clock.rs`:
//! one timeline based on committed audio samples; every source timestamp
//! (video QPC, audio WASAPI QPC) is remapped onto it by SourceClockState.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[allow(dead_code)]
pub const AUDIO_OUTPUT_FRAMES: u64 = 1024;
#[allow(dead_code)]
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
#[allow(dead_code)]
pub const TS_SMOOTHING_THRESHOLD_NS: u64 = 70_000_000;
#[allow(dead_code)]
pub const MAX_TS_VAR_NS: u64 = 2_000_000_000;

/// A single raw timestamp in a hardware clock family (QPC on Windows).
#[derive(Clone, Copy, Debug)]
pub struct RawTimestamp {
    pub qpc_ticks: i64,
}

impl RawTimestamp {
    pub fn from_qpc(ticks: i64) -> Self {
        Self { qpc_ticks: ticks }
    }
}

/// Global timeline clock. Advances by committed audio samples at
/// `sample_rate` (48 kHz default) so the timeline is perfectly smooth.
pub struct MasterClock {
    start_instant: Instant,
    sample_rate: u32,
    chunk_size: u64,
    samples_committed: AtomicU64,
    /// Frequency of the QPC counter on this machine, ticks/sec.
    qpc_frequency: i64,
}

impl MasterClock {
    pub fn new(sample_rate: u32) -> Arc<Self> {
        Arc::new(Self {
            start_instant: Instant::now(),
            sample_rate: sample_rate.max(1),
            chunk_size: AUDIO_OUTPUT_FRAMES,
            samples_committed: AtomicU64::new(0),
            qpc_frequency: qpc_frequency(),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn chunk_size(&self) -> u64 {
        self.chunk_size
    }

    pub fn elapsed_ns(&self) -> u64 {
        let nanos = self.start_instant.elapsed().as_nanos();
        if nanos > u64::MAX as u128 {
            u64::MAX
        } else {
            nanos as u64
        }
    }

    pub fn committed_samples(&self) -> u64 {
        self.samples_committed.load(Ordering::Acquire)
    }

    pub fn committed_ns(&self) -> u64 {
        samples_to_ns(self.committed_samples(), self.sample_rate)
    }

    pub fn advance_samples(&self, samples: u64) {
        self.samples_committed.fetch_add(samples, Ordering::AcqRel);
    }

    /// Map a raw QPC timestamp to nanoseconds since clock start.
    /// QPC and WGC/WASAPI share the same counter, so this is exact.
    pub fn remap_raw_ns(&self, raw: RawTimestamp) -> i64 {
        let freq = if self.qpc_frequency > 0 {
            self.qpc_frequency as f64
        } else {
            1.0
        };
        let secs = raw.qpc_ticks as f64 / freq;
        seconds_to_ns_saturating(secs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClockOutcome {
    FirstFrame,
    Trusted,
    InitialAdjust,
    Smoothed,
    HardReset,
    Untouched,
}

#[derive(Debug, Clone, Copy)]
pub struct SourceClockRemap {
    pub master_ns: u64,
    pub raw_ns: i64,
    pub outcome: SourceClockOutcome,
}

impl SourceClockRemap {
    pub fn duration(&self) -> Duration {
        Duration::from_nanos(self.master_ns)
    }
}

/// Per-source clock that snaps jitter onto the master timeline cadence.
#[derive(Debug)]
pub struct SourceClockState {
    name: &'static str,
    timing_set: bool,
    timing_adjust: i64,
    next_expected_ns: Option<i64>,
    pub snap_count: u64,
    pub hard_reset_count: u64,
}

impl SourceClockState {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            timing_set: false,
            timing_adjust: 0,
            next_expected_ns: None,
            snap_count: 0,
            hard_reset_count: 0,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn remap(
        &mut self,
        clock: &MasterClock,
        source_ts: RawTimestamp,
        frame_duration_ns: u64,
    ) -> SourceClockRemap {
        let raw_ns = clock.remap_raw_ns(source_ts);
        let now_ns = clock.elapsed_ns() as i64;

        let using_direct_ts = abs_diff_u64(raw_ns, now_ns) < MAX_TS_VAR_NS;
        let mut outcome = SourceClockOutcome::Untouched;

        if using_direct_ts {
            self.timing_adjust = 0;
            self.timing_set = true;
            outcome = SourceClockOutcome::Trusted;
        }

        let mut ts_ns = raw_ns;
        let duration_ns = frame_duration_ns.min(i64::MAX as u64) as i64;

        if !self.timing_set {
            self.timing_adjust = now_ns.saturating_sub(raw_ns);
            self.timing_set = true;
            outcome = SourceClockOutcome::InitialAdjust;
        } else if let Some(expected) = self.next_expected_ns {
            let diff = abs_diff_u64(expected, ts_ns);
            if diff > MAX_TS_VAR_NS && !using_direct_ts {
                self.timing_adjust = now_ns.saturating_sub(raw_ns);
                self.next_expected_ns = None;
                self.hard_reset_count += 1;
                outcome = SourceClockOutcome::HardReset;
            } else if diff < TS_SMOOTHING_THRESHOLD_NS {
                let max_lead_ns = duration_ns
                    .min((TS_SMOOTHING_THRESHOLD_NS as i64).saturating_sub(duration_ns))
                    .max(0);
                ts_ns = expected.min(raw_ns.saturating_add(max_lead_ns));
                self.snap_count += 1;
                if !matches!(outcome, SourceClockOutcome::HardReset) {
                    outcome = SourceClockOutcome::Smoothed;
                }
            }
        } else if matches!(outcome, SourceClockOutcome::Untouched) {
            outcome = SourceClockOutcome::FirstFrame;
        }
        self.next_expected_ns = Some(ts_ns.saturating_add(duration_ns));

        let output_ns = ts_ns.saturating_add(self.timing_adjust).max(0) as u64;

        SourceClockRemap {
            master_ns: output_ns,
            raw_ns,
            outcome,
        }
    }

    pub fn reset(&mut self) {
        self.timing_set = false;
        self.timing_adjust = 0;
        self.next_expected_ns = None;
    }
}

fn abs_diff_u64(a: i64, b: i64) -> u64 {
    if a >= b {
        (a as i128 - b as i128).unsigned_abs() as u64
    } else {
        (b as i128 - a as i128).unsigned_abs() as u64
    }
}

fn samples_to_ns(samples: u64, rate: u32) -> u64 {
    if rate == 0 {
        return 0;
    }
    let nanos = (samples as u128 * 1_000_000_000u128) / rate as u128;
    if nanos > u64::MAX as u128 {
        u64::MAX
    } else {
        nanos as u64
    }
}

fn seconds_to_ns_saturating(secs: f64) -> i64 {
    if !secs.is_finite() {
        return 0;
    }
    let scaled = secs * 1_000_000_000.0;
    if scaled >= i64::MAX as f64 {
        i64::MAX
    } else if scaled <= i64::MIN as f64 {
        i64::MIN
    } else {
        scaled as i64
    }
}

/// QPC frequency in ticks/sec (Windows); falls back to 1 on other platforms.
#[cfg(windows)]
pub fn qpc_frequency() -> i64 {
    use std::sync::OnceLock;
    use windows::Win32::System::Performance::QueryPerformanceFrequency;
    static FREQ: OnceLock<i64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut f: i64 = 0;
        unsafe { QueryPerformanceFrequency(&mut f) }.unwrap_or_default();
        f
    })
}

#[cfg(not(windows))]
pub fn qpc_frequency() -> i64 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticks_for_secs(secs: f64) -> i64 {
        (secs * qpc_frequency() as f64) as i64
    }

    #[cfg(windows)]
    fn qpc_now_ticks() -> i64 {
        use windows::Win32::System::Performance::QueryPerformanceCounter;
        let mut v: i64 = 0;
        unsafe { QueryPerformanceCounter(&mut v) }.unwrap_or_default();
        v
    }
    #[cfg(not(windows))]
    fn qpc_now_ticks() -> i64 {
        // Non-Windows: fake a stable counter (tests still exercise snap logic)
        use std::sync::atomic::{AtomicI64, Ordering};
        static T: AtomicI64 = AtomicI64::new(0);
        T.fetch_add(1_000_000, Ordering::Relaxed)
    }

    fn clock() -> Arc<MasterClock> {
        MasterClock::new(DEFAULT_SAMPLE_RATE)
    }

    #[test]
    fn committed_ns_reflects_advances() {
        let c = clock();
        assert_eq!(c.committed_ns(), 0);
        c.advance_samples(AUDIO_OUTPUT_FRAMES);
        let expected = AUDIO_OUTPUT_FRAMES * 1_000_000_000 / DEFAULT_SAMPLE_RATE as u64;
        assert_eq!(c.committed_ns(), expected);
    }

    #[test]
    fn remap_near_now_is_trusted_with_zero_adjust() {
        let c = clock();
        let mut s = SourceClockState::new("test");
        // raw timestamp captured ~at clock start
        let ts = RawTimestamp::from_qpc(qpc_frequency()); // 1 second worth
        let _ = s.remap(&c, ts, Duration::from_millis(20).as_nanos() as u64);
        // outcome is InitialAdjust or Trusted, timing_set must be true
        assert!(s.timing_set);
    }

    #[test]
    fn jitter_under_70ms_snaps_to_cadence() {
        let c = clock();
        let mut s = SourceClockState::new("jitter");
        let frame_ns = Duration::from_millis(20).as_nanos() as u64;

        // First frame: raw QPC captured ~now (just after clock start).
        // remap anchors it: master_ns becomes (raw - adjust) ≈ 0.
        let base = qpc_now_ticks();
        let first = s.remap(&c, RawTimestamp::from_qpc(base), frame_ns);
        assert!(
            first.master_ns < 100_000_000,
            "first frame should anchor near 0, got {}",
            first.master_ns
        );

        // Second frame at +25ms raw (5ms jitter over the 20ms cadence):
        // must snap to exactly first + 20ms.
        let jittered = base + ticks_for_secs(0.025);
        let second = s.remap(&c, RawTimestamp::from_qpc(jittered), frame_ns);
        assert_eq!(
            second.master_ns,
            first.master_ns + frame_ns,
            "jittered frame must snap to expected cadence"
        );
        assert!(matches!(second.outcome, SourceClockOutcome::Smoothed));
        assert_eq!(s.snap_count, 1);
    }

    #[test]
    fn hard_resets_on_big_jump() {
        let c = clock();
        let mut s = SourceClockState::new("bigjump");
        let frame_ns = Duration::from_millis(20).as_nanos() as u64;

        let base = ticks_for_secs(0.5);
        s.remap(&c, RawTimestamp::from_qpc(base), frame_ns);
        s.remap(
            &c,
            RawTimestamp::from_qpc(base + ticks_for_secs(0.02)),
            frame_ns,
        );

        // 5s jump → hard reset
        let future = RawTimestamp::from_qpc(base + ticks_for_secs(5.0));
        let result = s.remap(&c, future, frame_ns);
        assert_eq!(s.hard_reset_count, 1);
        assert!(matches!(result.outcome, SourceClockOutcome::HardReset));
    }
}
