# 0010. Communicate Rust → UI via Tauri events, not polling

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0001](0001-rust-capture-logic-react-ui.md)
- **PRD section**: 4.5 UI

## Context

The UI needs live recording status: timer, state (idle/recording/error),
finished file path. Polling from React every few hundred milliseconds is
wasteful and adds latency; Tauri has a native event system designed for
exactly this push pattern.

## Decision

Rust emits `record-status` (state, duration_ms, file_path on finish, error)
and `preview-frame` (downscaled BGRA) events; React subscribes with
`listen()` and unsubscribes on unmount. Commands stay request/response:
`list_sources`, `start_record`, `stop_record`, `open_folder`.

## Consequences

**Positive**
- Real-time UI without polling; single source of truth (Rust state).
- Events carry the only data the UI needs (state transitions, not frames
  except preview).

**Negative / tradeoffs**
- Event subscription lifecycle must be managed in React (unlisten on unmount).

**Neutral**
- The `record-status` payload shape is the de-facto UI contract; documented
  in the PRD/plan.

## Alternatives considered

- **React polls `get_status()` every 500 ms** — wasteful, adds latency,
  misses edge transitions; rejected.
- **WebSocket/SSE layer** — overkill inside one process; rejected.
