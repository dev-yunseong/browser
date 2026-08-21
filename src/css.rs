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
    /// An unevaluated `calc()` / `clamp()` / `min()` / `max()` expression.
    ///
    /// These cannot be folded at parse time because their operands may be
    /// viewport- or container-relative, so the tree is kept and resolved once
    /// those sizes are known.
    Math(MathExpr),
}

/// A parsed CSS math expression.
#[derive(Debug, Clone, PartialEq)]
pub enum MathExpr {
    /// A leaf length or plain number.
    Value(f32, Option<Unit>),
    Add(Box<MathExpr>, Box<MathExpr>),
    Sub(Box<MathExpr>, Box<MathExpr>),
    Mul(Box<MathExpr>, Box<MathExpr>),
    Div(Box<MathExpr>, Box<MathExpr>),
    Min(Vec<MathExpr>),
    Max(Vec<MathExpr>),
    /// `clamp(min, preferred, max)`.
    Clamp(Box<MathExpr>, Box<MathExpr>, Box<MathExpr>),
}

/// The lengths a math expression may be resolved against.
#[derive(Debug, Clone, Copy)]
pub struct MathContext {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub font_size: f32,
    /// Basis for percentages, or `None` when it is not yet known — an expression
    /// containing a percentage then stays unresolved rather than guessing.
    pub percent_basis: Option<f32>,
}

impl MathExpr {
    /// Evaluate to pixels, or `None` if an operand cannot be resolved yet.
    pub fn resolve(&self, ctx: &MathContext) -> Option<f32> {
        match self {
            MathExpr::Value(n, unit) => match unit {
                None | Some(Unit::Px) => Some(*n),
                Some(Unit::Em) => Some(n * ctx.font_size),
                Some(Unit::Vw) => Some(ctx.viewport_width * (n / 100.0)),
                Some(Unit::Vh) => Some(ctx.viewport_height * (n / 100.0)),
                Some(Unit::Percent) => ctx.percent_basis.map(|b| b * (n / 100.0)),
                Some(Unit::Fr) => None,
            },
            MathExpr::Add(a, b) => Some(a.resolve(ctx)? + b.resolve(ctx)?),
            MathExpr::Sub(a, b) => Some(a.resolve(ctx)? - b.resolve(ctx)?),
            MathExpr::Mul(a, b) => Some(a.resolve(ctx)? * b.resolve(ctx)?),
            MathExpr::Div(a, b) => {
                let divisor = b.resolve(ctx)?;
                if divisor == 0.0 { None } else { Some(a.resolve(ctx)? / divisor) }
            }
            MathExpr::Min(args) => args
                .iter()
                .map(|a| a.resolve(ctx))
                .collect::<Option<Vec<f32>>>()?
                .into_iter()
                .reduce(f32::min),
            MathExpr::Max(args) => args
                .iter()
                .map(|a| a.resolve(ctx))
                .collect::<Option<Vec<f32>>>()?
                .into_iter()
                .reduce(f32::max),
            MathExpr::Clamp(lo, val, hi) => {
                let (lo, val, hi) = (lo.resolve(ctx)?, val.resolve(ctx)?, hi.resolve(ctx)?);
                // Per spec clamp() is max(lo, min(val, hi)), so a lo above hi wins.
                Some(val.min(hi).max(lo))
            }
        }
    }

