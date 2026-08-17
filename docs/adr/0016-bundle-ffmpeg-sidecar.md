# 0016. Bundle ffmpeg as a Tauri sidecar instead of requiring it on PATH

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0008](0008-encode-h264-aac-ffmpeg-next-mp4.md), [0013](0013-defer-audio-mixing-to-ffmpeg-at-finish.md)
- **PRD section**: distribution concern for PD-0001/0002 output path

## Context

The finish step (ADR-0013) spawns `Command::new("ffmpeg")` to encode
`video.raw` + WAVs into the final MP4. That resolves against the user's
PATH — fine on the dev machine (chocolatey), but an installed app on a
clean machine has no ffmpeg, and every recording fails at finish. The
encode uses `libx264` (GPL) plus AAC and the `amix` filter, which rules
out LGPL-only builds.

## Decision

We bundle the **gyan.dev "essentials" ffmpeg build (GPL, libx264, ~103 MB)**
as a Tauri **sidecar**: `bundle.externalBin = ["binaries/ffmpeg"]`, file
stored as `src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe` and placed
next to the app executable by the bundler. `mux.rs` resolves the binary as:
executable directory (`ffmpeg.exe`, installed builds) → dev copy under
`src-tauri/binaries/` → bare `"ffmpeg"` on PATH as a last resort. The
103 MB binary is gitignored; a checked-in `download-ffmpeg.sh` (and the
PowerShell equivalent in `binaries/README.md`) fetches it, so the repo
stays lean and the pinned URL is recorded.

## Consequences

**Positive**
- Installed app needs zero user setup for encoding; recording works out of
  the box.
- Version pinned by the download script — no drift between user machines.
- PATH fallback keeps dev machines with a system ffmpeg working unchanged.

**Negative / tradeoffs**
- Installer grows by ~100 MB (accepted: it is the price of a working app).
- Build/CI must run the download script before bundling (documented).

**Neutral**
- A future NVENC hardware path (gyan build includes it) does not change
  this decision.

## Alternatives considered

- **Require ffmpeg on PATH** — rejected: every end-user install fails.
- **ffmpeg-next Rust bindings (ADR-0008 v1 plan)** — no FFmpeg dev libs on
  the system; the binary sidecar keeps the build toolchain-free.
- **LGPL build + mpeg4 codec** — smaller, but quality/compat of H.264
  output matters more than 40 MB.
