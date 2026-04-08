# Core Web Standards Study for Aura Browser Engine

## 1. Reference URLs
*   **WHATWG HTML Living Standard:** [https://html.spec.whatwg.org/multipage/](https://html.spec.whatwg.org/multipage/)
*   **W3C CSS Snapshot (Latest):** [https://www.w3.org/TR/css-2023/](https://www.w3.org/TR/css-2023/)
*   **WHATWG DOM Living Standard:** [https://dom.spec.whatwg.org/](https://dom.spec.whatwg.org/)
*   **MDN Web Docs Compatibility Tables:** [https://developer.mozilla.org/en-US/docs/Web/HTTP/Browser_compatibility](https://developer.mozilla.org/en-US/docs/Web/HTTP/Browser_compatibility) (Backed by [mdn/browser-compat-data](https://github.com/mdn/browser-compat-data))

## 2. Key Functional Requirements

### 2.1. Document Lifecycle and Parsing Rules
*   **Technical Requirements:**
    *   **Tokenization & Tree Construction:** Implement the HTML5 parsing algorithm, handling character encoding, state machines for tokenization (Data, Tag Open, Attribute Name, etc.), and tree construction rules.
    *   **Error Handling:** Support recovery from syntax errors as per the HTML5 spec (e.g., auto-closing tags, misnested tags).
    *   **Document States:** Manage `document.readyState` (`loading`, `interactive`, `complete`) and dispatch corresponding events (`DOMContentLoaded`, `load`).
*   **Explanation:**
    HTML 파싱은 단순히 태그를 읽는 것을 넘어, 잘못 작성된 마크업도 표준에 따라 복구(Error Recovery)해야 합니다. 문자열을 토큰으로 변환하는 Tokenization 과정과 이를 바탕으로 DOM 트리를 구성하는 Tree Construction 과정이 엄격한 상태 머신으로 구현되어야 합니다. 또한, 파싱 진행 상황에 따라 문서의 상태(Loading -> Interactive -> Complete)를 관리하고 적절한 이벤트를 발생시켜야 브라우저의 생명주기가 정상적으로 동작합니다.

### 2.2. CSS Cascade & Inheritance Rules
*   **Technical Requirements:**
    *   **Cascade Algorithm:** Compute final values by evaluating Origin & Importance (User Agent, User, Author), Context (Shadow DOM), Specificity (A, B, C counts), and Order of Appearance.
    *   **Inheritance:** Propagate inheritable properties (e.g., `color`, `font-family`) from parent to child nodes. Handle `inherit`, `initial`, `unset`, and `revert` keywords.
    *   **Value Computation:** Resolve values through the pipeline: Declared -> Cascaded -> Specified -> Computed -> Used -> Actual Values.
*   **Explanation:**
    CSS는 동일한 요소에 여러 스타일이 충돌할 때 어떤 스타일을 적용할지 결정하는 캐스케이드(Cascade) 알고리즘이 핵심입니다. 스타일의 출처, 중요도(!important), 선택자의 명시도(Specificity), 그리고 선언 순서를 기준으로 우선순위를 계산합니다. 또한, 폰트나 색상처럼 부모 요소의 스타일이 자식에게 전달되는 상속(Inheritance) 메커니즘을 정확히 구현해야 하며, 각종 단위(px, em, %)를 최종 픽셀 값으로 변환하는 파이프라인이 필요합니다.

### 2.3. Flexbox and Grid Layout Algorithms
*   **Technical Requirements:**
    *   **Flexbox (`display: flex`):** Implement 1D layout along Main and Cross axes. Handle flex lines, flex-grow/shrink resolution, and alignment (`justify-content`, `align-items`).
    *   **Grid (`display: grid`):** Implement 2D layout with tracks, lines, and areas. Handle track sizing algorithms (e.g., `minmax()`, `fr` units), grid item placement (auto-placement and explicit lines), and track alignment.
    *   **Box Model Integration:** Ensure these layouts correctly interact with margin, border, padding, and intrinsic/extrinsic sizing constraints.
*   **Explanation:**
    최신 웹 레이아웃의 필수 요소인 Flexbox와 Grid 알고리즘입니다. Flexbox는 1차원(행 또는 열) 방향으로 요소들을 배치하며, 가용 공간을 비율에 따라 분배(grow/shrink)하고 정렬하는 로직이 필요합니다. Grid는 2차원 격자 구조를 생성하여 복잡한 레이아웃을 구성하며, 트랙(행/열)의 크기를 계산하고 요소를 배치하는 알고리즘이 매우 복잡합니다. 두 시스템 모두 표준 박스 모델과 정확하게 상호작용해야 합니다.

### 2.4. Event Loop and DOM Manipulation APIs
*   **Technical Requirements:**
    *   **Event Loop:** Implement a non-blocking execution model with Task Queues (Macro-tasks like I/O, events) and Microtask Queues (Promises, MutationObservers).
    *   **DOM APIs:** Provide core interfaces (`Node`, `Element`, `Document`) for traversal (`parentNode`, `childNodes`) and manipulation (`appendChild`, `insertBefore`, `removeChild`).
    *   **Reactivity:** Connect DOM mutations to layout invalidation (setting "dirty" bits) to trigger selective re-rendering without blocking the main thread.
*   **Explanation:**
    브라우저의 심장인 이벤트 루프는 메인 스레드를 차단하지 않고 다양한 작업(사용자 입력, 네트워크 응답, JS 실행)을 처리하는 시스템입니다. 매크로 태스크와 마이크로 태스크 큐의 우선순위를 관리해야 합니다. DOM API는 자바스크립트가 웹 페이지를 동적으로 조작할 수 있게 해주는 인터페이스입니다. 노드 추가/수정/삭제 시 렌더링 엔진에 변경 사항을 알리고(Dirty bit 설정), 필요한 부분만 다시 레이아웃하고 그리는 반응형 구조가 필수적입니다.

## 3. Baseline for 7-Depth Issue Tree (Next Steps)
This study serves as the foundational **Depth 1 (Vision) & Depth 2 (Domain)** context. Future issues will branch out from these core domains (HTML Parsing, CSS Engine, Layout Algorithms, Event Loop/DOM API) into specific system implementations and granular tasks.
