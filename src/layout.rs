use crate::style::StyledNode;
use crate::css::{Value, Unit};
use std::collections::HashMap;
use markup5ever_rcdom::NodeData;
use ab_glyph::{Font, FontRef, PxScale};
extern crate stacker;

// ── Intrinsic sizing helpers ──────────────────────────────────────────────────

/// Measure the width of `text` rendered at `font_size` px.
/// When `wrap_width` is `f32::INFINITY`, no wrapping occurs (max-content).
/// When finite, line-breaks at word boundaries (min-content: longest word).
fn measure_text_width(text: &str, font_size: f32, wrap_width: f32) -> f32 {
    let trimmed = text.trim();
    if trimmed.is_empty() { return 0.0; }
    let font = FontRef::try_from_slice(FONT_DATA).unwrap();
    let scale = PxScale::from(font_size.max(1.0));
    let units = font.units_per_em().unwrap_or(1000.0) as f32;
    let space_w = font.h_advance_unscaled(font.glyph_id(' ')) * (scale.x / units);

    let mut max_w: f32 = 0.0;
    let mut line_w: f32 = 0.0;

    for word in trimmed.split_whitespace() {
        let mut word_w = 0.0f32;
        for c in word.chars() {
            word_w += font.h_advance_unscaled(font.glyph_id(c)) * (scale.x / units);
        }
        if wrap_width.is_finite() && line_w + word_w > wrap_width && line_w > 0.0 {
            max_w = max_w.max(line_w);
            line_w = 0.0;
        }
        if line_w > 0.0 { line_w += space_w; }
        line_w += word_w;
    }
    max_w.max(line_w)
}

/// Read a raw `px` value from `specified_values` for a single property.
/// Returns 0.0 for anything that isn't an explicit pixel length.
fn read_px_direct(sn: &StyledNode, prop: &str) -> f32 {
    match sn.specified_values.get(&crate::css::intern(prop)) {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    }
}

/// Horizontal padding + border contribution for intrinsic sizing (px only).
fn horiz_padding_border(sn: &StyledNode) -> f32 {
    read_px_direct(sn, "padding-left")
        + read_px_direct(sn, "padding-right")
        + read_px_direct(sn, "border-width") * 2.0
}

/// Compute the **max-content** width of a `StyledNode` subtree.
///
/// - Text nodes: total width with no line wrapping.
/// - Images: explicit `width` attribute/style, or 100 px default.
/// - `display: none`: 0.
/// - Block elements: max over children's max-content widths.
/// - Inline/inline-block elements: sum of children's max-content widths on one line.
pub fn compute_max_content_width(sn: &StyledNode, vw: f32, vh: f32) -> f32 {
    // Iterative post-order traversal to avoid stack overflows on deep DOM trees.
    //
    // Frame::Pre  — process the node's early-return cases or push children + Post.
    // Frame::Post — reconstruct the parent's inline-run / block logic from child values.
    //
    // `val_stack` holds computed widths; Post frames pop their children's widths and push
    // the aggregated result.  The final answer is `val_stack[0]`.

    enum Frame<'a> {
        Pre(&'a StyledNode),
        /// Carries metadata needed to reconstruct the inline-run accumulation:
        ///   - node pointer for re-deriving child display types
        ///   - number of non-skipped children whose values are on `val_stack`
        ///   - pad_border offset for the parent
        Post {
            node: *const StyledNode,
            num_children: usize,
            pad_border: f32,
        },
    }

    let mut work: Vec<Frame> = vec![Frame::Pre(sn)];
    let mut val_stack: Vec<f32> = Vec::new();

    while let Some(frame) = work.pop() {
        match frame {
            Frame::Pre(node) => {
                if is_none_display(node) { val_stack.push(0.0); continue; }

                if let NodeData::Text { ref contents } = node.node.data {
                    let font_size = match node.specified_values.get(&crate::css::intern("font-size")) {
                        Some(Value::Length(v, Unit::Px)) => *v,
                        _ => 16.0,
                    };
                    val_stack.push(measure_text_width(&contents.borrow(), font_size, f32::INFINITY));
                    continue;
                }

                let disp = get_display_type(node);
                if disp == DisplayType::Image {
                    let w = read_px_direct(node, "width");
                    val_stack.push(if w > 0.0 { w } else { 100.0 });
                    continue;
                }
                if should_skip(node) { val_stack.push(0.0); continue; }
                if let Some(Value::Length(v, Unit::Px)) = node.specified_values.get(&crate::css::intern("width")) {
                    val_stack.push(*v);
                    continue;
                }

                let pad_border = horiz_padding_border(node);
                let non_skip: Vec<&StyledNode> = node.children.iter().filter(|c| !should_skip(c)).collect();
                let num_children = non_skip.len();

                // Push Post before children so it processes AFTER all children are done.
                work.push(Frame::Post { node: node as *const StyledNode, num_children, pad_border });
                for child in non_skip.into_iter().rev() {
                    work.push(Frame::Pre(child));
                }
            }
            Frame::Post { node, num_children, pad_border } => {
                // Re-derive children display types from the stored node pointer.
                // SAFETY: `node` was derived from a reference that outlives this call,
                // and we only read it (no mutation), so this is safe.
                let node_ref = unsafe { &*node };
                let non_skip_children: Vec<&StyledNode> = node_ref.children.iter()
                    .filter(|c| !should_skip(c))
                    .collect();

                // Pop child values in forward order (they were pushed in reverse → LIFO gives forward).
                let start = val_stack.len().saturating_sub(num_children);
                let child_vals: Vec<f32> = val_stack.drain(start..).collect();

                let mut inline_run_width: f32 = 0.0;
                let mut max_w: f32 = 0.0;
                for (child_val, child) in child_vals.into_iter().zip(non_skip_children.iter()) {
                    let child_disp = get_display_type(child);
                    if is_block_level(child_disp) {
                        max_w = max_w.max(inline_run_width);
                        inline_run_width = 0.0;
                        max_w = max_w.max(child_val);
                    } else {
                        inline_run_width += child_val;
                    }
                }
                max_w = max_w.max(inline_run_width);
                val_stack.push(max_w + pad_border);
            }
        }
    }

    val_stack.pop().unwrap_or(0.0)
}

/// Compute the **min-content** width of a `StyledNode` subtree.
///
/// - Text nodes: width of the longest single unbreakable word.
/// - Images: explicit `width` attribute/style, or 100 px default.
/// - `display: none`: 0.
/// - All elements: max over children's min-content widths (wrapping can isolate any child).
pub fn compute_min_content_width(sn: &StyledNode, vw: f32, vh: f32) -> f32 {
    // Iterative approach to avoid stack overflows.
    enum Frame<'a> {
        Pre(&'a StyledNode),
        Post { num_children: usize, pad_border: f32 },
    }

    let mut work: Vec<Frame> = vec![Frame::Pre(sn)];
    let mut val_stack: Vec<f32> = Vec::new();

    while let Some(frame) = work.pop() {
        match frame {
            Frame::Pre(node) => {
                if is_none_display(node) { val_stack.push(0.0); continue; }

                if let NodeData::Text { ref contents } = node.node.data {
                    let font_size = match node.specified_values.get(&crate::css::intern("font-size")) {
                        Some(Value::Length(v, Unit::Px)) => *v,
                        _ => 16.0,
                    };
                    let text = contents.borrow();
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        val_stack.push(0.0);
                    } else {
                        let min_w = trimmed.split_whitespace()
                            .map(|word| measure_text_width(word, font_size, f32::INFINITY))
                            .fold(0.0f32, f32::max);
                        val_stack.push(min_w);
                    }
                    continue;
                }

                let disp = get_display_type(node);
                if disp == DisplayType::Image {
                    let w = read_px_direct(node, "width");
                    val_stack.push(if w > 0.0 { w } else { 100.0 });
                    continue;
                }
                if should_skip(node) { val_stack.push(0.0); continue; }
                if let Some(Value::Length(v, Unit::Px)) = node.specified_values.get(&crate::css::intern("width")) {
                    val_stack.push(*v);
                    continue;
                }

                let pad_border = horiz_padding_border(node);
                let non_skip: Vec<&StyledNode> = node.children.iter().filter(|c| !should_skip(c)).collect();
                let num_children = non_skip.len();

                work.push(Frame::Post { num_children, pad_border });
                for child in non_skip.into_iter().rev() {
                    work.push(Frame::Pre(child));
                }
            }
            Frame::Post { num_children, pad_border } => {
                let start = val_stack.len().saturating_sub(num_children);
                let child_vals = val_stack.drain(start..);
                let max_child = child_vals.fold(0.0f32, f32::max);
                val_stack.push(max_child + pad_border);
            }
        }
    }

    val_stack.pop().unwrap_or(0.0)
}

