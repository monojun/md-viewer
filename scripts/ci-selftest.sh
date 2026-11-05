#!/bin/bash
# CI(맥·리눅스): 앱을 --self-test 로 띄워 샘플 문서를 그리게 하고, 결과 JSON 과 화면 캡처를 selftest-<이름>/ 에 남긴다.
# 문서 하나짜리 창, 같은 샘플을 세 칸에 나란히 띄운 창, 칸 닫기·끌어다 놓기·새 창으로 떼기 시나리오를 차례로 점검한다.
#   bash scripts/ci-selftest.sh <실행 파일> <macos|linux>
set -uo pipefail
BIN="$1"
LABEL="$2"
OUT="selftest-$LABEL"
mkdir -p "$OUT"

run() { # <칸 수> <이름> [나란히 둘 문서…]
  local n="$1" name="$2"
  shift 2
  "$BIN" --self-test samples/sample.md --report "$OUT/$name.json" --hold 6 "$@" &
  local pid=$!
  for _ in $(seq 1 120); do
    [ -s "$OUT/$name.json" ] && break
    sleep 1
  done
  sleep 2
  case "$LABEL" in
    linux) import -window root "$OUT/$name.png" || true ;;
    macos) screencapture -x "$OUT/$name.png" || true ;;
  esac
  wait "$pid"
  node scripts/check-selftest.mjs "$OUT/$name.json" "$LABEL" "$n"
}

cp samples/sample.md "$OUT/second.md" # 시나리오용 두 번째 문서 (같은 창에 같은 파일은 두 번 열리지 않는다)
run 1 report && run 3 report-3 samples/sample.md samples/sample.md && run 1 scenario --scenario "$OUT/second.md"
