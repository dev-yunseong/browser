use browser::dom;
use markup5ever_rcdom::NodeData;

#[test]
fn test_unclosed_tags_recovery() {
    let html = "<html><body><div><p>Hello world"; 
    let dom = dom::parse_html(html);
    
    // Recovery should still work with full document structure
    let root = &dom.document;
    let html_node = &root.children.borrow()[0];
    let body_node = &html_node.children.borrow()[1];
    let div_node = &body_node.children.borrow()[0];
    let p_node = &div_node.children.borrow()[0];
    
    if let NodeData::Element { ref name, .. } = p_node.data {
        assert_eq!(name.local.to_string(), "p");
    } else {
        panic!("Missing p tag");
    }
}

#[test]
fn test_quirks_mode_detection() {
    // A classic quirks mode trigger
    let html = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN"><html><body></body></html>"#;
    let dom = dom::parse_html(html);
    
    assert!(dom.quirks_mode.get() != html5ever::tree_builder::NoQuirks);
}

#[test]
fn test_script_tokenization() {
    let html = "<html><body><script>console.log('<b>not a tag</b>')</script></body></html>";
    let dom = dom::parse_html(html);
    
    let root = &dom.document;
    let html_node = &root.children.borrow()[0];
    let body_node = &html_node.children.borrow()[1];
    let script_node = &body_node.children.borrow()[0];
    
    if let NodeData::Element { ref name, .. } = script_node.data {
        assert_eq!(name.local.to_string(), "script");
    }
    
    assert_eq!(script_node.children.borrow().len(), 1); // Only one text child
}
