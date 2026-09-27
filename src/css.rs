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
    /// Every layer of a comma-separated `box-shadow` list, in paint order
    /// (first layer on top). Stored under the `box-shadow-layers` key; the
    /// `box-shadow` key keeps only the first layer for older readers.
    BoxShadowList(Vec<BoxShadow>),
    /// All custom properties (`--name`) in effect on an element, stored once under
    /// the reserved key `--` and shared (via `Arc`) with descendants that do not
    /// declare their own.
    CustomProps(CustomProps),
}

/// Shared custom-property map. Equality and hashing use the `Arc` identity so
/// that comparing or hashing style maps stays cheap with thousands of variables.
#[derive(Debug, Clone)]
pub struct CustomProps(pub Arc<HashMap<Arc<str>, Value>>);

impl PartialEq for CustomProps {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Hash for CustomProps {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
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
            Value::BoxShadowList(list) => list.hash(state),
            Value::CustomProps(p) => p.hash(state),
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
    /// CSS Grid fractional unit (flexible tracks).
    Fr,
    /// Root-relative em (`rem`), resolved against the root element font size.
    Rem,
    /// `vmin` / `vmax` viewport units.
    Vmin,
    Vmax,
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

// ── Stylesheet parsing ────────────────────────────────────────────────────────

/// Parse a CSS stylesheet.
///
/// The parser is brace-, string- and parenthesis-aware, so `url(...)` values that
/// contain `@`, `;` or `}` and quoted strings with braces do not break rule
/// boundaries. Comments are removed first. `@media` blocks are evaluated against the
/// fixed viewport (see [`VIEWPORT_WIDTH_PX`]); non-matching blocks are dropped.
/// `@supports` and `@layer` blocks are flattened into the surrounding rule list;
/// other at-rules (`@font-face`, `@keyframes`, `@import`, ...) are skipped.
pub fn parse_css(source: &str) -> Stylesheet {
    let cleaned = strip_comments(source);
    let mut items = Vec::new();
    parse_rule_list(&cleaned, &mut items);
    Stylesheet { items }
}

/// Remove `/* ... */` comments and HTML comment delimiters, respecting strings.
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            let end = skip_string(b, i);
            out.extend_from_slice(&b[i..end]);
            i = end;
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let mut j = i + 2;
            while j + 1 < b.len() && !(b[j] == b'*' && b[j + 1] == b'/') {
                j += 1;
            }
            i = (j + 2).min(b.len());
            out.push(b' ');
        } else if c == b'<' && b[i..].starts_with(b"<!--") {
            i += 4;
            out.push(b' ');
        } else if c == b'-' && b[i..].starts_with(b"-->") {
            i += 3;
            out.push(b' ');
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Given `b[start]` is a quote character, return the index just past the closing quote.
fn skip_string(b: &[u8], start: usize) -> usize {
    let q = b[start];
    let mut i = start + 1;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q || b[i] == b'\n' {
            return i + 1;
        }
        i += 1;
    }
    b.len()
}

/// Find the first byte in `stops` at nesting depth 0 (outside strings, parens and
/// brackets), starting at `start`. Returns `(index, byte)`.
fn find_top_level(b: &[u8], start: usize, stops: &[u8]) -> Option<(usize, u8)> {
    let mut depth = 0i32;
    let mut i = start;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' | b'\'' => {
                i = skip_string(b, i);
                continue;
            }
            b'\\' => {
                i += 2;
                continue;
            }
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                if depth > 0 {
                    depth -= 1;
                } else if stops.contains(&c) {
                    return Some((i, c));
                }
            }
            _ => {
                if depth == 0 && stops.contains(&c) {
                    return Some((i, c));
                }
            }
        }
        i += 1;
    }
    None
}

/// `b[open]` is `{`; return the index of the matching `}` (or `b.len()`).
fn matching_brace(b: &[u8], open: usize) -> usize {
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                i = skip_string(b, i);
                continue;
            }
            b'\\' => {
                i += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Split `s` on `sep` occurring at nesting depth 0 (outside strings/parens/brackets).
pub fn split_top_level(s: &str, sep: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    loop {
        match find_top_level(b, start, &[sep]) {
            Some((idx, _)) => {
                parts.push(&s[start..idx]);
                start = idx + 1;
            }
            None => {
                parts.push(&s[start..]);
                break;
            }
        }
    }
    parts
}

fn parse_rule_list(src: &str, out: &mut Vec<RuleOrAtRule>) {
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b'}' || b[i] == b';' {
            i += 1;
            continue;
        }
        if b[i] == b'@' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b'_') {
                j += 1;
            }
            let name = src[i + 1..j].to_ascii_lowercase();
            let Some((end, ch)) = find_top_level(b, j, &[b'{', b';']) else {
                break;
            };
            let prelude = src[j..end].trim().to_string();
            if ch == b';' {
                i = end + 1;
                continue;
            }
            let close = matching_brace(b, end);
            let inner = &src[end + 1..close.min(src.len())];
            match name.as_str() {
                "media" => {
                    if evaluate_media_query(&prelude) {
                        let mut nested = Vec::new();
                        parse_rule_list(inner, &mut nested);
                        let mut rules = Vec::new();
                        flatten_items(nested, &mut rules);
                        out.push(RuleOrAtRule::AtRule(AtRule::Media { query: prelude, rules }));
                    }
                }
                "supports" => {
                    if evaluate_supports(&prelude) {
                        parse_rule_list(inner, out);
                    }
                }
                "layer" | "document" | "-moz-document" | "scope" | "starting-style" => {
                    parse_rule_list(inner, out);
                }
                _ => {}
            }
            i = close + 1;
            continue;
        }
        let Some((end, ch)) = find_top_level(b, i, &[b'{', b';', b'}']) else {
            break;
        };
        if ch != b'{' {
            i = end + 1;
            continue;
        }
        let close = matching_brace(b, end);
        let prelude = src[i..end].trim();
        let body = &src[end + 1..close.min(src.len())];
        let selectors = parse_selector_list(prelude);
        if !selectors.is_empty() {
            // Nested rules (CSS nesting) are not supported: take declarations only.
            let declarations = parse_declaration_block(body);
            out.push(RuleOrAtRule::Rule(Rule { selectors, declarations }));
        }
        i = close + 1;
    }
}

fn flatten_items(items: Vec<RuleOrAtRule>, rules: &mut Vec<Rule>) {
    for item in items {
        match item {
            RuleOrAtRule::Rule(r) => rules.push(r),
            RuleOrAtRule::AtRule(AtRule::Media { rules: r, .. }) => rules.extend(r),
            _ => {}
        }
    }
}

fn evaluate_supports(prelude: &str) -> bool {
    let p = prelude.trim().to_ascii_lowercase();
    !p.starts_with("not")
}

// ── Media queries ─────────────────────────────────────────────────────────────

/// Viewport width used for `@media` query evaluation.
///
/// The render canvas is fixed at 800 px (see `src/main.rs`). All `@media`
/// conditions are evaluated against this value so that responsive stylesheets
/// activate the rules that were authored for an ~800 px viewport.
const VIEWPORT_WIDTH_PX: f32 = 800.0;
/// Viewport height used for `@media` height features.
const VIEWPORT_HEIGHT_PX: f32 = 768.0;

/// Returns true if the media query list (the text between `@media` and `{`)
/// matches the fixed screen viewport. A comma-separated list matches when any
/// entry matches. Each entry may start with `only`/`not`, a media type, and a
/// chain of `(feature)` conditions joined by `and` (or `or`).
pub fn evaluate_media_query(query_str: &str) -> bool {
    let q = query_str.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    split_top_level(&q, b',').into_iter().any(|mq| evaluate_single_media_query(mq))
}

