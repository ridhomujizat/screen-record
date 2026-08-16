# ADR — Architecture Decision Records

> Format: singkat, per-decision. Status: Proposed / Accepted / Superseded.
> Setiap keputusan punya: Konteks → Keputusan → Konsekuensi.

---

## ADR-001: Rust untuk seluruh logika capture/encode, React hanya UI

**Status:** Accepted (dasar arsitektur)

**Konteks:**
Screen capture (WGC/WASAPI), sinkronisasi A/V, dan encoding butuh akses
hardware + performa. Kalau semua di JS: tidak ada akses WGC/WASAPI loopback,
performance jelek, dan JS thread tersumbat.

**Keputusan:**
- Semua capture/encode/mux di **Rust** (modul `src-tauri/src/capture/`).
- Tauri command `#[tauri::command]` sebagai batas: `list_sources`,
  `start_record`, `stop_record`, `on_progress` (event).
- React memanggil via `invoke()`; tidak ada logika capture di frontend.

**Konsekuensi:**
- (+) Performa native, akses API Windows langsung.
- (+) Sync logic bisa diuji tanpa UI.
- (−) Rust jadi mayoritas kode; butuh keahlian Rust (sudah ada: Cargo 1.94).
- (−) Hot-reload hanya untuk frontend; Rust perlu rebuild.

---

## ADR-002: Platform-agnostic core dengan trait `ScreenCapture`

**Status:** Accepted — menjawab kebutuhan "tidak hanya Windows, siap macOS"

**Konteks:**
Cap memakai per-platform implementasi (`CMSampleBufferCapture`,
`Direct3DCapture`, `X11Capture`) di belakang satu `ScreenCaptureFormat` trait.
Kita di Windows sekarang, tapi struktur harus bisa tumbuh ke macOS.

**Keputusan:**
- Definisi `trait ScreenCapture` (async, `Send`) di `platform/mod.rs`:

```rust
// pola (detail di implementasi)
#[async_trait]
pub trait ScreenCapture: Send {
    async fn start(&mut self, tx: mpsc::Sender<VideoFrame>) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
    fn video_info(&self) -> VideoInfo;
}
```

- Implementasi per-OS di `platform/windows.rs` (WGC) dan `platform/macos.rs`
  (stub yang return "not implemented" / cfg-off).
- `platform/mod.rs` memilih implementasi via `#[cfg(target_os = "...")]`
  `pub type PlatformCapture = ...;` — satu titik alih.
- Seluruh pipeline (clock, encode, mux) **tidak pernah** menyentuh detail OS;
  hanya memakai `VideoFrame { inner, timestamp }` & `AudioFrame { inner, timestamp }`.

**Konsekuensi:**
- (+) macOS tinggal implement 1 trait + reuse semua pipeline.
- (+) Tipe frame abstrak (`VideoFrame`) menyembunyikan BGRA/D3D vs pixelbuffer.
- (−) Sedikit overhead abstraksi; dipotong dengan trait kecil & jelas.

---

## ADR-003: Satu `MasterClock` berbasis sampel audio sebagai timeline global

**Status:** Accepted (port dari Cap `crates/timestamp/master_clock.rs`)

**Konteks:**
A/V drift adalah masalah #1 screen recorder. Cap menyelesaikannya dengan
MasterClock yang maju berdasarkan **jumlah sampel audio yang di-commit**
(48kHz), dan semua source timestamp (video QPC, audio WASAPI QPC) di-remap
ke clock itu.

**Keputusan:**
- Port `MasterClock` + `SourceClockState` dari Cap ke `src-tauri/src/capture/clock.rs`.
- Konstanta dipertahankan: `AUDIO_OUTPUT_FRAMES=1024`, `DEFAULT_SAMPLE_RATE=48k`,
  snap threshold `70ms`, hard-reset `2s`.
- Alasan audio sebagai master: audio timestamp dari WASAPI/WGC share sumber
  waktu QPC; sampel audio menghasilkan timeline mulus tanpa lompatan.

