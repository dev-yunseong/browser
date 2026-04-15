# 렌더링/레이아웃 실패 분석 명세

작성일: 2026-04-08

## 목적

현재 Aura Browser에서 관찰되는 아래 증상에 대해, 코드에서 직접 확인되는 사실과 아직 검증이 필요한 증상 연결을 분리해 기록한다.

- 블록 내부가 검게 뭉개져 내용이 보이지 않음
- 콘텐츠 폭이 지나치게 좁아 실제 내용을 담지 못함
- 요소가 겹치거나 현대적인 사이트 레이아웃이 무너짐

이 문서는 수정안을 제안하는 문서가 아니라, 개발 시작 전에 원인 후보를 검증 가능한 형태로 고정하는 명세다.

## 사용 규칙

각 항목은 아래 3단계로만 기록한다.

1. `Code-confirmed fact`
2. `Condition to affect symptom`
3. `Validation case`

이 문서에서 `Code-confirmed fact`가 아닌 내용은 증상 연결을 위한 조건 또는 검증 항목으로만 취급한다.

## 핵심 직접 원인 요약

아래 항목들은 현재 코드에서 직접 확인되는, 상위 수준의 직접 원인이다.

1. Style resolution 단계에서 selector 구조가 보존되지 않는다.
2. Cascade 우선순위와 source order가 일부 왜곡된다.
3. Layout 단계는 제한된 속성만 소비하며 line-box / vertical-advance model 이 없다.
4. Render 단계에는 text glyph 좌표계 불일치와 shadow rectangle fill 경로가 존재한다.

## A. 렌더링 계층

### A1. Text glyph pixel coordinates do not include layout-space offset

Code-confirmed fact