fn evaluate_single_media_query(mq: &str) -> bool {
    let mut s = mq.trim();
    let mut negate = false;
    if let Some(rest) = s.strip_prefix("not ") {
        negate = true;
        s = rest.trim();
    } else if let Some(rest) = s.strip_prefix("only ") {
        s = rest.trim();
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut results: Vec<bool> = Vec::new();
    let mut any_or = false;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'(' {
            // Balanced condition.
            let mut depth = 0;
            let mut j = i;
            while j < b.len() {
                if b[j] == b'(' {
                    depth += 1;
                } else if b[j] == b')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            let inner = &s[i + 1..j.min(s.len())];
            results.push(media_condition_matches(inner));
            i = j + 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'(' {
            j += 1;
        }
        let word = &s[i..j];
        match word {
            "and" => {}
            "or" => any_or = true,
            "all" | "screen" => results.push(true),
            "not" => {
                negate = !negate;
            }
            _ => results.push(false), // print, speech, tv, ...
        }
        i = j;
    }
    let matched = if results.is_empty() {
        true
    } else if any_or {
        results.iter().any(|r| *r)
    } else {
        results.iter().all(|r| *r)
    };
    matched != negate
}

fn media_length_px(v: &str) -> Option<f32> {
    let v = v.trim();
    match parse_value(v) {
        Value::Length(n, Unit::Px) => Some(n),
        Value::Length(n, Unit::Em) | Value::Length(n, Unit::Rem) => Some(n * 16.0),
        Value::Number(n) => Some(n),
        _ => None,
    }
}

/// Evaluate the inside of one `( ... )` media condition.
fn media_condition_matches(inner: &str) -> bool {
    let inner = inner.trim();
    if inner.starts_with('(') || inner.starts_with("not ") {
        // Nested boolean condition: evaluate as a query.
        return evaluate_single_media_query(inner);
    }
    // Range syntax: `width >= 600px`, `400px <= width <= 700px`.
    for op in [">=", "<=", ">", "<", "="] {
        if inner.contains(op) && !inner.contains(':') {
            let parts: Vec<&str> = inner.split(|c| c == '<' || c == '>' || c == '=').map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
            if parts.len() == 2 {
                let (name, val, name_first) = if parts[0].chars().next().map_or(false, |c| c.is_ascii_alphabetic()) {
                    (parts[0], parts[1], true)
                } else {
                    (parts[1], parts[0], false)
                };
                let actual = match name {
                    "width" | "device-width" => VIEWPORT_WIDTH_PX,
                    "height" | "device-height" => VIEWPORT_HEIGHT_PX,
                    _ => return false,
                };
                let Some(v) = media_length_px(val) else { return false };
                let op_str: String = inner.chars().filter(|c| matches!(c, '<' | '>' | '=')).collect();
                let (lhs, rhs) = if name_first { (actual, v) } else { (v, actual) };
                return match op_str.as_str() {
                    ">=" => lhs >= rhs,
                    "<=" => lhs <= rhs,
                    ">" => lhs > rhs,
                    "<" => lhs < rhs,
                    _ => (lhs - rhs).abs() < 0.01,
                };
            }
            return false;
        }
    }
    let (name, value) = match inner.split_once(':') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => (inner, ""),
    };
    let w = VIEWPORT_WIDTH_PX;
    let h = VIEWPORT_HEIGHT_PX;
    match name {
        "min-width" | "min-device-width" => media_length_px(value).map_or(false, |v| w >= v),
        "max-width" | "max-device-width" => media_length_px(value).map_or(false, |v| w <= v),
        "width" | "device-width" => media_length_px(value).map_or(false, |v| (w - v).abs() < 0.01),
        "min-height" | "min-device-height" => media_length_px(value).map_or(false, |v| h >= v),
        "max-height" | "max-device-height" => media_length_px(value).map_or(false, |v| h <= v),
        "orientation" => value == if w >= h { "landscape" } else { "portrait" },
        // Headless renderer defaults to dark preference.
        "prefers-color-scheme" => value == "dark",
        "prefers-reduced-motion" => value == "no-preference",
        "hover" | "any-hover" => value.is_empty() || value == "hover",
        "pointer" | "any-pointer" => value.is_empty() || value == "fine",
        "color" => true,
        "min-resolution" | "-webkit-min-device-pixel-ratio" | "min--moz-device-pixel-ratio" => {
            let v = value.trim_end_matches("dppx").trim_end_matches("x");
            if value.ends_with("dpi") {
                value.trim_end_matches("dpi").parse::<f32>().map_or(false, |d| d <= 96.0)
            } else {
                v.parse::<f32>().map_or(false, |d| d <= 1.0)
            }
        }
        "max-resolution" | "-webkit-max-device-pixel-ratio" => true,
        "aspect-ratio" | "min-aspect-ratio" | "max-aspect-ratio" => {
            let ratio = w / h;
            let parse_ratio = |s: &str| -> Option<f32> {
                let (a, b) = s.split_once('/').unwrap_or((s, "1"));
                Some(a.trim().parse::<f32>().ok()? / b.trim().parse::<f32>().ok()?)
            };
            match (name, parse_ratio(value)) {
                ("min-aspect-ratio", Some(r)) => ratio >= r,
                ("max-aspect-ratio", Some(r)) => ratio <= r,
                ("aspect-ratio", Some(r)) => (ratio - r).abs() < 0.001,
                _ => false,
            }
        }
        _ => false,
    }
}

// ── Declarations ──────────────────────────────────────────────────────────────

/// Parse the inside of a `{ ... }` declaration block (or an inline `style` attribute).
pub fn parse_declaration_block(body: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    for decl in split_top_level(body, b';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let b = decl.as_bytes();
        let Some((colon, _)) = find_top_level(b, 0, &[b':']) else { continue };
        let name = decl[..colon].trim();
        if name.is_empty() || name.contains('{') || name.contains('}') {
            continue;
        }
        let name = if name.starts_with("--") { name.to_string() } else { name.to_ascii_lowercase() };
        let (value, important) = strip_important(decl[colon + 1..].trim());
        if value.is_empty() && !name.starts_with("--") {
            continue;
        }
        parse_declaration(&name, value, important, &mut out);
    }
    out
}

/// Split a trailing `!important` off a declaration value.
pub fn strip_important(value: &str) -> (&str, bool) {
    let v = value.trim_end();
    if let Some(pos) = v.rfind('!') {
        let tail = v[pos + 1..].trim();
        if tail.eq_ignore_ascii_case("important") {
            return (v[..pos].trim_end(), true);
        }
    }
    (v, false)
}

fn push_decl(out: &mut Vec<Declaration>, name: &str, value: Value, important: bool) {
    out.push(Declaration { name: intern(name), value, important });
}

/// Longhands that a shorthand property sets. Used by the cascade to let a
/// later shorthand whose value contains `var()` reset earlier longhands.
pub fn shorthand_longhands(name: &str) -> &'static [&'static str] {
    match name {
        "margin" => &["margin-top", "margin-right", "margin-bottom", "margin-left"],
        "padding" => &["padding-top", "padding-right", "padding-bottom", "padding-left"],
        "inset" => &["top", "right", "bottom", "left"],
        "border" => &[
            "border-top-width", "border-right-width", "border-bottom-width", "border-left-width",
            "border-top-style", "border-right-style", "border-bottom-style", "border-left-style",
            "border-top-color", "border-right-color", "border-bottom-color", "border-left-color",
            "border-width", "border-style", "border-color",
        ],
        "border-top" => &["border-top-width", "border-top-style", "border-top-color"],
        "border-right" => &["border-right-width", "border-right-style", "border-right-color"],
        "border-bottom" => &["border-bottom-width", "border-bottom-style", "border-bottom-color"],
        "border-left" => &["border-left-width", "border-left-style", "border-left-color"],
        "border-width" => &["border-top-width", "border-right-width", "border-bottom-width", "border-left-width"],
        "border-style" => &["border-top-style", "border-right-style", "border-bottom-style", "border-left-style"],
        "border-color" => &["border-top-color", "border-right-color", "border-bottom-color", "border-left-color"],
        "flex" => &["flex-grow", "flex-shrink", "flex-basis"],
        "flex-flow" => &["flex-direction", "flex-wrap"],
        "gap" | "grid-gap" => &["row-gap", "column-gap"],
        "font" => &["font-style", "font-variant", "font-weight", "font-size", "line-height", "font-family"],
        "background" => &[
            "background-color", "background-image", "background-position", "background-size",
            "background-repeat", "background-origin", "background-clip",
        ],
        "overflow" => &["overflow-x", "overflow-y"],
        "box-shadow" => &["box-shadow-layers"],
        "list-style" => &["list-style-type"],
        "place-items" => &["align-items", "justify-items"],
        "place-content" => &["align-content", "justify-content"],
        "place-self" => &["align-self", "justify-self"],
        _ => &[],
    }
}

