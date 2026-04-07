use crate::style::StyledNode;
use crate::css::{Value, Unit};

#[derive(Default, Debug, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct LayoutBox<'a> {
    pub dimensions: Rect,
    pub style_node: &'a StyledNode,
    pub children: Vec<LayoutBox<'a>>,
    pub link_url: Option<String>,
    pub padding: f32,
    pub margin: f32,
}

pub fn build_layout_tree<'a>(style_node: &'a StyledNode, mut current_y: f32, parent_width: f32) -> (Option<LayoutBox<'a>>, f32) {
    // 1. Handle display: none
    if let Some(Value::Keyword(d)) = style_node.specified_values.get("display") {
        if d == "none" {
            return (None, current_y);
        }
    }

    // 2. Extract spacing
    let padding = match style_node.specified_values.get("padding") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    };
    let margin = match style_node.specified_values.get("margin") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    };

    let mut link_url = None;
    if let markup5ever_rcdom::NodeData::Element { ref attrs, ref name, .. } = style_node.node.data {
        if name.local.to_string() == "a" {
            for attr in attrs.borrow().iter() {
                if attr.name.local.to_string() == "href" {
                    link_url = Some(attr.value.to_string());
                }
            }
        }
    }

    let mut layout = LayoutBox {
        dimensions: Rect {
            x: margin,
            y: current_y + margin,
            width: parent_width - (margin * 2.0),
            height: 0.0,
        },
        style_node,
        children: Vec::new(),
        link_url,
        padding,
        margin,
    };

    // Override width if set in CSS
    if let Some(Value::Length(w, Unit::Px)) = style_node.specified_values.get("width") {
        layout.dimensions.width = *w;
    } else if let Some(Value::Length(w, Unit::Vw)) = style_node.specified_values.get("width") {
        layout.dimensions.width = parent_width * (*w / 100.0);
    }

    let start_y = current_y + margin;
    current_y += margin + padding;

    for child in &style_node.children {
        // Skip some elements like head, title, style, meta from layout
        if let markup5ever_rcdom::NodeData::Element { ref name, .. } = child.node.data {
            let t = name.local.to_string();
            if t == "head" || t == "style" || t == "meta" || t == "title" {
                continue;
            }
        }

        let (child_layout, new_y) = build_layout_tree(child, current_y, layout.dimensions.width - (padding * 2.0));
        if let Some(mut child_box) = child_layout {
            child_box.dimensions.x += padding; // Offset by parent padding
            layout.children.push(child_box);
            current_y = new_y;
        }
    }

    current_y += padding + margin;
    layout.dimensions.height = current_y - start_y - margin;

    (Some(layout), current_y)
}

impl<'a> LayoutBox<'a> {
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&'a StyledNode> {
        if x >= self.dimensions.x && x <= self.dimensions.x + self.dimensions.width &&
           y >= self.dimensions.y && y <= self.dimensions.y + self.dimensions.height {
            
            for child in self.children.iter().rev() {
                if let Some(node) = child.hit_test(x, y) {
                    return Some(node);
                }
            }
            
            return Some(self.style_node);
        }
        None
    }

    pub fn get_links(&self) -> Vec<(Rect, String)> {
        let mut links = Vec::new();
        if let Some(ref url) = self.link_url {
            links.push((self.dimensions, url.clone()));
        }
        for child in &self.children {
            links.extend(child.get_links());
        }
        links
    }
}

pub fn print_layout_tree(layout: &LayoutBox, indent: usize) {
    let indent_str = " ".repeat(indent * 2);

    let tag = if let markup5ever_rcdom::NodeData::Element { ref name, .. } = layout.style_node.node.data {
        name.local.to_string()
    } else if let markup5ever_rcdom::NodeData::Text { ref contents } = layout.style_node.node.data {
        let text = contents.borrow().to_string();
        format!("Text({:?})", text.trim())
    } else {
        "Node".to_string()
    };

    if !tag.contains("Text(\"\")") {
        println!("{}{} [x: {}, y: {}, w: {}, h: {}]", 
            indent_str, tag, 
            layout.dimensions.x, layout.dimensions.y, 
            layout.dimensions.width, layout.dimensions.height);
    }

    for child in &layout.children {
        print_layout_tree(child, indent + 1);
    }
}
