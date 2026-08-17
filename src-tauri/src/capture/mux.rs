//! MP4 muxing & encoding (ADR-0008, jalur termurah-yang-benar).
//!
//! Tidak ada FFmpeg dev libs di sistem (hanya binary chocolatey), dan pipe
//! rawvideo → ffmpeg 8.0 gyan.dev mati di frame ~9 (bug build). Jadi M4:
//! - Video: tulis raw BGRA ke file sementara + WAV audio (kita tulis sendiri)
//! - Akhir: ffmpeg encode (libx264 ultrafast) dari file → H.264, mux WAV → MP4
//!
//! Kunci sync (ADR-0004): WGC mengirim frame SAAT LAYAR BERUBAH (VFR), bukan
//! 30fps rata. Kalau ditulis apa adanya + `-r 30`, video jadi "ngebut" (durasi
//! = frame_count/30) dan audio ketinggalan. Solusi: **CFR** — duplikasi frame
//! terakhir mengikuti timestamp master timeline sehingga video berdurasi
//! sungguhan, dan audio di-*align* ke video start.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

pub struct Muxer {
    width: u32,
    height: u32,
    fps: u32,
    raw_path: PathBuf,
    wav_path: PathBuf,
    raw_writer: Option<std::io::BufWriter<std::fs::File>>,
    wav_writer: Option<std::io::BufWriter<std::fs::File>>,
    frames_written: u64,
    wav_samples: u64,
    wav_rate: u32,
    wav_channels: u16,
    // CFR state
    last_frame: Vec<u8>,
    video_start_ns: Option<u64>,
    last_video_ns: Option<u64>,
    audio_start_ns: Option<u64>,
    audio_offset_ns: i64,
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
            wav_path: PathBuf::new(),
            raw_writer: None,
            wav_writer: None,
            frames_written: 0,
            wav_samples: 0,
            wav_rate: 48_000,
            wav_channels: 2,
            last_frame: Vec::new(),
            video_start_ns: None,
            last_video_ns: None,
            audio_start_ns: None,
            audio_offset_ns: 0,
            recorded_ns: 0,
        }
    }

    /// Begin a session; raw video + wav written into `dir`.
    pub fn start(&mut self, dir: &Path, audio_rate: u32, audio_channels: u16) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {dir:?}: {e}"))?;
        self.raw_path = dir.join("video.raw");
        self.wav_path = dir.join("audio.wav");

        let raw = std::fs::File::create(&self.raw_path)
            .map_err(|e| format!("create raw: {e}"))?;
        self.raw_writer = Some(std::io::BufWriter::new(raw));

        let wav = std::fs::File::create(&self.wav_path)
            .map_err(|e| format!("create wav: {e}"))?;
        let mut w = std::io::BufWriter::new(wav);
        w.write_all(&[0u8; 44]).map_err(|e| format!("wav header: {e}"))?;
        self.wav_writer = Some(w);

        self.wav_rate = audio_rate.max(1);
        self.wav_channels = audio_channels.max(1);
        Ok(())
    }

    /// Append one BGRA frame with its master-timeline position (ns).
    /// CFR: duplicate last frame to fill any gap before this frame.
    pub fn push_video(&mut self, data: &[u8], master_ns: u64) -> Result<(), String> {
        let Some(w) = self.raw_writer.as_mut() else {
            return Err("muxer not started".into());
        };

        if self.video_start_ns.is_none() {
            self.video_start_ns = Some(master_ns);
        }
        let start = self.video_start_ns.unwrap_or(master_ns);

        // If we already have a previous frame, duplicate it to fill the gap
        // so the video runs at constant 30fps wall-clock duration.
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

    /// Append audio samples (f32 interleaved) → WAV (s16).
    /// Tracks first audio timestamp to compute A/V start offset.
    pub fn push_audio(
        &mut self,
        samples: &[f32],
        rate: u32,
        channels: u16,
        master_ns: u64,
    ) -> Result<(), String> {
        let mut samples = samples;
        let mut local_offset: i64;

        if self.audio_start_ns.is_none() {
            self.audio_start_ns = Some(master_ns);
            if let Some(vs) = self.video_start_ns {
                local_offset = vs as i64 - master_ns as i64;
                if local_offset < 0 {
                    // audio starts EARLIER than video → trim leading samples
                    let trim_ns = (-local_offset) as u64;
                    let trim_samples = (trim_ns as f64 * rate as f64 / 1e9).ceil() as usize;
                    let to_skip = (trim_samples * channels as usize).min(samples.len());
                    samples = &samples[to_skip..];
                    local_offset = 0;
                }
                self.audio_offset_ns = local_offset;
            }
        }

        let Some(w) = self.wav_writer.as_mut() else {
            return Err("muxer not started".into());
        };
        let mut out = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&out).map_err(|e| format!("write wav: {e}"))?;
        self.wav_samples += (samples.len() as u64) / channels.max(1) as u64;
        Ok(())
    }

    /// Finish: close files, patch WAV, ffmpeg encode+mux → MP4, cleanup raw.
    /// Applies audio offset (`-itsoffset`) so A/V start together.
    pub fn finish(&mut self, out_mp4: &Path) -> Result<PathBuf, String> {
        if let Some(mut w) = self.raw_writer.take() {
            let _ = w.flush();
        }
        if let Some(mut w) = self.wav_writer.take() {
            let _ = w.flush();
            drop(w);
        }
        patch_wav_header(&self.wav_path, self.wav_rate, self.wav_channels)
            .map_err(|e| e.to_string())?;

        let out = out_mp4.to_string_lossy().to_string();
        let mut args: Vec<String> = vec![
            "-y".into(),
            "-f".into(), "rawvideo".into(),
            "-pix_fmt".into(), "bgra".into(),
            "-s".into(), format!("{}x{}", self.width, self.height),
            "-r".into(), self.fps.to_string(),
            "-i".into(), self.raw_path.to_string_lossy().to_string(),
        ];
        // audio input with optional start offset (align to video start)
        if self.audio_offset_ns != 0 {
            let secs = self.audio_offset_ns as f64 / 1e9;
            args.push("-itsoffset".into());
            args.push(format!("{secs:.6}"));
        }
        args.extend([
            "-i".into(),
            self.wav_path.to_string_lossy().to_string(),
            "-c:v".into(), "libx264".into(),
            "-preset".into(), "ultrafast".into(),
            "-crf".into(), "23".into(),
            "-pix_fmt".into(), "yuv420p".into(),
            "-c:a".into(), "aac".into(),
            "-b:a".into(), "192k".into(),
            "-shortest".into(),
            out.clone(),
        ]);

        let status = Command::new("ffmpeg")
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("spawn ffmpeg: {e}"))?;

        if !status.success() {
            return Err(format!("ffmpeg encode failed: {status}"));
        }

        let _ = std::fs::remove_file(&self.raw_path);
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
