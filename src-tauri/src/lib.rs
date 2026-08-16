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
async fn start_record(app: AppHandle, target_id: u64, kind: String) -> Result<(), String> {
    let target = match kind.as_str() {
        "display" => platform::CaptureTarget::Display(target_id),
        "window" => platform::CaptureTarget::Window(target_id),
        _ => return Err("unknown target kind".into()),
    };
    let recorder: State<'_, Recorder> = app.state::<Recorder>();
    recorder.start(app.clone(), target).await
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Recorder::new())
        .invoke_handler(tauri::generate_handler![
            list_sources,
            start_record,
            stop_record,
            is_recording
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
