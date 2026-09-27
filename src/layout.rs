//! Layout: turns the styled tree into positioned boxes.
//!
//! Coordinate contract: every `LayoutBox::dimensions` is the **border box** in
//! absolute page coordinates (x, y, width, height). `padding`, `border` and
//! `margin` hold the resolved edge sizes; `get_content_rect()` derives the
//! content box. Text is laid out into per-line fragments: each fragment is a
//! `LayoutBox` whose `style_node` is the text node and whose `text_fragment`
//! holds the text shown on that line; its rect is the glyph content area
//! (baseline − ascent … baseline + descent).
//!
//! Supported formatting: block flow with margin collapsing, floats and
//! clearance, inline formatting (line breaking at spaces and between CJK
//! characters, `white-space`, `vertical-align`, `text-align`, `text-indent`,
//! `text-overflow: ellipsis`, `-webkit-line-clamp`), inline-block and other
//! atomic inlines, shrink-to-fit sizing, flexbox, a simple grid, a simple
//! automatic table layout, and relative/absolute/fixed positioning.

use crate::css::{Unit, Value};
use crate::style::StyledNode;
use markup5ever_rcdom::NodeData;
use std::collections::HashMap;
extern crate stacker;

/// Rounded ascent / descent of the node's primary font in px (shared with the painter).
fn font_metrics(sn: &StyledNode, fs: f32) -> crate::fonts::LineMetrics {
    crate::fonts::chain_for(&sn.specified_values).metrics(fs)
}

/// Snap a coordinate to 1/64 px (the precision browsers lay out with).
fn snap(v: f32) -> f32 {
    (v * 64.0).round() / 64.0
}

/// Horizontal advance of `c` in px for the node's font (same as the painter).
fn char_advance(sn: &StyledNode, c: char, font_size: f32) -> f32 {
    crate::fonts::chain_for(&sn.specified_values).advance(c, font_size)
}

/// Width of `text` at `font_size` in the node's font with `letter_spacing`
/// added after every character (same as the painter).
fn text_width(sn: &StyledNode, text: &str, font_size: f32, letter_spacing: f32) -> f32 {
    crate::fonts::chain_for(&sn.specified_values).measure_spaced(text, font_size, letter_spacing)
}

/// Measure the width of `text` rendered at `font_size` px.
/// When `wrap_width` is `f32::INFINITY`, no wrapping occurs (max-content).
/// When finite, line-breaks at word boundaries and reports the widest line.
#[allow(dead_code)]
fn measure_text_width(sn: &StyledNode, text: &str, font_size: f32, wrap_width: f32) -> f32 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    let space_w = char_advance(sn, ' ', font_size);
    let mut max_w: f32 = 0.0;
    let mut line_w: f32 = 0.0;
    for word in trimmed.split_whitespace() {
        let word_w = text_width(sn, word, font_size, 0.0);
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

impl EdgeSizes {
    fn horizontal(&self) -> f32 {
        self.left + self.right
    }
    fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Debug)]
pub struct LayoutBox<'a> {
    /// Border box in absolute coordinates.
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
    /// For text boxes: the (whitespace-processed) text shown by this line
    /// fragment. `None` for element boxes.
    pub text_fragment: Option<String>,
    /// Placeholder for an absolutely/fixed positioned box whose containing block
    /// has not finished layout yet. Its rect origin is the static position.
    abs_placeholder: bool,
}

impl<'a> Clone for LayoutBox<'a> {
    /// Iterative clone to avoid stack overflows on deeply nested layout trees.
    fn clone(&self) -> Self {
        enum Frame<'f> {
            Pre(*const LayoutBox<'f>),
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
                        text_fragment: src.text_fragment.clone(),
                        abs_placeholder: src.abs_placeholder,
                    };
                    let num_children = src.children.len();
                    work.push(Frame::Post { num_children, partial });
                    for child in src.children.iter().rev() {
                        work.push(Frame::Pre(child as *const LayoutBox<'a>));
                    }
                }
                Frame::Post { num_children, mut partial } => {
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
    fn drop(&mut self) {
        let mut queue: Vec<LayoutBox<'a>> = std::mem::take(&mut self.children);
        while let Some(mut node) = queue.pop() {
            queue.extend(std::mem::take(&mut node.children));
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

// ── Public entry points ───────────────────────────────────────────────────────

pub fn build_layout_tree<'a>(
    style_node: &'a StyledNode,
    container_start_x: f32,
    current_x: f32,
    current_y: f32,
    container_width: f32,
    vw: f32,
    vh: f32,
) -> (Option<LayoutBox<'a>>, f32, f32) {
    build_layout_tree_with_cb(style_node, container_start_x, current_x, current_y, container_width, vw, vh, None)
}

/// Variant that takes the initial containing block for absolutely positioned
/// boxes that have no positioned ancestor. `None` = the viewport at (0,0,vw×vh).
pub fn build_layout_tree_with_cb<'a>(
    style_node: &'a StyledNode,
    container_start_x: f32,
    _current_x: f32,
    current_y: f32,
    container_width: f32,
    vw: f32,
    vh: f32,
    containing_block: Option<Rect>,
) -> (Option<LayoutBox<'a>>, f32, f32) {
    build_layout_tree_with_images(style_node, container_start_x, current_y, container_width, vw, vh, containing_block, ImageSizes::default())
}

/// [`build_layout_tree_with_cb`] with the natural sizes of loaded images, so
/// `<img>` boxes with an `auto` width or height get their decoded size and
/// aspect ratio.
#[allow(clippy::too_many_arguments)]
pub fn build_layout_tree_with_images<'a>(
    style_node: &'a StyledNode,
    container_start_x: f32,
    current_y: f32,
    container_width: f32,
    vw: f32,
    vh: f32,
    containing_block: Option<Rect>,
    images: ImageSizes,
) -> (Option<LayoutBox<'a>>, f32, f32) {
    let mut ctx = Ctx::new(vw, vh);
    ctx.images = images;
    let result = layout_root(style_node, container_start_x, current_y, container_width, &mut ctx);
    let Some(mut root) = result else {
        return (None, container_start_x, current_y);
    };
    let icb = containing_block.unwrap_or(Rect { x: 0.0, y: 0.0, width: vw, height: vh });
    if ctx.pending_abs > 0 {
        resolve_pending_abs(&mut root, icb, true, &mut ctx);
    }
    let final_y = root.dimensions.y + root.dimensions.height + root.margin.bottom;
    let final_x = container_start_x;
    (Some(root), final_x, final_y)
}

/// Compute the **max-content** width of a `StyledNode` subtree (border box).
pub fn compute_max_content_width(sn: &StyledNode, vw: f32, vh: f32) -> f32 {
    let mut ctx = Ctx::new(vw, vh);
    intrinsic_widths(sn, &mut ctx).1
}

/// Compute the **min-content** width of a `StyledNode` subtree (border box).
pub fn compute_min_content_width(sn: &StyledNode, vw: f32, vh: f32) -> f32 {
    let mut ctx = Ctx::new(vw, vh);
    intrinsic_widths(sn, &mut ctx).0
}

/// Per-layout state shared by every box.
struct Ctx {
    vw: f32,
    vh: f32,
    intrinsic: HashMap<usize, (f32, f32)>,
    /// Number of unresolved absolute/fixed placeholders in the tree.
    pending_abs: usize,
    /// Memoized `top_margin_chain` results keyed by (node, containing block width).
    margin_chain: HashMap<(usize, u32), (Strut, bool)>,
    /// Memoized `inline_contains_block` results.
    contains_block: HashMap<usize, bool>,
    /// Natural sizes of loaded images.
    images: ImageSizes,
}

impl Ctx {
    fn new(vw: f32, vh: f32) -> Self {
        Ctx {
            vw,
            vh,
            intrinsic: HashMap::new(),
            pending_abs: 0,
            margin_chain: HashMap::new(),
            contains_block: HashMap::new(),
            images: ImageSizes::default(),
        }
    }
}

/// Natural (intrinsic) pixel sizes of loaded images, keyed by absolute URL.
/// Layout uses them to size `<img>` elements whose width or height is `auto`.
#[derive(Debug, Clone, Default)]
pub struct ImageSizes {
    base: Option<url::Url>,
    sizes: HashMap<String, (f32, f32)>,
}

lazy_static::lazy_static! {
    /// Header-decoded dimensions keyed by (url, byte length).
    static ref IMAGE_DIMENSIONS: std::sync::Mutex<HashMap<(String, usize), Option<(u32, u32)>>> =
        std::sync::Mutex::new(HashMap::new());
}

impl ImageSizes {
    /// Read the dimensions of every encoded image in `cache` (only the image
    /// header is parsed; results are memoized). `base` resolves relative `src`.
    pub fn from_cache(cache: &HashMap<String, Vec<u8>>, base: Option<&url::Url>) -> Self {
        let mut out = ImageSizes { base: base.cloned(), sizes: HashMap::new() };
        let mut memo = IMAGE_DIMENSIONS.lock().unwrap_or_else(|e| e.into_inner());
        if memo.len() > 4096 {
            memo.clear();
        }
        for (url, bytes) in cache {
            let dims = *memo.entry((url.clone(), bytes.len())).or_insert_with(|| {
                image::ImageReader::new(std::io::Cursor::new(bytes.as_slice()))
                    .with_guessed_format()
                    .ok()
                    .and_then(|r| r.into_dimensions().ok())
            });
            if let Some((w, h)) = dims {
                out.sizes.insert(url.clone(), (w as f32, h as f32));
            }
        }
        out
    }

    /// Record the natural size of the image at `url`.
    pub fn insert(&mut self, url: &str, width: f32, height: f32) {
        self.sizes.insert(url.to_string(), (width, height));
    }

    fn get(&self, src: &str) -> Option<(f32, f32)> {
        let src = src.trim();
        if src.is_empty() {
            return None;
        }
        if let Some(v) = self.sizes.get(src) {
            return Some(*v);
        }
        let resolved = self.base.as_ref()?.join(src).ok()?;
        self.sizes.get(resolved.as_str()).copied()
    }
}

// ── Style access helpers ──────────────────────────────────────────────────────

fn sval<'s>(sn: &'s StyledNode, prop: &str) -> Option<&'s Value> {
    sn.specified_values.get(prop)
}

fn skw<'s>(sn: &'s StyledNode, prop: &str) -> Option<&'s str> {
    match sn.specified_values.get(prop) {
        Some(Value::Keyword(k)) => Some(k.as_ref()),
        _ => None,
    }
}

fn tag_name(sn: &StyledNode) -> Option<String> {
    match sn.node.data {
        NodeData::Element { ref name, .. } => Some(name.local.to_string()),
        _ => None,
    }
}

fn is_tag(sn: &StyledNode, tag: &str) -> bool {
    matches!(sn.node.data, NodeData::Element { ref name, .. } if name.local.as_ref() == tag)
}

fn attr(sn: &StyledNode, name: &str) -> Option<String> {
    match sn.node.data {
        NodeData::Element { ref attrs, .. } => attrs
            .borrow()
            .iter()
            .find(|a| a.name.local.as_ref() == name)
            .map(|a| a.value.to_string()),
        _ => None,
    }
}

fn is_text(sn: &StyledNode) -> bool {
    matches!(sn.node.data, NodeData::Text { .. })
}

fn text_of(sn: &StyledNode) -> String {
    match sn.node.data {
        NodeData::Text { ref contents } => contents.borrow().to_string(),
        _ => String::new(),
    }
}

/// Resolve a length value to px. `pct_base` is the percentage basis (`None` when
/// percentages cannot be resolved, which yields `None`).
fn resolve_len(v: &Value, pct_base: Option<f32>, ctx: &Ctx) -> Option<f32> {
    match v {
        Value::Length(n, Unit::Px) => Some(*n),
        Value::Length(n, Unit::Percent) => pct_base.map(|b| b * n / 100.0),
        Value::Length(n, Unit::Vw) => Some(ctx.vw * n / 100.0),
        Value::Length(n, Unit::Vh) => Some(ctx.vh * n / 100.0),
        Value::Length(n, Unit::Vmin) => Some(ctx.vw.min(ctx.vh) * n / 100.0),
        Value::Length(n, Unit::Vmax) => Some(ctx.vw.max(ctx.vh) * n / 100.0),
        Value::Length(n, Unit::Em) | Value::Length(n, Unit::Rem) => Some(n * 16.0),
        Value::Number(n) if *n == 0.0 => Some(0.0),
        Value::Keyword(k) if k.starts_with("calc(") || k.starts_with("-webkit-calc(") => {
            let mut missing_base = false;
            let r = crate::css::eval_calc(k, &mut |n, u| match u {
                None | Some(Unit::Px) => Some(n),
                Some(Unit::Percent) => match pct_base {
                    Some(b) => Some(b * n / 100.0),
                    None => {
                        missing_base = true;
                        Some(0.0)
                    }
                },
                Some(Unit::Vw) => Some(ctx.vw * n / 100.0),
                Some(Unit::Vh) => Some(ctx.vh * n / 100.0),
                Some(Unit::Em) | Some(Unit::Rem) => Some(n * 16.0),
                _ => None,
            });
            if missing_base { None } else { r }
        }
        _ => None,
    }
}

fn prop_len(sn: &StyledNode, prop: &str, pct_base: Option<f32>, ctx: &Ctx) -> Option<f32> {
    sval(sn, prop).and_then(|v| resolve_len(v, pct_base, ctx))
}

fn is_auto(sn: &StyledNode, prop: &str) -> bool {
    match sval(sn, prop) {
        None => true,
        Some(Value::Keyword(k)) => k.as_ref() == "auto",
        _ => false,
    }
}

fn font_size(sn: &StyledNode) -> f32 {
    match sval(sn, "font-size") {
        Some(Value::Length(v, Unit::Px)) => v.max(0.0),
        _ => 16.0,
    }
}

/// Used line height in px.
fn line_height(sn: &StyledNode) -> f32 {
    let fs = font_size(sn);
    match sval(sn, "line-height") {
        Some(Value::Length(v, Unit::Px)) => v.max(0.0),
        Some(Value::Length(v, Unit::Percent)) => fs * v / 100.0,
        Some(Value::Number(n)) => fs * n,
        _ => font_metrics(sn, fs).normal_line_height(),
    }
}

fn letter_spacing(sn: &StyledNode) -> f32 {
    match sval(sn, "letter-spacing") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    }
}

fn get_position_type(sn: &StyledNode) -> PositionType {
    match skw(sn, "position") {
        Some("relative") => PositionType::Relative,
        Some("absolute") => PositionType::Absolute,
        Some("fixed") => PositionType::Fixed,
        Some("sticky") | Some("-webkit-sticky") => PositionType::Sticky,
        _ => PositionType::Static,
    }
}

fn is_out_of_flow_positioned(sn: &StyledNode) -> bool {
    matches!(get_position_type(sn), PositionType::Absolute | PositionType::Fixed)
}

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

fn get_float(sn: &StyledNode) -> Option<FloatSide> {
    if is_out_of_flow_positioned(sn) {
        return None;
    }
    match skw(sn, "float") {
        Some("left") | Some("inline-start") => Some(FloatSide::Left),
        Some("right") | Some("inline-end") => Some(FloatSide::Right),
        _ => None,
    }
}

fn get_clear(sn: &StyledNode) -> Option<ClearValue> {
    match skw(sn, "clear") {
        Some("left") => Some(ClearValue::Left),
        Some("right") => Some(ClearValue::Right),
        Some("both") => Some(ClearValue::Both),
        _ => None,
    }
}

fn get_line_break_clear(sn: &StyledNode) -> Option<ClearValue> {
    if let Some(clear) = get_clear(sn) {
        return Some(clear);
    }
    match attr(sn, "clear").as_deref() {
        Some("left") => Some(ClearValue::Left),
        Some("right") => Some(ClearValue::Right),
        Some("all") | Some("both") => Some(ClearValue::Both),
        _ => None,
    }
}

fn is_none_display(sn: &StyledNode) -> bool {
    skw(sn, "display") == Some("none")
}

fn should_skip(child: &StyledNode) -> bool {
    if is_none_display(child) {
        return true;
    }
    match child.node.data {
        NodeData::Element { ref name, ref attrs, .. } => {
            let t = name.local.as_ref();
            if matches!(t, "head" | "style" | "meta" | "title" | "script" | "link" | "noscript" | "template" | "base") {
                return true;
            }
            // <input type="hidden"> never renders, regardless of CSS.
            if t == "input" {
                let is_hidden = attrs.borrow().iter().any(|a| {
                    a.name.local.as_ref() == "type" && a.value.to_string().eq_ignore_ascii_case("hidden")
                });
                if is_hidden {
                    return true;
                }
            }
            false
        }
        NodeData::Text { .. } | NodeData::Document => false,
        _ => true, // comments, doctypes, processing instructions
    }
}

fn is_line_break_element(child: &StyledNode) -> bool {
    is_tag(child, "br")
}

fn is_form_control(sn: &StyledNode) -> bool {
    matches!(tag_name(sn).as_deref(), Some("input" | "button" | "select" | "textarea"))
}

/// Replaced elements: laid out from their own size, children ignored.
fn is_replaced(sn: &StyledNode) -> bool {
    match tag_name(sn).as_deref() {
        Some("img" | "svg" | "canvas" | "video" | "iframe" | "embed" | "object" | "audio") => true,
        Some("input" | "select" | "textarea") => true,
        _ => false,
    }
}

fn get_display_type(sn: &StyledNode) -> DisplayType {
    if let NodeData::Text { .. } = sn.node.data {
        return DisplayType::Inline;
    }
    if !matches!(sn.node.data, NodeData::Element { .. }) {
        return DisplayType::Block;
    }
    // Form controls always report Input so collect_form_controls finds them.
    if is_form_control(sn) {
        return DisplayType::Input;
    }
    let tag = tag_name(sn).unwrap_or_default();
    if matches!(tag.as_str(), "img" | "svg" | "canvas" | "video" | "iframe" | "embed" | "object") {
        return DisplayType::Image;
    }
    if let Some(d) = skw(sn, "display") {
        match d {
            "block" | "flow-root" | "-webkit-box" | "-moz-box" | "contents" => return DisplayType::Block,
            "inline-block" | "-webkit-inline-box" => return DisplayType::InlineBlock,
            "flex" | "inline-flex" | "-webkit-flex" | "-webkit-inline-flex" => return DisplayType::Flex,
            "grid" | "inline-grid" => return DisplayType::Grid,
            "list-item" => return DisplayType::ListItem,
            "table" | "inline-table" => return DisplayType::Table,
            "table-row" => return DisplayType::TableRow,
            "table-cell" => return DisplayType::TableCell,
            "table-row-group" | "table-header-group" | "table-footer-group" | "table-caption" => return DisplayType::Block,
            "inline" => return DisplayType::Inline,
            "none" => return DisplayType::Inline,
            _ => {}
        }
    }
    match tag.as_str() {
        "html" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "body" | "header"
        | "footer" | "nav" | "section" | "article" | "ul" | "ol" | "main" | "aside"
        | "form" | "details" | "summary" | "figure" | "figcaption" | "address"
        | "blockquote" | "pre" | "hr" | "fieldset" | "legend" | "dl" | "dt" | "dd"
        | "menu" | "dir" | "hgroup" | "search" | "dialog" | "optgroup" | "option"
        | "center" | "thead" | "tbody" | "tfoot" | "caption" | "colgroup" => DisplayType::Block,
        "li" => DisplayType::ListItem,
        "table" => DisplayType::Table,
        "tr" => DisplayType::TableRow,
        "th" | "td" => DisplayType::TableCell,
        _ => DisplayType::Inline,
    }
}

/// Raw `display` keyword with tag defaults (for outer/inner classification).
fn display_keyword(sn: &StyledNode) -> String {
    if let Some(d) = skw(sn, "display") {
        return d.to_string();
    }
    match get_display_type(sn) {
        DisplayType::Block => "block",
        DisplayType::Inline => "inline",
        DisplayType::InlineBlock => "inline-block",
        DisplayType::ListItem => "list-item",
        DisplayType::Table => {
            if is_tag(sn, "table") { "table" } else { "block" }
        }
        DisplayType::TableRow => "table-row",
        DisplayType::TableCell => "table-cell",
        DisplayType::Input => "inline-block",
        DisplayType::Image => "inline",
        DisplayType::Flex => "flex",
        DisplayType::Grid => "grid",
    }
    .to_string()
}

/// Inner layout model of a box.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Inner {
    Flow,
    Flex,
    Grid,
    Table,
    Replaced,
}

fn inner_display(sn: &StyledNode) -> Inner {
    if !matches!(sn.node.data, NodeData::Element { .. }) {
        return Inner::Flow;
    }
    if is_replaced(sn) {
        return Inner::Replaced;
    }
    match display_keyword(sn).as_str() {
        "flex" | "inline-flex" | "-webkit-flex" | "-webkit-inline-flex" => Inner::Flex,
        "-webkit-box" | "-webkit-inline-box" | "-moz-box" => {
            // Legacy box: vertical orientation behaves like a block (line clamp);
            // horizontal like a row flex container.
            if skw(sn, "-webkit-box-orient") == Some("vertical") {
                Inner::Flow
            } else {
                Inner::Flex
            }
        }
        "grid" | "inline-grid" => Inner::Grid,
        "table" | "inline-table" => Inner::Table,
        _ => Inner::Flow,
    }
}

/// True when the element is inline-level in its parent's flow (before blockification).
fn is_inline_level_display(sn: &StyledNode) -> bool {
    if is_text(sn) {
        return true;
    }
    let d = display_keyword(sn);
    matches!(
        d.as_str(),
        "inline" | "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "-webkit-inline-box" | "-webkit-inline-flex" | "contents"
    )
}

/// Inline-level boxes that are laid out as a single unit on a line.
fn is_atomic_inline(sn: &StyledNode) -> bool {
    if is_text(sn) {
        return false;
    }
    if is_replaced(sn) || is_tag(sn, "button") {
        return true;
    }
    let d = display_keyword(sn);
    matches!(d.as_str(), "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "-webkit-inline-box" | "-webkit-inline-flex")
}

/// True when this box establishes a new block formatting context for its contents.
fn establishes_bfc_sn(sn: &StyledNode) -> bool {
    if !matches!(sn.node.data, NodeData::Element { .. }) {
        return true;
    }
    if get_float(sn).is_some() || is_out_of_flow_positioned(sn) {
        return true;
    }
    let d = display_keyword(sn);
    if matches!(
        d.as_str(),
        "inline-block" | "flow-root" | "table-cell" | "table-caption" | "flex" | "inline-flex" | "grid" | "inline-grid" | "table" | "inline-table" | "-webkit-box" | "-webkit-inline-box"
    ) {
        return true;
    }
    if is_tag(sn, "html") || is_tag(sn, "button") || is_replaced(sn) {
        return true;
    }
    let ov = |p: &str| matches!(skw(sn, p), Some(v) if v != "visible" && v != "clip");
    ov("overflow") || ov("overflow-x") || ov("overflow-y")
}

fn overflow_clips(sn: &StyledNode) -> bool {
    let ov = |p: &str| matches!(skw(sn, p), Some(v) if v != "visible");
    ov("overflow") || ov("overflow-x") || ov("overflow-y")
}

fn is_border_box(sn: &StyledNode) -> bool {
    skw(sn, "box-sizing") == Some("border-box")
}

/// Resolved padding / border / margin (auto margins become 0 with a flag).
#[derive(Clone, Copy, Debug, Default)]
struct BoxModel {
    margin: EdgeSizes,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin_auto_left: bool,
    margin_auto_right: bool,
    margin_auto_top: bool,
    margin_auto_bottom: bool,
}

impl BoxModel {
    fn pb_h(&self) -> f32 {
        self.padding.horizontal() + self.border.horizontal()
    }
    fn pb_v(&self) -> f32 {
        self.padding.vertical() + self.border.vertical()
    }
}

fn border_side_width(sn: &StyledNode, side: &str) -> f32 {
    let style = skw(sn, &format!("border-{}-style", side)).or_else(|| skw(sn, "border-style"));
    let width_v = sval(sn, &format!("border-{}-width", side)).or_else(|| sval(sn, "border-width"));
    let w = match width_v {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Number(n)) => *n,
        Some(Value::Keyword(k)) => match k.as_ref() {
            "thin" => 1.0,
            "medium" => 3.0,
            "thick" => 5.0,
            _ => 0.0,
        },
        _ => 0.0,
    };
    match style {
        Some("none") | Some("hidden") => 0.0,
        // No style at all: only honour widths that were set without a style by
        // legacy callers (the uniform `border-width` key).
        None => {
            if sval(sn, &format!("border-{}-width", side)).is_none() && sval(sn, "border-width").is_some() {
                w.max(0.0)
            } else {
                0.0
            }
        }
        _ => w.max(0.0),
    }
}

