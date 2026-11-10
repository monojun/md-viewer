// 뷰어 화면. 창 하나에 문서 칸(Pane)을 1~3개 나란히 둔다. 앱 껍데기는 Tauri(src-tauri/)다.
//  - 화면 → 껍데기: invoke('get_layout'·'load_document'·'open_link' …)
//  - 껍데기 → 화면: eval 로 MDV.changed(칸)·MDV.addPane(칸, 자리)·MDV.drag.over(x) 등을 부른다.
// 칸 번호는 껍데기가 정한다(앱 전체에서 하나뿐). 메뉴 명령(찾기·개요·닫기 등)은 '지금 칸'(active)에 적용된다.
import DOMPurify from 'dompurify';
import { createMarkdown, slugify, rootUrlFromFrontMatter } from './markdown.js';
import { createEditor } from './editor.js';

const md = createMarkdown();
const $ = (sel) => document.querySelector(sel);
const content = $('#content');
const outline = $('#outline');
const tauri = window.__TAURI__;
const invoke = (cmd, args) => tauri.core.invoke(cmd, args);

const MAX_PANES = 3;
const panes = new Map(); // 칸 번호 → Pane
let active = null; // 지금 칸
let selfTest = false; // 자체 점검: 'report' 결과를 보내는 창, 'scenario' 시나리오를 도는 창
let ready = false;
const logged = [];

const call = (cmd, args) => invoke(cmd, args).catch((e) => console.warn('[mdv]', cmd, e));

// 웹뷰 안 오류를 앱 로그(MDViewer.log)로
const logError = (what) => {
  const message = String(what && what.stack ? what.stack : what);
  logged.push(message.slice(0, 300));
  call('log_message', { message });
};
window.addEventListener('error', (e) => logError(e.error || e.message));
window.addEventListener('unhandledrejection', (e) => logError(e.reason));

// ── 지연 로드 (수식·다이어그램이 있는 문서에서만) ─────────────────────
const loaded = {};
function loadScript(src) {
  if (!loaded[src]) {
    loaded[src] = new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.onload = resolve;
      s.onerror = () => reject(new Error('load failed: ' + src));
      document.head.appendChild(s);
    });
  }
  return loaded[src];
}

async function ensureMathJax() {
  if (!window.MathJax || !window.MathJax.tex2svgPromise) {
    window.MathJax = {
      loader: { load: [] },
      tex: { tags: 'all', inlineMath: [], displayMath: [], processEscapes: false },
      svg: { fontCache: 'local' },
      options: { enableMenu: false },
      startup: { typeset: false },
    };
    await loadScript('vendor/tex-svg-full.js');
    await window.MathJax.startup.promise;
    // tex2svg 직접 호출 시 MathJax 가 CSS 를 넣지 않으므로 직접 붙인다
    document.head.appendChild(window.MathJax.svgStylesheet());
  }
  return window.MathJax;
}

async function ensureMermaid() {
  if (!window.mermaid) {
    await loadScript('vendor/mermaid.min.js');
    // 테마 색(theme.css 의 강조색·본문색)에 맞춘 다이어그램
    window.mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      theme: 'base',
      fontFamily: 'Pretendard, sans-serif',
      themeVariables: {
        primaryColor: '#eef3fc',
        primaryBorderColor: '#2a64d0',
        primaryTextColor: '#24272c',
        lineColor: '#6a7079',
        secondaryColor: '#f3f5f8',
        tertiaryColor: '#fafbfc',
      },
      flowchart: { curve: 'basis' },
    });
  }
  return window.mermaid;
}

// MathJax(수식 번호)와 Mermaid 는 전역 상태를 쓰므로 칸 여럿이 동시에 그리지 않게 한 줄로 세운다
let lane = Promise.resolve();
const oneAtATime = (fn) => (lane = lane.then(fn, fn));

let mermaidId = 0;

// ── 경로 ────────────────────────────────────────────────────────────
function resolveBase(text, docURL) {
  const root = rootUrlFromFrontMatter(text);
  if (!root || !docURL) return docURL;
  const dir = root.endsWith('/') ? root : root + '/';
  try {
    return new URL(dir, docURL).href;
  } catch (_) {
    return docURL;
  }
}

// file: URL → 운영체제 경로 (윈도우 /C:/… 와 공유 폴더 //서버/… 포함)
function localPath(url) {
  let p = decodeURIComponent(url.pathname);
  if (/^\/[A-Za-z]:/.test(p)) p = p.slice(1);
  return url.host ? `//${url.host}${p}` : p;
}