- [src/render.rs](/home/yunseong/dev/browser/src/render.rs#L121) 에서 글리프를 `with_scale_and_position(scale, point(current_x, current_y))`로 생성한다.
- [src/render.rs](/home/yunseong/dev/browser/src/render.rs#L123) 에서 `outline.draw(|gx, gy, coverage| ...)`가 넘겨주는 `gx`, `gy`를 그대로 픽스맵 좌표로 사용한다.
- 현재 브라우저 렌더러는 이 콜백 좌표를 별도 page-space 보정 없이 픽스맵 픽셀 좌표로 사용한다.

Condition to affect symptom

- 로컬 `ab_glyph` 구현 [/home/yunseong/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ab_glyph-0.2.32/src/outlined.rs](/home/yunseong/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ab_glyph-0.2.32/src/outlined.rs#L100) 은 `offset = self.glyph.position - self.px_bounds.min` 를 사용해 glyph bounds 내부 rasterization을 수행하므로, 현재 렌더러가 `gx`, `gy`를 그대로 page-space 픽셀로 쓰는 방식은 좌표계 불일치를 만들 가능성이 높다.
- 동일 프레임 안에서 여러 glyph가 실제 레이아웃 위치와 다른 픽셀 위치에 누적되면, 텍스트가 엉킨 덩어리처럼 보일 수 있다.
- 이 항목만으로 "검은 블록" 증상의 지배 원인이라고는 아직 단정하지 않는다.

Validation case

- 단일 텍스트 노드 하나를 멀리 떨어진 두 위치에 각각 렌더했을 때, 두 텍스트 픽셀이 실제 박스 위치에 그려지는지 이미지 비교로 확인한다.
- 동일 텍스트를 여러 박스에 배치했을 때 좌상단 근처로 픽셀이 몰리는지 캡처한다.

### A2. `box-shadow` is rendered as a filled rectangle, not as a blurred shadow

Code-confirmed fact

- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L246) 이후에서 `box-shadow`는 `offset_x`, `offset_y`, `blur`, `spread`, `color`, `inset`까지 파싱된다.
- 그러나 [src/render.rs](/home/yunseong/dev/browser/src/render.rs#L20) 이후 렌더링은 blur/spread 를 사용하지 않는다.
- [src/render.rs](/home/yunseong/dev/browser/src/render.rs#L24) 에서 요소와 같은 크기의 사각형을 한 번 `fill_rect` 한다.
- 즉 현재 구현은 CSS shadow가 아니라 "오프셋된 반투명 직사각형 채우기"다.

Condition to affect symptom

- 요소 배경이 투명하거나 shadow가 요소 밖으로 드러나는 경우, 반투명 직사각형이 그대로 보일 수 있다.
- 이 조건이 맞으면 검은 면적 또는 어두운 직사각형성 아티팩트가 생길 수 있다.

Validation case

- `background: transparent; box-shadow: 0 0 20px rgba(0,0,0,.6)` 같은 최소 재현 페이지에서 사각형 채움이 보이는지 확인한다.
- 불투명 배경 박스와 투명 배경 박스를 각각 렌더해 shadow 노출 차이를 캡처한다.

### A3. Backgrounds are painted as solid rectangles for each layout box

Code-confirmed fact

- [src/render.rs](/home/yunseong/dev/browser/src/render.rs#L31) 이후에서 배경은 `background-color` 또는 `background`가 `Value::Color`인 경우 요소 박스 크기 그대로 `fill_rect` 된다.
- 배경 렌더링은 radius나 clipping 없이 현재 박스 경계에 직접 적용된다.

Condition to affect symptom

- 잘못 계산된 박스 크기나 좌표가 있을 경우, 배경도 그 잘못된 박스 전체를 그대로 칠한다.

Validation case

- 투명해야 하는 요소와 불투명 배경 요소를 섞은 최소 케이스에서 잘못된 박스 크기만큼 배경이 칠해지는지 확인한다.

## B. 폭 축소와 레이아웃 붕괴를 만드는 구조

### B1. Render width is hardcoded to `800px`

Code-confirmed fact

- [src/main.rs](/home/yunseong/dev/browser/src/main.rs#L189) 에서 렌더 폭은 항상 `800u32`다.
- [src/main.rs](/home/yunseong/dev/browser/src/main.rs#L190) 이후 전체 레이아웃은 이 폭을 기준으로 계산된다.

Condition to affect symptom

- 사용자 창 폭이 800보다 넓어도 콘텐츠 레이아웃은 항상 800 기준으로만 계산된다.
- 따라서 더 넓은 뷰포트를 전제한 페이지는 조기 줄바꿈, 압축, 겹침이 생길 수 있다.

Validation case

- 동일 페이지를 실제 창 폭 800과 1400에서 렌더했을 때, 현재 엔진 산출 이미지가 동일한지 확인한다.

### B2. `@rule` blocks are removed before CSS parsing

Code-confirmed fact

- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L77) 에서 `strip_at_rules`를 호출한다.
- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L131) 이후 구현은 `@media`, `@supports`, `@keyframes` 등 `@rule`을 내용째 제거한다.

Condition to affect symptom

- 반응형 또는 조건부 레이아웃 규칙이 전부 사라지면, 페이지는 의도와 다른 기본 규칙만으로 배치된다.

Validation case

- `@media (min-width: 1000px)` 안에서만 폭/정렬이 바뀌는 테스트 페이지를 만들고, 해당 규칙이 완전히 무시되는지 확인한다.

### B3. Many CSS units are parsed but not consumed by layout

Code-confirmed fact

- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L283) 에서 `vw`, `vh`, `em`, `%` 를 파싱한다.
- `rem`은 별도 단위로 보존되지 않고 `Unit::Em`으로 흡수된다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L131) 과 [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L227) 의 실제 계산은 `Px`와 `%`만 반영한다.
- 따라서 `margin`, `padding`, `width` 등은 `em`/`rem`/`vw`/`vh` 선언일 때 레이아웃 단계에서 대부분 소비되지 않는다.

Condition to affect symptom

- spacing, width, padding이 사라지면 콘텐츠가 과도하게 붙거나 좁아질 수 있다.

Validation case

- `padding: 2rem`, `width: 50vw`, `margin-left: 3em` 같은 속성을 각각 단독 테스트해 계산 결과가 0 또는 미적용 상태로 떨어지는지 확인한다.

### B4. `font-size` resolution differs between layout and paint

Code-confirmed fact

- [src/render.rs](/home/yunseong/dev/browser/src/render.rs#L88) 에서 페인트 단계는 `Value::Length(v, _)` 이면 단위와 무관하게 `v`를 그대로 픽셀 font size로 사용한다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L195) 에서 텍스트 레이아웃은 `font-size`를 `get_prop`로 읽는다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L227) 이후 `get_prop`는 `Px` 와 `%`만 계산한다.
- 따라서 `em` / `rem` / `vw` / `vh` 기반 `font-size`는 레이아웃 단계와 페인트 단계가 서로 다른 값을 볼 수 있다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L227) 이후 `get_prop`는 `%`를 항상 `cw * (v / 100.0)` 으로 계산한다.
- CSS `font-size: %`는 부모 글꼴 크기 기준이어야 하는데, 현재 구현은 컨테이너 폭 기준으로 계산한다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L195) 에서 텍스트 레이아웃은 `font_size`를 최소 `16.0`으로 clamp 한다.
- 즉 `font-size: 12px` 같은 지원되는 px 값조차 layout 단계와 paint 단계가 다른 값을 볼 수 있다.

Condition to affect symptom

- 텍스트의 실제 래핑 폭, 줄 수, 페인트 크기가 서로 어긋날 수 있다.

Validation case

- 부모 `font-size: 20px`, 자식 `font-size: 50%` 케이스에서 실제 산출이 10px가 아니라 컨테이너 폭 기반 숫자로 튀는지 확인한다.
- `font-size: 2em`, `font-size: 10vw` 같은 케이스에서 레이아웃 단계와 페인트 단계의 계산값이 다른지 비교한다.
- `font-size: 12px` 같은 케이스에서 layout은 16px, paint는 12px로 분리되는지 확인한다.

### B5. Layout only consumes a small subset of properties

Code-confirmed fact

- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L100) 이후 실제 레이아웃 계산이 읽는 핵심 속성은 `margin*`, `padding*`, `border-width`, `width`, `font-size`, `display` 정도다.
- `max-width`, `min-width`, `gap`, `grid-*`, `flex-basis`, `justify-content`, `align-items`, `position` 등은 reviewed files 기준으로 레이아웃 계산에 소비되지 않는다.

Condition to affect symptom

- 현대 페이지가 의존하는 레이아웃 규칙이 계산 단계에서 빠지면, 폭과 정렬은 기본 흐름만으로 결정된다.

Validation case

- 각 속성별로 최소 페이지를 만들고, 스타일 맵에는 들어가지만 레이아웃 결과에는 반영되지 않는지 확인한다.

### B6. Inline style shorthand values are not expanded

Code-confirmed fact

- [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L107) 이후의 `parse_inline_style`는 shorthand 분해 없이 값을 그대로 `parse_value`에 넘긴다.
- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L278) 의 `parse_value`는 `"12px 16px"` 같은 다중값을 `Keyword`로 저장한다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L227) 의 `get_prop`는 `Px` 와 `%`만 읽는다.

