// MD Viewer 앱 껍데기 (Tauri — 맥·윈도우·리눅스). 문서 하나에 창 하나.
// 화면은 dist/viewer.html (web/ 를 묶은 것)이고, 이 파일은 파일 읽기·감시, 창·메뉴, 링크 열기를 맡는다.
// 화면 쪽과는 두 길로 주고받는다: 화면 → 여기 = invoke 명령(load_document 등), 여기 → 화면 = eval("MDV.…").
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::menu::{
    Menu, MenuBuilder, MenuEvent, MenuItem, MenuItemBuilder, PredefinedMenuItem, Submenu,
    SubmenuBuilder,
};
use tauri::{
    AppHandle, Manager, RunEvent, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    Window, WindowEvent, Wry,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

const MD_EXTS: &[&str] = &["md", "markdown", "mdown", "mkd", "mkdn", "mdtext", "txt"];
const RECENT_MAX: usize = 10;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct Prefs {
    outline_open: bool,
    zoom: f64,
    width: f64,
    height: f64,
    recent: Vec<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            outline_open: false,
            zoom: 1.0,
            width: 900.0,
            height: 1000.0,
            recent: vec![],
        }
    }
}

struct Doc {
    path: PathBuf,
    anchor: Option<String>,
    last_text: Option<String>, // 감시가 같은 내용으로 여러 번 울려도 다시 그리지 않게
    #[allow(dead_code)]
    watcher: Option<notify::RecommendedWatcher>, // 창이 닫혀 Doc 이 지워지면 감시도 끝난다
}

struct SelfTest {
    file: PathBuf,
    report: PathBuf,
    hold: u64,
    reported: AtomicBool,
}

struct AppState {
    docs: Mutex<HashMap<String, Doc>>, // 창 label → 문서
    order: Mutex<Vec<String>>,         // 열린 순서 (새 창 계단 배치·메뉴 대상 찾기)
    counter: AtomicUsize,
    prefs: Mutex<Prefs>,
    prefs_path: PathBuf,
    log_path: PathBuf,
    recent_menu: Mutex<Option<Submenu<Wry>>>,
    self_test: Option<SelfTest>,
}

// ── 명령줄 ──────────────────────────────────────────────────────────
// 파일 경로(윈도우·리눅스 파일 연결), --self-test <md> --report <json> [--hold 초], --set-default
struct Cli {
    files: Vec<PathBuf>,
    self_test: Option<SelfTest>,
    set_default: bool,
}

fn parse_cli(args: impl IntoIterator<Item = String>, cwd: &Path) -> Cli {
    let mut cli = Cli {
        files: vec![],
        self_test: None,
        set_default: false,
    };
    let (mut test_file, mut report, mut hold) = (None, None, 0);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--self-test" => test_file = it.next().map(|f| cwd.join(f)),
            "--report" => report = it.next().map(|f| cwd.join(f)),
            "--hold" => hold = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--set-default" => cli.set_default = true,
            s if s.starts_with('-') => {}
            s => {
                let p = match Url::parse(s) {
                    Ok(u) if u.scheme() == "file" => u.to_file_path().unwrap_or_default(),
                    _ => cwd.join(s),
                };
                if p.is_file() {
                    cli.files.push(p);
                }
            }
        }
    }
    if let Some(file) = test_file {
        let report = report.unwrap_or_else(|| cwd.join("selftest-report.json"));
        cli.self_test = Some(SelfTest {
            file,
            report,
            hold,
            reported: AtomicBool::new(false),
        });
    }
    cli
}

// ── 파일 ────────────────────────────────────────────────────────────
fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE
            .decode_with_bom_removal(bytes)
            .0
            .into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE
            .decode_with_bom_removal(bytes)
            .0
            .into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::EUC_KR.decode(bytes).0.into_owned(), // CP949 (옛 한글 문서)
    }
}

fn is_markdown(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MD_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn file_title(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "MD Viewer".into())
}