**Konsekuensi:**
- (+) Video & audio otomatis di timeline yang sama → sinkron.
- (+) Jitter mikro di-snap (tak ada audio/video saling mengejar).
- (−) Kalau audio gagal total, video perlu fallback clock (P1 — pakai
  wall-clock remap jika tidak ada frame audio).

---

## ADR-004: First-frame alignment (trim/advance audio) di pipeline

**Status:** Accepted (port dari Cap `apply_video_start_gate`)

**Konteks:**
Video & audio start dari thread berbeda → frame pertama tidak sejajar;
tanpa perbaikan, offset awal (bisa puluhan ms) menempel sepanjang rekaman.

**Keputusan:**
- Video start gate: pipeline menunggu frame video pertama, catat
  `video_start_ns` (di master timeline).
- Audio frame pertama dibandingkan offset-nya terhadap video start:
  - audio lebih awal → **trim** leading samples;
  - video lebih awal → **advance** audio timeline (silence).
- Batas koreksi `AV_START_ALIGNMENT_LIMIT_NS`; di luar itu passthrough + warn.

**Konsekuensi:**
- (+) Frame pertama benar-benar sejajar.
- (+) Metadata `audio_start_time` konsisten dengan isi file.

---

## ADR-005: Gap handling & tail padding untuk audio kontinu

**Status:** Accepted

**Konteks:**
Audio callback bisa drop (load CPU, device hiccup). Tanpa penanganan,
timeline audio menyusut → video lebih panjang → drift di akhir.

**Keputusan:**
- Gap tracker (port dari Cap): gap > 70ms (wired) → insert silence;
  tail audio di-pad agar durasi == durasi video.
- Semua silence synthesis dibatasi ukuran frame (maks 1s per frame) biar
  tidak alokasi buffer raksasa.

**Konsekuensi:**
- (+) Rekaman panjang tetap sinkron di ujung.
- (−) Silence palsu kalau device benar-benar mati (diterima; lebih baik dari
  timeline rusak).

---

## ADR-006: Windows capture = WGC (Windows.Graphics.Capture), bukan Desktop Duplication

**Status:** Accepted

**Konteks:**
Cap memakai WGC (Direct3D11CaptureFramePool) — modern, support window/area
crop natural, timestamp `SystemRelativeTime` (QPC). Desktop Duplication lama
hanya full-screen & tanpa timestamp QPC mudah.

**Keputusan:**
- `platform/windows.rs` memakai WGC:
  - `ID3D11Device` (fallback hardware → WARP).
  - `Direct3D11CaptureFramePool::CreateFreeThreaded` (format BGRA8).
  - Event `FrameArrived` → `TryGetNextFrame` → timestamp QPC → kirim ke channel.
  - Cadence gate (frame pool callback bisa > fps nominal) — cap ke 30fps.
- Crate `windows` (windows-rs) langsung, tanpa wrapper scap (kita butuh
  kontrol penuh & minim dependency).

**Konsekuensi:**
- (+) API modern, crop/area mudah, timestamp presisi.
- (+) Tidak perlu dependency eksternal berat.
- (−) WGC butuh app window context utk `GraphicsCaptureItem` (Tauri punya).

---

## ADR-007: Audio system = WASAPI loopback via CPAL

**Status:** Accepted

**Konteks:**
System audio (yang keluar dari speaker) perlu di-loopback. Windows: WASAPI
loopback. CPAL sudah dipakai Cap & memberi `timestamp().capture` (QPC) yang
kompatibel dengan MasterClock.

**Keputusan:**
- CPAL (`cpal`) dengan host WASAPI, device default output → loopback.
- Format target 48kHz stereo F32; resample kalau device beda (mis. 44.1k).
- Timestamp dari `info.timestamp().capture` → `Timestamp::from_cpal`.

**Konsekuensi:**
- (+) Satu library utk cross-platform audio (macOS nanti: CoreAudio via CPAL).
- (+) Timestamp QPC konsisten dengan video.

---

## ADR-008: Encode H.264 + AAC via ffmpeg-next (software v1), mux MP4 internal

**Status:** Accepted (v1) — hardware encoder = P1 enhancement

