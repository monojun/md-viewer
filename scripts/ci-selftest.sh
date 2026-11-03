#!/bin/bash
# CI(맥·리눅스): 앱을 --self-test 로 띄워 샘플 문서를 그리게 하고, 결과 JSON 과 화면 캡처를 selftest-<이름>/ 에 남긴다.
#   bash scripts/ci-selftest.sh <실행 파일> <macos|linux>
set -uo pipefail
BIN="$1"
LABEL="$2"
OUT="selftest-$LABEL"
mkdir -p "$OUT"

"$BIN" --self-test samples/sample.md --report "$OUT/report.json" --hold 6 &
PID=$!
for _ in $(seq 1 120); do
  [ -s "$OUT/report.json" ] && break
  sleep 1
done
sleep 2
case "$LABEL" in
  linux) import -window root "$OUT/screen.png" || true ;;
  macos) screencapture -x "$OUT/screen.png" || true ;;
esac
wait "$PID"
node scripts/check-selftest.mjs "$OUT/report.json" "$LABEL"