Condition to affect symptom

- 인라인 `padding`, `margin` shorthand가 레이아웃 계산에서 사실상 소실될 수 있다.

Validation case

- `style="padding: 12px 16px"` 와 `style="margin: 8px 16px"` 를 각각 테스트해 box model 계산값이 0인지 확인한다.

### B7. Non-block boxes do not participate in a line-box formatting model

Code-confirmed fact

- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L187) 과 [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L188) 에서 block이 아닌 박스는 기본적으로 `final_y = current_y`를 반환한다.
- `Inline`, `InlineBlock`, `Input`, `Image` 에 대한 line box 기반 배치가 없다.

Condition to affect symptom

- inline 흐름 전체가 줄 높이, 줄바꿈, 형제 배치를 보존하지 못하면 겹침과 흐름 붕괴가 생길 수 있다.

Validation case

- 텍스트, 버튼, 이미지가 섞인 inline 흐름에서 다음 줄로 내려가야 할 요소가 같은 `y`에 남는지 확인한다.

### B8. Text siblings do not advance the next sibling’s `y`

Code-confirmed fact

- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L192) 의 `layout_text` 는 높이를 계산한다.
- 하지만 [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L223) 에서 `current_y`를 그대로 반환한다.
- 부모는 [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L177) 과 [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L178) 에서 이 값을 다음 형제의 `child_y`로 재사용한다.

