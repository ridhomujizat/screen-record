# 04. Sensitive Data Censoring

- **ADR reference**: [0014. Sensitive-data detection with PaddleOCR mobile latin via ONNX](../adr/0014-sensitive-data-detection-paddleocr-mobile-latn-onnx.md), [0015. Censor boxes stamped pre-disk-write with dwell](../adr/0015-censor-boxes-stamped-pre-encode-with-region-dwell.md)
- **Related diagram**: [01. System Context And Record Flow](01-system-context-and-record-flow.md), [02. A/V Synchronization Pipeline](02-av-sync-pipeline.md)
- **Purpose**: views for the censor feature — system context delta, pre-record setup flow, record-time sequence, scan/region verification flow, and censor-specific failure handling.

Scope: what joins the pipeline when censoring is enabled. Shared capture,
clock, and finish-time encode mechanics are unchanged from 01/02/03 and are
not repeated here.

## System Context (delta)

```mermaid
flowchart LR
  subgraph SR["screen-record (Tauri app)"]
    CFG["censor settings<br/>(keywords, 500×100, 5px)"]
    OCRW["OCR worker<br/>PP-OCRv4 mobile latn ×2<br/>(det + rec, ort CPU)"]
    REG[("region list<br/>Arc&lt;RwLock&gt;")]
    PUMP["sync pump<br/>(stamp boxes pre-write)"]
    MODELS[("ONNX models<br/>~15 MB bundled")]
  end

  subgraph OS["Windows"]
    WGC["WGC frames"]
    FS[("video.raw (clean)<br/>→ MP4")]
  end

  CFG --> OCRW
  MODELS --> OCRW
  WGC --> OCRW
  OCRW --> REG
  REG --> PUMP
  WGC --> PUMP
  PUMP --> FS
```

## Pre-Record Setup (settings before record)

```mermaid
sequenceDiagram
  autonumber
  participant U as User
  participant R as React UI
  participant T as Rust commands
  participant S as Settings store

  U->>R: open "Sensitive Data Sensor"
  R->>T: get_censor_config()
  T-->>R: keywords[], box 500×100, gap 5, enabled
  U->>R: toggle on, edit keywords / size
  R->>T: set_censor_config(cfg)
  T->>S: persist
  T-->>R: ok (models load lazily on start_record)
```

Invariants:

- Settings are written and validated **before** Record is pressed; no config
  changes mid-recording (v1).
- Preset keywords latin, case-insensitive; minimum practical length 4 chars.

## Record With Censoring (happy path)

```mermaid
sequenceDiagram
  autonumber
  participant W as WGC thread
  participant O as OCR worker (2 fps)
  participant P as Sync pump
  participant M as mux/video.raw
  participant FF as ffmpeg (finish)

  W->>P: BGRA frame (broadcast)
  P->>M: stamp active boxes → append (clean bytes)
  W->>O: latest-frame slot (skip if busy)
  O->>O: det → rec (latin) → keyword match
  O-->>P: region list update (first-hit activate, dwell remove)
  Note over P,M: every frame re-reads list — box present before value typed
  P->>FF: finish (unchanged, ADR-0013)
  FF-->>FF: encode MP4 — secret never on disk
```

## Scan → Region Verification Flow

```mermaid
flowchart TD
  A["scan tick (500 ms)<br/>frame ≤1280px"] --> B{"det lines found?"}
  B -- "No" --> C{"any active region<br/>missed 2 scans?"}
  C -- "Yes" --> X["remove region"]
  C -- "No" --> KEEP["keep region<br/>(dwell)"]
  B -- "Yes" --> D["rec each line (CTC)<br/>lowercase, in-memory only"]
  D --> E{"keyword match?"}
  E -- "No" --> KEEP2["mark region miss (+1)<br/>discard text immediately"]
  E -- "Yes" --> F{"center inside<br/>existing region<br/>same keyword?"}
  F -- "Yes" --> G["refresh region<br/>reset miss count"]
  F -- "No" --> H["activate region:<br/>x = kw.right + 5<br/>y = kw.center_y − 50<br/>500 × 100, clamp"]
  G --> LIST["publish list (RwLock)"]
  H --> LIST
  X --> LIST
```

Notes:

- First detection activates immediately (no confirmation wait) — the leak
  window is only the scan interval.
- Full recognized text beyond the matched keyword is never stored, logged,
  or emitted.

## Geometry

```mermaid
flowchart LR
  subgraph FRAME["frame (full-res coords)"]
    KW["keyword bbox<br/>'Password'"] -- "gap 5px" --> BOX["solid black<br/>500 × 100<br/>centered on kw line"]
  end
  KW -->|"kw.right + 5"| BOX
  KW -->|"kw.center_y − 50"| BOX
  CLAMP["clamp to frame bounds<br/>(right-edge clip → warn badge)"] -.-> BOX
```

## Failure Handling

```mermaid
flowchart TD
  A["failure occurs"] --> B{"when?"}
  B -- "start_record, model load fails" --> C["reject start<br/>'sensor enabled but model unavailable'<br/>(fail-closed)"]
  B -- "mid-record, worker panic / model error" --> D["stop recording<br/>status error 'censor-failed'<br/>file path surfaced — user decides keep/delete"]
  B -- "scan slow / busy" --> E["skip scan<br/>keep last regions applied<br/>(bounded staleness ≤ dwell)"]
  B -- "keyword clipped by frame edge" --> F["clamp box<br/>warn badge 'area terpotong'"]
  C --> K["log reason only<br/>— never recognized text"]
  D --> K
  E --> K
  F --> K
```

## Data Model Extension

```mermaid
erDiagram
  CENSOR_SETTINGS {
    BOOLEAN enabled
    TEXT keywords_json "array, latin, lowercase"
    INTEGER box_w "default 500"
    INTEGER box_h "default 100"
    INTEGER gap_px "default 5"
  }

  REGION {
    TEXT keyword
    INTEGER x INTEGER y
    INTEGER w INTEGER h
    INTEGER miss_count "dwell 2"
  }

  CENSOR_SETTINGS ||..o{ REGION : "runtime only (not persisted)"
```

- Settings: persisted app config (source of truth).
- Region list: process memory only, rebuilt from live scans.

## Implementation Checklist

1. `capture/censor/mod.rs`: config struct, region tracker (activate/refresh/
   dwell), geometry + clamp — unit tests first (pure functions).
2. Sync-pump hook: stamp boxes on BGRA before `video.raw` append; no-op when
   disabled.
3. `capture/censor/ocr.rs`: `ort` sessions ×2, pre/post-processing, keyword
   match worker; golden-output tests for DB + CTC helpers.
4. Bundle pinned ONNX models; fail-closed check at `start_record`.
5. UI: pre-record settings panel + `censor-status` badge + preview boxes.
6. Acceptance: form-login 30 s recording frame-checked; zero-regression run
   with censoring off.