const SANITIZE = {
  ADD_TAGS: ['iframe'],
  ADD_ATTR: ['target', 'lang', 'align'],
  ALLOWED_URI_REGEXP: /^(?:(?:https?|ftps?|mailto|tel|file|data):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i,
};

const ICONS = {
  popout:
    '<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><path d="M9.5 2.5h4v4M13.5 2.5 8 8M12 9.5v3a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  close:
    '<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>',
  edit:
    '<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true"><path d="M2.5 11.5V13.5H4.5L11.6 6.4L9.6 4.4L2.5 11.5Z M10.4 3.6L11.5 2.5A1 1 0 0 1 12.9 2.5L13.5 3.1A1 1 0 0 1 13.5 4.5L12.4 5.6Z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>',
};

let editUiMode = 'toggle'; // 'toggle' | 'pane' — 백엔드가 설정 변화 시 MDV.setEditUiMode 로 바꾼다

// ── 칸 ──────────────────────────────────────────────────────────────
class Pane {
  constructor(id, mode = 'view') {
    this.id = id;
    this.seq = 0;
    this.sources = [];
    this.docURL = null;
    this.baseURL = location.href;
    this.reloadTimer = 0;
    this.mode = mode; // 'view' | 'edit'
    this.editor = null; // CodeMirror 편집기 핸들 (edit 모드에서만)
    this.source = ''; // 편집 중일 때는 CodeMirror 의 현재 값, 그 외에는 파일 내용과 같다
    this.fileText = ''; // 마지막으로 읽은 디스크 내용 (dirty 판정 기준)
    this.dirty = false;
    this.pendingExternal = null; // 편집 중 외부 변경이 오면 보류한 디스크 내용

    this.el = document.createElement('section');
    this.el.className = 'pane';
    this.el.dataset.id = id;
    this.el.dataset.mode = mode;
    this.el.innerHTML = `
      <header class="pane-head">
        <span class="pane-name"></span><span class="pane-dir"></span>
        <button class="pane-btn pane-edit-btn" title="편집 (⌘E)">${ICONS.edit}</button>
        <button class="pane-btn pane-popout" title="이 칸을 새 창으로">${ICONS.popout}</button>
        <button class="pane-btn pane-close" title="칸 닫기">${ICONS.close}</button>
      </header>
      <div class="pane-conflict" role="alert">
        <span class="pane-conflict-msg">파일이 바깥에서 바뀌었습니다. 내 변경을 지키려면 왼쪽, 디스크 내용으로 되돌리려면 오른쪽.</span>
        <button class="pane-conflict-keep">내 변경 지키기</button>
        <button class="pane-conflict-reload">디스크로 되돌리기</button>
      </div>
      <div class="pane-scroll" tabindex="-1"><article class="write"></article></div>
      <div class="pane-edit"></div>`;
    this.head = this.el.querySelector('.pane-head');
    this.scroller = this.el.querySelector('.pane-scroll');
    this.write = this.el.querySelector('.write');
    this.editHost = this.el.querySelector('.pane-edit');
    this.conflict = this.el.querySelector('.pane-conflict');

    this.el.addEventListener('pointerdown', () => setActive(this), true);
    this.el.addEventListener('wheel', () => (sync.leader = this), { passive: true });
    this.el.querySelector('.pane-close').addEventListener('click', () => this.requestClose());
    this.el.querySelector('.pane-popout').addEventListener('click', () => call('pop_out_pane', { pane: this.id }));
    this.el.querySelector('.pane-edit-btn').addEventListener('click', () => this.toggleEdit());
    this.el.querySelector('.pane-conflict-keep').addEventListener('click', () => this.resolveConflict('keep'));
    this.el.querySelector('.pane-conflict-reload').addEventListener('click', () => this.resolveConflict('reload'));
    this.scroller.addEventListener('scroll', () => onPaneScroll(this), { passive: true });
  }

  setMode(mode) {
    this.mode = mode;
    this.el.dataset.mode = mode;
  }

  isEditing() {
    return !!this.editor;
  }

  setDirty(d) {
    if (this.dirty === d) return;
    this.dirty = d;
    this.el.classList.toggle('dirty', d);
    call('mark_dirty', { pane: this.id, dirty: d });
  }

  requestClose() {
    if (this.dirty) {
      const yes = window.confirm(`"${this.el.querySelector('.pane-name').textContent}" 에 저장 안 된 변경이 있습니다. 그대로 닫을까요?`);
      if (!yes) return;
    }
    call('close_pane', { pane: this.id });
  }

  // mode: 'first' 처음 열 때, 'changed' 파일이 바뀌었을 때(같은 내용이면 건너뜀),
  //       'force' 메뉴의 다시 읽기, 'external' 편집 중 외부 변경을 사용자가 수용
  async load(mode) {
    // 편집 중이고 dirty 이고 외부에서 바뀌었으면 자동 로드 금지 — 충돌 바를 띄우고 사용자에게 맡긴다
    if (mode === 'changed' && this.isEditing() && this.dirty) {
      const peek = await invoke('load_document', { pane: this.id, reload: true, force: true });
      if (peek && peek.text !== this.source) {
        this.pendingExternal = peek;
        this.conflict.classList.add('show');
      }
      return;
    }
    const doc = await invoke('load_document', { pane: this.id, reload: mode !== 'first', force: mode === 'force' || mode === 'external' });
    if (!doc) return;
    this.el.querySelector('.pane-name').textContent = doc.title;
    this.el.querySelector('.pane-dir').textContent = doc.dir;
    this.head.title = this.path = doc.path;
    this.source = doc.text;
    this.fileText = doc.text;
    this.setDirty(false);
    this.conflict.classList.remove('show');
    this.pendingExternal = null;
    if (this.editor) this.editor.applyExternalSource(doc.text);
    await this.render(doc.text, { docURL: doc.docUrl, anchor: doc.anchor, preserveScroll: mode !== 'first' });
    // 자체 점검 중에는 다시 읽을 때마다 보고해서, 파일을 고치면 결과가 바뀌는지로 자동 새로고침도 확인한다
    if (selfTest === 'report' && ready && mode !== 'first') await report(mode);
  }

  reload(force) {
    clearTimeout(this.reloadTimer);
    // 저장 한 번에 감시 이벤트가 여러 개 오므로 잠깐 모았다가 읽는다. 저장 도중이라 못 읽으면 다음 이벤트를 기다린다.
    this.reloadTimer = setTimeout(() => this.load(force ? 'force' : 'changed').catch(() => {}), force ? 0 : 120);
  }

  resolveConflict(choice) {
    this.conflict.classList.remove('show');
    const external = this.pendingExternal;
    this.pendingExternal = null;
    if (!external) return;
    if (choice === 'reload') {
      // 편집 내용 버리고 디스크 내용으로
      this.source = external.text;
      this.fileText = external.text;
      this.setDirty(false);
      if (this.editor) this.editor.applyExternalSource(external.text);
      this.render(external.text, { docURL: external.docUrl, preserveScroll: true });
    }
    // 'keep' 은 아무 일 안 함 (내 편집 그대로)
  }

  // Toggle UX: 보기 ↔ 편집. Pane UX 에서는 처음부터 edit 모드로 열리므로 끌 일이 거의 없다.
  async toggleEdit() {
    if (this.editor) {
      await this.exitEdit();
    } else {
      await this.enterEdit();
    }
  }

  async enterEdit() {
    if (this.editor) return;
    if (!this.fileText && this.source) this.fileText = this.source;
    const initial = this.source;
    this.setMode('edit');
    this.el.classList.add('editing');
    this.editor = createEditor(this.editHost, initial, {
      onChange: (text) => {
        this.source = text;
        this.setDirty(text !== this.fileText);
      },
      onPreview: (text) => {
        this.render(text, { docURL: this.docURL, preserveScroll: true });
        // 편집 → 보기 칸 공유 (같은 창의 보기 칸)
        syncViewPanes(this, text);
      },
      onCursorLine: (line) => sync.fromEditor(this, line, 'cursor'),
      onScrollLine: (line) => sync.fromEditor(this, line, 'scroll'),
      onError: (e) => logError(e),
    });
    // CodeMirror 안에서도 ⌘S / ⌘⇧S 로 저장하게. 메뉴 단축키는 CodeMirror 포커스에서 가로채질 수 있다.
    this.editor.cm.setOption('extraKeys', Object.assign({}, this.editor.cm.getOption('extraKeys'), {
      'Cmd-S': () => this.save(),
      'Ctrl-S': () => this.save(),
      'Shift-Cmd-S': () => this.saveAs(),
      'Shift-Ctrl-S': () => this.saveAs(),
      'Cmd-E': () => this.exitEdit(),
      'Ctrl-E': () => this.exitEdit(),
    }));
    this.editor.focus();
  }

  async exitEdit() {
    if (!this.editor) return;
    if (this.dirty) {
      const yes = window.confirm('저장 안 된 변경이 있습니다. 편집 모드를 나가면서 디스크로 되돌릴까요?');
      if (!yes) {
        this.editor.focus();
        return;
      }
      // 되돌리기
      this.source = this.fileText;
      this.setDirty(false);
      await this.render(this.fileText, { docURL: this.docURL, preserveScroll: true });
    }
    this.editor.destroy();
    this.editor = null;
    this.setMode('view');
    this.el.classList.remove('editing');
    this.conflict.classList.remove('show');
  }

  async save() {
    if (!this.editor) return; // 편집기가 없으면 저장할 소스도 없다
    const text = this.editor.getSource();
    try {
      await invoke('save_document', { pane: this.id, path: null, text });
      this.fileText = text;
      this.source = text;
      this.setDirty(false);
    } catch (e) {
      logError(e);
      window.alert('저장 실패: ' + (e.message || e));
    }
  }

  async saveAs() {
    if (!this.editor) return;
    const text = this.editor.getSource();
    try {
      const res = await invoke('save_document_as', { pane: this.id, text });
      if (res) {
        this.fileText = text;
        this.source = text;
        this.setDirty(false);
      }
    } catch (e) {
      logError(e);
      window.alert('저장 실패: ' + (e.message || e));
    }
  }

  fail(e) {
    this.write.textContent = '문서를 열 수 없습니다: ' + e;
    logError(e);
  }

  headings() {
    return [...this.write.querySelectorAll('h1, h2, h3, h4, h5, h6')];
  }

  async render(text, opts = {}) {
    const seq = ++this.seq;
    const keepScroll = !!opts.preserveScroll;
    const scrollTop = this.scroller.scrollTop;

    this.docURL = opts.docURL || null;
    this.baseURL = resolveBase(text, this.docURL) || location.href;

    const env = {};
    this.write.innerHTML = DOMPurify.sanitize(md.render(text, env), SANITIZE);
    this.sources = env.sources || [];
    this.placeFootnotes();
    this.fixRelativeUrls();
    this.buildToc();
    if (this === active) refreshOutline();
    if (this === active && !find.bar.hidden && find.input.value) find.search(find.input.value, false);

    const settle = () => {
      if (keepScroll) this.scroller.scrollTop = scrollTop;
      else if (opts.anchor) this.scrollToAnchor(opts.anchor);
    };
    settle();
    if (!keepScroll && !opts.anchor) this.scroller.scrollTop = 0;

    await oneAtATime(() => Promise.all([this.renderMath(seq), this.renderDiagrams(seq)]));
    if (seq !== this.seq) return;
    settle();
    if (this === active) updateActiveOutline();
  }

  // 그림 경로를 문서 위치 기준으로. 화면은 file: 을 직접 못 읽으므로 asset 주소로 바꾼다.
  fixRelativeUrls() {
    for (const img of this.write.querySelectorAll('img[src]')) {
      const src = img.getAttribute('src');
      let url;
      try {
        if (/^[A-Za-z]:[\\/]/.test(src)) url = new URL('file:///' + src.replace(/\\/g, '/'));
        else if (/^[a-z][a-z0-9+.-]*:/i.test(src) && !/^file:/i.test(src)) continue;
        else url = new URL(src, this.baseURL);
      } catch (_) {
        continue;
      }
      img.src = url.protocol === 'file:' ? tauri.core.convertFileSrc(localPath(url)) : url.href;
    }
  }

  // 각주 정의를 문서 끝이 아니라 원문에 적힌 자리로 (쓴 사람이 둔 위치 그대로)
  placeFootnotes() {
    for (const slot of this.write.querySelectorAll('.fn-slot')) {
      const def = this.write.querySelector(`.md-def-footnote[data-label="${CSS.escape(slot.dataset.label || '')}"]`);
      if (def) slot.replaceWith(def);
      else slot.remove();
    }
    const area = this.write.querySelector('.footnotes-area');
    if (area && !area.children.length) area.remove();
  }

  buildToc() {
    const tocs = this.write.querySelectorAll('.md-toc');
    if (!tocs.length) return;
    const headings = this.headings();
    const html = headings
      .map((h) => `<a class="md-toc-item md-toc-${h.tagName.toLowerCase()}" href="#${encodeURIComponent(h.id)}"></a>`)
      .join('');
    for (const toc of tocs) {
      toc.innerHTML = `<div class="md-toc-content">${html}</div>`;
      toc.querySelectorAll('.md-toc-item').forEach((a, i) => (a.textContent = headings[i].textContent));
    }
  }

  async renderMath(seq) {
    const blocks = [...this.write.querySelectorAll('.md-math-block[data-source]')];
    if (!blocks.length) return;
    const MJ = await ensureMathJax();
    if (seq !== this.seq) return;
    MJ.texReset();
    for (const el of blocks) {
      try {
        const node = await MJ.tex2svgPromise(this.sources[el.dataset.source], { display: true });
        if (seq !== this.seq) return;
        el.replaceChildren(node);
      } catch (e) {
        logError(e);
        el.textContent = this.sources[el.dataset.source];
        el.classList.add('md-error');
      }
    }
  }

  async renderDiagrams(seq) {
    const blocks = [...this.write.querySelectorAll('.md-diagram[data-source]')];
    if (!blocks.length) return;
    const mermaid = await ensureMermaid();
    for (const el of blocks) {
      if (seq !== this.seq) return;
      try {
        const { svg } = await mermaid.render('mdv-mermaid-' + ++mermaidId, this.sources[el.dataset.source]);
        el.innerHTML = svg;
      } catch (e) {
        logError(e);
        el.classList.add('md-error');
        el.textContent = (e && e.message ? e.message : String(e)).split('\n')[0];
      }
    }
    document.querySelectorAll('body > [id^="dmdv-mermaid-"]').forEach((n) => n.remove());
  }

  // 칸마다 같은 제목 id 가 있을 수 있으므로 문서 전체가 아니라 이 칸 안에서 찾는다
  scrollToAnchor(raw) {
    let name = raw;
    try {
      name = decodeURIComponent(raw);
    } catch (_) {}
    let el = this.write.querySelector('#' + CSS.escape(name));
    if (!el) {
      const want = slugify(name);
      el = this.headings().find((h) => h.id === want || slugify(h.textContent) === want) || null;
    }
    if (el) {
      sync.leader = this;
      el.scrollIntoView({ block: 'start' });
      if (this === active) updateActiveOutline();
    }
    return !!el;
  }

  // 칸 위쪽 기준 y 좌표 (스크롤 포함)
  yOf(el) {
    return el.getBoundingClientRect().top - this.scroller.getBoundingClientRect().top + this.scroller.scrollTop;
  }
}

const paneOf = (node) => panes.get(+node.closest('.pane')?.dataset.id);
const ordered = () => [...content.children].map(paneOf).filter(Boolean);

function layoutChanged() {
  document.body.classList.toggle('multi', panes.size > 1);
}

function setActive(pane) {
  if (!pane || pane === active) return;
  active = pane;
  for (const p of panes.values()) p.el.classList.toggle('active', p === active);
  refreshOutline();
  if (!find.bar.hidden && find.input.value) find.search(find.input.value, false);
  call('set_active_pane', { pane: pane.id });
}

function addPane(id, index, mode) {
  const pane = new Pane(id, mode || 'view');
  panes.set(id, pane);
  content.insertBefore(pane.el, content.children[index] || null);
  layoutChanged();
  return pane;
}

// 같은 파일 경로의 '보기 칸' 들에 편집기의 현재 소스로 다시 그린다 (Pane UX: 편집 ↔ 보기 라이브 공유).
function syncViewPanes(editPane, text) {
  if (!editPane.path) return;
  for (const p of panes.values()) {
    if (p === editPane || p.isEditing() || p.path !== editPane.path) continue;
    p.render(text, { docURL: p.docURL, preserveScroll: true }).catch((e) => logError(e));
  }
}

function removePane(id) {
  const pane = panes.get(id);
  if (!pane) return;
  pane.el.remove();
  panes.delete(id);
  if (active === pane) active = null;
  layoutChanged();
}

// ── 앵커·링크 ───────────────────────────────────────────────────────
document.addEventListener(
  'click',
  (e) => {
    if (e.target.closest('.task-list-item-checkbox')) {
      e.preventDefault();
      return;
    }
    const item = e.target.closest('.outline-item');
    if (item) {
      e.preventDefault();
      active?.scrollToAnchor(item.dataset.target);
      return;
    }
    const a = e.target.closest('.write a[href]');
    if (!a) return;
    e.preventDefault();
    const owner = paneOf(a);
    if (!owner) return;
    const href = a.getAttribute('href');
    if (href.startsWith('#')) {
      owner.scrollToAnchor(href.slice(1));
      return;
    }
    let url;
    try {
      url = new URL(href, owner.baseURL);
    } catch (_) {
      return;
    }
    // ⌥(Alt)+클릭: .md 링크를 새 창 대신 옆 칸으로
    call('open_link', { url: url.href, pane: owner.id, beside: e.altKey });
  },
  true
);

// ── 개요(사이드바) — 지금 칸의 제목들 ────────────────────────────────
function setOutline(open, notify = true) {
  document.body.classList.toggle('outline-open', open);
  if (notify) call('set_outline', { open });
  updateActiveOutline();
}

function refreshOutline() {
  outline.replaceChildren();
  const headings = active ? active.headings() : [];
  if (!headings.length) {
    outline.innerHTML = '<div class="outline-empty">제목이 없습니다</div>';
    return;
  }
  const min = Math.min(...headings.map((h) => +h.tagName[1]));
  for (const h of headings) {
    const a = document.createElement('a');
    a.className = 'outline-item';
    a.style.paddingLeft = 14 + (+h.tagName[1] - min) * 14 + 'px';
    a.textContent = h.textContent;
    a.dataset.target = h.id;
    a.title = h.textContent;
    outline.appendChild(a);
  }
  updateActiveOutline();
}

function updateActiveOutline() {
  if (!active || !document.body.classList.contains('outline-open')) return;
  const heads = active.headings();
  const top = active.scroller.getBoundingClientRect().top + 80;
  let current = null;
  for (const h of heads) {
    if (h.getBoundingClientRect().top <= top) current = h;
    else break;
  }
  if (!current && heads.length) current = heads[0];
  for (const a of outline.children) a.classList.toggle('active', !!current && a.dataset.target === current.id);
  const act = outline.querySelector('.active');
  if (act) {
    const box = outline.getBoundingClientRect();
    const r = act.getBoundingClientRect();
    if (r.top < box.top || r.bottom > box.bottom) act.scrollIntoView({ block: 'nearest' });
  }
}

let rafPending = false;
function onPaneScroll(pane) {
  if (pane === active && !rafPending) {
    rafPending = true;
    requestAnimationFrame(() => {
      rafPending = false;
      updateActiveOutline();
    });
  }
  sync.follow(pane);
}

// ── 나란히 스크롤 ───────────────────────────────────────────────────
// 사용자가 움직인 칸(leader)을 따라 다른 칸도 같은 자리로. '같은 자리'는 같은 제목(같은 글자, 같은 순번)을 찾아
// 그 제목 구간 안에서 같은 비율만큼 내려간 곳이다. 같은 제목이 없으면 문서 전체 비율로 맞춘다.
// 고친 판과 옛 판을 비교할 때 내용 길이가 달라도 같은 절끼리 맞춰 보이게 하려는 것.
const sync = {
  on: false,
  leader: null,

  position(p) {
    const s = p.scroller;
    const max = s.scrollHeight - s.clientHeight;
    const pos = { ratio: max > 0 ? s.scrollTop / max : 0, atEnd: max > 0 && s.scrollTop >= max - 2 };
    const heads = p.headings();
    const ys = heads.map((h) => p.yOf(h));
    let i = -1;
    while (i + 1 < heads.length && ys[i + 1] <= s.scrollTop + 1) i++;
    if (i >= 0) {
      const end = i + 1 < heads.length ? ys[i + 1] : s.scrollHeight;
      pos.key = headingKey(heads, i);
      pos.frac = (s.scrollTop - ys[i]) / Math.max(1, end - ys[i]);
    }
    return pos;
  },

  apply(q, pos) {
    const s = q.scroller;
    const max = s.scrollHeight - s.clientHeight;
    let y = pos.ratio * max;
    if (pos.atEnd) y = max;
    else if (pos.key) {
      const heads = q.headings();
      const i = heads.findIndex((_, k) => headingKey(heads, k) === pos.key);
      if (i >= 0) {
        const start = q.yOf(heads[i]);
        const end = i + 1 < heads.length ? q.yOf(heads[i + 1]) : s.scrollHeight;
        y = start + pos.frac * (end - start);
      }
    }
    s.scrollTop = Math.max(0, Math.min(max, y));
  },

  follow(p) {
    if (!this.on || panes.size < 2 || this.leader !== p) return;
    const pos = this.position(p);
    for (const q of panes.values()) if (q !== p) this.apply(q, pos);
  },

  // 편집기(소스 줄 기준)가 리더. 같은 파일 경로의 보기 칸 들을 data-sl 로 찾아 그 블록으로 스크롤한다.
  fromEditor(editPane, line, kind) {
    if (!editPane.path) return;
    for (const q of panes.values()) {
      if (q === editPane || q.isEditing() || q.path !== editPane.path) continue;
      const el = findBlockBySourceLine(q.write, line);
      if (!el) continue;
      const scroller = q.scroller;
      const top = scroller.getBoundingClientRect().top;
      const y = el.getBoundingClientRect().top - top + scroller.scrollTop;
      scroller.scrollTop = Math.max(0, y - (kind === 'cursor' ? scroller.clientHeight * 0.3 : 0));
    }
  },
};

// 프리뷰 DOM 에서 소스 줄 번호(data-sl) 로 블록을 찾는다. 정확한 줄이 없으면 그 바로 아래의 가장 가까운 블록.
function findBlockBySourceLine(root, line) {
  const nodes = root.querySelectorAll('[data-sl]');
  let best = null;
  let bestSl = -1;
  for (const n of nodes) {
    const sl = parseInt(n.dataset.sl, 10);
    if (Number.isFinite(sl) && sl <= line && sl > bestSl) {
      best = n;
      bestSl = sl;
    }
  }
  return best;
}

function headingKey(heads, i) {
  const text = heads[i].textContent.trim();
  let nth = 0;
  for (let k = 0; k < i; k++) if (heads[k].textContent.trim() === text) nth++;
  return text + '\u0000' + nth;
}

document.addEventListener('keydown', () => (sync.leader = active), true);

// ── 끌어다 놓기 — 놓일 자리 미리 보기 ─────────────────────────────────
// 칸이 남으면 마우스 위치에 따라 왼쪽·사이·오른쪽에 끼워 넣고, 꽉 찼으면 마우스 아래 칸을 바꾼다.
const dropzone = $('#dropzone');
const drag = {
  names: [],

  slotAt(x) {
    const list = ordered();
    const n = list.length;
    const box = content.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (x - box.left) / Math.max(1, box.width)));
    if (n >= MAX_PANES) return { replace: list[Math.min(n - 1, Math.floor(frac * n))].id, at: 0 };
    return { at: Math.min(n, Math.round(frac * n)) };
  },

  enter(names, x) {
    this.names = names || [];
    this.over(x);
  },

  over(x) {
    if (!this.names.length) return;
    const slot = this.slotAt(x);
    const list = ordered();
    const label = this.names.length > 1 ? `${this.names[0]} 외 ${this.names.length - 1}개` : this.names[0];
    const cells = list.map((p) => {
      const name = p.el.querySelector('.pane-name').textContent;
      const hit = slot.replace === p.id;
      return `<div class="drop-cell${hit ? ' target' : ''}"><span>${escapeHtml(hit ? label : name)}</span>${
        hit ? '<em>이 칸을 바꿈</em>' : ''
      }</div>`;
    });
    if (slot.replace == null) {
      cells.splice(slot.at, 0, `<div class="drop-cell target"><span>${escapeHtml(label)}</span><em>여기에 열림</em></div>`);
    }
    dropzone.innerHTML = cells.join('');
    dropzone.hidden = false;
  },

  leave() {
    this.names = [];
    dropzone.hidden = true;
  },

  drop(paths, x) {
    const had = this.names.length;
    const slot = had ? this.slotAt(x) : null;
    this.leave();
    if (!slot || !paths.length) return;
    call('drop_files', { paths, at: slot.at, replace: slot.replace ?? null });
  },
};

