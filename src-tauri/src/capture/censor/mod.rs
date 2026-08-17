//! Sensitive-data censoring (PD-0003, ADR-0015): keyword-anchored solid
//! boxes stamped on frames *before* the disk write, with dwell-stabilized
//! regions. Pure logic + persistence live here; OCR in `ocr.rs`, worker
//! wiring in `capture/mod.rs`.

use std::path::PathBuf;
use tauri::AppHandle;

pub mod ocr;

pub const DWELL_SCANS: u32 = 2;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CensorConfig {
    pub enabled: bool,
    pub keywords: Vec<String>,
    pub box_w: i32,
    pub box_h: i32,
    pub gap: i32,
}

impl Default for CensorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            keywords: [
                "password", "kata sandi", "api key", "secret", "token", "credential", "passphrase",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            box_w: 500,
            box_h: 100,
            gap: 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn center_y(&self) -> i32 {
        self.y + self.h / 2
    }
    pub fn center_x(&self) -> i32 {
        self.x + self.w / 2
    }
}

/// Censor box anchored 5px to the right of the keyword label, vertically
/// centered on the label line (PRD §4.3): 500×100 default, clamped.
pub fn anchor_box(kw: &Rect, cfg: &CensorConfig, frame_w: i32, frame_h: i32) -> Rect {
    let x = (kw.right() + cfg.gap).clamp(0, frame_w.saturating_sub(1));
    let y = (kw.center_y() - cfg.box_h / 2).clamp(0, (frame_h - cfg.box_h).max(0));
    let w = cfg.box_w.min(frame_w - x);
    let h = cfg.box_h.min(frame_h - y);
    Rect { x, y, w: w.max(0), h: h.max(0) }
}

/// Fill censor rectangles with solid black on a BGRA frame.
pub fn stamp(bgra: &mut [u8], w: u32, h: u32, rects: &[Rect]) {
    for r in rects {
        let x0 = r.x.max(0) as u32;
        let y0 = r.y.max(0) as u32;
        let x1 = (r.x + r.w).max(0).min(w as i32) as u32;
        let y1 = (r.y + r.h).max(0).min(h as i32) as u32;
        for y in y0..y1 {
            let row = y as usize * w as usize * 4;
            for x in x0..x1 {
                let i = row + x as usize * 4;
                bgra[i] = 0;
                bgra[i + 1] = 0;
                bgra[i + 2] = 0;
                bgra[i + 3] = 255;
            }
        }
    }
}

/// One live sensor target: the keyword label it was anchored to.
#[derive(Debug, Clone)]
pub struct Region {
    pub keyword: String,
    /// Last seen label bbox (full-res) — association key.
    pub kw_rect: Rect,
    pub misses: u32,
}

/// Dwell-stabilized active region list (ADR-0015): activate on first hit,
/// refresh in place on nearby re-detection, remove after DWELL_SCANS misses.
#[derive(Debug, Default)]
pub struct RegionTracker {
    regions: Vec<Region>,
}

impl RegionTracker {
    pub fn update(&mut self, hits: Vec<(String, Rect)>) {
        const MARGIN: i32 = 80; // association tolerance around the old label
        let mut hit_idx: Vec<usize> = Vec::new();
        for (kw, rect) in hits {
            let kw = kw.to_lowercase();
            match self.regions.iter().position(|r| {
                r.keyword == kw
                    && rect.center_x().abs_diff(r.kw_rect.center_x())
                        <= (r.kw_rect.w + MARGIN) as u32
                    && rect.center_y().abs_diff(r.kw_rect.center_y())
                        <= (r.kw_rect.h + MARGIN) as u32
            }) {
                Some(i) => {
                    self.regions[i].kw_rect = rect;
                    self.regions[i].misses = 0;
                    hit_idx.push(i);
                }
                None => {
                    hit_idx.push(self.regions.len()); // new region counts as seen
                    self.regions.push(Region { keyword: kw, kw_rect: rect, misses: 0 });
                }
            }
        }
        // Misses only accrue on scans that did NOT re-see the region.
        for (i, r) in self.regions.iter_mut().enumerate() {
            if !hit_idx.contains(&i) {
                r.misses += 1;
            }
        }
        self.regions.retain(|r| r.misses < DWELL_SCANS);
    }

