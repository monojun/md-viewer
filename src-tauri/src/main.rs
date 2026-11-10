// MD Viewer 앱 껍데기 (Tauri — 맥·윈도우·리눅스). 창 하나에 문서 칸(pane)을 1~3개 나란히 둔다.
// 화면은 dist/viewer.html (web/ 를 묶은 것)이고, 이 파일은 파일 읽기·감시, 창·칸·메뉴, 링크 열기를 맡는다.
// 화면 쪽과는 두 길로 주고받는다: 화면 → 여기 = invoke 명령(load_document 등), 여기 → 화면 = eval("MDV.…").
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::menu::{
    CheckMenuItem, CheckMenuItemBuilder, Menu, MenuBuilder, MenuEvent, MenuItem, MenuItemBuilder,
    PredefinedMenuItem, Submenu, SubmenuBuilder,
};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, State, Url, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, Window, WindowEvent, Wry,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

const MD_EXTS: &[&str] = &["md", "markdown", "mdown", "mkd", "mkdn", "mdtext", "txt"];
const RECENT_MAX: usize = 10;
const MAX_PANES: usize = 3;

// 맥은 Finder 에서 더블클릭한 파일을 앱 준비(setup)보다 먼저 Opened 이벤트로 보낸다.
// 그때는 AppState 가 아직 없으므로 여기 모아 두었다가 setup 끝에서 연다.
#[cfg(target_os = "macos")]
static PENDING_OPEN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

#[derive(Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "camelCase")]
struct Prefs {
    outline_open: bool,
    sync_scroll: bool,
    zoom: f64,
    width: f64, // 칸 하나의 너비. 칸이 여럿이면 창은 그 배수로 연다
    height: f64,
    recent: Vec<PathBuf>,
    // 편집 UX: "toggle"(한 번에 하나), "pane"(편집·보기 칸을 함께). 사용자가 설정에서 바꾼다.
    edit_ui_mode: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            outline_open: false,
            sync_scroll: false,
            zoom: 1.0,
            width: 900.0,
            height: 1000.0,
            recent: vec![],
            edit_ui_mode: "toggle".into(),
        }
    }
}

// 저장 시 원문 바이트와 같게 되도록 기억해 두는 메타 정보.
// 인코딩(UTF-8·UTF-16LE·UTF-16BE·EUC-KR), BOM 여부, 줄바꿈(CRLF 또는 LF).
#[derive(Clone, Debug)]
struct DocMeta {
    encoding: &'static encoding_rs::Encoding,
    has_bom: bool,
    crlf: bool,
}

struct Pane {
    id: u32, // 앱 전체에서 하나뿐인 번호. 화면 쪽도 이 번호로 칸을 가리킨다
    path: PathBuf,
    anchor: Option<String>,
    last_text: Option<String>, // 감시가 같은 내용으로 여러 번 울려도 다시 그리지 않게
    meta: Option<DocMeta>,     // 저장 시 인코딩·줄바꿈을 원본대로 되돌리려고
    dirty: bool,               // 편집 중 저장 안 된 변경. 창 제목의 '•' 과 외부 변경 확인 바에 쓴다.
    #[allow(dead_code)]
    watcher: Option<notify::RecommendedWatcher>, // 칸이 닫혀 Pane 이 지워지면 감시도 끝난다
}

struct Win {
    panes: Vec<Pane>, // 왼쪽부터
    active: u32,      // 메뉴(닫기·폴더에서 보기 등)가 가리키는 칸
}

struct SelfTest {
    file: PathBuf,
    report: PathBuf,
    hold: u64,
    scenario: bool,  // 칸 닫기·끌어다 놓기·새 창으로 떼기를 차례로 해 본다
    roundtrip: bool, // 바이트 왕복(읽기 → 바로 쓰기) 이 같은지 확인한다
    reported: AtomicBool,
}

// 잠금 규칙: wins 를 잠근 채로 창 크기·위치를 묻거나 바꾸지 않는다.
// (명령은 다른 스레드에서 돌고, 창 조작은 메인 스레드를 기다리는데 메인 스레드의 창 이벤트도 wins 를 잠근다)
struct AppState {
    wins: Mutex<HashMap<String, Win>>, // 창 label → 칸들
    order: Mutex<Vec<String>>,         // 열린 순서 (새 창 계단 배치·메뉴 대상 찾기)
    counter: AtomicUsize,
    pane_counter: AtomicU32,
    prefs: Mutex<Prefs>,
    prefs_path: PathBuf,
    log_path: PathBuf,
    recent_menu: Mutex<Option<Submenu<Wry>>>,
    sync_item: Mutex<Option<CheckMenuItem<Wry>>>,
    self_test: Option<SelfTest>,
}

// ── 명령줄 ──────────────────────────────────────────────────────────
// 파일 경로(윈도우·리눅스 파일 연결), --set-default,
// --self-test <md> --report <json> [--hold 초] [--scenario] [나란히 둘 md…]
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
    let (mut test_file, mut report, mut hold, mut scenario, mut roundtrip) =
        (None, None, 0, false, false);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--self-test" => test_file = it.next().map(|f| cwd.join(f)),
            "--report" => report = it.next().map(|f| cwd.join(f)),
            "--hold" => hold = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--scenario" => scenario = true,
            "--roundtrip" => roundtrip = true,
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
            scenario,
            roundtrip,
            reported: AtomicBool::new(false),
        });
    }
    cli
}

// ── 파일 ────────────────────────────────────────────────────────────
// 바이트에서 문자열과 저장용 메타를 함께 뽑는다. 문자열은 LF 로 정리해 화면(JS)에 넘기고,
// 저장할 때 encode_full 이 메타를 보고 원본 꼴(BOM·CRLF·인코딩)로 되돌린다.
fn decode_full(bytes: &[u8]) -> (String, DocMeta) {
    let (raw, encoding, has_bom) = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        (
            String::from_utf8_lossy(rest).into_owned(),
            encoding_rs::UTF_8,
            true,
        )
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        (
            encoding_rs::UTF_16LE
                .decode_with_bom_removal(bytes)
                .0
                .into_owned(),
            encoding_rs::UTF_16LE,
            true,
        )
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        (
            encoding_rs::UTF_16BE
                .decode_with_bom_removal(bytes)
                .0
                .into_owned(),
            encoding_rs::UTF_16BE,
            true,
        )
    } else if let Ok(s) = std::str::from_utf8(bytes) {
        (s.to_owned(), encoding_rs::UTF_8, false)
    } else {
        // CP949 (옛 한글 문서)
        (
            encoding_rs::EUC_KR.decode(bytes).0.into_owned(),
            encoding_rs::EUC_KR,
            false,
        )
    };
    // 첫 LF 바로 앞이 CR 이면 CRLF 로 본다 (혼합 문서는 드물고, 왕복 보존을 쉽게)
    let crlf = raw
        .as_bytes()
        .windows(2)
        .find(|w| w[1] == b'\n')
        .is_some_and(|w| w[0] == b'\r');
    let normalized = if crlf { raw.replace("\r\n", "\n") } else { raw };
    (
        normalized,
        DocMeta {
            encoding,
            has_bom,
            crlf,
        },
    )
}

