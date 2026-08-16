# 02. A/V Synchronization Pipeline

- **ADR reference**: [0003. Single master clock for A/V sync](../adr/0003-single-master-clock-for-av-sync.md)
- **Related diagram**: [01. System Context And Record Flow](01-system-context-and-record-flow.md)
- **Purpose**: internal data flow for synchronization — source timestamps → MasterClock remap, first-frame alignment, gap handling, and failure handling.

Scope: what happens to every video/audio frame between capture and the
encoder, and how the pipeline fails safely. UI flows are in 01.

## Frame → Master Timeline Flow

```mermaid
flowchart LR
  subgraph SRC["Sources (QPC timestamps)"]
    V["WGC video frame<br/>SystemRelativeTime"]
    A["WASAPI audio frame<br/>capture timestamp"]
  end

  subgraph CLK["clock.rs"]
    MC["MasterClock<br/>(48kHz sample counter)"]
    SC["SourceClockState.remap()<br/>trusted / snap &lt;70ms / reset &gt;2s"]
  end

  subgraph TL["timeline.rs"]
    GATE["Video start gate<br/>trim/advance audio"]
    GAP["AudioGapTracker<br/>insert silence"]
  end

  subgraph OUT["encode.rs + mux.rs"]
    ENC["H.264 + AAC"]
    MUX["MP4 muxer (PTS from master timeline)"]
  end

  V --> SC
  A --> SC
  SC --> MC
  SC --> GATE
  GATE --> GAP
  GAP --> ENC
  ENC --> MUX
  MC --> MUX
```

## Verification / Decision Flowchart (SourceClock remap)

```mermaid
flowchart TD
  A["source timestamp arrives"] --> B{"close to wall clock<br/>&lt; 2s?"}
  B -- "Yes" --> T["Trusted — use directly"]
  B -- "No" --> C{"first frame?"}
  C -- "Yes" --> I["InitialAdjust — set baseline offset"]
  C -- "No" --> D{"diff from expected cadence?"}
  D -- "&lt; 70ms" --> S["Smoothed — snap to cadence"]
  D -- "&gt; 70ms" --> H["HardReset — re-anchor, flush buffers"]
  T --> E["master_ns on timeline"]
  I --> E
  S --> E
  H --> E
```

## Failure Handling

```mermaid
flowchart TD
  A["Failure occurs"] --> B{"before/after<br/>first frame?"}
  B -- "before" --> C["Abort start, clean error to UI<br/>(no file created)"]
  B -- "after" --> D{"failure class"}
  D -- "Audio gap &lt; threshold" --> E["Insert silence (timeline.rs)"]
  D -- "Audio source dead &gt; timeout" --> F["Reset source buffer, warn"]
  D -- "Video source lost (WGC error)" --> G["Stop recording, finalize partial file, error event"]
  D -- "Disk full" --> H["Stop cleanly, keep valid partial, error event"]
  C --> K["Log redacted reason only"]
  E --> K
  F --> K
  G --> K
  H --> K
```

## Implementation Checklist

1. Port `clock.rs` (MasterClock + SourceClockState) with Cap's unit tests first.
2. Port `timeline.rs` (video start gate, gap tracker, tail padding) with
   synthetic-frame tests.
3. Wire WGC video source (`platform/windows.rs`) into the pipeline (M2).
4. Wire WASAPI loopback audio source into the pipeline (M3).
5. Add encoder + muxer (`encode.rs`, `mux.rs`) with PTS from master timeline (M4).
6. Emit `record-status` / `preview-frame` events; React subscribes (M5).
7. Tests: clock jitter/reset, first-frame alignment, gap fill, long-run drift check.
