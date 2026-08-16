# 0002. Use a platform-agnostic capture core with a per-OS `ScreenCapture` trait

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0001](0001-rust-capture-logic-react-ui.md), [0003](0003-single-master-clock-for-av-sync.md), [0006](0006-windows-capture-wgc.md)
- **PRD section**: 10 Nota Arsitektur Awal, 4.1 Target Capture

## Context

The product must run on Windows now and on macOS later. Cap solves this with
per-platform capture implementations (`CMSampleBufferCapture`,
`Direct3DCapture`, `X11Capture`) behind a single capture trait. If we write
Windows-only code with no seam, adding macOS means refactoring the whole
pipeline. If we over-abstract, we build interfaces nobody uses.

## Decision

We define one `ScreenCapture` trait (async, `Send`) in
`src-tauri/src/capture/platform/mod.rs`, plus a per-OS type alias selected by
`#[cfg(target_os)]`. Implementations live in `platform/windows.rs` (WGC, v1)
and `platform/macos.rs` (stub, cfg-off until v2). All pipeline code
(clock, timeline, encode, mux) consumes only typed `VideoFrame` /
`AudioFrame` structs that carry `{ inner, timestamp }`; it never touches
OS-specific types.

## Consequences

**Positive**
- macOS is one trait implementation + one alias; the pipeline is untouched.
- Frame abstraction hides BGRA/D3D (Windows) vs CVPixelBuffer (macOS).
- Windows v1 ships without macOS code compiled in.

**Negative / tradeoffs**
- A small abstraction layer; kept minimal by making the trait tiny and precise.

**Neutral**
- The trait is the contract that must stay stable across OS ports.

## Alternatives considered

- **Two pipelines, cfg-selected wholesale** — duplicates clock/encode/mux; rejected.
- **No trait, Windows-only code now** — guarantees macOS refactor; rejected.
- **scap-rs wrapper crate (as Cap does)** — adds a dependency we do not
  control; rejected; we write WGC directly (ADR-006).
