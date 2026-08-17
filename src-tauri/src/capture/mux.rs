//! MP4 muxing & encoding (ADR-0008, jalur termurah-yang-benar).
//!
//! Tidak ada FFmpeg dev libs di sistem (hanya binary chocolatey), dan pipe
//! rawvideo → ffmpeg 8.0 gyan.dev mati di frame ~9 (bug build). Jadi M4:
//! - Video: tulis raw BGRA ke file sementara + WAV audio (kita tulis sendiri)
//! - Akhir: ffmpeg encode (libx264 ultrafast) dari file → H.264, mux WAV → MP4
//!
//! Bonus: raw file = data aman kalau app crash; encode terjadi sekali di akhir.

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
}

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
        // WAV header placeholder (44 bytes) — patched at finish.
        w.write_all(&[0u8; 44]).map_err(|e| format!("wav header: {e}"))?;
        self.wav_writer = Some(w);

        self.wav_rate = audio_rate.max(1);
        self.wav_channels = audio_channels.max(1);
        Ok(())
    }

    /// Append one BGRA frame.
    pub fn push_video(&mut self, data: &[u8]) -> Result<(), String> {
        let Some(w) = self.raw_writer.as_mut() else {
            return Err("muxer not started".into());
        };
        w.write_all(data).map_err(|e| format!("write raw: {e}"))?;
        self.frames_written += 1;
        Ok(())
    }

    /// Append audio samples (f32 interleaved) → WAV (s16).
    pub fn push_audio(&mut self, samples: &[f32], _rate: u32, _channels: u16) -> Result<(), String> {
        let Some(w) = self.wav_writer.as_mut() else {
            return Err("muxer not started".into());
        };
        let mut out = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&out).map_err(|e| format!("write wav: {e}"))?;
        self.wav_samples += (samples.len() as u64) / self.wav_channels.max(1) as u64;
        Ok(())
    }

    /// Finish: close files, patch WAV, ffmpeg encode+mux → MP4, cleanup raw.
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
        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-f", "rawvideo",
                "-pix_fmt", "bgra",
                "-s", &format!("{}x{}", self.width, self.height),
                "-r", &self.fps.to_string(),
                "-i", &self.raw_path.to_string_lossy().to_string(),
                "-i", &self.wav_path.to_string_lossy().to_string(),
                "-c:v", "libx264",
                "-preset", "ultrafast",
                "-crf", "23",
                "-pix_fmt", "yuv420p",
                "-c:a", "aac",
                "-b:a", "192k",
                "-shortest",
                &out,
            ])
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
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let data_len = f.metadata()?.len().saturating_sub(44);

    let mut buf = [0u8; 44];
    buf[0..4].copy_from_slice(b"RIFF");
    buf[4..8].copy_from_slice(&((36u32 + data_len as u32) - 8).to_le_bytes());
    buf[8..12].copy_from_slice(b"WAVE");
    buf[12..16].copy_from_slice(b"fmt ");
    buf[16..20].copy_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    buf[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    buf[22..24].copy_from_slice(&channels.to_le_bytes());
    buf[24..28].copy_from_slice(&rate.to_le_bytes());
    let byte_rate = rate * channels as u32 * 2;
    buf[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    buf[32..34].copy_from_slice(&(channels * 2).to_le_bytes());
    buf[34..36].copy_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf[36..40].copy_from_slice(b"data");
    buf[40..44].copy_from_slice(&(data_len as u32).to_le_bytes());

    f.seek(SeekFrom::Start(0))?;
    f.write_all(&buf)?;
    Ok(())
}