function escapeHtml(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
}

// ── 찾기 (⌘F) — 지금 칸 안에서 ──────────────────────────────────────
// 강조 표시는 CSS Custom Highlight API. 없는 웹뷰(오래된 리눅스 WebKitGTK)에서는 개수·이동만 된다.
const canHighlight = typeof Highlight === 'function' && !!(window.CSS && CSS.highlights);
const find = {
  bar: $('#findbar'),
  input: $('#find-input'),
  count: $('#find-count'),
  ranges: [],
  index: -1,
  // 강조 객체는 하나를 계속 비우고 채운다 (새로 만들면 WebKit 이 옛 강조를 지우지 않는 잔상이 남음)
  hlAll: canHighlight ? new Highlight() : null,
  hlCurrent: canHighlight ? new Highlight() : null,

  open() {
    this.bar.hidden = false;
    this.input.focus();
    this.input.select();
    if (this.input.value) this.search(this.input.value, true);
  },
  close() {
    this.bar.hidden = true;
    this.ranges = [];
    this.index = -1;
    this.hlAll?.clear();
    this.hlCurrent?.clear();
    active?.scroller.focus({ preventScroll: true });
  },
  search(q, jump = true) {
    this.ranges = [];
    const needle = q.toLowerCase();
    if (needle && active) {
      const walker = document.createTreeWalker(active.write, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        const hay = node.data.toLowerCase();
        for (let i = hay.indexOf(needle); i !== -1; i = hay.indexOf(needle, i + needle.length)) {
          const r = new Range();
          r.setStart(node, i);
          r.setEnd(node, i + needle.length);
          this.ranges.push(r);
        }
      }
    }
    this.hlAll?.clear();
    for (const r of this.ranges) this.hlAll?.add(r);
    this.index = -1;
    if (this.ranges.length) {
      const top = active.scroller.getBoundingClientRect().top;
      const first = this.ranges.findIndex((r) => r.getBoundingClientRect().top >= top);
      this.index = first === -1 ? 0 : first;
    }
    this.show(jump);
  },
  step(dir) {
    if (this.bar.hidden) {
      this.open();
      return;
    }
    if (!this.ranges.length) return;
    this.index = (this.index + dir + this.ranges.length) % this.ranges.length;
    this.show(true);
  },
  show(jump) {
    const n = this.ranges.length;
    this.count.textContent = this.input.value ? (n ? `${this.index + 1}/${n}` : '없음') : '';
    this.bar.classList.toggle('no-match', !!this.input.value && !n);
    this.hlCurrent?.clear();
    if (this.index < 0) return;
    const r = this.ranges[this.index];
    this.hlCurrent?.add(r);
    if (jump && active) {
      const rect = r.getBoundingClientRect();
      const box = active.scroller.getBoundingClientRect();
      if (rect.top < box.top + 60 || rect.bottom > box.bottom - 40) {
        sync.leader = active;
        active.scroller.scrollBy(0, rect.top - box.top - box.height / 3);
      }
    }
  },
};
if (canHighlight) {
  CSS.highlights.set('mdv-find', find.hlAll);
  CSS.highlights.set('mdv-find-current', find.hlCurrent);
}
find.input.addEventListener('input', () => find.search(find.input.value, true));
find.input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    e.preventDefault();
    find.step(e.shiftKey ? -1 : 1);
  } else if (e.key === 'Escape') {
    e.preventDefault();
    find.close();
  }
});
$('#find-prev').addEventListener('click', () => find.step(-1));
$('#find-next').addEventListener('click', () => find.step(1));
$('#find-close').addEventListener('click', () => find.close());
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !find.bar.hidden) find.close();
  else if (e.key === 'F3') {
    // 윈도우 관례
    e.preventDefault();
    find.step(e.shiftKey ? -1 : 1);
  }
});

