//! Audio capture (ADR-0007, ADR-0012): CPAL on the WASAPI host.
//!
//! Two modes, one code path:
//! - `System` — loopback: INPUT stream on the default OUTPUT device + a
//!   silent render "keepalive" so packets flow even when nothing plays
//!   (OBS trick). Timestamps anchor to QPC → MasterClock.
//! - `Mic`    — normal INPUT stream on the selected input device. Same
//!   callback, same QPC anchor family → same MasterClock (ADR-0003), so
//!   mic / system / video are on one timeline with no extra sync code.

use crate::capture::clock::RawTimestamp;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

/// One chunk of captured audio.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Interleaved f32 samples at the device's native rate.
    pub samples: Vec<f32>,
    /// QPC timestamp of the capture instant.
    pub timestamp: RawTimestamp,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone)]
pub enum AudioMode {
    /// System audio: WASAPI loopback on the default output device.
    System,
    /// Microphone: input stream on the named device (default if None).
    Mic { device: Option<String> },
}

/// Which audio sources a session records (PRD PD-0002 §4.1).
#[derive(Debug, Clone, Default)]
pub struct AudioOpts {
    pub system: bool,
    pub mic: bool,
    pub mic_device: Option<String>,
}

pub struct AudioCapturer {
    mode: AudioMode,
    stream: Option<cpal::Stream>,
    keepalive: Option<cpal::Stream>,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    drops: Arc<AtomicU64>,
    errored: Arc<AtomicBool>,
    pub sample_rate: u32,
    pub channels: u16,
}

fn safe_buffer_frames(supported: &cpal::SupportedBufferSize, sample_rate: u32) -> cpal::BufferSize {
    match supported {
        cpal::SupportedBufferSize::Range { min, max } => {
            let target = if sample_rate > 0 {
                ((sample_rate as u64 * 80) / 1000).clamp(256, 16384) as u32
            } else {
                4096
            };
            cpal::BufferSize::Fixed(target.clamp(*min, *max))
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    }
}

/// Resolve the device + its default config for a mode (fail fast).
fn resolve_device(mode: &AudioMode) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let host = cpal::default_host();
    match mode {
        AudioMode::System => {
            let dev = host
                .default_output_device()
                .ok_or("no default output device")?;
            let cfg = dev
                .default_output_config()
                .map_err(|e| format!("default_output_config: {e}"))?;
            Ok((dev, cfg))
        }
        AudioMode::Mic { device } => {
            let dev = match device {
                None => host
                    .default_input_device()
                    .ok_or("no default input device")?,
                Some(name) => host
                    .input_devices()
                    .map_err(|e| format!("input_devices: {e}"))?
                    .find(|d| d.name().ok().as_deref() == Some(name.as_str()))
                    .ok_or_else(|| format!("input device not found: {name}"))?,
            };
            let cfg = dev
                .default_input_config()
                .map_err(|e| format!("default_input_config: {e}"))?;
            Ok((dev, cfg))
        }
    }
}

impl AudioCapturer {
    pub fn new(mode: AudioMode) -> Result<Self, String> {
        let (_, cfg) = resolve_device(&mode)?;
        Ok(Self {
            mode,
            stream: None,
            keepalive: None,
            stop: Arc::new(AtomicBool::new(false)),
            frames: Arc::new(AtomicU64::new(0)),
            drops: Arc::new(AtomicU64::new(0)),
            errored: Arc::new(AtomicBool::new(false)),
            sample_rate: cfg.sample_rate().0,
            channels: cfg.channels(),
        })
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }
    pub fn drops(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }
    /// Set by the CPAL error callback (device lost / format change).
    pub fn errored(&self) -> bool {
        self.errored.load(Ordering::Relaxed)
    }

