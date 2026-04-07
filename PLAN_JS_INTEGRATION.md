# JavaScript 엔진 통합 및 동적 상호작용 설계 (Phase 1)

Aura Browser에 생명력을 불어넣기 위한 JavaScript 엔진 통합 첫 번째 단계 계획입니다.

## 1. 기술 선정: Boa Engine (Pure Rust)
- **이유:** C 바인딩 없이 순수 Rust로 구현되어 있어 안정적이며, 메모리 관리가 용이하고 브라우저 바이너리에 쉽게 포함할 수 있습니다.

## 2. 시스템 설계 (Architecture)
### 가. JS 실행 환경 (Runtime)
- `BrowserApp` 상태에 `boa_engine::Context`를 유지합니다.
- 페이지 로드 시마다 새로운 컨텍스트를 생성하거나 초기화하여 스크립트 격리를 보장합니다.

### 나. DOM-JS 브릿지 기초
- Rust의 DOM 트리 데이터를 JS에서 읽을 수 있는 인터페이스(Binding)를 설계합니다.
- 초기 단계 목표: `console.log`를 브라우저 콘솔(터미널)에 출력하도록 연결.

### 다. 스크립트 추출 및 실행
- HTML 파싱 과정에서 `<script>` 태그 내의 텍스트 콘텐츠를 추출합니다.
- 페이지 렌더링 직전에 추출된 코드를 Boa 엔진에서 실행합니다.

## 3. 구현 단계
1. **의존성 추가:** `cargo add boa_engine`.
2. **런타임 모듈 생성:** `src/js.rs` 파일 생성 및 Boa 엔진 초기화 로직 구현.
3. **콘솔 로그 바인딩:** JS의 `console.log`가 Rust의 `println!`을 호출하도록 바인딩.
4. **통합 실행:** `main.rs`에서 페이지 로드 시 스크립트를 찾아 실행하는 파이프라인 추가.

---

## 4. 테스트 및 검증
- **테스트 케이스:** `<script>console.log("Hello from Aura JS!");</script>`가 포함된 HTML을 로드했을 때 터미널에 메시지가 출력되는지 확인.
- **수치 연산:** JS에서 계산된 값이 정상적으로 로그에 찍히는지 확인.
