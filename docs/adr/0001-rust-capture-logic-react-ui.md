# 0001. Use Rust for all capture/encode logic; React is UI only

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0002](0002-platform-agnostic-capture-core.md), [0003](0003-single-master-clock-for-av-sync.md)
- **PRD section**: 4.5 UI, 10 Nota Arsitektur

## Context

Screen capture (WGC/WASAPI), A/V synchronization, and encoding require direct
hardware access and steady performance. Doing this in JavaScript is not
viable: browsers/webviews expose no loopback-audio capture or WGC access, a
JS thread stalls under 30fps BGRA frame throughput, and there is no native
codec pipeline without heavy WASM. The frontend (React) must stay responsive
while frames stream in.

## Decision

We implement all capture, encoding, and muxing logic in Rust inside
`src-tauri/src/capture/`, and expose it to the frontend exclusively through
Tauri commands (`list_sources`, `start_record`, `stop_record`) and Tauri
events (`record-status`, `preview-frame`). React only renders UI state and
calls `invoke()`; it never touches frame bytes or timing logic.

## Consequences

**Positive**
- Native access to WGC, WASAPI, and QPC timestamps.
- Sync logic is unit-testable in Rust without a UI.
- Frontend stays lightweight and hot-reloadable.

**Negative / tradeoffs**
- Rust is the majority of the codebase; rebuilds (not hot-reload) for Rust changes.
- Team must be comfortable with Rust (already the case: Cargo 1.94).

**Neutral**
- Tauri command boundary is the single integration surface; changing it
  requires coordinated frontend + backend changes.

## Alternatives considered

- **JavaScript + WASM capture** — no WGC/loopback access; rejected.
- **Native module loaded from webview** — reinvents Tauri IPC; rejected.