/// Parse one declaration, expanding shorthands into longhands.
///
/// Custom properties (`--x`) keep their raw text. Any other value that contains
/// `var()` cannot be parsed until computed-value time, so it is stored as
/// `Value::RawCustomProp(raw)` under its own (possibly shorthand) name; the style
/// system substitutes the variables and then calls this function again.
pub fn parse_declaration(name: &str, raw: &str, important: bool, out: &mut Vec<Declaration>) {
    let raw = raw.trim();
    if name.starts_with("--") {
        push_decl(out, name, Value::RawCustomProp(intern(raw)), important);
        return;
    }
    if raw.contains("var(") {
        push_decl(out, name, Value::RawCustomProp(intern(raw)), important);
        return;
    }
    // background shorthand + image/position/size/repeat/origin/clip longhands (src/background.rs)
    if crate::background::is_background_property(name) {
        crate::background::push_background_declarations(name, raw, important, out);
        return;
    }
    let lower = raw.to_ascii_lowercase();
    // CSS-wide keywords apply to every longhand of a shorthand.
    if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer") {
        let longhands = shorthand_longhands(name);
        let kw = Value::Keyword(intern(if lower == "revert" || lower == "revert-layer" { "unset" } else { &lower }));
        if longhands.is_empty() {
            push_decl(out, name, kw, important);
        } else {
            for lh in longhands {
                push_decl(out, lh, kw.clone(), important);
            }
            if matches!(name, "overflow" | "background") {
                push_decl(out, name, kw, important);
            }
        }
        return;
    }
    match name {
        "margin" | "padding" => {
            let parts = split_respecting_parens(raw);
            if let Some((t, r, bt, l)) = quad(&parts) {
                push_decl(out, &format!("{}-top", name), parse_value(t), important);
                push_decl(out, &format!("{}-right", name), parse_value(r), important);
                push_decl(out, &format!("{}-bottom", name), parse_value(bt), important);
                push_decl(out, &format!("{}-left", name), parse_value(l), important);
            }
        }
        "inset" => {
            let parts = split_respecting_parens(raw);
            if let Some((t, r, bt, l)) = quad(&parts) {
                push_decl(out, "top", parse_value(t), important);
                push_decl(out, "right", parse_value(r), important);
                push_decl(out, "bottom", parse_value(bt), important);
                push_decl(out, "left", parse_value(l), important);
            }
        }
        "border" => {
            let (w, s, c) = parse_border_parts(raw);
            for side in ["top", "right", "bottom", "left"] {
                push_decl(out, &format!("border-{}-width", side), w.clone(), important);
                push_decl(out, &format!("border-{}-style", side), s.clone(), important);
                push_decl(out, &format!("border-{}-color", side), c.clone(), important);
            }
            // Legacy uniform keys read by the painter.
            push_decl(out, "border-width", w, important);
            push_decl(out, "border-style", s, important);
            push_decl(out, "border-color", c, important);
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let (w, s, c) = parse_border_parts(raw);
            push_decl(out, &format!("{}-width", name), w, important);
            push_decl(out, &format!("{}-style", name), s, important);
            push_decl(out, &format!("{}-color", name), c, important);
        }
        "border-width" | "border-style" | "border-color" => {
            let suffix = &name["border-".len()..];
            let parts = split_respecting_parens(raw);
            if let Some((t, r, bt, l)) = quad(&parts) {
                let conv = |s: &str| -> Value {
                    if suffix == "width" { parse_border_width(s) } else { parse_value(s) }
                };
                push_decl(out, &format!("border-top-{}", suffix), conv(t), important);
                push_decl(out, &format!("border-right-{}", suffix), conv(r), important);
                push_decl(out, &format!("border-bottom-{}", suffix), conv(bt), important);
                push_decl(out, &format!("border-left-{}", suffix), conv(l), important);
                push_decl(out, name, conv(t), important);
            }
        }
        "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" => {
            push_decl(out, name, parse_border_width(raw), important);
        }
        // border-radius shorthand: "border-radius: <tl> [<tr> [<br> [<bl>]]]"
        // CSS allows up to 4 corner values.  We use the top-left (first) value as a
        // uniform radius for all corners — sufficient for the rounded-input / button
        // use-case (Google search bar, etc.).  The "/" elliptical syntax is not supported.
        "border-radius" => {
            let first = raw.split_whitespace().next().unwrap_or("0");
            let first = first.split('/').next().unwrap_or("0").trim();
            push_decl(out, name, parse_value(first), important);
        }
        "box-shadow" => {
            // `box-shadow` keeps the first layer (older painters read a single shadow);
            // `box-shadow-layers` carries every layer. `none` yields an empty list so it
            // still overrides earlier shadows in the cascade.
            let layers: Vec<BoxShadow> = split_top_level(raw, b',')
                .into_iter()
                .filter_map(|layer| parse_box_shadow(layer.trim()))
                .collect();
            if let Some(first) = layers.first() {
                push_decl(out, name, Value::BoxShadow(first.clone()), important);
            }
            push_decl(out, "box-shadow-layers", Value::BoxShadowList(layers), important);
        }
        // font shorthand: [style] [variant] [weight] [stretch] <size>[/<line-height>] <family>
        "font" => {
            let parts = split_respecting_parens(raw);
            let size_idx = parts.iter().position(|part| {
                let size_part = part.split_once('/').map(|(sz, _)| sz).unwrap_or(part);
                matches!(parse_value(size_part), Value::Length(_, _))
                    || matches!(size_part, "xx-small" | "x-small" | "small" | "medium" | "large" | "x-large" | "xx-large" | "smaller" | "larger")
            });
            let Some(idx) = size_idx else {
                // System font keywords (caption, menu, ...): ignore.
                return;
            };
            let mut style = Value::Keyword(intern("normal"));
            let mut weight = Value::Keyword(intern("normal"));
            for p in &parts[..idx] {
                let pl = p.to_ascii_lowercase();
                match pl.as_str() {
                    "italic" | "oblique" => style = Value::Keyword(intern(&pl)),
                    "bold" | "bolder" | "lighter" => weight = Value::Keyword(intern(&pl)),
                    _ => {
                        if let Ok(n) = pl.parse::<f32>() {
                            weight = Value::Number(n);
                        }
                    }
                }
            }
            let size_token = parts[idx].as_str();
            let (size_str, lh_inline) = match size_token.split_once('/') {
                Some((s, l)) => (s, Some(l.to_string())),
                None => (size_token, None),
            };
            let mut rest_start = idx + 1;
            let mut line_height = lh_inline.filter(|l| !l.is_empty());
            if line_height.is_none() {
                // "13px / 1.5 sans" (spaces around the slash)
                if parts.get(idx + 1).map(|s| s.as_str()) == Some("/") {
                    line_height = parts.get(idx + 2).cloned();
                    rest_start = idx + 3;
                } else if let Some(stripped) = parts.get(idx + 1).and_then(|s| s.strip_prefix('/')) {
                    if !stripped.is_empty() {
                        line_height = Some(stripped.to_string());
                        rest_start = idx + 2;
                    }
                }
            }
            push_decl(out, "font-style", style, important);
            push_decl(out, "font-weight", weight, important);
            push_decl(out, "font-size", parse_value(size_str), important);
            let lh_val = match line_height {
                Some(l) if l != "normal" => parse_value(&l),
                _ => Value::Keyword(intern("normal")),
            };
            push_decl(out, "line-height", lh_val, important);
            if rest_start < parts.len() {
                let family = parts[rest_start..].join(" ");
                push_decl(out, "font-family", Value::Keyword(intern(&family)), important);
            }
        }
        // flex shorthand: "flex: <grow> [<shrink> [<basis>]]" or keyword
        "flex" => {
            let parts = split_respecting_parens(raw);
            let num = |s: &str| s.parse::<f32>().ok();
            let (g, s, basis) = match parts.len() {
                0 => return,
                1 => match parts[0].as_str() {
                    "none" => (0.0, 0.0, Value::Keyword(intern("auto"))),
                    "auto" => (1.0, 1.0, Value::Keyword(intern("auto"))),
                    "initial" => (0.0, 1.0, Value::Keyword(intern("auto"))),
                    p => match num(p) {
                        // Single unitless number expands to `flex: <n> 1 0%`.
                        Some(n) => (n, 1.0, Value::Length(0.0, Unit::Percent)),
                        None => (1.0, 1.0, parse_value(p)),
                    },
                },
                2 => match (num(&parts[0]), num(&parts[1])) {
                    (Some(g), Some(s)) => (g, s, Value::Length(0.0, Unit::Percent)),
                    (Some(g), None) => (g, 1.0, parse_value(&parts[1])),
                    (None, Some(g)) => (g, 1.0, parse_value(&parts[0])),
                    _ => return,
                },
                _ => match (num(&parts[0]), num(&parts[1])) {
                    (Some(g), Some(s)) => (g, s, parse_value(&parts[2])),
                    _ => return,
                },
            };
            push_decl(out, "flex-grow", Value::Number(g), important);
            push_decl(out, "flex-shrink", Value::Number(s), important);
            push_decl(out, "flex-basis", basis, important);
        }
        "flex-flow" => {
            for p in raw.split_whitespace() {
                let pl = p.to_ascii_lowercase();
                if matches!(pl.as_str(), "row" | "row-reverse" | "column" | "column-reverse") {
                    push_decl(out, "flex-direction", Value::Keyword(intern(&pl)), important);
                } else {
                    push_decl(out, "flex-wrap", Value::Keyword(intern(&pl)), important);
                }
            }
        }
        // gap shorthand: "gap: <row-gap> [<col-gap>]"
        "gap" | "grid-gap" => {
            let parts = split_respecting_parens(raw);
            let row_val = parts.first().map(|s| parse_value(s)).unwrap_or(Value::Number(0.0));
            let col_val = parts.get(1).map(|s| parse_value(s)).unwrap_or_else(|| row_val.clone());
            push_decl(out, "row-gap", row_val, important);
            push_decl(out, "column-gap", col_val, important);
        }
        // list-style shorthand: only list-style-type matters for layout/paint.
        "list-style" => {
            let mut type_val = None;
            for p in raw.split_whitespace() {
                let pl = p.to_ascii_lowercase();
                if matches!(pl.as_str(), "inside" | "outside") || pl.starts_with("url(") {
                    continue;
                }
                type_val = Some(Value::Keyword(intern(&pl)));
                break;
            }
            push_decl(out, "list-style-type", type_val.unwrap_or(Value::Keyword(intern("none"))), important);
        }
        "overflow" => {
            let parts: Vec<&str> = raw.split_whitespace().collect();
            let x = parts.first().copied().unwrap_or("visible").to_ascii_lowercase();
            let y = parts.get(1).copied().map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| x.clone());
            push_decl(out, "overflow", Value::Keyword(intern(&x)), important);
            push_decl(out, "overflow-x", Value::Keyword(intern(&x)), important);
            push_decl(out, "overflow-y", Value::Keyword(intern(&y)), important);
        }
        "text-decoration" | "text-decoration-line" => {
            let line = raw
                .split_whitespace()
                .map(|p| p.to_ascii_lowercase())
                .find(|p| matches!(p.as_str(), "none" | "underline" | "overline" | "line-through"))
                .unwrap_or_else(|| "none".to_string());
            push_decl(out, "text-decoration", Value::Keyword(intern(&line)), important);
        }
        "place-items" | "place-content" | "place-self" => {
            let parts: Vec<&str> = raw.split_whitespace().collect();
            let a = parts.first().copied().unwrap_or("normal");
            let b = parts.get(1).copied().unwrap_or(a);
            let suffix = &name["place-".len()..];
            push_decl(out, &format!("align-{}", suffix), Value::Keyword(intern(a)), important);
            push_decl(out, &format!("justify-{}", suffix), Value::Keyword(intern(b)), important);
        }
        // CSS Grid track lists: store the raw value string as a Keyword so that
        // layout can later call parse_track_list() on it to expand repeat() etc.
        // parse_value() would incorrectly interpret "1fr 1fr" as a single Fr value.
        "grid-template-columns" | "grid-template-rows" => {
            push_decl(out, name, Value::Keyword(intern(raw)), important);
        }
        "font-family" | "content" | "transition" | "animation" | "grid-template-areas" | "grid-area"
        | "quotes" | "will-change" | "font-feature-settings"
        | "clip" | "clip-path" | "mask" | "filter" | "backdrop-filter" | "counter-reset" | "counter-increment" => {
            push_decl(out, name, Value::Keyword(intern(raw)), important);
        }
        _ => {
            push_decl(out, name, parse_value(raw), important);
        }
    }
}

