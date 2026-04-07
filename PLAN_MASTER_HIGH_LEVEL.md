# Aura Browser 수준 고도화 마스터 플랜 (Advanced Engineering)

전문적인 브라우저 엔진으로 거듭나기 위한 기술적 연구 결과와 단계별 구현 계획입니다.

## 1. 하이레벨 엔진 구성 요소 (Professional Requirements)
실제 브라우저 수준의 성능과 호환성을 위해 다음 시스템이 필요합니다.

- **리소스 로더 (Resource Loader):** 이미지, 폰트, 스크립트를 병렬로 다운로드하고 캐싱하는 통합 시스템.
- **레이어 합성 (Compositing):** 전체 페이지를 하나의 캔버스에 그리는 대신, 배경/콘텐츠/UI를 레이어별로 분리하여 GPU에서 합성 (성능 극대화).
- **이벤트 루프 (Event Loop):** JS의 마이크로태스크(Microtasks)와 렌더링 프레임 동기화를 위한 정교한 타이밍 제어.
- **보안 샌드박스 (Security Sandbox):** 사이트 간 데이터 유출을 막기 위한 프로세스 격리.

## 2. 이미지 출력 기능 구현 (Graphics)
- **태그 지원:** `<img>` 태그의 `src` 속성을 해석하여 리소스를 비동기로 가져옵니다.
- **디코딩:** `image` 크레이트를 사용하여 PNG, JPEG, WEBP 데이터를 픽셀 버퍼로 변환합니다.
- **레이아웃 연동:** 이미지의 가로/세로 비율을 계산하여 레이아웃 박스 크기를 결정합니다.
- **렌더링:** `tiny-skia`를 통해 지정된 좌표에 비트맵 데이터를 전송(Bitblt)합니다.

## 3. 표준 JavaScript 및 Web API 구현 (Standards)
Boa 엔진을 기반으로 W3C 표준에 가까운 객체들을 실제 Rust 로직과 연결합니다.

- **DOM Level 1~3:** `Node`, `Element`, `HTMLElement` 클래스 상속 구조 구현.
- **Web API:**
    - `Timer`: `setTimeout`, `setInterval` (비동기 루프 연동).
    - `Fetch`: `window.fetch`를 통한 동적 데이터 요청.
    - `Storage`: `localStorage`, `sessionStorage`.
- **브릿지 자동화:** 매번 수동으로 바인딩하는 대신, Rust 매크로를 사용하여 DOM 속성을 JS 객체로 자동 노출하는 시스템 구축.

## 4. 단기 실행 계획 (Immediate Action)
1. **이미지 엔진 탑재:** `main.rs`와 `render.rs`를 수정하여 실제 사진이 보이도록 구현.
2. **JS 엔진 표준화:** 가짜 객체를 실제 Rust DOM 노드를 참조하는 `JsObject`로 교체 시작.
