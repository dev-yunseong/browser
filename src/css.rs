use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::hash::{Hash, Hasher};
use lazy_static::lazy_static;

lazy_static! {
    static ref STRING_INTERNER: Mutex<HashSet<Arc<str>>> = Mutex::new(HashSet::new());
}

pub fn intern(s: &str) -> Arc<str> {
    let mut interner = STRING_INTERNER.lock().unwrap();
    if let Some(arc) = interner.get(s) {
        return arc.clone();
    }
    let arc: Arc<str> = Arc::from(s);
    interner.insert(arc.clone());
    arc
}

/// A single color stop inside a CSS gradient.
#[derive(Debug, Clone, PartialEq)]
pub struct CssColorStop {
    pub color: Color,
    /// Position in [0.0, 1.0]. `None` means "auto-distribute".
    pub position: Option<f32>,
}

impl Hash for CssColorStop {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.color.hash(state);
        match self.position {
            Some(f) => f.to_bits().hash(state),
            None => 0u32.hash(state),
        }
    }
}

impl Eq for CssColorStop {}

/// The direction / angle for a linear gradient.
#[derive(Debug, Clone, PartialEq)]
pub enum LinearDirection {
    /// Angle in radians, measured clockwise from "up" (12 o'clock).
    Angle(f32),
    /// `to <side>` keyword: (dx, dy) unit vector in CSS geometry (+y = down).
    ToSide(f32, f32),
}

impl Hash for LinearDirection {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            LinearDirection::Angle(f) => f.to_bits().hash(state),
            LinearDirection::ToSide(x, y) => { x.to_bits().hash(state); y.to_bits().hash(state); }
        }
    }
}

impl Eq for LinearDirection {}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub enum GradientValue {
    Linear {
        direction: LinearDirection,
        stops: Vec<CssColorStop>,
    },
    Radial {
        /// `true` = circle, `false` = ellipse (we render both as circle for simplicity).
        circle: bool,
        stops: Vec<CssColorStop>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Keyword(Arc<str>),
    Length(f32, Unit),
    Color(Color),
    BoxShadow(BoxShadow),
    Number(f32),
    /// Represents `fit-content(N px)` — uses available space up to N px,
    /// but no more than max-content and no less than min-content.
    FitContent(f32),
    Transform(Vec<TransformOp>),
    /// Represents a CSS custom property reference: `var(--name)` or `var(--name, fallback)`.
    CssVar { name: Arc<str>, fallback: Option<Box<Value>> },
    /// Holds the raw (unparsed) string value of a CSS custom property (`--foo: bar`).
    /// Used internally so that custom property values can be re-parsed when resolved
    /// by a `var()` reference on another property.
    RawCustomProp(Arc<str>),
    /// CSS gradient: `linear-gradient(...)` or `radial-gradient(...)`.
    Gradient(GradientValue),
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Value::Keyword(s) => s.hash(state),
            Value::Length(f, u) => {
                f.to_bits().hash(state);
                u.hash(state);
            }
            Value::Color(c) => c.hash(state),
            Value::BoxShadow(s) => s.hash(state),
            Value::Number(f) => f.to_bits().hash(state),
            Value::FitContent(f) => f.to_bits().hash(state),
            Value::Transform(ops) => ops.hash(state),
            Value::CssVar { name, fallback } => {
                name.hash(state);
                fallback.hash(state);
            }
            Value::RawCustomProp(s) => s.hash(state),
            Value::Gradient(g) => g.hash(state),
        }
    }
}

/// A length value for CSS transform translate functions: either px or percent.
///
/// Percentages are relative to the element's own width (for translateX) or
/// height (for translateY) at paint time, so they must be stored unevaluated
/// and resolved when the element dimensions are known.
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq)]
pub enum TranslateLength {
    Px(OrderedFloat),
    Percent(OrderedFloat),
}

