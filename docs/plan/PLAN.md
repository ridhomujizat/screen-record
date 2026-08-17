# Plan — Implementasi Screen Recorder (M1 → M6)

> Platform: Windows v1 (WGC + WASAPI). Struktur sudah siap macOS (ADR-002, ADR-011).
> Setiap milestone = bisa dijalankan & diverifikasi.
> Terkait: [PRD PD-0001](../prd/PD-0001-screen-recorder-tauri-react.md) ·
> [Diagram 01](../diagram/01-system-context-and-record-flow.md), [02](../diagram/02-av-sync-pipeline.md)

---

## Gambaran

```
M1 ✅ scaffold (sudah: Tauri 2 + React 19 + TS)
M2 ✅ capture video (WGC) → preview frame ke UI
M3 ✅ audio + A/V sync (WASAPI loopback + MasterClock + alignment)
M4 ✅ encode + mux → MP4 valid, stop bersih (CFR + align)
M5 ✅ UI penuh (target list, window capture, timer, buka folder)
M6 ✅ robustness (disk guard, frame-size guard, area capture)
M7 ✅ microphone (capture + WAV per-source + amix + meter) — [PD-0002](../prd/PD-0002-microphone-capture-and-sync.md)
M8 ✅ sensitive data sensor (OCR keyword → box) — [PD-0003](../prd/PD-0003-sensitive-data-censoring.md)
M9 ✅ ffmpeg sidecar (bundle, no user install) — [ADR-0016](../adr/0016-bundle-ffmpeg-sidecar.md)
```

Tiap milestone punya **definition of done** (DoD) yang bisa dicoba manual.

---

## M1 — Scaffold & integrasi Tauri-Rust (✅ selesai)

**DoD:** `npm run tauri dev` jalan, command Rust bisa dipanggil dari React.

**Yang sudah ada:**
- Tauri 2 + React 19 + TS, Vite 7 (create-tauri-app).
- Rename identity: `screen-record` / `com.omnix.screen-record`, lib `screen_record_lib`.
- Struktur `capture/` (clock, timeline, platform) + command invoke.

---

## M2 — Capture video (WGC) + preview (✅ selesai)

**DoD:** Pilih "Full Screen" → klik Record → UI menampilkan live preview frame (BGRA → canvas), klik Stop → capture berhenti.

**Yang sudah ada:**
- `platform/windows.rs`: WGC (D3D11 + `Direct3D11CaptureFramePool`) — hardware → WARP fallback, BGRA8, QPC timestamp, frame thread, display enumeration (HMONITOR).
- `platform/mod.rs` trait `ScreenCapture` + `CaptureTarget` (Display/Window). `macos.rs` stub cfg-off (ADR-002/011).
- `capture/mod.rs` orchestrator: start/stop, stop-signal, preview pump dengan downscale ≤480px, event `record-status` & `preview-frame` (ADR-0010).
- `clock.rs` (MasterClock + SourceClockState, port Cap) + `timeline.rs` (alignment, gap, tail pad) — **sudah + 11 unit test** (ADR-003/004/005).
- React UI: list sources, Record/Stop, canvas preview, status/error.
- Test: `cargo test` 11 pass; `examples/capture_test.rs` — headless capture (1 frame di env headless; banyak frame di sesi nyata).

**Verifikasi manual (sesi interaktif):** `npm run tauri dev` → pilih Display → Record → layar terlihat di canvas.

**Catatan:** di environment headless/RDP, WGC hanya mengirim 1 frame karena tidak ada perubahan layar — bukan bug pipeline.

---

## M3 — Audio + A/V sync (✅ selesai)

**DoD:** Record dengan suara sistem; A/V offset < 50ms (terverifikasi: **-14ms** di headless test).

