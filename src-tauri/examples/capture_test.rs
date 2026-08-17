//! Headless capture test: record the primary display + system audio for 3s,
//! mux to an MP4, print frame counts + A/V sync offset. Requires ffmpeg on PATH.

use screen_record_lib::capture::{
    audio, clock, mux,
    platform::{self, ScreenCapture},
};
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

    let (vtx, _) = tokio::sync::broadcast::channel::<platform::VideoFrame>(64);
    let mut vrx = vtx.subscribe();
    let mut cap = platform::create_capture(target, 30);
    println!("starting video capture...");
    cap.start(vtx).await.expect("start video");
    println!("video started");

    let (atx, mut arx) = tokio::sync::mpsc::channel::<audio::AudioFrame>(512);
    let mut audio_cap = audio::AudioCapturer::new(audio::AudioMode::System).expect("audio capturer");
    audio_cap.start(atx).expect("start audio");
    println!(
        "audio started ({}Hz, {}ch)",
        audio_cap.sample_rate, audio_cap.channels
    );

    let clock = clock::MasterClock::new(clock::DEFAULT_SAMPLE_RATE);
    let mut vcs = clock::SourceClockState::new("video");
    let mut acs = clock::SourceClockState::new("audio");
    let mut last_video_ns: Option<u64> = None;
    let mut sync_ms: i64 = 0;

    let mut muxer: Option<mux::Muxer> = None;
    let out_dir = std::env::temp_dir().join("sr-m4-test");

    let start = Instant::now();
    let mut vframes = 0u64;
    let mut aframes = 0u64;
    while start.elapsed() < Duration::from_secs(3) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            Ok(vf) = vrx.recv() => {
                vframes += 1;
                let r = vcs.remap(&clock, vf.timestamp, 33_333_333);
                last_video_ns = Some(r.master_ns);
                if muxer.is_none() {
                    let mut m = mux::Muxer::new(vf.width, vf.height, 30);
                    m.start(&out_dir).expect("muxer start");
                    muxer = Some(m);
                    println!("muxer started {}x{}", vf.width, vf.height);
                }
                if let Some(m) = muxer.as_mut() {
                    m.push_video(&vf.data, r.master_ns).expect("push video");
                }
            }
            Some(af) = arx.recv() => {
                aframes += 1;
                let spf = af.samples.len() as u64 / af.channels.max(1) as u64;
                let frame_ns = spf * 1_000_000_000 / af.sample_rate as u64;
                let r = acs.remap(&clock, af.timestamp, frame_ns);
                if let (Some(v), Some(a)) = (last_video_ns, Some(r.master_ns)) {
                    sync_ms = (v as i64 - a as i64) / 1_000_000;
                }
                if let Some(m) = muxer.as_mut() {
                    m.push_audio("system", &af.samples, af.sample_rate, af.channels, r.master_ns).expect("push audio");
                }
            }
        }
    }

    cap.stop().await.expect("stop video");
    audio_cap.stop().expect("stop audio");

    if let Some(m) = muxer.as_mut() {
        let out = std::env::temp_dir().join("sr-m4-test.mp4");
        match m.finish(&out) {
            Ok(p) => println!("MP4 saved: {}", p.display()),
            Err(e) => println!("MP4 finish error: {e}"),
        }
    }

    println!(
        "DONE: {vframes} video ({:.1} fps), {aframes} audio, sync {sync_ms}ms",
        vframes as f64 / 3.0
    );
}