fn box_model(sn: &StyledNode, cb_width: f32, ctx: &Ctx) -> BoxModel {
    let mut bm = BoxModel::default();
    if !matches!(sn.node.data, NodeData::Element { .. }) {
        return bm;
    }
    let base = Some(cb_width);
    let m = |p: &str| prop_len(sn, p, base, ctx).unwrap_or(0.0);
    bm.margin_auto_left = matches!(sval(sn, "margin-left"), Some(Value::Keyword(k)) if k.as_ref() == "auto");
    bm.margin_auto_right = matches!(sval(sn, "margin-right"), Some(Value::Keyword(k)) if k.as_ref() == "auto");
    bm.margin_auto_top = matches!(sval(sn, "margin-top"), Some(Value::Keyword(k)) if k.as_ref() == "auto");
    bm.margin_auto_bottom = matches!(sval(sn, "margin-bottom"), Some(Value::Keyword(k)) if k.as_ref() == "auto");
    bm.margin = EdgeSizes { left: m("margin-left"), right: m("margin-right"), top: m("margin-top"), bottom: m("margin-bottom") };
    let p = |p: &str| prop_len(sn, p, base, ctx).unwrap_or(0.0).max(0.0);
    bm.padding = EdgeSizes { left: p("padding-left"), right: p("padding-right"), top: p("padding-top"), bottom: p("padding-bottom") };
    bm.border = EdgeSizes {
        left: border_side_width(sn, "left"),
        right: border_side_width(sn, "right"),
        top: border_side_width(sn, "top"),
        bottom: border_side_width(sn, "bottom"),
    };
    bm
}

/// Specified width converted to a border-box width, if definite.
fn specified_border_width(sn: &StyledNode, cb_width: Option<f32>, bm: &BoxModel, ctx: &Ctx) -> Option<f32> {
    let v = sval(sn, "width")?;
    let w = resolve_len(v, cb_width, ctx)?;
    Some(if is_border_box(sn) { w.max(bm.pb_h()) } else { w.max(0.0) + bm.pb_h() })
}

fn specified_border_height(sn: &StyledNode, cb_height: Option<f32>, bm: &BoxModel, ctx: &Ctx) -> Option<f32> {
    let v = sval(sn, "height")?;
    let h = resolve_len(v, cb_height, ctx)?;
    Some(if is_border_box(sn) { h.max(bm.pb_v()) } else { h.max(0.0) + bm.pb_v() })
}

/// Clamp a border-box width by min-width / max-width.
fn clamp_border_width(sn: &StyledNode, w: f32, cb_width: Option<f32>, bm: &BoxModel, ctx: &Ctx) -> f32 {
    let to_border = |v: f32| if is_border_box(sn) { v.max(bm.pb_h()) } else { v + bm.pb_h() };
    let mut w = w;
    if let Some(max) = prop_len(sn, "max-width", cb_width, ctx) {
        w = w.min(to_border(max));
    }
    if let Some(min) = prop_len(sn, "min-width", cb_width, ctx) {
        w = w.max(to_border(min));
    }
    w.max(bm.pb_h())
}

fn clamp_border_height(sn: &StyledNode, h: f32, cb_height: Option<f32>, bm: &BoxModel, ctx: &Ctx) -> f32 {
    let to_border = |v: f32| if is_border_box(sn) { v.max(bm.pb_v()) } else { v + bm.pb_v() };
    let mut h = h;
    if let Some(max) = prop_len(sn, "max-height", cb_height, ctx) {
        h = h.min(to_border(max));
    }
    if let Some(min) = prop_len(sn, "min-height", cb_height, ctx) {
        h = h.max(to_border(min));
    }
    h.max(bm.pb_v())
}

// ── Box construction ──────────────────────────────────────────────────────────

impl<'a> LayoutBox<'a> {
    fn new(style_node: &'a StyledNode) -> Self {
        let display = get_display_type(style_node);
        let z_index = match style_node.specified_values.get("z-index") {
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
            text_fragment: None,
            abs_placeholder: false,
        };

        if let NodeData::Element { ref attrs, ref name, .. } = style_node.node.data {
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
            if tag == "input" && matches!(input_type.as_str(), "submit" | "button" | "reset") {
                layout.input_label = Some(match input_value {
                    Some(v) => v,
                    None => match input_type.as_str() {
                        "submit" => "Submit".to_string(),
                        "reset" => "Reset".to_string(),
                        _ => String::new(),
                    },
                });
            }
        }
        layout
    }

    fn with_box_model(mut self, bm: &BoxModel) -> Self {
        self.margin = bm.margin;
        self.padding = bm.padding;
        self.border = bm.border;
        self
    }

    fn content_x(&self) -> f32 {
        self.dimensions.x + self.border.left + self.padding.left
    }
    fn content_y(&self) -> f32 {
        self.dimensions.y + self.border.top + self.padding.top
    }
    fn padding_box(&self) -> Rect {
        Rect {
            x: self.dimensions.x + self.border.left,
            y: self.dimensions.y + self.border.top,
            width: (self.dimensions.width - self.border.horizontal()).max(0.0),
            height: (self.dimensions.height - self.border.vertical()).max(0.0),
        }
    }
    fn margin_box_width(&self) -> f32 {
        self.dimensions.width + self.margin.horizontal()
    }
    fn margin_box_height(&self) -> f32 {
        self.dimensions.height + self.margin.vertical()
    }
}

/// Iterative subtree translation (avoids stack overflows on deep trees).
pub fn offset_layout_box(layout: &mut LayoutBox, dx: f32, dy: f32) {
    if dx == 0.0 && dy == 0.0 {
        return;
    }
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

/// Apply `top`/`left`/`right`/`bottom` as visual offsets for `position: relative`.
fn apply_relative_offset(layout: &mut LayoutBox, cb_width: f32, cb_height: Option<f32>, ctx: &Ctx) {
    if layout.position != PositionType::Relative {
        return;
    }
    let sn = layout.style_node;
    let left = if is_auto(sn, "left") { None } else { prop_len(sn, "left", Some(cb_width), ctx) };
    let right = if is_auto(sn, "right") { None } else { prop_len(sn, "right", Some(cb_width), ctx) };
    let top = if is_auto(sn, "top") { None } else { prop_len(sn, "top", cb_height, ctx) };
    let bottom = if is_auto(sn, "bottom") { None } else { prop_len(sn, "bottom", cb_height, ctx) };
    let dx = match (left, right) {
        (Some(l), _) => l,
        (None, Some(r)) => -r,
        _ => 0.0,
    };
    let dy = match (top, bottom) {
        (Some(t), _) => t,
        (None, Some(b)) => -b,
        _ => 0.0,
    };
    offset_layout_box(layout, dx, dy);
}

// ── Margin collapsing ─────────────────────────────────────────────────────────

/// Adjoining margins collapse to max(positive) + min(negative).
#[derive(Clone, Copy, Default, Debug)]
struct Strut {
    pos: f32,
    neg: f32,
}

impl Strut {
    fn of(m: f32) -> Self {
        let mut s = Strut::default();
        s.add(m);
        s
    }
    fn add(&mut self, m: f32) {
        if m >= 0.0 {
            self.pos = self.pos.max(m);
        } else {
            self.neg = self.neg.min(m);
        }
    }
    fn merge(&mut self, o: Strut) {
        self.pos = self.pos.max(o.pos);
        self.neg = self.neg.min(o.neg);
    }
    fn resolve(&self) -> f32 {
        self.pos + self.neg
    }
}

/// A block container whose top margin can collapse with its first child's.
fn can_collapse_top(sn: &StyledNode, bm: &BoxModel) -> bool {
    matches!(sn.node.data, NodeData::Element { .. })
        && bm.border.top == 0.0
        && bm.padding.top == 0.0
        && !establishes_bfc_sn(sn)
        && inner_display(sn) == Inner::Flow
        && !matches!(get_display_type(sn), DisplayType::TableCell | DisplayType::Table)
}

fn can_collapse_bottom(sn: &StyledNode, bm: &BoxModel, ctx: &Ctx) -> bool {
    can_collapse_top_like(sn)
        && bm.border.bottom == 0.0
        && bm.padding.bottom == 0.0
        && is_auto_height(sn, ctx)
        && prop_len(sn, "min-height", None, ctx).map_or(true, |v| v <= 0.0)
}

fn can_collapse_top_like(sn: &StyledNode) -> bool {
    matches!(sn.node.data, NodeData::Element { .. })
        && !establishes_bfc_sn(sn)
        && inner_display(sn) == Inner::Flow
        && !matches!(get_display_type(sn), DisplayType::TableCell | DisplayType::Table)
}

fn is_auto_height(sn: &StyledNode, ctx: &Ctx) -> bool {
    match sval(sn, "height") {
        None => true,
        Some(Value::Keyword(k)) => matches!(k.as_ref(), "auto" | "min-content" | "max-content" | "fit-content"),
        Some(Value::Length(_, Unit::Percent)) => true, // treated as auto unless the CB height is definite
        Some(v) => resolve_len(v, None, ctx).is_none(),
    }
}

/// Classification of a child in a block container's flow.
#[derive(Clone, Copy, PartialEq, Debug)]
enum FlowKind {
    Skip,
    Block,
    Inline,
    Float,
    Abs,
}

fn flow_kind(child: &StyledNode, ctx: &mut Ctx) -> FlowKind {
    if should_skip(child) {
        return FlowKind::Skip;
    }
    if is_text(child) {
        return FlowKind::Inline;
    }
    if !matches!(child.node.data, NodeData::Element { .. }) {
        return FlowKind::Skip;
    }
    if is_out_of_flow_positioned(child) {
        return FlowKind::Abs;
    }
    if get_float(child).is_some() {
        return FlowKind::Float;
    }
    if is_inline_level_display(child) || is_line_break_element(child) {
        if !is_atomic_inline(child) && inline_contains_block(child, ctx) {
            return FlowKind::Block;
        }
        return FlowKind::Inline;
    }
    FlowKind::Block
}

/// Inline (non-atomic) element that contains in-flow block-level descendants.
/// Such elements are laid out as blocks (an approximation of block-in-inline splitting).
fn inline_contains_block(sn: &StyledNode, ctx: &mut Ctx) -> bool {
    let key = sn as *const StyledNode as usize;
    if let Some(v) = ctx.contains_block.get(&key) {
        return *v;
    }
    let v = stacker::maybe_grow(256 * 1024, 16 * 1024 * 1024, || inline_contains_block_inner(sn, ctx));
    ctx.contains_block.insert(key, v);
    v
}

fn inline_contains_block_inner(sn: &StyledNode, ctx: &mut Ctx) -> bool {
    for c in &sn.children {
        if should_skip(c) || is_text(c) || !matches!(c.node.data, NodeData::Element { .. }) {
            continue;
        }
        if is_out_of_flow_positioned(c) || get_float(c).is_some() {
            continue;
        }
        if !is_inline_level_display(c) && !is_line_break_element(c) {
            return true;
        }
        if !is_atomic_inline(c) && inline_contains_block(c, ctx) {
            return true;
        }
    }
    false
}

fn is_collapsible_whitespace_text(sn: &StyledNode) -> bool {
    if !is_text(sn) {
        return false;
    }
    let preserve = matches!(skw(sn, "white-space"), Some("pre" | "pre-wrap" | "break-spaces" | "pre-line"));
    let t = text_of(sn);
    if preserve {
        t.is_empty()
    } else {
        t.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
    }
}

/// The top margin strut a block contributes at its top edge: its own margin plus
/// the margins of first in-flow block descendants that collapse through it.
fn top_margin_chain(sn: &StyledNode, cb_width: f32, ctx: &mut Ctx) -> (Strut, bool) {
    let key = (sn as *const StyledNode as usize, cb_width.to_bits());
    if let Some(v) = ctx.margin_chain.get(&key) {
        return *v;
    }
    let v = stacker::maybe_grow(256 * 1024, 16 * 1024 * 1024, || top_margin_chain_inner(sn, cb_width, ctx));
    ctx.margin_chain.insert(key, v);
    v
}

fn top_margin_chain_inner(sn: &StyledNode, cb_width: f32, ctx: &mut Ctx) -> (Strut, bool) {
    let bm = box_model(sn, cb_width, ctx);
    let mut strut = Strut::of(bm.margin.top);
    if !can_collapse_top(sn, &bm) {
        return (strut, false);
    }
    let inner_w = (cb_width - bm.margin.horizontal() - bm.pb_h()).max(0.0);
    for c in &sn.children {
        match flow_kind(c, ctx) {
            FlowKind::Skip | FlowKind::Float | FlowKind::Abs => continue,
            FlowKind::Inline => {
                if is_collapsible_whitespace_text(c) {
                    continue;
                }
                return (strut, false);
            }
            FlowKind::Block => {
                if get_clear(c).is_some() {
                    return (strut, false);
                }
                let (child_strut, _) = top_margin_chain(c, inner_w, ctx);
                strut.merge(child_strut);
                return (strut, true);
            }
        }
    }
    (strut, false)
}

// ── Floats ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct FloatArea {
    /// Margin box in absolute coordinates.
    rect: Rect,
    side: FloatSide,
}

#[derive(Default)]
struct FloatCtx {
    areas: Vec<FloatArea>,
}

impl FloatCtx {
    /// Available horizontal band [left, right) at y..y+h between `min_x` and `max_x`.
    fn band(&self, y: f32, h: f32, min_x: f32, max_x: f32) -> (f32, f32) {
        let h = h.max(0.01);
        let mut left = min_x;
        let mut right = max_x;
        for fa in &self.areas {
            if fa.rect.y < y + h && fa.rect.y + fa.rect.height > y && fa.rect.height > 0.0 {
                match fa.side {
                    FloatSide::Left => left = left.max(fa.rect.x + fa.rect.width),
                    FloatSide::Right => right = right.min(fa.rect.x),
                }
            }
        }
        (left, right.max(left))
    }
    /// Smallest float bottom edge below `y` (for moving down past floats).
    fn next_bottom_after(&self, y: f32) -> Option<f32> {
        self.areas
            .iter()
            .map(|a| a.rect.y + a.rect.height)
            .filter(|b| *b > y + 0.01)
            .fold(None, |acc: Option<f32>, b| Some(acc.map_or(b, |a| a.min(b))))
    }
    fn clear_y(&self, cv: ClearValue) -> Option<f32> {
        self.areas
            .iter()
            .filter(|fa| match cv {
                ClearValue::Left => fa.side == FloatSide::Left,
                ClearValue::Right => fa.side == FloatSide::Right,
                ClearValue::Both => true,
            })
            .map(|fa| fa.rect.y + fa.rect.height)
            .fold(None, |acc: Option<f32>, b| Some(acc.map_or(b, |a| a.max(b))))
    }
    fn bottom(&self) -> Option<f32> {
        self.clear_y(ClearValue::Both)
    }
    fn last_top(&self) -> f32 {
        self.areas.iter().map(|a| a.rect.y).fold(f32::NEG_INFINITY, f32::max)
    }
}

/// Lay out a float and place it in `floats`. `y` is the earliest allowed top.
fn place_float<'a>(
    sn: &'a StyledNode,
    side: FloatSide,
    y: f32,
    content_x: f32,
    content_w: f32,
    cb: Cb,
    floats: &mut FloatCtx,
    ctx: &mut Ctx,
) -> Option<LayoutBox<'a>> {
    let mut lb = layout_block_level(sn, 0.0, 0.0, cb, None, BlockOpts { shrink_to_fit: true, ..Default::default() }, ctx)?.lb;
    let w = lb.margin_box_width();
    let h = lb.margin_box_height();
    let mut top = y.max(floats.last_top());
    if let Some(cv) = get_clear(sn) {
        if let Some(cy) = floats.clear_y(cv) {
            top = top.max(cy);
        }
    }
    let max_x = content_x + content_w;
    loop {
        let (left, right) = floats.band(top, h, content_x, max_x);
        if right - left >= w - 0.01 || (left <= content_x + 0.01 && right >= max_x - 0.01) {
            let x = match side {
                FloatSide::Left => left,
                FloatSide::Right => right - w,
            };
            let dx = x + lb.margin.left - lb.dimensions.x;
            let dy = top + lb.margin.top - lb.dimensions.y;
            offset_layout_box(&mut lb, dx, dy);
            floats.areas.push(FloatArea { rect: Rect { x, y: top, width: w, height: h }, side });
            return Some(lb);
        }
        match floats.next_bottom_after(top) {
            Some(b) => top = b,
            None => {
                let x = match side {
                    FloatSide::Left => content_x,
                    FloatSide::Right => max_x - w,
                };
                let dx = x + lb.margin.left - lb.dimensions.x;
                let dy = top + lb.margin.top - lb.dimensions.y;
                offset_layout_box(&mut lb, dx, dy);
                floats.areas.push(FloatArea { rect: Rect { x, y: top, width: w, height: h }, side });
                return Some(lb);
            }
        }
    }
}

// ── Block-level layout ────────────────────────────────────────────────────────

/// Containing block for percentages: content width and (definite) height.
#[derive(Clone, Copy, Debug)]
struct Cb {
    width: f32,
    height: Option<f32>,
}

#[derive(Clone, Copy, Default)]
struct BlockOpts {
    /// The box's top margin already collapsed with its first child's; place that
    /// child at the content top without re-applying the collapsed margins.
    absorb_top: bool,
    /// Use shrink-to-fit width when `width` is auto (floats, inline-blocks, ...).
    shrink_to_fit: bool,
    /// Force this border-box width (flex items, positioned boxes).
    forced_width: Option<f32>,
    /// Force this border-box height (stretched flex items, positioned boxes).
    forced_height: Option<f32>,
}

struct BlockOut<'a> {
    lb: LayoutBox<'a>,
    /// Bottom margin strut propagating out of this box (own margin-bottom merged
    /// with the last child's when they collapse).
    bottom_strut: Strut,
    /// The box has no in-flow content and no height: its margins collapse through.
    collapsed_through: bool,
    /// Baseline of the first / last line box inside (absolute y).
    first_baseline: Option<f32>,
    last_baseline: Option<f32>,
}

/// Lay out the root of a layout call.
fn layout_root<'a>(sn: &'a StyledNode, x: f32, y: f32, width: f32, ctx: &mut Ctx) -> Option<LayoutBox<'a>> {
    if should_skip(sn) && !matches!(sn.node.data, NodeData::Document) {
        return None;
    }
    let cb = Cb { width, height: Some(ctx.vh) };
    if is_text(sn) {
        // A bare text node: lay it out in an anonymous inline formatting context.
        let mut holder = LayoutBox::new(sn);
        holder.display = DisplayType::Inline;
        let mut children = Vec::new();
        let out = layout_inline_run(&[sn], sn, x, y, width, cb, &mut FloatCtx::default(), ctx, &mut children);
        if children.len() == 1 {
            return children.pop();
        }
        holder.children = children;
        holder.dimensions = Rect { x, y, width: out.max_line_width, height: out.height };
        holder.text_fragment = Some(String::new());
        return Some(holder);
    }
    let shrink = matches!(sn.node.data, NodeData::Element { .. })
        && (is_inline_level_display(sn) || get_float(sn).is_some() || is_out_of_flow_positioned(sn));
    let mut floats = FloatCtx::default();
    let bm = box_model(sn, width, ctx);
    let top = y + bm.margin.top;
    let (_, absorbs) = top_margin_chain(sn, width, ctx);
    let out = layout_block_level(
        sn,
        x,
        top,
        cb,
        Some(&mut floats),
        BlockOpts { shrink_to_fit: shrink, absorb_top: absorbs, ..Default::default() },
        ctx,
    )?;
    let mut lb = out.lb;
    if shrink && lb.position == PositionType::Absolute {
        // A positioned root is placed at the requested origin.
        let dx = x + lb.margin.left - lb.dimensions.x;
        offset_layout_box(&mut lb, dx, 0.0);
    }
    Some(lb)
}

/// Lay out a block-level box. `x` is the left content edge of the containing
/// block (the box's margin edge starts there), `y` its border-top edge.
/// `floats` is the parent's float context (shared when this box does not
/// establish a new block formatting context).
fn layout_block_level<'a>(
    sn: &'a StyledNode,
    x: f32,
    y: f32,
    cb: Cb,
    floats: Option<&mut FloatCtx>,
    opts: BlockOpts,
    ctx: &mut Ctx,
) -> Option<BlockOut<'a>> {
    stacker::maybe_grow(256 * 1024, 16 * 1024 * 1024, move || layout_block_level_inner(sn, x, y, cb, floats, opts, ctx))
}

