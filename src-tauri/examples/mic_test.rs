//! Mic + system audio test: record display + loopback + microphone for 3s,
//! mux to a single-track MP4, print per-source frame counts + sync offsets.
//! Verifies ADR-0012/0013 end to end. Requires ffmpeg on PATH.

use screen_record_lib::capture::{
    audio, clock, mux,
    platform::{self, ScreenCapture},
};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("both"); // both | mic | system

    let targets = platform::list_targets();
    let Some((target, _, _, _)) = targets.first() else {
        println!("no display");
        return;
    };
    let target = *target;

    let (vtx, _) = tokio::sync::broadcast::channel::<platform::VideoFrame>(64);
    let mut vrx = vtx.subscribe();
    let mut cap = platform::create_capture(target, 30);
    cap.start(vtx).await.expect("start video");
    println!("video started");

    let use_system = mode == "both" || mode == "system";
    let use_mic = mode == "both" || mode == "mic";

    let (atx_sys, mut arx_sys) = tokio::sync::mpsc::channel::<audio::AudioFrame>(512);
    let (atx_mic, mut arx_mic) = tokio::sync::mpsc::channel::<audio::AudioFrame>(512);

    let mut sys_cap = if use_system {
        let mut c = audio::AudioCapturer::new(audio::AudioMode::System).expect("system capturer");
        c.start(atx_sys).expect("start system audio");
        println!("system audio started ({}Hz, {}ch)", c.sample_rate, c.channels);
        Some(c)
    } else {
        None
    };

    let mic_name = args.get(2).cloned();
    let mut mic_cap = if use_mic {
        let mut c = audio::AudioCapturer::new(audio::AudioMode::Mic {
            device: mic_name.clone(),
        })
        .expect("mic capturer");
        c.start(atx_mic).expect("start mic");
        println!(
            "mic started ({}Hz, {}ch, device={:?})",
            c.sample_rate,
            c.channels,
            mic_name.unwrap_or_default()
        );
        Some(c)
    } else {
        None
    };

    let clock = clock::MasterClock::new(clock::DEFAULT_SAMPLE_RATE);
    let mut vcs = clock::SourceClockState::new("video");
    let mut scs = clock::SourceClockState::new("system-audio");
    let mut mcs = clock::SourceClockState::new("mic-audio");

    let mut muxer: Option<mux::Muxer> = None;
    let out_dir = std::env::temp_dir().join("sr-m7-test");

    let mut vframes = 0u64;
    let mut sframes = 0u64;
    let mut mframes = 0u64;
    let mut first_video_ns: Option<u64> = None;
    let mut first_sys_ns: Option<u64> = None;
    let mut first_mic_ns: Option<u64> = None;

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            Ok(vf) = vrx.recv() => {
                vframes += 1;
                let r = vcs.remap(&clock, vf.timestamp, 33_333_333);
                first_video_ns.get_or_insert(r.master_ns);
                if muxer.is_none() {
                    let mut m = mux::Muxer::new(vf.width, vf.height, 30);
                    m.start(&out_dir).expect("muxer start");
                    muxer = Some(m);
                }
                if let Some(m) = muxer.as_mut() {
                    m.push_video(&vf.data, r.master_ns).expect("push video");
                }
            }
            Some(af) = arx_sys.recv() => {
                sframes += 1;
                let spf = af.samples.len() as u64 / af.channels.max(1) as u64;
                let fnns = spf * 1_000_000_000 / af.sample_rate as u64;
                let r = scs.remap(&clock, af.timestamp, fnns);
                first_sys_ns.get_or_insert(r.master_ns);
                if let Some(m) = muxer.as_mut() {
                    m.push_audio("system", &af.samples, af.sample_rate, af.channels, r.master_ns).expect("push system audio");
                }
            }
            Some(af) = arx_mic.recv() => {
                mframes += 1;
                let spf = af.samples.len() as u64 / af.channels.max(1) as u64;
                let fnns = spf * 1_000_000_000 / af.sample_rate as u64;
                let r = mcs.remap(&clock, af.timestamp, fnns);
                first_mic_ns.get_or_insert(r.master_ns);
                if let Some(m) = muxer.as_mut() {
                    m.push_audio("mic", &af.samples, af.sample_rate, af.channels, r.master_ns).expect("push mic audio");
                }
            }
        }
    }

    cap.stop().await.expect("stop video");
    if let Some(c) = sys_cap.as_mut() {
        c.stop().expect("stop system audio");
    }
    if let Some(c) = mic_cap.as_mut() {
        c.stop().expect("stop mic");
    }

    let out = std::env::temp_dir().join(format!("sr-m7-{mode}.mp4"));
    if let Some(m) = muxer.as_mut() {
        match m.finish(&out) {
            Ok(p) => println!("MP4 saved: {}", p.display()),
            Err(e) => {
                println!("finish error: {e}");
                return;
            }
        }
    }

    let off = |v: Option<u64>, a: Option<u64>| -> String {
        match (v, a) {
            (Some(v), Some(a)) => format!("{}ms", (a as i64 - v as i64) / 1_000_000),
            _ => "n/a".into(),
        }
    };
    println!("DONE mode={mode}: {vframes} video, {sframes} system, {mframes} mic");
    println!(
        "first-frame offsets vs video — system: {}, mic: {}",
        off(first_video_ns, first_sys_ns),
        off(first_video_ns, first_mic_ns)
    );
}
