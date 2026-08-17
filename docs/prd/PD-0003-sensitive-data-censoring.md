# PRD — PD-0003: Sensitive Data Censoring (OCR Keyword → Sensor Area)

> Status: Active · Date: 2026-08-17 · Epic: screen-record
> Lanjutan dari [PD-0001](PD-0001-screen-recorder-tauri-react.md) & [PD-0002](PD-0002-microphone-capture-and-sync.md).
> Keputusan arsitektur: [ADR-0014](../adr/0014-sensitive-data-detection-paddleocr-mobile-latn-onnx.md), [ADR-0015](../adr/0015-censor-boxes-stamped-pre-encode-with-region-dwell.md) · Diagram: [04](../diagram/04-sensitive-data-censoring.md)
> Platform: Windows v1 (OCR via ONNX Runtime CPU — platform-agnostic, siap macOS)

---

## 1. Ringkasan

Saat user merekam layar sambil memasukkan **password / API key / token**, nilai
yang diketik tampil di field input dan ikut terekam. Fitur ini mendeteksi
**label** sensitif di layar (mis. kata "Password", "API Key") secara real-time
dengan **PaddleOCR model mobile latin yang ringan** (deteksi + recognisi teks),
lalu menutup area input **di samping label tersebut** dengan kotak solid —
**jarak 5px dari kata, ukuran 500×100** — pada frame video **sebelum frame
ditulis ke disk**, sehingga nilai sensitif tidak pernah ada di file output.

Prinsip kunci: **kita tidak mendeteksi nilai yang diketik** (tidak perlu — dan
berbahaya kalau salah). Kita mendeteksi *label*-nya, dan sensor area tetangga
tempat nilai itu akan muncul. Label harus terlihat dulu sebelum user bisa
mengetik, jadi kotak sensor sudah aktif sebelum nilai muncul.

Alur penggunaan:
1. **Sebelum record** — user membuka pengaturan sensor: aktifkan fitur, kelola
   daftar keyword (preset tersedia), box size & gap bisa disesuaikan (default
   500×100 / 5px). Disimpan di settings app.
2. **Saat record** — OCR worker memindai frame secara berkala; keyword yang
   cocok → region sensor aktif → setiap frame yang ditulis ke `video.raw`
   ditutup kotak solid pada region tersebut.
3. **Hasil** — MP4 tidak pernah berisi nilai di area input yang berlabel
   sensitif.

## 2. Tujuan & Non-Tujuan

### Tujuan
- Setting **sebelum record**: daftar keyword (case-insensitive, latin),
  ukuran box (default **500×100 px**), gap (**5 px**), toggle on/off;
  dipersist di settings.
- Deteksi real-time: PaddleOCR **PP-OCRv4 mobile** (det + rec **latin**),
  via ONNX Runtime CPU — model total < 15 MB, bundled di app.
- Sensor diterapkan **sebelum penulisan frame** (`video.raw`) — data
  sensitif tidak pernah menulis ke disk, bukan overlay pasca-rekam.
- Kotak **solid hitam** (bukan blur — blur reversible; pixelasi bisa
  di-OCR ulang).
- Region persist (dwell) antar-scan supaya tidak flicker saat OCR jitter
  atau frame sesekali gagal terdeteksi.
- Teks hasil OCR **tidak pernah di-log / di-emit**; hanya keyword cocok +
  bbox yang dipakai; sisanya langsung dibuang.

### Non-Tujuan (v1)
- Deteksi bahasa non-latin (CJK, arab) — model latin saja.
- Deteksi nilai sensitif tanpa label (mis. tabel credential tanpa header).
- Tracking window scroll (region dibuat ulang oleh scan berikutnya; worst-case
  ada jendela ~1 detik — lihat §8 Risiko).
- Blur/pixelasi sebagai gaya sensor (P1, kosmetik — solid tetap default).
- Sensor pada preview UI (preview adalah tampilan user sendiri, bukan output).
- Redact audio / metadata.

## 3. User Persona & Use Case

- **Persona**: sama dengan PD-0001 — developer/creator tutorial — yang saat
  mendemokan harus login / memasukkan API key di depan layar.
- Use case inti: buka pengaturan sensor → pastikan keyword "password", "api
  key" aktif → Record → buka halaman login, ketik password (nilai tertutup
  kotak hitam di samping label "Password") → Stop → MP4 aman dibagikan.

## 4. Fitur & Persyaratan

### 4.1 Pengaturan Pre-Record (P0)
- [ ] Panel "Sensitive Data Sensor" sebelum record: toggle, daftar keyword
      (add/remove), preset default: `password`, `kata sandi`, `api key`,
      `secret`, `token`, `credential`, `passphrase`.
- [ ] Input numerik: Box Width (default **500**), Box Height (default
      **100**), Gap (default **5**) — satuan px koordinat frame penuh.
- [ ] Persist di settings; Tauri command `get_censor_config` /
      `set_censor_config`.
- [ ] Saat toggle on dan model gagal load → `start_record` ditolak dengan
      pesan (fail-closed — jangan merekam dengan sensor mati).

### 4.2 Deteksi OCR (P0 — ADR-0014)
- [ ] Worker thread terpisah; scan tiap **500 ms** (2 fps) pada frame
      terbaru yang di-downscale ke ≤ 1280 px lebar.
- [ ] Pipeline per scan: DB text-detection → crop text-line → CTC
      recognition (latin mobile) → lowercase match terhadap daftar keyword
      (substring pada satu text-line).
- [ ] Bbox di-scale balik ke koordinat frame penuh.
- [ ] Worker sibuk/lambat → skip scan (ambil frame terbaru berikutnya),
      sensor region terakhir tetap dipakai. Tidak pernah memblok jalur
      capture/encode.