fn layout_block_level_inner<'a>(
    sn: &'a StyledNode,
    x: f32,
    y: f32,
    cb: Cb,
    floats: Option<&mut FloatCtx>,
    opts: BlockOpts,
    ctx: &mut Ctx,
) -> Option<BlockOut<'a>> {
    if should_skip(sn) && !matches!(sn.node.data, NodeData::Document) {
        return None;
    }
    let bm = box_model(sn, cb.width, ctx);
    let mut lb = LayoutBox::new(sn).with_box_model(&bm);
    let inner = inner_display(sn);

    // ── Width ────────────────────────────────────────────────────────────
    let avail = (cb.width - bm.margin.horizontal()).max(0.0);
    let specified = specified_border_width(sn, Some(cb.width), &bm, ctx);
    let keyword_width = skw(sn, "width");
    let mut border_w = if let Some(fw) = opts.forced_width {
        fw
    } else if let Some(w) = specified {
        w
    } else if let Some(Value::FitContent(limit)) = sval(sn, "width") {
        let (min_c, max_c) = intrinsic_widths(sn, ctx);
        max_c.min(avail.min(*limit + bm.pb_h())).max(min_c)
    } else if let Some(k) = keyword_width.filter(|k| matches!(*k, "min-content" | "max-content" | "fit-content" | "-webkit-fit-content")) {
        let (min_c, max_c) = intrinsic_widths(sn, ctx);
        match k {
            "min-content" => min_c,
            "max-content" => max_c,
            _ => max_c.min(avail).max(min_c),
        }
    } else if inner == Inner::Replaced {
        replaced_size(sn, &bm, cb, ctx).0
    } else if opts.shrink_to_fit
        || is_tag(sn, "button")
        || matches!(get_display_type(sn), DisplayType::Table | DisplayType::TableCell)
    {
        let (min_c, max_c) = intrinsic_widths(sn, ctx);
        max_c.min(avail).max(min_c)
    } else {
        avail
    };
    if opts.forced_width.is_none() {
        border_w = clamp_border_width(sn, border_w, Some(cb.width), &bm, ctx);
    }

    // Auto margins (block-level boxes in normal flow center with margin: auto).
    let mut margin_left = bm.margin.left;
    let mut margin_right = bm.margin.right;
    if !opts.shrink_to_fit && opts.forced_width.is_none() {
        let remaining = cb.width - border_w - bm.margin.horizontal();
        if bm.margin_auto_left && bm.margin_auto_right {
            let each = (remaining / 2.0).max(0.0);
            margin_left = bm.margin.left + each;
            margin_right = bm.margin.right + (remaining - each).max(0.0);
        } else if bm.margin_auto_left {
            margin_left = bm.margin.left + remaining.max(0.0);
        } else if bm.margin_auto_right {
            margin_right = bm.margin.right + remaining.max(0.0);
        }
    }
    lb.margin.left = margin_left;
    lb.margin.right = margin_right;
    lb.dimensions.x = x + margin_left;
    lb.dimensions.y = y;
    lb.dimensions.width = border_w;

    let content_w = (border_w - bm.pb_h()).max(0.0);
    let specified_h = if let Some(fh) = opts.forced_height {
        Some(fh)
    } else {
        specified_border_height(sn, cb.height, &bm, ctx)
    };
    let child_cb = Cb {
        width: content_w,
        height: specified_h.map(|h| (h - bm.pb_v()).max(0.0)),
    };

    // ── Contents ─────────────────────────────────────────────────────────
    let content_x = lb.content_x();
    let content_y = lb.content_y();
    let bfc = establishes_bfc_sn(sn) || floats.is_none();
    let mut own_floats = FloatCtx::default();
    let floats: &mut FloatCtx = match floats {
        Some(f) if !bfc => f,
        _ => &mut own_floats,
    };

    let mut first_baseline = None;
    let mut last_baseline = None;
    let mut bottom_strut = Strut::of(bm.margin.bottom);
    let mut collapsed_through = false;
    let content_h = match inner {
        Inner::Replaced => {
            let (_, h) = replaced_size_for(sn, &bm, cb, ctx, Some(border_w));
            let border_h = clamp_border_height(sn, specified_h.unwrap_or(h), cb.height, &bm, ctx);
            let content_h = (border_h - bm.pb_v()).max(0.0);
            layout_placeholder(&mut lb, sn, content_x, content_y, content_w, content_h, ctx);
            content_h
        }
        Inner::Flex => {
            let out = layout_flex(&mut lb, content_x, content_y, content_w, child_cb, specified_h.is_some(), ctx);
            first_baseline = out.first_baseline;
            last_baseline = out.first_baseline;
            out.height
        }
        Inner::Grid => layout_grid(&mut lb, content_x, content_y, content_w, child_cb, ctx),
        Inner::Table => {
            let out = layout_table(&mut lb, content_x, content_y, content_w, child_cb, ctx);
            first_baseline = out.1;
            last_baseline = out.1;
            out.0
        }
        Inner::Flow => {
            let collapse_bottom = can_collapse_bottom(sn, &bm, ctx) && opts.forced_height.is_none();
            let out = layout_flow(
                &mut lb,
                sn,
                content_x,
                content_y,
                content_w,
                child_cb,
                floats,
                opts.absorb_top,
                collapse_bottom,
                ctx,
            );
            first_baseline = out.first_baseline;
            last_baseline = out.last_baseline;
            if collapse_bottom {
                bottom_strut.merge(out.bottom_strut);
            }
            let mut h = out.content_height;
            if bfc {
                if let Some(fb) = floats.bottom() {
                    h = h.max(fb - content_y);
                }
            }
            if !out.has_content && collapse_bottom && specified_h.is_none() && bm.pb_v() == 0.0 && h <= 0.0 {
                collapsed_through = true;
            }
            h
        }
    };

    let mut border_h = match specified_h {
        Some(h) => h,
        None => content_h + bm.pb_v(),
    };
    if opts.forced_height.is_none() {
        border_h = clamp_border_height(sn, border_h, cb.height, &bm, ctx);
    }
    if collapsed_through && border_h > 0.0 {
        collapsed_through = false;
    }
    lb.dimensions.height = border_h;
    // Buttons center their contents vertically when the box is taller than them.
    if matches!(lb.display, DisplayType::Input) && inner == Inner::Flow {
        let extra = (border_h - bm.pb_v()) - content_h;
        if extra > 0.01 {
            for c in lb.children.iter_mut() {
                offset_layout_box(c, 0.0, extra / 2.0);
            }
        }
    }

    if lb.display == DisplayType::ListItem {
        // Marker text is assigned by the parent (which knows the item index).
    }

    // Positioned boxes are containing blocks for their absolute descendants.
    if lb.position != PositionType::Static && ctx.pending_abs > 0 {
        let cb_rect = lb.padding_box();
        resolve_pending_abs(&mut lb, cb_rect, false, ctx);
    }

    Some(BlockOut { lb, bottom_strut, collapsed_through, first_baseline, last_baseline })
}


/// Border-box size of a replaced element.
fn replaced_size(sn: &StyledNode, bm: &BoxModel, cb: Cb, ctx: &Ctx) -> (f32, f32) {
    replaced_size_for(sn, bm, cb, ctx, None)
}

/// Border-box size of a replaced element (CSS 2.1 §10.3.2, §10.4, §10.6.2 and
/// CSS Sizing 4 aspect-ratio). `used_border_w` overrides the width when the
/// formatting context already decided it (e.g. a stretched flex item), so the
/// height follows the aspect ratio of that width.
fn replaced_size_for(sn: &StyledNode, bm: &BoxModel, cb: Cb, ctx: &Ctx, used_border_w: Option<f32>) -> (f32, f32) {
    let w = used_border_w.or_else(|| specified_border_width(sn, Some(cb.width), bm, ctx));
    let h = specified_border_height(sn, cb.height, bm, ctx);
    let tag = tag_name(sn).unwrap_or_default();
    let (default_w, default_h) = match tag.as_str() {
        "img" => (100.0, 66.7),
        "input" => (160.0, 24.0),
        "select" => (120.0, 24.0),
        "textarea" => (160.0, 48.0),
        _ => (300.0, 150.0),
    };
    if tag == "svg" {
        let mut ratio = default_h / default_w;
        // Outer <svg>: `auto` width is 100% of the containing block; the height
        // follows the viewBox aspect ratio (or the 150px default).
        let vb = svg_view_box(sn);
        if let Some((vbw, vbh)) = vb {
            ratio = vbh / vbw;
        }
        if w.is_none() && h.is_none() {
            let cw = if cb.width < 1.0e5 { (cb.width - bm.margin.horizontal()).max(0.0) } else { vb.map_or(300.0, |v| v.0) };
            let content_w = (cw - bm.pb_h()).max(0.0);
            let ch = if vb.is_some() { content_w * ratio } else { 150.0 };
            let bw = clamp_border_width(sn, content_w + bm.pb_h(), Some(cb.width), bm, ctx);
            let bh = clamp_border_height(sn, ch + bm.pb_v(), cb.height, bm, ctx);
            return (bw, bh);
        }
        if vb.is_none() {
            if let (Some(w), None) = (w, h) {
                return (clamp_border_width(sn, w, Some(cb.width), bm, ctx), 150.0 + bm.pb_v());
            }
        }
    }
    let (pb_h, pb_v) = (bm.pb_h(), bm.pb_v());
    let (nat_w, nat_h, nat_ratio) = natural_dimensions(sn, &tag, ctx);
    // Used aspect ratio (width / height): `aspect-ratio: <ratio>` wins over the
    // natural ratio; `auto && <ratio>` only fills in when there is none.
    let ratio = match aspect_ratio(sn) {
        Some((true, r)) => nat_ratio.or(r),
        Some((false, r)) => r.or(nat_ratio),
        None => nat_ratio,
    };
    let limit = |prop: &str, base: Option<f32>, pb: f32| {
        prop_len(sn, prop, base, ctx).map(|v| if is_border_box(sn) { (v - pb).max(0.0) } else { v.max(0.0) })
    };
    let min_w = limit("min-width", Some(cb.width), pb_h).unwrap_or(0.0);
    let max_w = limit("max-width", Some(cb.width), pb_h).unwrap_or(f32::INFINITY).max(min_w);
    let min_h = limit("min-height", cb.height, pb_v).unwrap_or(0.0);
    let max_h = limit("max-height", cb.height, pb_v).unwrap_or(f32::INFINITY).max(min_h);
    let clamp_w = |v: f32| if used_border_w.is_some() { v } else { v.min(max_w).max(min_w) };
    let clamp_h = |v: f32| v.min(max_h).max(min_h);
    let spec_w = w.map(|w| (w - pb_h).max(0.0));
    let spec_h = h.map(|h| (h - pb_v).max(0.0));
    let (cw, ch) = match (spec_w, spec_h) {
        (Some(w), Some(h)) => (clamp_w(w), clamp_h(h)),
        (Some(w), None) => {
            let w = clamp_w(w);
            let h = ratio.map(|r| w / r).or(nat_h).unwrap_or(if tag == "img" { w * 0.667 } else { default_h });
            (w, clamp_h(h))
        }
        (None, Some(h)) => {
            let h = clamp_h(h);
            let w = ratio.map(|r| h * r).or(nat_w).unwrap_or(if tag == "img" { h / 0.667 } else { default_w });
            (clamp_w(w), h)
        }
        (None, None) => match (nat_w, nat_h, ratio) {
            (Some(nw), Some(nh), Some(r)) => {
                let nh = if nat_ratio == Some(r) { nh } else { nw / r };
                constrain_replaced(nw, nh, min_w, max_w, min_h, max_h)
            }
            (Some(nw), Some(nh), None) => (clamp_w(nw), clamp_h(nh)),
            (Some(nw), None, r) => {
                let w = clamp_w(nw);
                (w, clamp_h(r.map_or(default_h, |r| w / r)))
            }
            (None, Some(nh), r) => {
                let h = clamp_h(nh);
                (clamp_w(r.map_or(default_w, |r| h * r)), h)
            }
            (None, None, Some(r)) => {
                let w = if tag == "img" { default_w.min(cb.width.max(1.0)) } else { default_w };
                constrain_replaced(w, w / r, min_w, max_w, min_h, max_h)
            }
            (None, None, None) => {
                let w = if tag == "img" { default_w.min(cb.width.max(1.0)) } else { default_w };
                let h = if tag == "img" { w * 0.667 } else { default_h };
                (clamp_w(w), clamp_h(h))
            }
        },
    };
    (cw + pb_h, (ch + pb_v).max(if tag == "img" { 1.0 } else { 0.0 }))
}

/// Lay out the generated `::placeholder` box of an empty text control inside
/// its content box. A single-line `<input>` centers it vertically, like the
/// inner editor of Chromium; a `<textarea>` keeps it at the top.
fn layout_placeholder<'a>(
    lb: &mut LayoutBox<'a>,
    sn: &'a StyledNode,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    content_h: f32,
    ctx: &mut Ctx,
) {
    let Some(ph) = sn.children.iter().find(|c| is_tag(c, "::placeholder")) else { return };
    let cb = Cb { width: content_w, height: Some(content_h) };
    let Some(out) = layout_block_level(ph, content_x, content_y, cb, None, BlockOpts::default(), ctx) else { return };
    let mut child = out.lb;
    if is_tag(sn, "input") {
        let dy = (content_h - child.dimensions.height) / 2.0;
        offset_layout_box(&mut child, 0.0, dy);
    }
    lb.children.push(child);
}

/// CSS 2.1 §10.4 table: resolve min/max constraints for a replaced element
/// whose width and height are both `auto`, preserving the aspect ratio.
fn constrain_replaced(w: f32, h: f32, min_w: f32, max_w: f32, min_h: f32, max_h: f32) -> (f32, f32) {
    if w <= 0.0 || h <= 0.0 {
        return (w.min(max_w).max(min_w), h.min(max_h).max(min_h));
    }
    let (over_w, under_w) = (w > max_w, w < min_w);
    let (over_h, under_h) = (h > max_h, h < min_h);
    match (over_w, under_w, over_h, under_h) {
        (true, _, true, _) => {
            if max_w / w <= max_h / h {
                (max_w, (max_w * h / w).max(min_h))
            } else {
                ((max_h * w / h).max(min_w), max_h)
            }
        }
        (_, true, _, true) => {
            if min_w / w <= min_h / h {
                ((min_h * w / h).min(max_w), min_h)
            } else {
                (min_w, (min_w * h / w).min(max_h))
            }
        }
        (_, true, true, _) => (min_w, max_h),
        (true, _, _, true) => (max_w, min_h),
        (true, _, _, _) => (max_w, (max_w * h / w).max(min_h)),
        (_, true, _, _) => (min_w, (min_w * h / w).min(max_h)),
        (_, _, true, _) => ((max_h * w / h).max(min_w), max_h),
        (_, _, _, true) => ((min_h * w / h).min(max_w), min_h),
        _ => (w, h),
    }
}

/// Natural width, height and aspect ratio (width / height) of a replaced
/// element. Images use the decoded size when loaded; otherwise the
/// `width`/`height` attributes still give the ratio (HTML maps them to
/// `aspect-ratio: auto w / h`).
fn natural_dimensions(sn: &StyledNode, tag: &str, ctx: &Ctx) -> (Option<f32>, Option<f32>, Option<f32>) {
    let attr_px = |name: &str| attr(sn, name).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| *v > 0.0);
    match tag {
        "img" => {
            if let Some((w, h)) = attr(sn, "src").and_then(|src| ctx.images.get(&src)) {
                let r = if w > 0.0 && h > 0.0 { Some(w / h) } else { None };
                return (Some(w), Some(h), r);
            }
            let r = match (attr_px("width"), attr_px("height")) {
                (Some(w), Some(h)) => Some(w / h),
                _ => None,
            };
            (None, None, r)
        }
        "canvas" => {
            let w = attr_px("width").unwrap_or(300.0);
            let h = attr_px("height").unwrap_or(150.0);
            (Some(w), Some(h), Some(w / h))
        }
        "video" | "iframe" | "embed" | "object" => (Some(300.0), Some(150.0), if tag == "video" { Some(2.0) } else { None }),
        "svg" => (None, None, svg_view_box(sn).map(|(w, h)| w / h)),
        _ => (None, None, None),
    }
}

/// Parsed `aspect-ratio`: `(has auto, ratio)`, or `None` when it is `auto`/unset.
fn aspect_ratio(sn: &StyledNode) -> Option<(bool, Option<f32>)> {
    let text = match sval(sn, "aspect-ratio")? {
        Value::Number(n) => return if *n > 0.0 { Some((false, Some(*n))) } else { None },
        Value::Keyword(k) => k.to_string(),
        Value::Length(n, Unit::Px) if *n > 0.0 => return Some((false, Some(*n))),
        _ => return None,
    };
    let has_auto = text.split_whitespace().any(|t| t == "auto");
    let rest: String = text.split_whitespace().filter(|t| *t != "auto").collect::<Vec<_>>().join(" ");
    if rest.is_empty() {
        return None;
    }
    let mut parts = rest.split('/').map(|p| p.trim().parse::<f32>().ok());
    let r = match (parts.next().flatten(), parts.next()) {
        (Some(a), None) => Some(a),
        (Some(a), Some(Some(b))) if b > 0.0 => Some(a / b),
        _ => None,
    }
    .filter(|r| *r > 0.0 && r.is_finite());
    Some((has_auto, r))
}

/// `viewBox="minx miny w h"` of an svg element, as (w, h).
fn svg_view_box(sn: &StyledNode) -> Option<(f32, f32)> {
    let vb = attr(sn, "viewBox").or_else(|| attr(sn, "viewbox"))?;
    let nums: Vec<f32> = vb
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if nums.len() == 4 && nums[2] > 0.0 && nums[3] > 0.0 {
        Some((nums[2], nums[3]))
    } else {
        None
    }
}

struct FlowOut {
    content_height: f32,
    bottom_strut: Strut,
    first_baseline: Option<f32>,
    last_baseline: Option<f32>,
    has_content: bool,
}

/// Lay out the children of a block container (block and inline formatting).
fn layout_flow<'a>(
    parent: &mut LayoutBox<'a>,
    sn: &'a StyledNode,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    cb: Cb,
    floats: &mut FloatCtx,
    absorb_top: bool,
    collapse_bottom: bool,
    ctx: &mut Ctx,
) -> FlowOut {
    let mut cursor = content_y;
    let mut pending = Strut::default();
    let mut has_content = false;
    let mut first_baseline: Option<f32> = None;
    let mut last_baseline: Option<f32> = None;
    let mut list_counter: i32 = match attr(sn, "start").and_then(|s| s.trim().parse::<i32>().ok()) {
        Some(s) => s - 1,
        None => 0,
    };
    let children: Vec<&'a StyledNode> = sn.children.iter().collect();
    let kinds: Vec<FlowKind> = children.iter().map(|c| flow_kind(c, ctx)).collect();
    let line_clamp = line_clamp_of(sn);
    let mut lines_used: usize = 0;

    let mut i = 0;
    while i < children.len() {
        match kinds[i] {
            FlowKind::Skip => {
                i += 1;
            }
            FlowKind::Inline | FlowKind::Float | FlowKind::Abs if kinds[i] == FlowKind::Inline || run_has_inline(&kinds[i..]) => {
                // Gather a run of inline-level content (floats/abs inside the run
                // are placed by the inline formatting context).
                let start = i;
                while i < children.len() && matches!(kinds[i], FlowKind::Inline | FlowKind::Float | FlowKind::Abs | FlowKind::Skip) {
                    i += 1;
                }
                let run: Vec<&'a StyledNode> = children[start..i]
                    .iter()
                    .zip(&kinds[start..i])
                    .filter(|(_, k)| **k != FlowKind::Skip)
                    .map(|(c, _)| *c)
                    .collect();
                if run.iter().all(|c| is_collapsible_whitespace_text(c)) {
                    continue;
                }
                let run_top = if run_has_line_content(&run, ctx) { cursor + pending.resolve() } else { cursor };
                let mut out_children = Vec::new();
                let out = layout_inline_run_clamped(
                    &run,
                    sn,
                    content_x,
                    run_top,
                    content_w,
                    cb,
                    floats,
                    ctx,
                    &mut out_children,
                    line_clamp.map(|n| n.saturating_sub(lines_used)),
                );
                parent.children.extend(out_children);
                if out.line_count > 0 {
                    lines_used += out.line_count;
                    has_content = true;
                    pending = Strut::default();
                    cursor = run_top + out.height;
                    if first_baseline.is_none() {
                        first_baseline = out.first_baseline;
                    }
                    if out.last_baseline.is_some() {
                        last_baseline = out.last_baseline;
                    }
                }
            }
            FlowKind::Float => {
                let child = children[i];
                let side = get_float(child).unwrap_or(FloatSide::Left);
                let fy = cursor + pending.resolve();
                if let Some(fb) = place_float(child, side, fy, content_x, content_w, cb, floats, ctx) {
                    parent.children.push(fb);
                }
                i += 1;
            }
            FlowKind::Abs => {
                let child = children[i];
                parent.children.push(make_abs_placeholder(child, content_x, cursor, ctx));
                i += 1;
            }
            FlowKind::Inline => unreachable!(),
            FlowKind::Block => {
                let child = children[i];
                i += 1;
                let (child_strut, child_absorbs) = top_margin_chain(child, content_w, ctx);
                let mut strut = pending;
                let first_inflow = !has_content;
                if !(absorb_top && first_inflow) {
                    strut.merge(child_strut);
                }
                let mut top = cursor + strut.resolve();
                let mut cleared = false;
                if let Some(cv) = get_clear(child) {
                    if let Some(cy) = floats.clear_y(cv) {
                        if cy > top {
                            top = cy;
                            cleared = true;
                        }
                    }
                }
                // BFC roots (overflow != visible, flex, ...) avoid floats.
                let (mut bx, mut bw) = (content_x, content_w);
                let child_bfc = establishes_bfc_sn(child);
                if child_bfc && !floats.areas.is_empty() {
                    let mut probe_top = top;
                    let bm = box_model(child, content_w, ctx);
                    let want_w = specified_border_width(child, Some(content_w), &bm, ctx).map(|w| w + bm.margin.horizontal());
                    for _ in 0..32 {
                        let (l, r) = floats.band(probe_top, 1.0, content_x, content_x + content_w);
                        let fits = want_w.map_or(true, |w| r - l >= w - 0.01);
                        if fits || (l <= content_x + 0.01 && r >= content_x + content_w - 0.01) {
                            bx = l;
                            bw = r - l;
                            break;
                        }
                        match floats.next_bottom_after(probe_top) {
                            Some(b) => probe_top = b,
                            None => break,
                        }
                    }
                    if probe_top > top {
                        top = probe_top;
                    }
                }
                let child_cb = Cb { width: bw, height: cb.height };
                let opts = BlockOpts { absorb_top: child_absorbs, ..Default::default() };
                let shared = if child_bfc { None } else { Some(&mut *floats) };
                let Some(out) = layout_block_level(child, bx, top, child_cb, shared.or(Some(&mut FloatCtx::default())), opts, ctx) else {
                    continue;
                };
                let mut lb = out.lb;
                if lb.display == DisplayType::ListItem {
                    list_counter += 1;
                    lb.list_marker = list_marker_text(child, list_counter);
                }
                if out.collapsed_through && !cleared {
                    // Empty block: its margins collapse with the surrounding ones.
                    let mut s = strut;
                    s.merge(out.bottom_strut);
                    pending = s;
                    if absorb_top && first_inflow {
                        pending = Strut::default();
                        pending.merge(out.bottom_strut);
                    }
                } else {
                    has_content = true;
                    cursor = lb.dimensions.y + lb.dimensions.height;
                    pending = out.bottom_strut;
                    if first_baseline.is_none() {
                        first_baseline = out.first_baseline;
                    }
                    if out.last_baseline.is_some() {
                        last_baseline = out.last_baseline;
                    }
                }
                apply_relative_offset(&mut lb, content_w, cb.height, ctx);
                parent.children.push(lb);
            }
        }
    }

    let content_height = if collapse_bottom {
        cursor - content_y
    } else {
        cursor + pending.resolve() - content_y
    };
    FlowOut {
        content_height: content_height.max(0.0),
        bottom_strut: if collapse_bottom { pending } else { Strut::default() },
        first_baseline,
        last_baseline,
        has_content,
    }
}

/// True when a slice of flow kinds starting at a float/abs continues into inline content
/// before the next block (so the float belongs to the inline formatting context).
fn run_has_inline(kinds: &[FlowKind]) -> bool {
    for k in kinds {
        match k {
            FlowKind::Inline => return true,
            FlowKind::Block => return false,
            _ => {}
        }
    }
    false
}

/// True when the run contains something that creates a line box.
fn run_has_line_content(run: &[&StyledNode], ctx: &mut Ctx) -> bool {
    run.iter().any(|c| {
        if is_text(c) {
            !is_collapsible_whitespace_text(c)
        } else {
            flow_kind(c, ctx) == FlowKind::Inline
        }
    })
}

fn line_clamp_of(sn: &StyledNode) -> Option<usize> {
    let clamp = match sval(sn, "-webkit-line-clamp").or_else(|| sval(sn, "line-clamp")) {
        Some(Value::Number(n)) if *n >= 1.0 => Some(*n as usize),
        _ => None,
    }?;
    if skw(sn, "display") == Some("-webkit-box") || skw(sn, "display") == Some("-webkit-inline-box") {
        Some(clamp)
    } else {
        None
    }
}

fn list_marker_text(sn: &StyledNode, index: i32) -> Option<String> {
    let style_type = skw(sn, "list-style-type").unwrap_or("disc");
    match style_type {
        "none" => None,
        "decimal" => Some(format!("{}.", index)),
        "decimal-leading-zero" => Some(format!("{:02}.", index)),
        "lower-alpha" | "lower-latin" => Some(format!("{}.", (b'a' + ((index - 1).rem_euclid(26)) as u8) as char)),
        "upper-alpha" | "upper-latin" => Some(format!("{}.", (b'A' + ((index - 1).rem_euclid(26)) as u8) as char)),
        "circle" => Some("\u{25E6}".to_string()),
        "square" => Some("\u{25AA}".to_string()),
        _ => Some("\u{2022}".to_string()),
    }
}

