# PRD — PD-0001: Screen Recorder (Tauri + React)

> Status: Active · Date: 2026-08-17 · Epic: screen-record
> Dasar: pelajaran dari `capsoftware/cap` — lihat `docs/cap-screen-record-study.md`
> Keputusan arsitektur: [ADR-0001](../adr/0001-rust-capture-logic-react-ui.md) s/d [ADR-0011](../adr/0011-macos-capture-screencapturekit-draft.md) · Diagram: [01](../diagram/01-system-context-and-record-flow.md), [02](../diagram/02-av-sync-pipeline.md)
> Platform: Windows (v1), macOS (dirancang, diimplementasi belakangan — lihat ADR-0011)

---

## 1. Ringkasan

Aplikasi desktop untuk merekam layar (full display / window / area), dengan
**system audio** (dan nanti mic), yang menghasilkan file MP4 dengan
**sinkronisasi audio-video yang benar** sejak frame pertama.

Aplikasi berbasis **Tauri 2 + React 19 + TypeScript**. Logika capture/encode
berada di sisi **Rust** (Tauri command + native Windows API), frontend React
hanya untuk UI: pilih target, tombol record, preview, status.

## 2. Tujuan & Non-Tujuan

### Tujuan
- Record display penuh, window, atau area tertentu (region select).
- System audio (WASAPI loopback) — direkam bersama video, A/V sync benar.
- Output MP4 (H.264 + AAC).
- UI Tauri: daftar sumber capture, tombol record/stop, indikator berjalan.
- Struktur kode **platform-agnostic** supaya macOS bisa ditambahkan nanti
  tanpa refactor besar.

### Non-Tujuan (v1)
- Editor video, trim/crop timeline, overlay kamera, share/upload, cloud.
- Rekaman window khusus yang menutup diri, window exclude list.
- Streaming, screenshots, GIF.
- Mic & kamera (mic kini diimplementasi: [PD-0002](PD-0002-microphone-capture-and-sync.md); kamera tetap v2).

## 3. User Persona & Use Case

- **Persona utama**: developer/creator yang mau merekam tutorial singkat
  dengan suara sistem, lalu share file MP4.
- Use case inti: pilih "Record Full Screen" → klik Record → lakukan aktivitas
  → klik Stop → file `.mp4` tersimpan → buka lokasi file.

## 4. Fitur & Persyaratan

### 4.1 Target Capture (P0)
- [ ] Full display (pilih monitor mana).
- [ ] Window tertentu (list jendela terbuka).
- [ ] Area/region (drag-select; simpan bounds; crop di capture).
- Semua target memakai satu pipeline — hanya bounds yang beda.

### 4.2 Capture (P0)
- [ ] Video: WGC (Windows.Graphics.Capture) via Direct3D11, format BGRA8.
- [ ] Frame rate: nominal 30fps (cap mengikuti refresh monitor, gate ke 30).
- [ ] Audio: WASAPI loopback, 48kHz stereo (fallback ke config device).
- [ ] Cursor: overlay default (bisa nonaktif).

### 4.3 Sinkronisasi A/V (P0 — kriteria kualitas)
- [ ] Satu master clock; semua timestamp (video & audio) di-remap ke sana.
- [ ] Frame pertama video & audio sejajar (trim/advance audio).
- [ ] Gap audio diisi silence; tail audio di-pad ke durasi video.
- [ ] Toleransi sync: ≤ 50ms drift selama 10 menit rekaman (target).

### 4.4 Encoding & Output (P0)
- [ ] H.264 (software x264; hardware NVENC sebagai enhancement v1.x).
- [ ] AAC audio.
- [ ] Container MP4 (fragmented muxing internal; output standard MP4).
- [ ] Pilihan kualitas: Standard / High (bitrate) — sederhana di v1.

