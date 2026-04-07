use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Keyword(String),
    Length(f32, Unit),
    Color(String),
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
    // A VERY simplified CSS parser for demonstration purposes.
    // In a real browser, we would use the `cssparser` crate.
    let mut rules = Vec::new();
    let source = source.replace('\n', "");
    
    // Naive split by '}'
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
                // Extremely naive selector parsing (only handles tags like 'body', 'h1', 'div', etc)
                // In reality, this needs to handle classes, ids, pseudo-classes...
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
                // Naive value parsing
                let val = if val_str.ends_with("px") {
                    Value::Length(val_str.trim_end_matches("px").parse().unwrap_or(0.0), Unit::Px)
                } else if val_str.ends_with("vw") {
                    Value::Length(val_str.trim_end_matches("vw").parse().unwrap_or(0.0), Unit::Vw)
                } else if val_str.starts_with('#') || val_str == "red" || val_str == "blue" {
                    Value::Color(val_str.clone())
                } else {
                    Value::Keyword(val_str.clone())
                };
                declarations.insert(key, val);
            }
        }
        
        rules.push(Rule { selectors, declarations });
    }
    
    Stylesheet { rules }
}
