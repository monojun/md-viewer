// 코드 블록 문법 색칠. CodeMirror 5 토크나이저를 정적으로 돌려(runMode)
// cm-keyword, cm-string … 클래스를 붙인다. 색은 theme.css 의 .cm-* 규칙이 정한다.
import CodeMirror from 'codemirror/lib/codemirror.js';
import 'codemirror/addon/runmode/runmode.js';
import 'codemirror/mode/meta.js';

import 'codemirror/mode/python/python.js';
import 'codemirror/mode/shell/shell.js';
import 'codemirror/mode/javascript/javascript.js';
import 'codemirror/mode/jsx/jsx.js';
import 'codemirror/mode/css/css.js';
import 'codemirror/mode/xml/xml.js';
import 'codemirror/mode/htmlmixed/htmlmixed.js';
import 'codemirror/mode/yaml/yaml.js';
import 'codemirror/mode/markdown/markdown.js';
import 'codemirror/mode/sql/sql.js';
import 'codemirror/mode/clike/clike.js';
import 'codemirror/mode/go/go.js';
import 'codemirror/mode/rust/rust.js';
import 'codemirror/mode/r/r.js';
import 'codemirror/mode/fortran/fortran.js';
import 'codemirror/mode/perl/perl.js';
import 'codemirror/mode/ruby/ruby.js';
import 'codemirror/mode/toml/toml.js';
import 'codemirror/mode/dockerfile/dockerfile.js';
import 'codemirror/mode/diff/diff.js';
import 'codemirror/mode/properties/properties.js';
import 'codemirror/mode/powershell/powershell.js';
import 'codemirror/mode/swift/swift.js';
import 'codemirror/mode/lua/lua.js';
import 'codemirror/mode/php/php.js';
import 'codemirror/mode/cmake/cmake.js';
import 'codemirror/mode/julia/julia.js';
import 'codemirror/mode/octave/octave.js';
import 'codemirror/mode/nginx/nginx.js';
import 'codemirror/mode/stex/stex.js';
import 'codemirror/mode/tcl/tcl.js';
import 'codemirror/mode/haskell/haskell.js';
import 'codemirror/mode/dart/dart.js';
import 'codemirror/mode/vb/vb.js';
import 'codemirror/mode/protobuf/protobuf.js';
import 'codemirror/mode/http/http.js';
import 'codemirror/mode/mathematica/mathematica.js';

export function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function findMode(lang) {
  const key = lang.trim();
  if (!key) return null;
  const lower = key.toLowerCase();
  return (
    CodeMirror.findModeByName(key) ||
    CodeMirror.findModeByExtension(lower) ||
    CodeMirror.findModeByMIME(lower) ||
    null
  );
}

export function highlight(code, lang) {
  const info = lang ? findMode(lang) : null;
  if (!info || info.mode === 'null' || !CodeMirror.modes[info.mode]) return escapeHtml(code);
  const spec = info.mime || (info.mimes && info.mimes[0]) || info.mode;
  let out = '';
  try {
    CodeMirror.runMode(code, spec, (text, style) => {
      const safe = escapeHtml(text);
      out += style ? `<span class="${style.split(/\s+/).map((s) => 'cm-' + s).join(' ')}">${safe}</span>` : safe;
    });
  } catch (_) {
    return escapeHtml(code);
  }
  return out;
}
