# PRD — PD-0002: Microphone Capture & Sync (System + Mic Mixing)

> Status: Active · Date: 2026-08-17 · Epic: screen-record
> Lanjutan dari [PD-0001](PD-0001-screen-recorder-tauri-react.md) (M1–M6 selesai; mic dulu non-goal v1, sekarang diimplementasi).
> Keputusan arsitektur: [ADR-0012](../adr/0012-microphone-capture-cpal-input.md), [ADR-0013](../adr/0013-defer-audio-mixing-to-ffmpeg-at-finish.md) · Diagram: [03](../diagram/03-microphone-capture-and-mixing.md)
> Platform: Windows v1 (CPAL input; struktur sama utk macOS nanti)

---

## 1. Ringkasan

Tambahkan **microphone** sebagai sumber audio kedua di samping system audio
(WASAPI loopback). Mic direkam dengan timestamp QPC yang sama, di-remap ke
`MasterClock` yang sama (ADR-0003), ditulis sebagai WAV per-source pada master
timeline, lalu **dicampur oleh ffmpeg saat finish** (`amix`) menjadi satu
track AAC di MP4 (ADR-0013).

Mode rekam audio:
- **System only** (default — perilaku hari ini, backward compatible).
- **Mic only** (tanpa keepalive loopback).
- **System + mic** (campur keduanya).

## 2. Tujuan & Non-Tujuan

### Tujuan
- Pilih device mic (list dari CPAL `input_devices()`); default device dipakai
  kalau user tidak memilih.
- Mic frame masuk pipeline dengan timestamp QPC (`info.timestamp().capture`,
  anchor sama dengan loopback) → `SourceClockState("mic-audio")` → MasterClock.
- WAV per-source dirender pada master timeline: first-frame trim, gap-fill
  silence (ADR-0005 diterapkan di layer penulisan WAV).
- Mixing system + mic di ffmpeg saat finish (`amix` + `adelay` + `aresample`,
  `normalize=0`).
- Level meter live per sumber (RMS → event `audio-meter`, throttled).
- Mic di-cabut / berubah format di tengah rekaman → rekaman tetap valid,
  mic jadi silence + warning di status.

### Non-Tujuan (v1)
- Noise suppression, echo cancellation (AEC), compressor/limiter, gain per
  device yang di-persist.
- Track audio terpisah di MP4 (satu track campuran saja).
- Monitor/mix live yang bisa didengar user saat merekam.
- Bluetooth multipoint guarantees (HFP di-toleransi, tidak di-solve).
- Kamera (tetap nanti).

## 3. User Persona & Use Case

- **Persona**: sama dengan PD-0001 (developer/creator tutorial) — tapi kini
  butuh narasi suara (mic) di atas demonstrasi dengan suara sistem.
- Use case inti: pilih target layar → aktifkan "Microphone" + pilih device →
  Record → bicara sambil mendemokan → Stop → satu MP4 dengan suara campuran
  yang sinkron sejak frame pertama.

## 4. Fitur & Persyaratan

### 4.1 Device & Seleksi (P0)
- [ ] Command `list_audio_devices` → list input device (nama, default flag).
- [ ] UI dropdown mic + toggle mode: System / Mic / System+Mic.
- [ ] Mic hilang/tidak ada → mode mic dinonaktifkan dengan pesan, system tetap jalan.

### 4.2 Capture (P0)
- [ ] CPAL input stream pada device terpilih, konversi ke f32 interleaved
  (konversi format sama dengan loopback), native rate (resample di ffmpeg).
- [ ] Timestamp QPC via anchor yang sama dengan system audio (satu keluarga
  waktu → satu MasterClock, tanpa clock baru).
- [ ] Buffer kecil + `try_send` + drop counter (tidak pernah blok callback).

### 4.3 Sync (P0 — kriteria kualitas)
- [ ] Mic memakai `SourceClockState` sendiri; snap jitter < 70ms, hard-reset > 2s
  (ADR-0003, berlaku utk semua sumber).
