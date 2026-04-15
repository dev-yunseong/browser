# Aura Browser 아키텍처 2.0 및 실급 DOM 구현 리포트

현재의 단일 파이프라인 구조를 실제 브라우저 엔진(예: WebKit, Blink)의 구조와 유사하게 분리하고, 자바스크립트가 실제 화면을 조작할 수 있는 실질적인 DOM API 구현 방안을 제시합니다.

## 1. 엔진 모듈 분리 (Modularization)
기존의 소스 파일들을 논리적 단위로 그룹화하여 독립적인 라이브러리 형태로 관리합니다.

- **`aura-html`:** `html5ever`를 기반으로 하되, 에러 복구 및 실시간 스트리밍 파싱을 지원하는 독립 파서 모듈.
- **`aura-css`:** `cssparser` 크레이트를 도입하거나, 현재의 파서를 재작성하여 선택자 우선순위(Specificity)와 미디어 쿼리를 지원하는 전용 모듈.
- **`aura-dom`:** Rust의 `Rc<RefCell<Node>>` 구조를 넘어, JS가 참조하기 쉬운 고유 ID 기반의 Node 관리 시스템 구축.
- **`aura-layout`:** 플렉스박스(Flexbox) 및 그리드(Grid)를 지원하는 정교한 배치 엔진.

## 2. 실질적 DOM API 구현 (JS-Rust Bridge)
현재의 가짜(Mock) 객체를 대신하여, JS 명령어가 실제 Rust의 데이터 구조를 변경하도록 설계합니다.

### 가. 대상 핵심 객체
1. **`Document`:** `querySelector`, `createElement`, `body` 속성 구현.
2. **`Element`:** `innerHTML`, `style`, `getAttribute`, `setAttribute` 구현.
3. **`EventTarget`:** `addEventListener`, `removeEventListener`를 통한 이벤트 시스템 구축.

### 나. 작동 원리 (Binding Strategy)
- **Proxy Pattern:** Boa 엔진의 `NativeObject` 기능을 사용하여 JS의 `Element` 객체가 Rust의 `NodeIndex`를 가리키도록 합니다.
- **예시:** JS에서 `btn.style.backgroundColor = "red"` 호출 시 -> Rust의 해당 노드 스타일 맵이 즉시 갱신되고 -> 엔진이 `Dirty Flag`를 감지하여 해당 부분만 리렌더링.

## 3. 이벤트 루프 및 상호작용 (Interactivity)
검색 버튼 등이 작동하지 않는 이유는 `onclick` 핸들러가 Rust 엔진 내부로 전달되지 않기 때문입니다.

- **Event Queue 도입:** 사용자의 클릭 이벤트를 큐에 쌓고, JS 엔진이 이를 하나씩 꺼내어 등록된 콜백 함수를 실행하는 구조로 변경합니다.
- **Navigation Trigger:** JS에서 `location.href`를 변경하거나 `form.submit()` 호출 시, Rust의 네트워크 모듈이 새 URL을 로드하도록 이벤트를 전파합니다.

## 4. 구현 로드맵
1. **1단계:** `src/dom` 폴더를 생성하고 모든 노드 조작 로직을 스타일/레이아웃에서 분리.
2. **2단계:** `Boa`의 `NativeFunction`을 사용하여 `document.querySelector`를 실제 Rust DOM 검색과 연결.
3. **3단계:** 버튼 클릭 시 JS 함수가 실행되도록 하는 `Event Dispatcher` 구현.
