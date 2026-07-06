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
use tauri::menu::{
    IsMenuItem, Menu, MenuItem, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder,
};
use tauri::{
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry,
};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

/* ==== State ==== */

struct DocState {
    file_path: PathBuf,
    is_untitled: bool,
    is_edit_mode: bool,
    // Initial file contents, consumed by get_doc_state on renderer boot.
    file_data: Option<String>,
    undo_data: HashMap<String, Value>,
}

// Every window has its own menu, updated in place. A menu shared by all
// windows dies with the first one that closes (Windows destroys a window's
// menu along with it), and replacing a window's menu bar makes it re-layout.
struct MenuHandles {
    menu: Menu<Wry>,
    recent: Submenu<Wry>,
    // Document windows only.
    save: Option<MenuItem<Wry>>,
    // Text commands while a card is edited, card commands otherwise; one of
    // the two is in the menu bar at a time.
    edit_text: Submenu<Wry>,
    edit_card: Submenu<Wry>,
    edit_is_text: bool,
}

#[derive(Clone, Copy)]
struct DocMenuState {
    is_untitled: bool,
    is_edit_mode: bool,
}

#[derive(Default)]
struct AppState {
    docs: Mutex<HashMap<String, DocState>>,
    doc_counter: Mutex<u32>,
    // Menu handles per window label (home and document windows).
    menus: Mutex<HashMap<String, MenuHandles>>,
    // Held while a document window is being opened, so that two concurrent
    // requests (e.g. a double-click in the home window) can't open the same
    // file twice.
    opening: Mutex<()>,
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

fn to_ms(time: SystemTime) -> Option<f64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as f64)
}

fn now_ms() -> f64 {
    to_ms(SystemTime::now()).unwrap_or(0.0)
}

fn settings_path(app: &AppHandle) -> PathBuf {
    let dir = app.path().app_config_dir().expect("no app config dir");
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
    refresh_recent_menus(app);
}

fn add_to_recent_documents(app: &AppHandle, file_path: &Path) {
    let meta = fs::metadata(file_path).ok();
    let meta_ms = |time: Option<SystemTime>| time.and_then(to_ms).unwrap_or_else(now_ms);
    let path = file_path.to_string_lossy().to_string();
    let entry = RecentDoc {
        name: file_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        path: path.clone(),
        birthtime_ms: meta_ms(meta.as_ref().and_then(|m| m.created().ok())),
        atime_ms: now_ms(),
        mtime_ms: meta_ms(meta.as_ref().and_then(|m| m.modified().ok())),
    };
    let mut docs = get_recent_documents(app);
    docs.retain(|rd| rd.path != path);
    docs.push(entry);
    set_recent_documents(app, &docs);
}

