//! Headless capture test: record the primary display + system audio for 3s,
//! print frame counts and A/V sync offset. Requires a desktop session.

use screen_record_lib::capture::{
    audio, clock,
    platform::{self, ScreenCapture},
};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    // --- video ---
    let targets = platform::list_targets();
    println!("targets: {targets:?}");
    let Some((target, _, _, _)) = targets.first() else {
        println!("no display");
        return;
    };
    let target = *target;

    let (vtx, _) = tokio::sync::broadcast::channel::<platform::VideoFrame>(64);
    let mut vrx = vtx.subscribe();
    let mut cap = platform::create_capture(target, 30);
    println!("starting video capture...");
    cap.start(vtx).await.expect("start video");
    println!("video started");

    // --- audio ---
    let (atx, mut arx) = tokio::sync::mpsc::channel::<audio::AudioFrame>(16);
    let mut audio_cap = audio::SystemAudioCapturer::new().expect("audio capturer");
    audio_cap.start(atx).expect("start audio");
    println!(
        "audio started ({}Hz, {}ch)",
        audio_cap.sample_rate, audio_cap.channels
    );

    let clock = clock::MasterClock::new(clock::DEFAULT_SAMPLE_RATE);
    let mut vcs = clock::SourceClockState::new("video");
    let mut acs = clock::SourceClockState::new("audio");
    let mut first_video_ns: Option<u64> = None;
    let mut first_audio_ns: Option<u64> = None;
    let mut sync_ms: i64 = 0;

    let start = Instant::now();
    let mut vframes = 0u64;
    let mut aframes = 0u64;
    while start.elapsed() < Duration::from_secs(3) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                // progress tick; loop re-checks start.elapsed()
            }
            Ok(vf) = vrx.recv() => {
                vframes += 1;
                let r = vcs.remap(&clock, vf.timestamp, 33_333_333);
                if first_video_ns.is_none() { first_video_ns = Some(r.master_ns); }
                if vframes <= 3 { println!("vframe {vframes}: {}x{} ts={}", vf.width, vf.height, r.master_ns); }
            }
            Some(af) = arx.recv() => {
                aframes += 1;
                let samples_per_frame = af.samples.len() as u64 / af.channels.max(1) as u64;
                let frame_ns = samples_per_frame * 1_000_000_000 / af.sample_rate as u64;
                let r = acs.remap(&clock, af.timestamp, frame_ns);
                if first_audio_ns.is_none() { first_audio_ns = Some(r.master_ns); }
                if aframes <= 3 { println!("aframe {aframes}: {} samples ts={}", af.samples.len(), r.master_ns); }
            }
        }
        if let (Some(v), Some(a)) = (first_video_ns, first_audio_ns) {
            sync_ms = (v as i64 - a as i64) / 1_000_000;
        }
    }

    cap.stop().await.expect("stop video");
    audio_cap.stop().expect("stop audio");

    println!(
        "DONE: {vframes} video frames ({:.1} fps), {aframes} audio frames, first video={first_video_ns:?}ns, first audio={first_audio_ns:?}ns, A/V offset = {sync_ms}ms",
        vframes as f64 / 3.0
    );
}
