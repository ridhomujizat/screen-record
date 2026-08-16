# Pelajaran: Bagaimana Cap (capsoftware/cap) Melakukan Screen Record & Audio Sync

> Dipelajari dari source code `capsoftware/cap` (commit main, di-clone shallow).
> Tujuan: jadi referensi implementasi aplikasi screen record Tauri + React kita.
> Referensi file asli: `crates/recording`, `crates/timestamp`, `crates/audio`, `crates/scap-direct3d`, `crates/cap-muxer`.

---

## 1. Arsitektur Capture (per platform)

Cap punya layering yang rapi — capture source dipisah dari encoding/muxing:

```
Screen/Window/Area target
   │
   ▼
ScreenCaptureSource (crates/recording/src/sources/screen_capture/)
   ├── macOS  → CMSampleBufferCapture  (ScreenCaptureKit)
   ├── Windows→ Direct3DCapture        (WGC: Windows.Graphics.Capture)
   └── Linux  → X11Capture             (X11 + ffmpeg)
   │
   ▼
OutputPipeline (crates/recording/src/output_pipeline/)
   ├── video source → H264 encoder
   ├── audio source → AAC encoder
   └── muxer: MP4 / fragmented M4S (2-detik segment utk instant-mode)
```

Kunci: **semua frame (video & audio) membawa timestamp asli dari hardware**,
dan pipeline menyatukannya lewat **satu MasterClock**.

---

## 2. Screen Capture Windows (WGC = Windows.Graphics.Capture)

File: `crates/scap-direct3d/src/lib.rs` + `crates/recording/src/sources/screen_capture/windows.rs`

- Pakai **Direct3D11 + WGC** (`Direct3D11CaptureFramePool`), bukan Desktop Duplication API lama.
- Flow:
  1. Buat `ID3D11Device` (fallback hardware → WARP kalau GPU tak mendukung).
  2. `Direct3D11CaptureFramePool::CreateFreeThreaded` dengan ukuran frame pool & format `R8G8B8A8Unorm` (BGRA support flag).
  3. Daftarkan event handler `FrameArrived` — dipanggil tiap ada frame baru dari OS.
  4. Dalam handler: `TryGetNextFrame()` → `ContentSize()` → `Surface()` → cast ke `ID3D11Texture2D`.
  5. Kalau target area/crop: `CopySubresourceRegion` ke texture crop.
  6. **Timestamp video** = `frame.inner().SystemRelativeTime()` → `Timestamp::PerformanceCounter(...)` (QPC — QueryPerformanceCounter).

```rust
// pola inti (windows.rs ~line 505)
move |frame| {
    let capture_time = frame.inner().SystemRelativeTime()?;   // QPC dari WGC
    let timestamp = Timestamp::PerformanceCounter(PerformanceCounterTimestamp::new(capture_time.Duration));
    // → VideoFrame { frame, timestamp } dikirim ke channel output_pipeline
}
```

- **Cadence gate**: WGC kasih frame per update layar (bisa = refresh rate monitor).
  Cap batasi ke nominal fps dengan `FrameCadenceGate` sebelum konversi apa pun
  (file `crates/recording/src/sources/screen_capture/cadence.rs`).
- Kalau resolusi layar berubah saat merekam: frame di-**scale** dulu biar konsisten dengan dimensi awal.
- Cursor: flag `show_cursor` (bisa digabung/di-exclude).
- WGC menangkap frame **tanpa menampilkan dirinya sendiri** — yang kita butuhkan utk rec tanpa loop.

---

## 3. Audio Capture (System Audio + Mic)

- **System audio (Windows)**: pakai **WASAPI loopback** via CPAL (lihat `create_system_audio_capturer` di windows.rs).
  - Timestamp: `Timestamp::from_cpal(info.timestamp().capture)` — QPC asli dari WASAPI.
  - Ada resampler ke format pipeline (format mismatch di-handle di mixer).
- **Mic**: source terpisah (`crates/recording/src/feeds/microphone.rs`, `sources/microphone.rs`), juga CPAL.
- **Mixer**: `crates/recording/src/sources/audio_mixer.rs` (1290 baris) — mencampur system audio + mic menjadi satu stream, dan **menangani gap/starvation**:
  - Kalau ada gap antar frame > threshold (70ms wired / 160ms wireless): **insert silence** supaya timeline tetap kontinu.
  - Kalau source "stall" terlalu lama: reset buffer source.
  - Deteksi format mismatch (sample rate/channel berubah) → re-init source.

