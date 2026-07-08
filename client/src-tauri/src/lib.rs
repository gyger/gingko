// Gingko Writer Desktop — Tauri 2 backend.
// Port of the Electron main process (src/electron/main.js).

mod legacy;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tauri::menu::{IsMenuItem, Menu, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry};
use tauri_plugin_dialog::DialogExt;

/* ==== State ==== */

struct DocState {
    file_path: PathBuf,
    is_untitled: bool,
    // Initial file contents, consumed by get_doc_state on renderer boot.
    file_data: Option<String>,
    undo_data: HashMap<String, Value>,
    // sha1 of the last content this app wrote (or loaded); used to tell
    // external file modifications apart from our own saves.
    last_saved_hash: Option<[u8; 20]>,
    // Keeps the file watcher alive for the lifetime of the window.
    watcher: Option<notify::RecommendedWatcher>,
}

#[derive(Default)]
struct MenuContext {
    has_doc: bool,
    is_untitled: bool,
    is_edit_mode: bool,
}

#[derive(Default)]
struct AppState {
    docs: Mutex<HashMap<String, DocState>>,
    doc_counter: Mutex<u32>,
    menu_ctx: Mutex<MenuContext>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RecentDoc {
    name: String,
    path: String,
    birthtime_ms: f64,
    atime_ms: f64,
    mtime_ms: f64,
}

/* ==== Helpers ==== */

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

fn settings_path(app: &AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_config_dir()
        .expect("no app config dir");
    let _ = fs::create_dir_all(&dir);
    dir.join("settings.json")
}

fn load_settings(app: &AppHandle) -> Value {
    fs::read_to_string(settings_path(app))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_settings(app: &AppHandle, settings: &Value) {
    if let Ok(s) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(settings_path(app), s);
    }
}

fn get_recent_documents(app: &AppHandle) -> Vec<RecentDoc> {
    load_settings(app)
        .get("recentDocuments")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn set_recent_documents(app: &AppHandle, docs: &[RecentDoc]) {
    let mut settings = load_settings(app);
    settings["recentDocuments"] = serde_json::to_value(docs).unwrap_or(json!([]));
    save_settings(app, &settings);
}

fn add_to_recent_documents(app: &AppHandle, file_path: &Path) {
    let meta = fs::metadata(file_path);
    let (birthtime_ms, mtime_ms) = match meta {
        Ok(m) => (
            m.created()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as f64)
                .unwrap_or_else(now_ms),
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as f64)
                .unwrap_or_else(now_ms),
        ),
        Err(_) => (now_ms(), now_ms()),
    };
    let name = file_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let path_str = file_path.to_string_lossy().to_string();
    let entry = RecentDoc {
        name,
        path: path_str.clone(),
        birthtime_ms,
        atime_ms: now_ms(),
        mtime_ms,
    };
    let mut docs: Vec<RecentDoc> = get_recent_documents(app)
        .into_iter()
        .filter(|rd| rd.path != path_str)
        .collect();
    docs.push(entry);
    set_recent_documents(app, &docs);
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

fn date_hash_strings() -> (String, String) {
    let ms = now_ms() as u64;
    let mut hasher = Sha1::new();
    hasher.update(ms.to_string().as_bytes());
    let hash = hex::encode(hasher.finalize());
    // ISO date (yyyy-mm-dd) from days since epoch, no chrono dependency.
    let days = ms / 86_400_000;
    let (y, m, d) = civil_from_days(days as i64);
    (format!("{:04}-{:02}-{:02}", y, m, d), hash[0..6].to_string())
}

// Howard Hinnant's algorithm: days since 1970-01-01 -> (y, m, d)
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn swp_path(file_path: &Path) -> PathBuf {
    let mut p = file_path.as_os_str().to_owned();
    p.push(".swp");
    PathBuf::from(p)
}

// Equivalent of filenamifyPath(filePath, { replacement: '%' })
fn namify_path(file_path: &Path) -> String {
    file_path
        .to_string_lossy()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '%',
            c => c,
        })
        .collect()
}

fn undo_store_path(app: &AppHandle, file_path: &Path) -> PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .expect("no app data dir")
        .join("versionhistory");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{}.json", namify_path(file_path)))
}

