//! MP4 muxing & encoding (ADR-0008, ADR-0013).
//!
//! Tidak ada FFmpeg dev libs di sistem (hanya binary chocolatey), jadi:
//! - Video: tulis raw BGRA ke file sementara (CFR duplication → durasi sungguhan).
//! - Audio: tiap sumber (system / mic) menulis WAV sendiri yang dirender pada
//!   master timeline (ADR-0012/0013): anchor = video start; frame lebih awal
//!   → trim; gap → silence; tail → pad ke durasi video (ADR-0005).
//! - Akhir: 1 WAV → encode langsung; ≥2 WAV → ffmpeg `aformat` + `amix`
//!   (normalize=0) jadi satu track AAC.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const TARGET_RATE: u32 = 48_000;

/// One source's audio, rendered as a timeline-anchored WAV.
///
/// Sample position on the timeline is derived from each frame's master-ns;
/// the writer's cursor enforces ADR-0004/0005 (trim / gap-fill / tail pad)
/// with a single mechanism: *the WAV always spans [anchor, padded_end] and
/// samples land at their master-timeline position.*
pub struct WavWriter {
    path: PathBuf,
    writer: Option<std::io::BufWriter<std::fs::File>>,
    rate: u32,
    channels: u16,
    /// Timeline position (in frames at `rate`) already written.
    cursor_frames: u64,
    /// Session anchor (master_ns of the first video frame).
    anchor_ns: u64,
}

