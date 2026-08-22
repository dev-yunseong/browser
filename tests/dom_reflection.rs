use browser::engine;
use std::collections::HashMap;
use url::Url;

/// Builds an engine over `html` with the page laid out at 800px, the width the
/// parity harness renders at.
fn engine_with_html(html: &str) -> browser::engine::BrowserEngine {
    let base_url = Url::parse("https://example.com/").expect("base url");
    let mut css_cache = HashMap::new();
    let (page, _) = engine::process_html_with_cache(
        html,
        &base_url,
        &HashMap::new(),
        &mut css_cache,
        None,
        &HashMap::new(),
        None,
        None,
        None,
        800.0,
    )
    .expect("process_html_with_cache");
    let mut engine = browser::engine::BrowserEngine::new();
    engine.init_js_for_page(&page);
    engine
}

/// `getBoundingClientRect` reports the border box. Layout stores the content box
/// on an axis a stated value settled, so a control with `height: 40px` and
/// `box-sizing: border-box` used to report the 22px left after its padding and
/// border were taken out.
#[test]
fn bounding_rect_of_a_border_box_control_includes_padding_and_border() {
    let mut engine = engine_with_html(
        r#"<!DOCTYPE html><html><body style="margin:0">
             <button id="b" style="display:inline-block;box-sizing:border-box;height:40px;
                                   padding:8px 16px;border:1px solid #888;font-size:14px;
                                   line-height:21px;">Code</button>
           </body></html>"#,
    );
    let h = engine.evaluate_js("String(document.getElementById('b').getBoundingClientRect().height)");
    assert_eq!(h, "40", "border-box height must be reported whole");
}

/// The same box measured rather than stated already *is* a border box, so the
/// two paths have to agree.
#[test]
fn bounding_rect_of_an_auto_height_control_is_not_double_counted() {
    let mut engine = engine_with_html(
        r#"<!DOCTYPE html><html><body style="margin:0">
             <button id="b" style="display:inline-block;box-sizing:border-box;
                                   padding:8px 16px;border:1px solid #888;font-size:14px;
                                   line-height:20px;">Code</button>
           </body></html>"#,
    );
    let h = engine.evaluate_js("String(document.getElementById('b').getBoundingClientRect().height)");
    assert_eq!(h, "38", "20px line + 16px padding + 2px border");
}

/// Every `<img>` on the page was wrapped by a freshly created, detached one:
/// `HTMLImageElement`'s constructor was the legacy `new Image(w, h)`, which makes
/// an element rather than adopting the one it was handed. The wrapper had no
/// attributes, no parent and no layout box.
#[test]
fn an_img_element_keeps_its_attributes_and_its_parent() {
    let mut engine = engine_with_html(
        r#"<!DOCTYPE html><html><body>
             <div id="wrap"><img id="i" class="pic" src="a.png" alt="x" width="40" height="30"></div>
           </body></html>"#,
    );
    assert_eq!(engine.evaluate_js("document.getElementById('i').getAttribute('src')"), "a.png");
    assert_eq!(engine.evaluate_js("document.getElementById('i').className"), "pic");
    assert_eq!(engine.evaluate_js("document.getElementById('i').getAttribute('alt')"), "x");
    assert_eq!(engine.evaluate_js("document.getElementById('i').parentElement.id"), "wrap");
    assert_eq!(
        engine.evaluate_js("String(document.getElementById('wrap').children[0] === document.getElementById('i'))"),
        "true",
    );
}

/// An `<img>` reached through the tree has a box of its own, rather than
/// reporting whatever the last parentless box happened to be.
#[test]
fn an_img_element_reports_its_own_box() {
    let mut engine = engine_with_html(
        r#"<!DOCTYPE html><html><body style="margin:0">
             <div><img id="i" src="a.png" alt="" width="40" height="30" style="display:block"></div>
           </body></html>"#,
    );
    let w = engine.evaluate_js("String(document.getElementById('i').getBoundingClientRect().width)");
    let h = engine.evaluate_js("String(document.getElementById('i').getBoundingClientRect().height)");
    assert_eq!((w.as_str(), h.as_str()), ("40", "30"));
}

/// `new Image(w, h)` still makes a new, detached `<img>` — the legacy
/// constructor the wrapper used to be.
#[test]
fn the_image_constructor_still_creates_an_element() {
    let mut engine = engine_with_html(r#"<!DOCTYPE html><html><body></body></html>"#);
    assert_eq!(engine.evaluate_js("new Image(10, 20).tagName"), "IMG");
    assert_eq!(engine.evaluate_js("new Image(10, 20).getAttribute('width')"), "10");
    assert_eq!(engine.evaluate_js("String(new Image().parentElement)"), "null");
}

/// A generated box is a synthesised element with no parent, so every one of them
/// keys to the empty path. Whichever went in last used to answer
/// `getBoundingClientRect` for any element whose own key could not be built.
#[test]
fn a_generated_box_does_not_answer_for_a_real_element() {
    let mut engine = engine_with_html(
        r#"<!DOCTYPE html><html><head><style>
             .deco::before { content: ""; display: block; width: 7px; height: 3px; }
           </style></head><body style="margin:0">
             <div class="deco" style="width:200px;height:50px"></div>
             <div id="after" style="width:120px;height:60px"></div>
           </body></html>"#,
    );
    let w = engine.evaluate_js("String(document.getElementById('after').getBoundingClientRect().width)");
    let h = engine.evaluate_js("String(document.getElementById('after').getBoundingClientRect().height)");
    assert_eq!((w.as_str(), h.as_str()), ("120", "60"));
}