impl TranslateLength {
    /// Resolve the length against `element_size` (width for X, height for Y).
    pub fn resolve(&self, element_size: f32) -> f32 {
        match self {
            TranslateLength::Px(v) => v.0,
            TranslateLength::Percent(v) => v.0 / 100.0 * element_size,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub enum TransformOp {
    Translate(TranslateLength, TranslateLength),
    Scale(OrderedFloat, OrderedFloat),
    Rotate(OrderedFloat),
    Matrix(OrderedFloat, OrderedFloat, OrderedFloat, OrderedFloat, OrderedFloat, OrderedFloat),
}

use std::ops::Deref;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct OrderedFloat(pub f32);

impl Deref for OrderedFloat {
    type Target = f32;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Hash for OrderedFloat {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl Eq for OrderedFloat {}

impl std::ops::Mul<f32> for OrderedFloat {
    type Output = f32;
    fn mul(self, rhs: f32) -> Self::Output {
        self.0 * rhs
    }
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub struct BoxShadow {
    pub offset_x: OrderedFloat,
    pub offset_y: OrderedFloat,
    pub blur: OrderedFloat,
    pub spread: OrderedFloat,
    pub color: Color,
    pub inset: bool,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub enum Unit {
    Px,
    Vw,
    Vh,
    Em,
    Percent,
}

#[derive(Debug, Clone)]
pub struct Declaration {
    pub name: Arc<str>,
    pub value: Value,
    pub important: bool,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone)]
pub enum AtRule {
    Media {
        query: String,
        rules: Vec<Rule>,
    },
    Unknown(String),
}

#[derive(Debug, Clone)]
pub enum RuleOrAtRule {
    Rule(Rule),
    AtRule(AtRule),
}

#[derive(Debug, Clone)]
pub struct Stylesheet {
    pub items: Vec<RuleOrAtRule>,
}

impl Stylesheet {
    pub fn all_rules(&self) -> Vec<&Rule> {
        let mut rules = Vec::new();
        for item in &self.items {
            match item {
                RuleOrAtRule::Rule(r) => rules.push(r),
                RuleOrAtRule::AtRule(AtRule::Media { rules: r, .. }) => {
                    for m_rule in r { rules.push(m_rule); }
                }
                _ => {}
            }
        }
        rules
    }
}

pub fn parse_css(source: &str) -> Stylesheet {
    let mut items = Vec::new();
    // Pre-processing to handle comments and whitespace better
    let source = source.replace('\n', " ");
    
    // Simple @rule preservation (Issue #21)
    // For now, we still strip them to avoid breaking the simple parser, 
    // but we'll implement a proper @media parser soon.
    let source = strip_at_rules(&source);

    let blocks: Vec<&str> = source.split('}').collect();
    for block in blocks {
        if block.trim().is_empty() { continue; }

        let mut parts = block.splitn(2, '{');
        let selectors_str = parts.next().unwrap_or("").trim();
        let declarations_str = parts.next().unwrap_or("").trim();

        if selectors_str.is_empty() || declarations_str.is_empty() { continue; }

        let mut selectors = Vec::new();
        for s in selectors_str.split(',') {
            let s = s.trim();
            if !s.is_empty() {
                selectors.push(parse_selector(s));
            }
        }

        if selectors.is_empty() { continue; }

        let mut declarations = Vec::new();
        for decl in declarations_str.split(';') {
            let decl = decl.trim();
            if decl.is_empty() { continue; }
            
            let mut kv = decl.splitn(2, ':');
            let key = intern(&kv.next().unwrap_or("").trim().to_lowercase());
            let mut val_raw = kv.next().unwrap_or("").trim().to_string();
            if key.is_empty() || val_raw.is_empty() { continue; }

            let important = val_raw.ends_with("!important");
            if important {
                val_raw = val_raw.trim_end_matches("!important").trim().to_string();
            }

            match &*key {
                "border" => {
                    let mut temp_map = HashMap::new();
                    parse_border_shorthand(&val_raw, &mut temp_map);
                    for (k, v) in temp_map {
                        declarations.push(Declaration { name: intern(&k), value: v, important });
                    }
                }
                // border-radius shorthand: "border-radius: <tl> [<tr> [<br> [<bl>]]]"
                // CSS allows up to 4 corner values.  We use the top-left (first) value as a
                // uniform radius for all corners — sufficient for the rounded-input / button
                // use-case (Google search bar, etc.).  The "/" elliptical syntax is not supported.
                "border-radius" => {
                    let first = val_raw.split_whitespace().next().unwrap_or("0");
                    // Strip the "/" elliptical part if present (e.g. "8px / 4px")
                    let first = first.split('/').next().unwrap_or("0").trim();
                    let value = parse_value(first);
                    declarations.push(Declaration { name: key, value, important });
                }
                "padding" => {
                    let mut temp_map = HashMap::new();
                    parse_quad_shorthand("padding", &val_raw, &mut temp_map);
                    for (k, v) in temp_map {
                        declarations.push(Declaration { name: intern(&k), value: v, important });
                    }
                }
                "margin" => {
                    let mut temp_map = HashMap::new();
                    parse_quad_shorthand("margin", &val_raw, &mut temp_map);
                    for (k, v) in temp_map {
                        declarations.push(Declaration { name: intern(&k), value: v, important });
                    }
                }
                "box-shadow" => {
                    if let Some(shadow) = parse_box_shadow(&val_raw) {
                        declarations.push(Declaration { name: key, value: Value::BoxShadow(shadow), important });
                    }
                }
                // font shorthand: "font: <style> <variant> <weight> <size>/<line-height> <family>"
                // We only extract the size / line-height / family pieces that affect layout.
                "font" => {
                    let parts: Vec<&str> = val_raw.split_whitespace().collect();
                    let size_idx = parts.iter().position(|part| {
                        let size_part = part.split_once('/').map(|(sz, _)| sz).unwrap_or(part);
                        matches!(parse_value(size_part), Value::Length(_, _))
                    });

                    if let Some(idx) = size_idx {
                        let size_token = parts[idx];
                        let (size_str, line_height_str) =
                            size_token.split_once('/').map_or((size_token, None), |(size, line)| {
                                (size, Some(line))
                            });

                        if let Value::Length(v, unit) = parse_value(size_str) {
                            declarations.push(Declaration {
                                name: intern("font-size"),
                                value: Value::Length(v, unit),
                                important,
                            });
                        }

                        if let Some(line) = line_height_str {
                            let line = line.trim();
                            if !line.is_empty() && line != "normal" {
                                declarations.push(Declaration {
                                    name: intern("line-height"),
                                    value: parse_value(line),
                                    important,
                                });
                            }
                        }

                        if idx + 1 < parts.len() {
                            let family = parts[idx + 1..].join(" ");
                            if !family.is_empty() {
                                declarations.push(Declaration {
                                    name: intern("font-family"),
                                    value: Value::Keyword(intern(&family)),
                                    important,
                                });
                            }
                        }
                    }
                }
                // flex shorthand: "flex: <grow> [<shrink> [<basis>]]" or keyword
                "flex" => {
                    let parts: Vec<&str> = val_raw.split_whitespace().collect();
                    match parts.len() {
                        0 => {}
                        1 => {
                            match parts[0] {
                                "none"    => {
                                    declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(0.0), important });
                                    declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(0.0), important });
                                    declarations.push(Declaration { name: intern("flex-basis"),  value: Value::Keyword(intern("auto")), important });
                                }
                                "auto"    => {
                                    declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(1.0), important });
                                    declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(1.0), important });
                                    declarations.push(Declaration { name: intern("flex-basis"),  value: Value::Keyword(intern("auto")), important });
                                }
                                _ => {
                                    // Single unitless number expands to `flex: <n> 1 0%`.
                                    if let Ok(n) = parts[0].parse::<f32>() {
                                        declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(n),   important });
                                        declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(1.0), important });
                                        declarations.push(Declaration { name: intern("flex-basis"),  value: Value::Length(0.0, Unit::Percent), important });
                                    }
                                }
                            }
                        }
                        2 => {
                            if let (Ok(g), Ok(s)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>()) {
                                declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(g), important });
                                declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(s), important });
                            } else if let Ok(g) = parts[0].parse::<f32>() {
                                declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(g), important });
                                declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(1.0), important });
                                declarations.push(Declaration { name: intern("flex-basis"),  value: parse_value(parts[1]), important });
                            }
                        }
                        _ => {
                            if let (Ok(g), Ok(s)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>()) {
                                declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(g), important });
                                declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(s), important });
                                declarations.push(Declaration { name: intern("flex-basis"),  value: parse_value(parts[2]), important });
                            }
                        }
                    }
                }
                // gap shorthand: "gap: <row-gap> [<col-gap>]"
                "gap" => {
                    let parts: Vec<&str> = val_raw.split_whitespace().collect();
                    let row_val = parts.first().map(|s| parse_value(s)).unwrap_or(Value::Number(0.0));
                    let col_val = parts.get(1).map(|s| parse_value(s)).unwrap_or_else(|| row_val.clone());
                    declarations.push(Declaration { name: intern("row-gap"),    value: row_val,  important });
                    declarations.push(Declaration { name: intern("column-gap"), value: col_val,  important });
                }
                // list-style shorthand: "list-style: none | disc | decimal | ..."
                // We only care about list-style-type for now.
                "list-style" => {
                    let first = val_raw.split_whitespace().next().unwrap_or("disc");
                    // Common values: none, disc, circle, square, decimal, etc.
                    let type_val = match first {
                        "none" | "disc" | "circle" | "square" | "decimal"
                        | "lower-alpha" | "upper-alpha" | "lower-roman" | "upper-roman" => {
                            Value::Keyword(intern(first))
                        }
                        _ => parse_value(first),
                    };
                    declarations.push(Declaration { name: intern("list-style-type"), value: type_val, important });
                }
                // inset shorthand: "inset: <top> [<right> [<bottom> [<left>]]]"
                // Same quad syntax as margin/padding, maps to top/right/bottom/left.
                "inset" => {
                    let parts: Vec<&str> = val_raw.split_whitespace().collect();
                    let (top, right, bottom, left) = match parts.len() {
                        1 => (parts[0], parts[0], parts[0], parts[0]),
                        2 => (parts[0], parts[1], parts[0], parts[1]),
                        3 => (parts[0], parts[1], parts[2], parts[1]),
                        4 => (parts[0], parts[1], parts[2], parts[3]),
                        _ => ("0", "0", "0", "0"),
                    };
                    declarations.push(Declaration { name: intern("top"),    value: parse_value(top),    important });
                    declarations.push(Declaration { name: intern("right"),  value: parse_value(right),  important });
                    declarations.push(Declaration { name: intern("bottom"), value: parse_value(bottom), important });
                    declarations.push(Declaration { name: intern("left"),   value: parse_value(left),   important });
                }
                _ => {
                    // CSS custom properties (--foo) store their raw value so that
                    // var() references can re-parse them at resolution time.
                    let value = if key.starts_with("--") {
                        Value::RawCustomProp(intern(&val_raw))
                    } else {
                        parse_value(&val_raw)
                    };
                    declarations.push(Declaration { name: key, value, important });
                }
            }
        }

        items.push(RuleOrAtRule::Rule(Rule { selectors, declarations }));
    }

    Stylesheet { items }
}