fn quad(parts: &[String]) -> Option<(&str, &str, &str, &str)> {
    match parts.len() {
        1 => Some((&parts[0], &parts[0], &parts[0], &parts[0])),
        2 => Some((&parts[0], &parts[1], &parts[0], &parts[1])),
        3 => Some((&parts[0], &parts[1], &parts[2], &parts[1])),
        4 => Some((&parts[0], &parts[1], &parts[2], &parts[3])),
        _ => None,
    }
}

fn is_border_style_keyword(s: &str) -> bool {
    matches!(
        s,
        "none" | "hidden" | "dotted" | "dashed" | "solid" | "double" | "groove" | "ridge" | "inset" | "outset"
    )
}

fn parse_border_width(s: &str) -> Value {
    match s.trim().to_ascii_lowercase().as_str() {
        "thin" => Value::Length(1.0, Unit::Px),
        "medium" => Value::Length(3.0, Unit::Px),
        "thick" => Value::Length(5.0, Unit::Px),
        other => match parse_value(other) {
            Value::Number(n) => Value::Length(n, Unit::Px),
            v => v,
        },
    }
}

/// Split a `border` / `border-<side>` shorthand into (width, style, color).
/// Omitted parts take their initial values: `medium`, `none`, `currentcolor`.
fn parse_border_parts(val: &str) -> (Value, Value, Value) {
    let mut width = Value::Length(3.0, Unit::Px);
    let mut style = Value::Keyword(intern("none"));
    let mut color = Value::Keyword(intern("currentcolor"));
    for part in split_respecting_parens(val) {
        let pl = part.to_ascii_lowercase();
        if is_border_style_keyword(&pl) {
            style = Value::Keyword(intern(&pl));
        } else if matches!(pl.as_str(), "thin" | "medium" | "thick") {
            width = parse_border_width(&pl);
        } else if let Some(c) = parse_color(&part) {
            color = Value::Color(c);
        } else if pl == "currentcolor" {
            color = Value::Keyword(intern("currentcolor"));
        } else {
            match parse_value(&part) {
                v @ Value::Length(_, _) => width = v,
                Value::Number(n) => width = Value::Length(n, Unit::Px),
                _ => {}
            }
        }
    }
    (width, style, color)
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
}

pub fn parse_border_shorthand_pub(val: &str, declarations: &mut HashMap<String, Value>) {
    let (w, s, c) = parse_border_parts(val);
    declarations.insert("border-width".to_string(), w);
    declarations.insert("border-style".to_string(), s);
    declarations.insert("border-color".to_string(), c);
}

// ── Selectors ─────────────────────────────────────────────────────────────────

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

/// Attribute selector operators beyond plain presence / exact equality.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AttrOp {
    Equals,
    /// `~=` whitespace-separated word match
    Includes,
    /// `|=` exact or prefix followed by `-`
    DashMatch,
    /// `^=`
    Prefix,
    /// `$=`
    Suffix,
    /// `*=`
    Substring,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttrOpSelector {
    pub name: String,
    pub op: AttrOp,
    pub value: String,
    pub case_insensitive: bool,
}

/// Structured pseudo-classes understood by the style system.
#[derive(Debug, Clone)]
pub enum PseudoClass {
    Not(Vec<Selector>),
    /// `:is()`, `:matches()`, `:any()`
    Is(Vec<Selector>),
    /// `:where()` — like `:is()` but contributes zero specificity.
    Where(Vec<Selector>),
    FirstChild,
    LastChild,
    OnlyChild,
    /// `:nth-child(an+b [of S])`
    NthChild(i32, i32, Option<Vec<Selector>>),
    NthLastChild(i32, i32, Option<Vec<Selector>>),
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthOfType(i32, i32),
    NthLastOfType(i32, i32),
    Empty,
    Root,
    Hover,
    Focus,
    /// `:link` / `:any-link` — `a`/`area` elements with an `href`.
    Link,
    Checked,
    Disabled,
    Enabled,
    /// A pseudo-class this engine never matches (`:visited`, `:active`, `:has()`, unknown).
    Never(String),
}

#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub class: Vec<String>,
    /// Presence / case-sensitive equality attribute selectors.
    pub attributes: Vec<AttributeSelector>,
    /// Name of the first pseudo-class, kept for callers that only need to know that
    /// a pseudo-class is present. Structured data lives in `pseudos`.
    pub pseudo_class: Option<String>,
    /// Pseudo-element (`"before"` or `"after"`), set when the selector ends with
    /// `::before` or `::after`.  Single-colon pseudo-classes (`:hover`, `:focus`,
    /// `:root`) stay in `pseudo_class`.
    pub pseudo_element: Option<String>,
    pub combinator: Option<Combinator>,
    pub ancestor: Option<Box<Selector>>,
    /// `*` was written explicitly (a constraint that matches every element).
    pub universal: bool,
    /// Attribute selectors with operators other than presence / equality.
    pub attr_ops: Vec<AttrOpSelector>,
    /// Structured pseudo-classes of this compound selector.
    pub pseudos: Vec<PseudoClass>,
    /// The selector can never match a real element (unsupported pseudo-element such
    /// as `::placeholder`, or a parse failure).
    pub never_matches: bool,
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

fn add_spec(a: (usize, usize, usize), b: (usize, usize, usize)) -> (usize, usize, usize) {
    (a.0 + b.0, a.1 + b.1, a.2 + b.2)
}

