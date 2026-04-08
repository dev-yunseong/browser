use crate::style::StyledNode;
use crate::css::{Value, Unit};
use std::collections::HashMap;
use markup5ever_rcdom::NodeData;
use ab_glyph::{Font, FontRef, PxScale};

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct EdgeSizes {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone)]
pub struct LayoutBox<'a> {
    pub dimensions: Rect,
    pub padding: EdgeSizes,
    pub border: EdgeSizes,
    pub margin: EdgeSizes,
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

pub fn build_layout_tree<'a>(
    style_node: &'a StyledNode,
    container_start_x: f32,
    current_x: f32,
    current_y: f32,
    container_width: f32,
    vw: f32,
    vh: f32,
) -> (Option<LayoutBox<'a>>, f32, f32) {
    let mut layout = LayoutBox::new(style_node);
    if layout.display == DisplayType::Inline && is_none_display(style_node) {
        return (None, current_x, current_y);
    }
    layout.measure_box_model(container_width, vw, vh);
    layout.perform_layout(container_start_x, current_x, current_y, container_width, vw, vh)
}

impl<'a> LayoutBox<'a> {
    fn new(style_node: &'a StyledNode) -> Self {
        let display = get_display_type(style_node);
        let mut layout = LayoutBox {
            dimensions: Rect::default(),
            padding: EdgeSizes::default(),
            border: EdgeSizes::default(),
            margin: EdgeSizes::default(),
            style_node,
            children: Vec::new(),
            link_url: None,
            image_url: None,
            event_handlers: HashMap::new(),
            display,
        };

        if let NodeData::Element { ref attrs, ref name, .. } = style_node.node.data {
            let tag = name.local.to_string();
            for attr in attrs.borrow().iter() {
                let name = attr.name.local.to_string();
                let value = attr.value.to_string();
                match name.as_str() {
                    "href" if tag == "a" => layout.link_url = Some(value),
                    "src" if tag == "img" => layout.image_url = Some(value),
                    "onclick" => { layout.event_handlers.insert("click".to_string(), value); }
                    _ => {}
                }
            }
        }
        layout
    }

    fn measure_box_model(&mut self, container_width: f32, vw: f32, vh: f32) {
        let sn = self.style_node;
        self.margin.top = get_prop(sn, "margin-top", "margin", container_width, vw, vh);
        self.margin.bottom = get_prop(sn, "margin-bottom", "margin", container_width, vw, vh);
        self.margin.left = get_prop(sn, "margin-left", "margin", container_width, vw, vh);
        self.margin.right = get_prop(sn, "margin-right", "margin", container_width, vw, vh);
        self.padding.top = get_prop(sn, "padding-top", "padding", container_width, vw, vh);
        self.padding.bottom = get_prop(sn, "padding-bottom", "padding", container_width, vw, vh);
        self.padding.left = get_prop(sn, "padding-left", "padding", container_width, vw, vh);
        self.padding.right = get_prop(sn, "padding-right", "padding", container_width, vw, vh);

        let b_width = match sn.specified_values.get("border-width") {
            Some(Value::Length(v, Unit::Px)) => *v,
            _ => if self.display == DisplayType::Input { 1.0 } else { 0.0 },
        };
        self.border = EdgeSizes { left: b_width, right: b_width, top: b_width, bottom: b_width };
    }

