//! PaddleOCR PP-OCRv4 mobile latin via ONNX Runtime (ADR-0014).
//!
//! det: DB text-detection mobile (ch_PP-OCRv4_det_mobile.onnx)
//! rec: latin CTC recognition mobile (en_PP-OCRv4_rec_mobile.onnx)
//! Recognized text is used in-process for keyword matching only — never
//! logged or emitted (PRD PD-0003 §4.2).

use std::path::{Path, PathBuf};

use ort::session::Session;
use ort::value::Tensor;

use super::Rect;

pub const MAX_SCAN_W: u32 = 1280;
const DET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const DET_STD: [f32; 3] = [0.229, 0.224, 0.225];
const DB_THRESH: f32 = 0.3;
const DB_BOX_THRESH: f32 = 0.45;
const DB_MIN_BOX: u32 = 3;
const DB_EXPAND: f32 = 0.12;
const REC_MAX_W: u32 = 640;

pub struct OcrEngine {
    det: Session,
    rec: Session,
    det_in: String,
    rec_in: String,
    /// [0]="" (CTC blank), [1..=n]=dict chars, [n+1]=" ".
    charset: Vec<String>,
    rec_h: u32,
}

impl OcrEngine {
    /// Load + validate models from `dir` (det.onnx, rec.onnx, en_dict.txt).
    /// Fails closed on any mismatch (PRD §4.1).
    pub fn load(dir: &Path) -> Result<Self, String> {
        let mut charset = vec![String::new()];
        let dict = std::fs::read_to_string(dir.join("en_dict.txt"))
            .map_err(|e| format!("read en_dict.txt: {e}"))?;
        for line in dict.lines() {
            charset.push(line.to_string());
        }
        charset.push(" ".to_string());

        let mk = |f: &str| -> Result<Session, String> {
            Session::builder()
                .map_err(|e| e.to_string())?
                .with_intra_threads(2)
                .map_err(|e| e.to_string())?
                .commit_from_file(dir.join(f))
                .map_err(|e| format!("load {f}: {e}"))
        };
        let det = mk("det.onnx")?;
        let rec = mk("rec.onnx")?;

        let det_in = det.inputs()[0].name().to_string();
        let rec_in = rec.inputs()[0].name().to_string();

        let mut eng = Self { det, rec, det_in, rec_in, charset, rec_h: 48 };
        // Probe the rec model: fixed input height (v4=48, older=32) and
        // output classes must match the charset or decode would be garbage.
        let (ok, classes) = eng.try_rec_probe(48);
        let (ok, classes) = if ok { (ok, classes) } else { eng.try_rec_probe(32) };
        if !ok {
            return Err("rec model probe failed (height 48 and 32 both rejected)".into());
        }
        if classes as usize != eng.charset.len() {
            return Err(format!(
                "rec charset mismatch: model has {classes} classes, dict has {}",
                eng.charset.len()
            ));
        }
        Ok(eng)
    }

    fn try_rec_probe(&mut self, h: u32) -> (bool, u64) {
        let data = vec![0f32; (3 * h * 64) as usize];
        let input = match Tensor::from_array((vec![1usize, 3, h as usize, 64usize], data)) {
            Ok(t) => t,
            Err(_) => return (false, 0),
        };
        let name = self.rec_in.clone();
        let run = self.rec.run(ort::inputs![name => input]);
        let Ok(out) = run else { return (false, 0) };
        let Ok((shape, _)) = out[0].try_extract_tensor::<f32>() else { return (false, 0) };
        if shape.len() != 3 || shape[2] <= 0 {
            return (false, 0);
        }
        self.rec_h = h;
        (true, shape[2] as u64)
    }

