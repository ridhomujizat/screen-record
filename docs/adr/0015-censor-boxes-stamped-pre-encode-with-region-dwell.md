# 0015. Stamp censor boxes on frames pre-disk-write, anchored to keyword labels with dwell

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0013](0013-defer-audio-mixing-to-ffmpeg-at-finish.md), [0014](0014-sensitive-data-detection-paddleocr-mobile-latn-onnx.md)
- **PRD section**: PD-0003 §4.3, §5

## Context

The pipeline today is: WGC frame → sync pump → raw BGRA appended to
`video.raw` (+ per-source WAVs) → ffmpeg encodes MP4 at finish (ADR-0013).
Censoring has three possible insertion points: (a) UI overlay only, (b) the
sync pump before the disk write, (c) post-processing the finished file. It
also has a geometry question — where exactly the box sits relative to the
detected keyword — and a stability question: OCR scans run at 2 fps while
frames are written at 30 fps, and single-scan misses would flicker the box.

The security property is absolute: **the secret must never reach persistent
storage**, not even a temp file that gets deleted.

## Decision

The sync pump stamps **solid black rectangles** on the BGRA frame **before
appending it to `video.raw`** (and before anything else consumes the frame
for output). Box geometry is anchored to the matched keyword bbox `kw` in
full-resolution coordinates:

```
GAP = 5px, BOX_W = 500px, BOX_H = 100px   (defaults; user-adjustable)
box.x = kw.right + GAP
box.y = kw.center_y − BOX_H/2
box.w = BOX_W, box.h = BOX_H, clamped to frame bounds
```

An active-region list (keyword, box, last-seen scan id) lives behind
`Arc<RwLock<Vec<Region>>>`: a region activates on **first** detection
(no confirmation delay — first frame after the scan is already censored) and
is removed only after **2 consecutive scans** fail to re-see it (~1 s dwell).
Re-detections whose keyword matches and whose center falls inside the old
region refresh it in place instead of creating a new one. Every written frame
reads the current list and fills the rectangles; the branch is skipped
entirely when censoring is disabled (zero regression on the existing path).
Solid black over blur/pixelation is deliberate: blur is reversible and
pixelated small text can be re-OCR'd.

## Consequences

**Positive**
- Secrets never touch disk: `video.raw`, the MP4, and every temp artifact are
  clean by construction — no post-hoc redaction, no "shred the temp file" edge
  cases, works unchanged with finish-time ffmpeg (ADR-0013).
- First-detection activation means the only leak window is the scan interval
  itself (≤ ~0.5–1 s) and only when a labeled dialog appears mid-record —
  typing into a field requires the label to be visible first, so the box is
  normally already active before the first keystroke lands.
- Dwell + refresh absorbs OCR jitter and single-scan misses; no flicker.
- When disabled: same code path as today, byte-identical output.

**Negative / tradeoffs**
- The raw dump is no longer a pixel-perfect copy of the screen — acceptable:
  the recording is the product, not a forensic mirror.
- Region updates lag fast UI changes by up to one dwell (~1 s), so a stale
  box can cover the wrong pixels briefly; bounded and self-healing.
- Right-side-only anchoring clips at the right frame edge (clamped) — flip-
  to-left is deferred until real layouts demand it.

**Neutral**
- The keyword label text itself stays visible (only the neighbor area is
  boxed) — labels are not secret; the typed value is.
- Preview shows the same boxes from the same list, so what the user sees in
  the UI is what lands in the file.

## Alternatives considered

- **UI-only overlay** — zero pipeline change, but the file records the secret;
  rejected outright (defeats the purpose).
- **Post-process the finished MP4** — no runtime cost, but secrets sit on
  disk during and after recording until redaction runs; rejected on the
  persistence guarantee.
- **ffmpeg `drawbox` fed live via sidecar filter script** — equivalent
  visual result, but `video.raw` still contains the secret pre-encode and the
  plumbing is more moving parts than an in-process fill.
- **Keyword-anchored box vs. tracking the caret/input element** — element
  tracking needs UI-automation APIs (per-platform, fragile, permissioned);
  label anchoring is one arithmetic expression.