// ── Inline formatting context ─────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Item<'a> {
    Text(&'a StyledNode),
    Open(&'a StyledNode),
    Close(&'a StyledNode),
    Atomic(&'a StyledNode),
    Break(&'a StyledNode),
    Float(&'a StyledNode),
    Abs(&'a StyledNode),
}

fn flatten_inline<'a>(nodes: &[&'a StyledNode], out: &mut Vec<Item<'a>>) {
    for n in nodes {
        let n: &'a StyledNode = n;
        if should_skip(n) {
            continue;
        }
        if is_text(n) {
            out.push(Item::Text(n));
            continue;
        }
        if !matches!(n.node.data, NodeData::Element { .. }) {
            continue;
        }
        if is_out_of_flow_positioned(n) {
            out.push(Item::Abs(n));
        } else if get_float(n).is_some() {
            out.push(Item::Float(n));
        } else if is_line_break_element(n) {
            out.push(Item::Break(n));
        } else if is_atomic_inline(n) || !is_inline_level_display(n) {
            out.push(Item::Atomic(n));
        } else if skw(n, "display") == Some("contents") {
            let kids: Vec<&'a StyledNode> = n.children.iter().collect();
            flatten_inline(&kids, out);
        } else {
            out.push(Item::Open(n));
            let kids: Vec<&'a StyledNode> = n.children.iter().collect();
            flatten_inline(&kids, out);
            out.push(Item::Close(n));
        }
    }
}

#[derive(Clone)]
enum PieceKind<'a> {
    /// A run of non-space characters from one text node (a word or one CJK char).
    Text(&'a StyledNode, String),
    /// A (collapsed or preserved) space.
    Space(&'a StyledNode, String),
    Open(&'a StyledNode),
    #[allow(dead_code)]
    Close(&'a StyledNode),
    Atomic(usize, &'a StyledNode),
    Break(&'a StyledNode),
    Float(&'a StyledNode),
    Abs(&'a StyledNode),
}

#[derive(Clone)]
struct Piece<'a> {
    kind: PieceKind<'a>,
    width: f32,
    /// A line break opportunity exists immediately before this piece.
    break_before: bool,
    /// Space that disappears at the start / end of a line.
    collapsible: bool,
    /// The text may be broken between any two characters if it overflows.
    break_anywhere: bool,
}

impl<'a> Piece<'a> {
    fn is_content(&self) -> bool {
        matches!(self.kind, PieceKind::Text(..) | PieceKind::Atomic(..))
    }
    fn is_space(&self) -> bool {
        matches!(self.kind, PieceKind::Space(..))
    }
}

fn is_cjk(c: char) -> bool {
    let u = c as u32;
    (0x1100..=0x11FF).contains(&u)
        || (0x2E80..=0x303F).contains(&u)
        || (0x3040..=0x30FF).contains(&u)
        || (0x3130..=0x318F).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFF00..=0xFFEF).contains(&u)
}

/// Characters that may not start a line (closing punctuation and similar).
fn no_break_before(c: char) -> bool {
    matches!(
        c,
        ',' | '.' | '!' | '?' | ':' | ';' | ')' | ']' | '}' | '%' | '\u{2026}' | '\u{00B7}' | '\u{3001}' | '\u{3002}'
            | '\u{300D}' | '\u{300F}' | '\u{FF09}' | '\u{FF0C}' | '\u{FF0E}' | '\u{201D}' | '\u{2019}' | '\'' | '"'
    )
}

fn apply_text_transform(text: &str, sn: &StyledNode) -> String {
    match skw(sn, "text-transform") {
        Some("uppercase") => text.to_uppercase(),
        Some("lowercase") => text.to_lowercase(),
        Some("capitalize") => {
            let mut out = String::with_capacity(text.len());
            let mut at_word_start = true;
            for c in text.chars() {
                if at_word_start && c.is_alphabetic() {
                    out.extend(c.to_uppercase());
                    at_word_start = false;
                } else {
                    out.push(c);
                    if c.is_whitespace() {
                        at_word_start = true;
                    }
                }
            }
            out
        }
        _ => text.to_string(),
    }
}

#[derive(Clone, Copy, PartialEq)]
struct WhiteSpace {
    collapse: bool,
    wrap: bool,
    preserve_newlines: bool,
}

fn white_space_of(sn: &StyledNode) -> WhiteSpace {
    match skw(sn, "white-space") {
        Some("nowrap") => WhiteSpace { collapse: true, wrap: false, preserve_newlines: false },
        Some("pre") => WhiteSpace { collapse: false, wrap: false, preserve_newlines: true },
        Some("pre-wrap") | Some("break-spaces") => WhiteSpace { collapse: false, wrap: true, preserve_newlines: true },
        Some("pre-line") => WhiteSpace { collapse: true, wrap: true, preserve_newlines: true },
        _ => WhiteSpace { collapse: true, wrap: true, preserve_newlines: false },
    }
}

/// Convert flattened inline items into measured pieces with break opportunities.
/// Atomic pieces get width 0 here; callers fill them in.
fn make_pieces<'a>(items: &[Item<'a>]) -> Vec<Piece<'a>> {
    let mut pieces: Vec<Piece<'a>> = Vec::new();
    let mut prev_space = true; // leading collapsible spaces are removed
    let mut break_next = false;
    let mut atomic_idx = 0usize;
    for item in items {
        match *item {
            Item::Text(node) => {
                let ws = white_space_of(node);
                let fs = font_size(node);
                let ls = letter_spacing(node);
                let word_break = skw(node, "word-break").unwrap_or("normal");
                let keep_all = word_break == "keep-all";
                let break_all = word_break == "break-all";
                let anywhere = matches!(skw(node, "overflow-wrap").or_else(|| skw(node, "word-wrap")), Some("break-word" | "anywhere"))
                    || word_break == "break-word";
                let text = apply_text_transform(&text_of(node), node);
                let space_w = char_advance(node, ' ', fs) + ls;
                let mut word = String::new();
                let flush = |word: &mut String, pieces: &mut Vec<Piece<'a>>, break_next: &mut bool| {
                    if word.is_empty() {
                        return;
                    }
                    let w = text_width(node, word, fs, ls);
                    pieces.push(Piece {
                        kind: PieceKind::Text(node, std::mem::take(word)),
                        width: w,
                        break_before: *break_next,
                        collapsible: false,
                        break_anywhere: anywhere || break_all,
                    });
                    *break_next = false;
                };
                for c in text.chars() {
                    let is_newline = c == '\n' || c == '\r';
                    if is_newline && ws.preserve_newlines {
                        flush(&mut word, &mut pieces, &mut break_next);
                        if c == '\r' {
                            continue;
                        }
                        pieces.push(Piece { kind: PieceKind::Break(node), width: 0.0, break_before: false, collapsible: false, break_anywhere: false });
                        prev_space = ws.collapse;
                        break_next = false;
                        continue;
                    }
                    let is_space = matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c');
                    if is_space {
                        flush(&mut word, &mut pieces, &mut break_next);
                        if ws.collapse {
                            if prev_space {
                                continue;
                            }
                            pieces.push(Piece { kind: PieceKind::Space(node, " ".into()), width: space_w, break_before: false, collapsible: true, break_anywhere: false });
                            prev_space = true;
                        } else {
                            let n = if c == '\t' { 8 } else { 1 };
                            pieces.push(Piece {
                                kind: PieceKind::Space(node, " ".repeat(n)),
                                width: space_w * n as f32,
                                break_before: false,
                                collapsible: false,
                                break_anywhere: false,
                            });
                            prev_space = false;
                        }
                        break_next = ws.wrap;
                        continue;
                    }
                    prev_space = false;
                    if (is_cjk(c) && !keep_all) || break_all {
                        flush(&mut word, &mut pieces, &mut break_next);
                        let bb = if no_break_before(c) { false } else { ws.wrap || break_next };
                        let bb = bb && (pieces.last().map_or(true, |p| !matches!(p.kind, PieceKind::Open(_))) || break_next || ws.wrap);
                        pieces.push(Piece {
                            kind: PieceKind::Text(node, c.to_string()),
                            width: char_advance(node, c, fs) + ls,
                            break_before: bb && ws.wrap,
                            collapsible: false,
                            break_anywhere: anywhere,
                        });
                        break_next = ws.wrap;
                        continue;
                    }
                    if word.is_empty() && break_next && no_break_before(c) {
                        // e.g. "한글," — the comma sticks to the previous character.
                        break_next = false;
                    }
                    word.push(c);
                    if c == '-' && ws.wrap && word.chars().count() > 1 {
                        flush(&mut word, &mut pieces, &mut break_next);
                        break_next = true;
                    }
                }
                flush(&mut word, &mut pieces, &mut break_next);
            }
            Item::Open(node) => {
                let edge = open_edge(node);
                if edge > 0.0 {
                    prev_space = false;
                }
                pieces.push(Piece { kind: PieceKind::Open(node), width: edge, break_before: false, collapsible: false, break_anywhere: false });
            }
            Item::Close(node) => {
                let edge = close_edge(node);
                if edge > 0.0 {
                    prev_space = false;
                }
                pieces.push(Piece { kind: PieceKind::Close(node), width: edge, break_before: false, collapsible: false, break_anywhere: false });
            }
            Item::Atomic(node) => {
                let wrap = white_space_of(node).wrap || parent_allows_wrap(&pieces);
                pieces.push(Piece {
                    kind: PieceKind::Atomic(atomic_idx, node),
                    width: 0.0,
                    break_before: wrap || break_next,
                    collapsible: false,
                    break_anywhere: false,
                });
                atomic_idx += 1;
                prev_space = false;
                break_next = wrap;
            }
            Item::Break(node) => {
                pieces.push(Piece { kind: PieceKind::Break(node), width: 0.0, break_before: false, collapsible: false, break_anywhere: false });
                prev_space = true;
                break_next = false;
            }
            Item::Float(node) => {
                pieces.push(Piece { kind: PieceKind::Float(node), width: 0.0, break_before: false, collapsible: false, break_anywhere: false });
            }
            Item::Abs(node) => {
                pieces.push(Piece { kind: PieceKind::Abs(node), width: 0.0, break_before: false, collapsible: false, break_anywhere: false });
            }
        }
    }
    pieces
}

/// Whether the context before an atomic inline allows wrapping (uses the most
/// recent text piece's white-space, defaulting to wrap).
fn parent_allows_wrap(pieces: &[Piece]) -> bool {
    for p in pieces.iter().rev() {
        match p.kind {
            PieceKind::Text(n, _) | PieceKind::Space(n, _) => return white_space_of(n).wrap,
            _ => {}
        }
    }
    true
}

fn inline_edges(node: &StyledNode) -> (f32, f32) {
    // Percentages on inline boxes resolve against the containing block; use 0.
    let px = |p: &str| match sval(node, p) {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    };
    let left = px("margin-left") + border_side_width(node, "left") + px("padding-left").max(0.0);
    let right = px("margin-right") + border_side_width(node, "right") + px("padding-right").max(0.0);
    (left, right)
}

fn open_edge(node: &StyledNode) -> f32 {
    inline_edges(node).0
}

fn close_edge(node: &StyledNode) -> f32 {
    inline_edges(node).1
}

struct IfcOut {
    height: f32,
    line_count: usize,
    first_baseline: Option<f32>,
    last_baseline: Option<f32>,
    max_line_width: f32,
}

fn layout_inline_run<'a>(
    run: &[&'a StyledNode],
    container: &'a StyledNode,
    content_x: f32,
    top: f32,
    content_w: f32,
    cb: Cb,
    floats: &mut FloatCtx,
    ctx: &mut Ctx,
    out: &mut Vec<LayoutBox<'a>>,
) -> IfcOut {
    layout_inline_run_clamped(run, container, content_x, top, content_w, cb, floats, ctx, out, None)
}

/// A laid-out atomic inline with its baseline offset from the margin-box top.
struct AtomicBox<'a> {
    lb: Option<LayoutBox<'a>>,
    baseline_from_top: f32,
}

fn layout_atomic<'a>(node: &'a StyledNode, cb: Cb, ctx: &mut Ctx) -> Option<AtomicBox<'a>> {
    let out = layout_block_level(node, 0.0, 0.0, cb, None, BlockOpts { shrink_to_fit: true, ..Default::default() }, ctx)?;
    let mut lb = out.lb;
    lb.dimensions.y += lb.margin.top;
    for c in lb.children.iter_mut() {
        offset_layout_box(c, 0.0, lb.margin.top);
    }
    let margin_top_y = lb.dimensions.y - lb.margin.top;
    let has_baseline = !overflow_clips(node) && !matches!(lb.display, DisplayType::Image) && !is_replaced(node);
    let baseline = if has_baseline {
        match inner_display(node) {
            Inner::Flex | Inner::Table => out.first_baseline,
            _ => out.last_baseline,
        }
        .map(|b| b + lb.margin.top - margin_top_y)
    } else {
        None
    };
    let baseline_from_top = match baseline {
        Some(b) => b,
        // Form controls without text put their baseline inside the box: text
        // inputs at their centered text baseline, empty buttons at the content
        // box bottom.
        None if is_form_control(node) => {
            let content_top = lb.margin.top + lb.border.top + lb.padding.top;
            let content_h = (lb.dimensions.height - lb.border.vertical() - lb.padding.vertical()).max(0.0);
            if is_tag(node, "button") {
                content_top + content_h
            } else {
                let m = inline_metrics(node);
                content_top + (content_h - (m.ascent + m.descent)) / 2.0 + m.ascent
            }
        }
        None => lb.margin_box_height(),
    };
    Some(AtomicBox { lb: Some(lb), baseline_from_top })
}

/// Vertical metrics of an inline box relative to its own baseline.
#[derive(Clone, Copy)]
struct InlineMetrics {
    ascent: f32,
    descent: f32,
    line_height: f32,
    x_height: f32,
}

fn inline_metrics(sn: &StyledNode) -> InlineMetrics {
    let fs = font_size(sn);
    let m = font_metrics(sn, fs);
    InlineMetrics { ascent: m.ascent, descent: m.descent, line_height: line_height(sn), x_height: fs * 0.5 }
}

impl InlineMetrics {
    fn half_leading(&self) -> f32 {
        (self.line_height - (self.ascent + self.descent)) / 2.0
    }
}

/// Baseline shift (positive = down) of an inline box relative to its parent's baseline.
fn baseline_shift(node: &StyledNode, own: &InlineMetrics, parent: &InlineMetrics, ctx: &Ctx) -> f32 {
    match sval(node, "vertical-align") {
        Some(Value::Keyword(k)) => match k.as_ref() {
            "sub" => parent.ascent * 0.2 + 1.0,
            "super" => -(parent.ascent * 0.4 + 1.0),
            "text-top" => own.ascent - parent.ascent,
            "text-bottom" => parent.descent - own.descent,
            "middle" => (own.ascent - own.descent) / 2.0 - parent.x_height / 2.0,
            _ => 0.0,
        },
        Some(v) => match resolve_len(v, Some(own.line_height), ctx) {
            Some(len) => -len,
            None => 0.0,
        },
        None => 0.0,
    }
}

fn vertical_align_kw(node: &StyledNode) -> Option<&str> {
    skw(node, "vertical-align")
}

/// One line's worth of pieces.
struct LineSpan {
    start: usize,
    end: usize,
    y: f32,
    left: f32,
    right: f32,
    forced_end: bool,
}

fn layout_inline_run_clamped<'a>(
    run: &[&'a StyledNode],
    container: &'a StyledNode,
    content_x: f32,
    top: f32,
    content_w: f32,
    cb: Cb,
    floats: &mut FloatCtx,
    ctx: &mut Ctx,
    out: &mut Vec<LayoutBox<'a>>,
    max_lines: Option<usize>,
) -> IfcOut {
    let mut items = Vec::new();
    flatten_inline(run, &mut items);
    let mut pieces = make_pieces(&items);

    // Lay out atomic inlines up front (their size does not depend on the line).
    let mut atomics: Vec<AtomicBox<'a>> = Vec::new();
    for p in pieces.iter_mut() {
        if let PieceKind::Atomic(_, node) = p.kind {
            match layout_atomic(node, Cb { width: content_w, height: cb.height }, ctx) {
                Some(a) => {
                    p.width = a.lb.as_ref().map(|b| b.margin_box_width()).unwrap_or(0.0);
                    atomics.push(a);
                }
                None => atomics.push(AtomicBox { lb: None, baseline_from_top: 0.0 }),
            }
        }
    }

    let strut = inline_metrics(container);
    let text_align = skw(container, "text-align").unwrap_or("left").to_string();
    let indent = prop_len(container, "text-indent", Some(content_w), ctx).unwrap_or(0.0);
    let ellipsis = skw(container, "text-overflow") == Some("ellipsis") && overflow_clips(container);
    let max_x = content_x + content_w;

    let mut y = top;
    let mut lines_out = 0usize;
    let mut first_baseline = None;
    let mut last_baseline = None;
    let mut max_line_width: f32 = 0.0;
    let mut open_stack: Vec<&'a StyledNode> = Vec::new();
    let mut deferred_floats: Vec<&'a StyledNode> = Vec::new();

    let mut i = 0;
    while i < pieces.len() {
        // Skip collapsible spaces at the start of a line.
        while i < pieces.len() && pieces[i].collapsible {
            i += 1;
        }
        // Place floats deferred from the previous line.
        for f in deferred_floats.drain(..) {
            let side = get_float(f).unwrap_or(FloatSide::Left);
            if let Some(fb) = place_float(f, side, y, content_x, content_w, cb, floats, ctx) {
                out.push(fb);
            }
        }
        if i >= pieces.len() {
            break;
        }
        let line_h_est = strut.line_height.max(1.0);
        let indent_now = if lines_out == 0 { indent } else { 0.0 };
        // Find a vertical position where the first unbreakable segment fits.
        let first_seg_w = segment_width(&pieces, i);
        let (mut left, mut right) = floats.band(y, line_h_est, content_x, max_x);
        let mut guard = 0;
        while right - left - indent_now < first_seg_w - 0.01 && guard < 64 {
            guard += 1;
            if left <= content_x + 0.01 && right >= max_x - 0.01 {
                break;
            }
            match floats.next_bottom_after(y) {
                Some(b) => {
                    y = b;
                    let band = floats.band(y, line_h_est, content_x, max_x);
                    left = band.0;
                    right = band.1;
                }
                None => break,
            }
        }
        let avail = (right - left - indent_now).max(0.0);

        // Greedy line filling.
        let start = i;
        let mut width = 0.0f32; // including trailing spaces
        let mut last_break: Option<usize> = None;
        let mut has_content = false;
        let mut forced_end = false;
        let mut end = pieces.len();
        let mut j = i;
        while j < pieces.len() {
            let p = &pieces[j];
            match p.kind {
                PieceKind::Break(_) => {
                    end = j + 1;
                    forced_end = true;
                    break;
                }
                PieceKind::Float(f) => {
                    // Place now if it fits beside the content so far.
                    let fw = {
                        let bm = box_model(f, content_w, ctx);
                        specified_border_width(f, Some(content_w), &bm, ctx).map(|w| w + bm.margin.horizontal()).unwrap_or(0.0)
                    };
                    if !has_content || width + fw <= avail {
                        let side = get_float(f).unwrap_or(FloatSide::Left);
                        if let Some(fb) = place_float(f, side, y, content_x, content_w, cb, floats, ctx) {
                            out.push(fb);
                        }
                        let band = floats.band(y, line_h_est, content_x, max_x);
                        left = band.0;
                        right = band.1;
                    } else {
                        deferred_floats.push(f);
                    }
                    j += 1;
                    continue;
                }
                _ => {}
            }
            let avail_now = (right - left - indent_now).max(0.0);
            if p.is_content() && p.break_before && has_content {
                last_break = Some(j);
            }
            if p.is_content() {
                let fits = width + p.width <= avail_now + 0.01;
                if !fits && has_content {
                    if let Some(b) = last_break {
                        end = b;
                        break;
                    }
                }
                if !fits && p.break_anywhere {
                    if let PieceKind::Text(node, text) = p.kind.clone() {
                        let (a, b) = split_text_to_fit(node, &text, avail_now - width, font_size(node), letter_spacing(node), !has_content);
                        if !b.is_empty() {
                            if a.is_empty() {
                                end = j;
                                break;
                            }
                            let fs = font_size(node);
                            let ls = letter_spacing(node);
                            let wa = text_width(node, &a, fs, ls);
                            let wb = text_width(node, &b, fs, ls);
                            let bb = pieces[j].break_before;
                            pieces[j] = Piece { kind: PieceKind::Text(node, a), width: wa, break_before: bb, collapsible: false, break_anywhere: true };
                            pieces.insert(j + 1, Piece { kind: PieceKind::Text(node, b), width: wb, break_before: true, collapsible: false, break_anywhere: true });
                            end = j + 1;
                            break;
                        }
                    }
                }
                has_content = true;
            }
            width += p.width;
            j += 1;
        }
        let n_now = pieces.len();
        if j >= n_now && !forced_end {
            end = n_now;
        }
        // Back up over Open pieces that precede the break so they start the next line.
        let mut line_end = end;
        if !forced_end {
            while line_end > start && matches!(pieces[line_end - 1].kind, PieceKind::Open(_)) && line_end < n_now {
                line_end -= 1;
            }
            if line_end == start {
                line_end = end;
            }
        }
        let clamp_hit = max_lines.map_or(false, |m| lines_out + 1 >= m && line_end < n_now);
        let span = LineSpan { start, end: line_end, y, left: left + indent_now, right, forced_end };
        let (line_h, baseline, line_w) = build_line(
            &mut pieces,
            &span,
            &mut atomics,
            &mut open_stack,
            container,
            &strut,
            &text_align,
            ellipsis || clamp_hit,
            clamp_hit,
            cb,
            ctx,
            out,
        );
        max_line_width = max_line_width.max(line_w + indent_now);
        if line_h > 0.0 || span_has_forced_break(&pieces, &span) {
            lines_out += 1;
            if first_baseline.is_none() {
                first_baseline = Some(baseline);
            }
            last_baseline = Some(baseline);
        }
        y += line_h;
        // <br clear=...> / clear on a forced break moves the next line below floats.
        if span.forced_end && line_end > 0 {
            if let PieceKind::Break(br) = pieces[line_end - 1].kind {
                if let Some(cv) = get_line_break_clear(br) {
                    if let Some(cy) = floats.clear_y(cv) {
                        y = y.max(cy);
                    }
                }
            }
        }
        i = line_end;
        let n_now = pieces.len();
        if clamp_hit {
            // Drop the remaining content; keep out-of-flow boxes.
            i = n_now;
        }
        if i == start {
            i += 1; // safety: always make progress
        }
    }
    for f in deferred_floats.drain(..) {
        let side = get_float(f).unwrap_or(FloatSide::Left);
        if let Some(fb) = place_float(f, side, y, content_x, content_w, cb, floats, ctx) {
            out.push(fb);
        }
    }
    IfcOut { height: y - top, line_count: lines_out, first_baseline, last_baseline, max_line_width }
}

/// Split `text` so that the first part fits in `room` px. With `take_one`, at
/// least one character is kept in the first part.
fn split_text_to_fit(sn: &StyledNode, text: &str, room: f32, fs: f32, ls: f32, take_one: bool) -> (String, String) {
    let mut acc = 0.0;
    let mut split = text.len();
    for (ci, c) in text.char_indices() {
        let cw = char_advance(sn, c, fs) + ls;
        if acc + cw > room + 0.01 && !(take_one && ci == 0) {
            split = ci;
            break;
        }
        acc += cw;
    }
    (text[..split].to_string(), text[split..].to_string())
}

fn span_has_forced_break(pieces: &[Piece], span: &LineSpan) -> bool {
    span.forced_end && pieces[span.start..span.end].iter().any(|p| matches!(p.kind, PieceKind::Break(_)))
}

/// Width of the unbreakable segment starting at `i` (up to the next break opportunity).
fn segment_width(pieces: &[Piece], i: usize) -> f32 {
    let mut w = 0.0;
    let mut j = i;
    let mut seen_content = false;
    while j < pieces.len() {
        let p = &pieces[j];
        if matches!(p.kind, PieceKind::Break(_)) {
            break;
        }
        if p.is_content() && p.break_before && seen_content {
            break;
        }
        if p.is_space() && seen_content {
            break;
        }
        if p.is_content() {
            seen_content = true;
        }
        w += p.width;
        j += 1;
    }
    w
}

/// Build the fragments of one line. Returns (line height, baseline y, content width).
#[allow(clippy::too_many_arguments)]
fn build_line<'a>(
    pieces: &mut Vec<Piece<'a>>,
    span: &LineSpan,
    atomics: &mut Vec<AtomicBox<'a>>,
    open_stack: &mut Vec<&'a StyledNode>,
    container: &'a StyledNode,
    strut: &InlineMetrics,
    text_align: &str,
    ellipsis: bool,
    force_ellipsis: bool,
    cb: Cb,
    ctx: &mut Ctx,
    out: &mut Vec<LayoutBox<'a>>,
) -> (f32, f32, f32) {
    // Work on a copy of the line's pieces with trailing spaces removed.
    let mut line: Vec<Piece<'a>> = pieces[span.start..span.end].to_vec();
    let mut k = line.len();
    while k > 0 {
        match line[k - 1].kind {
            PieceKind::Space(..) if line[k - 1].collapsible || !span.forced_end => {
                line.remove(k - 1);
                k -= 1;
            }
            PieceKind::Close(_) | PieceKind::Float(_) | PieceKind::Abs(_) | PieceKind::Break(_) => k -= 1,
            _ => break,
        }
    }
    let avail = (span.right - span.left).max(0.0);
    let content_width = |l: &[Piece]| -> f32 {
        l.iter().filter(|p| !matches!(p.kind, PieceKind::Float(_) | PieceKind::Abs(_) | PieceKind::Break(_))).map(|p| p.width).sum()
    };
    if (ellipsis && content_width(&line) > avail + 0.01) || force_ellipsis {
        truncate_with_ellipsis(&mut line, avail, force_ellipsis);
    }
    let line_w = content_width(&line);

    // ── Vertical metrics ──
    let has_line_content = line.iter().any(|p| match p.kind {
        PieceKind::Text(..) | PieceKind::Atomic(..) | PieceKind::Break(_) => true,
        PieceKind::Space(..) => !p.collapsible,
        PieceKind::Open(_) | PieceKind::Close(_) => p.width > 0.0,
        _ => false,
    }) || (span.forced_end && pieces[span.start..span.end].iter().any(|p| matches!(p.kind, PieceKind::Break(_))));

    // Inline box contexts: (node, metrics, shift relative to root baseline)
    let mut ctx_stack: Vec<(&'a StyledNode, InlineMetrics, f32)> = Vec::new();
    let mut min_top = -(strut.ascent + strut.half_leading());
    let mut max_bottom = strut.descent + strut.half_leading();
    let root_shift = 0.0;
    for node in open_stack.iter() {
        let parent = ctx_stack.last().map(|c| (c.1, c.2)).unwrap_or((*strut, root_shift));
        let m = inline_metrics(node);
        let shift = parent.1 + baseline_shift(node, &m, &parent.0, ctx);
        ctx_stack.push((node, m, shift));
    }
    // Shifts computed per piece index for the placement pass.
    let mut piece_shift: Vec<f32> = vec![0.0; line.len()];
    let mut aligned_edges: Vec<(usize, bool, f32)> = Vec::new(); // (piece idx, is_top, height)
    let contributes = |m: &InlineMetrics, shift: f32, min_top: &mut f32, max_bottom: &mut f32| {
        let hl = m.half_leading();
        *min_top = min_top.min(shift - m.ascent - hl);
        *max_bottom = max_bottom.max(shift + m.descent + hl);
    };
    if has_line_content {
        for c in ctx_stack.iter() {
            contributes(&c.1, c.2, &mut min_top, &mut max_bottom);
        }
    }
    for (idx, p) in line.iter().enumerate() {
        let (parent_m, parent_shift) = ctx_stack.last().map(|c| (c.1, c.2)).unwrap_or((*strut, root_shift));
        match p.kind {
            PieceKind::Open(node) => {
                let m = inline_metrics(node);
                let shift = parent_shift + baseline_shift(node, &m, &parent_m, ctx);
                ctx_stack.push((node, m, shift));
                piece_shift[idx] = shift;
                if has_line_content {
                    contributes(&m, shift, &mut min_top, &mut max_bottom);
                }
            }
            PieceKind::Close(_) => {
                piece_shift[idx] = parent_shift;
                ctx_stack.pop();
            }
            PieceKind::Text(..) | PieceKind::Space(..) => {
                piece_shift[idx] = parent_shift;
            }
            PieceKind::Atomic(ai, node) => {
                let a = &atomics[ai];
                let h = a.lb.as_ref().map(|b| b.margin_box_height()).unwrap_or(0.0);
                match vertical_align_kw(node) {
                    Some("top") => aligned_edges.push((idx, true, h)),
                    Some("bottom") => aligned_edges.push((idx, false, h)),
                    Some("middle") => {
                        let t = parent_shift - parent_m.x_height / 2.0 - h / 2.0;
                        piece_shift[idx] = t;
                        min_top = min_top.min(t);
                        max_bottom = max_bottom.max(t + h);
                    }
                    Some("text-top") => {
                        let t = parent_shift - parent_m.ascent;
                        piece_shift[idx] = t;
                        min_top = min_top.min(t);
                        max_bottom = max_bottom.max(t + h);
                    }
                    Some("text-bottom") => {
                        let t = parent_shift + parent_m.descent - h;
                        piece_shift[idx] = t;
                        min_top = min_top.min(t);
                        max_bottom = max_bottom.max(t + h);
                    }
                    _ => {
                        let raise = match sval(node, "vertical-align") {
                            Some(Value::Keyword(k)) if k.as_ref() == "sub" => -(parent_m.ascent * 0.2 + 1.0),
                            Some(Value::Keyword(k)) if k.as_ref() == "super" => parent_m.ascent * 0.4 + 1.0,
                            Some(Value::Keyword(_)) | None => 0.0,
                            Some(v) => resolve_len(v, Some(line_height(node)), ctx).unwrap_or(0.0),
                        };
                        let t = parent_shift - raise - a.baseline_from_top;
                        piece_shift[idx] = t;
                        min_top = min_top.min(t);
                        max_bottom = max_bottom.max(t + h);
                    }
                }
            }
            _ => {}
        }
    }
    let mut line_h = if has_line_content { max_bottom - min_top } else { 0.0 };
    for &(idx, is_top, h) in &aligned_edges {
        if h > line_h {
            if is_top {
                max_bottom = min_top + h;
            } else {
                min_top = max_bottom - h;
            }
            line_h = h;
        }
        piece_shift[idx] = if is_top { min_top } else { max_bottom - h };
    }
    let baseline_y = snap(span.y - min_top);

    // ── Horizontal placement ──
    let free = avail - line_w;
    let offset = match text_align {
        "center" | "-webkit-center" => (free / 2.0).max(0.0),
        "right" | "end" | "-webkit-right" => free.max(0.0),
        _ => 0.0,
    };
    let mut x = snap(span.left + offset);

    // Fragment tree construction.
    let mut frag_stack: Vec<LayoutBox<'a>> = Vec::new();
    // Re-open continuing inline boxes.
    for node in open_stack.iter() {
        let m = inline_metrics(node);
        let shift = ctx_stack_shift_for(node, open_stack, strut, ctx);
        let mut fb = LayoutBox::new(node);
        let bm = box_model(node, cb.width, ctx);
        fb.padding = EdgeSizes { left: 0.0, right: 0.0, top: bm.padding.top, bottom: bm.padding.bottom };
        fb.border = EdgeSizes { left: 0.0, right: 0.0, top: bm.border.top, bottom: bm.border.bottom };
        fb.margin = EdgeSizes::default();
        fb.dimensions.x = x;
        fb.dimensions.y = baseline_y + shift - m.ascent - bm.padding.top - bm.border.top;
        fb.dimensions.height = m.ascent + m.descent + bm.padding.vertical() + bm.border.vertical();
        frag_stack.push(fb);
    }
    let push_child = |frag_stack: &mut Vec<LayoutBox<'a>>, out: &mut Vec<LayoutBox<'a>>, b: LayoutBox<'a>| {
        match frag_stack.last_mut() {
            Some(parent) => parent.children.push(b),
            None => out.push(b),
        }
    };
    let mut pending_text: Option<(LayoutBox<'a>, String)> = None;
    let flush_text = |pending_text: &mut Option<(LayoutBox<'a>, String)>, frag_stack: &mut Vec<LayoutBox<'a>>, out: &mut Vec<LayoutBox<'a>>| {
        if let Some((mut b, mut t)) = pending_text.take() {
            // A fragment that starts with a space (e.g. " more" after an inline
            // element) starts its glyphs after that space: move the space out of
            // the fragment so the rect begins at the first glyph.
            let body = t.trim_start();
            if body.len() != t.len() && !body.is_empty() {
                let lead = &t[..t.len() - body.len()];
                let lead_w = text_width(b.style_node, lead, font_size(b.style_node), letter_spacing(b.style_node));
                b.dimensions.x += lead_w;
                b.dimensions.width = (b.dimensions.width - lead_w).max(0.0);
                t = body.to_string();
            }
            // The painter wraps words that overflow the rect and ignores
            // letter-spacing; make sure the rect is wide enough for its own
            // measurement so a line fragment is never re-wrapped.
            let painter_w = text_width(b.style_node, t.trim(), font_size(b.style_node), letter_spacing(b.style_node));
            if painter_w > b.dimensions.width {
                b.dimensions.width = painter_w;
            }
            b.text_fragment = Some(t);
            match frag_stack.last_mut() {
                Some(parent) => parent.children.push(b),
                None => out.push(b),
            }
        }
    };
    for (idx, p) in line.iter().enumerate() {
        match &p.kind {
            PieceKind::Text(node, text) | PieceKind::Space(node, text) => {
                let node: &'a StyledNode = node;
                let same = pending_text.as_ref().map_or(false, |(b, _)| std::ptr::eq(b.style_node, node));
                if !same {
                    flush_text(&mut pending_text, &mut frag_stack, out);
                    let m = inline_metrics(node);
                    let mut b = LayoutBox::new(node);
                    b.dimensions = Rect { x, y: snap(baseline_y + piece_shift[idx] - m.ascent), width: 0.0, height: m.ascent + m.descent };
                    pending_text = Some((b, String::new()));
                }
                if let Some((b, t)) = pending_text.as_mut() {
                    t.push_str(text);
                    b.dimensions.width += p.width;
                }
                x += p.width;
            }
            PieceKind::Open(node) => {
                flush_text(&mut pending_text, &mut frag_stack, out);
                let m = inline_metrics(node);
                let bm = box_model(node, cb.width, ctx);
                let mut fb = LayoutBox::new(node).with_box_model(&bm);
                fb.margin.top = 0.0;
                fb.margin.bottom = 0.0;
                fb.dimensions.x = x + bm.margin.left;
                fb.dimensions.y = baseline_y + piece_shift[idx] - m.ascent - bm.padding.top - bm.border.top;
                fb.dimensions.height = m.ascent + m.descent + bm.padding.vertical() + bm.border.vertical();
                x += p.width;
                frag_stack.push(fb);
                open_stack.push(node);
            }
            PieceKind::Close(_) => {
                flush_text(&mut pending_text, &mut frag_stack, out);
                if let Some(mut fb) = frag_stack.pop() {
                    let right_edge = fb.margin.right;
                    x += p.width;
                    fb.dimensions.width = (x - right_edge - fb.dimensions.x).max(0.0);
                    finish_inline_fragment(&mut fb, cb, ctx);
                    push_child(&mut frag_stack, out, fb);
                }
                open_stack.pop();
            }
            PieceKind::Atomic(ai, node) => {
                flush_text(&mut pending_text, &mut frag_stack, out);
                if let Some(mut b) = atomics[*ai].lb.take() {
                    let target_x = x + b.margin.left;
                    let target_y = baseline_y + piece_shift[idx] + b.margin.top;
                    let dx = target_x - b.dimensions.x;
                    let dy = target_y - b.dimensions.y;
                    offset_layout_box(&mut b, dx, dy);
                    apply_relative_offset(&mut b, cb.width, cb.height, ctx);
                    let _ = node;
                    push_child(&mut frag_stack, out, b);
                }
                x += p.width;
            }
            PieceKind::Abs(node) => {
                flush_text(&mut pending_text, &mut frag_stack, out);
                // A block-level box's static position is the start of the line.
                let ph_x = if is_inline_level_display(node) { x } else { span.left };
                let ph = make_abs_placeholder(node, ph_x, span.y, ctx);
                push_child(&mut frag_stack, out, ph);
            }
            PieceKind::Break(node) => {
                // <br> gets a zero-width box so hit testing and ids still see it.
                flush_text(&mut pending_text, &mut frag_stack, out);
                if matches!(node.node.data, NodeData::Element { .. }) {
                    let mut b = LayoutBox::new(node);
                    b.dimensions = Rect { x, y: span.y, width: 0.0, height: line_h };
                    push_child(&mut frag_stack, out, b);
                }
            }
            PieceKind::Float(_) => {}
        }
    }
    flush_text(&mut pending_text, &mut frag_stack, out);
    // Close fragments that continue on the next line.
    while let Some(mut fb) = frag_stack.pop() {
        fb.margin.right = 0.0;
        fb.padding.right = 0.0;
        fb.border.right = 0.0;
        fb.dimensions.width = (x - fb.dimensions.x).max(0.0);
        finish_inline_fragment(&mut fb, cb, ctx);
        push_child(&mut frag_stack, out, fb);
    }
    let _ = container;
    (snap(line_h), baseline_y, line_w)
}

/// Shift of a continuing inline box (re-derived from the open stack).
fn ctx_stack_shift_for(node: &StyledNode, open_stack: &[&StyledNode], strut: &InlineMetrics, ctx: &Ctx) -> f32 {
    let mut parent_m = *strut;
    let mut shift = 0.0;
    for n in open_stack {
        let m = inline_metrics(n);
        shift += baseline_shift(n, &m, &parent_m, ctx);
        parent_m = m;
        if std::ptr::eq(*n, node) {
            break;
        }
    }
    shift
}

fn finish_inline_fragment(fb: &mut LayoutBox, cb: Cb, ctx: &mut Ctx) {
    if fb.position != PositionType::Static && ctx.pending_abs > 0 {
        let rect = fb.padding_box();
        resolve_pending_abs(fb, rect, false, ctx);
    }
    apply_relative_offset(fb, cb.width, cb.height, ctx);
}

/// Truncate a line's pieces so that they fit `avail` with a trailing ellipsis.
fn truncate_with_ellipsis(line: &mut Vec<Piece>, avail: f32, always: bool) {
    // Find the text node that will carry the ellipsis.
    let mut x = 0.0;
    let mut cut: Option<usize> = None;
    let mut ellipsis_node = None;
    for p in line.iter() {
        if let PieceKind::Text(n, _) | PieceKind::Space(n, _) = p.kind {
            ellipsis_node = Some(n);
            break;
        }
    }
    let Some(enode) = ellipsis_node else { return };
    let ew = text_width(enode, "\u{2026}", font_size(enode), letter_spacing(enode));
    let limit = (avail - ew).max(0.0);
    let total: f32 = line.iter().map(|p| p.width).sum();
    if !always && total <= avail + 0.01 {
        return;
    }
    for (idx, p) in line.iter().enumerate() {
        if matches!(p.kind, PieceKind::Close(_) | PieceKind::Float(_) | PieceKind::Abs(_) | PieceKind::Break(_)) {
            continue;
        }
        if x + p.width > limit + 0.01 {
            cut = Some(idx);
            break;
        }
        x += p.width;
    }
    let cut = match cut {
        Some(c) => c,
        None => {
            if always {
                // Everything fits: append the ellipsis after the last text piece.
                if let Some(pos) = line.iter().rposition(|p| matches!(p.kind, PieceKind::Text(..))) {
                    if let PieceKind::Text(n, ref mut t) = line[pos].kind {
                        t.push('\u{2026}');
                        line[pos].width += text_width(n, "\u{2026}", font_size(n), letter_spacing(n));
                    }
                }
            }
            return;
        }
    };
    // Keep the part of the cut piece that fits (text only).
    let mut new_line: Vec<Piece> = line[..cut].to_vec();
    let mut appended = false;
    if let PieceKind::Text(n, ref text) = line[cut].kind {
        let fs = font_size(n);
        let ls = letter_spacing(n);
        let mut acc = x;
        let mut kept = String::new();
        for c in text.chars() {
            let cw = char_advance(n, c, fs) + ls;
            if acc + cw > limit + 0.01 {
                break;
            }
            acc += cw;
            kept.push(c);
        }
        kept.push('\u{2026}');
        let w = text_width(n, &kept, fs, ls);
        new_line.push(Piece { kind: PieceKind::Text(n, kept), width: w, break_before: false, collapsible: false, break_anywhere: false });
        appended = true;
    }
    if !appended {
        // Drop trailing spaces, then append the ellipsis to the last text piece or a new one.
        while new_line.last().map_or(false, |p| p.is_space()) {
            new_line.pop();
        }
        new_line.push(Piece { kind: PieceKind::Text(enode, "\u{2026}".to_string()), width: ew, break_before: false, collapsible: false, break_anywhere: false });
    }
    // Keep closing pieces (whose opening piece was kept) so fragments close properly.
    let mut dropped_opens = 0usize;
    let start_rest = if appended { cut + 1 } else { cut };
    for p in line[start_rest..].iter() {
        match p.kind {
            PieceKind::Open(_) => dropped_opens += 1,
            PieceKind::Close(_) => {
                if dropped_opens > 0 {
                    dropped_opens -= 1;
                } else {
                    let mut q = p.clone();
                    q.width = 0.0;
                    new_line.push(q);
                }
            }
            PieceKind::Abs(_) => new_line.push(p.clone()),
            _ => {}
        }
    }
    *line = new_line;
}

// ── Flexbox ───────────────────────────────────────────────────────────────────

struct FlexOut {
    /// Content-box height of the container.
    height: f32,
    first_baseline: Option<f32>,
}

fn gap_px(sn: &StyledNode, prop: &str, basis: Option<f32>, ctx: &Ctx) -> f32 {
    match sval(sn, prop) {
        Some(Value::Number(n)) => *n,
        Some(v) => resolve_len(v, basis, ctx).unwrap_or(0.0),
        None => 0.0,
    }
    .max(0.0)
}

fn flex_factor(sn: &StyledNode, prop: &str, default: f32) -> f32 {
    match sval(sn, prop) {
        Some(Value::Number(n)) => n.max(0.0),
        _ => default,
    }
}

/// A child of a flex container that takes part in flex layout.
enum FlexChild<'a> {
    Element(&'a StyledNode),
    /// A run of non-whitespace text wrapped in an anonymous block.
    Text(&'a StyledNode),
}

impl<'a> FlexChild<'a> {
    fn node(&self) -> &'a StyledNode {
        match self {
            FlexChild::Element(n) | FlexChild::Text(n) => n,
        }
    }
}

struct FlexItem<'a> {
    child: FlexChild<'a>,
    bm: BoxModel,
    grow: f32,
    shrink: f32,
    /// Border-box flex base size.
    basis: f32,
    min_main: f32,
    max_main: f32,
    /// Border-box main size after flexing.
    main: f32,
    frozen: bool,
    /// Laid-out box (valid after the cross-size pass).
    lb: Option<LayoutBox<'a>>,
    baseline: Option<f32>,
    cross: f32,
    align: String,
    auto_main_start: bool,
    auto_main_end: bool,
    auto_cross_start: bool,
    auto_cross_end: bool,
}

impl<'a> FlexItem<'a> {
    fn outer_main(&self, row: bool) -> f32 {
        self.main + if row { self.bm.margin.horizontal() } else { self.bm.margin.vertical() }
    }
    fn outer_cross(&self, row: bool) -> f32 {
        self.cross + if row { self.bm.margin.vertical() } else { self.bm.margin.horizontal() }
    }
}

/// Lay out an anonymous block holding one text node (used for text directly
/// inside flex/grid containers). The box's `text_fragment` is empty so the
/// painter draws only its line fragments.
fn layout_anon_text<'a>(text: &'a StyledNode, container: &'a StyledNode, width: f32, cb: Cb, ctx: &mut Ctx) -> (LayoutBox<'a>, Option<f32>) {
    let mut holder = LayoutBox::new(text);
    holder.display = DisplayType::Block;
    holder.text_fragment = Some(String::new());
    let mut kids = Vec::new();
    let out = layout_inline_run(&[text], container, 0.0, 0.0, width, cb, &mut FloatCtx::default(), ctx, &mut kids);
    holder.children = kids;
    holder.dimensions = Rect { x: 0.0, y: 0.0, width, height: out.height };
    (holder, out.first_baseline)
}

