import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";

interface SourceInfo {
  id: number;
  kind: string; // "display" | "window"
  label: string;
  width: number;
  height: number;
}

interface RecordStatus {
  state: string;
  durationMs: number;
  filePath: string | null;
  error: string | null;
  framesCaptured: number;
  framesDropped: number;
  audioFrames: number;
  syncOffsetMs: number;
}

const STATUS_IDLE: RecordStatus = {
  state: "idle",
  durationMs: 0,
  filePath: null,
  error: null,
  framesCaptured: 0,
  framesDropped: 0,
  audioFrames: 0,
  syncOffsetMs: 0,
};

function fmtDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

export default function App() {
  const [sources, setSources] = useState<SourceInfo[]>([]);
  const [selected, setSelected] = useState<SourceInfo | null>(null);
  const [status, setStatus] = useState<RecordStatus>(STATUS_IDLE);
  const [elapsed, setElapsed] = useState(0);
  const [areaMode, setAreaMode] = useState(false);
  const [areaX, setAreaX] = useState("0");
  const [areaY, setAreaY] = useState("0");
  const [areaW, setAreaW] = useState("");
  const [areaH, setAreaH] = useState("");
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    let un: UnlistenFn | undefined;
    listen<RecordStatus>("record-status", (e) => {
      setStatus(e.payload);
      if (e.payload.state !== "recording" && timerRef.current) {
        window.clearInterval(timerRef.current);
        timerRef.current = null;
      }
    }).then((fn) => (un = fn));
    return () => { un?.(); };
  }, []);

  useEffect(() => {
    let un: UnlistenFn | undefined;
    listen<{ data: number[]; width: number; height: number }>("preview-frame", (e) => {
      const { data, width, height } = e.payload;
      const canvas = canvasRef.current;
      if (!canvas) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
      const img = ctx.createImageData(width, height);
      const src = data;
      const dst = img.data;
      for (let i = 0, j = 0; i < src.length; i += 4, j += 4) {
        dst[j] = src[i + 2];
        dst[j + 1] = src[i + 1];
        dst[j + 2] = src[i];
        dst[j + 3] = 255;
      }
      ctx.putImageData(img, 0, 0);
    }).then((fn) => (un = fn));
    return () => { un?.(); };
  }, []);

  async function loadSources() {
    const s = await invoke<SourceInfo[]>("list_sources");
    setSources(s);
    if (!selected && s.length > 0) setSelected(s[0]);
  }
  useEffect(() => { loadSources(); }, []);

  async function startRec() {
    if (!selected) return;
    setStatus({ ...STATUS_IDLE, state: "starting" });
    setElapsed(0);
    try {
      // area mode: pass crop bounds (physical px within the display)
      let bounds: [number, number, number, number] | undefined;
      let kind = selected.kind;
      if (areaMode && selected.kind === "display") {
        const x = Number(areaX), y = Number(areaY), w = Number(areaW), h = Number(areaH);
        if (!(w > 0 && h > 0)) throw new Error("Area size must be > 0");
        bounds = [x, y, x + w, y + h];
        kind = "area";
      }
      await invoke("start_record", { targetId: selected.id, kind, bounds });
      timerRef.current = window.setInterval(() => setElapsed((e) => e + 1000), 1000);
    } catch (e) {
      setStatus({ ...STATUS_IDLE, state: "error", error: String(e) });
    }
  }

  async function stopRec() {
    try {
      await invoke("stop_record");
    } catch (e) {
      setStatus({ ...STATUS_IDLE, state: "error", error: String(e) });
    }
  }

  const recording = status.state === "recording";
  const displays = sources.filter((s) => s.kind === "display");
  const windows = sources.filter((s) => s.kind === "window");
  const done = status.state === "finished";

  return (
    <div className="app">
      <div className="card">
        {/* Header */}
        <header className="card-header">
          <div className="brand">
            <span className="brand-dot" />
            <span className="brand-name">Screen Record</span>
          </div>
          <span className={`pill ${recording ? "pill-rec" : "pill-idle"}`}>
            {recording ? "● Recording" : "● Idle"}
          </span>
        </header>

        {/* Target selector */}
        <section className="section">
          <label className="label">Capture target</label>
          <div className="target-row">
            <div className="select-wrap">
              <select
                value={selected ? `${selected.kind}:${selected.id}` : ""}
                onChange={(e) => {
                  const [kind, idStr] = e.target.value.split(":");
                  const s = sources.find((x) => x.kind === kind && x.id === Number(idStr));
                  if (s) setSelected(s);
                }}
                disabled={recording}
              >
                <optgroup label="Displays">
                  {displays.map((s) => (
                    <option key={`d:${s.id}`} value={`display:${s.id}`}>{s.label}</option>
                  ))}
                </optgroup>
                <optgroup label="Windows">
                  {windows.map((s) => (
                    <option key={`w:${s.id}`} value={`window:${s.id}`}>{s.label}</option>
                  ))}
                </optgroup>
              </select>
            </div>
            <button
              className="btn btn-ghost"
              onClick={loadSources}
              disabled={recording}
              title="Refresh sources"
            >
              ↻
            </button>
          </div>
          {selected && !recording && (
            <p className="meta">
              {selected.width}×{selected.height}
            </p>
          )}

          {selected?.kind === "display" && !recording && (
            <div className="area-row">
              <label className="area-toggle">
                <input
                  type="checkbox"
                  checked={areaMode}
                  onChange={(e) => setAreaMode(e.target.checked)}
                />
                Record area (crop)
              </label>
              {areaMode && (
                <div className="area-inputs">
                  <input placeholder="x" value={areaX} onChange={(e) => setAreaX(e.target.value)} />
                  <input placeholder="y" value={areaY} onChange={(e) => setAreaY(e.target.value)} />
                  <input placeholder="w" value={areaW} onChange={(e) => setAreaW(e.target.value)} />
                  <input placeholder="h" value={areaH} onChange={(e) => setAreaH(e.target.value)} />
                </div>
              )}
            </div>
          )}
        </section>

        {/* Preview / recording state */}
        <section className="section">
          <div className={`preview ${recording ? "preview-live" : "preview-idle"}`}>
            {recording ? (
              <>
                <canvas ref={canvasRef} />
                <div className="rec-bar">
                  <span className="rec-dot" />
                  <span className="rec-time">{fmtDuration(elapsed || status.durationMs)}</span>
                </div>
              </>
            ) : (
              <div className="placeholder">
                <div className="placeholder-icon">◻</div>
                <p>Select a target and press Record</p>
              </div>
            )}
          </div>
        </section>

        {/* Actions */}
        <section className="section actions">
          {recording ? (
            <button className="btn btn-stop" onClick={stopRec}>
              <span className="btn-icon">■</span> Stop
            </button>
          ) : (
            <button
              className="btn btn-record"
              onClick={startRec}
              disabled={!selected || status.state === "starting"}
            >
              <span className="btn-icon">●</span>
              {status.state === "starting" ? "Starting…" : "Start Recording"}
            </button>
          )}
        </section>

        {/* Error */}
        {status.error && <p className="error">{status.error}</p>}

        {/* Result */}
        {done && status.filePath && (
          <section className="result">
            <div className="result-head">
              <span className="result-check">✓</span>
              <span>Recording saved</span>
            </div>
            <code className="result-path">{status.filePath}</code>
            <div className="result-stats">
              <span>{status.framesCaptured} frames</span>
              <span>·</span>
              <span>sync {status.syncOffsetMs}ms</span>
            </div>
            <button className="btn btn-ghost btn-block" onClick={() => invoke("open_folder", { path: status.filePath! })}>
              Open Folder
            </button>
          </section>
        )}
      </div>
    </div>
  );
}
