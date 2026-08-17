# Architecture Decision Records

This directory holds the Architecture Decision Records (ADRs) for TesAPI. Each ADR captures a single durable architectural decision: the problem it solves, the option chosen, and the tradeoffs accepted.

## Purpose

ADRs are the **immutable historical record** — they explain *why* the spec is shaped the way it is. When the PRD changes a decision, the old ADR is marked `Superseded` and a new ADR is written; ADRs are never edited after they reach `Accepted` status.

If you are a new engineer, read the ADR index below before diving into code or the 2,500-line PRD. The ADRs are short on purpose.

## Index

| #    | Title                                                                                     | Status   | Date       |
|------|-------------------------------------------------------------------------------------------|----------|------------|
| 0001 | Use Rust for all capture/encode logic; React is UI only                                   | Accepted | 2026-08-17 |
| 0002 | Use a platform-agnostic capture core with a per-OS `ScreenCapture` trait                  | Accepted | 2026-08-17 |
| 0003 | Use a single audio-sample-based MasterClock as the global A/V timeline                    | Accepted | 2026-08-17 |
| 0004 | Align first video and audio frames by trimming or advancing audio                         | Accepted | 2026-08-17 |
| 0005 | Insert silence on audio gaps and pad the audio tail to video duration                     | Accepted | 2026-08-17 |
| 0006 | Capture screen on Windows with WGC, not Desktop Duplication                               | Accepted | 2026-08-17 |
| 0007 | Capture system audio with WASAPI loopback via CPAL                                        | Accepted | 2026-08-17 |
| 0008 | Encode H.264 + AAC with ffmpeg-next; mux to a standard MP4                                | Accepted | 2026-08-17 |
| 0009 | Keep the module layout flat and minimal; no unneeded abstraction layers                   | Accepted | 2026-08-17 |
| 0010 | Communicate Rust → UI via Tauri events, not polling                                       | Accepted | 2026-08-17 |
| 0011 | Add macOS capture later via ScreenCaptureKit behind the same trait (draft)                | Proposed | 2026-08-17 |
| 0012 | Capture microphone with a CPAL input stream on the same QPC timeline                      | Accepted | 2026-08-17 |
| 0013 | Mix system audio and microphone at finish via ffmpeg, not a live Rust mixer                | Accepted | 2026-08-17 |

## How to add an ADR

1. Copy [`0000-template.md`](90%20System/Templates/Project%20Docs/adr/0000-template.md).
2. Rename to `NNNN-kebab-case-title.md`, where `NNNN` is the next free 4-digit number.
3. Fill in Context, Decision, Consequences, Alternatives. Aim for 50–150 lines.
4. Set `Status: Proposed` while it is under review. Flip to `Accepted` when merged.
5. Add a row to the index table above.
6. If this ADR replaces an older one, set the old ADR's status to `Superseded by NNNN` and link both ways.

## Status lifecycle

```
Proposed  ──► Accepted ──► Deprecated
              │
              └─────────► Superseded by NNNN
```

- **Proposed** — under review, not yet binding.
- **Accepted** — the decision is in effect. ADR is immutable from this point.
- **Deprecated** — the decision is no longer recommended but no replacement exists yet.
- **Superseded by NNNN** — a new ADR replaces this one. Both records remain in the repo.

## Conventions

- Filenames: `NNNN-kebab-case-title.md`, zero-padded to 4 digits.
- One decision per ADR. If a decision splits naturally into two, write two ADRs.
- No edits to `Accepted` ADRs except metadata flips (status, links). Substantive changes require a new ADR.
- Cite the PRD section that codifies the decision.
- Cross-link related ADRs in the header.
