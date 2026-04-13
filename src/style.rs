use crate::css::{Stylesheet, Value, Selector, parse_value, parse_color, Combinator, intern};

use markup5ever_rcdom::{Handle, NodeData};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::hash::{Hash, Hasher};
use rayon::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyMap(pub Arc<HashMap<Arc<str>, Value>>);

impl Hash for PropertyMap {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Since we can't easily hash a HashMap, we use a simple approach:
        // For deduplication in a HashSet, we need a consistent hash.
        // We can sort keys and hash them.
        let mut keys: Vec<&Arc<str>> = self.0.keys().collect();
        keys.sort();
        for k in keys {
            k.hash(state);
            self.0.get(k).unwrap().hash(state);
        }
    }
}

impl std::ops::Deref for PropertyMap {
    type Target = HashMap<Arc<str>, Value>;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Default)]
struct StyleStore {
    cache: HashSet<PropertyMap>,
}

impl StyleStore {
    fn intern(&mut self, map: HashMap<Arc<str>, Value>) -> PropertyMap {
        let wrapper = PropertyMap(Arc::new(map));
        if let Some(existing) = self.cache.get(&wrapper) {
            existing.clone()
        } else {
            self.cache.insert(wrapper.clone());
            wrapper
        }
    }
}

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
        children_idx.push(flatten_dom(child, arena, Some(idx)));
    }
    
    arena[idx].children_idx = children_idx;
    idx
}

fn matches_selector_arena(selector: &Selector, idx: usize, arena: &[NodeDataSend], hovered_id: Option<&str>, focused_id: Option<&str>) -> bool {
    let node = &arena[idx];
    
    let has_constraint = selector.tag.is_some() || selector.id.is_some() || !selector.class.is_empty() || !selector.attributes.is_empty() || selector.pseudo_class.is_some();
    if !has_constraint { return false; }

    if let Some(ref s_tag) = selector.tag {
        if &node.tag != s_tag { return false; }
    }
    if let Some(ref s_id) = selector.id {
        if node.id.as_deref() != Some(s_id) { return false; }
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

fn apply_attribute_styles_arena(node: &NodeDataSend, map: &mut HashMap<Arc<str>, Value>) {
    match node.tag.as_str() {
        "img" => {
            for (k, v) in &node.attrs {
                if k == "width" { if let Ok(val) = v.trim_end_matches("px").parse::<f32>() { map.insert(intern("width"), Value::Length(val, crate::css::Unit::Px)); } }
                if k == "height" { if let Ok(val) = v.trim_end_matches("px").parse::<f32>() { map.insert(intern("height"), Value::Length(val, crate::css::Unit::Px)); } }
            }
        }
        "font" => {
            for (k, v) in &node.attrs {
                if k == "color" { if let Some(c) = parse_color(v) { map.insert(intern("color"), Value::Color(c)); } }
                if k == "size" {
                    let size_map = [("1", 10.0f32), ("2", 13.0), ("3", 16.0), ("4", 18.0), ("5", 24.0), ("6", 32.0), ("7", 48.0)];
                    for (s, px) in &size_map {
                        if s == v { map.insert(intern("font-size"), Value::Length(*px, crate::css::Unit::Px)); }
                    }
                }
            }
        }
        _ => {}
    }
}

impl PropertyMap {
    pub fn new() -> Self {
        Self(Arc::new(HashMap::new()))
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
    _csp_policy: Option<&crate::js::CspPolicy>,
) -> StyledNode {
    let mut arena = Vec::new();
    flatten_dom(root, &mut arena, None);

    // Phase 1: Parallel CSS Matching
    let mut raw_styles: Vec<HashMap<Arc<str>, Value>> = arena.par_iter().enumerate().map(|(idx, node)| {
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
                for (k, v) in overrides { map.insert(intern(k), parse_value(v)); }
            }
        }
        map
    }).collect();

    // Phase 2: Sequential Inheritance & Deduplication
    let mut store = StyleStore::default();
    let mut arena_idx = 0;
    build_final_tree(root, &mut arena_idx, &mut raw_styles, parent_style, &mut store)
}

fn build_final_tree(
    handle: &Handle,
    arena_idx: &mut usize,
    raw_styles: &mut [HashMap<Arc<str>, Value>],
    parent_style: Option<&PropertyMap>,
    store: &mut StyleStore
) -> StyledNode {
    let current_idx = *arena_idx;
    *arena_idx += 1;
    
    let mut specified_values = std::mem::take(&mut raw_styles[current_idx]);
    
    if parent_style.is_none() && current_idx == 0 {
        specified_values.insert(intern("color"), Value::Color(crate::css::Color { r: 0, g: 0, b: 0, a: 255 }));
        specified_values.insert(intern("font-size"), Value::Length(16.0, crate::css::Unit::Px));
    }
    
    if let Some(p) = parent_style {
        let inheritable = ["color", "font-size", "font-family", "font-weight", "line-height", "text-align", "list-style-type"];
        for prop in inheritable {
            let prop_arc = intern(prop);
            if let Some(v) = p.get(&prop_arc) {
                specified_values.entry(prop_arc).or_insert_with(|| v.clone());
            }
        }
    }
    
    // Simple Em/Percent resolution for font-size
    let mut resolved_fs = 16.0f32;
    let fs_key = intern("font-size");
    if let Some(val) = specified_values.get(&fs_key) {
        match val {
            Value::Length(v, crate::css::Unit::Px) => resolved_fs = *v,
            Value::Length(v, crate::css::Unit::Percent) => {
                let parent_fs = match parent_style {
                    Some(p) => match p.get(&fs_key) { Some(Value::Length(pv, crate::css::Unit::Px)) => *pv, _ => 16.0 }, _ => 16.0
                };
                resolved_fs = parent_fs * (v / 100.0);
            }
            Value::Length(v, crate::css::Unit::Em) => {
                let parent_fs = match parent_style {
                    Some(p) => match p.get(&fs_key) { Some(Value::Length(pv, crate::css::Unit::Px)) => *pv, _ => 16.0 }, _ => 16.0
                };
                resolved_fs = parent_fs * v;
            }
            _ => {}
        }
    }
    if resolved_fs != 16.0 {
        specified_values.insert(fs_key, Value::Length(resolved_fs, crate::css::Unit::Px));
    }

    // Intern the final map
    let interned_map = store.intern(specified_values);
    
    let children = handle.children.borrow().iter().map(|child| {
        build_final_tree(child, arena_idx, raw_styles, Some(&interned_map), store)
    }).collect();

    StyledNode {
        node: handle.clone(),
        specified_values: interned_map,
        children,
    }
}

fn apply_default_styles(tag: &str, map: &mut HashMap<Arc<str>, Value>) {
    match tag {
        "h1" => {
            map.entry(intern("font-size")).or_insert(Value::Length(32.0, crate::css::Unit::Px));
            map.entry(intern("font-weight")).or_insert(Value::Keyword(intern("bold")));
            map.entry(intern("margin-top")).or_insert(Value::Length(21.0, crate::css::Unit::Px));
            map.entry(intern("margin-bottom")).or_insert(Value::Length(21.0, crate::css::Unit::Px));
        }
        "h2" => {
            map.entry(intern("font-size")).or_insert(Value::Length(24.0, crate::css::Unit::Px));
            map.entry(intern("font-weight")).or_insert(Value::Keyword(intern("bold")));
            map.entry(intern("margin-top")).or_insert(Value::Length(14.0, crate::css::Unit::Px));
            map.entry(intern("margin-bottom")).or_insert(Value::Length(14.0, crate::css::Unit::Px));
        }
        "h3" => {
            map.entry(intern("font-size")).or_insert(Value::Length(18.0, crate::css::Unit::Px));
            map.entry(intern("font-weight")).or_insert(Value::Keyword(intern("bold")));
        }
        "h4" | "h5" | "h6" => {
            map.entry(intern("font-weight")).or_insert(Value::Keyword(intern("bold")));
        }
        "a" => {
            map.entry(intern("color")).or_insert(Value::Color(parse_color("#0000ee").unwrap()));
            map.entry(intern("text-decoration")).or_insert(Value::Keyword(intern("underline")));
        }
        "strong" | "b" => {
            map.entry(intern("font-weight")).or_insert(Value::Keyword(intern("bold")));
        }
        "em" | "i" => {
            map.entry(intern("font-style")).or_insert(Value::Keyword(intern("italic")));
        }
        "code" | "pre" | "kbd" | "samp" => {
            map.entry(intern("font-family")).or_insert(Value::Keyword(intern("monospace")));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 240, g: 240, b: 240, a: 255 }));
        }
        "button" | "input" | "select" | "textarea" => {
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            map.entry(intern("padding")).or_insert(Value::Length(4.0, crate::css::Unit::Px));
        }
        "ul" | "ol" => {
            map.entry(intern("padding-left")).or_insert(Value::Length(24.0, crate::css::Unit::Px));
        }
        "p" => {
            map.entry(intern("margin-top")).or_insert(Value::Length(8.0, crate::css::Unit::Px));
            map.entry(intern("margin-bottom")).or_insert(Value::Length(8.0, crate::css::Unit::Px));
        }
        _ => {}
    }
}

