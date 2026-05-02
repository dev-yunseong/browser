use crate::css::{Unit, Value};
use crate::style::StyledNode;
use ab_glyph::{Font, FontRef, PxScale};
use markup5ever_rcdom::NodeData;
use std::collections::HashMap;
extern crate stacker;

// ── Intrinsic sizing helpers ──────────────────────────────────────────────────

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct IntrinsicSizeKey {
    node: *const StyledNode,
    vw: u32,
    vh: u32,
}

struct IntrinsicSizeCache {
    max_content: HashMap<IntrinsicSizeKey, f32>,
    min_content: HashMap<IntrinsicSizeKey, f32>,
}

impl IntrinsicSizeCache {
    fn new() -> Self {
        Self {
            max_content: HashMap::new(),
            min_content: HashMap::new(),
        }
    }

    fn key(sn: &StyledNode, vw: f32, vh: f32) -> IntrinsicSizeKey {
        IntrinsicSizeKey {
            node: sn as *const StyledNode,
            vw: vw.to_bits(),
            vh: vh.to_bits(),
        }
    }

    fn max_content_width(&mut self, sn: &StyledNode, vw: f32, vh: f32) -> f32 {
        enum Frame<'a> {
            Pre(&'a StyledNode),
            Post {
                node: *const StyledNode,
                key: IntrinsicSizeKey,
                num_children: usize,
                pad_border: f32,
            },
        }

        let mut work: Vec<Frame> = vec![Frame::Pre(sn)];
        let mut val_stack: Vec<f32> = Vec::new();

        while let Some(frame) = work.pop() {
            match frame {
                Frame::Pre(node) => {
                    let key = Self::key(node, vw, vh);
                    if let Some(width) = self.max_content.get(&key) {
                        val_stack.push(*width);
                        continue;
                    }

                    let width = if is_none_display(node) {
                        Some(0.0)
                    } else if let NodeData::Text { ref contents } = node.node.data {
                        let font_size =
                            match node.specified_values.get(&crate::css::intern("font-size")) {
                                Some(Value::Length(v, Unit::Px)) => *v,
                                _ => 16.0,
                            };
                        Some(measure_text_width(
                            &contents.borrow(),
                            font_size,
                            f32::INFINITY,
                        ))
                    } else {
                        let disp = get_display_type(node);
                        if disp == DisplayType::Image {
                            let w = read_px_direct(node, "width");
                            Some(if w > 0.0 { w } else { 100.0 })
                        } else if should_skip(node) {
                            Some(0.0)
                        } else if let Some(Value::Length(v, Unit::Px)) =
                            node.specified_values.get(&crate::css::intern("width"))
                        {
                            Some(*v)
                        } else {
                            None
                        }
                    };

                    if let Some(width) = width {
                        self.max_content.insert(key, width);
                        val_stack.push(width);
                        continue;
                    }

                    let pad_border = horiz_padding_border(node);
                    let non_skip: Vec<&StyledNode> =
                        node.children.iter().filter(|c| !should_skip(c)).collect();
                    let num_children = non_skip.len();

                    work.push(Frame::Post {
                        node: node as *const StyledNode,
                        key,
                        num_children,
                        pad_border,
                    });
                    for child in non_skip.into_iter().rev() {
                        work.push(Frame::Pre(child));
                    }
                }
                Frame::Post {
                    node,
                    key,
                    num_children,
                    pad_border,
                } => {
                    let node_ref = unsafe { &*node };
                    let non_skip_children: Vec<&StyledNode> = node_ref
                        .children
                        .iter()
                        .filter(|c| !should_skip(c))
                        .collect();
                    let start = val_stack.len().saturating_sub(num_children);
                    let child_vals: Vec<f32> = val_stack.drain(start..).collect();

                    let mut inline_run_width: f32 = 0.0;
                    let mut max_w: f32 = 0.0;
                    let mut float_width: f32 = 0.0;
                    let mut percent_width_sum: f32 = 0.0;
                    for (child_val, child) in child_vals.into_iter().zip(non_skip_children.iter()) {
                        if is_line_break_element(child) {
                            max_w = max_w.max(inline_run_width);
                            inline_run_width = 0.0;
                            continue;
                        }
                        if get_display_type(node_ref) == DisplayType::TableRow {
                            if let Some(percent) = specified_width_percent(child) {
                                percent_width_sum += percent;
                                continue;
                            }
                        }
                        let child_total = child_val + horiz_margin(child);
                        if get_float(child).is_some() {
                            float_width += child_total;
                            continue;
                        }
                        let child_disp = get_display_type(child);
                        if is_block_level(child_disp) {
                            max_w = max_w.max(inline_run_width);
                            inline_run_width = 0.0;
                            max_w = max_w.max(child_total);
                        } else {
                            inline_run_width += child_total;
                        }
                    }
                    max_w = max_w.max(inline_run_width);
                    let fixed_width = max_w + float_width;
                    let width = if get_display_type(node_ref) == DisplayType::TableRow
                        && percent_width_sum > 0.0
                        && percent_width_sum < 100.0
                    {
                        fixed_width / (1.0 - percent_width_sum / 100.0) + pad_border
                    } else {
                        fixed_width + pad_border
                    };
                    self.max_content.insert(key, width);
                    val_stack.push(width);
                }
            }
        }

        val_stack.pop().unwrap_or(0.0)
    }

    fn min_content_width(&mut self, sn: &StyledNode, vw: f32, vh: f32) -> f32 {
        enum Frame<'a> {
            Pre(&'a StyledNode),
            Post {
                key: IntrinsicSizeKey,
                num_children: usize,
                pad_border: f32,
            },
        }

        let mut work: Vec<Frame> = vec![Frame::Pre(sn)];
        let mut val_stack: Vec<f32> = Vec::new();

        while let Some(frame) = work.pop() {
            match frame {
                Frame::Pre(node) => {
                    let key = Self::key(node, vw, vh);
                    if let Some(width) = self.min_content.get(&key) {
                        val_stack.push(*width);
                        continue;
                    }

                    let width = if is_none_display(node) {
                        Some(0.0)
                    } else if let NodeData::Text { ref contents } = node.node.data {
                        let font_size =
                            match node.specified_values.get(&crate::css::intern("font-size")) {
                                Some(Value::Length(v, Unit::Px)) => *v,
                                _ => 16.0,
                            };
                        let text = contents.borrow();
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            Some(0.0)
                        } else {
                            Some(
                                trimmed
                                    .split_whitespace()
                                    .map(|word| measure_text_width(word, font_size, f32::INFINITY))
                                    .fold(0.0f32, f32::max),
                            )
                        }
                    } else {
                        let disp = get_display_type(node);
                        if disp == DisplayType::Image {
                            let w = read_px_direct(node, "width");
                            Some(if w > 0.0 { w } else { 100.0 })
                        } else if should_skip(node) {
                            Some(0.0)
                        } else if let Some(Value::Length(v, Unit::Px)) =
                            node.specified_values.get(&crate::css::intern("width"))
                        {
                            Some(*v)
                        } else {
                            None
                        }
                    };

                    if let Some(width) = width {
                        self.min_content.insert(key, width);
                        val_stack.push(width);
                        continue;
                    }

                    let pad_border = horiz_padding_border(node);
                    let non_skip: Vec<&StyledNode> =
                        node.children.iter().filter(|c| !should_skip(c)).collect();
                    let num_children = non_skip.len();

                    work.push(Frame::Post {
                        key,
                        num_children,
                        pad_border,
                    });
                    for child in non_skip.into_iter().rev() {
                        work.push(Frame::Pre(child));
                    }
                }
                Frame::Post {
                    key,
                    num_children,
                    pad_border,
                } => {
                    let start = val_stack.len().saturating_sub(num_children);
                    let child_vals = val_stack.drain(start..);
                    let width = child_vals.fold(0.0f32, f32::max) + pad_border;
                    self.min_content.insert(key, width);
                    val_stack.push(width);
                }
            }
        }

        val_stack.pop().unwrap_or(0.0)
    }
}

/// Measure the width of `text` rendered at `font_size` px.
/// When `wrap_width` is `f32::INFINITY`, no wrapping occurs (max-content).
/// When finite, line-breaks at word boundaries (min-content: longest word).
fn measure_text_width(text: &str, font_size: f32, wrap_width: f32) -> f32 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
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
        if line_w > 0.0 {
            line_w += space_w;
        }
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

fn horiz_margin(sn: &StyledNode) -> f32 {
    read_px_direct(sn, "margin-left") + read_px_direct(sn, "margin-right")
}

fn border_box_width(cb: &LayoutBox<'_>) -> f32 {
    cb.dimensions.width + cb.padding.left + cb.padding.right + cb.border.left + cb.border.right
}

fn border_box_height(cb: &LayoutBox<'_>) -> f32 {
    cb.dimensions.height + cb.padding.top + cb.padding.bottom + cb.border.top + cb.border.bottom
}

fn margin_box_width(cb: &LayoutBox<'_>) -> f32 {
    border_box_width(cb) + cb.margin.left + cb.margin.right
}

fn margin_box_height(cb: &LayoutBox<'_>) -> f32 {
    border_box_height(cb) + cb.margin.top + cb.margin.bottom
}

/// Compute the **max-content** width of a `StyledNode` subtree.
///
/// - Text nodes: total width with no line wrapping.
/// - Images: explicit `width` attribute/style, or 100 px default.
/// - `display: none`: 0.
/// - Block elements: max over children's max-content widths.
/// - Inline/inline-block elements: sum of children's max-content widths on one line.
pub fn compute_max_content_width(sn: &StyledNode, vw: f32, vh: f32) -> f32 {
    let mut cache = IntrinsicSizeCache::new();
    cache.max_content_width(sn, vw, vh)
}

/// Compute the **min-content** width of a `StyledNode` subtree.
///
/// - Text nodes: width of the longest single unbreakable word.
/// - Images: explicit `width` attribute/style, or 100 px default.
/// - `display: none`: 0.
/// - All elements: max over children's min-content widths (wrapping can isolate any child).
pub fn compute_min_content_width(sn: &StyledNode, vw: f32, vh: f32) -> f32 {
    let mut cache = IntrinsicSizeCache::new();
    cache.min_content_width(sn, vw, vh)
}

fn is_shrink_wrap(d: DisplayType) -> bool {
    matches!(
        d,
        DisplayType::InlineBlock
            | DisplayType::Table
            | DisplayType::TableCell
            | DisplayType::Image
            // Form controls without an explicit CSS width shrink-wrap to content.
            // Buttons in particular must size to their label text.
            | DisplayType::Input
    )
}

// ── Float layout types ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum FloatSide {
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ClearValue {
    Left,
    Right,
    Both,
}

#[derive(Clone, Debug)]
struct FloatArea {
    y: f32,
    height: f32,
    width: f32,
    side: FloatSide,
}

struct FloatContext {
    areas: Vec<FloatArea>,
    container_width: f32,
}

impl FloatContext {
    fn new(container_width: f32) -> Self {
        FloatContext {
            areas: vec![],
            container_width,
        }
    }
    /// Returns (avail_width, left_indent) for a horizontal band at y..y+max(h,1).
    fn available_at(&self, y: f32, h: f32) -> (f32, f32) {
        let band = h.max(1.0);
        let mut left_w = 0.0f32;
        let mut right_w = 0.0f32;
        for fa in &self.areas {
            if fa.y < y + band && fa.y + fa.height > y {
                match fa.side {
                    FloatSide::Left => left_w += fa.width,
                    FloatSide::Right => right_w += fa.width,
                }
            }
        }
        let avail = (self.container_width - left_w - right_w).max(0.0);
        (avail, left_w)
    }
    /// Minimum y to be completely clear of floats on the given side.
    fn clear_y(&self, cv: ClearValue) -> f32 {
        self.areas
            .iter()
            .filter(|fa| match cv {
                ClearValue::Left => fa.side == FloatSide::Left,
                ClearValue::Right => fa.side == FloatSide::Right,
                ClearValue::Both => true,
            })
            .map(|fa| fa.y + fa.height)
            .fold(0.0f32, f32::max)
    }
    /// Bottom edge of the lowest registered float.
    fn bottom(&self) -> f32 {
        self.areas
            .iter()
            .map(|fa| fa.y + fa.height)
            .fold(0.0f32, f32::max)
    }
    fn add(&mut self, area: FloatArea) {
        self.areas.push(area);
    }
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
    /// Label text for button/submit/reset input elements.
    /// Sourced from the `value` attribute of `<input type="submit|button|reset">`.
    /// Rendered centered inside the button rect by the paint pass.
    pub input_label: Option<String>,
    pub event_handlers: HashMap<String, String>,
    pub display: DisplayType,
    pub z_index: i32,
    pub position: PositionType,
    /// Marker text for list items (e.g. "•" for disc, "1." for decimal).
    /// `None` when `list-style-type: none` or the element is not a list item.
    pub list_marker: Option<String>,
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
            Post {
                num_children: usize,
                partial: LayoutBox<'f>,
            },
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
                        input_label: src.input_label.clone(),
                        event_handlers: src.event_handlers.clone(),
                        display: src.display,
                        z_index: src.z_index,
                        position: src.position,
                        list_marker: src.list_marker.clone(),
                    };
                    let num_children = src.children.len();
                    // Push Post first so it is processed after all children.
                    work.push(Frame::Post {
                        num_children,
                        partial,
                    });
                    // Push children in reverse so the first child is popped first.
                    for child in src.children.iter().rev() {
                        work.push(Frame::Pre(child as *const LayoutBox<'a>));
                    }
                }
                Frame::Post {
                    num_children,
                    mut partial,
                } => {
                    // Drain the last num_children cloned nodes from result_stack.
                    let start = result_stack.len().saturating_sub(num_children);
                    partial.children = result_stack.drain(start..).collect();
                    result_stack.push(partial);
                }
            }
        }

        result_stack
            .pop()
            .expect("LayoutBox::clone: result stack must have exactly one element")
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

/// CSS `position` property values.
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum PositionType {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
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
    Grid,
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
    let mut intrinsic_cache = IntrinsicSizeCache::new();
    build_layout_tree_with_cb_cached(
        style_node,
        container_start_x,
        current_x,
        current_y,
        container_width,
        vw,
        vh,
        None,
        &mut intrinsic_cache,
    )
}

/// Internal variant that threads the nearest positioned ancestor rect (containing block)
/// for absolute/fixed positioning resolution.
///
/// `containing_block`: `Some(rect)` = nearest `position: relative/absolute/fixed` ancestor's
/// padding-box rect. `None` = use the initial containing block (viewport at 0,0,vw×vh).
pub fn build_layout_tree_with_cb<'a>(
    style_node: &'a StyledNode,
    container_start_x: f32,
    current_x: f32,
    current_y: f32,
    container_width: f32,
    vw: f32,
    vh: f32,
    containing_block: Option<Rect>,
) -> (Option<LayoutBox<'a>>, f32, f32) {
    let mut intrinsic_cache = IntrinsicSizeCache::new();
    build_layout_tree_with_cb_cached(
        style_node,
        container_start_x,
        current_x,
        current_y,
        container_width,
        vw,
        vh,
        containing_block,
        &mut intrinsic_cache,
    )
}

fn build_layout_tree_with_cb_cached<'a>(
    style_node: &'a StyledNode,
    container_start_x: f32,
    current_x: f32,
    current_y: f32,
    container_width: f32,
    vw: f32,
    vh: f32,
    containing_block: Option<Rect>,
    intrinsic_cache: &mut IntrinsicSizeCache,
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
        layout.perform_layout(
            container_start_x,
            current_x,
            current_y,
            container_width,
            vw,
            vh,
            containing_block,
            intrinsic_cache,
        )
    })
}

impl<'a> LayoutBox<'a> {
    fn new(style_node: &'a StyledNode) -> Self {
        let display = get_display_type(style_node);
        let z_index = match style_node
            .specified_values
            .get(&crate::css::intern("z-index"))
        {
            Some(Value::Number(n)) => *n as i32,
            _ => 0,
        };
        let position = get_position_type(style_node);
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
            input_label: None,
            event_handlers: HashMap::new(),
            display,
            z_index,
            position,
            list_marker: None,
        };

        if let NodeData::Element {
            ref attrs,
            ref name,
            ..
        } = style_node.node.data
        {
            let tag = name.local.to_string();
            let mut input_type = String::new();
            let mut input_value: Option<String> = None;
            for attr in attrs.borrow().iter() {
                let name = attr.name.local.to_string();
                let value = attr.value.to_string();
                match name.as_str() {
                    "href" if tag == "a" => layout.link_url = Some(value),
                    "src" if tag == "img" => layout.image_url = Some(value),
                    "alt" if tag == "img" => layout.alt_text = Some(value),
                    "onclick" => {
                        layout.event_handlers.insert("click".to_string(), value);
                    }
                    "type" if tag == "input" => input_type = value.to_ascii_lowercase(),
                    "value" if tag == "input" => input_value = Some(value),
                    _ => {}
                }
            }
            // Populate input_label for button-like input elements.
            // <input type="submit"> defaults to "Submit" if no value attribute is present.
            // <input type="button"> and <input type="reset"> use value or a blank label.
            if tag == "input" && matches!(input_type.as_str(), "submit" | "button" | "reset") {
                layout.input_label = Some(match input_value {
                    Some(v) => v,
                    None => match input_type.as_str() {
                        "submit" => "Submit".to_string(),
                        "reset"  => "Reset".to_string(),
                        _        => String::new(),
                    },
                });
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
            _ => {
                if self.display == DisplayType::Input {
                    1.0
                } else {
                    0.0
                }
            }
        };
        self.border = EdgeSizes {
            left: b_width,
            right: b_width,
            top: b_width,
            bottom: b_width,
        };
    }