    /// Scan one BGRA frame; returns matched (keyword, label-bbox full-res).
    pub fn scan(
        &mut self,
        bgra: &[u8],
        w: u32,
        h: u32,
        keywords: &[String],
    ) -> Result<Vec<(String, Rect)>, String> {
        let expected = w as usize * h as usize * 4;
        if bgra.len() != expected {
            return Err(format!("frame size {} != expected {expected}", bgra.len()));
        }
        let scale = (w.min(MAX_SCAN_W) as f32) / w as f32;
        let dw = ((w as f32 * scale).ceil() as u32).max(1);
        let dh = ((h as f32 * scale).ceil() as u32).max(1);

        // 1) downscale BGRA → RGB u8 (bilinear)
        let rgb = bgra_to_rgb(bgra, w, h, dw, dh);

        // 2) det input: normalized + zero-padded to /32
        let pw = dw.div_ceil(32) * 32;
        let ph = dh.div_ceil(32) * 32;
        let plane = (pw * ph) as usize;
        let mut input = vec![0f32; plane * 3];
        for y in 0..dh as usize {
            for x in 0..dw as usize {
                let si = (y * dw as usize + x) * 3;
                let o = y * pw as usize + x;
                input[o] = (rgb[si] as f32 / 255.0 - DET_MEAN[0]) / DET_STD[0];
                input[plane + o] = (rgb[si + 1] as f32 / 255.0 - DET_MEAN[1]) / DET_STD[1];
                input[2 * plane + o] = (rgb[si + 2] as f32 / 255.0 - DET_MEAN[2]) / DET_STD[2];
            }
        }
        let t = Tensor::from_array((vec![1usize, 3, ph as usize, pw as usize], input))
            .map_err(|e| e.to_string())?;
        let name = self.det_in.clone();
        // Copy the prob map out so the session borrow ends before `recognize`
        // takes `&mut self` (rc.13 `run` holds a mutable session borrow).
        let probs: Vec<f32> = {
            let out = self.det.run(ort::inputs![name => t]).map_err(|e| e.to_string())?;
            let (shape, p) = out[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| e.to_string())?;
            if shape.len() != 4 || p.len() != plane {
                return Err(format!("det output shape mismatch: {shape:?}"));
            }
            p.to_vec()
        };

        // 3) DB boxes (downscaled coords) → recognize each line
        let boxes = db_boxes(&probs, pw, ph, dw, dh);
        #[cfg(test)]
        eprintln!("[ocr-test] det boxes: {boxes:?} (dw={dw} dh={dh})");
        let mut hits = Vec::new();
        for (bx, by, bw, bh) in boxes {
            let text = self.recognize(&rgb, dw, dh, bx, by, bw, bh);
            #[cfg(test)]
            eprintln!("[ocr-test] rec line: {text:?} @ {bx},{by} {bw}x{bh}");
            if text.len() < 2 {
                continue;
            }
            let lower = text.to_lowercase();
            for kw in keywords {
                if lower.contains(kw.as_str()) {
                    let r = Rect {
                        x: (bx as f32 / scale) as i32,
                        y: (by as f32 / scale) as i32,
                        w: (bw as f32 / scale).max(1.0) as i32,
                        h: (bh as f32 / scale).max(1.0) as i32,
                    };
                    hits.push((kw.clone(), r));
                }
            }
        }
        Ok(hits)
    }