fn flex_children<'a>(sn: &'a StyledNode) -> Vec<&'a StyledNode> {
    let mut v = Vec::new();
    for c in &sn.children {
        if should_skip(c) {
            continue;
        }
        if skw(c, "display") == Some("contents") && matches!(c.node.data, NodeData::Element { .. }) {
            v.extend(flex_children(c));
            continue;
        }
        v.push(c);
    }
    v
}

fn layout_flex<'a>(
    lb: &mut LayoutBox<'a>,
    content_x: f32,
    content_y: f32,
    content_w: f32,
    cb: Cb,
    height_definite: bool,
    ctx: &mut Ctx,
) -> FlexOut {
    let sn = lb.style_node;
    let legacy_box = matches!(skw(sn, "display"), Some("-webkit-box" | "-webkit-inline-box"));
    let dir = if legacy_box {
        if skw(sn, "-webkit-box-orient") == Some("vertical") { "column" } else { "row" }
    } else {
        skw(sn, "flex-direction").unwrap_or("row")
    };
    let row = dir == "row" || dir == "row-reverse";
    let reverse = dir.ends_with("-reverse");
    let wrap_kw = skw(sn, "flex-wrap").unwrap_or("nowrap");
    let wrap = wrap_kw == "wrap" || wrap_kw == "wrap-reverse";
    let wrap_reverse = wrap_kw == "wrap-reverse";
    let container_cross_def: Option<f32> = if row {
        if height_definite { cb.height } else { None }
    } else {
        Some(content_w)
    };
    let main_avail: Option<f32> = if row { Some(content_w) } else if height_definite { cb.height } else { None };
    let main_gap = if row { gap_px(sn, "column-gap", Some(content_w), ctx) } else { gap_px(sn, "row-gap", cb.height, ctx) };
    let cross_gap = if row { gap_px(sn, "row-gap", cb.height, ctx) } else { gap_px(sn, "column-gap", Some(content_w), ctx) };
    let align_items = skw(sn, "align-items").unwrap_or("normal").to_string();
    let justify = if legacy_box {
        match skw(sn, "-webkit-box-pack") {
            Some("center") => "center",
            Some("end") => "flex-end",
            Some("justify") => "space-between",
            _ => "flex-start",
        }
        .to_string()
    } else {
        skw(sn, "justify-content").unwrap_or("normal").to_string()
    };
    let align_content = skw(sn, "align-content").unwrap_or("normal").to_string();
    let item_cb = Cb { width: content_w, height: cb.height };

    // ── Collect items ──
    let mut children: Vec<(i32, usize, FlexChild<'a>)> = Vec::new();
    for (idx, c) in flex_children(sn).into_iter().enumerate() {
        if is_text(c) {
            if is_collapsible_whitespace_text(c) {
                continue;
            }
            children.push((0, idx, FlexChild::Text(c)));
            continue;
        }
        if is_out_of_flow_positioned(c) {
            lb.children.push(make_abs_placeholder(c, content_x, content_y, ctx));
            continue;
        }
        let order = match sval(c, "order").or_else(|| if legacy_box { sval(c, "-webkit-box-ordinal-group") } else { None }) {
            Some(Value::Number(n)) => *n as i32,
            _ => 0,
        };
        children.push((order, idx, FlexChild::Element(c)));
    }
    children.sort_by_key(|(o, i, _)| (*o, *i));

    let mut items: Vec<FlexItem<'a>> = Vec::new();
    for (_, _, child) in children {
        let node = child.node();
        let is_el = matches!(child, FlexChild::Element(_));
        let bm = if is_el { box_model(node, content_w, ctx) } else { BoxModel::default() };
        let (grow, shrink) = if is_el {
            if legacy_box {
                (flex_factor(node, "-webkit-box-flex", 0.0), flex_factor(node, "-webkit-box-flex", 0.0))
            } else {
                (flex_factor(node, "flex-grow", 0.0), flex_factor(node, "flex-shrink", 1.0))
            }
        } else {
            (0.0, 1.0)
        };
        let align = if is_el {
            match skw(node, "align-self") {
                Some(a) if a != "auto" => a.to_string(),
                _ => align_items.clone(),
            }
        } else {
            align_items.clone()
        };
        let (auto_ms, auto_me, auto_cs, auto_ce) = if row {
            (bm.margin_auto_left, bm.margin_auto_right, bm.margin_auto_top, bm.margin_auto_bottom)
        } else {
            (bm.margin_auto_top, bm.margin_auto_bottom, bm.margin_auto_left, bm.margin_auto_right)
        };
        items.push(FlexItem {
            child,
            bm,
            grow,
            shrink,
            basis: 0.0,
            min_main: 0.0,
            max_main: f32::INFINITY,
            main: 0.0,
            frozen: false,
            lb: None,
            baseline: None,
            cross: 0.0,
            align,
            auto_main_start: auto_ms,
            auto_main_end: auto_me,
            auto_cross_start: auto_cs,
            auto_cross_end: auto_ce,
        });
    }

    // ── Flex base sizes ──
    for item in items.iter_mut() {
        let node = item.child.node();
        let is_el = matches!(item.child, FlexChild::Element(_));
        if !is_el {
            let (min_c, max_c) = inline_intrinsic(&[node], sn, ctx);
            if row {
                item.basis = max_c;
                item.min_main = min_c;
            } else {
                let (b, _) = layout_anon_text(node, sn, content_w, item_cb, ctx);
                item.basis = b.dimensions.height;
                item.min_main = item.basis;
            }
            item.main = item.basis;
            continue;
        }
        let bm = item.bm;
        let pb_main = if row { bm.pb_h() } else { bm.pb_v() };
        let basis_v = sval(node, "flex-basis");
        let to_border = |v: f32| if is_border_box(node) { v.max(pb_main) } else { v.max(0.0) + pb_main };
        let mut basis: Option<f32> = match basis_v {
            Some(Value::Keyword(k)) if matches!(k.as_ref(), "auto" | "content") => None,
            Some(v @ Value::Length(..)) | Some(v @ Value::Number(_)) => {
                let r = match v {
                    Value::Number(n) => Some(*n),
                    _ => resolve_len(v, main_avail, ctx),
                };
                r.map(to_border)
            }
            Some(v @ Value::Keyword(_)) => resolve_len(v, main_avail, ctx).map(to_border),
            _ => None,
        };
        let is_content_basis = matches!(basis_v, Some(Value::Keyword(k)) if k.as_ref() == "content");
        if basis.is_none() && !is_content_basis {
            basis = if row {
                specified_border_width(node, Some(content_w), &bm, ctx)
            } else {
                specified_border_height(node, if height_definite { cb.height } else { None }, &bm, ctx)
            };
        }
        let content_size = |ctx: &mut Ctx| -> f32 {
            if row {
                if inner_display(node) == Inner::Replaced {
                    replaced_size(node, &bm, item_cb, ctx).0
                } else {
                    intrinsic_widths(node, ctx).1
                }
            } else {
                // Column: lay out at the cross size to measure the height.
                let cross_w = column_item_cross_width(node, &bm, content_w, &item_cb, ctx);
                layout_block_level(node, 0.0, 0.0, item_cb, None, BlockOpts { forced_width: Some(cross_w), ..Default::default() }, ctx)
                    .map(|o| o.lb.dimensions.height)
                    .unwrap_or(0.0)
            }
        };
        let basis_px = match basis {
            Some(b) => b,
            None => content_size(ctx),
        };
        item.basis = basis_px;
        // min / max main size
        let (min_prop, max_prop) = if row { ("min-width", "max-width") } else { ("min-height", "max-height") };
        let pct_base = if row { Some(content_w) } else { main_avail };
        let min_spec = prop_len(node, min_prop, pct_base, ctx).map(to_border);
        let max_spec = prop_len(node, max_prop, pct_base, ctx).map(to_border);
        item.max_main = max_spec.unwrap_or(f32::INFINITY);
        item.min_main = match min_spec {
            Some(m) => m,
            None => {
                let explicit_auto = matches!(sval(node, min_prop), Some(Value::Keyword(k)) if k.as_ref() == "auto") || sval(node, min_prop).is_none();
                if explicit_auto && !overflow_clips(node) {
                    // Automatic minimum size: content size, capped by a definite size.
                    let content_min = if row {
                        if inner_display(node) == Inner::Replaced {
                            replaced_size(node, &bm, item_cb, ctx).0
                        } else {
                            intrinsic_widths(node, ctx).0
                        }
                    } else {
                        content_size(ctx)
                    };
                    let spec = if row {
                        specified_border_width(node, Some(content_w), &bm, ctx)
                    } else {
                        specified_border_height(node, main_avail, &bm, ctx)
                    };
                    match spec {
                        Some(s) => content_min.min(s),
                        None => content_min,
                    }
                    .min(item.max_main)
                } else {
                    pb_main
                }
            }
        };
        item.main = item.basis.max(item.min_main).min(item.max_main).max(item.min_main.min(item.max_main));
    }

    // ── Lines ──
    let mut lines: Vec<Vec<usize>> = Vec::new();
    {
        let mut cur: Vec<usize> = Vec::new();
        let mut used = 0.0;
        for (i, item) in items.iter().enumerate() {
            let om = item.outer_main(row);
            let add = if cur.is_empty() { om } else { main_gap + om };
            if wrap && !cur.is_empty() && main_avail.map_or(false, |a| used + add > a + 0.01) {
                lines.push(std::mem::take(&mut cur));
                used = 0.0;
                cur.push(i);
                used += om;
                continue;
            }
            used += add;
            cur.push(i);
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }

    // ── Resolve flexible lengths per line ──
    if let Some(avail) = main_avail {
        for line in &lines {
            resolve_flexible_lengths(&mut items, line, avail, main_gap, row);
        }
    }

    // ── Cross sizes: lay out items with their main size ──
    for item in items.iter_mut() {
        let node = item.child.node();
        match item.child {
            FlexChild::Text(t) => {
                let width = if row { item.main } else { content_w };
                let (mut b, bl) = layout_anon_text(t, sn, width, item_cb, ctx);
                if !row {
                    b.dimensions.height = item.main;
                }
                item.cross = if row { b.dimensions.height } else { b.dimensions.width };
                item.baseline = bl;
                item.lb = Some(b);
            }
            FlexChild::Element(_) => {
                let bm = item.bm;
                let opts = if row {
                    BlockOpts { forced_width: Some(item.main), ..Default::default() }
                } else {
                    let cross_w = column_item_cross_width(node, &bm, content_w, &item_cb, ctx);
                    BlockOpts { forced_width: Some(cross_w), forced_height: Some(item.main), ..Default::default() }
                };
                if let Some(out) = layout_block_level(node, 0.0, 0.0, item_cb, None, opts, ctx) {
                    let b = out.lb;
                    item.cross = if row { b.dimensions.height } else { b.dimensions.width };
                    item.baseline = out.first_baseline.map(|v| v - b.dimensions.y);
                    item.lb = Some(b);
                }
            }
        }
    }

    // Line cross sizes.
    let mut line_cross: Vec<f32> = Vec::new();
    for line in &lines {
        let mut max_outer: f32 = 0.0;
        // Baseline-aligned items share a baseline.
        let mut max_above: f32 = 0.0;
        let mut max_below: f32 = 0.0;
        for &i in line {
            let it = &items[i];
            if row && it.align == "baseline" {
                let bl = it.baseline.unwrap_or(it.cross) + it.bm.margin.top;
                max_above = max_above.max(bl);
                max_below = max_below.max(it.outer_cross(row) - bl);
            } else {
                max_outer = max_outer.max(it.outer_cross(row));
            }
        }
        max_outer = max_outer.max(max_above + max_below);
        line_cross.push(max_outer);
    }
    if lines.len() == 1 {
        if let Some(c) = container_cross_def {
            line_cross[0] = c;
        }
    }
    // align-content: stretch/normal distributes extra cross space to lines.
    let total_cross: f32 = line_cross.iter().sum::<f32>() + cross_gap * (lines.len().saturating_sub(1)) as f32;
    let mut cross_start_offset = 0.0;
    let mut cross_between = 0.0;
    if lines.len() > 1 {
        if let Some(c) = container_cross_def {
            let free = c - total_cross;
            match align_content.as_str() {
                "center" => cross_start_offset = free / 2.0,
                "flex-end" | "end" => cross_start_offset = free,
                "space-between" if free > 0.0 => cross_between = free / (lines.len() - 1) as f32,
                "space-around" if free > 0.0 => {
                    cross_between = free / lines.len() as f32;
                    cross_start_offset = cross_between / 2.0;
                }
                "flex-start" | "start" | "baseline" => {}
                _ => {
                    if free > 0.0 {
                        let extra = free / lines.len() as f32;
                        for lc in line_cross.iter_mut() {
                            *lc += extra;
                        }
                    }
                }
            }
        }
    }

    // Stretch: re-layout items whose cross size must fill the line.
    for (li, line) in lines.iter().enumerate() {
        for &i in line {
            let it = &mut items[i];
            let stretch = matches!(it.align.as_str(), "stretch" | "normal") && !it.auto_cross_start && !it.auto_cross_end;
            if !stretch {
                continue;
            }
            let node = it.child.node();
            let cross_auto = match it.child {
                FlexChild::Text(_) => true,
                FlexChild::Element(_) => {
                    if row {
                        is_auto_height(node, ctx)
                    } else {
                        is_auto(node, "width")
                    }
                }
            };
            if !cross_auto {
                continue;
            }
            let margins = if row { it.bm.margin.vertical() } else { it.bm.margin.horizontal() };
            let mut target = (line_cross[li] - margins).max(0.0);
            if let FlexChild::Element(_) = it.child {
                let bm = it.bm;
                target = if row {
                    clamp_border_height(node, target, cb.height, &bm, ctx)
                } else {
                    clamp_border_width(node, target, Some(content_w), &bm, ctx)
                };
            }
            if (target - it.cross).abs() < 0.01 {
                continue;
            }
            match it.child {
                FlexChild::Text(_) => {
                    if let Some(b) = it.lb.as_mut() {
                        if row {
                            b.dimensions.height = target;
                        } else {
                            b.dimensions.width = target;
                        }
                    }
                    it.cross = target;
                }
                FlexChild::Element(_) => {
                    let opts = if row {
                        BlockOpts { forced_width: Some(it.main), forced_height: Some(target), ..Default::default() }
                    } else {
                        BlockOpts { forced_width: Some(target), forced_height: Some(it.main), ..Default::default() }
                    };
                    if let Some(out) = layout_block_level(node, 0.0, 0.0, item_cb, None, opts, ctx) {
                        let b = out.lb;
                        it.cross = target;
                        it.baseline = out.first_baseline.map(|v| v - b.dimensions.y);
                        it.lb = Some(b);
                    }
                }
            }
        }
    }

    // ── Main-axis sizes of the container ──
    let used_main: Vec<f32> = lines
        .iter()
        .map(|line| line.iter().map(|&i| items[i].outer_main(row)).sum::<f32>() + main_gap * (line.len().saturating_sub(1)) as f32)
        .collect();
    let container_main = match main_avail {
        Some(a) => a,
        None => used_main.iter().cloned().fold(0.0, f32::max),
    };

    // ── Positioning ──
    let mut first_baseline: Option<f32> = None;
    let mut cross_cursor = cross_start_offset;
    let line_order: Vec<usize> = if wrap_reverse { (0..lines.len()).rev().collect() } else { (0..lines.len()).collect() };
    let mut placed: Vec<(usize, f32, f32)> = Vec::new(); // (item idx, main pos, cross pos) relative to content box
    for &li in &line_order {
        let line = &lines[li];
        let lc = line_cross[li];
        let mut free = container_main - used_main[li];
        let auto_count: usize = line.iter().map(|&i| items[i].auto_main_start as usize + items[i].auto_main_end as usize).sum();
        let auto_each = if free > 0.0 && auto_count > 0 { free / auto_count as f32 } else { 0.0 };
        if auto_count > 0 && free > 0.0 {
            free = 0.0;
        }
        let n = line.len() as f32;
        let (mut main_cursor, between) = match justify.as_str() {
            "flex-end" | "end" | "right" => (free, 0.0),
            "center" => (free / 2.0, 0.0),
            "space-between" => (0.0, if n > 1.0 && free > 0.0 { free / (n - 1.0) } else { 0.0 }),
            "space-around" => {
                if free > 0.0 {
                    (free / n / 2.0, free / n)
                } else {
                    (free / 2.0, 0.0)
                }
            }
            "space-evenly" => {
                if free > 0.0 {
                    (free / (n + 1.0), free / (n + 1.0))
                } else {
                    (free / 2.0, 0.0)
                }
            }
            _ => (0.0, 0.0),
        };
        // Line baseline for baseline alignment.
        let line_baseline = line
            .iter()
            .filter(|&&i| row && items[i].align == "baseline")
            .map(|&i| items[i].baseline.unwrap_or(items[i].cross) + items[i].bm.margin.top)
            .fold(0.0f32, f32::max);
        for (k, &i) in line.iter().enumerate() {
            let it = &items[i];
            if k > 0 {
                main_cursor += main_gap + between;
            }
            let (m_start, m_end) = if row { (it.bm.margin.left, it.bm.margin.right) } else { (it.bm.margin.top, it.bm.margin.bottom) };
            let (c_start, c_end) = if row { (it.bm.margin.top, it.bm.margin.bottom) } else { (it.bm.margin.left, it.bm.margin.right) };
            if it.auto_main_start {
                main_cursor += auto_each;
            }
            let main_pos = main_cursor + m_start;
            main_cursor += it.main + m_start + m_end;
            if it.auto_main_end {
                main_cursor += auto_each;
            }
            let outer_cross = it.cross + c_start + c_end;
            let free_cross = lc - outer_cross;
            let cross_pos = if it.auto_cross_start && it.auto_cross_end {
                free_cross.max(0.0) / 2.0 + c_start
            } else if it.auto_cross_start {
                free_cross.max(0.0) + c_start
            } else if it.auto_cross_end {
                c_start
            } else {
                match it.align.as_str() {
                    "flex-end" | "end" | "self-end" => free_cross + c_start,
                    "center" => free_cross / 2.0 + c_start,
                    "baseline" if row => line_baseline - it.baseline.unwrap_or(it.cross) - it.bm.margin.top + c_start,
                    _ => c_start,
                }
            };
            placed.push((i, main_pos, cross_cursor + cross_pos));
        }
        cross_cursor += lc + cross_gap + cross_between;
    }
    let cross_used = (cross_cursor - cross_gap - cross_between).max(0.0);
    let content_h = if row {
        match container_cross_def {
            Some(c) => c,
            None => cross_used,
        }
    } else {
        match main_avail {
            Some(a) => a,
            None => container_main,
        }
    };

    let main_extent = if row { content_w } else { content_h };
    let mut boxes: Vec<(usize, LayoutBox<'a>)> = Vec::new();
    for (i, main_pos, cross_pos) in placed {
        let it = &mut items[i];
        let Some(mut b) = it.lb.take() else { continue };
        let main_size = it.main;
        let main_pos = if reverse { main_extent - main_pos - main_size } else { main_pos };
        let (x, y) = if row { (content_x + main_pos, content_y + cross_pos) } else { (content_x + cross_pos, content_y + main_pos) };
        let dx = x - b.dimensions.x;
        let dy = y - b.dimensions.y;
        offset_layout_box(&mut b, dx, dy);
        if first_baseline.is_none() {
            first_baseline = Some(match it.baseline {
                Some(bl) => b.dimensions.y + bl,
                None => b.dimensions.y + b.dimensions.height,
            });
        }
        if let Some(m) = matches!(it.child, FlexChild::Element(_)).then_some(()) {
            let _ = m;
            b.margin = it.bm.margin;
            apply_relative_offset(&mut b, content_w, cb.height, ctx);
        }
        boxes.push((i, b));
    }
    // Keep DOM (order-modified) paint order.
    boxes.sort_by_key(|(i, _)| *i);
    for (_, b) in boxes {
        lb.children.push(b);
    }
    FlexOut { height: content_h.max(0.0), first_baseline }
}

/// Cross (width) size of an item in a column flex container before stretching.
fn column_item_cross_width(node: &StyledNode, bm: &BoxModel, content_w: f32, cb: &Cb, ctx: &mut Ctx) -> f32 {
    if let Some(w) = specified_border_width(node, Some(content_w), bm, ctx) {
        return clamp_border_width(node, w, Some(content_w), bm, ctx);
    }
    let avail = (content_w - bm.margin.horizontal()).max(0.0);
    let parent_align = "stretch";
    let self_align = skw(node, "align-self").filter(|a| *a != "auto");
    let align = self_align.unwrap_or(parent_align);
    let _ = cb;
    if matches!(align, "stretch" | "normal") && !bm.margin_auto_left && !bm.margin_auto_right {
        clamp_border_width(node, avail, Some(content_w), bm, ctx)
    } else {
        let (min_c, max_c) = if inner_display(node) == Inner::Replaced {
            let s = replaced_size(node, bm, *cb, ctx).0;
            (s, s)
        } else {
            intrinsic_widths(node, ctx)
        };
        clamp_border_width(node, max_c.min(avail).max(min_c), Some(content_w), bm, ctx)
    }
}

/// CSS Flexbox §9.7 "Resolving Flexible Lengths" for one line.
fn resolve_flexible_lengths(items: &mut [FlexItem], line: &[usize], avail: f32, gap: f32, row: bool) {
    let margins = |it: &FlexItem| if row { it.bm.margin.horizontal() } else { it.bm.margin.vertical() };
    let gaps = gap * (line.len().saturating_sub(1)) as f32;
    let hypo_sum: f32 = line.iter().map(|&i| items[i].main + margins(&items[i])).sum::<f32>() + gaps;
    let growing = hypo_sum < avail;
    for &i in line {
        let it = &mut items[i];
        let factor = if growing { it.grow } else { it.shrink };
        it.frozen = factor == 0.0 || (growing && it.basis > it.main) || (!growing && it.basis < it.main);
        if it.frozen {
            // keep hypothetical size
        } else {
            it.main = it.basis;
        }
    }
    let initial_free = {
        let used: f32 = line.iter().map(|&i| {
            let it = &items[i];
            (if it.frozen { it.main } else { it.basis }) + margins(it)
        }).sum::<f32>() + gaps;
        avail - used
    };
    for _ in 0..16 {
        if line.iter().all(|&i| items[i].frozen) {
            break;
        }
        let used: f32 = line.iter().map(|&i| {
            let it = &items[i];
            (if it.frozen { it.main } else { it.basis }) + margins(it)
        }).sum::<f32>() + gaps;
        let mut free = avail - used;
        let sum_factors: f32 = line.iter().filter(|&&i| !items[i].frozen).map(|&i| if growing { items[i].grow } else { items[i].shrink }).sum();
        if sum_factors < 1.0 {
            let scaled = initial_free * sum_factors;
            if scaled.abs() < free.abs() {
                free = scaled;
            }
        }
        let mut total_violation = 0.0;
        let mut targets: Vec<(usize, f32)> = Vec::new();
        if growing {
            for &i in line {
                let it = &items[i];
                if it.frozen {
                    continue;
                }
                let t = it.basis + if sum_factors > 0.0 { free * it.grow / sum_factors } else { 0.0 };
                targets.push((i, t));
            }
        } else {
            let sum_scaled: f32 = line.iter().filter(|&&i| !items[i].frozen).map(|&i| items[i].shrink * items[i].basis).sum();
            for &i in line {
                let it = &items[i];
                if it.frozen {
                    continue;
                }
                let t = if sum_scaled > 0.0 { it.basis + free * (it.shrink * it.basis) / sum_scaled } else { it.basis };
                targets.push((i, t));
            }
        }
        let mut clamped: Vec<(usize, f32, f32)> = Vec::new();
        for (i, t) in targets {
            let it = &items[i];
            let c = t.min(it.max_main).max(it.min_main).max(0.0);
            total_violation += c - t;
            clamped.push((i, t, c));
        }
        for &(i, t, c) in &clamped {
            let it = &mut items[i];
            it.main = c;
            if total_violation.abs() < 0.01 {
                it.frozen = true;
            } else if total_violation > 0.0 && c > t {
                it.frozen = true;
            } else if total_violation < 0.0 && c < t {
                it.frozen = true;
            }
        }
        if total_violation.abs() < 0.01 {
            break;
        }
    }
    for &i in line {
        let it = &mut items[i];
        it.main = it.main.max(0.0);
    }
}

// ── Tables (automatic layout, simplified) ─────────────────────────────────────

fn is_row_group(sn: &StyledNode) -> bool {
    match skw(sn, "display") {
        Some(d) => matches!(d, "table-row-group" | "table-header-group" | "table-footer-group"),
        None => matches!(tag_name(sn).as_deref(), Some("tbody" | "thead" | "tfoot")),
    }
}

fn is_table_row(sn: &StyledNode) -> bool {
    match skw(sn, "display") {
        Some(d) => d == "table-row",
        None => is_tag(sn, "tr"),
    }
}

fn is_table_cell(sn: &StyledNode) -> bool {
    match skw(sn, "display") {
        Some(d) => d == "table-cell",
        None => matches!(tag_name(sn).as_deref(), Some("td" | "th")),
    }
}

/// Rows of a table: (optional row group, row, cells).
fn table_rows<'a>(table: &'a StyledNode) -> Vec<(Option<&'a StyledNode>, &'a StyledNode, Vec<&'a StyledNode>)> {
    let mut rows = Vec::new();
    let cells_of = |row: &'a StyledNode| -> Vec<&'a StyledNode> {
        row.children.iter().filter(|c| !should_skip(c) && is_table_cell(c)).collect()
    };
    for c in &table.children {
        if should_skip(c) || !matches!(c.node.data, NodeData::Element { .. }) {
            continue;
        }
        if is_row_group(c) {
            for r in &c.children {
                if !should_skip(r) && is_table_row(r) {
                    rows.push((Some(c), r, cells_of(r)));
                }
            }
        } else if is_table_row(c) {
            rows.push((None, c, cells_of(c)));
        }
    }
    rows
}

fn colspan(cell: &StyledNode) -> usize {
    attr(cell, "colspan").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 1000)
}

fn table_spacing(table: &StyledNode) -> f32 {
    if skw(table, "border-collapse") == Some("collapse") {
        return 0.0;
    }
    match sval(table, "border-spacing") {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Keyword(k)) => k.split_whitespace().next().and_then(|p| p.trim_end_matches("px").parse().ok()).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Per-column (min, max) widths.
fn table_columns(table: &StyledNode, ctx: &mut Ctx) -> Vec<(f32, f32)> {
    let rows = table_rows(table);
    let ncols = rows.iter().map(|(_, _, cells)| cells.iter().map(|c| colspan(c)).sum::<usize>()).max().unwrap_or(0);
    let mut cols = vec![(0.0f32, 0.0f32); ncols];
    for (_, _, cells) in &rows {
        let mut col = 0;
        for cell in cells {
            let span = colspan(cell);
            let (mut mn, mut mx) = intrinsic_widths(cell, ctx);
            let bm = box_model(cell, 0.0, ctx);
            if let Some(w) = specified_border_width(cell, None, &bm, ctx) {
                mx = w.max(mn);
                mn = mn.max(w.min(mx));
            }
            if span == 1 {
                if col < ncols {
                    cols[col].0 = cols[col].0.max(mn);
                    cols[col].1 = cols[col].1.max(mx);
                }
            } else {
                let each_min = mn / span as f32;
                let each_max = mx / span as f32;
                for k in col..(col + span).min(ncols) {
                    cols[k].0 = cols[k].0.max(each_min);
                    cols[k].1 = cols[k].1.max(each_max);
                }
            }
            col += span;
        }
    }
    cols
}

fn table_intrinsic(table: &StyledNode, ctx: &mut Ctx) -> (f32, f32) {
    let cols = table_columns(table, ctx);
    let sp = table_spacing(table);
    let n = cols.len() as f32;
    let extra = if cols.is_empty() { 0.0 } else { sp * (n + 1.0) };
    (cols.iter().map(|c| c.0).sum::<f32>() + extra, cols.iter().map(|c| c.1).sum::<f32>() + extra)
}

fn layout_table<'a>(lb: &mut LayoutBox<'a>, content_x: f32, content_y: f32, content_w: f32, _cb: Cb, ctx: &mut Ctx) -> (f32, Option<f32>) {
    let table = lb.style_node;
    let rows = table_rows(table);
    let cols = table_columns(table, ctx);
    let sp = table_spacing(table);
    let n = cols.len();
    if n == 0 {
        // No rows (e.g. a clearfix `display: table` pseudo element): lay out children as flow.
        let mut floats = FloatCtx::default();
        let out = layout_flow(lb, table, content_x, content_y, content_w, Cb { width: content_w, height: None }, &mut floats, false, false, ctx);
        return (out.content_height, out.first_baseline);
    }
    let avail = (content_w - sp * (n as f32 + 1.0)).max(0.0);
    let total_min: f32 = cols.iter().map(|c| c.0).sum();
    let total_max: f32 = cols.iter().map(|c| c.1).sum();
    let widths: Vec<f32> = if avail >= total_max {
        let extra = avail - total_max;
        if total_max > 0.0 {
            cols.iter().map(|c| c.1 + extra * c.1 / total_max).collect()
        } else {
            vec![avail / n as f32; n]
        }
    } else if avail > total_min && total_max > total_min {
        let t = (avail - total_min) / (total_max - total_min);
        cols.iter().map(|c| c.0 + (c.1 - c.0) * t).collect()
    } else {
        cols.iter().map(|c| c.0).collect()
    };
    let mut col_x = Vec::with_capacity(n);
    let mut xx = content_x + sp;
    for w in &widths {
        col_x.push(xx);
        xx += w + sp;
    }

    let mut y = content_y + sp;
    let mut first_baseline = None;
    let mut group_boxes: Vec<(usize, LayoutBox<'a>)> = Vec::new();
    let mut current_group: Option<(*const StyledNode, LayoutBox<'a>)> = None;
    let mut row_boxes_direct: Vec<LayoutBox<'a>> = Vec::new();
    let mut order = 0usize;
    for (group, row, cells) in rows {
        let row_top = y;
        let mut cell_boxes: Vec<(LayoutBox<'a>, f32, Option<f32>)> = Vec::new();
        let mut col = 0;
        for cell in cells {
            let span = colspan(cell);
            if col >= n {
                break;
            }
            let end = (col + span).min(n);
            let w: f32 = widths[col..end].iter().sum::<f32>() + sp * (end - col - 1) as f32;
            if let Some(out) = layout_block_level(cell, 0.0, 0.0, Cb { width: w, height: None }, None, BlockOpts { forced_width: Some(w), ..Default::default() }, ctx) {
                let mut b = out.lb;
                let dx = col_x[col] - b.dimensions.x;
                let dy = row_top - b.dimensions.y;
                offset_layout_box(&mut b, dx, dy);
                let content_h = b.dimensions.height;
                let bl = out.first_baseline.map(|v| v + dy);
                cell_boxes.push((b, content_h, bl));
            }
            col = end;
        }
        let row_bm = box_model(row, content_w, ctx);
        let mut row_h = cell_boxes.iter().map(|(_, h, _)| *h).fold(0.0f32, f32::max);
        if let Some(h) = specified_border_height(row, None, &row_bm, ctx) {
            row_h = row_h.max(h);
        }
        // Stretch cells to the row height and align their contents.
        for (b, h, bl) in cell_boxes.iter_mut() {
            let extra = row_h - *h;
            if extra > 0.0 {
                let va = skw(b.style_node, "vertical-align").unwrap_or("middle");
                let shift = match va {
                    "top" | "baseline" => 0.0,
                    "bottom" => extra,
                    _ => extra / 2.0,
                };
                if shift != 0.0 {
                    for c in b.children.iter_mut() {
                        offset_layout_box(c, 0.0, shift);
                    }
                    if let Some(v) = bl.as_mut() {
                        *v += shift;
                    }
                }
                b.dimensions.height = row_h;
            }
            if first_baseline.is_none() {
                first_baseline = *bl;
            }
        }
        let mut rb = LayoutBox::new(row);
        rb.dimensions = Rect { x: content_x + sp, y: row_top, width: (content_w - 2.0 * sp).max(0.0), height: row_h };
        rb.children = cell_boxes.into_iter().map(|(b, _, _)| b).collect();
        y = row_top + row_h + sp;
        match group {
            Some(g) => {
                let gp = g as *const StyledNode;
                let same = current_group.as_ref().map_or(false, |(p, _)| *p == gp);
                if !same {
                    if let Some((_, gb)) = current_group.take() {
                        group_boxes.push((order, gb));
                        order += 1;
                    }
                    let mut gb = LayoutBox::new(g);
                    gb.dimensions = Rect { x: content_x, y: row_top, width: content_w, height: 0.0 };
                    current_group = Some((gp, gb));
                }
                if let Some((_, gb)) = current_group.as_mut() {
                    gb.dimensions.height = row_top + row_h - gb.dimensions.y;
                    gb.children.push(rb);
                }
            }
            None => {
                if let Some((_, gb)) = current_group.take() {
                    group_boxes.push((order, gb));
                    order += 1;
                }
                group_boxes.push((order, rb));
                order += 1;
            }
        }
    }
    if let Some((_, gb)) = current_group.take() {
        group_boxes.push((order, gb));
    }
    let _ = &mut row_boxes_direct;
    for (_, b) in group_boxes {
        lb.children.push(b);
    }
    ((y - content_y).max(0.0), first_baseline)
}

// ── Grid (explicit column tracks, auto placement) ─────────────────────────────

fn layout_grid<'a>(lb: &mut LayoutBox<'a>, content_x: f32, content_y: f32, content_w: f32, cb: Cb, ctx: &mut Ctx) -> f32 {
    let sn = lb.style_node;
    let read_tracks = |prop: &str| -> Vec<Value> {
        match sval(sn, prop) {
            Some(Value::Keyword(k)) => crate::css::parse_track_list(k),
            _ => Vec::new(),
        }
    };
    let col_tracks = read_tracks("grid-template-columns");
    let row_tracks = read_tracks("grid-template-rows");
    let col_gap = gap_px(sn, "column-gap", Some(content_w), ctx);
    let row_gap = gap_px(sn, "row-gap", cb.height, ctx);
    let num_cols = col_tracks.len().max(1);
    let total_gaps = col_gap * (num_cols - 1) as f32;
    let avail = (content_w - total_gaps).max(0.0);
    let fixed: f32 = col_tracks
        .iter()
        .map(|t| match t {
            Value::Length(v, Unit::Px) => *v,
            Value::Length(v, Unit::Percent) => content_w * v / 100.0,
            _ => 0.0,
        })
        .sum();
    let fr_total: f32 = col_tracks.iter().map(|t| if let Value::Length(v, Unit::Fr) = t { *v } else { 0.0 }).sum();
    let auto_count = col_tracks.iter().filter(|t| matches!(t, Value::Keyword(k) if k.as_ref() == "auto")).count() as f32;
    let flex_space = (avail - fixed).max(0.0);
    let col_widths: Vec<f32> = if col_tracks.is_empty() {
        vec![content_w]
    } else {
        col_tracks
            .iter()
            .map(|t| match t {
                Value::Length(v, Unit::Px) => *v,
                Value::Length(v, Unit::Percent) => content_w * v / 100.0,
                Value::Length(v, Unit::Fr) => if fr_total > 0.0 { flex_space * v / fr_total } else { 0.0 },
                Value::Keyword(k) if k.as_ref() == "auto" => {
                    if fr_total > 0.0 { 0.0 } else if auto_count > 0.0 { flex_space / auto_count } else { 0.0 }
                }
                _ => 0.0,
            })
            .collect()
    };
    let mut col_x = Vec::new();
    let mut xx = content_x;
    for w in &col_widths {
        col_x.push(xx);
        xx += w + col_gap;
    }
    let grid_children: Vec<&'a StyledNode> = sn
        .children
        .iter()
        .filter(|c| !should_skip(c) && !(is_text(c) && is_collapsible_whitespace_text(c)))
        .collect();
    let mut placed: Vec<(usize, usize, LayoutBox<'a>)> = Vec::new();
    let mut idx = 0usize;
    for c in grid_children {
        if is_out_of_flow_positioned(c) {
            lb.children.push(make_abs_placeholder(c, content_x, content_y, ctx));
            continue;
        }
        let col = idx % num_cols;
        let row = idx / num_cols;
        idx += 1;
        let w = col_widths.get(col).copied().unwrap_or(content_w);
        if is_text(c) {
            let (b, _) = layout_anon_text(c, sn, w, cb, ctx);
            placed.push((row, col, b));
            continue;
        }
        let bm = box_model(c, w, ctx);
        let fw = match specified_border_width(c, Some(w), &bm, ctx) {
            Some(sw) => sw,
            None => (w - bm.margin.horizontal()).max(0.0),
        };
        if let Some(out) = layout_block_level(c, 0.0, 0.0, Cb { width: w, height: None }, None, BlockOpts { forced_width: Some(fw), ..Default::default() }, ctx) {
            placed.push((row, col, out.lb));
        }
    }
    let num_rows = placed.iter().map(|(r, _, _)| r + 1).max().unwrap_or(0);
    let mut row_h = vec![0.0f32; num_rows];
    for (r, _, b) in &placed {
        row_h[*r] = row_h[*r].max(b.margin_box_height());
    }
    for (r, h) in row_h.iter_mut().enumerate() {
        if let Some(Value::Length(v, Unit::Px)) = row_tracks.get(r) {
            *h = h.max(*v);
        }
    }
    let mut row_y = Vec::new();
    let mut yy = content_y;
    for h in &row_h {
        row_y.push(yy);
        yy += h + row_gap;
    }
    for (r, c, mut b) in placed {
        let x = col_x[c] + b.margin.left;
        let y = row_y[r] + b.margin.top;
        let dx = x - b.dimensions.x;
        let dy = y - b.dimensions.y;
        offset_layout_box(&mut b, dx, dy);
        let stretch_h = (row_h[r] - b.margin.vertical()).max(0.0);
        if is_auto_height(b.style_node, ctx) && stretch_h > b.dimensions.height {
            b.dimensions.height = stretch_h;
        }
        lb.children.push(b);
    }
    if num_rows == 0 {
        0.0
    } else {
        (yy - row_gap - content_y).max(0.0)
    }
}

// ── Absolute / fixed positioning ──────────────────────────────────────────────

/// A placeholder for an out-of-flow box; `(x, y)` is its static position.
fn make_abs_placeholder<'a>(sn: &'a StyledNode, x: f32, y: f32, ctx: &mut Ctx) -> LayoutBox<'a> {
    let mut b = LayoutBox::new(sn);
    b.abs_placeholder = true;
    b.dimensions = Rect { x, y, width: 0.0, height: 0.0 };
    ctx.pending_abs += 1;
    b
}

/// Replace pending placeholders in `root`'s subtree whose containing block is
/// `cb_rect` (absolute boxes; fixed boxes only when `is_viewport`).
fn resolve_pending_abs(root: &mut LayoutBox, cb_rect: Rect, is_viewport: bool, ctx: &mut Ctx) {
    let mut stack: Vec<*mut LayoutBox> = vec![root as *mut LayoutBox];
    while let Some(ptr) = stack.pop() {
        if ctx.pending_abs == 0 {
            break;
        }
        // SAFETY: each pointer refers to a distinct node of the tree owned by `root`;
        // children vectors are only modified for the node currently being visited.
        let node = unsafe { &mut *ptr };
        for k in 0..node.children.len() {
            let child = &node.children[k];
            if child.abs_placeholder {
                let is_fixed = child.position == PositionType::Fixed;
                if is_fixed && !is_viewport {
                    continue;
                }
                let sn = child.style_node;
                let (sx, sy) = (child.dimensions.x, child.dimensions.y);
                let cbr = if is_fixed { Rect { x: 0.0, y: 0.0, width: ctx.vw, height: ctx.vh } } else { cb_rect };
                ctx.pending_abs = ctx.pending_abs.saturating_sub(1);
                if let Some(b) = layout_abs(sn, sx, sy, cbr, ctx) {
                    node.children[k] = b;
                } else {
                    let mut empty = LayoutBox::new(sn);
                    empty.dimensions = Rect { x: sx, y: sy, width: 0.0, height: 0.0 };
                    node.children[k] = empty;
                }
            } else {
                stack.push(&mut node.children[k] as *mut LayoutBox);
            }
        }
    }
}

fn offset_prop(sn: &StyledNode, prop: &str, base: f32, ctx: &Ctx) -> Option<f32> {
    if is_auto(sn, prop) {
        None
    } else {
        prop_len(sn, prop, Some(base), ctx)
    }
}

/// Lay out an absolutely positioned box against its containing block (padding box).
fn layout_abs<'a>(sn: &'a StyledNode, sx: f32, sy: f32, cbr: Rect, ctx: &mut Ctx) -> Option<LayoutBox<'a>> {
    let bm = box_model(sn, cbr.width, ctx);
    let left = offset_prop(sn, "left", cbr.width, ctx);
    let right = offset_prop(sn, "right", cbr.width, ctx);
    let top = offset_prop(sn, "top", cbr.height, ctx);
    let bottom = offset_prop(sn, "bottom", cbr.height, ctx);
    let cb = Cb { width: cbr.width, height: Some(cbr.height) };
    let replaced = inner_display(sn) == Inner::Replaced;
    let spec_w = specified_border_width(sn, Some(cbr.width), &bm, ctx);
    let mut border_w = match spec_w {
        Some(w) => w,
        None if replaced => replaced_size(sn, &bm, cb, ctx).0,
        None => match (left, right) {
            (Some(l), Some(r)) => (cbr.width - l - r - bm.margin.horizontal()).max(0.0),
            _ => {
                let l = left.unwrap_or((sx - cbr.x).max(0.0));
                let avail = (cbr.width - l - right.unwrap_or(0.0) - bm.margin.horizontal()).max(0.0);
                let (min_c, max_c) = intrinsic_widths(sn, ctx);
                max_c.min(avail).max(min_c)
            }
        },
    };
    border_w = clamp_border_width(sn, border_w, Some(cbr.width), &bm, ctx);
    let mut ml = bm.margin.left;
    let mut mr = bm.margin.right;
    if let (Some(l), Some(r)) = (left, right) {
        let free = cbr.width - l - r - border_w - ml - mr;
        if bm.margin_auto_left && bm.margin_auto_right {
            if free >= 0.0 {
                ml += free / 2.0;
                mr += free / 2.0;
            } else {
                mr += free;
            }
        } else if bm.margin_auto_left {
            ml += free;
        } else if bm.margin_auto_right {
            mr += free;
        }
    }
    let x = if let Some(l) = left {
        cbr.x + l + ml
    } else if let Some(r) = right {
        cbr.x + cbr.width - r - mr - border_w
    } else {
        sx + ml
    };
    let spec_h = specified_border_height(sn, Some(cbr.height), &bm, ctx);
    let forced_h = match spec_h {
        Some(h) => Some(clamp_border_height(sn, h, Some(cbr.height), &bm, ctx)),
        None if replaced => None,
        None => match (top, bottom) {
            (Some(t), Some(b)) => Some(clamp_border_height(sn, (cbr.height - t - b - bm.margin.vertical()).max(0.0), Some(cbr.height), &bm, ctx)),
            _ => None,
        },
    };
    let out = layout_block_level(sn, 0.0, 0.0, cb, None, BlockOpts { forced_width: Some(border_w), forced_height: forced_h, ..Default::default() }, ctx)?;
    let mut lb = out.lb;
    let border_h = lb.dimensions.height;
    let mut mt = bm.margin.top;
    let mut mb = bm.margin.bottom;
    if let (Some(t), Some(b)) = (top, bottom) {
        let free = cbr.height - t - b - border_h - mt - mb;
        if bm.margin_auto_top && bm.margin_auto_bottom && free > 0.0 {
            mt += free / 2.0;
            mb += free / 2.0;
        } else if bm.margin_auto_top && free > 0.0 {
            mt += free;
        } else if bm.margin_auto_bottom && free > 0.0 {
            mb += free;
        }
    }
    let y = if let Some(t) = top {
        cbr.y + t + mt
    } else if let Some(b) = bottom {
        cbr.y + cbr.height - b - mb - border_h
    } else {
        sy + mt
    };
    let dx = x - lb.dimensions.x;
    let dy = y - lb.dimensions.y;
    offset_layout_box(&mut lb, dx, dy);
    lb.margin = EdgeSizes { left: ml, right: mr, top: mt, bottom: mb };
    Some(lb)
}