fn is_shrink_wrap(d: DisplayType) -> bool {
    matches!(d, DisplayType::InlineBlock | DisplayType::Table | DisplayType::TableCell | DisplayType::Image)
}

// ── Float layout types ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum FloatSide { Left, Right }

#[derive(Clone, Copy, PartialEq, Debug)]
enum ClearValue { Left, Right, Both }

#[derive(Clone, Debug)]
struct FloatArea { y: f32, height: f32, width: f32, side: FloatSide }

struct FloatContext { areas: Vec<FloatArea>, container_width: f32 }

impl FloatContext {
    fn new(container_width: f32) -> Self {
        FloatContext { areas: vec![], container_width }
    }
    /// Returns (avail_width, left_indent) for a horizontal band at y..y+max(h,1).
    fn available_at(&self, y: f32, h: f32) -> (f32, f32) {
        let band = h.max(1.0);
        let mut left_w = 0.0f32;
        let mut right_w = 0.0f32;
        for fa in &self.areas {
            if fa.y < y + band && fa.y + fa.height > y {
                match fa.side {
                    FloatSide::Left  => left_w  += fa.width,
                    FloatSide::Right => right_w += fa.width,
                }
            }
        }
        let avail = (self.container_width - left_w - right_w).max(0.0);
        (avail, left_w)
    }
    /// Minimum y to be completely clear of floats on the given side.
    fn clear_y(&self, cv: ClearValue) -> f32 {
        self.areas.iter()
            .filter(|fa| match cv {
                ClearValue::Left  => fa.side == FloatSide::Left,
                ClearValue::Right => fa.side == FloatSide::Right,
                ClearValue::Both  => true,
            })
            .map(|fa| fa.y + fa.height)
            .fold(0.0f32, f32::max)
    }
    /// Bottom edge of the lowest registered float.
    fn bottom(&self) -> f32 {
        self.areas.iter().map(|fa| fa.y + fa.height).fold(0.0f32, f32::max)
    }
    fn add(&mut self, area: FloatArea) { self.areas.push(area); }
}

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);
        Rect {
            x,
            y,
            width: (x2 - x).max(0.0),
            height: (y2 - y).max(0.0),
        }
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        let x_overlap = self.x < other.x + other.width && self.x + self.width > other.x;
        let y_overlap = self.y < other.y + other.height && self.y + self.height > other.y;
        x_overlap && y_overlap
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct EdgeSizes {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

#[derive(Debug)]
pub struct LayoutBox<'a> {
    pub dimensions: Rect,
    pub padding: EdgeSizes,
    pub border: EdgeSizes,
    pub margin: EdgeSizes,
    pub style_node: &'a StyledNode,
    pub children: Vec<LayoutBox<'a>>,
    pub link_url: Option<String>,
    pub image_url: Option<String>,
    pub alt_text: Option<String>,
    pub event_handlers: HashMap<String, String>,
    pub display: DisplayType,
    pub z_index: i32,
}

impl<'a> Clone for LayoutBox<'a> {
    /// Iterative clone to avoid stack overflows on deeply nested layout trees.
    ///
    /// The default derive(Clone) would call `children.clone()` which recurses
    /// into each child's clone, potentially blowing the stack with thousands of
    /// nested elements.  This implementation uses an explicit work stack.
    fn clone(&self) -> Self {
        // Strategy: post-order traversal using raw pointers so the lifetime
        // of the source reference doesn't constrain the Frame<'a> type parameter.
        //
        // SAFETY: Each pointer on the stack points into the *original* tree being
        // cloned.  We only read through them (no writes); the borrow of `self`
        // that drives the entire clone call ensures all source nodes remain live.

        enum Frame<'f> {
            /// Pointer to a source node that still needs to be cloned.
            Pre(*const LayoutBox<'f>),
            /// A partially-built clone waiting for its children.
            Post { num_children: usize, partial: LayoutBox<'f> },
        }

        let mut work: Vec<Frame<'a>> = vec![Frame::Pre(self as *const LayoutBox<'a>)];
        let mut result_stack: Vec<LayoutBox<'a>> = Vec::new();