**Yang sudah ada:**
- `capture/audio.rs`: WASAPI loopback via CPAL (`build_input_stream_raw` pada default output device), konversi f32, **silence keepalive** (trik OBS — audio tetap mengalir walau tak ada yang diputar), QPC timestamp via anchor (stock CPAL tak expose raw counter — di-anchor di callback pertama + elapsed).
- Integrasi di `capture/mod.rs`: video task + audio thread (cpal::Stream tak Send → std::thread) + sync monitor yang remap kedua source ke satu `MasterClock` (global OnceLock) & hitung offset.
- UI: status tampilkan video frames, audio frames, sync offset ms.
- Verifikasi headless: `examples/capture_test.rs` → 300 audio frames/3s (100/s, benar), first video=55.9µs vs first audio=14.4ms → **offset -14ms** (audio 14ms lebih lambat; dalam toleransi 50ms).

**Catatan:** video hanya 2 frame/3s di headless (WGC menunggu perubahan layar); di sesi nyata 30fps. `SourceClockState` snap jitter & hard-reset sudah di-test unit.

---

## M4 — Encode + mux (✅ selesai, v2 dengan CFR + A/V align)

**DoD:** `stop_record` menghasilkan `.mp4` (H.264+AAC) valid, audio & video sinkron dari frame pertama.

**Yang sudah ada:**
- `capture/mux.rs`: tulis raw BGRA + WAV (header sendiri) saat rekam; stop → ffmpeg encode libx264 ultrafast + AAC → MP4.
- **v2 fix (audio delay):** WGC kirim frame VFR (saat layar berubah). Dulu ditulis apa adanya + `-r 30` → video "ngebut" → audio delay & terpotong. Sekarang **CFR frame duplication** (isi gap dengan frame terakhir sesuai master timeline) → video berdurasi sungguhan; **first-frame align** (trim audio awal / `-itsoffset` audio) → A/V start bersamaan.
- Output: `~/Videos/screen-record/rec-<epoch>.mp4`.
- Verifikasi: ffprobe → video start 0.000s & audio start 0.000s, durasi 2.867 vs 2.858 (hampir sama).

---

## M5 — UI penuh (✅ selesai — area select pindah ke M6)

**DoD:** Alur lengkap: list sumber → pilih → record → stop → file tersimpan → buka folder.

**Yang sudah ada:**
- Window capture: `EnumWindows` + `GetWindowTextW` → list jendela (skip title kosong & window sendiri) → dropdown optgroup Display/Windows.
- Timer live (1s interval), tombol refresh sumber, result card (path + open folder + frames/sync).
- Test: window enumeration — 12 total pass.

**Pindah ke M6:** area select (drag-select overlay + crop bounds).

## M6 — Robustness & polish (✅ selesai)

**DoD:** Rekaman tidak rusak pada kasus: resolusi berubah, audio drop, stop mendadak, disk penuh.

**Yang sudah ada:**
- **Disk space guard**: cek ≥ 1 GB free sebelum start (GetDiskFreeSpaceExW) — tolak + pesan jelas.
- **Frame-size guard**: frame dengan ukuran beda (resolusi berubah) di-drop di muxer, tidak merusak raw stream.
- **Area capture**: `CaptureTarget::Area` (display + bounds fisik), crop software dari staging buffer; UI checkbox "Record area" + input x/y/w/h untuk display; command `start_record` terima `bounds`.
- Verifikasi: `examples/area_test.rs` → crop 100,100,500,400 → MP4 400×300 valid (H.264+AAC, 61 frame).

**Catatan:** area select masih input numerik (bukan drag-select overlay — itu enhancement berikutnya).

---

## M7 — Microphone capture + mixing (✅ selesai)

> PRD: [PD-0002](../prd/PD-0002-microphone-capture-and-sync.md) · ADR: [0012](../adr/0012-microphone-capture-cpal-input.md) (capture), [0013](../adr/0013-defer-audio-mixing-to-ffmpeg-at-finish.md) (mixing) · Diagram: [03](../diagram/03-microphone-capture-and-mixing.md)

**DoD terverifikasi (headless):** mode "both" → MP4 valid **satu track AAC campuran**;
first-frame offset mic **+16ms**, system **+18ms** vs video (toleransi ≤50ms ✅);
mode mic-only & system-only jalan; `cargo test` 17 pass (5 test WavWriter baru);
ffprobe: tepat 1 stream video + 1 stream audio di semua mode.