fn load_undo_data(app: &AppHandle, file_path: &Path) -> HashMap<String, Value> {
    fs::read_to_string(undo_store_path(app, file_path))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn persist_undo_data(app: &AppHandle, file_path: &Path, data: &HashMap<String, Value>) {
    if let Ok(s) = serde_json::to_string(data) {
        let _ = fs::write(undo_store_path(app, file_path), s);
    }
}

fn sha1_of(data: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finalize().into()
}

// Watch the document's file for external modifications (e.g. an agent or
// editor writing the .gkw on disk). Our own saves are filtered out by
// comparing the content hash against last_saved_hash. On a real external
// change, the new content is pushed to the window as a 'file-changed' event.
fn start_watching(app: &AppHandle, label: &str) {
    use notify::{EventKind, RecursiveMode, Watcher};

    let (path, parent) = {
        let state: State<AppState> = app.state();
        let docs = state.docs.lock().unwrap();
        let Some(doc) = docs.get(label) else { return };
        let path = doc.file_path.clone();
        let Some(parent) = path.parent().map(PathBuf::from) else { return };
        (path, parent)
    };

    let app2 = app.clone();
    let label2 = label.to_string();
    let watch_path = path.clone();
    let watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return;
        }
        if !event.paths.iter().any(|p| p == &watch_path) {
            return;
        }
        // Let in-progress writes settle before reading.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let Ok(bytes) = fs::read(&watch_path) else { return };
        let new_hash = sha1_of(&bytes);
        {
            let state: State<AppState> = app2.state();
            let mut docs = state.docs.lock().unwrap();
            let Some(doc) = docs.get_mut(&label2) else { return };
            if doc.file_path != watch_path || doc.last_saved_hash == Some(new_hash) {
                return;
            }
            doc.last_saved_hash = Some(new_hash);
        }
        if let Ok(content) = String::from_utf8(bytes) {
            if let Some(win) = app2.get_webview_window(&label2) {
                let _ = win.emit("file-changed", content);
            }
        }
    });

    if let Ok(mut w) = watcher {
        if w.watch(&parent, RecursiveMode::NonRecursive).is_ok() {
            let state: State<AppState> = app.state();
            let mut docs = state.docs.lock().unwrap();
            if let Some(doc) = docs.get_mut(label) {
                doc.watcher = Some(w);
            }
        }
    }
}

fn title_text(file_path: &Path) -> String {
    format!(
        "{} - Gingko Writer",
        file_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    )
}

/* ==== Menu ==== */

