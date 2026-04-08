# Browser

**Browser**는 Vibe Coding으로 밑바닥부터 하나씩 쌓아 올린 웹 브라우저입니다. Rust의 강력함과 egui의 유연함을 결합하여, 네트워크 통신부터 HTML/CSS 파싱, 레이아웃 엔진, 그리고 JavaScript 실행까지 브라우저의 핵심 파이프라인을 직접 구현했습니다.

A browser built from scratch through Vibe Coding. It's not just an application; it's a journey into the heart of the web.

## Core Tech Stack
- **Language:** Rust (Edition 2021)
- **GUI & Windowing:** `eframe` / `egui` (Immediate-mode GUI framework)
- **HTML Parsing:** `html5ever`, `markup5ever_rcdom` (Spec-compliant HTML5 parser)
- **CSS Parsing:** Custom implementation based on `cssparser`
- **2D Rendering:** `tiny-skia` (High-performance CPU-based rasterizer)
- **JavaScript Engine:** `boa_engine` (Pure-Rust JS interpreter)
- **Networking:** `reqwest` (Blocking mode)
- **Font Rendering:** `ab_glyph`

## How to Run
본 프로젝트는 성능 최적화와 매끄러운 렌더링을 위해 **Release 모드**로 실행하는 것을 강력하게 권장합니다.

```bash
# Clone the repository
# git clone https://github.com/yunseong/browser.git
# cd browser

# Run in optimized release mode
cargo run --release
```

## Features
- **Network:** HTML 및 외부 CSS/Image 리소스를 `reqwest`로 가져옵니다.
- **DOM:** `html5ever`를 사용해 HTML을 파싱하여 DOM 트리를 구축합니다.
- **Style:** DOM 트리와 CSS 규칙을 결합하여 스타일 트리를 생성합니다.
- **Layout:** 스타일 트리를 기반으로 블록, 인라인, 인라인-블록 레이아웃을 계산합니다.
- **Render:** `tiny-skia`를 이용해 최종 픽셀 데이터를 그리고 `egui` 텍스처로 표시합니다.
- **JS Runtime:** `boa_engine`을 통합하여 기본적인 JavaScript 실행 환경을 제공합니다.

Enjoy the vibe of the web. 🚀
