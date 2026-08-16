# 0003. Use a single audio-sample-based MasterClock as the global A/V timeline

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0004](0004-first-frame-alignment.md), [0005](0005-audio-gap-handling-tail-padding.md)
- **PRD section**: 4.3 Sinkronisasi A/V, 7 Risiko (A/V drift)

## Context

A/V drift is the top quality problem in screen recorders. Video timestamps
(WGC `SystemRelativeTime`) and audio timestamps (WASAPI `capture`) both derive
from the Windows QPC clock, so they share a source of truth — but they arrive
on independent threads with jitter, gaps, and different rates. Without one
canonical timeline, audio and video drift apart over long recordings. Cap
solves this with a MasterClock that advances by committed audio samples and
per-source remapping that snaps jitter onto the expected cadence.

## Decision

We port Cap's `MasterClock` + `SourceClockState` into
`src-tauri/src/capture/clock.rs`. The MasterClock advances on committed audio
samples at 48 kHz (chunk 1024); every source timestamp (video QPC, audio
WASAPI QPC) is remapped into this one timeline by `SourceClockState::remap()`:
timestamps within 70 ms of the expected cadence are snapped onto it,
timestamps more than 2 s away trigger a hard reset (re-anchor), and the first
frame establishes the initial offset. Audio is the timeline master because
sample counts produce a perfectly smooth clock.

## Consequences

**Positive**
- Video and audio land on one timeline, so they stay synchronized.
- Micro-jitter is absorbed (no audio/video chasing each other).
- Proven design: this is Cap's production sync core, tests ported too.

**Negative / tradeoffs**
- If audio fails entirely, video needs a fallback (P1: wall-clock remap when
  no audio frames arrive).

**Neutral**
- Constants (70 ms snap, 2 s reset, 48 kHz, 1024 frames) are part of the
  contract and should not be tuned per-platform.

## Alternatives considered

- **Video wall-clock as master** — video timestamps jump at refresh-rate
  boundaries; rejected.
- **Per-track independent clocks, offset at the end** — complex post-hoc
  alignment; rejected.
- **`Instant::now()` per frame on both sides** — loses QPC precision and
  cross-thread coherence; rejected.
