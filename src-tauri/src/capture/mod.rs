//! Recording orchestrator (ADR-0009: flat, minimal).
//!
//! Connects the platform capture source → frames → UI events. v1 (M2):
//! captures video frames and emits them as preview frames. Encoding + muxing
//! land in M4; clock/timeline units are here and tested (ADR-0003/0004/0005).

pub mod clock;
pub mod platform;
pub mod timeline;

use platform::ScreenCapture as _;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{Mutex, oneshot};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStatus {
    pub state: String,
    pub duration_ms: u64,
    pub file_path: Option<String>,
    pub error: Option<String>,
    pub frames_captured: u64,
    pub frames_dropped: u64,
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
}

impl RecorderState {
    fn new() -> Self {
        Self {
            recording: AtomicBool::new(false),
            stop_tx: Mutex::new(None),
            handle: Mutex::new(None),
            frames_captured: Arc::new(AtomicU64::new(0)),
            frames_dropped: Arc::new(AtomicU64::new(0)),
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
        let (tx, mut rx) = tokio::sync::mpsc::channel::<platform::VideoFrame>(16);

        // Capture task: owns the platform capture, stops on signal.
        let state = self.state.clone();
        let app_cap = app.clone();
        let cap_task = tokio::spawn(async move {
            let mut cap = platform::create_capture(target, 30);
            if let Err(e) = cap.start(tx).await {
                let _ = app_cap.emit(
                    "record-status",
                    RecordStatus {
                        state: "error".into(),
                        duration_ms: 0,
                        file_path: None,
                        error: Some(e),
                        frames_captured: 0,
                        frames_dropped: 0,
                    },
                );
                state.recording.store(false, Ordering::Relaxed);
                return;
            }
            let _ = stop_rx.await;
            let _ = cap.stop().await;
            state.recording.store(false, Ordering::Relaxed);
        });

        // Preview pump: forward frames to the UI as events (downscaled).
        let app_pump = app.clone();
        let frames_captured = self.state.frames_captured.clone();
        let pump = tokio::spawn(async move {
            let mut last_log = std::time::Instant::now();
            while let Some(vf) = rx.recv().await {
                frames_captured.fetch_add(1, Ordering::Relaxed);
                if last_log.elapsed() >= std::time::Duration::from_secs(5) {
                    eprintln!(
                        "[record] preview frames: {} ({w}x{h})",
                        frames_captured.load(Ordering::Relaxed),
                        w = vf.width,
                        h = vf.height
                    );
                    last_log = std::time::Instant::now();
                }
                // Downscale to ≤480px wide (bilinear-ish nearest) so IPC is small.
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
                let payload = PreviewPayload {
                    data: small,
                    width: ow,
                    height: oh,
                };
                let _ = app_pump.emit("preview-frame", &payload);
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
        let _ = pump; // keep pump alive via task handle leak; dropped at process end (v1)

        let _ = app.emit(
            "record-status",
            RecordStatus {
                state: "recording".into(),
                duration_ms: 0,
                file_path: None,
                error: None,
                frames_captured: 0,
                frames_dropped: 0,
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
            },
        );
        Ok("stopped".into())
    }

    pub async fn is_recording(&self) -> bool {
        self.state.recording.load(Ordering::Relaxed)
    }
}