    fn perform_layout(
        &mut self, 
        container_start_x: f32, 
        mut current_x: f32, 
        mut current_y: f32, 
        container_width: f32,
        vw: f32,
        vh: f32,
    ) -> (Option<LayoutBox<'a>>, f32, f32) {
        let is_block = is_block_level(self.display);
        if is_block && current_x > container_start_x {
            current_x = container_start_x;
            current_y += 5.0;
        }

        let mut width = match self.style_node.specified_values.get("width") {
            Some(Value::Length(v, Unit::Px)) => *v,
            Some(Value::Length(v, Unit::Percent)) => container_width * (v / 100.0),
            Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
            Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
            Some(Value::Length(v, Unit::Em)) => {
                let fs = match self.style_node.specified_values.get("font-size") {
                    Some(Value::Length(fv, Unit::Px)) => *fv,
                    _ => 16.0,
                };
                fs * v
            }
            _ => if is_block { (container_width - self.margin.left - self.margin.right).max(0.0) } else { 0.0 },
        };

        // Handle box-sizing: border-box (B5)
        // If border-box is used, subtract padding and border from the specified width
        let box_sizing = self.style_node.specified_values.get("box-sizing")
            .and_then(|v| if let Value::Keyword(k) = v { Some(k.as_str()) } else { None })
            .unwrap_or("content-box"); // Default is content-box

        if box_sizing == "border-box" && width > 0.0 {
            width = (width - self.padding.left - self.padding.right - self.border.left - self.border.right).max(0.0);
        }

        if let Some(Value::Length(v, Unit::Px)) = self.style_node.specified_values.get("max-width") {
            width = width.min(*v);
        }
        if let Some(Value::Length(v, Unit::Px)) = self.style_node.specified_values.get("min-width") {
            width = width.max(*v);
        }
// margin: auto 처리 (너비가 결정된 후 수행)
        if is_block && width < container_width {
            let mut is_auto = false;
            for prop in ["margin", "margin-left", "margin-right"] {
                if let Some(val) = self.style_node.specified_values.get(prop) {
                    match val {
                        Value::Keyword(s) if s.contains("auto") => { is_auto = true; break; }
                        _ => {}
                    }
                }
            }
            
            if is_auto {
                let leftover = (container_width - width).max(0.0);
                self.margin.left = leftover / 2.0;
                self.margin.right = leftover / 2.0;
            }
        }
self.dimensions.x = current_x + self.margin.left;
self.dimensions.y = current_y + self.margin.top;
self.dimensions.width = width;


        let height = match self.style_node.specified_values.get("height") {
            Some(Value::Length(v, Unit::Px)) => *v,
            Some(Value::Length(_, Unit::Percent)) => 0.0, // viewport height or parent height needed
            Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
            Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
            Some(Value::Length(v, Unit::Em)) => {
                let fs = match self.style_node.specified_values.get("font-size") {
                    Some(Value::Length(fv, Unit::Px)) => *fv,
                    _ => 16.0,
                };
                fs * v
            }
            _ => 0.0,
        };

        if let NodeData::Text { ref contents } = self.style_node.node.data {
            return self.layout_text(contents.borrow().to_string(), current_x, current_y, container_width);
        }

        // For shrink-wrap contexts (inline/table-cell with no explicit width), pass
        // INFINITY so text inside computes its natural unwrapped width instead of
        // wrapping to container_width and inflating the element to full container width.
        // Block-level children inside such a context must still receive a finite width,
        // so we keep a separate finite fallback for them.
        let effective_cw = if !is_block && width == 0.0 {
            f32::INFINITY
        } else if width > 0.0 {
            width
        } else {
            container_width
        };
        let child_container_w = (effective_cw
            - self.padding.left - self.padding.right
            - self.border.left - self.border.right)
            .max(0.0);
        // Finite child container width used for block-level children nested inside
        // a shrink-wrap parent (INFINITY must not reach block children or their
        // width would become INFINITY).
        let finite_child_cw = (if width > 0.0 { width } else { container_width }
            - self.padding.left - self.padding.right
            - self.border.left - self.border.right)
            .max(0.0);
        let mut child_x = self.dimensions.x + self.padding.left + self.border.left;
        let mut child_y = self.dimensions.y + self.padding.top + self.border.top;
        let mut max_child_y = child_y;
        let mut max_child_x = child_x;

        if self.display == DisplayType::Flex {
            // Basic flex row layout (B11): lay children out in a horizontal row.
            // flex-direction: row is the default; column is not yet implemented.
            let justify = self.style_node.specified_values.get("justify-content")
                .and_then(|v| if let Value::Keyword(k) = v { Some(k.as_str()) } else { None })
                .unwrap_or("flex-start");

            // First pass: measure all children at their natural widths.
            let mut flex_children: Vec<LayoutBox> = Vec::new();
            let mut total_child_w = 0.0f32;
            let mut max_child_h = 0.0f32;
            let mut flex_x = child_x;
            for child_node in &self.style_node.children {
                if should_skip(child_node) { continue; }
                let (child_box, next_x, _) = build_layout_tree(child_node, child_x, flex_x, child_y, child_container_w, vw, vh);
                if let Some(cb) = child_box {
                    let cw = cb.dimensions.width + cb.margin.left + cb.margin.right;
                    let ch = cb.dimensions.height + cb.margin.top + cb.margin.bottom;
                    total_child_w += cw;
                    max_child_h = max_child_h.max(ch);
                    flex_children.push(cb);
                    flex_x = next_x;
                }
            }

            // Second pass: apply justification offsets.
            let free_space = (child_container_w - total_child_w).max(0.0);
            let (start_offset, gap) = match justify {
                "center" => (free_space / 2.0, 0.0),
                "flex-end" => (free_space, 0.0),
                "space-between" => {
                    let n = flex_children.len().saturating_sub(1);
                    (0.0, if n > 0 { free_space / n as f32 } else { 0.0 })
                }
                "space-around" => {
                    let n = flex_children.len();
                    let g = if n > 0 { free_space / n as f32 } else { 0.0 };
                    (g / 2.0, g)
                }
                _ => (0.0, 0.0), // flex-start
            };

            let mut cursor_x = child_x + start_offset;
            for (i, mut cb) in flex_children.into_iter().enumerate() {
                let dx = cursor_x - cb.dimensions.x;
                offset_layout_box(&mut cb, dx, 0.0);
                cursor_x += cb.dimensions.width + cb.margin.left + cb.margin.right + if i > 0 { gap } else { 0.0 };
                max_child_y = max_child_y.max(cb.dimensions.y + cb.dimensions.height + cb.margin.bottom);
                max_child_x = max_child_x.max(cb.dimensions.x + cb.dimensions.width + cb.margin.right);
                self.children.push(cb);
            }
        } else {
            let mut line_height = 0.0f32;
            for child_node in &self.style_node.children {
                if should_skip(child_node) { continue; }
                
                let child_display = get_display_type(child_node);
                let child_is_block = is_block_level(child_display);
                
                if child_is_block {
                    // Start a new line if we were in the middle of one
                    if child_x > self.dimensions.x + self.padding.left + self.border.left {
                        child_x = self.dimensions.x + self.padding.left + self.border.left;
                        child_y += line_height;
                        line_height = 0.0;
                    }
                }

                // Block-level children must not receive INFINITY as their container width
                let cw = if child_container_w.is_infinite() && child_is_block {
                    finite_child_cw
                } else {
                    child_container_w
                };
                
                let (child_box, next_x, next_y) = build_layout_tree(child_node, self.dimensions.x + self.padding.left + self.border.left, child_x, child_y, cw, vw, vh);
                
                if let Some(cb) = child_box {
                    if child_is_block {
                        child_y = next_y;
                        child_x = self.dimensions.x + self.padding.left + self.border.left;
                        line_height = 0.0;
                    } else {
                        // Inline/InlineBlock: advance x and track line height
                        child_x = next_x;
                        line_height = line_height.max(cb.dimensions.height + cb.margin.top + cb.margin.bottom);
                        
                        // Simple wrapping: if next_x exceeds container, wrap to next line
                        if child_x > self.dimensions.x + self.padding.left + self.border.left + child_container_w && child_container_w.is_finite() {
                            child_x = self.dimensions.x + self.padding.left + self.border.left;
                            child_y += line_height;
                            line_height = 0.0;
                        }
                    }
                    
                    max_child_y = max_child_y.max(child_y + line_height);
                    max_child_x = max_child_x.max(cb.dimensions.x + cb.dimensions.width + cb.margin.right);
                    self.children.push(cb);
                }
            }
        }

        if self.dimensions.width <= 0.0 {
            let derived = max_child_x - self.dimensions.x + self.padding.right + self.border.right;
            // Clamp to the finite container_width. When container_width is INFINITY
            // (shrink-wrap context), use the derived natural width directly.
            self.dimensions.width = if container_width.is_finite() {
                derived.min(container_width)
            } else {
                derived  // natural shrink-wrap width; finite because block children used finite_child_cw
            };
        }
        // Respect explicit height (B9); otherwise derive from children.
        // Remove the unconditional max(20.0) clamp — it was distorting small boxes.
        let content_height = (max_child_y - self.dimensions.y + self.padding.bottom + self.border.bottom).max(0.0);
        let mut final_h = if height > 0.0 { height } else { content_height };

        if let Some(Value::Length(v, Unit::Px)) = self.style_node.specified_values.get("max-height") {
            final_h = final_h.min(*v);
        }
        if let Some(Value::Length(v, Unit::Px)) = self.style_node.specified_values.get("min-height") {
            final_h = final_h.max(*v);
        }
        self.dimensions.height = final_h;

        let final_x = if is_block { container_start_x } else { self.dimensions.x + self.dimensions.width + self.margin.right };
        let final_y = if is_block { self.dimensions.y + self.dimensions.height + self.margin.bottom } else { current_y };
        (Some(self.clone()), final_x, final_y)
    }