**Yang sudah ada:**
- **WavWriter per-source** (`mux.rs`): tiap sumber menulis WAV sendiri yang
  dirender pada master timeline (anchor = video start) — satu mekanisme untuk
  trim (audio lebih awal), gap-fill silence, late-start (silence di depan),
  dan tail-pad ke durasi video (ADR-0004/0005 akhirnya diterapkan di layer
  WAV). 5 unit test: gap, trim, late-start, tail-pad, routing per-source.
- **`AudioCapturer`** (`audio.rs`): generalisasi loopback → dua mode `System`
  (loopback + keepalive) dan `Mic{device}` (input stream biasa, tanpa
  keepalive). Callback/konversi/anchor QPC shared — tidak ada clock baru.
- **`finish()` amix** (`mux.rs`): ≥2 WAV → `filter_complex`
  `aformat=48000/stereo` per input + `amix=normalize=0` → satu AAC;
  1 WAV → jalur lama tanpa `-itsoffset` (WAV sudah aligned by construction,
  offset selalu 0); 0 WAV → video-only. stderr ffmpeg kini ditangkap ke
  pesan error (debugging).
- **Orchestrator** (`mod.rs`): dua thread audio dengan **re-init saat stream
  error** (unplug / BT HFP switch — pola Cap `rate_changed`), meter RMS mic →
  event `audio-meter` (throttle 100ms), `RecordStatus` + `micFrames`/`micDrops`,
  **fix clock per-sesi** (bug lama `OnceLock` global — sesi ke-2 memakai clock
  sesi pertama).
- **UI** (`App.tsx`): dropdown mode audio (System / Mic / System+Mic), pilihan
  device mic (`list_audio_devices`), meter level live saat rekam, stat mic di
  result card.
- **Simplifikasi vs ADR-0013**: `adelay` tidak diperlukan — late-start sudah
  tercakup oleh gap-fill silence di posisi timeline (satu mekanisme, bukan dua).
  Efek output identik.

**Langkah (urut, tiap langkah bisa diverifikasi):**

1. **Device list** — command `list_audio_devices` (CPAL `input_devices()`),
   UI dropdown + toggle mode audio (System / Mic / System+Mic).
2. **WavWriter per-source** — refactor kecil dari `mux.rs`: ekstrak penulis WAV
   jadi struct per sumber dengan sample cursor + gap-fill silence + first-frame
   trim. Unit test dulu (gap sintetis, mulai awal/terlambat). Gunakan logika
   `timeline.rs` yang sudah ada + ter-test.
3. **MicCapturer** — generalisasi `audio.rs`: mode `Input(device)` vs
   `LoopbackOutput` (keepalive hanya loopback); thread audio kedua di
   `capture/mod.rs`; `try_send` + drop counter.
4. **Pump** — `SourceClockState("mic-audio")`, tulis WAV per sumber, RMS →
   event `audio-meter` (throttle ~100ms); `RecordStatus` + field mic.
5. **finish() amix** — ≥2 WAV → `filter_complex` `aresample=48000` + `adelay`
   per offset + `amix=normalize=0`; jalur 1-WAV tidak berubah.
6. **Robustness mic** — unplug → pad silence + warning; format mismatch
   (BT HFP) → re-init stream.
7. **Verifikasi** — `examples/capture_test.rs` dengan mic; ffprobe: tepat satu
   track AAC; cek offset waveform (clap test) ≤ 50ms; `cargo test` pass.

**Bug lama yang ikut dibereskan:** ~~`global_clock()` di `capture/mod.rs`
memakai `OnceLock` statis — sesi rekam ke-2 mendapat clock sesi pertama. Ganti
jadi state per-session (field `RecorderState`).~~ ✅ clock kini per-sesi.

