use crate::css::{Stylesheet, Value, Selector, parse_value, parse_color, Combinator};
use markup5ever_rcdom::{Handle, NodeData};
use std::collections::HashMap;
use rayon::prelude::*;

pub type PropertyMap = HashMap<String, Value>;

#[derive(Debug)]
pub struct StyledNode {
    pub node: Handle,
    pub specified_values: PropertyMap,
    pub children: Vec<StyledNode>,
}

pub struct NodeDataSend {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<(String, String)>,
    pub is_element: bool,
    pub parent_idx: Option<usize>,
    pub children_idx: Vec<usize>,
}

// Flatten RcDom into a Vec for parallel processing
fn flatten_dom(handle: &Handle, arena: &mut Vec<NodeDataSend>, parent_idx: Option<usize>) -> usize {
    let idx = arena.len();
    
    let mut tag = String::new();
    let mut id = None;
    let mut classes = Vec::new();
    let mut attrs_vec = Vec::new();
    let mut is_element = false;

    if let NodeData::Element { ref name, ref attrs, .. } = handle.data {
        is_element = true;
        tag = name.local.to_string();
        for attr in attrs.borrow().iter() {
            let k = attr.name.local.to_string();
            let v = attr.value.to_string();
            if k == "id" { id = Some(v.clone()); }
            if k == "class" { classes = v.split_whitespace().map(|s| s.to_string()).collect(); }
            attrs_vec.push((k, v));
        }
    }

    arena.push(NodeDataSend {
        tag, id, classes, attrs: attrs_vec, is_element, parent_idx, children_idx: Vec::new()
    });

    let mut children_idx = Vec::new();
    for child in handle.children.borrow().iter() {
        let child_idx = flatten_dom(child, arena, Some(idx));
        children_idx.push(child_idx);
    }
    
    arena[idx].children_idx = children_idx;
    idx
}

fn matches_selector_arena(selector: &Selector, idx: usize, arena: &[NodeDataSend], hovered_id: Option<&str>, focused_id: Option<&str>) -> bool {
    let node = &arena[idx];
    
    let has_constraint = selector.tag.is_some() || selector.id.is_some() || !selector.class.is_empty() || !selector.attributes.is_empty() || selector.pseudo_class.is_some();
    if !has_constraint { return false; }

    if let Some(ref s_tag) = selector.tag {
        if s_tag != &node.tag && s_tag != "*" { return false; }
    }
    if let Some(ref s_id) = selector.id {
        if Some(s_id.as_str()) != node.id.as_deref() { return false; }
    }
    for s_class in &selector.class {
        if !node.classes.contains(s_class) { return false; }
    }
    if let Some(ref pseudo) = selector.pseudo_class {
        if pseudo == "hover" {
            if Some(node.id.as_deref()) != Some(hovered_id) || node.id.is_none() { return false; }
        } else if pseudo == "focus" {
            if Some(node.id.as_deref()) != Some(focused_id) || node.id.is_none() { return false; }
        } else { return false; }
    }
    for attr_sel in &selector.attributes {
        let mut matched = false;
        for (k, v) in &node.attrs {
            if k == &attr_sel.name {
                match &attr_sel.value {
                    crate::css::AttributeMatch::Exists => { matched = true; break; }
                    crate::css::AttributeMatch::Equals(val) => { if v == val { matched = true; break; } }
                }
            }
        }
        if !matched { return false; }
    }

    if let Some(ref ancestor_sel) = selector.ancestor {
        let combinator = selector.combinator.as_ref().unwrap_or(&Combinator::Descendant);
        match combinator {
            Combinator::Descendant => {
                let mut current = node.parent_idx;
                let mut matched = false;
                while let Some(p_idx) = current {
                    if matches_selector_arena(ancestor_sel, p_idx, arena, hovered_id, focused_id) {
                        matched = true; break;
                    }
                    current = arena[p_idx].parent_idx;
                }
                if !matched { return false; }
            }
            Combinator::Child => {
                if let Some(p_idx) = node.parent_idx {
                    if !matches_selector_arena(ancestor_sel, p_idx, arena, hovered_id, focused_id) { return false; }
                } else { return false; }
            }
            Combinator::NextSibling => {
                if let Some(p_idx) = node.parent_idx {
                    let p_node = &arena[p_idx];
                    let mut found = false;
                    for &sib_idx in p_node.children_idx.iter().rev() {
                        if sib_idx >= idx { continue; }
                        if arena[sib_idx].is_element {
                            if matches_selector_arena(ancestor_sel, sib_idx, arena, hovered_id, focused_id) { found = true; }
                            break;
                        }
                    }
                    if !found { return false; }
                } else { return false; }
            }
            Combinator::SubsequentSibling => {
                if let Some(p_idx) = node.parent_idx {
                    let p_node = &arena[p_idx];
                    let mut matched = false;
                    for &sib_idx in &p_node.children_idx {
                        if sib_idx >= idx { break; }
                        if arena[sib_idx].is_element {
                            if matches_selector_arena(ancestor_sel, sib_idx, arena, hovered_id, focused_id) {
                                matched = true; break;
                            }
                        }
                    }
                    if !matched { return false; }
                } else { return false; }
            }
        }
    }
    true
}

