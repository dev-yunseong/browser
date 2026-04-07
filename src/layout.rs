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
}

pub fn build_layout_tree<'a>(style_node: &'a StyledNode, mut current_y: f32, parent_width: f32) -> (LayoutBox<'a>, f32) {
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
            x: 0.0,
            y: current_y,
            width: parent_width,
            height: 0.0, // Calculated later
        },
        style_node,
        children: Vec::new(),
        link_url,
    };

    // Override width if set in CSS
    if let Some(Value::Length(w, Unit::Px)) = style_node.specified_values.get("width") {
        layout.dimensions.width = *w;
    } else if let Some(Value::Length(w, Unit::Vw)) = style_node.specified_values.get("width") {
        layout.dimensions.width = parent_width * (*w / 100.0);
    }

    let start_y = current_y;
    current_y += 20.0; // padding/margin top (naive)

    for child in &style_node.children {
        // Skip some elements like head, title, style, meta from layout
        if let markup5ever_rcdom::NodeData::Element { ref name, .. } = child.node.data {
            let t = name.local.to_string();
            if t == "head" || t == "style" || t == "meta" || t == "title" {
                continue;
            }
        }

        let (child_layout, new_y) = build_layout_tree(child, current_y, layout.dimensions.width);
        layout.children.push(child_layout);
        current_y = new_y;
    }

    layout.dimensions.height = current_y - start_y;

    (layout, current_y)
}

impl<'a> LayoutBox<'a> {
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&'a StyledNode> {
        // ... (existing code)
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

    if !tag.contains("Text(\"\")") { // don't print empty text nodes
        println!("{}{} [x: {}, y: {}, w: {}, h: {}]", 
            indent_str, tag, 
            layout.dimensions.x, layout.dimensions.y, 
            layout.dimensions.width, layout.dimensions.height);
    }

    for child in &layout.children {
        print_layout_tree(child, indent + 1);
    }
}
