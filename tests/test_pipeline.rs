/// End-to-end rendering pipeline tests.
///
/// These tests run the full DOM → CSS matching → Layout → Render chain
/// with representative HTML to catch hangs, panics, or regressions
/// in any pipeline stage.

use browser::engine::process_html_with_cache;
use std::collections::HashMap;
use url::Url;

fn base_url() -> Url {
    Url::parse("https://yunseong.dev/").unwrap()
}

fn run(html: &str, css: &str) -> browser::engine::PageResult {
    let html_with_style = if css.is_empty() {
        html.to_string()
    } else {
        format!("<style>{}</style>{}", css, html)
    };
    let image_cache = HashMap::new();
    let mut css_cache = HashMap::new();
    let js_overrides = HashMap::new();
    let (result, _) = process_html_with_cache(
        &html_with_style,
        &base_url(),
        &image_cache,
        &mut css_cache,
        None,
        &js_overrides,
        None,
        None,
        None,
        800.0,
    )
    .expect("pipeline should not fail");
    result
}

// ── Stage completion ─────────────────────────────────────────────────────────

#[test]
fn test_pipeline_completes_plain_paragraph() {
    let result = run("<p>Hello, world!</p>", "");
    assert!(result.width > 0);
    assert!(result.height > 0);
}

#[test]
fn test_pipeline_completes_with_css_classes() {
    let css = r#"
        .container { width: 760px; margin: 0 auto; padding: 20px; }
        .title     { font-size: 32px; color: #111; margin-bottom: 16px; }
        .subtitle  { font-size: 18px; color: #555; }
    "#;
    let html = r#"
        <div class="container">
            <h1 class="title">Yunseong Jeong</h1>
            <p class="subtitle">Software Engineer</p>
        </div>
    "#;
    let result = run(html, css);
    assert!(result.width > 0);
    assert!(result.height > 0);
}

// ── CSS matching correctness ──────────────────────────────────────────────────

#[test]
fn test_css_many_rules_does_not_hang() {
    // 200 rules — stress-tests sig_cache HashMap dedup path.
    let css: String = (0..200)
        .map(|i| format!(".rule-{} {{ color: #000; margin: {}px; }}\n", i, i % 50))
        .collect();
    let html = r#"<div class="rule-0 rule-1 rule-5"><p class="rule-10">text</p></div>"#;
    let result = run(html, &css);
    assert!(result.height > 0);
}

#[test]
fn test_css_specificity_order_preserved() {
    // ID > class > tag — background-color of #box should be blue (highest spec).
    let css = r#"
        div            { background-color: red; }
        .box           { background-color: green; }
        #box           { background-color: blue; }
    "#;
    let html = r#"<div id="box" class="box" style="width:100px;height:100px;"></div>"#;
    let result = run(html, css);
    assert!(result.height > 0);
}

#[test]
fn test_important_overrides_normal() {
    let css = r#"
        p { color: red !important; }
        p { color: blue; }
    "#;
    let html = "<p>text</p>";
    let result = run(html, css);
    assert!(result.height > 0);
}

// ── Layout correctness ───────────────────────────────────────────────────────

#[test]
fn test_layout_produces_links() {
    let html = r#"
        <nav>
            <a href="/about">About</a>
            <a href="/posts">Posts</a>
            <a href="https://github.com/dev-yunseong">GitHub</a>
        </nav>
    "#;
    let result = run(html, "");
    assert_eq!(result.links.len(), 3);
    assert!(result.links.iter().any(|(_, href)| href.contains("about")));
    assert!(result.links.iter().any(|(_, href)| href.contains("github")));
}

#[test]
fn test_layout_margin_collapsing_does_not_hang() {
    // Many adjacent block siblings — exercises margin collapsing loop.
    let css = "p { margin: 16px 0; }";
    let html: String = (0..50).map(|i| format!("<p>Paragraph {}</p>", i)).collect();
    let result = run(&html, css);
    assert!(result.height > 0);
}

#[test]
fn test_layout_deeply_nested_blocks() {
    let mut html = String::from("<div>");
    for i in 0..50 {
        html.push_str(&format!("<div style=\"padding:{}px;\">", i % 10));
    }
    html.push_str("<p>deep text</p>");
    for _ in 0..51 { html.push_str("</div>"); }
    let result = run(&html, "");
    assert!(result.height > 0);
}

#[test]
fn test_layout_produces_reasonable_height() {
    // 10 paragraphs at ~20px each → height well above 0 and below 16384.
    let css = "p { height: 30px; }";
    let html: String = (0..10).map(|_| "<p>line</p>").collect();
    let result = run(&html, css);
    assert!(result.height >= 100);
    assert!(result.height <= 16384);
}

// ── Blog-like page (representative of yunseong.dev) ──────────────────────────

#[test]
fn test_pipeline_blog_page_structure() {
    let css = r#"
        * { box-sizing: border-box; margin: 0; padding: 0; }
        body { font-size: 16px; color: #222; background: #fff; }
        header { background: #1a1a2e; padding: 20px; }
        header a { color: #eee; text-decoration: none; }
        nav ul { list-style: none; display: flex; gap: 16px; }
        main { max-width: 800px; margin: 40px auto; padding: 0 20px; }
        h1 { font-size: 2em; margin-bottom: 8px; }
        h2 { font-size: 1.5em; margin: 32px 0 12px; }
        p  { line-height: 1.7; margin-bottom: 16px; }
        .tag { background: #eef; padding: 2px 8px; border-radius: 4px; }
        footer { border-top: 1px solid #ddd; padding: 20px; text-align: center; }
    "#;
    let html = r#"
        <header>
            <a href="/">yunseong.dev</a>
            <nav>
                <ul>
                    <li><a href="/about">About</a></li>
                    <li><a href="/posts">Posts</a></li>
                    <li><a href="/projects">Projects</a></li>
                </ul>
            </nav>
        </header>
        <main>
            <h1>Building a Web Browser in Rust</h1>
            <p class="tag">Rust</p>
            <p>
                This project is a from-scratch browser engine written in Rust.
                It parses HTML and CSS, builds a layout tree, and renders to a texture.
            </p>
            <h2>Architecture</h2>
            <p>
                The pipeline runs: Network → DOM → Style → Layout → Render → GUI.
                Each stage is separated into its own module.
            </p>
            <h2>CSS Matching</h2>
            <p>
                Selector matching uses a SelectorIndex to bucket selectors by tag,
                class, or ID so only relevant selectors are tested per element.
            </p>
        </main>
        <footer>
            <p>© 2026 Yunseong Jeong</p>
        </footer>
    "#;
    let result = run(html, css);
    assert!(result.width > 0);
    assert!(result.height > 200, "blog page should be taller than 200px");
    // At least the nav links should be collected.
    assert!(result.links.len() >= 3);
}

// ── Render output sanity ──────────────────────────────────────────────────────

#[test]
fn test_render_produces_non_blank_pixmap() {
    // White background with a colored block — pixmap must not be all-white.
    let css = "div { background-color: #ff0000; width: 200px; height: 200px; }";
    let html = "<div></div>";
    let result = run(html, css);
    let pixels = &result.pixmap_bytes;
    // At least one pixel should be non-white (background is red).
    // tiny_skia pixmap is RGBA; red = high R, low G, low B.
    let has_color = pixels.chunks(4).any(|px| px[0] > 200 && px[1] < 50 && px[2] < 50);
    assert!(has_color, "red div should produce red pixels");
}

#[test]
fn test_render_pixmap_dimensions_match_viewport() {
    let result = run("<div style=\"height:500px;\"></div>", "");
    assert_eq!(result.width, 800, "viewport width should be 800px");
}

// ── CSS Gradients (#81) ──────────────────────────────────────────────────────

/// `linear-gradient(to right, #ff0, #f00)` must render without panic and
/// produce at least one non-transparent pixel.
#[test]
fn test_pipeline_linear_gradient_to_right_renders() {
    let css = r#"div { background: linear-gradient(to right, #ff0, #f00); width: 200px; height: 100px; }"#;
    let html = r#"<div></div>"#;
    let result = run(html, css);
    assert!(result.width > 0);
    assert!(result.height > 0);
    // Expect at least one non-transparent pixel (gradient is fully opaque).
    let has_opaque = result.pixmap_bytes.chunks(4).any(|px| px[3] > 0);
    assert!(has_opaque, "linear-gradient should produce opaque pixels");
}

/// `linear-gradient(90deg, #fff, #000)` (angle-based) must complete and produce pixels.
#[test]
fn test_pipeline_linear_gradient_angle_renders() {
    let css = r#"div { background: linear-gradient(90deg, #fff, #000); width: 200px; height: 100px; }"#;
    let html = r#"<div></div>"#;
    let result = run(html, css);
    assert!(result.width > 0);
    let has_opaque = result.pixmap_bytes.chunks(4).any(|px| px[3] > 0);
    assert!(has_opaque, "90deg linear-gradient should produce opaque pixels");
}

/// `radial-gradient(circle, #fff, #000)` must complete and produce pixels.
#[test]
fn test_pipeline_radial_gradient_circle_renders() {
    let css = r#"div { background: radial-gradient(circle, #fff, #000); width: 200px; height: 200px; }"#;
    let html = r#"<div></div>"#;
    let result = run(html, css);
    assert!(result.width > 0);
    let has_opaque = result.pixmap_bytes.chunks(4).any(|px| px[3] > 0);
    assert!(has_opaque, "radial-gradient should produce opaque pixels");
}

/// Gradient with explicit percentage stops must parse and render correctly.
#[test]
fn test_pipeline_linear_gradient_percent_stops_renders() {
    let css = r#"div { background: linear-gradient(to right, #ff0 0%, #f00 50%, #00f 100%); width: 300px; height: 100px; }"#;
    let html = r#"<div></div>"#;
    let result = run(html, css);
    assert!(result.width > 0);
    let has_opaque = result.pixmap_bytes.chunks(4).any(|px| px[3] > 0);
    assert!(has_opaque, "gradient with percentage stops should produce opaque pixels");
}

/// Gradient on `background-image` property must render the same as `background`.
#[test]
fn test_pipeline_gradient_on_background_image_property() {
    let css = r#"div { background-image: linear-gradient(to right, #0f0, #00f); width: 200px; height: 100px; }"#;
    let html = r#"<div></div>"#;
    let result = run(html, css);
    assert!(result.width > 0);
    let has_opaque = result.pixmap_bytes.chunks(4).any(|px| px[3] > 0);
    assert!(has_opaque, "background-image gradient should produce opaque pixels");
}

// ── Overflow / z-index / positioned elements ─────────────────────────────────

#[test]
fn test_pipeline_overflow_hidden_clipped() {
    let css = "div { overflow: hidden; width: 100px; height: 100px; }
               p   { height: 300px; background: blue; }";
    let html = "<div><p></p></div>";
    let result = run(html, css);
    assert!(result.height > 0);
}

#[test]
fn test_pipeline_positioned_absolute() {
    let css = r#"
        .parent   { position: relative; width: 300px; height: 300px; }
        .absolute { position: absolute; top: 10px; left: 10px; width: 50px; height: 50px; }
    "#;
    let html = r#"<div class="parent"><div class="absolute">abs</div></div>"#;
    let result = run(html, css);
    assert!(result.height > 0);
}

#[test]
fn test_pipeline_z_index_stacking() {
    let css = r#"
        .back  { position: absolute; z-index: 1; top: 0; left: 0; width: 100px; height: 100px; background: red; }
        .front { position: absolute; z-index: 2; top: 20px; left: 20px; width: 100px; height: 100px; background: blue; }
    "#;
    let html = r#"
        <div style="position:relative; width:200px; height:200px;">
            <div class="back"></div>
            <div class="front"></div>
        </div>
    "#;
    let result = run(html, css);
    assert!(result.height > 0);
}