**Verifikasi manual (sesi interaktif):** `npm run tauri dev` → pilih mode
"System + Microphone" → pilih device → Record → bicara + mainkan suara →
Stop → putar MP4: kedua suara terdengar di satu track, sinkron; meter bergerak
saat bicara. Clap test utk offset ≤50ms.

**Catatan headless:** WGC hanya mengirim 1–2 frame saat layar tak berubah →
durasi MP4 pendek; offset A/V yang diukur (16–18ms) tetap valid karena
dihitung dari first-frame timestamps di master timeline. Di sesi nyata 30fps.

---

## M8 — Sensitive data sensor (✅ selesai)

**DoD:** setting keyword pre-record → sensor aktif saat label muncul → frame output bersih.

**Yang sudah ada:**
- `capture/censor/mod.rs`: `CensorConfig` (persist JSON app-config-dir), geometri anchor (kanan label +5px, 500×100, clamp), `RegionTracker` dwell (aktif di hit pertama, mati setelah 2 scan miss), `stamp()` BGRA solid hitam — 8 unit test.
- `capture/censor/ocr.rs`: `ort` (ONNX Runtime CPU) × PP-OCRv4 mobile latin (det DB + rec CTC, model di-pin di `models/`, dibundel via `bundle.resources`), pre/post-processing Rust murni, CTC decode, keyword match substring per text-line — termasuk test end-to-end dengan label "Password" raster manual (det → rec → " Password" → hit).
- Pump `capture/mod.rs`: feed frame terbaru ke worker 2 fps (slot latest), stamp region SEBELUM tulis `video.raw` (rahasia tidak pernah ke disk); worker fatal → recording stop + status `censor-failed` (fail-closed).
- UI: panel sensor (toggle, chip keyword, box W/H/gap), badge "● N area disensor" via event `censor-status`, overlay kotak di preview canvas (rect sama dengan yang distempel).

**Verifikasi:** `cargo test` (31 pass); `npm run tauri dev` → aktifkan sensor → record → buka form login → kotak hitam muncul di samping label "Password" di MP4.

---

## M9 — ffmpeg sidecar (✅ selesai)

**DoD:** app jalan di mesin tanpa ffmpeg di PATH.

**Yang sudah ada:** `bundle.externalBin` + `binaries/download-ffmpeg.sh` (gyan essentials GPL, libx264), resolusi `ffmpeg_bin()` di `mux.rs` (exe dir → dev binaries → PATH). Binary 103 MB di-gitignore; tester CI/runner unduh via script.

---

## Struktur target akhir (untuk referensi saat implementasi)

```
src-tauri/src/
├── lib.rs                  ← Tauri builder, commands, event emit
├── main.rs
└── capture/
    ├── mod.rs              ← orchestrator (start/stop), state, error type
    ├── clock.rs            ← MasterClock + SourceClockState (port Cap)
    ├── timeline.rs         ← first-frame align, gap tracker, tail pad
    ├── encode.rs           ← H264 + AAC (ffmpeg-next)
    ├── mux.rs              ← MP4 muxer (ffmpeg-next)
    ├── platform/
        ├── mod.rs          ← trait ScreenCapture, type alias per OS
        ├── windows.rs      ← WGC + WASAPI loopback (v1)
        └── macos.rs        ← stub cfg-off (v2)
    └── censor/             ← M8: sensitive data sensor (PD-0003) ✅
        ├── mod.rs          ← config, region tracker (dwell), geometri 5px/500×100
        └── ocr.rs          ← ort ×2 (PP-OCRv4 mobile latn det+rec), pre/post-process
    binaries/               ← M9: ffmpeg sidecar (gitignored, download-ffmpeg.sh)
```

## Urutan kerja yang disarankan

1. M2 dulu (video saja) — paling cepat memberi hasil visual.
2. M3 — tambah audio + clock; **ini bagian tersulit, lakukan dengan test
   unit clock dulu**.
3. M4 — encode/mux setelah pipeline stabil.
4. M5 — UI polish.
5. M6 — robustness.

> Saran: tiap milestone di-commit terpisah; verifikasi DoD sebelum lanjut.
