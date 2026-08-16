# 01. System Context And Record Flow

- **ADR reference**: [0001. Rust capture logic, React UI only](../adr/0001-rust-capture-logic-react-ui.md)
- **Related diagram**: [02. A/V Synchronization Pipeline](02-av-sync-pipeline.md)
- **Purpose**: system context (boundaries), happy-path record flow, KISS default path, and the start/stop sequence between React, Rust, and the OS capture APIs.

Scope: end-to-end of one recording session — source selection through a
playable MP4. It is *not* about internal A/V sync mechanics (see 02) or
failure branches (see the Failure Handling section in 02).

## System Context

```mermaid
flowchart LR
  subgraph SR["screen-record (Tauri app)"]
    UI["React UI"]
    IPC["Tauri IPC (commands + events)"]
    CORE["Rust capture core (capture/)"]
    PLAT["platform/ (WGC / WASAPI)"]
    CORE --> PLAT
    IPC --> CORE
    UI <--> IPC
  end

  subgraph OS["Windows"]
    WGC["Windows.Graphics.Capture (D3D11)"]
    WASAPI["WASAPI loopback"]
  end

  subgraph FF["FFmpeg libs"]
    ENC["H.264 + AAC encoders"]
    MUX["MP4 muxer"]
  end

  PLAT --> WGC
  PLAT --> WASAPI
  CORE --> ENC
  CORE --> MUX
  MUX --> FS[("output.mp4 on disk")]
```

## Happy-Path Record Sequence

```mermaid
sequenceDiagram
  autonumber
  participant U as User
  participant R as React UI
  participant T as Tauri IPC
  participant C as Capture core (Rust)
  participant W as WGC / WASAPI
  participant M as Encoder + Muxer

  U->>R: opens app
  R->>T: invoke("list_sources")
  T->>C: list_sources
  C-->>R: displays / windows list
  U->>R: picks Full Screen, clicks Record
  R->>T: invoke("start_record", target)
  T->>C: start_record
  C->>W: start WGC + WASAPI loopback
  W-->>C: video/audio frames (QPC timestamps)
  C->>M: encode + mux (master timeline)
  C-->>R: event "record-status" {recording}
  R->>U: shows timer
  U->>R: clicks Stop
  R->>T: invoke("stop_record")
  T->>C: stop_record
  C->>W: stop sources
  C->>M: flush encoders + muxer
  M-->>FS: output.mp4
  C-->>R: event "record-status" {finished, path}
  R->>U: result card + "Open folder"
```

Invariants:

- Exactly one recording session at a time (v1).
- `stop_record` is idempotent — the second call is a no-op.
- The file is only reported as finished after the muxer flushes and closes.

## KISS Default Path

```mermaid
flowchart TD
  A["Open app"] --> B["Pick Full Screen"]
  B --> C["Click Record"]
  C --> D["Record (frames → encoder → MP4)"]
  D --> E["Click Stop"]
  E --> F["Flush + close MP4"]
  F --> G["Show result + Open folder"]

  X["Escape hatch: Window / Area target"] -.-> B
  Y["Escape hatch: error (device lost, disk full)"] -.-> Z["Show error state, keep partial file"]
```

KISS defaults:

- Default target is **Full Screen** (single display). Window/Area are escape
  hatches through the same pipeline (different bounds only).
- Default quality **Standard**; High is a setting, not a separate path.
- One recording at a time; no pause in v1.