        while let Some(frame) = work.pop() {
            match frame {
                Frame::Pre(src_ptr) => {
                    // SAFETY: pointer was derived from a live reference; no aliased writes.
                    let src = unsafe { &*src_ptr };
                    let partial = LayoutBox {
                        dimensions: src.dimensions,
                        padding: src.padding,
                        border: src.border,
                        margin: src.margin,
                        style_node: src.style_node,
                        children: Vec::with_capacity(src.children.len()),
                        link_url: src.link_url.clone(),
                        image_url: src.image_url.clone(),
                        alt_text: src.alt_text.clone(),
                        event_handlers: src.event_handlers.clone(),
                        display: src.display,
                        z_index: src.z_index,
                    };
                    let num_children = src.children.len();
                    // Push Post first so it is processed after all children.
                    work.push(Frame::Post { num_children, partial });
                    // Push children in reverse so the first child is popped first.
                    for child in src.children.iter().rev() {
                        work.push(Frame::Pre(child as *const LayoutBox<'a>));
                    }
                }
                Frame::Post { num_children, mut partial } => {
                    // Drain the last num_children cloned nodes from result_stack.
                    let start = result_stack.len().saturating_sub(num_children);
                    partial.children = result_stack.drain(start..).collect();
                    result_stack.push(partial);
                }
            }
        }

        result_stack.pop().expect("LayoutBox::clone: result stack must have exactly one element")
    }
}

impl<'a> Drop for LayoutBox<'a> {
    /// Iterative drop to avoid stack overflows on deeply nested layout trees.
    ///
    /// The default recursive drop (impl'd by the compiler for Vec<LayoutBox>)
    /// would recurse once per nesting level.  With 5 000 nested elements this
    /// blows the stack in debug builds.  We instead drain the tree breadth-first
    /// into a work queue so the OS call stack depth stays O(1).
    fn drop(&mut self) {
        // Drain self.children into the work queue, leaving self.children empty.
        // When this function returns, Rust's generated destructor runs on `self`,
        // but self.children is now empty so no recursive drop occurs.
        let mut queue: Vec<LayoutBox<'a>> = std::mem::take(&mut self.children);
        while let Some(mut node) = queue.pop() {
            // Move node's children into the queue before node is dropped.
            queue.extend(std::mem::take(&mut node.children));
            // node is now dropped here with children = [], so no recursion.
        }
    }
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
    // Guard against stack overflow on deeply nested DOM trees.
    // Allocate a fresh 64 MiB stack segment when less than 512 KiB remains.
    // A single large segment is more reliable than many chained small segments;
    // 64 MiB / ~8 KB per frame (debug) ≈ 8192 frames — enough for 5000-level DOMs.
    stacker::maybe_grow(512 * 1024, 64 * 1024 * 1024, move || {
        let mut layout = LayoutBox::new(style_node);
        if layout.display == DisplayType::Inline && is_none_display(style_node) {
            return (None, current_x, current_y);
        }
        layout.measure_box_model(container_width, vw, vh);
        layout.perform_layout(container_start_x, current_x, current_y, container_width, vw, vh)
    })
}

impl<'a> LayoutBox<'a> {
    fn new(style_node: &'a StyledNode) -> Self {
        let display = get_display_type(style_node);
        let z_index = match style_node.specified_values.get(&crate::css::intern("z-index")) {
            Some(Value::Number(n)) => *n as i32,
            _ => 0,
        };
        let mut layout = LayoutBox {
            dimensions: Rect::default(),
            padding: EdgeSizes::default(),
            border: EdgeSizes::default(),
            margin: EdgeSizes::default(),
            style_node,
            children: Vec::new(),
            link_url: None,
            image_url: None,
            alt_text: None,
            event_handlers: HashMap::new(),
            display,
            z_index,
        };

        if let NodeData::Element { ref attrs, ref name, .. } = style_node.node.data {
            let tag = name.local.to_string();
            for attr in attrs.borrow().iter() {
                let name = attr.name.local.to_string();
                let value = attr.value.to_string();
                match name.as_str() {
                    "href" if tag == "a" => layout.link_url = Some(value),
                    "src" if tag == "img" => layout.image_url = Some(value),
                    "alt" if tag == "img" => layout.alt_text = Some(value),
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

        let b_width = match sn.specified_values.get(&crate::css::intern("border-width")) {
            Some(Value::Length(v, Unit::Px)) => *v,
            _ => if self.display == DisplayType::Input { 1.0 } else { 0.0 },
        };
        self.border = EdgeSizes { left: b_width, right: b_width, top: b_width, bottom: b_width };
    }

    fn perform_layout(
        &mut self, 
        container_start_x: f32, 
        _initial_x: f32, 
        mut current_y: f32, 
        container_width: f32,
        vw: f32,
        vh: f32,
    ) -> (Option<LayoutBox<'a>>, f32, f32) {
        let is_block = is_block_level(self.display);
        
        // Block formatting context or similar check
        if is_block && _initial_x > container_start_x {
            current_y += 5.0; // Break line before block
        }

        let mut width = match self.style_node.specified_values.get(&crate::css::intern("width")) {
            Some(Value::Length(v, Unit::Px)) => *v,
            Some(Value::Length(v, Unit::Percent)) => container_width * (v / 100.0),
            Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
            Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
            // CSS Intrinsic & Extrinsic Sizing Level 3
            Some(Value::Keyword(k)) if **k == *"min-content" => {
                compute_min_content_width(self.style_node, vw, vh)
            }
            Some(Value::Keyword(k)) if **k == *"max-content" => {
                compute_max_content_width(self.style_node, vw, vh)
            }
            Some(Value::Keyword(k)) if **k == *"fit-content" => {
                let max_c = compute_max_content_width(self.style_node, vw, vh);
                let min_c = compute_min_content_width(self.style_node, vw, vh);
                // fit-content without argument: min(max-content, max(min-content, available))
                max_c.min(container_width).max(min_c)
            }
            Some(Value::FitContent(limit)) => {
                let limit = *limit;
                let max_c = compute_max_content_width(self.style_node, vw, vh);
                let min_c = compute_min_content_width(self.style_node, vw, vh);
                // fit-content(N): min(max-content, max(min-content, min(available, N)))
                let available = container_width.min(limit);
                max_c.min(available).max(min_c)
            }
            _ => {
                if is_shrink_wrap(self.display) {
                    let max_c = compute_max_content_width(self.style_node, vw, vh);
                    let min_c = compute_min_content_width(self.style_node, vw, vh);
                    // Shrink-wrap: min(max-content, max(min-content, available))
                    max_c.min(container_width).max(min_c)
                } else if is_block {
                    (container_width - self.margin.left - self.margin.right).max(0.0)
                } else {
                    0.0
                }
            }
        };

        let box_sizing = self.style_node.specified_values.get(&crate::css::intern("box-sizing"))
            .and_then(|v| if let Value::Keyword(k) = v { Some(&**k) } else { None })
            .unwrap_or("content-box");

        if box_sizing == "border-box" && width > 0.0 {
            width = (width - self.padding.left - self.padding.right - self.border.left - self.border.right).max(0.0);
        }

        if let Some(Value::Length(v, Unit::Px)) = self.style_node.specified_values.get(&crate::css::intern("max-width")) { width = width.min(*v); }
        if let Some(Value::Length(v, Unit::Px)) = self.style_node.specified_values.get(&crate::css::intern("min-width")) { width = width.max(*v); }

        if is_block && width < container_width {
            let mut is_auto = false;
            for prop in ["margin", "margin-left", "margin-right"] {
                if let Some(Value::Keyword(s)) = self.style_node.specified_values.get(&crate::css::intern(prop)) {
                    if s.contains("auto") { is_auto = true; break; }
                }
            }
            if is_auto {
                let leftover = (container_width - width).max(0.0);
                self.margin.left = leftover / 2.0;
                self.margin.right = leftover / 2.0;
            }
        }

        self.dimensions.x = container_start_x + self.margin.left;
        self.dimensions.y = current_y + self.margin.top;
        self.dimensions.width = width;

        let height = match self.style_node.specified_values.get(&crate::css::intern("height")) {
            Some(Value::Length(v, Unit::Px)) => *v,
            Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
            Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
            // CSS Intrinsic & Extrinsic Sizing Level 3 — height axis
            // For block containers, min-content and max-content height are both
            // equivalent to the natural auto height (content-derived). Return 0.0
            // so the existing content-height calculation takes over.
            Some(Value::Keyword(k)) if **k == *"min-content" || **k == *"max-content" || **k == *"fit-content" => 0.0,
            Some(Value::FitContent(_)) => 0.0,
            _ => 0.0,
        };

        if let NodeData::Text { ref contents } = self.style_node.node.data {
            return self.layout_text(contents.borrow().to_string(), self.dimensions.x, current_y, container_width);
        }

        // Image sizing: images need explicit dimension handling before child layout.
        // The image cache is not available at layout time, so we use placeholder
        // dimensions based on CSS-specified values. The object-fit logic at render time
        // will use the actual decoded image dimensions.
        if self.display == DisplayType::Image {
            // width is already set by the shrink-wrap / explicit-CSS path above.
            // If no CSS width was specified, the shrink-wrap path returns a value from
            // compute_max_content_width (100px default for images). Keep that or fall back to 150.
            if self.dimensions.width <= 0.0 {
                self.dimensions.width = 150.0_f32.min(container_width);
            }
            // Use CSS height if specified; otherwise derive a 2:3 placeholder from width.
            let final_h = if height > 0.0 { height } else { self.dimensions.width * 0.667 };
            self.dimensions.height = final_h.max(1.0);
            let final_x = self.dimensions.x + self.dimensions.width + self.margin.right;
            let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
            return (Some(self.clone()), final_x, final_y);
        }

        let inner_width = if width > 0.0 { width } else { (container_width - self.padding.left - self.padding.right - self.border.left - self.border.right).max(0.0) };
        let mut child_y = self.dimensions.y + self.padding.top + self.border.top;
        let mut max_child_x = self.dimensions.x;

        if self.display == DisplayType::Flex {
            let flex_direction = self.style_node.specified_values.get(&crate::css::intern("flex-direction"))
                .and_then(|v| if let Value::Keyword(k) = v { Some(&**k) } else { None })
                .unwrap_or("row");
            let justify = self.style_node.specified_values.get(&crate::css::intern("justify-content"))
                .and_then(|v| if let Value::Keyword(k) = v { Some(&**k) } else { None })
                .unwrap_or("flex-start");

            let mut flex_children = Vec::new();
            let mut total_main_size = 0.0f32;
            let mut max_cross_size = 0.0f32;

            for child_node in &self.style_node.children {
                if should_skip(child_node) { continue; }
                // In flex, we often want children to shrink-wrap first
                let (cb_opt, _, _) = build_layout_tree(child_node, 0.0, 0.0, 0.0, f32::INFINITY, vw, vh);
                if let Some(cb) = cb_opt {
                    if flex_direction == "row" {
                        total_main_size += cb.dimensions.width + cb.margin.left + cb.margin.right;
                        max_cross_size = max_cross_size.max(cb.dimensions.height + cb.margin.top + cb.margin.bottom);
                    } else {
                        total_main_size += cb.dimensions.height + cb.margin.top + cb.margin.bottom;
                        max_cross_size = max_cross_size.max(cb.dimensions.width + cb.margin.left + cb.margin.right);
                    }
                    flex_children.push(cb);
                }
            }

            let main_container_size = if flex_direction == "row" { inner_width } else { height.max(total_main_size) };
            let free_space = (main_container_size - total_main_size).max(0.0);
            
            // Simple flex-grow: distribute free space equally among children for now
            let grow_share = if !flex_children.is_empty() { free_space / flex_children.len() as f32 } else { 0.0 };

            let mut main_cursor = 0.0f32;
            // Apply justification offsets if no grow
            if free_space > 0.0 && grow_share < 0.1 {
                match justify {
                    "center" => main_cursor += free_space / 2.0,
                    "flex-end" => main_cursor += free_space,
                    _ => {}
                }
            }

            for mut cb in flex_children {
                let main_grow = if flex_direction == "row" { grow_share } else { 0.0 };
                let cross_grow = if flex_direction == "row" { 0.0 } else { grow_share };
                
                cb.dimensions.width += main_grow;
                cb.dimensions.height += cross_grow;

                let (x, y) = if flex_direction == "row" {
                    (self.dimensions.x + self.padding.left + self.border.left + main_cursor + cb.margin.left,
                     self.dimensions.y + self.padding.top + self.border.top + cb.margin.top)
                } else {
                    (self.dimensions.x + self.padding.left + self.border.left + cb.margin.left,
                     self.dimensions.y + self.padding.top + self.border.top + main_cursor + cb.margin.top)
                };
                
                let dx = x - cb.dimensions.x;
                let dy = y - cb.dimensions.y;
                offset_layout_box(&mut cb, dx, dy);
                
                main_cursor += if flex_direction == "row" { 
                    cb.dimensions.width + cb.margin.left + cb.margin.right 
                } else { 
                    cb.dimensions.height + cb.margin.top + cb.margin.bottom 
                };
                
                max_child_x = max_child_x.max(cb.dimensions.x + cb.dimensions.width + cb.margin.right);
                child_y = child_y.max(cb.dimensions.y + cb.dimensions.height + cb.margin.bottom);
                self.children.push(cb);
            }
            
            // Finalize flex container size
            if self.dimensions.width <= 0.0 { self.dimensions.width = if flex_direction == "row" { main_container_size } else { max_cross_size }; }
            self.dimensions.height = if flex_direction == "row" { max_cross_size } else { main_container_size };
            
            let final_x = if is_block { container_start_x } else { self.dimensions.x + self.dimensions.width + self.margin.right };
            let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
            return (Some(self.clone()), final_x, final_y);
        }

        // --- FLOAT-AWARE SINGLE-PASS LAYOUT ---
        //
        // Pass 1 (classify): iterate self.style_node.children (immutable borrow of self)
        //   and classify each child as Float / Block / Inline.
        // Pass 2 (build+position): build LayoutBoxes and push to a local `result` Vec.
        //   After the loop the immutable borrow on self.style_node ends, so we can
        //   assign `self.children = result` safely.
        //
        // This two-step split is necessary to satisfy Rust's borrow checker:
        // we cannot call self.children.push() while borrowing self.style_node.children.

        enum ChildKind { Float(FloatSide), Block, Inline }
        struct ChildEntry<'entry> {
            node: &'entry StyledNode,
            kind: ChildKind,
            clear: Option<ClearValue>,
        }

        let mut entries: Vec<ChildEntry<'a>> = Vec::new();
        for child_node in &self.style_node.children {
            if should_skip(child_node) { continue; }
            let float_side = get_float(child_node);
            let clear_val  = get_clear(child_node);
            let child_disp = get_display_type(child_node);
            let kind = if let Some(side) = float_side {
                ChildKind::Float(side)
            } else if is_block_level(child_disp) {
                ChildKind::Block
            } else {
                ChildKind::Inline
            };
            entries.push(ChildEntry { node: child_node, kind, clear: clear_val });
        }
        // Immutable borrow of self.style_node.children is now released.

        let container_x = self.dimensions.x + self.padding.left + self.border.left;
        let mut float_ctx = FloatContext::new(inner_width);
        let mut cursor_y = self.dimensions.y + self.padding.top + self.border.top;
        let mut prev_margin_bottom = 0.0f32;
        let mut result: Vec<LayoutBox<'a>> = Vec::new();

        // Inline line accumulator
        struct InlineLine<'a> { members: Vec<LayoutBox<'a>>, width: f32, height: f32 }
        let mut cur_line = InlineLine::<'a> { members: vec![], width: 0.0, height: 0.0 };
        let mut line_start_y = cursor_y;

        // Flush the current inline line into `result`, advancing cursor_y.
        macro_rules! flush_line {
            () => {
                if !cur_line.members.is_empty() {
                    let (_, left_indent) = float_ctx.available_at(line_start_y, cur_line.height.max(1.0));
                    let mut lx = container_x + left_indent;
                    for mut m in cur_line.members.drain(..) {
                        let dx = lx - (m.dimensions.x - m.margin.left);
                        let dy = cursor_y - (m.dimensions.y - m.margin.top);
                        offset_layout_box(&mut m, dx, dy);
                        max_child_x = max_child_x.max(m.dimensions.x + m.dimensions.width + m.margin.right);
                        lx += m.dimensions.width + m.margin.left + m.margin.right;
                        result.push(m);
                    }
                    cursor_y += cur_line.height;
                    cur_line.width = 0.0;
                    cur_line.height = 0.0;
                    line_start_y = cursor_y;
                }
            }
        }

        for entry in entries {
            match entry.kind {
                // ── Float child ───────────────────────────────────────────────
                ChildKind::Float(side) => {
                    flush_line!();
                    // Build with origin (0,0); offset_layout_box will reposition.
                    // Use inner_width so explicit CSS widths resolve correctly.
                    let (cb_opt, _, _) = build_layout_tree(entry.node, 0.0, 0.0, 0.0, inner_width, vw, vh);
                    if let Some(mut cb) = cb_opt {
                        let float_w = cb.dimensions.width + cb.margin.left + cb.margin.right;
                        let float_h = cb.dimensions.height + cb.margin.top + cb.margin.bottom;
                        let (avail_w, left_indent) = float_ctx.available_at(cursor_y, float_h);
                        let fx = match side {
                            FloatSide::Left  => container_x + left_indent,
                            FloatSide::Right => container_x + left_indent + avail_w - float_w,
                        };
                        let dx = fx - (cb.dimensions.x - cb.margin.left);
                        let dy = cursor_y - (cb.dimensions.y - cb.margin.top);
                        offset_layout_box(&mut cb, dx, dy);
                        float_ctx.add(FloatArea { y: cursor_y, height: float_h, width: float_w, side });
                        max_child_x = max_child_x.max(cb.dimensions.x + cb.dimensions.width + cb.margin.right);
                        result.push(cb);
                    }
                    // cursor_y does NOT advance for floats
                }

                // ── Block child ───────────────────────────────────────────────
                ChildKind::Block => {
                    flush_line!();
                    if let Some(cv) = entry.clear {
                        cursor_y = float_ctx.clear_y(cv).max(cursor_y);
                    }
                    let (avail_w, left_indent) = float_ctx.available_at(cursor_y, 0.0);
                    let block_x = container_x + left_indent;
                    let (cb_opt, _, _) = build_layout_tree(entry.node, block_x, block_x, 0.0, avail_w, vw, vh);
                    if let Some(mut cb) = cb_opt {
                        let collapsed = prev_margin_bottom.max(cb.margin.top);
                        let dy = (cursor_y + collapsed) - (cb.dimensions.y + cb.margin.top);
                        offset_layout_box(&mut cb, 0.0, dy);
                        cursor_y = cb.dimensions.y + cb.dimensions.height;
                        prev_margin_bottom = cb.margin.bottom;
                        max_child_x = max_child_x.max(cb.dimensions.x + cb.dimensions.width + cb.margin.right);
                        result.push(cb);
                    }
                    line_start_y = cursor_y; // keep line_start_y in sync after block advances cursor_y
                }

                // ── Inline child ──────────────────────────────────────────────
                ChildKind::Inline => {
                    prev_margin_bottom = 0.0;
                    let (avail_w, left_indent) = float_ctx.available_at(line_start_y, cur_line.height.max(16.0));
                    let (cb_opt, _, _) = build_layout_tree(
                        entry.node,
                        container_x + left_indent,
                        container_x + left_indent + cur_line.width,
                        0.0, avail_w, vw, vh,
                    );
                    if let Some(cb) = cb_opt {
                        let item_w = cb.dimensions.width + cb.margin.left + cb.margin.right;
                        if cur_line.width + item_w > avail_w && !cur_line.members.is_empty() {
                            flush_line!();
                            // Re-lay out for new line with updated float-aware width
                            let (aw2, li2) = float_ctx.available_at(line_start_y, 16.0);
                            let (cb2_opt, _, _) = build_layout_tree(
                                entry.node,
                                container_x + li2, container_x + li2,
                                0.0, aw2, vw, vh,
                            );
                            if let Some(cb2) = cb2_opt {
                                cur_line.width  = cb2.dimensions.width  + cb2.margin.left + cb2.margin.right;
                                cur_line.height = cb2.dimensions.height + cb2.margin.top  + cb2.margin.bottom;
                                cur_line.members.push(cb2);
                            }
                        } else {
                            cur_line.width  += item_w;
                            cur_line.height  = cur_line.height.max(
                                cb.dimensions.height + cb.margin.top + cb.margin.bottom);
                            cur_line.members.push(cb);
                        }
                    }
                }
            }
        }

        flush_line!();
        cursor_y += prev_margin_bottom;
        // Clearfix: ensure the container is tall enough to cover all floated children.
        cursor_y = cursor_y.max(float_ctx.bottom());

        // Now safe to mutably assign self.children (immutable borrow of self.style_node ended above).
        self.children = result;

        if self.dimensions.width <= 0.0 {
            let derived = max_child_x - self.dimensions.x + self.padding.right + self.border.right;
            self.dimensions.width = if container_width.is_finite() { derived.min(container_width) } else { derived };
        }

        let content_height = (cursor_y - self.dimensions.y + self.padding.bottom + self.border.bottom).max(0.0);
        let mut final_h = if height > 0.0 { height } else { content_height };
        if let Some(Value::Length(v, Unit::Px)) = self.style_node.specified_values.get(&crate::css::intern("max-height")) { final_h = final_h.min(*v); }
        if let Some(Value::Length(v, Unit::Px)) = self.style_node.specified_values.get(&crate::css::intern("min-height")) { final_h = final_h.max(*v); }
        self.dimensions.height = final_h;

        let final_x = if is_block { container_start_x } else { self.dimensions.x + self.dimensions.width + self.margin.right };
        let final_y = if is_block { self.dimensions.y + self.dimensions.height + self.margin.bottom } else { cursor_y };
        (Some(self.clone()), final_x, final_y)
    }

    fn layout_text(&mut self, text: String, current_x: f32, current_y: f32, container_width: f32) -> (Option<LayoutBox<'a>>, f32, f32) {
        let trimmed = text.trim();
        if trimmed.is_empty() { return (None, current_x, current_y); }
        let font_size = match self.style_node.specified_values.get(&crate::css::intern("font-size")) {
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
            
            // If word is too long for current line
            if container_width.is_finite() && line_w + word_w > container_width && line_w > 0.0 {
                max_w = max_w.max(line_w);
                line_w = 0.0;
                lines_count += 1;
            }

            // If a single word is LONGER than the entire container, we must break it char-by-char
            if container_width.is_finite() && word_w > container_width {
                for c in word.chars() {
                    let char_w = font.h_advance_unscaled(font.glyph_id(c)) * (scale.x / units);
                    if line_w + char_w > container_width && line_w > 0.0 {
                        max_w = max_w.max(line_w);
                        line_w = 0.0;
                        lines_count += 1;
                    }
                    line_w += char_w;
                }
            } else {
                if line_w > 0.0 { line_w += space_w; }
                line_w += word_w;
            }
        }
        max_w = max_w.max(line_w);
        
        self.dimensions.x = current_x + self.margin.left;
        self.dimensions.y = current_y + self.margin.top;
        self.dimensions.width = if container_width.is_finite() { max_w.min(container_width) } else { max_w };
        
        let line_height = font_size * 1.4;
        self.dimensions.height = lines_count as f32 * line_height;
        
        let final_x = self.dimensions.x + self.dimensions.width + self.margin.right;
        let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
        (Some(self.clone()), final_x, final_y)
    }
}

fn get_float(sn: &StyledNode) -> Option<FloatSide> {
    match sn.specified_values.get(&crate::css::intern("float")) {
        Some(Value::Keyword(k)) => match &**k {
            "left"  => Some(FloatSide::Left),
            "right" => Some(FloatSide::Right),
            _ => None,
        },
        _ => None,
    }
}

fn get_clear(sn: &StyledNode) -> Option<ClearValue> {
    match sn.specified_values.get(&crate::css::intern("clear")) {
        Some(Value::Keyword(k)) => match &**k {
            "left"  => Some(ClearValue::Left),
            "right" => Some(ClearValue::Right),
            "both"  => Some(ClearValue::Both),
            _ => None,
        },
        _ => None,
    }
}

fn get_prop(sn: &StyledNode, p1: &str, p2: &str, cw: f32, vw: f32, vh: f32) -> f32 {
    match sn.specified_values.get(&crate::css::intern(p1)).or(sn.specified_values.get(&crate::css::intern(p2))) {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Length(v, Unit::Percent)) => cw * (v / 100.0),
        Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
        Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
        _ => 0.0,
    }
}

fn get_display_type(sn: &StyledNode) -> DisplayType {
    if let NodeData::Text { .. } = sn.node.data { return DisplayType::Inline; }
    if let Some(Value::Keyword(d)) = sn.specified_values.get(&crate::css::intern("display")) {
        match &**d {
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
    if let Some(Value::Keyword(d)) = sn.specified_values.get(&crate::css::intern("display")) { **d == *"none" } else { false }
}

fn should_skip(child: &StyledNode) -> bool {
    if let NodeData::Element { ref name, .. } = child.node.data {
        let t = name.local.to_string();
        matches!(t.as_str(), "head" | "style" | "meta" | "title" | "script" | "link" | "noscript")
    } else { false }
}

/// Iterative replacement for the formerly recursive offset_layout_box.
/// Walks the entire LayoutBox tree with an explicit stack to avoid stack overflows.
///
/// SAFETY note: We use raw pointers here to work around the borrow checker's inability to
/// prove that each node is visited exactly once.  The tree structure guarantees no aliasing
/// (each LayoutBox is owned by exactly one parent), and we only write to `dimensions.x/y`
/// (not to the `children` slice itself), so there is no overlap between the write target
/// and the pointer sources on the stack.
pub fn offset_layout_box(layout: &mut LayoutBox, dx: f32, dy: f32) {
    // Use a stack of raw mutable pointers so we can push children without holding
    // a mutable borrow on the parent at the same time.
    let mut stack: Vec<*mut LayoutBox> = vec![layout as *mut LayoutBox];
    while let Some(ptr) = stack.pop() {
        // SAFETY: Each pointer comes from a uniquely-owned LayoutBox node; no two
        // entries on the stack alias the same allocation.
        let node = unsafe { &mut *ptr };
        node.dimensions.x += dx;
        node.dimensions.y += dy;
        for child in &mut node.children {
            stack.push(child as *mut LayoutBox);
        }
    }
}

impl<'a> LayoutBox<'a> {
    /// Iterative hit-test.  Visits children in reverse order (last painter wins)
    /// using an explicit DFS stack to avoid stack overflows on deep trees.
    ///
    /// Semantics match the original recursive implementation:
    ///   1. Check whether `self` contains the point.
    ///   2. Among children (in reverse/last-painter-first order), recurse into
    ///      the first subtree that hits.
    ///   3. If no child hits, return `self`.
    ///
    /// The stack carries `(node, child_index)` pairs so we can iterate children
    /// of each node one by one and abort as soon as we find a hit.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<&LayoutBox<'a>> {
        // Check if the root contains the point at all.
        let root_d = self.dimensions;
        if !(x >= root_d.x && x <= root_d.x + root_d.width
            && y >= root_d.y && y <= root_d.y + root_d.height)
        {
            return None;
        }

        // DFS stack: each entry is a node that contains the point, plus the index of
        // the next child to try (children are tried in reverse, i.e., last painter first).
        // Invariant: every node on the stack contains the point.
        let mut stack: Vec<(&LayoutBox<'a>, isize)> = vec![(self, self.children.len() as isize - 1)];

        while let Some((node, child_idx)) = stack.last_mut() {
            let idx = *child_idx;
            if idx < 0 {
                // No more children to try for this node — it is the deepest hit.
                let result = *node;
                stack.pop();
                return Some(result);
            }
            *child_idx -= 1;
            let child = &node.children[idx as usize];
            let d = child.dimensions;
            if x >= d.x && x <= d.x + d.width && y >= d.y && y <= d.y + d.height {
                // This child contains the point — descend into it.
                let num = child.children.len() as isize - 1;
                stack.push((child, num));
            }
        }

        // Stack is empty — root was the only hit.
        Some(self)
    }
    /// Iterative collect_links — avoids stack overflow on deep trees.
    pub fn collect_links(&self, list: &mut Vec<(Rect, String)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let Some(ref url) = node.link_url { list.push((node.dimensions, url.clone())); }
            for child in node.children.iter().rev() { stack.push(child); }
        }
    }

    /// Iterative collect_event_handlers — avoids stack overflow on deep trees.
    pub fn collect_event_handlers(&self, list: &mut Vec<(Rect, String)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let Some(script) = node.event_handlers.get("click") {
                list.push((node.dimensions, script.clone()));
            }
            for child in node.children.iter().rev() { stack.push(child); }
        }
    }

    /// Iterative collect_form_controls — avoids stack overflow on deep trees.
    ///
    /// Only collects text-input-like controls, NOT buttons.
    /// Buttons are handled via collect_event_handlers (onclick).
    /// If we add buttons here, egui puts a TextEdit overlay on top which
    /// consumes the click before the onclick handler can fire.
    pub fn collect_form_controls(&self, list: &mut Vec<(Rect, &'a StyledNode)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if node.display == DisplayType::Input {
                if let NodeData::Element { ref name, .. } = node.style_node.node.data {
                    let tag = name.local.to_string();
                    if matches!(tag.as_str(), "input" | "textarea" | "select") {
                        list.push((node.dimensions, node.style_node));
                    }
                }
            }
            for child in node.children.iter().rev() { stack.push(child); }
        }
    }

