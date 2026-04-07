# Aura Browser 차세대 로드맵 (Future Roadmap)

브라우저 엔진의 완성도를 높이고 사용자 경험을 개선하기 위한 중장기 개발 계획입니다.

## 1. 이미지 및 미디어 렌더링 (Graphics & Media)
- **비트맵 이미지 지원:** `<img>` 태그를 파싱하여 JPEG, PNG, WEBP 이미지를 화면에 출력.
- **이미지 캐싱:** 동일한 이미지를 여러 번 로드하지 않도록 메모리 내 캐시 시스템 구축.
- **SVG 지원:** 벡터 그래픽 렌더링 라이브러리 연동.

## 2. 레이아웃 엔진 고도화 (Advanced Layout)
- **Flexbox 지원:** 현대 웹 디자인의 핵심인 `display: flex`와 가로/세로 정렬 로직 구현.
- **반응형 디자인:** 윈도우 창 크기 변경에 따른 실시간 리레이아웃(Re-layout) 최적화.
- **z-index 및 레이어:** 요소들이 겹칠 때 앞뒤 순서를 결정하는 스태킹 컨텍스트(Stacking Context) 구현.

## 3. JavaScript 엔진 통합 (Interactivity)
- **Boa 엔진 연동:** Rust 기반 JS 엔진 `Boa`를 사용하여 기본적인 스크립트 실행 환경 구축.
- **DOM API 바인딩:** `document.querySelector`, `element.innerHTML` 등을 JS에서 조작할 수 있도록 Rust-JS 브릿지 설계.
- **이벤트 핸들러:** `onclick`, `onmouseover` 등 사용자 상호작용 스크립트 처리.

## 4. 브라우저 크롬(UI) 및 편의 기능
- **탭(Tab) 시스템:** 여러 페이지를 동시에 열고 전환할 수 있는 멀티 탭 기능.
- **북마크 및 기록:** 방문한 페이지 저장 및 관리 기능.
- **다크 모드:** 브라우저 UI 및 웹 콘텐츠에 대한 자동 다크 모드 테마 적용.

## 5. 성능 및 보안 (Performance & Security)
- **병렬 렌더링:** 렌더링 로직을 여러 스레드에서 처리하여 성능 향상.
- **샌드박싱(Sandboxing):** JS 실행 환경과 네트워크 통신을 격리하여 보안 강화.
