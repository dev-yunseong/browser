use browser::layout::build_layout_tree;
use browser::dom;
use browser::css;
use browser::style;
use std::collections::HashMap;
use markup5ever_rcdom::NodeData;

#[test]
fn test_interleaved_block_inline_stacking() {
    let html = r#"
        <div style="width: 100px;">
            <span style="display: inline; height: 20px;">Inline 1</span>
            <div style="display: block; height: 30px;">Block 1</div>
            <span style="display: inline; height: 20px;">Inline 2</span>
        </div>
    "#;
    let dom = dom::parse_html(html);
    let stylesheet = css::parse_css("");
    let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None);

    // Find the outer div
    let outer_div = style_tree.children.iter()
        .find(|n| matches!(n.node.data, NodeData::Element { ref name, .. } if name.local.to_string() == "html"))
        .and_then(|html| html.children.iter().find(|n| matches!(n.node.data, NodeData::Element { ref name, .. } if name.local.to_string() == "body")))
        .and_then(|body| body.children.iter().find(|n| matches!(n.node.data, NodeData::Element { ref name, .. } if name.local.to_string() == "div")))
        .unwrap();

    let (layout_opt, _, _) = build_layout_tree(outer_div, 0.0, 0.0, 0.0, 100.0, 1000.0, 1000.0);
    let layout = layout_opt.unwrap();

    // The order in layout.children should be: Inline 1, Block 1, Inline 2
    // And their y-coordinates should be increasing.
    
    for (i, child) in layout.children.iter().enumerate() {
        println!("Child {}: {:?} at y={}", i, child.display, child.dimensions.y);
    }

    assert_eq!(layout.children.len(), 3);
    
    let inline1 = &layout.children[0];
    let block1 = &layout.children[1];
    let inline2 = &layout.children[2];

    // Check vertical stacking
    assert!(block1.dimensions.y >= inline1.dimensions.y + inline1.dimensions.height, 
        "Block 1 (y={}) should be below Inline 1 (y={}, h={})", 
        block1.dimensions.y, inline1.dimensions.y, inline1.dimensions.height);
    
    assert!(inline2.dimensions.y >= block1.dimensions.y + block1.dimensions.height,
        "Inline 2 (y={}) should be below Block 1 (y={}, h={})",
        inline2.dimensions.y, block1.dimensions.y, block1.dimensions.height);
}
