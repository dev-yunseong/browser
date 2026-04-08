use aura_browser::dom;
use aura_browser::css;
use aura_browser::style;
use aura_browser::layout;
use std::collections::HashMap;

#[test]
fn test_complex_selector_matching_c1_c2() {
    // .card .title should match .title only if it's inside .card
    let html = r#"<div class="card"><div class="title">Match</div></div><div class="title">No Match</div>"#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css(".card .title { color: red; }");
    
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());
    
    // Find the two .title nodes
    let html_node = &style_tree.children[0]; // <html>
    let body_node = &html_node.children[1]; // <body>
    
    let card_div = &body_node.children[0]; // <div class="card">
    let title_inside = &card_div.children[0]; // <div class="title"> inside card
    let title_outside = &body_node.children[1]; // <div class="title"> outside card
    
    let red = css::Value::Color(css::Color { r: 255, g: 0, b: 0, a: 255 });
    assert_eq!(title_inside.specified_values.get("color").unwrap(), &red);
    // title_outside should inherit default black from root, not red from the rule
    assert_ne!(title_outside.specified_values.get("color").unwrap(), &red);
}

#[test]
fn test_empty_selector_no_global_match_c3() {
    // [data-x] should NOT match everything if it becomes empty
    let html = r#"<div>No Match</div>"#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css("[data-x] { color: red; }");
    
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());
    let div_style = find_node_by_tag(&style_tree, "div").expect("div not found");
    
    let red = css::Value::Color(css::Color { r: 255, g: 0, b: 0, a: 255 });
    assert_ne!(div_style.specified_values.get("color").unwrap(), &red);
}

#[test]
fn test_pseudo_class_truncation_still_works_for_base_c4() {
    // a:hover should at least match a (this is current known limitation, but better than nothing)
    let stylesheet = css::parse_css("a:hover { color: red; }");
    let selector = &stylesheet.rules[0].selectors[0];
    
    assert_eq!(selector.tag.as_deref(), Some("a"));
}

#[test]
fn test_font_size_percent_calc_fixed_b4() {
    // 50% font-size should be 10px (50% of 20px)
    let html = r#"<div style="font-size: 20px;"><span style="font-size: 50%;">Text</span></div>"#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css(""); 
    
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());
    let span_style = find_node_by_tag(&style_tree, "span").expect("span not found");
    
    let font_size = match span_style.specified_values.get("font-size") {
        Some(css::Value::Length(v, _)) => *v,
        _ => 0.0,
    };
    
    assert_eq!(font_size, 10.0);
}

#[test]
fn test_all_units_consumed_b3() {
    // vw, vh, em are now handled
    let html = r#"<div style="margin-left: 10vw; padding-top: 50vh; width: 2em; font-size: 20px;"></div>"#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css("");
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());
    
    // vw=800, vh=1000
    let (layout_opt, _, _) = layout::build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 1000.0);
    let root = layout_opt.unwrap();
    let div = find_layout_node_by_tag(&root, "div").expect("div not found");
    
    assert_eq!(div.margin.left, 80.0); // 10% of 800
    assert_eq!(div.padding.top, 500.0); // 50% of 1000
    assert_eq!(div.dimensions.width, 40.0); // 2 * 20px
}

#[test]
fn test_explicit_height_respected_b9() {
    let html = r#"<div style="height: 200px;"></div>"#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css("");
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());
    
    let (layout_opt, _, _) = layout::build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 768.0);
    let root = layout_opt.unwrap();
    let div = find_layout_node_by_tag(&root, "div").expect("div not found");
    
    assert_eq!(div.dimensions.height, 200.0);
}

#[test]
fn test_selector_list_specificity_c5() {
    // h1, .big { color: red } matched on <h1 class="big"> should use .big's specificity (0,1,1)
    // if matched against another rule .big { color: blue }
    // Actually, we just need to verify it picks the max specificity.
    
    let html = r#"<h1 class="big"></h1>"#;
    let dom = dom::parse_html(html);
    
    // In style.rs, we need to check how it was matched.
    // We can't easily check the internal 'matches' vector, but we can verify the result
    // if we have two conflicting rules.
    let stylesheet2 = css::parse_css("h1, .big { color: red; } h1 { color: blue; }");
    let style_tree = style::build_style_tree(&dom.document, &stylesheet2, None, &HashMap::new());
    let h1_style = find_node_by_tag(&style_tree, "h1").expect("h1 not found");
    
    // .big (0,1,0) > h1 (0,0,1). So red should win.
    assert_eq!(h1_style.specified_values.get("color").unwrap(), &css::Value::Color(css::Color { r: 255, g: 0, b: 0, a: 255 }));
}

// Helpers
fn find_node_by_tag<'a>(sn: &'a style::StyledNode, tag: &str) -> Option<&'a style::StyledNode> {
    if let markup5ever_rcdom::NodeData::Element { ref name, .. } = sn.node.data {
        if name.local.to_string() == tag { return Some(sn); }
    }
    for child in &sn.children {
        if let Some(res) = find_node_by_tag(child, tag) { return Some(res); }
    }
    None
}

fn find_layout_node_by_tag<'a>(lb: &'a layout::LayoutBox<'a>, tag: &str) -> Option<&'a layout::LayoutBox<'a>> {
    if let markup5ever_rcdom::NodeData::Element { ref name, .. } = lb.style_node.node.data {
        if name.local.to_string() == tag { return Some(lb); }
    }
    for child in &lb.children {
        if let Some(res) = find_layout_node_by_tag(child, tag) { return Some(res); }
    }
    None
}