pub fn parse_inline_style_into_vec(style_str: &str, list: &mut Vec<crate::css::Declaration>) {
    for decl in style_str.split(';') {
        let decl = decl.trim();
        if decl.is_empty() { continue; }
        let mut kv = decl.splitn(2, ':');
        let key = intern(&kv.next().unwrap_or("").trim().to_lowercase());
        let val_raw = kv.next().unwrap_or("").trim();
        if key.is_empty() || val_raw.is_empty() { continue; }

        let important = val_raw.ends_with("!important");
        let val = if important { val_raw.trim_end_matches("!important").trim() } else { val_raw };

        match &*key {
            "border" => {
                let mut temp_map = HashMap::new();
                crate::css::parse_border_shorthand_pub(val, &mut temp_map);
                for (k, v) in temp_map {
                    list.push(crate::css::Declaration { name: intern(&k), value: v, important });
                }
            }
            "padding" => {
                let mut temp_map = HashMap::new();
                crate::css::parse_quad_shorthand(intern("padding").as_ref(), val, &mut temp_map);
                for (k, v) in temp_map {
                    list.push(crate::css::Declaration { name: intern(&k), value: v, important });
                }
            }
            "margin" => {
                let mut temp_map = HashMap::new();
                crate::css::parse_quad_shorthand(intern("margin").as_ref(), val, &mut temp_map);
                for (k, v) in temp_map {
                    list.push(crate::css::Declaration { name: intern(&k), value: v, important });
                }
            }
            _ => {
                list.push(crate::css::Declaration {
                    name: key,
                    value: parse_value(val),
                    important,
                });
            }
        }
    }
}