    fn recognize(&mut self, rgb: &[u8], dw: u32, dh: u32, x: u32, y: u32, bw: u32, bh: u32) -> String {
        // crop clamp
        let x1 = (x + bw).min(dw);
        let y1 = (y + bh).min(dh);
        let cw = x1.saturating_sub(x).max(1);
        let ch = y1.saturating_sub(y).max(1);
        let th = self.rec_h;
        let tw = (((cw as f32 / ch as f32) * th as f32).round() as u32).clamp(16, REC_MAX_W);

        // bilinear resize crop → normalized NCHW [-1,1]
        let mut input = vec![0f32; (3 * th * tw) as usize];
        let plane = (th * tw) as usize;
        for ty in 0..th {
            let sy = y + ((ty as f32 + 0.5) * ch as f32 / th as f32) as u32;
            let sy = sy.clamp(0, dh - 1);
            for tx in 0..tw {
                let sx = x + ((tx as f32 + 0.5) * cw as f32 / tw as f32) as u32;
                let sx = sx.clamp(0, dw - 1);
                // rgb is the full-frame buffer — index with absolute coords/stride dw
                let si = (sy * dw + sx) as usize * 3;
                let o = (ty * tw + tx) as usize;
                // PaddleOCR rec is trained on cv2 BGR — feed B,G,R planes.
                input[o] = rgb[si + 2] as f32 / 127.5 - 1.0;
                input[plane + o] = rgb[si + 1] as f32 / 127.5 - 1.0;
                input[2 * plane + o] = rgb[si] as f32 / 127.5 - 1.0;
            }
        }
        let Ok(t) = Tensor::from_array((vec![1usize, 3, th as usize, tw as usize], input)) else {
            return String::new();
        };
        let name = self.rec_in.clone();
        let Ok(out) = self.rec.run(ort::inputs![name => t]) else {
            return String::new();
        };
        let Ok((shape, logits)) = out[0].try_extract_tensor::<f32>() else {
            return String::new();
        };
        if shape.len() != 3 {
            return String::new();
        }
        let tlen = shape[1] as usize;
        let classes = shape[2] as usize;
        ctc_decode(logits, tlen, classes, &self.charset)
    }
}

/// Greedy CTC: argmax per timestep, collapse repeats, drop blank (index 0).
fn ctc_decode(logits: &[f32], tlen: usize, classes: usize, charset: &[String]) -> String {
    let mut out = String::new();
    let mut prev: usize = 0;
    for t in 0..tlen {
        let row = &logits[t * classes..(t + 1) * classes];
        let mut best = 0usize;
        let mut best_v = f32::MIN;
        for (i, v) in row.iter().enumerate() {
            if *v > best_v {
                best_v = *v;
                best = i;
            }
        }
        if best != 0 && best != prev && best < charset.len() {
            out.push_str(&charset[best]);
        }
        prev = best;
    }
    out
}

