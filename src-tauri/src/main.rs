#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::{Emitter, Manager};
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

fn read_file(path: &Path) -> Result<OpenedFile, String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    Ok(OpenedFile {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_string_lossy().into_owned(),
        contents,
    })
}

/// Files the system asked the app to open (double click, "Open With", command line),
/// kept until the window takes them with `pending_files`.
#[derive(Default)]
struct Pending(Mutex<Vec<PathBuf>>);

fn queue_files(app: &tauri::AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    app.state::<Pending>().0.lock().unwrap().extend(paths);
    // The window may not be listening yet; it also asks once when it starts.
    let _ = app.emit("files-pending", ());
}

/// Hand over the files queued by the system, emptying the queue.
#[tauri::command]
fn pending_files(pending: tauri::State<Pending>) -> Result<Vec<OpenedFile>, String> {
    let paths = std::mem::take(&mut *pending.0.lock().unwrap());
    paths.iter().map(|p| read_file(p)).collect()
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
        files.push(read_file(&path)?);
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

/// Files named on a command line, as canonical paths so a file already open in a tab is
/// recognised however it was named. `cwd` is where that command ran.
fn file_args(args: impl IntoIterator<Item = PathBuf>, cwd: &Path) -> Vec<PathBuf> {
    args.into_iter()
        .filter_map(|p| std::fs::canonicalize(cwd.join(p)).ok())
        .filter(|p| p.is_file())
        .collect()
}

fn main() {
    tauri::Builder::default()
        // Must come first: a second launch hands its files to the running window and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let args = argv.into_iter().skip(1).map(PathBuf::from);
            queue_files(app, file_args(args, Path::new(&cwd)));
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(Pending::default())
        .invoke_handler(tauri::generate_handler![
            open_markdown,
            save_markdown,
            pending_files
        ])
        .setup(|app| {
            // Windows and Linux pass the double-clicked file as an argument.
            let args = std::env::args_os().skip(1).map(PathBuf::from);
            let cwd = std::env::current_dir().unwrap_or_default();
            queue_files(app.handle(), file_args(args, &cwd));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running MD Viewer")
        .run(|_app, _event| {
            // macOS sends the file as an event, also while the app is already running.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                let paths = urls.iter().filter_map(|u| u.to_file_path().ok());
                queue_files(_app, paths.collect());
            }
        });
}