// 문자열 + 메타 → 저장용 바이트. decode_full 이 벗겨 둔 BOM·CRLF 를 다시 붙이고 원래 인코딩으로 바꾼다.
fn encode_full(text: &str, meta: &DocMeta) -> Vec<u8> {
    let with_crlf;
    let src: &str = if meta.crlf {
        with_crlf = text.replace('\n', "\r\n");
        &with_crlf
    } else {
        text
    };
    let mut out = Vec::new();
    if meta.has_bom {
        match meta.encoding.name() {
            "UTF-8" => out.extend_from_slice(&[0xEF, 0xBB, 0xBF]),
            "UTF-16LE" => out.extend_from_slice(&[0xFF, 0xFE]),
            "UTF-16BE" => out.extend_from_slice(&[0xFE, 0xFF]),
            _ => {}
        }
    }
    // encoding_rs 는 UTF-16 을 '내보내기 인코딩' 으로는 쓰지 않으므로(WHATWG: UTF-8 로 돌림) 직접 짠다.
    match meta.encoding.name() {
        "UTF-16LE" => {
            for u in src.encode_utf16() {
                out.push((u & 0xff) as u8);
                out.push((u >> 8) as u8);
            }
        }
        "UTF-16BE" => {
            for u in src.encode_utf16() {
                out.push((u >> 8) as u8);
                out.push((u & 0xff) as u8);
            }
        }
        _ => {
            let (encoded, _, _) = meta.encoding.encode(src);
            out.extend_from_slice(&encoded);
        }
    }
    out
}

// 테스트용: 파일 전체 왕복(읽기 → 바로 쓰기) 이 같은 바이트인지 검사한다. --roundtrip 과 자체 점검에서 쓴다.
fn check_roundtrip(path: &Path) -> std::io::Result<(bool, usize)> {
    let bytes = fs::read(path)?;
    let (text, meta) = decode_full(&bytes);
    let encoded = encode_full(&text, &meta);
    Ok((encoded == bytes, bytes.len()))
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

fn canonical(p: PathBuf) -> PathBuf {
    fs::canonicalize(&p).unwrap_or(p)
}

fn file_title(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "MD Viewer".into())
}

fn json<T: Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