    fn layout_text(&mut self, text: String, current_x: f32, current_y: f32, container_width: f32) -> (Option<LayoutBox<'a>>, f32, f32) {
        let trimmed = text.trim();
        if trimmed.is_empty() { return (None, current_x, current_y); }
        // Use the actual specified font-size; fall back to 16px only when unspecified.
        // Do NOT clamp to 16px — that causes layout/paint to diverge for small fonts (B4).
        let font_size = match self.style_node.specified_values.get("font-size") {
            Some(Value::Length(v, Unit::Px)) => v.max(1.0),
            _ => 16.0,
        };
        let font = FontRef::try_from_slice(FONT_DATA).unwrap();
        let scale = PxScale::from(font_size);
        let units = font.units_per_em().unwrap_or(1000.0) as f32;

        let mut lines_count = 1;
        let mut line_w: f32 = 0.0;
        let mut max_w: f32 = 0.0;
        let space_w = font.h_advance_unscaled(font.glyph_id(' ')) * (scale.x / units);

        for word in trimmed.split_whitespace() {
            let mut word_w = 0.0;
            for c in word.chars() { word_w += font.h_advance_unscaled(font.glyph_id(c)) * (scale.x / units); }
            // Only wrap when container_width is a real finite constraint.
            // INFINITY = shrink-wrap context: compute natural single-line width.
            if container_width.is_finite() && line_w + word_w > container_width && line_w > 0.0 {
                max_w = max_w.max(line_w);
                line_w = word_w;
                lines_count += 1;
            } else {
                if line_w > 0.0 { line_w += space_w; }
                line_w += word_w;
            }
        }
        max_w = max_w.max(line_w);
        self.dimensions.x = current_x + self.margin.left;
        self.dimensions.y = current_y + self.margin.top;
        // In shrink-wrap context (INFINITY), use the natural text width directly.
        self.dimensions.width = if container_width.is_finite() {
            max_w.min(container_width)
        } else {
            max_w
        };
        self.dimensions.height = lines_count as f32 * (font_size * 1.4);
        let final_x = self.dimensions.x + self.dimensions.width + self.margin.right;
        // Advance y past the text block so the next sibling starts below (B8).
        let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
        (Some(self.clone()), final_x, final_y)
    }
}

