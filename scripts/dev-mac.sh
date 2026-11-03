#!/bin/bash
# 맥에서 직접 빌드하기 (개발용). 그냥 쓰려면 INSTALL.md — GitHub Release 의 완성 파일을 받는다.
#   scripts/dev-mac.sh               → 이 맥의 칩용 .app 빌드
#   scripts/dev-mac.sh --universal   → 인텔·Apple 칩 겸용
#   scripts/dev-mac.sh --install     → 빌드 후 /Applications 에 설치
# node_modules 와 Rust 빌드 산출물은 ~/.cache/mdviewer 에 둔다 (저장소가 클라우드 동기화 폴더에 있어도 무거운 파일이 동기화되지 않게).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

CACHE="$HOME/.cache/mdviewer"
NODE="$CACHE/node"
mkdir -p "$NODE"
if ! cmp -s package.json "$NODE/package.json" || ! cmp -s package-lock.json "$NODE/package-lock.json" || [ ! -d "$NODE/node_modules" ]; then
  cp package.json package-lock.json "$NODE/"
  (cd "$NODE" && npm ci --no-audit --no-fund --silent)
fi
export MDV_NODE_MODULES="$NODE/node_modules"
export CARGO_TARGET_DIR="$CACHE/target"

ARGS=(--bundles app)
OUT="$CARGO_TARGET_DIR/release"
if [[ " $* " == *" --universal "* ]]; then
  ARGS+=(--target universal-apple-darwin)
  OUT="$CARGO_TARGET_DIR/universal-apple-darwin/release"
fi
"$NODE/node_modules/.bin/tauri" build "${ARGS[@]}"
APP="$OUT/bundle/macos/MD Viewer.app"
echo "✔ 빌드: $APP"

if [[ " $* " == *" --install "* ]]; then
  if [ -w /Applications ]; then DEST="/Applications/MD Viewer.app"; else mkdir -p "$HOME/Applications"; DEST="$HOME/Applications/MD Viewer.app"; fi
  pkill -x md-viewer 2>/dev/null && sleep 0.5 || true
  rm -rf "$DEST"
  cp -R "$APP" "$DEST"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST" >/dev/null 2>&1 || true
  echo "✔ 설치: $DEST"
fi
