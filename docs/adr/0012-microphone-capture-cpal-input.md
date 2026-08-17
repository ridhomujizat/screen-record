# 0012. Capture microphone with a CPAL input stream on the same QPC timeline

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0003](0003-single-master-clock-for-av-sync.md), [0007](0007-system-audio-wasapi-loopback-cpal.md), [0013](0013-defer-audio-mixing-to-ffmpeg-at-finish.md)
- **PRD section**: PD-0002 §4.1–4.2

## Context

Users need narration on top of system-audio demos. The mic is a second audio
source arriving on its own thread with its own device clock. The danger is a
mic that lands on a *different* timeline from video and system audio — the
whole sync design (ADR-0003) depends on every source sharing one clock family.
On Windows, CPAL exposes input devices and stamps callbacks with
`info.timestamp().capture` (QPC), exactly like the loopback path we already
ship (ADR-0007). Device formats vary (16 kHz mono to 48 kHz stereo; wireless
mics jitter more than wired — Cap uses 90 ms vs 20 ms buffer timeouts for the
same reason).

## Decision

We capture the microphone with CPAL on the WASAPI host as a normal **input**
stream on the user-selected device (`input_devices()`, default device if none
selected), converted to interleaved f32 with the same sample-format
conversion the loopback path uses. Each frame is timestamped via the same
QPC anchor used for loopback, gets its own `SourceClockState("mic-audio")`,
and is remapped onto the existing `MasterClock` — no new clock, no new
timestamp family. Frames leave the callback via `try_send` + drop counter
(callback never blocks). The stream runs at the device's native rate;
resampling to 48 kHz is deferred to ffmpeg at finish (ADR-0013). No silence
keepalive — that exists only because loopback stops delivering when nothing
plays.

## Consequences

**Positive**
- One timestamp family: mic, system audio, and WGC video all land on the
  MasterClock unchanged — relative sync is already solved by ADR-0003.
- Reuses the loopback capturer's conversion and anchoring code (small diff).
- Same crate and host cover macOS later (CoreAudio via CPAL).

**Negative / tradeoffs**
- A second live CPAL stream (more callbacks, small CPU cost — measured < 2%).
- Bluetooth headsets may switch A2DP→HFP when the mic opens, briefly
  disturbing system audio; we handle via error/mismatch re-init, not by
  preventing it.
- Native-rate capture means per-source rates downstream until finish-time
  resampling.

**Neutral**
- Mic-only recordings skip the loopback and keepalive entirely.

## Alternatives considered

- **Raw WASAPI capture bindings** — more control, much more unsafe code;
  rejected for v1 (same reasoning as ADR-0007).
- **Resample to 48 kHz in Rust before writing** — needs a Rust resampler we
  don't have; ffmpeg's swresampler already runs at finish. Rejected (ADR-0013).
- **Tauri/webview `getUserMedia`** — wrong permission model for a desktop
  recorder, extra latency, no QPC timestamps. Rejected.
