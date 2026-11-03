// --self-test 결과(JSON)가 samples/sample.md 기준을 만족하는지 확인한다. CI 와 로컬 점검에 쓴다.
//   node scripts/check-selftest.mjs <report.json> <이름>
import fs from 'node:fs';

const [file, label = ''] = process.argv.slice(2);
const r = JSON.parse(fs.readFileSync(file, 'utf8'));
console.log(`[${label}]`, JSON.stringify(r, null, 2));

const expected = { headings: 11, tocItems: 11, codeBlocks: 4, tables: 1, math: 1, diagrams: 1, footnotes: 1, errors: 0 };
const problems = [];
if (r.timeout) problems.push('시간 초과 — 화면이 점검 결과를 보내지 않음');
else {
  for (const [k, v] of Object.entries(expected)) if (r[k] !== v) problems.push(`${k}: ${r[k]} (기대값 ${v})`);
  if (!(r.highlightedTokens > 20)) problems.push(`코드 색칠 토큰 수: ${r.highlightedTokens}`);
  if (!r.images?.length || !r.images.every((i) => i.loaded)) problems.push('그림을 못 읽음: ' + JSON.stringify(r.images));
  if (r.logged?.length) problems.push('화면 오류 기록: ' + r.logged.join(' | '));
}
if (problems.length) {
  console.error('✘ ' + problems.join('\n✘ '));
  process.exit(1);
}
console.log(`✔ [${label}] 자체 점검 통과`);
