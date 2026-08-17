//! Recording orchestrator (ADR-0009: flat, minimal).
//!
//! Connects video (platform) + audio sources (system loopback, mic —
//! ADR-0012) → MasterClock remap → per-source WAVs (ADR-0013) → UI events.

pub mod audio;
pub mod clock;
pub mod mux;
pub mod platform;
pub mod timeline;

use platform::ScreenCapture as _;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, oneshot};
use tauri::{AppHandle, Emitter};

use audio::{AudioMode, AudioOpts};
use clock::{MasterClock, SourceClockState};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStatus {
    pub state: String,
    pub duration_ms: u64,
    pub file_path: Option<String>,
    pub error: Option<String>,
    pub frames_captured: u64,
    pub frames_dropped: u64,
    pub audio_frames: u64,
    pub mic_frames: u64,
    pub mic_drops: u64,
    pub sync_offset_ms: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPayload {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMeter {
    pub rms: f32,
}

pub struct Recorder {
    state: Arc<RecorderState>,
}

struct RecorderState {
    recording: AtomicBool,
    stop_tx: Mutex<Option<oneshot::Sender<()>>>,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    frames_captured: Arc<AtomicU64>,
    frames_dropped: Arc<AtomicU64>,
    audio_frames: Arc<AtomicU64>,
    mic_frames: Arc<AtomicU64>,
    mic_drops: Arc<AtomicU64>,
    sync_offset_ms: Arc<AtomicI64>,
}

impl RecorderState {
    fn new() -> Self {
        Self {
            recording: AtomicBool::new(false),
            stop_tx: Mutex::new(None),
            handle: Mutex::new(None),
            frames_captured: Arc::new(AtomicU64::new(0)),
            frames_dropped: Arc::new(AtomicU64::new(0)),
            audio_frames: Arc::new(AtomicU64::new(0)),
            mic_frames: Arc::new(AtomicU64::new(0)),
            mic_drops: Arc::new(AtomicU64::new(0)),
            sync_offset_ms: Arc::new(AtomicI64::new(0)),
        }
    }

    fn status(&self, state_name: &str, duration_ms: u64) -> RecordStatus {
        RecordStatus {
            state: state_name.into(),
            duration_ms,
            file_path: None,
            error: None,
            frames_captured: self.frames_captured.load(Ordering::Relaxed),
            frames_dropped: self.frames_dropped.load(Ordering::Relaxed),
            audio_frames: self.audio_frames.load(Ordering::Relaxed),
            mic_frames: self.mic_frames.load(Ordering::Relaxed),
            mic_drops: self.mic_drops.load(Ordering::Relaxed),
            sync_offset_ms: self.sync_offset_ms.load(Ordering::Relaxed),
        }
    }
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RecorderState::new()),
        }
    }

    pub async fn start(
        &self,
        app: AppHandle,
        target: platform::CaptureTarget,
        audio: AudioOpts,
    ) -> Result<(), String> {
        if self.state.recording.load(Ordering::Relaxed) {
            return Err("already recording".into());
        }

        // Reset per-session counters (state lives for the app lifetime).
        for c in [
            &self.state.frames_captured,
            &self.state.frames_dropped,
            &self.state.audio_frames,
            &self.state.mic_frames,
            &self.state.mic_drops,
        ] {
            c.store(0, Ordering::Relaxed);
        }
        self.state.sync_offset_ms.store(0, Ordering::Relaxed);

        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        // Video: broadcast so both preview (expensive) and sync pump can read.
        let (vtx, _) = tokio::sync::broadcast::channel::<platform::VideoFrame>(64);
        let mut vrx = vtx.subscribe();
        let mut vrx_2 = vtx.subscribe();
        let (atx_sys, mut arx_sys) = tokio::sync::mpsc::channel::<audio::AudioFrame>(512);
        let (atx_mic, mut arx_mic) = tokio::sync::mpsc::channel::<audio::AudioFrame>(512);

        // Per-session clock (fixes the old global-OnceLock bug where session 2
        // reused session 1's timeline).
        let clock = MasterClock::new(clock::DEFAULT_SAMPLE_RATE);
        let sys_clock_state = Arc::new(std::sync::Mutex::new(SourceClockState::new("system-audio")));
        let mic_clock_state = Arc::new(std::sync::Mutex::new(SourceClockState::new("mic-audio")));
        let video_clock_state = Arc::new(std::sync::Mutex::new(SourceClockState::new("screen-video")));

        // Video capture task
        let state = self.state.clone();
        let app_cap = app.clone();
        let cap_task = tokio::spawn(async move {
            let mut cap = platform::create_capture(target, 30);
            if let Err(e) = cap.start(vtx).await {
                let mut st = state.status("error", 0);
                st.error = Some(e);
                let _ = app_cap.emit("record-status", st);
                state.recording.store(false, Ordering::Relaxed);
                return;
            }
            let _ = stop_rx.await;
            let _ = cap.stop().await;
            state.recording.store(false, Ordering::Relaxed);
        });

        // Audio capture threads (WASAPI via CPAL) — std::thread because
        // cpal::Stream is not Send. Mic thread re-inits the stream on error
        // (unplug / BT format switch — Cap pattern); gap-fill keeps the WAV
        // timeline valid across the outage.
        if audio.system {
            spawn_audio_thread(AudioMode::System, atx_sys, self.state.clone(), None);
        }
        if audio.mic {
            spawn_audio_thread(
                AudioMode::Mic {
                    device: audio.mic_device.clone(),
                },
                atx_mic,
                self.state.clone(),
                Some(self.state.mic_drops.clone()),
            );
        }

        // Preview task (downscale + IPC; off the sync path)
        let state_preview = self.state.clone();
        let app_sync = app.clone();
        let frames_captured2 = self.state.frames_captured.clone();
        let preview_task = tokio::spawn(async move {
            loop {
                match vrx.recv().await {
                    Ok(vf) => {
                        frames_captured2.fetch_add(1, Ordering::Relaxed);
                        emit_preview(&app_sync, &vf);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Preview fell behind; skip frames and keep going.
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let _ = preview_task;
        let _ = state_preview;

        // Sync pump: remap video + both audio sources onto the master
        // timeline, write per-source WAVs, meter the mic, finalize the MP4.
        let state_pump = self.state.clone();
        let app_sync2 = app.clone();
        let sync_offset = self.state.sync_offset_ms.clone();
        let video_cs2 = video_clock_state.clone();
        let sys_cs2 = sys_clock_state.clone();
        let mic_cs2 = mic_clock_state.clone();
        let mic_enabled = audio.mic;
        let pump = tokio::spawn(async move {
            let mut last_audio_ns: Option<u64> = None;
            let mut last_video_ns: Option<u64> = None;
            let mut muxer: Option<mux::Muxer> = None;
            let mut last_meter = Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now);

            loop {
                tokio::select! {
                    Ok(vf) = vrx_2.recv() => {
                        let remap = {
                            let mut cs = video_cs2.lock().unwrap();
                            cs.remap(&clock, vf.timestamp, 33_333_333)
                        };
                        last_video_ns = Some(remap.master_ns);
                        // start muxer on first frame (know width/height)
                        if muxer.is_none() {
                            let mut m = mux::Muxer::new(vf.width, vf.height, 30);
                            let dir = std::env::temp_dir().join("screen-record-m7");
                            m.start(&dir).ok();
                            muxer = Some(m);
                            eprintln!("[mux] started {w}x{h}", w = vf.width, h = vf.height);
                        }
                        if let Some(m) = muxer.as_mut() {
                            let _ = m.push_video(&vf.data, remap.master_ns);
                        }
                    }
                    Some(af) = arx_sys.recv() => {
                        state_pump.audio_frames.fetch_add(1, Ordering::Relaxed);
                        let frame_ns = (af.samples.len() as u64 * 1_000_000_000
                            / (af.sample_rate as u64 * af.channels as u64))
                            .max(1);
                        let remap = {
                            let mut cs = sys_cs2.lock().unwrap();
                            cs.remap(&clock, af.timestamp, frame_ns)
                        };
                        last_audio_ns = Some(remap.master_ns);
                        if let Some(m) = muxer.as_mut() {
                            let _ = m.push_audio("system", &af.samples, af.sample_rate, af.channels, remap.master_ns);
                        }
                    }
                    Some(af) = arx_mic.recv() => {
                        state_pump.mic_frames.fetch_add(1, Ordering::Relaxed);
                        let frame_ns = (af.samples.len() as u64 * 1_000_000_000
                            / (af.sample_rate as u64 * af.channels as u64))
                            .max(1);
                        let remap = {
                            let mut cs = mic_cs2.lock().unwrap();
                            cs.remap(&clock, af.timestamp, frame_ns)
                        };
                        if last_audio_ns.is_none() {
                            last_audio_ns = Some(remap.master_ns);
                        }
                        // Mic level meter (RMS, throttled ~100ms)
                        if mic_enabled {
                            let now = Instant::now();
                            if now.duration_since(last_meter) >= Duration::from_millis(100) {
                                last_meter = now;
                                let n = af.samples.len().max(1) as f32;
                                let rms = (af.samples.iter().map(|s| s * s).sum::<f32>() / n).sqrt();
                                let _ = app_sync2.emit("audio-meter", AudioMeter { rms });
                            }
                        }
                        if let Some(m) = muxer.as_mut() {
                            let _ = m.push_audio("mic", &af.samples, af.sample_rate, af.channels, remap.master_ns);
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {
                        if !state_pump.recording.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                }
            }

            // live drift for the status card
            if let (Some(v), Some(a)) = (last_video_ns, last_audio_ns) {
                sync_offset.store((v as i64 - a as i64) / 1_000_000, Ordering::Relaxed);
            }

            // Finish muxing → MP4 in ~/Videos/screen-record
            if let Some(m) = muxer.as_mut() {
                let videos = dirs_videos_dir();
                let out = videos.join(format!("rec-{}.mp4", chrono_like_timestamp()));
                match m.finish(&out) {
                    Ok(p) => {
                        eprintln!("[mux] MP4 saved: {}", p.display());
                        let mut st = state_pump.status("finished", last_audio_ns.map(|n| n / 1_000_000).unwrap_or(0));
                        st.file_path = Some(p.to_string_lossy().to_string());
                        let _ = app_sync2.emit("record-status", st);
                    }
                    Err(e) => {
                        eprintln!("[mux] finish error: {e}");
                        let mut st = state_pump.status("error", 0);
                        st.error = Some(e);
                        let _ = app_sync2.emit("record-status", st);
                    }
                }
            }
        });

        {
            let mut st = self.state.stop_tx.lock().await;
            *st = Some(stop_tx);
        }
        {
            let mut h = self.state.handle.lock().await;
            *h = Some(cap_task);
        }
        self.state.recording.store(true, Ordering::Relaxed);
        let _ = pump;

        let _ = app.emit("record-status", self.state.status("recording", 0));

        Ok(())
    }

    pub async fn stop(&self, app: AppHandle) -> Result<String, String> {
        if !self.state.recording.load(Ordering::Relaxed) {
            return Err("not recording".into());
        }
        self.state.recording.store(false, Ordering::Relaxed);

        let stop_tx = self.state.stop_tx.lock().await.take();
        if let Some(tx) = stop_tx {
            let _ = tx.send(());
        }
        if let Some(h) = self.state.handle.lock().await.take() {
            let _ = h.await;
        }

        let _ = app.emit("record-status", self.state.status("idle", 0));
        Ok("stopped".into())
    }

    pub async fn is_recording(&self) -> bool {
        self.state.recording.load(Ordering::Relaxed)
    }
}

/// Run one audio source for the whole session; on stream error, rebuild the
/// capturer (new device config) while recording continues — gap-fill silence
/// keeps the WAV timeline valid (ADR-0013 failure handling).
fn spawn_audio_thread(
    mode: AudioMode,
    tx: tokio::sync::mpsc::Sender<audio::AudioFrame>,
    state: Arc<RecorderState>,
    drops_slot: Option<Arc<AtomicU64>>,
) {
    std::thread::spawn(move || {
        let label = match mode {
            AudioMode::System => "system",
            AudioMode::Mic { .. } => "mic",
        };
        let mut drops_base: u64 = 0;
        loop {
            let mut cap = match audio::AudioCapturer::new(mode.clone()) {
                Ok(c) => c,
                Err(e) => {
                    // No device at start → record continues without this
                    // source (PRD PD-0002 §4.1); log and give up.
                    eprintln!("[audio:{label}] init error: {e}");
                    return;
                }
            };
            if let Err(e) = cap.start(tx.clone()) {
                eprintln!("[audio:{label}] start error: {e}");
                return;
            }
            eprintln!("[audio:{label}] started ({}Hz, {}ch)", cap.sample_rate, cap.channels);

            loop {
                std::thread::sleep(Duration::from_millis(200));
                if !state.recording.load(Ordering::Relaxed) {
                    let _ = cap.stop();
                    if let Some(slot) = &drops_slot {
                        slot.store(drops_base + cap.drops(), Ordering::Relaxed);
                    }
                    return;
                }
                if let Some(slot) = &drops_slot {
                    slot.store(drops_base + cap.drops(), Ordering::Relaxed);
                }
                if cap.errored() {
                    // ponytail: 1s fixed backoff; exponential if reconnect storms show up
                    eprintln!("[audio:{label}] stream error → re-init");
                    let drops_now = drops_base + cap.drops();
                    let _ = cap.stop();
                    drops_base = drops_now;
                    if let Some(slot) = &drops_slot {
                        slot.store(drops_base, Ordering::Relaxed);
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    break;
                }
            }
            // rebuild (device may have come back / changed format)
            continue;
        }
    });
}

// ---- helpers (kept small) ----

fn dirs_videos_dir() -> PathBuf {
    // ~/Videos/screen-record (fallback to temp)
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let videos = PathBuf::from(home).join("Videos").join("screen-record");
    let _ = std::fs::create_dir_all(&videos);
    videos
}

fn chrono_like_timestamp() -> String {
    // epoch secs (filename uniquifier; no chrono dep)
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("m{}", now.as_secs())
}

fn emit_preview(app: &AppHandle, vf: &platform::VideoFrame) {
    let max_w = 480u32;
    let scale = if vf.width > max_w {
        max_w as f32 / vf.width as f32
    } else {
        1.0
    };
    let ow = (vf.width as f32 * scale).max(1.0) as u32;
    let oh = (vf.height as f32 * scale).max(1.0) as u32;
    let mut small = Vec::with_capacity((ow * oh * 4) as usize);
    for y in 0..oh {
        let sy = ((y as f32 / scale).min((vf.height - 1) as f32)) as usize;
        for x in 0..ow {
            let sx = ((x as f32 / scale).min((vf.width - 1) as f32)) as usize;
            let si = (sy * vf.width as usize + sx) * 4;
            small.extend_from_slice(&vf.data[si..si + 4]);
        }
    }
    let _ = app.emit(
        "preview-frame",
        &PreviewPayload {
            data: small,
            width: ow,
            height: oh,
        },
    );
}
