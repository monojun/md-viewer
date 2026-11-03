# MD Viewer 설치

GitHub 계정이나 로그인 없이 설치할 수 있다. 사람이 따라 해도 되고,
사용 중인 Claude 같은 AI 도우미에게 **"github.com/monojun/md-viewer 설치해줘"** 라고 하면 이 문서대로 설치한다.

> **AI 도우미에게**
> - 먼저 운영체제를 확인하고 해당 절만 따른다. 소스를 빌드하지 않는다.
> - `sudo` 가 들어간 줄은 비밀번호가 필요하므로 사용자에게 직접 실행해 달라고 한다.
> - ".md 를 항상 이 앱으로 열기" 는 사용자에게 원하는지 물어본 뒤에 안내한다.
> - 끝나면 사용자의 .md 파일 하나를 열어 보여 확인한다.

브라우저로 받으려면 [최신 Release 페이지](https://github.com/monojun/md-viewer/releases/latest)에서 운영체제에 맞는 파일을 받는다
(맨 아래 "브라우저로 받은 경우" 참고).

## 1. macOS (11 이상, 인텔·Apple 칩 공용)

```bash
mkdir -p /tmp/mdv && cd /tmp/mdv
URL=$(curl -fsSL https://api.github.com/repos/monojun/md-viewer/releases/latest | grep -o 'https://[^"]*universal\.dmg')
curl -fL -o md-viewer.dmg "$URL"
hdiutil attach -nobrowse -quiet -mountpoint /tmp/mdv/mnt md-viewer.dmg
osascript -e 'quit app id "io.github.monojun.mdviewer"' 2>/dev/null || true
rm -rf "/Applications/MD Viewer.app"
ditto "/tmp/mdv/mnt/MD Viewer.app" "/Applications/MD Viewer.app"
hdiutil detach -quiet /tmp/mdv/mnt
xattr -dr com.apple.quarantine "/Applications/MD Viewer.app" 2>/dev/null || true
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "/Applications/MD Viewer.app"
```

- `/Applications` 에 쓸 권한이 없으면 위 경로를 `~/Applications/MD Viewer.app` 로 바꾼다.
- 확인: `open -a "MD Viewer" <파일.md>`
- **.md 를 항상 이 앱으로 열기** — Finder 에서 .md 파일 하나 선택 → `⌘I`(정보 가져오기) → "다음으로 열기"에서 **MD Viewer** → **모두 변경…** → **계속**.
  (프로그램으로 바꾸면 확인 창이 여러 번 뜨고 결과가 일정하지 않아, 맥은 이 방법을 쓴다.)

## 2. Windows (10·11, 64비트)

PowerShell 에서:

```powershell
$ProgressPreference = 'SilentlyContinue'
$rel = Invoke-RestMethod https://api.github.com/repos/monojun/md-viewer/releases/latest
$asset = $rel.assets | Where-Object { $_.name -like '*x64-setup.exe' } | Select-Object -First 1
$setup = Join-Path $env:TEMP $asset.name
Invoke-WebRequest $asset.browser_download_url -OutFile $setup
Start-Process $setup -ArgumentList '/S' -Wait
```

- 관리자 권한 없이 사용자 폴더에 설치되고 시작 메뉴에 **MD Viewer** 가 생긴다.
- 화면 엔진 WebView2 는 Windows 10·11 에 보통 들어 있다. 없으면 설치 프로그램이 인터넷에서 받는다.
- **.md 를 항상 이 앱으로 열기** — Windows 는 앱이 기본 앱을 직접 바꿀 수 없다.
  .md 파일 우클릭 → **연결 프로그램 → 다른 앱 선택** → MD Viewer → **항상**.

## 3. Linux (x86_64)

**deb — Ubuntu 22.04 이상·Debian 계열 (권장)**

```bash
mkdir -p /tmp/mdv && cd /tmp/mdv
URL=$(curl -fsSL https://api.github.com/repos/monojun/md-viewer/releases/latest | grep -o 'https://[^"]*amd64\.deb')
curl -fL -o md-viewer.deb "$URL"
sudo apt install -y ./md-viewer.deb   # 비밀번호가 필요하다
md-viewer --set-default               # .md 기본 앱으로 (원할 때)
```

**AppImage — sudo 없이, 다른 배포판**

```bash
mkdir -p ~/Applications && cd ~/Applications
URL=$(curl -fsSL https://api.github.com/repos/monojun/md-viewer/releases/latest | grep -o 'https://[^"]*amd64\.AppImage')
curl -fL -o MD-Viewer.AppImage "$URL"
chmod +x MD-Viewer.AppImage
./MD-Viewer.AppImage <파일.md>
```

- Ubuntu 24.04 에서 AppImage 가 뜨지 않으면 `sudo apt install libfuse2t64` 가 필요하다.

## 업데이트 · 지우기

- **업데이트:** 같은 순서를 다시 실행하면 최신 버전으로 바뀐다.
- **지우기:** macOS — `/Applications/MD Viewer.app` 삭제 · Windows — 설정 › 앱 › MD Viewer 제거 · Linux — `sudo apt remove md-viewer`

## 브라우저로 받은 경우

이 앱은 코드 서명을 하지 않았다. 그래서 브라우저로 받은 파일은 처음 실행할 때 운영체제 경고가 뜬다
(위의 명령으로 받으면 대개 뜨지 않는다).

- macOS: "확인할 수 없는 개발자" → **시스템 설정 › 개인정보 보호 및 보안 › 그래도 열기**
- Windows: "Windows의 PC 보호" → **추가 정보 → 실행**