// 편집기들은 임시 파일에 쓰고 이름을 바꿔 저장하므로 파일 대신 폴더를 보고 이름으로 거른다.
fn watch_file(app: &AppHandle, label: &str, path: &Path) -> Option<notify::RecommendedWatcher> {
    let (app, label, name) = (app.clone(), label.to_string(), path.file_name()?.to_owned());
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        // 우리가 읽을 때 생기는 접근 이벤트(리눅스)는 무시
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        if ev
            .paths
            .iter()
            .any(|p| p.file_name() == Some(name.as_os_str()))
        {
            if let Some(w) = app.get_webview_window(&label) {
                let _ = w.eval("MDV.changed()");
            }
        }
    })
    .ok()?;
    watcher
        .watch(path.parent()?, RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

fn log_line(app: &AppHandle, msg: &str) {
    let st = app.state::<AppState>();
    if let Some(dir) = st.log_path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&st.log_path)
    {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}

fn save_prefs(app: &AppHandle) {
    let st = app.state::<AppState>();
    if st.self_test.is_some() {
        return;
    }
    let json = serde_json::to_vec_pretty(&*st.prefs.lock().unwrap()).unwrap_or_default();
    if let Some(dir) = st.prefs_path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&st.prefs_path, json);
}

// ── 창 ──────────────────────────────────────────────────────────────
fn is_app_url(url: &Url) -> bool {
    matches!(url.scheme(), "tauri" | "asset" | "about" | "data" | "blob")
        || matches!(
            url.host_str(),
            Some("tauri.localhost" | "asset.localhost" | "ipc.localhost")
        )
}

fn scroll_to(w: &WebviewWindow, anchor: &str) {
    let _ = w.eval(format!(
        "MDV.scrollToAnchor({})",
        serde_json::to_string(anchor).unwrap_or_default()
    ));
}

fn open_document(app: &AppHandle, path: PathBuf, anchor: Option<String>) {
    let path = fs::canonicalize(&path).unwrap_or(path);
    let st = app.state::<AppState>();

    // 이미 열린 문서면 그 창을 앞으로
    let existing = st
        .docs
        .lock()
        .unwrap()
        .iter()
        .find(|(_, d)| d.path == path)
        .map(|(l, _)| l.clone());
    if let Some(w) = existing.and_then(|l| app.get_webview_window(&l)) {
        let _ = w.unminimize();
        let _ = w.set_focus();
        if let Some(a) = anchor {
            scroll_to(&w, &a);
        }
        return;
    }

    let label = format!("doc{}", st.counter.fetch_add(1, Ordering::SeqCst) + 1);
    let (width, height) = {
        let p = st.prefs.lock().unwrap();
        (p.width, p.height)
    };
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("viewer.html".into()))
        .title(file_title(&path))
        .inner_size(width, height)
        .min_inner_size(420.0, 300.0)
        .on_navigation(is_app_url);
    // 새 창은 마지막 창에서 오른쪽 아래로 비켜 연다
    let last = st
        .order
        .lock()
        .unwrap()
        .last()
        .cloned()
        .and_then(|l| app.get_webview_window(&l));
    let last_pos = last.and_then(|w| {
        let sf = w.scale_factor().ok()?;
        Some(w.outer_position().ok()?.to_logical::<f64>(sf))
    });
    builder = match last_pos {
        Some(p) => builder.position(p.x + 26.0, p.y + 26.0),
        None => builder.center(),
    };

    // 화면이 곧바로 load_document 를 부르므로 창보다 먼저 등록한다
    st.docs.lock().unwrap().insert(
        label.clone(),
        Doc {
            path: path.clone(),
            anchor,
            last_text: None,
            watcher: None,
        },
    );
    match builder.build() {
        Ok(_) => {
            st.order.lock().unwrap().push(label.clone());
            let watcher = watch_file(app, &label, &path);
            if let Some(d) = st.docs.lock().unwrap().get_mut(&label) {
                d.watcher = watcher;
            }
            remember_recent(app, &path);
        }
        Err(e) => {
            st.docs.lock().unwrap().remove(&label);
            log_line(app, &format!("창 열기 실패 {}: {e}", path.display()));
        }
    }
}

