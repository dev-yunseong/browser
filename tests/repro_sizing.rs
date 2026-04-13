use browser::layout::build_layout_tree;
use browser::dom;
use browser::css;
use browser::style;
use std::collections::HashMap;
use markup5ever_rcdom::NodeData;

#[test]
fn test_inline_block_shrink_wrap() {
    // A container of 500px, containing an inline-block with short text.
    // It should NOT be 500px wide.
    let html = r#"
        <div style="width: 500px;">
            <div id="target" style="display: inline-block;">Short</div>
        </div>
    "#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css("");
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None, None);

    // Find the #target div
    let target_node = find_node_by_id(&style_tree, "target").unwrap();

    let (layout_opt, _, _) = build_layout_tree(target_node, 0.0, 0.0, 0.0, 500.0, 1000.0, 1000.0);
    let layout = layout_opt.unwrap();

    println!("Inline-block width: {}", layout.dimensions.width);
    // "Short" should be much less than 500px. 
    // Default font size 16px, 5 letters. Should be around 40-60px.
    assert!(layout.dimensions.width < 100.0);
    assert!(layout.dimensions.width > 0.0);
}

#[test]
fn test_table_cell_shrink_wrap() {
    let html = r#"
        <div style="width: 500px;">
            <table>
                <tr>
                    <td id="target">Short</td>
                </tr>
            </table>
        </div>
    "#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css("");
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None, None);

    let target_node = find_node_by_id(&style_tree, "target").unwrap();

    let (layout_opt, _, _) = build_layout_tree(target_node, 0.0, 0.0, 0.0, 500.0, 1000.0, 1000.0);
    let layout = layout_opt.unwrap();

    println!("Table-cell width: {}", layout.dimensions.width);
    assert!(layout.dimensions.width < 100.0);
    assert!(layout.dimensions.width > 0.0);
}

#[test]
fn test_image_default_size() {
    let html = r#"<img id="target" src="foo.png">"#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css("");
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None, None);

    let target_node = find_node_by_id(&style_tree, "target").unwrap();

    let (layout_opt, _, _) = build_layout_tree(target_node, 0.0, 0.0, 0.0, 500.0, 1000.0, 1000.0);
    let layout = layout_opt.unwrap();

    println!("Image width: {}", layout.dimensions.width);
    // Default image width in compute_max_content_width is 100.0
    assert_eq!(layout.dimensions.width, 100.0);
}

fn find_node_by_id<'a>(sn: &'a style::StyledNode, id: &str) -> Option<&'a style::StyledNode> {
    if let NodeData::Element { ref attrs, .. } = sn.node.data {
        for attr in attrs.borrow().iter() {
            if attr.name.local.to_string() == "id" && attr.value.to_string() == id {
                return Some(sn);
            }
        }
    }
    for child in &sn.children {
        if let Some(found) = find_node_by_id(child, id) {
            return Some(found);
        }
    }
    None
}
