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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub enum TransformOp {
    Translate(OrderedFloat, OrderedFloat), // px
    Scale(OrderedFloat, OrderedFloat),     // factor
    Rotate(OrderedFloat),                  // radians
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
                                }
                                "auto"    => {
                                    declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(1.0), important });
                                    declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(1.0), important });
                                }
                                _ => {
                                    // single unitless number → flex-grow
                                    if let Ok(n) = parts[0].parse::<f32>() {
                                        declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(n),   important });
                                        declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(1.0), important });
                                    }
                                }
                            }
                        }
                        2 => {
                            if let (Ok(g), Ok(s)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>()) {
                                declarations.push(Declaration { name: intern("flex-grow"),   value: Value::Number(g), important });
                                declarations.push(Declaration { name: intern("flex-shrink"), value: Value::Number(s), important });
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

fn strip_at_rules(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut depth = 0usize;
    let mut in_at_rule = false;
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '@' && depth == 0 {
            in_at_rule = true;
        }
        if in_at_rule {
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                if depth > 0 {
                    depth -= 1;
                }
                if depth == 0 {
                    in_at_rule = false;
                }
            } else if c == ';' && depth == 0 {
                in_at_rule = false;
            }
        } else {
            result.push(c);
        }
        i += 1;
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
                let mut p_parts = part.splitn(2, ':');
                let base_part = p_parts.next().unwrap_or(part);
                let pseudo = p_parts.next().map(|s| s.to_string());
                
                if base_part.is_empty() && pseudo.is_none() { continue; }

                let mut current_sel = Selector::new();
                current_sel.pseudo_class = pseudo;
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

    #[test]
    fn test_parse_percent() {
        let v = parse_value("50%");
        assert_eq!(v, Value::Length(50.0, Unit::Percent));
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
                    let x = parse_px_or_zero(args.get(0).copied().unwrap_or("0"));
                    let y = parse_px_or_zero(args.get(1).copied().unwrap_or("0"));
                    ops.push(TransformOp::Translate(OrderedFloat(x), OrderedFloat(y)));
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

fn parse_px_or_zero(s: &str) -> f32 {
    s.trim_end_matches("px").parse::<f32>().unwrap_or(0.0)
}
