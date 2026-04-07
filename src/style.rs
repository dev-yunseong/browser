use crate::css::{Stylesheet, Value, Selector};
use markup5ever_rcdom::{Handle, NodeData};
use std::collections::HashMap;

pub type PropertyMap = HashMap<String, Value>;

#[derive(Debug)]
pub struct StyledNode {
    pub node: Handle,
    pub specified_values: PropertyMap,
    pub children: Vec<StyledNode>,
}

pub fn build_style_tree(root: &Handle, stylesheet: &Stylesheet, parent_style: Option<&PropertyMap>) -> StyledNode {
    let mut specified_values = HashMap::new();

    // 1. Apply inherited values from parent
    if let Some(parent) = parent_style {
        let inheritable = ["color", "font-size", "font-family"];
        for prop in inheritable {
            if let Some(val) = parent.get(prop) {
                specified_values.insert(prop.to_string(), val.clone());
            }
        }
    }

    // 2. Match rules with specificity
    if let NodeData::Element { ref name, ref attrs, .. } = root.data {
        let tag_name = name.local.to_string();
        let mut id = None;
        let mut classes = Vec::new();

        for attr in attrs.borrow().iter() {
            if attr.name.local.to_string() == "id" {
                id = Some(attr.value.to_string());
            } else if attr.name.local.to_string() == "class" {
                classes = attr.value.to_string().split_whitespace().map(|s| s.to_string()).collect();
            }
        }

        // Get all matching rules and sort by specificity
        let mut matches = Vec::new();
        for rule in &stylesheet.rules {
            for selector in &rule.selectors {
                if matches_selector(selector, &tag_name, id.as_deref(), &classes) {
                    matches.push((selector.specificity(), rule));
                    break;
                }
            }
        }
        
        matches.sort_by(|a, b| a.0.cmp(&b.0));

        for (_, rule) in matches {
            for (k, v) in &rule.declarations {
                specified_values.insert(k.clone(), v.clone());
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

fn matches_selector(selector: &Selector, tag: &str, id: Option<&str>, classes: &[String]) -> bool {
    if let Some(ref s_tag) = selector.tag {
        if s_tag != tag { return false; }
    }
    if let Some(ref s_id) = selector.id {
        if Some(s_id.as_str()) != id { return false; }
    }
    for s_class in &selector.class {
        if !classes.contains(s_class) { return false; }
    }
    true
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

pub fn extract_external_css_links(handle: &Handle) -> Vec<String> {
    let mut links = Vec::new();
    if let NodeData::Element { ref name, ref attrs, .. } = handle.data {
        if name.local.to_string() == "link" {
            let mut is_stylesheet = false;
            let mut href = None;
            for attr in attrs.borrow().iter() {
                if attr.name.local.to_string() == "rel" && attr.value.to_string() == "stylesheet" {
                    is_stylesheet = true;
                } else if attr.name.local.to_string() == "href" {
                    href = Some(attr.value.to_string());
                }
            }
            if is_stylesheet {
                if let Some(h) = href { links.push(h); }
            }
        }
    }
    for child in handle.children.borrow().iter() {
        links.extend(extract_external_css_links(child));
    }
    links
}
