//! Headless capture test: record the primary display for 3 seconds,
//! print how many frames arrived. Requires a running app session (WGC).
use screen_record_lib::capture::platform::{self, ScreenCapture};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let targets = platform::list_targets();
    println!("targets: {targets:?}");
    let Some((target, _, _, _)) = targets.first() else {
        println!("no display");
        return;
    };
    let target = *target;

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut cap = platform::create_capture(target, 30);

    println!("starting capture of {target:?}...");
    cap.start(tx).await.expect("start");
    println!("started");

    let start = Instant::now();
    let mut frames = 0u64;
    let mut bytes = 0u64;
    let mut last_ts = None;
    while start.elapsed() < Duration::from_secs(3) {
        tokio::select! {
            Some(vf) = rx.recv() => {
                frames += 1;
                bytes += vf.data.len() as u64;
                last_ts = Some(vf.timestamp);
                if frames <= 3 {
                    println!("frame {frames}: {}x{} {} bytes", vf.width, vf.height, vf.data.len());
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
    println!("stopping...");
    cap.stop().await.expect("stop");
    println!(
        "DONE: {frames} frames in 3s ({} fps), {bytes} bytes total, last_ts={last_ts:?}",
        frames as f64 / 3.0
    );
}
