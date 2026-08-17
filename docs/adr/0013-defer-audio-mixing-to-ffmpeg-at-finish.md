# 0013. Mix system audio and microphone at finish via ffmpeg, not a live Rust mixer

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0005](0005-audio-gap-handling-tail-padding.md), [0007](0007-system-audio-wasapi-loopback-cpal.md), [0008](0008-encode-h264-aac-ffmpeg-next-mp4.md), [0012](0012-microphone-capture-cpal-input.md)
- **PRD section**: PD-0002 §4.3–4.4

## Context

With two audio sources (system loopback + mic) at possibly different sample
rates and channel counts, something must combine them into the single AAC
track v1 ships. Cap solves this with a 1,300-line live `AudioMixer`
(per-source resampler filter graphs, stall windows, buffering trackers) —
appropriate for its editor/monitor needs, heavy for ours. Our pipeline
already defers encoding to the ffmpeg binary at finish (ADR-0008), and
already owns the alignment logic (first-frame trim, offset) in the WAV
writer. Mixing correctness reduces to: every WAV must be a faithful render
of its source's master-timeline span, then alignment at combine time is
trivial.

The one real correctness hazard is sequential WAV writing: if a source drops
frames (try_send drop, device stall), later samples shift early and the
file's "samples since start" timeline diverges from the master timeline.
This is the ADR-0005 gap commitment, applied at the WAV layer.

## Decision

Each audio source writes its **own WAV rendered on the master timeline**:
the writer tracks a sample cursor; a frame arriving after its cursor position
inserts silence for the gap (bounded chunks), a source whose first audio
precedes video start gets its leading samples trimmed (existing behavior,
now per-source). At `finish()`, with two or more tracks we invoke ffmpeg
with `filter_complex` — `aresample=48000` per input, `adelay` per source
start-offset (audio starting after video), `amix=inputs=N:normalize=0` —
then encode as today. The single-track path stays byte-for-byte the existing
arguments. Live level metering is simple RMS on incoming frames in the pump
(event `audio-meter`), computed before any mixing.

## Consequences

**Positive**
- Zero new sync-critical Rust: alignment rides timestamps already on the
  MasterClock; mixing/resampling ride ffmpeg's swresampler.
- Single-track output plays everywhere; no multi-track editor complexity.
- Gap-fill at the WAV layer finally enforces ADR-0005 and fixes the latent
  sequential-write drift for the existing system-audio path too.
- Mic-only / system-only paths are just "one WAV" — no special casing.

**Negative / tradeoffs**
- No live *mixed* monitor output (not a v1 goal; metering covers the need).
- Finish time grows slightly (one extra filter graph; negligible vs x264).
- Mixed clipping protection is clamping only (limiter is a non-goal).

**Neutral**
- `adelay` (filter) and `-itsoffset` (input option) have identical semantics
  for our case; we keep `-itsoffset` on the single-source path and `adelay`
  inside the multi-source graph so the existing path never changes.

## Alternatives considered

- **Live Rust mixer (Cap-style AudioMixer)** — needed only for live monitor
  output or per-track encode; rejected as ~10× the code for zero v1 value.
- **Separate audio tracks in the MP4** — players don't reliably downmix;
  every consumer then needs editing; rejected for v1.
- **Mix in Rust at write time (sum into one WAV live)** — requires
  cross-rate resampling in Rust plus a buffering window to align chunk
  boundaries; rejected — same complexity class as a live mixer.
