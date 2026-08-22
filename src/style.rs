use crate::css::{Stylesheet, Value, Selector, parse_value, parse_color, Combinator, intern, SelectorKey, PseudoClass};

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
                    // Mark as complex if the match depends on anything the
                    // `ElementSignature` cache does not capture, since a
                    // non-complex selector's verdict is reused for every element
                    // with the same tag, id and classes.
                    //
                    // That is an ancestor combinator, an attribute constraint —
                    // and any pseudo-class, which is the one that had been
                    // missed. `:first-child` is decided by where an element sits
                    // among its siblings, so caching the first `.row:first-child`
                    // verdict handed it to every `.row` on the page: yunseong's
                    // project list gave each of its five entries the first one's
                    // zero top padding and came out 128px short.
                    is_complex: sel.ancestor.is_some()
                        || !sel.attributes.is_empty()
                        || !sel.pseudo_classes.is_empty(),
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

/// Element siblings of `idx`, in document order, together with `idx`'s position
/// among them. Structural pseudo-classes count elements only, never text nodes.
fn element_siblings(idx: usize, arena: &[NodeDataSend]) -> (Vec<usize>, usize) {
    let Some(parent) = arena[idx].parent_idx else {
        return (vec![idx], 0);
    };
    let siblings: Vec<usize> = arena[parent]
        .children_idx
        .iter()
        .copied()
        .filter(|&c| arena[c].is_element)
        .collect();
    let position = siblings.iter().position(|&c| c == idx).unwrap_or(0);
    (siblings, position)
}

fn has_attr(node: &NodeDataSend, name: &str) -> bool {
    node.attrs.iter().any(|(k, _)| k == name)
}

fn matches_pseudo_class(
    pseudo: &PseudoClass,
    idx: usize,
    arena: &[NodeDataSend],
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
) -> bool {
    let node = &arena[idx];
    match pseudo {
        PseudoClass::Hover => node.id.is_some() && node.id.as_deref() == hovered_id,
        PseudoClass::Focus => node.id.is_some() && node.id.as_deref() == focused_id,
        PseudoClass::Root => node.tag == "html",
        PseudoClass::Not(inner) => !inner
            .iter()
            .any(|sel| matches_selector_arena(sel, idx, arena, hovered_id, focused_id)),
        PseudoClass::Is(inner) => inner
            .iter()
            .any(|sel| matches_selector_arena(sel, idx, arena, hovered_id, focused_id)),
        PseudoClass::Has(inner) => inner.iter().any(|(combinator, sel)| {
            has_relative_match(combinator, sel, idx, arena, hovered_id, focused_id)
        }),
        PseudoClass::FirstChild => element_siblings(idx, arena).1 == 0,
        PseudoClass::LastChild => {
            let (siblings, pos) = element_siblings(idx, arena);
            pos + 1 == siblings.len()
        }
        PseudoClass::OnlyChild => element_siblings(idx, arena).0.len() == 1,
        PseudoClass::FirstOfType | PseudoClass::LastOfType => {
            let (siblings, _) = element_siblings(idx, arena);
            let same_type: Vec<usize> = siblings
                .into_iter()
                .filter(|&c| arena[c].tag == node.tag)
                .collect();
            match pseudo {
                PseudoClass::FirstOfType => same_type.first() == Some(&idx),
                _ => same_type.last() == Some(&idx),
            }
        }
        PseudoClass::NthChild(a, b) => {
            // `:nth-child(an+b)` is 1-based, and n runs over the non-negative
            // integers, so a match needs (position - b) to be a non-negative
            // multiple of a. a == 0 degenerates to a single fixed position.
            let position = element_siblings(idx, arena).1 as i32 + 1;
            let offset = position - b;
            if *a == 0 {
                offset == 0
            } else {
                offset % a == 0 && offset / a >= 0
            }
        }
        PseudoClass::AnyLink => node.tag == "a" && has_attr(node, "href"),
        PseudoClass::Enabled => !has_attr(node, "disabled"),
        PseudoClass::Disabled => has_attr(node, "disabled"),
        PseudoClass::Checked => has_attr(node, "checked") || has_attr(node, "selected"),
        PseudoClass::Empty => arena[idx].children_idx.is_empty(),
        PseudoClass::Unsupported => false,
    }
}

/// The element side of `:has(...)`: does anything in `anchor`'s relative scope
/// match `sel`?
///
/// The scope is what the combinator names — the whole subtree for a descendant,
/// the element children for `>`, the next element sibling for `+`, every later
/// one for `~`. The inner selector is then matched against each candidate the
/// ordinary way, so `:has(.a .b)` reads its own ancestor chain against the
/// document rather than against the anchor's subtree; that is a superset of the
/// scoped rule and only differs for a selector whose ancestor part sits above
/// the anchor.
fn has_relative_match(
    combinator: &Combinator,
    sel: &Selector,
    anchor: usize,
    arena: &[NodeDataSend],
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
) -> bool {
    let hit = |c: usize| matches_selector_arena(sel, c, arena, hovered_id, focused_id);
    match combinator {
        Combinator::Descendant => {
            let mut stack: Vec<usize> = arena[anchor].children_idx.clone();
            while let Some(c) = stack.pop() {
                if arena[c].is_element && hit(c) {
                    return true;
                }
                stack.extend(arena[c].children_idx.iter().copied());
            }
            false
        }
        Combinator::Child => arena[anchor]
            .children_idx
            .iter()
            .any(|&c| arena[c].is_element && hit(c)),
        Combinator::NextSibling | Combinator::SubsequentSibling => {
            let (siblings, pos) = element_siblings(anchor, arena);
            let later = &siblings[(pos + 1).min(siblings.len())..];
            if matches!(combinator, Combinator::NextSibling) {
                later.first().is_some_and(|&c| hit(c))
            } else {
                later.iter().any(|&c| hit(c))
            }
        }
    }
}

/// `:has(...)` over the DOM handles, for the pass that decides which generated
/// boxes a page asks for. See `has_relative_match` for the scoping rule.
fn has_relative_match_handle(combinator: &Combinator, sel: &Selector, anchor: &Handle) -> bool {
    let is_element = |h: &Handle| matches!(h.data, NodeData::Element { .. });
    match combinator {
        Combinator::Descendant => {
            let mut stack: Vec<Handle> = anchor.children.borrow().iter().cloned().collect();
            while let Some(c) = stack.pop() {
                if is_element(&c) && selector_matches_element(sel, &c) {
                    return true;
                }
                stack.extend(c.children.borrow().iter().cloned());
            }
            false
        }
        Combinator::Child => anchor
            .children
            .borrow()
            .iter()
            .any(|c| is_element(c) && selector_matches_element(sel, c)),
        Combinator::NextSibling | Combinator::SubsequentSibling => {
            let Some(parent) = anchor.parent.take().and_then(|w| {
                let up = w.upgrade();
                anchor.parent.set(Some(w));
                up
            }) else {
                return false;
            };
            let children = parent.children.borrow();
            let siblings: Vec<&Handle> = children.iter().filter(|c| is_element(c)).collect();
            let anchor_ptr = std::rc::Rc::as_ptr(anchor);
            let Some(pos) = siblings
                .iter()
                .position(|c| std::rc::Rc::as_ptr(c) == anchor_ptr)
            else {
                return false;
            };
            let later = &siblings[(pos + 1).min(siblings.len())..];
            if matches!(combinator, Combinator::NextSibling) {
                later.first().is_some_and(|c| selector_matches_element(sel, c))
            } else {
                later.iter().any(|c| selector_matches_element(sel, c))
            }
        }
    }
}