fn build_menu(app: &AppHandle, ctx: &MenuContext) -> tauri::Result<Menu<Wry>> {
    let recent_docs = get_recent_documents(app);

    let mut file_menu = SubmenuBuilder::new(app, "&File")
        .item(
            &MenuItemBuilder::with_id("menu:new", "&New File")
                .accelerator("CmdOrCtrl+N")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id("menu:open", "&Open...")
                .accelerator("CmdOrCtrl+O")
                .build(app)?,
        );

    {
        let mut recent_menu = SubmenuBuilder::new(app, "Open &Recent");
        let mut items: Vec<tauri::menu::MenuItem<Wry>> = vec![];
        for (idx, rd) in recent_docs.iter().enumerate() {
            items.push(
                MenuItemBuilder::with_id(
                    format!("recent:{}", rd.path),
                    format!("&{}.  {}", idx + 1, rd.name),
                )
                .build(app)?,
            );
        }
        for item in items.iter() {
            recent_menu = recent_menu.item(item);
        }
        file_menu = file_menu.item(&recent_menu.build()?);
    }

    if ctx.has_doc {
        file_menu = file_menu
            .item(&MenuItemBuilder::with_id("menu:close", "&Close").build(app)?)
            .separator()
            .item(
                &MenuItemBuilder::with_id(
                    "menu:save",
                    if ctx.is_untitled { "Save" } else { "Saved" },
                )
                .accelerator("CmdOrCtrl+S")
                .enabled(ctx.is_untitled)
                .build(app)?,
            )
            .item(
                &MenuItemBuilder::with_id("menu:saveas", "Save As...")
                    .accelerator("CmdOrCtrl+Shift+S")
                    .build(app)?,
            )
            .separator()
            .item(&MenuItemBuilder::with_id("menu:export", "Export...").build(app)?);
    }

    #[cfg(not(target_os = "macos"))]
    {
        file_menu = file_menu
            .separator()
            .item(&MenuItemBuilder::with_id("menu:exit", "E&xit Gingko Writer").build(app)?);
    }

    let file_menu = file_menu.build()?;

    let edit_menu = if ctx.is_edit_mode {
        SubmenuBuilder::new(app, "&Edit")
            .item(&PredefinedMenuItem::undo(app, Some("&Undo"))?)
            .item(&PredefinedMenuItem::redo(app, Some("&Redo"))?)
            .separator()
            .item(&PredefinedMenuItem::cut(app, Some("Cu&t"))?)
            .item(&PredefinedMenuItem::copy(app, Some("&Copy"))?)
            .item(&PredefinedMenuItem::paste(app, Some("&Paste"))?)
            .build()?
    } else {
        // Note: no accelerators here — the renderer's Mousetrap bindings handle
        // these keys, so menu accelerators would double-fire (and swallow keys
        // inside textareas). Menu items remain clickable.
        SubmenuBuilder::new(app, "&Edit")
            .item(&MenuItemBuilder::with_id("menu:undo", "&Undo/Redo (Version History)").build(app)?)
            .separator()
            .item(&MenuItemBuilder::with_id("menu:cut", "Cu&t current subtree").build(app)?)
            .item(&MenuItemBuilder::with_id("menu:copy", "&Copy current subtree").build(app)?)
            .item(&MenuItemBuilder::with_id("menu:paste", "&Paste subtree below current card").build(app)?)
            .item(&MenuItemBuilder::with_id("menu:pasteinto", "&Paste subtree as child of current card").build(app)?)
            .build()?
    };

    let view_menu = {
        let mut theme_menu = SubmenuBuilder::new(app, "&Theme");
        for (id, label) in [
            ("theme:default", "Default"),
            ("theme:classic", "Classic"),
            ("theme:gray", "Gray"),
            ("theme:green", "Green"),
            ("theme:turquoise", "Turquoise"),
            ("theme:dark", "Dark"),
        ] {
            theme_menu = theme_menu.item(&MenuItemBuilder::with_id(id, label).build(app)?);
        }
        SubmenuBuilder::new(app, "&View")
            .item(&theme_menu.build()?)
            .build()?
    };

    let help_menu = SubmenuBuilder::new(app, "&Help")
        .item(&MenuItemBuilder::with_id("menu:shortcuts", "&Keyboard Shortcuts").build(app)?)
        .item(&MenuItemBuilder::with_id("menu:videos", "Help &Videos").build(app)?)
        .item(&MenuItemBuilder::with_id("menu:faq", "&FAQ && Documentation").build(app)?)
        .item(&MenuItemBuilder::with_id("menu:support", "Contact &Support").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id("menu:devtools", "Toggle Developer Tools").build(app)?)
        .build()?;

    let menu = if ctx.has_doc {
        Menu::with_items(
            app,
            &[
                &file_menu as &dyn IsMenuItem<Wry>,
                &edit_menu,
                &view_menu,
                &help_menu,
            ],
        )?
    } else {
        Menu::with_items(
            app,
            &[
                &file_menu as &dyn IsMenuItem<Wry>,
                &edit_menu,
                &help_menu,
            ],
        )?
    };
    Ok(menu)
}

fn apply_menu(app: &AppHandle) {
    let state: State<AppState> = app.state();
    let ctx = state.menu_ctx.lock().unwrap();
    if let Ok(menu) = build_menu(app, &ctx) {
        let _ = app.set_menu(menu);
    }
}

fn focused_doc_window(app: &AppHandle) -> Option<WebviewWindow> {
    let windows = app.webview_windows();
    windows
        .values()
        .find(|w| w.label().starts_with("doc-") && w.is_focused().unwrap_or(false))
        .cloned()
        .or_else(|| {
            windows
                .values()
                .find(|w| w.label().starts_with("doc-"))
                .cloned()
        })
}

// Window creation must not happen on the main thread (menu events run there):
// WebviewWindowBuilder::build() deadlocks on Windows if the event loop is
// blocked by the caller (wry#583). Spawn a worker thread instead.
fn open_doc_off_main(app: &AppHandle, file_path: Option<PathBuf>, close_home: bool) {
    let app = app.clone();
    std::thread::spawn(move || {
        let from_home = if close_home { app.get_webview_window("home") } else { None };
        if create_doc_window(&app, file_path, None).is_ok() {
            if let Some(home) = from_home {
                let _ = home.destroy();
            }
        }
    });
}

