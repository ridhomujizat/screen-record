//! Timeline alignment & gap handling (ADR-0004, ADR-0005).
#![allow(dead_code)] // dipakai penuh di M3 (audio+sync); M2 preview-only
//!
//! Port of Cap's `output_pipeline/core.rs` video start gate + gap tracker:
//! - align the first video & audio frames (trim/advance audio),
//! - fill audio gaps with silence,
//! - pad the audio tail to the video duration.

use std::time::Duration;

#[allow(dead_code)]
pub const AV_START_ALIGNMENT_LIMIT_NS: i128 = 500_000_000; // 500ms safety valve
#[allow(dead_code)]
pub const WIRED_GAP_THRESHOLD: Duration = Duration::from_millis(70);
#[allow(dead_code)]
pub const SILENCE_FRAME_MAX: Duration = Duration::from_secs(1);

/// Compute the trim/advance decision for the first audio frame given the
/// video start on the master timeline (ns) and the audio frame start (ns).
///
/// Returns:
/// - `Positive(n)` → audio is earlier by n ns → trim n from the audio head.
/// - `Negative(n)` → video is earlier → advance audio timeline by n ns.
/// - `None` → offset exceeds the alignment limit → passthrough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignAction {
    TrimAudio(u64),
    AdvanceAudio(u64),
    Passthrough,
}

pub fn align_first_audio(video_start_ns: u64, audio_start_ns: u64) -> AlignAction {
    let offset: i128 = video_start_ns as i128 - audio_start_ns as i128;
    if offset.abs() > AV_START_ALIGNMENT_LIMIT_NS {
        return AlignAction::Passthrough;
    }
    if offset == 0 {
        return AlignAction::Passthrough;
    }
    if offset > 0 {
        AlignAction::TrimAudio(offset as u64)
    } else {
        AlignAction::AdvanceAudio((-offset) as u64)
    }
}

/// Track audio gaps; insert silence when a frame gap exceeds the threshold.
#[derive(Debug)]
pub struct AudioGapTracker {
    sample_rate: u32,
    last_frame_end_ns: Option<u64>,
    gap_threshold: Duration,
    total_silence_inserted_ns: u64,
    silence_insertion_count: u64,
}

impl AudioGapTracker {
    pub fn new(sample_rate: u32, gap_threshold: Duration) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            last_frame_end_ns: None,
            gap_threshold,
            total_silence_inserted_ns: 0,
            silence_insertion_count: 0,
        }
    }

    pub fn total_silence_inserted_ns(&self) -> u64 {
        self.total_silence_inserted_ns
    }

    pub fn silence_insertion_count(&self) -> u64 {
        self.silence_insertion_count
    }

    /// Given the next audio frame's start (master ns) and its duration (ns),
    /// return how many ns of silence to insert before it (0 if no gap).
    pub fn on_frame(&mut self, frame_start_ns: u64, frame_duration_ns: u64) -> u64 {
        let gap = match self.last_frame_end_ns {
            Some(prev_end) => {
                if frame_start_ns > prev_end {
                    frame_start_ns.saturating_sub(prev_end)
                } else {
                    0
                }
            }
            None => 0,
        };

        self.last_frame_end_ns = Some(frame_start_ns.saturating_add(frame_duration_ns));

        if gap >= self.gap_threshold.as_nanos() as u64 {
            self.total_silence_inserted_ns += gap;
            self.silence_insertion_count += 1;
            gap
        } else {
            0
        }
    }

    pub fn silence_samples(&self, gap_ns: u64) -> usize {
        ((gap_ns as f64 / 1_000_000_000.0) * self.sample_rate as f64).ceil() as usize
    }

    pub fn reset(&mut self) {
        self.last_frame_end_ns = None;
    }
}

/// Pad the audio tail so the track reaches the video duration.
/// `video_duration_ns` is the target; `audio_elapsed_ns` is current audio length.
pub fn tail_padding_ns(video_duration_ns: u64, audio_elapsed_ns: u64) -> u64 {
    video_duration_ns.saturating_sub(audio_elapsed_ns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_audio_early_trims() {
        // video starts at 100ms, audio at 80ms → audio is 20ms early → trim
        assert_eq!(
            align_first_audio(100_000_000, 80_000_000),
            AlignAction::TrimAudio(20_000_000)
        );
    }

    #[test]
    fn align_video_early_advances_audio() {
        // video at 100ms, audio at 120ms → video early → advance audio by 20ms
        assert_eq!(
            align_first_audio(100_000_000, 120_000_000),
            AlignAction::AdvanceAudio(20_000_000)
        );
    }

    #[test]
    fn align_outside_limit_passthrough() {
        assert_eq!(
            align_first_audio(100_000_000, 100_000_000 + 700_000_000),
            AlignAction::Passthrough
        );
    }

    #[test]
    fn gap_under_threshold_no_silence() {
        let mut t = AudioGapTracker::new(48_000, WIRED_GAP_THRESHOLD);
        t.on_frame(0, 20_000_000);
        // 30ms gap < 70ms threshold → 0 silence
        assert_eq!(t.on_frame(50_000_000, 20_000_000), 0);
        assert_eq!(t.silence_insertion_count(), 0);
    }

    #[test]
    fn gap_over_threshold_inserts_silence() {
        let mut t = AudioGapTracker::new(48_000, WIRED_GAP_THRESHOLD);
        t.on_frame(0, 20_000_000);
        // 100ms gap > 70ms → silence = 100ms
        let silence = t.on_frame(120_000_000, 20_000_000);
        assert_eq!(silence, 100_000_000);
        assert_eq!(t.silence_insertion_count(), 1);
        assert_eq!(t.total_silence_inserted_ns(), 100_000_000);
        // 100ms at 48k = 4800 samples
        assert_eq!(t.silence_samples(silence), 4_800);
    }

    #[test]
    fn tail_padding_fills_short_audio() {
        assert_eq!(tail_padding_ns(10_000_000_000, 8_000_000_000), 2_000_000_000);
        assert_eq!(tail_padding_ns(10_000_000_000, 12_000_000_000), 0);
    }
}