// ── 자체 점검 ───────────────────────────────────────────────────────
// 숫자는 첫 칸(샘플 문서) 기준. 칸이 여럿이면 칸 수와 너비도 남긴다.
async function report(mode, extra = {}) {
  const list = ordered();
  const write = list[0].write;
  const imgs = [...write.querySelectorAll('img')];
  await Promise.all(imgs.map((img) => img.decode().catch(() => {})));
  const count = (sel) => write.querySelectorAll(sel).length;
  await invoke('self_test_report', {
    report: {
      mode,
      userAgent: navigator.userAgent,
      headings: list[0].headings().length,
      tocItems: count('.md-toc-item'),
      codeBlocks: count('pre.md-fences'),
      highlightedTokens: count('pre.md-fences [class^="cm-"]'),
      tables: count('table'),
      math: count('.md-math-block mjx-container'),
      diagrams: count('.md-diagram > svg'),
      footnotes: count('.md-def-footnote'),
      images: imgs.map((img) => ({ src: img.getAttribute('src').slice(0, 120), loaded: img.complete && img.naturalWidth > 0 })),
      errors: document.querySelectorAll('.write .md-error').length,
      logged,
      fontLoaded: document.fonts.check('16px "Pretendard"'),
      highlightApi: canHighlight,
      panes: list.length,
      paneWidths: list.map((p) => Math.round(p.el.getBoundingClientRect().width)),
      writeWidth: Math.round(write.getBoundingClientRect().width),
      ...extra,
    },
  });
}

