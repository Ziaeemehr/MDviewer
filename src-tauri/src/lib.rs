use std::path::{Path, PathBuf};
use std::sync::Mutex;

use std::io::{Read, Write};

#[cfg(desktop)]
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, FilePath};
use tauri_plugin_fs::{FsExt, OpenOptions};

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

/// Name to show for a file picked in a dialog. On Android the dialog returns a `content://`
/// URI whose last segment is an opaque document id, so the name is taken from the open file
/// itself when the system exposes it, and from the URI otherwise.
fn picked_name(file: &FilePath, _opened: &std::fs::File) -> String {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        let link = std::fs::read_link(format!("/proc/self/fd/{}", _opened.as_raw_fd()));
        if let Some(name) = link.ok().as_deref().and_then(Path::file_name) {
            return name.to_string_lossy().into_owned();
        }
    }
    match file {
        FilePath::Path(p) => p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        // e.g. `primary%3ADownload%2Fnotes.md`
        FilePath::Url(u) => {
            let last = u.path_segments().and_then(|s| s.last()).unwrap_or("");
            let id = percent_decode(last);
            let name = id.rsplit(['/', ':']).next().unwrap_or("");
            if name.contains('.') { name.to_string() } else { "document.md".to_string() }
        }
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read a file returned by the dialog: a path on desktop, a `content://` URI on Android.
fn read_picked(app: &tauri::AppHandle, file: FilePath) -> Result<OpenedFile, String> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    let mut contents = String::new();
    let opened = app
        .fs()
        .open(file.clone(), opts)
        .and_then(|mut f| f.read_to_string(&mut contents).map(|_| f))
        .map_err(|e| format!("{}: {}", file, e))?;
    Ok(OpenedFile {
        name: picked_name(&file, &opened),
        path: file.to_string(),
        contents,
    })
}

/// Files the system asked the app to open (double click, "Open With", command line),
/// kept until the window takes them with `pending_files`.
#[derive(Default)]
struct Pending(Mutex<Vec<PathBuf>>);

#[cfg(desktop)]
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
    let mut dialog = app.dialog().file();
    // Android filters by MIME type, and many file managers report .md files as
    // application/octet-stream, so a filter there would hide them.
    #[cfg(desktop)]
    {
        dialog = dialog.add_filter("Markdown", &["md", "markdown", "mdown", "txt"]);
    }
    if let Some(dir) = folder_of(&path) {
        dialog = dialog.set_directory(dir);
    }

    dialog
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .map(|picked| read_picked(&app, picked))
        .collect()
}

/// Extensions offered by the save dialog. Android takes a single MIME type instead, used as
/// the type of the new document.
#[cfg(desktop)]
const MARKDOWN_SAVE_TYPES: &[&str] = &["md", "markdown", "txt"];
#[cfg(mobile)]
const MARKDOWN_SAVE_TYPES: &[&str] = &["text/markdown"];

#[derive(serde::Serialize)]
struct SavedFile {
    path: String,
    name: String,
}

/// Ask where to save, then write the file. Returns where it was saved, or None if the dialog
/// was cancelled. `path` is where the file was opened from; the dialog starts in that folder.
/// The webview cannot download blobs reliably on every platform, so saving goes through here.
#[tauri::command]
async fn save_markdown(
    app: tauri::AppHandle,
    name: String,
    path: Option<String>,
    contents: String,
) -> Result<Option<SavedFile>, String> {
    let mut dialog = app
        .dialog()
        .file()
        .set_file_name(name)
        .add_filter("Markdown", MARKDOWN_SAVE_TYPES);
    if let Some(dir) = folder_of(&path) {
        dialog = dialog.set_directory(dir);
    }

    match dialog.blocking_save_file() {
        Some(file) => {
            let mut opts = OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            let saved = app
                .fs()
                .open(file.clone(), opts)
                .and_then(|mut f| f.write_all(contents.as_bytes()).map(|_| f))
                .map_err(|e| format!("{}: {}", file, e))?;
            Ok(Some(SavedFile {
                name: picked_name(&file, &saved),
                path: file.to_string(),
            }))
        }
        None => Ok(None),
    }
}

/// Files named on a command line, as canonical paths so a file already open in a tab is
/// recognised however it was named. `cwd` is where that command ran.
#[cfg(desktop)]
fn file_args(args: impl IntoIterator<Item = PathBuf>, cwd: &Path) -> Vec<PathBuf> {
    args.into_iter()
        .filter_map(|p| std::fs::canonicalize(cwd.join(p)).ok())
        .filter(|p| p.is_file())
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // Must come first: a second launch hands its files to the running window and exits.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
        let args = argv.into_iter().skip(1).map(PathBuf::from);
        queue_files(app, file_args(args, Path::new(&cwd)));
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }));
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(Pending::default())
        .invoke_handler(tauri::generate_handler![
            open_markdown,
            save_markdown,
            pending_files
        ])
        .setup(|_app| {
            // Windows and Linux pass the double-clicked file as an argument.
            #[cfg(desktop)]
            {
                let args = std::env::args_os().skip(1).map(PathBuf::from);
                let cwd = std::env::current_dir().unwrap_or_default();
                queue_files(_app.handle(), file_args(args, &cwd));
            }
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