fn get_prop(sn: &StyledNode, p1: &str, p2: &str, cw: f32, vw: f32, vh: f32) -> f32 {
    match sn.specified_values.get(p1).or(sn.specified_values.get(p2)) {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Length(v, Unit::Percent)) => cw * (v / 100.0),
        Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
        Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
        Some(Value::Length(v, Unit::Em)) => {
            // Em should be relative to current element's font-size
            let fs = match sn.specified_values.get("font-size") {
                Some(Value::Length(fv, Unit::Px)) => *fv,
                _ => 16.0,
            };
            fs * v
        }
        _ => 0.0,
    }
}

fn get_display_type(sn: &StyledNode) -> DisplayType {
    if let NodeData::Text { .. } = sn.node.data { return DisplayType::Inline; }
    if let Some(Value::Keyword(d)) = sn.specified_values.get("display") {
        match d.as_str() {
            "block" => return DisplayType::Block,
            "inline-block" => return DisplayType::InlineBlock,
            "flex" => return DisplayType::Flex,
            "none" => return DisplayType::Inline, // will be handled by is_none_display
            _ => {}
        }
    }
    if let NodeData::Element { ref name, .. } = sn.node.data {
        match name.local.to_string().as_str() {
            // Genuine block-level elements (fill container width, force line break)
            "html" |
            "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" |
            "body" | "header" | "footer" | "nav" | "section" | "article" |
            "ul" | "ol" | "li" | "main" | "aside" | "form" |
            "details" | "summary" | "figure" | "figcaption" | "address" |
            "blockquote" | "pre" | "hr" | "fieldset" | "legend" => DisplayType::Block,
            // table and its sub-elements: use TableRow/TableCell so they shrink-wrap
            // rather than expand to full container width like block elements do.
            "table" => DisplayType::Table,
            "tr" => DisplayType::TableRow,
            "th" | "td" => DisplayType::TableCell,
            "thead" | "tbody" | "tfoot" | "caption" => DisplayType::Block,
            "input" | "button" | "select" | "textarea" => DisplayType::Input,
            "img" => DisplayType::Image,
            _ => DisplayType::Inline,
        }
    } else { DisplayType::Block }
}