// --scenario: 칸 둘로 시작해 닫기 → 끌어다 놓기 → 새 창으로 떼기를 껍데기 명령으로 해 보고, 단계마다 칸 수·창 너비를 남긴다
async function scenario() {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const until = async (ok) => {
    for (let i = 0; i < 100 && !ok(); i++) await sleep(100);
    await sleep(500); // 창 크기 바뀜이 화면에 닿을 때까지
  };
  const steps = [];
  const snap = (step) => steps.push({ step, panes: panes.size, width: window.innerWidth });
  snap('start');
  const [first, second] = ordered();
  await invoke('close_pane', { pane: second.id });
  await until(() => panes.size === 1);
  snap('close');
  await invoke('drop_files', { paths: [second.path], at: 0, replace: null });
  await until(() => panes.size === 2 && ordered()[0] !== first && ordered()[0].write.childElementCount > 0);
  snap('drop');
  await invoke('pop_out_pane', { pane: ordered()[0].id });
  await until(() => panes.size === 1);
  snap('pop-out');
  return steps;
}

// ── 껍데기가 부르는 것들 ────────────────────────────────────────────
window.MDV = {
  changed: (id) => panes.get(id)?.reload(false),
  reload: (force) => panes.forEach((p) => p.reload(force)),
  goto: (id, anchor) => panes.get(id)?.scrollToAnchor(anchor),
  addPane: (id, index, mode) => {
    const pane = addPane(id, index, mode);
    pane.load('first').then(() => {
      if (mode === 'edit') pane.enterEdit();
    }).catch((e) => pane.fail(e));
  },
  replacePane: (oldId, newId) => {
    const old = panes.get(oldId);
    const index = old ? [...content.children].indexOf(old.el) : content.children.length;
    removePane(oldId);
    MDV.addPane(newId, index);
  },
  removePane,
  focusPane: (id) => {
    const pane = panes.get(id);
    if (!pane) return;
    setActive(pane);
    if (pane.editor) pane.editor.focus();
    else pane.scroller.focus({ preventScroll: true });
  },
  setSync: (on) => (sync.on = !!on),
  toggleOutline: () => setOutline(!document.body.classList.contains('outline-open')),
  find: { open: () => find.open(), next: () => find.step(1), prev: () => find.step(-1), close: () => find.close() },
  drag,

  // ── 편집 ────────────────────────────────────────────────────────
  save: () => active && active.editor && active.save(),
  saveAs: () => active && active.editor && active.saveAs(),
  toggleEdit: () => active && active.toggleEdit(),
  openEditPane: () => {
    if (!active || !active.path) return;
    call('add_edit_pane', { path: active.path });
  },
  setEditUiMode: (mode) => {
    editUiMode = mode === 'pane' ? 'pane' : 'toggle';
    document.body.dataset.editUiMode = editUiMode;
  },
  // '모든 칸에 저장 안 된 변경이 없는가' — 창 닫기 전 확인에 쓴다 (껍데기 측이 미리 묻는다면)
  isAnyDirty: () => [...panes.values()].some((p) => p.dirty),
};

