# 0009. Keep the module layout flat and minimal; no unneeded abstraction layers

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0002](0002-platform-agnostic-capture-core.md), [0001](0001-rust-capture-logic-react-ui.md)
- **PRD section**: 10 Nota Arsitektur Awal

## Context

Every abstraction is a cost: more files, more seams to understand, more
places for bugs. The only abstraction we *know* we need (not speculate about)
is the per-OS capture seam (ADR-002), because macOS is an explicit product
requirement. Everything else should be as flat as the code allows.

## Decision

The capture core lives in a flat `src-tauri/src/capture/` module with one
file per concern: `clock.rs` (MasterClock), `timeline.rs` (alignment, gap
handling), `encode.rs` (H.264/AAC wrapper), `mux.rs` (MP4 muxer), and
`platform/` containing only the OS seam (trait + `windows.rs` +
`macos.rs` stub). `mod.rs` is a thin orchestrator connecting
source → clock → encoder → muxer and exposing start/stop. We add no config
framework, no event bus, no trait for single-implementation pieces (e.g. the
muxer stays concrete until a second muxer exists).

## Consequences

**Positive**
- Each file is small and focused; easy to read and test.
- Adding macOS = one file + one alias + tests (ADR-002).
- Fewer seams than a heavily-layered design.

**Negative / tradeoffs**
- `mod.rs` orchestrator carries more responsibility than in a layered design;
  accepted at this size.

**Neutral**
- If a second encoder/muxer ever appears, extracting a trait there is a
  small, safe refactor.

## Alternatives considered

- **Trait-everything architecture** — interfaces with one implementation;
  rejected (YAGNI).
- **Single monolithic `recorder.rs`** — 1500+ lines, untestable in parts;
  rejected.