fn open_modal_off_main(app: &AppHandle, kind: &str) {
    let app = app.clone();
    let kind = kind.to_string();
    std::thread::spawn(move || {
        open_modal_window(&app, &kind);
    });
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "menu:new" => {
            open_doc_off_main(app, None, true);
        }
        "menu:open" => {
            open_file_dialog(app);
        }
        "menu:exit" => {
            app.exit(0);
        }
        "menu:shortcuts" => open_modal_off_main(app, "shortcuts"),
        "menu:videos" => open_modal_off_main(app, "videos"),
        "menu:faq" => open_modal_off_main(app, "faq"),
        "menu:support" => open_modal_off_main(app, "support"),
        "menu:devtools" => {
            if let Some(win) = focused_doc_window(app) {
                win.open_devtools();
            }
        }
        "menu:close" => {
            if let Some(win) = focused_doc_window(app) {
                let _ = win.close();
            }
        }
        id if id.starts_with("recent:") => {
            let path = id.trim_start_matches("recent:").to_string();
            open_doc_off_main(app, Some(PathBuf::from(path)), true);
        }
        // Forwarded to the focused document window's JS side.
        id if id.starts_with("theme:") => {
            if let Some(win) = focused_doc_window(app) {
                let _ = win.emit("menu-clicked", id);
            }
        }
        "menu:save" | "menu:saveas" | "menu:export" | "menu:undo" | "menu:cut" | "menu:copy"
        | "menu:paste" | "menu:pasteinto" => {
            if let Some(win) = focused_doc_window(app) {
                let _ = win.emit("menu-clicked", id);
            }
        }
        _ => {}
    }
}

fn open_file_dialog(app: &AppHandle) {
    let app = app.clone();
    // Runs the blocking dialog and the window creation on a worker thread;
    // both would deadlock on the main thread (wry#583).
    std::thread::spawn(move || {
        let picked = app
            .dialog()
            .file()
            .add_filter("Gingko Writer Document", &["gkw"])
            .add_filter("Gingko Desktop Legacy", &["gko"])
            .add_filter("Markdown Document", &["md"])
            .add_filter("All Files", &["*"])
            .blocking_pick_file();
        if let Some(fp) = picked.and_then(|f| f.into_path().ok()) {
            let from_home = app.get_webview_window("home");
            if create_doc_window(&app, Some(fp), None).is_ok() {
                if let Some(home) = from_home {
                    let _ = home.destroy();
                }
            }
        }
    });
}

fn open_modal_window(app: &AppHandle, kind: &str) {
    let (url, title, w, h) = match kind {
        "shortcuts" => ("shortcuts.html", "Keyboard Shortcuts", 1000.0, 800.0),
        "videos" => ("videos.html", "Help Videos", 800.0, 400.0),
        "faq" => ("faq.html", "FAQ & Documentation", 445.0, 600.0),
        "support" => ("support.html", "Contact Support", 800.0, 445.0),
        _ => return,
    };
    let label = format!("modal-{}", kind);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(w, h)
        .build();
}

/* ==== Doc window lifecycle ==== */