fn apply_attribute_styles_arena(node: &NodeDataSend, map: &mut PropertyMap) {
    match node.tag.as_str() {
        "img" => {
            for (k, v) in &node.attrs {
                if k == "width" { if let Ok(val) = v.trim_end_matches("px").parse::<f32>() { map.insert("width".to_string(), Value::Length(val, crate::css::Unit::Px)); } }
                if k == "height" { if let Ok(val) = v.trim_end_matches("px").parse::<f32>() { map.insert("height".to_string(), Value::Length(val, crate::css::Unit::Px)); } }
            }
        }
        "font" => {
            for (k, v) in &node.attrs {
                if k == "color" { if let Some(c) = parse_color(v) { map.insert("color".to_string(), Value::Color(c)); } }
                if k == "size" {
                    let size_map = [("1", 10.0f32), ("2", 13.0), ("3", 16.0), ("4", 18.0), ("5", 24.0), ("6", 32.0), ("7", 48.0)];
                    for (s, px) in &size_map {
                        if s == v { map.insert("font-size".to_string(), Value::Length(*px, crate::css::Unit::Px)); }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Build a style tree, applying CSS rules, inline styles, and JS overrides.
pub fn build_style_tree(
    root: &Handle,
    stylesheet: &Stylesheet,
    parent_style: Option<&PropertyMap>,
    js_overrides: &HashMap<String, HashMap<String, String>>,
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
) -> StyledNode {
    let mut arena = Vec::new();
    flatten_dom(root, &mut arena, None);

    // Phase 1: Parallel CSS Matching
    let mut pre_styles: Vec<PropertyMap> = arena.par_iter().enumerate().map(|(idx, node)| {
        if !node.is_element { return HashMap::new(); }
        let mut map = HashMap::new();
        apply_default_styles(&node.tag, &mut map);
        
        let mut matches = Vec::new();
        for rule in stylesheet.all_rules() {
            let mut highest = None;
            for sel in &rule.selectors {
                if matches_selector_arena(sel, idx, &arena, hovered_id, focused_id) {
                    let spec = sel.specificity();
                    if highest.is_none() || spec > highest.unwrap() { highest = Some(spec); }
                }
            }
            if let Some(s) = highest { matches.push((s, rule)); }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        
        let mut important = HashMap::new();
        for (_, rule) in &matches {
            for decl in &rule.declarations {
                if !decl.important { map.insert(decl.name.clone(), decl.value.clone()); }
                else { important.insert(decl.name.clone(), decl.value.clone()); }
            }
        }
        
        apply_attribute_styles_arena(node, &mut map);
        
        let mut inline_important = HashMap::new();
        if let Some(v) = node.attrs.iter().find(|(k, _)| k == "style").map(|(_, v)| v) {
            let mut inline_map = Vec::new();
            parse_inline_style_into_vec(v, &mut inline_map);
            for decl in inline_map {
                if !decl.important { map.insert(decl.name, decl.value); }
                else { inline_important.insert(decl.name, decl.value); }
            }
        }
        
        for (k, v) in important { map.insert(k, v); }
        for (k, v) in inline_important { map.insert(k, v); }
        
        if let Some(ref id) = node.id {
            if let Some(overrides) = js_overrides.get(id) {
                for (k, v) in overrides { map.insert(k.clone(), parse_value(v)); }
            }
        }
        map
    }).collect();

    // Phase 2: Sequential Inheritance
    let mut arena_idx = 0;
    build_final_tree(root, &mut arena_idx, &mut pre_styles, parent_style)
}

fn build_final_tree(
    handle: &Handle,
    arena_idx: &mut usize,
    pre_styles: &mut [PropertyMap],
    parent_style: Option<&PropertyMap>
) -> StyledNode {
    let current_idx = *arena_idx;
    *arena_idx += 1;
    
    let mut specified_values = std::mem::take(&mut pre_styles[current_idx]);
    
    if parent_style.is_none() && current_idx == 0 {
        specified_values.insert("color".to_string(), Value::Color(crate::css::Color { r: 0, g: 0, b: 0, a: 255 }));
        specified_values.insert("font-size".to_string(), Value::Length(16.0, crate::css::Unit::Px));
    }
    
    if let Some(p) = parent_style {
        let inheritable = ["color", "font-size", "font-family", "font-weight", "line-height", "text-align", "list-style-type"];
        for prop in inheritable {
            if let Some(v) = p.get(prop) {
                specified_values.entry(prop.to_string()).or_insert_with(|| v.clone());
            }
        }
    }
    
    let mut resolved_fs = 16.0f32;
    if let Some(val) = specified_values.get("font-size") {
        match val {
            Value::Length(v, crate::css::Unit::Px) => resolved_fs = *v,
            Value::Length(v, crate::css::Unit::Percent) => {
                let parent_fs = match parent_style {
                    Some(p) => match p.get("font-size") { Some(Value::Length(pv, crate::css::Unit::Px)) => *pv, _ => 16.0 }, _ => 16.0
                };
                resolved_fs = parent_fs * (v / 100.0);
            }
            Value::Length(v, crate::css::Unit::Em) => {
                let parent_fs = match parent_style {
                    Some(p) => match p.get("font-size") { Some(Value::Length(pv, crate::css::Unit::Px)) => *pv, _ => 16.0 }, _ => 16.0
                };
                resolved_fs = parent_fs * v;
            }
            _ => {}
        }
    }
    specified_values.insert("font-size".to_string(), Value::Length(resolved_fs, crate::css::Unit::Px));

    let keys: Vec<String> = specified_values.keys().cloned().collect();
    for k in keys {
        if k == "font-size" { continue; }
        if let Some(Value::Length(v, crate::css::Unit::Em)) = specified_values.get(&k) {
            specified_values.insert(k, Value::Length(v * resolved_fs, crate::css::Unit::Px));
        }
    }
    
    let children = handle.children.borrow().iter().map(|child| {
        build_final_tree(child, arena_idx, pre_styles, Some(&specified_values))
    }).collect();
    
    StyledNode {
        node: handle.clone(),
        specified_values,
        children
    }
}

pub fn parse_inline_style_into_vec(style_str: &str, list: &mut Vec<crate::css::Declaration>) {
    for decl in style_str.split(';') {
        let decl = decl.trim();
        if decl.is_empty() { continue; }
        let mut kv = decl.splitn(2, ':');
        let key = kv.next().unwrap_or("").trim().to_lowercase();
        let mut val_raw = kv.next().unwrap_or("").trim().to_string();
        if key.is_empty() || val_raw.is_empty() { continue; }

        let important = val_raw.ends_with("!important");
        if important {
            val_raw = val_raw.trim_end_matches("!important").trim().to_string();
        }

        match key.as_str() {
            "padding" => {
                let mut temp_map = HashMap::new();
                crate::css::parse_quad_shorthand("padding", &val_raw, &mut temp_map);
                for (k, v) in temp_map {
                    list.push(crate::css::Declaration { name: k, value: v, important });
                }
            }
            "margin" => {
                let mut temp_map = HashMap::new();
                crate::css::parse_quad_shorthand("margin", &val_raw, &mut temp_map);
                for (k, v) in temp_map {
                    list.push(crate::css::Declaration { name: k, value: v, important });
                }
            }
            "border" => {
                let mut temp_map = HashMap::new();
                crate::css::parse_border_shorthand_pub(&val_raw, &mut temp_map);
                for (k, v) in temp_map {
                    list.push(crate::css::Declaration { name: k, value: v, important });
                }
            }
            _ => {
                list.push(crate::css::Declaration {
                    name: key,
                    value: parse_value(&val_raw),
                    important,
                });
            }
        }
    }
}

pub fn parse_inline_style(style_str: &str, map: &mut PropertyMap) {
    let mut list = Vec::new();
    parse_inline_style_into_vec(style_str, &mut list);
    for decl in list {
        map.insert(decl.name, decl.value);
    }
}

fn apply_default_styles(tag: &str, map: &mut PropertyMap) {
    match tag {
        "h1" => {
            map.entry("font-size".to_string()).or_insert(Value::Length(32.0, crate::css::Unit::Px));
            map.entry("font-weight".to_string()).or_insert(Value::Keyword("bold".to_string()));
            map.entry("margin-top".to_string()).or_insert(Value::Length(21.0, crate::css::Unit::Px));
            map.entry("margin-bottom".to_string()).or_insert(Value::Length(21.0, crate::css::Unit::Px));
        }
        "h2" => {
            map.entry("font-size".to_string()).or_insert(Value::Length(24.0, crate::css::Unit::Px));
            map.entry("font-weight".to_string()).or_insert(Value::Keyword("bold".to_string()));
            map.entry("margin-top".to_string()).or_insert(Value::Length(14.0, crate::css::Unit::Px));
            map.entry("margin-bottom".to_string()).or_insert(Value::Length(14.0, crate::css::Unit::Px));
        }
        "h3" => {
            map.entry("font-size".to_string()).or_insert(Value::Length(18.0, crate::css::Unit::Px));
            map.entry("font-weight".to_string()).or_insert(Value::Keyword("bold".to_string()));
        }
        "h4" | "h5" | "h6" => {
            map.entry("font-weight".to_string()).or_insert(Value::Keyword("bold".to_string()));
        }
        "a" => {
            map.entry("color".to_string()).or_insert(Value::Color(parse_color("#0000ee").unwrap()));
            map.entry("text-decoration".to_string()).or_insert(Value::Keyword("underline".to_string()));
        }
        "strong" | "b" => {
            map.entry("font-weight".to_string()).or_insert(Value::Keyword("bold".to_string()));
        }
        "em" | "i" => {
            map.entry("font-style".to_string()).or_insert(Value::Keyword("italic".to_string()));
        }
        "code" | "pre" | "kbd" | "samp" => {
            map.entry("font-family".to_string()).or_insert(Value::Keyword("monospace".to_string()));
            map.entry("background-color".to_string()).or_insert(Value::Color(crate::css::Color { r: 240, g: 240, b: 240, a: 255 }));
        }
        "button" | "input" | "select" | "textarea" => {
            map.entry("border-width".to_string()).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry("border-color".to_string()).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry("background-color".to_string()).or_insert(Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            map.entry("padding".to_string()).or_insert(Value::Length(4.0, crate::css::Unit::Px));
        }
        "ul" | "ol" => {
            map.entry("padding-left".to_string()).or_insert(Value::Length(24.0, crate::css::Unit::Px));
        }
        "p" => {
            map.entry("margin-top".to_string()).or_insert(Value::Length(8.0, crate::css::Unit::Px));
            map.entry("margin-bottom".to_string()).or_insert(Value::Length(8.0, crate::css::Unit::Px));
        }
        _ => {}
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_inline_style() {
        let mut map = PropertyMap::new();
        parse_inline_style("color: red; font-size: 14px; background-color: #fff", &mut map);
        assert!(map.contains_key("color"));
        assert!(map.contains_key("font-size"));
        assert!(map.contains_key("background-color"));
    }

    #[test]
    fn test_focus_pseudo_class_matching() {
        let mut arena = Vec::new();
        arena.push(NodeDataSend {
            tag: "div".to_string(),
            id: Some("target".to_string()),
            classes: Vec::new(),
            attrs: Vec::new(),
            is_element: true,
            parent_idx: None,
            children_idx: Vec::new(),
        });

        let mut selector = crate::css::Selector::default();
        selector.pseudo_class = Some("focus".to_string());

        assert!(!matches_selector_arena(&selector, 0, &arena, None, None));
        assert!(matches_selector_arena(&selector, 0, &arena, None, Some("target")));
    }
}
