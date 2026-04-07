use crate::style::StyledNode;
use crate::css::{Value, Unit};
use std::collections::HashMap;
use markup5ever_rcdom::NodeData;

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
    pub image_url: Option<String>,
    pub event_handlers: HashMap<String, String>,
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
    Image,
    Flex,
}

fn get_display_type(style_node: &StyledNode) -> DisplayType {
    if let NodeData::Text { .. } = style_node.node.data {
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
            "flex" | "inline-flex" => return DisplayType::Flex,
            "none" => {}
            _ => {}
        }
    }

    if let NodeData::Element { ref name, .. } = style_node.node.data {
        let tag = name.local.to_string();
        match tag.as_str() {
            "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" |
            "ul" | "ol" | "header" | "footer" | "nav" | "section" |
            "article" | "main" | "aside" | "body" | "html" | "form" |
            "fieldset" | "blockquote" | "pre" | "hr" | "figure" => DisplayType::Block,
            "li" => DisplayType::ListItem,
            "table" => DisplayType::Table,
            "tr" => DisplayType::TableRow,
            "td" | "th" => DisplayType::TableCell,
            "input" | "textarea" | "button" | "select" => DisplayType::Input,
            "img" => DisplayType::Image,
            _ => DisplayType::Inline,
        }
    } else {
        DisplayType::Block
    }
}

fn should_skip(child: &StyledNode) -> bool {
    if let NodeData::Element { ref name, .. } = child.node.data {
        let t = name.local.to_string();
        matches!(t.as_str(), "head" | "style" | "meta" | "title" | "script" | "link" | "noscript")
    } else {
        false
    }
}

fn get_length(style_node: &StyledNode, prop: &str, default: f32, container_width: f32) -> f32 {
    match style_node.specified_values.get(prop) {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Length(v, Unit::Vw)) => container_width * (v / 100.0),
        Some(Value::Length(v, Unit::Percent)) => container_width * (v / 100.0),
        Some(Value::Length(v, Unit::Em)) => v * 16.0,
        _ => default,
    }
}

fn get_keyword<'a>(style_node: &'a StyledNode, prop: &str) -> Option<&'a str> {
    match style_node.specified_values.get(prop) {
        Some(Value::Keyword(s)) => Some(s.as_str()),
        _ => None,
    }
}

