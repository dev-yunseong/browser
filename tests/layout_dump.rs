//! Developer tool: dump the layout of an HTML file as JSON for comparison with
//! a reference browser's `getBoundingClientRect()` output.
//!
//! Ignored by default. Run with:
//!   LAYOUT_DUMP_HTML=page.html LAYOUT_DUMP_CSS=a.css:b.css LAYOUT_DUMP_OUT=out.json \
//!     cargo test --release --test layout_dump -- --ignored --nocapture
//!
//! Each element below `<body>` gets its document pre-order index (matching
//! `document.querySelectorAll('body *')` order) so rows can be joined with
//! the reference dump.

use browser::layout::LayoutBox;
use markup5ever_rcdom::{Handle, NodeData};
use std::collections::HashMap;

fn index_elements(handle: &Handle, in_body: bool, map: &mut HashMap<usize, usize>, next: &mut usize) {
    let mut now_in_body = in_body;
    if let NodeData::Element { ref name, .. } = handle.data {
        if in_body {
            map.insert(std::rc::Rc::as_ptr(handle) as usize, *next);
            *next += 1;
        }
        if name.local.as_ref() == "body" {
            now_in_body = true;
        }
    }
    for child in handle.children.borrow().iter() {
        index_elements(child, now_in_body, map, next);
    }
}

fn dump(lb: &LayoutBox, map: &HashMap<usize, usize>, out: &mut Vec<serde_json::Value>) {
    let ptr = std::rc::Rc::as_ptr(&lb.style_node.node) as usize;
    if let Some(ref t) = lb.text_fragment {
        if !t.is_empty() {
            let d = lb.dimensions;
            out.push(serde_json::json!({
                "i": -1, "x": d.x, "y": d.y, "w": d.width, "h": d.height, "t": t,
            }));
        }
    }
    if let Some(idx) = map.get(&ptr) {
        let d = lb.dimensions;
        out.push(serde_json::json!({
            "i": idx,
            "x": d.x,
            "y": d.y,
            "w": d.width,
            "h": d.height,
            "cw": d.width,
            "disp": format!("{:?}", lb.display),
        }));
    }
    for c in &lb.children {
        dump(c, map, out);
    }
}

#[test]
#[ignore]
fn layout_dump() {
    let html_path = std::env::var("LAYOUT_DUMP_HTML").expect("LAYOUT_DUMP_HTML");
    let css_paths = std::env::var("LAYOUT_DUMP_CSS").unwrap_or_default();
    let out_path = std::env::var("LAYOUT_DUMP_OUT").unwrap_or_else(|_| "layout.json".into());
    let html = std::fs::read_to_string(html_path).unwrap();
    let mut css_text = String::new();
    for p in css_paths.split(':').filter(|p| !p.is_empty()) {
        css_text.push_str(&std::fs::read_to_string(p).unwrap());
        css_text.push('\n');
    }
    let dom = browser::dom::parse_html(&html);
    let stylesheet = browser::css::parse_css(&css_text);
    let style_tree = browser::style::build_style_tree(
        &dom.document,
        &stylesheet,
        None,
        &HashMap::new(),
        None,
        None,
        None,
    );
    let (layout, _, final_y) =
        browser::layout::build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 768.0);
    let layout = layout.unwrap();
    let mut map = HashMap::new();
    let mut next = 0;
    index_elements(&dom.document, false, &mut map, &mut next);
    let mut out = Vec::new();
    dump(&layout, &map, &mut out);
    std::fs::write(&out_path, serde_json::to_string(&out).unwrap()).unwrap();
    println!("elements={} boxes={} height={}", next, out.len(), final_y);
}
