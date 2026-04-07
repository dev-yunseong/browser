use crate::css::{Stylesheet, Value};
use markup5ever_rcdom::{Handle, NodeData};
use std::collections::HashMap;

pub type PropertyMap = HashMap<String, Value>;

pub struct StyledNode {
    pub node: Handle,
    pub specified_values: PropertyMap,
    pub children: Vec<StyledNode>,
}

pub fn build_style_tree(root: &Handle, stylesheet: &Stylesheet, parent_style: Option<&PropertyMap>) -> StyledNode {
    let mut specified_values = HashMap::new();

    // 1. Apply inherited values from parent
    if let Some(parent) = parent_style {
        // Properties that are inherited by default in CSS
        let inheritable = ["color", "font-size", "font-family"];
        for prop in inheritable {
            if let Some(val) = parent.get(prop) {
                specified_values.insert(prop.to_string(), val.clone());
            }
        }
    }

    // 2. Match tag-specific rules
    if let NodeData::Element { ref name, .. } = root.data {
        let tag_name = name.local.to_string();

        for rule in &stylesheet.rules {
            for selector in &rule.selectors {
                if let Some(ref s_tag) = selector.tag {
                    if *s_tag == tag_name {
                        for (k, v) in &rule.declarations {
                            specified_values.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
        }
    }

    let children = root.children
        .borrow()
        .iter()
        .map(|child| build_style_tree(child, stylesheet, Some(&specified_values)))
        .collect();

    StyledNode {
        node: root.clone(),
        specified_values,
        children,
    }
}

// Helper to extract text from <style> nodes
pub fn extract_css_from_dom(handle: &Handle) -> String {
    let mut css = String::new();

    if let NodeData::Element { ref name, .. } = handle.data {
        if name.local.to_string() == "style" {
            for child in handle.children.borrow().iter() {
                if let NodeData::Text { ref contents } = child.data {
                    css.push_str(&contents.borrow());
                }
            }
        }
    }

    for child in handle.children.borrow().iter() {
        css.push_str(&extract_css_from_dom(child));
    }

    css
}

pub fn print_style_tree(node: &StyledNode, indent: usize) {
    let indent_str = " ".repeat(indent * 2);

    match node.node.data {
        NodeData::Element { ref name, .. } => {
            println!("{}<{} ...> {:?}", indent_str, name.local, node.specified_values);
        }
        NodeData::Text { ref contents } => {
            let text = contents.borrow().to_string();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                println!("{}Text: {:?}", indent_str, trimmed);
            }
        }
        NodeData::Document => println!("{}Document", indent_str),
        _ => {}
    }

    for child in &node.children {
        print_style_tree(child, indent + 1);
    }
}
