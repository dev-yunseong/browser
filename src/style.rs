use crate::css::{Stylesheet, Value, Selector, parse_value, parse_color, Combinator};
use markup5ever_rcdom::{Handle, NodeData};
use std::collections::HashMap;

pub type PropertyMap = HashMap<String, Value>;

#[derive(Debug)]
pub struct StyledNode {
    pub node: Handle,
    pub specified_values: PropertyMap,
    pub children: Vec<StyledNode>,
}

/// Build a style tree, applying CSS rules, inline styles, and JS overrides.
///
/// `js_overrides` maps element id → property → value string.
/// These are applied last (highest priority), overriding CSS and inline styles.
pub fn build_style_tree(
    root: &Handle,
    stylesheet: &Stylesheet,
    parent_style: Option<&PropertyMap>,
    js_overrides: &HashMap<String, HashMap<String, String>>,
    hovered_id: Option<&str>,
) -> StyledNode {
    let mut specified_values = HashMap::new();

    // 1. Set initial defaults for root (or inherit from parent)
    if parent_style.is_none() {
        // Default color for the very first node (usually <html>)
        specified_values.insert("color".to_string(), Value::Color(crate::css::Color { r: 0, g: 0, b: 0, a: 255 }));
        specified_values.insert("font-size".to_string(), Value::Length(16.0, crate::css::Unit::Px));
    }

    // 2. Inherit properties from parent
    if let Some(parent) = parent_style {
        let inheritable = [
            "color", "font-size", "font-family", "font-weight",
            "line-height", "text-align", "list-style-type",
        ];
        for prop in inheritable {
            if let Some(val) = parent.get(prop) {
                specified_values.insert(prop.to_string(), val.clone());
            }
        }
    }

    if let NodeData::Element { ref name, ref attrs, .. } = root.data {
        let tag_name = name.local.to_string();
        let mut id: Option<String> = None;

        for attr in attrs.borrow().iter() {
            let attr_name = attr.name.local.to_string();
            match attr_name.as_str() {
                "id" => id = Some(attr.value.to_string()),
                _ => {}
            }
        }

        // 2. Apply default browser styles (UA origin)
        apply_default_styles(&tag_name, &mut specified_values);

        // 3. Match CSS rules and collect declarations
        let mut matches = Vec::new();
        for rule in stylesheet.all_rules() {
            let mut highest_match: Option<(usize, usize, usize)> = None;
            for selector in &rule.selectors {
                if matches_selector(selector, root, hovered_id) {
                    let spec = selector.specificity();
                    if highest_match.is_none() || spec > highest_match.unwrap() {
                        highest_match = Some(spec);
                    }
                }
            }
            if let Some(spec) = highest_match {
                matches.push((spec, rule));
            }
        }
        
        // Sort by specificity and source order
        matches.sort_by(|a, b| a.0.cmp(&b.0));

        // Track importance to allow !important to override
        let mut important_properties = HashMap::new();

        // Apply normal declarations from rules
        for (_, rule) in &matches {
            for decl in &rule.declarations {
                if !decl.important {
                    specified_values.insert(decl.name.clone(), decl.value.clone());
                } else {
                    important_properties.insert(decl.name.clone(), decl.value.clone());
                }
            }
        }

        // 4. Apply element-specific attribute styles (normal priority)
        apply_attribute_styles(&tag_name, &attrs.borrow(), &mut specified_values);

        // 5. Apply inline style attribute (normal priority)
        let mut inline_important = HashMap::new();
        for attr in attrs.borrow().iter() {
            if attr.name.local.to_string() == "style" {
                let mut inline_map = Vec::new();
                parse_inline_style_into_vec(&attr.value.to_string(), &mut inline_map);
                for decl in inline_map {
                    if !decl.important {
                        specified_values.insert(decl.name, decl.value);
                    } else {
                        inline_important.insert(decl.name, decl.value);
                    }
                }
            }
        }

        // Apply Author !important declarations (rules then inline)
        for (k, v) in important_properties {
            specified_values.insert(k, v);
        }
        for (k, v) in inline_important {
            specified_values.insert(k, v);
        }

        // B4: If font-size is a percentage, it must be resolved against the *inherited* font-size
        // before other properties use it (if any do). Currently layout uses specified_values directly.
        if let Some(Value::Length(v, crate::css::Unit::Percent)) = specified_values.get("font-size") {
            let parent_fs = match parent_style {
                Some(parent) => match parent.get("font-size") {
                    Some(Value::Length(pv, crate::css::Unit::Px)) => *pv,
                    _ => 16.0,
                },
                _ => 16.0,
            };
            specified_values.insert("font-size".to_string(), Value::Length(parent_fs * (v / 100.0), crate::css::Unit::Px));
        }

        // 6. Apply JS overrides (highest priority)
        if let Some(ref element_id) = id {
            if let Some(overrides) = js_overrides.get(element_id) {
                for (k, v) in overrides {
                    specified_values.insert(k.clone(), parse_value(v));
                }
            }
        }
    }

    let children = root.children
        .borrow()
        .iter()
        .map(|child| build_style_tree(child, stylesheet, Some(&specified_values), js_overrides, hovered_id))
        .collect();

    StyledNode {
        node: root.clone(),
        specified_values,
        children,
    }
}

/// Parse `style="..."` attribute content and return a list of declarations.
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

/// Apply default browser-like styles for known HTML tags.
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

