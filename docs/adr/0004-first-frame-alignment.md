# 0004. Align first video and audio frames by trimming or advancing audio

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0003](0003-single-master-clock-for-av-sync.md)
- **PRD section**: 4.3 Sinkronisasi A/V

## Context

Video and audio sources start on different threads; their first frames arrive
at different moments (tens of milliseconds apart). Without correction, that
initial offset persists for the whole recording, and any downstream editor
that assumes both start at 0 will drift from the first frame.

## Decision

The pipeline implements a video start gate: it waits for the first video
frame, records `video_start_ns` on the master timeline, then compares the
first audio frame's offset. If audio is earlier, leading samples are trimmed
to the video start; if video is earlier, the audio timeline is advanced by
silence. Corrections beyond `AV_START_ALIGNMENT_LIMIT_NS` pass through with a
warning instead of over-correcting. Trimmed audio timestamps are advanced so
metadata (`audio_start_time`) matches the file content.

## Consequences

**Positive**
- First frames of A/V are truly aligned; no persistent startup offset.
- Metadata is consistent with the actual captured content.

**Negative / tradeoffs**
- A tiny startup wait (until first video frame) before audio commits.

**Neutral**
- The alignment limit doubles as a safety valve for misbehaving sources.

## Alternatives considered

- **Start both sources simultaneously and hope** — no guarantee; rejected.
- **Timestamp both from the same instant at start** — still race on first
  frames; rejected.
