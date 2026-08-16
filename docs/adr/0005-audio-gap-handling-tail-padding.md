# 0005. Insert silence on audio gaps and pad the audio tail to video duration

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0003](0003-single-master-clock-for-av-sync.md)
- **PRD section**: 4.3 Sinkronisasi A/V

## Context

Audio callbacks drop under CPU load or device hiccups. If gaps are ignored,
the audio timeline shrinks while video keeps growing, and the recording ends
desynchronized. Cap handles this with a gap tracker that inserts silence and
pads the tail so audio duration matches video.

## Decision

We port Cap's `AudioGapTracker` into `src-tauri/src/capture/timeline.rs`.
Gaps between audio frames above a threshold (70 ms wired, 160 ms wireless)
are filled with synthesized silence; the audio track tail is padded so its
final duration equals the video duration. Silence frames are capped at 1 s
each to avoid giant single-buffer allocations.

## Consequences

**Positive**
- Long recordings stay synchronized at the end.
- Deterministic, testable behavior (synthetic frames in unit tests).

**Negative / tradeoffs**
- If a device is truly dead, we write silence instead of erroring — accepted;
  a broken timeline is worse.

**Neutral**
- Gap accounting (overlap-trim summary) is surfaced for downstream tooling.

## Alternatives considered

- **Drop audio on gap and let the muxer stretch** — muxer timestamps
  disagree; rejected.
- **Restart the audio source on gap** — causes re-buffering; rejected.