fn date_hash_strings() -> (String, String) {
    let ms = now_ms() as u64;
    let mut hasher = Sha1::new();
    hasher.update(ms.to_string().as_bytes());
    let hash = hex::encode(hasher.finalize());
    // ISO date (yyyy-mm-dd) from days since epoch, no chrono dependency.
    let days = ms / 86_400_000;
    let (y, m, d) = civil_from_days(days as i64);
    (
        format!("{:04}-{:02}-{:02}", y, m, d),
        hash[0..6].to_string(),
    )
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

// Paths reaching us from the recent-documents list, the command line and file
// dialogs can differ in case or prefix on Windows while naming the same file.
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
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

fn persist_undo_data(
    app: &AppHandle,
    file_path: &Path,
    data: &HashMap<String, Value>,
) -> Result<(), String> {
    let s = serde_json::to_string(data).map_err(|e| e.to_string())?;
    fs::write(undo_store_path(app, file_path), s).map_err(|e| e.to_string())
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

fn build_menu(app: &AppHandle, doc: Option<DocMenuState>) -> tauri::Result<MenuHandles> {
    let recent = SubmenuBuilder::new(app, "Open &Recent").build()?;
    fill_recent_menu(app, &recent)?;

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
        )
        .item(&recent);

    let mut save = None;
    if let Some(doc) = doc {
        let item = MenuItemBuilder::with_id("menu:save", "Save")
            .accelerator("CmdOrCtrl+S")
            .build(app)?;
        set_save_state(&item, doc.is_untitled);
        file_menu = file_menu
            .item(&MenuItemBuilder::with_id("menu:close", "&Close").build(app)?)
            .separator()
            .item(&item)
            .item(
                &MenuItemBuilder::with_id("menu:saveas", "Save As...")
                    .accelerator("CmdOrCtrl+Shift+S")
                    .build(app)?,
            )
            .separator()
            .item(&MenuItemBuilder::with_id("menu:export", "Export...").build(app)?);
        save = Some(item);
    }

    #[cfg(not(target_os = "macos"))]
    {
        file_menu = file_menu
            .separator()
            .item(&MenuItemBuilder::with_id("menu:exit", "E&xit Gingko Writer").build(app)?);
    }

    let file_menu = file_menu.build()?;

    let edit_text = SubmenuBuilder::new(app, "&Edit")
        .item(&PredefinedMenuItem::undo(app, Some("&Undo"))?)
        .item(&PredefinedMenuItem::redo(app, Some("&Redo"))?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, Some("Cu&t"))?)
        .item(&PredefinedMenuItem::copy(app, Some("&Copy"))?)
        .item(&PredefinedMenuItem::paste(app, Some("&Paste"))?)
        .build()?;

    // No accelerators: the renderer's Mousetrap bindings handle these keys,
    // menu accelerators would fire twice (and swallow keys in textareas).
    let edit_card = SubmenuBuilder::new(app, "&Edit")
        .item(&MenuItemBuilder::with_id("menu:undo", "&Undo/Redo (Version History)").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id("menu:cut", "Cu&t current subtree").build(app)?)
        .item(&MenuItemBuilder::with_id("menu:copy", "&Copy current subtree").build(app)?)
        .item(
            &MenuItemBuilder::with_id("menu:paste", "&Paste subtree below current card")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::with_id("menu:pasteinto", "&Paste subtree as child of current card")
                .build(app)?,
        )
        .build()?;

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
    let view_menu = SubmenuBuilder::new(app, "&View")
        .item(&theme_menu.build()?)
        .build()?;

    let help_menu = SubmenuBuilder::new(app, "&Help")
        .item(&MenuItemBuilder::with_id("menu:shortcuts", "&Keyboard Shortcuts").build(app)?)
        .item(&MenuItemBuilder::with_id("menu:videos", "Help &Videos").build(app)?)
        .item(&MenuItemBuilder::with_id("menu:faq", "&FAQ && Documentation").build(app)?)
        .item(&MenuItemBuilder::with_id("menu:support", "Contact &Support").build(app)?)
        .separator()
        .item(&MenuItemBuilder::with_id("menu:devtools", "Toggle Developer Tools").build(app)?)
        .build()?;

    let edit_is_text = doc.is_some_and(|d| d.is_edit_mode);
    let edit_menu = if edit_is_text { &edit_text } else { &edit_card };
    let mut menus: Vec<&dyn IsMenuItem<Wry>> = vec![&file_menu, edit_menu];
    if doc.is_some() {
        menus.push(&view_menu);
    }
    menus.push(&help_menu);
    let menu = Menu::with_items(app, &menus)?;
    Ok(MenuHandles {
        menu,
        recent,
        save,
        edit_text,
        edit_card,
        edit_is_text,
    })
}

fn set_save_state(save: &MenuItem<Wry>, is_untitled: bool) {
    let _ = save.set_text(if is_untitled { "Save" } else { "Saved" });
    let _ = save.set_enabled(is_untitled);
}

fn fill_recent_menu(app: &AppHandle, recent: &Submenu<Wry>) -> tauri::Result<()> {
    while recent.remove_at(0)?.is_some() {}
    for (idx, rd) in get_recent_documents(app).iter().enumerate() {
        recent.append(
            &MenuItemBuilder::with_id(
                format!("recent:{}", rd.path),
                format!("&{}.  {}", idx + 1, rd.name),
            )
            .build(app)?,
        )?;
    }
    Ok(())
}