/// Viewport width used for `@media` query evaluation.
///
/// The render canvas is fixed at 800 px (see `src/main.rs`). All `@media`
/// conditions are evaluated against this value so that responsive stylesheets
/// activate the rules that were authored for an ~800 px viewport.
const VIEWPORT_WIDTH_PX: f32 = 800.0;

/// Parse a single media condition string like "(min-width: 992px)" or
/// "(max-width: 768px)".  Returns `true` if the VIEWPORT_WIDTH_PX satisfies
/// the condition.  Unknown/unsupported queries return `false` so their blocks
/// are safely skipped.
fn media_query_matches(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    // Strip surrounding parens if present
    let q = q.trim_start_matches('(').trim_end_matches(')');
    if let Some(rest) = q.strip_prefix("min-width:") {
        let val_str = rest.trim().trim_end_matches("px").trim();
        if let Ok(min) = val_str.parse::<f32>() {
            return VIEWPORT_WIDTH_PX >= min;
        }
    } else if let Some(rest) = q.strip_prefix("max-width:") {
        let val_str = rest.trim().trim_end_matches("px").trim();
        if let Ok(max) = val_str.parse::<f32>() {
            return VIEWPORT_WIDTH_PX <= max;
        }
    } else if let Some(rest) = q.strip_prefix("prefers-color-scheme:") {
        // Headless renderer defaults to dark preference.
        let scheme = rest.trim();
        return scheme == "dark";
    }
    false
}

/// Returns true if the @media query string (the part after `@media` before the
/// opening `{`) should be treated as matching given VIEWPORT_WIDTH_PX.
/// Handles simple single-condition queries like `(min-width: 992px)` and
/// compound `screen and (min-width: 992px)` queries.
fn evaluate_media_query(query_str: &str) -> bool {
    // Strip "screen"/"all"/"print" type tokens and "and" keywords, then evaluate
    // whatever condition remains.
    let q = query_str.trim().to_lowercase();
    // Skip print-only queries
    if q.starts_with("print") { return false; }
    // Extract the parenthesised portion
    if let Some(start) = q.find('(') {
        let sub = &q[start..];
        if let Some(end) = sub.rfind(')') {
            let condition = &sub[..=end];
            return media_query_matches(condition);
        }
    }
    // Bare "all" or "screen" without a condition → always match
    if q == "all" || q == "screen" { return true; }
    false
}