- [ ] WAV mic & WAV system dirender pada master timeline (gap → silence;
  audio lebih awal dari video → trim leading samples).
- [ ] Audio mulai setelah video → delay via `adelay`/`-itsoffset` (semantik sama
  dengan jalur single-source hari ini).
- [ ] Toleransi: mic-vs-video ≤ 50ms, mic-vs-system ≤ 50ms, drift ≤ 50ms / 10 menit.

### 4.4 Mixing & Output (P0)
- [ ] ≥ 2 track audio → ffmpeg `filter_complex`:
  `aresample=48000` per input + `adelay` per offset + `amix=normalize=0`.
- [ ] 1 track → jalur encode hari ini tanpa perubahan (regresi nol).
- [ ] Clamping saat campur (tidak clipping di luar ±1.0 sebelum AAC).

### 4.5 UI (P0)
- [ ] Meter level mic (bar) live saat merekam — cukup RMS yang di-throttle.
- [ ] Status bertambah: mic frames, mic drops, meter.

### 4.6 Robustness (P1)
- [ ] Mic unplug / stream error mid-record → source dihentikan, WAV di-pad silence,
  warning event; rekaman tetap difinalisasi.
- [ ] Format berubah mid-record (mis. BT HFP switch) → deteksi mismatch →
  re-init source (pola Cap `rate_changed`).

## 5. Kriteria Sukses / Acceptance

1. Record 60s mode "System + Mic" → MP4 valid satu track audio campuran;
   suara sistem & mic keduanya terdengar ( VLC / WMP ).
2. Clap test: offset mic-vs-video ≤ 50ms dan mic-vs-system ≤ 50ms
   (cek via waveform cross-correlation).
3. Mode "Mic only" jalan (tanpa loopback/keepalive).
4. Meter level bergerak saat bicara; diam saat senyap.
5. Cabut mic di tengah rekaman → file tetap valid, sisanya silence + warning.
6. `cargo test` pass termasuk unit test WavWriter gap-fill/trim baru.

## 6. Metrik

- CPU tambahan saat mic aktif: < 2% satu core (satu stream CPAL lagi).
- Latency start tetap < 500ms.
- Ukuran file: naik kecil (AAC campuran, bukan 2 track).

## 7. Batas / Out of Scope (v1)

- AEC/noise suppression/gain persist (plugin ffmpeg `afftdn`/`compand` bisa
  jadi enhancement v1.x tanpa ubah pipeline).
- Track terpisah per sumber di MP4.
- macOS permission mic flow (desain CPAL host sama; permission UI nanti).

## 8. Risiko

| Risiko | Dampak | Mitigasi |
|---|---|---|
| BT headset: buka mic → switch A2DP→HFP | System audio putus/berubah format | Deteksi mismatch/error → re-init source (Cap pattern); rekomendasi wired di UI copy |
| Mic wireless jitter besar | Drift mic | Snap 70ms + hard reset 2s (ADR-0003); threshold wireless 90ms ala Cap bila perlu |
| Campur dua sumber penuh → clipping | Audio pecah | Clamp + `normalize=0`; (limiter = non-goal) |
| Mic menangkap speaker (echo) | Rekaman jelek | No AEC v1 — dokumentasikan "pakai headphone" |
| try_send drop saat CPU tinggi | Gap di WAV mic | Gap-fill silence di WavWriter + drop counter di status |

## 9. Milestone

- **M7 (mic)**: lihat [Plan](../plan/PLAN.md) — device list → WavWriter
  per-source → mic capturer → amix → UI meter.

## 10. Nota Arsitektur (delta dari PD-0001)

```
platform/ + audio.rs
  MicCapturer (CPAL input, mode input vs loopback) ──┐
  SystemAudioCapturer (loopback + keepalive) ─────────┤
                                                      ▼
capture/mod.rs  pump: tiap sumber → SourceClockState → WavWriter (master timeline)
                                                      ▼
mux.rs  finish(): ≥2 WAV → ffmpeg amix+adelay+aresample → MP4 (1 track AAC)
```