fn is_block_level(d: DisplayType) -> bool {
    // Table/TableRow/TableCell are NOT block-level: they shrink-wrap to content
    // rather than filling the full container width.
    matches!(d, DisplayType::Block | DisplayType::ListItem | DisplayType::Flex)
}

fn is_none_display(sn: &StyledNode) -> bool {
    if let Some(Value::Keyword(d)) = sn.specified_values.get("display") { d == "none" } else { false }
}

fn should_skip(child: &StyledNode) -> bool {
    if let NodeData::Element { ref name, .. } = child.node.data {
        let t = name.local.to_string();
        matches!(t.as_str(), "head" | "style" | "meta" | "title" | "script" | "link" | "noscript")
    } else { false }
}

pub fn offset_layout_box(layout: &mut LayoutBox, dx: f32, dy: f32) {
    layout.dimensions.x += dx;
    layout.dimensions.y += dy;
    for child in &mut layout.children { offset_layout_box(child, dx, dy); }
}

impl<'a> LayoutBox<'a> {
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&LayoutBox<'a>> {
        let d = self.dimensions;
        if x >= d.x && x <= d.x + d.width && y >= d.y && y <= d.y + d.height {
            for child in self.children.iter().rev() {
                if let Some(node) = child.hit_test(x, y) { return Some(node); }
            }
            return Some(self);
        }
        None
    }
    pub fn collect_links(&self, list: &mut Vec<(Rect, String)>) {
        if let Some(ref url) = self.link_url { list.push((self.dimensions, url.clone())); }
        for child in &self.children { child.collect_links(list); }
    }
    pub fn collect_event_handlers(&self, list: &mut Vec<(Rect, String)>) {
        if let Some(script) = self.event_handlers.get("click") { list.push((self.dimensions, script.clone())); }
        for child in &self.children { child.collect_event_handlers(list); }
    }
    pub fn collect_form_controls(&self, list: &mut Vec<(Rect, &'a StyledNode)>) {
        // Only collect text-input-like controls, NOT buttons.
        // Buttons are handled via collect_event_handlers (onclick).
        // If we add buttons here, egui puts a TextEdit overlay on top which
        // consumes the click before the onclick handler can fire.
        if self.display == DisplayType::Input {
            if let NodeData::Element { ref name, .. } = self.style_node.node.data {
                let tag = name.local.to_string();
                if matches!(tag.as_str(), "input" | "textarea" | "select") {
                    list.push((self.dimensions, self.style_node));
                }
            }
        }
        for child in &self.children { child.collect_form_controls(list); }
    }
    pub fn collect_images(&self, list: &mut Vec<(Rect, String)>) {
        if let Some(ref url) = self.image_url { list.push((self.dimensions, url.clone())); }
        for child in &self.children { child.collect_images(list); }
    }
}