---

## 4. Kunci Sinkronisasi A/V: MasterClock + SourceClockState

File: `crates/timestamp/src/master_clock.rs` — ini **jantung A/V sync Cap**.

### MasterClock
- Satu jam global untuk seluruh pipeline. Mulai dari `Timestamps::now()` saat record.
- Menghitung waktu dari **jumlah sampel audio yang sudah di-commit** (`samples_committed`), rate default 48kHz, chunk 1024 sampel.
- `tick()` / `advance_samples()` majukan jam; `elapsed_ns()` = wall clock sejak start.
- Karena berbasis sampel audio, timeline selalu mulus (tak melompat) — audio jadi "jantung" timeline.

### SourceClockState (remap per-source)
Setiap source (video, system-audio, mic, kamera) punya `SourceClockState` yang **memetakan timestamp raw hardware ke timeline master**:

```rust
let remap = source.clock_state.remap(&master_clock, raw_timestamp, frame_duration_ns);
```

Logika remap (penting banget):
1. **Direct/Trusted**: kalau timestamp raw dekat dengan wall-clock (selisih < 2s) → pakai langsung (`Trusted`, adjust = 0).
2. **InitialAdjust**: frame pertama → hitung offset `now - raw` sebagai baseline.
3. **Smoothed (snap jitter)**: kalau timestamp menyimpang dari cadence yang diharapkan tapi < 70ms → **snap ke next_expected** (cadence ladder). Ini menghilangkan jitter mikro tanpa merusak kecepatan.
4. **HardReset**: kalau lompatan > 2s → buang baseline, re-sync (mis. device di-reset).
5. `next_expected_ns = ts + duration` → prediksi cadence utk frame berikutnya.

Hasil remap = `master_ns` (waktu di timeline master). **Semua source (audio & video) dibawa ke timeline yang sama ini** → itu kenapa A/V sinkron.

Konstanta kunci:
- `TS_SMOOTHING_THRESHOLD_NS = 70ms` — jitter di bawah ini di-snap.
- `MAX_TS_VAR_NS = 2s` — di atas ini dianggap hard reset.
- `AUDIO_OUTPUT_FRAMES = 1024`, `DEFAULT_SAMPLE_RATE = 48_000`.

---

## 5. Audio Timestamp Generator (agar sample-counted & rate-aware)

File: `crates/recording/src/output_pipeline/core.rs` (`AudioTimestampGenerator`)

- Audio timeline dihitung dari **jumlah sampel** (`total_samples`) → `samples_to_nanos(total, rate)`, bukan dari wall clock.
- **Penting**: generator jalan di sample rate asli source (mis. mic 44.1kHz), bukan selalu 48kHz. Kalau dihitung pake 48kHz padahal source 44.1kHz, timeline lag → gap-tracker "mengoreksi" dengan silence palsu → video jadi salah tempo.
- `advance_clock()`: konversi cumulative samples → clock samples (pakai total kumulatif, bukan per-buffer, biar tidak ada drift pembulatan untuk ratio non-integer 44.1k→48k).
- Ini mencegah audio "menjauh" dari video selama rekaman panjang.

---

## 6. Alignment Frame Pertama (Video Start Gate)

File: `crates/recording/src/output_pipeline/core.rs` → `apply_video_start_gate`

Karena audio & video mulai dari thread berbeda, frame pertama bisa tidak sejajar. Solusinya:
1. Video mulai → catat `video_start_ns`.
2. Audio frame pertama datang → hitung offset `video_start - audio_start` (dalam ns).
3. Kalau offset < limit (`AV_START_ALIGNMENT_LIMIT_NS`):
   - **Audio lebih awal** (offset > 0) → **trim** leading samples audio sebanyak offset.
   - **Video lebih awal** (offset < 0) → **advance audio timeline** dengan silence sebesar offset.
4. Timestamp audio yang di-trim ikut dimajukan (`frame.timestamp + trim_duration`) supaya metadata `mic_start_time` konsisten.

→ Hasilnya frame pertama video & audio benar-benar mulai bersamaan di timeline.

---

## 7. Gap Tracking & Tail Padding

File: `crates/recording/src/output_pipeline/core.rs` (`AudioGapTracker`)

- Selama rekam, kalau ada gap audio > threshold → insert silence (audio di timeline tetap kontinu).
- Di akhir rekaman, track audio di-pad dengan silence supaya panjangnya **sama dengan video** (`audio_tail_padding_duration` = target − elapsed).
- Ada summary overlap-trim (`AudioGapSummary`) yang di-surface ke editor supaya bisa kompensasi drift saat editing.

