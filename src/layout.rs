use crate::style::StyledNode;
use crate::css::{Value, Unit};

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub struct LayoutBox<'a> {
    pub dimensions: Rect,
    pub style_node: &'a StyledNode,
    pub children: Vec<LayoutBox<'a>>,
    pub link_url: Option<String>,
    pub display: DisplayType,
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum DisplayType {
    Block,
    Inline,
    InlineBlock,
    ListItem,
    Table,
    TableRow,
    TableCell,
    Input,
}

fn get_display_type(style_node: &StyledNode) -> DisplayType {
    if let markup5ever_rcdom::NodeData::Text { .. } = style_node.node.data {
        return DisplayType::Inline;
    }

    if let Some(Value::Keyword(ref d)) = style_node.specified_values.get("display") {
        match d.as_str() {
            "block" => return DisplayType::Block,
            "inline" => return DisplayType::Inline,
            "inline-block" => return DisplayType::InlineBlock,
            "list-item" => return DisplayType::ListItem,
            "table" => return DisplayType::Table,
            "table-row" => return DisplayType::TableRow,
            "table-cell" => return DisplayType::TableCell,
            _ => {}
        }
    }

    if let markup5ever_rcdom::NodeData::Element { ref name, .. } = style_node.node.data {
        let tag = name.local.to_string();
        match tag.as_str() {
            "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | 
            "ul" | "header" | "footer" | "nav" | "section" | "article" | "body" | "html" => {
                DisplayType::Block
            }
            "li" => DisplayType::ListItem,
            "table" => DisplayType::Table,
            "tr" => DisplayType::TableRow,
            "td" | "th" => DisplayType::TableCell,
            "input" | "textarea" => DisplayType::Input,
            _ => DisplayType::Inline,
        }
    } else {
        DisplayType::Block
    }
}