    /// Iterative collect_images — avoids stack overflow on deep trees.
    pub fn collect_images(&self, list: &mut Vec<(Rect, String)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let Some(ref url) = node.image_url { list.push((node.dimensions, url.clone())); }
            for child in node.children.iter().rev() { stack.push(child); }
        }
    }

    /// Iterative collect_element_ids — avoids stack overflow on deep trees.
    pub fn collect_element_ids(&self, list: &mut Vec<(Rect, String)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let NodeData::Element { ref attrs, .. } = node.style_node.node.data {
                for attr in attrs.borrow().iter() {
                    if attr.name.local.to_string() == "id" {
                        list.push((node.dimensions, attr.value.to_string()));
                    }
                }
            }
            for child in node.children.iter().rev() { stack.push(child); }
        }
    }

    /// Iterative collect_focusable_elements — avoids stack overflow on deep trees.
    pub fn collect_focusable_elements(&self, list: &mut Vec<(Rect, String)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let NodeData::Element { ref name, ref attrs, .. } = node.style_node.node.data {
                let tag = name.local.to_string();
                let mut id = None;
                let mut has_href = false;

                for attr in attrs.borrow().iter() {
                    let key = attr.name.local.to_string();
                    if key == "id" { id = Some(attr.value.to_string()); }
                    if key == "href" { has_href = true; }
                }

                let is_focusable = match tag.as_str() {
                    "a" => has_href,
                    "button" | "input" | "select" | "textarea" => true,
                    _ => false,
                };

                if is_focusable {
                    // Only focus elements with explicit IDs for simplicity.
                    if let Some(actual_id) = id {
                        list.push((node.dimensions, actual_id));
                    }
                }
            }
            for child in node.children.iter().rev() { stack.push(child); }
        }
    }

    pub fn establishes_stacking_context(&self) -> bool {
        self.z_index != 0 || self.establishes_bfc()
    }

    pub fn establishes_bfc(&self) -> bool {
        match self.display {
            DisplayType::InlineBlock | DisplayType::Flex | DisplayType::TableCell => true,
            _ => {
                // overflow != visible also establishes BFC
                if let Some(Value::Keyword(v)) = self.style_node.specified_values.get(&crate::css::intern("overflow")) {
                    **v != *"visible"
                } else {
                    false
                }
            }
        }
    }
}