// 편집기들은 임시 파일에 쓰고 이름을 바꿔 저장하므로 파일 대신 폴더를 보고 이름으로 거른다.
fn watch_file(
    app: &AppHandle,
    label: &str,
    pane: u32,
    path: &Path,
) -> Option<notify::RecommendedWatcher> {
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
                let _ = w.eval(format!("MDV.changed({pane})"));
            }
        }
    })
    .ok()?;
    watcher
        .watch(path.parent()?, RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

fn new_pane(app: &AppHandle, label: &str, path: PathBuf, anchor: Option<String>) -> Pane {
    let id = app
        .state::<AppState>()
        .pane_counter
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    Pane {
        id,
        watcher: watch_file(app, label, id, &path),
        path,
        anchor,
        last_text: None,
        meta: None,
        dirty: false,
    }
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

fn titles_of(win: &Win) -> String {
    let base = win
        .panes
        .iter()
        .map(|p| file_title(&p.path))
        .collect::<Vec<_>>()
        .join("  ·  ");
    if win.panes.iter().any(|p| p.dirty) {
        format!("• {base}")
    } else {
        base
    }
}

fn update_title(app: &AppHandle, label: &str) {
    let title = {
        let st = app.state::<AppState>();
        let wins = st.wins.lock().unwrap();
        wins.get(label).map(titles_of)
    };
    if let (Some(t), Some(w)) = (title, app.get_webview_window(label)) {
        let _ = w.set_title(&t);
    }
}

// 창 너비를 바꾸되 화면(작업 영역) 밖으로 나가지 않게 줄이거나 왼쪽으로 옮긴다.
fn set_width_on_screen(w: &WebviewWindow, width: f64) {
    let Ok(sf) = w.scale_factor() else { return };
    let (Ok(inner), Ok(outer), Ok(pos)) = (w.inner_size(), w.outer_size(), w.outer_position())
    else {
        return;
    };
    let (inner, outer, pos) = (
        inner.to_logical::<f64>(sf),
        outer.to_logical::<f64>(sf),
        pos.to_logical::<f64>(sf),
    );
    let frame = (outer.width - inner.width).max(0.0);
    let mut width = width.max(420.0);
    if let Ok(Some(m)) = w.current_monitor() {
        let area = m.work_area();
        let (ax, aw) = (
            f64::from(area.position.x) / sf,
            f64::from(area.size.width) / sf,
        );
        width = width.min(aw - frame);
        if pos.x + width + frame > ax + aw {
            let x = (ax + aw - width - frame).max(ax);
            let _ = w.set_position(LogicalPosition::new(x, pos.y));
        }
    }
    let _ = w.set_size(LogicalSize::new(width, inner.height));
}

// 칸 수가 바뀌면 칸 하나의 너비는 그대로 두고 창을 늘이거나 줄인다 (최대화·전체 화면이면 그대로).
fn resize_for_panes(w: &WebviewWindow, from: usize, to: usize) {
    if from == 0
        || from == to
        || w.is_fullscreen().unwrap_or(false)
        || w.is_maximized().unwrap_or(false)
    {
        return;
    }
    let Ok(sf) = w.scale_factor() else { return };
    let Ok(inner) = w.inner_size() else { return };
    let width = inner.to_logical::<f64>(sf).width;
    set_width_on_screen(w, width / from as f64 * to as f64);
}

fn find_pane(app: &AppHandle, path: &Path) -> Option<(String, u32)> {
    let st = app.state::<AppState>();
    let wins = st.wins.lock().unwrap();
    wins.iter().find_map(|(label, win)| {
        win.panes
            .iter()
            .find(|p| p.path == path)
            .map(|p| (label.clone(), p.id))
    })
}

fn show_pane(app: &AppHandle, label: &str, pane: u32, anchor: Option<String>) {
    let Some(w) = app.get_webview_window(label) else {
        return;
    };
    let _ = w.unminimize();
    let _ = w.set_focus();
    let mut js = format!("MDV.focusPane({pane});");
    if let Some(a) = anchor {
        js += &format!("MDV.goto({pane},{});", json(&a));
    }
    let _ = w.eval(js);
}

/// 문서 하나를 새 창으로. 이미 어느 칸에 열려 있으면 그 칸을 앞으로.
fn open_document(app: &AppHandle, path: PathBuf, anchor: Option<String>) {
    let path = canonical(path);
    if let Some((label, pane)) = find_pane(app, &path) {
        show_pane(app, &label, pane, anchor);
        return;
    }
    open_window(app, vec![(path, anchor)]);
}

/// 한꺼번에 연 문서들 (Finder 에서 여럿 골라 열기, 열기 창에서 여럿 고르기).
/// 2~3개면 한 창에 나란히, 하나이거나 4개 이상이면 각각 새 창. 이미 열린 문서는 그 칸을 앞으로.
fn open_documents(app: &AppHandle, paths: Vec<PathBuf>) {
    let mut fresh: Vec<PathBuf> = vec![];
    for p in paths.into_iter().map(canonical) {
        if let Some((label, pane)) = find_pane(app, &p) {
            show_pane(app, &label, pane, None);
        } else if !fresh.contains(&p) {
            fresh.push(p);
        }
    }
    if (2..=MAX_PANES).contains(&fresh.len()) {
        open_window(app, fresh.into_iter().map(|p| (p, None)).collect());
    } else {
        for p in fresh {
            open_window(app, vec![(p, None)]);
        }
    }
}

/// 문서들을 칸으로 나란히 둔 새 창.
fn open_window(app: &AppHandle, docs: Vec<(PathBuf, Option<String>)>) {
    if docs.is_empty() {
        return;
    }
    let st = app.state::<AppState>();
    let label = format!("doc{}", st.counter.fetch_add(1, Ordering::SeqCst) + 1);
    let panes: Vec<Pane> = docs
        .into_iter()
        .take(MAX_PANES)
        .map(|(p, a)| new_pane(app, &label, canonical(p), a))
        .collect();
    let count = panes.len();
    let (width, height) = {
        let p = st.prefs.lock().unwrap();
        (p.width * count as f64, p.height)
    };
    let win = Win {
        active: panes[0].id,
        panes,
    };
    let title = titles_of(&win);
    let paths: Vec<PathBuf> = win.panes.iter().map(|p| p.path.clone()).collect();

    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("viewer.html".into()))
        .title(title)
        .inner_size(width, height)
        .min_inner_size(420.0, 300.0)
        .on_navigation(is_app_url)
        // 자체 점검(CI)에서는 화면 캡처에 다른 창이 끼지 않게 맨 앞에 띄운다
        .always_on_top(st.self_test.is_some());
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

    // 화면이 곧바로 get_layout 을 부르므로 창보다 먼저 등록한다
    st.wins.lock().unwrap().insert(label.clone(), win);
    match builder.build() {
        Ok(w) => {
            st.order.lock().unwrap().push(label.clone());
            if count > 1 {
                set_width_on_screen(&w, width);
            }
            for p in paths {
                remember_recent(app, &p);
            }
        }
        Err(e) => {
            st.wins.lock().unwrap().remove(&label);
            log_line(app, &format!("창 열기 실패: {e}"));
        }
    }
}

/// 창에 칸을 더한다. at: 끼워 넣을 자리(없으면 오른쪽 끝), replace: 이 칸을 바꿔 끼운다(칸이 꽉 찼을 때 끌어다 놓기).
/// 이 창에 이미 있는 문서는 그 칸을 앞으로만. 자리가 모자라면 남는 문서는 새 창으로.
fn add_panes(
    app: &AppHandle,
    label: &str,
    paths: Vec<PathBuf>,
    at: Option<usize>,
    replace: Option<u32>,
) {
    let Some(w) = app.get_webview_window(label) else {
        for p in paths {
            open_document(app, p, None);
        }
        return;
    };
    let st = app.state::<AppState>();
    let mut js = String::new();
    let (mut opened, mut extra) = (vec![], vec![]);
    let (before, after) = {
        let mut wins = st.wins.lock().unwrap();
        let Some(win) = wins.get_mut(label) else {
            return;
        };
        let before = win.panes.len();
        let mut at = at.unwrap_or(before).min(before);
        let mut replace = replace.filter(|id| win.panes.iter().any(|p| p.id == *id));
        for path in paths.into_iter().map(canonical) {
            if let Some(p) = win.panes.iter().find(|p| p.path == path) {
                win.active = p.id;
                continue;
            }
            if let Some(old) = replace.take() {
                let i = win.panes.iter().position(|p| p.id == old).unwrap_or(0);
                let pane = new_pane(app, label, path.clone(), None);
                js += &format!("MDV.replacePane({old},{});", pane.id);
                win.active = pane.id;
                win.panes[i] = pane; // 옛 칸의 감시도 여기서 끝난다
                opened.push(path);
            } else if win.panes.len() < MAX_PANES {
                let pane = new_pane(app, label, path.clone(), None);
                js += &format!("MDV.addPane({},{at});", pane.id);
                win.active = pane.id;
                win.panes.insert(at, pane);
                at += 1;
                opened.push(path);
            } else {
                extra.push(path);
            }
        }
        js += &format!("MDV.focusPane({});", win.active);
        (before, win.panes.len())
    };
    let _ = w.eval(js);
    update_title(app, label);
    resize_for_panes(&w, before, after);
    let _ = w.set_focus();
    for p in opened {
        remember_recent(app, &p);
    }
    for p in extra {
        open_document(app, p, None);
    }
}

/// 칸 하나를 닫는다. 마지막 칸이면 창을 닫는다. 닫힌 칸의 문서 경로를 돌려준다.
fn remove_pane(app: &AppHandle, label: &str, id: u32) -> Option<PathBuf> {
    let st = app.state::<AppState>();
    let (path, before, left, active) = {
        let mut wins = st.wins.lock().unwrap();
        let win = wins.get_mut(label)?;
        let i = win.panes.iter().position(|p| p.id == id)?;
        let before = win.panes.len();
        let path = win.panes.remove(i).path;
        if let Some(next) = win.panes.get(i.min(win.panes.len().saturating_sub(1))) {
            if win.active == id {
                win.active = next.id;
            }
        }
        (path, before, win.panes.len(), win.active)
    };
    let w = app.get_webview_window(label)?;
    if left == 0 {
        let _ = w.close();
    } else {
        let _ = w.eval(format!("MDV.removePane({id});MDV.focusPane({active});"));
        update_title(app, label);
        resize_for_panes(&w, before, left);
    }
    Some(path)
}

/// 다른 창들의 문서를 이 창으로 모은다 (칸이 다 찰 때까지, 최근 창부터).
fn merge_windows(app: &AppHandle, target: &str) {
    let st = app.state::<AppState>();
    let order = st.order.lock().unwrap().clone();
    let moving: Vec<(String, u32)> = {
        let wins = st.wins.lock().unwrap();
        let room = MAX_PANES.saturating_sub(wins.get(target).map_or(MAX_PANES, |w| w.panes.len()));
        order
            .iter()
            .rev()
            .filter(|l| l.as_str() != target)
            .filter_map(|l| wins.get(l).map(|w| (l, w)))
            .flat_map(|(l, w)| w.panes.iter().map(move |p| (l.clone(), p.id)))
            .take(room)
            .collect()
    };
    let paths: Vec<PathBuf> = moving
        .into_iter()
        .filter_map(|(l, id)| remove_pane(app, &l, id))
        .collect();
    add_panes(app, target, paths, None, None);
}

/// 칸 하나를 떼어 새 창으로.
fn pop_out(app: &AppHandle, label: &str, id: u32) {
    let count = {
        let st = app.state::<AppState>();
        let wins = st.wins.lock().unwrap();
        wins.get(label).map_or(0, |w| w.panes.len())
    };
    if count < 2 {
        return;
    }
    if let Some(path) = remove_pane(app, label, id) {
        open_window(app, vec![(path, None)]);
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

/// 열기 창. beside 에 창 label 을 주면 고른 문서를 그 창에 칸으로 더한다.
fn show_open_dialog(app: &AppHandle, beside: Option<String>) {
    let handle = app.clone();
    app.dialog()
        .file()
        .set_title(if beside.is_some() {
            "옆에 열기"
        } else {
            "마크다운 열기"
        })
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
                if handle.state::<AppState>().wins.lock().unwrap().is_empty() {
                    handle.exit(0);
                }
                return;
            }
            match beside {
                Some(label) => add_panes(&handle, &label, paths, None, None),
                None => open_documents(&handle, paths),
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

fn item_noacc(app: &AppHandle, id: &str, text: &str) -> tauri::Result<MenuItem<Wry>> {
    MenuItemBuilder::with_id(id, text).build(app)
}

struct AppMenu {
    menu: Menu<Wry>,
    recent: Submenu<Wry>,
    sync: CheckMenuItem<Wry>,
}

fn build_menu(app: &AppHandle, sync_on: bool) -> tauri::Result<AppMenu> {
    let recent = SubmenuBuilder::with_id(app, "recent", "최근 사용").build()?;
    let reveal = if cfg!(target_os = "macos") {
        "Finder에서 보기"
    } else if cfg!(windows) {
        "탐색기에서 보기"
    } else {
        "폴더에서 보기"
    };
    #[allow(unused_mut)]
    let mut file = SubmenuBuilder::new(app, "파일")
        .item(&item(app, "open", "열기…", "CmdOrCtrl+O")?)
        .item(&item(app, "open-beside", "옆에 열기…", "Alt+CmdOrCtrl+O")?)
        .item(&recent)
        .separator()
        .item(&item(app, "save", "저장", "CmdOrCtrl+S")?)
        .item(&item(app, "save-as", "다른 이름으로 저장…", "Shift+CmdOrCtrl+S")?)
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
    let sync = CheckMenuItemBuilder::with_id("sync-scroll", "나란히 스크롤")
        .checked(sync_on)
        .build(app)?;
    let view = SubmenuBuilder::new(app, "보기")
        .item(&item(app, "outline", "개요 사이드바", "Shift+CmdOrCtrl+L")?)
        .item(&sync)
        .separator()
        .item(&item(app, "edit-toggle", "편집 모드", "CmdOrCtrl+E")?)
        .item(&item(app, "open-edit-pane", "편집 칸으로 열기", "Alt+CmdOrCtrl+E")?)
        .item(&item_noacc(app, "ui-toggle", "편집 UX: 토글 모드")?)
        .item(&item_noacc(app, "ui-pane", "편집 UX: 칸 모드")?)
        .separator()
        .item(&item(app, "zoom-reset", "실제 크기", "CmdOrCtrl+0")?)
        .item(&item(app, "zoom-in", "확대", "CmdOrCtrl+=")?)
        .item(&item(app, "zoom-out", "축소", "CmdOrCtrl+-")?)
        .separator()
        .item(&item(app, "reload", "다시 읽기", "CmdOrCtrl+R")?)
        .item(&item(app, "fullscreen", "전체 화면", fullscreen)?)
        .build()?;
    let merge = MenuItemBuilder::with_id("merge", "창 합치기 (나란히 보기)").build(app)?;
    let pop = MenuItemBuilder::with_id("pop-out", "이 칸을 새 창으로").build(app)?;

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
        // 이 id 로 만들어야 Tauri 가 메뉴 막대를 붙인 뒤 '윈도우 메뉴'로 등록한다. 그래야 macOS 가
        // '이동 및 크기 조절'(fn+control+화살표 타일 배치) 항목을 이 메뉴에 넣어 단축키가 동작한다.
        // (메뉴 막대를 붙이기 전에 set_as_windows_menu_for_nsapp 를 부르면 아무 일도 일어나지 않는다)
        let window = SubmenuBuilder::with_id(app, tauri::menu::WINDOW_SUBMENU_ID, "윈도우")
            .item(&PredefinedMenuItem::minimize(app, Some("최소화"))?)
            .item(&PredefinedMenuItem::maximize(app, Some("확대/축소"))?)
            .separator()
            .item(&merge)
            .item(&pop)
            .build()?;
        menu = menu.item(&window);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let window = SubmenuBuilder::new(app, "창")
            .item(&merge)
            .item(&pop)
            .build()?;
        menu = menu.item(&window);
    }
    Ok(AppMenu {
        menu: menu.build()?,
        recent,
        sync,
    })
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref().to_string();
    let win = focused_window(app);
    let js = |code: &str| {
        if let Some(w) = &win {
            let _ = w.eval(code);
        }
    };
    // 이 창의 (칸 수, 지금 칸 번호, 지금 칸 문서)
    let current = |w: &WebviewWindow| {
        let st = app.state::<AppState>();
        let wins = st.wins.lock().unwrap();
        let win = wins.get(w.label())?;
        let pane = win.panes.iter().find(|p| p.id == win.active)?;
        Some((win.panes.len(), pane.id, pane.path.clone()))
    };
    match id.as_str() {
        "open" => show_open_dialog(app, None),
        "open-beside" => show_open_dialog(app, win.as_ref().map(|w| w.label().to_string())),
        "reveal" => {
            if let Some((_, _, p)) = win.as_ref().and_then(current) {
                let _ = app.opener().reveal_item_in_dir(p);
            }
        }
        "close" => {
            if let Some(w) = &win {
                match current(w) {
                    Some((n, pane, _)) if n > 1 => {
                        remove_pane(app, w.label(), pane);
                    }
                    _ => {
                        let _ = w.close();
                    }
                }
            }
        }
        "print" => {
            if let Some(w) = &win {
                let _ = w.print();
            }
        }
        "quit" => app.exit(0),
        "save" => js("MDV.save()"),
        "save-as" => js("MDV.saveAs()"),
        "edit-toggle" => js("MDV.toggleEdit()"),
        "open-edit-pane" => js("MDV.openEditPane()"),
        "ui-toggle" => set_edit_ui_mode(app.clone(), app.state::<AppState>(), "toggle".into()),
        "ui-pane" => set_edit_ui_mode(app.clone(), app.state::<AppState>(), "pane".into()),
        "find" => js("MDV.find.open()"),
        "find-next" => js("MDV.find.next()"),
        "find-prev" => js("MDV.find.prev()"),
        "outline" => js("MDV.toggleOutline()"),
        "reload" => js("MDV.reload(true)"),
        "merge" => {
            if let Some(w) = &win {
                merge_windows(app, w.label());
            }
        }
        "pop-out" => {
            if let Some(w) = &win {
                if let Some((_, pane, _)) = current(w) {
                    pop_out(app, w.label(), pane);
                }
            }
        }
        "sync-scroll" => {
            let st = app.state::<AppState>();
            let on = {
                let mut p = st.prefs.lock().unwrap();
                p.sync_scroll = !p.sync_scroll;
                p.sync_scroll
            };
            if let Some(item) = st.sync_item.lock().unwrap().as_ref() {
                let _ = item.set_checked(on);
            }
            for w in app.webview_windows().values() {
                let _ = w.eval(format!("MDV.setSync({on})"));
            }
            save_prefs(app);
        }
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

// 파일을 끌어 오는 동안 화면에 놓일 자리를 보여 주고, 놓으면 화면이 고른 자리로 drop_files 를 부른다.
fn on_drag(window: &Window, event: &tauri::DragDropEvent) {
    use tauri::DragDropEvent as D;
    let app = window.app_handle();
    let Some(w) = app.get_webview_window(window.label()) else {
        return;
    };
    // 화면 좌표(CSS px) = 창 좌표 ÷ 확대 배율. 이름은 PhysicalPosition 이지만 실제 단위는 운영체제마다 다르다
    // (wry 0.57: 맥·리눅스는 이미 논리 좌표, 윈도우만 물리 픽셀이라 화면 배율로 한 번 더 나눈다)
    let x = |p: &tauri::PhysicalPosition<f64>| {
        let sf = if cfg!(windows) {
            window.scale_factor().unwrap_or(1.0)
        } else {
            1.0
        };
        let zoom = app.state::<AppState>().prefs.lock().unwrap().zoom;
        p.x / sf / zoom
    };
    let md = |paths: &[PathBuf]| -> Vec<PathBuf> {
        paths.iter().filter(|p| is_markdown(p)).cloned().collect()
    };
    let code = match event {
        D::Enter { paths, position } => {
            let names: Vec<String> = md(paths).iter().map(|p| file_title(p)).collect();
            format!("MDV.drag.enter({},{})", json(&names), x(position))
        }
        D::Over { position } => format!("MDV.drag.over({})", x(position)),
        D::Drop { paths, position } => {
            format!("MDV.drag.drop({},{})", json(&md(paths)), x(position))
        }
        D::Leave => "MDV.drag.leave()".into(),
        _ => return,
    };
    let _ = w.eval(code);
}

fn on_window_event(window: &Window, event: &WindowEvent) {
    let app = window.app_handle();
    match event {
        WindowEvent::Destroyed => {
            let st = app.state::<AppState>();
            st.wins.lock().unwrap().remove(window.label());
            st.order.lock().unwrap().retain(|l| l != window.label());
        }
        WindowEvent::Resized(size) => {
            let special = window.is_fullscreen().unwrap_or(false)
                || window.is_maximized().unwrap_or(false)
                || window.is_minimized().unwrap_or(false);
            let logical = size.to_logical::<f64>(window.scale_factor().unwrap_or(1.0));
            if !special && logical.width >= 200.0 {
                let st = app.state::<AppState>();
                let count = st
                    .wins
                    .lock()
                    .unwrap()
                    .get(window.label())
                    .map_or(1, |w| w.panes.len().max(1));
                let mut p = st.prefs.lock().unwrap();
                p.width = logical.width / count as f64;
                p.height = logical.height;
            }
        }
        WindowEvent::CloseRequested { .. } => save_prefs(app),
        WindowEvent::DragDrop(ev) => on_drag(window, ev),
        _ => {}
    }
}

// ── 화면에서 부르는 명령 ────────────────────────────────────────────
// 창을 만들거나 닫을 수 있는 명령은 async 로 둔다 (윈도우에서 동기 명령 안에서 창을 만들면 멈춘다).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Layout {
    panes: Vec<u32>,
    active: u32,
    outline: bool,
    sync: bool,
    edit_ui_mode: String,
    self_test: bool, // 이 창이 점검 결과를 보낸다 (자체 점검의 첫 창만)
    scenario: bool,
    roundtrip: Option<String>, // --self-test --roundtrip 모드일 때 검사할 파일 경로
}

#[tauri::command]
fn get_layout(window: WebviewWindow, state: State<'_, AppState>) -> Result<Layout, String> {
    let (panes, active) = {
        let wins = state.wins.lock().unwrap();
        let win = wins.get(window.label()).ok_or("창 정보 없음")?;
        (win.panes.iter().map(|p| p.id).collect(), win.active)
    };
    let prefs = state.prefs.lock().unwrap().clone();
    if (prefs.zoom - 1.0).abs() > 0.001 {
        let _ = window.set_zoom(prefs.zoom);
    }
    let roundtrip = state.self_test.as_ref().and_then(|t| {
        if t.roundtrip {
            Some(t.file.to_string_lossy().into_owned())
        } else {
            None
        }
    });
    Ok(Layout {
        panes,
        active,
        outline: prefs.outline_open,
        sync: prefs.sync_scroll,
        edit_ui_mode: prefs.edit_ui_mode,
        self_test: state.self_test.is_some() && window.label() == "doc1",
        scenario: state.self_test.as_ref().is_some_and(|t| t.scenario),
        roundtrip,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocPayload {
    text: String,
    title: String,
    dir: String,
    path: String,
    doc_url: String,
    anchor: Option<String>,
}

/// reload: 감시·메뉴로 다시 읽는 경우(스크롤 유지). force 가 아니면 내용이 같을 때 None.
#[tauri::command]
fn load_document(
    window: WebviewWindow,
    state: State<'_, AppState>,
    pane: u32,
    reload: bool,
    force: bool,
) -> Result<Option<DocPayload>, String> {
    let label = window.label();
    let path = {
        let wins = state.wins.lock().unwrap();
        wins.get(label)
            .and_then(|w| w.panes.iter().find(|p| p.id == pane))
            .map(|p| p.path.clone())
            .ok_or("문서 정보 없음")?
    };
    let (text, meta) =
        decode_full(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    let anchor = {
        let mut wins = state.wins.lock().unwrap();
        let p = wins
            .get_mut(label)
            .and_then(|w| w.panes.iter_mut().find(|p| p.id == pane))
            .ok_or("문서 정보 없음")?;
        if reload && !force && p.last_text.as_deref() == Some(text.as_str()) {
            return Ok(None);
        }
        p.last_text = Some(text.clone());
        p.meta = Some(meta);
        // 디스크와 같은 내용을 다시 읽었으므로 더 이상 변경 중이 아니다
        p.dirty = false;
        p.anchor.take()
    };
    Ok(Some(DocPayload {
        title: file_title(&path),
        dir: path
            .parent()
            .and_then(|d| d.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_string_lossy().into_owned(),
        doc_url: Url::from_file_path(&path)
            .map(|u| u.to_string())
            .unwrap_or_default(),
        text,
        anchor,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveResult {
    path: String,
    bytes: usize,
}

fn do_save(
    app: &AppHandle,
    label: &str,
    pane: u32,
    path: Option<PathBuf>,
    text: &str,
) -> Result<SaveResult, String> {
    let st = app.state::<AppState>();
    let (save_path, meta, old_path) = {
        let wins = st.wins.lock().unwrap();
        let p = wins
            .get(label)
            .and_then(|w| w.panes.iter().find(|p| p.id == pane))
            .ok_or("문서 정보 없음")?;
        let save_path = path.unwrap_or_else(|| p.path.clone());
        let meta = p.meta.clone().unwrap_or(DocMeta {
            encoding: encoding_rs::UTF_8,
            has_bom: false,
            crlf: false,
        });
        (save_path, meta, p.path.clone())
    };
    let bytes = encode_full(text, &meta);
    fs::write(&save_path, &bytes).map_err(|e| format!("{}: {e}", save_path.display()))?;
    let path_changed = save_path != old_path;
    {
        let mut wins = st.wins.lock().unwrap();
        let p = wins
            .get_mut(label)
            .and_then(|w| w.panes.iter_mut().find(|p| p.id == pane))
            .ok_or("문서 정보 없음")?;
        // 디스크에 방금 쓴 내용을 '마지막으로 본 내용' 으로. 감시 이벤트가 되돌아와도 다시 그리지 않는다.
        p.last_text = Some(text.to_string());
        p.dirty = false;
        if path_changed {
            p.path = canonical(save_path.clone());
            p.watcher = watch_file(app, label, pane, &p.path);
        }
    }
    update_title(app, label);
    if path_changed {
        remember_recent(app, &save_path);
    }
    let size = bytes.len();
    Ok(SaveResult {
        path: save_path.to_string_lossy().into_owned(),
        bytes: size,
    })
}

/// 문서 저장. text 는 화면(CodeMirror)의 현재 소스. 메타(인코딩·BOM·CRLF)는 칸이 기억한 원본 꼴 그대로.
/// path 를 주면 '다른 이름으로 저장'(새 경로), 주지 않으면 원 경로에 덮어쓴다.
#[tauri::command]
fn save_document(
    app: AppHandle,
    window: WebviewWindow,
    pane: u32,
    path: Option<String>,
    text: String,
) -> Result<SaveResult, String> {
    let label = window.label().to_string();
    let path = path.filter(|s| !s.is_empty()).map(PathBuf::from);
    do_save(&app, &label, pane, path, &text)
}

/// '다른 이름으로 저장' 대화상자를 연다. 사용자가 고르면 그 경로로 저장. 취소하면 아무 일도 안 한다.
#[tauri::command]
async fn save_document_as(
    app: AppHandle,
    window: WebviewWindow,
    pane: u32,
    text: String,
) -> Result<Option<SaveResult>, String> {
    let (label, suggest_name) = {
        let st = app.state::<AppState>();
        let wins = st.wins.lock().unwrap();
        let p = wins
            .get(window.label())
            .and_then(|w| w.panes.iter().find(|p| p.id == pane))
            .ok_or("문서 정보 없음")?;
        (window.label().to_string(), file_title(&p.path))
    };
    let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
    app.dialog()
        .file()
        .set_title("다른 이름으로 저장")
        .add_filter("Markdown", MD_EXTS)
        .set_file_name(&suggest_name)
        .save_file(move |picked| {
            let _ = tx.send(picked.and_then(|f| f.into_path().ok()));
        });
    let picked = rx.recv().map_err(|e| e.to_string())?;
    let Some(path) = picked else { return Ok(None) };
    let res = do_save(&app, &label, pane, Some(path), &text)?;
    Ok(Some(res))
}

/// 화면이 알려 주는 '저장 안 된 변경' 상태. 창 제목 '•' 와 외부 변경 처리에 쓴다.
#[tauri::command]
fn mark_dirty(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    pane: u32,
    dirty: bool,
) {
    let label = window.label().to_string();
    let changed = {
        let mut wins = state.wins.lock().unwrap();
        let Some(win) = wins.get_mut(&label) else {
            return;
        };
        let Some(p) = win.panes.iter_mut().find(|p| p.id == pane) else {
            return;
        };
        if p.dirty == dirty {
            false
        } else {
            p.dirty = dirty;
            true
        }
    };
    if changed {
        update_title(&app, &label);
    }
}

/// --roundtrip 전용(그리고 자체 점검) 바이트 왕복 검사. 화면이 아직 없을 때도 쓴다.
#[tauri::command]
fn roundtrip_check_cmd(path: String) -> Result<RoundtripReport, String> {
    let p = PathBuf::from(&path);
    let (ok, bytes) = check_roundtrip(&p).map_err(|e| format!("{path}: {e}"))?;
    Ok(RoundtripReport { ok, bytes })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RoundtripReport {
    ok: bool,
    bytes: usize,
}

#[tauri::command]
fn set_active_pane(window: WebviewWindow, state: State<'_, AppState>, pane: u32) {
    if let Some(w) = state.wins.lock().unwrap().get_mut(window.label()) {
        if w.panes.iter().any(|p| p.id == pane) {
            w.active = pane;
        }
    }
}

#[tauri::command]
async fn close_pane(app: AppHandle, window: WebviewWindow, pane: u32) {
    remove_pane(&app, window.label(), pane);
}

/// Pane UX: 같은 파일 경로의 '편집 칸' 을 만든다. add_panes 는 같은 경로의 중복 칸을 막지만,
/// 편집 칸 + 보기 칸을 함께 두는 Pane UX 에서는 중복이 필요하다. mode 는 frontend 가 가진다.
#[tauri::command]
async fn add_edit_pane(app: AppHandle, window: WebviewWindow, path: String) -> Result<u32, String> {
    let label = window.label().to_string();
    let path = canonical(PathBuf::from(path));
    let (before, after, pane_id) = {
        let st = app.state::<AppState>();
        let mut wins = st.wins.lock().unwrap();
        let win = wins.get_mut(&label).ok_or("창 정보 없음")?;
        let before = win.panes.len();
        if before >= MAX_PANES {
            return Err("칸이 꽉 찼습니다".into());
        }
        let pane = new_pane(&app, &label, path.clone(), None);
        let id = pane.id;
        win.active = id;
        win.panes.push(pane);
        (before, win.panes.len(), id)
    };
    let w = app
        .get_webview_window(&label)
        .ok_or_else(|| "창을 찾지 못함".to_string())?;
    let _ = w.eval(format!(
        "MDV.addPane({pane_id},{idx},\"edit\");MDV.focusPane({pane_id});",
        idx = after - 1
    ));
    update_title(&app, &label);
    resize_for_panes(&w, before, after);
    Ok(pane_id)
}

#[tauri::command]
async fn pop_out_pane(app: AppHandle, window: WebviewWindow, pane: u32) {
    pop_out(&app, window.label(), pane);
}

#[tauri::command]
async fn drop_files(
    app: AppHandle,
    window: WebviewWindow,
    paths: Vec<PathBuf>,
    at: usize,
    replace: Option<u32>,
) {
    let paths: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.is_file() && is_markdown(p))
        .collect();
    add_panes(&app, window.label(), paths, Some(at), replace);
}

/// 문서 안 링크. 웹 주소는 기본 브라우저로, .md 는 새 창(beside 면 옆 칸)으로, 그 밖의 파일은 기본 앱으로.
#[tauri::command]
async fn open_link(
    app: AppHandle,
    window: WebviewWindow,
    url: String,
    pane: u32,
    beside: bool,
) -> Result<(), String> {
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
            let (current, index) = {
                let st = app.state::<AppState>();
                let wins = st.wins.lock().unwrap();
                let win = wins.get(window.label());
                let i = win.and_then(|w| w.panes.iter().position(|p| p.id == pane));
                (
                    win.zip(i).map(|(w, i)| w.panes[i].path.clone()),
                    i.unwrap_or(0),
                )
            };
            if current.is_some_and(|c| same_file(&c, &path)) {
                if let Some(a) = anchor {
                    let _ = window.eval(format!("MDV.goto({pane},{})", json(&a)));
                }
            } else if !path.exists() {
                return Err(format!("파일 없음: {}", path.display()));
            } else if is_markdown(&path) && beside {
                add_panes(&app, window.label(), vec![path], Some(index + 1), None);
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

/// 편집 UX 모드 저장 ("toggle" | "pane"). 설정 창이 따로 없어, 보기 메뉴의 두 항목으로 고른다.
#[tauri::command]
fn set_edit_ui_mode(app: AppHandle, state: State<'_, AppState>, mode: String) {
    let mode = if mode == "pane" { "pane" } else { "toggle" };
    state.prefs.lock().unwrap().edit_ui_mode = mode.into();
    save_prefs(&app);
    for w in app.webview_windows().values() {
        let _ = w.eval(format!("MDV.setEditUiMode({})", json(&mode)));
    }
}

#[tauri::command]
fn log_message(app: AppHandle, message: String) {
    log_line(&app, &message);
}

/// --self-test: 화면이 다 그린 뒤 보내는 점검 결과를 파일로 쓰고 끝낸다 (CI 에서 세 운영체제 확인용).
#[tauri::command]
fn self_test_report(app: AppHandle, state: State<'_, AppState>, report: serde_json::Value) {
    let Some(test) = &state.self_test else { return };
    let (path, hold) = (test.report.clone(), test.hold);
    let first = !test.reported.swap(true, Ordering::SeqCst); // 끝내기 예약은 첫 보고 때 한 번만
    let windows = app.webview_windows().len();
    let handle = app.clone();
    // 맥 메뉴는 메인 스레드에서만 읽을 수 있다
    let _ = app.run_on_main_thread(move || {
        let mut report = report;
        report["windows"] = serde_json::json!(windows);
        #[cfg(target_os = "macos")]
        {
            report["windowsMenu"] = serde_json::json!(mac_windows_menu());
        }
        let _ = fs::write(
            &path,
            serde_json::to_string_pretty(&report).unwrap_or_default(),
        );
        if first {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(hold));
                handle.exit(0);
            });
        }
    });
}

/// 맥이 '윈도우 메뉴'로 알고 있는 메뉴의 항목들. 비어 있으면 fn+control+화살표 배치가 동작하지 않는다.
#[cfg(target_os = "macos")]
fn mac_windows_menu() -> Vec<String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else {
        return vec![];
    };
    let Some(menu) = NSApplication::sharedApplication(mtm).windowsMenu() else {
        return vec![];
    };
    (0..menu.numberOfItems())
        .filter_map(|i| menu.itemAtIndex(i))
        .map(|item| item.title().to_string())
        .collect()
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
    // `--self-test <md> --roundtrip` (화면 없이): 바이트 왕복만 확인하고 끝낸다.
    // --scenario 와 함께 쓰면 화면 점검에서 왕복 결과도 보고하도록 Layout 으로 넘긴다.
    if let Some(t) = cli.self_test.as_ref() {
        if t.roundtrip && !t.scenario {
            let res = check_roundtrip(&t.file);
            let (ok, bytes, err) = match res {
                Ok((ok, b)) => (ok, b, None),
                Err(e) => (false, 0, Some(e.to_string())),
            };
            let report = serde_json::json!({
                "mode": "roundtrip",
                "roundtrip": { "ok": ok, "bytes": bytes, "error": err },
                "file": t.file.to_string_lossy(),
            });
            let _ = fs::write(
                &t.report,
                serde_json::to_string_pretty(&report).unwrap_or_default(),
            );
            std::process::exit(if ok { 0 } else { 1 });
        }
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
                    None => show_open_dialog(app, None),
                }
            }
            open_documents(app, more.files);
        }));
    }

    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_layout,
            load_document,
            save_document,
            save_document_as,
            mark_dirty,
            roundtrip_check_cmd,
            set_active_pane,
            close_pane,
            add_edit_pane,
            pop_out_pane,
            drop_files,
            open_link,
            set_outline,
            set_edit_ui_mode,
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
            let sync_on = prefs.sync_scroll;
            let self_test = cli.self_test.take();
            let test_file = self_test
                .as_ref()
                .map(|t| (t.file.clone(), t.report.clone()));
            app.manage(AppState {
                wins: Mutex::new(HashMap::new()),
                order: Mutex::new(vec![]),
                counter: AtomicUsize::new(0),
                pane_counter: AtomicU32::new(0),
                prefs: Mutex::new(prefs),
                prefs_path,
                log_path: app.path().app_log_dir()?.join("MDViewer.log"),
                recent_menu: Mutex::new(None),
                sync_item: Mutex::new(None),
                self_test,
            });
            let menu = build_menu(&handle, sync_on)?;
            handle.set_menu(menu.menu)?;
            {
                let st = app.state::<AppState>();
                *st.recent_menu.lock().unwrap() = Some(menu.recent);
                *st.sync_item.lock().unwrap() = Some(menu.sync);
            }
            refresh_recent(&handle);

            if let Some((file, report)) = test_file {
                // 명령줄에 문서를 더 주면 한 창에 나란히 띄워 점검한다
                let docs = std::iter::once(file)
                    .chain(std::mem::take(&mut cli.files))
                    .map(|p| (p, None))
                    .collect();
                open_window(&handle, docs);
                // 화면이 끝내 보고하지 않으면 시간 초과로 실패 처리
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(90));
                    let _ = fs::write(&report, r#"{"timeout": true}"#);
                    h.exit(2);
                });
            } else if !cli.files.is_empty() || has_pending_open() {
                #[allow(unused_mut)]
                let mut files = std::mem::take(&mut cli.files);
                #[cfg(target_os = "macos")]
                files.append(&mut PENDING_OPEN.lock().unwrap());
                open_documents(&handle, files);
            } else {
                // 맥은 Finder 에서 연 파일이 조금 뒤 Opened 이벤트로 온다. 그때까지 아무것도 없으면 열기 창.
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(700));
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || {
                        if h2.state::<AppState>().wins.lock().unwrap().is_empty() {
                            show_open_dialog(&h2, None);
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

    app.run(on_run_event);
}

fn has_pending_open() -> bool {
    #[cfg(target_os = "macos")]
    return !PENDING_OPEN.lock().unwrap().is_empty();
    #[cfg(not(target_os = "macos"))]
    false
}

// 맥 전용 앱 이벤트: Finder·Dock 에서 연 파일, Dock 아이콘 클릭, 마지막 창을 닫아도 앱 유지(일반 맥 앱처럼).
#[cfg(target_os = "macos")]
fn on_run_event(app: &AppHandle, event: tauri::RunEvent) {
    use tauri::RunEvent;
    match event {
        RunEvent::Opened { urls } => {
            let paths: Vec<PathBuf> = urls
                .into_iter()
                .filter_map(|u| u.to_file_path().ok())
                .collect();
            if app.try_state::<AppState>().is_some() {
                open_documents(app, paths);
            } else {
                PENDING_OPEN.lock().unwrap().extend(paths);
            }
        }
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => show_open_dialog(app, None),
        RunEvent::ExitRequested {
            code: None, api, ..
        } if app.state::<AppState>().self_test.is_none() => api.prevent_exit(),
        _ => {}
    }
}

// 윈도우·리눅스는 파일을 명령줄 인자로 받고, 마지막 창을 닫으면 끝난다 (기본 동작).
#[cfg(not(target_os = "macos"))]
fn on_run_event(_app: &AppHandle, _event: tauri::RunEvent) {}

// ── 테스트 ─────────────────────────────────────────────────────────
// decode_full → encode_full 이 원본 바이트를 그대로 되돌리는지 확인한다.
// 자체 점검의 --roundtrip 은 이 두 함수를 샘플 문서에 적용해 바이트가 같은지 본다.
#[cfg(test)]
mod tests {
    use super::*;

    fn rt(bytes: &[u8]) {
        let (text, meta) = decode_full(bytes);
        let out = encode_full(&text, &meta);
        assert_eq!(
            out, bytes,
            "roundtrip mismatch: meta={:?}, text_len={}",
            meta,
            text.len()
        );
    }

    #[test]
    fn roundtrip_utf8_lf() {
        rt(b"hello\n\xea\xb0\x80\xeb\x82\x98\n");
    }

    #[test]
    fn roundtrip_utf8_bom_lf() {
        let mut v = vec![0xEF, 0xBB, 0xBF];
        v.extend_from_slice(b"title\n\xec\x95\x88\xeb\x85\x95\n");
        rt(&v);
    }

    #[test]
    fn roundtrip_utf8_crlf() {
        rt(b"a\r\nb\r\n\xea\xb0\x80\r\n");
    }

    #[test]
    fn roundtrip_utf8_bom_crlf() {
        let mut v = vec![0xEF, 0xBB, 0xBF];
        v.extend_from_slice(b"x\r\ny\r\n");
        rt(&v);
    }

    #[test]
    fn roundtrip_utf16le_bom() {
        // "가\n나\n"
        let s = "가\n나\n";
        let mut v: Vec<u8> = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            v.push((u & 0xff) as u8);
            v.push((u >> 8) as u8);
        }
        rt(&v);
    }

    #[test]
    fn roundtrip_utf16be_bom() {
        let s = "가\n나\n";
        let mut v: Vec<u8> = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            v.push((u >> 8) as u8);
            v.push((u & 0xff) as u8);
        }
        rt(&v);
    }

    #[test]
    fn roundtrip_euc_kr() {
        // '가' = 0xB0 0xA1 in EUC-KR
        rt(&[0xB0, 0xA1, b'\n']);
    }

    #[test]
    fn roundtrip_empty() {
        rt(b"");
    }

    #[test]
    fn roundtrip_utf16le_crlf() {
        let s = "가\r\n나\r\n";
        let mut v: Vec<u8> = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            v.push((u & 0xff) as u8);
            v.push((u >> 8) as u8);
        }
        rt(&v);
    }
}
