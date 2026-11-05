// --self-test 결과(JSON)가 samples/sample.md 기준을 만족하는지 확인한다. CI 와 로컬 점검에 쓴다.
//   node scripts/check-selftest.mjs <report.json> <이름> [칸 수]
//   이름이 macos… 이면 fn+control+화살표 배치에 필요한 '윈도우' 메뉴 등록도 본다.
import fs from 'node:fs';

const [file, label = '', panes = '1'] = process.argv.slice(2);
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
  if (r.panes !== Number(panes)) problems.push(`칸 수: ${r.panes} (기대값 ${panes})`);
  const widths = r.paneWidths || [];
  if (Math.max(...widths) - Math.min(...widths) > 2) problems.push('칸 너비가 고르지 않음: ' + widths.join(', '));
  if (r.scenario) {
    // 칸 둘 → 닫기 1 → 놓기 2 → 떼기 1, 창은 칸 수만큼 넓어졌다 좁아진다 (화면이 좁으면 덜 넓어질 수 있다)
    const got = r.scenario.map((s) => s.panes).join(',');
    if (got !== '2,1,2,1') problems.push(`시나리오 칸 수: ${got} (기대값 2,1,2,1)`);
    const [start, close, drop, pop] = r.scenario.map((s) => s.width);
    if (!(close < start && drop > close && pop < drop)) problems.push('시나리오 창 너비가 칸 수를 따르지 않음: ' + [start, close, drop, pop].join(', '));
    if (r.windows !== 2) problems.push(`떼어 낸 칸이 새 창이 되지 않음 (창 ${r.windows}개)`);
  }
  if (label.startsWith('macos') && !r.windowsMenu?.length) problems.push('맥 윈도우 메뉴가 등록되지 않음 (fn+control+화살표 배치가 안 됨)');
}
if (problems.length) {
  console.error('✘ ' + problems.join('\n✘ '));
  process.exit(1);
}
console.log(`✔ [${label}] 자체 점검 통과`);
