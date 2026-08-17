# 03. Microphone Capture And Mixing

- **ADR reference**: [0012. Microphone capture via CPAL input](../adr/0012-microphone-capture-cpal-input.md), [0013. Mix at finish via ffmpeg](../adr/0013-defer-audio-mixing-to-ffmpeg-at-finish.md)
- **Related diagram**: [01. System Context And Record Flow](01-system-context-and-record-flow.md), [02. A/V Synchronization Pipeline](02-av-sync-pipeline.md)
- **Purpose**: data flow and sequences for the second audio source (mic) — capture, timeline-rendered per-source WAVs, finish-time mixing, metering, and mic-specific failure handling.

Scope: what changes when a microphone joins the recording. Shared A/V sync
mechanics (MasterClock remap, snap/reset) are in 02 and are reused unchanged.

## System Context (delta)

```mermaid
flowchart LR
  subgraph SR["screen-record (Tauri app)"]
    CORE["capture core"]
    W1["WavWriter system<br/>(master timeline)"]
    W2["WavWriter mic<br/>(master timeline)"]
    MIX["finish: ffmpeg<br/>amix + adelay + aresample"]
    CORE --> W1
    CORE --> W2
    W1 --> MIX
    W2 --> MIX
  end

  subgraph OS["Windows"]
    WASAPI["WASAPI loopback<br/>(default output)"]
    MICIN["WASAPI capture<br/>(selected input device)"]
  end

  WASAPI --> CORE
  MICIN --> CORE
  MIX --> FS[("output.mp4<br/>single mixed AAC track")]
```

## Frame Flow (mic joins the existing pipeline)

```mermaid
flowchart LR
  subgraph SRC["Sources (QPC timestamps)"]
    V["WGC video"]
    A1["System audio<br/>(loopback + keepalive)"]
    A2["Mic audio<br/>(input device)"]
  end

  subgraph CLK["clock.rs (unchanged, ADR-0003)"]
    SC2["SourceClockState<br/>'mic-audio'"]
  end

  subgraph W["wav writers (per source)"]
    WW1["cursor + gap-fill + trim"]
    WW2["cursor + gap-fill + trim"]
  end

  M["RMS meter → event audio-meter"]
  F["ffmpeg amix at finish"]

  A1 --> WW1
  A2 --> SC2 --> WW2
  V --> VOFF["video_start_ns"]
  VOFF --> WW1
  VOFF --> WW2
  WW1 --> F
  WW2 --> F
  A2 --> M
```

Invariants:

- One `MasterClock`, one QPC anchor — mic adds a `SourceClockState`, never a clock.
- Each WAV is a faithful render of its source's master-timeline span
  (gap → silence, early-start → trim, late-start → `adelay` at finish).
- Callbacks never block: `try_send` + drop counter, per source.

## Record With Mic (happy path)

```mermaid
sequenceDiagram
  autonumber
  participant U as User
  participant R as React UI
  participant T as Tauri IPC
  participant C as Capture core
  participant P as CPAL (WASAPI)
  participant F as ffmpeg (finish)

  U->>R: enables Mic, picks device
  R->>T: invoke("list_audio_devices")
  T-->>R: input devices (default flag)
  U->>R: clicks Record (system+mic)
  R->>T: invoke("start_record", target, audio opts)
  T->>C: start
  C->>P: open loopback (keepalive) + input streams
  P-->>C: audio frames (QPC each)
  C->>C: remap both to MasterClock, write per-source WAVs
  C-->>R: "record-status" + "audio-meter" (mic RMS)
  U->>R: clicks Stop
  R->>T: invoke("stop_record")
  C->>P: stop streams
  C->>F: amix (aresample + adelay) + x264 + AAC
  F-->>C: output.mp4
  C-->>R: "record-status" {finished, path}
```

## Finish Path Selection

```mermaid
flowchart TD
  S["stop_record"] --> A{"audio WAVs written?"}
  A -- "0" --> V0["video-only encode (no audio args)"]
  A -- "1" --> V1["existing single-source path<br/>(-itsoffset if offset)"]
  A -- "2+" --> V2["filter_complex:<br/>aresample=48000 + adelay per source<br/>+ amix=normalize=0"]
  V0 --> OUT["output.mp4"]
  V1 --> OUT
  V2 --> OUT
```

## Failure Handling (mic-specific)

```mermaid
flowchart TD
  A["mic failure occurs"] --> B{"when?"}
  B -- "at start (no device / open fails)" --> C["disable mic mode, record continues<br/>system-only, warn UI"]
  B -- "mid-recording" --> D{"class"}
  D -- "stream error / device unplugged" --> E["stop mic source, pad WAV with silence<br/>to video end, warn; finalize valid file"]
  D -- "format changed (BT HFP switch)" --> F["re-init stream with new config<br/>(Cap rate_changed pattern); continue"]
  D -- "frames dropped (try_send)" --> G["gap-fill silence at WAV cursor<br/>+ drop counter in status"]
  D -- "wireless jitter" --> H["snap < 70ms / hard reset > 2s<br/>(ADR-0003, unchanged)"]
  C --> K["log + status event"]
  E --> K
  F --> K
  G --> K
  H --> K
```

## Implementation Checklist

1. `list_audio_devices` command (CPAL `input_devices`) + UI dropdown & mode toggle.
2. Extract per-source `WavWriter` from `mux.rs`: sample cursor, gap-fill,
   first-frame trim — unit tests first (synthetic gaps, early/late starts).
3. Generalize `audio.rs` capturer: input mode vs loopback mode (keepalive only
   for loopback); second audio thread in `capture/mod.rs`.
4. Pump: `SourceClockState("mic-audio")`, per-source WAV writes, RMS →
   `audio-meter` event (throttled ~100ms).
5. `finish()`: multi-WAV `filter_complex` (amix/adelay/aresample); single-source
   path unchanged; `normalize=0`, clamp before AAC.
6. Robustness: mic unplug → pad + warn; format mismatch → re-init.
7. Tests: WavWriter gap/trim units; `examples/capture_test.rs` with mic;
   ffprobe asserts single AAC track; clap/waveform offset check ≤ 50ms.
