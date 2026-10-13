#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri_plugin_dialog::DialogExt;

/// Ask where to save, then write the file. Returns false if the dialog was cancelled.
/// The webview cannot download blobs reliably on every platform, so saving goes through here.
#[tauri::command]
async fn save_markdown(
    app: tauri::AppHandle,
    name: String,
    contents: String,
) -> Result<bool, String> {
    let picked = app
        .dialog()
        .file()
        .set_file_name(name)
        .add_filter("Markdown", &["md", "markdown", "txt"])
        .blocking_save_file();

    match picked {
        Some(file) => {
            let path = file.into_path().map_err(|e| e.to_string())?;
            std::fs::write(path, contents).map_err(|e| e.to_string())?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![save_markdown])
        .run(tauri::generate_context!())
        .expect("error while running MD Viewer");
}