impl WavWriter {
    pub fn create(
        dir: &Path,
        name: &str,
        rate: u32,
        channels: u16,
        anchor_ns: u64,
    ) -> Result<Self, String> {
        let path = dir.join(format!("{name}.wav"));
        let f = std::fs::File::create(&path).map_err(|e| format!("create {path:?}: {e}"))?;
        let mut w = std::io::BufWriter::new(f);
        w.write_all(&[0u8; 44]).map_err(|e| format!("wav header: {e}"))?;
        Ok(Self {
            path,
            writer: Some(w),
            rate: rate.max(1),
            channels: channels.max(1),
            cursor_frames: 0,
            anchor_ns,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Master-timeline ns → frame position relative to the anchor.
    fn pos_frames(&self, master_ns: u64) -> u64 {
        let ns = master_ns.saturating_sub(self.anchor_ns);
        (ns as u128 * self.rate as u128 / 1_000_000_000u128) as u64
    }

    fn write_silence(&mut self, frames: u64) -> Result<(), String> {
        if frames == 0 {
            return Ok(());
        }
        let Some(w) = self.writer.as_mut() else {
            return Err("wav writer finished".into());
        };
        // 1-second zero chunk (ADR-0005: bounded silence writes)
        let chunk_frames = self.rate as u64;
        let chunk = vec![0u8; (self.rate as usize) * (self.channels as usize) * 2];
        let mut remaining = frames;
        while remaining > 0 {
            let n = remaining.min(chunk_frames);
            let bytes = (n as usize) * (self.channels as usize) * 2;
            w.write_all(&chunk[..bytes])
                .map_err(|e| format!("write silence: {e}"))?;
            remaining -= n;
        }
        self.cursor_frames += frames;
        Ok(())
    }

    /// Append one interleaved f32 chunk at its master-timeline position.
    pub fn push(&mut self, samples: &[f32], master_ns: u64) -> Result<(), String> {
        let Some(_) = self.writer else {
            return Err("wav writer finished".into());
        };

        // Audio that starts before the video anchor → trim leading samples
        // (first-frame alignment, ADR-0004).
        let pre_ns = self.anchor_ns.saturating_sub(master_ns);
        let skip_frames = (pre_ns as u128 * self.rate as u128 / 1_000_000_000u128) as usize;
        let skip_interleaved = (skip_frames * self.channels as usize).min(samples.len());
        let samples = &samples[skip_interleaved..];
        if samples.is_empty() {
            return Ok(());
        }

        // Gap before this frame's position → silence (ADR-0005).
        let pos = self.pos_frames(master_ns);
        if pos > self.cursor_frames {
            self.write_silence(pos - self.cursor_frames)?;
        }

        let Some(w) = self.writer.as_mut() else {
            return Err("wav writer finished".into());
        };
        let mut out = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&out).map_err(|e| format!("write wav: {e}"))?;
        self.cursor_frames += (samples.len() / self.channels as usize) as u64;
        Ok(())
    }

    /// Pad to `end_ns` (master timeline) so the track spans the video duration.
    pub fn tail_pad_to(&mut self, end_ns: u64) -> Result<(), String> {
        let target = self.pos_frames(end_ns);
        if target > self.cursor_frames {
            self.write_silence(target - self.cursor_frames)?;
        }
        Ok(())
    }

    pub fn cursor_frames(&self) -> u64 {
        self.cursor_frames
    }

    /// Flush + patch the RIFF header; returns the WAV path.
    pub fn finish(&mut self) -> Result<PathBuf, String> {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
        patch_wav_header(&self.path, self.rate, self.channels).map_err(|e| e.to_string())?;
        Ok(self.path.clone())
    }
}

pub struct Muxer {
    width: u32,
    height: u32,
    fps: u32,
    raw_path: PathBuf,
    raw_writer: Option<std::io::BufWriter<std::fs::File>>,
    frames_written: u64,
    wavs: Vec<(&'static str, WavWriter)>,
    // CFR state
    last_frame: Vec<u8>,
    video_start_ns: Option<u64>,
    last_video_ns: Option<u64>,
    recorded_ns: u64, // total video duration on master timeline
}

const FRAME_NS: u64 = 1_000_000_000 / 30; // 33.33ms

impl Muxer {
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            width,
            height,
            fps: fps.max(1),
            raw_path: PathBuf::new(),
            raw_writer: None,
            frames_written: 0,
            wavs: Vec::new(),
            last_frame: Vec::new(),
            video_start_ns: None,
            last_video_ns: None,
            recorded_ns: 0,
        }
    }

    /// Begin a session; raw video written into `dir` (WAVs are created lazily
    /// per audio source on first push, using the device's actual rate).
    pub fn start(&mut self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {dir:?}: {e}"))?;
        self.raw_path = dir.join("video.raw");
        let raw = std::fs::File::create(&self.raw_path)
            .map_err(|e| format!("create raw: {e}"))?;
        self.raw_writer = Some(std::io::BufWriter::new(raw));
        Ok(())
    }

    /// Append one BGRA frame with its master-timeline position (ns).
    /// CFR: duplicate last frame to fill any gap before this frame.
    pub fn push_video(&mut self, data: &[u8], master_ns: u64) -> Result<(), String> {
        let Some(w) = self.raw_writer.as_mut() else {
            return Err("muxer not started".into());
        };

        // Guard: frame size must match the configured dimensions (resolution
        // change mid-recording would corrupt the raw stream). Drop mismatched.
        let expected = (self.width as usize) * (self.height as usize) * 4;
        if data.len() != expected {
            eprintln!(
                "[mux] dropped frame: size {} != expected {expected} (resolution change?)",
                data.len()
            );
            return Ok(());
        }

        if self.video_start_ns.is_none() {
            self.video_start_ns = Some(master_ns);
        }
        let start = self.video_start_ns.unwrap_or(master_ns);

        if !self.last_frame.is_empty() {
            let gap_ns = master_ns.saturating_sub(start);
            let desired_frames = gap_ns / FRAME_NS;
            let mut written = self.frames_written;
            while written < desired_frames {
                w.write_all(&self.last_frame)
                    .map_err(|e| format!("write raw dup: {e}"))?;
                written += 1;
                self.frames_written += 1;
            }
        }

        w.write_all(data).map_err(|e| format!("write raw: {e}"))?;
        self.frames_written += 1;
        self.last_frame = data.to_vec();
        self.last_video_ns = Some(master_ns);
        self.recorded_ns = master_ns.saturating_sub(start);
        Ok(())
    }

    /// Append audio samples (f32 interleaved) for one source, rendered on the
    /// master timeline. Frames arriving before the first video frame are
    /// dropped (the session starts at video start).
    pub fn push_audio(
        &mut self,
        source: &'static str,
        samples: &[f32],
        rate: u32,
        channels: u16,
        master_ns: u64,
    ) -> Result<(), String> {
        let Some(anchor) = self.video_start_ns else {
            return Ok(()); // video not started yet
        };
        if let Some((_, w)) = self.wavs.iter_mut().find(|(s, _)| *s == source) {
            w.push(samples, master_ns)
        } else {
            let dir = self.raw_path.parent().unwrap_or(Path::new("."));
            let mut w = WavWriter::create(dir, source, rate, channels, anchor)?;
            w.push(samples, master_ns)?;
            self.wavs.push((source, w));
            Ok(())
        }
    }

    /// Finish: tail-pad every WAV to the video duration, flush, then
    /// ffmpeg encode+mux → MP4 (1 WAV: direct; ≥2: aformat+amix). Cleanup raw.
    pub fn finish(&mut self, out_mp4: &Path) -> Result<PathBuf, String> {
        if let Some(mut w) = self.raw_writer.take() {
            let _ = w.flush();
        }

        // Tail pad (ADR-0005): every track spans the video duration, so the
        // mixed/encoded audio never ends before the video.
        let video_end_ns = self
            .video_start_ns
            .map(|a| a + self.recorded_ns)
            .unwrap_or(0);
        let mut wav_paths: Vec<PathBuf> = Vec::new();
        for (_, w) in self.wavs.iter_mut() {
            w.tail_pad_to(video_end_ns)?;
            wav_paths.push(w.finish()?);
        }

        let out = out_mp4.to_string_lossy().to_string();
        let mut args: Vec<String> = vec![
            "-y".into(),
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "bgra".into(),
            "-s".into(),
            format!("{}x{}", self.width, self.height),
            "-r".into(),
            self.fps.to_string(),
            "-i".into(),
            self.raw_path.to_string_lossy().to_string(),
        ];

        if wav_paths.is_empty() {
            // video-only
            args.extend([
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "ultrafast".into(),
                "-crf".into(),
                "23".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                out.clone(),
            ]);
        } else if wav_paths.len() == 1 {
            // single track: WAV is already timeline-aligned (starts at video
            // start by construction) — same args as the pre-mic aligned path.
            args.extend([
                "-i".into(),
                wav_paths[0].to_string_lossy().to_string(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "ultrafast".into(),
                "-crf".into(),
                "23".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "192k".into(),
                "-shortest".into(),
                out.clone(),
            ]);
        } else {
            // ≥2 tracks: normalize each (native rate/channels → 48k stereo)
            // then mix without attenuation, single AAC track out.
            for p in &wav_paths {
                args.extend(["-i".into(), p.to_string_lossy().to_string()]);
            }
            let mut fc = String::new();
            for i in 0..wav_paths.len() {
                // audio inputs start at index 1 (0 = rawvideo); labels from a0
                fc.push_str(&format!(
                    "[{src}:a]aformat=sample_rates={TARGET_RATE}:channel_layouts=stereo[a{i}];",
                    src = i + 1
                ));
            }
            let labels: String = (0..wav_paths.len())
                .map(|i| format!("[a{i}]"))
                .collect();
            fc.push_str(&format!(
                "{labels}amix=inputs={n}:normalize=0[aout]",
                n = wav_paths.len()
            ));
            args.extend([
                "-filter_complex".into(),
                fc,
                "-map".into(),
                "0:v".into(),
                "-map".into(),
                "[aout]".into(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "ultrafast".into(),
                "-crf".into(),
                "23".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "192k".into(),
                "-shortest".into(),
                out.clone(),
            ]);
        }

        let out_err = std::process::Stdio::piped();
        let child = Command::new("ffmpeg")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(out_err)
            .spawn()
            .map_err(|e| format!("spawn ffmpeg: {e}"))?;
        let mut child = child;
        let stderr_tail = child
            .stderr
            .take()
            .map(|mut s| {
                use std::io::Read;
                let mut buf = String::new();
                let _ = s.read_to_string(&mut buf);
                buf.lines().rev().take(4).collect::<Vec<_>>().join(" | ")
            })
            .unwrap_or_default();
        let status = child
            .wait()
            .map_err(|e| format!("wait ffmpeg: {e}"))?;

        if !status.success() {
            return Err(format!(
                "ffmpeg encode failed: {status}; last stderr: {stderr_tail}"
            ));
        }

        let _ = std::fs::remove_file(&self.raw_path);
        for p in &wav_paths {
            let _ = std::fs::remove_file(p);
        }
        Ok(PathBuf::from(&out))
    }
}

fn patch_wav_header(path: &Path, rate: u32, channels: u16) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let data_len = f.metadata()?.len().saturating_sub(44);

    let mut buf = [0u8; 44];
    buf[0..4].copy_from_slice(b"RIFF");
    buf[4..8].copy_from_slice(&((36u32 + data_len as u32) - 8).to_le_bytes());
    buf[8..12].copy_from_slice(b"WAVE");
    buf[12..16].copy_from_slice(b"fmt ");
    buf[16..20].copy_from_slice(&16u32.to_le_bytes());
    buf[20..22].copy_from_slice(&1u16.to_le_bytes());
    buf[22..24].copy_from_slice(&channels.to_le_bytes());
    buf[24..28].copy_from_slice(&rate.to_le_bytes());
    let byte_rate = rate * channels as u32 * 2;
    buf[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    buf[32..34].copy_from_slice(&(channels * 2).to_le_bytes());
    buf[34..36].copy_from_slice(&16u16.to_le_bytes());
    buf[36..40].copy_from_slice(b"data");
    buf[40..44].copy_from_slice(&(data_len as u32).to_le_bytes());

    f.seek(SeekFrom::Start(0))?;
    f.write_all(&buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_wav(path: &Path) -> (u32, u16, Vec<i16>) {
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut std::fs::File::open(path).unwrap(), &mut b).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..12], b"WAVE");
        let ch = u16::from_le_bytes(b[22..24].try_into().unwrap());
        let rate = u32::from_le_bytes(b[24..28].try_into().unwrap());
        let data = b[44..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        (rate, ch, data)
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn wav_gap_fills_silence() {
        let dir = std::env::temp_dir().join("sr-wav-test-gap");
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = WavWriter::create(&dir, "gap", 8_000, 1, 0).unwrap();

        let loud = vec![0.5f32; 100]; // 100 frames @ 8kHz = 12.5ms
        w.push(&loud, 0).unwrap();
        w.push(&loud, 1_000 * MS).unwrap(); // next frame at t=1s
        w.finish().unwrap();

        let (rate, ch, data) = read_wav(&dir.join("gap.wav"));
        assert_eq!((rate, ch), (8_000, 1));
        // cursor: 100 + silence(8000-100) + 100 = 8100
        assert_eq!(data.len(), 8_100);
        assert!(data[..100].iter().all(|&s| s != 0));
        assert!(data[200..7_900].iter().all(|&s| s == 0), "gap must be silence");
        assert!(data[8_000..].iter().all(|&s| s != 0));
    }

    #[test]
    fn wav_trims_audio_before_anchor() {
        let dir = std::env::temp_dir().join("sr-wav-test-trim");
        std::fs::create_dir_all(&dir).unwrap();
        // anchor (video start) = 1s; audio starts at 999ms → 1ms = 8 frames trimmed
        let mut w = WavWriter::create(&dir, "trim", 8_000, 1, 1_000 * MS).unwrap();
        w.push(&vec![0.5f32; 100], 999 * MS).unwrap();
        // next push lands at 1.1s (pos 800) — no overlap after the trim
        w.push(&vec![0.5f32; 100], 1_100 * MS).unwrap();
        w.finish().unwrap();

        let (_, _, data) = read_wav(&dir.join("trim.wav"));
        // trimmed to 92; silence 92..800 (708); then 100 → 900 total
        assert_eq!(data.len(), 900);
        assert!(data[..92].iter().all(|&s| s != 0));
        assert!(data[92..800].iter().all(|&s| s == 0));
        assert!(data[800..].iter().all(|&s| s != 0));
    }

    #[test]
    fn wav_late_start_leads_silence() {
        let dir = std::env::temp_dir().join("sr-wav-test-late");
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = WavWriter::create(&dir, "late", 8_000, 1, 0).unwrap();
        w.push(&vec![0.5f32; 100], 500 * MS).unwrap(); // starts at 0.5s
        w.finish().unwrap();

        let (_, _, data) = read_wav(&dir.join("late.wav"));
        assert_eq!(data.len(), 4_000 + 100);
        assert!(data[..3_900].iter().all(|&s| s == 0), "head must be silence");
    }

    #[test]
    fn wav_tail_pads_to_target() {
        let dir = std::env::temp_dir().join("sr-wav-test-tail");
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = WavWriter::create(&dir, "tail", 8_000, 1, 0).unwrap();
        w.push(&vec![0.5f32; 100], 0).unwrap();
        w.tail_pad_to(1_000 * MS).unwrap();
        w.finish().unwrap();

        let (_, _, data) = read_wav(&dir.join("tail.wav"));
        assert_eq!(data.len(), 8_000);
        assert!(data[200..].iter().all(|&s| s == 0));
    }

    #[test]
    fn muxer_routes_per_source() {
        let dir = std::env::temp_dir().join("sr-wav-test-mux");
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = Muxer::new(4, 4, 30);
        m.start(&dir).unwrap();
        let frame = vec![0u8; 4 * 4 * 4];
        m.push_video(&frame, 0).unwrap();

        // system @48k stereo, mic @16k mono — each gets its own WAV/rate
        m.push_audio("system", &vec![0.1f32; 96], 48_000, 2, 0).unwrap();
        m.push_audio("mic", &vec![0.2f32; 32], 16_000, 1, 0).unwrap();
        m.push_audio("mic", &vec![0.2f32; 32], 16_000, 1, 100 * MS).unwrap();

        let sys = m.wavs.iter().find(|(s, _)| *s == "system").unwrap();
        let mic = m.wavs.iter().find(|(s, _)| *s == "mic").unwrap();
        assert_eq!(sys.1.cursor_frames(), 48);
        // mic: 32 + silence(1600-32=1568) + 32 = 1632
        assert_eq!(mic.1.cursor_frames(), 1_632);
        assert_eq!(m.wavs.len(), 2);
    }
}