fn strip_at_rules(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '@' {
            // Collect the at-keyword and query up to the first '{' or ';'
            let at_start = i;
            i += 1; // skip '@'
            // Read keyword (letters only)
            let mut keyword = String::new();
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '-') {
                keyword.push(chars[i]);
                i += 1;
            }
            let keyword = keyword.to_lowercase();

            if keyword == "media" {
                // Collect the query text up to the opening '{'
                let mut query = String::new();
                while i < len && chars[i] != '{' {
                    query.push(chars[i]);
                    i += 1;
                }
                if i >= len { break; }
                i += 1; // consume '{'

                // Decide whether to include the body.
                let include = evaluate_media_query(&query);

                // Walk the nested braces, copying content only if include == true.
                let mut depth = 1usize;
                while i < len && depth > 0 {
                    let c = chars[i];
                    if c == '{' { depth += 1; }
                    else if c == '}' {
                        depth -= 1;
                        if depth == 0 { i += 1; break; }
                    }
                    if include { result.push(c); }
                    i += 1;
                }
                // closing '}' already consumed by the break above
            } else {
                // Non-@media at-rule: skip it entirely.
                // Determine if it's a block rule (has '{...}') or a simple statement ending with ';'.
                // Collect up to the first '{' or ';' to decide.
                let mut preamble = String::new();
                while i < len && chars[i] != '{' && chars[i] != ';' {
                    preamble.push(chars[i]);
                    i += 1;
                }
                if i < len && chars[i] == '{' {
                    i += 1; // consume '{'
                    let mut depth = 1usize;
                    while i < len && depth > 0 {
                        let c = chars[i];
                        if c == '{' { depth += 1; }
                        else if c == '}' { depth -= 1; }
                        i += 1;
                    }
                } else if i < len && chars[i] == ';' {
                    i += 1; // consume ';'
                }
                let _ = (at_start, preamble); // suppress unused warnings
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

pub fn parse_quad_shorthand(prefix: &str, val: &str, declarations: &mut HashMap<String, Value>) {
    let parts: Vec<&str> = val.split_whitespace().collect();
    let (top, right, bottom, left) = match parts.len() {
        1 => (parts[0], parts[0], parts[0], parts[0]),
        2 => (parts[0], parts[1], parts[0], parts[1]),
        3 => (parts[0], parts[1], parts[2], parts[1]),
        4 => (parts[0], parts[1], parts[2], parts[3]),
        _ => return,
    };
    declarations.insert(format!("{}-top", prefix), parse_value(top));
    declarations.insert(format!("{}-right", prefix), parse_value(right));
    declarations.insert(format!("{}-bottom", prefix), parse_value(bottom));
    declarations.insert(format!("{}-left", prefix), parse_value(left));
    // Also set the combined shorthand for backward compatibility
    declarations.insert(prefix.to_string(), parse_value(top));
}

#[derive(Debug, Clone, PartialEq)]
pub enum Combinator {
    Descendant,
    Child,
    NextSibling,
    SubsequentSibling,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributeMatch {
    Exists,
    Equals(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttributeSelector {
    pub name: String,
    pub value: AttributeMatch,
}

#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub class: Vec<String>,
    pub attributes: Vec<AttributeSelector>,
    pub pseudo_class: Option<String>,
    /// Pseudo-element (`"before"` or `"after"`), set when the selector ends with
    /// `::before` or `::after`.  Single-colon pseudo-classes (`:hover`, `:focus`,
    /// `:root`) stay in `pseudo_class`.
    pub pseudo_element: Option<String>,
    pub combinator: Option<Combinator>,
    pub ancestor: Option<Box<Selector>>,
}

/// The most-specific "key" feature of the rightmost part of a selector.
/// Used to bucket selectors into an index for O(1) candidate lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SelectorKey {
    Id(String),
    Class(String),
    Tag(String),
    Universal,
}

impl Selector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn specificity(&self) -> (usize, usize, usize) {
        let (mut a, mut b, mut c) = (0, 0, 0);
        if self.id.is_some() { a += 1; }
        b += self.class.len();
        b += self.attributes.len();
        if self.pseudo_class.is_some() { b += 1; }
        if self.tag.is_some() { c += 1; }

        if let Some(ref d) = self.ancestor {
            let (da, db, dc) = d.specificity();
            a += da; b += db; c += dc;
        }
        (a, b, c)
    }

    /// Returns the most specific "key" feature of this selector's subject (rightmost) part.
    /// Used to bucket selectors for fast candidate lookup (ID > first class > tag > universal).
    pub fn key_feature(&self) -> SelectorKey {
        if let Some(ref id) = self.id {
            return SelectorKey::Id(id.clone());
        }
        if let Some(cls) = self.class.first() {
            return SelectorKey::Class(cls.clone());
        }
        if let Some(ref tag) = self.tag {
            return SelectorKey::Tag(tag.clone());
        }
        SelectorKey::Universal
    }
}

pub fn parse_selector(s: &str) -> Selector {
    // Pre-process string to ensure spaces around combinators for easy splitting
    let s = s.replace('>', " > ").replace('+', " + ").replace('~', " ~ ");
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.is_empty() { return Selector::new(); }

    let mut root: Option<Selector> = None;
    let mut pending_combinator: Option<Combinator> = None;

    for part in parts {
        match part {
            ">" => pending_combinator = Some(Combinator::Child),
            "+" => pending_combinator = Some(Combinator::NextSibling),
            "~" => pending_combinator = Some(Combinator::SubsequentSibling),
            _ => {
                // Split on the first `:` to separate the base selector from any pseudo.
                let mut p_parts = part.splitn(2, ':');
                let base_part = p_parts.next().unwrap_or(part);
                // The remainder after the first `:` may start with another `:` for
                // pseudo-elements (::before, ::after) or be a plain pseudo-class name.
                let pseudo_rest = p_parts.next();

                let (pseudo_class, pseudo_element): (Option<String>, Option<String>) = match pseudo_rest {
                    None => (None, None),
                    Some(rest) => {
                        if let Some(pe) = rest.strip_prefix(':') {
                            // Double-colon pseudo-element: ::before / ::after
                            let name = pe.to_lowercase();
                            if name == "before" || name == "after" {
                                (None, Some(name))
                            } else {
                                // Unknown pseudo-element — skip.
                                (None, None)
                            }
                        } else {
                            // Single-colon pseudo-class: :hover, :focus, :root, etc.
                            (Some(rest.to_string()), None)
                        }
                    }
                };

                if base_part.is_empty() && pseudo_class.is_none() && pseudo_element.is_none() { continue; }

                let mut current_sel = Selector::new();
                current_sel.pseudo_class = pseudo_class;
                current_sel.pseudo_element = pseudo_element;
                let mut current_token = String::new();
                let mut last_char = ' ';

                let mut chars = base_part.chars().chain(std::iter::once(' ')).peekable();
                while let Some(c) = chars.next() {
                    if c == '#' || c == '.' || c == ' ' || c == '[' {
                        if !current_token.is_empty() {
                            match last_char {
                                '#' => current_sel.id = Some(current_token.clone()),
                                '.' => current_sel.class.push(current_token.clone()),
                                _ => current_sel.tag = Some(current_token.clone()),
                            }
                            current_token.clear();
                        }
                        if c == '[' {
                            // Parse attribute selector [attr=val]
                            let mut attr_content = String::new();
                            while let Some(ac) = chars.next() {
                                if ac == ']' { break; }
                                attr_content.push(ac);
                            }
                            if !attr_content.is_empty() {
                                if let Some(eq_idx) = attr_content.find('=') {
                                    let name = attr_content[..eq_idx].trim().to_string();
                                    let val = attr_content[eq_idx+1..].trim().trim_matches('"').trim_matches('\'').to_string();
                                    current_sel.attributes.push(AttributeSelector {
                                        name,
                                        value: AttributeMatch::Equals(val),
                                    });
                                } else {
                                    current_sel.attributes.push(AttributeSelector {
                                        name: attr_content.trim().to_string(),
                                        value: AttributeMatch::Exists,
                                    });
                                }
                            }
                            last_char = ' '; // Reset after attribute
                            continue;
                        }
                        last_char = c;
                    } else {
                        current_token.push(c);
                    }
                }
                
                // Inherit combinator from previous part, or default to Descendant if there was a previous element
                let combinator = pending_combinator.take().unwrap_or(if root.is_some() { Combinator::Descendant } else { Combinator::Descendant });
                
                if let Some(prev) = root {
                    current_sel.ancestor = Some(Box::new(prev));
                    current_sel.combinator = Some(combinator);
                }
                root = Some(current_sel);
            }
        }
    }

    root.unwrap_or_default()
}

pub fn parse_border_shorthand_pub(val: &str, declarations: &mut HashMap<String, Value>) {
    parse_border_shorthand(val, declarations);
}

fn parse_border_shorthand(val: &str, declarations: &mut HashMap<String, Value>) {
    let parts: Vec<&str> = val.split_whitespace().collect();
    for part in parts {
        if part.ends_with("px") || part.chars().all(|c| c.is_numeric()) {
            declarations.insert("border-width".to_string(), parse_value(part));
        } else if let Some(color) = parse_color(part) {
            declarations.insert("border-color".to_string(), Value::Color(color));
        } else if matches!(part, "solid" | "dashed" | "dotted" | "none") {
            declarations.insert("border-style".to_string(), Value::Keyword(intern(part)));
        }
    }
}

/// Split a string by spaces while respecting parentheses nesting.
pub fn split_respecting_parens(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;

    for c in s.chars() {
        match c {
            '(' => { depth += 1; current.push(c); }
            ')' => { depth -= 1; current.push(c); }
            ' ' | '\t' if depth == 0 => {
                let t = current.trim().to_string();
                if !t.is_empty() { parts.push(t); }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let t = current.trim().to_string();
    if !t.is_empty() { parts.push(t); }
    parts
}

pub fn parse_box_shadow(s: &str) -> Option<BoxShadow> {
    if s == "none" { return None; }
    let parts = split_respecting_parens(s);
    let mut values: Vec<f32> = Vec::new();
    let mut color = Color { r: 0, g: 0, b: 0, a: 128 };
    let mut inset = false;

    for part in &parts {
        if part == "inset" {
            inset = true;
        } else if let Some(c) = parse_color(part) {
            color = c;
        } else {
            let num_str = part.trim_end_matches("px");
            if let Ok(v) = num_str.parse::<f32>() {
                values.push(v);
            }
        }
    }

    if values.is_empty() { return None; }

    Some(BoxShadow {
        offset_x: OrderedFloat(values.get(0).copied().unwrap_or(0.0)),
        offset_y: OrderedFloat(values.get(1).copied().unwrap_or(0.0)),
        blur: OrderedFloat(values.get(2).copied().unwrap_or(0.0)),
        spread: OrderedFloat(values.get(3).copied().unwrap_or(0.0)),
        color,
        inset,
    })
}

/// Split gradient arguments by top-level commas (ignoring commas inside `rgb()` etc.).
fn split_gradient_args(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;
    for c in s.chars() {
        match c {
            '(' => { depth += 1; current.push(c); }
            ')' => { depth -= 1; current.push(c); }
            ',' if depth == 0 => {
                let t = current.trim().to_string();
                if !t.is_empty() { parts.push(t); }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let t = current.trim().to_string();
    if !t.is_empty() { parts.push(t); }
    parts
}

/// Parse a single color stop like `#ff0`, `red`, `blue 30%`, `rgba(0,0,0,0.5) 100%`.
fn parse_color_stop(s: &str) -> Option<CssColorStop> {
    let s = s.trim();
    // Find the split point: last whitespace not inside parens.
    let mut depth: i32 = 0;
    let mut last_space: Option<usize> = None;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ' ' | '\t' if depth == 0 => last_space = Some(i),
            _ => {}
        }
    }

    let (color_str, position) = if let Some(sp) = last_space {
        let possible_pos = s[sp + 1..].trim();
        if possible_pos.ends_with('%') || possible_pos.ends_with("px") {
            (&s[..sp], Some(possible_pos))
        } else {
            (s, None)
        }
    } else {
        (s, None)
    };

    let color = parse_color(color_str.trim())?;
    let pos = position.map(|p| {
        if p.ends_with('%') {
            p.trim_end_matches('%').parse::<f32>().unwrap_or(0.0) / 100.0
        } else {
            // px positions are not normalized here; we treat them as ratios (best-effort)
            p.trim_end_matches("px").parse::<f32>().unwrap_or(0.0) / 100.0
        }
    });

    Some(CssColorStop { color, position: pos })
}

/// Parse `linear-gradient(...)` or `radial-gradient(...)`. Returns `None` on failure.
pub fn parse_gradient(val: &str) -> Option<GradientValue> {
    let val = val.trim();

    let (is_linear, inner) = if let Some(rest) = val.strip_prefix("linear-gradient(").and_then(|r| r.strip_suffix(')')) {
        (true, rest)
    } else if let Some(rest) = val.strip_prefix("radial-gradient(").and_then(|r| r.strip_suffix(')')) {
        (false, rest)
    } else {
        return None;
    };

    let args = split_gradient_args(inner);
    if args.is_empty() { return None; }

    if is_linear {
        // First arg may be a direction keyword or angle.
        let mut stop_start = 0;
        let direction = {
            let first = args[0].trim().to_lowercase();
            if first.ends_with("deg") {
                // e.g. "90deg"
                let deg: f32 = first.trim_end_matches("deg").parse().unwrap_or(0.0);
                stop_start = 1;
                LinearDirection::Angle(deg.to_radians())
            } else if first.starts_with("to ") {
                let side = first[3..].trim();
                let (dx, dy) = match side {
                    "right"        => (1.0_f32, 0.0_f32),
                    "left"         => (-1.0, 0.0),
                    "bottom"       => (0.0, 1.0),
                    "top"          => (0.0, -1.0),
                    "right bottom" | "bottom right" => (1.0, 1.0),
                    "right top"    | "top right"    => (1.0, -1.0),
                    "left bottom"  | "bottom left"  => (-1.0, 1.0),
                    "left top"     | "top left"     => (-1.0, -1.0),
                    _              => (0.0, 1.0),
                };
                stop_start = 1;
                LinearDirection::ToSide(dx, dy)
            } else {
                // No explicit direction — default is "to bottom" (top → bottom)
                LinearDirection::ToSide(0.0, 1.0)
            }
        };

        let stops: Vec<CssColorStop> = args[stop_start..]
            .iter()
            .filter_map(|s| parse_color_stop(s))
            .collect();

        if stops.len() < 2 { return None; }

        // Auto-distribute stops that have no explicit position.
        let stops = auto_distribute_stops(stops);
        Some(GradientValue::Linear { direction, stops })
    } else {
        // radial-gradient: first arg may be shape keyword.
        let mut stop_start = 0;
        let first = args[0].trim().to_lowercase();
        let circle = if first == "circle" || first.starts_with("circle ") {
            stop_start = 1;
            true
        } else if first == "ellipse" || first.starts_with("ellipse ") || first.starts_with("closest") || first.starts_with("farthest") {
            stop_start = 1;
            false
        } else {
            false
        };

        let stops: Vec<CssColorStop> = args[stop_start..]
            .iter()
            .filter_map(|s| parse_color_stop(s))
            .collect();

        if stops.len() < 2 { return None; }

        let stops = auto_distribute_stops(stops);
        Some(GradientValue::Radial { circle, stops })
    }
}

/// Assign evenly-spaced positions to any stops that don't have an explicit position.
fn auto_distribute_stops(mut stops: Vec<CssColorStop>) -> Vec<CssColorStop> {
    // If the first stop has no position, assign 0.0.
    if stops[0].position.is_none() {
        stops[0].position = Some(0.0);
    }
    // If the last stop has no position, assign 1.0.
    let last = stops.len() - 1;
    if stops[last].position.is_none() {
        stops[last].position = Some(1.0);
    }
    // Fill in any remaining None positions by linear interpolation between
    // the nearest positioned neighbors.
    let n = stops.len();
    let mut i = 0;
    while i < n {
        if stops[i].position.is_none() {
            // Find the next stop that has a position.
            let mut j = i + 1;
            while j < n && stops[j].position.is_none() {
                j += 1;
            }
            // Interpolate between stops[i-1] and stops[j].
            let start_pos = stops[i - 1].position.unwrap_or(0.0);
            let end_pos = stops[j].position.unwrap_or(1.0);
            let count = (j - i + 1) as f32;
            for k in i..j {
                let t = (k - i + 1) as f32 / count;
                stops[k].position = Some(start_pos + t * (end_pos - start_pos));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    stops
}

pub fn parse_value(val: &str) -> Value {
    let val = val.trim();
    // Strip !important
    let val = val.trim_end_matches("!important").trim();

    // Intrinsic sizing keywords (CSS Sizing Level 3)
    if val == "min-content" || val == "max-content" || val == "fit-content" {
        return Value::Keyword(intern(val));
    }

    // var(--custom-property) or var(--custom-property, fallback)
    if val.starts_with("var(") && val.ends_with(')') {
        let inner = &val[4..val.len() - 1]; // strip "var(" and ")"
        // Split on first comma to separate name from optional fallback.
        // We must be careful: the fallback itself may contain commas (e.g. rgb(1,2,3)).
        // A simple approach: find the first top-level comma.
        let mut depth = 0i32;
        let mut comma_pos: Option<usize> = None;
        for (i, c) in inner.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => { comma_pos = Some(i); break; }
                _ => {}
            }
        }
        let (name_str, fallback_str) = if let Some(pos) = comma_pos {
            (&inner[..pos], Some(inner[pos + 1..].trim()))
        } else {
            (inner, None)
        };
        let name_str = name_str.trim();
        if name_str.starts_with("--") {
            let fallback = fallback_str.map(|fb| Box::new(parse_value(fb)));
            return Value::CssVar { name: intern(name_str), fallback };
        }
    }

    // gradient: linear-gradient(...) or radial-gradient(...)
    if val.starts_with("linear-gradient(") || val.starts_with("radial-gradient(") {
        if let Some(g) = parse_gradient(val) {
            return Value::Gradient(g);
        }
    }

    // transform: translate(...) rotate(...)
    if val.contains('(') && (val.starts_with("translate") || val.starts_with("scale") || val.starts_with("rotate") || val.starts_with("matrix")) {
        let ops = parse_transform_list(val);
        if !ops.is_empty() {
            return Value::Transform(ops);
        }
    }
    // fit-content(<length>) — e.g. fit-content(300px)
    if val.starts_with("fit-content(") && val.ends_with(')') {
        let inner = &val["fit-content(".len()..val.len() - 1];
        let px_val = inner.trim_end_matches("px").parse::<f32>().unwrap_or(0.0);
        return Value::FitContent(px_val);
    }

    if val.ends_with("px") {
        Value::Length(val.trim_end_matches("px").parse().unwrap_or(0.0), Unit::Px)
    } else if val.ends_with("vw") {
        Value::Length(val.trim_end_matches("vw").parse().unwrap_or(0.0), Unit::Vw)
    } else if val.ends_with("vh") {
        Value::Length(val.trim_end_matches("vh").parse().unwrap_or(0.0), Unit::Vh)
    } else if val.ends_with("em") || val.ends_with("rem") {
        let num = val.trim_end_matches("rem").trim_end_matches("em");
        Value::Length(num.parse().unwrap_or(1.0), Unit::Em)
    } else if val.ends_with('%') {
        Value::Length(val.trim_end_matches('%').parse().unwrap_or(0.0), Unit::Percent)
    } else if let Some(color) = parse_color(val) {
        Value::Color(color)
    } else if let Ok(num) = val.parse::<f32>() {
        Value::Number(num)
    } else {
        Value::Keyword(intern(val))
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_lowercase();
    if s.starts_with('#') {
        let hex = &s[1..];
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some(Color { r, g, b, a: 255 })
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color { r, g, b, a: 255 })
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Color { r, g, b, a })
            }
            _ => None,
        };
    }
    if s.starts_with("rgba(") || s.starts_with("rgb(") {
        if let Some(content) = s.split(|c| c == '(' || c == ')').nth(1) {
            let parts: Vec<&str> = content.split(',').map(|p| p.trim()).collect();
            if parts.len() >= 3 {
                let r = parts[0].parse().ok()?;
                let g = parts[1].parse().ok()?;
                let b = parts[2].parse().ok()?;
                let a = if parts.len() == 4 {
                    (parts[3].parse::<f32>().ok()? * 255.0).clamp(0.0, 255.0) as u8
                } else {
                    255
                };
                return Some(Color { r, g, b, a });
            }
        }
    }
    // hsl() - approximate conversion
    if s.starts_with("hsl(") {
        if let Some(content) = s.split(|c| c == '(' || c == ')').nth(1) {
            let parts: Vec<&str> = content.split(',').map(|p| p.trim()).collect();
            if parts.len() >= 3 {
                let h: f32 = parts[0].parse().ok()?;
                let s_pct: f32 = parts[1].trim_end_matches('%').parse().ok()?;
                let l_pct: f32 = parts[2].trim_end_matches('%').parse().ok()?;
                let (r, g, b) = hsl_to_rgb(h, s_pct / 100.0, l_pct / 100.0);
                return Some(Color { r, g, b, a: 255 });
            }
        }
    }
    named_color(&s)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = if h < 60.0 { (c, x, 0.0) }
        else if h < 120.0 { (x, c, 0.0) }
        else if h < 180.0 { (0.0, c, x) }
        else if h < 240.0 { (0.0, x, c) }
        else if h < 300.0 { (x, 0.0, c) }
        else { (c, 0.0, x) };
    (
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

fn named_color(s: &str) -> Option<Color> {
    let color = match s {
        "white"       => Color { r: 255, g: 255, b: 255, a: 255 },
        "black"       => Color { r: 0,   g: 0,   b: 0,   a: 255 },
        "red"         => Color { r: 255, g: 0,   b: 0,   a: 255 },
        "green"       => Color { r: 0,   g: 128, b: 0,   a: 255 },
        "blue"        => Color { r: 0,   g: 0,   b: 255, a: 255 },
        "yellow"      => Color { r: 255, g: 255, b: 0,   a: 255 },
        "cyan"        => Color { r: 0,   g: 255, b: 255, a: 255 },
        "magenta"     => Color { r: 255, g: 0,   b: 255, a: 255 },
        "silver"      => Color { r: 192, g: 192, b: 192, a: 255 },
        "gray"        => Color { r: 128, g: 128, b: 128, a: 255 },
        "grey"        => Color { r: 128, g: 128, b: 128, a: 255 },
        "orange"      => Color { r: 255, g: 165, b: 0,   a: 255 },
        "purple"      => Color { r: 128, g: 0,   b: 128, a: 255 },
        "pink"        => Color { r: 255, g: 192, b: 203, a: 255 },
        "gold"        => Color { r: 255, g: 215, b: 0,   a: 255 },
        "transparent" => Color { r: 0,   g: 0,   b: 0,   a: 0   },
        "navy"        => Color { r: 0,   g: 0,   b: 128, a: 255 },
        "teal"        => Color { r: 0,   g: 128, b: 128, a: 255 },
        "lime"        => Color { r: 0,   g: 255, b: 0,   a: 255 },
        "maroon"      => Color { r: 128, g: 0,   b: 0,   a: 255 },
        "olive"       => Color { r: 128, g: 128, b: 0,   a: 255 },
        "aqua"        => Color { r: 0,   g: 255, b: 255, a: 255 },
        "fuchsia"     => Color { r: 255, g: 0,   b: 255, a: 255 },
        "coral"       => Color { r: 255, g: 127, b: 80,  a: 255 },
        "salmon"      => Color { r: 250, g: 128, b: 114, a: 255 },
        "tomato"      => Color { r: 255, g: 99,  b: 71,  a: 255 },
        "indigo"      => Color { r: 75,  g: 0,   b: 130, a: 255 },
        "violet"      => Color { r: 238, g: 130, b: 238, a: 255 },
        "khaki"       => Color { r: 240, g: 230, b: 140, a: 255 },
        "beige"       => Color { r: 245, g: 245, b: 220, a: 255 },
        "ivory"       => Color { r: 255, g: 255, b: 240, a: 255 },
        "lavender"    => Color { r: 230, g: 230, b: 250, a: 255 },
        "lightgray" | "lightgrey" => Color { r: 211, g: 211, b: 211, a: 255 },
        "darkgray" | "darkgrey"   => Color { r: 169, g: 169, b: 169, a: 255 },
        "lightblue"   => Color { r: 173, g: 216, b: 230, a: 255 },
        "darkblue"    => Color { r: 0,   g: 0,   b: 139, a: 255 },
        "lightgreen"  => Color { r: 144, g: 238, b: 144, a: 255 },
        "darkgreen"   => Color { r: 0,   g: 100, b: 0,   a: 255 },
        "lightyellow" => Color { r: 255, g: 255, b: 224, a: 255 },
        "mintcream"   => Color { r: 245, g: 255, b: 250, a: 255 },
        "whitesmoke"  => Color { r: 245, g: 245, b: 245, a: 255 },
        "gainsboro"   => Color { r: 220, g: 220, b: 220, a: 255 },
        "aliceblue"   => Color { r: 240, g: 248, b: 255, a: 255 },
        "currentcolor" | "inherit" | "initial" | "unset" => return None,
        _ => return None,
    };
    Some(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_selector() {
        let s = parse_selector("div#main.header.active");
        assert_eq!(s.tag, Some("div".to_string()));
        assert_eq!(s.id, Some("main".to_string()));
        assert_eq!(s.class, vec!["header".to_string(), "active".to_string()]);
    }

    #[test]
    fn test_parse_box_shadow() {
        let s = parse_box_shadow("0px 2px 8px rgba(0, 0, 0, 0.15)");
        assert!(s.is_some());
        let s = s.unwrap();
        assert_eq!(s.offset_x, OrderedFloat(0.0));
        assert_eq!(s.offset_y, OrderedFloat(2.0));
        assert_eq!(s.blur, OrderedFloat(8.0));
    }

    /// `box-shadow: none` must parse to `None` (no shadow rendered).
    #[test]
    fn test_parse_box_shadow_none_returns_none() {
        let s = parse_box_shadow("none");
        assert!(s.is_none(), "box-shadow: none must return None");
    }

    /// `box-shadow: 2px 2px 4px #888` must parse with correct offset and hex color.
    #[test]
    fn test_parse_box_shadow_hex_color_with_offset() {
        let s = parse_box_shadow("2px 2px 4px #888");
        assert!(s.is_some(), "box-shadow with hex color must parse successfully");
        let s = s.unwrap();
        assert_eq!(s.offset_x, OrderedFloat(2.0));
        assert_eq!(s.offset_y, OrderedFloat(2.0));
        assert_eq!(s.blur, OrderedFloat(4.0));
        assert!(!s.inset, "shadow without inset keyword must not be inset");
    }

    /// `box-shadow: inset 0 1px 3px rgba(0,0,0,0.2)` must parse with inset=true.
    #[test]
    fn test_parse_box_shadow_inset() {
        let s = parse_box_shadow("inset 0 1px 3px rgba(0,0,0,0.2)");
        assert!(s.is_some(), "inset box-shadow must parse successfully");
        let s = s.unwrap();
        assert!(s.inset, "shadow with inset keyword must have inset=true");
        assert_eq!(s.offset_x, OrderedFloat(0.0));
        assert_eq!(s.offset_y, OrderedFloat(1.0));
        assert_eq!(s.blur, OrderedFloat(3.0));
    }

    /// `box-shadow: 0 2px 8px rgba(0,0,0,0.15)` with spread must parse spread correctly.
    #[test]
    fn test_parse_box_shadow_with_spread() {
        let s = parse_box_shadow("0 2px 4px 2px #000");
        assert!(s.is_some(), "box-shadow with spread must parse");
        let s = s.unwrap();
        assert_eq!(s.offset_x, OrderedFloat(0.0));
        assert_eq!(s.offset_y, OrderedFloat(2.0));
        assert_eq!(s.blur, OrderedFloat(4.0));
        assert_eq!(s.spread, OrderedFloat(2.0));
    }

    /// The `box-shadow` CSS property in a stylesheet must produce a `Value::BoxShadow`.
    #[test]
    fn test_css_box_shadow_property_parses_to_value() {
        let ss = parse_css("div { box-shadow: 0 2px 8px rgba(0,0,0,0.15); }");
        let rule = match &ss.items[0] {
            RuleOrAtRule::Rule(r) => r,
            _ => panic!("expected rule"),
        };
        let has_shadow = rule.declarations.iter().any(|d| {
            d.name.as_ref() == "box-shadow" && matches!(d.value, Value::BoxShadow(_))
        });
        assert!(has_shadow, "box-shadow property must produce Value::BoxShadow");
    }

    /// `box-shadow: none` in a stylesheet must produce NO `Value::BoxShadow` declaration.
    #[test]
    fn test_css_box_shadow_none_produces_no_declaration() {
        let ss = parse_css("div { box-shadow: none; }");
        let rule = match &ss.items[0] {
            RuleOrAtRule::Rule(r) => r,
            _ => panic!("expected rule"),
        };
        let has_shadow = rule.declarations.iter().any(|d| {
            d.name.as_ref() == "box-shadow"
        });
        assert!(!has_shadow, "box-shadow: none must not produce a box-shadow declaration");
    }

    #[test]
    fn test_parse_percent() {
        let v = parse_value("50%");
        assert_eq!(v, Value::Length(50.0, Unit::Percent));
    }

    #[test]
    fn test_parse_font_shorthand_extracts_size_and_line_height() {
        let ss = parse_css("a { font: 13px/27px Roboto,Arial,sans-serif; }");
        let rule = match &ss.items[0] {
            RuleOrAtRule::Rule(rule) => rule,
            _ => panic!("expected rule"),
        };
        assert!(rule.declarations.iter().any(|d| {
            d.name.as_ref() == "font-size"
                && matches!(d.value, Value::Length(v, Unit::Px) if (v - 13.0).abs() < 1e-5)
        }));
        assert!(rule.declarations.iter().any(|d| {
            d.name.as_ref() == "line-height"
                && matches!(d.value, Value::Length(v, Unit::Px) if (v - 27.0).abs() < 1e-5)
        }));
    }

    #[test]
    fn test_named_color() {
        assert!(parse_color("navy").is_some());
        assert!(parse_color("transparent").is_some());
    }

    #[test]
    fn test_key_feature_id() {
        let s = parse_selector("#foo");
        assert_eq!(s.key_feature(), SelectorKey::Id("foo".to_string()));
    }

    #[test]
    fn test_key_feature_id_priority_over_class() {
        // ID is more specific than class; id should be returned even when class is present
        let s = parse_selector("div#main.header");
        assert_eq!(s.key_feature(), SelectorKey::Id("main".to_string()));
    }

    #[test]
    fn test_key_feature_class() {
        let s = parse_selector(".bar");
        assert_eq!(s.key_feature(), SelectorKey::Class("bar".to_string()));
    }

    #[test]
    fn test_key_feature_tag() {
        let s = parse_selector("div");
        assert_eq!(s.key_feature(), SelectorKey::Tag("div".to_string()));
    }

    #[test]
    fn test_key_feature_universal() {
        // A selector with only pseudo-class or empty falls through to Universal
        let mut s = Selector::new();
        s.pseudo_class = Some("hover".to_string());
        assert_eq!(s.key_feature(), SelectorKey::Universal);
    }

    #[test]
    fn test_key_feature_complex_selector_rightmost() {
        // For "div .bar", the rightmost part (.bar) should be the key
        let s = parse_selector("div .bar");
        // The rightmost selector part is .bar, so key should be Class("bar")
        assert_eq!(s.key_feature(), SelectorKey::Class("bar".to_string()));
    }

    #[test]
    fn test_parse_linear_gradient_to_right() {
        let v = parse_value("linear-gradient(to right, #ff0, #f00)");
        match v {
            Value::Gradient(GradientValue::Linear { direction, stops }) => {
                assert!(matches!(direction, LinearDirection::ToSide(dx, _) if dx > 0.0));
                assert_eq!(stops.len(), 2);
                assert_eq!(stops[0].position, Some(0.0));
                assert_eq!(stops[1].position, Some(1.0));
            }
            other => panic!("expected linear gradient, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_linear_gradient_angle() {
        let v = parse_value("linear-gradient(90deg, #fff, #000)");
        match v {
            Value::Gradient(GradientValue::Linear { direction, stops }) => {
                assert!(matches!(direction, LinearDirection::Angle(_)));
                assert_eq!(stops.len(), 2);
            }
            other => panic!("expected linear gradient, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_radial_gradient_circle() {
        let v = parse_value("radial-gradient(circle, #fff, #000)");
        match v {
            Value::Gradient(GradientValue::Radial { circle, stops }) => {
                assert!(circle);
                assert_eq!(stops.len(), 2);
                assert_eq!(stops[0].position, Some(0.0));
                assert_eq!(stops[1].position, Some(1.0));
            }
            other => panic!("expected radial gradient, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_gradient_color_stops_with_percentages() {
        let v = parse_value("linear-gradient(to right, #ff0 0%, #f00 50%, #00f 100%)");
        match v {
            Value::Gradient(GradientValue::Linear { stops, .. }) => {
                assert_eq!(stops.len(), 3);
                assert!((stops[0].position.unwrap() - 0.0).abs() < 1e-5);
                assert!((stops[1].position.unwrap() - 0.5).abs() < 1e-5);
                assert!((stops[2].position.unwrap() - 1.0).abs() < 1e-5);
            }
            other => panic!("expected linear gradient, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_gradient_auto_distribute_middle_stops() {
        // Three stops: first and last have positions, middle does not.
        let v = parse_value("linear-gradient(to right, red 0%, green, blue 100%)");
        match v {
            Value::Gradient(GradientValue::Linear { stops, .. }) => {
                assert_eq!(stops.len(), 3);
                assert!((stops[1].position.unwrap() - 0.5).abs() < 1e-5,
                    "middle stop should be auto-distributed to 0.5, got {:?}", stops[1].position);
            }
            other => panic!("expected linear gradient, got {:?}", other),
        }
    }

    #[test]
    fn test_media_query_max_width_applies_at_800px() {
        // @media (max-width: 900px) must apply at the 800px viewport.
        let ss = parse_css("@media (max-width: 900px) { p { color: red; } }");
        let rules = ss.all_rules();
        assert!(!rules.is_empty(), "@media (max-width: 900px) should be included at 800px viewport");
        assert!(rules.iter().any(|r| r.declarations.iter().any(|d| {
            d.name.as_ref() == "color"
        })));
    }

    #[test]
    fn test_media_query_min_width_does_not_apply_at_800px() {
        // @media (min-width: 900px) must NOT apply at the 800px viewport.
        let ss = parse_css("@media (min-width: 900px) { p { color: red; } }");
        let rules = ss.all_rules();
        assert!(
            rules.is_empty(),
            "@media (min-width: 900px) should be excluded at 800px viewport, got {} rules",
            rules.len()
        );
    }

    #[test]
    fn test_media_screen_always_matches() {
        // @media screen without a condition must always match.
        let ss = parse_css("@media screen { p { color: blue; } }");
        let rules = ss.all_rules();
        assert!(!rules.is_empty(), "@media screen should always match");
    }

    #[test]
    fn test_media_print_never_matches() {
        // @media print must never match for screen rendering.
        let ss = parse_css("@media print { p { color: blue; } }");
        let rules = ss.all_rules();
        assert!(rules.is_empty(), "@media print should never match for screen rendering");
    }

    #[test]
    fn test_media_screen_and_min_width_applies_at_800px() {
        // @media screen and (min-width: 600px) must apply when viewport >= 600px.
        let ss = parse_css("@media screen and (min-width: 600px) { p { color: green; } }");
        let rules = ss.all_rules();
        assert!(!rules.is_empty(), "@media screen and (min-width: 600px) should apply at 800px");
    }

    #[test]
    fn test_media_prefers_color_scheme_dark_activates() {
        // @media (prefers-color-scheme: dark) must activate (headless renderer is dark).
        let ss = parse_css("@media (prefers-color-scheme: dark) { body { background: black; } }");
        let rules = ss.all_rules();
        assert!(
            !rules.is_empty(),
            "@media (prefers-color-scheme: dark) should be included"
        );
        assert!(rules.iter().any(|r| r.declarations.iter().any(|d| {
            d.name.as_ref() == "background"
        })));
    }

    #[test]
    fn test_media_prefers_color_scheme_light_does_not_activate() {
        // @media (prefers-color-scheme: light) must NOT activate.
        let ss = parse_css("@media (prefers-color-scheme: light) { body { background: white; } }");
        let rules = ss.all_rules();
        assert!(
            rules.is_empty(),
            "@media (prefers-color-scheme: light) should be excluded, got {} rules",
            rules.len()
        );
    }

    #[test]
    fn test_translate_y_percent_parses_correctly() {
        let v = parse_value("translateY(-50%)");
        match v {
            Value::Transform(ops) => {
                assert_eq!(ops.len(), 1);
                match &ops[0] {
                    TransformOp::Translate(x, y) => {
                        assert_eq!(*x, TranslateLength::Px(OrderedFloat(0.0)));
                        assert_eq!(*y, TranslateLength::Percent(OrderedFloat(-50.0)));
                    }
                    other => panic!("expected Translate, got {:?}", other),
                }
            }
            other => panic!("expected Transform, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_xy_px_parses_correctly() {
        let v = parse_value("translate(10px, 20px)");
        match v {
            Value::Transform(ops) => {
                assert_eq!(ops.len(), 1);
                match &ops[0] {
                    TransformOp::Translate(x, y) => {
                        assert_eq!(*x, TranslateLength::Px(OrderedFloat(10.0)));
                        assert_eq!(*y, TranslateLength::Px(OrderedFloat(20.0)));
                    }
                    other => panic!("expected Translate, got {:?}", other),
                }
            }
            other => panic!("expected Transform, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_x_percent_parses_correctly() {
        let v = parse_value("translateX(50%)");
        match v {
            Value::Transform(ops) => {
                assert_eq!(ops.len(), 1);
                match &ops[0] {
                    TransformOp::Translate(x, y) => {
                        assert_eq!(*x, TranslateLength::Percent(OrderedFloat(50.0)));
                        assert_eq!(*y, TranslateLength::Px(OrderedFloat(0.0)));
                    }
                    other => panic!("expected Translate, got {:?}", other),
                }
            }
            other => panic!("expected Transform, got {:?}", other),
        }
    }
}

fn parse_transform_list(val: &str) -> Vec<TransformOp> {
    let mut ops = Vec::new();
    let parts = split_respecting_parens(val);
    for part in parts {
        let part = part.trim();
        if part.is_empty() { continue; }

        if let Some(open) = part.find('(') {
            let name = &part[..open].to_lowercase();
            let args_str = &part[open + 1..part.len() - 1];
            let args: Vec<&str> = args_str.split(',').map(|s| s.trim()).collect();

            match name.as_str() {
                "translate" => {
                    let x = parse_translate_length(args.get(0).copied().unwrap_or("0"));
                    let y = parse_translate_length(args.get(1).copied().unwrap_or("0"));
                    ops.push(TransformOp::Translate(x, y));
                }
                "translatex" => {
                    let x = parse_translate_length(args.get(0).copied().unwrap_or("0"));
                    ops.push(TransformOp::Translate(x, TranslateLength::Px(OrderedFloat(0.0))));
                }
                "translatey" => {
                    let y = parse_translate_length(args.get(0).copied().unwrap_or("0"));
                    ops.push(TransformOp::Translate(TranslateLength::Px(OrderedFloat(0.0)), y));
                }
                "scale" => {
                    let x = args.get(0).and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
                    let y = args.get(1).and_then(|s| s.parse::<f32>().ok()).unwrap_or(x);
                    ops.push(TransformOp::Scale(OrderedFloat(x), OrderedFloat(y)));
                }
                "rotate" => {
                    let mut rad = 0.0;
                    if let Some(arg) = args.get(0) {
                        let arg = arg.trim();
                        if arg.ends_with("deg") {
                            let deg = arg.trim_end_matches("deg").parse::<f32>().unwrap_or(0.0);
                            rad = deg.to_radians();
                        } else {
                            rad = arg.parse::<f32>().unwrap_or(0.0);
                        }
                    }
                    ops.push(TransformOp::Rotate(OrderedFloat(rad)));
                }
                "matrix" => {
                    if args.len() == 6 {
                        let a = args[0].parse().unwrap_or(1.0);
                        let b = args[1].parse().unwrap_or(0.0);
                        let c = args[2].parse().unwrap_or(0.0);
                        let d = args[3].parse().unwrap_or(1.0);
                        let e = args[4].parse().unwrap_or(0.0);
                        let f = args[5].parse().unwrap_or(0.0);
                        ops.push(TransformOp::Matrix(OrderedFloat(a), OrderedFloat(b), OrderedFloat(c), OrderedFloat(d), OrderedFloat(e), OrderedFloat(f)));
                    }
                }
                _ => {}
            }
        }
    }
    ops
}

fn parse_translate_length(s: &str) -> TranslateLength {
    let s = s.trim();
    if s.ends_with('%') {
        let v = s.trim_end_matches('%').parse::<f32>().unwrap_or(0.0);
        TranslateLength::Percent(OrderedFloat(v))
    } else {
        let v = s.trim_end_matches("px")
                  .trim_end_matches("rem")
                  .trim_end_matches("em")
                  .parse::<f32>()
                  .unwrap_or(0.0);
        TranslateLength::Px(OrderedFloat(v))
    }
}