    /// `true` if any leaf is a percentage, i.e. resolution needs a basis.
    pub fn needs_percent_basis(&self) -> bool {
        match self {
            MathExpr::Value(_, unit) => matches!(unit, Some(Unit::Percent)),
            MathExpr::Add(a, b) | MathExpr::Sub(a, b) | MathExpr::Mul(a, b) | MathExpr::Div(a, b) => {
                a.needs_percent_basis() || b.needs_percent_basis()
            }
            MathExpr::Min(args) | MathExpr::Max(args) => args.iter().any(Self::needs_percent_basis),
            MathExpr::Clamp(a, b, c) => {
                a.needs_percent_basis() || b.needs_percent_basis() || c.needs_percent_basis()
            }
        }
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
            // The tree is hashed by its debug form: math values are rare and this
            // avoids a hand-written Hash for every operator.
            Value::Math(expr) => format!("{expr:?}").hash(state),
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

/// Remove `/* ... */` comments from a stylesheet.
///
/// Each comment becomes a single space rather than nothing: CSS treats a comment
/// as a token boundary, so `a/**/b` is two identifiers and must not be joined
/// into one. Quoted strings and unquoted `url(...)` tokens are copied through
/// verbatim, so a `/*` inside `content: "/*"` or inside an inline SVG data URI
/// is not mistaken for the start of a comment. An unterminated comment runs to
/// end of input, as the spec requires.
fn strip_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                let start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                out.push_str(&source[start..i.min(bytes.len())]);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                out.push(' ');
            }
            b'u' | b'U' if source[i..].len() >= 4 && source[i..i + 4].eq_ignore_ascii_case("url(") => {
                let start = i;
                i += 4;
                // An unquoted url() ends at the first ')'; a quoted one is handled
                // by the string arm on the next pass, so stop at the quote.
                while i < bytes.len() && bytes[i] != b')' && bytes[i] != b'"' && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b')' {
                    i += 1;
                }
                out.push_str(&source[start..i]);
            }
            _ => {
                // Advance one whole character so multi-byte text survives intact.
                let ch = source[i..].chars().next().unwrap_or('\0');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// The four border edges in CSS quad order (top, right, bottom, left).
pub const BORDER_SIDES: [&str; 4] = ["top", "right", "bottom", "left"];

pub fn parse_css(source: &str) -> Stylesheet {
    let mut items = Vec::new();
    // Comments must go before anything else looks at the text: this parser splits
    // on `{`, `}` and `;`, and a comment is not a token to it, so an un-stripped
    // `/* ... */` ends up glued to the following selector and silently discards
    // the rule. Real stylesheets are full of comments, so that one omission drops
    // most of a hand-written sheet on the floor.
    let source = strip_comments(source);
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
                    for (k, v) in &temp_map {
                        declarations.push(Declaration { name: intern(k), value: v.clone(), important });
                    }
                    // Also emit the per-side longhands, so a later `border-bottom`
                    // can override one edge without discarding the other three.
                    for side in BORDER_SIDES {
                        for part in ["width", "style", "color"] {
                            if let Some(v) = temp_map.get(&format!("border-{part}")) {
                                declarations.push(Declaration {
                                    name: intern(&format!("border-{side}-{part}")),
                                    value: v.clone(),
                                    important,
                                });
                            }
                        }
                    }
                }
                // Single-edge shorthand: `border-bottom: 1px solid #ccc`. Hairline
                // rules on one edge are how most page furniture is drawn, so an
                // unexpanded value here loses a large share of a site's structure.
                "border-top" | "border-right" | "border-bottom" | "border-left" => {
                    let side = key.rsplit('-').next().unwrap_or("top").to_string();
                    let mut temp_map = HashMap::new();
                    parse_border_shorthand(&val_raw, &mut temp_map);
                    // A one-edge shorthand resets the parts it does not mention.
                    if !temp_map.contains_key("border-width") {
                        temp_map.insert("border-width".to_string(), Value::Length(3.0, Unit::Px));
                    }
                    for part in ["width", "style", "color"] {
                        if let Some(v) = temp_map.get(&format!("border-{part}")) {
                            declarations.push(Declaration {
                                name: intern(&format!("border-{side}-{part}")),
                                value: v.clone(),
                                important,
                            });
                        }
                    }
                }
                // Quad forms: `border-width: 1px 2px`, `border-color: red blue`.
                "border-width" | "border-color" | "border-style" => {
                    let part = key.rsplit('-').next().unwrap_or("width").to_string();
                    let values = split_respecting_parens(&val_raw);
                    let resolved: Vec<Value> = values.iter().map(|v| {
                        if part == "color" {
                            parse_color(v).map(Value::Color).unwrap_or_else(|| parse_value(v))
                        } else {
                            parse_value(v)
                        }
                    }).collect();
                    let quad: [&Value; 4] = match resolved.len() {
                        1 => [&resolved[0], &resolved[0], &resolved[0], &resolved[0]],
                        2 => [&resolved[0], &resolved[1], &resolved[0], &resolved[1]],
                        3 => [&resolved[0], &resolved[1], &resolved[2], &resolved[1]],
                        _ if resolved.len() >= 4 => [&resolved[0], &resolved[1], &resolved[2], &resolved[3]],
                        _ => {
                            declarations.push(Declaration { name: key, value: parse_value(&val_raw), important });
                            continue;
                        }
                    };
                    // Keep the uniform property for the existing consumers, then the
                    // per-side longhands.
                    declarations.push(Declaration { name: key.clone(), value: quad[0].clone(), important });
                    for (side, value) in BORDER_SIDES.iter().zip(quad) {
                        declarations.push(Declaration {
                            name: intern(&format!("border-{side}-{part}")),
                            value: value.clone(),
                            important,
                        });
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
                // CSS Grid track lists: store the raw value string as a Keyword so that
                // layout can later call parse_track_list() on it to expand repeat() etc.
                // parse_value() would incorrectly interpret "1fr 1fr" as a single Fr value.
                "grid-template-columns" | "grid-template-rows" => {
                    declarations.push(Declaration {
                        name: key,
                        value: Value::Keyword(intern(&val_raw)),
                        important,
                    });
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
/// Viewport the media queries are evaluated against.
///
/// Rendering happens at a fixed page width with no scrolling, so these stand in
/// for a real viewport.
const VIEWPORT_WIDTH_PX: f32 = 800.0;
const VIEWPORT_HEIGHT_PX: f32 = 768.0;

/// Evaluate a media query list — the text between `@media` and the opening `{`.
///
/// A list is comma-separated and matches if any of its queries does; each query
/// is an optional `not`, an optional media type, and `and`-joined conditions.
/// Both the legacy `(min-width: 768px)` form and the range form
/// `(width >= 768px)` are handled, since modern stylesheets mix them freely and
/// treating an unrecognised query as "no match" silently drops whole responsive
/// layers of a design.
fn evaluate_media_query(query_str: &str) -> bool {
    split_top_level_commas(&query_str.to_lowercase())
        .iter()
        .any(|q| evaluate_single_media_query(q))
}

fn split_top_level_commas(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

fn evaluate_single_media_query(query: &str) -> bool {
    let query = query.trim();
    let (negated, rest) = match query.strip_prefix("not ") {
        Some(rest) => (true, rest.trim()),
        None => (false, query),
    };

    let mut matches = true;
    let mut remainder = rest;

    // A leading media type, if any, comes before the first condition.
    if !remainder.starts_with('(') {
        let type_end = remainder.find('(').unwrap_or(remainder.len());
        let media_type = remainder[..type_end].trim_end().trim_end_matches("and").trim();
        if !media_type.is_empty() && !matches!(media_type, "all" | "screen") {
            return negated;
        }
        remainder = remainder[type_end..].trim();
    }

    for condition in split_conditions(remainder) {
        if !media_condition_matches(&condition) {
            matches = false;
            break;
        }
    }

    matches != negated
}

/// Split the `and`-joined conditions of one query. `and` inside parentheses (as
/// in `(width >= 100px) and (width <= 200px)`) is only a separator at depth 0.
fn split_conditions(text: &str) -> Vec<String> {
    let mut conditions = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            'a' if depth == 0 && text[i..].starts_with("and") => {
                conditions.push(std::mem::take(&mut current));
                for _ in 0..2 {
                    chars.next();
                }
            }
            _ => current.push(c),
        }
    }
    conditions.push(current);
    conditions.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
}

/// Parse a media-feature length such as `768px`, `48em` or `1.5dppx`.
fn media_length(value: &str) -> Option<f32> {
    let value = value.trim();
    for (suffix, scale) in [("px", 1.0), ("rem", 16.0), ("em", 16.0), ("dppx", 1.0), ("x", 1.0)] {
        if let Some(num) = value.strip_suffix(suffix) {
            return num.trim().parse::<f32>().ok().map(|n| n * scale);
        }
    }
    value.parse::<f32>().ok()
}

/// Evaluate one parenthesised media condition.
///
/// An unrecognised feature does not match. That is the conservative direction:
/// applying a rule guarded by a condition this engine cannot evaluate would
/// change the layout on a guess.
fn media_condition_matches(condition: &str) -> bool {
    let c = condition.trim().trim_start_matches('(').trim_end_matches(')').trim();

    // Range form: `width >= 768px`, `768px <= width <= 1011px`.
    if c.contains('<') || c.contains('>') {
        return media_range_matches(c);
    }

    let Some((feature, value)) = c.split_once(':') else {
        // A bare feature is true when the feature has a non-zero value.
        return matches!(c, "width" | "height" | "color" | "hover" | "pointer" | "any-hover" | "any-pointer");
    };
    let (feature, value) = (feature.trim(), value.trim());

    match feature {
        "min-width" | "min-device-width" => media_length(value).is_some_and(|v| VIEWPORT_WIDTH_PX >= v),
        "max-width" | "max-device-width" => media_length(value).is_some_and(|v| VIEWPORT_WIDTH_PX <= v),
        "width" | "device-width" => media_length(value).is_some_and(|v| VIEWPORT_WIDTH_PX == v),
        "min-height" | "min-device-height" => media_length(value).is_some_and(|v| VIEWPORT_HEIGHT_PX >= v),
        "max-height" | "max-device-height" => media_length(value).is_some_and(|v| VIEWPORT_HEIGHT_PX <= v),
        "height" | "device-height" => media_length(value).is_some_and(|v| VIEWPORT_HEIGHT_PX == v),
        // Defaults chosen to match a headless Chromium with no user overrides,
        // which is what the reference screenshots are taken with.
        "prefers-color-scheme" => value == "light",
        "prefers-reduced-motion" => value == "no-preference",
        "prefers-contrast" => value == "no-preference",
        "forced-colors" => value == "none",
        "hover" | "any-hover" => value == "hover",
        "pointer" | "any-pointer" => value == "fine",
        "orientation" => value == "landscape",
        "display-mode" => value == "browser",
        "min-resolution" => media_length(value).is_some_and(|v| v <= 1.0),
        "max-resolution" => media_length(value).is_some_and(|v| v >= 1.0),
        "resolution" => media_length(value).is_some_and(|v| v == 1.0),
        "scripting" => value == "enabled",
        _ => false,
    }
}

/// Evaluate the range syntax, in both its one-sided and two-sided forms.
fn media_range_matches(condition: &str) -> bool {
    let tokens: Vec<&str> = condition.split_whitespace().collect();
    let feature_value = |name: &str| -> Option<f32> {
        match name {
            "width" | "device-width" => Some(VIEWPORT_WIDTH_PX),
            "height" | "device-height" => Some(VIEWPORT_HEIGHT_PX),
            "resolution" => Some(1.0),
            _ => None,
        }
    };
    let compare = |lhs: f32, op: &str, rhs: f32| match op {
        "<" => lhs < rhs,
        "<=" => lhs <= rhs,
        ">" => lhs > rhs,
        ">=" => lhs >= rhs,
        "=" | "==" => lhs == rhs,
        _ => false,
    };

    match tokens.as_slice() {
        [lhs, op, rhs] => match (feature_value(lhs), media_length(rhs)) {
            (Some(v), Some(bound)) => compare(v, op, bound),
            _ => match (media_length(lhs), feature_value(rhs)) {
                (Some(bound), Some(v)) => compare(bound, op, v),
                _ => false,
            },
        },
        [low, op1, feature, op2, high] => {
            let Some(v) = feature_value(feature) else { return false };
            let (Some(low), Some(high)) = (media_length(low), media_length(high)) else {
                return false;
            };
            compare(low, op1, v) && compare(v, op2, high)
        }
        _ => false,
    }
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

/// A pseudo-class in a form the matcher can evaluate without re-parsing text.
///
/// Anything this engine does not implement becomes `Unsupported`, which never
/// matches. That is the safe direction: treating an unknown pseudo-class as
/// "matches" would widen a rule like `input::-webkit-outer-spin-button {
/// display: none }` to every input on the page.
#[derive(Debug, Clone)]
pub enum PseudoClass {
    Hover,
    Focus,
    Root,
    /// `:not(a, b)` — matches when none of the inner selectors match.
    Not(Vec<Selector>),
    /// `:is()` / `:where()` / legacy `:matches()` — matches when any inner does.
    Is(Vec<Selector>),
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    /// `:nth-child(an+b)`, stored as `(a, b)`.
    NthChild(i32, i32),
    AnyLink,
    Enabled,
    Disabled,
    Checked,
    Empty,
    Unsupported,
}

#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub class: Vec<String>,
    pub attributes: Vec<AttributeSelector>,
    /// Raw pseudo-class text, kept for specificity and debugging. The evaluated
    /// form lives in `pseudo_classes`.
    pub pseudo_class: Option<String>,
    /// Every pseudo-class on this compound selector, parsed. All must match.
    pub pseudo_classes: Vec<PseudoClass>,
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
        // `:not()` and `:is()` contribute their most specific argument instead of
        // themselves; every other pseudo-class counts as one class.
        for pc in &self.pseudo_classes {
            match pc {
                PseudoClass::Not(inner) | PseudoClass::Is(inner) => {
                    if let Some((ia, ib, ic)) = inner.iter().map(|s| s.specificity()).max() {
                        a += ia;
                        b += ib;
                        c += ic;
                    }
                }
                _ => b += 1,
            }
        }
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

/// Split a pseudo-class remainder such as `not(.a):first-child` on top-level
/// colons, so a nested `:not(:hover)` is not torn apart.
fn split_pseudo_classes(rest: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => {
                if i > start {
                    parts.push(&rest[start..i]);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < rest.len() {
        parts.push(&rest[start..]);
    }
    parts
}

/// Split a selector list on top-level commas (`:not(.a, .b)`).
fn split_selector_list(arg: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let bytes = arg.as_bytes();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(&arg[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&arg[start..]);
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

/// Parse the `an+b` micro-syntax used by `:nth-child()`.
fn parse_nth(arg: &str) -> Option<(i32, i32)> {
    let arg = arg.trim().to_lowercase();
    match arg.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Ok(b) = arg.parse::<i32>() {
        return Some((0, b));
    }
    let (a_part, b_part) = arg.split_once('n')?;
    let a = match a_part.trim() {
        "" | "+" => 1,
        "-" => -1,
        other => other.parse::<i32>().ok()?,
    };
    let b_part = b_part.trim();
    let b = if b_part.is_empty() { 0 } else { b_part.replace(' ', "").parse::<i32>().ok()? };
    Some((a, b))
}

/// Turn one pseudo-class token (`hover`, `not(.show)`, `nth-child(2n+1)`) into
/// its evaluated form.
fn parse_pseudo_class(token: &str) -> PseudoClass {
    let token = token.trim();
    let (name, arg) = match token.split_once('(') {
        Some((n, rest)) => (n.trim().to_lowercase(), rest.strip_suffix(')').unwrap_or(rest)),
        None => (token.to_lowercase(), ""),
    };
    let inner = || split_selector_list(arg).into_iter().map(parse_selector).collect::<Vec<_>>();
    match name.as_str() {
        "hover" => PseudoClass::Hover,
        "focus" => PseudoClass::Focus,
        "root" => PseudoClass::Root,
        "not" => PseudoClass::Not(inner()),
        "is" | "where" | "matches" | "any" | "-webkit-any" => PseudoClass::Is(inner()),
        "first-child" => PseudoClass::FirstChild,
        "last-child" => PseudoClass::LastChild,
        "only-child" => PseudoClass::OnlyChild,
        "first-of-type" => PseudoClass::FirstOfType,
        "last-of-type" => PseudoClass::LastOfType,
        "nth-child" => parse_nth(arg).map_or(PseudoClass::Unsupported, |(a, b)| PseudoClass::NthChild(a, b)),
        "link" | "any-link" => PseudoClass::AnyLink,
        "enabled" => PseudoClass::Enabled,
        "disabled" => PseudoClass::Disabled,
        "checked" => PseudoClass::Checked,
        "empty" => PseudoClass::Empty,
        _ => PseudoClass::Unsupported,
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

                let (pseudo_class, pseudo_element, pseudo_classes): (Option<String>, Option<String>, Vec<PseudoClass>) =
                    match pseudo_rest {
                        None => (None, None, Vec::new()),
                        Some(rest) => {
                            if let Some(pe) = rest.strip_prefix(':') {
                                // Double-colon pseudo-element: ::before / ::after
                                let name = pe.to_lowercase();
                                if name == "before" || name == "after" {
                                    (None, Some(name), Vec::new())
                                } else {
                                    // An unimplemented pseudo-element must not decay into
                                    // its bare subject: `input::-webkit-outer-spin-button
                                    // { display: none }` would then hide every input.
                                    (None, None, vec![PseudoClass::Unsupported])
                                }
                            } else {
                                // Single-colon pseudo-classes, possibly several in a row.
                                let parsed = split_pseudo_classes(rest)
                                    .into_iter()
                                    .map(parse_pseudo_class)
                                    .collect();
                                (Some(rest.to_string()), None, parsed)
                            }
                        }
                    };

                if base_part.is_empty() && pseudo_class.is_none() && pseudo_element.is_none() && pseudo_classes.is_empty() { continue; }

                let mut current_sel = Selector::new();
                current_sel.pseudo_class = pseudo_class;
                current_sel.pseudo_classes = pseudo_classes;
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

/// Parse a CSS math function (`calc()`, `clamp()`, `min()`, `max()`) into an
/// expression tree, or `None` if it is not one or cannot be understood.
pub fn parse_math_function(val: &str) -> Option<MathExpr> {
    let trimmed = val.trim();
    let lower = trimmed.to_ascii_lowercase();
    for name in ["calc(", "clamp(", "min(", "max("] {
        if !lower.starts_with(name) {
            continue;
        }
        let inner = trimmed.get(name.len()..)?.strip_suffix(')')?;
        return match name {
            "calc(" => parse_math_sum(inner),
            "clamp(" => {
                let args = split_math_args(inner);
                let [lo, val, hi] = args.as_slice() else { return None };
                Some(MathExpr::Clamp(
                    Box::new(parse_math_sum(lo)?),
                    Box::new(parse_math_sum(val)?),
                    Box::new(parse_math_sum(hi)?),
                ))
            }
            _ => {
                let args: Option<Vec<MathExpr>> =
                    split_math_args(inner).iter().map(|a| parse_math_sum(a)).collect();
                let args = args?;
                if args.is_empty() {
                    return None;
                }
                Some(if name == "min(" { MathExpr::Min(args) } else { MathExpr::Max(args) })
            }
        };
    }
    None
}

/// Split comma-separated arguments of a math function, ignoring commas nested
/// inside another function call.
fn split_math_args(inner: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, b) in inner.bytes().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                args.push(inner[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    args.push(inner[start..].trim());
    args.into_iter().filter(|a| !a.is_empty()).collect()
}

/// `<sum> := <product> (('+' | '-') <product>)*`
///
/// Scanned right to left so the left-associative tree comes out with the
/// rightmost operator at the root. `+` and `-` require surrounding whitespace in
/// CSS, which is what keeps them apart from a sign on a number.
fn parse_math_sum(expr: &str) -> Option<MathExpr> {
    let expr = expr.trim();
    if let Some((lhs, op, rhs)) = split_top_level_operator(expr, &[b'+', b'-'], true) {
        let (l, r) = (parse_math_sum(lhs)?, parse_math_product(rhs)?);
        return Some(if op == b'+' {
            MathExpr::Add(Box::new(l), Box::new(r))
        } else {
            MathExpr::Sub(Box::new(l), Box::new(r))
        });
    }
    parse_math_product(expr)
}

/// `<product> := <term> (('*' | '/') <term>)*`
fn parse_math_product(expr: &str) -> Option<MathExpr> {
    let expr = expr.trim();
    if let Some((lhs, op, rhs)) = split_top_level_operator(expr, &[b'*', b'/'], false) {
        let (l, r) = (parse_math_product(lhs)?, parse_math_term(rhs)?);
        return Some(if op == b'*' {
            MathExpr::Mul(Box::new(l), Box::new(r))
        } else {
            MathExpr::Div(Box::new(l), Box::new(r))
        });
    }
    parse_math_term(expr)
}

/// Find the last top-level occurrence of one of `ops` and split there.
///
/// `require_space` marks the additive operators, which CSS requires to be
/// surrounded by whitespace precisely so `10px -5px` cannot be read as a
/// subtraction and `calc(1px + -2px)` stays unambiguous.
fn split_top_level_operator<'a>(
    expr: &'a str,
    ops: &[u8],
    require_space: bool,
) -> Option<(&'a str, u8, &'a str)> {
    let bytes = expr.as_bytes();
    let mut depth = 0usize;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => depth = depth.saturating_sub(1),
            b if depth == 0 && ops.contains(&b) => {
                if require_space {
                    let spaced = i > 0
                        && bytes[i - 1].is_ascii_whitespace()
                        && bytes.get(i + 1).is_some_and(|c| c.is_ascii_whitespace());
                    if !spaced {
                        continue;
                    }
                } else if i == 0 {
                    continue;
                }
                return Some((&expr[..i], b, &expr[i + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// `<term> := <number><unit> | '(' <sum> ')' | <math-function>`
fn parse_math_term(expr: &str) -> Option<MathExpr> {
    let expr = expr.trim();
    if let Some(inner) = expr.strip_prefix('(').and_then(|e| e.strip_suffix(')')) {
        return parse_math_sum(inner);
    }
    if let Some(nested) = parse_math_function(expr) {
        return Some(nested);
    }
    let lower = expr.to_ascii_lowercase();
    for (suffix, unit) in [
        ("px", Some(Unit::Px)),
        ("rem", Some(Unit::Em)),
        ("em", Some(Unit::Em)),
        ("vw", Some(Unit::Vw)),
        ("vh", Some(Unit::Vh)),
        ("vmin", Some(Unit::Vw)),
        ("vmax", Some(Unit::Vw)),
        ("%", Some(Unit::Percent)),
    ] {
        if let Some(num) = lower.strip_suffix(suffix) {
            return num.trim().parse::<f32>().ok().map(|n| MathExpr::Value(n, unit));
        }
    }
    lower.parse::<f32>().ok().map(|n| MathExpr::Value(n, None))
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

    // calc() / clamp() / min() / max(): kept as an expression until the viewport
    // and container sizes needed to evaluate them are known.
    if let Some(expr) = parse_math_function(val) {
        return Value::Math(expr);
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
    } else if val.ends_with("fr") {
        Value::Length(val.trim_end_matches("fr").parse().unwrap_or(1.0), Unit::Fr)
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
    fn test_media_range_syntax_and_compound_queries() {
        // Modern stylesheets mix the range form with the legacy one, and guard
        // whole responsive layers behind multi-condition queries.
        let applies = |q: &str| !parse_css(&format!("{q} {{ p {{ color: green; }} }}")).all_rules().is_empty();

        assert!(applies("@media (width >= 768px)"), "800px satisfies width >= 768px");
        assert!(!applies("@media (width <= 767px)"), "800px does not satisfy width <= 767px");
        assert!(applies("@media (768px <= width <= 1011px)"), "800px is inside the range");
        assert!(!applies("@media (1012px <= width <= 1279px)"), "800px is outside the range");
        assert!(
            applies("@media screen and (min-width: 600px) and (max-width: 900px)"),
            "both conditions hold at 800px"
        );
        assert!(
            !applies("@media (min-width: 600px) and (min-height: 2000px)"),
            "a compound query fails when any condition fails"
        );
        assert!(applies("@media (max-width: 100px), (min-width: 700px)"), "a list matches if any query does");
        assert!(!applies("@media (unknown-feature: 3)"), "an unevaluable feature must not apply");
    }

    #[test]
    fn test_media_screen_and_min_width_applies_at_800px() {
        // @media screen and (min-width: 600px) must apply when viewport >= 600px.
        let ss = parse_css("@media screen and (min-width: 600px) { p { color: green; } }");
        let rules = ss.all_rules();
        assert!(!rules.is_empty(), "@media screen and (min-width: 600px) should apply at 800px");
    }

    #[test]
    fn test_media_prefers_color_scheme_light_activates() {
        // The renderer reports no colour-scheme preference, which resolves to
        // light — the same default a browser with untouched settings reports, and
        // the one the Chromium reference screenshots are taken under.
        let ss = parse_css("@media (prefers-color-scheme: light) { body { background: white; } }");
        let rules = ss.all_rules();
        assert!(
            !rules.is_empty(),
            "@media (prefers-color-scheme: light) should be included"
        );
        assert!(rules.iter().any(|r| r.declarations.iter().any(|d| {
            d.name.as_ref() == "background"
        })));
    }

    #[test]
    fn test_media_prefers_color_scheme_dark_does_not_activate() {
        // A site's dark-mode layer must stay off, or it paints dark surfaces
        // under text coloured for a light one.
        let ss = parse_css("@media (prefers-color-scheme: dark) { body { background: black; } }");
        let rules = ss.all_rules();
        assert!(
            rules.is_empty(),
            "@media (prefers-color-scheme: dark) should be excluded, got {} rules",
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

    for token in split_track_tokens(&expanded) {
        tracks.push(parse_track_token(token));
    }
    tracks
}

/// Split a track list into tokens, keeping bracketed functions whole.
///
/// Splitting on plain whitespace tears `minmax(0, 1fr)` into `minmax(0,` and
/// `1fr)`, which turns a two-column template into three junk tracks and
/// collapses the grid. Line names in `[...]` are dropped, as this engine has no
/// use for them.
fn split_track_tokens(list: &str) -> Vec<&str> {
    let bytes = list.as_bytes();
    let mut tokens = Vec::new();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    let mut in_line_names = false;

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' if depth == 0 => {
                if let Some(s) = start.take() {
                    tokens.push(&list[s..i]);
                }
                in_line_names = true;
            }
            b']' if in_line_names => in_line_names = false,
            _ if in_line_names => {}
            b'(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            b')' => {
                depth = depth.saturating_sub(1);
            }
            b if b.is_ascii_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    tokens.push(&list[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        tokens.push(&list[s..]);
    }
    tokens.into_iter().map(str::trim).filter(|t| !t.is_empty()).collect()
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
    // `minmax(min, max)` is sized by its growth limit here: the common forms on
    // real pages are `minmax(0, 1fr)` and `minmax(auto, 1fr)`, where the max is
    // what decides the track and the min only guards against overflow.
    if let Some(args) = t.strip_prefix("minmax(").and_then(|r| r.strip_suffix(')')) {
        if let Some((_, max)) = args.rsplit_once(',') {
            return parse_track_token(max);
        }
    }
    // `fit-content(x)` sizes to content up to a cap; `auto` is the closest track
    // this engine models.
    if t.starts_with("fit-content(") {
        return Value::Keyword(intern("auto"));
    }
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