fn create_doc_window(
    app: &AppHandle,
    file_path: Option<PathBuf>,
    init_file_data: Option<String>,
) -> Result<(), String> {
    let state: State<AppState> = app.state();

    // Gingko Desktop 2.x saved .gko files as 7z archives of a database.
    // Offer to convert those; the result opens as a new Untitled document
    // (the original file is left untouched).
    let (file_path, init_file_data) = match file_path {
        Some(fp)
            if fs::read(&fp)
                .map(|raw| raw.starts_with(legacy::SEVENZ_MAGIC))
                .unwrap_or(false) =>
        {
            let convert = app
                .dialog()
                .message(
                    "This file is from Gingko Desktop 2.x, and must be converted to the new format.\n\n\
                     Convert and open a copy? (The original file is not modified.)",
                )
                .title("Legacy Gingko Desktop file")
                .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
                    "Convert".into(),
                    "Cancel".into(),
                ))
                .blocking_show();
            if !convert {
                return Err("Conversion declined".into());
            }
            match legacy::convert_gko_to_markdown(&fp) {
                Ok(markdown) => (None, Some(markdown)),
                Err(e) => {
                    app.dialog()
                        .message(format!("Could not convert this file:\n{}", e))
                        .title("Conversion failed")
                        .blocking_show();
                    return Err(e);
                }
            }
        }
        other => (other, init_file_data),
    };

    // Refuse to open the same file twice.
    if let Some(ref fp) = file_path {
        let docs = state.docs.lock().unwrap();
        if docs.values().any(|d| &d.file_path == fp) {
            app.dialog()
                .message("Cannot open file twice")
                .title("File already open")
                .blocking_show();
            return Err("File already open".into());
        }
    }

    let is_untitled = file_path.is_none();
    let (date_string, file_hash) = date_hash_strings();

    let (file_path, file_data) = match file_path {
        None => {
            // Initialize new document in temp. Write the document file itself
            // too, not just the swap copy: save_file only runs once content
            // changes, so an untouched document (single empty card) would
            // otherwise have no file for save_as to copy from.
            let fp = temp_dir().join(format!("Untitled-{}-{}.gkw", date_string, file_hash));
            let _ = fs::write(&fp, "");
            let _ = fs::write(swp_path(&fp), "");
            (fp, init_file_data)
        }
        Some(fp) => {
            // Only text-based documents are supported (legacy 7z archives
            // were already converted above).
            let raw = fs::read(&fp).map_err(|e| e.to_string())?;
            let data = String::from_utf8(raw).map_err(|_| {
                app.dialog()
                    .message("This file is not a text document, and cannot be opened.")
                    .title("Cannot open file")
                    .blocking_show();
                "Not a UTF-8 text file".to_string()
            })?;

            // Save backup copy
            let base = fp
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let backup = temp_dir().join(format!("{}-{}-{}.gkw.bak", base, date_string, file_hash));
            fs::copy(&fp, &backup).map_err(|e| e.to_string())?;

            // Open swap copy
            fs::copy(&fp, swp_path(&fp)).map_err(|e| e.to_string())?;

            add_to_recent_documents(app, &fp);
            (fp, Some(data))
        }
    };

    let undo_data = load_undo_data(app, &file_path);

    let label = {
        let mut counter = state.doc_counter.lock().unwrap();
        *counter += 1;
        format!("doc-{}", *counter)
    };

    {
        let mut docs = state.docs.lock().unwrap();
        let last_saved_hash = file_data.as_ref().map(|d| sha1_of(d.as_bytes()));
        docs.insert(
            label.clone(),
            DocState {
                file_path: file_path.clone(),
                is_untitled,
                file_data,
                undo_data,
                last_saved_hash,
                watcher: None,
            },
        );
    }

    {
        let mut ctx = state.menu_ctx.lock().unwrap();
        ctx.has_doc = true;
        ctx.is_untitled = is_untitled;
        ctx.is_edit_mode = is_untitled;
    }
    apply_menu(app);

    WebviewWindowBuilder::new(app, &label, WebviewUrl::App("renderer.html".into()))
        .title(title_text(&file_path))
        .inner_size(800.0, 600.0)
        .disable_drag_drop_handler()
        .build()
        .map_err(|e| e.to_string())?;

    start_watching(app, &label);

    Ok(())
}

fn cleanup_doc_window(app: &AppHandle, label: &str) {
    let state: State<AppState> = app.state();
    let mut docs = state.docs.lock().unwrap();
    if let Some(doc) = docs.remove(label) {
        let _ = fs::remove_file(swp_path(&doc.file_path));
    }
}

/* ==== Commands ==== */

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HomeState {
    recent_documents: Vec<RecentDoc>,
    language: String,
}

