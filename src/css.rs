use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Keyword(String),
    Length(f32, Unit),
    Color(Color),
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub class: Vec<String>,
}

impl Selector {
    pub fn new() -> Self {
        Selector {
            tag: None,
            id: None,
            class: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

pub fn parse_css(source: &str) -> Stylesheet {
    let mut rules = Vec::new();
    let source = source.replace('\n', " ");
    
    let blocks: Vec<&str> = source.split('}').collect();
    for block in blocks {
        if block.trim().is_empty() { continue; }
        
        let mut parts = block.split('{');
        let selectors_str = parts.next().unwrap_or("").trim();
        let declarations_str = parts.next().unwrap_or("").trim();
        
        let mut selectors = Vec::new();
        for s in selectors_str.split(',') {
            let s = s.trim();
            if !s.is_empty() {
                let mut selector = Selector::new();
                selector.tag = Some(s.to_string());
                selectors.push(selector);
            }
        }
        
        let mut declarations = HashMap::new();
        for decl in declarations_str.split(';') {
            let decl = decl.trim();
            if decl.is_empty() { continue; }
            let mut kv = decl.split(':');
            let key = kv.next().unwrap_or("").trim().to_string();
            let val_str = kv.next().unwrap_or("").trim().to_string();
            if !key.is_empty() && !val_str.is_empty() {
                declarations.insert(key, parse_value(&val_str));
            }
        }
        
        rules.push(Rule { selectors, declarations });
    }
    
    Stylesheet { rules }
}

fn parse_value(val: &str) -> Value {
    let val = val.trim();
    if val.ends_with("px") {
        Value::Length(val.trim_end_matches("px").parse().unwrap_or(0.0), Unit::Px)
    } else if val.ends_with("vw") {
        Value::Length(val.trim_end_matches("vw").parse().unwrap_or(0.0), Unit::Vw)
    } else if let Some(color) = parse_color(val) {
        Value::Color(color)
    } else {
        Value::Keyword(val.to_string())
    }
}

fn parse_color(s: &str) -> Option<Color> {
    let s = s.to_lowercase();
    if s.starts_with('#') {
        let hex = &s[1..];
        if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            return Some(Color { r, g, b, a: 255 });
        } else if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color { r, g, b, a: 255 });
        }
    } else if s.starts_with("rgb") {
        let parts: Vec<&str> = s.split(|c| c == '(' || c == ')' || c == ',')
            .filter(|p| !p.trim().is_empty() && !p.contains("rgb"))
            .collect();
        if parts.len() >= 3 {
            let r = parts[0].trim().parse().ok()?;
            let g = parts[1].trim().parse().ok()?;
            let b = parts[2].trim().parse().ok()?;
            let a = if parts.len() == 4 {
                (parts[3].trim().parse::<f32>().ok()? * 255.0) as u8
            } else {
                255
            };
            return Some(Color { r, g, b, a });
        }
    } else {
        match s.as_str() {
            "white" => return Some(Color { r: 255, g: 255, b: 255, a: 255 }),
            "black" => return Some(Color { r: 0, g: 0, b: 0, a: 255 }),
            "red" => return Some(Color { r: 255, g: 0, b: 0, a: 255 }),
            "blue" => return Some(Color { r: 0, g: 0, b: 255, a: 255 }),
            "green" => return Some(Color { r: 0, g: 128, b: 0, a: 255 }),
            "gray" | "grey" => return Some(Color { r: 128, g: 128, b: 128, a: 255 }),
            "silver" => return Some(Color { r: 192, g: 192, b: 192, a: 255 }),
            _ => {}
        }
    }
    None
}