    pub fn active_rects(&self, cfg: &CensorConfig, frame_w: i32, frame_h: i32) -> Vec<Rect> {
        self.regions
            .iter()
            .map(|r| anchor_box(&r.kw_rect, cfg, frame_w, frame_h))
            .filter(|r| r.w > 0 && r.h > 0)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CensorStatus {
    pub active: usize,
    pub frame_w: u32,
    pub frame_h: u32,
    pub rects: Vec<Rect>,
}

// ---- persistence (JSON in the app config dir) ----

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("censor.json"))
}

pub fn load_config(app: &AppHandle) -> CensorConfig {
    let Ok(p) = config_path(app) else {
        return CensorConfig::default();
    };
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(app: &AppHandle, cfg: &CensorConfig) -> Result<(), String> {
    let p = config_path(app)?;
    let mut clean = cfg.clone();
    clean.keywords = clean
        .keywords
        .iter()
        .map(|k| k.trim().to_lowercase())
        .filter(|k| k.len() >= 4)
        .collect();
    clean.keywords.dedup();
    if clean.box_w < 8 || clean.box_h < 8 {
        return Err("box size too small (min 8px)".into());
    }
    if clean.gap < 0 {
        return Err("gap must be >= 0".into());
    }
    let s = serde_json::to_string_pretty(&clean).map_err(|e| e.to_string())?;
    std::fs::write(p, s).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CensorConfig {
        CensorConfig { enabled: true, keywords: vec!["password".into()], box_w: 500, box_h: 100, gap: 5 }
    }

    #[test]
    fn anchor_geometry() {
        // "di samping kata 'password' berjarak 5px dengan sensor 500x100"
        let kw = Rect { x: 10, y: 50, w: 100, h: 20 }; // right=110, cy=60
        let b = anchor_box(&kw, &cfg(), 1920, 1080);
        assert_eq!(b, Rect { x: 115, y: 10, w: 500, h: 100 });
    }

    #[test]
    fn anchor_clamps_at_frame_edge() {
        let kw = Rect { x: 1800, y: 500, w: 100, h: 20 }; // right=1900 on 1920 frame
        let b = anchor_box(&kw, &cfg(), 1920, 1080);
        assert_eq!(b.x, 1905);
        assert_eq!(b.w, 1920 - 1905); // clipped, not off-screen
        assert!(b.x + b.w <= 1920);
    }

    #[test]
    fn anchor_clamps_vertical() {
        let kw = Rect { x: 100, y: 0, w: 50, h: 20 }; // cy=10 → y=-40 → clamp 0
        let b = anchor_box(&kw, &cfg(), 1920, 1080);
        assert_eq!(b.y, 0);
    }

    #[test]
    fn stamp_fills_black() {
        // 4x4 BGRA frame, all white; stamp 2x2 box at origin
        let mut frame = vec![255u8; 4 * 4 * 4];
        stamp(&mut frame, 4, 4, &[Rect { x: 0, y: 0, w: 2, h: 2 }]);
        for y in 0..2 {
            for x in 0..2 {
                let i = (y * 4 + x) * 4;
                assert_eq!(&frame[i..i + 4], &[0, 0, 0, 255]);
            }
        }
        assert_eq!(&frame[2 * 4..2 * 4 + 4], &[255; 4]); // outside untouched (px 2,0)
    }

    fn hit(x: i32) -> Vec<(String, Rect)> {
        vec![("Password".into(), Rect { x, y: 100, w: 120, h: 24 })]
    }

    #[test]
    fn tracker_first_hit_activates_and_dwell_removes() {
        let mut t = RegionTracker::default();
        t.update(hit(10));
        assert_eq!(t.len(), 1); // active immediately, no confirmation wait
        t.update(vec![]); // one scan missed → still alive (dwell 2)
        assert_eq!(t.len(), 1);
        t.update(vec![]); // second miss → removed
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn tracker_refresh_in_place_on_nearby_redetect() {
        let mut t = RegionTracker::default();
        t.update(hit(10));
        t.update(hit(15)); // small label-detection jitter → same region
        assert_eq!(t.len(), 1);
        assert_eq!(t.regions[0].misses, 0);
        t.update(hit(900)); // far away → new region, old one dies by dwell
        assert_eq!(t.len(), 2);
        t.update(hit(902));
        assert_eq!(t.len(), 1); // only the far one refreshed
    }

    #[test]
    fn active_rects_use_anchor_geometry() {
        let mut t = RegionTracker::default();
        t.update(hit(10));
        let rects = t.active_rects(&cfg(), 1920, 1080);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].x, 10 + 120 + 5);
    }
}
