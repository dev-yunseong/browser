# Aura Browser (GEMINI.md)

이 파일은 Aura Browser 프로젝트의 구조, 설정 및 개발 컨벤션을 안내하는 지침서입니다. 향후 AI 에이전트와의 협업 시 이 문서를 기반으로 문맥을 파악하십시오.

## 프로젝트 개요 (Project Overview)

**Aura Browser**는 Rust를 사용하여 밑바닥부터 구현 중인 웹 브라우저입니다. 네트워크 호출부터 HTML/CSS 파싱, 레이아웃 계산, 2D 렌더링, 그리고 JavaScript 실행 엔진까지 브라우저의 핵심 파이프라인을 직접 구현하는 것을 목표로 합니다.

### 핵심 기술 스택
- **언어:** Rust (Edition 2021)
- **GUI & Windowing:** `eframe` / `egui` (즉시 모드 GUI 프레임워크)
- **HTML 파싱:** `html5ever`, `markup5ever_rcdom` (Spec-compliant HTML5 parser)
- **CSS 파싱:** `cssparser` 기반 커스텀 구현
- **2D 렌더링:** `tiny-skia` (CPU 기반 고성능 래스터라이저)
- **JavaScript 엔진:** `boa_engine` (Pure-Rust JS interpreter)
- **네트워크:** `reqwest` (Blocking 모드 사용)
- **폰트 렌더링:** `ab_glyph`

### 아키텍처 파이프라인
1.  **Network:** `reqwest`를 통해 HTML 및 외부 CSS/이미지 리소스 취득.
2.  **DOM:** `html5ever`로 HTML을 파싱하여 `RcDom` 트리 생성 (`src/dom.rs`).
3.  **Style:** DOM 트리와 파싱된 CSS 규칙을 결합하여 `StyledNode` 트리 생성. 상속 및 선택자 명시도(Specificity) 계산 포함 (`src/style.rs`, `src/css.rs`).
4.  **Layout:** `StyledNode` 트리를 순회하며 각 요소의 크기와 위치(Rect)를 계산하여 `LayoutBox` 트리 생성. Block, Inline, Inline-Block, Table 등 지원 (`src/layout.rs`).
5.  **Render:** `LayoutBox` 트리를 순회하며 `tiny-skia`를 사용하여 Pixmap에 드로잉 (`src/render.rs`).
6.  **GUI:** 최종 렌더링된 Pixmap을 `egui` 텍스처로 변환하여 화면에 출력. 주소창, 탐색 버튼, 히스토리 관리 등 수행 (`src/main.rs`).

## 빌드 및 실행 (Building and Running)

프로젝트 루트 디렉토리에서 다음 명령어를 사용합니다.

```bash
# 프로젝트 빌드
cargo build

# 브라우저 실행
cargo run

# 최적화된 릴리스 모드로 실행 (렌더링 속도 향상)
cargo run --release

# 테스트 코드 실행
cargo test

# 코드 린트 체크
cargo clippy
```

## 개발 워크플로우 (Mandatory Workflow)

모든 개발 작업은 반드시 아래의 **Plan-Review-Act** 단계를 거쳐야 합니다.

1.  **Plan (계획):** 수행할 작업에 대한 상세한 기술적 계획을 수립합니다.
2.  **Review (검토):** 계획을 'Reviewer Agent'(`generalist` sub-agent)에게 전달하여 검토를 요청합니다.
3.  **Iteration (반복):** Reviewer가 'Pass'를 선언할 때까지 피드백을 반영하여 계획을 수정하고 재검토를 받습니다.
4.  **Development (개발):** Reviewer가 명시적으로 **'Pass'**라고 승인한 경우에만 실제 코드 수정을 시작합니다.


### 1. 언어 정책
- **내부 사고 및 코드 주석:** 영문 (English)
- **사용자 응답 및 사용자 대상 문서:** 국문 (Korean)
- **에이전트 메모리 및 지침:** 영문 (English)

### 2. 코드 품질 및 안정성
- **에러 핸들링:** 프로덕션 코드에서 `.unwrap()` 사용을 지양하고, 대신 `Result`나 `Option`을 적절히 처리하여 안정성을 확보하십시오.
- **테스트 필수:** 새로운 기능 구현 또는 버그 수정 시 반드시 `tests/` 디렉토리에 자동화된 테스트 케이스를 추가하십시오.
- **고정 뷰포트:** 현재 렌더링 너비는 **800px**로 고정되어 있습니다 (`src/main.rs`). 높이는 레이아웃 결과에 따라 가변적입니다.

### 3. 주요 모듈 역할
- `src/main.rs`: `eframe::App` 구현, 이벤트 루프, 비동기 리소스 로딩 관리.
- `src/css.rs`: CSS 토큰화 및 규칙 파싱.
- `src/style.rs`: 스타일 적용 및 상속 로직.
- `src/layout.rs`: 박스 모델 및 레이아웃 엔진.
- `src/render.rs`: Skia 기반 드로잉 로직.
- `src/js.rs`: Boa 엔진 래퍼 및 브라우저 API 모킹.

## 향후 로드맵 (Roadmap)
- [ ] 레이아웃 엔진 고도화 (Flexbox, Grid 지원 등)
- [ ] JavaScript DOM API 확장 (이벤트 리스너, 요소 조작 등)
- [ ] 폼 입력 및 상호작용성 강화
- [ ] 이미지 캐싱 최적화 및 점진적 렌더링 개선