### 4.5 UI (P0)
- [ ] List sumber (display/window/area) dengan preview thumbnail.
- [ ] Tombol Record/Stop + timer.
- [ ] Notifikasi sukses/gagal + tombol "buka folder".
- [ ] Overlay kecil "recording" (optional, kalau murah).

### 4.6 Robustness (P1)
- [ ] Stop bersih: semua source di-stop, muxer di-flush, file final valid.
- [ ] Error handling: device hilang, resolusi berubah (scale frame), disk penuh.
- [ ] Rekaman tetap tersimpan walau UI di-close (opsional; kalau murah).

## 5. Kriteria Sukses / Acceptance (v1)

1. Record full display 60 detik dengan system audio → MP4 bisa diputar di
   VLC/Windows Media Player.
2. Audio & video sinkron: klip berisi bunyi "beep" di layar + suara, offset
   visual vs audibel ≤ 50ms (dicek manual via waveform+frame).
3. Record window & area bekerja dengan crop benar (pixel tepat).
4. Pause tidak diperlukan di v1; stop sekali tekan → file valid.
5. `cargo check` bersih di Windows; struktur modul `#[cfg]` siap untuk macOS.

## 6. Metrik

- Ukuran file: kira-kira 1–2 MB/menit (720p, standard) — wajar utk H.264.
- CPU usage saat record: < 20% satu core (software encoder, 720p30).
- Latency start: < 500ms dari klik Record sampai frame pertama.

## 7. Batas / Out of Scope (v1)

- macOS capture (ScreenCaptureKit) — **struktur siap, implementasi nanti**.
- Kamera & mic terpisah, mixer multi-source.
- Editor, effects, zoom, annotation.
- Cloud sync, share link, auth.
- Rekaman tanpa batas waktu (batas praktis: ruang disk).

## 8. Risiko

| Risiko | Dampak | Mitigasi |
|---|---|---|
| WGC butuh app manifest / GPU | Gagal init | Fallback WARP (software) seperti Cap |
| A/V drift pada rekaman panjang | Sync memburuk | MasterClock berbasis sampel + snap jitter (<70ms), hard-reset (>2s) |
| Privacy permission (macOS nanti) | Gagal capture | Desain permission flow sejak awal |
| Kompleksitas Rust naik | Lambat develop | Pisah modul kecil: `capture`, `clock`, `encode`, `mux` |
| Disk penuh saat rekam | Rekaman rusak | Cek ruang disk sebelum start; error ke UI |

## 9. Milestone

- **M1 (scaffold)**: modul Rust kosong + command Tauri terhubung. ✅ done
- **M2 (capture video)**: WGC → frame BGRA → preview ke UI (tanpa encode).
- **M3 (audio + sync)**: WASAPI loopback + master clock + alignment.
- **M4 (encode & mux)**: H.264+AAC → MP4 valid; stop bersih.
- **M5 (UI penuh)**: target list, area select, timer, buka folder.
- **M6 (robustness)**: error handling, resolve-change scaling, polish.

> Implementasi langkah-demi-langkah ada di [Plan](../plan/PLAN.md); alur capture & sync ada di
> [Diagram 01](../diagram/01-system-context-and-record-flow.md) & [02](../diagram/02-av-sync-pipeline.md).

## 10. Nota Arsitektur Awal (singkat)

```
Tauri command layer (invoke: list_sources, start_record, stop_record, ...)
        │
        ▼
src-tauri/src/capture/            ← logika rekaman (Rust, platform-agnostic core)
        ├── mod.rs
        ├── clock.rs              ← MasterClock + SourceClockState (port dari Cap)
        ├── timeline.rs           ← audio gap/silence/pad, first-frame align
        ├── encode.rs             ← H.264 + AAC
        ├── mux.rs                ← MP4 muxer
        └── platform/
            ├── mod.rs            ← trait ScreenCapture + audio source
            ├── windows.rs        ← WGC + WASAPI (v1)
            └── macos.rs          ← ScreenCaptureKit (v2, stub dulu)
```
