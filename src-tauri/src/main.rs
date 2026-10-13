#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use tauri_plugin_dialog::DialogExt;

/// Folder of a file the app already knows, used as the starting point of a dialog.
fn folder_of(path: &Option<String>) -> Option<PathBuf> {
    let dir = Path::new(path.as_deref()?).parent()?;
    dir.is_dir().then(|| dir.to_path_buf())
}

#[derive(serde::Serialize)]
struct OpenedFile {
    path: String,
    name: String,
    contents: String,
}

/// Let the user pick one or more files and return their contents with their paths.
/// `path` is the file in the current tab; the dialog starts in its folder.
#[tauri::command]
async fn open_markdown(
    app: tauri::AppHandle,
    path: Option<String>,
) -> Result<Vec<OpenedFile>, String> {
    let mut dialog = app
        .dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown", "mdown", "txt"]);
    if let Some(dir) = folder_of(&path) {
        dialog = dialog.set_directory(dir);
    }

    let mut files = Vec::new();
    for picked in dialog.blocking_pick_files().unwrap_or_default() {
        let path = picked.into_path().map_err(|e| e.to_string())?;
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {}", path.display(), e))?;
        files.push(OpenedFile {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.to_string_lossy().into_owned(),
            contents,
        });
    }
    Ok(files)
}

/// Ask where to save, then write the file. Returns the saved path, or None if the dialog
/// was cancelled. `path` is where the file was opened from; the dialog starts in that folder.
/// The webview cannot download blobs reliably on every platform, so saving goes through here.
#[tauri::command]
async fn save_markdown(
    app: tauri::AppHandle,
    name: String,
    path: Option<String>,
    contents: String,
) -> Result<Option<String>, String> {
    let mut dialog = app
        .dialog()
        .file()
        .set_file_name(name)
        .add_filter("Markdown", &["md", "markdown", "txt"]);
    if let Some(dir) = folder_of(&path) {
        dialog = dialog.set_directory(dir);
    }

    match dialog.blocking_save_file() {
        Some(file) => {
            let path = file.into_path().map_err(|e| e.to_string())?;
            std::fs::write(&path, contents).map_err(|e| e.to_string())?;
            Ok(Some(path.to_string_lossy().into_owned()))
        }
        None => Ok(None),
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![open_markdown, save_markdown])
        .run(tauri::generate_context!())
        .expect("error while running MD Viewer");
}