/// Iterative print_layout_tree — avoids stack overflow on deep trees.
pub fn print_layout_tree(layout: &LayoutBox, indent: usize) {
    let mut stack: Vec<(&LayoutBox, usize)> = vec![(layout, indent)];
    while let Some((node, depth)) = stack.pop() {
        let indent_str = " ".repeat(depth * 2);
        println!("{}{} [{:?}] [{:.1},{:.1} {:.1}x{:.1}]",
            indent_str, "Node", node.display,
            node.dimensions.x, node.dimensions.y,
            node.dimensions.width, node.dimensions.height);
        // Push children in reverse order so the first child is printed first.
        for child in node.children.iter().rev() {
            stack.push((child, depth + 1));
        }
    }
}

impl<'a> LayoutBox<'a> {
    pub fn get_content_rect(&self) -> Rect {
        Rect {
            x: self.dimensions.x + self.border.left + self.padding.left,
            y: self.dimensions.y + self.border.top + self.padding.top,
            width: (self.dimensions.width - self.border.left - self.border.right - self.padding.left - self.padding.right).max(0.0),
            height: (self.dimensions.height - self.border.top - self.border.bottom - self.padding.top - self.padding.bottom).max(0.0),
        }
    }

    /// Returns the CSS `opacity` value for this box, clamped to [0.0, 1.0].
    /// Defaults to 1.0 (fully opaque) if the property is absent or unparseable.
    pub fn get_opacity(&self) -> f32 {
        match self.style_node.specified_values.get(&crate::css::intern("opacity")) {
            Some(Value::Number(n)) => n.clamp(0.0, 1.0),
            _ => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;
    use crate::dom;
    use crate::css;
    use crate::style;

    #[test]
    fn test_button_coordinate_collection() {
        let html = r#"<button onclick="alert(1)" style="width: 100px; height: 50px; margin: 10px;">Click me</button>"#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None, None);
        
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
        let mut style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None, None);
        
        // Ensure the style is manually set if parser was ambiguous
        if let NodeData::Element { .. } = style_tree.children[0].node.data {
            let mut map = (*style_tree.children[0].specified_values.0).clone();
            map.insert(crate::css::intern("display"), css::Value::Keyword(crate::css::intern("block")));
            map.insert(crate::css::intern("width"), css::Value::Length(500.0, css::Unit::Px));
            map.insert(crate::css::intern("margin-left"), css::Value::Keyword(crate::css::intern("auto")));
            map.insert(crate::css::intern("margin-right"), css::Value::Keyword(crate::css::intern("auto")));
            style_tree.children[0].specified_values = style::PropertyMap(Arc::new(map));
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
        let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None, None);

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
        let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None, None);

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

    // ── Float layout tests ────────────────────────────────────────────────────

    /// Deep-search the layout tree for the first child that has `float: left` or `float: right`.
    fn find_float_child_deep<'a>(layout: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
        for child in &layout.children {
            match child.style_node.specified_values.get(&crate::css::intern("float")) {
                Some(Value::Keyword(k)) if &**k == "left" || &**k == "right" => return Some(child),
                _ => {}
            }
            if let Some(f) = find_float_child_deep(child) { return Some(f); }
        }
        None
    }

