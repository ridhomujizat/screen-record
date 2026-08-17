# 0014. Detect sensitive-data labels with PaddleOCR mobile latin models via ONNX Runtime

- **Status**: Accepted
- **Date**: 2026-08-17
- **Deciders**: Omnix architecture
- **Related ADRs**: [0002](0002-platform-agnostic-capture-core.md), [0009](0009-flat-minimal-module-layout.md), [0015](0015-censor-boxes-stamped-pre-encode-with-region-dwell.md)
- **PRD section**: PD-0003 §4.2

## Context

PD-0003 needs real-time text detection on recorded frames to find sensitive
*labels* ("Password", "API Key") so a censor box can cover the adjacent input
field. Constraints:

- Must run inside the Rust process on the existing frame pump — no sidecar,
  no Python runtime shipped to users.
- Light on CPU: it shares the machine with WGC capture, audio callbacks and
  frame writing; scans run at 2 fps on ≤1280 px frames.
- Latin text only for v1, mostly UI chrome (small antialiased fonts, high
  contrast, upright).
- Must stay platform-agnostic (macOS path per ADR-0002/0011).
- The user story explicitly names **PaddleOCR lightweight latin** as the
  detection engine.

Available options: WinRT `Windows.Media.Ocr`, tesseract bindings, a Python
PaddleOCR sidecar, or PaddleOCR *models* executed through ONNX Runtime.

## Decision

We run **PaddleOCR PP-OCRv4 mobile models in ONNX format** (DB text-detection
mobile + latin CTC recognition mobile, ~15 MB total, pinned versions bundled
in the app) through the **`ort` crate** (ONNX Runtime, CPU execution provider)
inside a dedicated worker thread in `capture/censor/ocr.rs`. Pre-processing
(downscale, normalize) and post-processing (DB binarize/polygon extraction,
CTC decode against the bundled charset) are plain Rust. The worker pulls the
latest broadcast frame into a single-slot cell (skip when busy), scans every
500 ms, and publishes matched keyword + bbox only — recognized full text is
never logged or emitted.

## Consequences

**Positive**
- Models are the industry-benchmark lightweight pair (~4.7 MB det + ~6 MB
  rec), accurate on UI/scene text at small sizes where tesseract struggles.
- `ort` is a pure-binding crate with prebuilt runtimes — no C toolchain per
  user, no Python, works on Windows and macOS identically.
- Swapping to a GPU execution provider (DirectML/CoreML) later is a config
  change, not a pipeline change.
- OCR never blocks capture: single-slot latest-frame + skip-if-busy, results
  flow one-way into the region list (ADR-0015).

**Negative / tradeoffs**
- DB post-processing (binarize + connected components + polygon simplify) and
  CTC decode are hand-written (~300–400 lines) — the fiddliest part of the
  feature; mitigated by porting the reference PaddleOCR/RapidOCR algorithm
  and unit tests against golden outputs.
- Two model files to pin and bundle; version drift in PaddleOCR releases is
  handled by pinning, not tracking.
- CPU cost is real (~≤10% of one core at 2 fps/1280 px) — accepted, gated
  behind user opt-in.

**Neutral**
- Recognition output stays in-process memory only; the privacy surface is
  the keyword list in settings (not secret) and box geometry.

## Alternatives considered

- **Windows.Media.Ocr (WinRT)** — zero new dependencies and word-level boxes;
  rejected: Windows-only (breaks the macOS trait path), latency/quality not
  under our control, and the story names PaddleOCR.
- **tesseract-rs / leptonica** — heavy C dependency, weak on small
  antialiased UI fonts, line boxes coarse; rejected on accuracy for the exact
  case we need.
- **Python PaddleOCR sidecar** — full fidelity upstream, but ships a Python
  runtime (~1 GB), IPC boundary, packaging pain; rejected for a desktop app.
- **RapidOCR (same models, Python)** — same Python objection; we implement
  the equivalent of its pipeline in Rust instead.