fn focused_window(app: &AppHandle) -> Option<WebviewWindow> {
    let wins = app.webview_windows();
    wins.values()
        .find(|w| w.is_focused().unwrap_or(false))
        .cloned()
        .or_else(|| {
            let order = app.state::<AppState>().order.lock().unwrap().clone();
            order.iter().rev().find_map(|l| wins.get(l).cloned())
        })
}

fn show_open_dialog(app: &AppHandle) {
    let handle = app.clone();
    app.dialog()
        .file()
        .set_title("마크다운 열기")
        .add_filter("Markdown", MD_EXTS)
        .pick_files(move |picked| {
            let paths: Vec<PathBuf> = picked
                .unwrap_or_default()
                .into_iter()
                .filter_map(|f| f.into_path().ok())
                .collect();
            if paths.is_empty() {
                // 맥은 창이 없어도 메뉴 막대에 남지만, 윈도우·리눅스는 보이는 것 없이 떠 있게 되므로 끝낸다
                #[cfg(not(target_os = "macos"))]
                if handle.state::<AppState>().docs.lock().unwrap().is_empty() {
                    handle.exit(0);
                }
                return;
            }
            for p in paths {
                open_document(&handle, p, None);
            }
        });
}

// ── 최근 사용 ──────────────────────────────────────────────────────
fn remember_recent(app: &AppHandle, path: &Path) {
    let st = app.state::<AppState>();
    if st.self_test.is_some() {
        return;
    }
    {
        let mut p = st.prefs.lock().unwrap();
        p.recent.retain(|r| r != path);
        p.recent.insert(0, path.to_path_buf());
        p.recent.truncate(RECENT_MAX);
    }
    save_prefs(app);
    refresh_recent(app);
}