// Builds the menu for a new window and keeps its handles for later updates.
// On macOS the menu bar is app-wide, so the new window's menu replaces it.
fn window_with_menu<'a>(
    app: &'a AppHandle,
    builder: WebviewWindowBuilder<'a, Wry, AppHandle>,
    label: &str,
    doc: Option<DocMenuState>,
) -> tauri::Result<WebviewWindowBuilder<'a, Wry, AppHandle>> {
    let handles = build_menu(app, doc)?;
    let menu = handles.menu.clone();
    app.state::<AppState>()
        .menus
        .lock()
        .unwrap()
        .insert(label.to_string(), handles);
    #[cfg(target_os = "macos")]
    {
        app.set_menu(menu)?;
        Ok(builder)
    }
    #[cfg(not(target_os = "macos"))]
    Ok(builder.menu(menu))
}

// Menu updates run on the main thread, like window events, so the two never
// wait on each other.
fn refresh_recent_menus(app: &AppHandle) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        for m in app2.state::<AppState>().menus.lock().unwrap().values() {
            let _ = fill_recent_menu(&app2, &m.recent);
        }
    });
}

// Bring a document window's menu in line with its state.
fn update_doc_menu(app: &AppHandle, label: &str) {
    let app2 = app.clone();
    let label = label.to_string();
    let _ = app.run_on_main_thread(move || {
        let state: State<AppState> = app2.state();
        let Some((is_untitled, is_edit_mode)) = state
            .docs
            .lock()
            .unwrap()
            .get(&label)
            .map(|d| (d.is_untitled, d.is_edit_mode))
        else {
            return;
        };
        let mut menus = state.menus.lock().unwrap();
        let Some(m) = menus.get_mut(&label) else {
            return;
        };
        if let Some(save) = &m.save {
            set_save_state(save, is_untitled);
        }
        if m.edit_is_text != is_edit_mode {
            let (old, new) = if is_edit_mode {
                (&m.edit_card, &m.edit_text)
            } else {
                (&m.edit_text, &m.edit_card)
            };
            if m.menu.remove(old).is_ok() && m.menu.insert(new, 1).is_ok() {
                m.edit_is_text = is_edit_mode;
            }
        }
    });
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
        let _ = if close_home {
            open_doc(&app, file_path, None)
        } else {
            create_doc_window(&app, file_path, None)
        };
    });
}

fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "menu:new" => open_doc_off_main(app, None, true),
        "menu:open" => open_file_dialog(app),
        // Close each window instead of exiting, so documents run their close
        // handling (save prompt for untitled docs, pending saves). The app
        // exits once the last window is gone.
        "menu:exit" => {
            for win in app.webview_windows().values() {
                let _ = win.close();
            }
        }
        "menu:shortcuts" => open_modal(app, "shortcuts"),
        "menu:videos" => open_modal(app, "videos"),
        "menu:faq" => open_modal(app, "faq"),
        "menu:support" => open_modal(app, "support"),
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
        "menu:save" | "menu:saveas" | "menu:export" | "menu:undo" | "menu:cut" | "menu:copy"
        | "menu:paste" | "menu:pasteinto" => emit_to_focused_doc(app, id),
        id if id.starts_with("theme:") => emit_to_focused_doc(app, id),
        _ => {
            if let Some(path) = id.strip_prefix("recent:") {
                open_doc_off_main(app, Some(PathBuf::from(path)), true);
            }
        }
    }
}

// For menu actions handled by the document's renderer. A plain emit() would
// reach every window.
fn emit_to_focused_doc(app: &AppHandle, id: &str) {
    if let Some(win) = focused_doc_window(app) {
        let _ = win.emit_to(win.label(), "menu-clicked", id);
    }
}

fn open_file_dialog(app: &AppHandle) {
    let app = app.clone();
    // Off the main thread for the same reason as open_doc_off_main; the
    // blocking dialog would deadlock there, too.
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
            let _ = open_doc(&app, Some(fp), None);
        }
    });
}

fn open_modal(app: &AppHandle, kind: &'static str) {
    let (url, title, w, h) = match kind {
        "shortcuts" => ("shortcuts.html", "Keyboard Shortcuts", 1000.0, 800.0),
        "videos" => ("videos.html", "Help Videos", 800.0, 400.0),
        "faq" => ("faq.html", "FAQ & Documentation", 445.0, 600.0),
        "support" => ("support.html", "Contact Support", 800.0, 445.0),
        _ => return,
    };
    // Off the main thread, see open_doc_off_main.
    let app = app.clone();
    std::thread::spawn(move || {
        let label = format!("modal-{}", kind);
        if let Some(existing) = app.get_webview_window(&label) {
            let _ = existing.set_focus();
            return;
        }
        let _ = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
            .title(title)
            .inner_size(w, h)
            .build();
    });
}

