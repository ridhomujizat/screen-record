# 0006. Capture screen on Windows with WGC (Windows.Graphics.Capture), not Desktop Duplication

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0002](0002-platform-agnostic-capture-core.md)
- **PRD section**: 4.2 Capture, 8 Risiko

## Context

Windows offers two screen capture APIs. Desktop Duplication (DXGI) captures
full displays only, has no natural window/area crop, and does not expose
frame timestamps in a QPC-friendly way. WGC
(`Windows.Graphics.Capture` via `Direct3D11CaptureFramePool`) captures
displays, windows, and areas; delivers frames on a callback with
`SystemRelativeTime` (QPC); and is what Cap ships. WGC requires a D3D11
device and a graphics-capture-enabled context (Tauri window qualifies).

## Decision

We implement `platform/windows.rs` on WGC: create an `ID3D11Device`
(hardware, falling back to WARP), build a `Direct3D11CaptureFramePool`
(BGRA8), register the `FrameArrived` handler, and in it call
`TryGetNextFrame`, take `SystemRelativeTime` as the frame timestamp
(`Timestamp::PerformanceCounter`), crop via `CopySubresourceRegion` for
window/area targets, apply a cadence gate (cap to nominal fps), and forward
the frame over a channel. We use the `windows` crate directly, not the `scap`
wrapper Cap uses, to keep dependencies minimal and under our control.

## Consequences

**Positive**
- Modern API with natural window/area crop and precise QPC timestamps.
- Same timestamp source as WASAPI audio → feeds MasterClock (ADR-003).
- No heavy external dependency (we control the wrapper).

**Negative / tradeoffs**
- WGC needs a D3D11 device and app context; WARP fallback adds complexity.
- More raw windows-rs code to write than using scap.

**Neutral**
- Frame callback can fire above nominal fps; the cadence gate is mandatory.

## Alternatives considered

- **Desktop Duplication (DXGI)** — display-only, no crop, awkward timestamps;
  rejected.
- **scap-rs wrapper** — extra dependency, less control, Windows-only anyway;
  rejected.
- **ffmpeg gdigrab** — slow, no window list, poor timestamps; rejected.
