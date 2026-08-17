// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use tauri::{AppHandle, Manager, State};

pub mod capture;

use capture::{Recorder, platform};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub id: u64,
    pub kind: String, // "display" | "window"
    pub label: String,
    pub width: u32,
    pub height: u32,
}

#[tauri::command]
async fn list_sources() -> Result<Vec<SourceInfo>, String> {
    let targets = platform::list_targets();
    Ok(targets
        .into_iter()
        .map(|(target, label, w, h)| {
            let (id, kind) = match target {
                platform::CaptureTarget::Display(id) => (id, "display".to_string()),
                platform::CaptureTarget::Window(id) => (id, "window".to_string()),
                platform::CaptureTarget::Area { display, .. } => {
                    (display, "area".to_string())
                }
            };
            SourceInfo {
                id,
                kind,
                label,
                width: w,
                height: h,
            }
        })
        .collect())
}

#[tauri::command]
async fn start_record(
    app: AppHandle,
    target_id: u64,
    kind: String,
    bounds: Option<(u32, u32, u32, u32)>,
) -> Result<(), String> {
    let target = match kind.as_str() {
        "display" => platform::CaptureTarget::Display(target_id),
        "window" => platform::CaptureTarget::Window(target_id),
        "area" => match bounds {
            Some((l, t, r, b)) => platform::CaptureTarget::Area {
                display: target_id,
                left: l,
                top: t,
                right: r,
                bottom: b,
            },
            None => return Err("area capture requires bounds".into()),
        },
        _ => return Err("unknown target kind".into()),
    };

    // Disk space guard (M6): refuse to start if < 1 GB free.
    if let Some(free_gb) = free_disk_gb() {
        if free_gb < 1.0 {
            return Err(format!(
                "Not enough disk space: {free_gb:.1} GB free (need ≥ 1 GB)"
            ));
        }
    }

    let recorder: State<'_, Recorder> = app.state::<Recorder>();
    recorder.start(app.clone(), target).await
}

fn free_disk_gb() -> Option<f64> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let wide: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                windows::core::PCWSTR(wide.as_ptr()),
                Some(&mut free),
                None,
                None,
            )
        };
        ok.is_ok().then(|| free as f64 / 1e9)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[tauri::command]
async fn stop_record(app: AppHandle) -> Result<String, String> {
    let recorder: State<'_, Recorder> = app.state::<Recorder>();
    recorder.stop(app.clone()).await
}

#[tauri::command]
async fn is_recording(app: AppHandle) -> Result<bool, String> {
    let recorder: State<'_, Recorder> = app.state::<Recorder>();
    Ok(recorder.is_recording().await)
}

#[tauri::command]
async fn open_folder(path: String) -> Result<(), String> {
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Recorder::new())
        .invoke_handler(tauri::generate_handler![
            list_sources,
            start_record,
            stop_record,
            is_recording,
            open_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
