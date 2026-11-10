// 마크다운 → HTML.
//   - 한 줄 바꿈은 화면에서도 줄바꿈 (breaks)
//   - 인라인 $수식$·==형광펜==·~아래첨자~ 는 쓰지 않는다 (달러·물결표가 많은 일반 글이 깨지지 않게)
//   - 목록 항목은 항상 <p> 로 감싼다 (항목 사이 간격이 이 구조에서 나온다)
//   - 각주 정의는 [^1]: 모양 그대로 적힌 자리에, 참조는 회색 상자 위첨자
import MarkdownIt from 'markdown-it';
import footnote from 'markdown-it-footnote';
import taskLists from 'markdown-it-task-lists';
import { full as emoji } from 'markdown-it-emoji';
import frontMatter from 'markdown-it-front-matter';
import { highlight, escapeHtml } from './highlight.js';

export function slugify(text) {
  return text
    .trim()
    .toLowerCase()
    .replace(/[!"#$%&'()*+,./:;<=>?@[\\\]^`{|}~·]/g, '')
    .replace(/\s+/g, '-');
}

// $$ … $$ 블록 수식. 렌더는 viewer.js 가 MathJax 로 나중에 한다.
function mathBlock(state, startLine, endLine, silent) {
  let pos = state.bMarks[startLine] + state.tShift[startLine];
  let max = state.eMarks[startLine];
  if (state.sCount[startLine] - state.blkIndent >= 4) return false;
  if (state.src.slice(pos, pos + 2) !== '$$') return false;

  const first = state.src.slice(pos + 2, max).trim();
  let next = startLine;
  let content;
  if (first.length >= 2 && first.endsWith('$$')) {
    content = first.slice(0, -2);
  } else {
    const lines = first ? [first] : [];
    let found = false;
    for (next = startLine + 1; next < endLine; next++) {
      pos = state.bMarks[next] + state.tShift[next];
      max = state.eMarks[next];
      if (pos < max && state.sCount[next] < state.blkIndent) break;
      const line = state.src.slice(pos, max).trim();
      if (line.endsWith('$$')) {
        if (line.length > 2) lines.push(line.slice(0, -2));
        found = true;
        break;
      }
      lines.push(state.src.slice(state.bMarks[next] + state.blkIndent, max));
    }
    if (!found) return false;
    content = lines.join('\n');
  }
  if (silent) return true;

  state.line = next + 1;
  const token = state.push('math_block', 'div', 0);
  token.block = true;
  token.content = content;
  token.map = [startLine, state.line];
  token.markup = '$$';
  return true;
}

// [TOC] 단독 문단 → 목차 자리표시. 내용은 렌더 후 제목에서 채운다.
function tocRule(state) {
  const t = state.tokens;
  for (let i = 0; i < t.length - 2; i++) {
    if (t[i].type === 'paragraph_open' && t[i + 1].type === 'inline' && /^\[toc\]$/i.test(t[i + 1].content.trim()) && t[i + 2].type === 'paragraph_close') {
      const tok = new state.Token('html_block', '', 0);
      tok.content = '<div class="md-toc"></div>\n';
      t.splice(i, 3, tok);
    }
  }
}

// 각주 정의가 있던 자리에 표시를 남긴다 (footnote_tail 이 정의를 문서 끝으로 옮기기 전)
function footnoteSlots(state) {
  const t = state.tokens;
  for (let i = t.length - 1; i >= 0; i--) {
    if (t[i].type !== 'footnote_reference_open') continue;
    const tok = new state.Token('html_block', '', 0);
    tok.content = `<div class="fn-slot" data-label="${escapeHtml(t[i].meta.label)}"></div>\n`;
    t.splice(i, 0, tok);
  }
}

// 목록 안 문단을 항상 <p> 로 (tight list 도 loose 처럼)
function looseLists(state) {
  for (const tok of state.tokens) {
    if ((tok.type === 'paragraph_open' || tok.type === 'paragraph_close') && tok.hidden) tok.hidden = false;
  }
}

// 제목 id: 본문 링크(#제목)와 개요가 같은 값을 쓴다
function headingIds(state) {
  const used = new Map();
  const t = state.tokens;
  for (let i = 0; i < t.length; i++) {
    if (t[i].type !== 'heading_open') continue;
    const inline = t[i + 1];
    const text = (inline.children || []).filter((c) => c.type === 'text' || c.type === 'code_inline' || c.type === 'emoji').map((c) => c.content).join('');
    let id = slugify(text) || 'heading';
    const n = used.get(id) || 0;
    used.set(id, n + 1);
    if (n) id += '-' + n;
    t[i].attrSet('id', id);
  }
}

// 블록 토큰마다 원문 몇 번째 줄에서 왔는지 data-sl 로 적는다 (편집기 ↔ 프리뷰 스크롤 동기화)
// html_block 처럼 열고-닫는 짝이 없는 토큰도 있으므로 토큰 종류를 명시해 고른다.
const SL_TYPES = new Set([
  'heading_open', 'paragraph_open', 'blockquote_open', 'bullet_list_open',
  'ordered_list_open', 'list_item_open', 'table_open',
  'fence', 'code_block', 'math_block', 'hr', 'html_block',
]);
function sourceLines(state) {
  for (const tok of state.tokens) {
    if (tok.map && SL_TYPES.has(tok.type)) {
      tok.attrSet('data-sl', String(tok.map[0]));
    }
  }
}

export function createMarkdown() {
  const md = new MarkdownIt({ html: true, linkify: true, breaks: true, typographer: false });
  md.linkify.set({ fuzzyLink: false, fuzzyEmail: false });

  md.use(footnote);
  md.core.ruler.before('footnote_tail', 'footnote_slots', footnoteSlots);
  md.use(taskLists, { enabled: true });
  md.use(emoji, { shortcuts: {} });
  md.use(frontMatter, () => {});
  md.block.ruler.before('fence', 'math_block', mathBlock, { alt: ['paragraph', 'reference', 'blockquote', 'list'] });
  md.core.ruler.push('toc', tocRule);
  md.core.ruler.push('loose_lists', looseLists);
  md.core.ruler.push('heading_ids', headingIds);
  md.core.ruler.push('source_lines', sourceLines);

  const r = md.renderer.rules;

  r.front_matter = (tokens, idx) => `<pre class="md-meta-block">${escapeHtml(tokens[idx].meta)}</pre>\n`;
  // 수식·다이어그램 원문은 HTML 속성에 넣지 않고 env.sources 로 넘긴다
  // (DOMPurify 가 '-->' 같은 값이 든 속성을 지우기 때문)
  const source = (env, text) => (env.sources || (env.sources = [])).push(text) - 1;
  const slAttr = (tok) => {
    const sl = tok.attrGet('data-sl');
    return sl == null ? '' : ` data-sl="${sl}"`;
  };
  r.math_block = (tokens, idx, options, env) =>
    `<div class="md-math-block"${slAttr(tokens[idx])} data-source="${source(env, tokens[idx].content)}"></div>\n`;

  const fence = (tok, env) => {
    const lang = (tok.info || '').trim().split(/\s+/)[0] || '';
    const body = tok.content.replace(/\n$/, '');
    if (lang.toLowerCase() === 'mermaid')
      return `<div class="md-diagram"${slAttr(tok)} data-source="${source(env, body)}"></div>\n`;
    return `<pre class="md-fences"${slAttr(tok)}${lang ? ` lang="${escapeHtml(lang)}"` : ''}><code>${highlight(body, lang)}</code></pre>\n`;
  };
  r.fence = (tokens, idx, options, env) => fence(tokens[idx], env);
  r.code_block = (tokens, idx, options, env) => fence(tokens[idx], env);

  // 각주 — 정의를 [^이름]: 모양으로 보여 준다
  r.footnote_caption = (tokens, idx) => String(tokens[idx].meta.id + 1);
  r.footnote_ref = (tokens, idx, options, env, slf) => {
    const id = tokens[idx].meta.id + 1;
    const sub = tokens[idx].meta.subId > 0 ? ':' + tokens[idx].meta.subId : '';
    const label = escapeHtml(tokens[idx].meta.label || String(id));
    return `<sup class="md-footnote"><a href="#fn-${id}" id="fnref-${id}${sub}">${label}</a></sup>`;
  };
  r.footnote_block_open = () => '<div class="footnotes-area">\n';
  r.footnote_block_close = () => '</div>\n';
  r.footnote_open = (tokens, idx) => {
    const id = tokens[idx].meta.id + 1;
    const label = escapeHtml(tokens[idx].meta.label || String(id));
    return `<div class="md-def-footnote" id="fn-${id}" data-label="${label}"><span class="md-def-name"><span class="md-def-mark">[^</span>${label}<span class="md-def-mark">]:</span></span> `;
  };
  r.footnote_close = () => '</div>\n';
  r.footnote_anchor = (tokens, idx) => {
    const id = tokens[idx].meta.id + 1;
    const sub = tokens[idx].meta.subId > 0 ? ':' + tokens[idx].meta.subId : '';
    return ` <a href="#fnref-${id}${sub}" class="reversefootnote" title="본문으로">↩</a>`;
  };

  return md;
}

// front matter 의 root-url: 그림 같은 상대 경로의 기준 폴더 (예: root-url: ../assets)
export function rootUrlFromFrontMatter(src) {
  const m = /^---\r?\n([\s\S]*?)\r?\n---/.exec(src);
  if (!m) return null;
  const line = /^root-url:\s*(.+?)\s*$/m.exec(m[1]);
  return line ? line[1].replace(/^["']|["']$/g, '') : null;
}
