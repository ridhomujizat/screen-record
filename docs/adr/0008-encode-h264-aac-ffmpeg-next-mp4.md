# 0008. Encode H.264 + AAC with ffmpeg-next; mux to a standard MP4

- **Status**: Accepted (v1)
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0003](0003-single-master-clock-for-av-sync.md), [0002](0002-platform-agnostic-capture-core.md)
- **PRD section**: 4.4 Encoding & Output

## Context

We need an encoder (H.264) and audio codec (AAC) muxed into a playable
container. Options: (a) ffmpeg bindings (`ffmpeg-next`), software encode;
(b) hardware encode (NVENC/Media Foundation) from day one; (c) shelling out to
an FFmpeg binary. Cap uses ffmpeg bindings with software first and hardware
later. We prioritize a correct, portable v1 over peak performance; hardware
encode is a P1 enhancement that must not change the pipeline interface.

## Decision

We use `ffmpeg-next` (with `ffmpeg-sys-next`) for H.264 (ultrafast preset,
software) + AAC encode and MP4 muxing, writing to a standard MP4 file.
PTS values come from the master timeline (ADR-003), never wall clock. The
encoder is wrapped behind a small interface in `encode.rs` so a hardware
encoder (NVENC) can replace the software one later without touching the
pipeline. We deliberately skip fragmented M4S (Cap's instant-mode) — we do
not need crash-recovery or instant preview in v1.

## Consequences

**Positive**
- Portable software encode; good enough for tutorials.
- One library covers encode + mux + resample.
- Standard MP4 plays everywhere (VLC, Windows Media Player).

**Negative / tradeoffs**
- FFmpeg dev libraries must be present on the build machine (vcpkg/msys2);
  documented in README.
- Higher CPU usage than hardware encode; accepted in v1.

**Neutral**
- A single pipeline change (swap encoder impl) is the future hardware path.

## Alternatives considered

- **NVENC crate / Media Foundation from v1** — performance now, but more
  platform coupling and setup before the pipeline is proven; deferred.
- **Shell out to FFmpeg binary** — easiest but slow startup, fragile version
  pinning, no in-process frame control; rejected.
- **Fragmented M4S output** — complexity without a v1 need; rejected (P1).
