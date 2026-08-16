# 0011. Add macOS capture later via ScreenCaptureKit behind the same trait (draft)

- **Status**: Proposed
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0002](0002-platform-agnostic-capture-core.md), [0006](0006-windows-capture-wgc.md)
- **PRD section**: 7 Batas / Out of Scope, 5 Kriteria Sukses

## Context

macOS is a declared future target. Its capture stack differs: ScreenCaptureKit
(`SCShareableContent`, `SCCaptureSession`) for display/window capture,
CoreAudio for audio, and a privacy permission model (screen-recording + mic)
with system-settings UX. The v1 Windows pipeline must not be built in a way
that blocks this. This ADR records the plan so the constraint is explicit;
implementation happens in v2.

## Decision

When macOS lands (v2), implement `platform/macos.rs` with ScreenCaptureKit
(using the `sc` crate, as Cap does, or `screencapturekit-rs`) behind the
same `ScreenCapture` trait, audio via CPAL's CoreAudio host, and a
permission flow (guided system-settings steps + re-check) in the UI. The
type alias in `platform/mod.rs` switches on `target_os`. No pipeline change
is expected: clock, timeline, encode, and mux are OS-agnostic by design
(ADR-002/003/008).

## Consequences

**Positive**
- The v1 investment in a platform seam pays off with a low-cost macOS port.
- Permissions are designed as a UX concern, not an afterthought.

**Negative / tradeoffs**
- macOS permissions are inherently more user-facing (screen-recording opt-in);
  the UI must handle denial gracefully.
- ScreenCaptureKit requires macOS 12.3+; deployment floor becomes explicit.

**Neutral**
- Frame conversion (CVPixelBuffer → common type) is the one new piece of
  OS-specific glue, isolated in `platform/macos.rs`.

## Alternatives considered

- **Defer the platform seam until macOS is real** — the refactor would touch
  every pipeline file; rejected because macOS is a stated requirement.
- **CGDisplayStream (deprecated) / AVFoundation screen capture** — less
  capable or deprecated; rejected.
