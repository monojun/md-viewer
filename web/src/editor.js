// 공통 편집기 — CodeMirror 5. Toggle·Pane UX 가 함께 쓴다.
// 역할: 원문 보관·입력 받기, 500ms 디바운스로 프리뷰 HTML 생성(markdown.js 재사용),
//      커서·스크롤이 바뀔 때 '지금 보고 있는 소스 줄'을 알려 준다 (프리뷰 쪽과 양방향 동기화에 쓴다).
import CodeMirror from 'codemirror/lib/codemirror.js';
import 'codemirror/mode/markdown/markdown.js';
import 'codemirror/mode/yaml/yaml.js';
import 'codemirror/mode/javascript/javascript.js';
import 'codemirror/mode/css/css.js';
import 'codemirror/mode/xml/xml.js';
import 'codemirror/mode/htmlmixed/htmlmixed.js';
import 'codemirror/addon/edit/continuelist.js';

const PREVIEW_DEBOUNCE_MS = 500;

export function createEditor(container, initialSource, callbacks = {}) {
  container.classList.add('mdv-editor');
  const cm = CodeMirror(container, {
    value: initialSource ?? '',
    mode: 'markdown',
    lineNumbers: false,
    lineWrapping: true,
    indentUnit: 2,
    tabSize: 4,
    smartIndent: false,
    viewportMargin: 30,
    // IME(한국어) 에서 조합 중 글자를 가로채지 않도록. CodeMirror 5 기본값이 환경별로 다르다.
    inputStyle: 'contenteditable',
    spellcheck: false,
    autofocus: false,
    extraKeys: {
      Enter: 'newlineAndIndentContinueMarkdownList',
      Tab: (c) => c.replaceSelection('  '),
    },
  });

  let previewTimer = 0;
  const firePreview = () => {
    clearTimeout(previewTimer);
    previewTimer = setTimeout(() => {
      try {
        callbacks.onPreview?.(cm.getValue());
      } catch (e) {
        callbacks.onError?.(e);
      }
    }, PREVIEW_DEBOUNCE_MS);
  };

  let suppressChange = false;
  cm.on('change', () => {
    if (suppressChange) return;
    callbacks.onChange?.(cm.getValue());
    firePreview();
  });

  // 커서가 움직인 줄을 프리뷰에 알린다 (프리뷰 쪽이 같은 소스 줄의 블록으로 스크롤한다)
  cm.on('cursorActivity', () => {
    const line = cm.getCursor().line;
    callbacks.onCursorLine?.(line);
  });

  // 편집기 자체 스크롤이 움직일 때 '맨 위에 보이는 소스 줄'. 프리뷰 쪽이 따라간다.
  cm.on('scroll', () => {
    const info = cm.getScrollInfo();
    const line = cm.lineAtHeight(info.top, 'local');
    callbacks.onScrollLine?.(line);
  });

  // 레이아웃이 완전히 만들어진 뒤에만 refresh 가 효과가 있다 (0 높이에서 만든 뒤 보여 주면 비어 보인다)
  requestAnimationFrame(() => cm.refresh());

  return {
    cm,

    // 저장 후 외부에서 다시 읽은 바이트를 반영한다 (사용자 편집이 아닌 외부 원본 갱신)
    applyExternalSource(text) {
      const cursor = cm.getCursor();
      const scroll = cm.getScrollInfo();
      suppressChange = true;
      cm.setValue(text ?? '');
      suppressChange = false;
      try {
        cm.setCursor(cursor);
      } catch (_) {}
      cm.scrollTo(scroll.left, scroll.top);
      callbacks.onPreview?.(cm.getValue());
    },

    getSource: () => cm.getValue(),

    getCursorLine: () => cm.getCursor().line,

    lineCount: () => cm.lineCount(),

    scrollToLine(line) {
      const l = Math.max(0, Math.min(line, cm.lineCount() - 1));
      const y = cm.heightAtLine(l, 'local');
      cm.scrollTo(null, y);
    },

    setCursorLine(line) {
      const l = Math.max(0, Math.min(line, cm.lineCount() - 1));
      cm.setCursor({ line: l, ch: 0 });
    },

    focus: () => cm.focus(),

    refresh: () => cm.refresh(),

    destroy() {
      clearTimeout(previewTimer);
      const el = cm.getWrapperElement();
      el.parentNode?.removeChild(el);
    },
  };
}