pub fn build_layout_tree<'a>(
    style_node: &'a StyledNode,
    container_start_x: f32,
    mut current_x: f32,
    mut current_y: f32,
    container_width: f32
) -> (Option<LayoutBox<'a>>, f32, f32) {
    if let Some(Value::Keyword(d)) = style_node.specified_values.get("display") {
        if d == "none" {
            return (None, current_x, current_y);
        }
    }

    let display = get_display_type(style_node);

    let padding = match style_node.specified_values.get("padding") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    };
    let margin = match style_node.specified_values.get("margin") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    };

    // 3. 텍스트 노드 특수 처리
    if let markup5ever_rcdom::NodeData::Text { ref contents } = style_node.node.data {
        let text = contents.borrow().to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return (None, current_x, current_y);
        }

        let font_size = match style_node.specified_values.get("font-size") {
            Some(Value::Length(v, Unit::Px)) => *v,
            _ => 16.0,
        };

        let avg_char_width = 8.0 * (font_size / 16.0);
        let line_height = font_size * 1.25;
        
        let available_width = container_width - (current_x - container_start_x) - (margin * 2.0);
        if available_width < avg_char_width && current_x > container_start_x {
            current_y += line_height;
            current_x = container_start_x;
        }

        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let mut lines_count = 0;
        let mut current_line_len = 0;
        let mut estimated_width: f32 = 0.0;
        let max_chars_per_line = ((container_width - (padding * 2.0)) / avg_char_width).max(1.0) as usize;

        for word in words {
            if current_line_len + word.len() + 1 > max_chars_per_line && current_line_len > 0 {
                lines_count += 1;
                estimated_width = estimated_width.max(current_line_len as f32 * avg_char_width);
                current_line_len = word.len();
            } else {
                if current_line_len > 0 { current_line_len += 1; }
                current_line_len += word.len();
            }
        }
        if current_line_len > 0 { 
            lines_count += 1;
            estimated_width = estimated_width.max(current_line_len as f32 * avg_char_width);
        }

        let total_height = (lines_count as f32 * line_height) + (padding * 2.0);
        let layout_width = if lines_count > 1 { container_width - (margin * 2.0) } else { estimated_width };

        let layout = LayoutBox {
            dimensions: Rect {
                x: current_x + margin,
                y: current_y + margin,
                width: layout_width.min(container_width - (margin * 2.0)),
                height: total_height,
            },
            style_node,
            children: Vec::new(),
            link_url: None,
            display,
        };

        let next_x;
        let next_y;
        if lines_count > 1 {
            next_x = container_start_x;
            next_y = current_y + total_height + margin;
        } else {
            next_x = current_x + layout.dimensions.width + (margin * 2.0);
            next_y = current_y;
        }

        return (Some(layout), next_x, next_y);
    }

    // 4. 일반 박스 처리
    let mut layout_width_spec = match style_node.specified_values.get("width") {
        Some(Value::Length(w, Unit::Px)) => *w,
        Some(Value::Length(w, Unit::Vw)) => container_width * (*w / 100.0),
        _ => if let DisplayType::Block | DisplayType::ListItem | DisplayType::Table | DisplayType::TableRow = display { 
            container_width - (margin * 2.0) 
        } else if let DisplayType::Input = display {
            200.0 // Default input width
        } else { 
            0.0 
        },
    };

    let force_new_line = match display {
        DisplayType::Block | DisplayType::ListItem | DisplayType::Table | DisplayType::TableRow => true,
        _ => false,
    };

    if force_new_line {
        if current_x > container_start_x {
            current_y += 25.0; 
            current_x = container_start_x;
        }
    }

    let x_offset = if let DisplayType::ListItem = display { 20.0 } else { 0.0 };

    let mut layout = LayoutBox {
        dimensions: Rect {
            x: current_x + margin + x_offset,
            y: current_y + margin,
            width: layout_width_spec - x_offset,
            height: 0.0,
        },
        style_node,
        children: Vec::new(),
        link_url: None,
        display,
    };

    if let markup5ever_rcdom::NodeData::Element { ref attrs, ref name, .. } = style_node.node.data {
        if name.local.to_string() == "a" {
            for attr in attrs.borrow().iter() {
                if attr.name.local.to_string() == "href" {
                    layout.link_url = Some(attr.value.to_string());
                }
            }
        }
    }

    // 5. 자식 노드 배치
    let mut child_current_x = layout.dimensions.x + padding;
    let mut child_current_y = layout.dimensions.y + padding;
    let mut child_start_x = child_current_x;
    
    let children_count = style_node.children.len() as f32;
    let cell_width = if let DisplayType::TableRow = display {
        (layout.dimensions.width - (padding * 2.0)) / children_count.max(1.0)
    } else {
        0.0
    };

    let mut max_y_in_box = child_current_y;
    let mut max_x_in_box = child_current_x;

    for child in &style_node.children {
        if let markup5ever_rcdom::NodeData::Element { ref name, .. } = child.node.data {
            let t = name.local.to_string();
            if t == "head" || t == "style" || t == "meta" || t == "title" || t == "script" {
                continue;
            }
        }

        let child_container_w = if let DisplayType::TableRow = display { cell_width } else { layout.dimensions.width - (padding * 2.0) };

        let (child_layout, next_x, next_y) = build_layout_tree(
            child,
            child_start_x,
            child_current_x,
            child_current_y,
            child_container_w
        );

        if let Some(child_box) = child_layout {
            max_y_in_box = max_y_in_box.max(child_box.dimensions.y + child_box.dimensions.height);
            max_x_in_box = max_x_in_box.max(next_x);
            layout.children.push(child_box);
            child_current_x = next_x;
            child_current_y = next_y;
        }
    }

    if layout.dimensions.width <= 0.0 {
        layout.dimensions.width = (max_x_in_box - layout.dimensions.x + padding).min(container_width - (margin * 2.0));
    }
    
    let min_h = if let DisplayType::Input = display { 24.0 } else { 20.0 };
    layout.dimensions.height = (max_y_in_box - layout.dimensions.y + padding).max(min_h);

    let final_x;
    let final_y;
    if force_new_line {
        final_x = container_start_x;
        final_y = layout.dimensions.y + layout.dimensions.height + margin;
    } else {
        final_x = layout.dimensions.x + layout.dimensions.width + margin;
        final_y = current_y;
    }

    (Some(layout), final_x, final_y)
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

    pub fn get_form_controls(&self) -> Vec<(Rect, &'a StyledNode)> {
        let mut controls = Vec::new();
        if let DisplayType::Input = self.display {
            controls.push((self.dimensions, self.style_node));
        }
        for child in &self.children {
            controls.extend(child.get_form_controls());
        }
        controls
    }
}

pub fn print_layout_tree(layout: &LayoutBox, indent: usize) {
    let indent_str = " ".repeat(indent * 2);
    let tag = if let markup5ever_rcdom::NodeData::Element { ref name, .. } = layout.style_node.node.data {
        name.local.to_string()
    } else if let markup5ever_rcdom::NodeData::Text { ref contents } = layout.style_node.node.data {
        format!("Text({:?})", contents.borrow().trim())
    } else {
        "Node".to_string()
    };

    println!("{}{} [{:?}] [x: {:.1}, y: {:.1}, w: {:.1}, h: {:.1}]", 
        indent_str, tag, layout.display,
        layout.dimensions.x, layout.dimensions.y, 
        layout.dimensions.width, layout.dimensions.height);

    for child in &layout.children {
        print_layout_tree(child, indent + 1);
    }
}