pub fn build_layout_tree<'a>(
    style_node: &'a StyledNode,
    container_start_x: f32,
    mut current_x: f32,
    mut current_y: f32,
    container_width: f32,
) -> (Option<LayoutBox<'a>>, f32, f32) {
    if let Some(Value::Keyword(d)) = style_node.specified_values.get("display") {
        if d == "none" {
            return (None, current_x, current_y);
        }
    }

    let display = get_display_type(style_node);

    // ── Spacing ──────────────────────────────────────────────────────────────
    let padding_top = get_length(style_node, "padding-top", 0.0, container_width)
        .max(get_length(style_node, "padding", 0.0, container_width));
    let padding_bottom = get_length(style_node, "padding-bottom", 0.0, container_width)
        .max(get_length(style_node, "padding", 0.0, container_width));
    let padding_left = get_length(style_node, "padding-left", 0.0, container_width)
        .max(get_length(style_node, "padding", 0.0, container_width));
    let padding_right = get_length(style_node, "padding-right", 0.0, container_width)
        .max(get_length(style_node, "padding", 0.0, container_width));
    let padding_h = padding_left + padding_right;

    let margin_top = get_length(style_node, "margin-top", 0.0, container_width)
        .max(get_length(style_node, "margin", 0.0, container_width));
    let margin_bottom = get_length(style_node, "margin-bottom", 0.0, container_width)
        .max(get_length(style_node, "margin", 0.0, container_width));
    let margin_left = get_length(style_node, "margin-left", 0.0, container_width)
        .max(get_length(style_node, "margin", 0.0, container_width));
    let margin_right = get_length(style_node, "margin-right", 0.0, container_width)
        .max(get_length(style_node, "margin", 0.0, container_width));
    let margin_h = margin_left + margin_right;

    // ── Text node ────────────────────────────────────────────────────────────
    if let NodeData::Text { ref contents } = style_node.node.data {
        let text = contents.borrow().to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return (None, current_x, current_y);
        }

        let font_size = get_length(style_node, "font-size", 16.0, container_width);
        let avg_char_width = 8.0 * (font_size / 16.0);
        let line_height = font_size * 1.4;

        let available_width = container_width - (current_x - container_start_x) - margin_h;
        if available_width < avg_char_width && current_x > container_start_x {
            current_y += line_height;
            current_x = container_start_x;
        }

        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let max_chars = ((container_width - padding_h) / avg_char_width).max(1.0) as usize;

        let mut lines_count = 0;
        let mut current_line_len = 0usize;
        let mut estimated_width: f32 = 0.0;

        for word in &words {
            if current_line_len + word.len() + 1 > max_chars && current_line_len > 0 {
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

        let total_height = lines_count as f32 * line_height;
        let layout_width = if lines_count > 1 {
            container_width - margin_h
        } else {
            estimated_width
        };

        let layout = LayoutBox {
            dimensions: Rect {
                x: current_x + margin_left,
                y: current_y + margin_top,
                width: layout_width.min(container_width - margin_h),
                height: total_height,
            },
            style_node,
            children: Vec::new(),
            link_url: None,
            image_url: None,
            event_handlers: HashMap::new(),
            display,
        };

        let (next_x, next_y) = if lines_count > 1 {
            (container_start_x, current_y + total_height + margin_bottom)
        } else {
            (current_x + layout.dimensions.width + margin_right, current_y)
        };

        return (Some(layout), next_x, next_y);
    }

    // ── Element node ─────────────────────────────────────────────────────────
    let is_block = matches!(display,
        DisplayType::Block | DisplayType::ListItem | DisplayType::Table |
        DisplayType::TableRow | DisplayType::Flex
    );

    if is_block && current_x > container_start_x {
        current_y += 0.0; // block already starts on new line
        current_x = container_start_x;
    }

    // Explicit width
    let layout_width = match style_node.specified_values.get("width") {
        Some(Value::Length(w, Unit::Px)) => *w,
        Some(Value::Length(w, Unit::Vw)) => container_width * (w / 100.0),
        Some(Value::Length(w, Unit::Percent)) => (container_width - margin_h) * (w / 100.0),
        Some(Value::Length(w, Unit::Em)) => w * 16.0,
        _ => match display {
            DisplayType::Block | DisplayType::ListItem | DisplayType::Table |
            DisplayType::Flex => container_width - margin_h,
            DisplayType::Input => 200.0,
            DisplayType::Image => 100.0,
            _ => 0.0,
        },
    };

    // Cap at max-width
    let max_width = match style_node.specified_values.get("max-width") {
        Some(Value::Length(w, Unit::Px)) => *w,
        Some(Value::Length(w, Unit::Percent)) => container_width * (w / 100.0),
        _ => f32::MAX,
    };
    let layout_width = layout_width.min(max_width);

    let x_offset = if let DisplayType::ListItem = display { 20.0 } else { 0.0 };

    let mut layout = LayoutBox {
        dimensions: Rect {
            x: current_x + margin_left + x_offset,
            y: current_y + margin_top,
            width: layout_width,
            height: 0.0,
        },
        style_node,
        children: Vec::new(),
        link_url: None,
        image_url: None,
        event_handlers: HashMap::new(),
        display,
    };

    // Collect element attributes
    if let NodeData::Element { ref attrs, ref name, .. } = style_node.node.data {
        let tag = name.local.to_string();
        for attr in attrs.borrow().iter() {
            let attr_name = attr.name.local.to_string();
            let attr_val = attr.value.to_string();
            match attr_name.as_str() {
                "href" if tag == "a" => layout.link_url = Some(attr_val),
                "src" if tag == "img" => layout.image_url = Some(attr_val),
                "onclick" => { layout.event_handlers.insert("click".to_string(), attr_val); }
                _ => {}
            }
        }
        if tag == "img" {
            let h = get_length(style_node, "height", 100.0, container_width);
            layout.dimensions.height = h;
            if layout_width == 100.0 {
                layout.dimensions.width = get_length(style_node, "width", 100.0, container_width);
            }
        }
    }

    // ── Dispatch to flex layout ───────────────────────────────────────────────
    if let DisplayType::Flex = display {
        return build_flex_layout(style_node, layout, padding_left, padding_right, padding_top, padding_bottom, margin_bottom, container_start_x);
    }

    // ── Normal child layout ───────────────────────────────────────────────────
    let child_container_width = layout.dimensions.width - padding_h;
    let child_start_x = layout.dimensions.x + padding_left;
    let mut child_current_x = child_start_x;
    let mut child_current_y = layout.dimensions.y + padding_top;

    let children_count = style_node.children.iter()
        .filter(|c| !should_skip(c))
        .count() as f32;
    let cell_width = if let DisplayType::TableRow = display {
        (child_container_width) / children_count.max(1.0)
    } else {
        0.0
    };

    let mut max_y_in_box = child_current_y;

    for child in &style_node.children {
        if should_skip(child) { continue; }

        let child_w = if let DisplayType::TableRow = display {
            cell_width
        } else {
            child_container_width
        };

        let (child_layout, next_x, next_y) = build_layout_tree(
            child,
            child_start_x,
            child_current_x,
            child_current_y,
            child_w,
        );

        if let Some(child_box) = child_layout {
            max_y_in_box = max_y_in_box.max(child_box.dimensions.y + child_box.dimensions.height);
            layout.children.push(child_box);
            child_current_x = next_x;
            child_current_y = next_y;
        }
    }

    // Auto-size width if not specified
    if layout.dimensions.width <= 0.0 {
        layout.dimensions.width = (child_current_x - layout.dimensions.x + padding_right)
            .min(container_width - margin_h);
    }

    // Explicit height
    let explicit_height = match style_node.specified_values.get("height") {
        Some(Value::Length(h, Unit::Px)) => Some(*h),
        Some(Value::Length(_h, Unit::Percent)) => None, // skip for now
        _ => None,
    };

    let min_height = match display {
        DisplayType::Input => 30.0,
        DisplayType::Image => 100.0,
        _ => 0.0,
    };

    layout.dimensions.height = if let Some(h) = explicit_height {
        h
    } else {
        (max_y_in_box - layout.dimensions.y + padding_bottom).max(min_height)
    };

    let (final_x, final_y) = if is_block {
        (container_start_x, layout.dimensions.y + layout.dimensions.height + margin_bottom)
    } else {
        (layout.dimensions.x + layout.dimensions.width + margin_right, current_y)
    };

    (Some(layout), final_x, final_y)
}

/// Flexbox layout: positions children according to flex rules.
fn build_flex_layout<'a>(
    style_node: &'a StyledNode,
    mut layout: LayoutBox<'a>,
    padding_left: f32,
    padding_right: f32,
    padding_top: f32,
    padding_bottom: f32,
    margin_bottom: f32,
    container_start_x: f32,
) -> (Option<LayoutBox<'a>>, f32, f32) {
    let flex_direction = get_keyword(style_node, "flex-direction").unwrap_or("row");
    let justify_content = get_keyword(style_node, "justify-content").unwrap_or("flex-start");
    let align_items = get_keyword(style_node, "align-items").unwrap_or("stretch");

    let inner_x = layout.dimensions.x + padding_left;
    let inner_y = layout.dimensions.y + padding_top;
    let inner_width = layout.dimensions.width - padding_left - padding_right;

    // Build all children at (0,0) to get their natural sizes
    let mut children: Vec<LayoutBox<'a>> = Vec::new();
    for child in &style_node.children {
        if should_skip(child) { continue; }
        let (child_box, _, _) = build_layout_tree(child, 0.0, 0.0, 0.0, inner_width);
        if let Some(cb) = child_box {
            children.push(cb);
        }
    }

    if flex_direction == "column" {
        // Stack children vertically
        let mut current_y = inner_y;
        for child in &mut children {
            let dx = inner_x - child.dimensions.x;
            let dy = current_y - child.dimensions.y;
            offset_layout_box(child, dx, dy);
            current_y += child.dimensions.height;
        }
        let total_height = (current_y - inner_y + padding_bottom).max(20.0);
        layout.dimensions.height = total_height;
        layout.children = children;
        let final_y = layout.dimensions.y + layout.dimensions.height + margin_bottom;
        return (Some(layout), container_start_x, final_y);
    }

    // Row layout
    let total_child_width: f32 = children.iter().map(|c| c.dimensions.width).sum();
    let max_child_height: f32 = children.iter()
        .map(|c| c.dimensions.height)
        .fold(0.0_f32, f32::max);
    let n = children.len();

    let (start_x, gap) = match justify_content {
        "center" => (
            inner_x + (inner_width - total_child_width) / 2.0,
            0.0
        ),
        "flex-end" => (inner_x + inner_width - total_child_width, 0.0),
        "space-between" => {
            let gap = if n > 1 {
                (inner_width - total_child_width) / (n as f32 - 1.0)
            } else {
                0.0
            };
            (inner_x, gap.max(0.0))
        }
        "space-around" => {
            let unit = if n > 0 {
                (inner_width - total_child_width) / n as f32
            } else {
                0.0
            };
            (inner_x + unit / 2.0, unit.max(0.0))
        }
        "space-evenly" => {
            let unit = if n > 0 {
                (inner_width - total_child_width) / (n as f32 + 1.0)
            } else {
                0.0
            };
            (inner_x + unit, unit.max(0.0))
        }
        _ => (inner_x, 0.0), // flex-start
    };

    let mut current_x = start_x;
    for child in &mut children {
        let child_y = match align_items {
            "center" => inner_y + (max_child_height - child.dimensions.height) / 2.0,
            "flex-end" => inner_y + max_child_height - child.dimensions.height,
            "stretch" => {
                // Stretch child height to fill container
                child.dimensions.height = max_child_height;
                inner_y
            }
            _ => inner_y, // flex-start
        };

        let dx = current_x - child.dimensions.x;
        let dy = child_y - child.dimensions.y;
        offset_layout_box(child, dx, dy);
        current_x += child.dimensions.width + gap;
    }

    let total_height = (max_child_height + padding_top + padding_bottom).max(20.0);
    layout.dimensions.height = total_height;
    layout.children = children;
    let final_y = layout.dimensions.y + layout.dimensions.height + margin_bottom;
    (Some(layout), container_start_x, final_y)
}