// ── Intrinsic sizes ───────────────────────────────────────────────────────────

fn px_margin(sn: &StyledNode, prop: &str) -> f32 {
    match sval(sn, prop) {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    }
}

fn horizontal_margins_px(sn: &StyledNode) -> f32 {
    if !matches!(sn.node.data, NodeData::Element { .. }) {
        return 0.0;
    }
    px_margin(sn, "margin-left") + px_margin(sn, "margin-right")
}

/// (min-content, max-content) border-box widths of `sn`.
fn intrinsic_widths(sn: &StyledNode, ctx: &mut Ctx) -> (f32, f32) {
    let key = sn as *const StyledNode as usize;
    if let Some(v) = ctx.intrinsic.get(&key) {
        return *v;
    }
    let v = stacker::maybe_grow(256 * 1024, 16 * 1024 * 1024, || intrinsic_widths_inner(sn, ctx));
    ctx.intrinsic.insert(key, v);
    v
}

fn intrinsic_widths_inner(sn: &StyledNode, ctx: &mut Ctx) -> (f32, f32) {
    if should_skip(sn) && !matches!(sn.node.data, NodeData::Document) {
        return (0.0, 0.0);
    }
    if is_text(sn) {
        return inline_intrinsic(&[sn], sn, ctx);
    }
    let bm = box_model(sn, 0.0, ctx);
    let big = Cb { width: 1.0e6, height: None };
    if inner_display(sn) == Inner::Replaced {
        let w = replaced_size(sn, &bm, big, ctx).0;
        return (w, w);
    }
    if let Some(w) = sval(sn, "width").and_then(|v| match v {
        Value::Length(_, Unit::Percent) => None,
        Value::Keyword(k) if k.contains('%') => None,
        v => resolve_len(v, None, ctx),
    }) {
        let bw = if is_border_box(sn) { w.max(bm.pb_h()) } else { w.max(0.0) + bm.pb_h() };
        let bw = clamp_border_width(sn, bw, None, &bm, ctx);
        return (bw, bw);
    }
    let (mut mn, mut mx) = match inner_display(sn) {
        Inner::Flex => flex_intrinsic(sn, ctx),
        Inner::Grid => grid_intrinsic(sn, ctx),
        Inner::Table => {
            if table_rows(sn).is_empty() {
                block_intrinsic(sn, ctx)
            } else {
                table_intrinsic(sn, ctx)
            }
        }
        _ => block_intrinsic(sn, ctx),
    };
    mn += bm.pb_h();
    mx += bm.pb_h();
    let mx = clamp_border_width(sn, mx.max(mn), None, &bm, ctx);
    let mn = clamp_border_width(sn, mn, None, &bm, ctx).min(mx);
    (mn, mx)
}

