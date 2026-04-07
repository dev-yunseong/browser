# Rust 웹 브라우저 개발 계획 (고수준 아키텍처)

웹 브라우저를 처음부터 개발하는 것은 매우 거대하고 복잡한 프로젝트입니다. 성공적인 진행을 위해 여러 단계의 모듈로 나누어 점진적으로 개발하는 것을 권장합니다.

## 1. 초기 설정 및 기반 기술 검토 (Phase 1)
*   **프로젝트 초기화:** `cargo new browser`
*   **라이브러리(Crates) 선정:**
    *   **네트워크 (Networking):** `reqwest` (HTTP 요청 처리)
    *   **윈도우 및 이벤트 처리:** `winit`
    *   **2D 렌더링:** `wgpu`, `vello` 또는 `skia-safe`
    *   **기존 파서 활용 (선택사항):** `html5ever` (HTML), `cssparser` (CSS)

## 2. 네트워크 모듈 (Phase 2)
*   URL을 입력받아 HTTP/HTTPS GET 요청을 보내고 원시 HTML 텍스트를 다운로드하는 기능 구현.

## 3. 파싱 및 DOM 생성 (Phase 3)
*   **HTML 파서:** 다운로드한 HTML 문자열을 토큰화(Tokenization)하고 파싱하여 DOM(Document Object Model) 트리 구조를 구축.
*   **CSS 파서:** `<style>` 태그나 외부 CSS 파일을 파싱하여 스타일 규칙(Style Rules)을 메모리에 저장.

## 4. 스타일 트리 및 레이아웃 엔진 (Phase 4)
*   **스타일 트리 (Style Tree):** DOM 트리와 CSS 규칙을 결합하여 각 노드의 최종 스타일이 적용된 트리를 생성.
*   **레이아웃 엔진 (Layout Engine):** 스타일 트리를 순회하며 브라우저 창의 크기에 맞춰 각 요소의 정확한 좌표(x, y)와 크기(width, height)를 계산. (초기에는 단순한 블록(Block) 및 인라인(Inline) 레이아웃부터 시작)

## 5. 페인팅(Painting) 및 렌더링 (Phase 5)
*   계산된 레이아웃 박스들을 실제 화면의 픽셀로 변환.
*   배경색, 테두리, 텍스트, 이미지를 그래픽 API(`wgpu` 등)를 사용하여 화면에 출력.

## 6. 상호작용 및 UI (Phase 6)
*   주소창, 뒤로가기/앞으로가기 버튼 등 기본적인 브라우저 크롬(UI) 구현.
*   스크롤, 마우스 클릭 등 사용자 입력 이벤트 처리.

## 7. JavaScript 엔진 연동 (Phase 7 - 심화)
*   순수 Rust로 작성된 JS 엔진인 `Boa`를 연동하거나 `rusty_v8`을 사용하여 JavaScript 실행 환경 구축.
*   DOM 요소 조작 API 바인딩 제공.

---
**다음 단계 제안:**
가장 먼저 `cargo init`으로 프로젝트를 생성하고, URL을 입력받아 콘솔에 HTML 소스를 출력하는 간단한 **네트워크 클라이언트**부터 만들어보는 것을 추천합니다. 준비가 되시면 알려주세요!