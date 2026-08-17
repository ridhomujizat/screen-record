# Plan — Implementasi Screen Recorder (M1 → M6)

> Platform: Windows v1 (WGC + WASAPI). Struktur sudah siap macOS (ADR-002, ADR-011).
> Setiap milestone = bisa dijalankan & diverifikasi.
> Terkait: [PRD PD-0001](../prd/PD-0001-screen-recorder-tauri-react.md) ·
> [Diagram 01](../diagram/01-system-context-and-record-flow.md), [02](../diagram/02-av-sync-pipeline.md)

---

## Gambaran

```
M1 ✅ scaffold (sudah: Tauri 2 + React 19 + TS)
M2   capture video (WGC) → preview frame ke UI (tanpa encode)
M3   audio + A/V sync (WASAPI loopback + MasterClock + alignment)
M4   encode + mux → MP4 valid, stop bersih
M5   UI penuh (target list, area select, timer, buka folder)
M6   robustness (error, scale-on-resize, polish)
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

## M5 — UI penuh

**DoD:** Alur lengkap: list sumber → pilih → record → stop → file tersimpan
→ buka folder.

**React:**
- `TargetPicker`: daftar display + windows (+ area select nanti).
- `AreaSelect`: overlay drag-select (koordinat dikirim ke Rust).
- `RecordControls`: tombol record/stop, timer (dari event), status.
- `ResultCard`: path file + tombol "Buka Folder" (plugin opener).
- Status bar: error dari Rust (event) tampil rapi.

**Rust tambahan:**
- Command `open_folder(path)` (plugin opener sudah terpasang).
- Emit `record-status` (idle/recording/finished/error + path).

**Catatan:**
- Area select: hitung koordinat relatif ke display (logical → physical
  seperti Cap `logical_area_to_physical_bounds`).
- Window capture: pakai `GraphicsCaptureItem` dari window handle.

---

## M6 — Robustness & polish

**DoD:** Rekaman tidak rusak pada kasus: resolusi berubah, audio drop, stop
mendadak, disk penuh (error jelas ke UI).

**Tasks:**
- [ ] Resolusi layar berubah → scale frame ke dimensi awal (seperti Cap).
- [ ] Audio gap → silence (sudah di M3); device hilang → error event.
- [ ] Stop path idempotent; double-stop aman.
- [ ] Cek free disk sebelum start (mis. butuh 500MB) → tolak + pesan.
- [ ] Rekaman partial tersimpan kalau error (tulis fragment, bukan file rusak)
      — P1.
- [ ] `cargo check` + `npm run build` bersih; test unit jalan.

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
    └── platform/
        ├── mod.rs          ← trait ScreenCapture, type alias per OS
        ├── windows.rs      ← WGC + WASAPI loopback (v1)
        └── macos.rs        ← stub cfg-off (v2)
```

## Urutan kerja yang disarankan

1. M2 dulu (video saja) — paling cepat memberi hasil visual.
2. M3 — tambah audio + clock; **ini bagian tersulit, lakukan dengan test
   unit clock dulu**.
3. M4 — encode/mux setelah pipeline stabil.
4. M5 — UI polish.
5. M6 — robustness.

> Saran: tiap milestone di-commit terpisah; verifikasi DoD sebelum lanjut.