fn matches_selector_arena(selector: &Selector, idx: usize, arena: &[NodeDataSend], hovered_id: Option<&str>, focused_id: Option<&str>) -> bool {
    // A `::before` / `::after` rule targets the generated box, never its subject.
    // Applying it to the element as well lets a decorative rule rewrite the real
    // element's style — `.foo::before { position: fixed }` was taking `.foo` out
    // of flow, collapsing its height and clipping away everything inside it.
    // The generated boxes are built separately by `inject_pseudo_elements`.
    if selector.pseudo_element.is_some() {
        return false;
    }
    let node = &arena[idx];
    
    let has_constraint = selector.tag.is_some() || selector.id.is_some() || !selector.class.is_empty() || !selector.attributes.is_empty() || !selector.pseudo_classes.is_empty();
    if !has_constraint { return false; }

    // `*` matches any element; treating it as a literal tag name meant a rule
    // like `* { box-sizing: border-box }` — which nearly every stylesheet opens
    // with — never applied, so padded boxes came out wider than their container.
    if let Some(ref s_tag) = selector.tag {
        if s_tag != "*" && &node.tag != s_tag { return false; }
    }
    if let Some(ref s_id) = selector.id {
        if node.id.as_deref() != Some(s_id) { return false; }
    }
    for s_class in &selector.class {
        if !node.classes.contains(s_class) { return false; }
    }
    for pseudo in &selector.pseudo_classes {
        if !matches_pseudo_class(pseudo, idx, arena, hovered_id, focused_id) {
            return false;
        }
    }
    for attr_sel in &selector.attributes {
        let mut matched = false;
        for (k, v) in &node.attrs {
            if k == &attr_sel.name && attr_sel.value.matches(v) {
                matched = true;
                break;
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
        // `<canvas>`, `<video>` and the embedded-content elements are replaced
        // too: their box comes from their `width`/`height` attributes, and the
        // spec's default for all of them is 300x150. A page whose hero is a
        // canvas loses that whole box otherwise.
        "canvas" | "video" | "iframe" | "embed" | "object" => {
            for (k, v) in &node.attrs {
                if matches!(k.as_str(), "width" | "height") {
                    if let Some(len) = parse_legacy_length_attr(v) {
                        map.entry(intern(k)).or_insert(len);
                    }
                }
            }
            map.entry(intern("width")).or_insert(Value::Length(300.0, crate::css::Unit::Px));
            map.entry(intern("height")).or_insert(Value::Length(150.0, crate::css::Unit::Px));
        }
        // An inline `<svg>` is a replaced element and takes up space whether or
        // not anything can draw it. Its size comes from its `width`/`height`
        // attributes, and its proportions from `viewBox`; with neither width nor
        // height stated the SVG spec's own defaults are `100%`, which is why a
        // logo with only a `viewBox` fills its container and takes a square of
        // height with it. Sizing it at nothing left a page hundreds of pixels
        // short of the browser's.
        "svg" => {
            let mut ratio = None;
            let mut attr_width = None;
            let mut attr_height = None;
            for (k, v) in &node.attrs {
                match k.as_str() {
                    "width" | "height" => {
                        if let Some(len) = parse_legacy_length_attr(v) {
                            if let Value::Length(px, crate::css::Unit::Px) = len {
                                if k == "width" {
                                    attr_width = Some(px);
                                } else {
                                    attr_height = Some(px);
                                }
                            }
                            map.entry(intern(k)).or_insert(len);
                        }
                    }
                    "viewbox" | "viewBox" => {
                        let nums: Vec<f32> = v
                            .split(|c: char| c == ',' || c.is_whitespace())
                            .filter(|p| !p.is_empty())
                            .filter_map(|p| p.parse::<f32>().ok())
                            .collect();
                        if let [_, _, w, h] = nums.as_slice() {
                            if *w > 0.0 && *h > 0.0 {
                                ratio = Some(*w / *h);
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Without a `viewBox` the width and height attributes are the
            // intrinsic size, and their ratio is what `height: auto` follows.
            // github's CTA ships `<svg width="2280" height="1200">` as a
            // spacer, and reading no ratio from it made the block 100px too
            // tall — and every section below it that much too low.
            if ratio.is_none() {
                if let (Some(w), Some(h)) = (attr_width, attr_height) {
                    if w > 0.0 && h > 0.0 {
                        ratio = Some(w / h);
                    }
                }
            }
            if let Some(r) = ratio {
                map.entry(intern("aspect-ratio")).or_insert(Value::Number(r));
            }
            // The spec's default for both is `100%`; the ratio then supplies
            // whichever one is not stated.
            if !map.contains_key(&intern("width")) && !map.contains_key(&intern("height")) {
                map.entry(intern("width"))
                    .or_insert(Value::Length(100.0, crate::css::Unit::Percent));
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
        // Layer order comes first — see the note on the full sort below — and
        // the rule index is the tie-break within a layer.
        matched.sort_by_key(|&(spec, ridx)| (all_rules[ridx].layer_rank, spec, ridx));
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

        // Cascade layer first, then specificity, then position in the sheet.
        //
        // Layer order *beats* specificity: an unlayered rule wins over a rule in
        // any layer however specific that one is. github states its component
        // rules inside `@layer primer-brand` and overrides them with plain
        // page-level classes, so ranking by specificity alone let
        // `.Pillar__description:not(.Pillar--has-border …)` — four classes deep
        // — keep its 24px margin over the unlayered
        // `.lp-SectionTemplate-customer-description`'s 16px, and every pillar
        // came out 8px too tall.
        //
        // The rule index is the tie-break the cascade actually specifies, and
        // without it equal-specificity rules were applied in whatever order they
        // came out of the signature cache and the selector index. That order is
        // not stable between runs, so the same page laid out differently from one
        // render to the next — and equal specificity is the common case in a
        // design system, where nearly every rule is a single class.
        rule_matches.sort_by_key(|&(rule_idx, spec)| (all_rules[rule_idx].layer_rank, spec, rule_idx));

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

/// The computed `font-size` of the `<html>` element, if the tree has one.
fn find_root_font_size(node: &StyledNode) -> Option<f32> {
    if let NodeData::Element { ref name, .. } = node.node.data {
        if name.local.as_ref() == "html" {
            return match node.specified_values.get(&intern("font-size")) {
                Some(Value::Length(v, crate::css::Unit::Px)) => Some(*v),
                _ => None,
            };
        }
    }
    node.children.iter().find_map(find_root_font_size)
}

/// Fold `calc()` / `clamp()` / `min()` / `max()` values in a computed style tree
/// down to pixels.
///
/// This runs after style computation, where `em` is already resolved and the
/// viewport is known, but before layout. Expressions containing a percentage are
/// left in place: their basis is the containing block, which only layout knows,
/// so they are resolved there instead of being guessed at here.
pub fn resolve_math_values(node: &mut StyledNode, viewport_width: f32, viewport_height: f32) {
    // Computed maps are shared and deduplicated between nodes, so an already
    // resolved map is reused rather than rebuilt for every node that points at it.
    // `rem` inside a math function resolves against the root element, not the
    // element the expression sits on.
    let root_font_size = find_root_font_size(node).unwrap_or(16.0);

    let mut resolved: HashMap<usize, PropertyMap> = HashMap::new();
    let mut stack: Vec<*mut StyledNode> = vec![node as *mut StyledNode];

    // SAFETY: each pointer comes from a live &mut and is visited exactly once,
    // so no two mutable references to the same node exist at the same time.
    while let Some(ptr) = stack.pop() {
        let node = unsafe { &mut *ptr };
        let key = Arc::as_ptr(&node.specified_values.0) as usize;

        if let Some(cached) = resolved.get(&key) {
            node.specified_values = cached.clone();
        } else if node.specified_values.values().any(|v| matches!(v, Value::Math(_))) {
            let font_size = match node.specified_values.get(&intern("font-size")) {
                Some(Value::Length(v, crate::css::Unit::Px)) => *v,
                _ => 16.0,
            };
            let ctx = crate::css::MathContext {
                viewport_width,
                viewport_height,
                font_size,
                font_style: crate::layout::resolved_font_style(node),
                root_font_size,
                percent_basis: None,
            };
            let mut map = (*node.specified_values.0).clone();
            for value in map.values_mut() {
                let Value::Math(expr) = value else { continue };
                // A percentage needs the containing block, which only layout
                // knows; those expressions stay put and are resolved there.
                if expr.needs_percent_basis() {
                    continue;
                }
                if let Some(px) = expr.resolve(&ctx) {
                    *value = Value::Length(px, crate::css::Unit::Px);
                }
            }
            let new_map = PropertyMap(Arc::new(map));
            resolved.insert(key, new_map.clone());
            node.specified_values = new_map;
        }

        for child in node.children.iter_mut() {
            stack.push(child as *mut StyledNode);
        }
    }
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
    // `rem` resolves against the root element's font size. The traversal is
    // depth-first from the root, so this is set before any descendant reads it.
    let mut root_fs: f32 = 16.0;

    while let Some(frame) = work.pop() {
        match frame {
            Frame::Pre { handle, parent_pm } => {
                // Read the current sequential index BEFORE incrementing.
                let current_idx = *arena_idx;
                *arena_idx += 1;

                let is_root_element = matches!(
                    handle.data,
                    NodeData::Element { ref name, .. } if name.local.as_ref() == "html"
                );

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
                        // `visibility` inherits, which is what lets a subtree be
                        // hidden by setting it once on an ancestor — the form
                        // sites actually use.
                        "visibility",
                        "text-indent",
                        "letter-spacing",
                        "word-spacing",
                        "text-transform",
                        "word-break",
                        "overflow-wrap",
                        "direction",
                    ];
                    for prop in inheritable {
                        let prop_arc = intern(prop);
                        if let Some(v) = p.get(&prop_arc) {
                            specified_values.entry(prop_arc).or_insert_with(|| v.clone());
                        }
                    }

                    // `text-decoration` does not inherit, but the line an
                    // ancestor draws runs under its in-flow descendants — and
                    // since the text node carrying the glyphs is where the line
                    // is actually drawn, propagating it is what puts it there.
                    // Nothing did, so no link on any page was underlined.
                    //
                    // It stops at an atomic inline, which starts a decoration
                    // context of its own: a button inside a link is not
                    // underlined.
                    let decoration = intern("text-decoration");
                    let starts_own_context = matches!(
                        specified_values.get(&intern("display")),
                        Some(Value::Keyword(k))
                            if matches!(
                                &**k,
                                "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "table"
                            )
                    );
                    if !starts_own_context {
                        if let Some(v) = p.get(&decoration) {
                            specified_values.entry(decoration).or_insert_with(|| v.clone());
                        }
                    }
                }

                // --- Step 3: Resolve font-size em/% first (needs parent font-size) ---
                let fs_key = intern("font-size");
                let parent_fs = parent_pm.as_ref()
                    .and_then(|p| p.get(&fs_key))
                    .and_then(|v| if let Value::Length(pv, crate::css::Unit::Px) = v { Some(*pv) } else { None })
                    .unwrap_or(16.0);
                // A `font-size` in `ch` or `ex` is measured in the parent's
                // font, not this element's, which is what is being computed.
                let parent_font = {
                    let family = match parent_pm
                        .as_ref()
                        .and_then(|p| p.get(&intern("font-family")))
                    {
                        Some(Value::Keyword(k)) => crate::layout::generic_family_for(k),
                        Some(Value::RawCustomProp(raw)) => crate::layout::generic_family_for(raw),
                        _ => crate::font::GenericFamily::default(),
                    };
                    crate::font::FontStyle { family, ..crate::font::FontStyle::regular() }
                };
                // A font size may be written in any of these units, and may arrive
                // through a custom property — design systems keep their type scale
                // in one. Resolving a var() only when it already held pixels left
                // every `font-size: var(--scale-step)` unresolved, so headings
                // silently inherited body size.
                fn font_size_px(
                    value: &Value,
                    parent_fs: f32,
                    root_fs: f32,
                    parent_font: crate::font::FontStyle,
                ) -> Option<f32> {
                    match value {
                        Value::Length(v, crate::css::Unit::Px) => Some(*v),
                        Value::Length(v, crate::css::Unit::Percent) => Some(parent_fs * (v / 100.0)),
                        Value::Length(v, crate::css::Unit::Em) => Some(parent_fs * v),
                        Value::Length(v, crate::css::Unit::Rem) => Some(root_fs * v),
                        // A `font-size` in `ch` or `ex` is measured in the
                        // *parent's* font, since this element's is what is
                        // being computed.
                        Value::Length(v, crate::css::Unit::Ch) => {
                            Some(crate::font::fonts().zero_advance(parent_fs, parent_font) * v)
                        }
                        Value::Length(v, crate::css::Unit::Ex) => {
                            Some(crate::font::fonts().x_height(parent_fs, parent_font) * v)
                        }
                        Value::Keyword(kw) if kw.as_ref() == "inherit" => Some(parent_fs),
                        Value::Keyword(kw) if kw.as_ref() == "initial" => Some(16.0),
                        _ => None,
                    }
                }
                if let Some(val) = specified_values.get(&fs_key) {
                    let resolved_fs = match val {
                        Value::CssVar { .. } => resolve_var(val, &custom_props, 0)
                            .as_ref()
                            .and_then(|resolved| font_size_px(resolved, parent_fs, root_fs, parent_font)),
                        other => font_size_px(other, parent_fs, root_fs, parent_font),
                    };
                    if let Some(fs) = resolved_fs {
                        specified_values.insert(fs_key.clone(), Value::Length(fs, crate::css::Unit::Px));
                    }
                }
                let own_fs = specified_values.get(&fs_key)
                    .and_then(|v| if let Value::Length(pv, crate::css::Unit::Px) = v { Some(*pv) } else { None })
                    .unwrap_or(parent_fs);
                // `ch` and `ex` are metrics of the element's *own* face, so the
                // stack has to be resolved before either unit can be. A design
                // that caps its prose at `46ch` gets a wider measure under
                // `system-ui` than under `sans-serif`.
                let own_font = {
                    let ff_key = intern("font-family");
                    let stated = specified_values
                        .get(&ff_key)
                        .cloned()
                        .or_else(|| parent_pm.as_ref().and_then(|p| p.get(&ff_key)).cloned());
                    let stack = match stated.as_ref() {
                        Some(v @ Value::CssVar { .. }) => resolve_var(v, &custom_props, 0),
                        other => other.cloned(),
                    };
                    let family = match stack.as_ref() {
                        Some(Value::Keyword(k)) => crate::layout::generic_family_for(k),
                        Some(Value::RawCustomProp(raw)) => crate::layout::generic_family_for(raw),
                        _ => crate::font::GenericFamily::default(),
                    };
                    crate::font::FontStyle { family, ..crate::font::FontStyle::regular() }
                };
                // The root element is visited before anything that can reference
                // it, so its font size is known by the time a `rem` needs it.
                if is_root_element {
                    root_fs = own_fs;
                }

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
                                    Value::Length(n, crate::css::Unit::Rem) => Value::Length(n * root_fs, crate::css::Unit::Px),
                                    Value::Length(n, crate::css::Unit::Ch) => {
                                        Value::Length(n * crate::font::fonts().zero_advance(own_fs, own_font), crate::css::Unit::Px)
                                    }
                                    Value::Length(n, crate::css::Unit::Ex) => {
                                        Value::Length(n * crate::font::fonts().x_height(own_fs, own_font), crate::css::Unit::Px)
                                    }
                                    // A custom property may itself hold a math
                                    // expression naming further properties —
                                    // `--half: calc(var(--gutter) / 2)` — so the
                                    // substitution has to run on the result too,
                                    // or the expression is left unresolvable.
                                    // Font-relative operands fold here for the
                                    // same reason as below: only the style pass
                                    // knows both this element's font size and
                                    // the root's, and a `rem` inside a custom
                                    // property's `calc()` is how a design system
                                    // writes its gutters.
                                    Value::Math(expr) => Value::Math(expr.substitute_vars(&|name| {
                                        match custom_props.get(&intern(name)) {
                                            Some(Value::RawCustomProp(raw)) => Some(raw.to_string()),
                                            Some(Value::Length(v, crate::css::Unit::Px)) => Some(format!("{v}px")),
                                            Some(Value::Number(v)) => Some(v.to_string()),
                                            _ => None,
                                        }
                                    }).fold_font_relative(own_fs, root_fs, own_font)),
                                    Value::Keyword(kw) if kw.as_ref().eq_ignore_ascii_case("currentcolor") => {
                                        own_color.clone().unwrap_or(v.clone())
                                    }
                                    _ => v,
                                }
                            })
                        }
                        // A math expression may name custom properties —
                        // `calc(var(--gutter) * .5)` is how design systems derive
                        // one spacing step from another — so substitute them while
                        // this element's properties are in hand. The font-relative
                        // operands are folded here too, for the same reason: this
                        // is the only place that knows both this element's font
                        // size and the root's. Percentages and viewport units wait
                        // for layout, which is where the containing block is known.
                        Value::Math(expr) => {
                            let substituted = expr.substitute_vars(&|name| {
                                match custom_props.get(&intern(name)) {
                                    Some(Value::RawCustomProp(raw)) => Some(raw.to_string()),
                                    Some(Value::Length(v, crate::css::Unit::Px)) => Some(format!("{v}px")),
                                    Some(Value::Number(v)) => Some(v.to_string()),
                                    _ => None,
                                }
                            });
                            Some(Value::Math(substituted.fold_font_relative(
                                own_fs, root_fs, own_font,
                            )))
                        }
                        // `em` resolves against this element's own font size; `rem`
                        // against the root's, which is what keeps a design system's
                        // spacing scale from compounding inside nested text.
                        Value::Length(n, crate::css::Unit::Em) => {
                            Some(Value::Length(n * own_fs, crate::css::Unit::Px))
                        }
                        Value::Length(n, crate::css::Unit::Rem) => {
                            Some(Value::Length(n * root_fs, crate::css::Unit::Px))
                        }
                        // `ch` and `ex` come from the font's own metrics. `max-width`
                        // in `ch` is how a design caps its measure, so ignoring the
                        // unit lets prose run the full width of its container.
                        Value::Length(n, crate::css::Unit::Ch) => Some(Value::Length(
                            n * crate::font::fonts().zero_advance(own_fs, own_font),
                            crate::css::Unit::Px,
                        )),
                        Value::Length(n, crate::css::Unit::Ex) => Some(Value::Length(
                            n * crate::font::fonts().x_height(own_fs, own_font),
                            crate::css::Unit::Px,
                        )),
                        _ => None,
                    };
                    if let Some(r) = resolved {
                        specified_values.insert(key, r);
                    }
                }

                promote_resolved_background(&mut specified_values);

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

/// Font size and block margins of a heading, in `em`, per the HTML spec's
/// suggested rendering (§15.3.6). They are `em` rather than pixels on purpose:
/// a heading inside a container with its own font size scales with it, and a
/// page that sets `html { font-size }` expects the whole scale to follow.
fn heading_metrics(tag: &str) -> Option<(f32, f32)> {
    match tag {
        "h1" => Some((2.0, 0.67)),
        "h2" => Some((1.5, 0.83)),
        "h3" => Some((1.17, 1.0)),
        "h4" => Some((1.0, 1.33)),
        "h5" => Some((0.83, 1.67)),
        "h6" => Some((0.67, 2.33)),
        _ => None,
    }
}

fn apply_default_styles(tag: &str, map: &mut HashMap<Arc<str>, Value>) {
    // Block margins in the UA sheet are all `em`, so they track the element's
    // own font size. Stating them in pixels — as this did — pinned a paragraph's
    // rhythm to a 16px body no matter what the page actually set, and the error
    // compounded down a page built from stacked prose blocks.
    if let Some((size_em, margin_em)) = heading_metrics(tag) {
        map.entry(intern("font-size")).or_insert(Value::Length(size_em, crate::css::Unit::Em));
        map.entry(intern("font-weight")).or_insert(Value::Keyword(intern("bold")));
        // The margin is `em` of the heading's *own* size, which the size above
        // has just set, so it resolves against the scaled value.
        map.entry(intern("margin-top")).or_insert(Value::Length(margin_em, crate::css::Unit::Em));
        map.entry(intern("margin-bottom")).or_insert(Value::Length(margin_em, crate::css::Unit::Em));
    }
    match tag {
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
            if tag == "pre" {
                map.entry(intern("margin-top")).or_insert(Value::Length(1.0, crate::css::Unit::Em));
                map.entry(intern("margin-bottom")).or_insert(Value::Length(1.0, crate::css::Unit::Em));
            }
        }
        // Form controls do not inherit the page's font: the UA sheet gives them
        // one of their own, which is why a button inside `body { line-height: 1.4 }`
        // is not 1.4 lines tall. Inheriting it made every control several pixels
        // taller than a browser draws it, once per control down the page.
        "input" => {
            map.entry(intern("line-height")).or_insert(Value::Keyword(intern("normal")));
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            map.entry(intern("padding")).or_insert(Value::Length(4.0, crate::css::Unit::Px));
            // UA defaults: HTML spec §14.3 — <input> default size = 20 chars ≈ 160 px at 13 px font.
            map.entry(intern("width")).or_insert(Value::Length(160.0, crate::css::Unit::Px));
            // The height is content-driven, as it is in a browser: one line of
            // the control's own font plus its padding and border. Pinning it to
            // a fixed 24px made a padded field several pixels taller than the
            // browser draws it, and the field is the tallest thing in its row.
            map.entry(intern("font-size")).or_insert(Value::Length(13.3333, crate::css::Unit::Px));
        }
        "textarea" => {
            map.entry(intern("line-height")).or_insert(Value::Keyword(intern("normal")));
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            map.entry(intern("padding")).or_insert(Value::Length(4.0, crate::css::Unit::Px));
            // UA defaults: cols=20, rows=2 → ~160 × 48 px.
            map.entry(intern("width")).or_insert(Value::Length(160.0, crate::css::Unit::Px));
            map.entry(intern("height")).or_insert(Value::Length(48.0, crate::css::Unit::Px));
        }
        "select" => {
            map.entry(intern("line-height")).or_insert(Value::Keyword(intern("normal")));
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 180, g: 180, b: 180, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            map.entry(intern("padding")).or_insert(Value::Length(4.0, crate::css::Unit::Px));
            map.entry(intern("width")).or_insert(Value::Length(120.0, crate::css::Unit::Px));
            map.entry(intern("font-size")).or_insert(Value::Length(13.3333, crate::css::Unit::Px));
        }
        // The reference renderer's own defaults, measured rather than guessed:
        // `padding: 1px 6px`, a 2px `outset` bevel, `#efefef`, 13.3333px and a
        // centred label. A button styled by the page overrides all of it, but an
        // unstyled one has to be the size the browser makes it or the row it
        // sits in is the wrong height and its label wraps where it should not.
        "button" => {
            map.entry(intern("line-height")).or_insert(Value::Keyword(intern("normal")));
            map.entry(intern("font-size")).or_insert(Value::Length(13.3333, crate::css::Unit::Px));
            // Chromium computes 2px here, but it draws an `outset` bevel from
            // two greys rather than a flat edge; a 1px edge measures closer to
            // that than a 2px one does.
            map.entry(intern("border-width")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("border-color")).or_insert(Value::Color(crate::css::Color { r: 190, g: 190, b: 190, a: 255 }));
            map.entry(intern("background-color")).or_insert(Value::Color(crate::css::Color { r: 239, g: 239, b: 239, a: 255 }));
            map.entry(intern("padding-top")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("padding-bottom")).or_insert(Value::Length(1.0, crate::css::Unit::Px));
            map.entry(intern("padding-left")).or_insert(Value::Length(6.0, crate::css::Unit::Px));
            map.entry(intern("padding-right")).or_insert(Value::Length(6.0, crate::css::Unit::Px));
            map.entry(intern("text-align")).or_insert(Value::Keyword(intern("center")));
            // A form control does not inherit the page's text colour: the UA
            // sheet gives it `ButtonText`, which is what keeps a default button
            // readable on its own grey chrome. Letting it inherit painted a
            // dark page's light text onto that grey and the label vanished.
            map.entry(intern("color"))
                .or_insert(Value::Color(crate::css::Color { r: 0, g: 0, b: 0, a: 255 }));
        }
        // <center> is a legacy presentational element — UA default maps it to a block
        // with text-align: center, matching browsers' built-in stylesheet.
        "center" => {
            map.entry(intern("display")).or_insert(Value::Keyword(intern("block")));
            map.entry(intern("text-align")).or_insert(Value::Keyword(intern("center")));
        }
        "ul" | "ol" => {
            map.entry(intern("margin-top")).or_insert(Value::Length(1.0, crate::css::Unit::Em));
            map.entry(intern("margin-bottom")).or_insert(Value::Length(1.0, crate::css::Unit::Em));
            map.entry(intern("padding-left")).or_insert(Value::Length(40.0, crate::css::Unit::Px));
            map.entry(intern("list-style-type")).or_insert(Value::Keyword(intern(
                if tag == "ul" { "disc" } else { "decimal" },
            )));
        }
        "p" | "dl" => {
            map.entry(intern("margin-top")).or_insert(Value::Length(1.0, crate::css::Unit::Em));
            map.entry(intern("margin-bottom")).or_insert(Value::Length(1.0, crate::css::Unit::Em));
        }
        // `margin: 1em 40px` — the side margins are what makes a blockquote read
        // as a quote at all, so they are as load-bearing as the block ones.
        "blockquote" | "figure" => {
            map.entry(intern("margin-top")).or_insert(Value::Length(1.0, crate::css::Unit::Em));
            map.entry(intern("margin-bottom")).or_insert(Value::Length(1.0, crate::css::Unit::Em));
            map.entry(intern("margin-left")).or_insert(Value::Length(40.0, crate::css::Unit::Px));
            map.entry(intern("margin-right")).or_insert(Value::Length(40.0, crate::css::Unit::Px));
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

/// Tag, id and classes of a DOM handle, or `None` if it is not an element.
fn element_info(handle: &Handle) -> Option<(String, Option<String>, Vec<String>)> {
    let NodeData::Element { ref name, ref attrs, .. } = handle.data else {
        return None;
    };
    let mut id = None;
    let mut classes = Vec::new();
    for attr in attrs.borrow().iter() {
        match attr.name.local.as_ref() {
            "id" => id = Some(attr.value.to_string()),
            "class" => classes = attr.value.split_whitespace().map(str::to_string).collect(),
            _ => {}
        }
    }
    Some((name.local.to_string(), id, classes))
}

fn parent_element(handle: &Handle) -> Option<Handle> {
    let weak = handle.parent.take();
    let parent = weak.as_ref().and_then(|w| w.upgrade());
    handle.parent.set(weak);
    parent.filter(|p| matches!(p.data, NodeData::Element { .. }))
}

/// Element siblings of `handle` in document order, and its index among them.
fn element_sibling_position(handle: &Handle) -> Option<(usize, usize)> {
    let parent = parent_element(handle)?;
    let children = parent.children.borrow();
    let elements: Vec<&Handle> = children
        .iter()
        .filter(|c| matches!(c.data, NodeData::Element { .. }))
        .collect();
    let index = elements.iter().position(|c| std::rc::Rc::ptr_eq(c, handle))?;
    Some((index, elements.len()))
}

/// Match one compound selector (no combinators) against a DOM element.
fn compound_matches_element(sel: &Selector, handle: &Handle) -> bool {
    let Some((tag, id, classes)) = element_info(handle) else {
        return false;
    };
    if sel.tag.as_ref().is_some_and(|t| t != "*" && *t != tag) {
        return false;
    }
    if sel.id.as_ref().is_some_and(|i| id.as_deref() != Some(i)) {
        return false;
    }
    if sel.class.iter().any(|c| !classes.contains(c)) {
        return false;
    }
    for attr_sel in &sel.attributes {
        let NodeData::Element { ref attrs, .. } = handle.data else {
            return false;
        };
        let matched = attrs.borrow().iter().any(|a| {
            a.name.local.as_ref() == attr_sel.name && attr_sel.value.matches(a.value.as_ref())
        });
        if !matched {
            return false;
        }
    }
    for pseudo in &sel.pseudo_classes {
        let ok = match pseudo {
            PseudoClass::Not(inner) => !inner.iter().any(|s| selector_matches_element(s, handle)),
            PseudoClass::Is(inner) => inner.iter().any(|s| selector_matches_element(s, handle)),
            PseudoClass::Has(inner) => inner
                .iter()
                .any(|(combinator, s)| has_relative_match_handle(combinator, s, handle)),
            PseudoClass::Root => tag == "html",
            PseudoClass::FirstChild => element_sibling_position(handle).is_some_and(|(i, _)| i == 0),
            PseudoClass::LastChild => element_sibling_position(handle).is_some_and(|(i, n)| i + 1 == n),
            PseudoClass::OnlyChild => element_sibling_position(handle).is_some_and(|(_, n)| n == 1),
            PseudoClass::NthChild(a, b) => element_sibling_position(handle).is_some_and(|(i, _)| {
                let offset = i as i32 + 1 - b;
                if *a == 0 { offset == 0 } else { offset % a == 0 && offset / a >= 0 }
            }),
            // Interactive and unimplemented states do not apply to a freshly
            // rendered page, so their rules must not be injected.
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    // Require at least one constraint to avoid matching everything with `*::before`.
    sel.tag.is_some()
        || sel.id.is_some()
        || !sel.class.is_empty()
        || !sel.attributes.is_empty()
        || !sel.pseudo_classes.is_empty()
}

/// Check whether a CSS selector (ignoring its `pseudo_element` field) matches a
/// DOM element, ancestor combinators included.
///
/// Walking the combinators matters as much here as it does for ordinary rules:
/// matching only the rightmost compound turns `.markdown-body sup>a::before {
/// content: "[" }` into a rule that brackets every link on the page.
fn selector_matches_element(sel: &Selector, handle: &Handle) -> bool {
    if !compound_matches_element(sel, handle) {
        return false;
    }
    let Some(ancestor_sel) = sel.ancestor.as_ref() else {
        return true;
    };
    match sel.combinator.as_ref().unwrap_or(&Combinator::Descendant) {
        Combinator::Descendant => {
            let mut current = parent_element(handle);
            while let Some(node) = current {
                if selector_matches_element(ancestor_sel, &node) {
                    return true;
                }
                current = parent_element(&node);
            }
            false
        }
        Combinator::Child => parent_element(handle)
            .is_some_and(|p| selector_matches_element(ancestor_sel, &p)),
        Combinator::NextSibling | Combinator::SubsequentSibling => {
            let Some(parent) = parent_element(handle) else {
                return false;
            };
            let children = parent.children.borrow();
            let elements: Vec<&Handle> = children
                .iter()
                .filter(|c| matches!(c.data, NodeData::Element { .. }))
                .collect();
            let Some(index) = elements.iter().position(|c| std::rc::Rc::ptr_eq(c, handle)) else {
                return false;
            };
            match sel.combinator.as_ref().unwrap_or(&Combinator::Descendant) {
                Combinator::NextSibling => index
                    .checked_sub(1)
                    .is_some_and(|i| selector_matches_element(ancestor_sel, elements[i])),
                _ => elements[..index]
                    .iter()
                    .any(|c| selector_matches_element(ancestor_sel, c)),
            }
        }
    }
}

fn selector_base_matches_element(sel: &Selector, node: &StyledNode) -> bool {
    selector_matches_element(sel, &node.node)
}

/// Build a synthetic `StyledNode` that acts as a pseudo-element.
///
/// `content_text` — the text to inject (may be empty for block-level decorators).
/// `pseudo_decls` — the CSS declarations from the matching rule.
/// `parent_values` — the parent element's computed style, used to inherit properties.
/// Resolve the escapes in a CSS string.
///
/// An icon set writes its glyph as `content: "\f52a"` — a codepoint, not the
/// four characters that spell it. Keeping the escape verbatim drew "f52a" next
/// to every such icon, where a browser draws the glyph or, when the icon font
/// never loaded, nothing at all.
fn unescape_css_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(&next) = chars.peek() else { break };
        if !next.is_ascii_hexdigit() {
            // `\"` and friends stand for the character itself; an escaped
            // newline stands for nothing.
            chars.next();
            if next != '\n' {
                out.push(next);
            }
            continue;
        }
        // Up to six hex digits, ended by a single optional space.
        let mut hex = String::new();
        while hex.len() < 6 {
            match chars.peek() {
                Some(&h) if h.is_ascii_hexdigit() => {
                    hex.push(h);
                    chars.next();
                }
                _ => break,
            }
        }
        if chars.peek() == Some(&' ') {
            chars.next();
        }
        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            Some(ch) => out.push(ch),
            // An unpaired surrogate or an out-of-range value is the replacement
            // character, per css-syntax-3.
            None => out.push('\u{fffd}'),
        }
    }
    out
}

/// Put a `background` shorthand that has only just resolved into the slot paint
/// reads.
///
/// `background: var(--x)` leaves the shorthand holding the reference and the
/// colour slot holding the reset the shorthand emits, because the substitution
/// happens long after the shorthand was split. A colour slot that something
/// else filled is left alone: the reset is transparent, so anything opaque
/// there came from a later declaration.
fn promote_resolved_background(map: &mut HashMap<Arc<str>, Value>) {
    let colour_slot_is_reset = matches!(
        map.get(&intern("background-color")),
        None | Some(Value::Color(crate::css::Color { a: 0, .. }))
    );
    if !colour_slot_is_reset {
        return;
    }
    match map.get(&intern("background")).cloned() {
        Some(v @ Value::Color(_)) => {
            map.insert(intern("background-color"), v);
        }
        Some(v @ Value::Gradient(_)) => {
            map.entry(intern("background-image")).or_insert(v);
        }
        _ => {}
    }
}

fn make_pseudo_styled_node(
    content_text: String,
    pseudo_decls: &[crate::css::Declaration],
    parent_values: &PropertyMap,
) -> StyledNode {
    use html5ever::tendril::StrTendril;
    use html5ever::{ns, LocalName, QualName};
    use markup5ever_rcdom::Node;

    // A generated box is an element, not a text run: `::before { content: "";
    // display: block; width: 100%; height: 100% }` is how a page draws a circle
    // behind an icon or a wash behind a card, and none of that survives being
    // modelled as a text node — layout sizes a text node by its text, so an
    // empty `content` came out nothing at all and a stated width and height were
    // never read. The content, when there is any, becomes a text child of it.
    let element_handle = Node::new(NodeData::Element {
        name: QualName::new(None, ns!(html), LocalName::from("span")),
        attrs: std::cell::RefCell::new(vec![]),
        template_contents: std::cell::RefCell::new(None),
        mathml_annotation_xml_integration_point: false,
    });

    // Build the property map: start with inheritable properties from parent, then
    // apply the pseudo-element's own declarations on top.
    let mut map: HashMap<Arc<str>, Value> = HashMap::new();

    // Inherit a small set of properties from the parent element.
    let inheritable = [
        "color", "font-size", "font-family", "font-weight",
        "font-style", "line-height", "text-align", "white-space",
        "visibility", "text-indent", "letter-spacing", "word-spacing",
        "text-transform", "word-break", "overflow-wrap", "direction",
    ];
    for prop in &inheritable {
        let key = intern(prop);
        if let Some(v) = parent_values.get(&key) {
            map.insert(key, v.clone());
        }
    }

    // Custom properties inherit too, and a generated box is where a design
    // system reaches for them — `::before { background: var(--bgColor) }` is how
    // github draws the disc behind its play button. Without them the `var()`
    // below resolves to nothing and the box paints no fill.
    let mut custom_props: HashMap<Arc<str>, Value> = HashMap::new();
    for (key, value) in parent_values.0.iter() {
        if key.starts_with("--") {
            custom_props.insert(key.clone(), value.clone());
            map.insert(key.clone(), value.clone());
        }
    }
    for decl in pseudo_decls {
        if decl.name.starts_with("--") {
            custom_props.insert(decl.name.clone(), decl.value.clone());
        }
    }

    // Apply pseudo-element declarations (skip `content` itself).
    for decl in pseudo_decls {
        if decl.name.as_ref() == "content" { continue; }
        let value = match &decl.value {
            v @ Value::CssVar { .. } => resolve_var(v, &custom_props, 0).unwrap_or_else(|| v.clone()),
            // `border-radius: inherit` is how a generated box takes the shape of
            // the box it decorates, and it is the element it hangs off that it
            // inherits from.
            Value::Keyword(k) if k.as_ref() == "inherit" => {
                match parent_values.get(&decl.name) {
                    Some(v) => v.clone(),
                    None => continue,
                }
            }
            v => v.clone(),
        };
        map.insert(decl.name.clone(), value);
    }

    promote_resolved_background(&mut map);
    let values = PropertyMap(Arc::new(map));
    let children = if content_text.is_empty() {
        Vec::new()
    } else {
        let text_handle = Node::new(NodeData::Text {
            contents: std::cell::RefCell::new(StrTendril::from(content_text.as_str())),
        });
        vec![StyledNode {
            node: text_handle,
            specified_values: values.clone(),
            children: Vec::new(),
        }]
    };

    StyledNode {
        node: element_handle,
        specified_values: values,
        children,
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

    // Every rule that matches contributes, not just the one that carries
    // `content`: a design system states the shape once and overrides a size or a
    // colour in a later, equally specific rule. Taking only the last rule with a
    // `content` declaration threw the overrides away, so a wash narrowed to half
    // its host by a modifier class still covered the whole of it.
    let mut matching: [Vec<(usize, (usize, usize, usize), &crate::css::Rule)>; 2] =
        [Vec::new(), Vec::new()];
    for (order, rule) in all_rules.iter().enumerate() {
        for sel in &rule.selectors {
            let slot = match sel.pseudo_element.as_deref() {
                Some("before") => 0,
                Some("after") => 1,
                _ => continue,
            };
            if !selector_base_matches_element(sel, node) {
                continue;
            }
            matching[slot].push((order, sel.specificity(), rule));
            break;
        }
    }

    let mut built: [Option<StyledNode>; 2] = [None, None];
    for (slot, rules) in matching.iter_mut().enumerate() {
        rules.sort_by_key(|(order, spec, _)| (*spec, *order));

        // Cascade order, then the important declarations on top of all of them.
        let mut declarations: Vec<crate::css::Declaration> = Vec::new();
        for (_, _, rule) in rules.iter() {
            declarations.extend(rule.declarations.iter().filter(|d| !d.important).cloned());
        }
        for (_, _, rule) in rules.iter() {
            declarations.extend(rule.declarations.iter().filter(|d| d.important).cloned());
        }

        // The winning `content` decides whether there is a box at all.
        let Some(content_str) = declarations
            .iter()
            .rev()
            .find(|d| d.name.as_ref() == "content")
            .and_then(|decl| match &decl.value {
                Value::Keyword(k) if matches!(k.as_ref(), "none" | "normal") => None,
                Value::Keyword(k) => {
                    // Strip wrapping quotes that the CSS parser preserves.
                    let s = k.as_ref();
                    let inner = if (s.starts_with('"') && s.ends_with('"'))
                        || (s.starts_with('\'') && s.ends_with('\''))
                    {
                        &s[1..s.len() - 1]
                    } else {
                        s
                    };
                    Some(unescape_css_string(inner))
                }
                _ => None,
            })
        else {
            continue;
        };

        built[slot] = Some(make_pseudo_styled_node(
            content_str,
            &declarations,
            parent_values,
        ));
    }

    let [before_node, after_node] = built;
    (before_node, after_node)
}

/// The prefix under which a field's `::placeholder` declarations are stored on
/// the field's own property map.
///
/// `::placeholder` styles text that has no element of its own — the renderer
/// draws it straight into the field's box — so there is nowhere to hang a
/// separate style node. Keeping the declarations on the field under a prefix no
/// real property can collide with lets paint read them where it draws the text.
pub const PLACEHOLDER_PREFIX: &str = "-placeholder-";

/// Copy the declarations of every matching `input::placeholder` rule onto the
/// input itself, prefixed.
///
/// Without this, github's hero drew "you@domain.com" over the "Enter your
/// email" label that is meant to replace it: the page hides the placeholder
/// with `::placeholder { opacity: 0 }` and floats the label on top.
fn collect_placeholder_style(node: &mut StyledNode, all_rules: &[&crate::css::Rule]) {
    if !matches!(node.node.data, NodeData::Element { .. }) {
        return;
    }
    let mut found: Vec<(Arc<str>, Value)> = Vec::new();
    for rule in all_rules {
        let targets_placeholder = rule.selectors.iter().any(|sel| {
            sel.pseudo_element.as_deref() == Some("placeholder")
                && selector_base_matches_element(sel, node)
        });
        if !targets_placeholder {
            continue;
        }
        for decl in &rule.declarations {
            found.push((
                intern(&format!("{PLACEHOLDER_PREFIX}{}", decl.name)),
                decl.value.clone(),
            ));
        }
    }
    if found.is_empty() {
        return;
    }
    let mut values: HashMap<Arc<str>, Value> = (*node.specified_values).clone();
    for (key, value) in found {
        values.insert(key, value);
    }
    node.specified_values = PropertyMap(Arc::new(values));
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
        collect_placeholder_style(node, all_rules);

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

    /// The same case rule applies to a custom property set in a `style`
    /// attribute: lowercasing `--Spacer-size` there meant every `var()` naming
    /// it missed, and the declaration fell through to its own fallback — which
    /// is how a 20px spacer came out 112px tall.
    #[test]
    fn test_inline_custom_property_names_are_case_sensitive() {
        let html = r#"<div style="--Spacer-size: 20px"><div id="s" style="height: var(--Spacer-size, 112px)">x</div></div>"#;
        let tree = make_tree(html, "");
        fn find_id<'a>(n: &'a StyledNode, id: &str) -> Option<&'a StyledNode> {
            if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = n.node.data {
                if attrs.borrow().iter().any(|a| a.name.local.as_ref() == "id" && a.value.as_ref() == id) {
                    return Some(n);
                }
            }
            n.children.iter().find_map(|c| find_id(c, id))
        }
        let s = find_id(&tree, "s").expect("spacer");
        assert_eq!(get_length_px(s, "height"), Some(20.0));
    }

    /// A design system's tokens are mixed-case — `--borderColor-default`, not
    /// `--bordercolor-default`. Custom property names are case-sensitive, so
    /// lowercasing them at parse time made every `var()` naming one look up a
    /// property nothing had defined, and the whole palette resolved to nothing.
    #[test]
    fn test_custom_property_names_are_case_sensitive() {
        let html = r#"<html data-color-mode="dark" data-dark-theme="dark"><body><div id="x">hi</div></body></html>"#;
        let css = r#"
          [data-color-mode="dark"][data-dark-theme="dark"] {
            --borderColor-default: #3d444d;
            --borderWidth-thin: 1px;
          }
          #x { border-right: var(--borderWidth-thin) solid var(--borderColor-default); }
        "#;
        let tree = make_tree(html, css);
        let x = find_node(&tree, "div").expect("div");

        assert_eq!(get_length_px(x, "border-right-width"), Some(1.0));
        assert_eq!(get_keyword(x, "border-right-style").as_deref(), Some("solid"));
        assert_eq!(
            get_color(x, "border-right-color"),
            Some(Color { r: 0x3d, g: 0x44, b: 0x4d, a: 255 })
        );
    }

    /// A `var()` written first in a border shorthand is the width, not the
    /// colour. Reading it as the colour left the width to default to `medium`,
    /// which drew a 3px rule in the element's text colour where the page asked
    /// for a hairline in a muted one.
    #[test]
    fn test_border_shorthand_leading_var_is_the_width() {
        let html = r#"<html><body><div id="x">hi</div></body></html>"#;
        let css = r#"
          :root { --w: 2px; }
          #x { border-top: var(--w) solid #fff9; }
        "#;
        let tree = make_tree(html, css);
        let x = find_node(&tree, "div").expect("div");

        assert_eq!(get_length_px(x, "border-top-width"), Some(2.0));
        assert_eq!(
            get_color(x, "border-top-color"),
            Some(Color { r: 255, g: 255, b: 255, a: 0x99 }),
            "#fff9 is a four-digit hex — a white at 60% alpha"
        );
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
    /// Whether `parent_tag` has a child carrying `text`.
    ///
    /// A generated box is an element with the content as its own text child, so
    /// the search goes one level deeper than the parent's own children.
    fn find_text_child(root: &StyledNode, parent_tag: &str, text: &str) -> bool {
        let node = match find_node(root, parent_tag) {
            Some(n) => n,
            None => return false,
        };
        fn holds(node: &StyledNode, text: &str) -> bool {
            if let markup5ever_rcdom::NodeData::Text { ref contents } = node.node.data {
                return contents.borrow().as_ref() == text;
            }
            node.children.iter().any(|c| holds(c, text))
        }
        node.children.iter().any(|c| holds(c, text))
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

    /// `::placeholder` has no element of its own, so its declarations are kept
    /// on the field under a prefix for paint to read where it draws the text.
    #[test]
    fn test_placeholder_rule_lands_on_the_input() {
        let tree = make_tree(
            r#"<html><body><input id="e" placeholder="you@domain.com"></body></html>"#,
            r#"input::placeholder { opacity: 0; color: #ff0000; }"#,
        );
        fn find_input(n: &StyledNode) -> Option<&StyledNode> {
            if let markup5ever_rcdom::NodeData::Element { ref name, .. } = n.node.data {
                if name.local.as_ref() == "input" {
                    return Some(n);
                }
            }
            n.children.iter().find_map(find_input)
        }
        let input = find_input(&tree).expect("input");
        assert!(
            matches!(
                input
                    .specified_values
                    .get(&intern(&format!("{PLACEHOLDER_PREFIX}opacity"))),
                Some(Value::Number(v)) if *v == 0.0
            ),
            "the placeholder's opacity should be stored on the field"
        );
        assert!(
            input
                .specified_values
                .get(&intern(&format!("{PLACEHOLDER_PREFIX}color")))
                .is_some(),
            "and so should its colour"
        );
        // The field's own `color` and `opacity` must be untouched by it.
        assert!(input.specified_values.get(&intern("opacity")).is_none());
    }

    /// A `::placeholder` rule that does not match the field leaves it alone.
    #[test]
    fn test_placeholder_rule_for_another_selector_is_not_applied() {
        let tree = make_tree(
            r#"<html><body><input id="e" placeholder="x"></body></html>"#,
            r#".other::placeholder { opacity: 0; }"#,
        );
        fn find_input(n: &StyledNode) -> Option<&StyledNode> {
            if let markup5ever_rcdom::NodeData::Element { ref name, .. } = n.node.data {
                if name.local.as_ref() == "input" {
                    return Some(n);
                }
            }
            n.children.iter().find_map(find_input)
        }
        let input = find_input(&tree).expect("input");
        assert!(
            input
                .specified_values
                .get(&intern(&format!("{PLACEHOLDER_PREFIX}opacity")))
                .is_none()
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

    /// An icon set writes its glyph as `content: "\\f52a"` — a codepoint, not the
    /// four characters that spell it. Keeping the escape verbatim drew "f52a"
    /// next to every such icon.
    #[test]
    fn test_content_escapes_are_resolved() {
        let cases = [
            ("\\f52a", "\u{f52a}"),
            ("\\2192 ", "\u{2192}"),
            ("\\2192  x", "\u{2192} x"),
            ("a\\\"b", "a\"b"),
            ("plain", "plain"),
            ("\\", ""),
        ];
        for (source, want) in cases {
            assert_eq!(
                super::unescape_css_string(source),
                want,
                "{source:?} should unescape to {want:?}"
            );
        }
    }

    /// Every matching `::before` rule contributes, not only the one that
    /// carries `content`. A design system states the shape once and overrides a
    /// size in a modifier rule; taking the last rule with `content` threw the
    /// override away, so a wash narrowed to half its host still covered all of it.
    #[test]
    fn test_a_generated_box_cascades_every_matching_rule() {
        fn pseudo_of<'a>(n: &'a StyledNode, host_class: &str) -> Option<&'a StyledNode> {
            fn find<'a>(n: &'a StyledNode, cls: &str) -> Option<&'a StyledNode> {
                if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = n.node.data {
                    if attrs.borrow().iter().any(|a| {
                        a.name.local.as_ref() == "class"
                            && a.value.as_ref().split_whitespace().any(|c| c == cls)
                    }) {
                        return n.children.first();
                    }
                }
                n.children.iter().find_map(|c| find(c, cls))
            }
            find(n, host_class)
        }

        let css = r#"
            .panel::before { content: ""; width: 100%; height: 100%; background: #9a7cff }
            .half::before { width: 50%; height: 50% }
            .named::before { content: "x" }
        "#;
        let tree = make_tree(
            r#"<div class="panel"></div><div class="panel half"></div><div class="panel named"></div>"#,
            css,
        );

        fn percent(n: &StyledNode, prop: &str) -> Option<f32> {
            match n.specified_values.get(&intern(prop)) {
                Some(Value::Length(v, Unit::Percent)) => Some(*v),
                _ => None,
            }
        }

        let plain = pseudo_of(&tree, "panel").expect("the plain panel's box");
        assert_eq!(percent(plain, "width"), Some(100.0), "percent kept as stated");

        let half = pseudo_of(&tree, "half").expect("the half panel's box");
        assert_eq!(
            (percent(half, "width"), percent(half, "height")),
            (Some(50.0), Some(50.0)),
            "a modifier rule with no `content` still overrides the size",
        );

        // The later `content` wins, and the earlier rule's properties survive.
        let named = pseudo_of(&tree, "named").expect("the named panel's box");
        assert!(
            named.specified_values.get(&intern("background")).is_some()
                || named.specified_values.get(&intern("background-color")).is_some(),
            "the first rule's background must survive the second rule's `content`",
        );
    }

    /// `text-decoration` does not inherit, but the line an ancestor draws runs
    /// under its in-flow descendants — and the text node carrying the glyphs is
    /// where the line is actually drawn, so it has to reach there. Nothing
    /// carried it, and no link on any page was underlined.
    #[test]
    fn test_text_decoration_reaches_the_text_it_underlines() {
        fn decoration_of<'a>(n: &'a StyledNode, text: &str) -> Option<String> {
            if let markup5ever_rcdom::NodeData::Text { ref contents } = n.node.data {
                if contents.borrow().trim() == text {
                    return match n.specified_values.get(&intern("text-decoration")) {
                        Some(Value::Keyword(k)) => Some(k.to_string()),
                        _ => Some("<unset>".to_string()),
                    };
                }
            }
            n.children.iter().find_map(|c| decoration_of(c, text))
        }

        let tree = make_tree(
            r##"<a href="#">a link</a>
               <a href="#"><span>a link with a span</span></a>
               <a href="#" style="text-decoration: none">a bare link</a>
               <p style="text-decoration: underline">underlined <em>and stressed</em></p>"##,
            "",
        );
        assert_eq!(decoration_of(&tree, "a link").as_deref(), Some("underline"));
        assert_eq!(decoration_of(&tree, "a link with a span").as_deref(), Some("underline"));
        assert_eq!(decoration_of(&tree, "a bare link").as_deref(), Some("none"));
        assert_eq!(decoration_of(&tree, "and stressed").as_deref(), Some("underline"));
    }

    /// It stops at an atomic inline, which starts a decoration context of its
    /// own: a button inside a link is not underlined.
    #[test]
    fn test_text_decoration_stops_at_an_atomic_inline() {
        fn decoration_of<'a>(n: &'a StyledNode, text: &str) -> Option<String> {
            if let markup5ever_rcdom::NodeData::Text { ref contents } = n.node.data {
                if contents.borrow().trim() == text {
                    return match n.specified_values.get(&intern("text-decoration")) {
                        Some(Value::Keyword(k)) => Some(k.to_string()),
                        _ => Some("<unset>".to_string()),
                    };
                }
            }
            n.children.iter().find_map(|c| decoration_of(c, text))
        }

        let tree = make_tree(
            r##"<a href="#">before <span style="display: inline-block">a button</span> after</a>"##,
            "",
        );
        assert_eq!(decoration_of(&tree, "before").as_deref(), Some("underline"));
        assert_eq!(decoration_of(&tree, "a button").as_deref(), Some("<unset>"));
    }

    /// Cascade layer order beats specificity: an unlayered rule wins over one
    /// in any layer, however specific that one is. github states its component
    /// rules inside `@layer primer-brand` and overrides them with plain
    /// page-level classes, so ranking by specificity alone kept a four-class
    /// `:not()` selector's 24px margin over the unlayered rule's 16px and every
    /// pillar came out 8px too tall.
    #[test]
    fn test_an_unlayered_rule_beats_a_more_specific_layered_one() {
        let css = r#"
            @layer components;
            @layer components {
              .desc:not(.bordered .desc:last-child) { margin-bottom: 24px }
            }
            .customer-desc { margin-bottom: 16px }
        "#;
        let tree = make_tree(r#"<div><p id="p" class="desc customer-desc">x</p><a>y</a></div>"#, css);
        fn find_id<'a>(n: &'a StyledNode, id: &str) -> Option<&'a StyledNode> {
            if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = n.node.data {
                if attrs.borrow().iter().any(|a| a.name.local.as_ref() == "id" && a.value.as_ref() == id) {
                    return Some(n);
                }
            }
            n.children.iter().find_map(|c| find_id(c, id))
        }
        let p = find_id(&tree, "p").expect("the paragraph");
        assert_eq!(
            get_length_px(p, "margin-bottom"),
            Some(16.0),
            "the unlayered rule wins even though the layered one is far more specific",
        );
    }

    /// Within one layer the cascade falls back to specificity as usual.
    #[test]
    fn test_specificity_still_decides_inside_one_layer() {
        let css = r#"
            @layer components {
              .desc.narrow { margin-bottom: 24px }
              .desc { margin-bottom: 4px }
            }
        "#;
        let tree = make_tree(r#"<p id="p" class="desc narrow">x</p>"#, css);
        fn find_id<'a>(n: &'a StyledNode, id: &str) -> Option<&'a StyledNode> {
            if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = n.node.data {
                if attrs.borrow().iter().any(|a| a.name.local.as_ref() == "id" && a.value.as_ref() == id) {
                    return Some(n);
                }
            }
            n.children.iter().find_map(|c| find_id(c, id))
        }
        let p = find_id(&tree, "p").expect("the paragraph");
        assert_eq!(get_length_px(p, "margin-bottom"), Some(24.0));
    }

    /// `:has()` is how a modern design system reacts to what an element
    /// *contains*. github's hero pads itself only when it holds a UI panel —
    /// `.lp-SectionHero-visual:has(.…--copilotUI) { padding: 48px 96px }` —
    /// and with the pseudo-class unsupported the section came out 31px short.
    #[test]
    fn test_has_matches_on_what_an_element_contains() {
        fn height_of<'a>(n: &'a StyledNode, id: &str) -> Option<f32> {
            fn find<'a>(n: &'a StyledNode, id: &str) -> Option<&'a StyledNode> {
                if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = n.node.data {
                    if attrs.borrow().iter().any(|a| a.name.local.as_ref() == "id" && a.value.as_ref() == id) {
                        return Some(n);
                    }
                }
                n.children.iter().find_map(|c| find(c, id))
            }
            find(n, id).and_then(|s| get_length_px(s, "height"))
        }

        let css = ".box:has(.flag) { height: 40px }";
        let tree = make_tree(
            r#"<div id="a" class="box"><span><i class="flag"></i></span></div>
               <div id="b" class="box"><span><i class="other"></i></span></div>"#,
            css,
        );
        assert_eq!(height_of(&tree, "a"), Some(40.0), ":has() must match a deep descendant");
        assert_eq!(height_of(&tree, "b"), None, ":has() must not match without one");
    }

    /// A relative selector carries the combinator it was written with.
    #[test]
    fn test_has_honours_its_leading_combinator() {
        fn matched(html: &str, css: &str, id: &str) -> bool {
            fn find<'a>(n: &'a StyledNode, id: &str) -> Option<&'a StyledNode> {
                if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = n.node.data {
                    if attrs.borrow().iter().any(|a| a.name.local.as_ref() == "id" && a.value.as_ref() == id) {
                        return Some(n);
                    }
                }
                n.children.iter().find_map(|c| find(c, id))
            }
            let tree = make_tree(html, css);
            find(&tree, id).and_then(|s| get_length_px(s, "height")) == Some(40.0)
        }

        // `> .flag` is a child, not any descendant.
        let child_html = r#"<div id="a" class="box"><i class="flag"></i></div>
                            <div id="b" class="box"><span><i class="flag"></i></span></div>"#;
        assert!(matched(child_html, ".box:has(> .flag) { height: 40px }", "a"));
        assert!(!matched(child_html, ".box:has(> .flag) { height: 40px }", "b"));

        // `+ .flag` is the *next* element sibling; `~ .flag` is any later one.
        let sib_html = r#"<div id="a" class="box"></div><i class="flag"></i>
                          <div id="b" class="box"></div><em></em><i class="flag"></i>"#;
        assert!(matched(sib_html, ".box:has(+ .flag) { height: 40px }", "a"));
        assert!(!matched(sib_html, ".box:has(+ .flag) { height: 40px }", "b"));
        assert!(matched(sib_html, ".box:has(~ .flag) { height: 40px }", "b"));
    }

    /// `:has()` takes a selector *list*, and contributes the specificity of its
    /// most specific argument — the same rule `:is()` follows.
    #[test]
    fn test_has_parses_a_list_and_carries_its_specificity() {
        use crate::css::{parse_selector, Combinator, PseudoClass};
        let sel = parse_selector(".card:has(> .a, .b .c)");
        let [PseudoClass::Has(ref inner)] = sel.pseudo_classes[..] else {
            panic!("expected one :has(), got {:?}", sel.pseudo_classes);
        };
        assert_eq!(inner.len(), 2);
        assert!(matches!(inner[0].0, Combinator::Child));
        assert!(matches!(inner[1].0, Combinator::Descendant));
        // `.card` plus the two classes of `.b .c`, the most specific argument.
        assert_eq!(sel.specificity(), (0, 3, 0));
    }

    /// The character before `=` picks the attribute match. Reading every form
    /// as a plain equality left the operator stuck on the attribute's name, so
    /// the selector matched nothing — and github styles every one of its links
    /// through `[class^="Primer_Brand__Link-module__Link___"]`.
    #[test]
    fn test_attribute_selector_operators() {
        use crate::css::{parse_selector, AttributeMatch};
        let cases: [(&str, AttributeMatch, &[&str], &[&str]); 5] = [
            ("[class^=\"Link__\"]", AttributeMatch::Prefix("Link__".into()), &["Link__a b"], &["x Link__a"]),
            ("[class$=\"-end\"]", AttributeMatch::Suffix("-end".into()), &["a-end"], &["a-end b"]),
            ("[class*=\"mid\"]", AttributeMatch::Contains("mid".into()), &["a midb"], &["a b"]),
            ("[class~=\"one\"]", AttributeMatch::Includes("one".into()), &["one two", "two one"], &["oneone"]),
            ("[lang|=\"en\"]", AttributeMatch::DashMatch("en".into()), &["en", "en-GB"], &["eng", "fr"]),
        ];
        for (source, expected, hits, misses) in cases {
            let sel = parse_selector(source);
            assert_eq!(sel.attributes.len(), 1, "{source} should parse one attribute selector");
            assert_eq!(sel.attributes[0].value, expected, "{source} picked the wrong match");
            assert!(
                sel.attributes[0].name == "class" || sel.attributes[0].name == "lang",
                "{source} left the operator on the name: {}",
                sel.attributes[0].name
            );
            for hit in hits {
                assert!(sel.attributes[0].value.matches(hit), "{source} should match {hit:?}");
            }
            for miss in misses {
                assert!(!sel.attributes[0].value.matches(miss), "{source} should not match {miss:?}");
            }
        }
    }

    /// Only whitespace *outside* brackets and parentheses separates a
    /// selector's compounds. Splitting on every space tore `:not(.a + .a)` into
    /// three parts, so github's
    /// `.LogoSuite--default:not(.LogoSuite + .LogoSuite) .logobar` matched
    /// nothing and its logo strip lost its 32px of top padding.
    #[test]
    fn test_a_pseudo_class_argument_is_one_compound() {
        use crate::css::{parse_selector, Combinator, PseudoClass};
        let sel = parse_selector(".a:not(.b + .b) .inner");
        assert_eq!(sel.class, vec!["inner".to_string()]);
        assert!(matches!(sel.combinator, Some(Combinator::Descendant)));
        let ancestor = sel.ancestor.as_ref().expect("the `.a:not(…)` compound");
        assert_eq!(ancestor.class, vec!["a".to_string()]);
        let [PseudoClass::Not(ref inner)] = ancestor.pseudo_classes[..] else {
            panic!("expected one :not(), got {:?}", ancestor.pseudo_classes);
        };
        assert_eq!(inner.len(), 1);
        assert!(
            matches!(inner[0].combinator, Some(Combinator::NextSibling)),
            "the argument keeps its own combinator"
        );
    }

    /// An empty operand matches nothing, per the selectors spec — and keeps
    /// `[class^=""]` from styling every element on the page.
    #[test]
    fn test_empty_attribute_operand_matches_nothing() {
        use crate::css::AttributeMatch;
        for m in [
            AttributeMatch::Prefix(String::new()),
            AttributeMatch::Suffix(String::new()),
            AttributeMatch::Contains(String::new()),
            AttributeMatch::Includes(String::new()),
        ] {
            assert!(!m.matches("anything"), "{m:?} should match nothing");
        }
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
    // A `style` attribute carries logical properties as readily as a stylesheet
    // does, so it goes through the same rewrite first.
    for decl in crate::css::expand_logical_declarations(style_str) {
        let decl = decl.trim();
        if decl.is_empty() { continue; }
        let mut kv = decl.splitn(2, ':');
        // Property names are case-insensitive, custom property names are not —
        // the same rule the stylesheet parser follows. Lowercasing a
        // `--Spacer-size` set in a `style` attribute meant every `var()` naming
        // it missed, and the declaration fell through to its own fallback.
        let raw_key = kv.next().unwrap_or("").trim();
        let key = if raw_key.starts_with("--") {
            intern(raw_key)
        } else {
            intern(&raw_key.to_lowercase())
        };
        let val_raw = kv.next().unwrap_or("").trim();
        if key.is_empty() || val_raw.is_empty() { continue; }

        let important = val_raw.ends_with("!important");
        let val = if important { val_raw.trim_end_matches("!important").trim() } else { val_raw };

        match &*key {
            // The same reset a stylesheet's `background` performs: a `style`
            // attribute's shorthand clears the colour too.
            "background" => {
                crate::css::expand_background_shorthand(val, important, list);
            }
            "border" => {
                let mut temp_map = HashMap::new();
                crate::css::parse_border_shorthand_pub(val, &mut temp_map);
                for (k, v) in &temp_map {
                    list.push(crate::css::Declaration { name: intern(k), value: v.clone(), important });
                }
                for side in crate::css::BORDER_SIDES {
                    for part in ["width", "style", "color"] {
                        if let Some(v) = temp_map.get(&format!("border-{part}")) {
                            list.push(crate::css::Declaration {
                                name: intern(&format!("border-{side}-{part}")),
                                value: v.clone(),
                                important,
                            });
                        }
                    }
                }
            }
            // Single-edge shorthand in an inline style, expanded the same way the
            // stylesheet parser expands it.
            "border-top" | "border-right" | "border-bottom" | "border-left" => {
                let side = key.rsplit('-').next().unwrap_or("top").to_string();
                let mut temp_map = HashMap::new();
                crate::css::parse_border_shorthand_pub(val, &mut temp_map);
                temp_map
                    .entry("border-width".to_string())
                    .or_insert(Value::Length(3.0, crate::css::Unit::Px));
                for part in ["width", "style", "color"] {
                    if let Some(v) = temp_map.get(&format!("border-{part}")) {
                        list.push(crate::css::Declaration {
                            name: intern(&format!("border-{side}-{part}")),
                            value: v.clone(),
                            important,
                        });
                    }
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
                crate::css::expand_flex_shorthand(val, important, list);
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
            "border-radius" => {
                crate::css::expand_border_radius(val, important, list);
            }
            "filter" | "-webkit-filter" => {
                crate::css::expand_filter(val, important, list);
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