pub fn print_layout_tree(layout: &LayoutBox, indent: usize) {
    let indent_str = " ".repeat(indent * 2);
    println!("{}{} [{:?}] [{:.1},{:.1} {:.1}x{:.1}]", indent_str, "Node", layout.display, layout.dimensions.x, layout.dimensions.y, layout.dimensions.width, layout.dimensions.height);
    for child in &layout.children { print_layout_tree(child, indent + 1); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;
    use crate::css;
    use crate::style;

    #[test]
    fn test_button_coordinate_collection() {
        let html = r#"<button onclick="alert(1)" style="width: 100px; height: 50px; margin: 10px;">Click me</button>"#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());
        
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 1024.0, 1024.0, 768.0);
        let layout = layout_opt.unwrap();
        
        let mut handlers = Vec::new();
        layout.collect_event_handlers(&mut handlers);
        
        assert_eq!(handlers.len(), 1);
        let (rect, script) = &handlers[0];
        assert_eq!(script, "alert(1)");
        // x: current_x(0) + margin_left(10) = 10.0
        assert_eq!(rect.x, 10.0);
        assert_eq!(rect.width, 100.0);
    }

    #[test]
    fn test_margin_auto_centering() {
        let html = r#"<div style="display: block; width: 500px; margin: auto;">Content</div>"#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let mut style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());
        
        // Ensure the style is manually set if parser was ambiguous
        if let NodeData::Element { .. } = style_tree.children[0].node.data {
            style_tree.children[0].specified_values.insert("display".to_string(), css::Value::Keyword("block".to_string()));
            style_tree.children[0].specified_values.insert("width".to_string(), css::Value::Length(500.0, css::Unit::Px));
            style_tree.children[0].specified_values.insert("margin".to_string(), css::Value::Keyword("auto".to_string()));
        }

        let (layout_opt, _, _) = build_layout_tree(&style_tree.children[0], 0.0, 0.0, 0.0, 1000.0, 1000.0, 768.0);
        let layout = layout_opt.unwrap();
        
        assert_eq!(layout.dimensions.width, 500.0);
        assert_eq!(layout.dimensions.x, 250.0); // (1000 - 500) / 2
    }

    #[test]
    fn test_text_keeps_parent_flow_position() {
        let html = r#"<div style="margin-left: 48px; margin-top: 24px;">Hello world</div>"#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());

        let div_node = style_tree
            .children
            .iter()
            .find(|child| {
                matches!(
                    child.node.data,
                    NodeData::Element { ref name, .. } if name.local.to_string() == "html"
                )
            })
            .and_then(|html| {
                html.children.iter().find(|child| {
                    matches!(
                        child.node.data,
                        NodeData::Element { ref name, .. } if name.local.to_string() == "body"
                    )
                })
            })
            .and_then(|body| {
                body.children.iter().find(|child| {
                    matches!(
                        child.node.data,
                        NodeData::Element { ref name, .. } if name.local.to_string() == "div"
                    )
                })
            })
            .unwrap();

        let (layout_opt, _, _) = build_layout_tree(div_node, 0.0, 0.0, 0.0, 800.0, 800.0, 768.0);
        let layout = layout_opt.unwrap();
        let text = find_first_inline(&layout).unwrap();

        assert_eq!(text.dimensions.x, 48.0);
        assert_eq!(text.dimensions.y, 24.0);
        assert!(text.dimensions.width > 0.0);
        assert!(text.dimensions.height > 0.0);
    }

    fn find_first_inline<'a>(layout: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
        if matches!(layout.display, DisplayType::Inline) {
            return Some(layout);
        }

        for child in &layout.children {
            if let Some(found) = find_first_inline(child) {
                return Some(found);
            }
        }

        None
    }

    #[test]
    fn test_inline_element_shrinks_to_content() {
        // An inline <span> should derive its width from text content,
        // NOT expand to the full container_width (800px).
        let html = r#"<span>Hi</span>"#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new());

        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 768.0);
        let layout = layout_opt.unwrap();

        fn find_span<'a>(b: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
            if let NodeData::Element { ref name, .. } = b.style_node.node.data {
                if name.local.to_string() == "span" { return Some(b); }
            }
            for c in &b.children { if let Some(f) = find_span(c) { return Some(f); } }
            None
        }

        let span = find_span(&layout).expect("span not found");
        assert!(span.dimensions.width > 0.0, "span width must be > 0");
        assert!(span.dimensions.width < 800.0,
            "span width {} must be < container_width 800 (should shrink to content)",
            span.dimensions.width);
    }
}