/// Apply styles derived from HTML attributes (e.g., width/height on img, color on font).
/// Called with the borrowed attrs from NodeData::Element.
fn apply_attribute_styles(tag: &str, attrs_borrow: &std::cell::Ref<Vec<html5ever::Attribute>>, map: &mut PropertyMap) {
    match tag {
        "img" => {
            for attr in attrs_borrow.iter() {
                match attr.name.local.as_ref() {
                    "width" => {
                        if let Ok(v) = attr.value.trim_end_matches("px").parse::<f32>() {
                            map.insert("width".to_string(), Value::Length(v, crate::css::Unit::Px));
                        }
                    }
                    "height" => {
                        if let Ok(v) = attr.value.trim_end_matches("px").parse::<f32>() {
                            map.insert("height".to_string(), Value::Length(v, crate::css::Unit::Px));
                        }
                    }
                    _ => {}
                }
            }
        }
        "font" => {
            for attr in attrs_borrow.iter() {
                match attr.name.local.as_ref() {
                    "color" => {
                        if let Some(c) = parse_color(&attr.value) {
                            map.insert("color".to_string(), Value::Color(c));
                        }
                    }
                    "size" => {
                        let size_map = [("1", 10.0f32), ("2", 13.0), ("3", 16.0), ("4", 18.0), ("5", 24.0), ("6", 32.0), ("7", 48.0)];
                        let v = attr.value.as_ref();
                        for (s, px) in &size_map {
                            if *s == v {
                                map.insert("font-size".to_string(), Value::Length(*px, crate::css::Unit::Px));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn get_parent(handle: &Handle) -> Option<Handle> {
    let p = handle.parent.take();
    let res = p.as_ref().and_then(|p| p.upgrade());
    handle.parent.set(p);
    res
}

fn matches_selector(selector: &Selector, handle: &Handle, hovered_id: Option<&str>) -> bool {
    // Check current node matches primary part of selector
    if let NodeData::Element { ref name, ref attrs, .. } = handle.data {
        let tag = name.local.to_string();
        let mut id = None;
        let mut classes = Vec::new();
        for attr in attrs.borrow().iter() {
            match attr.name.local.as_ref() {
                "id" => id = Some(attr.value.to_string()),
                "class" => classes = attr.value.to_string().split_whitespace().map(|s| s.to_string()).collect(),
                _ => {}
            }
        }

        // Base match
        let has_constraint = selector.tag.is_some() || selector.id.is_some() || !selector.class.is_empty() || !selector.attributes.is_empty() || selector.pseudo_class.is_some();
        if !has_constraint { return false; }

        if let Some(ref s_tag) = selector.tag {
            if s_tag != &tag && s_tag != "*" { return false; }
        }
        if let Some(ref s_id) = selector.id {
            if Some(s_id.as_str()) != id.as_deref() { return false; }
        }
        for s_class in &selector.class {
            if !classes.contains(s_class) { return false; }
        }

        // Pseudo-class match
        if let Some(ref pseudo) = selector.pseudo_class {
            if pseudo == "hover" {
                if Some(id.as_deref()) != Some(hovered_id) || id.is_none() {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Attribute match
        for attr_sel in &selector.attributes {
            let mut matched = false;
            for attr in attrs.borrow().iter() {
                if attr.name.local.as_ref() == attr_sel.name {
                    match &attr_sel.value {
                        crate::css::AttributeMatch::Exists => { matched = true; break; }
                        crate::css::AttributeMatch::Equals(v) => {
                            if attr.value.as_ref() == v { matched = true; break; }
                        }
                    }
                }
            }
            if !matched { return false; }
        }

        // If there's an ancestor requirement, check it
        if let Some(ref ancestor_sel) = selector.ancestor {
            let combinator = selector.combinator.as_ref().unwrap_or(&Combinator::Descendant);
            
            match combinator {
                Combinator::Descendant => {
                    let mut current = get_parent(handle);
                    let mut matched = false;
                    while let Some(parent_handle) = current {
                        if matches_selector(ancestor_sel, &parent_handle, hovered_id) {
                            matched = true;
                            break;
                        }
                        current = get_parent(&parent_handle);
                    }
                    if !matched { return false; }
                }
                Combinator::Child => {
                    if let Some(p) = get_parent(handle) {
                        if !matches_selector(ancestor_sel, &p, hovered_id) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                Combinator::NextSibling => {
                    if let Some(p) = get_parent(handle) {
                        let children = p.children.borrow();
                        let index = children.iter().position(|child| std::ptr::eq(child, handle));
                        if let Some(idx) = index {
                            let mut found = false;
                            for prev_idx in (0..idx).rev() {
                                let sib = &children[prev_idx];
                                if let NodeData::Element { .. } = sib.data {
                                    if matches_selector(ancestor_sel, sib, hovered_id) {
                                        found = true;
                                    }
                                    break;
                                }
                            }
                            if !found { return false; }
                        } else {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                Combinator::SubsequentSibling => {
                    if let Some(p) = get_parent(handle) {
                        let children = p.children.borrow();
                        let index = children.iter().position(|child| std::ptr::eq(child, handle));
                        if let Some(idx) = index {
                            let mut matched = false;
                            for prev_idx in 0..idx {
                                let sib = &children[prev_idx];
                                if let NodeData::Element { .. } = sib.data {
                                    if matches_selector(ancestor_sel, sib, hovered_id) {
                                        matched = true;
                                        break;
                                    }
                                }
                            }
                            if !matched { return false; }
                        } else {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
            }
        }

        true
    } else {
        false
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
}
