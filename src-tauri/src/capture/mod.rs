//! Recording orchestrator (ADR-0009: flat, minimal).
//!
//! Connects video (platform) + audio (WASAPI loopback) sources → frames →
//! master clock → UI events. M3 adds audio capture and clock integration;
//! encoding + muxing land in M4.

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
use tokio::sync::{Mutex, oneshot};
use tauri::{AppHandle, Emitter};

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
    pub sync_offset_ms: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPayload {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
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
            sync_offset_ms: Arc::new(AtomicI64::new(0)),
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
    ) -> Result<(), String> {
        if self.state.recording.load(Ordering::Relaxed) {
            return Err("already recording".into());
        }

        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        // Video: broadcast so both preview (expensive) and sync pump can read.
        let (vtx, _) = tokio::sync::broadcast::channel::<platform::VideoFrame>(64);
        let mut vrx = vtx.subscribe();
        let mut vrx_2 = vtx.subscribe();
        let (atx, mut arx) = tokio::sync::mpsc::channel::<audio::AudioFrame>(512);

        let clock = MasterClock::new(clock::DEFAULT_SAMPLE_RATE);
        set_global_clock(clock.clone());
        let audio_clock_state = Arc::new(std::sync::Mutex::new(SourceClockState::new("system-audio")));
        let video_clock_state = Arc::new(std::sync::Mutex::new(SourceClockState::new("screen-video")));

        // Video capture task
        let state = self.state.clone();
        let state_audio = state.clone();
        let state_pump = state.clone();
        let app_cap = app.clone();
        let cap_task = tokio::spawn(async move {
            let mut cap = platform::create_capture(target, 30);
            if let Err(e) = cap.start(vtx).await {
                let _ = app_cap.emit(
                    "record-status",
                    RecordStatus {
                        state: "error".into(),
                        duration_ms: 0,
                        file_path: None,
                        error: Some(e),
                        frames_captured: 0,
                        frames_dropped: 0,
                        audio_frames: 0,
                        sync_offset_ms: 0,
                    },
                );
                state.recording.store(false, Ordering::Relaxed);
                return;
            }
            let _ = stop_rx.await;
            let _ = cap.stop().await;
            state.recording.store(false, Ordering::Relaxed);
        });

        // Audio capture task (WASAPI loopback) — runs on a std thread because
        // cpal::Stream is not Send; tokio channel senders are Send + Sync.
        let audio_frames_counter = self.state.audio_frames.clone();
        let state_audio2 = state_audio.clone();
        let audio_task = std::thread::spawn(move || {
            let mut capturer = match audio::SystemAudioCapturer::new() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[audio] init error: {e}");
                    return;
                }
            };
            if let Err(e) = capturer.start(atx) {
                eprintln!("[audio] start error: {e}");
                return;
            }
            // Keep the capturer alive until recording stops.
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if !recording_flag(&state_audio2) {
                    break;
                }
            }
            let _ = capturer.stop();
            let _ = audio_frames_counter.load(Ordering::Relaxed);
        });

        // Sync monitor: remap video & audio frames onto master timeline,
        // measure A/V offset (drift).
        let app_sync = app.clone();
        let audio_frames_counter2 = self.state.audio_frames.clone();
        let sync_offset = self.state.sync_offset_ms.clone();
        let video_cs2 = video_clock_state.clone();
        let audio_cs2 = audio_clock_state.clone();
        let frames_captured2 = self.state.frames_captured.clone();
        let frames_captured3 = self.state.frames_captured.clone();
        let app_sync2 = app.clone();
        let preview_task = tokio::spawn(async move {
            // Preview is expensive (downscale + IPC); keep it off the sync path.
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

        let pump = tokio::spawn(async move {
            #[allow(unused_assignments)]
            let mut last_audio_ns: Option<u64> = None;
            let mut last_video_ns: Option<u64> = None;
            let mut muxer: Option<mux::Muxer> = None;
            let mut vw: u32 = 0;
            let mut vh: u32 = 0;
            let mut mux_started = false;

            loop {
                tokio::select! {
                    Ok(vf) = vrx_2.recv() => {
                        let remap = {
                            let mut cs = video_cs2.lock().unwrap();
                            cs.remap(&clock2(), vf.timestamp, 33_333_333)
                        };
                        last_video_ns = Some(remap.master_ns);
                        // start muxer on first frame (know width/height)
                        if muxer.is_none() {
                            vw = vf.width; vh = vf.height;
                            let mut m = mux::Muxer::new(vf.width, vf.height, 30);
                            let dir = std::env::temp_dir().join("screen-record-m4");
                            let _ = std::fs::create_dir_all(&dir);
                            m.start(&dir, 48_000, 2).ok();
                            muxer = Some(m);
                            mux_started = true;
                            eprintln!("[mux] started {w}x{h}", w = vf.width, h = vf.height);
                        }
                        if let Some(m) = muxer.as_mut() {
                            let _ = m.push_video(&vf.data, remap.master_ns);
                        }
                    }
                    Some(af) = arx.recv() => {
                        audio_frames_counter2.fetch_add(1, Ordering::Relaxed);
                        let frame_ns = (af.samples.len() as u64 * 1_000_000_000 / (af.sample_rate as u64 * af.channels as u64)).max(1);
                        let remap = {
                            let mut cs = audio_cs2.lock().unwrap();
                            cs.remap(&clock2(), af.timestamp, frame_ns)
                        };
                        last_audio_ns = Some(remap.master_ns);
                        // live drift
                        if let (Some(v), Some(a)) = (last_video_ns, last_audio_ns) {
                            let off = v as i64 - a as i64;
                            sync_offset.store(off / 1_000_000, Ordering::Relaxed);
                        }
                        if let Some(m) = muxer.as_mut() {
                            let _ = m.push_audio(&af.samples, af.sample_rate, af.channels, remap.master_ns);
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                        if !recording_flag(&state_pump) {
                            break;
                        }
                    }
                }
            }
            let _ = (mux_started, vw, vh);

            // Finish muxing → MP4 in ~/Videos/screen-record
            if let Some(m) = muxer.as_mut() {
                let videos = dirs_videos_dir();
                let out = videos.join(format!(
                    "rec-{}.mp4",
                    chrono_like_timestamp()
                ));
                match m.finish(&out) {
                    Ok(p) => {
                        eprintln!("[mux] MP4 saved: {}", p.display());
                        let _ = app_sync2.emit(
                            "record-status",
                            RecordStatus {
                                state: "finished".into(),
                                duration_ms: last_audio_ns.map(|n| n / 1_000_000).unwrap_or(0),
                                file_path: Some(p.to_string_lossy().to_string()),
                                error: None,
                                frames_captured: frames_captured3.load(Ordering::Relaxed),
                                frames_dropped: 0,
                                audio_frames: audio_frames_counter2.load(Ordering::Relaxed),
                                sync_offset_ms: sync_offset.load(Ordering::Relaxed),
                            },
                        );
                    }
                    Err(e) => {
                        eprintln!("[mux] finish error: {e}");
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
        let _ = audio_task;

        let _ = app.emit(
            "record-status",
            RecordStatus {
                state: "recording".into(),
                duration_ms: 0,
                file_path: None,
                error: None,
                frames_captured: 0,
                frames_dropped: 0,
                audio_frames: 0,
                sync_offset_ms: 0,
            },
        );

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

        let _ = app.emit(
            "record-status",
            RecordStatus {
                state: "idle".into(),
                duration_ms: 0,
                file_path: None,
                error: None,
                frames_captured: self.state.frames_captured.load(Ordering::Relaxed),
                frames_dropped: self.state.frames_dropped.load(Ordering::Relaxed),
                audio_frames: self.state.audio_frames.load(Ordering::Relaxed),
                sync_offset_ms: self.state.sync_offset_ms.load(Ordering::Relaxed),
            },
        );
        Ok("stopped".into())
    }

    pub async fn is_recording(&self) -> bool {
        self.state.recording.load(Ordering::Relaxed)
    }
}

// ---- helpers (kept small; M4 replaces some) ----

fn recording_flag(state: &Arc<RecorderState>) -> bool {
    state.recording.load(Ordering::Relaxed)
}

fn clock2() -> Arc<MasterClock> {
    global_clock()
}

fn set_global_clock(c: Arc<MasterClock>) {
    static CLOCK: std::sync::OnceLock<Arc<MasterClock>> = std::sync::OnceLock::new();
    let _ = CLOCK.set(c);
}

fn global_clock() -> Arc<MasterClock> {
    static CLOCK: std::sync::OnceLock<Arc<MasterClock>> = std::sync::OnceLock::new();
    CLOCK
        .get_or_init(|| MasterClock::new(clock::DEFAULT_SAMPLE_RATE))
        .clone()
}


fn dirs_videos_dir() -> PathBuf {
    use std::path::PathBuf;
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