/// Bilinear BGRA→RGB downscale (identity when same size).
fn bgra_to_rgb(bgra: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 3) as usize];
    if sw == dw && sh == dh {
        for i in 0..(dw * dh) as usize {
            out[i * 3] = bgra[i * 4 + 2];
            out[i * 3 + 1] = bgra[i * 4 + 1];
            out[i * 3 + 2] = bgra[i * 4];
        }
        return out;
    }
    for y in 0..dh {
        let fy = (y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5;
        let y0 = fy.floor().clamp(0.0, (sh - 1) as f32) as u32;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = fy - y0 as f32;
        for x in 0..dw {
            let fx = (x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5;
            let x0 = fx.floor().clamp(0.0, (sw - 1) as f32) as u32;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = fx - x0 as f32;
            for c in 0..3usize {
                let s = 2 - c; // BGRA: R at +2, G at +1, B at +0 → rgb[c]
                let p00 = bgra[((y0 * sw + x0) * 4) as usize + s] as f32;
                let p01 = bgra[((y0 * sw + x1) * 4) as usize + s] as f32;
                let p10 = bgra[((y1 * sw + x0) * 4) as usize + s] as f32;
                let p11 = bgra[((y1 * sw + x1) * 4) as usize + s] as f32;
                let v = p00 * (1.0 - wx) * (1.0 - wy)
                    + p01 * wx * (1.0 - wy)
                    + p10 * (1.0 - wx) * wy
                    + p11 * wx * wy;
                out[((y * dw + x) * 3) as usize + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// DB post-process (simplified, axis-aligned): threshold → 8-connected
/// components → score/size filter → small expansion (ADR-0015 notes).
fn db_boxes(probs: &[f32], pw: u32, ph: u32, dw: u32, dh: u32) -> Vec<(u32, u32, u32, u32)> {
    let pwu = pw as usize;
    let bitmap: Vec<bool> = probs.iter().map(|p| *p > DB_THRESH).collect();
    let mut visited = vec![false; bitmap.len()];
    let mut boxes = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    for start in 0..bitmap.len() {
        if !bitmap[start] || visited[start] {
            continue;
        }
        stack.clear();
        stack.push(start);
        visited[start] = true;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        let mut count = 0usize;
        let mut sum = 0f32;
        while let Some(i) = stack.pop() {
            let (cx, cy) = (i % pwu, i / pwu);
            x0 = x0.min(cx);
            y0 = y0.min(cy);
            x1 = x1.max(cx);
            y1 = y1.max(cy);
            count += 1;
            sum += probs[i];
            for dy in [usize::MAX, 0, 1] {
                for dx in [usize::MAX, 0, 1] {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    // wrapping add implements -1 safely (8-neighbourhood)
                    let nx = cx.wrapping_add(dx);
                    let ny = cy.wrapping_add(dy);
                    if nx >= pwu || ny >= ph as usize {
                        continue;
                    }
                    let ni = ny * pwu + nx;
                    if bitmap[ni] && !visited[ni] {
                        visited[ni] = true;
                        stack.push(ni);
                    }
                }
            }
        }
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        if bw < DB_MIN_BOX as usize || bh < DB_MIN_BOX as usize || count < 12 {
            continue;
        }
        if sum / (count as f32) < DB_BOX_THRESH {
            continue;
        }
        // expand a touch (cheap stand-in for DB unclip)
        let pad = (DB_EXPAND * (bw.max(bh)) as f32).max(2.0) as i32;
        let ex0 = (x0 as i32 - pad).max(0) as u32;
        let ey0 = (y0 as i32 - pad).max(0) as u32;
        let ex1 = (x1 as i32 + pad + 1).min(dw as i32) as u32;
        let ey1 = (y1 as i32 + pad + 1).min(dh as i32) as u32;
        if ex1 > ex0 && ey1 > ey0 {
            boxes.push((ex0, ey0, ex1 - ex0, ey1 - ey0));
        }
    }
    boxes
}

/// Resolve the bundled models dir: Tauri resource path, with a dev-time
/// fallback to `<crate>/models` (tauri.conf.json `bundle.resources`).
pub fn models_dir(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;
    if let Ok(res) = app.path().resolve("models", tauri::path::BaseDirectory::Resource) {
        if res.join("det.onnx").exists() {
            return res;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctc_decodes_collapses_and_drops_blank() {
        // charset: [blank, 'a', 'b', ' ']
        let cs = vec!["".into(), "a".into(), "b".into(), " ".into()];
        // t0: 'a', t1: 'a' (repeat), t2: blank, t3: 'b', t4: 'a'
        let mk = |idx: usize| -> Vec<f32> {
            let mut r = vec![-5f32; 4];
            r[idx] = 5.0;
            r
        };
        let mut logits = Vec::new();
        for i in [1, 1, 0, 2, 1] {
            logits.extend(mk(i));
        }
        assert_eq!(ctc_decode(&logits, 5, 4, &cs), "aba");
    }

    #[test]
    fn db_boxes_finds_two_blobs() {
        // 32x8 "prob map": two hot rectangles (≥3px tall each)
        let (pw, ph) = (32u32, 8u32);
        let mut probs = vec![0.0f32; (pw * ph) as usize];
        for y in 1..4 {
            for x in 2..10 {
                probs[(y * pw + x) as usize] = 0.9;
            }
        }
        for y in 5..8 {
            for x in 20..28 {
                probs[(y * pw + x) as usize] = 0.8;
            }
        }
        let boxes = db_boxes(&probs, pw, ph, 30, 8); // dw=30 < pw → clamp test too
        assert_eq!(boxes.len(), 2);
        let (bx, by, bw, bh) = boxes[0];
        assert!(bx <= 2 && by <= 1, "box covers blob1: {boxes:?}");
        assert!(bx + bw >= 10 && by + bh >= 3);
        assert!(bx + bw <= 30 && by + bh <= 8);
    }

    #[test]
    fn db_boxes_rejects_low_score() {
        let (pw, ph) = (32u32, 8u32);
        let probs = vec![0.35f32; (pw * ph) as usize]; // above thresh but weak mean
        assert!(db_boxes(&probs, pw, ph, 32, 8).is_empty());
    }

    #[test]
    fn bgra_to_rgb_identity_and_downscale() {
        // 2x2 BGRA (red, green / blue, white) → identity
        let src: Vec<u8> = vec![
            0, 0, 255, 255, 0, 255, 0, 255, //
            255, 0, 0, 255, 255, 255, 255, 255,
        ];
        let rgb = bgra_to_rgb(&src, 2, 2, 2, 2);
        assert_eq!(rgb, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
        // downscale 2x2 → 1x1 = average of the 4 px (R:(255+0+0+255)/4≈128)
        let one = bgra_to_rgb(&src, 2, 2, 1, 1);
        assert_eq!(one.len(), 3);
        assert!((one[0] as i32 - 128).abs() <= 1, "r={}", one[0]);
    }

    // Real-model smoke: blank frame → no hits (needs models/ present).
    #[test]
    fn scan_blank_frame_no_hits() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
        if !dir.join("det.onnx").exists() {
            return; // models not bundled in CI — skip silently
        }
        let mut eng = OcrEngine::load(&dir).expect("models load");
        let frame = vec![249u8; 320 * 200 * 4]; // light gray
        let hits = eng
            .scan(&frame, 320, 200, &["password".to_string()])
            .expect("scan runs");
        assert!(hits.is_empty(), "blank frame must yield no hits: {hits:?}");
    }

    /// 5x7 bitmap font for the letters of "Password" (test-only rasterizer).
    fn glyph(c: char) -> [&'static str; 7] {
        match c {
            'P' => ["11110", "10001", "10001", "11110", "10000", "10000", "00000"],
            'a' => ["00000", "00000", "01110", "00001", "01111", "10001", "01111"],
            's' => ["00000", "00000", "01111", "10000", "01110", "00001", "11110"],
            'w' => ["00000", "00000", "10001", "10001", "10101", "10101", "01010"],
            'o' => ["00000", "00000", "01110", "10001", "10001", "10001", "01110"],
            'r' => ["00000", "00000", "11110", "10001", "10000", "10000", "10000"],
            'd' => ["00010", "00010", "01110", "10010", "10010", "10010", "01110"],
            _ => ["00000"; 7],
        }
    }

    fn draw_text(frame: &mut [u8], w: u32, text: &str, ox: u32, oy: u32, scale: u32) {
        for (ci, ch) in text.chars().enumerate() {
            let rows = glyph(ch);
            for (ry, row) in rows.iter().enumerate() {
                for (rx, b) in row.chars().enumerate() {
                    if b != '1' {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let x = ox + (ci as u32) * (5 * scale + 2) + rx as u32 * scale + dx;
                            let y = oy + ry as u32 * scale + dy;
                            let i = ((y * w + x) * 4) as usize;
                            frame[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
                        }
                    }
                }
            }
        }
    }

    // End-to-end: drawn "Password" label is detected and bbox lands on it.
    #[test]
    fn scan_detects_drawn_password_label() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
        if !dir.join("det.onnx").exists() {
            return;
        }
        let mut eng = OcrEngine::load(&dir).expect("models load");
        let (w, h) = (1280u32, 720u32);
        let mut frame = vec![255u8; (w * h * 4) as usize];
        draw_text(&mut frame, w, "Password", 100, 300, 8); // 40x56 glyphs
        let hits = eng
            .scan(&frame, w, h, &["password".to_string()])
            .expect("scan runs");
        assert!(!hits.is_empty(), "label 'Password' must be detected");
        let r = hits[0].1;
        assert!(r.x >= 60 && r.x <= 160, "bbox x near label: {r:?}");
        assert!(r.y >= 260 && r.y <= 340, "bbox y near label: {r:?}");
        assert!(r.w > 100 && r.h > 20, "bbox covers label: {r:?}");
    }
}