fn max_spec(list: &[Selector]) -> (usize, usize, usize) {
    list.iter().map(|s| s.specificity()).max().unwrap_or((0, 0, 0))
}

impl Selector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn specificity(&self) -> (usize, usize, usize) {
        let mut spec = (0, 0, 0);
        if self.id.is_some() { spec.0 += 1; }
        spec.1 += self.class.len() + self.attributes.len() + self.attr_ops.len();
        if self.tag.is_some() { spec.2 += 1; }
        if self.pseudo_element.is_some() { spec.2 += 1; }
        if self.pseudos.is_empty() && self.pseudo_class.is_some() {
            spec.1 += 1;
        }
        for p in &self.pseudos {
            let s = match p {
                PseudoClass::Not(list) | PseudoClass::Is(list) => max_spec(list),
                PseudoClass::Where(_) => (0, 0, 0),
                PseudoClass::NthChild(_, _, Some(list)) | PseudoClass::NthLastChild(_, _, Some(list)) => {
                    add_spec((0, 1, 0), max_spec(list))
                }
                _ => (0, 1, 0),
            };
            spec = add_spec(spec, s);
        }
        if let Some(ref d) = self.ancestor {
            spec = add_spec(spec, d.specificity());
        }
        spec
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

    /// True when the compound has at least one condition (so that an empty,
    /// failed-to-parse selector never matches everything).
    pub fn has_constraint(&self) -> bool {
        self.universal
            || self.tag.is_some()
            || self.id.is_some()
            || !self.class.is_empty()
            || !self.attributes.is_empty()
            || !self.attr_ops.is_empty()
            || !self.pseudos.is_empty()
            || self.pseudo_class.is_some()
            || self.pseudo_element.is_some()
    }
}

/// Parse a comma-separated selector list. Selectors that fail to parse are dropped.
pub fn parse_selector_list(s: &str) -> Vec<Selector> {
    split_top_level(s, b',')
        .into_iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .filter_map(parse_complex_selector)
        .collect()
}

/// Parse a single complex selector (no top-level commas). On failure a selector
/// that never matches is returned.
pub fn parse_selector(s: &str) -> Selector {
    parse_complex_selector(s.trim()).unwrap_or_else(|| Selector { never_matches: true, ..Selector::default() })
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii()
}

struct SelParser<'a> {
    chars: Vec<char>,
    pos: usize,
    _src: &'a str,
}

impl<'a> SelParser<'a> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }
    fn skip_ws(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
        self.pos > start
    }
    fn ident(&mut self) -> Option<String> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c == '\\' {
                self.pos += 1;
                // Hex escape: up to 6 hex digits followed by optional whitespace.
                let mut hex = String::new();
                while hex.len() < 6 && matches!(self.peek(), Some(h) if h.is_ascii_hexdigit()) {
                    hex.push(self.bump().unwrap());
                }
                if !hex.is_empty() {
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                    if matches!(self.peek(), Some(w) if w == ' ') {
                        self.pos += 1;
                    }
                } else if let Some(ch) = self.bump() {
                    out.push(ch);
                }
            } else if is_ident_char(c) {
                out.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if out.is_empty() { None } else { Some(out) }
    }
    /// Read a balanced `( ... )` argument; the opening paren is at `self.pos`.
    fn paren_args(&mut self) -> Option<String> {
        if self.peek() != Some('(') {
            return None;
        }
        self.pos += 1;
        let mut depth = 1;
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                }
                '"' | '\'' => {
                    out.push(c);
                    while let Some(d) = self.bump() {
                        out.push(d);
                        if d == c {
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
            out.push(c);
        }
        Some(out)
    }
}

/// Parse `an+b` syntax. Returns (a, b).
fn parse_nth(s: &str) -> Option<(i32, i32)> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(npos) = s.find('n') {
        let a_str = &s[..npos];
        let a = match a_str {
            "" | "+" => 1,
            "-" => -1,
            _ => a_str.parse::<i32>().ok()?,
        };
        let b_str = &s[npos + 1..];
        let b = if b_str.is_empty() { 0 } else { b_str.trim_start_matches('+').parse::<i32>().ok()? };
        Some((a, b))
    } else {
        Some((0, s.trim_start_matches('+').parse::<i32>().ok()?))
    }
}

fn parse_complex_selector(s: &str) -> Option<Selector> {
    let mut p = SelParser { chars: s.chars().collect(), pos: 0, _src: s };
    let mut root: Option<Selector> = None;
    let mut pending: Option<Combinator> = None;
    p.skip_ws();
    loop {
        if p.peek().is_none() {
            break;
        }
        let had_ws = p.skip_ws();
        match p.peek() {
            None => break,
            Some('>') => {
                p.bump();
                pending = Some(Combinator::Child);
                p.skip_ws();
                continue;
            }
            Some('+') => {
                p.bump();
                pending = Some(Combinator::NextSibling);
                p.skip_ws();
                continue;
            }
            Some('~') => {
                p.bump();
                pending = Some(Combinator::SubsequentSibling);
                p.skip_ws();
                continue;
            }
            _ => {}
        }
        let _ = had_ws;
        let mut compound = parse_compound(&mut p)?;
        if let Some(prev) = root.take() {
            compound.combinator = Some(pending.take().unwrap_or(Combinator::Descendant));
            compound.ancestor = Some(Box::new(prev));
        } else if pending.is_some() {
            // Leading combinator (relative selector) — unsupported.
            return None;
        }
        root = Some(compound);
    }
    if pending.is_some() {
        return None;
    }
    root
}

fn parse_compound(p: &mut SelParser) -> Option<Selector> {
    let mut sel = Selector::new();
    let start = p.pos;
    loop {
        match p.peek() {
            Some('*') => {
                p.bump();
                sel.universal = true;
                // Namespace prefix `*|tag`
                if p.peek() == Some('|') {
                    p.bump();
                }
            }
            Some('#') => {
                p.bump();
                sel.id = Some(p.ident()?);
            }
            Some('.') => {
                p.bump();
                sel.class.push(p.ident()?);
            }
            Some('[') => {
                p.bump();
                let mut content = String::new();
                let mut quote: Option<char> = None;
                loop {
                    let c = p.bump()?;
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        }
                        content.push(c);
                        continue;
                    }
                    if c == '"' || c == '\'' {
                        quote = Some(c);
                        content.push(c);
                        continue;
                    }
                    if c == ']' {
                        break;
                    }
                    content.push(c);
                }
                parse_attribute_selector(&content, &mut sel)?;
            }
            Some(':') => {
                p.bump();
                let double = if p.peek() == Some(':') {
                    p.bump();
                    true
                } else {
                    false
                };
                let name = p.ident()?.to_ascii_lowercase();
                let args = if p.peek() == Some('(') { p.paren_args() } else { None };
                let legacy_element = matches!(name.as_str(), "before" | "after" | "first-line" | "first-letter");
                if double || legacy_element {
                    if name == "before" || name == "after" {
                        sel.pseudo_element = Some(name);
                    } else {
                        sel.pseudo_element = Some(name);
                        sel.never_matches = true;
                    }
                    continue;
                }
                if sel.pseudo_class.is_none() {
                    sel.pseudo_class = Some(match &args {
                        Some(a) => format!("{}({})", name, a),
                        None => name.clone(),
                    });
                }
                let pc = match (name.as_str(), args) {
                    ("not", Some(a)) => PseudoClass::Not(parse_selector_list(&a)),
                    ("is" | "matches" | "any" | "-webkit-any", Some(a)) => PseudoClass::Is(parse_selector_list(&a)),
                    ("where", Some(a)) => PseudoClass::Where(parse_selector_list(&a)),
                    ("first-child", None) => PseudoClass::FirstChild,
                    ("last-child", None) => PseudoClass::LastChild,
                    ("only-child", None) => PseudoClass::OnlyChild,
                    ("first-of-type", None) => PseudoClass::FirstOfType,
                    ("last-of-type", None) => PseudoClass::LastOfType,
                    ("only-of-type", None) => PseudoClass::OnlyOfType,
                    ("nth-child" | "nth-last-child", Some(a)) => {
                        let (expr, of) = match a.to_ascii_lowercase().find(" of ") {
                            Some(idx) => (a[..idx].to_string(), Some(parse_selector_list(&a[idx + 4..]))),
                            None => (a.clone(), None),
                        };
                        match parse_nth(&expr) {
                            Some((an, bn)) => {
                                if name == "nth-child" {
                                    PseudoClass::NthChild(an, bn, of)
                                } else {
                                    PseudoClass::NthLastChild(an, bn, of)
                                }
                            }
                            None => PseudoClass::Never(name.clone()),
                        }
                    }
                    ("nth-of-type" | "nth-last-of-type", Some(a)) => match parse_nth(&a) {
                        Some((an, bn)) => {
                            if name == "nth-of-type" {
                                PseudoClass::NthOfType(an, bn)
                            } else {
                                PseudoClass::NthLastOfType(an, bn)
                            }
                        }
                        None => PseudoClass::Never(name.clone()),
                    },
                    ("empty", None) => PseudoClass::Empty,
                    ("root", None) => PseudoClass::Root,
                    ("hover", None) => PseudoClass::Hover,
                    ("focus", None) => PseudoClass::Focus,
                    ("link" | "any-link", None) => PseudoClass::Link,
                    ("checked", None) => PseudoClass::Checked,
                    ("disabled", None) => PseudoClass::Disabled,
                    ("enabled", None) => PseudoClass::Enabled,
                    _ => PseudoClass::Never(name.clone()),
                };
                sel.pseudos.push(pc);
            }
            Some(c) if is_ident_char(c) || c == '\\' => {
                if p.pos != start {
                    // A type selector must come first in a compound.
                    return None;
                }
                let tag = p.ident()?;
                if p.peek() == Some('|') {
                    // Namespace prefix: `ns|tag`
                    p.bump();
                    if p.peek() == Some('*') {
                        p.bump();
                        sel.universal = true;
                    } else {
                        sel.tag = Some(p.ident()?.to_ascii_lowercase());
                    }
                } else {
                    sel.tag = Some(tag.to_ascii_lowercase());
                }
            }
            _ => break,
        }
    }
    if p.pos == start {
        return None;
    }
    Some(sel)
}

