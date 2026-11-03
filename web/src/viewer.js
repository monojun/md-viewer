// 뷰어 화면. 앱 껍데기가 두 가지다.
//  - Tauri(맥·윈도우·리눅스, src-tauri/): 문서를 invoke('load_document') 로 받아 오고, 링크·설정도 invoke 로 보낸다.
//    껍데기 쪽에서는 MDV.changed()·MDV.reload() 등을 eval 로 부른다.
//  - 옛 Swift 맥판(legacy/): 셸이 MDV.render(text, opts) 로 내용을 넣고, webkit.messageHandlers.mdv 로 돌려받는다.
import DOMPurify from 'dompurify';
import { createMarkdown, slugify, rootUrlFromFrontMatter } from './markdown.js';

const md = createMarkdown();
const $ = (sel) => document.querySelector(sel);
const write = $('#write');
const outline = $('#outline');
const tauri = window.__TAURI__ || null;

const state = { baseURL: location.href, docURL: null, renderSeq: 0, sources: [], logged: [] };

function post(msg) {
  if (tauri) {
    const call = {
      openLink: () => tauri.core.invoke('open_link', { url: msg.url }),
      outline: () => tauri.core.invoke('set_outline', { open: msg.open }),
      log: () => tauri.core.invoke('log_message', { message: msg.message }),
    }[msg.type];
    if (call) call().catch((e) => console.warn('[mdv]', e));
    return;
  }
  try {
    window.webkit.messageHandlers.mdv.postMessage(msg);
  } catch (_) {
    console.log('[mdv]', msg);
  }
}

