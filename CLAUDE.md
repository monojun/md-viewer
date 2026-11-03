# MD Viewer — Claude 안내

- **설치를 부탁받았다면** 소스를 빌드하지 말고 [INSTALL.md](INSTALL.md) 순서대로 GitHub Release 의 완성 파일을 받아 설치한다.
- **코드를 고친다면** README 의 "개발" 절을 본다. 화면은 `web/`, 앱 껍데기는 `src-tauri/src/main.rs` 하나다.
  화면 모양은 `web/theme.css` 의 변수(`--accent` 등)부터 바꾼다.
- 고친 뒤에는 `--self-test` 로 샘플 문서가 그대로 그려지는지 확인한다 (README "자체 점검").
- 새 버전: `package.json`·`src-tauri/Cargo.toml`·`src-tauri/tauri.conf.json` 의 version 을 함께 올리고, CHANGELOG.md 에 적은 뒤
  `vX.Y.Z` 태그를 push 하면 GitHub Actions 가 세 운영체제 설치 파일을 만들어 Release 에 올린다.
