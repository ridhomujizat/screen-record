# 0007. Capture system audio with WASAPI loopback via CPAL

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0003](0003-single-master-clock-for-av-sync.md)
- **PRD section**: 4.2 Capture

## Context

System audio (everything the user hears) must be captured alongside the
screen. On Windows this means WASAPI loopback on the default output device.
CPAL already wraps WASAPI loopback and — critically — exposes
`info.timestamp().capture`, which maps to the same QPC clock family as WGC
video timestamps. Cap uses CPAL for the same reason. The default device
format is not guaranteed to be 48 kHz stereo F32, so resampling is required.

## Decision

We capture system audio with CPAL on the WASAPI host, loopback on the default
output device, targeting 48 kHz stereo F32 internally. Each audio frame takes
`Timestamp::from_cpal(info.timestamp().capture)` so it lands on the shared
master timeline. A resampler converts device formats (e.g. 44.1 kHz) to the
pipeline target before enqueueing.

## Consequences

**Positive**
- One audio library that also covers macOS later (CoreAudio via CPAL).
- QPC timestamps consistent with video (ADR-003/0006).
- Format conversion is localized in one resampler.

**Negative / tradeoffs**
- Loopback captures everything the user hears — no per-app muting (v1).
- Device format changes mid-recording require the mixer's format-mismatch
  handling (ADR-005 timeline logic).

**Neutral**
- Default output device selection; device-switch behavior is out of scope v1.

## Alternatives considered

- **Raw WASAPI COM bindings** — more control, much more unsafe code;
  rejected for v1.
- **Media Foundation audio capture** — heavier pipeline, no CPAL timestamp
  bridge; rejected.
