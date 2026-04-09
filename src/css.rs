use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Keyword(String),
    Length(f32, Unit),
    Color(Color),
    BoxShadow(BoxShadow),
    Number(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    pub inset: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Unit {
    Px,
    Vw,
    Vh,
    Em,
    Percent,
}

#[derive(Debug, Clone)]
pub struct Declaration {
    pub name: String,
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
            let key = kv.next().unwrap_or("").trim().to_lowercase();
            let mut val_raw = kv.next().unwrap_or("").trim().to_string();
            if key.is_empty() || val_raw.is_empty() { continue; }

            let important = val_raw.ends_with("!important");
            if important {
                val_raw = val_raw.trim_end_matches("!important").trim().to_string();
            }

            match key.as_str() {
                "border" => {
                    let mut temp_map = HashMap::new();
                    parse_border_shorthand(&val_raw, &mut temp_map);
                    for (k, v) in temp_map {
                        declarations.push(Declaration { name: k, value: v, important });
                    }
                }
                "padding" => {
                    let mut temp_map = HashMap::new();
                    parse_quad_shorthand("padding", &val_raw, &mut temp_map);
                    for (k, v) in temp_map {
                        declarations.push(Declaration { name: k, value: v, important });
                    }
                }
                "margin" => {
                    let mut temp_map = HashMap::new();
                    parse_quad_shorthand("margin", &val_raw, &mut temp_map);
                    for (k, v) in temp_map {
                        declarations.push(Declaration { name: k, value: v, important });
                    }
                }
                "box-shadow" => {
                    if let Some(shadow) = parse_box_shadow(&val_raw) {
                        declarations.push(Declaration { name: key, value: Value::BoxShadow(shadow), important });
                    }
                }
                _ => {
                    declarations.push(Declaration { name: key, value: parse_value(&val_raw), important });
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
            declarations.insert("border-style".to_string(), Value::Keyword(part.to_string()));
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
        offset_x: values.get(0).copied().unwrap_or(0.0),
        offset_y: values.get(1).copied().unwrap_or(0.0),
        blur: values.get(2).copied().unwrap_or(0.0),
        spread: values.get(3).copied().unwrap_or(0.0),
        color,
        inset,
    })
}

pub fn parse_value(val: &str) -> Value {
    let val = val.trim();
    // Strip !important
    let val = val.trim_end_matches("!important").trim();

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
        Value::Keyword(val.to_string())
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
        assert_eq!(s.offset_x, 0.0);
        assert_eq!(s.offset_y, 2.0);
        assert_eq!(s.blur, 8.0);
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
}
