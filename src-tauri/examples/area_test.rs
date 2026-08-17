//! Area capture test: crop 400x300 from display, mux to MP4.
use screen_record_lib::capture::{audio, clock, mux, platform, platform::ScreenCapture};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let targets = platform::list_targets();
    let (target, _, dw, dh) = targets.first().unwrap().clone();
    let area = platform::CaptureTarget::Area { display: match target { platform::CaptureTarget::Display(d) => d, _ => 0 }, left: 100, top: 100, right: 500, bottom: 400 };

    let (vtx, _) = tokio::sync::broadcast::channel::<platform::VideoFrame>(64);
    let mut vrx = vtx.subscribe();
    let mut cap = platform::create_capture(area, 30);
    println!("starting area capture (display {dw}x{dh}, crop 100,100,500,400)...");
    cap.start(vtx).await.expect("start");

    let (atx, mut arx) = tokio::sync::mpsc::channel::<audio::AudioFrame>(512);
    let mut audio_cap = audio::SystemAudioCapturer::new().expect("audio");
    audio_cap.start(atx).expect("audio start");

    let clock = clock::MasterClock::new(48000);
    let mut vcs = clock::SourceClockState::new("v");
    let mut acs = clock::SourceClockState::new("a");
    let mut muxer: Option<mux::Muxer> = None;
    let mut vframes = 0u64;

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            Ok(vf) = vrx.recv() => {
                vframes += 1;
                let r = vcs.remap(&clock, vf.timestamp, 33_333_333);
                if vframes <= 2 { println!("frame {vframes}: {}x{}", vf.width, vf.height); }
                if muxer.is_none() {
                    let mut m = mux::Muxer::new(vf.width, vf.height, 30);
                    m.start(&std::env::temp_dir().join("sr-area"), 48000, 2).expect("mux start");
                    muxer = Some(m);
                }
                if let Some(m) = muxer.as_mut() { let _ = m.push_video(&vf.data, r.master_ns); }
            }
            Some(af) = arx.recv() => {
                let spf = af.samples.len() as u64 / af.channels.max(1) as u64;
                let fnns = spf * 1_000_000_000 / af.sample_rate as u64;
                let r = acs.remap(&clock, af.timestamp, fnns);
                if let Some(m) = muxer.as_mut() { let _ = m.push_audio(&af.samples, af.sample_rate, af.channels, r.master_ns); }
            }
        }
    }
    cap.stop().await.expect("stop");
    audio_cap.stop().expect("stop audio");
    if let Some(m) = muxer.as_mut() {
        match m.finish(&std::env::temp_dir().join("sr-area.mp4")) {
            Ok(p) => println!("MP4: {}", p.display()),
            Err(e) => println!("finish err: {e}"),
        }
    }
    println!("DONE: {vframes} video frames");
}