// 웹뷰 안 오류를 앱 로그(MDViewer.log)로
const logError = (what) => {
  const message = String(what && what.stack ? what.stack : what);
  state.logged.push(message.slice(0, 300));
  post({ type: 'log', message });
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

async function renderMath(seq) {
  const blocks = [...write.querySelectorAll('.md-math-block[data-source]')];
  if (!blocks.length) return;
  const MJ = await ensureMathJax();
  if (seq !== state.renderSeq) return;
  MJ.texReset();
  for (const el of blocks) {
    try {
      const node = await MJ.tex2svgPromise(state.sources[el.dataset.source], { display: true });
      if (seq !== state.renderSeq) return;
      el.replaceChildren(node);
    } catch (e) {
      logError(e);
      el.textContent = state.sources[el.dataset.source];
      el.classList.add('md-error');
    }
  }
}

let mermaidId = 0;
async function renderDiagrams(seq) {
  const blocks = [...write.querySelectorAll('.md-diagram[data-source]')];
  if (!blocks.length) return;
  const mermaid = await ensureMermaid();
  for (const el of blocks) {
    if (seq !== state.renderSeq) return;
    try {
      const { svg } = await mermaid.render('mdv-mermaid-' + ++mermaidId, state.sources[el.dataset.source]);
      el.innerHTML = svg;
    } catch (e) {
      logError(e);
      el.classList.add('md-error');
      el.textContent = (e && e.message ? e.message : String(e)).split('\n')[0];
    }
  }
  document.querySelectorAll('body > [id^="dmdv-mermaid-"]').forEach((n) => n.remove());
}

// ── 렌더 ────────────────────────────────────────────────────────────
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

// 그림 경로를 문서 위치 기준으로. Tauri 화면은 file: 을 직접 못 읽으므로 asset 주소로 바꾼다.
function fixRelativeUrls() {
  for (const img of write.querySelectorAll('img[src]')) {
    const src = img.getAttribute('src');
    let url;
    try {
      if (/^[A-Za-z]:[\\/]/.test(src)) url = new URL('file:///' + src.replace(/\\/g, '/'));
      else if (/^[a-z][a-z0-9+.-]*:/i.test(src) && !/^file:/i.test(src)) continue;
      else url = new URL(src, state.baseURL);
    } catch (_) {
      continue;
    }
    img.src = tauri && url.protocol === 'file:' ? tauri.core.convertFileSrc(localPath(url)) : url.href;
  }
}

// 각주 정의를 문서 끝이 아니라 원문에 적힌 자리로 (쓴 사람이 둔 위치 그대로)
function placeFootnotes() {
  for (const slot of write.querySelectorAll('.fn-slot')) {
    const def = write.querySelector(`.md-def-footnote[data-label="${CSS.escape(slot.dataset.label || '')}"]`);
    if (def) slot.replaceWith(def);
    else slot.remove();
  }
  const area = write.querySelector('.footnotes-area');
  if (area && !area.children.length) area.remove();
}

function buildToc(headings) {
  const tocs = write.querySelectorAll('.md-toc');
  if (!tocs.length) return;
  const html = headings
    .map((h) => `<a class="md-toc-item md-toc-${h.tagName.toLowerCase()}" href="#${encodeURIComponent(h.id)}"></a>`)
    .join('');
  for (const toc of tocs) {
    toc.innerHTML = `<div class="md-toc-content">${html}</div>`;
    toc.querySelectorAll('.md-toc-item').forEach((a, i) => (a.textContent = headings[i].textContent));
  }
}

function buildOutline(headings) {
  outline.replaceChildren();
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

function headingsOf() {
  return [...write.querySelectorAll('h1, h2, h3, h4, h5, h6')];
}

const SANITIZE = {
  ADD_TAGS: ['iframe'],
  ADD_ATTR: ['target', 'lang', 'align'],
  ALLOWED_URI_REGEXP: /^(?:(?:https?|ftps?|mailto|tel|file|data):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i,
};

async function render(text, opts = {}) {
  const seq = ++state.renderSeq;
  const keepScroll = !!opts.preserveScroll;
  const scrollY = window.scrollY;

  state.docURL = opts.docURL || null;
  state.baseURL = resolveBase(text, state.docURL) || location.href;
  if (opts.title) document.title = opts.title;
  if ('outline' in opts) setOutline(!!opts.outline, false); // 네이티브 Bool 은 숫자로 넘어온다

  const env = {};
  write.innerHTML = DOMPurify.sanitize(md.render(text, env), SANITIZE);
  state.sources = env.sources || [];
  placeFootnotes();
  fixRelativeUrls();
  const headings = headingsOf();
  buildToc(headings);
  buildOutline(headings);
  if (!find.bar.hidden && find.input.value) find.search(find.input.value, false);

  if (keepScroll) window.scrollTo(0, scrollY);
  else if (opts.anchor) scrollToAnchor(opts.anchor);
  else window.scrollTo(0, 0);

  await Promise.all([renderMath(seq), renderDiagrams(seq)]);
  if (seq !== state.renderSeq) return;
  if (keepScroll) window.scrollTo(0, scrollY);
  else if (opts.anchor) scrollToAnchor(opts.anchor);
  updateActiveOutline();
}

// ── 앵커·링크 ───────────────────────────────────────────────────────
function scrollToAnchor(raw) {
  let name = raw;
  try {
    name = decodeURIComponent(raw);
  } catch (_) {}
  let el = document.getElementById(name);
  if (!el) {
    const want = slugify(name);
    el = headingsOf().find((h) => h.id === want || slugify(h.textContent) === want) || null;
  }
  if (el) {
    el.scrollIntoView({ block: 'start' });
    updateActiveOutline();
  }
  return !!el;
}

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
      scrollToAnchor(item.dataset.target);
      return;
    }
    const a = e.target.closest('a[href]');
    if (!a) return;
    e.preventDefault();
    const href = a.getAttribute('href');
    if (href.startsWith('#')) {
      scrollToAnchor(href.slice(1));
      return;
    }
    let url;
    try {
      url = new URL(href, state.baseURL);
    } catch (_) {
      return;
    }
    post({ type: 'openLink', url: url.href });
  },
  true
);

// ── 개요(사이드바) ──────────────────────────────────────────────────
function setOutline(open, notify = true) {
  document.body.classList.toggle('outline-open', open);
  if (notify) post({ type: 'outline', open });
  updateActiveOutline();
}