fn block_intrinsic(sn: &StyledNode, ctx: &mut Ctx) -> (f32, f32) {
    let mut mn: f32 = 0.0;
    let mut mx: f32 = 0.0;
    let mut float_sum: f32 = 0.0;
    let children: Vec<&StyledNode> = sn.children.iter().collect();
    let kinds: Vec<FlowKind> = children.iter().map(|c| flow_kind(c, ctx)).collect();
    let mut i = 0;
    while i < children.len() {
        match kinds[i] {
            FlowKind::Skip | FlowKind::Abs => i += 1,
            FlowKind::Inline => {
                let start = i;
                while i < children.len() && matches!(kinds[i], FlowKind::Inline | FlowKind::Float | FlowKind::Abs | FlowKind::Skip) {
                    i += 1;
                }
                let run: Vec<&StyledNode> = children[start..i]
                    .iter()
                    .zip(&kinds[start..i])
                    .filter(|(_, k)| **k != FlowKind::Skip)
                    .map(|(c, _)| *c)
                    .collect();
                let (a, b) = inline_intrinsic(&run, sn, ctx);
                mn = mn.max(a);
                mx = mx.max(b + float_sum);
                float_sum = 0.0;
            }
            FlowKind::Float => {
                let c = children[i];
                let (a, b) = intrinsic_widths(c, ctx);
                let m = horizontal_margins_px(c);
                if get_clear(c).is_some() {
                    mx = mx.max(float_sum);
                    float_sum = 0.0;
                }
                float_sum += b + m;
                mn = mn.max(a + m);
                mx = mx.max(float_sum);
                i += 1;
            }
            FlowKind::Block => {
                let c = children[i];
                let (a, b) = intrinsic_widths(c, ctx);
                let m = horizontal_margins_px(c);
                mn = mn.max(a + m);
                mx = mx.max(b + m);
                if get_clear(c).is_some() || !establishes_bfc_sn(c) {
                    float_sum = 0.0;
                } else {
                    mx = mx.max(b + m + float_sum);
                }
                i += 1;
            }
        }
    }
    (mn, mx.max(mn))
}

/// (min, max) content widths of an inline formatting context.
fn inline_intrinsic(run: &[&StyledNode], container: &StyledNode, ctx: &mut Ctx) -> (f32, f32) {
    let _ = container;
    let mut items = Vec::new();
    flatten_inline(run, &mut items);
    let pieces = make_pieces(&items);
    // Atomic contributions.
    let mut atomic_min: HashMap<usize, f32> = HashMap::new();
    let mut atomic_max: HashMap<usize, f32> = HashMap::new();
    for p in &pieces {
        if let PieceKind::Atomic(ai, node) = p.kind {
            let (a, b) = intrinsic_widths(node, ctx);
            let m = horizontal_margins_px(node);
            atomic_min.insert(ai, a + m);
            atomic_max.insert(ai, b + m);
        }
    }
    let width_of = |p: &Piece, use_max: bool| -> f32 {
        match p.kind {
            PieceKind::Atomic(ai, _) => {
                if use_max { atomic_max[&ai] } else { atomic_min[&ai] }
            }
            _ => p.width,
        }
    };
    // max-content: sum per forced line.
    let mut max_w: f32 = 0.0;
    let mut line: f32 = 0.0;
    let mut trailing_space: f32 = 0.0;
    let mut float_extra: f32 = 0.0;
    for p in &pieces {
        match p.kind {
            PieceKind::Break(_) => {
                max_w = max_w.max(line - trailing_space);
                line = 0.0;
                trailing_space = 0.0;
            }
            PieceKind::Float(f) => {
                let (_, b) = intrinsic_widths(f, ctx);
                float_extra += b + horizontal_margins_px(f);
            }
            PieceKind::Abs(_) => {}
            _ => {
                let w = width_of(p, true);
                if p.is_space() && p.collapsible {
                    if line == 0.0 {
                        continue;
                    }
                    trailing_space += w;
                } else if p.is_space() {
                    trailing_space = 0.0;
                } else if w > 0.0 || p.is_content() {
                    trailing_space = 0.0;
                }
                line += w;
            }
        }
    }
    max_w = max_w.max(line - trailing_space) + float_extra;
    // min-content: widest unbreakable segment.
    let mut min_w: f32 = 0.0;
    let mut seg: f32 = 0.0;
    let mut seg_trailing: f32 = 0.0;
    for p in &pieces {
        match p.kind {
            PieceKind::Break(_) => {
                min_w = min_w.max(seg - seg_trailing);
                seg = 0.0;
                seg_trailing = 0.0;
            }
            PieceKind::Float(f) => {
                let (a, _) = intrinsic_widths(f, ctx);
                min_w = min_w.max(a + horizontal_margins_px(f));
            }
            PieceKind::Abs(_) => {}
            _ => {
                if p.is_content() && p.break_before {
                    min_w = min_w.max(seg - seg_trailing);
                    seg = 0.0;
                    seg_trailing = 0.0;
                }
                let w = width_of(p, false);
                if p.is_space() && p.collapsible {
                    seg_trailing += w;
                } else if p.is_content() {
                    seg_trailing = 0.0;
                }
                seg += w;
            }
        }
    }
    min_w = min_w.max(seg - seg_trailing);
    (min_w.max(0.0), max_w.max(min_w).max(0.0))
}

