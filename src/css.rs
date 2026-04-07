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
                // Special handling for border shorthand
                if key == "border" {
                    parse_border_shorthand(&val_str, &mut declarations);
                } else {
                    declarations.insert(key, parse_value(&val_str));
                }
            }
        }
        
        rules.push(Rule { selectors, declarations });
    }
    
    Stylesheet { rules }
}

fn parse_border_shorthand(val: &str, declarations: &mut HashMap<String, Value>) {
    let parts: Vec<&str> = val.split_whitespace().collect();
    for part in parts {
        if part.ends_with("px") || part.chars().all(|c| c.is_numeric()) {
            declarations.insert("border-width".to_string(), parse_value(part));
        } else if let Some(color) = parse_color(part) {
            declarations.insert("border-color".to_string(), Value::Color(color));
        } else if part == "solid" || part == "dashed" || part == "dotted" {
            declarations.insert("border-style".to_string(), Value::Keyword(part.to_string()));
        }
    }
}

pub fn parse_value(val: &str) -> Value {
    let val = val.trim();
    if val.ends_with("px") {
        Value::Length(val.trim_end_matches("px").parse().unwrap_or(0.0), Unit::Px)
    } else if val.ends_with("vw") {
        Value::Length(val.trim_end_matches("vw").parse().unwrap_or(0.0), Unit::Vw)
    } else if val.ends_with("em") {
        Value::Length(val.trim_end_matches("em").parse().unwrap_or(0.0), Unit::Em)
    } else if let Some(color) = parse_color(val) {
        Value::Color(color)
    } else {
        Value::Keyword(val.to_string())
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_lowercase();
    
    // 1. HEX Color (#fff, #ffffff)
    if s.starts_with('#') {
        let hex = &s[1..];
        match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                return Some(Color { r, g, b, a: 255 });
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                return Some(Color { r, g, b, a: 255 });
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                return Some(Color { r, g, b, a });
            }
            _ => return None,
        }
    }
    
    // 2. RGB/RGBA Color (rgb(255, 0, 0), rgba(255, 0, 0, 0.5))
    if s.starts_with("rgb") {
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
    
    // 3. Named Colors
    let mut names = HashMap::new();
    names.insert("white", Color { r: 255, g: 255, b: 255, a: 255 });
    names.insert("black", Color { r: 0, g: 0, b: 0, a: 255 });
    names.insert("red", Color { r: 255, g: 0, b: 0, a: 255 });
    names.insert("green", Color { r: 0, g: 128, b: 0, a: 255 });
    names.insert("blue", Color { r: 0, g: 0, b: 255, a: 255 });
    names.insert("yellow", Color { r: 255, g: 255, b: 0, a: 255 });
    names.insert("cyan", Color { r: 0, g: 255, b: 255, a: 255 });
    names.insert("magenta", Color { r: 255, g: 0, b: 255, a: 255 });
    names.insert("silver", Color { r: 192, g: 192, b: 192, a: 255 });
    names.insert("gray", Color { r: 128, g: 128, b: 128, a: 255 });
    names.insert("grey", Color { r: 128, g: 128, b: 128, a: 255 });
    names.insert("orange", Color { r: 255, g: 165, b: 0, a: 255 });
    names.insert("purple", Color { r: 128, g: 0, b: 128, a: 255 });
    names.insert("pink", Color { r: 255, g: 192, b: 203, a: 255 });
    names.insert("gold", Color { r: 255, g: 215, b: 0, a: 255 });
    names.insert("transparent", Color { r: 0, g: 0, b: 0, a: 0 });

    if let Some(color) = names.get(s.as_str()) {
        return Some(color.clone());
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_color_hex() {
        assert_eq!(parse_color("#ff0000"), Some(Color { r: 255, g: 0, b: 0, a: 255 }));
        assert_eq!(parse_color("#00f"), Some(Color { r: 0, g: 0, b: 255, a: 255 }));
        assert_eq!(parse_color("#ff000080"), Some(Color { r: 255, g: 0, b: 0, a: 128 }));
    }

    #[test]
    fn test_parse_color_rgb() {
        assert_eq!(parse_color("rgb(255, 0, 0)"), Some(Color { r: 255, g: 0, b: 0, a: 255 }));
        assert_eq!(parse_color("rgba(0, 255, 0, 0.5)"), Some(Color { r: 0, g: 255, b: 0, a: 127 }));
        assert_eq!(parse_color("rgb(  0,  0,  0  )"), Some(Color { r: 0, g: 0, b: 0, a: 255 }));
    }

    #[test]
    fn test_parse_color_names() {
        assert_eq!(parse_color("White"), Some(Color { r: 255, g: 255, b: 255, a: 255 }));
        assert_eq!(parse_color("transparent"), Some(Color { r: 0, g: 0, b: 0, a: 0 }));
    }

    #[test]
    fn test_parse_value() {
        assert_eq!(parse_value("10px"), Value::Length(10.0, Unit::Px));
        assert_eq!(parse_value("50vw"), Value::Length(50.0, Unit::Vw));
        assert_eq!(parse_value("bold"), Value::Keyword("bold".to_string()));
    }
}