**Konteks:**
Cap pakai ffmpeg (crate `ffmpeg-next` / `cap-enc-ffmpeg`) utk H264+AAC dan
MP4. Opsi: (a) ffmpeg-next (software), (b) NVENC via `nvenc` crate / Media
Foundation, (c) FFmpeg binary eksternal. v1 prioritaskan kesederhanaan &
portabilitas.

**Keputusan:**
- `ffmpeg-next` (bindings) untuk H.264 (preset ultrafast) + AAC + MP4 mux.
- Output standard MP4 (bukan fragmented M4S) untuk v1 — kita tidak butuh
  instant-preview/crash-recovery dulu.
- Jalur upgrade P1: hardware encoder (NVENC) dengan bitrate naik, tanpa
  ubah pipeline (encoder interface abstrak).
- Dependensi Rust: `ffmpeg-next` + `ffmpeg-sys-next`. Perlu FFmpeg lib
  (vcpkg/msys2) di dev machine — dokumentasikan di README.

**Konsekuensi:**
- (+) Software encoder portabel, kualitas cukup utk tutorial.
- (+) Satu library utk decode/encode/resample → sedikit dependensi.
- (−) Setup FFmpeg lib di Windows agak ribet (dokumentasi diperlukan).
- (−) CPU usage lebih tinggi dari hardware encode (diterima di v1).

---

## ADR-009: Pemisahan modul — tidak ada bloat layer

**Status:** Accepted

**Konteks:**
Ponytail rule: jangan bikin abstraksi tanpa kebutuhan. Tapi kita TAHU
kebutuhan (macOS) dari awal, jadi trait layer dibenarkan — selebihnya minimal.

**Keputusan:**
Struktur final `src-tauri/src/capture/`:
- `mod.rs` — orchestrator sederhana (start/stop, hubungkan source→encode→mux).
- `clock.rs` — MasterClock + SourceClockState (port Cap, ~200 baris).
- `timeline.rs` — first-frame align, gap tracker, tail pad.
- `encode.rs` — H264+AAC encoder wrap.
- `mux.rs` — MP4 muxer wrap (ffmpeg-next).
- `platform/mod.rs` — trait + type alias per OS.
- `platform/windows.rs` — WGC + WASAPI.
- `platform/macos.rs` — stub cfg-off (kompilasi aman di Windows).

Tidak ada: config framework, trait utk satu implementasi (encode bisa
langsung), event bus, plugin architecture.

**Konsekuensi:**
- (+) Setiap file kecil & fokus → mudah dibaca.
- (+) macOS = 1 file baru + 1 alias + test.
- (−) Orchestrator boleh agak "god object" — diterima di ukuran ini.

---

## ADR-010: Komunikasi Rust→UI via Tauri events, bukan polling

**Status:** Accepted

**Konteks:**
Timer, progress, error harus tampil di UI. Polling dari React tiap 500ms
boros & lambat.

**Keputusan:**
- Rust emit `app.emit("record-status", payload)` (state: idle/recording/error,
  duration, file path saat selesai).
- React `listen("record-status", ...)`.
- Command invoke: `list_sources`, `start_record`, `stop_record`, `open_folder`.

**Konsekuensi:**
- (+) UI real-time tanpa polling.
- (−) Event lifecycle perlu di-cleanup di React (unlisten).

---

## ADR-011 (Draft): macOS = ScreenCaptureKit via trait yang sama (v2)

**Status:** Proposed (belum implementasi; memastikan struktur aman)

**Konteks:**
Saat kita naik ke macOS: ScreenCaptureKit (`SCShareableContent`,
`SCCaptureSession`) untuk layar/window, CoreAudio untuk audio, permission
screen-recording + mic. Struktur ADR-002/003 dirancang agar ini tinggal
implementasi trait.

**Keputusan (rencana):**
- `platform/macos.rs`: implement `ScreenCapture` pakai `sc` crate (Cidre,
  dipakai Cap) atau `screencapturekit-rs`.
- Audio: CoreAudio loopback via CPAL (macOS host), atau `coreaudio-rs`.
- Permission flow: UI langkah-langkah + re-check (system settings).

**Konsekuensi:**
- (+) Tidak ada refactor pipeline saat macOS masuk.
- (−) macOS punya permission model berbeda — perlu penanganan UX terpisah.
