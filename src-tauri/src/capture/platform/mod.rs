//! Platform seam (ADR-0002): per-OS screen capture behind one trait.
#![allow(dead_code)] // width/height/timestamp dipakai M3
//!
//! The pipeline consumes only `VideoFrame` — it never sees OS-specific
//! types. Adding macOS = implement `ScreenCapture` in `macos.rs` and flip
//! the `#[cfg]` alias below.

use crate::capture::clock::RawTimestamp;

pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// BGRA8, tightly packed (stride = width*4).
    pub data: Vec<u8>,
    /// Hardware timestamp (QPC on Windows).
    pub timestamp: RawTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    Display(u64),
    Window(u64),
    /// Area capture: display handle + crop rect in PHYSICAL pixels (left, top, right, bottom).
    Area {
        display: u64,
        left: u32,
        top: u32,
        right: u32,
        bottom: u32,
    },
}

/// A captured video source. Implementations are per-OS.
#[async_trait::async_trait]
pub trait ScreenCapture: Send {
    /// Begin capture, pushing BGRA8 frames into `tx` (broadcast for preview + sync).
    async fn start(
        &mut self,
        tx: tokio::sync::broadcast::Sender<VideoFrame>,
    ) -> Result<(), String>;
    /// Stop capture cleanly.
    async fn stop(&mut self) -> Result<(), String>;
}

/// Per-OS concrete capture type.
#[cfg(target_os = "windows")]
pub type PlatformCapture = windows::WindowsScreenCapture;
#[cfg(target_os = "macos")]
pub type PlatformCapture = macos::MacOsScreenCapture;

#[cfg(target_os = "windows")]
pub fn create_capture(target: CaptureTarget, max_fps: u32) -> PlatformCapture {
    windows::WindowsScreenCapture::new(target, max_fps)
}
#[cfg(target_os = "macos")]
pub fn create_capture(target: CaptureTarget, _max_fps: u32) -> PlatformCapture {
    macos::MacOsScreenCapture::new(target)
}

/// List capture targets: (target, label, width, height) — displays then windows.
#[cfg(target_os = "windows")]
pub fn list_targets() -> Vec<(CaptureTarget, String, u32, u32)> {
    let mut v = windows::list_windows_capture_targets();
    v.extend(windows::list_windows_capture_targets_windows());
    v
}
#[cfg(target_os = "macos")]
pub fn list_targets() -> Vec<(CaptureTarget, String, u32, u32)> {
    vec![]
}