#[tauri::command(async)]
fn get_home_state(app: AppHandle) -> HomeState {
    let settings = load_settings(&app);
    HomeState {
        recent_documents: get_recent_documents(&app),
        language: settings
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("en")
            .to_string(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocInit {
    file_path: String,
    file_data: Option<String>,
    file_settings: Value,
    undo_data: Vec<Value>,
    is_untitled: bool,
}

#[tauri::command(async)]
fn get_doc_state(app: AppHandle, window: WebviewWindow, state: State<AppState>) -> Result<DocInit, String> {
    let mut docs = state.docs.lock().unwrap();
    let doc = docs
        .get_mut(window.label())
        .ok_or_else(|| format!("No doc state for window {}", window.label()))?;

    let file_settings = load_settings(&app)
        .get("fileSettings")
        .and_then(|fs_| fs_.get(doc.file_path.to_string_lossy().as_ref()))
        .cloned()
        .unwrap_or(Value::Null);

    let undo_data = doc
        .undo_data
        .iter()
        .map(|(k, v)| {
            let mut obj = v.clone();
            if let Some(map) = obj.as_object_mut() {
                map.insert("_id".to_string(), json!(k));
            }
            obj
        })
        .collect();

    Ok(DocInit {
        file_path: doc.file_path.to_string_lossy().to_string(),
        file_data: doc.file_data.take(),
        file_settings,
        undo_data,
        is_untitled: doc.is_untitled,
    })
}

#[tauri::command(async)]
fn new_document(app: AppHandle) -> Result<(), String> {
    let from_home = app.get_webview_window("home");
    create_doc_window(&app, None, None)?;
    if let Some(home) = from_home {
        let _ = home.destroy();
    }
    Ok(())
}

#[tauri::command(async)]
fn open_document(app: AppHandle, path: String) -> Result<(), String> {
    let from_home = app.get_webview_window("home");
    create_doc_window(&app, Some(PathBuf::from(path)), None)?;
    if let Some(home) = from_home {
        let _ = home.destroy();
    }
    Ok(())
}

#[tauri::command(async)]
fn open_document_dialog(app: AppHandle) {
    open_file_dialog(&app);
}

#[tauri::command(async)]
fn import_document(app: AppHandle, file_data: String) -> Result<(), String> {
    let from_home = app.get_webview_window("home");
    create_doc_window(&app, None, Some(file_data))?;
    if let Some(home) = from_home {
        let _ = home.destroy();
    }
    Ok(())
}

#[tauri::command(async)]
fn remove_recent_document(app: AppHandle, path: String) {
    let docs: Vec<RecentDoc> = get_recent_documents(&app)
        .into_iter()
        .filter(|rd| rd.path != path)
        .collect();
    set_recent_documents(&app, &docs);
    apply_menu(&app);
}

// Returns (filePath, timestampMs, isUntitled), mirroring the Electron 'file-saved' payload.
#[tauri::command(async)]
fn save_file(
    window: WebviewWindow,
    state: State<AppState>,
    data: String,
) -> Result<(String, f64, bool), String> {
    let mut docs = state.docs.lock().unwrap();
    let doc = docs
        .get_mut(window.label())
        .ok_or_else(|| format!("No doc state for window {}", window.label()))?;

    // Record the hash before writing, so the file watcher ignores this save.
    doc.last_saved_hash = Some(sha1_of(data.as_bytes()));

    let swp = swp_path(&doc.file_path);
    fs::write(&swp, &data).map_err(|e| e.to_string())?;
    fs::copy(&swp, &doc.file_path).map_err(|e| e.to_string())?;

    let _ = window.set_title(&title_text(&doc.file_path));
    Ok((
        doc.file_path.to_string_lossy().to_string(),
        now_ms(),
        doc.is_untitled,
    ))
}

#[tauri::command(async)]
fn save_as(
    app: AppHandle,
    window: WebviewWindow,
    state: State<AppState>,
    new_path: String,
) -> Result<(String, f64, bool), String> {
    let new_path = PathBuf::from(new_path);
    let label = window.label().to_string();

    let orig_path = {
        let mut docs = state.docs.lock().unwrap();
        let doc = docs
            .get_mut(&label)
            .ok_or_else(|| format!("No doc state for window {}", label))?;

        let orig_path = doc.file_path.clone();

        // Copy current contents to the new location (+ swap file).
        fs::copy(&orig_path, &new_path).map_err(|e| e.to_string())?;
        fs::copy(&orig_path, swp_path(&new_path)).map_err(|e| e.to_string())?;

        // Move undo history to the new key.
        let old_undo = undo_store_path(&app, &orig_path);
        let new_undo = undo_store_path(&app, &new_path);
        if old_undo.exists() {
            let _ = fs::copy(&old_undo, &new_undo);
            let _ = fs::remove_file(&old_undo);
        }

        // Clean up the old swap file.
        let _ = fs::remove_file(swp_path(&orig_path));

        doc.file_path = new_path.clone();
        doc.is_untitled = false;
        orig_path
    };
    let _ = orig_path;

    // The document path changed — watch the new location.
    start_watching(&app, &label);

    add_to_recent_documents(&app, &new_path);

    {
        let mut ctx = state.menu_ctx.lock().unwrap();
        ctx.is_untitled = false;
    }
    apply_menu(&app);

    let _ = window.set_title(&title_text(&new_path));
    Ok((new_path.to_string_lossy().to_string(), now_ms(), false))
}

// Persist pre-filtered commit objects. The renderer computes the commit
// (port of src/electron/commit.js) and filters already-saved immutables.
#[tauri::command(async)]
fn commit_data(
    app: AppHandle,
    window: WebviewWindow,
    state: State<AppState>,
    objects: Vec<Value>,
) -> Result<(), String> {
    let mut docs = state.docs.lock().unwrap();
    let doc = docs
        .get_mut(window.label())
        .ok_or_else(|| format!("No doc state for window {}", window.label()))?;

    for obj in objects {
        if let Some(id) = obj.get("_id").and_then(|v| v.as_str()).map(String::from) {
            let mut stripped = obj.clone();
            if let Some(map) = stripped.as_object_mut() {
                map.remove("_id");
            }
            doc.undo_data.insert(id, stripped);
        }
    }
    persist_undo_data(&app, &doc.file_path, &doc.undo_data);
    Ok(())
}

#[tauri::command(async)]
fn local_store_set(app: AppHandle, window: WebviewWindow, state: State<AppState>, key: String, value: Value) {
    let path_str = {
        let docs = state.docs.lock().unwrap();
        match docs.get(window.label()) {
            Some(doc) => doc.file_path.to_string_lossy().to_string(),
            None => return,
        }
    };
    let mut settings = load_settings(&app);
    if !settings["fileSettings"].is_object() {
        settings["fileSettings"] = json!({});
    }
    if !settings["fileSettings"][&path_str].is_object() {
        settings["fileSettings"][&path_str] = json!({});
    }
    settings["fileSettings"][&path_str][&key] = value;
    save_settings(&app, &settings);
}

#[tauri::command(async)]
fn set_edit_mode(app: AppHandle, state: State<AppState>, is_edit_mode: bool) {
    {
        let mut ctx = state.menu_ctx.lock().unwrap();
        if ctx.is_edit_mode == is_edit_mode {
            return;
        }
        ctx.is_edit_mode = is_edit_mode;
    }
    apply_menu(&app);
}

#[tauri::command(async)]
fn export_file(path: String, content: String) -> Result<(), String> {
    fs::write(path, content).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn export_docx(app: AppHandle, path: String, content: String) -> Result<(), String> {
    let target = PathBuf::from(&path);
    let tmp_md = temp_dir().join(format!(
        "{}.md",
        target
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "export".into())
    ));
    fs::write(&tmp_md, &content).map_err(|e| e.to_string())?;

    // Prefer a bundled pandoc next to the executable; fall back to
    // the dev-tree copy, then to pandoc on PATH.
    let exe_dir = app
        .path()
        .resource_dir()
        .ok()
        .or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)));
    let candidates: Vec<PathBuf> = {
        let mut v = vec![];
        if let Some(dir) = exe_dir {
            v.push(dir.join(if cfg!(windows) { "pandoc.exe" } else { "pandoc" }));
        }
        let dev_bin = if cfg!(target_os = "windows") {
            "src/bin/win/pandoc.exe"
        } else if cfg!(target_os = "macos") {
            "src/bin/mac/pandoc"
        } else {
            "src/bin/linux/pandoc"
        };
        v.push(PathBuf::from("..").join(dev_bin));
        v.push(PathBuf::from("pandoc"));
        v
    };

    let mut last_err = String::from("pandoc not found");
    for pandoc in candidates {
        let result = std::process::Command::new(&pandoc)
            .arg(&tmp_md)
            .arg("--from=gfm+hard_line_breaks")
            .arg("--to=docx")
            .arg(format!("--output={}", path))
            .output();
        match result {
            Ok(out) if out.status.success() => {
                let _ = fs::remove_file(&tmp_md);
                return Ok(());
            }
            Ok(out) => {
                last_err = String::from_utf8_lossy(&out.stderr).to_string();
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }
    let _ = fs::remove_file(&tmp_md);
    Err(last_err)
}

// Three-button "Save changes?" dialog. Returns "save" | "discard" | "cancel".
#[tauri::command(async)]
fn ask_save_changes() -> String {
    let answer = rfd::MessageDialog::new()
        .set_title("Save changes?")
        .set_description("Do you want to save your changes?")
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .set_level(rfd::MessageLevel::Warning)
        .show();
    match answer {
        rfd::MessageDialogResult::Yes => "save".into(),
        rfd::MessageDialogResult::No => "discard".into(),
        _ => "cancel".into(),
    }
}

// Pick a JSON file to import and return its contents.
#[tauri::command(async)]
fn import_json_dialog(window: WebviewWindow) -> Option<String> {
    window
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .blocking_pick_file()
        .and_then(|f| f.into_path().ok())
        .and_then(|p| fs::read_to_string(p).ok())
}

#[tauri::command(async)]
fn save_file_dialog(window: WebviewWindow, state: State<AppState>) -> Option<String> {
    let default_path = {
        let docs = state.docs.lock().unwrap();
        docs.get(window.label()).map(|d| d.file_path.clone())
    };
    let mut builder = window
        .dialog()
        .file()
        .add_filter("Gingko Writer Document", &["gkw"])
        .add_filter("Markdown Document", &["md"])
        .add_filter("All Files", &["*"]);
    if let Some(dp) = default_path {
        let in_temp = dp.starts_with(temp_dir());
        if !in_temp {
            builder = builder.set_file_name(
                dp.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            if let Some(parent) = dp.parent() {
                builder = builder.set_directory(parent);
            }
        }
    }
    builder
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command(async)]
fn export_file_dialog(window: WebviewWindow, state: State<AppState>, format: String) -> Option<String> {
    let default_name = {
        let docs = state.docs.lock().unwrap();
        docs.get(window.label()).map(|d| {
            let stem = d
                .file_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "export".into());
            format!("{}.{}", stem, format)
        })
    };
    let mut builder = window
        .dialog()
        .file()
        .add_filter(format.clone(), &[format.as_str()]);
    if let Some(name) = default_name {
        builder = builder.set_file_name(name);
    }
    builder
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command(async)]
fn close_document(app: AppHandle, window: WebviewWindow) {
    cleanup_doc_window(&app, window.label());
    let _ = window.destroy();
}

#[tauri::command(async)]
fn open_modal(app: AppHandle, kind: String) {
    open_modal_window(&app, &kind);
}

#[tauri::command(async)]
fn open_external(app: AppHandle, url: String) {
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().open_url(url, None::<String>);
}

/* ==== App setup ==== */

fn create_home_window(app: &AppHandle) {
    {
        let state: State<AppState> = app.state();
        let mut ctx = state.menu_ctx.lock().unwrap();
        ctx.has_doc = false;
        ctx.is_untitled = false;
        ctx.is_edit_mode = false;
    }
    apply_menu(app);
    let _ = WebviewWindowBuilder::new(app, "home", WebviewUrl::App("home.html".into()))
        .title("Gingko Writer - Home")
        .inner_size(800.0, 600.0)
        .build();
}

fn path_argument(args: &[String]) -> Option<PathBuf> {
    args.iter()
        .skip(1)
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // Second instance: open the file it was launched with, or focus.
            if let Some(path) = path_argument(&args) {
                open_doc_off_main(app, Some(path), false);
            } else if let Some(win) = app.webview_windows().values().next() {
                let _ = win.set_focus();
            }
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_home_state,
            get_doc_state,
            new_document,
            open_document,
            open_document_dialog,
            import_document,
            remove_recent_document,
            save_file,
            save_as,
            commit_data,
            local_store_set,
            set_edit_mode,
            export_file,
            export_docx,
            ask_save_changes,
            import_json_dialog,
            save_file_dialog,
            export_file_dialog,
            close_document,
            open_modal,
            open_external
        ])
        .on_menu_event(|app, event| {
            handle_menu_event(app, event.id().as_ref());
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if window.label().starts_with("doc-") {
                    cleanup_doc_window(window.app_handle(), window.label());
                }
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let args: Vec<String> = std::env::args().collect();
            if let Some(path) = path_argument(&args) {
                let _ = create_doc_window(&handle, Some(path), None);
            } else {
                create_home_window(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if app.webview_windows().is_empty() {
                    let app = app.clone();
                    std::thread::spawn(move || create_home_window(&app));
                }
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { ref urls } = event {
                for url in urls {
                    if let Ok(path) = url.to_file_path() {
                        open_doc_off_main(app, Some(path), false);
                    }
                }
            }
            let _ = (app, &event);
        });
}