fn refresh_recent(app: &AppHandle) {
    let st = app.state::<AppState>();
    let Some(sub) = st.recent_menu.lock().unwrap().clone() else {
        return;
    };
    let recent = st.prefs.lock().unwrap().recent.clone();
    while let Ok(Some(_)) = sub.remove_at(0) {}
    for (i, p) in recent.iter().enumerate() {
        let dir = p
            .parent()
            .and_then(|d| d.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Ok(item) = MenuItemBuilder::with_id(
            format!("recent:{i}"),
            format!("{}  —  {dir}", file_title(p)),
        )
        .build(app)
        {
            let _ = sub.append(&item);
        }
    }
    if !recent.is_empty() {
        if let Ok(sep) = PredefinedMenuItem::separator(app) {
            let _ = sub.append(&sep);
        }
    }
    if let Ok(clear) = MenuItemBuilder::with_id("recent-clear", "목록 지우기")
        .enabled(!recent.is_empty())
        .build(app)
    {
        let _ = sub.append(&clear);
    }
}

// ── 메뉴 ────────────────────────────────────────────────────────────
fn item(app: &AppHandle, id: &str, text: &str, accel: &str) -> tauri::Result<MenuItem<Wry>> {
    MenuItemBuilder::with_id(id, text)
        .accelerator(accel)
        .build(app)
}

#[allow(unused_mut)]
fn build_menu(app: &AppHandle) -> tauri::Result<(Menu<Wry>, Submenu<Wry>)> {
    let recent = SubmenuBuilder::with_id(app, "recent", "최근 사용").build()?;
    let reveal = if cfg!(target_os = "macos") {
        "Finder에서 보기"
    } else if cfg!(windows) {
        "탐색기에서 보기"
    } else {
        "폴더에서 보기"
    };
    let mut file = SubmenuBuilder::new(app, "파일")
        .item(&item(app, "open", "열기…", "CmdOrCtrl+O")?)
        .item(&recent)
        .separator()
        .item(&item(app, "reveal", reveal, "Shift+CmdOrCtrl+R")?)
        .separator()
        .item(&item(app, "close", "닫기", "CmdOrCtrl+W")?)
        .item(&item(app, "print", "프린트…", "CmdOrCtrl+P")?);
    #[cfg(not(target_os = "macos"))]
    {
        file = file
            .separator()
            .item(&item(app, "quit", "끝내기", "CmdOrCtrl+Q")?);
    }
    let edit = SubmenuBuilder::new(app, "편집")
        .item(&PredefinedMenuItem::copy(app, Some("복사"))?)
        .item(&PredefinedMenuItem::select_all(app, Some("전체 선택"))?)
        .separator()
        .item(&item(app, "find", "찾기…", "CmdOrCtrl+F")?)
        .item(&item(app, "find-next", "다음 찾기", "CmdOrCtrl+G")?)
        .item(&item(app, "find-prev", "이전 찾기", "Shift+CmdOrCtrl+G")?)
        .build()?;
    let fullscreen = if cfg!(target_os = "macos") {
        "Ctrl+Cmd+F"
    } else {
        "F11"
    };
    let view = SubmenuBuilder::new(app, "보기")
        .item(&item(app, "outline", "개요 사이드바", "Shift+CmdOrCtrl+L")?)
        .separator()
        .item(&item(app, "zoom-reset", "실제 크기", "CmdOrCtrl+0")?)
        .item(&item(app, "zoom-in", "확대", "CmdOrCtrl+=")?)
        .item(&item(app, "zoom-out", "축소", "CmdOrCtrl+-")?)
        .separator()
        .item(&item(app, "reload", "다시 읽기", "CmdOrCtrl+R")?)
        .item(&item(app, "fullscreen", "전체 화면", fullscreen)?)
        .build()?;

    let mut menu = MenuBuilder::new(app);
    #[cfg(target_os = "macos")]
    {
        let about = tauri::menu::AboutMetadata {
            name: Some("MD Viewer".into()),
            version: Some(app.package_info().version.to_string()),
            copyright: Some("MIT License".into()),
            ..Default::default()
        };
        let app_menu = SubmenuBuilder::new(app, "MD Viewer")
            .item(&PredefinedMenuItem::about(
                app,
                Some("MD Viewer에 관하여"),
                Some(about),
            )?)
            .separator()
            .item(&PredefinedMenuItem::services(app, Some("서비스"))?)
            .separator()
            .item(&PredefinedMenuItem::hide(app, Some("MD Viewer 가리기"))?)
            .item(&PredefinedMenuItem::hide_others(app, Some("기타 가리기"))?)
            .item(&PredefinedMenuItem::show_all(app, Some("모두 보기"))?)
            .separator()
            .item(&PredefinedMenuItem::quit(app, Some("MD Viewer 종료"))?)
            .build()?;
        menu = menu.item(&app_menu);
    }
    menu = menu.item(&file.build()?).item(&edit).item(&view);
    #[cfg(target_os = "macos")]
    {
        let window = SubmenuBuilder::new(app, "윈도우")
            .item(&PredefinedMenuItem::minimize(app, Some("최소화"))?)
            .item(&PredefinedMenuItem::maximize(app, Some("확대/축소"))?)
            .build()?;
        let _ = window.set_as_windows_menu_for_nsapp();
        menu = menu.item(&window);
    }
    Ok((menu.build()?, recent))
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref().to_string();
    let win = focused_window(app);
    let js = |code: &str| {
        if let Some(w) = &win {
            let _ = w.eval(code);
        }
    };
    let doc_path = |w: &WebviewWindow| {
        app.state::<AppState>()
            .docs
            .lock()
            .unwrap()
            .get(w.label())
            .map(|d| d.path.clone())
    };
    match id.as_str() {
        "open" => show_open_dialog(app),
        "reveal" => {
            if let Some(p) = win.as_ref().and_then(doc_path) {
                let _ = app.opener().reveal_item_in_dir(p);
            }
        }
        "close" => {
            if let Some(w) = &win {
                let _ = w.close();
            }
        }
        "print" => {
            if let Some(w) = &win {
                let _ = w.print();
            }
        }
        "quit" => app.exit(0),
        "find" => js("MDV.find.open()"),
        "find-next" => js("MDV.find.next()"),
        "find-prev" => js("MDV.find.prev()"),
        "outline" => js("MDV.toggleOutline()"),
        "reload" => js("MDV.reload(true)"),
        "zoom-in" | "zoom-out" | "zoom-reset" => {
            let st = app.state::<AppState>();
            let zoom = {
                let mut p = st.prefs.lock().unwrap();
                p.zoom = match id.as_str() {
                    "zoom-in" => (p.zoom + 0.1).min(3.0),
                    "zoom-out" => (p.zoom - 0.1).max(0.5),
                    _ => 1.0,
                };
                p.zoom = (p.zoom * 10.0).round() / 10.0;
                p.zoom
            };
            for w in app.webview_windows().values() {
                let _ = w.set_zoom(zoom);
            }
            save_prefs(app);
        }
        "fullscreen" => {
            if let Some(w) = &win {
                let on = w.is_fullscreen().unwrap_or(false);
                let _ = w.set_fullscreen(!on);
            }
        }
        "recent-clear" => {
            app.state::<AppState>().prefs.lock().unwrap().recent.clear();
            save_prefs(app);
            refresh_recent(app);
        }
        s if s.starts_with("recent:") => {
            let idx = s["recent:".len()..].parse::<usize>().unwrap_or(usize::MAX);
            let path = app
                .state::<AppState>()
                .prefs
                .lock()
                .unwrap()
                .recent
                .get(idx)
                .cloned();
            if let Some(p) = path.filter(|p| p.exists()) {
                open_document(app, p, None);
            }
        }
        _ => {}
    }
}

fn on_window_event(window: &Window, event: &WindowEvent) {
    let app = window.app_handle();
    match event {
        WindowEvent::Destroyed => {
            let st = app.state::<AppState>();
            st.docs.lock().unwrap().remove(window.label());
            st.order.lock().unwrap().retain(|l| l != window.label());
        }
        WindowEvent::Resized(size) => {
            let special = window.is_fullscreen().unwrap_or(false)
                || window.is_maximized().unwrap_or(false)
                || window.is_minimized().unwrap_or(false);
            let logical = size.to_logical::<f64>(window.scale_factor().unwrap_or(1.0));
            if !special && logical.width >= 200.0 {
                let st = app.state::<AppState>();
                let mut p = st.prefs.lock().unwrap();
                p.width = logical.width;
                p.height = logical.height;
            }
        }
        WindowEvent::CloseRequested { .. } => save_prefs(app),
        WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
            for p in paths.iter().filter(|p| is_markdown(p)) {
                open_document(app, p.clone(), None);
            }
        }
        _ => {}
    }
}