fn flex_intrinsic(sn: &StyledNode, ctx: &mut Ctx) -> (f32, f32) {
    let legacy_box = matches!(skw(sn, "display"), Some("-webkit-box" | "-webkit-inline-box"));
    let dir = if legacy_box {
        if skw(sn, "-webkit-box-orient") == Some("vertical") { "column" } else { "row" }
    } else {
        skw(sn, "flex-direction").unwrap_or("row")
    };
    let row = dir.starts_with("row");
    let wrap = matches!(skw(sn, "flex-wrap"), Some("wrap" | "wrap-reverse"));
    let gap = if row { gap_px(sn, "column-gap", None, ctx) } else { 0.0 };
    let mut mins: Vec<f32> = Vec::new();
    let mut maxs: Vec<f32> = Vec::new();
    for c in flex_children(sn) {
        if is_out_of_flow_positioned(c) {
            continue;
        }
        if is_text(c) {
            if is_collapsible_whitespace_text(c) {
                continue;
            }
            let (a, b) = inline_intrinsic(&[c], sn, ctx);
            mins.push(a);
            maxs.push(b);
            continue;
        }
        let (mut a, mut b) = intrinsic_widths(c, ctx);
        // A definite flex-basis in a row container overrides the content size.
        if row {
            if let Some(Value::Length(v, Unit::Px)) = sval(c, "flex-basis") {
                let bm = box_model(c, 0.0, ctx);
                let basis = if is_border_box(c) { v.max(bm.pb_h()) } else { v + bm.pb_h() };
                let grow = flex_factor(c, "flex-grow", 0.0);
                let shrink = flex_factor(c, "flex-shrink", 1.0);
                b = if grow > 0.0 { b.max(basis) } else { basis.max(if shrink > 0.0 { 0.0 } else { basis }) };
                if shrink == 0.0 {
                    a = a.max(basis);
                }
            }
        }
        let m = horizontal_margins_px(c);
        mins.push(a + m);
        maxs.push(b + m);
    }
    if mins.is_empty() {
        return (0.0, 0.0);
    }
    let gaps = gap * (mins.len() - 1) as f32;
    if row {
        let max = maxs.iter().sum::<f32>() + gaps;
        let min = if wrap { mins.iter().cloned().fold(0.0, f32::max) } else { mins.iter().sum::<f32>() + gaps };
        (min, max.max(min))
    } else {
        let min = mins.iter().cloned().fold(0.0, f32::max);
        let max = maxs.iter().cloned().fold(0.0, f32::max);
        (min, max.max(min))
    }
}

fn grid_intrinsic(sn: &StyledNode, ctx: &mut Ctx) -> (f32, f32) {
    let tracks = match sval(sn, "grid-template-columns") {
        Some(Value::Keyword(k)) => crate::css::parse_track_list(k),
        _ => Vec::new(),
    };
    let fixed: f32 = tracks.iter().map(|t| if let Value::Length(v, Unit::Px) = t { *v } else { 0.0 }).sum();
    let gap = gap_px(sn, "column-gap", None, ctx) * (tracks.len().saturating_sub(1)) as f32;
    let (mn, mx) = block_intrinsic(sn, ctx);
    if !tracks.is_empty() && tracks.iter().all(|t| matches!(t, Value::Length(_, Unit::Px))) {
        (fixed + gap, fixed + gap)
    } else {
        (mn.max(fixed + gap), (mx * tracks.len().max(1) as f32).max(fixed + gap))
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
                    if matches!(tag.as_str(), "input" | "textarea" | "select" | "button") {
                        list.push((node.dimensions, node.style_node));
                    }
                }
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
    }

    /// Iterative collect_form_element — finds the first `<form>` element in the layout tree
    /// and returns its `action` and `method` attributes.
    /// Returns `None` if no `<form>` element is found.
    pub fn collect_form_element(&self) -> Option<(String, String)> {
        let mut stack: Vec<&LayoutBox<'a>> = vec![self];
        while let Some(node) = stack.pop() {
            if let NodeData::Element { ref name, ref attrs, .. } = node.style_node.node.data {
                if name.local.to_string() == "form" {
                    let mut action = String::new();
                    let mut method = String::from("get");
                    for attr in attrs.borrow().iter() {
                        match attr.name.local.to_string().as_str() {
                            "action" => action = attr.value.to_string(),
                            "method" => method = attr.value.to_string().to_lowercase(),
                            _ => {}
                        }
                    }
                    return Some((action, method));
                }
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
        None
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

/// Border-box width (dimensions already hold the border box).
#[cfg(test)]
fn border_box_width(cb: &LayoutBox<'_>) -> f32 {
    cb.dimensions.width
}

#[cfg(test)]
fn is_block_level(d: DisplayType) -> bool {
    matches!(d, DisplayType::Block | DisplayType::ListItem | DisplayType::Flex | DisplayType::Grid)
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
        // The text box is the glyph content area: the line starts at y=24 and the
        // content area sits half the leading below it.
        let fs = 16.0;
        let m = font_metrics(text.style_node, fs);
        let half_leading = (m.normal_line_height() - (m.ascent + m.descent)) / 2.0;
        assert!((text.dimensions.y - (24.0 + half_leading)).abs() < 0.05, "text y {}", text.dimensions.y);
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

    /// Collect every line fragment of the text node whose contents contain `needle`.
    fn collect_text_fragments<'a>(layout: &'a LayoutBox<'a>, needle: &str, out: &mut Vec<&'a LayoutBox<'a>>) {
        if let NodeData::Text { ref contents } = layout.style_node.node.data {
            if contents.borrow().contains(needle) && layout.text_fragment.as_deref().map_or(false, |t| !t.is_empty()) {
                out.push(layout);
            }
        }
        for child in &layout.children {
            collect_text_fragments(child, needle, out);
        }
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
        // Text is laid out as one fragment per line: the sentence must span
        // several line fragments at increasing y.
        let mut frags: Vec<&LayoutBox> = Vec::new();
        collect_text_fragments(&layout, "This sentence", &mut frags);
        assert!(frags.len() >= 2, "inline text should wrap onto multiple lines, got {} fragments", frags.len());
        assert!(frags[1].dimensions.y > frags[0].dimensions.y);
        assert!(frags[1].dimensions.x < 280.0, "continuation lines start at the container edge");
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
                <span style="display: inline-block; width: 190px;">prefix</span>
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
            second.dimensions.y >= first.dimensions.y + 32.0,
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

        // Out-of-flow content does not contribute to the inline-block's width
        // (as in browsers); the positioned span keeps its own intrinsic width.
        let span = link.children.iter().find(|c| c.position == PositionType::Absolute).expect("positioned span");
        assert!(
            span.dimensions.width > 20.0,
            "positioned descendant should get its intrinsic width, got {}",
            span.dimensions.width
        );
        assert!(
            (span.dimensions.x - link.dimensions.x).abs() < 1.0,
            "positioned span sits at its static position inside the link"
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

        // `dimensions` is the border box.
        let border_w = login.dimensions.width;
        let border_h = login.dimensions.height;

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
        // CSS 2.1 §9.5: the block box itself ignores the float (full 800px width) but
        // its line boxes are shortened, so its text starts right of the 100px float.
        assert_eq!(sibling.dimensions.width, 800.0);
        let text = find_text_box_containing(sibling, "S").expect("sibling text");
        assert!(
            text.dimensions.x >= 100.0,
            "text beside a 100px left float must start at x>=100, got {}",
            text.dimensions.x
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

    // ── Layout rewrite: feature tests ─────────────────────────────────────────

    fn by_id<'a>(root: &'a LayoutBox<'a>, id: &str) -> &'a LayoutBox<'a> {
        find_element_by_id(root, id).unwrap_or_else(|| panic!("#{} not found", id))
    }

    #[test]
    fn test_content_box_width_adds_padding_and_border() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="a"></div><div id="b"></div>"#,
            "body{margin:0} #a{width:40px;padding:10px;border:5px solid red} #b{width:40px;padding:10px;border:5px solid red;box-sizing:border-box}",
            800.0,
            600.0,
        );
        assert_eq!(by_id(&root, "a").dimensions.width, 70.0);
        assert_eq!(by_id(&root, "b").dimensions.width, 40.0);
    }

    #[test]
    fn test_margin_collapses_through_parent_top() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="p"><div id="c">x</div></div>"#,
            "body{margin:0} #p{margin-top:10px} #c{margin-top:30px}",
            800.0,
            600.0,
        );
        let p = by_id(&root, "p");
        let c = by_id(&root, "c");
        assert_eq!(p.dimensions.y, 30.0, "parent moves down by the collapsed margin");
        assert_eq!(c.dimensions.y, 30.0, "child shares the parent's top edge");
    }

    #[test]
    fn test_empty_block_margins_collapse_through() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="a" style="height:10px"></div><div style="margin:20px 0"></div><div id="b" style="margin-top:15px">x</div>"#,
            "body{margin:0}",
            800.0,
            600.0,
        );
        assert_eq!(by_id(&root, "b").dimensions.y, 30.0, "10 + max(20, 20, 15)");
    }

    #[test]
    fn test_overflow_hidden_block_avoids_float() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="w"><div style="float:left;width:100px;height:50px"></div><div id="bfc" style="overflow:hidden">text</div></div>"#,
            "body{margin:0} #w{width:500px}",
            800.0,
            600.0,
        );
        let bfc = by_id(&root, "bfc");
        assert_eq!(bfc.dimensions.x, 100.0);
        assert_eq!(bfc.dimensions.width, 400.0);
    }

    #[test]
    fn test_korean_text_breaks_between_syllables() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="d">가나다라마바사아자차카타파하</div>"#,
            "body{margin:0} #d{width:60px;font-size:16px}",
            800.0,
            600.0,
        );
        let mut frags = Vec::new();
        collect_text_fragments(&root, "가나다", &mut frags);
        assert!(frags.len() >= 3, "no-space Korean text wraps between syllables, got {} lines", frags.len());
        for f in &frags {
            assert!(f.dimensions.width <= 61.0, "line width {} exceeds the container", f.dimensions.width);
        }
    }

    #[test]
    fn test_nowrap_keeps_text_on_one_line() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="d">one two three four five six</div>"#,
            "body{margin:0} #d{width:40px;white-space:nowrap}",
            800.0,
            600.0,
        );
        let mut frags = Vec::new();
        collect_text_fragments(&root, "one two", &mut frags);
        assert_eq!(frags.len(), 1);
    }

    #[test]
    fn test_text_overflow_ellipsis_truncates_line() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="d">a long headline that does not fit in the box</div>"#,
            "body{margin:0} #d{width:80px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}",
            800.0,
            600.0,
        );
        let mut frags = Vec::new();
        collect_text_fragments(&root, "a long headline", &mut frags);
        let t = frags[0].text_fragment.clone().unwrap();
        assert!(t.ends_with('\u{2026}'), "ellipsis appended, got {:?}", t);
        assert!(t.len() < "a long headline that does not fit in the box".len());
    }

    #[test]
    fn test_line_clamp_limits_lines() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="d">one two three four five six seven eight nine ten eleven twelve</div>"#,
            "body{margin:0} #d{width:60px;line-height:20px;display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;overflow:hidden}",
            800.0,
            600.0,
        );
        assert_eq!(by_id(&root, "d").dimensions.height, 40.0);
    }

    #[test]
    fn test_line_height_sets_line_box_height() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="d">text</div>"#,
            "body{margin:0} #d{line-height:30px;font-size:12px}",
            800.0,
            600.0,
        );
        assert_eq!(by_id(&root, "d").dimensions.height, 30.0);
    }

    #[test]
    fn test_inline_block_sits_on_baseline_and_middle_aligns() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="d">x<span id="m" style="display:inline-block;width:10px;height:40px;vertical-align:middle"></span></div>"#,
            "body{margin:0} #d{line-height:20px;font-size:16px}",
            800.0,
            600.0,
        );
        let d = by_id(&root, "d");
        let m = by_id(&root, "m");
        assert!(d.dimensions.height >= 40.0);
        assert!(m.dimensions.y >= d.dimensions.y - 0.01);
        assert!(m.dimensions.y + 40.0 <= d.dimensions.y + d.dimensions.height + 0.01);
    }

    #[test]
    fn test_inline_block_shrinks_to_fit_content() {
        let (root, _, _) = layout_from_html_css(
            r#"<div><span id="s" style="display:inline-block;padding:0 5px">abc</span></div>"#,
            "body{margin:0}",
            800.0,
            600.0,
        );
        let s = by_id(&root, "s");
        let text_w = text_width(s.style_node, "abc", 16.0, 0.0);
        assert!((s.dimensions.width - (text_w + 10.0)).abs() < 0.5, "got {}", s.dimensions.width);
    }

    #[test]
    fn test_flex_shrink_respects_min_content() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="f"><div id="a">unbreakableword</div><div id="b" style="width:500px"></div></div>"#,
            "body{margin:0} #f{display:flex;width:300px}",
            800.0,
            600.0,
        );
        let a = by_id(&root, "a");
        let min_word = text_width(a.style_node, "unbreakableword", 16.0, 0.0);
        assert!(a.dimensions.width >= min_word - 0.5, "item {} shrank below its min-content {}", a.dimensions.width, min_word);
    }

    #[test]
    fn test_flex_auto_margin_pushes_item_to_end() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="f"><div id="a" style="width:50px"></div><div id="b" style="width:50px;margin-left:auto"></div></div>"#,
            "body{margin:0} #f{display:flex;width:400px}",
            800.0,
            600.0,
        );
        assert_eq!(by_id(&root, "b").dimensions.x, 350.0);
    }

    #[test]
    fn test_flex_column_grow_fills_definite_height() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="f"><div id="a" style="height:20px"></div><div id="b" style="flex:1"></div></div>"#,
            "body{margin:0} #f{display:flex;flex-direction:column;height:200px}",
            800.0,
            600.0,
        );
        let b = by_id(&root, "b");
        assert_eq!(b.dimensions.y, 20.0);
        assert_eq!(b.dimensions.height, 180.0);
        assert_eq!(b.dimensions.width, 800.0, "column items stretch across");
    }

    #[test]
    fn test_flex_wrap_and_gap() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="f"><div id="a"></div><div id="b"></div><div id="c"></div></div>"#,
            "body{margin:0} #f{display:flex;flex-wrap:wrap;gap:10px 20px;width:250px} #f div{width:100px;height:30px}",
            800.0,
            600.0,
        );
        assert_eq!(by_id(&root, "b").dimensions.x, 120.0);
        assert_eq!(by_id(&root, "c").dimensions.x, 0.0);
        assert_eq!(by_id(&root, "c").dimensions.y, 40.0);
    }

    #[test]
    fn test_absolute_bottom_right_uses_final_container_size() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="p"><div style="height:100px"></div><span id="a">x</span></div>"#,
            "body{margin:0} #p{position:relative;width:300px} #a{position:absolute;right:10px;bottom:5px;width:20px;height:10px}",
            800.0,
            600.0,
        );
        let a = by_id(&root, "a");
        assert_eq!(a.dimensions.x, 270.0);
        assert_eq!(a.dimensions.y, 85.0);
    }

    #[test]
    fn test_absolute_centered_with_auto_margins() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="p"><div id="a"></div></div>"#,
            "body{margin:0} #p{position:relative;width:300px;height:200px} #a{position:absolute;top:0;bottom:0;left:0;right:0;margin:auto;width:100px;height:50px}",
            800.0,
            600.0,
        );
        let a = by_id(&root, "a");
        assert_eq!((a.dimensions.x, a.dimensions.y), (100.0, 75.0));
    }

    #[test]
    fn test_svg_without_size_uses_view_box_ratio() {
        let (root, _, _) = layout_from_html_css(
            r#"<span id="s"><svg id="g" viewBox="0 0 24 12"></svg></span>"#,
            "body{margin:0} #s{display:block;width:48px}",
            800.0,
            600.0,
        );
        let g = by_id(&root, "g");
        assert_eq!((g.dimensions.width, g.dimensions.height), (48.0, 24.0));
    }

    #[test]
    fn test_table_columns_share_width() {
        let (root, _, _) = layout_from_html_css(
            r#"<table id="t"><tr><td id="a">a</td><td id="b">bbbbbbbb</td></tr></table>"#,
            "body{margin:0} #t{width:400px;border-collapse:collapse} td{padding:0}",
            800.0,
            600.0,
        );
        let a = by_id(&root, "a");
        let b = by_id(&root, "b");
        assert!((a.dimensions.width + b.dimensions.width - 400.0).abs() < 0.5);
        assert!(b.dimensions.width > a.dimensions.width);
        assert_eq!(b.dimensions.x, a.dimensions.x + a.dimensions.width);
    }

    #[test]
    fn test_pseudo_element_box_takes_space() {
        let (root, _, _) = layout_from_html_css(
            r#"<div><span id="s">x</span></div>"#,
            r#"body{margin:0} #s::before{content:"";display:inline-block;width:30px;height:10px}"#,
            800.0,
            600.0,
        );
        let s = by_id(&root, "s");
        assert!(s.dimensions.width >= 30.0 + text_width(s.style_node, "x", 16.0, 0.0) - 0.5);
    }

    #[test]
    fn test_clearfix_after_contains_floats() {
        let (root, _, _) = layout_from_html_css(
            r#"<div id="c"><div style="float:left;width:10px;height:70px"></div></div>"#,
            r#"body{margin:0} #c::after{content:"";display:table;clear:both}"#,
            800.0,
            600.0,
        );
        assert_eq!(by_id(&root, "c").dimensions.height, 70.0);
    }
}

#[cfg(test)]
mod replaced_sizing_tests {
    use super::*;
    use std::collections::HashMap;

    fn find<'a, 'b>(lb: &'b LayoutBox<'a>, tag: &str) -> Option<&'b LayoutBox<'a>> {
        if is_tag(lb.style_node, tag) {
            return Some(lb);
        }
        lb.children.iter().find_map(|c| find(c, tag))
    }

    fn with_layout(html: &str, css: &str, focused: Option<&str>, images: ImageSizes, check: impl FnOnce(&LayoutBox)) {
        let dom = crate::dom::parse_html(html);
        let sheet = crate::css::parse_css(css);
        let st = crate::style::build_style_tree(&dom.document, &sheet, None, &HashMap::new(), None, focused, None);
        let (lb, _, _) = build_layout_tree_with_images(&st, 0.0, 0.0, 800.0, 800.0, 600.0, None, images);
        check(&lb.unwrap());
    }

    fn img_rect(html: &str, images: ImageSizes) -> (f32, f32) {
        let mut out = (0.0, 0.0);
        with_layout(html, "", None, images, |lb| {
            let img = find(lb, "img").expect("img box");
            out = (img.dimensions.width, img.dimensions.height);
        });
        out
    }

    fn sizes(url: &str, w: f32, h: f32) -> ImageSizes {
        let mut s = ImageSizes::default();
        s.insert(url, w, h);
        s
    }

    #[test]
    fn height_attribute_scales_width_by_natural_ratio() {
        let r = img_rect(r#"<img height="20" src="https://x/logo.png">"#, sizes("https://x/logo.png", 150.0, 40.0));
        assert_eq!(r, (75.0, 20.0));
    }

    #[test]
    fn auto_size_uses_natural_size() {
        let r = img_rect(r#"<img src="https://x/a.png">"#, sizes("https://x/a.png", 64.0, 32.0));
        assert_eq!(r, (64.0, 32.0));
    }

    #[test]
    fn relative_src_resolves_against_base() {
        let mut s = ImageSizes::from_cache(&HashMap::new(), url::Url::parse("https://x/dir/page.html").ok().as_ref());
        s.insert("https://x/dir/a.png", 30.0, 10.0);
        let r = img_rect(r#"<img height="20" src="a.png">"#, s);
        assert_eq!(r, (60.0, 20.0));
    }

    #[test]
    fn width_and_height_attributes_give_ratio_before_load() {
        let r = img_rect(
            r#"<div style="width:300px"><img width="400" height="200" style="width:100%;height:auto" src="https://x/none.png"></div>"#,
            ImageSizes::default(),
        );
        assert_eq!(r, (300.0, 150.0));
    }

    #[test]
    fn max_width_keeps_ratio_when_both_auto() {
        let r = img_rect(r#"<img style="max-width:100px" src="https://x/a.png">"#, sizes("https://x/a.png", 400.0, 200.0));
        assert_eq!(r, (100.0, 50.0));
        let r = img_rect(r#"<img style="min-height:100px" src="https://x/a.png">"#, sizes("https://x/a.png", 40.0, 20.0));
        assert_eq!(r, (200.0, 100.0));
    }

    #[test]
    fn aspect_ratio_property_sets_auto_height() {
        let r = img_rect(r#"<img style="width:200px;aspect-ratio:2 / 1" src="https://x/none.png">"#, ImageSizes::default());
        assert_eq!(r, (200.0, 100.0));
        // `auto && <ratio>` prefers the natural ratio once the image is known.
        let r = img_rect(
            r#"<img style="width:200px;aspect-ratio:auto 2 / 1" src="https://x/a.png">"#,
            sizes("https://x/a.png", 100.0, 100.0),
        );
        assert_eq!(r, (200.0, 200.0));
    }

    #[test]
    fn constraint_table_cases() {
        assert_eq!(constrain_replaced(400.0, 200.0, 0.0, 100.0, 0.0, f32::INFINITY), (100.0, 50.0));
        assert_eq!(constrain_replaced(400.0, 200.0, 0.0, 200.0, 0.0, 50.0), (100.0, 50.0));
        assert_eq!(constrain_replaced(10.0, 20.0, 50.0, f32::INFINITY, 0.0, 60.0), (50.0, 60.0));
        assert_eq!(constrain_replaced(30.0, 30.0, 0.0, f32::INFINITY, 0.0, f32::INFINITY), (30.0, 30.0));
    }

    fn placeholder_box<'a, 'b>(lb: &'b LayoutBox<'a>) -> Option<&'b LayoutBox<'a>> {
        find(lb, "::placeholder")
    }

    fn collect_text(b: &LayoutBox, out: &mut String) {
        if let Some(t) = &b.text_fragment {
            out.push_str(t);
        }
        for c in &b.children {
            collect_text(c, out);
        }
    }

    #[test]
    fn empty_input_shows_placeholder_styled_by_pseudo_element() {
        let css = "#q::placeholder{color:transparent} #q:focus::placeholder{color:#123456}";
        let html = r#"<input id="q" type="search" placeholder="Search here" style="width:200px;height:40px;padding:0;border:0">"#;
        with_layout(html, css, None, ImageSizes::default(), |lb| {
            let ph = placeholder_box(lb).expect("placeholder box");
            assert!(matches!(ph.style_node.specified_values.get("color"), Some(Value::Color(c)) if c.a == 0));
        });
        with_layout(html, css, Some("q"), ImageSizes::default(), |lb| {
            let input = find(lb, "input").unwrap();
            let ph = placeholder_box(lb).expect("placeholder box");
            assert!(matches!(ph.style_node.specified_values.get("color"), Some(Value::Color(c)) if c.r == 0x12 && c.b == 0x56));
            let mut text = String::new();
            collect_text(ph, &mut text);
            assert_eq!(text, "Search here");
            // Vertically centered in the 40px input.
            let mid = ph.dimensions.y + ph.dimensions.height / 2.0;
            assert!((mid - (input.dimensions.y + 20.0)).abs() < 1.0, "placeholder mid {mid}");
        });
    }

    #[test]
    fn input_with_value_has_no_placeholder() {
        let html = r#"<input type="text" value="typed" placeholder="Search here">"#;
        with_layout(html, "", None, ImageSizes::default(), |lb| {
            assert!(placeholder_box(lb).is_none());
        });
    }
}
