use crate::css::{Stylesheet, Value, Selector, parse_color, Combinator, intern, SelectorKey, PseudoClass, Unit};

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

/// Which box a selector styles: the element itself or one of its generated boxes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PseudoTarget {
    None,
    Before,
    After,
    Placeholder,
}

/// Cascaded declarations of an element and of its `::before`, `::after` and
/// `::placeholder` pseudo-elements.
type Cascaded = (
    HashMap<Arc<str>, Value>,
    Option<HashMap<Arc<str>, Value>>,
    Option<HashMap<Arc<str>, Value>>,
    Option<HashMap<Arc<str>, Value>>,
);

/// An entry in the selector index pointing to a specific selector within a rule.
#[derive(Clone)]
struct IndexEntry {
    specificity: (usize, usize, usize),
    rule_idx: usize,
    sel_idx: usize,
    /// True when the match result depends on more than tag/id/classes (ancestors,
    /// attributes, structural pseudo-classes), so it cannot be cached per signature.
    is_complex: bool,
    target: PseudoTarget,
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
    fn build(rules: &[&crate::css::Rule]) -> Self {
        let mut by_id: HashMap<String, Vec<IndexEntry>> = HashMap::new();
        let mut by_class: HashMap<String, Vec<IndexEntry>> = HashMap::new();
        let mut by_tag: HashMap<String, Vec<IndexEntry>> = HashMap::new();
        let mut universal: Vec<IndexEntry> = Vec::new();

        for (rule_idx, rule) in rules.iter().enumerate() {
            for (sel_idx, sel) in rule.selectors.iter().enumerate() {
                if sel.never_matches {
                    continue;
                }
                let target = match sel.pseudo_element.as_deref() {
                    None => PseudoTarget::None,
                    Some("before") => PseudoTarget::Before,
                    Some("after") => PseudoTarget::After,
                    Some("placeholder") => PseudoTarget::Placeholder,
                    Some(_) => continue,
                };
                let entry = IndexEntry {
                    specificity: sel.specificity(),
                    rule_idx,
                    sel_idx,
                    is_complex: sel.ancestor.is_some()
                        || !sel.attributes.is_empty()
                        || !sel.attr_ops.is_empty()
                        || !sel.pseudos.is_empty(),
                    target,
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
        classes.dedup();
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
    /// Text node with at least one character (used by `:empty`).
    pub has_text: bool,
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
        let mut has_text = false;

        match handle.data {
            NodeData::Element { ref name, ref attrs, .. } => {
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
            NodeData::Text { ref contents } => {
                has_text = !contents.borrow().is_empty();
            }
            _ => {}
        }

        arena.push(NodeDataSend {
            tag, id, classes, attrs: attrs_vec, is_element, has_text, parent_idx, children_idx: Vec::new()
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

struct MatchCtx<'a> {
    arena: &'a [NodeDataSend],
    hovered_id: Option<&'a str>,
    focused_id: Option<&'a str>,
}

fn attr_value<'a>(node: &'a NodeDataSend, name: &str) -> Option<&'a str> {
    node.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Element siblings of `idx` (including itself), in document order.
fn element_siblings(idx: usize, arena: &[NodeDataSend]) -> Vec<usize> {
    match arena[idx].parent_idx {
        Some(p) => arena[p].children_idx.iter().copied().filter(|&c| arena[c].is_element).collect(),
        None => vec![idx],
    }
}

fn pseudo_matches(pc: &PseudoClass, idx: usize, ctx: &MatchCtx) -> bool {
    let arena = ctx.arena;
    let node = &arena[idx];
    match pc {
        PseudoClass::Not(list) => !list.iter().any(|s| matches_selector_arena(s, idx, ctx)),
        PseudoClass::Is(list) | PseudoClass::Where(list) => list.iter().any(|s| matches_selector_arena(s, idx, ctx)),
        PseudoClass::FirstChild => element_siblings(idx, arena).first() == Some(&idx),
        PseudoClass::LastChild => element_siblings(idx, arena).last() == Some(&idx),
        PseudoClass::OnlyChild => element_siblings(idx, arena).len() == 1,
        PseudoClass::NthChild(a, b, of) | PseudoClass::NthLastChild(a, b, of) => {
            let mut sibs = element_siblings(idx, arena);
            if let Some(list) = of {
                if !list.iter().any(|s| matches_selector_arena(s, idx, ctx)) {
                    return false;
                }
                sibs.retain(|&s| list.iter().any(|sel| matches_selector_arena(sel, s, ctx)));
            }
            if matches!(pc, PseudoClass::NthLastChild(..)) {
                sibs.reverse();
            }
            match sibs.iter().position(|&s| s == idx) {
                Some(pos) => crate::css::nth_matches(*a, *b, pos as i32 + 1),
                None => false,
            }
        }
        PseudoClass::FirstOfType | PseudoClass::LastOfType | PseudoClass::OnlyOfType
        | PseudoClass::NthOfType(..) | PseudoClass::NthLastOfType(..) => {
            let mut sibs: Vec<usize> = element_siblings(idx, arena).into_iter().filter(|&s| arena[s].tag == node.tag).collect();
            match pc {
                PseudoClass::FirstOfType => sibs.first() == Some(&idx),
                PseudoClass::LastOfType => sibs.last() == Some(&idx),
                PseudoClass::OnlyOfType => sibs.len() == 1,
                PseudoClass::NthOfType(a, b) => sibs.iter().position(|&s| s == idx).map_or(false, |p| crate::css::nth_matches(*a, *b, p as i32 + 1)),
                PseudoClass::NthLastOfType(a, b) => {
                    sibs.reverse();
                    sibs.iter().position(|&s| s == idx).map_or(false, |p| crate::css::nth_matches(*a, *b, p as i32 + 1))
                }
                _ => false,
            }
        }
        PseudoClass::Empty => node.children_idx.iter().all(|&c| !arena[c].is_element && !arena[c].has_text),
        PseudoClass::Root => node.tag == "html",
        PseudoClass::Hover => node.id.is_some() && node.id.as_deref() == ctx.hovered_id,
        PseudoClass::Focus => node.id.is_some() && node.id.as_deref() == ctx.focused_id,
        PseudoClass::Link => matches!(node.tag.as_str(), "a" | "area") && attr_value(node, "href").is_some(),
        PseudoClass::Checked => {
            (node.tag == "input" && attr_value(node, "checked").is_some())
                || (node.tag == "option" && attr_value(node, "selected").is_some())
        }
        PseudoClass::Disabled | PseudoClass::Enabled => {
            let is_control = matches!(node.tag.as_str(), "input" | "button" | "select" | "textarea" | "option" | "optgroup" | "fieldset");
            let disabled = is_control && attr_value(node, "disabled").is_some();
            if matches!(pc, PseudoClass::Disabled) { disabled } else { is_control && !disabled }
        }
        PseudoClass::Never(_) => false,
    }
}

/// Match the subject compound of `selector` (ignoring any pseudo-element) and
/// its combinator chain against arena node `idx`.
fn matches_selector_arena(selector: &Selector, idx: usize, ctx: &MatchCtx) -> bool {
    let arena = ctx.arena;
    let node = &arena[idx];
    if !node.is_element || selector.never_matches || !selector.has_constraint() {
        return false;
    }

    if let Some(ref s_tag) = selector.tag {
        if !node.tag.eq_ignore_ascii_case(s_tag) { return false; }
    }
    if let Some(ref s_id) = selector.id {
        if node.id.as_deref() != Some(s_id) { return false; }
    }
    for s_class in &selector.class {
        if !node.classes.contains(s_class) { return false; }
    }
    for attr_sel in &selector.attributes {
        let matched = node.attrs.iter().any(|(k, v)| {
            k == &attr_sel.name
                && match &attr_sel.value {
                    crate::css::AttributeMatch::Exists => true,
                    crate::css::AttributeMatch::Equals(val) => v == val,
                }
        });
        if !matched { return false; }
    }
    for op in &selector.attr_ops {
        match attr_value(node, &op.name) {
            Some(v) if op.matches_value(v) => {}
            _ => return false,
        }
    }
    if selector.pseudos.is_empty() {
        if let Some(ref pseudo) = selector.pseudo_class {
            // Hand-built selectors (tests) may carry only the legacy string form.
            let ok = match pseudo.as_str() {
                "hover" => node.id.is_some() && node.id.as_deref() == ctx.hovered_id,
                "focus" => node.id.is_some() && node.id.as_deref() == ctx.focused_id,
                "root" => node.tag == "html",
                _ => false,
            };
            if !ok { return false; }
        }
    }
    for pc in &selector.pseudos {
        if !pseudo_matches(pc, idx, ctx) { return false; }
    }

    if let Some(ref ancestor_sel) = selector.ancestor {
        let combinator = selector.combinator.as_ref().unwrap_or(&Combinator::Descendant);
        match combinator {
            Combinator::Descendant => {
                let mut current = node.parent_idx;
                while let Some(p_idx) = current {
                    if matches_selector_arena(ancestor_sel, p_idx, ctx) {
                        return true;
                    }
                    current = arena[p_idx].parent_idx;
                }
                return false;
            }
            Combinator::Child => {
                return match node.parent_idx {
                    Some(p_idx) => matches_selector_arena(ancestor_sel, p_idx, ctx),
                    None => false,
                };
            }
            Combinator::NextSibling => {
                let sibs = element_siblings(idx, arena);
                let pos = sibs.iter().position(|&s| s == idx).unwrap_or(0);
                return pos > 0 && matches_selector_arena(ancestor_sel, sibs[pos - 1], ctx);
            }
            Combinator::SubsequentSibling => {
                let sibs = element_siblings(idx, arena);
                for &s in &sibs {
                    if s == idx { break; }
                    if matches_selector_arena(ancestor_sel, s, ctx) { return true; }
                }
                return false;
            }
        }
    }
    true
}

/// Presentational hints from HTML attributes. They sit below every author rule.
fn apply_attribute_styles_arena(node: &NodeDataSend, map: &mut HashMap<Arc<str>, Value>) {
    if attr_value(node, "hidden").is_some() {
        map.insert(intern("display"), Value::Keyword(intern("none")));
    }
    match node.tag.as_str() {
        "table" | "td" | "th" | "col" | "tr" => {
            for (k, v) in &node.attrs {
                if k == "width" {
                    if let Some(width) = parse_legacy_length_attr(v) {
                        map.insert(intern("width"), width);
                    }
                }
                if k == "height" {
                    if let Some(h) = parse_legacy_length_attr(v) {
                        map.insert(intern("height"), h);
                    }
                }
                if k == "align" {
                    let align = v.to_ascii_lowercase();
                    if matches!(align.as_str(), "left" | "center" | "right") {
                        map.insert(intern("text-align"), Value::Keyword(intern(&align)));
                    }
                }
                if k == "bgcolor" {
                    if let Some(c) = parse_color(v) {
                        map.insert(intern("background-color"), Value::Color(c));
                    }
                }
            }
        }
        "img" | "svg" | "video" | "canvas" | "iframe" | "embed" | "object" => {
            for (k, v) in &node.attrs {
                if k == "width" { if let Some(val) = parse_legacy_length_attr(v) { map.insert(intern("width"), val); } }
                if k == "height" { if let Some(val) = parse_legacy_length_attr(v) { map.insert(intern("height"), val); } }
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

/// Apply one cascaded declaration to a declared-value map.
///
/// A shorthand whose value still contains `var()` is stored unexpanded; it
/// removes longhands set by earlier declarations so that, once expanded at
/// computed-value time, it only fills longhands that no *later* declaration set.
fn apply_declaration(map: &mut HashMap<Arc<str>, Value>, name: &Arc<str>, value: &Value) {
    if matches!(value, Value::RawCustomProp(_)) && !name.starts_with("--") {
        for lh in crate::css::shorthand_longhands(name) {
            map.remove(*lh);
        }
    }
    map.insert(name.clone(), value.clone());
}

fn collect_matches(
    node: &NodeDataSend,
    idx: usize,
    sel_index: &SelectorIndex,
    sig_cache: &HashMap<ElementSignature, Vec<(usize, usize, (usize, usize, usize))>>,
    all_rules: &[&crate::css::Rule],
    ctx: &MatchCtx,
    target: PseudoTarget,
) -> Vec<((usize, usize, usize), usize)> {
    // (specificity, rule_idx) with the best specificity per rule.
    let mut rule_matches: Vec<((usize, usize, usize), usize)> = Vec::new();
    let mut push = |spec: (usize, usize, usize), rule_idx: usize| {
        if let Some(existing) = rule_matches.iter_mut().find(|(_, r)| *r == rule_idx) {
            if spec > existing.0 { existing.0 = spec; }
        } else {
            rule_matches.push((spec, rule_idx));
        }
    };
    if target == PseudoTarget::None {
        if let Some(simple) = sig_cache.get(&ElementSignature::from_node(node)) {
            for &(rule_idx, _, spec) in simple {
                push(spec, rule_idx);
            }
        }
    }
    for entry in sel_index.candidates(node) {
        if entry.target != target {
            continue;
        }
        if target == PseudoTarget::None && !entry.is_complex {
            continue; // handled by the signature cache
        }
        let sel = &all_rules[entry.rule_idx].selectors[entry.sel_idx];
        if matches_selector_arena(sel, idx, ctx) {
            push(entry.specificity, entry.rule_idx);
        }
    }
    // Cascade order: specificity, then source order.
    rule_matches.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    rule_matches
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
    // Quirks-mode detection: a document with no doctype (the common real-world
    // trigger — e.g. Hacker News ships no `<!DOCTYPE>`) renders in quirks
    // mode. `root` is the document node here, so a `Doctype` child means
    // standards/almost-standards mode. This does not attempt to replicate the
    // full legacy-doctype quirks list (html5ever already computes that
    // authoritatively on `RcDom::quirks_mode`, see `dom.rs`/`RcDom`), but the
    // no-doctype case is what matters for the quirks-only style resets below.
    let quirks_mode = !root
        .children
        .borrow()
        .iter()
        .any(|c| matches!(c.data, NodeData::Doctype { .. }));

    let mut arena = Vec::new();
    flatten_dom(root, &mut arena, None);

    // Snapshot all_rules into a Vec so we can index into it by rule_idx.
    let all_rules: Vec<&crate::css::Rule> = stylesheet.all_rules();
    // Pre-build selector index: O(M) — done once before the parallel phase.
    let sel_index = SelectorIndex::build(&all_rules);
    let keyframes = stylesheet.keyframes();

    let ctx = MatchCtx { arena: &arena, hovered_id, focused_id };

    // Pre-build element signature cache for simple (position-independent) selectors.
    let mut sig_cache: HashMap<ElementSignature, Vec<(usize, usize, (usize, usize, usize))>> = HashMap::new();
    for (idx, node) in arena.iter().enumerate() {
        if !node.is_element { continue; }
        let sig = ElementSignature::from_node(node);
        if sig_cache.contains_key(&sig) { continue; }
        let mut matched = Vec::new();
        for entry in sel_index.candidates(node) {
            if entry.is_complex || entry.target != PseudoTarget::None { continue; }
            let sel = &all_rules[entry.rule_idx].selectors[entry.sel_idx];
            if matches_selector_arena(sel, idx, &ctx) {
                matched.push((entry.rule_idx, entry.sel_idx, entry.specificity));
            }
        }
        sig_cache.insert(sig, matched);
    }

    // Phase 1: Parallel CSS Matching + cascade (index-accelerated).
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

    let mut raw_styles: Vec<Cascaded> = pool.install(|| {
        arena.par_iter().enumerate().map(|(idx, node)| {
        if !node.is_element { return (HashMap::new(), None, None, None); }
        let mut map = HashMap::new();
        apply_default_styles(&node.tag, node, &mut map, quirks_mode);
        apply_attribute_styles_arena(node, &mut map);

        let rule_matches = collect_matches(node, idx, &sel_index, &sig_cache, &all_rules, &ctx, PseudoTarget::None);

        // Apply matched rules; defer important declarations.
        let mut important: Vec<(Arc<str>, Value)> = Vec::new();
        for (_, rule_idx) in &rule_matches {
            for decl in &all_rules[*rule_idx].declarations {
                if !decl.important { apply_declaration(&mut map, &decl.name, &decl.value); }
                else { important.push((decl.name.clone(), decl.value.clone())); }
            }
        }

        let mut inline_important: Vec<(Arc<str>, Value)> = Vec::new();
        if let Some(v) = attr_value(node, "style") {
            for decl in crate::css::parse_declaration_block(v) {
                if !decl.important { apply_declaration(&mut map, &decl.name, &decl.value); }
                else { inline_important.push((decl.name, decl.value)); }
            }
        }

        let important_names: HashSet<Arc<str>> =
            important.iter().chain(inline_important.iter()).map(|(k, _)| k.clone()).collect();
        for (k, v) in important { apply_declaration(&mut map, &k, &v); }
        for (k, v) in inline_important { apply_declaration(&mut map, &k, &v); }

        if let Some(ref id) = node.id {
            if let Some(overrides) = js_overrides.get(id) {
                for (k, v) in overrides {
                    let mut decls = Vec::new();
                    let (val, _) = crate::css::strip_important(v);
                    crate::css::parse_declaration(&k.to_ascii_lowercase(), val, false, &mut decls);
                    for d in decls { apply_declaration(&mut map, &d.name, &d.value); }
                }
            }
        }

        // Animations sit above normal declarations and below !important ones.
        if !keyframes.is_empty() {
            for d in animation_declarations(&map, &keyframes) {
                if !important_names.contains(&d.name) {
                    apply_declaration(&mut map, &d.name, &d.value);
                }
            }
        }

        let pseudo = |target: PseudoTarget| -> Option<HashMap<Arc<str>, Value>> {
            let matches = collect_matches(node, idx, &sel_index, &sig_cache, &all_rules, &ctx, target);
            if matches.is_empty() { return None; }
            let mut pmap = HashMap::new();
            let mut imp = Vec::new();
            for (_, rule_idx) in &matches {
                for decl in &all_rules[*rule_idx].declarations {
                    if !decl.important { apply_declaration(&mut pmap, &decl.name, &decl.value); }
                    else { imp.push((decl.name.clone(), decl.value.clone())); }
                }
            }
            for (k, v) in imp { apply_declaration(&mut pmap, &k, &v); }
            Some(pmap)
        };
        let placeholder = if placeholder_text(node, &arena).is_some() {
            // UA style (Chromium html.css); author rules override it.
            let mut pmap = pseudo(PseudoTarget::Placeholder).unwrap_or_default();
            for (k, v) in [
                ("color", Value::Color(crate::css::Color { r: 0x75, g: 0x75, b: 0x75, a: 255 })),
                ("display", Value::Keyword(intern("block"))),
                ("white-space", Value::Keyword(intern("pre"))),
                ("overflow-x", Value::Keyword(intern("hidden"))),
                ("overflow-y", Value::Keyword(intern("hidden"))),
            ] {
                pmap.entry(intern(k)).or_insert(v);
            }
            Some(pmap)
        } else {
            None
        };
        (map, pseudo(PseudoTarget::Before), pseudo(PseudoTarget::After), placeholder)
    }).collect()
    });

    // Phase 2: Sequential Inheritance & computed values & pseudo-elements.
    let mut store = StyleStore::default();
    let mut arena_idx = 0;
    build_final_tree(root, &mut arena_idx, &mut raw_styles, &arena, parent_style, &mut store)
}

/// Declarations that running CSS animations contribute when the page is
/// rendered as a still frame. Every animation is treated as having run to
/// completion: a finite animation keeps its end keyframe when
/// `animation-fill-mode` is `forwards`/`both`; a paused one shows its start
/// keyframe (during its delay only with `backwards`/`both`). Infinite
/// animations and ones without a fill keep the base style. Only keyframes at
/// the exact end offset contribute (no interpolation).
fn animation_declarations(
    map: &HashMap<Arc<str>, Value>,
    keyframes: &HashMap<&str, &[crate::css::Keyframe]>,
) -> Vec<crate::css::Declaration> {
    let list = |name: &str| -> Vec<String> {
        map.get(name)
            .map(value_to_css_text)
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let names: Vec<String> = map
        .get("animation-name")
        .map(value_to_css_text)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .collect();
    if names.iter().all(|n| n.is_empty() || n == "none") {
        return Vec::new();
    }
    let counts = list("animation-iteration-count");
    let directions = list("animation-direction");
    let fills = list("animation-fill-mode");
    let states = list("animation-play-state");
    let delays = list("animation-delay");
    let pick = |l: &Vec<String>, i: usize, d: &str| -> String {
        if l.is_empty() { d.to_string() } else { l[i % l.len()].clone() }
    };
    let seconds = |t: &str| -> f32 {
        if let Some(ms) = t.strip_suffix("ms") { ms.parse::<f32>().map(|v| v / 1000.0).unwrap_or(0.0) }
        else { t.trim_end_matches('s').parse::<f32>().unwrap_or(0.0) }
    };
    let mut out = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let Some(frames) = keyframes.get(name.as_str()) else { continue };
        let fill = pick(&fills, i, "none");
        let fills_forwards = matches!(fill.as_str(), "forwards" | "both");
        let fills_backwards = matches!(fill.as_str(), "backwards" | "both");
        let direction = pick(&directions, i, "normal");
        let reversed_iteration = |iteration: u32| match direction.as_str() {
            "reverse" => true,
            "alternate" => iteration % 2 == 1,
            "alternate-reverse" => iteration % 2 == 0,
            _ => false,
        };
        let offset = if pick(&states, i, "running") == "paused" {
            if !fills_backwards && seconds(&pick(&delays, i, "0s")) > 0.0 {
                continue;
            }
            if reversed_iteration(0) { 1.0 } else { 0.0 }
        } else {
            let count = pick(&counts, i, "1");
            let Ok(count) = count.parse::<f32>() else { continue }; // infinite
            if !fills_forwards || count < 0.0 {
                continue;
            }
            let (iteration, progress) = if count == 0.0 {
                (0, 0.0)
            } else if count.fract() == 0.0 {
                (count as u32 - 1, 1.0)
            } else {
                (count.floor() as u32, count.fract())
            };
            if reversed_iteration(iteration) { 1.0 - progress } else { progress }
        };
        for frame in frames.iter() {
            if frame.offsets.iter().any(|o| (o - offset).abs() < 1e-4) {
                out.extend(frame.declarations.iter().cloned());
            }
        }
    }
    out
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
        "white-space"      => Some(Value::Keyword(intern("normal"))),
        "position"         => Some(Value::Keyword(intern("static"))),
        "float"            => Some(Value::Keyword(intern("none"))),
        "margin-top" | "margin-right" | "margin-bottom" | "margin-left" |
        "padding-top" | "padding-right" | "padding-bottom" | "padding-left" =>
            Some(Value::Length(0.0, Unit::Px)),
        _ => None,
    }
}

/// Properties inherited by default (CSS 2.1 + commonly used modern ones).
const INHERITED_PROPERTIES: &[&str] = &[
    "color",
    "font-size",
    "font-family",
    "font-weight",
    "font-style",
    "font-variant",
    "line-height",
    "letter-spacing",
    "word-spacing",
    "text-align",
    "text-indent",
    "text-transform",
    "text-shadow",
    "white-space",
    "word-break",
    "overflow-wrap",
    "word-wrap",
    "visibility",
    "cursor",
    "list-style-type",
    "list-style-position",
    "direction",
    "quotes",
    "border-collapse",
    "border-spacing",
    "caption-side",
    "empty-cells",
    "hyphens",
    "tab-size",
    "text-align-last",
    "writing-mode",
    "pointer-events",
    "-webkit-text-fill-color",
    "-webkit-font-smoothing",
    "text-rendering",
];

fn is_inherited(prop: &str) -> bool {
    INHERITED_PROPERTIES.contains(&prop)
}

/// Reserved key under which an element's custom properties are stored.
pub const CUSTOM_PROPS_KEY: &str = "--";

/// Maximum recursion depth for `var()` resolution, to prevent infinite loops from
/// cyclic custom property references (e.g. `--a: var(--b); --b: var(--a)`).
const VAR_RESOLVE_MAX_DEPTH: u32 = 32;

/// Maximum total length (in bytes) a single `var()`-substituted value may grow
/// to. Without this, a chain like `--a: var(--b) var(--b); --b: var(--c)
/// var(--c); ...` doubles in size at every level and can exhaust memory well
/// before `VAR_RESOLVE_MAX_DEPTH` is reached. Exceeding it makes the
/// declaration invalid at computed-value time, matching how an unresolvable
/// `var()` is already handled.
const VAR_SUBSTITUTE_MAX_LEN: usize = 64 * 1024;

/// Substitute every `var(--name[, fallback])` in `text` using `custom`.
/// Returns `None` when a referenced property is missing and has no fallback
/// (the declaration is then invalid at computed-value time), or when the
/// substituted result would exceed `VAR_SUBSTITUTE_MAX_LEN`.
fn substitute_vars(text: &str, custom: &HashMap<Arc<str>, Value>, depth: u32) -> Option<String> {
    if depth > VAR_RESOLVE_MAX_DEPTH {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("var(") {
        // Make sure "var(" is not the tail of another identifier.
        let prev_ok = pos == 0 || !rest[..pos].chars().last().map_or(false, |c| c.is_alphanumeric() || c == '-' || c == '_');
        out.push_str(&rest[..pos]);
        if !prev_ok {
            out.push_str("var(");
            rest = &rest[pos + 4..];
            continue;
        }
        let args_start = pos + 4;
        // find matching paren
        let bytes = rest.as_bytes();
        let mut depth_p = 1;
        let mut j = args_start;
        while j < bytes.len() {
            match bytes[j] {
                b'(' => depth_p += 1,
                b')' => {
                    depth_p -= 1;
                    if depth_p == 0 { break; }
                }
                _ => {}
            }
            j += 1;
        }
        let args = &rest[args_start..j.min(rest.len())];
        let (name, fallback) = match crate::css::split_top_level(args, b',').as_slice() {
            [n] => (n.trim().to_string(), None),
            [n, ..] => {
                let comma = args.find(',').unwrap_or(args.len());
                (n.trim().to_string(), Some(args[comma + 1..].to_string()))
            }
            [] => (String::new(), None),
        };
        // A custom property that is *registered* (present in `custom`, even with an
        // empty value from `--x: ;`) always substitutes its own value; the fallback
        // is only used when the property is not registered at all.
        let replacement = match custom.get(name.as_str()) {
            Some(Value::RawCustomProp(raw)) => substitute_vars(raw, custom, depth + 1),
            Some(other) => Some(value_to_css_text(other)),
            None => match fallback {
                Some(fb) => substitute_vars(fb.trim(), custom, depth + 1),
                None => None,
            },
        }?;
        out.push_str(&replacement);
        if out.len() > VAR_SUBSTITUTE_MAX_LEN {
            return None;
        }
        rest = if j < rest.len() { &rest[j + 1..] } else { "" };
    }
    out.push_str(rest);
    Some(out)
}

/// Best-effort serialization of a parsed value back to CSS text.
fn value_to_css_text(v: &Value) -> String {
    match v {
        Value::Keyword(k) => k.to_string(),
        Value::Length(n, u) => {
            let unit = match u {
                Unit::Px => "px",
                Unit::Percent => "%",
                Unit::Em => "em",
                Unit::Rem => "rem",
                Unit::Vw => "vw",
                Unit::Vh => "vh",
                Unit::Vmin => "vmin",
                Unit::Vmax => "vmax",
                Unit::Fr => "fr",
            };
            format!("{}{}", n, unit)
        }
        Value::Number(n) => format!("{}", n),
        Value::Color(c) => format!("rgba({},{},{},{})", c.r, c.g, c.b, c.a as f32 / 255.0),
        Value::RawCustomProp(s) => s.to_string(),
        _ => String::new(),
    }
}

/// Resolve `Value::CssVar` references using the provided custom properties map.
/// `depth` tracks recursion depth; returns `None` when the limit is reached.
fn resolve_var(value: &Value, custom_props: &HashMap<Arc<str>, Value>, depth: u32) -> Option<Value> {
    if depth > VAR_RESOLVE_MAX_DEPTH { return None; }
    if let Value::CssVar { name, fallback } = value {
        if let Some(raw) = custom_props.get(name) {
            if let Value::RawCustomProp(raw_str) = raw {
                let text = substitute_vars(raw_str, custom_props, depth + 1)?;
                // Re-parse the raw string as a CSS value at use time.
                return Some(crate::css::parse_value(&text));
            }
        }
        // Custom property not found — use fallback if present.
        if let Some(fb) = fallback {
            return match fb.as_ref() {
                v @ Value::CssVar { .. } => resolve_var(v, custom_props, depth + 1),
                v => Some(v.clone()),
            };
        }
        return None;
    }
    None
}

fn px_of(v: Option<&Value>) -> Option<f32> {
    match v {
        Some(Value::Length(n, Unit::Px)) => Some(*n),
        _ => None,
    }
}

/// Resolve em/rem inside a `calc()` keyword to px so layout can evaluate it.
fn absolutize_calc(expr: &str, em: f32, rem: f32) -> String {
    let mut out = String::with_capacity(expr.len());
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let starts_number = c.is_ascii_digit() || (c == '.' && chars.get(i + 1).map_or(false, |d| d.is_ascii_digit()));
        let prev_alpha = i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '-');
        if starts_number && !prev_alpha {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                j += 1;
            }
            let mut k = j;
            while k < chars.len() && chars[k].is_ascii_alphabetic() {
                k += 1;
            }
            let num: f32 = chars[i..j].iter().collect::<String>().parse().unwrap_or(0.0);
            let unit: String = chars[j..k].iter().collect();
            match unit.as_str() {
                "em" => out.push_str(&format!("{}px", num * em)),
                "rem" => out.push_str(&format!("{}px", num * rem)),
                _ => {
                    out.extend(chars[i..k].iter());
                }
            }
            i = k;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn font_weight_number(v: &Value, parent: f32) -> Option<f32> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Keyword(k) => match k.as_ref() {
            "normal" => Some(400.0),
            "bold" => Some(700.0),
            "bolder" => Some(if parent < 350.0 { 400.0 } else if parent < 550.0 { 700.0 } else { 900.0 }),
            "lighter" => Some(if parent < 550.0 { 100.0 } else if parent < 750.0 { 400.0 } else { 700.0 }),
            _ => None,
        },
        Value::Length(n, _) => Some(*n),
        _ => None,
    }
}

/// Compute the final values of one node from its cascaded (declared) values.
fn compute_values(
    mut specified_values: HashMap<Arc<str>, Value>,
    parent_pm: Option<&PropertyMap>,
    root_fs: f32,
    is_text: bool,
) -> HashMap<Arc<str>, Value> {
    // --- Step 1: custom properties (inherited) ---
    // All custom properties live in one shared map under the `--` key; an
    // element that declares none reuses its parent's map.
    let parent_custom: Option<Arc<HashMap<Arc<str>, Value>>> = parent_pm.and_then(|p| match p.get(CUSTOM_PROPS_KEY) {
        Some(Value::CustomProps(cp)) => Some(cp.0.clone()),
        _ => None,
    });
    let own_custom: Vec<(Arc<str>, Value)> = specified_values
        .iter()
        .filter(|(k, _)| k.starts_with("--") && k.as_ref() != CUSTOM_PROPS_KEY)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (k, _) in &own_custom {
        specified_values.remove(k);
    }
    let custom_arc: Option<Arc<HashMap<Arc<str>, Value>>> = if own_custom.is_empty() {
        parent_custom.clone()
    } else {
        let mut custom_props: HashMap<Arc<str>, Value> = parent_custom.as_deref().cloned().unwrap_or_default();
        for (k, v) in &own_custom {
            custom_props.insert(k.clone(), v.clone());
        }
        // Resolve var() references inside this element's own custom properties.
        for (k, v) in &own_custom {
            if let Value::RawCustomProp(raw) = v {
                if raw.contains("var(") {
                    match substitute_vars(raw, &custom_props, 0) {
                        Some(text) => {
                            custom_props.insert(k.clone(), Value::RawCustomProp(intern(&text)));
                        }
                        None => match parent_custom.as_ref().and_then(|p| p.get(k)) {
                            Some(pv) => {
                                custom_props.insert(k.clone(), pv.clone());
                            }
                            None => {
                                custom_props.remove(k);
                            }
                        },
                    }
                }
            }
        }
        Some(Arc::new(custom_props))
    };
    let empty_custom: HashMap<Arc<str>, Value> = HashMap::new();
    let custom_props: &HashMap<Arc<str>, Value> = custom_arc.as_deref().unwrap_or(&empty_custom);
    if let Some(ref arc) = custom_arc {
        specified_values.insert(intern(CUSTOM_PROPS_KEY), Value::CustomProps(crate::css::CustomProps(arc.clone())));
    }

    // --- Step 2: substitute var() in ordinary declarations ---
    let pending: Vec<(Arc<str>, Value)> = specified_values
        .iter()
        .filter(|(k, v)| !k.starts_with("--") && matches!(v, Value::RawCustomProp(_) | Value::CssVar { .. }))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (k, v) in pending {
        specified_values.remove(&k);
        match v {
            Value::RawCustomProp(raw) => {
                if let Some(text) = substitute_vars(&raw, custom_props, 0) {
                    let mut decls = Vec::new();
                    crate::css::parse_declaration(&k, &text, false, &mut decls);
                    for d in decls {
                        if d.name == k || !specified_values.contains_key(&d.name) {
                            specified_values.insert(d.name, d.value);
                        }
                    }
                }
            }
            v @ Value::CssVar { .. } => {
                if let Some(r) = resolve_var(&v, custom_props, 0) {
                    specified_values.insert(k, r);
                }
            }
            _ => {}
        }
    }

    // --- Step 3: Inherit inheritable properties (unless explicitly set) ---
    if let Some(p) = parent_pm {
        for prop in INHERITED_PROPERTIES {
            let prop_arc = intern(prop);
            if !specified_values.contains_key(&prop_arc) {
                if let Some(v) = p.get(&prop_arc) {
                    specified_values.insert(prop_arc, v.clone());
                }
            }
        }
        // Text decorations propagate to the text of inline descendants.
        if is_text {
            let td = intern("text-decoration");
            if let Some(v) = p.get(&td) {
                specified_values.insert(td, v.clone());
            }
        }
    }

    // --- Step 4: CSS-wide keywords ---
    let keys: Vec<Arc<str>> = specified_values.keys().filter(|k| !k.starts_with("--")).cloned().collect();
    for key in &keys {
        let action = match specified_values.get(key) {
            Some(Value::Keyword(kw)) => match kw.as_ref() {
                "inherit" => 1,
                "initial" => 2,
                "unset" => if is_inherited(key) { 1 } else { 2 },
                _ => 0,
            },
            _ => 0,
        };
        match action {
            1 => match parent_pm.and_then(|p| p.get(key)).cloned().or_else(|| initial_value(key)) {
                Some(v) => { specified_values.insert(key.clone(), v); }
                None => { specified_values.remove(key); }
            },
            2 => match initial_value(key) {
                Some(v) => { specified_values.insert(key.clone(), v); }
                None => { specified_values.remove(key); }
            },
            _ => {}
        }
    }

    // --- Step 5: font-size (needs parent font-size) ---
    let fs_key = intern("font-size");
    let parent_fs = parent_pm.and_then(|p| px_of(p.get(&fs_key))).unwrap_or(16.0);
    if let Some(val) = specified_values.get(&fs_key).cloned() {
        let resolved_fs = match &val {
            Value::Length(v, Unit::Px) => Some(*v),
            Value::Length(v, Unit::Percent) => Some(parent_fs * (v / 100.0)),
            Value::Length(v, Unit::Em) => Some(parent_fs * v),
            Value::Length(v, Unit::Rem) => Some(root_fs * v),
            Value::Length(v, Unit::Vw) => Some(8.0 * v),
            Value::Length(v, Unit::Vh) => Some(7.68 * v),
            Value::Number(n) if *n == 0.0 => Some(0.0),
            Value::Keyword(kw) => match kw.as_ref() {
                "xx-small" => Some(9.0),
                "x-small" => Some(10.0),
                "small" => Some(13.0),
                "medium" => Some(16.0),
                "large" => Some(18.0),
                "x-large" => Some(24.0),
                "xx-large" => Some(32.0),
                "xxx-large" => Some(48.0),
                "smaller" => Some(parent_fs / 1.2),
                "larger" => Some(parent_fs * 1.2),
                k if k.starts_with("calc(") => {
                    crate::css::eval_calc(k, &mut |n, u| match u {
                        None | Some(Unit::Px) => Some(n),
                        Some(Unit::Em) | Some(Unit::Percent) => Some(if matches!(u, Some(Unit::Percent)) { parent_fs * n / 100.0 } else { parent_fs * n }),
                        Some(Unit::Rem) => Some(root_fs * n),
                        _ => None,
                    })
                }
                _ => None,
            },
            _ => None,
        };
        match resolved_fs {
            Some(fs) => { specified_values.insert(fs_key.clone(), Value::Length(fs, Unit::Px)); }
            None => { specified_values.insert(fs_key.clone(), Value::Length(parent_fs, Unit::Px)); }
        }
    }
    let own_fs = px_of(specified_values.get(&fs_key)).unwrap_or(parent_fs);

    // --- Step 6: color / currentColor ---
    let color_key = intern("color");
    let own_color = match specified_values.get(&color_key).cloned() {
        Some(Value::Keyword(kw)) if kw.as_ref() == "currentcolor" => parent_pm.and_then(|p| p.get(&color_key)).cloned(),
        Some(Value::Color(c)) => Some(Value::Color(c)),
        Some(_) => parent_pm.and_then(|p| p.get(&color_key)).cloned().or_else(|| initial_value("color")),
        None => parent_pm.and_then(|p| p.get(&color_key)).cloned(),
    };
    if let Some(ref c) = own_color {
        specified_values.insert(color_key.clone(), c.clone());
    }

    // --- Step 7: remaining relative values ---
    let keys: Vec<Arc<str>> = specified_values
        .keys()
        .filter(|k| k.as_ref() != "font-size" && k.as_ref() != "color" && !k.starts_with("--"))
        .cloned()
        .collect();
    for key in keys {
        let val = specified_values[&key].clone();
        let resolved = match &val {
            Value::Keyword(kw) if kw.as_ref() == "currentcolor" => own_color.clone(),
            Value::Length(n, Unit::Em) => Some(Value::Length(n * own_fs, Unit::Px)),
            Value::Length(n, Unit::Rem) => Some(Value::Length(n * root_fs, Unit::Px)),
            Value::Length(n, Unit::Vmin) => Some(Value::Length(*n, Unit::Vh)),
            Value::Length(n, Unit::Vmax) => Some(Value::Length(*n, Unit::Vw)),
            Value::Length(n, Unit::Percent) if key.as_ref() == "line-height" => {
                Some(Value::Length(own_fs * n / 100.0, Unit::Px))
            }
            Value::Keyword(kw) if kw.starts_with("calc(") && (kw.contains("em") || kw.contains("rem")) => {
                let text = absolutize_calc(kw, own_fs, root_fs);
                Some(crate::css::parse_value(&text))
            }
            _ => None,
        };
        if let Some(r) = resolved {
            specified_values.insert(key, r);
        }
    }

    // opacity: out-of-range values clamp to [0, 1] (percentages become numbers).
    let op_key = intern("opacity");
    match specified_values.get(&op_key) {
        Some(Value::Number(n)) if !(0.0..=1.0).contains(n) => {
            let n = n.clamp(0.0, 1.0);
            specified_values.insert(op_key, Value::Number(n));
        }
        Some(Value::Length(n, Unit::Percent)) => {
            let n = (n / 100.0).clamp(0.0, 1.0);
            specified_values.insert(op_key, Value::Number(n));
        }
        _ => {}
    }

    // font-weight: compute to a number (CSS Fonts 4), resolving bolder/lighter
    // against the parent; the font matcher picks the nearest face weight.
    let fw_key = intern("font-weight");
    if let Some(v) = specified_values.get(&fw_key).cloned() {
        let parent_w = parent_pm
            .and_then(|p| p.get(&fw_key))
            .and_then(|pv| font_weight_number(pv, 400.0))
            .unwrap_or(400.0);
        if let Some(w) = font_weight_number(&v, parent_w) {
            specified_values.insert(fw_key, Value::Number(w.clamp(1.0, 1000.0)));
        }
    }

    specified_values
}

fn build_final_tree(
    root: &Handle,
    arena_idx: &mut usize,
    raw_styles: &mut [Cascaded],
    arena: &[NodeDataSend],
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

    enum Frame {
        Pre {
            handle: Handle,
            parent_pm: Option<PropertyMap>,
        },
        Post {
            handle: Handle,
            specified_values: PropertyMap,
            num_children: usize,
            before: Option<StyledNode>,
            after: Option<StyledNode>,
            placeholder: Option<StyledNode>,
        },
    }

    let mut work: Vec<Frame> = vec![Frame::Pre {
        handle: root.clone(),
        parent_pm: initial_parent_style.cloned(),
    }];
    let mut results: Vec<StyledNode> = Vec::new();
    let mut root_fs: f32 = 16.0;

    while let Some(frame) = work.pop() {
        match frame {
            Frame::Pre { handle, parent_pm } => {
                // Read the current sequential index BEFORE incrementing.
                let current_idx = *arena_idx;
                *arena_idx += 1;

                let (declared, before_decl, after_decl, placeholder_decl) = std::mem::take(&mut raw_styles[current_idx]);
                let mut declared = declared;
                if parent_pm.is_none() && current_idx == 0 {
                    declared.entry(intern("color")).or_insert_with(|| Value::Color(crate::css::Color { r: 0, g: 0, b: 0, a: 255 }));
                    declared.entry(intern("font-size")).or_insert_with(|| Value::Length(16.0, crate::css::Unit::Px));
                }
                let is_text = matches!(handle.data, NodeData::Text { .. });
                let computed = compute_values(declared, parent_pm.as_ref(), root_fs, is_text);
                if arena[current_idx].tag == "html" {
                    if let Some(fs) = px_of(computed.get("font-size")) {
                        root_fs = fs;
                    }
                }
                let interned_map = store.intern(computed);

                let make_pseudo = |decl: Option<HashMap<Arc<str>, Value>>, kind: &str| -> Option<StyledNode> {
                    let decl = decl?;
                    let pmap = compute_values(decl, Some(&interned_map), root_fs, false);
                    let content = pseudo_content(pmap.get("content"), &arena[current_idx])?;
                    if matches!(pmap.get("display"), Some(Value::Keyword(k)) if k.as_ref() == "none") {
                        return None;
                    }
                    Some(make_pseudo_styled_node(content, pmap, kind))
                };
                let before = if matches!(handle.data, NodeData::Element { .. }) { make_pseudo(before_decl, "::before") } else { None };
                let after = if matches!(handle.data, NodeData::Element { .. }) { make_pseudo(after_decl, "::after") } else { None };
                // `::placeholder` is shown while the control's value is empty.
                let placeholder = placeholder_decl.and_then(|decl| {
                    let text = placeholder_text(&arena[current_idx], arena)?;
                    let pmap = compute_values(decl, Some(&interned_map), root_fs, false);
                    if matches!(pmap.get("display"), Some(Value::Keyword(k)) if k.as_ref() == "none") {
                        return None;
                    }
                    Some(make_pseudo_styled_node(text, pmap, "::placeholder"))
                });

                let children_handles: Vec<Handle> = handle.children.borrow().iter().cloned().collect();
                let num_children = children_handles.len();

                // Push Post FIRST — it will be processed only after ALL descendants finish.
                work.push(Frame::Post {
                    handle,
                    specified_values: interned_map.clone(),
                    num_children,
                    before,
                    after,
                    placeholder,
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
            Frame::Post { handle, specified_values, num_children, before, after, placeholder } => {
                // Children have all been processed and pushed onto `results`.
                // Drain the last num_children entries — they are in forward order
                // because children were pushed in reverse (LIFO gives forward order).
                let start = results.len().saturating_sub(num_children);
                let mut children: Vec<StyledNode> = Vec::with_capacity(num_children + 3);
                if let Some(p) = placeholder { children.push(p); }
                if let Some(b) = before { children.push(b); }
                children.extend(results.drain(start..));
                if let Some(a) = after { children.push(a); }
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

fn apply_default_styles(tag: &str, node: &NodeDataSend, map: &mut HashMap<Arc<str>, Value>, quirks_mode: bool) {
    use crate::css::Unit::Px;
    let px = |v: f32| Value::Length(v, Px);
    let kw = |s: &str| Value::Keyword(intern(s));
    let set = |map: &mut HashMap<Arc<str>, Value>, k: &str, v: Value| { map.entry(intern(k)).or_insert(v); };
    let set_quad = |map: &mut HashMap<Arc<str>, Value>, prefix: &str, v: f32| {
        for side in ["top", "right", "bottom", "left"] {
            map.entry(intern(&format!("{}-{}", prefix, side))).or_insert(Value::Length(v, Px));
        }
    };
    let set_border = |map: &mut HashMap<Arc<str>, Value>, w: f32, style: &str, c: crate::css::Color| {
        map.entry(intern("border-width")).or_insert(Value::Length(w, Px));
        map.entry(intern("border-style")).or_insert(Value::Keyword(intern(style)));
        map.entry(intern("border-color")).or_insert(Value::Color(c.clone()));
        for side in ["top", "right", "bottom", "left"] {
            map.entry(intern(&format!("border-{}-width", side))).or_insert(Value::Length(w, Px));
            map.entry(intern(&format!("border-{}-style", side))).or_insert(Value::Keyword(intern(style)));
            map.entry(intern(&format!("border-{}-color", side))).or_insert(Value::Color(c.clone()));
        }
    };
    let gray = crate::css::Color { r: 180, g: 180, b: 180, a: 255 };
    match tag {
        "h1" => {
            set(map, "font-size", px(32.0));
            set(map, "font-weight", kw("bold"));
            set(map, "margin-top", px(21.0));
            set(map, "margin-bottom", px(21.0));
        }
        "h2" => {
            set(map, "font-size", px(24.0));
            set(map, "font-weight", kw("bold"));
            set(map, "margin-top", px(14.0));
            set(map, "margin-bottom", px(14.0));
        }
        "h3" => {
            set(map, "font-size", px(18.0));
            set(map, "font-weight", kw("bold"));
        }
        "h4" | "h5" | "h6" => {
            set(map, "font-weight", kw("bold"));
        }
        "a" => {
            set(map, "color", Value::Color(parse_color("#0000ee").unwrap()));
            set(map, "text-decoration", kw("underline"));
        }
        "strong" | "b" => {
            set(map, "font-weight", kw("bold"));
        }
        "em" | "i" | "cite" | "var" | "dfn" | "address" => {
            set(map, "font-style", kw("italic"));
        }
        "code" | "pre" | "kbd" | "samp" => {
            set(map, "font-family", kw("monospace"));
            set(map, "background-color", Value::Color(crate::css::Color { r: 240, g: 240, b: 240, a: 255 }));
            if tag == "pre" {
                set(map, "white-space", kw("pre"));
            }
        }
        "input" => {
            let input_type = attr_value(node, "type").unwrap_or("text").to_ascii_lowercase();
            if matches!(input_type.as_str(), "checkbox" | "radio") {
                set(map, "width", px(13.0));
                set(map, "height", px(13.0));
                set_quad(map, "margin", 3.0);
                return;
            }
            set_border(map, 1.0, "solid", gray);
            set(map, "background-color", Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            set_quad(map, "padding", 4.0);
            if matches!(input_type.as_str(), "submit" | "button" | "reset" | "image") {
                set(map, "box-sizing", kw("border-box"));
            } else {
                // UA defaults: HTML spec §14.3 — <input> default size = 20 chars ≈ 160 px at 13 px font.
                set(map, "width", px(160.0));
                // Real browsers render single-line inputs at ~21 px; use 24 for legibility.
                set(map, "height", px(24.0));
            }
        }
        "textarea" => {
            set_border(map, 1.0, "solid", gray);
            set(map, "background-color", Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            set_quad(map, "padding", 4.0);
            // UA defaults: cols=20, rows=2 → ~160 × 48 px.
            set(map, "width", px(160.0));
            set(map, "height", px(48.0));
            set(map, "white-space", kw("pre-wrap"));
        }
        "select" => {
            set_border(map, 1.0, "solid", gray);
            set(map, "background-color", Value::Color(crate::css::Color { r: 255, g: 255, b: 255, a: 255 }));
            set_quad(map, "padding", 4.0);
            set(map, "width", px(120.0));
            set(map, "height", px(24.0));
            set(map, "box-sizing", kw("border-box"));
        }
        "button" => {
            set_border(map, 1.0, "solid", gray);
            set(map, "background-color", Value::Color(crate::css::Color { r: 240, g: 240, b: 240, a: 255 }));
            set_quad(map, "padding", 4.0);
            set(map, "box-sizing", kw("border-box"));
            set(map, "text-align", kw("center"));
            // Width is content-driven (shrink-wrap in layout); enforce a minimum height.
            set(map, "min-height", px(24.0));
        }
        // <center> is a legacy presentational element — UA default maps it to a block
        // with text-align: center, matching browsers' built-in stylesheet.
        "center" => {
            set(map, "display", kw("block"));
            set(map, "text-align", kw("center"));
        }
        "ul" | "ol" | "menu" | "dir" => {
            set(map, "padding-left", px(40.0));
            set(map, "list-style-type", kw(if tag == "ol" { "decimal" } else { "disc" }));
        }
        "dd" => {
            set(map, "margin-left", px(40.0));
        }
        "blockquote" | "figure" => {
            set(map, "margin-left", px(40.0));
            set(map, "margin-right", px(40.0));
            set(map, "margin-top", px(16.0));
            set(map, "margin-bottom", px(16.0));
        }
        "p" => {
            set(map, "margin-top", px(8.0));
            set(map, "margin-bottom", px(8.0));
        }
        "td" | "th" => {
            set_quad(map, "padding", 1.0);
            if tag == "th" {
                set(map, "font-weight", kw("bold"));
            }
        }
        "table" => {
            set(map, "border-spacing", px(2.0));
            // In quirks mode, Chromium's quirks UA sheet resets `table` so it
            // does not inherit alignment/text properties from an ancestor
            // like `<center>` (real-world case: Hacker News wraps its story
            // table in `<center>` with no doctype). `color` is left alone —
            // the spec's `-internal-quirk-inherit` for it means "still
            // inherit normally", which is what happens if we don't set it.
            if quirks_mode {
                set(map, "text-align", kw("start"));
                set(map, "white-space", kw("normal"));
                set(map, "line-height", kw("normal"));
                set(map, "font-weight", kw("normal"));
                set(map, "font-size", kw("medium"));
                set(map, "font-variant", kw("normal"));
                set(map, "font-style", kw("normal"));
            }
        }
        "hr" => {
            set_border(map, 1.0, "inset", crate::css::Color { r: 238, g: 238, b: 238, a: 255 });
            set(map, "margin-top", px(8.0));
            set(map, "margin-bottom", px(8.0));
        }
        "sub" | "sup" => {
            set(map, "vertical-align", kw(tag));
            set(map, "font-size", Value::Length(83.0, crate::css::Unit::Percent));
        }
        "small" => {
            set(map, "font-size", kw("smaller"));
        }
        "big" => {
            set(map, "font-size", kw("larger"));
        }
        "u" | "ins" => {
            set(map, "text-decoration", kw("underline"));
        }
        "s" | "strike" | "del" => {
            set(map, "text-decoration", kw("line-through"));
        }
        "nobr" => {
            set(map, "white-space", kw("nowrap"));
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

// ── Pseudo-element generation ─────────────────────────────────────────────────

/// Decode the `content` property into the generated text, or `None` when no
/// box is generated (`none`, `normal`, missing).
fn pseudo_content(content: Option<&Value>, element: &NodeDataSend) -> Option<String> {
    let raw = match content? {
        Value::Keyword(k) => k.to_string(),
        _ => return None,
    };
    let raw = raw.trim();
    if raw == "none" || raw == "normal" || raw.is_empty() {
        return None;
    }
    let mut out = String::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' {
            let q = c;
            i += 1;
            while i < chars.len() && chars[i] != q {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    // CSS escape: hex code point or literal char
                    let mut j = i + 1;
                    let mut hex = String::new();
                    while j < chars.len() && hex.len() < 6 && chars[j].is_ascii_hexdigit() {
                        hex.push(chars[j]);
                        j += 1;
                    }
                    if !hex.is_empty() {
                        if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            out.push(ch);
                        }
                        if j < chars.len() && chars[j] == ' ' {
                            j += 1;
                        }
                        i = j;
                    } else {
                        out.push(chars[i + 1]);
                        i += 2;
                    }
                    continue;
                }
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Function or keyword token
        let mut j = i;
        while j < chars.len() && !chars[j].is_whitespace() && chars[j] != '(' {
            j += 1;
        }
        let word: String = chars[i..j].iter().collect::<String>().to_ascii_lowercase();
        if j < chars.len() && chars[j] == '(' {
            let mut depth = 0;
            let mut k = j;
            while k < chars.len() {
                if chars[k] == '(' { depth += 1; }
                if chars[k] == ')' {
                    depth -= 1;
                    if depth == 0 { break; }
                }
                k += 1;
            }
            let args: String = chars[j + 1..k.min(chars.len())].iter().collect();
            if word == "attr" {
                if let Some(v) = attr_value(element, args.trim()) {
                    out.push_str(v);
                }
            }
            // url(), counter(), counters(): no text.
            i = k + 1;
            continue;
        }
        match word.as_str() {
            "open-quote" => out.push('\u{201C}'),
            "close-quote" => out.push('\u{201D}'),
            _ => {}
        }
        i = j;
    }
    Some(out)
}

/// Build a synthetic element `StyledNode` for a `::before` / `::after` box, with
/// a text child carrying the generated content (when non-empty).
fn make_pseudo_styled_node(
    content_text: String,
    values: HashMap<Arc<str>, Value>,
    kind: &str,
) -> StyledNode {
    use html5ever::tendril::StrTendril;
    use markup5ever_rcdom::Node;

    let element = Node::new(NodeData::Element {
        name: html5ever::QualName::new(None, html5ever::ns!(html), html5ever::LocalName::from(kind)),
        attrs: std::cell::RefCell::new(Vec::new()),
        template_contents: std::cell::RefCell::new(None),
        mathml_annotation_xml_integration_point: false,
    });
    let values = PropertyMap(Arc::new(values));
    let mut children = Vec::new();
    if !content_text.is_empty() {
        let text_handle = Node::new(NodeData::Text {
            contents: std::cell::RefCell::new(StrTendril::from(content_text.as_str())),
        });
        let text_values = compute_values(HashMap::new(), Some(&values), 16.0, true);
        children.push(StyledNode {
            node: text_handle,
            specified_values: PropertyMap(Arc::new(text_values)),
            children: Vec::new(),
        });
    }
    StyledNode { node: element, specified_values: values, children }
}

/// Placeholder text an `<input>`/`<textarea>` shows right now: its non-empty
/// `placeholder` attribute while the value is empty (line breaks removed).
fn placeholder_text(node: &NodeDataSend, arena: &[NodeDataSend]) -> Option<String> {
    let text = attr_value(node, "placeholder")?;
    match node.tag.as_str() {
        "input" => {
            let ty = attr_value(node, "type").unwrap_or("text").to_ascii_lowercase();
            let text_like = matches!(
                ty.as_str(),
                "text" | "search" | "email" | "url" | "tel" | "password" | "number" | ""
            );
            if !text_like || attr_value(node, "value").is_some_and(|v| !v.is_empty()) {
                return None;
            }
        }
        "textarea" => {
            if node.children_idx.iter().any(|&c| arena[c].has_text) {
                return None;
            }
        }
        _ => return None,
    }
    let text: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    if text.is_empty() { None } else { Some(text) }
}

/// Returns true when this styled node is a generated `::before` / `::after` box.
pub fn is_pseudo_element(node: &StyledNode) -> bool {
    matches!(&node.node.data, NodeData::Element { name, .. } if name.local.starts_with("::"))
}

pub fn parse_inline_style_into_vec(style_str: &str, list: &mut Vec<crate::css::Declaration>) {
    list.extend(crate::css::parse_declaration_block(style_str));
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
        // inherit on root → initial value for font-weight is normal (400)
        let fw = html.specified_values.get(&intern("font-weight"));
        assert!(fw.is_none() || fw == Some(&Value::Number(400.0)), "got {:?}", fw);
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
    fn test_var_empty_custom_property_does_not_use_fallback() {
        // `--accent: ;` is a valid, explicitly-empty custom property. Per spec,
        // `var(--accent, 999px)` must substitute the empty value, not the
        // fallback `999px` — so `width` must NOT end up as 999px (it becomes
        // an invalid, non-Length value instead, since `width: ` is empty).
        let tree = make_tree(
            r#"<html><body><p>text</p></body></html>"#,
            "html { --accent: ; } p { width: var(--accent, 999px); }",
        );
        let p = find_node(&tree, "p").expect("p not found");
        assert_ne!(
            get_length_px(p, "width"),
            Some(999.0),
            "empty --accent must not fall back to 999px"
        );
    }

    // --- quirks mode ---

    #[test]
    fn test_quirks_mode_table_does_not_inherit_center_alignment() {
        // Hacker News has no doctype and wraps its story table in `<center>`.
        // Chromium's quirks-mode UA sheet resets `table { text-align: start; ... }`
        // so the table (and its cells) don't inherit the `<center>` alignment.
        // Without a doctype, this document is in quirks mode.
        let tree = make_tree(
            r#"<center><table><tr><td id="cell">text</td></tr></table></center>"#,
            "",
        );
        let table = find_node(&tree, "table").expect("table not found");
        assert_eq!(
            get_keyword(table, "text-align").as_deref(),
            Some("start"),
            "quirks-mode table must reset text-align to start, not inherit center"
        );
    }

    #[test]
    fn test_standards_mode_table_inherits_center_alignment() {
        // The same markup, but with a doctype: standards mode. No reset
        // applies, so the table inherits `text-align: center` from `<center>`
        // normally.
        let tree = make_tree(
            r#"<!DOCTYPE html><html><body><center><table><tr><td id="cell">text</td></tr></table></center></body></html>"#,
            "",
        );
        let table = find_node(&tree, "table").expect("table not found");
        assert_eq!(
            get_keyword(table, "text-align").as_deref(),
            Some("center"),
            "standards-mode table must inherit center alignment normally"
        );
    }

    #[test]
    fn test_var_fanout_length_is_capped() {
        // `--a` doubles `--b` each level; without a total-length cap this
        // explodes exponentially. It must resolve to invalid-at-computed-value
        // (no color) instead of hanging or exhausting memory.
        let css = "html { \
            --z: red; \
            --y: var(--z) var(--z) var(--z) var(--z) var(--z) var(--z) var(--z) var(--z); \
            --x: var(--y) var(--y) var(--y) var(--y) var(--y) var(--y) var(--y) var(--y); \
            --w: var(--x) var(--x) var(--x) var(--x) var(--x) var(--x) var(--x) var(--x); \
            --v: var(--w) var(--w) var(--w) var(--w) var(--w) var(--w) var(--w) var(--w); \
            --u: var(--v) var(--v) var(--v) var(--v) var(--v) var(--v) var(--v) var(--v); \
            --t: var(--u) var(--u) var(--u) var(--u) var(--u) var(--u) var(--u) var(--u); \
        } p { color: var(--t); }";
        let tree = make_tree(r#"<html><body><p>text</p></body></html>"#, css);
        let p = find_node(&tree, "p").expect("p not found");
        // 8^6 = ~262144 "red " repetitions — far past the 64 KiB cap. If the cap
        // did not kick in, `color` would resolve to `red`. Instead the
        // declaration must be treated as invalid at computed-value time, which
        // for the inherited `color` property means falling back to the
        // inherited/initial value (black here, since there is no ancestor
        // `color` declaration) rather than `red` — and this must not hang or
        // blow the stack.
        assert_ne!(
            get_color(p, "color"),
            Some(Color { r: 255, g: 0, b: 0, a: 255 }),
            "fanned-out var() chain must be capped, not resolved to red"
        );
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
            // Generated content lives in a synthetic ::before/::after element.
            if is_pseudo_element(child) {
                for grandchild in &child.children {
                    if let markup5ever_rcdom::NodeData::Text { ref contents } = grandchild.node.data {
                        if contents.borrow().as_ref() == text { return true; }
                    }
                }
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

    // --- cascade order, selectors, var() substitution ---

    #[test]
    fn test_equal_specificity_later_rule_wins() {
        // Same specificity: source order decides, regardless of class order on the element.
        let tree = make_tree(
            r#"<html><body><p class="b a">text</p></body></html>"#,
            ".a { color: red; } .b { color: blue; }",
        );
        let p = find_node(&tree, "p").unwrap();
        assert_eq!(get_color(p, "color"), Some(Color { r: 0, g: 0, b: 255, a: 255 }));
    }

    #[test]
    fn test_structural_pseudo_classes_match() {
        let tree = make_tree(
            r#"<html><body><ul><li>1</li><li class="x">2</li><li>3</li></ul></body></html>"#,
            "li:first-child { width: 1px; } li:last-child { width: 3px; } li:nth-child(2) { height: 2px; } li:not(.x) { margin-top: 7px; }",
        );
        let ul = find_node(&tree, "ul").unwrap();
        let lis: Vec<&StyledNode> = ul.children.iter().filter(|c| matches!(&c.node.data, NodeData::Element { .. })).collect();
        assert_eq!(get_length_px(lis[0], "width"), Some(1.0));
        assert_eq!(get_length_px(lis[2], "width"), Some(3.0));
        assert_eq!(get_length_px(lis[1], "height"), Some(2.0));
        assert_eq!(get_length_px(lis[0], "margin-top"), Some(7.0));
        assert_eq!(get_length_px(lis[1], "margin-top"), None);
    }

    #[test]
    fn test_attribute_operator_selectors_match() {
        let tree = make_tree(
            r#"<html data-theme="greenLight"><body><a href="https://naver.com/x" class="link_a">x</a></body></html>"#,
            "html[data-theme=greenDark] a { width: 1px; } html[data-theme=greenLight] a { height: 2px; } a[href^='https'] { margin-top: 3px; } a[class*=nk_] { margin-left: 4px; }",
        );
        let a = find_node(&tree, "a").unwrap();
        assert_eq!(get_length_px(a, "width"), None);
        assert_eq!(get_length_px(a, "height"), Some(2.0));
        assert_eq!(get_length_px(a, "margin-top"), Some(3.0));
        assert_eq!(get_length_px(a, "margin-left"), Some(4.0));
    }

    #[test]
    fn test_universal_selector_applies() {
        let tree = make_tree(r#"<html><body><div>x</div></body></html>"#, "* { box-sizing: border-box; }");
        let div = find_node(&tree, "div").unwrap();
        assert_eq!(get_keyword(div, "box-sizing").as_deref(), Some("border-box"));
    }

    #[test]
    fn test_rem_resolves_against_root_font_size() {
        let tree = make_tree(
            r#"<html><body><div><p>x</p></div></body></html>"#,
            "html { font-size: 10.5px; } div { font-size: 30px; } p { font-size: 1.4rem; margin-top: 2rem; }",
        );
        let p = find_node(&tree, "p").unwrap();
        assert!((get_length_px(p, "font-size").unwrap() - 14.7).abs() < 0.01);
        assert!((get_length_px(p, "margin-top").unwrap() - 21.0).abs() < 0.01);
    }

    #[test]
    fn test_var_inside_shorthand_expands_after_substitution() {
        let tree = make_tree(
            r#"<html><body><div>x</div></body></html>"#,
            ":root { --gap: 12px; --line: #ff0000; } div { padding: 0 var(--gap); border-bottom: 1px solid var(--line); padding-left: 3px; }",
        );
        let div = find_node(&tree, "div").unwrap();
        assert_eq!(get_length_px(div, "padding-right"), Some(12.0));
        // A later longhand beats the var() shorthand.
        assert_eq!(get_length_px(div, "padding-left"), Some(3.0));
        assert_eq!(get_color(div, "border-bottom-color"), Some(Color { r: 255, g: 0, b: 0, a: 255 }));
    }

    #[test]
    fn test_var_resolved_in_box_shadow_layers_and_gradient() {
        let tree = make_tree(
            r#"<html><body><div>x</div></body></html>"#,
            ":root { --stroke: #e3e5e8; --a: #ffffff; --b: #000000; } div { box-shadow: 0 0 0 1px var(--stroke), 0 1px 2px rgba(0,0,0,.04); background-image: linear-gradient(var(--a), var(--b)); }",
        );
        let div = find_node(&tree, "div").unwrap();
        match div.specified_values.get(&intern("box-shadow-layers")) {
            Some(Value::BoxShadowList(layers)) => {
                assert_eq!(layers.len(), 2);
                assert_eq!(layers[0].color, Color { r: 0xe3, g: 0xe5, b: 0xe8, a: 255 });
            }
            other => panic!("expected box-shadow-layers list, got {:?}", other),
        }
        match div.specified_values.get(&intern("background-image")) {
            Some(Value::Gradient(crate::css::GradientValue::Linear { stops, .. })) => {
                assert_eq!(stops.len(), 2);
                assert_eq!(stops[0].color, Color { r: 255, g: 255, b: 255, a: 255 });
            }
            other => panic!("expected gradient, got {:?}", other),
        }
    }

    #[test]
    fn test_pseudo_element_respects_ancestor_selector() {
        // `.on .x::before` must not generate a box for `.x` outside `.on`.
        let tree = make_tree(
            r#"<html><body><div class="off"><span class="x">a</span></div></body></html>"#,
            r#".on .x::before { content: ""; display: block; }"#,
        );
        let span = find_node(&tree, "span").unwrap();
        assert!(!span.children.iter().any(is_pseudo_element));
    }

    #[test]
    fn test_pseudo_element_is_element_box_with_own_style() {
        let tree = make_tree(
            r#"<html><body><div class="cf">content</div></body></html>"#,
            r#".cf::after { content: ""; display: table; clear: both; width: 5px; }"#,
        );
        let div = find_node(&tree, "div").unwrap();
        let after = div.children.last().unwrap();
        assert!(is_pseudo_element(after));
        assert_eq!(get_length_px(after, "width"), Some(5.0));
        assert!(after.children.is_empty(), "empty content generates no text child");
    }

    #[test]
    fn test_url_with_at_sign_does_not_break_following_rules() {
        let tree = make_tree(
            r#"<html><body><p class="a">x</p></body></html>"#,
            ".z { background: url(https://x/@bg_1.png) 0 0 no-repeat #fff; } .a { color: rgb(1, 2, 3); }",
        );
        let p = find_node(&tree, "p").unwrap();
        assert_eq!(get_color(p, "color"), Some(Color { r: 1, g: 2, b: 3, a: 255 }));
    }

    #[test]
    fn test_hidden_attribute_sets_display_none() {
        let tree = make_tree(r#"<html><body><div hidden>x</div></body></html>"#, "");
        let div = find_node(&tree, "div").unwrap();
        assert_eq!(get_keyword(div, "display").as_deref(), Some("none"));
    }

    #[test]
    fn test_opacity_computes_clamped_number() {
        let tree = make_tree(
            r#"<html><body><p style="opacity:-56.81">x</p><i style="opacity:40%">y</i></body></html>"#,
            "",
        );
        let op = |tag: &str| find_node(&tree, tag).unwrap().specified_values.get(&intern("opacity")).cloned();
        assert_eq!(op("p"), Some(Value::Number(0.0)));
        assert_eq!(op("i"), Some(Value::Number(0.4)));
    }

    #[test]
    fn test_font_weight_computes_to_number() {
        let tree = make_tree(
            r#"<html><body><p>x<b>y</b></p><i>z</i></body></html>"#,
            "body { font-weight: 500 } p { font-weight: 800; } i { font-weight: bolder }",
        );
        let fw = |tag: &str| find_node(&tree, tag).unwrap().specified_values.get(&intern("font-weight")).cloned();
        assert_eq!(fw("p"), Some(Value::Number(800.0)));
        assert_eq!(fw("body"), Some(Value::Number(500.0)));
        // UA `b { font-weight: bold }` is 700; `bolder` from 500 is 700.
        assert_eq!(fw("b"), Some(Value::Number(700.0)));
        assert_eq!(fw("i"), Some(Value::Number(700.0)));
    }

    #[test]
    fn test_parse_selector_pseudo_class_unchanged() {
        use crate::css::parse_selector;
        let s = parse_selector("a:hover");
        assert_eq!(s.tag, Some("a".to_string()));
        assert_eq!(s.pseudo_class, Some("hover".to_string()));
        assert!(s.pseudo_element.is_none());
    }

    fn transform_of(node: &StyledNode) -> Option<Value> {
        node.specified_values.get(&intern("transform")).cloned()
    }

    #[test]
    fn test_finished_animation_with_fill_forwards_keeps_end_keyframe() {
        // Keyframes names are case-sensitive (CSS Modules hashes mix case).
        let css = ".r{width:100%;animation-fill-mode:forwards;animation-name:Roll___2LIf-}\
                   @keyframes Roll___2LIf-{0%{transform:none}to{transform:translateY(-100%)}}";
        let tree = make_tree(r#"<div class="r" style="animation-duration: .5s;">x</div>"#, css);
        let div = find_node(&tree, "div").unwrap();
        assert!(matches!(transform_of(div), Some(Value::Transform(_))), "got {:?}", transform_of(div));
    }

    #[test]
    fn test_animation_without_fill_or_infinite_keeps_base_style() {
        let css = "@keyframes fade{from{opacity:0}to{opacity:0.5}}\
                   .a{animation:fade 1s} .b{animation:fade 1s infinite forwards} .c{animation:fade 1s forwards}";
        let tree = make_tree(r#"<p class="a">a</p><span class="b">b</span><i class="c">c</i>"#, css);
        let op = |tag: &str| find_node(&tree, tag).unwrap().specified_values.get(&intern("opacity")).cloned();
        assert_eq!(op("p"), None);
        assert_eq!(op("span"), None);
        assert_eq!(op("i"), Some(Value::Number(0.5)));
    }

    #[test]
    fn test_animation_direction_and_important() {
        let css = "@keyframes m{from{opacity:0.25}to{opacity:0.75}}\
                   .rev{animation:m 1s reverse forwards} .alt{animation:m 1s 2 alternate both}\
                   .imp{animation:m 1s forwards; opacity:1 !important}";
        let tree = make_tree(r#"<p class="rev">a</p><span class="alt">b</span><i class="imp">c</i>"#, css);
        let op = |tag: &str| find_node(&tree, tag).unwrap().specified_values.get(&intern("opacity")).cloned();
        assert_eq!(op("p"), Some(Value::Number(0.25)));
        assert_eq!(op("span"), Some(Value::Number(0.25)));
        assert_eq!(op("i"), Some(Value::Number(1.0)));
    }
}