Condition to affect symptom

- 텍스트 뒤에 오는 형제가 줄 아래로 내려가야 하는 경우에도 같은 y에 배치될 수 있다.

Validation case

- 텍스트 뒤에 버튼/블록/이미지를 배치한 최소 페이지로 y 전진이 없는지 확인한다.

### B9. Explicit `height` is not consumed by layout

Code-confirmed fact

- CSS/inline으로 들어온 `height` 값은 `perform_layout`에서 읽히지 않는다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L185) 에서 비텍스트 박스 높이는 자식 결과와 최소 높이로 다시 계산된다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L185) 에서 비텍스트 박스 높이는 최소 `20.0` 으로 clamp 된다.

Condition to affect symptom

- 의도한 높이 대신 자식 내용 높이나 최소 높이로 박스가 변형될 수 있다.
- 작은 박스가 강제로 20px 이상으로 커지면, 배경과 shadow도 그 확장된 높이 기준으로 칠해질 수 있다.

Validation case

- `height: 200px` 고정 박스를 여러 종류의 자식 조합으로 테스트해 실제 높이가 무시되는지 확인한다.
- 내용이 거의 없는 작은 박스에서 실제 높이가 최소 20px로 강제되는지 확인한다.

### B10. `display` interpretation is limited and explicit values can be ignored

Code-confirmed fact

- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L237) 에서 명시적으로 인식하는 값은 `block`, `inline-block`, `flex`, `none` 정도다.
- 태그 기본값 목록에 `html`이 없어 [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L246) 이후 경로에서는 첫 `html` element 박스가 inline으로 분류된다.
- `grid`, `table-row`, `table-cell`, `list-item`, `inline` 같은 값은 별도 처리 없이 태그 기본값 분기로 떨어진다.
- 따라서 `div { display: inline }` 같은 경우, explicit `inline` 이 무시되고 태그 기본값인 block 으로 남을 수 있다.
- `display:inline`은 명시적으로 해석되지 않지만, 기본 display가 inline인 태그에서는 fallback 결과가 동일할 수 있다.

Condition to affect symptom

- 의도한 formatting context가 시작되지 않으면 전체 배치가 다른 모델로 계산된다.

Validation case

- `html`, `div { display: inline }`, `grid`, `list-item`, `table-row`, `inline-block` 최소 페이지를 각각 만들고 실제 display 분류를 출력한다.

### B11. Flex layout has a type but no flex formatting algorithm

Code-confirmed fact

- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L240) 에서 `display:flex` 는 `DisplayType::Flex` 로 분류된다.
- 그러나 `perform_layout` 내부에는 flex-specific 배치 알고리즘이 없다.
- [src/layout.rs](/home/yunseong/dev/browser/src/layout.rs#L260) 에서 `Flex` 는 block-level 로만 취급된다.

Condition to affect symptom

- flex container는 실제로는 일반 block 흐름처럼 배치된다.

Validation case

- `display:flex; justify-content:space-between` 최소 페이지를 만들고 자식이 row 분배되지 않는지 확인한다.

## C. CSS cascade / selector 문제

이 섹션의 핵심은 "선택자 일부를 못 읽는다"가 아니라, style resolution 단계가 selector 구조를 보존하지 못한다는 점이다. 즉 이 문제는 downstream layout 문제가 아니라, 레이아웃에 들어가기 전 스타일 결정 단계의 직접 결함이다.

### C1. Whitespace-delimited complex selectors are truncated to the last simple selector

Code-confirmed fact

- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L186) 에서 selector 파서는 whitespace-delimited selector에 대해 공백 기준 마지막 단순 selector만 취한다.
- `.card .title` 은 `.title` 로, `header nav a` 는 `a` 로 축약된다.

Condition to affect symptom

- 조합자 정보가 사라지면 원래 의도보다 넓거나 다른 범위에 rule이 적용될 수 있다.

Validation case

- `.card .title` 과 `.title` 외부 노드를 같이 둔 테스트에서 selector 축약 후 적용 범위를 비교한다.

### C2. Selector matching uses only the current node and does not evaluate selector structure

Code-confirmed fact

- [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L224) 의 `matches_selector` 는 현재 노드의 `tag`, `id`, `class` 만 비교한다.
- 조상, 부모-자식 관계, sibling 관계, combinator 구조는 전혀 검사하지 않는다.
- 즉 selector parsing 단계의 축약과 별개로, matching 단계 자체가 contextual selector 구조를 평가하지 않는다.

Condition to affect symptom

- descendant / child / contextual selector가 필요한 현대 페이지는, rule이 잘못 적용되거나 아예 적용되지 않을 수 있다.

Validation case

- `.card .title`, `main > section`, `nav a` 같은 selector를 각각 포함한 페이지에서, 조상 구조를 바꿔도 현재 엔진 결과가 동일한지 확인한다.

### C3. Empty selectors can become global matches

Code-confirmed fact

- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L189) 과 [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L201) 에서 `:where(...)`, `[data-x]` 같은 경우 최종 selector가 비어버릴 수 있다.
- [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L224) 이후 `matches_selector` 는 tag/id/class 제약이 모두 없으면 `true` 를 반환한다.

Condition to affect symptom

- 빈 selector는 전역 매칭처럼 동작할 수 있다.

Validation case

- `[data-x] { color: red }` 또는 `:where(.a) { ... }` 같은 규칙이 전역 적용되는지 확인한다.

### C4. Pseudo-classes and attribute selectors can broaden selectors even when they stay non-empty

Code-confirmed fact

- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L188) 에서 pseudo-class / pseudo-element 부분은 `split(':').next()` 로 잘린다.
- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L201) 에서 attribute selector를 만나면 그 뒤는 중단된다.
- 따라서 `a:hover` 는 `a` 로, `input[type=text]` 는 `input` 으로 축약될 수 있다.

Condition to affect symptom

- selector가 비어버리지 않더라도, 상호작용 상태나 attribute 조건이 사라진 채 더 넓은 범위에 rule이 적용될 수 있다.

Validation case

- `a:hover` 와 `input[type=text]` 규칙을 포함한 테스트 페이지에서 비상호작용 상태/다른 type input에도 rule이 적용되는지 확인한다.

### C5. Selector-list specificity inside one rule can be wrong

Code-confirmed fact

- [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L64) 이후에서 하나의 rule 안에 selector가 여러 개 있을 때, 매칭된 첫 selector의 specificity만 사용하고 `break` 한다.
- 따라서 같은 rule 안에 더 높은 specificity selector가 뒤에 있어도 반영되지 않을 수 있다.

Condition to affect symptom

- rule 내부 selector list 순서에 따라 최종 style 적용 강도가 달라질 수 있다.

Validation case

- 동일 rule 안에 low/high specificity selector를 함께 넣고, 현재 구현이 첫 매칭 selector specificity만 쓰는지 확인한다.

### C6. `<style>` / external `<link>` interleaving is not preserved