fn parse_attribute_selector(content: &str, sel: &mut Selector) -> Option<()> {
    let content = content.trim();
    let ops = ["~=", "|=", "^=", "$=", "*=", "="];
    let mut found: Option<(usize, &str)> = None;
    for op in ops {
        if let Some(idx) = content.find(op) {
            if found.map_or(true, |(fi, _)| idx < fi) {
                found = Some((idx, op));
            }
        }
    }
    let Some((idx, op)) = found else {
        let name = content.trim().trim_start_matches("*|").to_string();
        if name.is_empty() {
            return None;
        }
        sel.attributes.push(AttributeSelector { name: name.to_ascii_lowercase(), value: AttributeMatch::Exists });
        return Some(());
    };
    let name = content[..idx].trim().to_ascii_lowercase();
    let mut rest = content[idx + op.len()..].trim();
    let mut case_insensitive = false;
    // Flags: trailing ` i` / ` s` after the value.
    let value: String;
    if rest.starts_with('"') || rest.starts_with('\'') {
        let q = rest.chars().next().unwrap();
        let end = rest[1..].find(q).map(|e| e + 1)?;
        value = rest[1..end].to_string();
        let flags = rest[end + 1..].trim();
        case_insensitive = flags.eq_ignore_ascii_case("i");
    } else {
        let mut parts = rest.split_whitespace();
        value = parts.next().unwrap_or("").to_string();
        if let Some(flag) = parts.next() {
            case_insensitive = flag.eq_ignore_ascii_case("i");
        }
        rest = "";
    }
    let _ = rest;
    if op == "=" && !case_insensitive {
        sel.attributes.push(AttributeSelector { name, value: AttributeMatch::Equals(value) });
    } else {
        let op = match op {
            "~=" => AttrOp::Includes,
            "|=" => AttrOp::DashMatch,
            "^=" => AttrOp::Prefix,
            "$=" => AttrOp::Suffix,
            "*=" => AttrOp::Substring,
            _ => AttrOp::Equals,
        };
        sel.attr_ops.push(AttrOpSelector { name, op, value, case_insensitive });
    }
    Some(())
}

impl AttrOpSelector {
    /// Test an attribute value against this selector.
    pub fn matches_value(&self, actual: &str) -> bool {
        let (a, v) = if self.case_insensitive {
            (actual.to_ascii_lowercase(), self.value.to_ascii_lowercase())
        } else {
            (actual.to_string(), self.value.clone())
        };
        match self.op {
            AttrOp::Equals => a == v,
            AttrOp::Includes => !v.is_empty() && a.split_whitespace().any(|w| w == v),
            AttrOp::DashMatch => a == v || a.starts_with(&format!("{}-", v)),
            AttrOp::Prefix => !v.is_empty() && a.starts_with(&v),
            AttrOp::Suffix => !v.is_empty() && a.ends_with(&v),
            AttrOp::Substring => !v.is_empty() && a.contains(&v),
        }
    }
}

/// Check whether `index` (1-based) satisfies `an+b`.
pub fn nth_matches(a: i32, b: i32, index: i32) -> bool {
    if a == 0 {
        return index == b;
    }
    let diff = index - b;
    diff % a == 0 && diff / a >= 0
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


// ── Values ────────────────────────────────────────────────────────────────────

/// Split a numeric CSS token into (number, unit suffix). Returns `None` when the
/// token does not start with a number.
fn split_number_unit(s: &str) -> Option<(f32, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    let mut seen_digit = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        seen_digit = true;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            seen_digit = true;
        }
    }
    if !seen_digit {
        return None;
    }
    // Exponent
    if i + 1 < b.len() && (b[i] == b'e' || b[i] == b'E') && (b[i + 1].is_ascii_digit() || ((b[i + 1] == b'-' || b[i + 1] == b'+') && i + 2 < b.len() && b[i + 2].is_ascii_digit())) {
        i += 2;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    let _ = digits_start;
    let num: f32 = s[..i].parse().ok()?;
    Some((num, &s[i..]))
}

/// Parse a single length / percentage / number token (no whitespace).
pub fn parse_length_token(val: &str) -> Option<Value> {
    let (n, unit) = split_number_unit(val)?;
    let unit_l = unit.to_ascii_lowercase();
    Some(match unit_l.as_str() {
        "" => Value::Number(n),
        "px" => Value::Length(n, Unit::Px),
        "%" => Value::Length(n, Unit::Percent),
        "em" => Value::Length(n, Unit::Em),
        "rem" => Value::Length(n, Unit::Rem),
        "ex" => Value::Length(n * 0.5, Unit::Em),
        "ch" => Value::Length(n * 0.5, Unit::Em),
        "vw" => Value::Length(n, Unit::Vw),
        "vh" | "dvh" | "svh" | "lvh" => Value::Length(n, Unit::Vh),
        "vmin" => Value::Length(n, Unit::Vmin),
        "vmax" => Value::Length(n, Unit::Vmax),
        "fr" => Value::Length(n, Unit::Fr),
        "pt" => Value::Length(n * 96.0 / 72.0, Unit::Px),
        "pc" => Value::Length(n * 16.0, Unit::Px),
        "in" => Value::Length(n * 96.0, Unit::Px),
        "cm" => Value::Length(n * 96.0 / 2.54, Unit::Px),
        "mm" => Value::Length(n * 96.0 / 25.4, Unit::Px),
        "q" => Value::Length(n * 96.0 / 101.6, Unit::Px),
        _ => return None,
    })
}

pub fn parse_value(val: &str) -> Value {
    let val = val.trim();
    // Strip !important
    let val = strip_important(val).0;

    // Intrinsic sizing keywords (CSS Sizing Level 3)
    if val == "min-content" || val == "max-content" || val == "fit-content" {
        return Value::Keyword(intern(val));
    }

    // var(--custom-property) or var(--custom-property, fallback)
    if val.starts_with("var(") && val.ends_with(')') && find_top_level(val.as_bytes(), 4, &[b')']).map(|(i, _)| i) == Some(val.len() - 1) {
        let inner = &val[4..val.len() - 1]; // strip "var(" and ")"
        let (name_str, fallback_str) = match find_top_level(inner.as_bytes(), 0, &[b',']) {
            Some((pos, _)) => (&inner[..pos], Some(inner[pos + 1..].trim())),
            None => (inner, None),
        };
        let name_str = name_str.trim();
        if name_str.starts_with("--") {
            let fallback = fallback_str.map(|fb| Box::new(parse_value(fb)));
            return Value::CssVar { name: intern(name_str), fallback };
        }
    }

    let lower = val.to_ascii_lowercase();

    // gradient: linear-gradient(...) or radial-gradient(...)
    if lower.starts_with("linear-gradient(") || lower.starts_with("radial-gradient(") {
        if let Some(g) = parse_gradient(val) {
            return Value::Gradient(g);
        }
        return Value::Keyword(intern(val));
    }

    // transform: translate(...) rotate(...)
    if val.contains('(')
        && (lower.starts_with("translate") || lower.starts_with("scale") || lower.starts_with("rotate") || lower.starts_with("matrix"))
    {
        let ops = parse_transform_list(val);
        if !ops.is_empty() {
            return Value::Transform(ops);
        }
    }
    // fit-content(<length>) — e.g. fit-content(300px)
    if lower.starts_with("fit-content(") && val.ends_with(')') {
        let inner = &val["fit-content(".len()..val.len() - 1];
        let px_val = inner.trim().trim_end_matches("px").parse::<f32>().unwrap_or(0.0);
        return Value::FitContent(px_val);
    }
    // calc(): fold to a single length when all terms share one unit.
    if lower.starts_with("calc(") || lower.starts_with("-webkit-calc(") {
        if let Some(v) = fold_simple_calc(&lower) {
            return v;
        }
        return Value::Keyword(intern(&lower));
    }

    if let Some(v) = parse_length_token(val) {
        return v;
    }
    if let Some(color) = parse_color(val) {
        return Value::Color(color);
    }
    // Keywords are ASCII case-insensitive; keep strings / urls verbatim.
    if val.contains('"') || val.contains('\'') || lower.contains("url(") {
        Value::Keyword(intern(val))
    } else {
        Value::Keyword(intern(&lower))
    }
}