// ── 화면에서 부르는 명령 ────────────────────────────────────────────
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocPayload {
    text: String,
    title: String,
    doc_url: String,
    anchor: Option<String>,
    outline: bool,
    self_test: bool,
}

/// reload: 감시·메뉴로 다시 읽는 경우(스크롤 유지). force 가 아니면 내용이 같을 때 None.
#[tauri::command]
fn load_document(
    window: WebviewWindow,
    state: State<'_, AppState>,
    reload: bool,
    force: bool,
) -> Result<Option<DocPayload>, String> {
    let label = window.label().to_string();
    let path = state
        .docs
        .lock()
        .unwrap()
        .get(&label)
        .map(|d| d.path.clone())
        .ok_or("문서 정보 없음")?;
    let text = decode(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    let anchor = {
        let mut docs = state.docs.lock().unwrap();
        let doc = docs.get_mut(&label).ok_or("문서 정보 없음")?;
        if reload && !force && doc.last_text.as_deref() == Some(text.as_str()) {
            return Ok(None);
        }
        doc.last_text = Some(text.clone());
        doc.anchor.take()
    };
    let prefs = state.prefs.lock().unwrap().clone();
    if !reload && (prefs.zoom - 1.0).abs() > 0.001 {
        let _ = window.set_zoom(prefs.zoom);
    }
    Ok(Some(DocPayload {
        title: file_title(&path),
        doc_url: Url::from_file_path(&path)
            .map(|u| u.to_string())
            .unwrap_or_default(),
        text,
        anchor,
        outline: prefs.outline_open,
        self_test: state.self_test.is_some(),
    }))
}

/// 문서 안 링크. 웹 주소는 기본 브라우저로, .md 는 새 창으로, 그 밖의 파일은 기본 앱으로.
#[tauri::command]
async fn open_link(app: AppHandle, window: WebviewWindow, url: String) -> Result<(), String> {
    let u = Url::parse(&url).map_err(|e| e.to_string())?;
    match u.scheme() {
        "http" | "https" | "mailto" | "tel" | "ftp" | "ftps" => app
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|e| e.to_string())?,
        "file" => {
            let anchor = u.fragment().filter(|f| !f.is_empty()).map(str::to_string);
            let mut bare = u.clone();
            bare.set_fragment(None);
            bare.set_query(None);
            let path = bare
                .to_file_path()
                .map_err(|_| format!("파일 경로가 아님: {url}"))?;
            let current = app
                .state::<AppState>()
                .docs
                .lock()
                .unwrap()
                .get(window.label())
                .map(|d| d.path.clone());
            if current.is_some_and(|c| same_file(&c, &path)) {
                if let Some(a) = anchor {
                    scroll_to(&window, &a);
                }
            } else if !path.exists() {
                return Err(format!("파일 없음: {}", path.display()));
            } else if is_markdown(&path) {
                open_document(&app, path, anchor);
            } else {
                app.opener()
                    .open_path(path.to_string_lossy(), None::<&str>)
                    .map_err(|e| e.to_string())?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[tauri::command]
fn set_outline(app: AppHandle, state: State<'_, AppState>, open: bool) {
    state.prefs.lock().unwrap().outline_open = open;
    save_prefs(&app);
}

#[tauri::command]
fn log_message(app: AppHandle, message: String) {
    log_line(&app, &message);
}

/// --self-test: 화면이 다 그린 뒤 보내는 점검 결과를 파일로 쓰고 끝낸다 (CI 에서 세 운영체제 확인용).
#[tauri::command]
fn self_test_report(app: AppHandle, state: State<'_, AppState>, report: serde_json::Value) {
    let Some(test) = &state.self_test else { return };
    let _ = fs::write(
        &test.report,
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );
    if test.reported.swap(true, Ordering::SeqCst) {
        return; // 끝내기 예약은 첫 보고 때 한 번만
    }
    let hold = test.hold;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(hold));
        app.exit(0);
    });
}

// ── 기본 앱 지정 (--set-default) ─────────────────────────────────────
// 맥은 프로그램으로 기본 앱을 바꾸지 않는다 (2026-11 macOS 26 실측):
//  - 옛 LSSetDefaultRoleHandlerForContentType 는 성공만 돌려주고 바꾸지 않는다.
//  - NSWorkspace.setDefaultApplication 은 Finder 확인 창("…'MD Viewer'(으)로 열겠습니까")을 띄우는데,
//    요청이 겹치면 창이 여러 개 쌓이고 결과가 들쭉날쭉했다.
// Finder 의 '모두 변경' 이 가장 확실해서 안내만 한다.
#[cfg(target_os = "macos")]
fn set_default_handler() -> i32 {
    println!("맥에서는 Finder 에서 바꿔 주세요: .md 파일 선택 → ⌘I(정보 가져오기) → '다음으로 열기'에서 MD Viewer → '모두 변경…' → '계속'");
    2
}

#[cfg(target_os = "linux")]
fn set_default_handler() -> i32 {
    // deb 가 설치한 .desktop 파일 이름을 찾아 text/markdown 기본 앱으로
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [
        "/usr/share/applications".to_string(),
        format!("{home}/.local/share/applications"),
    ];
    let desktop = dirs
        .iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .find(|e| {
            e.path().extension().is_some_and(|x| x == "desktop")
                && fs::read_to_string(e.path()).is_ok_and(|s| {
                    s.lines()
                        .any(|l| l.starts_with("Exec=md-viewer") || l.contains("/md-viewer"))
                })
        });
    let Some(desktop) = desktop else {
        eprintln!("md-viewer 의 .desktop 파일을 찾지 못했습니다 (deb 로 설치했는지 확인)");
        return 1;
    };
    let name = desktop.file_name().to_string_lossy().into_owned();
    match std::process::Command::new("xdg-mime")
        .args(["default", &name, "text/markdown"])
        .status()
    {
        Ok(s) if s.success() => {
            println!("text/markdown → {name}");
            0
        }
        _ => 1,
    }
}

#[cfg(windows)]
fn set_default_handler() -> i32 {
    // 윈도우는 앱이 기본 앱을 직접 바꿀 수 없다. 설정 › 기본 앱 화면을 연다.
    let _ = std::process::Command::new("explorer")
        .arg("ms-settings:defaultapps")
        .spawn();
    0
}

// ── 시작 ────────────────────────────────────────────────────────────
fn main() {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut cli = parse_cli(std::env::args().skip(1), &cwd);
    if cli.set_default {
        std::process::exit(set_default_handler());
    }

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // 윈도우·리눅스는 파일을 더블클릭할 때마다 새 프로세스가 뜨므로 이미 떠 있는 쪽으로 넘긴다
    #[cfg(not(target_os = "macos"))]
    if cli.self_test.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let more = parse_cli(args.into_iter().skip(1), Path::new(&cwd));
            if more.files.is_empty() {
                match focused_window(app) {
                    Some(w) => {
                        let _ = w.set_focus();
                    }
                    None => show_open_dialog(app),
                }
            }
            for f in more.files {
                open_document(app, f, None);
            }
        }));
    }

    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            load_document,
            open_link,
            set_outline,
            log_message,
            self_test_report
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let prefs_path = app.path().app_config_dir()?.join("prefs.json");
            let prefs: Prefs = fs::read(&prefs_path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let self_test = cli.self_test.take();
            let test_file = self_test
                .as_ref()
                .map(|t| (t.file.clone(), t.report.clone()));
            app.manage(AppState {
                docs: Mutex::new(HashMap::new()),
                order: Mutex::new(vec![]),
                counter: AtomicUsize::new(0),
                prefs: Mutex::new(prefs),
                prefs_path,
                log_path: app.path().app_log_dir()?.join("MDViewer.log"),
                recent_menu: Mutex::new(None),
                self_test,
            });
            let (menu, recent) = build_menu(&handle)?;
            handle.set_menu(menu)?;
            *app.state::<AppState>().recent_menu.lock().unwrap() = Some(recent);
            refresh_recent(&handle);

            if let Some((file, report)) = test_file {
                open_document(&handle, file, None);
                // 화면이 끝내 보고하지 않으면 시간 초과로 실패 처리
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(90));
                    let _ = fs::write(&report, r#"{"timeout": true}"#);
                    h.exit(2);
                });
            } else if !cli.files.is_empty() {
                for f in std::mem::take(&mut cli.files) {
                    open_document(&handle, f, None);
                }
            } else {
                // 맥은 Finder 에서 연 파일이 조금 뒤 Opened 이벤트로 온다. 그때까지 아무것도 없으면 열기 창.
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(700));
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || {
                        if h2.state::<AppState>().docs.lock().unwrap().is_empty() {
                            show_open_dialog(&h2);
                        }
                    });
                });
            }
            Ok(())
        })
        .on_menu_event(on_menu)
        .on_window_event(on_window_event)
        .build(tauri::generate_context!())
        .expect("MD Viewer 를 시작하지 못했습니다");

    app.run(|app, event| match event {
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => {
            for u in urls {
                if let Ok(p) = u.to_file_path() {
                    open_document(app, p, None);
                }
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => show_open_dialog(app),
        // 맥은 마지막 창을 닫아도 앱이 남는다 (일반 맥 앱처럼)
        #[cfg(target_os = "macos")]
        RunEvent::ExitRequested {
            code: None, api, ..
        } if app.state::<AppState>().self_test.is_none() => api.prevent_exit(),
        _ => {}
    });
}