---

## 8. Post-Recording Sync Calibration (khusus kamera+mic)

File: `crates/recording/src/sync_calibration.rs` + `crates/audio/src/sync_analysis.rs`

Ini untuk **lip-sync kamera vs mic** (bukan screen vs system-audio), tapi idenya reusable:

1. **Deteksi event di audio**: transient/onset energi (window 10ms, hop 2.5ms; onset = energi > avg + 15dB).
2. **Deteksi event di video**: frame-motion peaks (luma diff antar frame, sampling grid 16px).
3. **Korelasi**: untuk tiap audio transient, cari video peak dalam ±500ms, ambil yang strength terbaik.
4. **Hitung offset**: weighted average dari semua matched event (weight = confidence), + consistency score.
5. **Simpan kalibrasi per-device** (`CalibrationStore`): offset di-average secara eksponensial antar sesi (decay 0.7^n) → makin sering rekam, makin akurat.
6. Konfirmasi tinggi (confidence > 0.5) baru dipakai.

> Screen-vs-system-audio TIDAK pakai kalibrasi ini — keduanya sudah di-sync real-time via MasterClock di atas (karena QPC/WASAPI & WGC share sumber waktu yang sama).

---

## 9. Encoding & Muxing

- **Video**: H264 (ffmpeg) — `H264EncoderBuilder` dengan BPP (bits-per-pixel) & preset:
  - Studio quality: `ULTRA_BPP`/medium preset, atau `QUALITY_BPP`/ultrafast.
  - Instant mode: `INSTANT_MODE_BPP`, preset ultrafast (biar cepat).
- **Audio**: AAC.
- **Container**:
  - Studio mode: MP4 langsung (`WindowsMuxer`/`AVFoundationMp4Muxer`).
  - Instant mode: **fragmented M4S, segment 2 detik** (`WindowsFragmentedM4SMuxer`) → bisa preview cepat + kalau crash, data tersegment aman.
  - Ada juga **out-of-process muxer** (`cap-muxer` binary) untuk isolasi — muxer crash tak membunuh rekaman.

---

## 10. Ringkasan "Resep" Sync untuk Implementasi Kita

Untuk membuat screen recorder Tauri + React (Windows) yang A/V sync-nya benar:

1. **Capture video**: WGC (Direct3D11CaptureFramePool + FrameArrived). Timestamp pakai `SystemRelativeTime` (QPC).
2. **Capture audio**: WASAPI loopback via CPAL. Timestamp pakai `info.timestamp().capture` (juga QPC).
3. **Satu jam bersama**: semua timestamp raw di-remap ke 1 MasterClock. Video & audio di timeline yang sama → sync.
4. **Snap jitter**: frame yang menyimpang < 70ms dari cadence → snap ke cadence, jangan pakai raw (hindari audio/video saling mengejar).
5. **Hard reset**: lompatan > 2s → re-anchor, jangan lanjutkan offset lama.
6. **Align awal**: trim/advance audio agar frame pertama sejajar dengan video.
7. **Gap**: insert silence saat audio drop, pad tail supaya durasi sama dengan video.
8. **Encode**: H264 ultrafast + AAC, mux ke MP4 (atau fragmented M4S kalau mau instant preview).
9. **Panjang rekaman**: biarkan pipeline jalan; stop = stop semua source, tunggu muxer flush.

---

## File Referensi (dalam repo cap-study di /tmp)

| Topik | File |
|---|---|
| Pipeline video (WGC) | `crates/recording/src/sources/screen_capture/windows.rs` |
| WGC low-level | `crates/scap-direct3d/src/lib.rs` |
| Audio mixer + gap handling | `crates/recording/src/sources/audio_mixer.rs` |
| **MasterClock / SourceClockState** | `crates/timestamp/src/master_clock.rs` |
| Audio timestamp generator | `crates/recording/src/output_pipeline/core.rs` |
| Video start gate / alignment | `crates/recording/src/output_pipeline/core.rs` |
| Post-rec sync analysis (kamera) | `crates/audio/src/sync_analysis.rs` |
| Latency correction (playback) | `crates/audio/src/latency.rs` |
| Contoh rekam screen+system-audio | `crates/recording/examples/av-sync-record.rs` |
| Muxer OOP | `crates/cap-muxer/` |
