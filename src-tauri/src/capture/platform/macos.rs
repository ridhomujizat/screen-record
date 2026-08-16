//! macOS capture stub (ADR-0011, Proposed — v2).
#![allow(dead_code)] // macOS stub utk v2 (ADR-0011)
//!
//! Not compiled on Windows. Implements `ScreenCapture` with
//! ScreenCaptureKit (via the `sc` crate) in v2. Until then, all
//! operations error out with a clear message.

use super::{CaptureTarget, ScreenCapture, VideoFrame};

pub struct MacOsScreenCapture {
    pub target: CaptureTarget,
}

impl MacOsScreenCapture {
    pub fn new(target: CaptureTarget) -> Self {
        Self { target }
    }
}

#[async_trait::async_trait]
impl ScreenCapture for MacOsScreenCapture {
    async fn start(
        &mut self,
        _tx: tokio::sync::mpsc::Sender<VideoFrame>,
    ) -> Result<(), String> {
        Err("macOS capture not implemented yet (v2, ADR-0011)".into())
    }

    async fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }
}