// 글자를 골랐을 때만 웹뷰 기본 메뉴(복사 등). 빈 곳 우클릭의 '새로 고침·뒤로' 같은 항목은 감춘다.
document.addEventListener('contextmenu', (e) => {
  if (!String(window.getSelection())) e.preventDefault();
});

(async () => {
  const layout = await invoke('get_layout');
  selfTest = layout.selfTest && (layout.scenario ? 'scenario' : 'report');
  sync.on = layout.sync;
  MDV.setEditUiMode(layout.editUiMode || 'toggle');
  setOutline(layout.outline, false);
  layout.panes.forEach((id, i) => addPane(id, i));
  setActive(panes.get(layout.active) || ordered()[0]);
  await Promise.all([...panes.values()].map((p) => p.load('first').catch((e) => p.fail(e))));
  ready = true;
  active?.scroller.focus({ preventScroll: true });
  if (selfTest === 'scenario') await report('scenario', { scenario: await scenario() });
  else if (selfTest) {
    let roundtrip = null;
    if (layout.roundtrip) {
      try {
        roundtrip = await invoke('roundtrip_check_cmd', { path: layout.roundtrip });
      } catch (e) {
        roundtrip = { ok: false, error: String(e) };
      }
    }
    await report('first', roundtrip ? { roundtrip } : {});
  }
})().catch(logError);