/* ==== Doc window lifecycle ==== */

// Opens a document window in place of the home window.
fn open_doc(
    app: &AppHandle,
    file_path: Option<PathBuf>,
    init_file_data: Option<String>,
) -> Result<(), String> {
    create_doc_window(app, file_path, init_file_data)?;
    if let Some(home) = app.get_webview_window("home") {
        let _ = home.destroy();
    }
    Ok(())
}

fn create_doc_window(
    app: &AppHandle,
    file_path: Option<PathBuf>,
    init_file_data: Option<String>,
) -> Result<(), String> {
    let state: State<AppState> = app.state();
    let _opening = state.opening.lock().unwrap();

    // Gingko Desktop 2.x .gko files (7z archives of a database) are converted
    // into a new Untitled document; the original file is left untouched.
    let (file_path, init_file_data) = match file_path {
        Some(fp) if legacy::is_legacy_gko(&fp) => {
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

    // A file can only be open once: surface the window that already has it.
    if let Some(ref fp) = file_path {
        let existing = {
            let docs = state.docs.lock().unwrap();
            docs.iter()
                .find(|(_, d)| same_file(&d.file_path, fp))
                .map(|(label, _)| label.clone())
        };
        if let Some(label) = existing {
            if let Some(win) = app.get_webview_window(&label) {
                if win.is_minimized().unwrap_or(false) {
                    let _ = win.unminimize();
                }
                let _ = win.show();
                let _ = win.set_focus();
            }
            return Ok(());
        }
    }

    let is_untitled = file_path.is_none();
    let (date_string, file_hash) = date_hash_strings();

    let (file_path, file_data) = match file_path {
        None => {
            let fp =
                std::env::temp_dir().join(format!("Untitled-{}-{}.gkw", date_string, file_hash));
            // Create the file itself, not just the swap copy: save_as copies
            // from it, and save_file only runs once the content changes.
            let _ = fs::write(&fp, "");
            let _ = fs::write(swp_path(&fp), "");
            (fp, init_file_data)
        }
        Some(fp) => {
            // Legacy archives were converted above; anything else must be text.
            let Ok(data) = String::from_utf8(fs::read(&fp).map_err(|e| e.to_string())?) else {
                app.dialog()
                    .message("This file is not a text document, and cannot be opened.")
                    .title("Cannot open file")
                    .blocking_show();
                return Err("Not a UTF-8 text file".into());
            };

            // Backup copy in temp, swap copy next to the file.
            let base = fp
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let backup = std::env::temp_dir()
                .join(format!("{}-{}-{}.gkw.bak", base, date_string, file_hash));
            fs::copy(&fp, backup).map_err(|e| e.to_string())?;
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

    state.docs.lock().unwrap().insert(
        label.clone(),
        DocState {
            file_path: file_path.clone(),
            is_untitled,
            is_edit_mode: is_untitled,
            file_data,
            undo_data,
        },
    );

    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("renderer.html".into()))
        .title(title_text(&file_path))
        .inner_size(800.0, 600.0)
        .disable_drag_drop_handler();
    let doc_menu = DocMenuState {
        is_untitled,
        is_edit_mode: is_untitled,
    };
    window_with_menu(app, builder, &label, Some(doc_menu))
        .and_then(|builder| builder.build())
        .map_err(|e| e.to_string())?;

    Ok(())
}

fn cleanup_doc_window(app: &AppHandle, label: &str) {
    let state: State<AppState> = app.state();
    let doc = state.docs.lock().unwrap().remove(label);
    if let Some(doc) = doc {
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
fn get_doc_state(
    app: AppHandle,
    window: WebviewWindow,
    state: State<AppState>,
) -> Result<DocInit, String> {
    let mut docs = state.docs.lock().unwrap();
    let doc = docs
        .get_mut(window.label())
        .ok_or_else(|| format!("No doc state for window {}", window.label()))?;

    let file_settings = load_settings(&app)
        .get("fileSettings")
        .and_then(|s| s.get(doc.file_path.to_string_lossy().as_ref()))
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
    open_doc(&app, None, None)
}

#[tauri::command(async)]
fn open_document(app: AppHandle, path: String) -> Result<(), String> {
    open_doc(&app, Some(PathBuf::from(path)), None)
}

#[tauri::command(async)]
fn open_document_dialog(app: AppHandle) {
    open_file_dialog(&app);
}

#[tauri::command(async)]
fn import_document(app: AppHandle, file_data: String) -> Result<(), String> {
    open_doc(&app, None, Some(file_data))
}

#[tauri::command(async)]
fn remove_recent_document(app: AppHandle, path: String) {
    let mut docs = get_recent_documents(&app);
    docs.retain(|rd| rd.path != path);
    set_recent_documents(&app, &docs);
}

// Returns (filePath, timestampMs, isUntitled), mirroring the Electron 'file-saved' payload.
#[tauri::command(async)]
fn save_file(
    window: WebviewWindow,
    state: State<AppState>,
    data: String,
) -> Result<(String, f64, bool), String> {
    let docs = state.docs.lock().unwrap();
    let doc = docs
        .get(window.label())
        .ok_or_else(|| format!("No doc state for window {}", window.label()))?;

    let swp = swp_path(&doc.file_path);
    fs::write(&swp, &data).map_err(|e| e.to_string())?;
    fs::copy(&swp, &doc.file_path).map_err(|e| e.to_string())?;

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

    {
        let mut docs = state.docs.lock().unwrap();
        let doc = docs
            .get_mut(window.label())
            .ok_or_else(|| format!("No doc state for window {}", window.label()))?;
        let orig_path = &doc.file_path;

        // Saving onto the current file: nothing to move. Copying a file onto
        // itself can truncate it, and the cleanup below would delete the
        // history and swap file of the file we keep.
        if same_file(orig_path, &new_path) {
            return Ok((
                orig_path.to_string_lossy().to_string(),
                now_ms(),
                doc.is_untitled,
            ));
        }

        fs::copy(orig_path, &new_path).map_err(|e| e.to_string())?;
        fs::copy(orig_path, swp_path(&new_path)).map_err(|e| e.to_string())?;
        let _ = fs::rename(
            undo_store_path(&app, orig_path),
            undo_store_path(&app, &new_path),
        );
        let _ = fs::remove_file(swp_path(orig_path));
        // An untitled document now lives at new_path; drop its temp file.
        if doc.is_untitled {
            let _ = fs::remove_file(orig_path);
        }

        doc.file_path = new_path.clone();
        doc.is_untitled = false;
    }

    add_to_recent_documents(&app, &new_path);
    update_doc_menu(&app, window.label());

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

    for mut obj in objects {
        if let Some(Value::String(id)) = obj.as_object_mut().and_then(|m| m.remove("_id")) {
            doc.undo_data.insert(id, obj);
        }
    }
    persist_undo_data(&app, &doc.file_path, &doc.undo_data)
}

#[tauri::command(async)]
fn local_store_set(
    app: AppHandle,
    window: WebviewWindow,
    state: State<AppState>,
    key: String,
    value: Value,
) {
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
fn set_edit_mode(
    app: AppHandle,
    window: WebviewWindow,
    state: State<AppState>,
    is_edit_mode: bool,
) {
    {
        let mut docs = state.docs.lock().unwrap();
        match docs.get_mut(window.label()) {
            Some(doc) if doc.is_edit_mode != is_edit_mode => doc.is_edit_mode = is_edit_mode,
            _ => return,
        }
    }
    update_doc_menu(&app, window.label());
}

#[tauri::command(async)]
fn export_file(path: String, content: String) -> Result<(), String> {
    fs::write(path, content).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn export_docx(app: AppHandle, path: String, content: String) -> Result<(), String> {
    let file_name = Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "export".into());
    let tmp_md = std::env::temp_dir().join(format!("{}.md", file_name));
    fs::write(&tmp_md, &content).map_err(|e| e.to_string())?;

    // Prefer a bundled pandoc, then the dev-tree copy, then pandoc on PATH.
    let exe_dir = app.path().resource_dir().ok().or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
    });
    let (bundled, dev_bin) = if cfg!(target_os = "windows") {
        ("pandoc.exe", "src/bin/win/pandoc.exe")
    } else if cfg!(target_os = "macos") {
        ("pandoc", "src/bin/mac/pandoc")
    } else {
        ("pandoc", "src/bin/linux/pandoc")
    };
    let candidates = exe_dir
        .map(|dir| dir.join(bundled))
        .into_iter()
        .chain([Path::new("..").join(dev_bin), PathBuf::from("pandoc")]);

    let mut result = Err("pandoc not found".to_string());
    for pandoc in candidates {
        result = match std::process::Command::new(&pandoc)
            .arg(&tmp_md)
            .arg("--from=gfm+hard_line_breaks")
            .arg("--to=docx")
            .arg(format!("--output={}", path))
            .output()
        {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(String::from_utf8_lossy(&out.stderr).to_string()),
            Err(e) => Err(e.to_string()),
        };
        if result.is_ok() {
            break;
        }
    }
    let _ = fs::remove_file(&tmp_md);
    result
}

// Three-button "Save changes?" dialog. Returns "save" | "discard" | "cancel".
#[tauri::command(async)]
fn ask_save_changes(window: WebviewWindow) -> String {
    let answer = window
        .dialog()
        .message("Do you want to save your changes?")
        .title("Save changes?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::YesNoCancel)
        .parent(&window)
        .blocking_show_with_result();
    match answer {
        MessageDialogResult::Yes => "save".into(),
        MessageDialogResult::No => "discard".into(),
        _ => "cancel".into(),
    }
}

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
    let current_path = {
        let docs = state.docs.lock().unwrap();
        docs.get(window.label()).map(|d| d.file_path.clone())
    };
    let mut builder = window
        .dialog()
        .file()
        .add_filter("Gingko Writer Document", &["gkw"])
        .add_filter("Markdown Document", &["md"])
        .add_filter("All Files", &["*"]);
    // Untitled documents live in the temp dir; don't suggest saving there.
    if let Some(path) = current_path.filter(|p| !p.starts_with(std::env::temp_dir())) {
        if let Some(name) = path.file_name() {
            builder = builder.set_file_name(name.to_string_lossy());
        }
        if let Some(dir) = path.parent() {
            builder = builder.set_directory(dir);
        }
    }
    builder
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command(async)]
fn export_file_dialog(
    window: WebviewWindow,
    state: State<AppState>,
    format: String,
) -> Option<String> {
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

// The Destroyed window event cleans up the document state.
#[tauri::command(async)]
fn close_document(window: WebviewWindow) {
    let _ = window.destroy();
}

/* ==== App setup ==== */

fn create_home_window(app: &AppHandle) {
    let builder = WebviewWindowBuilder::new(app, "home", WebviewUrl::App("home.html".into()))
        .title("Gingko Writer - Home")
        .inner_size(800.0, 600.0);
    let _ = window_with_menu(app, builder, "home", None).and_then(|builder| builder.build());
}

fn path_argument(args: &[String]) -> Option<PathBuf> {
    args.iter().skip(1).map(PathBuf::from).find(|p| p.is_file())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
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
        ])
        .on_menu_event(|app, event| handle_menu_event(app, event.id().as_ref()))
        .on_window_event(|window, event| match event {
            #[cfg(target_os = "macos")]
            tauri::WindowEvent::Focused(true) => {
                let state: State<AppState> = window.state();
                if let Some(m) = state.menus.lock().unwrap().get(window.label()) {
                    let _ = window.app_handle().set_menu(m.menu.clone());
                }
            }
            tauri::WindowEvent::Destroyed => {
                let state: State<AppState> = window.state();
                state.menus.lock().unwrap().remove(window.label());
                if window.label().starts_with("doc-") {
                    cleanup_doc_window(window.app_handle(), window.label());
                }
            }
            _ => {}
        })
        .setup(|app| {
            let handle = app.handle();
            let args: Vec<String> = std::env::args().collect();
            if let Some(path) = path_argument(&args) {
                let _ = create_doc_window(handle, Some(path), None);
            } else {
                create_home_window(handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            match event {
                tauri::RunEvent::Reopen { .. } if app.webview_windows().is_empty() => {
                    let app = app.clone();
                    std::thread::spawn(move || create_home_window(&app));
                }
                tauri::RunEvent::Opened { urls } => {
                    for path in urls.iter().filter_map(|url| url.to_file_path().ok()) {
                        open_doc_off_main(app, Some(path), false);
                    }
                }
                _ => {}
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