    fn perform_layout(
        mut self,
        container_start_x: f32,
        initial_x: f32,
        mut current_y: f32,
        container_width: f32,
        vw: f32,
        vh: f32,
        containing_block: Option<Rect>,
        intrinsic_cache: &mut IntrinsicSizeCache,
    ) -> (Option<LayoutBox<'a>>, f32, f32) {
        let is_block = is_block_level(self.display);

        // Block formatting context or similar check
        if is_block && initial_x > container_start_x {
            current_y += 5.0; // Break line before block
        }

        let is_floated = get_float(self.style_node).is_some();
        let specified_width = self
            .style_node
            .specified_values
            .get(&crate::css::intern("width"));
        let auto_width = specified_width.is_none();

        let mut width = match specified_width {
            Some(Value::Length(v, Unit::Px)) => *v,
            Some(Value::Length(v, Unit::Percent)) => container_width * (v / 100.0),
            Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
            Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
            // CSS Intrinsic & Extrinsic Sizing Level 3
            Some(Value::Keyword(k)) if **k == *"min-content" => {
                intrinsic_cache.min_content_width(self.style_node, vw, vh)
            }
            Some(Value::Keyword(k)) if **k == *"max-content" => {
                intrinsic_cache.max_content_width(self.style_node, vw, vh)
            }
            Some(Value::Keyword(k)) if **k == *"fit-content" => {
                let max_c = intrinsic_cache.max_content_width(self.style_node, vw, vh);
                let min_c = intrinsic_cache.min_content_width(self.style_node, vw, vh);
                // fit-content without argument: min(max-content, max(min-content, available))
                max_c.min(container_width).max(min_c)
            }
            Some(Value::FitContent(limit)) => {
                let limit = *limit;
                let max_c = intrinsic_cache.max_content_width(self.style_node, vw, vh);
                let min_c = intrinsic_cache.min_content_width(self.style_node, vw, vh);
                // fit-content(N): min(max-content, max(min-content, min(available, N)))
                let available = container_width.min(limit);
                max_c.min(available).max(min_c)
            }
            _ => {
                if is_floated || is_shrink_wrap(self.display) {
                    let max_c = intrinsic_cache.max_content_width(self.style_node, vw, vh);
                    let min_c = intrinsic_cache.min_content_width(self.style_node, vw, vh);
                    // Auto-width floats use shrink-to-fit sizing instead of filling the line.
                    max_c.min(container_width).max(min_c)
                } else if is_block {
                    (container_width - self.margin.left - self.margin.right).max(0.0)
                } else {
                    0.0
                }
            }
        };

        let box_sizing = self
            .style_node
            .specified_values
            .get(&crate::css::intern("box-sizing"))
            .and_then(|v| {
                if let Value::Keyword(k) = v {
                    Some(&**k)
                } else {
                    None
                }
            })
            .unwrap_or("content-box");

        if box_sizing == "border-box" && width > 0.0 {
            width = (width
                - self.padding.left
                - self.padding.right
                - self.border.left
                - self.border.right)
                .max(0.0);
        }

        if let Some(Value::Length(v, Unit::Px)) = self
            .style_node
            .specified_values
            .get(&crate::css::intern("max-width"))
        {
            let max_w = if box_sizing == "border-box" {
                (*v - self.padding.left - self.padding.right - self.border.left - self.border.right)
                    .max(0.0)
            } else {
                *v
            };
            width = width.min(max_w);
        }
        if let Some(Value::Length(v, Unit::Px)) = self
            .style_node
            .specified_values
            .get(&crate::css::intern("min-width"))
        {
            let min_w = if box_sizing == "border-box" {
                (*v - self.padding.left - self.padding.right - self.border.left - self.border.right)
                    .max(0.0)
            } else {
                *v
            };
            width = width.max(min_w);
        }

        if is_block && width < container_width {
            let mut is_auto = false;
            for prop in ["margin", "margin-left", "margin-right"] {
                if let Some(Value::Keyword(s)) = self
                    .style_node
                    .specified_values
                    .get(&crate::css::intern(prop))
                {
                    if s.contains("auto") {
                        is_auto = true;
                        break;
                    }
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

        let height = match self
            .style_node
            .specified_values
            .get(&crate::css::intern("height"))
        {
            Some(Value::Length(v, Unit::Px)) => *v,
            Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
            Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
            // CSS Intrinsic & Extrinsic Sizing Level 3 — height axis
            // For block containers, min-content and max-content height are both
            // equivalent to the natural auto height (content-derived). Return 0.0
            // so the existing content-height calculation takes over.
            Some(Value::Keyword(k))
                if **k == *"min-content" || **k == *"max-content" || **k == *"fit-content" =>
            {
                0.0
            }
            Some(Value::FitContent(_)) => 0.0,
            _ => 0.0,
        };

        if let NodeData::Text { ref contents } = self.style_node.node.data {
            let available_width = if container_width.is_finite() {
                let consumed = (initial_x - container_start_x).max(0.0);
                (container_width - consumed).max(0.0)
            } else {
                container_width
            };
            return self.layout_text(
                contents.borrow().to_string(),
                container_start_x,
                initial_x,
                current_y,
                available_width,
            );
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
            let final_h = if height > 0.0 {
                height
            } else {
                self.dimensions.width * 0.667
            };
            self.dimensions.height = final_h.max(1.0);
            let final_x = self.dimensions.x + self.dimensions.width + self.margin.right;
            let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
            return (Some(self), final_x, final_y);
        }

        let inner_width = if width > 0.0 {
            width
        } else {
            (container_width
                - self.padding.left
                - self.padding.right
                - self.border.left
                - self.border.right)
                .max(0.0)
        };
        let mut child_y = self.dimensions.y + self.padding.top + self.border.top;
        let mut max_child_x = self.dimensions.x;

        // Containing-block computation for positioned descendants.
        // Must appear before Flex and the main layout loop so both can access child_cb.
        let self_establishes_cb = matches!(
            self.position,
            PositionType::Relative
                | PositionType::Absolute
                | PositionType::Fixed
                | PositionType::Sticky
        );
        let viewport_rect = Rect {
            x: 0.0,
            y: 0.0,
            width: vw,
            height: vh,
        };
        let self_cb_rect = Rect {
            x: self.dimensions.x + self.padding.left + self.border.left,
            y: self.dimensions.y + self.padding.top + self.border.top,
            width: (self.dimensions.width
                - self.padding.left
                - self.padding.right
                - self.border.left
                - self.border.right)
                .max(0.0),
            height: 0.0, // height not finalised yet
        };
        let child_cb = if self_establishes_cb {
            Some(self_cb_rect)
        } else {
            containing_block
        };

        if self.display == DisplayType::Flex {
            // ── Read flex container properties ────────────────────────────────
            let flex_direction = self
                .style_node
                .specified_values
                .get(&crate::css::intern("flex-direction"))
                .and_then(|v| {
                    if let Value::Keyword(k) = v {
                        Some(&**k)
                    } else {
                        None
                    }
                })
                .unwrap_or("row");
            let is_row = flex_direction == "row" || flex_direction == "row-reverse";
            let justify = self
                .style_node
                .specified_values
                .get(&crate::css::intern("justify-content"))
                .and_then(|v| {
                    if let Value::Keyword(k) = v {
                        Some(&**k)
                    } else {
                        None
                    }
                })
                .unwrap_or("flex-start");
            let align_items = self
                .style_node
                .specified_values
                .get(&crate::css::intern("align-items"))
                .and_then(|v| {
                    if let Value::Keyword(k) = v {
                        Some(&**k)
                    } else {
                        None
                    }
                })
                .unwrap_or("stretch");
            let flex_wrap = self
                .style_node
                .specified_values
                .get(&crate::css::intern("flex-wrap"))
                .and_then(|v| {
                    if let Value::Keyword(k) = v {
                        Some(&**k)
                    } else {
                        None
                    }
                })
                .unwrap_or("nowrap");
            let do_wrap = flex_wrap == "wrap" || flex_wrap == "wrap-reverse";

            // gap / row-gap / column-gap
            let col_gap = match self
                .style_node
                .specified_values
                .get(&crate::css::intern("column-gap"))
                .or_else(|| {
                    self.style_node
                        .specified_values
                        .get(&crate::css::intern("gap"))
                }) {
                Some(Value::Length(v, Unit::Px)) => *v,
                Some(Value::Number(v)) => *v,
                _ => 0.0,
            };
            let row_gap = match self
                .style_node
                .specified_values
                .get(&crate::css::intern("row-gap"))
                .or_else(|| {
                    self.style_node
                        .specified_values
                        .get(&crate::css::intern("gap"))
                }) {
                Some(Value::Length(v, Unit::Px)) => *v,
                Some(Value::Number(v)) => *v,
                _ => 0.0,
            };
            // For a row flex container the gap between items on the main axis is col_gap;
            // for a column container it is row_gap.
            let main_gap = if is_row { col_gap } else { row_gap };
            let cross_gap = if is_row { row_gap } else { col_gap };

            // ── Measure all flex children ─────────────────────────────────────
            // Each child is laid out at inner_width to get natural dimensions.
            // We store (LayoutBox, flex-grow, flex-shrink, align-self, order).
            struct FlexItem<'fi> {
                cb: LayoutBox<'fi>,
                grow: f32,
                shrink: f32,
                align_self: Option<&'fi str>,
                order: i32,
            }

            let mut raw_items: Vec<FlexItem<'_>> = Vec::new();
            // Absolute/fixed children are out-of-flow in flex containers too.
            let mut flex_positioned_entries: Vec<&StyledNode> = Vec::new();

            for child_node in &self.style_node.children {
                if should_skip(child_node) {
                    continue;
                }
                // Absolute and fixed children are out of flex flow — collect for
                // deferred positioning after the container size is finalized.
                let child_pos = get_position_type(child_node);
                if matches!(child_pos, PositionType::Absolute | PositionType::Fixed) {
                    flex_positioned_entries.push(child_node);
                    continue;
                }
                // For row flex containers, block-level items must not stretch to fill the
                // container width — per CSS spec, flex items use their "hypothetical main size"
                // which is their max-content width when no explicit width is set.  Passing
                // max_content_width as container_width causes the block sizing path
                // (`container_width - margins`) to produce the correct shrink-wrapped size.
                // Column flex containers still pass inner_width so block children stretch normally.
                let child_has_explicit_width = matches!(
                    child_node
                        .specified_values
                        .get(&crate::css::intern("width")),
                    Some(Value::Length(_, _))
                        | Some(Value::Keyword(_))
                        | Some(Value::FitContent(_))
                );
                let child_display = get_display_type(child_node);
                let flex_basis = child_node
                    .specified_values
                    .get(&crate::css::intern("flex-basis"))
                    .and_then(|v| {
                        if !is_row {
                            return None;
                        }
                        match v {
                            Value::Length(px, Unit::Px) => Some((*px).max(0.0)),
                            Value::Length(pct, Unit::Percent) => {
                                Some((inner_width * (*pct / 100.0)).max(0.0))
                            }
                            Value::Number(n) => Some((*n).max(0.0)),
                            Value::Keyword(k) if &**k == "auto" => None,
                            _ => None,
                        }
                    });
                let measure_width =
                    if let Some(basis) = flex_basis {
                        basis.min(inner_width).max(0.0)
                    } else if is_row && is_block_level(child_display) && !child_has_explicit_width {
                        // Shrink-wrap: use max-content so block items don't fill the flex container.
                        intrinsic_cache
                            .max_content_width(child_node, vw, vh)
                            .min(inner_width)
                            .max(0.0)
                    } else {
                        inner_width
                    };
                let (cb_opt, _, _) = build_layout_tree_with_cb_cached(
                    child_node,
                    0.0,
                    0.0,
                    0.0,
                    measure_width,
                    vw,
                    vh,
                    child_cb,
                    intrinsic_cache,
                );
                if let Some(mut cb) = cb_opt {
                    if let Some(basis) = flex_basis {
                        cb.dimensions.width = basis.min(inner_width).max(0.0);
                    }
                    if cb.dimensions.width > inner_width {
                        cb.dimensions.width = inner_width;
                    }
                    // Flex items that come out with width=0 (e.g. inline <a> elements) need
                    // an intrinsic size so they participate correctly in the flex algorithm.
                    // Use max-content width capped to inner_width as the shrink-wrap fallback.
                    if cb.dimensions.width == 0.0 {
                        let max_c = intrinsic_cache.max_content_width(child_node, vw, vh);
                        cb.dimensions.width = max_c.min(inner_width).max(0.0);
                    }
                    let grow = child_node
                        .specified_values
                        .get(&crate::css::intern("flex-grow"))
                        .and_then(|v| {
                            if let Value::Number(n) = v {
                                Some(*n)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0.0);
                    let shrink = child_node
                        .specified_values
                        .get(&crate::css::intern("flex-shrink"))
                        .and_then(|v| {
                            if let Value::Number(n) = v {
                                Some(*n)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(1.0);
                    // align-self: the keyword stored as a &str tied to the child's Arc<str> lifetime.
                    // We keep it as Option<&str> borrowed from the child_node's map.
                    let align_self: Option<&str> = child_node
                        .specified_values
                        .get(&crate::css::intern("align-self"))
                        .and_then(|v| {
                            if let Value::Keyword(k) = v {
                                Some(&**k)
                            } else {
                                None
                            }
                        });
                    let order = child_node
                        .specified_values
                        .get(&crate::css::intern("order"))
                        .and_then(|v| match v {
                            Value::Number(n) => Some(*n as i32),
                            _ => None,
                        })
                        .unwrap_or(0);
                    raw_items.push(FlexItem {
                        cb,
                        grow,
                        shrink,
                        align_self,
                        order,
                    });
                }
            }

            // Apply `order` sorting (stable sort preserves DOM order for ties).
            raw_items.sort_by_key(|item| item.order);

            // ── Helper: compute main/cross size of a laid-out box ─────────────
            let main_size = |cb: &LayoutBox<'_>| -> f32 {
                if is_row {
                    margin_box_width(cb)
                } else {
                    margin_box_height(cb)
                }
            };
            let cross_size = |cb: &LayoutBox<'_>| -> f32 {
                if is_row {
                    margin_box_height(cb)
                } else {
                    margin_box_width(cb)
                }
            };

            // ── Build flex lines (wrapping) ────────────────────────────────────
            // Each line is a Vec of indices into raw_items.
            //
            // For column flex containers with auto height (height == 0), the main axis
            // has no definite size.  Using 0.0001 caused flex-shrink to collapse all items
            // to zero height.  Instead, use f32::INFINITY so the deficit is always 0
            // (no shrinking) and the container grows to fit its items.  Row containers
            // always have a definite main size (inner_width from the block width).
            let main_container_size = if is_row {
                inner_width
            } else if height > 0.0 {
                height
            } else {
                f32::INFINITY // auto-height column: no shrinking, grow to content
            };
            let mut lines: Vec<Vec<usize>> = Vec::new();
            {
                let mut cur_line: Vec<usize> = Vec::new();
                let mut line_main: f32 = 0.0;
                for (i, item) in raw_items.iter().enumerate() {
                    let item_main = main_size(&item.cb);
                    let gap_contribution = if cur_line.is_empty() { 0.0 } else { main_gap };
                    if do_wrap
                        && !cur_line.is_empty()
                        && line_main + gap_contribution + item_main > main_container_size
                    {
                        lines.push(std::mem::take(&mut cur_line));
                        line_main = 0.0;
                    }
                    if !cur_line.is_empty() {
                        line_main += main_gap;
                    }
                    line_main += item_main;
                    cur_line.push(i);
                }
                if !cur_line.is_empty() {
                    lines.push(cur_line);
                }
            }

            // ── Lay out each line ─────────────────────────────────────────────
            let container_main_start = if is_row {
                self.dimensions.x + self.padding.left + self.border.left
            } else {
                self.dimensions.y + self.padding.top + self.border.top
            };
            let container_cross_start = if is_row {
                self.dimensions.y + self.padding.top + self.border.top
            } else {
                self.dimensions.x + self.padding.left + self.border.left
            };

            let mut cross_cursor = 0.0f32; // offset within the container's cross axis

            for line_indices in &lines {
                let initial_line_mains: Vec<f32> = line_indices
                    .iter()
                    .map(|&i| {
                        if is_row {
                            raw_items[i].cb.dimensions.width
                        } else {
                            raw_items[i].cb.dimensions.height
                        }
                    })
                    .collect();

                // Compute total main size + gaps for this line.
                let gaps_total = if line_indices.len() > 1 {
                    main_gap * (line_indices.len() - 1) as f32
                } else {
                    0.0
                };
                let line_total_main: f32 = line_indices
                    .iter()
                    .map(|&i| main_size(&raw_items[i].cb))
                    .sum::<f32>()
                    + gaps_total;
                // When main_container_size is INFINITY (auto-height column), there is no
                // definite container size: free space is 0 and deficit is 0.
                let free = if main_container_size.is_infinite() {
                    0.0
                } else {
                    (main_container_size - line_total_main).max(0.0)
                };
                let deficit = if main_container_size.is_infinite() {
                    0.0
                } else {
                    (line_total_main - main_container_size).max(0.0)
                };

                // Flex-grow distribution (only if there is free space).
                let total_grow: f32 = line_indices.iter().map(|&i| raw_items[i].grow).sum();
                if free > 0.0 && total_grow > 0.0 {
                    for &i in line_indices {
                        let share = (raw_items[i].grow / total_grow) * free;
                        if is_row {
                            raw_items[i].cb.dimensions.width += share;
                        } else {
                            raw_items[i].cb.dimensions.height += share;
                        }
                    }
                }

                // Flex-shrink distribution (only if items overflow).
                let total_shrink_weighted: f32 = line_indices
                    .iter()
                    .map(|&i| raw_items[i].shrink * main_size(&raw_items[i].cb))
                    .sum();
                if deficit > 0.0 && total_shrink_weighted > 0.0 {
                    for &i in line_indices {
                        let ms = main_size(&raw_items[i].cb);
                        let weight = raw_items[i].shrink * ms / total_shrink_weighted;
                        let reduction = weight * deficit;
                        if is_row {
                            raw_items[i].cb.dimensions.width =
                                (raw_items[i].cb.dimensions.width - reduction).max(0.0);
                        } else {
                            raw_items[i].cb.dimensions.height =
                                (raw_items[i].cb.dimensions.height - reduction).max(0.0);
                        }
                    }
                }

                // A flex item's descendants may depend on the resolved main-axis size
                // (for example, `width: 100%` inside a growing navbar-collapse item).
                // Re-layout items whose main size changed so percentage widths and
                // auto-width descendants are measured against the final flexed size.
                for (&i, &initial_main) in line_indices.iter().zip(initial_line_mains.iter()) {
                    let final_main = if is_row {
                        raw_items[i].cb.dimensions.width
                    } else {
                        raw_items[i].cb.dimensions.height
                    };
                    if (final_main - initial_main).abs() < 0.5 {
                        continue;
                    }
                    let (reflowed_opt, _, _) = build_layout_tree_with_cb_cached(
                        raw_items[i].cb.style_node,
                        0.0,
                        0.0,
                        0.0,
                        if is_row {
                            final_main.max(0.0)
                        } else {
                            inner_width
                        },
                        vw,
                        vh,
                        child_cb,
                        intrinsic_cache,
                    );
                    if let Some(mut reflowed) = reflowed_opt {
                        if is_row {
                            reflowed.dimensions.width = final_main.max(0.0);
                        } else {
                            reflowed.dimensions.height = final_main.max(0.0);
                        }
                        raw_items[i].cb = reflowed;
                    }
                }

                // Recompute totals after grow/shrink/reflow.
                let gaps_total2 = if line_indices.len() > 1 {
                    main_gap * (line_indices.len() - 1) as f32
                } else {
                    0.0
                };
                let line_total_main2: f32 = line_indices
                    .iter()
                    .map(|&i| main_size(&raw_items[i].cb))
                    .sum::<f32>()
                    + gaps_total2;
                // Free space is 0 when container has no definite size (INFINITY).
                let free2 = if main_container_size.is_infinite() {
                    0.0
                } else {
                    (main_container_size - line_total_main2).max(0.0)
                };
                let n = line_indices.len();

                // Compute main-axis starting cursor and per-item gap for justify-content.
                let (mut main_cursor, between_gap) = match justify {
                    "flex-end" => (free2, 0.0),
                    "center" => (free2 / 2.0, 0.0),
                    "space-between" => (0.0, if n > 1 { free2 / (n - 1) as f32 } else { 0.0 }),
                    "space-around" => {
                        let slot = free2 / n as f32;
                        (slot / 2.0, slot)
                    }
                    "space-evenly" => {
                        let slot = free2 / (n + 1) as f32;
                        (slot, slot)
                    }
                    _ => (0.0, 0.0), // flex-start
                };

                // Cross-axis size of this line (max of all items' cross sizes).
                let line_cross: f32 = line_indices
                    .iter()
                    .map(|&i| cross_size(&raw_items[i].cb))
                    .fold(0.0_f32, f32::max);

                // Place each item.
                for (idx_in_line, &i) in line_indices.iter().enumerate() {
                    let item = &mut raw_items[i];

                    // align-self overrides align-items for this item.
                    let effective_align = item.align_self.unwrap_or(align_items);

                    // Stretch cross axis if needed.
                    if effective_align == "stretch" {
                        if is_row {
                            item.cb.dimensions.height =
                                (line_cross - item.cb.margin.top - item.cb.margin.bottom).max(0.0);
                        } else {
                            item.cb.dimensions.width =
                                (line_cross - item.cb.margin.left - item.cb.margin.right).max(0.0);
                        }
                    }

                    // Cross-axis offset within the line.
                    let item_cross = cross_size(&item.cb);
                    let cross_offset = match effective_align {
                        "flex-end" => line_cross - item_cross,
                        "center" => (line_cross - item_cross) / 2.0,
                        "baseline" => 0.0, // simplified: treat like flex-start
                        _ => 0.0,          // flex-start / stretch (already resized)
                    };

                    // Add gap between items (not before the first item).
                    if idx_in_line > 0 {
                        main_cursor += main_gap + between_gap;
                    }

                    // Compute absolute position.
                    let (x, y) = if is_row {
                        (
                            container_main_start + main_cursor + item.cb.margin.left,
                            container_cross_start
                                + cross_cursor
                                + cross_offset
                                + item.cb.margin.top,
                        )
                    } else {
                        (
                            container_cross_start
                                + cross_cursor
                                + cross_offset
                                + item.cb.margin.left,
                            container_main_start + main_cursor + item.cb.margin.top,
                        )
                    };

                    let dx = x - item.cb.dimensions.x;
                    let dy = y - item.cb.dimensions.y;
                    offset_layout_box(&mut item.cb, dx, dy);

                    main_cursor += if is_row {
                        margin_box_width(&item.cb)
                    } else {
                        margin_box_height(&item.cb)
                    };

                    max_child_x = max_child_x.max(
                        item.cb.dimensions.x + border_box_width(&item.cb) + item.cb.margin.right,
                    );
                    child_y = child_y.max(
                        item.cb.dimensions.y + border_box_height(&item.cb) + item.cb.margin.bottom,
                    );
                }

                cross_cursor += line_cross + cross_gap;
            }

            // Move items from raw_items into self.children.
            for item in raw_items {
                self.children.push(item.cb);
            }

            // Finalize flex container size.
            // cross_cursor already accumulated line heights + cross_gaps; subtract the last
            // trailing cross_gap (we don't add one after the last line).
            let total_cross = if cross_cursor > 0.0 && lines.len() > 1 {
                cross_cursor - cross_gap
            } else {
                cross_cursor
            };
            // For column containers, derive the main-axis (height) from the actual child
            // positions rather than main_container_size (which is near-zero when height is auto).
            // Include padding.bottom + border.bottom, consistent with block layout (line ~1197).
            let column_main = if !is_row {
                let content_top = self.dimensions.y + self.padding.top + self.border.top;
                (child_y - content_top + self.padding.bottom + self.border.bottom).max(0.0)
            } else {
                0.0
            };
            if self.dimensions.width <= 0.0 || (is_floated && auto_width) {
                let derived = max_child_x - self.dimensions.x + self.padding.right + self.border.right;
                self.dimensions.width = if is_row {
                    if container_width.is_finite() {
                        derived.min(container_width)
                    } else {
                        derived
                    }
                } else {
                    total_cross
                };
            }
            if self.dimensions.height <= 0.0 || height <= 0.0 {
                self.dimensions.height = if is_row { total_cross } else { column_main };
            }

            // ── Layout absolutely/fixedly positioned children inside flex container ──
            // Same logic as the block layout path: position after container size is known.
            if !flex_positioned_entries.is_empty() {
                let final_self_cb = Rect {
                    x: self.dimensions.x + self.padding.left + self.border.left,
                    y: self.dimensions.y + self.padding.top + self.border.top,
                    width: (self.dimensions.width
                        - self.padding.left
                        - self.padding.right
                        - self.border.left
                        - self.border.right)
                        .max(0.0),
                    height: (self.dimensions.height
                        - self.padding.top
                        - self.padding.bottom
                        - self.border.top
                        - self.border.bottom)
                        .max(0.0),
                };

                for pos_node in flex_positioned_entries {
                    let child_pos_type = get_position_type(pos_node);
                    let cb_for_child = if child_pos_type == PositionType::Fixed {
                        viewport_rect
                    } else if self_establishes_cb {
                        final_self_cb
                    } else {
                        containing_block.unwrap_or(viewport_rect)
                    };

                    let left_off =
                        resolve_offset(pos_node, "left", cb_for_child.width, vw, vh);
                    let right_off =
                        resolve_offset(pos_node, "right", cb_for_child.width, vw, vh);
                    let top_off =
                        resolve_offset(pos_node, "top", cb_for_child.height, vw, vh);
                    let bottom_off =
                        resolve_offset(pos_node, "bottom", cb_for_child.height, vw, vh);

                    let child_explicit_width =
                        pos_node.specified_values.get(&crate::css::intern("width"));
                    let child_layout_width = match child_explicit_width {
                        Some(Value::Length(v, Unit::Px)) => *v,
                        Some(Value::Length(v, Unit::Percent)) => cb_for_child.width * (v / 100.0),
                        _ => {
                            if let (Some(l), Some(r)) = (left_off, right_off) {
                                (cb_for_child.width - l - r).max(0.0)
                            } else {
                                let max_c = intrinsic_cache.max_content_width(pos_node, vw, vh);
                                max_c.min(cb_for_child.width)
                            }
                        }
                    };

                    let (pc_opt, _, _) = build_layout_tree_with_cb_cached(
                        pos_node,
                        0.0,
                        0.0,
                        0.0,
                        child_layout_width.max(1.0),
                        vw,
                        vh,
                        Some(cb_for_child),
                        intrinsic_cache,
                    );
                    if let Some(mut pc) = pc_opt {
                        let target_x = match (left_off, right_off) {
                            (Some(l), _) => cb_for_child.x + l + pc.margin.left,
                            (None, Some(r)) => {
                                cb_for_child.x + cb_for_child.width
                                    - r
                                    - pc.dimensions.width
                                    - pc.margin.right
                            }
                            (None, None) => cb_for_child.x + pc.margin.left,
                        };
                        let target_y = match (top_off, bottom_off) {
                            (Some(t), _) => cb_for_child.y + t + pc.margin.top,
                            (None, Some(b)) => {
                                cb_for_child.y + cb_for_child.height
                                    - b
                                    - pc.dimensions.height
                                    - pc.margin.bottom
                            }
                            (None, None) => cb_for_child.y + pc.margin.top,
                        };

                        let dx = target_x - pc.dimensions.x;
                        let dy = target_y - pc.dimensions.y;
                        offset_layout_box(&mut pc, dx, dy);
                        self.children.push(pc);
                    }
                }
            }

            let final_x = if is_block {
                container_start_x
            } else {
                self.dimensions.x + self.dimensions.width + self.margin.right
            };
            let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
            return (Some(self), final_x, final_y);
        }

        if self.display == DisplayType::Grid {
            // ── Grid formatting context ───────────────────────────────────────
            //
            // Implements basic CSS Grid layout:
            //   1. Parse grid-template-columns / grid-template-rows track lists.
            //   2. Resolve track sizes: px tracks are fixed; fr tracks share
            //      remaining available space proportionally; auto tracks share
            //      the leftover fr space equally.
            //   3. Read column-gap / row-gap for spacing.
            //   4. Auto-place children left-to-right, top-to-bottom.
            //   5. Stretch each child to fill its cell (default grid behaviour).

            // Helper: read a track-list stored as a CSS keyword value.
            let read_track_list = |prop: &str| -> Vec<crate::css::Value> {
                match self.style_node.specified_values.get(&crate::css::intern(prop)) {
                    Some(Value::Keyword(k)) => crate::css::parse_track_list(k),
                    _ => Vec::new(),
                }
            };

            let col_tracks = read_track_list("grid-template-columns");
            let row_tracks = read_track_list("grid-template-rows");

            let col_gap = match self.style_node.specified_values.get(&crate::css::intern("column-gap")) {
                Some(Value::Length(v, Unit::Px)) => *v,
                Some(Value::Number(v)) => *v,
                _ => 0.0,
            };
            let row_gap = match self.style_node.specified_values.get(&crate::css::intern("row-gap")) {
                Some(Value::Length(v, Unit::Px)) => *v,
                Some(Value::Number(v)) => *v,
                _ => 0.0,
            };

            // Number of columns: from explicit template, or 1 (block fallback).
            let num_cols = if !col_tracks.is_empty() { col_tracks.len() } else { 1 };

            // Resolve column widths.
            let total_col_gaps = if num_cols > 1 { col_gap * (num_cols - 1) as f32 } else { 0.0 };
            let available_for_cols = (inner_width - total_col_gaps).max(0.0);

            // Sum of fixed (non-fr, non-auto) column widths.
            let col_fixed_total: f32 = col_tracks.iter().map(|t| match t {
                Value::Length(v, Unit::Px) => *v,
                Value::Length(v, Unit::Percent) => inner_width * (v / 100.0),
                _ => 0.0,
            }).sum();

            // Total fr units across all column tracks.
            let col_fr_total: f32 = col_tracks.iter().map(|t| match t {
                Value::Length(v, crate::css::Unit::Fr) => *v,
                _ => 0.0,
            }).sum();

            // Space remaining after fixed tracks — split among fr/auto tracks.
            let col_fr_space = (available_for_cols - col_fixed_total).max(0.0);

            // Count auto columns (share fr space equally if no fr tracks).
            let auto_count = col_tracks.iter().filter(|t| matches!(t, Value::Keyword(k) if k.as_ref() == "auto")).count() as f32;

            let col_widths: Vec<f32> = if col_tracks.is_empty() {
                vec![inner_width]
            } else {
                col_tracks.iter().map(|t| match t {
                    Value::Length(v, Unit::Px) => *v,
                    Value::Length(v, Unit::Percent) => inner_width * (v / 100.0),
                    Value::Length(v, crate::css::Unit::Fr) => {
                        if col_fr_total > 0.0 { (v / col_fr_total) * col_fr_space } else { 0.0 }
                    }
                    Value::Keyword(k) if k.as_ref() == "auto" => {
                        if auto_count > 0.0 { col_fr_space / auto_count } else { 0.0 }
                    }
                    _ => 0.0,
                }).collect()
            };

            // Collect in-flow children (skip positioned / display:none / whitespace-only text nodes).
            let mut grid_children: Vec<&StyledNode> = Vec::new();
            for child_node in &self.style_node.children {
                if should_skip(child_node) { continue; }
                // Skip whitespace-only text nodes — they are not grid items.
                if let NodeData::Text { ref contents } = child_node.node.data {
                    if contents.borrow().chars().all(|c| c.is_whitespace()) {
                        continue;
                    }
                }
                let child_pos = get_position_type(child_node);
                if matches!(child_pos, PositionType::Absolute | PositionType::Fixed) { continue; }
                grid_children.push(child_node);
            }

            let num_children = grid_children.len();
            let num_rows_needed = if num_children == 0 {
                0
            } else {
                (num_children + num_cols - 1) / num_cols
            };

            // Pass 1: lay out each child at its cell width to determine natural heights.
            struct GridItem<'gi> {
                cb: LayoutBox<'gi>,
                col: usize,
                row: usize,
            }

            let mut grid_items: Vec<GridItem<'_>> = Vec::new();
            for (child_idx, child_node) in grid_children.iter().enumerate() {
                let col = child_idx % num_cols;
                let row = child_idx / num_cols;
                let cell_width = col_widths.get(col).copied().unwrap_or(inner_width);

                let (cb_opt, _, _) = build_layout_tree_with_cb_cached(
                    child_node,
                    0.0, 0.0, 0.0,
                    cell_width.max(1.0),
                    vw, vh,
                    child_cb,
                    intrinsic_cache,
                );
                if let Some(cb) = cb_opt {
                    grid_items.push(GridItem { cb, col, row });
                }
            }

            // Pass 2: compute implicit row heights (max of all cells in each row).
            let mut row_heights: Vec<f32> = vec![0.0_f32; num_rows_needed];
            for item in &grid_items {
                if item.row < row_heights.len() {
                    let item_h = item.cb.dimensions.height
                        + item.cb.padding.top + item.cb.padding.bottom
                        + item.cb.border.top + item.cb.border.bottom
                        + item.cb.margin.top + item.cb.margin.bottom;
                    row_heights[item.row] = row_heights[item.row].max(item_h);
                }
            }

            // Apply explicit row track heights if provided.
            for (row_idx, row_h) in row_heights.iter_mut().enumerate() {
                if let Some(track) = row_tracks.get(row_idx) {
                    let explicit_h = match track {
                        Value::Length(v, Unit::Px) => Some(*v),
                        Value::Length(v, Unit::Percent) => Some(vh * (v / 100.0)),
                        _ => None,
                    };
                    if let Some(eh) = explicit_h {
                        *row_h = row_h.max(eh);
                    }
                }
            }

            // Pass 3: compute absolute row and column positions.
            let content_top = self.dimensions.y + self.padding.top + self.border.top;
            let content_left = self.dimensions.x + self.padding.left + self.border.left;

            let mut row_tops: Vec<f32> = Vec::with_capacity(num_rows_needed);
            {
                let mut cur_top = content_top;
                for (row_idx, &rh) in row_heights.iter().enumerate() {
                    row_tops.push(cur_top);
                    cur_top += rh;
                    if row_idx + 1 < num_rows_needed { cur_top += row_gap; }
                }
            }

            let mut col_lefts: Vec<f32> = Vec::with_capacity(num_cols);
            {
                let mut cur_left = content_left;
                for (col_idx, &cw) in col_widths.iter().enumerate() {
                    col_lefts.push(cur_left);
                    cur_left += cw;
                    if col_idx + 1 < num_cols { cur_left += col_gap; }
                }
            }

            // Pass 4: position and (optionally) stretch each grid item.
            for item in &mut grid_items {
                let cell_x = col_lefts.get(item.col).copied().unwrap_or(content_left);
                let cell_y = row_tops.get(item.row).copied().unwrap_or(content_top);
                let cell_w = col_widths.get(item.col).copied().unwrap_or(inner_width);
                let cell_h = row_heights.get(item.row).copied().unwrap_or(0.0);

                // Default alignment: stretch (item fills cell on both axes).
                let item_x = cell_x + item.cb.margin.left;
                let item_y = cell_y + item.cb.margin.top;

                let dx = item_x - item.cb.dimensions.x;
                let dy = item_y - item.cb.dimensions.y;
                offset_layout_box(&mut item.cb, dx, dy);

                // Stretch width to the cell's content width.
                item.cb.dimensions.width = (cell_w
                    - item.cb.margin.left - item.cb.margin.right
                    - item.cb.padding.left - item.cb.padding.right
                    - item.cb.border.left - item.cb.border.right
                ).max(0.0);

                // Stretch height only when no explicit height is set.
                let has_explicit_height = matches!(
                    item.cb.style_node.specified_values.get(&crate::css::intern("height")),
                    Some(Value::Length(v, Unit::Px)) if *v > 0.0
                );
                if !has_explicit_height {
                    let avail_cell_h = (cell_h
                        - item.cb.margin.top - item.cb.margin.bottom
                        - item.cb.padding.top - item.cb.padding.bottom
                        - item.cb.border.top - item.cb.border.bottom
                    ).max(0.0);
                    if avail_cell_h > item.cb.dimensions.height {
                        item.cb.dimensions.height = avail_cell_h;
                    }
                }

                max_child_x = max_child_x.max(
                    item.cb.dimensions.x + border_box_width(&item.cb) + item.cb.margin.right,
                );
                child_y = child_y.max(
                    item.cb.dimensions.y + border_box_height(&item.cb) + item.cb.margin.bottom,
                );
            }

            // Move items into self.children.
            for item in grid_items {
                self.children.push(item.cb);
            }

            // Finalize grid container height.
            let content_height = (child_y - content_top
                + self.padding.bottom + self.border.bottom).max(0.0);
            if self.dimensions.height <= 0.0 || height <= 0.0 {
                self.dimensions.height = content_height;
            }

            let final_x = container_start_x; // grid containers are block-level
            let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
            return (Some(self), final_x, final_y);
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

        enum ChildKind {
            Float(FloatSide),
            Block,
            Inline,
            LineBreak,
            Positioned,
        }
        struct ChildEntry<'entry> {
            node: &'entry StyledNode,
            kind: ChildKind,
            clear: Option<ClearValue>,
        }

        let mut entries: Vec<ChildEntry<'a>> = Vec::new();
        for child_node in &self.style_node.children {
            if should_skip(child_node) {
                continue;
            }
            if is_line_break_element(child_node) {
                entries.push(ChildEntry {
                    node: child_node,
                    kind: ChildKind::LineBreak,
                    clear: get_line_break_clear(child_node),
                });
                continue;
            }
            let child_pos = get_position_type(child_node);
            // Absolute and fixed children are removed from normal flow entirely.
            if matches!(child_pos, PositionType::Absolute | PositionType::Fixed) {
                entries.push(ChildEntry {
                    node: child_node,
                    kind: ChildKind::Positioned,
                    clear: None,
                });
                continue;
            }
            let float_side = get_float(child_node);
            let clear_val = get_clear(child_node);
            let child_disp = get_display_type(child_node);
            let kind = if let Some(side) = float_side {
                ChildKind::Float(side)
            } else if is_block_level(child_disp) {
                ChildKind::Block
            } else {
                ChildKind::Inline
            };
            entries.push(ChildEntry {
                node: child_node,
                kind,
                clear: clear_val,
            });
        }
        // Immutable borrow of self.style_node.children is now released.

        let container_x = self.dimensions.x + self.padding.left + self.border.left;
        let mut float_ctx = FloatContext::new(inner_width);
        let mut cursor_y = self.dimensions.y + self.padding.top + self.border.top;
        let mut prev_margin_bottom = 0.0f32;
        // True once the first block child has been placed (used for parent-child margin
        // collapsing: Case 2 of the CSS spec).
        let mut first_block_placed = false;
        // Whether the parent's top edge is "open" to margin collapsing (no border/padding
        // separating parent from its first block child).
        let parent_open_top = self.padding.top == 0.0 && self.border.top == 0.0;
        // Whether the parent's bottom edge is "open" to margin collapsing.
        let parent_open_bottom = self.padding.bottom == 0.0 && self.border.bottom == 0.0;
        let mut result: Vec<LayoutBox<'a>> = Vec::new();

        // Read text-align for this container (used by flush_line! to position inline lines).
        // Only block containers should align their inline contents. Applying inherited
        // `text-align` inside inline boxes like <a> makes short links behave like wide
        // centered containers, which breaks grouping in legacy centered footers.
        let text_align = self
            .style_node
            .specified_values
            .get(&crate::css::intern("text-align"))
            .and_then(|v| if let Value::Keyword(k) = v { Some(&**k as &str) } else { None })
            .unwrap_or("left")
            .to_string();
        let white_space = self
            .style_node
            .specified_values
            .get(&crate::css::intern("white-space"))
            .and_then(|v| if let Value::Keyword(k) = v { Some(&**k as &str) } else { None })
            .unwrap_or("normal");
        let no_wrap = white_space == "nowrap";
        let applies_text_align = matches!(
            self.display,
            DisplayType::Block | DisplayType::ListItem | DisplayType::Flex | DisplayType::InlineBlock | DisplayType::TableCell
        );

        // Inline line accumulator
        struct InlineLine<'a> {
            members: Vec<LayoutBox<'a>>,
            width: f32,
            height: f32,
        }
        let mut cur_line = InlineLine::<'a> {
            members: vec![],
            width: 0.0,
            height: 0.0,
        };
        let mut line_start_y = cursor_y;

        // Flush the current inline line into `result`, advancing cursor_y.
        // Applies text-align: center/right by shifting the line's starting x offset.
        macro_rules! flush_line {
            () => {
                if !cur_line.members.is_empty() {
                    let (avail_w, left_indent) =
                        float_ctx.available_at(line_start_y, cur_line.height.max(1.0));
                    // Compute text-align offset within the available width.
                    let align_offset = if applies_text_align {
                        match text_align.as_str() {
                            "center" => (avail_w - cur_line.width) / 2.0,
                            "right" => avail_w - cur_line.width,
                            _ => 0.0, // left / default
                        }
                    } else {
                        0.0
                    };
                    let mut lx = container_x + left_indent + align_offset;
                    for mut m in cur_line.members.drain(..) {
                        let dx = lx - (m.dimensions.x - m.margin.left);
                        let dy = cursor_y - (m.dimensions.y - m.margin.top);
                        offset_layout_box(&mut m, dx, dy);
                        max_child_x =
                            max_child_x.max(m.dimensions.x + border_box_width(&m) + m.margin.right);
                        lx += margin_box_width(&m);
                        result.push(m);
                    }
                    cursor_y += cur_line.height;
                    cur_line.width = 0.0;
                    cur_line.height = 0.0;
                    line_start_y = cursor_y;
                }
            };
        }

        let mut positioned_entries: Vec<&StyledNode> = Vec::new();

        // Counter for ordered list item markers (1., 2., …).
        // Only incremented when a ListItem child is placed in normal flow.
        let mut list_item_counter: u32 = 0;

        for entry in entries {
            match entry.kind {
                // ── Absolutely / fixedly positioned child — skip normal flow ──
                ChildKind::Positioned => {
                    // Collect for deferred layout after normal-flow finalisation.
                    positioned_entries.push(entry.node);
                }

                // ── Forced line break (`<br>`) ───────────────────────────────
                ChildKind::LineBreak => {
                    if let Some(cv) = entry.clear {
                        cursor_y = float_ctx.clear_y(cv).max(cursor_y);
                    }
                    let had_inline_content = !cur_line.members.is_empty();
                    let break_height = resolved_line_height_px(entry.node);
                    flush_line!();
                    if !had_inline_content {
                        cursor_y += break_height;
                    }
                    prev_margin_bottom = 0.0;
                    line_start_y = cursor_y;
                }

                // ── Float child ───────────────────────────────────────────────
                ChildKind::Float(side) => {
                    flush_line!();
                    // Build with origin (0,0); offset_layout_box will reposition.
                    // Use inner_width so explicit CSS widths resolve correctly.
                    let (cb_opt, _, _) = build_layout_tree_with_cb_cached(
                        entry.node,
                        0.0,
                        0.0,
                        0.0,
                        inner_width,
                        vw,
                        vh,
                        child_cb,
                        intrinsic_cache,
                    );
                    if let Some(mut cb) = cb_opt {
                        let float_w = margin_box_width(&cb);
                        let float_h = margin_box_height(&cb);
                        let (avail_w, left_indent) = float_ctx.available_at(cursor_y, float_h);
                        let fx = match side {
                            FloatSide::Left => container_x + left_indent,
                            FloatSide::Right => container_x + left_indent + avail_w - float_w,
                        };
                        let dx = fx - (cb.dimensions.x - cb.margin.left);
                        let dy = cursor_y - (cb.dimensions.y - cb.margin.top);
                        offset_layout_box(&mut cb, dx, dy);
                        float_ctx.add(FloatArea {
                            y: cursor_y,
                            height: float_h,
                            width: float_w,
                            side,
                        });
                        max_child_x = max_child_x
                            .max(cb.dimensions.x + border_box_width(&cb) + cb.margin.right);
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
                    let (cb_opt, _, _) = build_layout_tree_with_cb_cached(
                        entry.node,
                        block_x,
                        block_x,
                        0.0,
                        avail_w,
                        vw,
                        vh,
                        child_cb,
                        intrinsic_cache,
                    );
                    if let Some(mut cb) = cb_opt {
                        // Assign list marker for ListItem boxes.
                        if cb.display == DisplayType::ListItem {
                            list_item_counter += 1;
                            let style_type = cb
                                .style_node
                                .specified_values
                                .get(&crate::css::intern("list-style-type"))
                                .and_then(|v| {
                                    if let Value::Keyword(k) = v { Some(k.as_ref()) } else { None }
                                })
                                .unwrap_or("disc");
                            cb.list_marker = match style_type {
                                "none" => None,
                                "decimal" => Some(format!("{}.", list_item_counter)),
                                "circle" => Some("\u{25E6}".to_string()),  // ◦
                                "square" => Some("\u{25AA}".to_string()),  // ▪
                                _ => Some("\u{2022}".to_string()),         // • (disc)
                            };
                        }
                        // CSS margin collapsing (spec § 8.3.1):
                        //
                        // Case 1 — adjacent siblings: the bottom margin of the previous
                        // block and the top margin of this block collapse to max(prev, cur).
                        //
                        // Case 2 — parent / first-child: when no border or padding separates
                        // the parent's top edge from the first block child, the child's top
                        // margin collapses *into* the parent's top margin (no internal space).
                        let collapsed = if !first_block_placed && parent_open_top {
                            // Case 2: no space between parent content edge and first child.
                            // The child's margin has already been "consumed" by the parent's
                            // own margin; don't add it as interior spacing.
                            0.0
                        } else {
                            // Case 1: standard adjacent-sibling collapse.
                            prev_margin_bottom.max(cb.margin.top)
                        };
                        first_block_placed = true;
                        // `cb` was built at current_y = 0, so cb.dimensions.y == cb.margin.top.
                        // We want the content box to land at cursor_y + collapsed.
                        let dy = (cursor_y + collapsed) - cb.dimensions.y;
                        offset_layout_box(&mut cb, 0.0, dy);
                        // cursor_y advances using pre-offset (normal-flow) bottom edge.
                        let normal_flow_bottom = cb.dimensions.y + cb.dimensions.height;
                        // Apply relative offset AFTER computing normal-flow bottom so sibling
                        // placement is not affected (position:relative is a visual-only nudge).
                        if cb.position == PositionType::Relative {
                            apply_relative_offset(&mut cb, vw, vh);
                        }
                        cursor_y = normal_flow_bottom;
                        prev_margin_bottom = cb.margin.bottom;
                        max_child_x = max_child_x
                            .max(cb.dimensions.x + border_box_width(&cb) + cb.margin.right);
                        result.push(cb);
                    }
                    line_start_y = cursor_y; // keep line_start_y in sync after block advances cursor_y
                }

                // ── Inline child ──────────────────────────────────────────────
                ChildKind::Inline => {
                    let (avail_w, left_indent) =
                        float_ctx.available_at(line_start_y, cur_line.height.max(16.0));
                    let remaining_w = (avail_w - cur_line.width).max(0.0);
                    let child_is_text = matches!(entry.node.node.data, NodeData::Text { .. });
                    let child_container_width = if child_is_text {
                        remaining_w
                    } else {
                        avail_w
                    };
                    let (cb_opt, _, _) = build_layout_tree_with_cb_cached(
                        entry.node,
                        container_x + left_indent,
                        container_x + left_indent + cur_line.width,
                        0.0,
                        child_container_width,
                        vw,
                        vh,
                        child_cb,
                        intrinsic_cache,
                    );
                    if let Some(mut cb) = cb_opt {
                        if cb.dimensions.width == 0.0
                            && !matches!(entry.node.node.data, NodeData::Text { .. })
                        {
                            let fallback_w = intrinsic_cache.max_content_width(entry.node, vw, vh);
                            cb.dimensions.width = fallback_w.min(child_container_width).max(0.0);
                        }
                        // Only reset prev_margin_bottom when a visible inline element is
                        // actually placed.  Empty/whitespace-only text nodes return None and
                        // must NOT interrupt adjacent-block margin collapsing.
                        prev_margin_bottom = 0.0;
                        let item_w = margin_box_width(&cb);
                        let keep_table_cells_on_row =
                            self.display == DisplayType::TableRow && cb.display == DisplayType::TableCell;
                        if !keep_table_cells_on_row
                            && !no_wrap
                            && cur_line.width + item_w > avail_w
                            && !cur_line.members.is_empty()
                        {
                            flush_line!();
                            // Re-lay out for new line with updated float-aware width
                            let (aw2, li2) = float_ctx.available_at(line_start_y, 16.0);
                            let remaining_w2 = (aw2 - cur_line.width).max(0.0);
                            let child_container_width2 = if child_is_text {
                                remaining_w2
                            } else {
                                aw2
                            };
                            let (cb2_opt, _, _) = build_layout_tree_with_cb_cached(
                                entry.node,
                                container_x + li2,
                                container_x + li2,
                                0.0,
                                child_container_width2,
                                vw,
                                vh,
                                child_cb,
                                intrinsic_cache,
                            );
                            if let Some(mut cb2) = cb2_opt {
                                if cb2.dimensions.width == 0.0
                                    && !matches!(entry.node.node.data, NodeData::Text { .. })
                                {
                                    let fallback_w =
                                        intrinsic_cache.max_content_width(entry.node, vw, vh);
                                    cb2.dimensions.width =
                                        fallback_w.min(child_container_width2).max(0.0);
                                }
                                // Apply relative offset after line flush positioning.
                                if cb2.position == PositionType::Relative {
                                    apply_relative_offset(&mut cb2, vw, vh);
                                }
                                cur_line.width =
                                    margin_box_width(&cb2);
                                cur_line.height =
                                    margin_box_height(&cb2);
                                cur_line.members.push(cb2);
                            }
                        } else {
                            // Apply relative offset after accumulation so line-height measurement
                            // uses the pre-offset dimensions, and the visual nudge is applied before push.
                            if cb.position == PositionType::Relative {
                                apply_relative_offset(&mut cb, vw, vh);
                            }
                            cur_line.width += item_w;
                            cur_line.height = cur_line
                                .height
                                .max(margin_box_height(&cb));
                            cur_line.members.push(cb);
                        }
                    }
                }
            }
        }

        flush_line!();
        // Case 2 (bottom) — parent / last-child margin collapsing:
        // When no border or padding separates the parent's bottom edge from the last
        // block child, the child's bottom margin collapses into the parent's bottom margin
        // (no interior spacing at the bottom).  Only apply the last child's margin when
        // the parent *does* have a bottom border or padding.
        if !parent_open_bottom {
            cursor_y += prev_margin_bottom;
        }
        // Clearfix: ensure the container is tall enough to cover all floated children.
        cursor_y = cursor_y.max(float_ctx.bottom());

        // Now safe to mutably assign self.children (immutable borrow of self.style_node ended above).
        self.children = result;

        if self.dimensions.width <= 0.0 || (is_floated && auto_width) {
            let derived = max_child_x - self.dimensions.x + self.padding.right + self.border.right;
            self.dimensions.width = if container_width.is_finite() {
                derived.min(container_width)
            } else {
                derived
            };
        }

        let content_height =
            (cursor_y - self.dimensions.y + self.padding.bottom + self.border.bottom).max(0.0);
        let mut final_h = if height > 0.0 { height } else { content_height };
        if let Some(Value::Length(v, Unit::Px)) = self
            .style_node
            .specified_values
            .get(&crate::css::intern("max-height"))
        {
            let max_h = if box_sizing == "border-box" {
                (*v - self.padding.top - self.padding.bottom - self.border.top - self.border.bottom)
                    .max(0.0)
            } else {
                *v
            };
            final_h = final_h.min(max_h);
        }
        if let Some(Value::Length(v, Unit::Px)) = self
            .style_node
            .specified_values
            .get(&crate::css::intern("min-height"))
        {
            let min_h = if box_sizing == "border-box" {
                (*v - self.padding.top - self.padding.bottom - self.border.top - self.border.bottom)
                    .max(0.0)
            } else {
                *v
            };
            final_h = final_h.max(min_h);
        }
        self.dimensions.height = final_h;

        // ── Layout absolutely/fixedly positioned children ─────────────────────
        // Now that self has its final dimensions, we can resolve absolute offsets against it.
        if !positioned_entries.is_empty() {
            // The final content-box of self (after height is known).
            let final_self_cb = Rect {
                x: self.dimensions.x + self.padding.left + self.border.left,
                y: self.dimensions.y + self.padding.top + self.border.top,
                width: (self.dimensions.width
                    - self.padding.left
                    - self.padding.right
                    - self.border.left
                    - self.border.right)
                    .max(0.0),
                height: (self.dimensions.height
                    - self.padding.top
                    - self.padding.bottom
                    - self.border.top
                    - self.border.bottom)
                    .max(0.0),
            };

            for pos_node in positioned_entries {
                let child_pos_type = get_position_type(pos_node);
                // fixed: containing block = viewport; absolute: nearest positioned ancestor.
                let cb_for_child = if child_pos_type == PositionType::Fixed {
                    viewport_rect
                } else {
                    // absolute: use this box's content area if self establishes a CB,
                    // otherwise fall back to the inherited containing_block.
                    if self_establishes_cb {
                        final_self_cb
                    } else {
                        containing_block.unwrap_or(viewport_rect)
                    }
                };

                // Intrinsic width for the positioned child.
                // If both left and right are specified and no explicit width, the element
                // stretches to fill the space between them (CSS spec §10.3.7).
                let left_offset = resolve_offset(pos_node, "left", cb_for_child.width, vw, vh);
                let right_offset = resolve_offset(pos_node, "right", cb_for_child.width, vw, vh);
                let top_offset = resolve_offset(pos_node, "top", cb_for_child.height, vw, vh);
                let bottom_offset = resolve_offset(pos_node, "bottom", cb_for_child.height, vw, vh);

                let child_explicit_width =
                    pos_node.specified_values.get(&crate::css::intern("width"));
                let child_layout_width = match child_explicit_width {
                    Some(Value::Length(v, Unit::Px)) => *v,
                    Some(Value::Length(v, Unit::Percent)) => cb_for_child.width * (v / 100.0),
                    _ => {
                        // Both left and right specified without explicit width → stretch.
                        if let (Some(l), Some(r)) = (left_offset, right_offset) {
                            (cb_for_child.width - l - r).max(0.0)
                        } else {
                            // Shrink-wrap: lay out at max-content width bounded by cb width.
                            let max_c = intrinsic_cache.max_content_width(pos_node, vw, vh);
                            max_c.min(cb_for_child.width)
                        }
                    }
                };

                // Build the child in a temporary origin; we'll reposition it below.
                let (pc_opt, _, _) = build_layout_tree_with_cb_cached(
                    pos_node,
                    0.0,
                    0.0,
                    0.0,
                    child_layout_width.max(1.0),
                    vw,
                    vh,
                    Some(cb_for_child),
                    intrinsic_cache,
                );
                if let Some(mut pc) = pc_opt {
                    // Determine final x.
                    let target_x = match (left_offset, right_offset) {
                        (Some(l), _) => cb_for_child.x + l + pc.margin.left,
                        (None, Some(r)) => {
                            cb_for_child.x + cb_for_child.width
                                - r
                                - pc.dimensions.width
                                - pc.margin.right
                        }
                        (None, None) => cb_for_child.x + pc.margin.left, // default to CB origin
                    };
                    // Determine final y.
                    let target_y = match (top_offset, bottom_offset) {
                        (Some(t), _) => cb_for_child.y + t + pc.margin.top,
                        (None, Some(b)) => {
                            cb_for_child.y + cb_for_child.height
                                - b
                                - pc.dimensions.height
                                - pc.margin.bottom
                        }
                        (None, None) => cb_for_child.y + pc.margin.top, // default to CB origin
                    };

                    let dx = target_x - pc.dimensions.x;
                    let dy = target_y - pc.dimensions.y;
                    offset_layout_box(&mut pc, dx, dy);

                    self.children.push(pc);
                }
            }
        }

        let final_x = if is_block {
            container_start_x
        } else {
            self.dimensions.x + self.dimensions.width + self.margin.right
        };
        let final_y = if is_block {
            self.dimensions.y + self.dimensions.height + self.margin.bottom
        } else {
            cursor_y
        };
        (Some(self), final_x, final_y)
    }

    fn layout_text(
        mut self,
        text: String,
        container_start_x: f32,
        current_x: f32,
        current_y: f32,
        container_width: f32,
    ) -> (Option<LayoutBox<'a>>, f32, f32) {
        let trimmed = text.trim();
        let font_size = match self
            .style_node
            .specified_values
            .get(&crate::css::intern("font-size"))
        {
            Some(Value::Length(v, Unit::Px)) => v.max(1.0),
            _ => 16.0,
        };
        let font = FontRef::try_from_slice(FONT_DATA).unwrap();
        let scale = PxScale::from(font_size);
        let units = font.units_per_em().unwrap_or(1000.0) as f32;
        let line_height = font_size * 1.4;
        let space_w = font.h_advance_unscaled(font.glyph_id(' ')) * (scale.x / units);
        let white_space = self
            .style_node
            .specified_values
            .get(&crate::css::intern("white-space"))
            .and_then(|v| if let Value::Keyword(k) = v { Some(&**k as &str) } else { None })
            .unwrap_or("normal");
        let no_wrap = white_space == "nowrap";

        // CSS white-space: normal — whitespace-only text nodes between inline
        // elements collapse to a single inter-element space.  We only insert
        // that space when we are NOT at the start of a line (i.e. there is
        // already inline content to our left).
        let at_line_start = current_x <= container_start_x + 0.5;

        if trimmed.is_empty() {
            // Whitespace-only node: emit a single-space-wide invisible box so
            // the next inline sibling is separated from the previous one.
            if !at_line_start && text.contains(|c: char| c.is_whitespace()) {
                let w = space_w.min(container_width.max(0.0));
                self.dimensions.x = current_x + self.margin.left;
                self.dimensions.y = current_y + self.margin.top;
                self.dimensions.width = w;
                self.dimensions.height = line_height;
                let final_x = self.dimensions.x + w + self.margin.right;
                let final_y = self.dimensions.y + line_height + self.margin.bottom;
                return (Some(self), final_x, final_y);
            }
            return (None, current_x, current_y);
        }

        // Detect leading / trailing whitespace in the original text node.
        // CSS spec collapses each run of whitespace to a single space; we
        // model that by prepending / appending one space_w when not at line start.
        let has_leading_space =
            !at_line_start && text.starts_with(|c: char| c.is_whitespace());
        let has_trailing_space = text.ends_with(|c: char| c.is_whitespace());

        let mut lines_count = 1;
        let mut line_w: f32 = 0.0;
        let mut max_w: f32 = 0.0;

        // Leading inter-element space (counts toward width but is invisible).
        if has_leading_space {
            line_w += space_w;
        }

        for word in trimmed.split_whitespace() {
            let mut word_w = 0.0;
            for c in word.chars() {
                word_w += font.h_advance_unscaled(font.glyph_id(c)) * (scale.x / units);
            }

            if no_wrap {
                if line_w > 0.0 {
                    line_w += space_w;
                }
                line_w += word_w;
                continue;
            }

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
                if line_w > 0.0 {
                    line_w += space_w;
                }
                line_w += word_w;
            }
        }

        // Trailing inter-element space: allows the following inline sibling to
        // start with a visible gap even though it has no leading whitespace itself.
        if has_trailing_space && line_w > 0.0 {
            line_w += space_w;
        }

        max_w = max_w.max(line_w);

        self.dimensions.x = current_x + self.margin.left;
        self.dimensions.y = current_y + self.margin.top;
        self.dimensions.width = if no_wrap {
            max_w
        } else if container_width.is_finite() {
            max_w.min(container_width)
        } else {
            max_w
        };

        self.dimensions.height = lines_count as f32 * line_height;

        let final_x = self.dimensions.x + self.dimensions.width + self.margin.right;
        let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
        (Some(self), final_x, final_y)
    }
}

fn get_float(sn: &StyledNode) -> Option<FloatSide> {
    match sn.specified_values.get(&crate::css::intern("float")) {
        Some(Value::Keyword(k)) => match &**k {
            "left" => Some(FloatSide::Left),
            "right" => Some(FloatSide::Right),
            _ => None,
        },
        _ => None,
    }
}

fn get_clear(sn: &StyledNode) -> Option<ClearValue> {
    match sn.specified_values.get(&crate::css::intern("clear")) {
        Some(Value::Keyword(k)) => match &**k {
            "left" => Some(ClearValue::Left),
            "right" => Some(ClearValue::Right),
            "both" => Some(ClearValue::Both),
            _ => None,
        },
        _ => None,
    }
}

fn get_position_type(sn: &StyledNode) -> PositionType {
    match sn.specified_values.get(&crate::css::intern("position")) {
        Some(Value::Keyword(k)) => match &**k {
            "relative" => PositionType::Relative,
            "absolute" => PositionType::Absolute,
            "fixed" => PositionType::Fixed,
            "sticky" => PositionType::Sticky,
            _ => PositionType::Static,
        },
        _ => PositionType::Static,
    }
}

/// Resolve a single offset property (`top`, `right`, `bottom`, `left`) from a `StyledNode`.
/// Returns `None` if the property is absent or `auto`.
fn resolve_offset(
    sn: &StyledNode,
    prop: &str,
    container_size: f32,
    vw: f32,
    vh: f32,
) -> Option<f32> {
    match sn.specified_values.get(&crate::css::intern(prop)) {
        Some(Value::Length(v, Unit::Px)) => Some(*v),
        Some(Value::Length(v, Unit::Percent)) => Some(container_size * (v / 100.0)),
        Some(Value::Length(v, Unit::Vw)) => Some(vw * (v / 100.0)),
        Some(Value::Length(v, Unit::Vh)) => Some(vh * (v / 100.0)),
        // Unitless 0 is a valid <length> in CSS (the only unitless length allowed).
        Some(Value::Number(v)) if *v == 0.0 => Some(0.0),
        Some(Value::Keyword(k)) if **k == *"auto" => None,
        _ => None,
    }
}

/// Apply `top`/`left`/`right`/`bottom` as visual offsets for `position: relative` elements.
///
/// The element keeps its normal-flow slot (no effect on layout of siblings),
/// but its rendered position is shifted by the offset values.
fn apply_relative_offset(layout: &mut LayoutBox, vw: f32, vh: f32) {
    let sn = layout.style_node;
    // For relative positioning, offsets resolve against the element's own width/height.
    // We use 0.0 as the container dimension since percentage offsets on relative
    // elements are relative to the containing block — a close enough approximation.
    let top = resolve_offset(sn, "top", layout.dimensions.height, vw, vh);
    let left = resolve_offset(sn, "left", layout.dimensions.width, vw, vh);
    let right = resolve_offset(sn, "right", layout.dimensions.width, vw, vh);
    let bottom = resolve_offset(sn, "bottom", layout.dimensions.height, vw, vh);

    let dx = match (left, right) {
        (Some(l), _) => l,
        (None, Some(r)) => -r,
        (None, None) => 0.0,
    };
    let dy = match (top, bottom) {
        (Some(t), _) => t,
        (None, Some(b)) => -b,
        (None, None) => 0.0,
    };

    if dx != 0.0 || dy != 0.0 {
        offset_layout_box(layout, dx, dy);
    }
}

fn resolved_font_size_px(sn: &StyledNode) -> f32 {
    match sn.specified_values.get(&crate::css::intern("font-size")) {
        Some(Value::Length(v, Unit::Px)) => (*v).max(1.0),
        _ => 16.0,
    }
}

fn resolved_line_height_px(sn: &StyledNode) -> f32 {
    let font_size = resolved_font_size_px(sn);
    match sn.specified_values.get(&crate::css::intern("line-height")) {
        Some(Value::Length(v, Unit::Px)) => (*v).max(0.0),
        Some(Value::Number(v)) => (font_size * *v).max(0.0),
        _ => font_size * 1.4,
    }
}

fn get_line_break_clear(sn: &StyledNode) -> Option<ClearValue> {
    if let Some(clear) = get_clear(sn) {
        return Some(clear);
    }

    if let NodeData::Element { ref attrs, .. } = sn.node.data {
        for attr in attrs.borrow().iter() {
            if attr.name.local.as_ref() != "clear" {
                continue;
            }
            return match attr.value.as_ref() {
                "left" => Some(ClearValue::Left),
                "right" => Some(ClearValue::Right),
                "all" | "both" => Some(ClearValue::Both),
                _ => None,
            };
        }
    }

    None
}

fn get_prop(sn: &StyledNode, p1: &str, p2: &str, cw: f32, vw: f32, vh: f32) -> f32 {
    match sn
        .specified_values
        .get(&crate::css::intern(p1))
        .or(sn.specified_values.get(&crate::css::intern(p2)))
    {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Length(v, Unit::Percent)) => cw * (v / 100.0),
        Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
        Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
        _ => 0.0,
    }
}

fn specified_width_percent(sn: &StyledNode) -> Option<f32> {
    match sn.specified_values.get(&crate::css::intern("width")) {
        Some(Value::Length(v, Unit::Percent)) => Some(*v),
        _ => None,
    }
}

fn get_display_type(sn: &StyledNode) -> DisplayType {
    if let NodeData::Text { .. } = sn.node.data {
        return DisplayType::Inline;
    }
    if let Some(Value::Keyword(d)) = sn.specified_values.get(&crate::css::intern("display")) {
        match &**d {
            "block" => return DisplayType::Block,
            "inline-block" => return DisplayType::InlineBlock,
            "flex" => return DisplayType::Flex,
            "grid" => return DisplayType::Grid,
            "none" => return DisplayType::Inline, // will be handled by is_none_display
            _ => {}
        }
    }
    if let NodeData::Element { ref name, .. } = sn.node.data {
        match name.local.to_string().as_str() {
            // Genuine block-level elements (fill container width, force line break)
            "html" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "body" | "header"
            | "footer" | "nav" | "section" | "article" | "ul" | "ol" | "main" | "aside"
            | "form" | "details" | "summary" | "figure" | "figcaption" | "address"
            | "blockquote" | "pre" | "hr" | "fieldset" | "legend"
            // <center> is a legacy block element with implicit text-align:center
            | "center" => DisplayType::Block,
            // List items get their own display type so markers can be painted
            "li" => DisplayType::ListItem,
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
    } else {
        DisplayType::Block
    }
}

fn is_block_level(d: DisplayType) -> bool {
    // Table/TableRow/TableCell are NOT block-level: they shrink-wrap to content
    // rather than filling the full container width.
    matches!(
        d,
        DisplayType::Block | DisplayType::ListItem | DisplayType::Flex | DisplayType::Grid
    )
}

fn is_none_display(sn: &StyledNode) -> bool {
    if let Some(Value::Keyword(d)) = sn.specified_values.get(&crate::css::intern("display")) {
        **d == *"none"
    } else {
        false
    }
}

fn should_skip(child: &StyledNode) -> bool {
    // First check the CSS display property — display:none always hides the element.
    if is_none_display(child) {
        return true;
    }
    if let NodeData::Element { ref name, ref attrs, .. } = child.node.data {
        let t = name.local.to_string();
        if matches!(
            t.as_str(),
            "head" | "style" | "meta" | "title" | "script" | "link" | "noscript"
        ) {
            return true;
        }
        if t == "svg" {
            // This engine does not rasterize SVG content yet.
            // Skipping the subtree avoids malformed icon blobs and keeps the
            // surrounding layout box size driven by CSS width/height.
            return true;
        }
        // <input type="hidden"> never renders, regardless of CSS.
        // Browsers treat this as a UA-level hardcoded rule that CSS cannot override.
        if t == "input" {
            let is_hidden = attrs.borrow().iter().any(|a| {
                a.name.local.to_string() == "type"
                    && a.value.to_string().eq_ignore_ascii_case("hidden")
            });
            if is_hidden {
                return true;
            }
        }
        false
    } else {
        false
    }
}

fn is_line_break_element(child: &StyledNode) -> bool {
    matches!(
        &child.node.data,
        NodeData::Element { name, .. } if name.local.to_string() == "br"
    )
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
        if !(x >= root_d.x
            && x <= root_d.x + root_d.width
            && y >= root_d.y
            && y <= root_d.y + root_d.height)
        {
            return None;
        }

        // DFS stack: each entry is a node that contains the point, plus the index of
        // the next child to try (children are tried in reverse, i.e., last painter first).
        // Invariant: every node on the stack contains the point.
        let mut stack: Vec<(&LayoutBox<'a>, isize)> =
            vec![(self, self.children.len() as isize - 1)];

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
            if let Some(ref url) = node.link_url {
                list.push((node.dimensions, url.clone()));
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
    }

    /// Iterative collect_event_handlers — avoids stack overflow on deep trees.
    pub fn collect_event_handlers(&self, list: &mut Vec<(Rect, String)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let Some(script) = node.event_handlers.get("click") {
                list.push((node.dimensions, script.clone()));
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
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
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
    }

    /// Iterative collect_images — avoids stack overflow on deep trees.
    pub fn collect_images(&self, list: &mut Vec<(Rect, String)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let Some(ref url) = node.image_url {
                list.push((node.dimensions, url.clone()));
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
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
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
    }

    /// Iterative collect_focusable_elements — avoids stack overflow on deep trees.
    pub fn collect_focusable_elements(&self, list: &mut Vec<(Rect, String)>) {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let NodeData::Element {
                ref name,
                ref attrs,
                ..
            } = node.style_node.node.data
            {
                let tag = name.local.to_string();
                let mut id = None;
                let mut has_href = false;

                for attr in attrs.borrow().iter() {
                    let key = attr.name.local.to_string();
                    if key == "id" {
                        id = Some(attr.value.to_string());
                    }
                    if key == "href" {
                        has_href = true;
                    }
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
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
    }

    pub fn establishes_stacking_context(&self) -> bool {
        // CSS spec: positioned elements (non-static) always form a stacking context,
        // as do elements with z-index != 0, opacity < 1, transforms, etc.
        let is_positioned = !matches!(self.position, PositionType::Static);
        is_positioned || self.z_index != 0 || self.establishes_bfc()
    }

    pub fn establishes_bfc(&self) -> bool {
        match self.display {
            DisplayType::InlineBlock | DisplayType::Flex | DisplayType::Grid | DisplayType::TableCell => true,
            _ => {
                // overflow != visible also establishes BFC
                if let Some(Value::Keyword(v)) = self
                    .style_node
                    .specified_values
                    .get(&crate::css::intern("overflow"))
                {
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
        println!(
            "{}{} [{:?}] [{:.1},{:.1} {:.1}x{:.1}]",
            indent_str,
            "Node",
            node.display,
            node.dimensions.x,
            node.dimensions.y,
            node.dimensions.width,
            node.dimensions.height
        );
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
            width: (self.dimensions.width
                - self.border.left
                - self.border.right
                - self.padding.left
                - self.padding.right)
                .max(0.0),
            height: (self.dimensions.height
                - self.border.top
                - self.border.bottom
                - self.padding.top
                - self.padding.bottom)
                .max(0.0),
        }
    }

    /// Returns the CSS `opacity` value for this box, clamped to [0.0, 1.0].
    /// Defaults to 1.0 (fully opaque) if the property is absent or unparseable.
    pub fn get_opacity(&self) -> f32 {
        match self
            .style_node
            .specified_values
            .get(&crate::css::intern("opacity"))
        {
            Some(Value::Number(n)) => n.clamp(0.0, 1.0),
            _ => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css;
    use crate::dom;
    use crate::style;
    use std::sync::Arc;

    #[test]
    fn test_button_coordinate_collection() {
        let html = r#"<button onclick="alert(1)" style="width: 100px; height: 50px; margin: 10px;">Click me</button>"#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 1024.0, 1024.0, 768.0);
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
        let mut style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        // Ensure the style is manually set if parser was ambiguous
        if let NodeData::Element { .. } = style_tree.children[0].node.data {
            let mut map = (*style_tree.children[0].specified_values.0).clone();
            map.insert(
                crate::css::intern("display"),
                css::Value::Keyword(crate::css::intern("block")),
            );
            map.insert(
                crate::css::intern("width"),
                css::Value::Length(500.0, css::Unit::Px),
            );
            map.insert(
                crate::css::intern("margin-left"),
                css::Value::Keyword(crate::css::intern("auto")),
            );
            map.insert(
                crate::css::intern("margin-right"),
                css::Value::Keyword(crate::css::intern("auto")),
            );
            style_tree.children[0].specified_values = style::PropertyMap(Arc::new(map));
        }

        let (layout_opt, _, _) = build_layout_tree(
            &style_tree.children[0],
            0.0,
            0.0,
            0.0,
            1000.0,
            1000.0,
            768.0,
        );
        let layout = layout_opt.unwrap();

        assert_eq!(layout.dimensions.width, 500.0);
        assert_eq!(layout.dimensions.x, 250.0); // (1000 - 500) / 2
    }

    #[test]
    fn test_text_keeps_parent_flow_position() {
        let html = r#"<div style="margin-left: 48px; margin-top: 24px;">Hello world</div>"#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

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

    fn find_text_box_containing<'a>(
        layout: &'a LayoutBox<'a>,
        needle: &str,
    ) -> Option<&'a LayoutBox<'a>> {
        if let NodeData::Text { ref contents } = layout.style_node.node.data {
            if contents.borrow().contains(needle) {
                return Some(layout);
            }
        }

        for child in &layout.children {
            if let Some(found) = find_text_box_containing(child, needle) {
                return Some(found);
            }
        }

        None
    }

    fn find_element_by_id<'a>(layout: &'a LayoutBox<'a>, id: &str) -> Option<&'a LayoutBox<'a>> {
        if let NodeData::Element { ref attrs, .. } = layout.style_node.node.data {
            for attr in attrs.borrow().iter() {
                if attr.name.local.to_string() == "id" && attr.value.to_string() == id {
                    return Some(layout);
                }
            }
        }
        for child in &layout.children {
            if let Some(found) = find_element_by_id(child, id) {
                return Some(found);
            }
        }
        None
    }

    /// Convenience test helper: parse HTML into a LayoutBox tree.
    /// Leaks DOM/stylesheet/style-tree so `LayoutBox<'static>` is valid for the
    /// lifetime of the test. The leak is acceptable in unit tests.
    fn layout_from_html(html: &str, width: f32, height: f32) -> (LayoutBox<'static>, f32, f32) {
        let dom = Box::leak(Box::new(dom::parse_html(html)));
        let ss = Box::leak(Box::new(css::parse_css("")));
        let style_tree = Box::leak(Box::new(style::build_style_tree(
            &dom.document,
            ss,
            None,
            &std::collections::HashMap::new(),
            None,
            None,
            None,
        )));
        let (layout_opt, fx, fy) =
            build_layout_tree(style_tree, 0.0, 0.0, 0.0, width, width, height);
        (layout_opt.expect("layout tree"), fx, fy)
    }

    fn layout_from_html_css(
        html: &str,
        css_src: &str,
        width: f32,
        height: f32,
    ) -> (LayoutBox<'static>, f32, f32) {
        let dom = Box::leak(Box::new(dom::parse_html(html)));
        let ss = Box::leak(Box::new(css::parse_css(css_src)));
        let style_tree = Box::leak(Box::new(style::build_style_tree(
            &dom.document,
            ss,
            None,
            &std::collections::HashMap::new(),
            None,
            None,
            None,
        )));
        let (layout_opt, fx, fy) =
            build_layout_tree(style_tree, 0.0, 0.0, 0.0, width, width, height);
        (layout_opt.expect("layout tree"), fx, fy)
    }

    #[test]
    fn test_inline_element_shrinks_to_content() {
        // An inline <span> should derive its width from text content,
        // NOT expand to the full container_width (800px).
        let html = r#"<span>Hi</span>"#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 768.0);
        let layout = layout_opt.unwrap();

        fn find_span<'a>(b: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
            if let NodeData::Element { ref name, .. } = b.style_node.node.data {
                if name.local.to_string() == "span" {
                    return Some(b);
                }
            }
            for c in &b.children {
                if let Some(f) = find_span(c) {
                    return Some(f);
                }
            }
            None
        }

        let span = find_span(&layout).expect("span not found");
        assert!(span.dimensions.width > 0.0, "span width must be > 0");
        assert!(
            span.dimensions.width < 800.0,
            "span width {} must be < container_width 800 (should shrink to content)",
            span.dimensions.width
        );
    }

    #[test]
    fn test_inline_text_wraps_against_remaining_line_width() {
        let html = r#"
            <div style="width: 800px;">
                <span style="display: inline-block; width: 280px;">prefix</span>
                This sentence should wrap based on the remaining line width after the inline prefix instead of overflowing past the viewport edge.
            </div>
        "#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 768.0);
        let layout = layout_opt.expect("layout");
        let text = find_text_box_containing(&layout, "This sentence").expect("text node not found");

        assert!(
            text.dimensions.x >= 280.0,
            "inline text should start after the prefix, got x={}",
            text.dimensions.x
        );
        assert!(
            text.dimensions.width <= 520.0,
            "inline text width should be limited by the remaining line width, got {}",
            text.dimensions.width
        );
        assert!(text.dimensions.height > 24.0,
            "inline text should wrap onto multiple lines when the prefix consumes horizontal space, got height={}",
            text.dimensions.height);
    }

    #[test]
    fn test_inline_block_wraps_when_remaining_line_width_is_insufficient() {
        let html = r#"
            <form style="width: 600px;">
                <span id="prefix" style="display: inline-block; width: 360px;">prefix</span>
                <span id="middle" style="display: inline-block;">
                    search controls should shrink against the remaining row width
                </span>
                <span id="tail">tail</span>
            </form>
        "#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 600.0, 600.0, 768.0);
        let layout = layout_opt.expect("layout");
        let prefix = find_element_by_id(&layout, "prefix").expect("prefix not found");
        let middle = find_element_by_id(&layout, "middle").expect("middle not found");
        let tail = find_element_by_id(&layout, "tail").expect("tail not found");

        assert!(
            middle.dimensions.y > prefix.dimensions.y,
            "middle inline-block should wrap when remaining row width is insufficient: prefix.y={}, middle.y={}",
            prefix.dimensions.y,
            middle.dimensions.y
        );
        assert!(
            middle.dimensions.width <= 600.0 + 1.0,
            "middle inline-block width must remain bounded by container width, got {}",
            middle.dimensions.width
        );
        assert!(
            tail.dimensions.y >= middle.dimensions.y,
            "tail should remain in stable flow after middle: middle.y={}, tail.y={}",
            middle.dimensions.y,
            tail.dimensions.y
        );
    }

    #[test]
    fn test_inline_link_wraps_instead_of_shrinking_to_tiny_remaining_width() {
        let html = r#"
            <div style="width: 200px;">
                <span style="display: inline-block; width: 180px;">prefix</span>
                <a id="tail-link">고급검색</a>
            </div>
        "#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 200.0, 200.0, 768.0);
        let layout = layout_opt.expect("layout");
        let prefix = find_text_box_containing(&layout, "prefix").expect("prefix text not found");
        let link = find_element_by_id(&layout, "tail-link").expect("tail link not found");

        assert!(
            link.dimensions.y > prefix.dimensions.y,
            "link should wrap to next line when only tiny remaining width is left: prefix.y={}, link.y={}",
            prefix.dimensions.y,
            link.dimensions.y
        );
        assert!(
            link.dimensions.width > 20.0,
            "wrapped link should retain sane intrinsic width instead of shrinking to the tiny leftover width, got {}",
            link.dimensions.width
        );
    }

    #[test]
    fn test_br_forces_following_inline_content_onto_next_line() {
        let html = r#"
            <div style="width: 600px;">
                <span id="search" style="display:inline-block; width: 458px; height: 25px;">search</span>
                <br>
                <span id="btn-g" style="display:inline-block; width: 160px; height: 30px;">Google Search</span>
                <span id="btn-i" style="display:inline-block; width: 160px; height: 30px;">I'm Feeling Lucky</span>
            </div>
        "#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 600.0, 600.0, 768.0);
        let layout = layout_opt.expect("layout");
        let search = find_element_by_id(&layout, "search").expect("search not found");
        let btn_g = find_element_by_id(&layout, "btn-g").expect("btn-g not found");
        let btn_i = find_element_by_id(&layout, "btn-i").expect("btn-i not found");

        assert!(
            btn_g.dimensions.y > search.dimensions.y,
            "content after <br> must start on a later line: search.y={}, btn_g.y={}",
            search.dimensions.y,
            btn_g.dimensions.y
        );
        assert!(
            btn_i.dimensions.y >= btn_g.dimensions.y,
            "following inline content should remain on the post-<br> line: btn_g.y={}, btn_i.y={}",
            btn_g.dimensions.y,
            btn_i.dimensions.y
        );
    }

    #[test]
    fn test_consecutive_br_adds_blank_line_height() {
        let html = r#"
            <div style="width: 400px;">
                first
                <br>
                <br>
                <span id="second">second</span>
            </div>
        "#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 400.0, 400.0, 600.0);
        let layout = layout_opt.expect("layout");
        let first = find_text_box_containing(&layout, "first").expect("first text");
        let second = find_element_by_id(&layout, "second").expect("second span");

        assert!(
            second.dimensions.y >= first.dimensions.y + 40.0,
            "consecutive <br> should create a blank line of vertical space: first.y={}, second.y={}",
            first.dimensions.y,
            second.dimensions.y
        );
    }

    #[test]
    fn test_br_clear_all_pushes_content_below_float() {
        let html = r#"
            <div style="width: 800px;">
                <div style="float:right; width:120px; height:60px;">header</div>
                <br clear="all">
                <span id="after">after</span>
            </div>
        "#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");
        let after = find_element_by_id(&layout, "after").expect("after span");

        assert!(
            after.dimensions.y >= 60.0,
            "<br clear=\"all\"> should push following content below the float, got y={}",
            after.dimensions.y
        );
    }

    #[test]
    fn test_inline_zero_width_falls_back_to_intrinsic_for_positioned_children() {
        let html = r#"
            <div style="width: 160px;">
                <a id="login-link" style="display: inline-block;">
                    <span style="position: absolute;">로그인</span>
                </a>
                <span id="after">after</span>
            </div>
        "#;
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 160.0, 160.0, 768.0);
        let layout = layout_opt.expect("layout");
        let link = find_element_by_id(&layout, "login-link").expect("login-link not found");
        let after = find_element_by_id(&layout, "after").expect("after not found");

        assert!(
            link.dimensions.width > 20.0,
            "inline box with positioned descendants should get intrinsic fallback width, got {}",
            link.dimensions.width
        );
        assert!(
            after.dimensions.x >= link.dimensions.x + link.dimensions.width - 1.0,
            "following inline content should flow after fallback-width element: link.right={}, after.x={}",
            link.dimensions.x + link.dimensions.width,
            after.dimensions.x
        );
    }

    // ── Float layout tests ────────────────────────────────────────────────────

    /// Deep-search the layout tree for the first child that has `float: left` or `float: right`.
    fn find_float_child_deep<'a>(layout: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
        for child in &layout.children {
            match child
                .style_node
                .specified_values
                .get(&crate::css::intern("float"))
            {
                Some(Value::Keyword(k)) if &**k == "left" || &**k == "right" => return Some(child),
                _ => {}
            }
            if let Some(f) = find_float_child_deep(child) {
                return Some(f);
            }
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
                                    if let NodeData::Element { ref name, .. } =
                                        div.style_node.node.data
                                    {
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
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let float_child = find_float_child_deep(&layout).expect("float child not found");
        assert_eq!(
            float_child.dimensions.x, 0.0,
            "float:left child should have x=0.0, got {}",
            float_child.dimensions.x
        );
    }

    #[test]
    fn test_float_right_x() {
        let html = r#"<div style="width:800px;"><div style="float:right;width:100px;height:50px;">F</div></div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let float_child = find_float_child_deep(&layout).expect("float child not found");
        assert_eq!(
            float_child.dimensions.x, 700.0,
            "float:right child (width 100px) in 800px container should have x=700.0, got {}",
            float_child.dimensions.x
        );
    }

    #[test]
    fn test_float_right_auto_width_shrink_wraps_contents() {
        let html = r#"
            <div style="width:800px;">
                <div id="header-actions" style="float:right; position:relative;">
                    <a id="apps" style="display:inline-block; width:24px; height:40px;">A</a>
                    <a id="login" style="display:inline-block; min-width:85px; min-height:40px; margin:12px 16px 12px 10px; padding:10px 12px; background:#0b57d0; border-radius:100px; color:#fff;">Login</a>
                </div>
            </div>
        "#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");
        let actions = find_element_by_id(&layout, "header-actions").expect("header-actions not found");
        let login = find_element_by_id(&layout, "login").expect("login not found");

        assert!(
            actions.dimensions.width < 300.0,
            "auto-width float should shrink-wrap its contents instead of expanding to the full line, got {}",
            actions.dimensions.width
        );
        assert!(
            login.dimensions.x + login.dimensions.width <= 800.0,
            "shrink-wrapped float contents should stay within the viewport, got right edge {}",
            login.dimensions.x + login.dimensions.width
        );
    }

    #[test]
    fn test_float_right_header_with_nested_utility_cluster_stays_in_viewport() {
        let html = r#"
            <div style="width:800px; padding:6px;">
                <div class="gb_Jd">
                    <div id="actions" class="gb_7d gb_Xd">
                        <div>
                            <div class="gb_Q">
                                <div class="gb_5"><a id="gmail" class="gb_4">Gmail</a></div>
                                <div class="gb_5"><a id="images" class="gb_4">Images</a></div>
                            </div>
                        </div>
                        <div class="gb_Id">
                            <div class="gb_od">
                                <div class="gb_Ad"><a id="apps" class="gb_C">A</a></div>
                            </div>
                            <a id="login" class="gb_Td">Login</a>
                        </div>
                    </div>
                </div>
            </div>
        "#;
        let css = r#"
            .gb_Xd { height:48px; vertical-align:middle; white-space:nowrap; align-items:center; display:flex; }
            .gb_7d { box-sizing:border-box; height:48px; padding:0 4px; padding-left:5px; flex:0 0 auto; justify-content:flex-end; }
            .gb_Jd .gb_7d { float:right; padding-left:32px; }
            .gb_Q { line-height:normal; padding-right:15px; }
            .gb_5 { display:inline-block; padding-left:15px; }
            .gb_5 .gb_4 { display:inline-block; line-height:24px; vertical-align:middle; }
            .gb_Id { position:relative; float:right; }
            .gb_od { display:inline; }
            .gb_Ad { display:inline-block; vertical-align:middle; padding:4px; }
            .gb_C { display:inline-block; height:40px; width:40px; padding:8px; box-sizing:border-box; }
            .gb_Td { display:inline-block; padding:10px 12px; margin:12px 16px 12px 10px; min-width:85px; min-height:40px; }
        "#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css(css);
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");
        let gmail = find_element_by_id(&layout, "gmail").expect("gmail not found");
        let images = find_element_by_id(&layout, "images").expect("images not found");
        let apps = find_element_by_id(&layout, "apps").expect("apps not found");
        let login = find_element_by_id(&layout, "login").expect("login not found");

        assert!(
            (gmail.dimensions.y - images.dimensions.y).abs() < 2.0,
            "Gmail and Images should stay on the same row: gmail.y={}, images.y={}",
            gmail.dimensions.y,
            images.dimensions.y
        );
        assert!(
            images.dimensions.x > gmail.dimensions.x,
            "Images should stay to the right of Gmail: gmail.x={}, images.x={}",
            gmail.dimensions.x,
            images.dimensions.x
        );
        assert!(
            apps.dimensions.x >= 0.0,
            "app launcher should not be pushed off the left edge, got x={}",
            apps.dimensions.x
        );
        assert!(
            apps.dimensions.x + border_box_width(apps) <= 800.0,
            "app launcher should stay inside the viewport, got right edge {}",
            apps.dimensions.x + border_box_width(apps)
        );
        assert!(
            login.dimensions.x + border_box_width(login) <= 800.0,
            "login button should stay inside the viewport, got right edge {}",
            login.dimensions.x + border_box_width(login)
        );
    }

    #[test]
    fn test_border_box_min_size_includes_padding() {
        let html = r#"
            <div style="width:800px;">
                <a id="login" style="display:inline-block; box-sizing:border-box; min-width:85px; min-height:40px; padding:10px 12px;">Login</a>
            </div>
        "#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");
        let login = find_element_by_id(&layout, "login").expect("login not found");

        let border_w = login.dimensions.width + login.padding.left + login.padding.right + login.border.left + login.border.right;
        let border_h = login.dimensions.height + login.padding.top + login.padding.bottom + login.border.top + login.border.bottom;

        assert!(
            (border_w - 85.0).abs() < 2.0,
            "border-box min-width should include padding: got border width {}",
            border_w
        );
        assert!(border_h >= 40.0, "border-box min-height should not shrink below 40px, got {}", border_h);
    }

    #[test]
    fn test_clear_left_advances_cursor() {
        let html = r#"<div style="width:800px;"><div style="float:left;width:100px;height:50px;">F</div><div style="clear:left;">C</div></div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        // Navigate to the outer div (width:800px) then look at its direct children.
        let outer_div = find_outer_div(&layout).expect("outer div not found");
        let clear_block = find_direct_non_float_block(outer_div)
            .expect("clear:left block not found among outer div's children");
        assert!(
            clear_block.dimensions.y >= 50.0,
            "clear:left block must start at or below float bottom (50px), got y={}",
            clear_block.dimensions.y
        );
    }

    #[test]
    fn test_float_intrusion_narrows_sibling_block() {
        // float:left 100px wide → sibling block in the same container gets avail_w = 700px
        let html = r#"<div style="width:800px;"><div style="float:left;width:100px;height:50px;">F</div><div style="display:block;">S</div></div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let outer_div = find_outer_div(&layout).expect("outer div not found");
        let sibling =
            find_direct_non_float_block(outer_div).expect("non-float sibling block not found");
        assert_eq!(
            sibling.dimensions.width, 700.0,
            "sibling block should be narrowed to 700px by the 100px left float, got {}",
            sibling.dimensions.width
        );
    }

    // ── Intrinsic sizing tests ────────────────────────────────────────────────

    /// Helper: navigate into the layout tree and find the first element whose
    /// local tag name matches `tag`.
    fn find_element_by_tag<'a>(b: &'a LayoutBox<'a>, tag: &str) -> Option<&'a LayoutBox<'a>> {
        if let NodeData::Element { ref name, .. } = b.style_node.node.data {
            if name.local.to_string() == tag {
                return Some(b);
            }
        }
        for c in &b.children {
            if let Some(found) = find_element_by_tag(c, tag) {
                return Some(found);
            }
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
        assert_eq!(
            css::parse_value("min-content"),
            css::Value::Keyword(crate::css::intern("min-content"))
        );
        assert_eq!(
            css::parse_value("max-content"),
            css::Value::Keyword(crate::css::intern("max-content"))
        );
    }

    /// `compute_max_content_width` on "Hello World" must be wider than `compute_min_content_width`.
    #[test]
    fn test_intrinsic_width_ordering() {
        let html = r#"<span>Hello World</span>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );

        // Locate the <span> StyledNode
        fn find_span_node<'a>(
            sn: &'a crate::style::StyledNode,
        ) -> Option<&'a crate::style::StyledNode> {
            if let NodeData::Element { ref name, .. } = sn.node.data {
                if name.local.to_string() == "span" {
                    return Some(sn);
                }
            }
            for c in &sn.children {
                if let Some(f) = find_span_node(c) {
                    return Some(f);
                }
            }
            None
        }
        let span_node = find_span_node(&style_tree).expect("span not found");

        let min_c = compute_min_content_width(span_node, 800.0, 600.0);
        let max_c = compute_max_content_width(span_node, 800.0, 600.0);

        assert!(min_c > 0.0, "min-content must be > 0, got {min_c}");
        assert!(
            max_c > min_c,
            "max-content ({max_c}) must be wider than min-content ({min_c}) for multi-word text"
        );
    }

    /// `width: min-content` — the div must not span the full 800 px container.
    #[test]
    fn test_width_min_content_layout() {
        let html = r#"<div style="width: min-content;">Hello World</div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();

        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!(div.dimensions.width > 0.0, "div width must be > 0");
        assert!(
            div.dimensions.width < 800.0,
            "div with width:min-content must be < 800px (container), got {}",
            div.dimensions.width
        );
    }

    /// `width: max-content` — the div must be wider than a min-content div.
    #[test]
    fn test_width_max_content_layout() {
        // min-content case
        let dom_min = dom::parse_html(r#"<div style="width: min-content;">Hello World</div>"#);
        let ss_min = css::parse_css("");
        let st_min = style::build_style_tree(
            &dom_min.document,
            &ss_min,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (lo_min, _, _) = build_layout_tree(&st_min, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout_min = lo_min.unwrap();
        let div_min_w = find_element_by_tag(&layout_min, "div")
            .unwrap()
            .dimensions
            .width;

        // max-content case
        let dom_max = dom::parse_html(r#"<div style="width: max-content;">Hello World</div>"#);
        let ss_max = css::parse_css("");
        let st_max = style::build_style_tree(
            &dom_max.document,
            &ss_max,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (lo_max, _, _) = build_layout_tree(&st_max, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout_max = lo_max.unwrap();
        let div_max_w = find_element_by_tag(&layout_max, "div")
            .unwrap()
            .dimensions
            .width;

        assert!(
            div_max_w >= div_min_w,
            "max-content width ({div_max_w}) must be >= min-content width ({div_min_w})"
        );
        assert!(
            div_max_w < 800.0,
            "max-content width ({div_max_w}) must be < container (800px) for short text"
        );
    }

    /// `width: fit-content(150px)` — clamps to at most 150 px.
    #[test]
    fn test_fit_content_with_limit() {
        // "Hello World" max-content is well under 800px but we clamp to 150px
        let html = r#"<div style="width: fit-content(150px);">Hello World this is some longer text for the test</div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!(
            div.dimensions.width <= 150.0,
            "fit-content(150px) must be <= 150px, got {}",
            div.dimensions.width
        );
        assert!(
            div.dimensions.width > 0.0,
            "fit-content(150px) must be > 0, got {}",
            div.dimensions.width
        );
    }

    /// `width: fit-content` (no argument) — shrinks to content but stays <= container.
    #[test]
    fn test_fit_content_no_arg() {
        let html = r#"<div style="width: fit-content;">Hello</div>"#;
        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!(div.dimensions.width > 0.0, "fit-content width must be > 0");
        assert!(
            div.dimensions.width <= 800.0,
            "fit-content width must be <= container (800px)"
        );
    }

    // ── get_opacity tests ────────────────────────────────────────────────────

    #[test]
    fn test_get_opacity_default() {
        // An element with no opacity style should return 1.0
        let html = r#"<div style="width:100px;height:50px;">Content</div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!(
            (div.get_opacity() - 1.0).abs() < f32::EPSILON,
            "default opacity must be 1.0"
        );
    }

    #[test]
    fn test_get_opacity_value() {
        // An element with opacity:0.5 should return 0.5
        let html = r#"<div style="width:100px;height:50px;opacity:0.5;">Content</div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.unwrap();
        let div = find_element_by_tag(&layout, "div").expect("div not found");
        assert!(
            (div.get_opacity() - 0.5).abs() < 0.01,
            "opacity must be 0.5, got {}",
            div.get_opacity()
        );
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
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str("leaf");
        for _ in 0..depth {
            html.push_str("</div>");
        }

        let dom = dom::parse_html(&html);
        let ss = css::parse_css("");
        // build_style_tree calls flatten_dom and build_final_tree — both iterative.
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);

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
        for _ in 0..5000 {
            html.push_str("</div>");
        }

        let dom = dom::parse_html(&html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        assert!(
            layout_opt.is_some(),
            "layout must succeed for 5000 mixed-display nested divs"
        );
    }

    // ── Image rendering tests ─────────────────────────────────────────────────

    fn find_image_box<'a>(lb: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
        if lb.display == DisplayType::Image {
            return Some(lb);
        }
        for c in &lb.children {
            if let Some(r) = find_image_box(c) {
                return Some(r);
            }
        }
        None
    }

    #[test]
    fn test_image_alt_text_stored() {
        let html = r#"<img src="x.png" alt="hello">"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree must be built");
        let img = find_image_box(&layout).expect("img node must be found");
        assert_eq!(
            img.alt_text,
            Some("hello".to_string()),
            "alt attribute must be stored as alt_text"
        );
    }

    #[test]
    fn test_image_fallback_height() {
        // Only width is specified — height must be derived as a non-zero placeholder.
        let html = r#"<img src="x.png" style="width:200px">"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree must be built");
        let img = find_image_box(&layout).expect("img node must be found");
        assert!(
            img.dimensions.height > 0.0,
            "image with only width specified must have non-zero height, got {}",
            img.dimensions.height
        );
    }

    #[test]
    fn test_image_no_dimensions_gets_default() {
        // No width or height — must fall back to ~150px default.
        let html = r#"<img src="x.png">"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree must be built");
        let img = find_image_box(&layout).expect("img node must be found");
        assert!(
            img.dimensions.width > 0.0,
            "image with no dimensions must have non-zero width"
        );
        assert!(
            img.dimensions.height > 0.0,
            "image with no dimensions must have non-zero height"
        );
    }

    // ── CSS Positioned layout tests ───────────────────────────────────────────

    /// Helper: find the first child of `parent` whose `position` matches.
    fn find_child_with_position<'a>(
        layout: &'a LayoutBox<'a>,
        pos: PositionType,
    ) -> Option<&'a LayoutBox<'a>> {
        let mut stack = vec![layout];
        while let Some(node) = stack.pop() {
            if node.position == pos {
                return Some(node);
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
        None
    }

    #[test]
    fn test_position_absolute_top_left() {
        // An absolutely-positioned child with top:10px; left:20px inside a
        // position:relative container (100×100 at origin) should land at (20, 10).
        let html = r#"<div style="position:relative;width:100px;height:100px;">
            <div style="position:absolute;top:10px;left:20px;width:30px;height:30px;"></div>
        </div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");
        let abs_box = find_child_with_position(&layout, PositionType::Absolute)
            .expect("absolute child must exist");
        assert_eq!(
            abs_box.dimensions.x, 20.0,
            "absolute child x should be 20px (left offset from relative container), got {}",
            abs_box.dimensions.x
        );
        assert_eq!(
            abs_box.dimensions.y, 10.0,
            "absolute child y should be 10px (top offset from relative container), got {}",
            abs_box.dimensions.y
        );
    }

    #[test]
    fn test_position_fixed_top_left() {
        // A fixed element with top:0; left:0 should land at viewport origin (0, 0).
        let html =
            r#"<div style="position:fixed;top:0px;left:0px;width:200px;height:50px;"></div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");
        let fixed_box =
            find_child_with_position(&layout, PositionType::Fixed).expect("fixed child must exist");
        assert_eq!(
            fixed_box.dimensions.x, 0.0,
            "fixed element with left:0 must have x=0, got {}",
            fixed_box.dimensions.x
        );
        assert_eq!(
            fixed_box.dimensions.y, 0.0,
            "fixed element with top:0 must have y=0, got {}",
            fixed_box.dimensions.y
        );
    }

    #[test]
    fn test_position_fixed_offset() {
        // A fixed element with top:20px; left:50px should land at (50, 20).
        let html =
            r#"<div style="position:fixed;top:20px;left:50px;width:100px;height:40px;"></div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");
        let fixed_box =
            find_child_with_position(&layout, PositionType::Fixed).expect("fixed child must exist");
        assert_eq!(
            fixed_box.dimensions.x, 50.0,
            "fixed element with left:50px must have x=50, got {}",
            fixed_box.dimensions.x
        );
        assert_eq!(
            fixed_box.dimensions.y, 20.0,
            "fixed element with top:20px must have y=20, got {}",
            fixed_box.dimensions.y
        );
    }

    #[test]
    fn test_position_relative_offset() {
        // A relative element with top:15px; left:10px should be offset from its
        // normal-flow position by those amounts.
        let html = r#"<div style="width:200px;">
            <div style="position:relative;top:15px;left:10px;width:50px;height:20px;"></div>
        </div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");
        let rel_box = find_child_with_position(&layout, PositionType::Relative)
            .expect("relative child must exist");
        // Normal flow would place this at x=0, y=0 (first block child in container).
        // After relative offset: x=10, y=15.
        assert_eq!(
            rel_box.dimensions.x, 10.0,
            "relative element with left:10px should have x=10, got {}",
            rel_box.dimensions.x
        );
        assert_eq!(
            rel_box.dimensions.y, 15.0,
            "relative element with top:15px should have y=15, got {}",
            rel_box.dimensions.y
        );
    }

    #[test]
    fn test_absolute_child_not_in_normal_flow() {
        // Siblings after an absolutely-positioned element should not be pushed
        // down by it — absolute elements are removed from normal flow.
        let html = r#"<div style="position:relative;width:200px;">
            <div style="position:absolute;top:0;left:0;width:50px;height:100px;"></div>
            <div id="sibling" style="width:50px;height:20px;background:red;"></div>
        </div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");

        // Find the normal-flow sibling (non-absolute, non-fixed, non-relative child).
        fn find_static_block<'a>(layout: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
            let mut stack = vec![layout];
            while let Some(node) = stack.pop() {
                if node.position == PositionType::Static
                    && node.display == DisplayType::Block
                    && node.dimensions.height > 0.0
                {
                    return Some(node);
                }
                for child in node.children.iter().rev() {
                    stack.push(child);
                }
            }
            None
        }

        let sibling = find_static_block(&layout).expect("sibling div must exist");
        // The sibling should start at y=0 (not y=100), since the absolute child
        // doesn't occupy space in normal flow.
        assert!(sibling.dimensions.y < 5.0,
            "normal-flow sibling should start at y≈0 (absolute child doesn't push it down), got y={}",
            sibling.dimensions.y);
    }

    // ── Margin collapsing tests ───────────────────────────────────────────────

    /// Case 1: Two adjacent `<p>` elements each with `margin: 16px 0`.
    /// CSS spec requires the gap to be 16px (collapsed), not 32px (summed).
    #[test]
    fn test_margin_collapsing_adjacent_siblings() {
        let html = r#"<div style="width:800px;">
            <p style="margin-top:16px;margin-bottom:16px;height:50px;">A</p>
            <p style="margin-top:16px;margin-bottom:16px;height:50px;">B</p>
        </div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");

        // Navigate html > body > div, then get the two <p> children.
        let outer_div = find_outer_div(&layout).expect("outer div not found");
        let ps: Vec<&LayoutBox> = outer_div
            .children
            .iter()
            .filter(|c| is_block_level(c.display))
            .collect();
        assert_eq!(ps.len(), 2, "expected 2 block children");

        let p1 = ps[0];
        let p2 = ps[1];

        let p1_bottom = p1.dimensions.y + p1.dimensions.height; // content bottom of p1
        let gap = p2.dimensions.y - p1_bottom;
        assert_eq!(gap, 16.0,
            "adjacent <p> margins should collapse to 16px gap, got {}px (p1.y={}, p1.h={}, p2.y={})",
            gap, p1.dimensions.y, p1.dimensions.height, p2.dimensions.y);
    }

    /// Case 1b: Asymmetric adjacent margins collapse to the larger value.
    /// `<div margin-bottom:32px>` followed by `<div margin-top:16px>` → 32px gap.
    #[test]
    fn test_margin_collapsing_asymmetric() {
        let html = r#"<div style="width:800px;">
            <div style="margin-bottom:32px;height:50px;">A</div>
            <div style="margin-top:16px;height:50px;">B</div>
        </div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");

        let outer_div = find_outer_div(&layout).expect("outer div not found");
        let blocks: Vec<&LayoutBox> = outer_div
            .children
            .iter()
            .filter(|c| is_block_level(c.display))
            .collect();
        assert_eq!(blocks.len(), 2, "expected 2 block children");

        let b1 = blocks[0];
        let b2 = blocks[1];
        let gap = b2.dimensions.y - (b1.dimensions.y + b1.dimensions.height);
        assert_eq!(
            gap, 32.0,
            "asymmetric margins should collapse to max(32,16)=32px, got {}px",
            gap
        );
    }

    /// Case 2 (top): First block child inside a padding-less parent.
    /// The child's top margin should collapse with the parent's — no internal
    /// space between the parent's content edge and the child's content edge.
    #[test]
    fn test_margin_collapsing_parent_first_child_top() {
        // Container has no padding or border; h1 has margin-top:32px.
        // The h1 should start at y=0 (same as the container's content top).
        let html = r#"<div style="width:800px;margin:0;padding:0;">
            <h1 style="margin-top:32px;height:40px;">Hello</h1>
        </div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");

        let outer_div = find_outer_div(&layout).expect("outer div not found");
        let h1 = outer_div
            .children
            .iter()
            .find(|c| is_block_level(c.display))
            .expect("h1 block child");

        // h1 content box should start at the parent's content top (no internal margin gap).
        assert_eq!(
            h1.dimensions.y, outer_div.dimensions.y,
            "first child margin should collapse into parent (no internal gap): h1.y={}, div.y={}",
            h1.dimensions.y, outer_div.dimensions.y
        );
    }

    /// Case 2 (bottom): Last block child inside a padding-less parent.
    /// The child's bottom margin should collapse with the parent's — no extra
    /// space added at the bottom of the parent's content area.
    #[test]
    fn test_margin_collapsing_parent_last_child_bottom() {
        // Container with no bottom padding/border; inner div has margin-bottom:24px.
        // Parent's content height should equal the child's height (24px margin not added inside).
        let html = r#"<div style="width:800px;margin:0;padding:0;">
            <div style="height:60px;margin-bottom:24px;">Inner</div>
        </div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");

        let outer_div = find_outer_div(&layout).expect("outer div not found");
        // Parent height should be 60px (the child height), not 84px (60 + 24 margin).
        assert_eq!(outer_div.dimensions.height, 60.0,
            "last child bottom margin should collapse into parent (height should be 60, not 84): got {}",
            outer_div.dimensions.height);
    }

    #[test]
    fn test_bootstrap_navbar_expand_lg_stays_horizontal() {
        let html = r#"
            <nav class="navbar navbar-expand-lg navbar-dark bg-dark shadow-sm">
                <div class="container-fluid">
                    <a class="navbar-brand" href="/">Yunseong</a>
                    <div class="collapse navbar-collapse" id="navbarNav">
                        <ul class="navbar-nav w-100">
                            <li class="nav-item"><a class="nav-link active" href="/">Home</a></li>
                            <li class="nav-item"><a class="nav-link active" href="/blog">Blog</a></li>
                            <li class="nav-item"><a class="nav-link active" href="/projects">Projects</a></li>
                            <li class="nav-item"><a class="nav-link active" href="/apps">Mini Apps</a></li>
                            <li class="nav-item"><a class="nav-link active" href="/chat">Curator</a></li>
                            <li class="nav-item ms-auto"><a class="nav-link" href="/login">Login</a></li>
                        </ul>
                    </div>
                </div>
            </nav>
        "#;
        let css = r#"
            .navbar { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; padding: 8px 16px; }
            .container-fluid { display: flex; flex-wrap: inherit; align-items: center; justify-content: space-between; width: 100%; }
            .navbar-brand { padding-top: 5px; padding-bottom: 5px; margin-right: 16px; font-size: 20px; }
            .navbar-nav { display: flex; flex-direction: column; padding-left: 0; margin-bottom: 0; }
            .navbar-collapse { flex-basis: 100%; flex-grow: 1; align-items: center; }
            .nav-link { display: block; padding: 8px; }
            .w-100 { width: 100%; }
            .ms-auto { margin-left: auto; }
            @media (min-width: 600px) {
                .navbar-expand-lg .navbar-nav { flex-direction: row; }
                .navbar-expand-lg .navbar-collapse { display: flex; flex-basis: auto; }
            }
        "#;

        let dom_tree = dom::parse_html(html);
        let ss = css::parse_css(css);
        let style_tree = style::build_style_tree(
            &dom_tree.document,
            &ss,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout");

        let ul = find_element_by_tag(&layout, "ul").expect("ul not found");
        assert!(
            ul.dimensions.width > 500.0,
            "navbar ul should expand close to full row width, got {}",
            ul.dimensions.width
        );

        let items: Vec<&LayoutBox> = ul
            .children
            .iter()
            .filter(|c| matches!(c.style_node.node.data, NodeData::Element { .. }))
            .collect();
        assert_eq!(items.len(), 6, "expected 6 nav items");

        let y0 = items[0].dimensions.y;
        let mut prev_right = items[0].dimensions.x + items[0].dimensions.width;
        for (idx, item) in items.iter().enumerate().skip(1) {
            assert!(
                (item.dimensions.y - y0).abs() < 1.0,
                "nav item {} should stay on the same row: y0={}, y={}",
                idx,
                y0,
                item.dimensions.y
            );
            assert!(
                item.dimensions.x >= prev_right - 1.0,
                "nav item {} should not overlap the previous item: prev_right={}, x={}",
                idx,
                prev_right,
                item.dimensions.x
            );
            prev_right = item.dimensions.x + item.dimensions.width;
        }

        assert!(items[5].dimensions.x > items[0].dimensions.x + 250.0,
            "login item should remain on the same horizontal navbar row, not collapse into the left cluster: x1={}, x6={}",
            items[0].dimensions.x, items[5].dimensions.x);
    }

    #[test]
    fn test_absolute_child_in_flex_container_is_out_of_flow() {
        // An absolute child inside a flex row should NOT participate in flex layout.
        // The two normal-flow flex items should be placed side-by-side; the absolute
        // child should appear at the top-left of the flex container (its CB), not
        // between or after the flex items.
        let html = r#"<div style="display:flex;flex-direction:row;position:relative;width:400px;height:50px;">
            <span style="width:80px;height:50px;">A</span>
            <span style="position:absolute;top:0;left:0;width:40px;height:40px;">B</span>
            <span style="width:80px;height:50px;">C</span>
        </div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");

        let flex_div = find_element_by_tag(&layout, "div").expect("flex div");

        // Find the absolute child.
        let abs_child = find_child_with_position(flex_div, PositionType::Absolute)
            .expect("absolute child in flex container");
        // It should be positioned at top-left of the flex container (0, 0) because
        // left:0; top:0 relative to the positioned flex container.
        assert!(
            abs_child.dimensions.x < 5.0,
            "absolute child in flex should be at left:0 of its CB, got x={}",
            abs_child.dimensions.x
        );
        assert!(
            abs_child.dimensions.y < 5.0,
            "absolute child in flex should be at top:0 of its CB, got y={}",
            abs_child.dimensions.y
        );

        // The two normal-flow flex items should both be present and at y≈0.
        let normal_flow_items: Vec<&LayoutBox> = flex_div
            .children
            .iter()
            .filter(|c| c.position == PositionType::Static)
            .collect();
        assert_eq!(
            normal_flow_items.len(),
            2,
            "flex container should have 2 normal-flow items (A and C), got {}",
            normal_flow_items.len()
        );
        // Both items should be on the same row (y ≈ same).
        let y0 = normal_flow_items[0].dimensions.y;
        assert!(
            (normal_flow_items[1].dimensions.y - y0).abs() < 2.0,
            "both flex items should be on the same row, got y0={} y1={}",
            y0,
            normal_flow_items[1].dimensions.y
        );
        // Second item should be to the right of the first.
        assert!(
            normal_flow_items[1].dimensions.x > normal_flow_items[0].dimensions.x,
            "C should be to the right of A in flex row"
        );
    }

    #[test]
    fn test_position_fixed_right_zero_anchors_to_viewport_right() {
        // A fixed element with right:0 should land so its right edge equals the viewport right.
        // viewport width = 800, element width = 120 → x = 800 - 0 - 120 = 680.
        let html = r#"<div style="position:fixed;top:0;right:0;width:120px;height:40px;"></div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");
        let fixed_box =
            find_child_with_position(&layout, PositionType::Fixed).expect("fixed element");
        let expected_x = 800.0 - 120.0; // right:0, no margin
        assert!(
            (fixed_box.dimensions.x - expected_x).abs() < 2.0,
            "fixed element with right:0 and width:120px should have x≈{}, got {}",
            expected_x,
            fixed_box.dimensions.x
        );
    }

    #[test]
    fn test_inset_shorthand_expands_to_trbl() {
        // inset: 10px should set top/right/bottom/left all to 10px.
        let html = r#"<div style="position:absolute;inset:10px;width:50px;height:30px;"></div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");
        let abs_box =
            find_child_with_position(&layout, PositionType::Absolute).expect("absolute element");
        // With inset:10px the element is offset 10px from viewport origin.
        // top:10px → y = 10; left:10px → x = 10.
        assert!(
            (abs_box.dimensions.x - 10.0).abs() < 2.0,
            "inset:10px should place element at x≈10, got {}",
            abs_box.dimensions.x
        );
        assert!(
            (abs_box.dimensions.y - 10.0).abs() < 2.0,
            "inset:10px should place element at y≈10, got {}",
            abs_box.dimensions.y
        );
    }

    // ── Issue #112: Google fidelity fixes ────────────────────────────────────

    /// `<input type="hidden">` must not produce a visible layout box.
    #[test]
    fn test_hidden_input_not_rendered() {
        let html = r#"<form><input type="hidden" name="hl" value="en"><input type="text" name="q"></form>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");

        // Walk the tree: collect all Input display boxes.
        fn collect_inputs<'a>(b: &'a LayoutBox<'a>, out: &mut Vec<&'a LayoutBox<'a>>) {
            if b.display == DisplayType::Input {
                if let NodeData::Element { ref attrs, .. } = b.style_node.node.data {
                    out.push(b);
                }
            }
            for c in &b.children {
                collect_inputs(c, out);
            }
        }
        let mut inputs = Vec::new();
        collect_inputs(&layout, &mut inputs);

        // Only the text input should appear; the hidden input must be absent.
        assert_eq!(inputs.len(), 1, "only 1 visible input expected (the text one), got {}", inputs.len());
    }

    /// `<center>` must render as a block and center its inline content.
    #[test]
    fn test_center_tag_produces_block() {
        let html = r#"<center>Hello</center>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");

        let center_box = find_element_by_tag(&layout, "center").expect("center element in tree");
        // Must be block-level (fills the container).
        assert_eq!(
            center_box.display,
            DisplayType::Block,
            "<center> should have DisplayType::Block, got {:?}",
            center_box.display
        );
    }

    /// `text-align: center` must shift inline children toward the horizontal midpoint.
    #[test]
    fn test_text_align_center_shifts_inline_content() {
        // A 800px container with text-align:center containing a short text span.
        let html = r#"<div style="width:800px; text-align:center;"><span>Hi</span></div>"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");

        let div = find_element_by_tag(&layout, "div").expect("div");
        // The span (or text node inside it) should be positioned past 200px
        // (i.e., not at x=0 as left-aligned would be).
        let first_child_x = div.children.first().map(|c| c.dimensions.x).unwrap_or(0.0);
        assert!(
            first_child_x > 100.0,
            "text-align:center should shift content to roughly midpoint; got x={}",
            first_child_x
        );
    }

    #[test]
    fn test_centered_inline_links_keep_intrinsic_width() {
        let html = r#"
            <center>
                <p style="font-size:8pt;color:#636363">
                    &copy; 2026 -
                    <a id="privacy" href="/privacy">개인정보처리방침</a>
                    -
                    <a id="terms" href="/terms">약관</a>
                </p>
            </center>
        "#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");

        let copyright = find_text_box_containing(&layout, "2026").expect("copyright text");
        let privacy = find_element_by_id(&layout, "privacy").expect("privacy link");
        let terms = find_element_by_id(&layout, "terms").expect("terms link");

        assert!(
            privacy.dimensions.width < 220.0,
            "center-inherited inline link should keep intrinsic width, got {}",
            privacy.dimensions.width
        );
        assert!(
            (privacy.dimensions.y - copyright.dimensions.y).abs() < 2.0,
            "privacy link should stay grouped on the same policy line: copyright.y={}, privacy.y={}",
            copyright.dimensions.y,
            privacy.dimensions.y
        );
        assert!(
            (terms.dimensions.y - privacy.dimensions.y).abs() < 2.0,
            "terms link should stay on the same policy line as privacy: privacy.y={}, terms.y={}",
            privacy.dimensions.y,
            terms.dimensions.y
        );
        assert!(
            terms.dimensions.x > privacy.dimensions.x,
            "terms link should remain to the right of privacy on the shared line"
        );
    }

    #[test]
    fn test_inline_utility_link_after_centered_controls_stays_grouped() {
        let html = r#"
            <center>
                <form style="margin-top: 80px;">
                    <table cellpadding="0" cellspacing="0">
                        <tr valign="top">
                            <td id="left-cell" width="25%">&nbsp;</td>
                            <td id="controls-cell" align="center" nowrap="">
                                <input id="search" style="width: 496px; height: 25px;">
                                <br>
                                <input id="primary" type="submit" value="Google Search" style="width: 160px; height: 30px;">
                                <input id="secondary" type="submit" value="I'm Feeling Lucky" style="width: 160px; height: 30px;">
                            </td>
                            <td id="utility-cell" class="fl sblc" align="left" nowrap="" width="25%">
                                <a id="advanced" href="/advanced_search">고급검색</a>
                            </td>
                        </tr>
                    </table>
                </form>
            </center>
        "#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style_tree =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) =
            build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree");

        let search = find_element_by_id(&layout, "search").expect("search input");
        let primary = find_element_by_id(&layout, "primary").expect("primary button");
        let secondary = find_element_by_id(&layout, "secondary").expect("secondary button");
        let advanced = find_element_by_id(&layout, "advanced").expect("advanced link");

        let search_center = search.dimensions.x + search.dimensions.width / 2.0;
        let button_cluster_center =
            (primary.dimensions.x + secondary.dimensions.x + secondary.dimensions.width) / 2.0;
        let advanced_center = advanced.dimensions.x + advanced.dimensions.width / 2.0;

        assert!(
            (advanced_center - search_center).abs() < 340.0,
            "utility link should stay near the centered search input: link_center={}, search_center={}",
            advanced_center,
            search_center
        );
        assert!(
            (advanced_center - button_cluster_center).abs() < 340.0,
            "utility link should stay grouped with action buttons: link_center={}, buttons_center={}",
            advanced_center,
            button_cluster_center
        );
        assert!(
            advanced.dimensions.x > 500.0,
            "utility link should not escape to the far-left edge, got x={}",
            advanced.dimensions.x
        );
        assert!(
            advanced.dimensions.x + advanced.dimensions.width < 800.0,
            "utility link should remain visible inside the viewport, got right edge {}",
            advanced.dimensions.x + advanced.dimensions.width
        );
    }

    /// Adjacent inline text runs separated only by whitespace-only text nodes must
    /// keep a visible gap between them.  This reproduces the Google footer bug where
    /// `© 2026` ran directly into `개인정보처리방침약관` with no space between them.
    #[test]
    fn test_inline_whitespace_text_node_creates_space_between_links() {
        let html = r##"<!DOCTYPE html>
<html><body>
<div style="width:800px">
  <span id="copy">&#169; 2026</span>
  <a id="link1" href="#">Privacy</a>
  <a id="link2" href="#">Terms</a>
</div>
</body></html>"##;

        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let copy = find_element_by_id(&layout, "copy").expect("copyright span");
        let link1 = find_element_by_id(&layout, "link1").expect("first link");
        let link2 = find_element_by_id(&layout, "link2").expect("second link");

        let copy_right_edge = copy.dimensions.x + copy.dimensions.width;
        let link1_left_edge = link1.dimensions.x;
        let link1_right_edge = link1.dimensions.x + link1.dimensions.width;
        let link2_left_edge = link2.dimensions.x;

        assert!(
            link1_left_edge > copy_right_edge,
            "link1 must not overlap with copyright span: copy_right={}, link1_left={}",
            copy_right_edge,
            link1_left_edge
        );
        assert!(
            link2_left_edge > link1_right_edge,
            "link2 must not overlap with link1: link1_right={}, link2_left={}",
            link1_right_edge,
            link2_left_edge
        );
        assert!(
            (copy.dimensions.y - link1.dimensions.y).abs() < 2.0,
            "copy and link1 should be on the same line: copy.y={}, link1.y={}",
            copy.dimensions.y,
            link1.dimensions.y
        );
        assert!(
            (link1.dimensions.y - link2.dimensions.y).abs() < 2.0,
            "link1 and link2 should be on the same line: link1.y={}, link2.y={}",
            link1.dimensions.y,
            link2.dimensions.y
        );
    }

    /// A text node that starts with whitespace and is preceded by a non-empty
    /// inline element must preserve a leading inter-element space.
    ///
    /// The leading space is included *inside* the span's bounding box — the span
    /// element itself starts at link1's right edge, but the visible content (`·`)
    /// is offset inward by one space width.  So we verify:
    ///   - sep starts at or after link1's right edge (no backward overlap)
    ///   - sep has positive width (the space and text content are accounted for)
    #[test]
    fn test_inline_text_node_with_leading_space_preserves_gap() {
        let html = r##"<!DOCTYPE html>
<html><body>
<div style="width:800px">
  <a id="link1" href="#">Privacy</a><span id="sep"> · Terms</span>
</div>
</body></html>"##;

        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let link1 = find_element_by_id(&layout, "link1").expect("first link");
        let sep = find_element_by_id(&layout, "sep").expect("separator span");

        let link1_right = link1.dimensions.x + link1.dimensions.width;

        // The separator span starts immediately after link1 — the leading space is
        // part of the span's own content and is reflected in its width, not in its x offset.
        assert!(
            sep.dimensions.x >= link1_right - 0.5,
            "sep must not start before link1's right edge: link1_right={}, sep.x={}",
            link1_right,
            sep.dimensions.x
        );
        assert!(
            sep.dimensions.width > 0.0,
            "sep must have positive width (space + text): width={}",
            sep.dimensions.width
        );
    }

    // ── Flexbox row layout tests (issue #142) ─────────────────────────────────

    /// `display:flex` children must lay out in a row (left-to-right) instead of
    /// stacking vertically like block children.
    #[test]
    fn test_flex_row_children_are_placed_horizontally() {
        let html = r#"<div style="display:flex;flex-direction:row;width:300px;height:50px;">
            <div id="a" style="width:80px;height:50px;">A</div>
            <div id="b" style="width:80px;height:50px;">B</div>
            <div id="c" style="width:80px;height:50px;">C</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("child A");
        let b = find_element_by_id(&layout, "b").expect("child B");
        let c = find_element_by_id(&layout, "c").expect("child C");

        // All children should be on the same row (same y coordinate).
        assert!(
            (a.dimensions.y - b.dimensions.y).abs() < 2.0,
            "A and B should be on the same row: a.y={}, b.y={}",
            a.dimensions.y, b.dimensions.y
        );
        assert!(
            (b.dimensions.y - c.dimensions.y).abs() < 2.0,
            "B and C should be on the same row: b.y={}, c.y={}",
            b.dimensions.y, c.dimensions.y
        );
        // Children should be ordered left-to-right.
        assert!(
            a.dimensions.x < b.dimensions.x,
            "A should be to the left of B: a.x={}, b.x={}",
            a.dimensions.x, b.dimensions.x
        );
        assert!(
            b.dimensions.x < c.dimensions.x,
            "B should be to the left of C: b.x={}, c.x={}",
            b.dimensions.x, c.dimensions.x
        );
    }

    /// `justify-content: center` must center flex children horizontally within
    /// the flex container.
    #[test]
    fn test_flex_justify_content_center() {
        let html = r#"<div style="display:flex;justify-content:center;width:400px;height:50px;">
            <div id="a" style="width:80px;height:50px;">A</div>
            <div id="b" style="width:80px;height:50px;">B</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("child A");
        let b = find_element_by_id(&layout, "b").expect("child B");

        // Total child width = 80 + 80 = 160px; container = 400px; free = 240px.
        // With justify-content:center, the left offset should be ~120px.
        assert!(
            a.dimensions.x > 80.0,
            "justify-content:center should offset children from the left: a.x={}",
            a.dimensions.x
        );
        // Right edge of last child should not reach the container's right edge.
        let b_right = b.dimensions.x + b.dimensions.width;
        assert!(
            b_right < 380.0,
            "justify-content:center should leave space on the right: b_right={}",
            b_right
        );
        // The gap on the left and right should be roughly equal (±10px tolerance).
        let flex_div = find_element_by_tag(&layout, "div").expect("flex container");
        let left_gap = a.dimensions.x - flex_div.dimensions.x;
        let right_gap = (flex_div.dimensions.x + flex_div.dimensions.width) - b_right;
        assert!(
            (left_gap - right_gap).abs() < 10.0,
            "left and right gaps should be roughly equal: left={}, right={}",
            left_gap, right_gap
        );
    }

    /// `align-items: center` must center flex children vertically within the
    /// cross axis of the flex container.
    #[test]
    fn test_flex_align_items_center() {
        let html = r#"<div style="display:flex;align-items:center;width:300px;height:100px;">
            <div id="a" style="width:80px;height:30px;">A</div>
            <div id="b" style="width:80px;height:60px;">B</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let flex_div = find_element_by_tag(&layout, "div").expect("flex container");
        let a = find_element_by_id(&layout, "a").expect("child A");
        let b = find_element_by_id(&layout, "b").expect("child B");

        let container_top = flex_div.dimensions.y;
        let container_height = flex_div.dimensions.height;

        // Child A (30px tall) should be vertically centered in the line height (60px max).
        // Expected center offset: (60 - 30) / 2 = 15px from the container top.
        let a_center = a.dimensions.y + a.dimensions.height / 2.0;
        let container_center = container_top + container_height / 2.0;
        assert!(
            (a_center - container_center).abs() < 10.0,
            "align-items:center should center child A vertically: a_center={}, container_center={}",
            a_center, container_center
        );

        // Child B (60px tall) should also be centered (which means it starts near container top).
        let b_center = b.dimensions.y + b.dimensions.height / 2.0;
        assert!(
            (b_center - container_center).abs() < 10.0,
            "align-items:center should center child B vertically: b_center={}, container_center={}",
            b_center, container_center
        );
    }

    /// `flex: 1` shorthand sets flex-grow:1 so that children with `flex:1` each
    /// receive an equal share of the remaining space in the flex container.
    #[test]
    fn test_flex_one_distributes_space_equally() {
        let html = r#"<div style="display:flex;width:300px;height:50px;">
            <div id="a" style="flex:1;">A</div>
            <div id="b" style="flex:1;">B</div>
            <div id="c" style="flex:1;">C</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("child A");
        let b = find_element_by_id(&layout, "b").expect("child B");
        let c = find_element_by_id(&layout, "c").expect("child C");

        // Each child should get ~100px (300px / 3).
        assert!(
            (a.dimensions.width - 100.0).abs() < 5.0,
            "flex:1 child A should get ~100px, got {}",
            a.dimensions.width
        );
        assert!(
            (b.dimensions.width - 100.0).abs() < 5.0,
            "flex:1 child B should get ~100px, got {}",
            b.dimensions.width
        );
        assert!(
            (c.dimensions.width - 100.0).abs() < 5.0,
            "flex:1 child C should get ~100px, got {}",
            c.dimensions.width
        );
        // All three children should be on the same row.
        assert!(
            (a.dimensions.y - b.dimensions.y).abs() < 2.0,
            "all flex:1 children should be on the same row"
        );
    }

    #[test]
    fn test_flex_basis_sets_initial_main_size() {
        let html = r#"<div style="display:flex;width:300px;height:50px;">
            <div id="a" style="flex:0 0 120px;">A</div>
            <div id="b" style="flex:0 0 80px;">B</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("child A");
        let b = find_element_by_id(&layout, "b").expect("child B");

        assert!(
            (a.dimensions.width - 120.0).abs() < 2.0,
            "flex-basis should drive child A width, got {}",
            a.dimensions.width
        );
        assert!(
            (b.dimensions.width - 80.0).abs() < 2.0,
            "flex-basis should drive child B width, got {}",
            b.dimensions.width
        );
        assert!(
            b.dimensions.x >= a.dimensions.x + a.dimensions.width - 1.0,
            "child B should be placed after child A in the row: a_right={}, b.x={}",
            a.dimensions.x + a.dimensions.width,
            b.dimensions.x
        );
    }

    // ── List marker tests ─────────────────────────────────────────────────────

    /// `<ul><li>` elements must have a disc marker (•) and `DisplayType::ListItem`.
    #[test]
    fn test_ul_li_has_disc_marker() {
        let html = r#"<ul><li id="a">Item A</li><li id="b">Item B</li></ul>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let item_a = find_element_by_id(&layout, "a").expect("li#a not found");
        let item_b = find_element_by_id(&layout, "b").expect("li#b not found");

        assert_eq!(item_a.display, DisplayType::ListItem, "li must be ListItem");
        assert_eq!(
            item_a.list_marker.as_deref(),
            Some("\u{2022}"),
            "ul li should have disc marker •"
        );
        assert_eq!(
            item_b.list_marker.as_deref(),
            Some("\u{2022}"),
            "second ul li should also have disc marker"
        );
    }

    /// `<ol><li>` elements must have decimal markers (1., 2., …).
    #[test]
    fn test_ol_li_has_decimal_marker() {
        let html = r#"<ol><li id="a">First</li><li id="b">Second</li><li id="c">Third</li></ol>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let item_a = find_element_by_id(&layout, "a").expect("li#a not found");
        let item_b = find_element_by_id(&layout, "b").expect("li#b not found");
        let item_c = find_element_by_id(&layout, "c").expect("li#c not found");

        assert_eq!(
            item_a.list_marker.as_deref(),
            Some("1."),
            "first ol li should have marker 1."
        );
        assert_eq!(
            item_b.list_marker.as_deref(),
            Some("2."),
            "second ol li should have marker 2."
        );
        assert_eq!(
            item_c.list_marker.as_deref(),
            Some("3."),
            "third ol li should have marker 3."
        );
    }

    /// `list-style-type: none` suppresses the marker.
    #[test]
    fn test_list_style_type_none_suppresses_marker() {
        let html = r#"<ul style="list-style-type:none"><li id="x">No marker</li></ul>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let item = find_element_by_id(&layout, "x").expect("li#x not found");
        assert!(
            item.list_marker.is_none(),
            "list-style-type:none should suppress marker, got {:?}",
            item.list_marker
        );
    }

    /// List items must have left padding so content is indented away from the marker.
    #[test]
    fn test_ul_has_default_left_padding() {
        let html = r#"<ul id="list"><li>Item</li></ul>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let list = find_element_by_id(&layout, "list").expect("ul not found");
        assert!(
            list.padding.left >= 30.0,
            "ul should have padding-left >= 30px for indentation, got {}",
            list.padding.left
        );
    }

    /// Google header cluster: a `.gb_Xd` flex container (display:flex, align-items:center)
    /// must lay out its children (Gmail link and image link) side by side on a single row.
    #[test]
    fn test_flex_google_header_cluster_gmail_and_images_on_same_row() {
        let html = r#"<div style="display:flex;align-items:center;height:48px;">
            <a id="gmail" href="https://mail.google.com">Gmail</a>
            <a id="images" href="/imghp">Images</a>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let gmail = find_element_by_id(&layout, "gmail").expect("Gmail link");
        let images = find_element_by_id(&layout, "images").expect("Images link");

        // Both links must be on the same horizontal row (same y ± 2px).
        assert!(
            (gmail.dimensions.y - images.dimensions.y).abs() < 2.0,
            "Gmail and Images must be on the same row: gmail.y={}, images.y={}",
            gmail.dimensions.y, images.dimensions.y
        );
        // Images link must be to the right of Gmail.
        assert!(
            images.dimensions.x > gmail.dimensions.x,
            "Images must be to the right of Gmail: gmail.x={}, images.x={}",
            gmail.dimensions.x, images.dimensions.x
        );
    }

    // ── Grid layout tests ─────────────────────────────────────────────────────

    /// `display: grid; grid-template-columns: 1fr 1fr` → two equal columns side by side.
    #[test]
    fn test_grid_two_equal_fr_columns() {
        let html = r#"<div id="grid">
            <div id="a">A</div>
            <div id="b">B</div>
        </div>"#;
        let css_src = "#grid { display: grid; grid-template-columns: 1fr 1fr; }";
        let (layout, _, _) = layout_from_html_css(html, css_src, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("cell A");
        let b = find_element_by_id(&layout, "b").expect("cell B");

        // Both children should be on the same row (same y).
        assert!(
            (a.dimensions.y - b.dimensions.y).abs() < 2.0,
            "A and B should be on the same row: a.y={}, b.y={}",
            a.dimensions.y, b.dimensions.y
        );
        // A should be to the left of B.
        assert!(
            a.dimensions.x < b.dimensions.x,
            "A should be to the left of B: a.x={}, b.x={}",
            a.dimensions.x, b.dimensions.x
        );
        // Each column should be roughly half the container width (400px in an 800px viewport).
        assert!(
            (a.dimensions.width - 400.0).abs() < 5.0,
            "1fr column A should be ~400px wide, got {}",
            a.dimensions.width
        );
        assert!(
            (b.dimensions.width - 400.0).abs() < 5.0,
            "1fr column B should be ~400px wide, got {}",
            b.dimensions.width
        );
    }

    /// `gap: 16px` inserts spacing between grid cells.
    #[test]
    fn test_grid_gap_spacing() {
        let html = r#"<div id="grid">
            <div id="a">A</div>
            <div id="b">B</div>
        </div>"#;
        let css_src = "#grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }";
        let (layout, _, _) = layout_from_html_css(html, css_src, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("cell A");
        let b = find_element_by_id(&layout, "b").expect("cell B");

        // B's left edge should be at least 16px to the right of A's right edge.
        let a_right = a.dimensions.x + a.dimensions.width;
        let gap = b.dimensions.x - a_right;
        assert!(
            gap >= 15.0,
            "gap between cells should be >= 16px, got {}",
            gap
        );
    }

    /// `grid-template-columns: 200px 1fr` → fixed + flexible column.
    #[test]
    fn test_grid_fixed_plus_fr_column() {
        let html = r#"<div id="grid">
            <div id="a">A</div>
            <div id="b">B</div>
        </div>"#;
        let css_src = "#grid { display: grid; grid-template-columns: 200px 1fr; }";
        let (layout, _, _) = layout_from_html_css(html, css_src, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("cell A");
        let b = find_element_by_id(&layout, "b").expect("cell B");

        // First column should be exactly 200px.
        assert!(
            (a.dimensions.width - 200.0).abs() < 2.0,
            "fixed column A should be 200px wide, got {}",
            a.dimensions.width
        );
        // Second column fills the rest (~600px in an 800px container).
        assert!(
            b.dimensions.width > 500.0,
            "fr column B should fill remaining space (>500px), got {}",
            b.dimensions.width
        );
        // A and B should be on the same row.
        assert!(
            (a.dimensions.y - b.dimensions.y).abs() < 2.0,
            "A and B should be on the same row: a.y={}, b.y={}",
            a.dimensions.y, b.dimensions.y
        );
    }

    /// `grid-template-columns: repeat(3, 1fr)` → three equal columns.
    #[test]
    fn test_grid_repeat_three_fr_columns() {
        let html = r#"<div id="grid">
            <div id="a">A</div>
            <div id="b">B</div>
            <div id="c">C</div>
        </div>"#;
        let css_src = "#grid { display: grid; grid-template-columns: repeat(3, 1fr); }";
        let (layout, _, _) = layout_from_html_css(html, css_src, 900.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("cell A");
        let b = find_element_by_id(&layout, "b").expect("cell B");
        let c = find_element_by_id(&layout, "c").expect("cell C");

        // All three should be on the same row (y within 2px).
        assert!(
            (a.dimensions.y - b.dimensions.y).abs() < 2.0 &&
            (b.dimensions.y - c.dimensions.y).abs() < 2.0,
            "A, B, C should be on the same row"
        );
        // Each column should be ~300px (900 / 3).
        assert!(
            (a.dimensions.width - 300.0).abs() < 5.0,
            "repeat(3, 1fr) column A should be ~300px, got {}",
            a.dimensions.width
        );
        assert!(
            (b.dimensions.width - 300.0).abs() < 5.0,
            "repeat(3, 1fr) column B should be ~300px, got {}",
            b.dimensions.width
        );
        assert!(
            (c.dimensions.width - 300.0).abs() < 5.0,
            "repeat(3, 1fr) column C should be ~300px, got {}",
            c.dimensions.width
        );
        // Children should be ordered left-to-right.
        assert!(a.dimensions.x < b.dimensions.x && b.dimensions.x < c.dimensions.x,
            "A, B, C should be ordered left-to-right");
    }

    /// Grid wraps children into multiple rows when more items than columns.
    #[test]
    fn test_grid_auto_rows_wrap() {
        let html = r#"<div id="grid">
            <div id="a">A</div>
            <div id="b">B</div>
            <div id="c">C</div>
            <div id="d">D</div>
        </div>"#;
        let css_src = "#grid { display: grid; grid-template-columns: 1fr 1fr; }";
        let (layout, _, _) = layout_from_html_css(html, css_src, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("cell A");
        let b = find_element_by_id(&layout, "b").expect("cell B");
        let c = find_element_by_id(&layout, "c").expect("cell C");
        let d = find_element_by_id(&layout, "d").expect("cell D");

        // Row 1: A and B at the same y.
        assert!(
            (a.dimensions.y - b.dimensions.y).abs() < 2.0,
            "A and B should be in row 1: a.y={}, b.y={}",
            a.dimensions.y, b.dimensions.y
        );
        // Row 2: C and D at the same y, below row 1.
        assert!(
            (c.dimensions.y - d.dimensions.y).abs() < 2.0,
            "C and D should be in row 2: c.y={}, d.y={}",
            c.dimensions.y, d.dimensions.y
        );
        assert!(
            c.dimensions.y > a.dimensions.y + 1.0,
            "Row 2 should be below row 1: c.y={}, a.y={}",
            c.dimensions.y, a.dimensions.y
        );
    }
}
