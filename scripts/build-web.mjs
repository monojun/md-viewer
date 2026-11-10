// 웹 화면(web/)을 dist/ 로 묶는다. Tauri 가 이 폴더를 앱 안에 넣는다.
// node_modules 위치: MDV_NODE_MODULES 환경변수(맥 로컬 개발은 ~/.cache 에 둔다) 또는 저장소의 node_modules.
import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const nm = process.env.MDV_NODE_MODULES || path.join(root, 'node_modules');
const require = createRequire(path.join(nm, '_'));
const esbuild = require('esbuild');
const dist = path.join(root, 'dist');

fs.rmSync(dist, { recursive: true, force: true });
fs.mkdirSync(path.join(dist, 'fonts'), { recursive: true });
fs.mkdirSync(path.join(dist, 'vendor'), { recursive: true });

await esbuild.build({
  entryPoints: [path.join(root, 'web/src/viewer.js')],
  bundle: true,
  minify: true,
  format: 'iife',
  target: ['es2020', 'safari15'],
  outfile: path.join(dist, 'viewer.bundle.js'),
  nodePaths: [nm],
  logLevel: 'warning',
});

const copy = (from, to) => fs.copyFileSync(from, path.join(dist, to));
copy(path.join(root, 'web/viewer.html'), 'viewer.html'); // CSP 는 tauri.conf.json 이 건다
copy(path.join(root, 'web/theme.css'), 'theme.css');
copy(path.join(root, 'web/app.css'), 'app.css');
copy(path.join(root, 'web/fonts/pretendard.css'), 'fonts/pretendard.css');
copy(path.join(nm, 'pretendard/dist/web/variable/woff2/PretendardVariable.woff2'), 'fonts/PretendardVariable.woff2');
copy(path.join(nm, 'mathjax/es5/tex-svg-full.js'), 'vendor/tex-svg-full.js');
copy(path.join(nm, 'mermaid/dist/mermaid.min.js'), 'vendor/mermaid.min.js');
copy(path.join(nm, 'codemirror/lib/codemirror.css'), 'vendor/codemirror.css');
console.log('web → dist/');
