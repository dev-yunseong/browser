use crate::css::{Stylesheet, Value, Selector, parse_value, parse_color, Combinator, intern, SelectorKey};

use markup5ever_rcdom::{Handle, NodeData};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
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

/// An entry in the selector index pointing to a specific selector within a rule.
#[derive(Clone)]
struct IndexEntry {
    specificity: (usize, usize, usize),
    rule_idx: usize,
    sel_idx: usize,
    /// True when the selector has an ancestor part (needs DOM context for full match).
    is_complex: bool,
}

/// Pre-built index that buckets selectors by their key feature for O(1) candidate lookup.
/// Built once per stylesheet before the parallel matching phase.
struct SelectorIndex {
    by_id:    HashMap<String, Vec<IndexEntry>>,
    by_class: HashMap<String, Vec<IndexEntry>>,
    by_tag:   HashMap<String, Vec<IndexEntry>>,
    universal: Vec<IndexEntry>,
}

impl SelectorIndex {
    fn build(stylesheet: &Stylesheet) -> Self {
        let mut by_id: HashMap<String, Vec<IndexEntry>> = HashMap::new();
        let mut by_class: HashMap<String, Vec<IndexEntry>> = HashMap::new();
        let mut by_tag: HashMap<String, Vec<IndexEntry>> = HashMap::new();
        let mut universal: Vec<IndexEntry> = Vec::new();

        for (rule_idx, rule) in stylesheet.all_rules().iter().enumerate() {
            for (sel_idx, sel) in rule.selectors.iter().enumerate() {
                let entry = IndexEntry {
                    specificity: sel.specificity(),
                    rule_idx,
                    sel_idx,
                    // Mark as complex if it has an ancestor combinator OR attribute constraints.
                    // Attribute selectors depend on per-node attribute values, which are not
                    // captured in the ElementSignature, so they must bypass the signature cache.
                    is_complex: sel.ancestor.is_some() || !sel.attributes.is_empty(),
                };
                match sel.key_feature() {
                    SelectorKey::Id(id)    => by_id.entry(id).or_default().push(entry),
                    SelectorKey::Class(cls) => by_class.entry(cls).or_default().push(entry),
                    SelectorKey::Tag(tag)  => by_tag.entry(tag).or_default().push(entry),
                    SelectorKey::Universal => universal.push(entry),
                }
            }
        }
        SelectorIndex { by_id, by_class, by_tag, universal }
    }

    /// Collect candidate entries for a given element node.
    /// Returns entries that *might* match the node (false positives possible for complex selectors;
    /// full `matches_selector_arena` call is still required to confirm).
    fn candidates<'a>(&'a self, node: &NodeDataSend) -> Vec<&'a IndexEntry> {
        let mut out: Vec<&IndexEntry> = Vec::new();
        out.extend(self.universal.iter());
        if !node.tag.is_empty() {
            if let Some(entries) = self.by_tag.get(&node.tag) {
                out.extend(entries.iter());
            }
        }
        for cls in &node.classes {
            if let Some(entries) = self.by_class.get(cls) {
                out.extend(entries.iter());
            }
        }
        if let Some(ref id) = node.id {
            if let Some(entries) = self.by_id.get(id) {
                out.extend(entries.iter());
            }
        }
        out
    }
}

/// Signature of an element for cache lookup. Classes are sorted so identical
/// sets of classes map to the same signature regardless of DOM order.
#[derive(Hash, Eq, PartialEq)]
struct ElementSignature {
    tag: String,
    id: Option<String>,
    classes: Vec<String>, // sorted
}