/// Recursively offset a layout box and all its descendants.
pub fn offset_layout_box(layout: &mut LayoutBox, dx: f32, dy: f32) {
    layout.dimensions.x += dx;
    layout.dimensions.y += dy;
    for child in &mut layout.children {
        offset_layout_box(child, dx, dy);
    }
}

// ── LayoutBox methods ─────────────────────────────────────────────────────────

impl<'a> LayoutBox<'a> {
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&LayoutBox<'a>> {
        let d = self.dimensions;
        if x >= d.x && x <= d.x + d.width && y >= d.y && y <= d.y + d.height {
            for child in self.children.iter().rev() {
                if let Some(node) = child.hit_test(x, y) {
                    return Some(node);
                }
            }
            return Some(self);
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

    pub fn get_images(&self) -> Vec<(Rect, String)> {
        let mut images = Vec::new();
        if let Some(ref url) = self.image_url {
            images.push((self.dimensions, url.clone()));
        }
        for child in &self.children {
            images.extend(child.get_images());
        }
        images
    }
}

pub fn print_layout_tree(layout: &LayoutBox, indent: usize) {
    let indent_str = " ".repeat(indent * 2);
    let tag = match &layout.style_node.node.data {
        NodeData::Element { ref name, .. } => name.local.to_string(),
        NodeData::Text { ref contents } => format!("Text({:?})", &contents.borrow()[..contents.borrow().len().min(20)]),
        _ => "Node".to_string(),
    };
    println!("{}{} [{:?}] [{:.1},{:.1} {:.1}x{:.1}]",
        indent_str, tag, layout.display,
        layout.dimensions.x, layout.dimensions.y,
        layout.dimensions.width, layout.dimensions.height);
    for child in &layout.children {
        print_layout_tree(child, indent + 1);
    }
}