    /// Start capture, pushing frames into `tx`.
    pub fn start(&mut self, tx: tokio::sync::mpsc::Sender<AudioFrame>) -> Result<(), String> {
        let (device, supported) = resolve_device(&self.mode)?;

        let sample_format = supported.sample_format();
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels();

        let mut config: cpal::StreamConfig = supported.clone().into();
        config.buffer_size = safe_buffer_frames(supported.buffer_size(), sample_rate);

        let stop = self.stop.clone();
        let frames = self.frames.clone();
        let drops = self.drops.clone();
        let errored = self.errored.clone();

        let stream = device
            .build_input_stream_raw(
                &config,
                sample_format,
                move |data, info| {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }

                    // QPC timestamp from CPAL (same clock family for loopback
                    // and mic input — both WASAPI) → one MasterClock.
                    let samples = match sample_format {
                        cpal::SampleFormat::F32 => {
                            data.as_slice::<f32>().unwrap_or(&[]).to_vec()
                        }
                        cpal::SampleFormat::I16 => {
                            let s = data.as_slice::<i16>().unwrap_or(&[]);
                            s.iter().map(|&v| v as f32 / 32768.0).collect()
                        }
                        cpal::SampleFormat::U16 => {
                            let s = data.as_slice::<u16>().unwrap_or(&[]);
                            s.iter().map(|&v| (v as f32 / 32768.0) - 1.0).collect()
                        }
                        cpal::SampleFormat::I32 => {
                            let s = data.as_slice::<i32>().unwrap_or(&[]);
                            s.iter().map(|&v| v as f32 / 2147483648.0).collect()
                        }
                        cpal::SampleFormat::F64 => {
                            let s = data.as_slice::<f64>().unwrap_or(&[]);
                            s.iter().map(|&v| v as f32).collect()
                        }
                        other => {
                            drops.fetch_add(1, Ordering::Relaxed);
                            eprintln!("[audio] unsupported format {other:?}");
                            return;
                        }
                    };

                    let ts_qpc = qpc_from_cpal_instant(info.timestamp().capture);
                    frames.fetch_add(1, Ordering::Relaxed);
                    let frame = AudioFrame {
                        samples,
                        timestamp: ts_qpc,
                        sample_rate,
                        channels,
                    };
                    // Never block the audio callback: WASAPI drops packets if
                    // we hold the callback. try_send + drop counter instead.
                    if tx.try_send(frame).is_err() {
                        drops.fetch_add(1, Ordering::Relaxed);
                    }
                },
                move |err| {
                    eprintln!("[audio] stream error: {err}");
                    errored.store(true, Ordering::Relaxed);
                },
                None,
            )
            .map_err(|e| format!("build_input_stream_raw: {e}"))?;

        // Silence keepalive: only loopback needs it (mic streams always flow).
        let keepalive = match self.mode {
            AudioMode::System => build_silence_keepalive(&device),
            AudioMode::Mic { .. } => None,
        };

        stream.play().map_err(|e| format!("stream.play: {e}"))?;
        if let Some(k) = &keepalive {
            let _ = k.play();
        }

        self.stream = Some(stream);
        self.keepalive = keepalive;
        self.sample_rate = sample_rate;
        self.channels = channels;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(s) = self.stream.take() {
            let _ = s.pause();
        }
        if let Some(k) = self.keepalive.take() {
            let _ = k.pause();
        }
        Ok(())
    }
}

fn build_silence_keepalive(output: &cpal::Device) -> Option<cpal::Stream> {
    let supported = output.default_output_config().ok()?;
    let mut config: cpal::StreamConfig = supported.clone().into();
    config.buffer_size = cpal::BufferSize::Default;

    output
        .build_output_stream_raw(
            &config,
            supported.sample_format(),
            |data, _| {
                data.bytes_mut().fill(0);
            },
            |_| {},
            None,
        )
        .ok()
}

/// Convert CPAL capture instant to QPC ticks (approximated via a QPC anchor
/// captured at stream start + elapsed wall time).
/// On WASAPI, `StreamInstant` is QPC-backed, but stock CPAL does not expose
/// the raw counter; anchoring at start and adding elapsed keeps timestamps
/// on the same monotonic family as WGC (both are QPC-derived).
#[cfg(windows)]
fn qpc_from_cpal_instant(instant: cpal::StreamInstant) -> RawTimestamp {
    use std::sync::OnceLock;
    use std::time::Duration;

    static ANCHOR: OnceLock<(cpal::StreamInstant, i64)> = OnceLock::new();
    let (anchor_instant, anchor_qpc) = *ANCHOR.get_or_init(|| {
        // Capture a QPC counter at the first callback; StreamInstant values are
        // comparable via duration_since, so the delta maps to QPC delta.
        let mut v: i64 = 0;
        unsafe {
            windows::Win32::System::Performance::QueryPerformanceCounter(&mut v)
        }
        .unwrap_or_default();
        (instant, v)
    });

    let elapsed = instant
        .duration_since(&anchor_instant)
        .unwrap_or(Duration::ZERO);
    let freq = crate::capture::clock::qpc_frequency();
    let ticks = anchor_qpc + (elapsed.as_nanos() as f64 * freq as f64 / 1e9) as i64;
    RawTimestamp::from_qpc(ticks)
}

#[cfg(not(windows))]
fn qpc_from_cpal_instant(_instant: cpal::StreamInstant) -> RawTimestamp {
    RawTimestamp::from_qpc(0)
}