impl ElementSignature {
    fn from_node(node: &NodeDataSend) -> Self {
        let mut classes = node.classes.clone();
        classes.sort_unstable();
        ElementSignature { tag: node.tag.clone(), id: node.id.clone(), classes }
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
fn flatten_dom(root: &Handle, arena: &mut Vec<NodeDataSend>, root_parent_idx: Option<usize>) -> usize {
    // Iterative replacement for the formerly recursive flatten_dom.
    // Uses an explicit heap stack to avoid stack overflows on deeply nested DOMs.
    //
    // Strategy:
    //   1. Push (handle, parent_idx) pairs onto the stack, children in reverse
    //      order so the first child is popped (and inserted into the arena) first.
    //   2. After the traversal, reconstruct children_idx by scanning the arena —
    //      every node already knows its parent_idx.
    let start_idx = arena.len();
    let mut stack: Vec<(Handle, Option<usize>)> = vec![(root.clone(), root_parent_idx)];

    while let Some((handle, parent_idx)) = stack.pop() {
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

        // Push children in REVERSE order so the first child is popped first,
        // preserving the original document (left-to-right) visit order.
        for child in handle.children.borrow().iter().rev() {
            stack.push((child.clone(), Some(idx)));
        }
    }

    // Reconstruct children_idx: every node knows its parent, so walk forward
    // and append each node's index to its parent's children list.
    for i in start_idx..arena.len() {
        if let Some(p) = arena[i].parent_idx {
            // Only link nodes that were created in this call (root_parent_idx
            // nodes belong to a different sub-tree inserted earlier).
            if p >= start_idx || root_parent_idx.map_or(true, |rp| p != rp) {
                arena[p].children_idx.push(i);
            }
        }
    }

    start_idx
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
        } else if pseudo == "root" {
            // :root matches the root element of the document — the <html> element.
            if node.tag != "html" { return false; }
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
        "table" | "td" | "th" => {
            for (k, v) in &node.attrs {
                if k == "width" {
                    if let Some(width) = parse_legacy_length_attr(v) {
                        map.insert(intern("width"), width);
                    }
                }
                if k == "align" {
                    let align = v.to_ascii_lowercase();
                    if matches!(align.as_str(), "left" | "center" | "right") {
                        map.insert(intern("text-align"), Value::Keyword(intern(&align)));
                    }
                }
            }
        }
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
        // <input type="hidden"> must not render — set display:none.
        "input" => {
            for (k, v) in &node.attrs {
                if k == "type" && v.eq_ignore_ascii_case("hidden") {
                    map.insert(intern("display"), Value::Keyword(intern("none")));
                    break;
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

    // Pre-build selector index: O(M) — done once before the parallel phase.
    let sel_index = SelectorIndex::build(stylesheet);
    // Snapshot all_rules into a Vec so we can index into it by rule_idx.
    let all_rules: Vec<&crate::css::Rule> = stylesheet.all_rules();

    // Pre-build element signature cache for simple (no-combinator) selectors.
    // Maps ElementSignature -> Vec<(specificity, rule_idx)>.
    // Computed sequentially once; read-only inside par_iter (HashMap is Sync when V is Sync).
    let mut sig_cache: HashMap<ElementSignature, Vec<(( usize, usize, usize), usize)>> = HashMap::new();
    for (idx, node) in arena.iter().enumerate() {
        if !node.is_element { continue; }
        let sig = ElementSignature::from_node(node);
        if sig_cache.contains_key(&sig) { continue; }
        // Gather simple-selector matches for this signature.
        // "Simple" means no ancestor combinator — result is position-independent.
        let candidates = sel_index.candidates(node);
        let mut rule_best: HashMap<usize, (usize, usize, usize)> = HashMap::new();
        for entry in &candidates {
            if entry.is_complex { continue; } // skip; needs full DOM context
            let sel = &all_rules[entry.rule_idx].selectors[entry.sel_idx];
            if matches_selector_arena(sel, idx, &arena, hovered_id, focused_id) {
                let e = rule_best.entry(entry.rule_idx).or_insert((0, 0, 0));
                if entry.specificity > *e { *e = entry.specificity; }
            }
        }
        let mut matched: Vec<((usize, usize, usize), usize)> = rule_best.into_iter().map(|(ridx, spec)| (spec, ridx)).collect();
        matched.sort_by_key(|&(spec, _)| spec);
        sig_cache.insert(sig, matched);
    }

    // Phase 1: Parallel CSS Matching (index-accelerated, O(N × bucket_size))
    //
    // Memory bound: cap at 4 threads so peak RSS stays bounded on large pages.
    // Static pool avoids recreating threads on every render call.
    static CSS_POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    let pool = CSS_POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .expect("CSS thread pool init failed")
    });

    let mut raw_styles: Vec<HashMap<Arc<str>, Value>> = pool.install(|| {
        arena.par_iter().enumerate().map(|(idx, node)| {
        if !node.is_element { return HashMap::new(); }
        let mut map = HashMap::new();
        apply_default_styles(&node.tag, &mut map);

        // --- Collect matching rules ---
        // Use a Vec to track (rule_idx, max_specificity) without HashMap allocation.
        let mut rule_matches: Vec<(usize, (usize, usize, usize))> = Vec::new();

        // 1. Simple-selector matches via the signature cache (no DOM traversal needed).
        // sig_cache is already deduped by rule_idx — extend directly, no find needed.
        let sig = ElementSignature::from_node(node);
        if let Some(simple_matches) = sig_cache.get(&sig) {
            rule_matches.extend(simple_matches.iter().map(|&(spec, rule_idx)| (rule_idx, spec)));
        }

        // 2. Complex-selector matches via the index (need full DOM context for ancestor checks)
        let candidates = sel_index.candidates(node);
        for entry in &candidates {
            if !entry.is_complex { continue; }
            let sel = &all_rules[entry.rule_idx].selectors[entry.sel_idx];
            if matches_selector_arena(sel, idx, &arena, hovered_id, focused_id) {
                if let Some(existing) = rule_matches.iter_mut().find(|(ridx, _)| *ridx == entry.rule_idx) {
                    if entry.specificity > existing.1 { existing.1 = entry.specificity; }
                } else {
                    rule_matches.push((entry.rule_idx, entry.specificity));
                }
            }
        }

        // Sort by specificity (ascending) so higher specificity overwrites lower
        rule_matches.sort_by_key(|&(_, spec)| spec);

        // Apply matched rules; defer important declarations.
        // Only allocate `important` if needed (most nodes have no !important rules).
        let mut important: Vec<(Arc<str>, Value)> = Vec::new();
        let mut inline_important: Vec<(Arc<str>, Value)> = Vec::new();
        for (rule_idx, _) in &rule_matches {
            for decl in &all_rules[*rule_idx].declarations {
                if !decl.important { map.insert(decl.name.clone(), decl.value.clone()); }
                else { important.push((decl.name.clone(), decl.value.clone())); }
            }
        }

        apply_attribute_styles_arena(node, &mut map);

        if let Some(v) = node.attrs.iter().find(|(k, _)| k == "style").map(|(_, v)| v) {
            let mut inline_map = Vec::new();
            parse_inline_style_into_vec(v, &mut inline_map);
            for decl in inline_map {
                if !decl.important { map.insert(decl.name, decl.value); }
                else { inline_important.push((decl.name, decl.value)); }
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
    }).collect()
    });

    // Phase 2: Sequential Inheritance & Deduplication
    let mut store = StyleStore::default();
    let mut arena_idx = 0;
    let mut tree = build_final_tree(root, &mut arena_idx, &mut raw_styles, parent_style, &mut store);

    // Phase 3: Inject ::before / ::after pseudo-element children.
    // For each element node in the tree, find CSS rules whose selectors have a
    // matching pseudo_element ("before" or "after").  When a matching rule provides
    // a `content` property that is not "none" or "normal", inject a synthetic text
    // StyledNode as the first (before) or last (after) child of the element.
    let all_rules: Vec<&crate::css::Rule> = stylesheet.all_rules();
    inject_pseudo_elements(&mut tree, &all_rules);

    tree
}

/// Returns the CSS initial value for a given property name, or `None` if not defined here.
/// Only properties that can be set to `initial` keyword need an entry.
fn initial_value(prop: &str) -> Option<Value> {
    use crate::css::{Unit, Color};
    match prop {
        "color"            => Some(Value::Color(Color { r: 0, g: 0, b: 0, a: 255 })),
        "font-size"        => Some(Value::Length(16.0, Unit::Px)),
        "font-weight"      => Some(Value::Keyword(intern("normal"))),
        "font-style"       => Some(Value::Keyword(intern("normal"))),
        "font-family"      => Some(Value::Keyword(intern("serif"))),
        "text-align"       => Some(Value::Keyword(intern("left"))),
        "text-decoration"  => Some(Value::Keyword(intern("none"))),
        "line-height"      => Some(Value::Keyword(intern("normal"))),
        "display"          => Some(Value::Keyword(intern("inline"))),
        "visibility"       => Some(Value::Keyword(intern("visible"))),
        "background-color" => Some(Value::Keyword(intern("transparent"))),
        "opacity"          => Some(Value::Number(1.0)),
        "border-width"     => Some(Value::Length(0.0, Unit::Px)),
        "border-style"     => Some(Value::Keyword(intern("none"))),
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" |
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" =>
            Some(Value::Length(0.0, Unit::Px)),
        _ => None,
    }
}

/// Maximum recursion depth for `var()` resolution, to prevent infinite loops from
/// cyclic custom property references (e.g. `--a: var(--b); --b: var(--a)`).
const VAR_RESOLVE_MAX_DEPTH: u32 = 32;

/// Resolve `Value::CssVar` references using the provided custom properties map.
/// `depth` tracks recursion depth; returns `None` when the limit is reached.
fn resolve_var(value: &Value, custom_props: &HashMap<Arc<str>, Value>, depth: u32) -> Option<Value> {
    if depth > VAR_RESOLVE_MAX_DEPTH { return None; }
    if let Value::CssVar { name, fallback } = value {
        if let Some(raw) = custom_props.get(name) {
            if let Value::RawCustomProp(raw_str) = raw {
                // Re-parse the raw string as a CSS value at use time.
                let resolved = crate::css::parse_value(raw_str);
                // Recurse in case the resolved value is itself a var().
                return Some(resolve_var_value(resolved, custom_props, depth + 1));
            }
        }
        // Custom property not found — use fallback if present.
        if let Some(fb) = fallback {
            return Some(resolve_var_value(*fb.clone(), custom_props, depth + 1));
        }
        return None;
    }
    None
}

/// Recursively resolve any `CssVar` values inside `value`.
fn resolve_var_value(value: Value, custom_props: &HashMap<Arc<str>, Value>, depth: u32) -> Value {
    match &value {
        Value::CssVar { .. } => resolve_var(&value, custom_props, depth).unwrap_or(value),
        _ => value,
    }
}

fn build_final_tree(
    root: &Handle,
    arena_idx: &mut usize,
    raw_styles: &mut [HashMap<Arc<str>, Value>],
    initial_parent_style: Option<&PropertyMap>,
    store: &mut StyleStore
) -> StyledNode {
    // Iterative replacement for the formerly recursive build_final_tree.
    //
    // The original function performs two interleaved operations:
    //   1. Pre-order (top-down): compute specified_values using the parent's PropertyMap.
    //   2. Post-order (bottom-up): assemble StyledNode once all children are known.
    //
    // Key insight for arena_idx: Pre frames must NOT store the arena index at push time,
    // because sibling subtrees haven't been processed yet.  Instead, each Pre frame reads
    // `*arena_idx` LIVE when it is popped — by that point all preceding Pre frames have
    // already incremented the counter, so `*arena_idx` is the correct sequential index for
    // the current node, matching exactly how `flatten_dom` assigned indices in pre-order.
    //
    // Stack discipline (LIFO):
    //   - Push children in REVERSE so the first child is at the top (popped first).
    //   - Push Post BEFORE children so Post is processed AFTER all descendants finish.

    enum Frame {
        Pre {
            handle: Handle,
            parent_pm: Option<PropertyMap>,
        },
        Post {
            handle: Handle,
            specified_values: PropertyMap,
            num_children: usize,
        },
    }

    let mut work: Vec<Frame> = vec![Frame::Pre {
        handle: root.clone(),
        parent_pm: initial_parent_style.cloned(),
    }];
    let mut results: Vec<StyledNode> = Vec::new();

    while let Some(frame) = work.pop() {
        match frame {
            Frame::Pre { handle, parent_pm } => {
                // Read the current sequential index BEFORE incrementing.
                let current_idx = *arena_idx;
                *arena_idx += 1;

                // Compute specified_values: apply inheritance, defaults, em/% resolution.
                let mut specified_values = std::mem::take(&mut raw_styles[current_idx]);

                if parent_pm.is_none() && current_idx == 0 {
                    specified_values.entry(intern("color")).or_insert_with(|| Value::Color(crate::css::Color { r: 0, g: 0, b: 0, a: 255 }));
                    specified_values.entry(intern("font-size")).or_insert_with(|| Value::Length(16.0, crate::css::Unit::Px));
                }

                // --- Step 1: Collect custom properties from this element's map ---
                // CSS custom properties are inherited by default.
                let mut custom_props: HashMap<Arc<str>, Value> = HashMap::new();
                // Inherit parent custom properties first.
                if let Some(ref p) = parent_pm {
                    for (k, v) in p.iter() {
                        if k.starts_with("--") {
                            custom_props.insert(k.clone(), v.clone());
                            // Also insert into specified_values so they flow into the
                            // PropertyMap and can be inherited by grandchildren.
                            specified_values.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }
                // Override/add with this element's own custom properties.
                for (k, v) in &specified_values {
                    if k.starts_with("--") {
                        custom_props.insert(k.clone(), v.clone());
                    }
                }

                // --- Step 2: Inherit inheritable properties (unless explicitly set) ---
                if let Some(ref p) = parent_pm {
                    let inheritable = [
                        "color",
                        "font-size",
                        "font-family",
                        "font-weight",
                        "font-style",
                        "line-height",
                        "text-align",
                        "list-style-type",
                        "white-space",
                    ];
                    for prop in inheritable {
                        let prop_arc = intern(prop);
                        if let Some(v) = p.get(&prop_arc) {
                            specified_values.entry(prop_arc).or_insert_with(|| v.clone());
                        }
                    }
                }

                // --- Step 3: Resolve font-size em/% first (needs parent font-size) ---
                let fs_key = intern("font-size");
                let parent_fs = parent_pm.as_ref()
                    .and_then(|p| p.get(&fs_key))
                    .and_then(|v| if let Value::Length(pv, crate::css::Unit::Px) = v { Some(*pv) } else { None })
                    .unwrap_or(16.0);
                if let Some(val) = specified_values.get(&fs_key) {
                    let resolved_fs = match val {
                        Value::Length(v, crate::css::Unit::Px) => Some(*v),
                        Value::Length(v, crate::css::Unit::Percent) => Some(parent_fs * (v / 100.0)),
                        Value::Length(v, crate::css::Unit::Em) => Some(parent_fs * v),
                        Value::Keyword(kw) if kw.as_ref() == "inherit" => Some(parent_fs),
                        Value::Keyword(kw) if kw.as_ref() == "initial" => Some(16.0),
                        Value::CssVar { .. } => {
                            resolve_var(val, &custom_props, 0)
                                .and_then(|resolved| if let Value::Length(pv, crate::css::Unit::Px) = resolved { Some(pv) } else { None })
                        }
                        _ => None,
                    };
                    if let Some(fs) = resolved_fs {
                        specified_values.insert(fs_key.clone(), Value::Length(fs, crate::css::Unit::Px));
                    }
                }
                let own_fs = specified_values.get(&fs_key)
                    .and_then(|v| if let Value::Length(pv, crate::css::Unit::Px) = v { Some(*pv) } else { None })
                    .unwrap_or(parent_fs);

                // --- Step 4: Resolve inherit / initial / var() / em (non-font-size) / currentColor ---
                let color_key = intern("color");
                // We need a snapshot of the current color for currentColor resolution.
                // First resolve the color property itself if needed.
                let own_color = {
                    let color_val = specified_values.get(&color_key).cloned();
                    match color_val.as_ref() {
                        Some(Value::Keyword(kw)) if kw.as_ref() == "inherit" => {
                            parent_pm.as_ref()
                                .and_then(|p| p.get(&color_key))
                                .cloned()
                                .or_else(|| initial_value("color"))
                        }
                        Some(Value::Keyword(kw)) if kw.as_ref() == "initial" => initial_value("color"),
                        Some(Value::CssVar { .. }) => {
                            color_val.as_ref().and_then(|v| resolve_var(v, &custom_props, 0))
                        }
                        Some(v) => Some(v.clone()),
                        None => parent_pm.as_ref().and_then(|p| p.get(&color_key)).cloned(),
                    }
                };
                if let Some(ref c) = own_color {
                    specified_values.insert(color_key.clone(), c.clone());
                }

                // Now resolve all other properties.
                let keys: Vec<Arc<str>> = specified_values.keys()
                    .filter(|k| k.as_ref() != "font-size" && k.as_ref() != "color" && !k.starts_with("--"))
                    .cloned()
                    .collect();
                for key in keys {
                    let val = specified_values[&key].clone();
                    let resolved = match &val {
                        Value::Keyword(kw) if kw.as_ref() == "inherit" => {
                            parent_pm.as_ref()
                                .and_then(|p| p.get(&key))
                                .cloned()
                                .or_else(|| initial_value(&key))
                        }
                        Value::Keyword(kw) if kw.as_ref() == "initial" => initial_value(&key),
                        Value::Keyword(kw) if kw.as_ref().eq_ignore_ascii_case("currentcolor") => {
                            own_color.clone()
                        }
                        Value::CssVar { .. } => {
                            resolve_var(&val, &custom_props, 0).map(|v| {
                                // After resolving var(), also resolve em/currentColor on the result.
                                match &v {
                                    Value::Length(n, crate::css::Unit::Em) => Value::Length(n * own_fs, crate::css::Unit::Px),
                                    Value::Keyword(kw) if kw.as_ref().eq_ignore_ascii_case("currentcolor") => {
                                        own_color.clone().unwrap_or(v.clone())
                                    }
                                    _ => v,
                                }
                            })
                        }
                        // Em resolution for non-font-size properties (resolves against own font-size).
                        Value::Length(n, crate::css::Unit::Em) => {
                            Some(Value::Length(n * own_fs, crate::css::Unit::Px))
                        }
                        _ => None,
                    };
                    if let Some(r) = resolved {
                        specified_values.insert(key, r);
                    }
                }

                let interned_map = store.intern(specified_values);

                let children_handles: Vec<Handle> = handle.children.borrow().iter().cloned().collect();
                let num_children = children_handles.len();

                // Push Post FIRST — it will be processed only after ALL descendants finish.
                work.push(Frame::Post {
                    handle,
                    specified_values: interned_map.clone(),
                    num_children,
                });

                // Push Pre frames for children in REVERSE order so the first child
                // is at the top of the stack and popped first (forward document order).
                for child_handle in children_handles.into_iter().rev() {
                    work.push(Frame::Pre {
                        handle: child_handle,
                        parent_pm: Some(interned_map.clone()),
                    });
                }
            }
            Frame::Post { handle, specified_values, num_children } => {
                // Children have all been processed and pushed onto `results`.
                // Drain the last num_children entries — they are in forward order
                // because children were pushed in reverse (LIFO gives forward order).
                let start = results.len().saturating_sub(num_children);
                let children: Vec<StyledNode> = results.drain(start..).collect();
                results.push(StyledNode {
                    node: handle,
                    specified_values,
                    children,
                });
            }
        }
    }

    results.pop().expect("build_final_tree: results stack should have exactly one element")
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
        "input" => {
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            map.entry(intern("padding")).or_insert(Value::Length(4.0, crate::css::Unit::Px));
            // UA defaults: HTML spec §14.3 — <input> default size = 20 chars ≈ 160 px at 13 px font.
            map.entry(intern("width")).or_insert(Value::Length(160.0, crate::css::Unit::Px));
            // Real browsers render single-line inputs at ~21 px; use 24 for legibility.
            map.entry(intern("height")).or_insert(Value::Length(24.0, crate::css::Unit::Px));
        }
        "textarea" => {
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            map.entry(intern("padding")).or_insert(Value::Length(4.0, crate::css::Unit::Px));
            // UA defaults: cols=20, rows=2 → ~160 × 48 px.
            map.entry(intern("width")).or_insert(Value::Length(160.0, crate::css::Unit::Px));
            map.entry(intern("height")).or_insert(Value::Length(48.0, crate::css::Unit::Px));
        }
        "select" => {
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            map.entry(intern("padding")).or_insert(Value::Length(4.0, crate::css::Unit::Px));
            map.entry(intern("width")).or_insert(Value::Length(120.0, crate::css::Unit::Px));
            map.entry(intern("height")).or_insert(Value::Length(24.0, crate::css::Unit::Px));
        }
        "button" => {
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 240, g: 240, b: 240, a: 255 }));
            map.entry(intern("padding")).or_insert(Value::Length(4.0, crate::css::Unit::Px));
            // Width is content-driven (shrink-wrap in layout); enforce a minimum height.
            map.entry(intern("min-height")).or_insert(Value::Length(24.0, crate::css::Unit::Px));
        }
        // <center> is a legacy presentational element — UA default maps it to a block
        // with text-align: center, matching browsers' built-in stylesheet.
        "center" => {
            map.entry(intern("display")).or_insert(Value::Keyword(intern("block")));
            map.entry(intern("text-align")).or_insert(Value::Keyword(intern("center")));
        }
        "ul" => {
            map.entry(intern("padding-left")).or_insert(Value::Length(40.0, crate::css::Unit::Px));
            map.entry(intern("list-style-type")).or_insert(Value::Keyword(intern("disc")));
        }
        "ol" => {
            map.entry(intern("padding-left")).or_insert(Value::Length(40.0, crate::css::Unit::Px));
            map.entry(intern("list-style-type")).or_insert(Value::Keyword(intern("decimal")));
        }
        "p" => {
            map.entry(intern("margin-top")).or_insert(Value::Length(8.0, crate::css::Unit::Px));
            map.entry(intern("margin-bottom")).or_insert(Value::Length(8.0, crate::css::Unit::Px));
        }
        _ => {}
    }
}

fn parse_legacy_length_attr(value: &str) -> Option<Value> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(percent) = trimmed.strip_suffix('%') {
        return percent
            .trim()
            .parse::<f32>()
            .ok()
            .map(|n| Value::Length(n, crate::css::Unit::Percent));
    }
    trimmed
        .trim_end_matches("px")
        .trim()
        .parse::<f32>()
        .ok()
        .map(|n| Value::Length(n, crate::css::Unit::Px))
}

// ── Pseudo-element injection ──────────────────────────────────────────────────

/// Check whether a CSS selector (ignoring its `pseudo_element` field) matches a
/// styled element node.  This is a simplified match: it only checks the
/// rightmost part of the selector (tag / id / class) and does not walk ancestor
/// combinators.  This is sufficient for the most common pseudo-element patterns
/// (`p::before`, `.clearfix::after`, `div.foo::before`, etc.).
///
/// Returns `true` when the selector base matches `node`.
fn selector_base_matches_element(sel: &crate::css::Selector, node: &StyledNode) -> bool {
    use crate::css::Selector;
    let (tag, id, classes) = match &node.node.data {
        NodeData::Element { ref name, ref attrs, .. } => {
            let t = name.local.to_string();
            let mut id: Option<String> = None;
            let mut classes: Vec<String> = Vec::new();
            for attr in attrs.borrow().iter() {
                let k = attr.name.local.to_string();
                let v = attr.value.to_string();
                if k == "id" { id = Some(v.clone()); }
                if k == "class" {
                    classes = v.split_whitespace().map(|s| s.to_string()).collect();
                }
            }
            (t, id, classes)
        }
        _ => return false, // Only elements can have pseudo-elements
    };

    if let Some(ref s_tag) = sel.tag {
        if *s_tag != tag { return false; }
    }
    if let Some(ref s_id) = sel.id {
        if id.as_deref() != Some(s_id) { return false; }
    }
    for s_class in &sel.class {
        if !classes.contains(s_class) { return false; }
    }
    // Require at least one constraint to avoid matching everything with `*::before`.
    let has_constraint = sel.tag.is_some() || sel.id.is_some() || !sel.class.is_empty();
    has_constraint
}

/// Build a synthetic `StyledNode` that acts as a pseudo-element.
///
/// `content_text` — the text to inject (may be empty for block-level decorators).
/// `pseudo_decls` — the CSS declarations from the matching rule.
/// `parent_values` — the parent element's computed style, used to inherit properties.
fn make_pseudo_styled_node(
    content_text: String,
    pseudo_decls: &[crate::css::Declaration],
    parent_values: &PropertyMap,
) -> StyledNode {
    use html5ever::tendril::StrTendril;
    use markup5ever_rcdom::Node;

    // Create a real text node handle so that layout recognises it as a text run.
    let text_handle = Node::new(NodeData::Text {
        contents: std::cell::RefCell::new(StrTendril::from(content_text.as_str())),
    });

    // Build the property map: start with inheritable properties from parent, then
    // apply the pseudo-element's own declarations on top.
    let mut map: HashMap<Arc<str>, Value> = HashMap::new();

    // Inherit a small set of properties from the parent element.
    let inheritable = [
        "color", "font-size", "font-family", "font-weight",
        "font-style", "line-height", "text-align", "white-space",
    ];
    for prop in &inheritable {
        let key = intern(prop);
        if let Some(v) = parent_values.get(&key) {
            map.insert(key, v.clone());
        }
    }

    // Apply pseudo-element declarations (skip `content` itself).
    for decl in pseudo_decls {
        if decl.name.as_ref() == "content" { continue; }
        map.insert(decl.name.clone(), decl.value.clone());
    }

    StyledNode {
        node: text_handle,
        specified_values: PropertyMap(Arc::new(map)),
        children: Vec::new(),
    }
}

/// Compute which synthetic nodes to inject for a single element node.
/// Returns `(before, after)` where each is `Some(StyledNode)` if a matching
/// `::before` / `::after` rule with a valid `content` exists.
fn compute_pseudo_injections(
    node: &StyledNode,
    all_rules: &[&crate::css::Rule],
) -> (Option<StyledNode>, Option<StyledNode>) {
    if !matches!(node.node.data, NodeData::Element { .. }) {
        return (None, None);
    }

    let parent_values = &node.specified_values;
    let mut before_node: Option<StyledNode> = None;
    let mut after_node:  Option<StyledNode> = None;

    for rule in all_rules {
        for sel in &rule.selectors {
            let pe = match &sel.pseudo_element {
                Some(pe) => pe.as_str(),
                None => continue,
            };
            if pe != "before" && pe != "after" { continue; }

            if !selector_base_matches_element(sel, node) { continue; }

            // Find `content` declaration; skip the rule if absent or suppressed.
            let content_decl = rule.declarations.iter().find(|d| d.name.as_ref() == "content");
            let content_str = match content_decl {
                Some(decl) => match &decl.value {
                    Value::Keyword(k) if matches!(k.as_ref(), "none" | "normal") => continue,
                    Value::Keyword(k) => {
                        // Strip wrapping quotes that the CSS parser preserves.
                        let s = k.as_ref();
                        if (s.starts_with('"') && s.ends_with('"'))
                            || (s.starts_with('\'') && s.ends_with('\''))
                        {
                            s[1..s.len()-1].to_string()
                        } else {
                            s.to_string()
                        }
                    }
                    _ => continue,
                },
                None => continue,
            };

            let synthetic = make_pseudo_styled_node(content_str, &rule.declarations, parent_values);
            match pe {
                "before" => { before_node = Some(synthetic); }
                "after"  => { after_node  = Some(synthetic); }
                _ => {}
            }
        }
    }

    (before_node, after_node)
}

/// Walk the entire styled tree and inject synthetic `::before` / `::after` children
/// wherever a matching CSS rule with a valid `content` property exists.
///
/// Uses an iterative depth-first traversal via raw pointers to avoid stack
/// overflows on deeply nested DOM trees (5 000+ levels).
fn inject_pseudo_elements(tree: &mut StyledNode, all_rules: &[&crate::css::Rule]) {
    // SAFETY: each raw pointer is derived from a live mutable reference.
    // We never alias two mutable references to the same node simultaneously.
    let mut work: Vec<*mut StyledNode> = vec![tree as *mut StyledNode];

    while let Some(ptr) = work.pop() {
        let node = unsafe { &mut *ptr };

        // Step 1: inject pseudo-elements for this node FIRST, before pushing children.
        let (before, after) = compute_pseudo_injections(node, all_rules);
        if let Some(b) = before { node.children.insert(0, b); }
        if let Some(a) = after  { node.children.push(a); }

        // Step 2: push children onto the work stack after the Vec is stable.
        for child in node.children.iter_mut().rev() {
            work.push(child as *mut StyledNode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{parse_css, Value, Unit, Color};
    use crate::dom::parse_html;

    /// Build a style tree from minimal HTML + CSS and return the root StyledNode.
    fn make_tree(html: &str, css: &str) -> StyledNode {
        let dom = parse_html(html);
        let stylesheet = parse_css(css);
        let js_overrides = HashMap::new();
        build_style_tree(&dom.document, &stylesheet, None, &js_overrides, None, None, None)
    }

    /// Walk the StyledNode tree depth-first to find the first node whose tag matches.
    fn find_node<'a>(root: &'a StyledNode, tag: &str) -> Option<&'a StyledNode> {
        if let markup5ever_rcdom::NodeData::Element { ref name, .. } = root.node.data {
            if name.local.as_ref() == tag { return Some(root); }
        }
        for child in &root.children {
            if let Some(found) = find_node(child, tag) { return Some(found); }
        }
        None
    }

    fn get_color(node: &StyledNode, prop: &str) -> Option<Color> {
        match node.specified_values.get(&intern(prop)) {
            Some(Value::Color(c)) => Some(c.clone()),
            _ => None,
        }
    }

    fn get_length_px(node: &StyledNode, prop: &str) -> Option<f32> {
        match node.specified_values.get(&intern(prop)) {
            Some(Value::Length(v, Unit::Px)) => Some(*v),
            _ => None,
        }
    }

    fn get_keyword(node: &StyledNode, prop: &str) -> Option<String> {
        match node.specified_values.get(&intern(prop)) {
            Some(Value::Keyword(k)) => Some(k.to_string()),
            _ => None,
        }
    }

    // --- inherit keyword ---

    #[test]
    fn test_inherit_color() {
        // The child explicitly sets color: inherit, so it should get the parent's color.
        let tree = make_tree(
            r#"<html><body><p style="color: red"><span style="color: inherit">text</span></p></body></html>"#,
            "",
        );
        let span = find_node(&tree, "span").expect("span not found");
        let c = get_color(span, "color").expect("color not found");
        assert_eq!(c, Color { r: 255, g: 0, b: 0, a: 255 });
    }

    #[test]
    fn test_inherit_on_root_falls_back_to_initial() {
        // On the root element there is no parent, so inherit should fall back to initial value.
        let tree = make_tree(
            r#"<html style="font-weight: inherit"></html>"#,
            "",
        );
        let html = find_node(&tree, "html").expect("html not found");
        // inherit on root → initial value for font-weight is "normal"
        let kw = get_keyword(html, "font-weight");
        assert!(kw.is_none() || kw.as_deref() == Some("normal"));
    }

    // --- initial keyword ---

    #[test]
    fn test_initial_resets_color() {
        // Even if CSS sets color to red on body, initial should give black.
        let tree = make_tree(
            r#"<html><body><p style="color: initial">text</p></body></html>"#,
            "body { color: red; }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let c = get_color(p, "color").expect("color not found");
        assert_eq!(c, Color { r: 0, g: 0, b: 0, a: 255 });
    }

    // --- em resolution on non-font-size properties ---

    #[test]
    fn test_em_resolves_against_own_font_size() {
        // margin-left: 2em on an element with font-size 20px → 40px
        let tree = make_tree(
            r#"<html><body><p>text</p></body></html>"#,
            "p { font-size: 20px; margin-left: 2em; }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let ml = get_length_px(p, "margin-left").expect("margin-left not found");
        assert!((ml - 40.0).abs() < 0.1, "expected 40px, got {}", ml);
    }

    #[test]
    fn test_em_font_size_resolves_against_parent() {
        // Child font-size: 2em, parent font-size: 10px → child should be 20px
        let tree = make_tree(
            r#"<html><body><p>text</p></body></html>"#,
            "body { font-size: 10px; } p { font-size: 2em; }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let fs = get_length_px(p, "font-size").expect("font-size not found");
        assert!((fs - 20.0).abs() < 0.1, "expected 20px, got {}", fs);
    }

    // --- currentColor ---

    #[test]
    fn test_currentcolor_border() {
        // border-color: currentColor should resolve to the element's own color.
        // Note: must use lowercase "currentcolor" because some CSS parsers normalize it.
        let tree = make_tree(
            r#"<html><body><p>text</p></body></html>"#,
            "p { color: rgb(10, 20, 30); border-color: currentcolor; }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let bc = get_color(p, "border-color").expect("border-color not found");
        assert_eq!(bc, Color { r: 10, g: 20, b: 30, a: 255 });
    }

    // --- var() resolution ---

    #[test]
    fn test_var_resolves_custom_property() {
        // Use html selector (instead of :root) to define custom property
        // and inherit it down to p via var()
        let tree = make_tree(
            r#"<html><body><p>text</p></body></html>"#,
            "html { --accent: #ff0000; } p { color: var(--accent); }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let c = get_color(p, "color").expect("color not found");
        assert_eq!(c, Color { r: 255, g: 0, b: 0, a: 255 });
    }

    #[test]
    fn test_var_resolves_custom_property_root() {
        // Use :root to define custom property (tests :root pseudo-class support)
        let tree = make_tree(
            r#"<html><body><p>text</p></body></html>"#,
            ":root { --accent: #ff0000; } p { color: var(--accent); }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let c = get_color(p, "color").expect("color not found");
        assert_eq!(c, Color { r: 255, g: 0, b: 0, a: 255 });
    }

    #[test]
    fn test_var_fallback_used_when_prop_missing() {
        let tree = make_tree(
            r#"<html><body><p>text</p></body></html>"#,
            "p { color: var(--missing, blue); }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let c = get_color(p, "color").expect("color not found");
        assert_eq!(c, Color { r: 0, g: 0, b: 255, a: 255 });
    }

    #[test]
    fn test_var_in_inline_style() {
        // Custom property defined in inline style and consumed via var() in CSS.
        let tree = make_tree(
            r#"<html><body><p style="--my-color: green; color: var(--my-color)">text</p></body></html>"#,
            "",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let c = get_color(p, "color").expect("color not found");
        assert_eq!(c, Color { r: 0, g: 128, b: 0, a: 255 });
    }

    #[test]
    fn test_var_cyclic_does_not_panic() {
        // Cyclic custom properties must not cause infinite recursion.
        // The cycle should be resolved to None (no crash, no value).
        let tree = make_tree(
            r#"<html><body><p>text</p></body></html>"#,
            "html { --a: var(--b); --b: var(--a); } p { color: var(--a, red); }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        // Should fall back to the fallback value since --a cycles
        let _c = get_color(p, "color"); // May be None or red — just must not panic
    }

    #[test]
    fn test_var_font_size_from_parent_div() {
        // `div { --size: 20px } span { font-size: var(--size) }` — the custom property
        // defined on div must be inherited by span via normal CSS inheritance.
        let tree = make_tree(
            r#"<html><body><div><span>text</span></div></body></html>"#,
            "div { --size: 20px; } span { font-size: var(--size); }",
        );
        let span = find_node(&tree, "span").expect("span not found");
        let fs = get_length_px(span, "font-size").expect("font-size not found");
        assert!((fs - 20.0).abs() < 0.1, "expected 20px, got {}", fs);
    }

    #[test]
    fn test_var_inherited_from_ancestor() {
        // Custom properties inherit through the element tree.
        // Grandparent defines --brand, grandchild uses it via var().
        let tree = make_tree(
            r#"<html><body><div><p><span>text</span></p></div></body></html>"#,
            "div { --brand: #1a73e8; } span { color: var(--brand); }",
        );
        let span = find_node(&tree, "span").expect("span not found");
        let c = get_color(span, "color").expect("color not found");
        assert_eq!(c, Color { r: 0x1a, g: 0x73, b: 0xe8, a: 255 });
    }

    // --- cascade ordering: !important ---

    #[test]
    fn test_important_author_overrides_normal() {
        // Normal author rule sets color red; !important rule sets it blue. Blue wins.
        let tree = make_tree(
            r#"<html><body><p class="a b">text</p></body></html>"#,
            ".a { color: red; } .b { color: blue !important; }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let c = get_color(p, "color").expect("color not found");
        assert_eq!(c, Color { r: 0, g: 0, b: 255, a: 255 });
    }

    #[test]
    fn test_inline_important_overrides_css_important() {
        // inline !important beats stylesheet !important
        let tree = make_tree(
            r#"<html><body><p style="color: green !important">text</p></body></html>"#,
            "p { color: red !important; }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        let c = get_color(p, "color").expect("color not found");
        assert_eq!(c, Color { r: 0, g: 128, b: 0, a: 255 });
    }

    #[test]
    fn test_inline_flex_shorthand_expands_to_longhands() {
        let mut decls = Vec::new();
        parse_inline_style_into_vec("flex: 1;", &mut decls);

        let grow = decls
            .iter()
            .find(|d| d.name.as_ref() == "flex-grow")
            .map(|d| d.value.clone());
        let shrink = decls
            .iter()
            .find(|d| d.name.as_ref() == "flex-shrink")
            .map(|d| d.value.clone());
        let basis = decls
            .iter()
            .find(|d| d.name.as_ref() == "flex-basis")
            .map(|d| d.value.clone());

        assert_eq!(grow, Some(Value::Number(1.0)));
        assert_eq!(shrink, Some(Value::Number(1.0)));
        assert_eq!(basis, Some(Value::Length(0.0, crate::css::Unit::Percent)));
    }

    // --- ::before / ::after pseudo-element injection ---

    /// Walk the styled tree and find the first child of `parent_tag` element that
    /// is a text node carrying the given text string.
    fn find_text_child(root: &StyledNode, parent_tag: &str, text: &str) -> bool {
        let node = match find_node(root, parent_tag) {
            Some(n) => n,
            None => return false,
        };
        for child in &node.children {
            if let markup5ever_rcdom::NodeData::Text { ref contents } = child.node.data {
                if contents.borrow().as_ref() == text { return true; }
            }
        }
        false
    }

    #[test]
    fn test_pseudo_before_content_injected() {
        // `p::before { content: ">> "; }` — should inject a synthetic ">> " text node
        // as the first child of every <p>.
        let tree = make_tree(
            r#"<html><body><p>hello</p></body></html>"#,
            r#"p::before { content: ">> "; }"#,
        );
        assert!(
            find_text_child(&tree, "p", ">> "),
            "::before content '>> ' should be the first child of <p>"
        );
    }

    #[test]
    fn test_pseudo_after_content_injected() {
        // `p::after { content: " <<"; }` — should inject as the last child of <p>.
        let tree = make_tree(
            r#"<html><body><p>hello</p></body></html>"#,
            r#"p::after { content: " <<"; }"#,
        );
        assert!(
            find_text_child(&tree, "p", " <<"),
            "::after content ' <<' should be a child of <p>"
        );
    }

    #[test]
    fn test_pseudo_before_content_none_suppressed() {
        // `content: none` must suppress the pseudo-element entirely.
        let tree = make_tree(
            r#"<html><body><p>hello</p></body></html>"#,
            r#"p::before { content: none; }"#,
        );
        let p = find_node(&tree, "p").expect("p not found");
        // The only child should be the "hello" text node, not a pseudo-element.
        let pseudo_injected = p.children.iter().any(|c| {
            matches!(&c.node.data, markup5ever_rcdom::NodeData::Text { ref contents }
                if contents.borrow().as_ref() != "hello")
        });
        assert!(!pseudo_injected, "content: none must suppress ::before injection");
    }

    #[test]
    fn test_pseudo_no_content_not_injected() {
        // A `::before` rule without a `content` declaration must not inject anything.
        let tree = make_tree(
            r#"<html><body><p>hello</p></body></html>"#,
            r#"p::before { color: red; }"#,
        );
        let p = find_node(&tree, "p").expect("p not found");
        let pseudo_count = p.children.iter().filter(|c| {
            matches!(&c.node.data, markup5ever_rcdom::NodeData::Text { ref contents }
                if contents.borrow().as_ref() != "hello")
        }).count();
        assert_eq!(pseudo_count, 0, "missing content declaration must suppress pseudo-element");
    }

    #[test]
    fn test_pseudo_before_inherits_parent_color() {
        // The synthetic ::before node should inherit color from the parent element.
        let tree = make_tree(
            r#"<html><body><p>hello</p></body></html>"#,
            r#"p { color: rgb(255, 0, 0); } p::before { content: "> "; }"#,
        );
        let p = find_node(&tree, "p").expect("p not found");
        let before = p.children.first().expect("::before child not found");
        let c = get_color(before, "color").expect("inherited color not found on ::before");
        assert_eq!(c, Color { r: 255, g: 0, b: 0, a: 255 });
    }

    #[test]
    fn test_pseudo_clearfix_after_empty_content() {
        // `::after { content: ""; display: block; clear: both; }` — clearfix pattern.
        let tree = make_tree(
            r#"<html><body><div class="cf">content</div></body></html>"#,
            r#".cf::after { content: ""; display: block; clear: both; }"#,
        );
        let div = find_node(&tree, "div").expect("div not found");
        let after = div.children.last().expect("::after child not found");
        let display = get_keyword(after, "display");
        assert_eq!(display.as_deref(), Some("block"),
            "clearfix ::after should have display: block");
        let clear = get_keyword(after, "clear");
        assert_eq!(clear.as_deref(), Some("both"),
            "clearfix ::after should have clear: both");
    }

    #[test]
    fn test_parse_selector_pseudo_element_before() {
        use crate::css::parse_selector;
        let s = parse_selector("p::before");
        assert_eq!(s.tag, Some("p".to_string()));
        assert_eq!(s.pseudo_element, Some("before".to_string()));
        assert!(s.pseudo_class.is_none());
    }

    #[test]
    fn test_parse_selector_pseudo_element_after() {
        use crate::css::parse_selector;
        let s = parse_selector(".clearfix::after");
        assert_eq!(s.class, vec!["clearfix".to_string()]);
        assert_eq!(s.pseudo_element, Some("after".to_string()));
        assert!(s.pseudo_class.is_none());
    }

    #[test]
    fn test_parse_selector_pseudo_class_unchanged() {
        use crate::css::parse_selector;
        let s = parse_selector("a:hover");
        assert_eq!(s.tag, Some("a".to_string()));
        assert_eq!(s.pseudo_class, Some("hover".to_string()));
        assert!(s.pseudo_element.is_none());
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
            "inset" => {
                // inset shorthand maps to top/right/bottom/left (same quad syntax).
                let parts: Vec<&str> = val.split_whitespace().collect();
                let (top, right, bottom, left) = match parts.len() {
                    1 => (parts[0], parts[0], parts[0], parts[0]),
                    2 => (parts[0], parts[1], parts[0], parts[1]),
                    3 => (parts[0], parts[1], parts[2], parts[1]),
                    4 => (parts[0], parts[1], parts[2], parts[3]),
                    _ => ("0", "0", "0", "0"),
                };
                use crate::css::parse_value;
                list.push(crate::css::Declaration { name: intern("top"),    value: parse_value(top),    important });
                list.push(crate::css::Declaration { name: intern("right"),  value: parse_value(right),  important });
                list.push(crate::css::Declaration { name: intern("bottom"), value: parse_value(bottom), important });
                list.push(crate::css::Declaration { name: intern("left"),   value: parse_value(left),   important });
            }
            "flex" => {
                let parts: Vec<&str> = val.split_whitespace().collect();
                match parts.len() {
                    0 => {}
                    1 => match parts[0] {
                        "none" => {
                            list.push(crate::css::Declaration { name: intern("flex-grow"), value: crate::css::Value::Number(0.0), important });
                            list.push(crate::css::Declaration { name: intern("flex-shrink"), value: crate::css::Value::Number(0.0), important });
                            list.push(crate::css::Declaration { name: intern("flex-basis"), value: crate::css::Value::Keyword(intern("auto")), important });
                        }
                        "auto" => {
                            list.push(crate::css::Declaration { name: intern("flex-grow"), value: crate::css::Value::Number(1.0), important });
                            list.push(crate::css::Declaration { name: intern("flex-shrink"), value: crate::css::Value::Number(1.0), important });
                            list.push(crate::css::Declaration { name: intern("flex-basis"), value: crate::css::Value::Keyword(intern("auto")), important });
                        }
                        _ => {
                            if let Ok(n) = parts[0].parse::<f32>() {
                                list.push(crate::css::Declaration { name: intern("flex-grow"), value: crate::css::Value::Number(n), important });
                                list.push(crate::css::Declaration { name: intern("flex-shrink"), value: crate::css::Value::Number(1.0), important });
                                list.push(crate::css::Declaration { name: intern("flex-basis"), value: crate::css::Value::Length(0.0, crate::css::Unit::Percent), important });
                            }
                        }
                    },
                    2 => {
                        if let (Ok(g), Ok(s)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>()) {
                            list.push(crate::css::Declaration { name: intern("flex-grow"), value: crate::css::Value::Number(g), important });
                            list.push(crate::css::Declaration { name: intern("flex-shrink"), value: crate::css::Value::Number(s), important });
                        } else if let Ok(g) = parts[0].parse::<f32>() {
                            list.push(crate::css::Declaration { name: intern("flex-grow"), value: crate::css::Value::Number(g), important });
                            list.push(crate::css::Declaration { name: intern("flex-shrink"), value: crate::css::Value::Number(1.0), important });
                            list.push(crate::css::Declaration {
                                name: intern("flex-basis"),
                                value: crate::css::parse_value(parts[1]),
                                important,
                            });
                        }
                    }
                    _ => {
                        if let (Ok(g), Ok(s)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>()) {
                            list.push(crate::css::Declaration { name: intern("flex-grow"), value: crate::css::Value::Number(g), important });
                            list.push(crate::css::Declaration { name: intern("flex-shrink"), value: crate::css::Value::Number(s), important });
                            list.push(crate::css::Declaration {
                                name: intern("flex-basis"),
                                value: crate::css::parse_value(parts[2]),
                                important,
                            });
                        }
                    }
                }
            }
            "gap" => {
                let parts: Vec<&str> = val.split_whitespace().collect();
                let row_val = parts
                    .first()
                    .map(|s| crate::css::parse_value(s))
                    .unwrap_or(crate::css::Value::Number(0.0));
                let col_val = parts
                    .get(1)
                    .map(|s| crate::css::parse_value(s))
                    .unwrap_or_else(|| row_val.clone());
                list.push(crate::css::Declaration { name: intern("row-gap"), value: row_val, important });
                list.push(crate::css::Declaration { name: intern("column-gap"), value: col_val, important });
            }
            // border-radius shorthand: "border-radius: <tl> [<tr> [<br> [<bl>]]]"
            // Use the first (top-left) value as a uniform radius.  The "/" elliptical syntax
            // is not supported; we take the text before any "/" as the horizontal radii list
            // and use the first token from that.
            "border-radius" => {
                let first = val.split_whitespace().next().unwrap_or("0");
                let first = first.split('/').next().unwrap_or("0").trim();
                let value = crate::css::parse_value(first);
                list.push(crate::css::Declaration { name: key, value, important });
            }
            "box-shadow" => {
                if let Some(shadow) = crate::css::parse_box_shadow(val) {
                    list.push(crate::css::Declaration {
                        name: key,
                        value: crate::css::Value::BoxShadow(shadow),
                        important,
                    });
                }
                // box-shadow: none → no declaration (no shadow rendered)
            }
            _ => {
                // CSS custom properties (--foo) in inline styles keep their raw string value.
                let value = if key.starts_with("--") {
                    crate::css::Value::RawCustomProp(crate::css::intern(val))
                } else {
                    parse_value(val)
                };
                list.push(crate::css::Declaration {
                    name: key,
                    value,
                    important,
                });
            }
        }
    }
}