let rafPending = false;
function updateActiveOutline() {
  if (!document.body.classList.contains('outline-open')) return;
  const heads = headingsOf();
  let current = null;
  for (const h of heads) {
    if (h.getBoundingClientRect().top <= 80) current = h;
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
window.addEventListener(
  'scroll',
  () => {
    if (rafPending) return;
    rafPending = true;
    requestAnimationFrame(() => {
      rafPending = false;
      updateActiveOutline();
    });
  },
  { passive: true }
);

// ── 찾기 (⌘F) ───────────────────────────────────────────────────────
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
  },
  search(q, jump = true) {
    this.ranges = [];
    const needle = q.toLowerCase();
    if (needle) {
      const walker = document.createTreeWalker(write, NodeFilter.SHOW_TEXT);
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
      const first = this.ranges.findIndex((r) => r.getBoundingClientRect().top >= 0);
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
    if (jump) {
      const rect = r.getBoundingClientRect();
      if (rect.top < 60 || rect.bottom > window.innerHeight - 40) {
        window.scrollBy(0, rect.top - window.innerHeight / 3);
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

// ── Tauri: 문서 받아 오기·다시 읽기·자체 점검 ───────────────────────────
// mode: 'first' 처음 열 때, 'changed' 파일이 바뀌었을 때(같은 내용이면 건너뜀), 'force' 메뉴의 다시 읽기
async function loadFromHost(mode) {
  const doc = await tauri.core.invoke('load_document', { reload: mode !== 'first', force: mode === 'force' });
  if (!doc) return;
  const opts = { title: doc.title, docURL: doc.docUrl, anchor: doc.anchor, preserveScroll: mode !== 'first' };
  if (mode === 'first') opts.outline = doc.outline;
  await render(doc.text, opts);
  // 자체 점검 중에는 다시 그릴 때마다 보고해서, 파일을 고치면 결과가 바뀌는지로 자동 새로고침도 확인한다
  if (doc.selfTest) await tauri.core.invoke('self_test_report', { report: { ...(await selfTestReport()), mode } });
}

let reloadTimer = 0;
function reload(force) {
  clearTimeout(reloadTimer);
  // 저장 한 번에 감시 이벤트가 여러 개 오므로 잠깐 모았다가 읽는다. 저장 도중이라 못 읽으면 다음 이벤트를 기다린다.
  reloadTimer = setTimeout(() => loadFromHost(force ? 'force' : 'changed').catch(() => {}), force ? 0 : 120);
}

async function selfTestReport() {
  const imgs = [...write.querySelectorAll('img')];
  await Promise.all(imgs.map((img) => img.decode().catch(() => {})));
  const count = (sel) => write.querySelectorAll(sel).length;
  return {
    userAgent: navigator.userAgent,
    headings: headingsOf().length,
    tocItems: count('.md-toc-item'),
    codeBlocks: count('pre.md-fences'),
    highlightedTokens: count('pre.md-fences [class^="cm-"]'),
    tables: count('table'),
    math: count('.md-math-block mjx-container'),
    diagrams: count('.md-diagram > svg'),
    footnotes: count('.md-def-footnote'),
    images: imgs.map((img) => ({ src: img.getAttribute('src').slice(0, 120), loaded: img.complete && img.naturalWidth > 0 })),
    errors: count('.md-error'),
    logged: state.logged,
    fontLoaded: document.fonts.check('16px "Pretendard"'),
    highlightApi: canHighlight,
    writeWidth: Math.round(write.getBoundingClientRect().width),
  };
}

window.MDV = {
  render,
  scrollToAnchor,
  reload,
  changed: () => reload(false),
  toggleOutline: () => setOutline(!document.body.classList.contains('outline-open')),
  find: { open: () => find.open(), next: () => find.step(1), prev: () => find.step(-1), close: () => find.close() },
};

if (tauri) {
  // 글자를 골랐을 때만 웹뷰 기본 메뉴(복사 등). 빈 곳 우클릭의 '새로 고침·뒤로' 같은 항목은 감춘다.
  document.addEventListener('contextmenu', (e) => {
    if (!String(window.getSelection())) e.preventDefault();
  });
  loadFromHost('first').catch((e) => {
    write.textContent = '문서를 열 수 없습니다: ' + e;
    logError(e);
  });
} else {
  post({ type: 'ready' });
}
