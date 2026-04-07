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

    pub fn specificity(&self) -> (usize, usize, usize) {
        // (ID, Class, Tag)
        let id_count = if self.id.is_some() { 1 } else { 0 };
        let class_count = self.class.len();
        let tag_count = if self.tag.is_some() { 1 } else { 0 };
        (id_count, class_count, tag_count)
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
                selectors.push(parse_selector(s));
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

fn parse_selector(s: &str) -> Selector {
    let mut selector = Selector::new();
    let mut current = String::new();
    let mut last_char = ' ';

    for c in s.chars().chain(std::iter::once(' ')) {
        if c == '#' || c == '.' || c == ' ' {
            if !current.is_empty() {
                match last_char {
                    '#' => selector.id = Some(current.clone()),
                    '.' => selector.class.push(current.clone()),
                    _ => selector.tag = Some(current.clone()),
                }
                current.clear();
            }
            last_char = c;
        } else {
            current.push(c);
        }
    }
    selector
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
    fn test_parse_selector() {
        let s = parse_selector("div#main.header.active");
        assert_eq!(s.tag, Some("div".to_string()));
        assert_eq!(s.id, Some("main".to_string()));
        assert_eq!(s.class, vec!["header".to_string(), "active".to_string()]);
    }
}