Code-confirmed fact

- [src/main.rs](/home/yunseong/dev/browser/src/main.rs#L175) 에서 먼저 모든 inline `<style>` 을 수집한다.
- [src/main.rs](/home/yunseong/dev/browser/src/main.rs#L177) 이후에서 모든 external stylesheet를 뒤에 이어붙인다.
- [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L237) 의 `<style>` 수집과 [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L254) 의 `<link>` 수집은 분리되어 있다.

Condition to affect symptom

- DOM 상에서 `<style>`, `<link>`, `<style>` 처럼 섞여 있던 실제 source order가 왜곡될 수 있다.

Validation case

- later inline `<style>` 이 earlier external CSS 를 덮어써야 하는 문서를 만들고, 현재 구현에서 순서가 뒤집히는지 확인한다.

### C7. Presentational attribute styles are applied after inline style

Code-confirmed fact

- [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L73) 에서 inline style 을 먼저 적용한다.
- [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L79) 에서 `apply_attribute_styles` 를 그 뒤에 적용한다.
- 현재 구현 범위에서는 `img`의 `width`/`height`, `font`의 `color`/`size` 같은 attribute 유래 스타일이 inline style 을 덮어쓸 수 있다.

Condition to affect symptom

- 개발자가 inline style 로 덮어쓰려 한 `img width/height` 또는 `font color/size` 값이 attribute 때문에 다시 바뀔 수 있다.

Validation case

- `<img width="300" style="width:100px">` 같은 케이스에서 최종 width가 100이 아니라 300으로 남는지 확인한다.

### C8. `!important` priority is dropped

Code-confirmed fact

- [src/css.rs](/home/yunseong/dev/browser/src/css.rs#L281) 에서 `!important` 는 문자열에서 제거만 된다.
- 중요도 정보는 별도 저장되지 않는다.
- [src/style.rs](/home/yunseong/dev/browser/src/style.rs#L72) 이후 적용은 중요도 정보 없이 단순 overwrite 다.

Condition to affect symptom

- `!important` 가 있어야 유지돼야 하는 색, 폭, 정렬이 일반 규칙에 덮일 수 있다.

Validation case

- 충돌하는 두 rule 중 하나에만 `!important` 를 붙인 테스트에서 현재 구현이 차이를 보이지 않는지 확인한다.

## 개발 시작 전 검증 케이스 목록

아래 케이스를 재현 테스트로 먼저 고정하면, 이후 수정이 실제로 무엇을 고쳤는지 분리해서 볼 수 있다.

1. Text glyph offset mismatch
2. Box-shadow rectangle fill behavior
3. Fixed 800px viewport
4. `@media` stripped entirely
5. `vw` / `vh` / `em` / `rem` / `% font-size` handling
6. Inline shorthand loss
7. Inline formatting collapse
8. Text sibling `y` not advancing
9. Explicit `height` ignored
10. `display` fallback and `html` element classification
11. Flex container treated as block flow
12. Complex selector truncation
13. Empty selector global match
14. Selector-list specificity bug
15. `<style>` / `<link>` source-order interleaving bug
16. Attribute style overriding inline style
17. `!important` ignored

## 개발 시작용 작업 묶음

이 문서가 다루는 항목은 아래 작업 묶음으로 나누는 것이 합리적이다.

### Workstream 1: Rendering correctness

- text glyph coordinate mismatch
- box-shadow rectangle-fill behavior

### Workstream 2: Layout correctness

- fixed viewport width
- inline formatting model absence
- text sibling y propagation
- height consumption
- `% font-size` calculation
- display classification
- flex formatting

### Workstream 3: CSS cascade and selector correctness

- `@rule` removal
- non-consumed units
- shorthand loss
- selector truncation
- empty selector global match
- selector-list specificity bug
- `<style>` / `<link>` source-order interleaving bug
- attribute style overriding inline style
- `!important` loss