/// Evaluate `calc()` expressions whose terms all resolve to the same unit
/// (e.g. `calc(10px + 2px)`, `calc(100% / 3)`). Mixed-unit expressions return
/// `None` and are resolved at layout time by [`eval_calc`].
fn fold_simple_calc(expr: &str) -> Option<Value> {
    let mut unit: Option<Unit> = None;
    let v = eval_calc_with(expr, &mut |n, u| {
        match (&unit, u) {
            (_, None) => Some(n),
            (None, Some(u)) => {
                unit = Some(u.clone());
                Some(n)
            }
            (Some(existing), Some(u)) if existing == u => Some(n),
            _ => None,
        }
    })?;
    Some(match unit {
        Some(u) => Value::Length(v, u),
        None => Value::Number(v),
    })
}

/// Evaluate a `calc()` expression, resolving each dimension with `resolve`.
/// `resolve(number, unit)` receives `None` for plain numbers and must return the
/// value in the target unit (usually px).
pub fn eval_calc(expr: &str, resolve: &mut dyn FnMut(f32, Option<&Unit>) -> Option<f32>) -> Option<f32> {
    eval_calc_with(expr, resolve)
}

fn eval_calc_with(input: &str, resolve: &mut dyn FnMut(f32, Option<&Unit>) -> Option<f32>) -> Option<f32> {
    let e = input.trim();
    let e = e.strip_prefix("-webkit-").unwrap_or(e);
    let inner = e.strip_prefix("calc").unwrap_or(e).trim();
    // Tokenize
    #[derive(Debug, Clone)]
    enum Tok {
        Num(f32, Option<Unit>),
        Op(char),
        LParen,
        RParen,
    }
    let mut toks: Vec<Tok> = Vec::new();
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            toks.push(Tok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            toks.push(Tok::RParen);
            i += 1;
            continue;
        }
        let prev_is_value = matches!(toks.last(), Some(Tok::Num(..)) | Some(Tok::RParen));
        if (c == '*' || c == '/') || ((c == '+' || c == '-') && prev_is_value) {
            toks.push(Tok::Op(c));
            i += 1;
            continue;
        }
        // "calc" keyword nested
        if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if word == "calc" {
                i = j;
                continue;
            }
            return None;
        }
        let mut j = i;
        if chars[j] == '+' || chars[j] == '-' {
            j += 1;
        }
        while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
            j += 1;
        }
        while j < chars.len() && (chars[j].is_ascii_alphabetic() || chars[j] == '%') {
            j += 1;
        }
        if j == i {
            return None;
        }
        let tok: String = chars[i..j].iter().collect();
        match parse_length_token(&tok)? {
            Value::Number(n) => toks.push(Tok::Num(n, None)),
            Value::Length(n, u) => toks.push(Tok::Num(n, Some(u))),
            _ => return None,
        }
        i = j;
    }
    // Recursive descent over tokens.
    fn expr(t: &[Tok], pos: &mut usize, r: &mut dyn FnMut(f32, Option<&Unit>) -> Option<f32>) -> Option<(f32, bool)> {
        let (mut v, mut has_unit) = term(t, pos, r)?;
        while let Some(Tok::Op(op)) = t.get(*pos) {
            if *op != '+' && *op != '-' {
                break;
            }
            let op = *op;
            *pos += 1;
            let (rhs, ru) = term(t, pos, r)?;
            has_unit |= ru;
            if op == '+' { v += rhs } else { v -= rhs }
        }
        Some((v, has_unit))
    }
    fn term(t: &[Tok], pos: &mut usize, r: &mut dyn FnMut(f32, Option<&Unit>) -> Option<f32>) -> Option<(f32, bool)> {
        let (mut v, mut has_unit) = factor(t, pos, r)?;
        while let Some(Tok::Op(op)) = t.get(*pos) {
            if *op != '*' && *op != '/' {
                break;
            }
            let op = *op;
            *pos += 1;
            let (rhs, ru) = factor(t, pos, r)?;
            has_unit |= ru;
            if op == '*' {
                v *= rhs
            } else if rhs != 0.0 {
                v /= rhs
            } else {
                return None;
            }
        }
        Some((v, has_unit))
    }
    fn factor(t: &[Tok], pos: &mut usize, r: &mut dyn FnMut(f32, Option<&Unit>) -> Option<f32>) -> Option<(f32, bool)> {
        match t.get(*pos)?.clone() {
            Tok::Num(n, u) => {
                *pos += 1;
                let v = r(n, u.as_ref())?;
                Some((v, u.is_some()))
            }
            Tok::LParen => {
                *pos += 1;
                let v = expr(t, pos, r)?;
                if matches!(t.get(*pos), Some(Tok::RParen)) {
                    *pos += 1;
                }
                Some(v)
            }
            Tok::Op('-') => {
                *pos += 1;
                let (v, u) = factor(t, pos, r)?;
                Some((-v, u))
            }
            _ => None,
        }
    }
    let mut pos = 0;
    let (v, _) = expr(&toks, &mut pos, resolve)?;
    Some(v)
}

fn parse_color_component(s: &str, max: f32) -> Option<f32> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%') {
        return Some(p.trim().parse::<f32>().ok()? / 100.0 * max);
    }
    if s == "none" {
        return Some(0.0);
    }
    s.parse::<f32>().ok()
}