    /// Among the DIRECT children of `layout`, return the first Block-display child
    /// that does NOT have a `float` CSS property set.
    fn find_direct_non_float_block<'a>(layout: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
        for child in &layout.children {
            if child.display == DisplayType::Block
                && !child.style_node.specified_values.contains_key("float")
            {
                return Some(child);
            }
        }
        None
    }

    /// Navigate html > body > first-div and return that div's layout box.
    fn find_outer_div<'a>(root: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
        for child in &root.children {
            if let NodeData::Element { ref name, .. } = child.style_node.node.data {
                if name.local.to_string() == "html" {
                    for body in &child.children {
                        if let NodeData::Element { ref name, .. } = body.style_node.node.data {
                            if name.local.to_string() == "body" {
                                for div in &body.children {
                                    if let NodeData::Element { ref name, .. } = div.style_node.node.data {
                                        if name.local.to_string() == "div" {
                                            return Some(div);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    #[test]
    fn test_float_left_x() {
        let html = r#"<div style="width:800px;"><div style="float:left;width:100px;height:50px;">F</div></div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom_tree.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let float_child = find_float_child_deep(&layout).expect("float child not found");
        assert_eq!(float_child.dimensions.x, 0.0,
            "float:left child should have x=0.0, got {}", float_child.dimensions.x);
    }

    #[test]
    fn test_float_right_x() {
        let html = r#"<div style="width:800px;"><div style="float:right;width:100px;height:50px;">F</div></div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom_tree.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let float_child = find_float_child_deep(&layout).expect("float child not found");
        assert_eq!(float_child.dimensions.x, 700.0,
            "float:right child (width 100px) in 800px container should have x=700.0, got {}",
            float_child.dimensions.x);
    }

    #[test]
    fn test_clear_left_advances_cursor() {
        let html = r#"<div style="width:800px;"><div style="float:left;width:100px;height:50px;">F</div><div style="clear:left;">C</div></div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom_tree.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        // Navigate to the outer div (width:800px) then look at its direct children.
        let outer_div = find_outer_div(&layout).expect("outer div not found");
        let clear_block = find_direct_non_float_block(outer_div)
            .expect("clear:left block not found among outer div's children");
        assert!(clear_block.dimensions.y >= 50.0,
            "clear:left block must start at or below float bottom (50px), got y={}",
            clear_block.dimensions.y);
    }

    #[test]
    fn test_float_intrusion_narrows_sibling_block() {
        // float:left 100px wide → sibling block in the same container gets avail_w = 700px
        let html = r#"<div style="width:800px;"><div style="float:left;width:100px;height:50px;">F</div><div style="display:block;">S</div></div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom_tree.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let outer_div = find_outer_div(&layout).expect("outer div not found");
        let sibling = find_direct_non_float_block(outer_div)
            .expect("non-float sibling block not found");
        assert_eq!(sibling.dimensions.width, 700.0,
            "sibling block should be narrowed to 700px by the 100px left float, got {}",
            sibling.dimensions.width);
    }

    // ── Intrinsic sizing tests ────────────────────────────────────────────────

    /// Helper: navigate into the layout tree and find the first element whose
    /// local tag name matches `tag`.
    fn find_element_by_tag<'a>(b: &'a LayoutBox<'a>, tag: &str) -> Option<&'a LayoutBox<'a>> {
        if let NodeData::Element { ref name, .. } = b.style_node.node.data {
            if name.local.to_string() == tag { return Some(b); }
        }
        for c in &b.children {
            if let Some(found) = find_element_by_tag(c, tag) { return Some(found); }
        }
        None
    }

    /// `parse_value("fit-content(200px)")` must return `Value::FitContent(200.0)`.
    #[test]
    fn test_css_fit_content_parse() {
        let v = css::parse_value("fit-content(200px)");
        assert_eq!(v, css::Value::FitContent(200.0));
    }

    /// `parse_value("min-content")` must return a Keyword.
    #[test]
    fn test_css_min_max_content_parse() {
        assert_eq!(css::parse_value("min-content"), css::Value::Keyword(crate::css::intern("min-content")));
        assert_eq!(css::parse_value("max-content"), css::Value::Keyword(crate::css::intern("max-content")));
    }

    /// `compute_max_content_width` on "Hello World" must be wider than `compute_min_content_width`.
    #[test]
    fn test_intrinsic_width_ordering() {
        let html = r#"<span>Hello World</span>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom_tree.document, &ss, None, &HashMap::new(), None, None, None);

        // Locate the <span> StyledNode
        fn find_span_node<'a>(sn: &'a crate::style::StyledNode) -> Option<&'a crate::style::StyledNode> {
            if let NodeData::Element { ref name, .. } = sn.node.data {
                if name.local.to_string() == "span" { return Some(sn); }
            }
            for c in &sn.children { if let Some(f) = find_span_node(c) { return Some(f); } }
            None
        }
        let span_node = find_span_node(&style_tree).expect("span not found");

        let min_c = compute_min_content_width(span_node, 800.0, 600.0);
        let max_c = compute_max_content_width(span_node, 800.0, 600.0);

        assert!(min_c > 0.0, "min-content must be > 0, got {min_c}");
        assert!(max_c > min_c,
            "max-content ({max_c}) must be wider than min-content ({min_c}) for multi-word text");
    }

    /// `width: min-content` — the div must not span the full 800 px container.
    #[test]
    fn test_width_min_content_layout() {
        let html = r#"<div style="width: min-content;">Hello World</div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom_tree.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();

        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!(div.dimensions.width > 0.0, "div width must be > 0");
        assert!(div.dimensions.width < 800.0,
            "div with width:min-content must be < 800px (container), got {}",
            div.dimensions.width);
    }

    /// `width: max-content` — the div must be wider than a min-content div.
    #[test]
    fn test_width_max_content_layout() {
        // min-content case
        let dom_min = dom::parse_html(r#"<div style="width: min-content;">Hello World</div>"#);
        let ss_min = css::parse_css("");
        let st_min = style::build_style_tree(&dom_min.document, &ss_min, None, &HashMap::new(), None, None, None);
        let (lo_min, _, _) = build_layout_tree(&st_min, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout_min = lo_min.unwrap();
        let div_min_w = find_element_by_tag(&layout_min, "div").unwrap().dimensions.width;

        // max-content case
        let dom_max = dom::parse_html(r#"<div style="width: max-content;">Hello World</div>"#);
        let ss_max = css::parse_css("");
        let st_max = style::build_style_tree(&dom_max.document, &ss_max, None, &HashMap::new(), None, None, None);
        let (lo_max, _, _) = build_layout_tree(&st_max, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout_max = lo_max.unwrap();
        let div_max_w = find_element_by_tag(&layout_max, "div").unwrap().dimensions.width;

        assert!(div_max_w >= div_min_w,
            "max-content width ({div_max_w}) must be >= min-content width ({div_min_w})");
        assert!(div_max_w < 800.0,
            "max-content width ({div_max_w}) must be < container (800px) for short text");
    }

    /// `width: fit-content(150px)` — clamps to at most 150 px.
    #[test]
    fn test_fit_content_with_limit() {
        // "Hello World" max-content is well under 800px but we clamp to 150px
        let html = r#"<div style="width: fit-content(150px);">Hello World this is some longer text for the test</div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom_tree.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!(div.dimensions.width <= 150.0,
            "fit-content(150px) must be <= 150px, got {}", div.dimensions.width);
        assert!(div.dimensions.width > 0.0,
            "fit-content(150px) must be > 0, got {}", div.dimensions.width);
    }

    /// `width: fit-content` (no argument) — shrinks to content but stays <= container.
    #[test]
    fn test_fit_content_no_arg() {
        let html = r#"<div style="width: fit-content;">Hello</div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom_tree.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!(div.dimensions.width > 0.0, "fit-content width must be > 0");
        assert!(div.dimensions.width <= 800.0, "fit-content width must be <= container (800px)");
    }

    // ── get_opacity tests ────────────────────────────────────────────────────

    #[test]
    fn test_get_opacity_default() {
        // An element with no opacity style should return 1.0
        let html = r#"<div style="width:100px;height:50px;">Content</div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!((div.get_opacity() - 1.0).abs() < f32::EPSILON, "default opacity must be 1.0");
    }

    #[test]
    fn test_get_opacity_value() {
        // An element with opacity:0.5 should return 0.5
        let html = r#"<div style="width:100px;height:50px;opacity:0.5;">Content</div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!((div.get_opacity() - 0.5).abs() < 0.01, "opacity must be 0.5, got {}", div.get_opacity());
    }

    // ── Deep-nesting / stack-overflow regression tests ───────────────────────

    /// 5000 nested <div> elements must not cause a stack overflow.
    ///
    /// This exercises every iterative conversion: flatten_dom (style.rs),
    /// build_final_tree (style.rs), build_layout_tree / perform_layout
    /// (layout.rs via stacker::maybe_grow), and all collect_* methods.
    #[test]
    fn test_deep_nesting_no_stack_overflow() {
        // Build 5000 nested divs: <div><div>...<div>leaf</div>...</div></div>
        let depth = 5000usize;
        let mut html = String::with_capacity(depth * 12);
        for _ in 0..depth { html.push_str("<div>"); }
        html.push_str("leaf");
        for _ in 0..depth { html.push_str("</div>"); }

        let dom = dom::parse_html(&html);
        let ss = css::parse_css("");
        // build_style_tree calls flatten_dom and build_final_tree — both iterative.
        let style_tree = style::build_style_tree(
            &dom.document, &ss, None, &HashMap::new(), None, None, None,
        );

        // build_layout_tree calls perform_layout which recurses via stacker::maybe_grow.
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree should be built for 5000 nested divs");

        // Exercise every iterative collect_* path.
        let mut links: Vec<(Rect, String)> = Vec::new();
        layout.collect_links(&mut links);

        let mut handlers: Vec<(Rect, String)> = Vec::new();
        layout.collect_event_handlers(&mut handlers);

        let mut images: Vec<(Rect, String)> = Vec::new();
        layout.collect_images(&mut images);

        let mut ids: Vec<(Rect, String)> = Vec::new();
        layout.collect_element_ids(&mut ids);

        let mut focusables: Vec<(Rect, String)> = Vec::new();
        layout.collect_focusable_elements(&mut focusables);

        // offset_layout_box is iterative — apply a trivial shift to exercise it.
        let mut owned = layout;
        offset_layout_box(&mut owned, 1.0, 1.0);

        // print_layout_tree is iterative — just call it to ensure it doesn't overflow.
        // Redirect output: in tests `print_layout_tree` uses println! so output goes to stdout.
        // We only verify it doesn't panic.
        // (Cannot suppress stdout in stable Rust without extra crates, but it's acceptable.)
    }

    /// 5000 nested divs with alternating inline-block display — exercises the
    /// mixed-display paths in compute_max/min_content_width and perform_layout.
    #[test]
    fn test_deep_nesting_mixed_display_no_stack_overflow() {
        let mut html = String::with_capacity(5000 * 40);
        for i in 0..5000 {
            if i % 2 == 0 {
                html.push_str(r#"<div style="display:inline-block;">"#);
            } else {
                html.push_str("<div>");
            }
        }
        html.push_str("x");
        for _ in 0..5000 { html.push_str("</div>"); }

        let dom = dom::parse_html(&html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document, &ss, None, &HashMap::new(), None, None, None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        assert!(layout_opt.is_some(), "layout must succeed for 5000 mixed-display nested divs");
    }

    // ── Image rendering tests ─────────────────────────────────────────────────

    fn find_image_box<'a>(lb: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
        if lb.display == DisplayType::Image { return Some(lb); }
        for c in &lb.children {
            if let Some(r) = find_image_box(c) { return Some(r); }
        }
        None
    }

    #[test]
    fn test_image_alt_text_stored() {
        let html = r#"<img src="x.png" alt="hello">"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style = style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree must be built");
        let img = find_image_box(&layout).expect("img node must be found");
        assert_eq!(img.alt_text, Some("hello".to_string()), "alt attribute must be stored as alt_text");
    }

    #[test]
    fn test_image_fallback_height() {
        // Only width is specified — height must be derived as a non-zero placeholder.
        let html = r#"<img src="x.png" style="width:200px">"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style = style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree must be built");
        let img = find_image_box(&layout).expect("img node must be found");
        assert!(img.dimensions.height > 0.0,
            "image with only width specified must have non-zero height, got {}", img.dimensions.height);
    }

    #[test]
    fn test_image_no_dimensions_gets_default() {
        // No width or height — must fall back to ~150px default.
        let html = r#"<img src="x.png">"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style = style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree must be built");
        let img = find_image_box(&layout).expect("img node must be found");
        assert!(img.dimensions.width > 0.0, "image with no dimensions must have non-zero width");
        assert!(img.dimensions.height > 0.0, "image with no dimensions must have non-zero height");
    }
}