- [ ] String hasil recognisi tidak masuk log/event/error — hanya keyword
      yang cocok + geometri.

### 4.3 Region & Geometri Sensor (P0 — ADR-0015)
- [ ] Dari bbox keyword `kw` (full-res):
      `box.x = kw.right + GAP`, `box.y = kw.center_y − BOX_H/2`,
      `box.w = BOX_W`, `box.h = BOX_H`; clamp ke batas frame.
- [ ] Region masuk daftar aktif pada deteksi pertama; keluar setelah
      **2 scan berturut-turut** (±1 s) tidak terdeteksi lagi (dwell) —
      mencegah flicker.
- [ ] Asosiasi antar-scan: keyword sama + pusat bbox masih dalam region
      lama (toleransi ± box) → region yang sama di-refresh, bukan region
      baru.
- [ ] Box solid hitam digambar di frame BGRA **sebelum** ditulis ke
      `video.raw` (di sync pump), sehingga juga sebelum encode MP4.

### 4.4 UI & Status (P0)
- [ ] Indikator saat record: jumlah region sensor aktif (badge kecil,
      mis. "● 2 area disensor") via event `censor-status`.
- [ ] Preview UI menampilkan kotak sensor (dari region yang sama) supaya
      user yakin fitur bekerja.

### 4.5 Robustness (P1)
- [ ] OCR worker panic/error mid-record → rekaman dihentikan + status
      `error` alasan `censor-failed` + path file (user memutuskan hapus/
      simpan). Fail-closed, bukan diam-diam lanjut tanpa sensor.
- [ ] Keyword muncul di tepi kanan frame → box di-clamp (lebar efektif
      menyusut) → badge warning "area sensor terpotong".

## 5. Kriteria Sukses / Acceptance

1. Setting keyword `password`, record 30 s, buka form login berlabel
   "Password", ketik nilai → frame output: kotak hitam 500×100 mulai
   5px di kanan label, nilai input tidak terbaca di MP4 (cek frame-by-frame).
2. Label muncul (dialog terbuka) → sensor tampil paling lama **1 detik**
   setelahnya (1 scan + 1 confirmasi dwell di luar; deteksi pertama langsung
   aktif — lihat catatan dwell di ADR-0015).
3. Label hilang (dialog ditutup) → sensor hilang dalam ≤ 1.5 s, tanpa
   flicker saat label diam di layar.
4. Sensor off / tanpa keyword cocok → output byte-identik dengan perilaku
   hari ini (regresi nol, jalur pump tanpa branch sensor saat disabled).
5. Grep log/session: tidak ada string teks layar selain keyword yang cocok.
6. `cargo test` pass termasuk unit test geometri (x/y/clamp) dan region
   tracker (dwell/refresh).

## 6. Metrik

- Biaya CPU OCR: ≤ 10% satu core rata-rata (scan 2 fps @ ≤1280px, mobile
  models); jalur encode tidak terganggu (0 frame drop tambahan).
- Latensi sensor: label baru → box aktif ≤ 1 s (p95).
- Ukuran app naik ≤ 20 MB (2 model ONNX + ort).

## 7. Batas / Out of Scope (v1)

- Multi-monitor offset (region mengikuti monitor yang direkam — otomatis
  benar karena OCR jalan pada frame target, bukan desktop virtual).
- Sensor kiri (RTL layout / label di kanan field) — v1 kanan saja; flip
  kiri = P1 saat ada bug report nyata.
- OCR GPU / DirectML — CPU dulu; `ort` execution provider bisa ditukar
  tanpa ubah pipeline.

## 8. Risiko

| Risiko | Dampak | Mitigasi |
|---|---|---|
| Jendela leak: label baru muncul → scan berikutnya belum jalan | Nilai sempat terekam ± 0.5–1 s | Label harus tampil sebelum user mengetik (urutan natural); scan 500 ms; dokumentasikan residual risk; P1: turunkan interval saat ada keystroke focus di area label |
| Keyword singkat cocok tidak sengaja ("token" di teks biasa) | Area non-sensitif tertutup | Match per text-line penuh (OCR line-based), keyword minimal 4 huruf di preset; user bisa hapus keyword |
| OCR miss (font kecil/low contrast/stylized) | Tidak tersensor | Model mobile v4 latih pada teks UI/scene; acceptance test pakai form nyata; user tetap bertanggung jawab cek preview |
| Worker ketinggalan saat adegan cepat berubah | Region basi menutup area salah | Dwell 2 scan + asosiasi posisi; region hilang otomatis |
| Model file corrupt/load gagal | Sensor diam-diam mati | Fail-closed di `start_record` + status error |

## 9. Milestone

- **M8 (censor)**: lihat [Plan](../plan/PLAN.md) — settings + geometri +
  region tracker (tanpa OCR, box manual) → integrasi OCR `ort` → UI status.

## 10. Nota Arsitektur (delta dari PD-0002)

```
capture/censor/mod.rs     — config, region tracker (dwell/refresh), geometri
capture/censor/ocr.rs     — ort session ×2 (det, rec), pre/post-process
models/                   — det + rec latn mobile ONNX (bundled)

platform/windows.rs  WGC ──► broadcast frame ──► preview UI
                                        │
                                        ▼ (latest-frame slot, skip if busy)
                              censor/ocr.rs worker (2 fps, ≤1280px)
                                        │ Vec<Region> (Arc<RwLock>)
                                        ▼
capture/mod.rs pump ──► stamp box BGRA ──► mux.rs video.raw ──► ffmpeg MP4
```