fn parse_alpha(s: &str) -> Option<u8> {
    let s = s.trim();
    let a = if let Some(p) = s.strip_suffix('%') {
        p.trim().parse::<f32>().ok()? / 100.0
    } else {
        s.parse::<f32>().ok()?
    };
    Some((a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Split color function arguments: accepts both `1, 2, 3, .5` and `1 2 3 / .5`.
fn color_args(content: &str) -> (Vec<String>, Option<String>) {
    if content.contains(',') {
        let parts: Vec<String> = content.split(',').map(|p| p.trim().to_string()).collect();
        if parts.len() == 4 {
            return (parts[..3].to_vec(), Some(parts[3].clone()));
        }
        return (parts, None);
    }
    let (main, alpha) = match content.split_once('/') {
        Some((m, a)) => (m, Some(a.trim().to_string())),
        None => (content, None),
    };
    (main.split_whitespace().map(|p| p.to_string()).collect(), alpha)
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        let h = |i: usize, n: usize| -> Option<u8> {
            let part = &hex[i..i + n];
            let part = if n == 1 { part.repeat(2) } else { part.to_string() };
            u8::from_str_radix(&part, 16).ok()
        };
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return match hex.len() {
            3 => Some(Color { r: h(0, 1)?, g: h(1, 1)?, b: h(2, 1)?, a: 255 }),
            4 => Some(Color { r: h(0, 1)?, g: h(1, 1)?, b: h(2, 1)?, a: h(3, 1)? }),
            6 => Some(Color { r: h(0, 2)?, g: h(2, 2)?, b: h(4, 2)?, a: 255 }),
            8 => Some(Color { r: h(0, 2)?, g: h(2, 2)?, b: h(4, 2)?, a: h(6, 2)? }),
            _ => None,
        };
    }
    if (s.starts_with("rgba(") || s.starts_with("rgb(")) && s.ends_with(')') {
        let content = &s[s.find('(')? + 1..s.len() - 1];
        let (parts, alpha) = color_args(content);
        if parts.len() >= 3 {
            let r = parse_color_component(&parts[0], 255.0)?.round().clamp(0.0, 255.0) as u8;
            let g = parse_color_component(&parts[1], 255.0)?.round().clamp(0.0, 255.0) as u8;
            let b = parse_color_component(&parts[2], 255.0)?.round().clamp(0.0, 255.0) as u8;
            let a = match alpha {
                Some(a) => parse_alpha(&a)?,
                None => 255,
            };
            return Some(Color { r, g, b, a });
        }
        return None;
    }
    if (s.starts_with("hsl(") || s.starts_with("hsla(")) && s.ends_with(')') {
        let content = &s[s.find('(')? + 1..s.len() - 1];
        let (parts, alpha) = color_args(content);
        if parts.len() >= 3 {
            let h_str = parts[0].trim_end_matches("deg");
            let h: f32 = h_str.parse().ok()?;
            let s_pct: f32 = parts[1].trim_end_matches('%').parse().ok()?;
            let l_pct: f32 = parts[2].trim_end_matches('%').parse().ok()?;
            let (r, g, b) = hsl_to_rgb(h.rem_euclid(360.0), s_pct / 100.0, l_pct / 100.0);
            let a = match alpha {
                Some(a) => parse_alpha(&a)?,
                None => 255,
            };
            return Some(Color { r, g, b, a });
        }
        return None;
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
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
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
    fn test_box_shadow_multiple_layers_are_kept_separately() {
        let ss = parse_css("div { box-shadow: 0 0 0 1px #e5e5e5, 0 1px 2px rgba(0,0,0,.04); }");
        let rules = ss.all_rules();
        let layers = rules[0].declarations.iter().find_map(|d| match (&*d.name, &d.value) {
            ("box-shadow-layers", Value::BoxShadowList(l)) => Some(l.clone()),
            _ => None,
        }).expect("box-shadow-layers declaration");
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].spread, OrderedFloat(1.0));
        assert_eq!(layers[0].color, Color { r: 0xe5, g: 0xe5, b: 0xe5, a: 255 });
        assert_eq!(layers[1].offset_y, OrderedFloat(1.0));
        assert_eq!(layers[1].blur, OrderedFloat(2.0));
        assert_eq!(layers[1].color.a, 10);
        // The legacy single-shadow key holds the first layer only.
        let first = rules[0].declarations.iter().find_map(|d| match (&*d.name, &d.value) {
            ("box-shadow", Value::BoxShadow(s)) => Some(s.clone()),
            _ => None,
        }).expect("box-shadow declaration");
        assert_eq!(first.spread, OrderedFloat(1.0));
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
            d.name.as_ref() == "background-color"
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

    /// `1fr` must parse as `Value::Length(1.0, Unit::Fr)`.
    #[test]
    fn test_parse_fr_unit() {
        let v = parse_value("1fr");
        assert_eq!(v, Value::Length(1.0, Unit::Fr));
        let v2 = parse_value("2.5fr");
        assert_eq!(v2, Value::Length(2.5, Unit::Fr));
    }

    /// `grid-template-columns: 1fr 1fr` should be stored as a raw Keyword.
    #[test]
    fn test_grid_template_columns_stored_as_keyword() {
        let ss = parse_css("div { grid-template-columns: 1fr 1fr; }");
        let rule = match &ss.items[0] {
            RuleOrAtRule::Rule(r) => r,
            _ => panic!("expected rule"),
        };
        let decl = rule.declarations.iter()
            .find(|d| d.name.as_ref() == "grid-template-columns")
            .expect("grid-template-columns declaration");
        assert!(matches!(&decl.value, Value::Keyword(k) if k.as_ref() == "1fr 1fr"),
            "expected Keyword(\"1fr 1fr\"), got {:?}", decl.value);
    }

    /// `parse_track_list("1fr 1fr")` → two Fr tracks.
    #[test]
    fn test_parse_track_list_two_fr() {
        let tracks = parse_track_list("1fr 1fr");
        assert_eq!(tracks.len(), 2);
        assert!(matches!(&tracks[0], Value::Length(v, Unit::Fr) if (*v - 1.0).abs() < 1e-5));
        assert!(matches!(&tracks[1], Value::Length(v, Unit::Fr) if (*v - 1.0).abs() < 1e-5));
    }

    /// `parse_track_list("200px 1fr")` → fixed + fractional.
    #[test]
    fn test_parse_track_list_mixed_px_fr() {
        let tracks = parse_track_list("200px 1fr");
        assert_eq!(tracks.len(), 2);
        assert!(matches!(&tracks[0], Value::Length(v, Unit::Px) if (*v - 200.0).abs() < 1e-5));
        assert!(matches!(&tracks[1], Value::Length(v, Unit::Fr) if (*v - 1.0).abs() < 1e-5));
    }

    /// `parse_track_list("repeat(3, 1fr)")` → three equal fr tracks.
    #[test]
    fn test_parse_track_list_repeat_three_fr() {
        let tracks = parse_track_list("repeat(3, 1fr)");
        assert_eq!(tracks.len(), 3,
            "repeat(3, 1fr) should produce 3 tracks, got {}", tracks.len());
        for t in &tracks {
            assert!(matches!(t, Value::Length(v, Unit::Fr) if (*v - 1.0).abs() < 1e-5),
                "each track should be 1fr, got {:?}", t);
        }
    }

    /// `parse_track_list("auto")` → single auto track.
    #[test]
    fn test_parse_track_list_auto() {
        let tracks = parse_track_list("auto");
        assert_eq!(tracks.len(), 1);
        assert!(matches!(&tracks[0], Value::Keyword(k) if k.as_ref() == "auto"));
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

/// Parse a CSS grid track list string (e.g. `"1fr 1fr"`, `"200px 1fr"`,
/// `"repeat(3, 1fr)"`, `"auto"`) into a `Vec<Value>`.
///
/// Each entry is one of:
/// - `Value::Length(n, Unit::Px)`      — fixed pixel track
/// - `Value::Length(n, Unit::Percent)` — percentage track
/// - `Value::Length(n, Unit::Fr)`      — fractional unit
/// - `Value::Keyword("auto")`          — auto-sized track
pub fn parse_track_list(val: &str) -> Vec<Value> {
    let val = val.trim();
    let mut tracks: Vec<Value> = Vec::new();

    // Expand repeat(...) before splitting on spaces.
    let expanded = expand_repeat_tracks(val);

    for token in expanded.split_whitespace() {
        let token = token.trim();
        if token.is_empty() { continue; }
        tracks.push(parse_track_token(token));
    }
    tracks
}

/// Expand `repeat(N, <track-list>)` within a track value string.
fn expand_repeat_tracks(val: &str) -> String {
    let lower = val.to_ascii_lowercase();
    if !lower.starts_with("repeat(") {
        // No repeat() at the start — return as-is.
        return val.to_string();
    }

    let mut result = String::with_capacity(val.len());

    // Find matching closing paren for repeat(...)
    let inner_start = "repeat(".len();
    let inner_chars: Vec<char> = val[inner_start..].chars().collect();
    let mut depth = 1i32;
    let mut end = 0;
    for (i, &c) in inner_chars.iter().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 { end = i; break; }
            }
            _ => {}
        }
    }
    let inner: String = inner_chars[..end].iter().collect();
    let rest: String = inner_chars[end + 1..].iter().collect();

    // Parse count and track definition.
    if let Some(comma_pos) = inner.find(',') {
        let count_str = inner[..comma_pos].trim();
        let track_def = inner[comma_pos + 1..].trim();
        if let Ok(n) = count_str.parse::<usize>() {
            for i in 0..n {
                if i > 0 { result.push(' '); }
                result.push_str(track_def);
            }
        }
    }

    // Recursively expand whatever comes after the closing paren.
    let rest_expanded = expand_repeat_tracks(rest.trim());
    if !rest_expanded.trim().is_empty() {
        result.push(' ');
        result.push_str(rest_expanded.trim());
    }
    result
}

fn parse_track_token(token: &str) -> Value {
    let t = token.trim().to_ascii_lowercase();
    if t == "auto" {
        Value::Keyword(intern("auto"))
    } else if t == "min-content" || t == "max-content" {
        Value::Keyword(intern(&t))
    } else if t.ends_with("fr") {
        let n = t.trim_end_matches("fr").parse::<f32>().unwrap_or(1.0);
        Value::Length(n, Unit::Fr)
    } else if t.ends_with("px") {
        let n = t.trim_end_matches("px").parse::<f32>().unwrap_or(0.0);
        Value::Length(n, Unit::Px)
    } else if t.ends_with('%') {
        let n = t.trim_end_matches('%').parse::<f32>().unwrap_or(0.0);
        Value::Length(n, Unit::Percent)
    } else if t.ends_with("em") || t.ends_with("rem") {
        let n = t.trim_end_matches("rem").trim_end_matches("em").parse::<f32>().unwrap_or(1.0);
        Value::Length(n, Unit::Em)
    } else {
        // fallback: try as keyword
        Value::Keyword(intern(token.trim()))
    }
}
