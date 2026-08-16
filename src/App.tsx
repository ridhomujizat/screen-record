import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";

interface SourceInfo {
  id: number;
  kind: string;
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
}

const STATUS_IDLE: RecordStatus = {
  state: "idle",
  durationMs: 0,
  filePath: null,
  error: null,
  framesCaptured: 0,
  framesDropped: 0,
};

function App() {
  const [sources, setSources] = useState<SourceInfo[]>([]);
  const [selected, setSelected] = useState<SourceInfo | null>(null);
  const [status, setStatus] = useState<RecordStatus>(STATUS_IDLE);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  // Subscribe to record-status events
  useEffect(() => {
    let un: UnlistenFn | undefined;
    listen<RecordStatus>("record-status", (e) => setStatus(e.payload)).then((fn) => (un = fn));
    return () => { un?.(); };
  }, []);

  // Preview frames: BGRA raw bytes → canvas
  useEffect(() => {
    let un: UnlistenFn | undefined;
    listen<{ data: number[]; width: number; height: number }>("preview-frame", (e) => {
      const { data, width, height } = e.payload;
      const canvas = canvasRef.current;
      if (!canvas) return;

      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      // Resize canvas if needed
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }

      const img = ctx.createImageData(width, height);
      // BGRA → RGBA for ImageData
      const src = data;
      const dst = img.data;
      for (let i = 0, j = 0; i < src.length; i += 4, j += 4) {
        dst[j] = src[i + 2]; // R = B
        dst[j + 1] = src[i + 1]; // G
        dst[j + 2] = src[i]; // B = R
        dst[j + 3] = 255;
      }
      ctx.putImageData(img, 0, 0);
    }).then((fn) => (un = fn));
    return () => { un?.(); };
  }, []);

  async function loadSources() {
    const s = await invoke<SourceInfo[]>("list_sources");
    setSources(s);
    if (s.length > 0 && !selected) setSelected(s[0]);
  }
  useEffect(() => { loadSources(); }, []);

  async function startRec() {
    if (!selected) return;
    setStatus({ ...STATUS_IDLE, state: "starting" });
    try {
      await invoke("start_record", { targetId: selected.id, kind: selected.kind });
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

  return (
    <main className="container">
      <h1>🎥 Screen Record</h1>

      <div className="row">
        <select
          value={selected ? `${selected.kind}:${selected.id}` : ""}
          onChange={(e) => {
            const [kind, idStr] = e.target.value.split(":");
            const s = sources.find((x) => x.kind === kind && x.id === Number(idStr));
            if (s) setSelected(s);
          }}
          disabled={recording}
        >
          {sources.map((s) => (
            <option key={`${s.kind}:${s.id}`} value={`${s.kind}:${s.id}`}>
              {s.label} ({s.width}x{s.height})
            </option>
          ))}
        </select>

        {recording ? (
          <button onClick={stopRec}>⏹ Stop</button>
        ) : (
          <button onClick={startRec} disabled={!selected || status.state === "starting"}>
            {status.state === "starting" ? "Starting..." : "⏺ Record"}
          </button>
        )}
      </div>

      {recording && (
        <div className="preview">
          <canvas
            ref={canvasRef}
            style={{ width: "100%", maxWidth: 640 }}
          />
          <p className="status">🔴 Recording…</p>
        </div>
      )}

      {status.error && <p className="error">Error: {status.error}</p>}
      {status.state === "idle" && status.framesCaptured > 0 && (
        <p className="status">
          Done. {status.framesCaptured} frames captured.
        </p>
      )}
    </main>
  );
}

export default App;
