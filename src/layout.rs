use crate::css::{Unit, Value};
use crate::style::StyledNode;
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
                        // Measure what will actually be drawn: `text-transform`
                        // changes a run's width, and sizing a box from the
                        // untransformed source makes uppercased text wrap inside
                        // a box built for its lowercase original.
                        {
                            let raw = contents.borrow().to_string();
                            let text = apply_text_transform(&raw, &node.specified_values);
                            Some(
                                measure_text_width(
                                    &text,
                                    font_size,
                                    f32::INFINITY,
                                    resolved_font_style(node),
                                    resolved_letter_spacing_px(node),
                                ) + collapsed_space_width(node, &raw, font_size),
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
                            Some(stated_outer_width(node, *v))
                        } else {
                            None
                        }
                    };

                    if let Some(width) = width {
                        let width = clamp_intrinsic_width(node, width);
                    self.max_content.insert(key, width);
                        val_stack.push(width);
                        continue;
                    }

                    let pad_border = horiz_padding_border(node);
                    let non_skip: Vec<&StyledNode> = node
                        .children
                        .iter()
                        .filter(|c| contributes_to_intrinsic_size(c))
                        .collect();
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
                        .filter(|c| contributes_to_intrinsic_size(c))
                        .collect();
                    let start = val_stack.len().saturating_sub(num_children);
                    let child_vals: Vec<f32> = val_stack.drain(start..).collect();

                    // A flex container is not a flow container: its items are
                    // blockified and laid on the main axis, so a row's
                    // max-content is the *sum* of its items plus the gaps
                    // between them however each item's own `display` reads,
                    // and a column's is the widest single item. Measuring one
                    // as flow took the max of the block-level items and left
                    // out the gaps, so a "field + button" row was sized to
                    // whichever half was wider and its button hung outside it.
                    if get_display_type(node_ref) == DisplayType::Flex {
                        let is_row = flex_axis_is_row(node_ref);
                        let mut sum: f32 = 0.0;
                        let mut widest: f32 = 0.0;
                        let mut items = 0usize;
                        for (child_val, child) in
                            child_vals.into_iter().zip(non_skip_children.iter())
                        {
                            if !is_flex_item(child) {
                                continue;
                            }
                            let total = child_val + horiz_margin(child);
                            sum += total;
                            widest = widest.max(total);
                            items += 1;
                        }
                        let gaps = if is_row && items > 1 {
                            column_gap_px(node_ref) * (items - 1) as f32
                        } else {
                            0.0
                        };
                        let width = if is_row { sum + gaps } else { widest } + pad_border;
                        let width = clamp_intrinsic_width(node_ref, width);
                    self.max_content.insert(key, width);
                        val_stack.push(width);
                        continue;
                    }

                    let mut inline_run_width: f32 = 0.0;
                    let mut max_w: f32 = 0.0;
                    let mut float_width: f32 = 0.0;
                    let mut percent_width_sum: f32 = 0.0;
                    let drops = edge_space_drops(node_ref, &non_skip_children);
                    for (i, (child_val, child)) in
                        child_vals.into_iter().zip(non_skip_children.iter()).enumerate()
                    {
                        let child_val = (child_val - drops[i]).max(0.0);
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
                        if is_block_level_for(child, child_disp) {
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
                    let width = clamp_intrinsic_width(node_ref, width);
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
                        let text = apply_text_transform(&contents.borrow(), &node.specified_values);
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            Some(0.0)
                        } else {
                            Some(
                                trimmed
                                    .split_whitespace()
                                    .map(|word| {
                                        measure_text_width(
                                            word,
                                            font_size,
                                            f32::INFINITY,
                                            resolved_font_style(node),
                                            resolved_letter_spacing_px(node),
                                        )
                                    })
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
                            Some(stated_outer_width(node, *v))
                        } else {
                            None
                        }
                    };

                    if let Some(width) = width {
                        let width = clamp_intrinsic_width(node, width);
                        self.min_content.insert(key, width);
                        val_stack.push(width);
                        continue;
                    }

                    let pad_border = horiz_padding_border(node);
                    let non_skip: Vec<&StyledNode> = node
                        .children
                        .iter()
                        .filter(|c| contributes_to_intrinsic_size(c))
                        .collect();
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
                    let start = val_stack.len().saturating_sub(num_children);
                    let child_vals: Vec<f32> = val_stack.drain(start..).collect();
                    // A single-line flex row cannot narrow past its items laid
                    // side by side, so its min-content is their sum plus the
                    // gaps. Only a wrapping row — or a column, whose items
                    // stack — may narrow to its widest item.
                    let flex_row_sum = get_display_type(node_ref) == DisplayType::Flex
                        && flex_axis_is_row(node_ref)
                        && !flex_container_wraps(node_ref);
                    let width = if flex_row_sum {
                        let non_skip_children: Vec<&StyledNode> = node_ref
                            .children
                            .iter()
                            .filter(|c| contributes_to_intrinsic_size(c))
                            .collect();
                        let items: Vec<f32> = child_vals
                            .iter()
                            .zip(non_skip_children.iter())
                            .filter(|(_, c)| is_flex_item(c))
                            .map(|(v, c)| v + horiz_margin(c))
                            .collect();
                        let gaps = if items.len() > 1 {
                            column_gap_px(node_ref) * (items.len() - 1) as f32
                        } else {
                            0.0
                        };
                        items.iter().sum::<f32>() + gaps + pad_border
                    } else {
                        child_vals.into_iter().fold(0.0f32, f32::max) + pad_border
                    };
                    let width = clamp_intrinsic_width(node_ref, width);
                    self.min_content.insert(key, width);
                    val_stack.push(width);
                }
            }
        }

        val_stack.pop().unwrap_or(0.0)
    }
}

/// The collapsed space a text node keeps on one edge of its parent's content.
///
/// CSS drops whitespace at the start and end of a block's inline content, so an
/// intrinsic measurement must not keep a space there. A period written as two
/// spans inside a `<p>` indented in the source carries a whitespace-only text
/// node on each side of them, and counting those made github-style date boxes
/// several spaces wider than the run they hold — wide enough to push the
/// heading beside them onto a second line.
fn edge_space_width(child: &StyledNode, leading: bool) -> f32 {
    let NodeData::Text { ref contents } = child.node.data else {
        return 0.0;
    };
    let raw = contents.borrow().to_string();
    if raw.is_empty() {
        return 0.0;
    }
    let font_size = node_font_size(child).max(1.0);
    let space = crate::font::fonts().advance(' ', font_size, resolved_font_style(child))
        + resolved_letter_spacing_px(child);
    if raw.trim().is_empty() {
        // The node is nothing but the space, so all of it goes.
        return space;
    }
    let at_edge = if leading {
        raw.starts_with(|c: char| c.is_whitespace())
    } else {
        raw.ends_with(|c: char| c.is_whitespace())
    };
    if at_edge {
        space
    } else {
        0.0
    }
}

/// How much to take off each child's intrinsic contribution because it sits at
/// the edge of a block's content. Indexed the same as the children.
fn edge_space_drops(node: &StyledNode, children: &[&StyledNode]) -> Vec<f32> {
    let mut drops = vec![0.0; children.len()];
    // Whitespace is dropped at the edge of a *block container*'s content, which
    // an inline box is not: there the space still separates this run from the
    // text beside it. Everything else — a block, an inline-block, a flex or grid
    // item, a table cell — establishes one.
    if children.is_empty() || get_display_type(node) == DisplayType::Inline {
        return drops;
    }
    drops[0] += edge_space_width(children[0], true);
    let last = children.len() - 1;
    drops[last] += edge_space_width(children[last], false);
    drops
}

/// The width of the collapsed whitespace a text node contributes to the run
/// around it.
///
/// Layout keeps one space where a text node begins or ends with whitespace, so
/// the intrinsic measurement has to keep it too. Trimming it away — as
/// `measure_text_width` does — made a run measure narrower than it lays out,
/// and a date like `2026-02 — Present`, written as two spans, wrapped inside a
/// box built without the space between them.
fn collapsed_space_width(node: &StyledNode, raw: &str, font_size: f32) -> f32 {
    let fonts = crate::font::fonts();
    let space = fonts.advance(' ', font_size.max(1.0), resolved_font_style(node))
        + resolved_letter_spacing_px(node);
    if raw.trim().is_empty() {
        // A whitespace-only node between two inline siblings is one space.
        return if raw.is_empty() { 0.0 } else { space };
    }
    let leading = raw.starts_with(|c: char| c.is_whitespace());
    let trailing = raw.ends_with(|c: char| c.is_whitespace());
    space * (leading as u8 + trailing as u8) as f32
}

/// Measure the width of `text` rendered at `font_size` px.
/// When `wrap_width` is `f32::INFINITY`, no wrapping occurs (max-content).
/// When finite, line-breaks at word boundaries (min-content: longest word).
/// How much a line may exceed its container before a word is pushed off it.
///
/// A browser lays text out in fixed-point units of 1/64 px, so a fit test there
/// is never decided by a difference smaller than that. This engine measures in
/// `f32`, where a shrink-to-fit box and the run inside it are summed in
/// different orders and land a few ULPs apart — enough, with an exact `>`, to
/// wrap a button's label that fits its own box exactly.
const LINE_FIT_EPSILON: f32 = 1.0 / 64.0;

/// How a run's lines are chosen — CSS `text-wrap-style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapStyle {
    /// Greedy: every line takes as many words as fit.
    Auto,
    /// Greedy, but a last line left holding a single word pulls one down from
    /// the line above it.
    Pretty,
    /// The same number of lines, made as even as the words allow.
    Balance,
}

/// `text-wrap-style`, from either the longhand or the `text-wrap` shorthand.
pub fn resolved_wrap_style(sn: &StyledNode) -> WrapStyle {
    for prop in ["text-wrap-style", "text-wrap"] {
        if let Some(Value::Keyword(k)) = sn.specified_values.get(&crate::css::intern(prop)) {
            // The shorthand states a mode and a style in either order, so both
            // words are looked at: `text-wrap: wrap pretty`.
            for word in k.split_whitespace() {
                match word {
                    "pretty" => return WrapStyle::Pretty,
                    "balance" => return WrapStyle::Balance,
                    _ => {}
                }
            }
        }
    }
    WrapStyle::Auto
}

/// Chromium balances a run only while it is short enough to be worth it, and
/// stops at six lines. Beyond that the cost of the search outweighs the look,
/// and a browser falls back to the greedy break.
const BALANCE_LINE_LIMIT: usize = 6;

/// Where a run of words breaks into lines, as the index of the first word on
/// each line.
///
/// `widths` are the words' own advances, `space_w` the space between two of
/// them, `first_indent` what an inline sibling already took on the first line,
/// and `avail` the width the run is laid into.
///
/// The greedy answer is the CSS default and what every other value is measured
/// against; `text-wrap-style` then adjusts it.
pub(crate) fn break_lines(
    widths: &[f32],
    space_w: f32,
    first_indent: f32,
    avail: f32,
    style: WrapStyle,
) -> Vec<usize> {
    let greedy = greedy_lines(widths, space_w, first_indent, avail);
    if widths.len() < 2 || greedy.len() < 2 || !avail.is_finite() {
        return greedy;
    }
    match style {
        WrapStyle::Auto => greedy,
        WrapStyle::Pretty => {
            // A last line holding one word is the orphan `pretty` exists to
            // avoid: the line above gives up its own last word, as long as the
            // two of them still fit together. github writes it on every body
            // run, and the greedy break left "onboarding" alone under a full
            // line on its customer stories.
            let last_start = greedy[greedy.len() - 1];
            if last_start + 1 != widths.len() {
                return greedy;
            }
            let prev_start = greedy[greedy.len() - 2];
            if last_start - prev_start < 2 {
                // The line above is a single word too; moving it down only
                // moves the orphan up.
                return greedy;
            }
            let pulled = line_width(widths, space_w, last_start - 1, widths.len());
            if pulled > avail + LINE_FIT_EPSILON {
                return greedy;
            }
            let mut out = greedy;
            let n = out.len();
            out[n - 1] = last_start - 1;
            out
        }
        WrapStyle::Balance => {
            if greedy.len() > BALANCE_LINE_LIMIT {
                return greedy;
            }
            // The evenest break is the narrowest width the run still fits in
            // without taking another line, so the answer is the smallest such
            // width — found by bisection, since fitting is monotone in it.
            let widest_word = widths.iter().fold(0.0_f32, |m, w| m.max(*w));
            let mut lo = widest_word.max(first_indent);
            let mut hi = avail;
            for _ in 0..24 {
                let mid = 0.5 * (lo + hi);
                if greedy_lines(widths, space_w, first_indent, mid).len() <= greedy.len() {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let balanced = greedy_lines(widths, space_w, first_indent, hi);
            if balanced.len() == greedy.len() {
                balanced
            } else {
                greedy
            }
        }
    }
}

/// The width of `widths[from..to]` set on one line, spaces included.
fn line_width(widths: &[f32], space_w: f32, from: usize, to: usize) -> f32 {
    let mut w = 0.0;
    for (i, word) in widths[from..to].iter().enumerate() {
        if i > 0 {
            w += space_w;
        }
        w += word;
    }
    w
}

/// The greedy break: every line takes as many words as fit.
fn greedy_lines(widths: &[f32], space_w: f32, first_indent: f32, avail: f32) -> Vec<usize> {
    let mut lines = vec![0usize];
    let mut line_w = first_indent;
    let mut words_on_line = 0usize;
    for (i, word_w) in widths.iter().enumerate() {
        // The space that goes before this word has to fit too — the same test
        // `layout_text` makes, so both agree about where the lines break.
        let needed = if words_on_line > 0 { space_w + word_w } else { *word_w };
        if avail.is_finite() && line_w + needed > avail + LINE_FIT_EPSILON && words_on_line > 0 {
            lines.push(i);
            line_w = 0.0;
            words_on_line = 0;
        }
        if words_on_line > 0 {
            line_w += space_w;
        }
        line_w += word_w;
        words_on_line += 1;
    }
    lines
}

fn measure_text_width(
    text: &str,
    font_size: f32,
    wrap_width: f32,
    style: crate::font::FontStyle,
    letter_spacing: f32,
) -> f32 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    let fonts = crate::font::fonts();
    let font_size = font_size.max(1.0);
    let space_w = fonts.advance(' ', font_size, style) + letter_spacing;

    let mut max_w: f32 = 0.0;
    let mut line_w: f32 = 0.0;

    for word in trimmed.split_whitespace() {
        let word_w = fonts.measure(word, font_size, style, letter_spacing);
        // The space that goes before this word has to fit too — see the same
        // test in `layout_text`, which decides where the lines actually break.
        let needed = if line_w > 0.0 { space_w + word_w } else { word_w };
        if wrap_width.is_finite()
            && line_w + needed > wrap_width + LINE_FIT_EPSILON
            && line_w > 0.0
        {
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
/// Read `aspect-ratio` as width-over-height, accepting both `16 / 9` and `1.777`.
/// Where a grid item sits on one axis: an optional explicit start line
/// (zero-based) and how many tracks it spans.
#[derive(Clone, Copy)]
struct GridPlacement {
    start: Option<usize>,
    span: usize,
}

/// Read `grid-column` / `grid-row` (or their `-start` / `-end` longhands).
///
/// Twelve-column systems place every item with `grid-column: auto / span 12`;
/// ignoring the span puts each item in a single 1/12-wide cell, which wraps body
/// text to one word per line and hides it under the container's overflow clip.
/// Named lines are not resolved — those fall back to auto placement.
fn read_grid_placement(sn: &StyledNode, axis: &str) -> GridPlacement {
    let read = |prop: &str| -> Option<String> {
        match sn.specified_values.get(&crate::css::intern(prop)) {
            Some(Value::Keyword(k)) => Some(k.trim().to_lowercase()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    };

    let (start_text, end_text) = match read(&format!("grid-{axis}")) {
        Some(shorthand) => match shorthand.split_once('/') {
            Some((a, b)) => (Some(a.trim().to_string()), Some(b.trim().to_string())),
            None => (Some(shorthand), None),
        },
        None => (read(&format!("grid-{axis}-start")), read(&format!("grid-{axis}-end"))),
    };

    // `span N` on the start side is a span with no explicit line.
    let parse_line = |text: &Option<String>| -> Option<i32> {
        let text = text.as_ref()?;
        if text.starts_with("span") || text == "auto" {
            return None;
        }
        text.parse::<i32>().ok()
    };
    let parse_span = |text: &Option<String>| -> Option<usize> {
        let rest = text.as_ref()?.strip_prefix("span")?.trim();
        rest.parse::<usize>().ok().filter(|n| *n > 0)
    };

    let start_line = parse_line(&start_text);
    let end_line = parse_line(&end_text);
    let span = parse_span(&start_text)
        .or_else(|| parse_span(&end_text))
        .or_else(|| match (start_line, end_line) {
            (Some(a), Some(b)) if b > a => Some((b - a) as usize),
            _ => None,
        })
        .unwrap_or(1);

    GridPlacement {
        // CSS grid lines are 1-based; a negative line counts from the end, which
        // needs the track count and is left to auto placement instead.
        start: start_line.filter(|l| *l >= 1).map(|l| (l - 1) as usize),
        span: span.max(1),
    }
}

/// Clamp a computed height to `min-height` / `max-height`.
///
/// Every formatting context has to apply these, not just block: a flex or grid
/// container whose height came from its own algorithm still answers to them,
/// and skipping the clamp there let a navbar sized by `min-height` collapse to
/// its content.
/// Apply `min-height` and `max-height`.
///
/// The bounds and the height have to be compared in the same box.
/// `min-height: 48px` on a `border-box` control bounds its *border* box, while
/// a height measured from the content is already one — so insetting the bound
/// and leaving the height alone, as this did, compared a bound shorn of its
/// padding against a height that still carried it, and a 48px search field came
/// out its content's 45px tall. `content_space` says which box `height` is in;
/// everything is compared in the border box and handed back the way it came.
fn clamp_height(
    sn: &StyledNode,
    box_sizing: &str,
    padding: &EdgeSizes,
    border: &EdgeSizes,
    height: f32,
    content_space: bool,
    vw: f32,
    vh: f32,
) -> f32 {
    let vert = padding.top + padding.bottom + border.top + border.bottom;
    let bound = |prop: &str| -> Option<f32> {
        // A percentage bound needs a containing block this function does not
        // have; passing NaN leaves it non-finite, which is discarded below —
        // the same as `none`.
        let v = resolve_constraint_px(
            sn.specified_values.get(&crate::css::intern(prop))?,
            f32::NAN,
            vw,
            vh,
            sn,
        )?;
        if !v.is_finite() {
            return None;
        }
        Some(if box_sizing == "border-box" { v } else { v + vert })
    };
    let mut border_h = if content_space { height + vert } else { height };
    if let Some(max_h) = bound("max-height") {
        border_h = border_h.min(max_h);
    }
    if let Some(min_h) = bound("min-height") {
        border_h = border_h.max(min_h);
    }
    if content_space {
        (border_h - vert).max(0.0)
    } else {
        border_h.max(0.0)
    }
}

/// Apply `text-transform` to a text run.
///
/// Labels set in small caps through `text-transform: uppercase` are common in
/// page furniture, and rendering them in their authored case reads as a
/// different design rather than as a rendering bug.
pub fn apply_text_transform(text: &str, sv: &crate::style::PropertyMap) -> String {
    let Some(Value::Keyword(k)) = sv.get(&crate::css::intern("text-transform")) else {
        return text.to_string();
    };
    match k.as_ref() {
        "uppercase" => text.to_uppercase(),
        "lowercase" => text.to_lowercase(),
        "capitalize" => text
            .split_inclusive(char::is_whitespace)
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect(),
        _ => text.to_string(),
    }
}

/// The border-box height `aspect-ratio` gives a box whose border-box width is
/// `border_w`.
///
/// The ratio sizes whichever box `box-sizing` names, so under `content-box` the
/// padding and border come off before it is applied and go back on after.
/// Applying it to the content box either way left a `border-box` frame short by
/// its own padding and border: github's hero frame — a `1000 / 1196` box with
/// 8px of padding and a 1px border — came out 21px short, and each of the four
/// hero sections ended that much high.
/// Re-derive a flex item's cross size from its `aspect-ratio` once flexing has
/// settled its main size.
///
/// `aspect-ratio` binds the two axes together, so growing or shrinking an item
/// along the main axis moves the other axis with it. Laying the item out again
/// re-derives its size from its *stated* width — the one flexing just overrode
/// — so a shrunk, ratio-sized frame kept the height it would have had at its
/// unshrunk width. On github's hero that is ~120px of extra height per section.
///
/// A stated size on the cross axis wins over the ratio, and is left alone.
fn apply_ratio_cross_size(cb: &mut LayoutBox<'_>, is_row: bool) {
    let Some(ratio) = read_aspect_ratio(cb.style_node) else {
        return;
    };
    let border_box = is_border_box(cb.style_node);
    if is_row {
        if has_stated(cb, "height") {
            return;
        }
        let bw = outer_width(cb);
        if bw <= 0.0 {
            return;
        }
        cb.dimensions.height =
            ratio_border_box_height(border_box, &cb.padding, &cb.border, ratio, bw);
        cb.content_box_height = false;
    } else {
        if has_stated(cb, "width") {
            return;
        }
        let bh = outer_height(cb);
        if bh <= 0.0 {
            return;
        }
        let horiz = cb.padding.left + cb.padding.right + cb.border.left + cb.border.right;
        let vert = cb.padding.top + cb.padding.bottom + cb.border.top + cb.border.bottom;
        cb.dimensions.width = if border_box {
            bh * ratio
        } else {
            (bh - vert).max(0.0) * ratio + horiz
        };
        cb.content_box_width = false;
    }
}

/// The mirror of `apply_ratio_cross_size`: take a ratio-sized item's *main*
/// size from the cross size that stretching just gave it.
///
/// Only applied when the main axis has nothing of its own to go on — no stated
/// size and nothing measured — so an item that shrink-wrapped around real
/// content keeps that width. An empty ratio-sized box has neither, and without
/// this came out zero along the main axis and never painted.
fn apply_ratio_main_size(cb: &mut LayoutBox<'_>, is_row: bool) {
    let Some(ratio) = read_aspect_ratio(cb.style_node) else {
        return;
    };
    let border_box = is_border_box(cb.style_node);
    let horiz = cb.padding.left + cb.padding.right + cb.border.left + cb.border.right;
    let vert = cb.padding.top + cb.padding.bottom + cb.border.top + cb.border.bottom;
    if is_row {
        if has_stated(cb, "width") || outer_width(cb) > 0.0 {
            return;
        }
        let bh = outer_height(cb);
        if bh <= 0.0 {
            return;
        }
        cb.dimensions.width = if border_box {
            bh * ratio
        } else {
            (bh - vert).max(0.0) * ratio + horiz
        };
        cb.content_box_width = false;
    } else {
        if has_stated(cb, "height") || outer_height(cb) > 0.0 {
            return;
        }
        let bw = outer_width(cb);
        if bw <= 0.0 {
            return;
        }
        cb.dimensions.height =
            ratio_border_box_height(border_box, &cb.padding, &cb.border, ratio, bw);
        cb.content_box_height = false;
    }
}

/// Whether this box hides its own baseline from the line it sits on.
///
/// An atomic inline normally lends the line the baseline of the text inside it,
/// and lines up with the surrounding text. Three things stop that, and each
/// makes the box's baseline its own bottom margin edge instead — so the whole
/// box sits above the line's baseline and the strut's descender space is still
/// kept underneath it:
///
///   * `contain: layout` (and the `content` and `strict` shorthands that
///     include it), per css-contain-2 §3.1 — github puts `contain: content` on
///     every link, which is 6px a row down its footer's link columns;
///   * a scroll container, since a scrolled baseline would not stay put;
///   * having no in-flow content to take a baseline from at all, which is what
///     an empty spacer or icon box is.
fn hides_own_baseline(cb: &LayoutBox<'_>) -> bool {
    // A form control's text is its value or its placeholder, which is not a
    // child node — so an empty child list does not mean an empty box, and the
    // control still lends the line the baseline of the line it holds.
    if cb.children.is_empty()
        && cb.display != DisplayType::Input
        && !is_form_control(cb.style_node)
    {
        return true;
    }
    let keyword = |prop: &str| match cb.style_node.specified_values.get(&crate::css::intern(prop)) {
        Some(Value::Keyword(k)) => Some(k.clone()),
        _ => None,
    };
    if let Some(k) = keyword("contain") {
        if k.split_whitespace()
            .any(|word| matches!(word, "layout" | "content" | "strict"))
        {
            return true;
        }
    }
    ["overflow", "overflow-y"].iter().any(|prop| {
        keyword(prop).is_some_and(|k| {
            k.split_whitespace()
                .any(|word| matches!(word, "hidden" | "scroll" | "auto" | "clip"))
        })
    })
}

/// The room a line box in this container keeps below the baseline, and above it.
fn strut_split(sn: &StyledNode) -> (f32, f32) {
    let line_height = resolved_line_height_px(sn);
    let below = crate::font::fonts().below_baseline(
        node_font_size(sn),
        resolved_font_style(sn),
        line_height,
    );
    ((line_height - below).max(0.0), below)
}

/// How far below a line member's own top its baseline sits.
///
/// An atomic inline whose baseline is its own bottom edge — an empty
/// `inline-block`, the twelve-pixel square an icon is — rests that edge on the
/// line's baseline, so its whole margin box is above it. Everything else is
/// itself a line box, and its baseline is its own strut's.
fn member_ascent(cb: &LayoutBox<'_>) -> f32 {
    let atomic = matches!(
        cb.display,
        DisplayType::InlineBlock
            | DisplayType::Flex
            | DisplayType::Grid
            | DisplayType::Table
            | DisplayType::Image
            | DisplayType::Input
    );
    if atomic && hides_own_baseline(cb) {
        return cb.margin.top + outer_height(cb) + cb.margin.bottom;
    }
    strut_split(cb.style_node).0
}

/// The height a line box has to be to hold `cb`, given the container's strut.
///
/// An atomic inline whose baseline is its own bottom edge sits entirely above
/// the line's baseline, so the strut's descender space is added under it.
/// Everything else contributes its own height, which is what this engine's line
/// boxes have always measured.
fn line_contribution(cb: &LayoutBox<'_>, container: &StyledNode) -> f32 {
    let own = outer_height(cb) + cb.margin.top + cb.margin.bottom;
    let atomic = matches!(
        cb.display,
        DisplayType::InlineBlock
            | DisplayType::Flex
            | DisplayType::Grid
            | DisplayType::Table
            | DisplayType::Image
            | DisplayType::Input
    );
    if !atomic || !hides_own_baseline(cb) {
        return own;
    }
    let (above, below) = strut_split(container);
    own.max(above) + below
}

/// The width and height a run of text takes when wrapped into `available`.
///
/// Greedy line breaking on spaces, the same rule `layout_text` follows, so the
/// two agree about how many lines a string needs.
fn wrapped_text_extent(sn: &StyledNode, text: &str, available: f32) -> (f32, f32) {
    let font_size = node_font_size(sn);
    let style = resolved_font_style(sn);
    let letter_spacing = resolved_letter_spacing_px(sn);
    let line_height = resolved_line_height_px(sn);
    let fonts = crate::font::fonts();
    let space = fonts.advance(' ', font_size, style) + letter_spacing;

    let mut lines = 1usize;
    let mut line_w = 0.0f32;
    let mut widest = 0.0f32;
    for word in text.split_whitespace() {
        let w = fonts.measure(word, font_size, style, letter_spacing);
        let with_space = if line_w > 0.0 { line_w + space + w } else { w };
        if line_w > 0.0 && with_space > available {
            widest = widest.max(line_w);
            lines += 1;
            line_w = w;
        } else {
            line_w = with_space;
        }
    }
    widest = widest.max(line_w);
    (widest.min(available.max(0.0)), lines as f32 * line_height)
}

/// The containing block an absolutely positioned child resolves against: its
/// ancestor's *padding* box, per CSS 2.2 §10.1.
///
/// Reading the content box instead left a `position: absolute; inset: 0`
/// overlay short by the padding it is meant to cover — the accent stripe down
/// the side of a padded card came out the height of its text rather than of the
/// card.
fn padding_box_of(cb: &LayoutBox<'_>) -> Rect {
    let border_w = outer_width(cb);
    let border_h = outer_height(cb);
    Rect {
        x: cb.dimensions.x + cb.border.left,
        y: cb.dimensions.y + cb.border.top,
        width: (border_w - cb.border.left - cb.border.right).max(0.0),
        height: (border_h - cb.border.top - cb.border.bottom).max(0.0),
    }
}

fn ratio_border_box_height(
    border_box: bool,
    padding: &EdgeSizes,
    border: &EdgeSizes,
    ratio: f32,
    border_w: f32,
) -> f32 {
    let horiz = padding.left + padding.right + border.left + border.right;
    let vert = padding.top + padding.bottom + border.top + border.bottom;
    if border_box {
        border_w / ratio
    } else {
        (border_w - horiz).max(0.0) / ratio + vert
    }
}

/// Resolve a size constraint — `min-width`, `max-width` and their block-axis
/// counterparts — to pixels against the containing block.
///
/// Anything that is not a length resolves to `None`, which is what `none` and
/// `auto` mean: no bound.
fn resolve_constraint_px(
    value: &Value,
    container: f32,
    vw: f32,
    vh: f32,
    sn: &StyledNode,
) -> Option<f32> {
    match value {
        Value::Length(v, Unit::Px) => Some(*v),
        Value::Length(v, Unit::Percent) => Some(container * (v / 100.0)),
        Value::Length(v, Unit::Vw) => Some(vw * (v / 100.0)),
        Value::Length(v, Unit::Vh) => Some(vh * (v / 100.0)),
        m @ Value::Math(_) => resolve_math_px(
            m,
            container,
            vw,
            vh,
            node_font_size(sn),
            resolved_font_style(sn),
        ),
        _ => None,
    }
}

fn read_aspect_ratio(sn: &StyledNode) -> Option<f32> {
    let value = sn.specified_values.get(&crate::css::intern("aspect-ratio"))?;
    let ratio = match value {
        Value::Number(n) => *n,
        Value::Keyword(k) => {
            let (w, h) = k.split_once('/')?;
            let (w, h) = (w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?);
            if h == 0.0 {
                return None;
            }
            w / h
        }
        _ => return None,
    };
    (ratio > 0.0 && ratio.is_finite()).then_some(ratio)
}

fn read_px_direct(sn: &StyledNode, prop: &str) -> f32 {
    match sn.specified_values.get(&crate::css::intern(prop)) {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    }
}

/// Horizontal padding + border contribution for intrinsic sizing (px only).
fn horiz_padding_border(sn: &StyledNode) -> f32 {
    let side = |side: &str| {
        let w = read_px_direct(sn, &format!("border-{side}-width"));
        if w > 0.0 { w } else { read_px_direct(sn, "border-width") }
    };
    read_px_direct(sn, "padding-left") + read_px_direct(sn, "padding-right") + side("left") + side("right")
}

/// Whether this element is a form control that holds a line of text of its own.
fn is_form_control(sn: &StyledNode) -> bool {
    matches!(
        sn.node.data,
        NodeData::Element { ref name, .. } if matches!(
            name.local.as_ref(),
            "input" | "textarea" | "select" | "button"
        )
    )
}

/// The single margin two adjoining margins collapse to.
///
/// CSS 2.2 §8.3.1: the largest positive plus the most negative, so a negative
/// margin still pulls the box back even when the margin it meets is zero.
/// Taking the plain maximum threw negative margins away entirely, and a page
/// that overlaps two sections on purpose — `margin-top: -10%` under a spacer —
/// got a gap where the overlap should be.
fn collapse_margins(a: f32, b: f32) -> f32 {
    a.max(b).max(0.0) + a.min(b).min(0.0)
}

/// Whether a child contributes to its parent's intrinsic (min- or max-content)
/// width.
///
/// An absolutely or fixedly positioned box is out of flow: it is sized against
/// its containing block, not against its parent's content, and it adds nothing
/// to what its parent has to be wide enough to hold. Counting github's floating
/// "Enter your email" label — `position: absolute` over the field it labels —
/// made the field 105px wider than the field itself.
fn contributes_to_intrinsic_size(child: &StyledNode) -> bool {
    !should_skip(child)
        && !matches!(
            get_position_type(child),
            PositionType::Absolute | PositionType::Fixed
        )
}

/// Whether a child of a flex or grid container is not an item of it.
///
/// CSS Flexible Box Layout §4: a contiguous run of text between two flex items
/// becomes an anonymous flex item, but "an anonymous flex item that contains
/// only white space is not rendered". The newlines and indentation between
/// markup tags are exactly that. Counting them as items gave a two-item row
/// four, and charged it the container's `gap` for each of the two it invented.
/// The `order` an item was given, which modifies the document order flex and
/// grid place their items in. Painting follows the same modified order.
fn order_of(sn: &StyledNode) -> i32 {
    match sn.specified_values.get(&crate::css::intern("order")) {
        Some(Value::Number(n)) => *n as i32,
        _ => 0,
    }
}

fn is_flex_item(child: &StyledNode) -> bool {
    if should_skip(child) {
        return false;
    }
    if let NodeData::Text { ref contents } = child.node.data {
        // Browsers drop it whatever `white-space` says — the space between two
        // flex items never lays out, preserved or not.
        return !contents.borrow().trim().is_empty();
    }
    true
}

/// Whether a stated width or height on this box already covers its padding and
/// border.
/// Clamp an intrinsic width by the box's own `min-width` and `max-width`.
///
/// What a box contributes to its parent's intrinsic size is its content bounded
/// by its own constraints. Leaving `min-width` out is how github's hero toggle
/// came out one button short of the row it holds: every button in it is
/// `min-width: 110px` around a label narrower than that, so the row was measured
/// from the labels and the last button was clipped away.
///
/// A percentage bound resolves against a containing block this measurement does
/// not have, so only the absolute forms count.
fn clamp_intrinsic_width(sn: &StyledNode, width: f32) -> f32 {
    let bound = |prop: &str| match sn.specified_values.get(&crate::css::intern(prop)) {
        // Under `content-box` the bound names the content width, and `width`
        // here is the outer one.
        Some(Value::Length(v, Unit::Px)) => Some(if is_border_box(sn) {
            *v
        } else {
            *v + horiz_padding_border(sn)
        }),
        _ => None,
    };
    let mut w = width;
    if let Some(max) = bound("max-width") {
        w = w.min(max);
    }
    if let Some(min) = bound("min-width") {
        w = w.max(min);
    }
    w
}

fn is_border_box(sn: &StyledNode) -> bool {
    matches!(
        sn.specified_values.get(&crate::css::intern("box-sizing")),
        Some(Value::Keyword(k)) if **k == *"border-box"
    )
}

/// The outer width a box with a *stated* `width` contributes to an intrinsic
/// measurement. Under `border-box` the stated value is already the outer width;
/// under `content-box` the padding and border sit outside it.
fn stated_outer_width(sn: &StyledNode, stated: f32) -> f32 {
    if is_border_box(sn) {
        stated
    } else {
        stated + horiz_padding_border(sn)
    }
}

fn horiz_margin(sn: &StyledNode) -> f32 {
    read_px_direct(sn, "margin-left") + read_px_direct(sn, "margin-right")
}

/// Whether `dimensions` already covers this box's padding and border on an
/// axis — true when nothing was stated for it, per the rule in `perform_layout`.
fn has_stated(cb: &LayoutBox<'_>, prop: &str) -> bool {
    cb.style_node
        .specified_values
        .contains_key(&crate::css::intern(prop))
}

/// This box's border-box width, whichever way `dimensions.width` was arrived at.
///
/// `border_box_width` adds padding and border unconditionally, which double
/// counts them for the common auto-width box whose `dimensions` already covers
/// them — a padded button then reserved its padding twice and the line came out
/// that much too wide.
pub(crate) fn outer_width(cb: &LayoutBox<'_>) -> f32 {
    if cb.content_box_width {
        border_box_width(cb)
    } else {
        cb.dimensions.width
    }
}

/// This box's border-box height. See `outer_width`.
pub(crate) fn outer_height(cb: &LayoutBox<'_>) -> f32 {
    if cb.content_box_height {
        border_box_height(cb)
    } else {
        cb.dimensions.height
    }
}

/// The floor `flex-shrink` may not push a row item below.
///
/// CSS resolves `min-width: auto` on a flex item to its min-content width, so
/// an item is never squeezed narrower than the longest word it contains. An
/// author who wants that opts out by stating `min-width` themselves — usually
/// `0`, which is why `minmax(0, 1fr)` and `min-width: 0` are such common
/// idioms — and a scroll container opts out by its `overflow`.
fn automatic_minimum_main_size(
    sn: &StyledNode,
    intrinsic_cache: &mut IntrinsicSizeCache,
    vw: f32,
    vh: f32,
) -> f32 {
    if sn.specified_values.contains_key(&crate::css::intern("min-width")) {
        return 0.0;
    }
    // A box with proportions of its own and a definite height carries that
    // height through the ratio — CSS calls it the transferred size suggestion,
    // and for a replaced element it is the whole content-based minimum: there
    // is no text inside to measure. It comes before the scroll-container
    // exception below, because an `<svg>` clipping its own contents is not a
    // scrollport for CSS boxes. github writes `overflow: hidden` on the logos in
    // its customer marquee, and reading that as "no minimum" shrank all twelve
    // of them to nothing wide and left the whole band empty.
    if let Some(ratio) = read_aspect_ratio(sn) {
        if ratio > 0.0 {
            if let Some(Value::Length(h, Unit::Px)) =
                sn.specified_values.get(&crate::css::intern("height"))
            {
                if *h > 0.0 {
                    return *h * ratio;
                }
            }
        }
    }
    let scrolls = matches!(
        sn.specified_values.get(&crate::css::intern("overflow")),
        Some(Value::Keyword(k)) if !matches!(k.as_ref(), "visible" | "clip")
    ) || matches!(
        sn.specified_values.get(&crate::css::intern("overflow-x")),
        Some(Value::Keyword(k)) if !matches!(k.as_ref(), "visible" | "clip")
    );
    if scrolls {
        return 0.0;
    }
    intrinsic_cache.min_content_width(sn, vw, vh)
}

impl<'a> LayoutBox<'a> {
    /// The rect paint, clipping and hit testing use: always the border box.
    ///
    /// `dimensions` is the border box already when the width or height was auto
    /// and the content box when it was stated, so the padding is added back only
    /// in the second case. Drawing `dimensions` unconditionally made a box with
    /// a declared width and padding paint its padding short.
    pub fn paint_rect(&self) -> Rect {
        Rect {
            x: self.dimensions.x,
            y: self.dimensions.y,
            width: outer_width(self),
            height: outer_height(self),
        }
    }
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

/// Whether a box sizes itself to its content rather than filling its container.
///
/// `inline-flex` and `inline-grid` need the element to answer this, not just its
/// `DisplayType`: they lay out their children like the block-level forms but
/// size the box like an inline-block.
fn is_shrink_wrap_for(sn: &StyledNode, d: DisplayType) -> bool {
    if is_inline_level_container(sn) {
        return true;
    }
    is_shrink_wrap(d)
}

/// `true` when the element's own `display` is one of the inline-level container
/// keywords.
fn is_inline_level_container(sn: &StyledNode) -> bool {
    // Out of flow, `inline-flex` blockifies to `flex` like everything else, so
    // the box neither flows inline nor shrink-wraps for that reason.
    if is_out_of_flow(sn) {
        return false;
    }
    matches!(
        sn.specified_values.get(&crate::css::intern("display")),
        Some(Value::Keyword(k)) if matches!(&**k, "inline-flex" | "inline-grid")
    )
}

/// The main axis of a flex container. Only meaningful when the node's
/// `DisplayType` is `Flex`.
fn flex_axis_is_row(sn: &StyledNode) -> bool {
    match sn
        .specified_values
        .get(&crate::css::intern("flex-direction"))
    {
        Some(Value::Keyword(k)) => matches!(&**k, "row" | "row-reverse"),
        _ => true,
    }
}

/// Whether a flex container is allowed to put its items on more than one line.
fn flex_container_wraps(sn: &StyledNode) -> bool {
    matches!(
        sn.specified_values.get(&crate::css::intern("flex-wrap")),
        Some(Value::Keyword(k)) if matches!(&**k, "wrap" | "wrap-reverse")
    )
}

/// The inline-axis gap between a flex or grid container's items, from
/// `column-gap` or the `gap` shorthand.
fn column_gap_px(sn: &StyledNode) -> f32 {
    match sn
        .specified_values
        .get(&crate::css::intern("column-gap"))
        .or_else(|| sn.specified_values.get(&crate::css::intern("gap")))
    {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Number(v)) => *v,
        _ => 0.0,
    }
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
    /// Width of the collapsed inter-element space reserved at the start of this
    /// text box, which paint must skip before placing the first glyph.
    pub text_leading: f32,
    pub alt_text: Option<String>,
    /// Label text for button/submit/reset input elements.
    /// Sourced from the `value` attribute of `<input type="submit|button|reset">`.
    /// Rendered centered inside the button rect by the paint pass.
    pub input_label: Option<String>,
    /// Placeholder text for an empty text field, drawn muted the way a browser
    /// draws it.
    pub input_placeholder: Option<String>,
    /// The text a field already holds. Painted like the placeholder but in the
    /// field's own colour; a GUI that overlays a real text widget draws its own
    /// background over this, so the raster and the widget cannot double up.
    pub input_value: Option<String>,
    /// The width this run was laid into, kept so paint can break its lines
    /// exactly where layout did. The box itself is only as wide as the widest
    /// line, which is not enough to re-run `text-wrap-style` against.
    pub wrap_width: f32,
    pub event_handlers: HashMap<String, String>,
    pub display: DisplayType,
    pub z_index: i32,
    pub position: PositionType,
    /// Marker text for list items (e.g. "•" for disc, "1." for decimal).
    /// `None` when `list-style-type: none` or the element is not a list item.
    pub list_marker: Option<String>,
    /// Whether `dimensions.width` holds the *content* box rather than the border
    /// box. Both are needed: a stated width has the padding and border taken off
    /// it, while a width arrived at by filling or shrink-wrapping already counts
    /// them. Everything outside layout goes through `outer_width`, which reads
    /// this to answer in the border box every reader expects.
    pub content_box_width: bool,
    /// The height axis of `content_box_width`.
    pub content_box_height: bool,
    /// Whether this box was placed against a containing block outside the
    /// subtree it sits in — an out-of-flow box whose positioned ancestor is
    /// further up than its parent.
    ///
    /// Such a box is already in page coordinates the moment it is placed, so
    /// moving the subtree around it must leave it where it is: a flex or grid
    /// item is laid out at the origin and offset into place afterwards, and
    /// carrying an absolutely positioned descendant along with it moved that
    /// descendant twice. github's hero carousel put its video 52px right of
    /// where the page has it, exactly the offset of the column it sits in.
    pub escapes_ancestor_offset: bool,
    /// The static position an out-of-flow box was placed from when its
    /// containing block belonged to an ancestor further up whose own height was
    /// not settled yet. That ancestor takes it from here and places it again.
    pub deferred_static_pos: Option<f32>,
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
                        text_leading: src.text_leading,
                        alt_text: src.alt_text.clone(),
                        input_label: src.input_label.clone(),
                        input_placeholder: src.input_placeholder.clone(),
                        input_value: src.input_value.clone(),
                        wrap_width: src.wrap_width,
                        event_handlers: src.event_handlers.clone(),
                        display: src.display,
                        z_index: src.z_index,
                        position: src.position,
                        list_marker: src.list_marker.clone(),
                        content_box_width: src.content_box_width,
                        content_box_height: src.content_box_height,
                        escapes_ancestor_offset: src.escapes_ancestor_offset,
                        deferred_static_pos: src.deferred_static_pos,
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
            text_leading: 0.0,
            alt_text: None,
            input_label: None,
            input_placeholder: None,
            input_value: None,
            wrap_width: f32::INFINITY,
            event_handlers: HashMap::new(),
            display,
            z_index,
            position,
            list_marker: None,
            content_box_width: false,
            content_box_height: false,
            escapes_ancestor_offset: false,
            deferred_static_pos: None,
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
            let mut input_placeholder: Option<String> = None;
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
                    "placeholder" if tag == "input" || tag == "textarea" => {
                        input_placeholder = Some(value)
                    }
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
            } else if input_value.as_deref().unwrap_or("").is_empty() {
                // An empty field shows its placeholder.
                if let Some(placeholder) = input_placeholder.filter(|p| !p.is_empty()) {
                    layout.input_placeholder = Some(placeholder);
                }
            } else if tag == "input" && !matches!(input_type.as_str(), "hidden" | "checkbox" | "radio") {
                // A field with something in it shows that instead. Leaving it to
                // the text widget a GUI overlays meant the raster — which is what
                // a screenshot and every headless render is — showed an empty box
                // where the page had a filled one.
                layout.input_value = input_value;
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

        let uniform = match sn.specified_values.get(&crate::css::intern("border-width")) {
            Some(Value::Length(v, Unit::Px)) => Some(*v),
            _ => None,
        };
        let default_width = if self.display == DisplayType::Input { 1.0 } else { 0.0 };
        let side_width = |side: &str| -> f32 {
            // `border-style: none` wins over any width, which is what makes
            // `border-bottom: none` on top of a shared rule actually remove the
            // rule instead of leaving a hairline behind.
            let style = sn
                .specified_values
                .get(&crate::css::intern(&format!("border-{side}-style")))
                .or_else(|| sn.specified_values.get(&crate::css::intern("border-style")));
            if matches!(style, Some(Value::Keyword(k)) if matches!(&**k, "none" | "hidden")) {
                return 0.0;
            }
            match sn.specified_values.get(&crate::css::intern(&format!("border-{side}-width"))) {
                Some(Value::Length(v, Unit::Px)) => *v,
                Some(Value::Number(v)) => *v,
                _ => uniform.unwrap_or(default_width),
            }
        };
        self.border = EdgeSizes {
            top: side_width("top"),
            right: side_width("right"),
            bottom: side_width("bottom"),
            left: side_width("left"),
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
        let is_block = is_block_level_for(self.style_node, self.display);

        // Block formatting context or similar check
        if is_block && initial_x > container_start_x {
            current_y += 5.0; // Break line before block
        }

        let is_floated = get_float(self.style_node).is_some();
        // What `dimensions.width` ends up meaning depends on where it came from,
        // and every reader below depends on knowing which:
        //
        //   * a **stated** width leaves it as the *content* box, since
        //     `border-box` sizing takes the padding and border back off;
        //   * an **auto** width leaves it as the *border* box, because both ways
        //     of arriving at one — shrink-to-fit from max-content, and a block
        //     filling `container_width - margins` — already count them in.
        //
        // Paint reads it as the border box, so an auto-width box draws correctly
        // and a stated-width one draws its padding short. See
        // `tools/parity/README.md` for why that has not been unified yet.
        let specified_width = self
            .style_node
            .specified_values
            .get(&crate::css::intern("width"));
        let auto_width = specified_width.is_none();

        // Whether the arm below yields a content-box width. A *stated* width is
        // one; a width arrived at by filling the line or shrink-wrapping the
        // content already counts the padding and border. `width: auto` is
        // written down but states nothing, so it belongs with the second group —
        // reading "the property is present" instead counted github's insets
        // twice and left the page hundreds of pixels tall.
        let mut width_is_content_box = true;
        let mut width = match specified_width {
            Some(Value::Length(v, Unit::Px)) => *v,
            Some(Value::Length(v, Unit::Percent)) => container_width * (v / 100.0),
            Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
            Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
            Some(v @ Value::Math(_)) => {
                resolve_math_px(v, container_width, vw, vh, node_font_size(self.style_node), resolved_font_style(self.style_node))
                    .unwrap_or(container_width)
            }
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
                width_is_content_box = false;
                if is_floated || is_shrink_wrap_for(self.style_node, self.display) {
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

        // `border-box` means a *stated* width already covers padding and border,
        // so they come off to leave the content width. An auto width has nothing
        // stated: shrink-to-fit already sized the box around its content, and
        // taking the padding off again made a button exactly its own padding too
        // narrow — so its label was drawn past the end of its background.
        if box_sizing == "border-box" && width > 0.0 && !auto_width {
            width = (width
                - self.padding.left
                - self.padding.right
                - self.border.left
                - self.border.right)
                .max(0.0);
        }

        // `min-width` and `max-width` have to be compared against `width` in the
        // space it is held in. A stated width leaves `width` as the content box;
        // an auto one leaves it as the border box, because shrink-to-fit sizes
        // the box around its content, padding and border included.
        let pad_border =
            self.padding.left + self.padding.right + self.border.left + self.border.right;
        let in_width_space = |stated: f32| -> f32 {
            match (box_sizing == "border-box", auto_width) {
                // The bound covers padding and border; `width` does not.
                (true, false) => (stated - pad_border).max(0.0),
                // The bound is a content width; `width` covers more than that.
                (false, true) => stated + pad_border,
                _ => stated,
            }
        };
        // A bound is a length like any other: `max-width: 60%` and
        // `max-width: calc(100% - 64px)` bind exactly as `max-width: 480px`
        // does. Reading only plain pixels dropped both — github's hero carousel
        // is held to `calc(100% - 2 * 32px)` and ran the full width of the
        // viewport instead, taking its glow off the edge of the page with it.
        let bound = |prop: &str| -> Option<f32> {
            resolve_constraint_px(
                self.style_node.specified_values.get(&crate::css::intern(prop))?,
                container_width,
                vw,
                vh,
                self.style_node,
            )
        };
        if let Some(v) = bound("max-width") {
            width = width.min(in_width_space(v));
        }
        if let Some(v) = bound("min-width") {
            width = width.max(in_width_space(v));
        }

        // `margin: 0 auto` centres the box's *border* box. `width` at this point
        // is the content width, so the padding and border have to be counted in
        // — leaving them out pushed a centred, padded container half its padding
        // off to one side and shifted its whole subtree with it.
        let self_outer_width = width + self.padding.left + self.padding.right + self.border.left + self.border.right;
        if is_block && self_outer_width < container_width {
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
                let leftover = (container_width - self_outer_width).max(0.0);
                self.margin.left = leftover / 2.0;
                self.margin.right = leftover / 2.0;
            }
        }

        self.dimensions.x = container_start_x + self.margin.left;
        self.dimensions.y = current_y + self.margin.top;
        self.dimensions.width = width;
        self.content_box_width = width_is_content_box;

        let height = match self
            .style_node
            .specified_values
            .get(&crate::css::intern("height"))
        {
            Some(Value::Length(v, Unit::Px)) => *v,
            Some(Value::Length(v, Unit::Vw)) => vw * (v / 100.0),
            Some(Value::Length(v, Unit::Vh)) => vh * (v / 100.0),
            // A percentage height resolves against the containing block's, and
            // only when that one is definite — otherwise CSS says `auto`, which
            // is what a zero here means to the code below. A containing block
            // still being laid out carries height 0, so the two cases fall out
            // of the same test. Without this an overlay written as
            // `position: absolute; height: 100%` came out nothing tall, and the
            // gradient wash it exists to paint never appeared.
            Some(Value::Length(v, Unit::Percent)) => containing_block
                .map(|cb| cb.height * (v / 100.0))
                .filter(|h| *h > 0.0)
                .unwrap_or(0.0),
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

        // A height that came out of a stated value is a content height; anything
        // else leaves the box to be measured, and a measured height is a border
        // box. `height: auto`, and a percentage against a containing block that
        // has no definite height of its own, both fall in the second group.
        self.content_box_height = height > 0.0;
        // The mirror of the width rule above: under `border-box` a stated
        // height already covers the padding and border, so they come off to
        // leave the content height that paint adds them back to. Without this a
        // `height: 48px` button drew 62px tall and pushed its whole row down.
        let height = if box_sizing == "border-box" && height > 0.0 {
            (height
                - self.padding.top
                - self.padding.bottom
                - self.border.top
                - self.border.bottom)
                .max(0.0)
        } else {
            height
        };

        if let NodeData::Text { ref contents } = self.style_node.node.data {
            // The full line-box width, not what is left of it: only the *first*
            // line of this run starts where the run does, and `layout_text`
            // works that out for itself.
            return self.layout_text(
                contents.borrow().to_string(),
                container_start_x,
                initial_x,
                current_y,
                container_width,
            );
        }

        // Image sizing: images need explicit dimension handling before child layout.
        // The image cache is not available at layout time, so we use placeholder
        // dimensions based on CSS-specified values. The object-fit logic at render time
        // will use the actual decoded image dimensions.
        if self.display == DisplayType::Image {
            // An image whose bytes never arrived is its alt text: a browser lays
            // the text out in the element's own box, so a broken image with a
            // sentence of alt is as tall as that sentence wrapped, not a fixed
            // placeholder. `aspect-ratio` is injected from the decoded bytes, so
            // its absence is what says the image has nothing of its own to be
            // sized by. github's pillar screenshots are all remote, and reading
            // them as a fixed box left each of its security columns 38px short.
            // `aspect-ratio` is injected from the decoded bytes, so its absence
            // is what says the image has nothing of its own to be sized by.
            let has_natural_size = read_aspect_ratio(self.style_node).is_some();
            let broken = !has_natural_size && height <= 0.0;
            let alt = self.alt_text.as_deref().unwrap_or("").trim().to_string();
            // An image that says nothing about itself and has nothing to say in
            // its place is not rendered at all where it is an ordinary inline
            // `<img>`: the whole element collapses, and a stated height does not
            // hold its row open. That is what `alt=""` asks for and what a
            // browser gives a source it could not fetch.
            //
            // A page that gave the image a display of its own still gets a box,
            // and the stated height holds: github's customer logos are
            // `inline-block` SVGs 42px tall, and collapsing those took 62px out
            // of the page. Its width is what an empty box shrinks to — nothing —
            // unless it is block-level, where it fills its line like any block.
            let plain_inline = !matches!(
                self.style_node.specified_values.get(&crate::css::intern("display")),
                Some(Value::Keyword(k)) if &**k != "inline"
            );
            if !has_natural_size && alt.is_empty() && auto_width {
                self.dimensions.width = if plain_inline || !is_block {
                    0.0
                } else {
                    container_width
                };
                self.dimensions.height = if plain_inline { 0.0 } else { height.max(0.0) };
                self.content_box_width = false;
                let final_x = self.dimensions.x + self.dimensions.width + self.margin.right;
                let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
                return (Some(self), final_x, final_y);
            }
            let alt_fallback = broken && !alt.is_empty();
            if alt_fallback {
                let avail = if auto_width {
                    container_width
                } else {
                    self.dimensions.width
                };
                let (text_w, text_h) = wrapped_text_extent(self.style_node, &alt, avail);
                if auto_width {
                    // A block-level image fills its line the way a block does;
                    // an inline one is only as wide as its text.
                    self.dimensions.width = if is_block { avail } else { text_w };
                    self.content_box_width = false;
                }
                self.dimensions.height = text_h.max(1.0);
                let final_x = self.dimensions.x + outer_width(&self) + self.margin.right;
                let final_y = self.dimensions.y + outer_height(&self) + self.margin.bottom;
                return (Some(self), final_x, final_y);
            }
            // A replaced element with a stated height, an auto width and
            // proportions of its own takes its width from the two — the mirror
            // of the height rule just below, and what CSS 10.3.2 says for a
            // replaced box. github's button icons are `width: auto; height:
            // 16px` around a 16x16 `viewBox`, and the 100px placeholder blew the
            // chevron beside "English" in its footer out to six times its size.
            //
            // `width: auto` is written down but states nothing, so it counts as
            // auto here the same as no width at all — those icons say it
            // outright.
            let width_is_auto = auto_width
                || matches!(
                    self.style_node.specified_values.get(&crate::css::intern("width")),
                    Some(Value::Keyword(k)) if &**k == "auto"
                );
            if width_is_auto && height > 0.0 {
                if let Some(ratio) = read_aspect_ratio(self.style_node) {
                    if ratio > 0.0 {
                        self.dimensions.width = height * ratio;
                        self.content_box_width = false;
                    }
                }
            }
            // width is already set by the shrink-wrap / explicit-CSS path above.
            // If no CSS width was specified, the shrink-wrap path returns a value from
            // compute_max_content_width (100px default for images). Keep that or fall back to 150.
            if self.dimensions.width <= 0.0 {
                self.dimensions.width = 150.0_f32.min(container_width);
                self.content_box_width = false;
            }
            // Use CSS height if specified; otherwise derive it from the image's
            // aspect ratio. `aspect-ratio` is injected from the decoded bytes once
            // the image has been fetched, so on the re-render after loading an
            // `<img width=…>` takes its real proportions instead of a placeholder.
            let final_h = if height > 0.0 {
                height
            } else if let Some(ratio) = read_aspect_ratio(self.style_node) {
                self.dimensions.width / ratio
            } else {
                self.dimensions.width * 0.667
            };
            self.dimensions.height = final_h.max(1.0);
            let final_x = self.dimensions.x + self.dimensions.width + self.margin.right;
            let final_y = self.dimensions.y + self.dimensions.height + self.margin.bottom;
            return (Some(self), final_x, final_y);
        }

        // The width children are laid out against. `width` is a content box when
        // it came from a stated value and a border box when it did not — see the
        // note where it is computed — so the padding comes off only in the
        // second case. Leaving it on meant a padded block handed its children
        // its own outer width, and their text ran the full width of the box
        // instead of stopping at its padding.
        let inner_width = if width > 0.0 {
            if auto_width {
                (width
                    - self.padding.left
                    - self.padding.right
                    - self.border.left
                    - self.border.right)
                    .max(0.0)
            } else {
                width
            }
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
        // A height that is settled before the children are laid out — stated
        // outright, or handed over by `aspect-ratio` once the width is known —
        // is a definite containing block, and percentage heights inside resolve
        // against it. Leaving it at zero meant the media that fills a
        // ratio-sized frame (`width: 100%; height: 100%`, which is how github's
        // hero holds its screenshot) resolved to `auto` and never painted.
        let vert_pad_border =
            self.padding.top + self.padding.bottom + self.border.top + self.border.bottom;
        let definite_content_height = if height > 0.0 {
            height
        } else {
            let bw = outer_width(&self);
            read_aspect_ratio(self.style_node)
                .filter(|_| bw > 0.0)
                .map(|ratio| {
                    (ratio_border_box_height(
                        box_sizing == "border-box",
                        &self.padding,
                        &self.border,
                        ratio,
                        bw,
                    ) - vert_pad_border)
                        .max(0.0)
                })
                .unwrap_or(0.0)
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
            height: definite_content_height,
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
                if !is_flex_item(child_node) {
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
                // In a column container the cross axis is horizontal, so an
                // `align-items` other than `stretch` shrink-wraps the item
                // instead of filling the line. Measuring it at the full width
                // regardless made every item exactly as wide as its container,
                // which left `center` nothing to centre.
                let child_align = child_node
                    .specified_values
                    .get(&crate::css::intern("align-self"))
                    .and_then(|v| if let Value::Keyword(k) = v { Some(&**k) } else { None })
                    .unwrap_or(align_items);
                let column_shrink_wraps = !is_row && child_align != "stretch";
                let measure_width =
                    if let Some(basis) = flex_basis {
                        basis.min(inner_width).max(0.0)
                    } else if (is_row || column_shrink_wraps)
                        && is_block_level_for(child_node, child_display)
                        && !child_has_explicit_width
                    {
                        // Shrink-wrap: use max-content so block items don't fill the flex
                        // container. The block sizing path subtracts the item's own margins
                        // from whatever container width it is handed, so they have to be
                        // added back here — max-content is the content width and knows
                        // nothing of them. Without that, an item with a side margin came
                        // out that much narrower than its own content, and its children
                        // were then shrunk to fit a box smaller than they needed.
                        (intrinsic_cache.max_content_width(child_node, vw, vh)
                            + horiz_margin(child_node))
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
                        // `flex-basis` sizes the box `box-sizing` names, exactly
                        // as `width` does — so a `flex: 1 1 0%` item starts at
                        // zero *content* width and still reserves its own
                        // padding on the line. Treating the basis as a border
                        // box instead handed every such item its padding as free
                        // space, and a padded column came out that much wide.
                        cb.content_box_width = !is_border_box(cb.style_node);
                    }
                    if cb.dimensions.width > inner_width {
                        cb.dimensions.width = inner_width;
                    }
                    // Flex items that come out with width=0 (e.g. inline <a> elements) need
                    // an intrinsic size so they participate correctly in the flex algorithm.
                    // Use max-content width capped to inner_width as the shrink-wrap fallback.
                    //
                    // `flex-basis: 0` is a zero width the author asked for, not a failure to
                    // measure, so it must survive this fallback: `flex: 1` expands to
                    // `1 1 0%`, and sizing those items by max-content instead makes every
                    // equal-width row (cards, columns, nav bars) come out proportional to
                    // its text and overflow the container.
                    if cb.dimensions.width == 0.0 && flex_basis.is_none() {
                        let max_c = intrinsic_cache.max_content_width(child_node, vw, vh);
                        cb.dimensions.width = max_c.min(inner_width).max(0.0);
                        // A max-content measurement already counts the insets.
                        cb.content_box_width = false;
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
            // An item's outer size on each axis. `margin_box_*` adds padding and
            // border unconditionally, which double counts them for the common
            // auto-sized item whose `dimensions` already covers them — a padded
            // pill then made its row that much taller than the browser draws it.
            let main_size = |cb: &LayoutBox<'_>| -> f32 {
                if is_row {
                    outer_width(cb) + cb.margin.left + cb.margin.right
                } else {
                    outer_height(cb) + cb.margin.top + cb.margin.bottom
                }
            };
            let cross_size = |cb: &LayoutBox<'_>| -> f32 {
                if is_row {
                    outer_height(cb) + cb.margin.top + cb.margin.bottom
                } else {
                    outer_width(cb) + cb.margin.left + cb.margin.right
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
                            // CSS gives a flex item an *automatic minimum size*:
                            // `min-width: auto` on a row item resolves to its
                            // min-content width, so shrinking never squeezes a
                            // box below the longest word it holds. Without that
                            // floor a row of tags came out with each tag broken
                            // across two lines, where the browser keeps every
                            // one of them whole.
                            let floor = automatic_minimum_main_size(
                                raw_items[i].cb.style_node,
                                intrinsic_cache,
                                vw,
                                vh,
                            );
                            raw_items[i].cb.dimensions.width =
                                (raw_items[i].cb.dimensions.width - reduction).max(floor);
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

                // The main size is settled; an `aspect-ratio` item's cross size
                // follows from it. Doing this after the reflow above rather than
                // inside it is what makes it stick: the reflow re-reads the
                // item's own stated width, which flexing has just overruled.
                for &i in line_indices {
                    apply_ratio_cross_size(&mut raw_items[i].cb, is_row);
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

                // Cross-axis size of this line: the tallest (or widest) item in
                // it. A single-line column container is the exception — its
                // cross axis is horizontal and the container's own width is
                // definite, so that width is the line. Taking the widest item
                // instead left `align-items: center` nothing to centre against,
                // since the widest item filled the line by definition.
                let mut line_cross: f32 = line_indices
                    .iter()
                    .map(|&i| cross_size(&raw_items[i].cb))
                    .fold(0.0_f32, f32::max);
                if !is_row && lines.len() == 1 {
                    line_cross = line_cross.max(inner_width);
                }
                // The row equivalent: a single line in a container with a
                // definite height fills that height, so `align-items: center`
                // centres against the container rather than against the tallest
                // item — which by definition fills the line.
                if is_row && lines.len() == 1 && height > 0.0 {
                    line_cross = line_cross.max(height);
                }

                // Place each item.
                for (idx_in_line, &i) in line_indices.iter().enumerate() {
                    let item = &mut raw_items[i];

                    // align-self overrides align-items for this item.
                    let effective_align = item.align_self.unwrap_or(align_items);

                    // Stretch cross axis if needed.
                    if effective_align == "stretch" {
                        // `line_cross` is a margin-box measurement, so what is
                        // left after the margins is the border box.
                        if is_row {
                            item.cb.dimensions.height =
                                (line_cross - item.cb.margin.top - item.cb.margin.bottom).max(0.0);
                            item.cb.content_box_height = false;
                        } else {
                            item.cb.dimensions.width =
                                (line_cross - item.cb.margin.left - item.cb.margin.right).max(0.0);
                            item.cb.content_box_width = false;
                        }
                        // Stretching settles the cross size, so for a
                        // ratio-sized item the *main* size now follows from it.
                        // Without this a `aspect-ratio: 2 / 1` item with nothing
                        // in it came out zero wide and never painted.
                        apply_ratio_main_size(&mut item.cb, is_row);
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
                    offset_item_keeping_escapes(&mut item.cb, dx, dy);

                    main_cursor += main_size(&item.cb);

                    max_child_x = max_child_x.max(
                        item.cb.dimensions.x + outer_width(&item.cb) + item.cb.margin.right,
                    );
                    child_y = child_y.max(
                        item.cb.dimensions.y + outer_height(&item.cb) + item.cb.margin.bottom,
                    );
                }

                cross_cursor += line_cross + cross_gap;
            }

            // Move items from raw_items into self.children.
            //
            // Through the flex algorithm an item's `dimensions` hold its *content*
            // box, because that is what flex-basis, grow and shrink operate on.
            // Everything past this point — paint, hit testing, the parent's own
            // sizing — reads `dimensions` as the border box, which is how every
            // other layout path stores it. Converting here, once the algorithm is
            // done, is what stops a padded flex item from painting its background
            // short of its own text and letting the next item sit on top of it.
            for mut item in raw_items {
                item.cb.dimensions.width = outer_width(&item.cb);
                item.cb.dimensions.height = outer_height(&item.cb);
                item.cb.content_box_width = false;
                item.cb.content_box_height = false;
                self.children.push(item.cb);
            }

            // Finalize flex container size.
            // cross_cursor added a cross_gap after every line, including the last
            // one, so one trailing gap always has to come back off. Taking it off
            // only for multi-line containers — as this did — made every single-line
            // flex row exactly one `gap` too tall, which on a page built from flex
            // rows accumulates into a visible drift down the page.
            let total_cross = if lines.is_empty() {
                cross_cursor
            } else {
                (cross_cursor - cross_gap).max(0.0)
            };
            // For column containers, derive the main-axis (height) from the actual child
            // positions rather than main_container_size (which is near-zero when height is auto).
            // Include padding.bottom + border.bottom, consistent with block layout (line ~1197).
            // A column container's height is measured from its own top edge, not
            // from its content edge: measuring from the content edge counts the
            // bottom padding but drops the top one.
            let column_main = if !is_row {
                (child_y - self.dimensions.y + self.padding.bottom + self.border.bottom).max(0.0)
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
                // Measured from the children out, so the insets are in it already.
                self.content_box_width = false;
            }
            // A stated height is the height, as it is in flow: deriving one from
            // the content whenever `dimensions.height` had not been set yet
            // threw it away, so a `display: flex` button with `height: 48px`
            // came out however tall its label made it.
            if height > 0.0 {
                self.dimensions.height = height;
            } else if self.dimensions.height <= 0.0 {
                // `total_cross` is the sum of the line heights, which is the
                // content box; a row container's own padding and border sit
                // outside it and have to be added back.
                self.dimensions.height = if is_row {
                    total_cross
                        + self.padding.top
                        + self.padding.bottom
                        + self.border.top
                        + self.border.bottom
                } else {
                    column_main
                };
                // A flex container may state an `aspect-ratio` instead of a
                // height, which is how a media wrapper reserves its space when
                // everything inside it is positioned. Without it the container
                // collapsed to nothing and the section below it moved up.
                let ratio_border_w = outer_width(&self);
                if let Some(ratio) = read_aspect_ratio(self.style_node) {
                    if height <= 0.0 && ratio_border_w > 0.0 {
                        self.dimensions.height = ratio_border_box_height(
                            box_sizing == "border-box",
                            &self.padding,
                            &self.border,
                            ratio,
                            ratio_border_w,
                        );
                    }
                }
                // A form control holds a line of its own even with no items in
                // it — see the same rule in the block path. Primer sets
                // `display: flex` on its text inputs, and without this
                // github's email field collapsed to its own top padding.
                if is_form_control(self.style_node) && self.children.is_empty() {
                    let line = resolved_line_height_px(self.style_node);
                    self.dimensions.height = self.dimensions.height.max(
                        line + self.padding.top
                            + self.padding.bottom
                            + self.border.top
                            + self.border.bottom,
                    );
                }
            }
            self.dimensions.height = clamp_height(
                self.style_node,
                box_sizing,
                &self.padding,
                &self.border,
                self.dimensions.height,
                self.content_box_height,
                vw,
                vh,
            );

            // ── Layout absolutely/fixedly positioned children inside flex container ──
            // Same logic as the block layout path: position after container size is known.
            if !flex_positioned_entries.is_empty() {
                let final_self_cb = padding_box_of(&self);

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
                        // Placed against a containing block further up than its
                        // parent, so it is already in page coordinates — see
                        // `escapes_ancestor_offset`.
                        pc.escapes_ancestor_offset = !self_establishes_cb;
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
                    // A one-track template is parsed as the length itself rather
                    // than as a keyword to split, and a single track is exactly
                    // what the animatable-disclosure pattern uses
                    // (`grid-template-rows: 0fr`). Reading only keywords left
                    // that template empty and the panel content-sized.
                    Some(v @ Value::Length(..)) => vec![v.clone()],
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

            // Auto-placement runs over the *order-modified* document order, the
            // same way it does for flex. github alternates its feature sections
            // by giving one column `order: 2` and the other `order: 1`, and
            // ignoring that put the screenshot on the side the text belongs on.
            grid_children.sort_by_key(|c| order_of(c));

            // Auto-placement, honouring explicit lines and spans. Occupancy is
            // tracked so an item with an explicit line does not get overwritten by
            // the item that follows it.
            struct Placement {
                col: usize,
                row: usize,
                col_span: usize,
                row_span: usize,
            }

            let mut occupied: Vec<Vec<bool>> = Vec::new();
            let mut placements: Vec<Placement> = Vec::with_capacity(grid_children.len());
            let (mut cursor_row, mut cursor_col) = (0usize, 0usize);

            for child_node in &grid_children {
                let col_place = read_grid_placement(child_node, "column");
                let row_place = read_grid_placement(child_node, "row");
                let col_span = col_place.span.min(num_cols).max(1);
                let row_span = row_place.span.max(1);

                let (col, row) = match (col_place.start, row_place.start) {
                    (Some(c), Some(r)) => (c.min(num_cols.saturating_sub(1)), r),
                    (Some(c), None) => (c.min(num_cols.saturating_sub(1)), cursor_row),
                    _ => {
                        if let Some(r) = row_place.start {
                            cursor_row = r;
                            cursor_col = 0;
                        }
                        // Walk forward to the first run of free tracks that fits.
                        loop {
                            if cursor_col + col_span > num_cols {
                                cursor_row += 1;
                                cursor_col = 0;
                                continue;
                            }
                            while occupied.len() <= cursor_row {
                                occupied.push(vec![false; num_cols]);
                            }
                            if (cursor_col..cursor_col + col_span).any(|c| occupied[cursor_row][c]) {
                                cursor_col += 1;
                                continue;
                            }
                            break;
                        }
                        (cursor_col, cursor_row)
                    }
                };

                while occupied.len() < row + row_span {
                    occupied.push(vec![false; num_cols]);
                }
                for r in row..row + row_span {
                    for c in col..(col + col_span).min(num_cols) {
                        occupied[r][c] = true;
                    }
                }
                if col + col_span >= cursor_col {
                    cursor_row = row;
                    cursor_col = col + col_span;
                }
                placements.push(Placement { col, row, col_span, row_span });
            }

            let num_rows_needed = occupied.len();

            // ── Resolve column widths ─────────────────────────────────────
            // Fixed tracks take their stated size; `auto` tracks take the widest
            // content placed in them; `fr` tracks share whatever is left. Sizing
            // `auto` from the free space instead — as this did — left nothing for
            // the `fr` beside it, so the second column of an `auto 1fr` sidebar
            // came out empty.
            let col_fixed_total: f32 = col_tracks.iter().map(|t| match t {
                Value::Length(v, Unit::Px) => *v,
                Value::Length(v, Unit::Percent) => inner_width * (v / 100.0),
                _ => 0.0,
            }).sum();

            let mut auto_widths: Vec<f32> = vec![0.0; num_cols];
            for (child_node, placement) in grid_children.iter().zip(&placements) {
                // Only an item confined to one track can size that track.
                if placement.col_span != 1 || placement.col >= num_cols {
                    continue;
                }
                if matches!(col_tracks.get(placement.col), Some(Value::Keyword(k)) if k.as_ref() == "auto") {
                    // An `auto` track is as wide as the widest thing in it, and
                    // an item is never narrower than its own `min-width` — which
                    // is how a search button is kept from shrinking to its label.
                    let content = intrinsic_cache.max_content_width(child_node, vw, vh);
                    auto_widths[placement.col] = auto_widths[placement.col].max(content);
                }
            }
            let auto_total: f32 = auto_widths.iter().sum();

            let col_fr_total: f32 = col_tracks.iter().map(|t| match t {
                Value::Length(v, crate::css::Unit::Fr) => *v,
                _ => 0.0,
            }).sum();
            let col_fr_space = (available_for_cols - col_fixed_total - auto_total).max(0.0);

            let col_widths: Vec<f32> = if col_tracks.is_empty() {
                vec![inner_width]
            } else {
                col_tracks.iter().enumerate().map(|(idx, t)| match t {
                    Value::Length(v, Unit::Px) => *v,
                    Value::Length(v, Unit::Percent) => inner_width * (v / 100.0),
                    Value::Length(v, crate::css::Unit::Fr) => {
                        if col_fr_total > 0.0 { (v / col_fr_total) * col_fr_space } else { 0.0 }
                    }
                    Value::Keyword(k) if k.as_ref() == "auto" => {
                        // With no flexible track to absorb it, an auto track
                        // spreads into the free space rather than hugging content.
                        if col_fr_total > 0.0 {
                            auto_widths[idx]
                        } else {
                            auto_widths[idx] + col_fr_space / auto_widths.iter().filter(|w| **w > 0.0).count().max(1) as f32
                        }
                    }
                    _ => 0.0,
                }).collect()
            };


            // Width of a run of tracks, gaps between them included.
            let span_width = |col: usize, span: usize| -> f32 {
                let end = (col + span).min(col_widths.len());
                let tracks: f32 = col_widths[col.min(col_widths.len())..end].iter().sum();
                let gaps = end.saturating_sub(col + 1) as f32 * col_gap;
                tracks + gaps
            };

            // Pass 1: lay out each child at its cell width to determine natural heights.
            struct GridItem<'gi> {
                cb: LayoutBox<'gi>,
                col: usize,
                row: usize,
                col_span: usize,
                row_span: usize,
            }

            let mut grid_items: Vec<GridItem<'_>> = Vec::new();
            for (child_node, placement) in grid_children.iter().zip(&placements) {
                let cell_width = span_width(placement.col, placement.col_span);

                let (cb_opt, _, _) = build_layout_tree_with_cb_cached(
                    child_node,
                    0.0, 0.0, 0.0,
                    cell_width.max(1.0),
                    vw, vh,
                    child_cb,
                    intrinsic_cache,
                );
                if let Some(cb) = cb_opt {
                    grid_items.push(GridItem {
                        cb,
                        col: placement.col,
                        row: placement.row,
                        col_span: placement.col_span,
                        row_span: placement.row_span,
                    });
                }
            }

            // Pass 2: compute implicit row heights (max of all cells in each row).
            let mut row_heights: Vec<f32> = vec![0.0_f32; num_rows_needed];
            for item in &grid_items {
                if item.row >= row_heights.len() {
                    continue;
                }
                let item_h = outer_height(&item.cb) + item.cb.margin.top + item.cb.margin.bottom;
                // A row-spanning item's height is shared across the rows it covers
                // rather than forced onto the first of them.
                let per_row = item_h / item.row_span as f32;
                for r in item.row..(item.row + item.row_span).min(row_heights.len()) {
                    row_heights[r] = row_heights[r].max(per_row);
                }
            }

            // Apply explicit row track heights if provided.
            // A row whose track states its own size does not grow to its
            // content: an item stretched into it takes the track's size exactly,
            // and whatever does not fit is the `overflow` property's business.
            let mut row_is_definite: Vec<bool> = vec![false; num_rows_needed];
            for (row_idx, row_h) in row_heights.iter_mut().enumerate() {
                if let Some(track) = row_tracks.get(row_idx) {
                    match track {
                        Value::Length(v, Unit::Px) => *row_h = row_h.max(*v),
                        Value::Length(v, Unit::Percent) => *row_h = row_h.max(vh * (v / 100.0)),
                        // A flexible row with a zero factor takes none of the
                        // free space and contributes no content of its own, so
                        // it is nothing tall. That is how a page holds a panel
                        // closed while keeping it animatable —
                        // `grid-template-rows: 0fr`, opened by swapping in
                        // `1fr` — and reading the row as content-sized instead
                        // left every collapsed accordion panel standing open.
                        // github's automation and collaboration sections came
                        // out ~300px tall each on that alone.
                        Value::Length(v, Unit::Fr) if *v == 0.0 => {
                            *row_h = 0.0;
                            row_is_definite[row_idx] = true;
                        }
                        _ => {}
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

            // CSS Box Alignment: a grid item fills its cell only under
            // `stretch`, which is the default. `align-items: center` is how a
            // page puts a portrait beside a taller column of text without
            // letting the portrait grow to match it — stretching regardless
            // made that image the height of the text next to it.
            let keyword_of = |sn: &StyledNode, prop: &str| -> Option<String> {
                match sn.specified_values.get(&crate::css::intern(prop)) {
                    Some(Value::Keyword(k)) => Some(k.to_string()),
                    _ => None,
                }
            };
            let container_align = keyword_of(self.style_node, "align-items");

            // Pass 4: position and (optionally) stretch each grid item.
            for item in &mut grid_items {
                let cell_x = col_lefts.get(item.col).copied().unwrap_or(content_left);
                let cell_y = row_tops.get(item.row).copied().unwrap_or(content_top);
                let cell_w = span_width(item.col, item.col_span);
                let cell_h: f32 = row_heights
                    [item.row.min(row_heights.len())..(item.row + item.row_span).min(row_heights.len())]
                    .iter()
                    .sum::<f32>()
                    + item.row_span.saturating_sub(1) as f32 * row_gap;

                // Width still fills the cell: `justify-items` other than
                // `stretch` needs the item re-measured shrink-to-fit, which is
                // not modelled.
                item.cb.dimensions.width = (cell_w
                    - item.cb.margin.left - item.cb.margin.right
                    - item.cb.padding.left - item.cb.padding.right
                    - item.cb.border.left - item.cb.border.right
                ).max(0.0);
                // What is left after the item's own insets is its content box.
                item.cb.content_box_width = true;

                let align = keyword_of(item.cb.style_node, "align-self")
                    .or_else(|| container_align.clone())
                    .unwrap_or_else(|| "stretch".to_string());
                let stretch_block = matches!(align.as_str(), "stretch" | "normal");

                // Stretch height only under `stretch`, and only when no
                // explicit height is set.
                let has_explicit_height = matches!(
                    item.cb.style_node.specified_values.get(&crate::css::intern("height")),
                    Some(Value::Length(v, Unit::Px)) if *v > 0.0
                );
                if stretch_block && !has_explicit_height {
                    let avail_cell_h = (cell_h
                        - item.cb.margin.top - item.cb.margin.bottom
                        - item.cb.padding.top - item.cb.padding.bottom
                        - item.cb.border.top - item.cb.border.bottom
                    ).max(0.0);
                    let definite_row = row_is_definite.get(item.row).copied().unwrap_or(false);
                    // `avail_cell_h` is what is left for the item's *content*
                    // once its own insets come off, so the comparison has to be
                    // against the item's content height too. Weighing it against
                    // the border box instead meant a padded item never looked
                    // short enough to stretch, and github's footer columns each
                    // kept their own height where the row makes them equal.
                    let item_content_h = (outer_height(&item.cb)
                        - item.cb.padding.top - item.cb.padding.bottom
                        - item.cb.border.top - item.cb.border.bottom)
                        .max(0.0);
                    if definite_row || avail_cell_h > item_content_h {
                        item.cb.dimensions.height = avail_cell_h;
                        item.cb.content_box_height = true;
                    }
                }

                // Where the item sits in a cell it does not fill.
                let free = (cell_h - margin_box_height(&item.cb)).max(0.0);
                let block_offset = match align.as_str() {
                    "center" => free / 2.0,
                    "end" | "flex-end" | "self-end" => free,
                    _ => 0.0,
                };
                let item_x = cell_x + item.cb.margin.left;
                let item_y = cell_y + item.cb.margin.top + block_offset;
                let dx = item_x - item.cb.dimensions.x;
                let dy = item_y - item.cb.dimensions.y;
                offset_item_keeping_escapes(&mut item.cb, dx, dy);

                max_child_x = max_child_x.max(
                    item.cb.dimensions.x + outer_width(&item.cb) + item.cb.margin.right,
                );
                child_y = child_y.max(
                    item.cb.dimensions.y + outer_height(&item.cb) + item.cb.margin.bottom,
                );
            }

            // A grid's content ends at the bottom of its last row, whatever the
            // items in it came out: a row is as tall as the tallest thing in it,
            // and an item that does not fill its row still leaves the row's
            // height behind it. Measuring only the item boxes made every grid
            // whose last row holds a short item come out that much short.
            if let (Some(top), Some(h)) = (row_tops.last(), row_heights.last()) {
                child_y = child_y.max(top + h);
            }

            // Move items into self.children.
            for item in grid_items {
                self.children.push(item.cb);
            }

            // Finalize grid container height.
            //
            // Measured from the container's own top edge, not from its content
            // edge: subtracting `content_top` counts the bottom padding but
            // silently drops the top one, so a padded grid came out exactly its
            // `padding-top` short. On a page that lays each entry out as a
            // padded grid — which is how a project list is built — that is a
            // whole `padding-top` lost per entry, and the page ends hundreds of
            // pixels short.
            let content_height = (child_y - self.dimensions.y
                + self.padding.bottom + self.border.bottom).max(0.0);
            // A stated height is the height; see the same rule in the flex path.
            if height > 0.0 {
                self.dimensions.height = height;
            } else if self.dimensions.height <= 0.0 {
                let ratio_border_w = outer_width(&self);
                self.dimensions.height = match read_aspect_ratio(self.style_node) {
                    Some(ratio) if ratio_border_w > 0.0 => ratio_border_box_height(
                        box_sizing == "border-box",
                        &self.padding,
                        &self.border,
                        ratio,
                        ratio_border_w,
                    ),
                    _ => content_height,
                };
            }
            self.dimensions.height = clamp_height(
                self.style_node,
                box_sizing,
                &self.padding,
                &self.border,
                self.dimensions.height,
                self.content_box_height,
                vw,
                vh,
            );

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
            } else if is_block_level_for(child_node, child_disp) {
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
        // Where the content box starts, so the parent/first-child collapse below
        // can tell "nothing placed yet" from "inline content already on a line".
        let initial_content_top = cursor_y;
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
                    let line_left = container_x + left_indent + align_offset;
                    let mut lx = line_left;
                    // Everything on a line hangs from one baseline. An atomic
                    // inline rests its bottom margin edge there, so a short one
                    // sits *below* the line's top rather than flush with it —
                    // which is where an icon beside a run of text goes, and
                    // where this put it three pixels too high.
                    let line_baseline = cur_line
                        .members
                        .iter()
                        .map(member_ascent)
                        .fold(0.0_f32, f32::max);
                    for mut m in cur_line.members.drain(..) {
                        // A text run's box spans the whole line box — its second
                        // and later lines start at the line's own left edge —
                        // and it carries what came before it on the first line
                        // as an indent. So it is placed at the line's edge
                        // rather than after its predecessor, and its right edge
                        // is where the next thing on the line goes.
                        let is_text = matches!(m.style_node.node.data, NodeData::Text { .. });
                        let place_at = if is_text { line_left } else { lx };
                        let dx = place_at - (m.dimensions.x - m.margin.left);
                        let top = cursor_y + (line_baseline - member_ascent(&m)).max(0.0);
                        let dy = top - (m.dimensions.y - m.margin.top);
                        offset_layout_box(&mut m, dx, dy);
                        max_child_x =
                            max_child_x.max(m.dimensions.x + outer_width(&m) + m.margin.right);
                        lx = place_at + outer_width(&m) + m.margin.left + m.margin.right;
                        result.push(m);
                    }
                    cursor_y += cur_line.height;
                    cur_line.width = 0.0;
                    cur_line.height = 0.0;
                    line_start_y = cursor_y;
                }
            };
        }

        // Each entry carries the flow cursor at the point the child was skipped:
        // that is its static position, which is where CSS puts an out-of-flow box
        // whose offsets are `auto`.
        // The third field is where the box belongs among its siblings. An
        // out-of-flow child is laid out after the in-flow ones — it has to be,
        // since its offsets resolve against a box whose size is not known until
        // then — but paint order among positioned boxes is *tree* order, so the
        // finished box goes back where the document put it. Appending them
        // instead let github's hero carousel paint its video under the two
        // gradients that sit behind it in the markup.
        let mut positioned_entries: Vec<(&StyledNode, f32, usize)> = Vec::new();

        // Counter for ordered list item markers (1., 2., …).
        // Only incremented when a ListItem child is placed in normal flow.
        let mut list_item_counter: u32 = 0;

        for entry in entries {
            match entry.kind {
                // ── Absolutely / fixedly positioned child — skip normal flow ──
                ChildKind::Positioned => {
                    // Collect for deferred layout after normal-flow finalisation.
                    positioned_entries.push((entry.node, cursor_y, result.len()));
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
                            .max(cb.dimensions.x + outer_width(&cb) + cb.margin.right);
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
                        let collapsed = if !first_block_placed
                            && parent_open_top
                            && result.is_empty()
                            && cursor_y == initial_content_top
                        {
                            // Case 2: no space between the parent's content edge
                            // and its first child. The child's margin does not
                            // vanish — it *becomes* the parent's top margin, so
                            // the parent moves down and the space ends up
                            // outside it. Dropping it instead pulled the top of
                            // every page up by its first paragraph's margin.
                            let collapsed_own = collapse_margins(self.margin.top, cb.margin.top);
                            let shift = collapsed_own - self.margin.top;
                            if shift != 0.0 {
                                self.margin.top = collapsed_own;
                                self.dimensions.y += shift;
                                cursor_y += shift;
                            }
                            0.0
                        } else {
                            // Case 1: standard adjacent-sibling collapse.
                            collapse_margins(prev_margin_bottom, cb.margin.top)
                        };
                        first_block_placed = true;
                        // `cb` was built at current_y = 0, so cb.dimensions.y == cb.margin.top.
                        // We want the content box to land at cursor_y + collapsed.
                        let dy = (cursor_y + collapsed) - cb.dimensions.y;
                        offset_layout_box(&mut cb, 0.0, dy);
                        // cursor_y advances using pre-offset (normal-flow) bottom
                        // edge — of the *border* box. `dimensions.height` is only
                        // that when no height was stated (see the note where
                        // `width` is computed), so reading it directly let a
                        // `height: 100px; padding: 8px` block hand the next one a
                        // cursor 16px too high, and everything below it climbed.
                        let normal_flow_bottom = cb.dimensions.y + outer_height(&cb);
                        // Apply relative offset AFTER computing normal-flow bottom so sibling
                        // placement is not affected (position:relative is a visual-only nudge).
                        if cb.position == PositionType::Relative {
                            apply_relative_offset(&mut cb, vw, vh);
                        }
                        cursor_y = normal_flow_bottom;
                        prev_margin_bottom = cb.margin.bottom;
                        max_child_x = max_child_x
                            .max(cb.dimensions.x + outer_width(&cb) + cb.margin.right);
                        result.push(cb);
                    }
                    line_start_y = cursor_y; // keep line_start_y in sync after block advances cursor_y
                }

                // ── Inline child ──────────────────────────────────────────────
                ChildKind::Inline => {
                    let (avail_w, left_indent) =
                        float_ctx.available_at(line_start_y, cur_line.height.max(16.0));
                    // The line's available width, whole. A text child must not be
                    // handed `avail_w - cur_line.width`: it is built with its
                    // start x already advanced past what the line holds, and
                    // takes that off itself. Subtracting here as well left the
                    // run with half the room it had, so a tag whose text exactly
                    // filled its box came out broken across two lines.
                    let child_container_width = avail_w;
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
                        // An image that came out zero wide meant it: it has
                        // nothing decoded and nothing to say in its place, and a
                        // browser gives it no room. Every other inline box that
                        // measured nothing is one this engine failed to size, so
                        // it falls back to its content.
                        if cb.dimensions.width == 0.0
                            && cb.display != DisplayType::Image
                            && !matches!(entry.node.node.data, NodeData::Text { .. })
                        {
                            let fallback_w = intrinsic_cache.max_content_width(entry.node, vw, vh);
                            cb.dimensions.width = fallback_w.min(child_container_width).max(0.0);
                            cb.content_box_width = false;
                        }
                        // Only reset prev_margin_bottom when a visible inline element is
                        // actually placed.  Empty/whitespace-only text nodes return None and
                        // must NOT interrupt adjacent-block margin collapsing.
                        //
                        // A block's bottom margin is held back to collapse with
                        // the *next block's* top margin. Real inline content
                        // after it forms an anonymous block, which has no margin
                        // to collapse with, so the held margin is simply space
                        // before it. Dropping it put an icon written between two
                        // paragraphs flush against the one above it.
                        if cur_line.members.is_empty() && prev_margin_bottom != 0.0 {
                            cursor_y += prev_margin_bottom;
                            line_start_y = cursor_y;
                        }
                        prev_margin_bottom = 0.0;
                        // A text run's box spans the whole line box, so the
                        // part of it the line already holds — the indent its
                        // first line starts at — is not new width.
                        let already_on_line = if matches!(entry.node.node.data, NodeData::Text { .. })
                        {
                            cur_line.width
                        } else {
                            0.0
                        };
                        let item_w = (outer_width(&cb) + cb.margin.left + cb.margin.right
                            - already_on_line)
                            .max(0.0);
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
                            let child_container_width2 = aw2;
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
                                    && cb2.display != DisplayType::Image
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
                                    outer_width(&cb2) + cb2.margin.left + cb2.margin.right;
                                cur_line.height = line_contribution(&cb2, self.style_node);
                                cur_line.members.push(cb2);
                            }
                        } else {
                            // Apply relative offset after accumulation so line-height measurement
                            // uses the pre-offset dimensions, and the visual nudge is applied before push.
                            if cb.position == PositionType::Relative {
                                apply_relative_offset(&mut cb, vw, vh);
                            }
                            cur_line.width += item_w;
                            cur_line.height =
                                cur_line.height.max(line_contribution(&cb, self.style_node));
                            cur_line.members.push(cb);
                        }
                    }
                }
            }
        }

        flush_line!();
        // Case 2 (bottom) — parent / last-child margin collapsing:
        // When no border, padding or stated height separates the parent's bottom
        // edge from its last block child, the child's bottom margin does not
        // vanish — it *becomes* the parent's bottom margin, so the space lands
        // below the parent rather than inside it. Dropping it left no gap at all
        // between a section ending in a paragraph and whatever followed.
        if !parent_open_bottom || height > 0.0 {
            cursor_y += prev_margin_bottom;
        } else if first_block_placed {
            self.margin.bottom = collapse_margins(self.margin.bottom, prev_margin_bottom);
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
            // Measured from the children out, so the insets are in it already.
            self.content_box_width = false;
        }

        let content_height =
            (cursor_y - self.dimensions.y + self.padding.bottom + self.border.bottom).max(0.0);
        // `aspect-ratio` is not only for images: a media container, a card or a
        // video wrapper states one and lets the height follow from the width.
        // Applying it only to replaced elements left every such box the height
        // of its content — zero, when the content inside it is positioned — and
        // the whole section below it moved up.
        // The ratio has to be applied to the box it sizes, which is the border
        // box under `border-box` sizing — see `ratio_border_box_height`.
        let ratio_border_w = outer_width(&self);
        let ratio_height = read_aspect_ratio(self.style_node)
            .filter(|_| height <= 0.0 && ratio_border_w > 0.0)
            .map(|ratio| {
                ratio_border_box_height(
                    box_sizing == "border-box",
                    &self.padding,
                    &self.border,
                    ratio,
                    ratio_border_w,
                )
            });
        let mut final_h = match (height > 0.0, ratio_height) {
            (true, _) => height,
            (false, Some(h)) => h,
            (false, None) => content_height,
        };
        // A form control with no in-flow children still holds one line: its
        // value or its placeholder sits on it, and a browser sizes the control
        // from that line rather than collapsing it to its padding. The element
        // is what decides this, not its `display`: a design system that sets
        // `display: flex` on its text inputs — which Primer does — still gets a
        // control the height of a line, and reading the `display` instead
        // collapsed github's email field to its own top padding.
        if (self.display == DisplayType::Input || is_form_control(self.style_node))
            && self.children.is_empty()
            && height <= 0.0
        {
            let line = resolved_line_height_px(self.style_node);
            final_h = final_h.max(
                line + self.padding.top + self.padding.bottom + self.border.top + self.border.bottom,
            );
        }
        self.dimensions.height = clamp_height(
            self.style_node,
            box_sizing,
            &self.padding,
            &self.border,
            final_h,
            self.content_box_height,
            vw,
            vh,
        );

        // ── Layout absolutely/fixedly positioned children ─────────────────────
        // Now that self has its final dimensions, we can resolve absolute offsets against it.
        if !positioned_entries.is_empty() {
            // The final content-box of self (after height is known).
            let final_self_cb = padding_box_of(&self);

            // Every insertion shifts the ones after it, and the entries are in
            // document order, so the offset is just how many have gone in.
            let mut inserted = 0usize;
            for (pos_node, static_y, sibling_index) in positioned_entries {
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

                if let Some(mut pc) = place_positioned_child(
                    pos_node,
                    static_y,
                    cb_for_child,
                    vw,
                    vh,
                    intrinsic_cache,
                ) {
                    // Placed against a containing block further up than its
                    // parent, so it is already in page coordinates — see
                    // `escapes_ancestor_offset`.
                    pc.escapes_ancestor_offset = !self_establishes_cb;
                    // The inherited block's height is only known once the
                    // ancestor that establishes it has finished its own layout,
                    // and that has not happened yet. Record what this box was
                    // placed from so that ancestor can place it again.
                    if !self_establishes_cb && child_pos_type != PositionType::Fixed {
                        pc.deferred_static_pos = Some(static_y);
                    }

                    let at = (sibling_index + inserted).min(self.children.len());
                    self.children.insert(at, pc);
                    inserted += 1;
                }
            }
        }

        // An absolute box laid out by a *static* ancestor was sized against a
        // containing block whose height was still zero: this box's own height is
        // not settled until its in-flow children are, and the descendant was
        // placed before that. Now that it is settled, place them again. Until
        // this ran, `height: 100%` on such a box — how github's hero lays its
        // glow behind the carousel — came out nothing and never painted.
        if self_establishes_cb {
            let settled_cb = padding_box_of(&self);
            let mut pending = Vec::new();
            collect_deferred_positioned(&mut self, &mut pending);
            for (path, static_y, node) in pending {
                if let Some(pc) =
                    place_positioned_child(node, static_y, settled_cb, vw, vh, intrinsic_cache)
                {
                    if let Some(slot) = child_at_path_mut(&mut self, &path) {
                        let mut replacement = pc;
                        replacement.escapes_ancestor_offset = true;
                        *slot = replacement;
                    }
                }
            }
        }
        // Flow advances past the *border* box. `dimensions` is only that when
        // nothing was stated for the axis — see the note where `width` is
        // computed — so a box that states a height and carries padding used to
        // hand the next block a cursor its own padding too high, and every
        // section below it climbed by that much.
        let final_x = if is_block {
            container_start_x
        } else {
            self.dimensions.x + outer_width(&self) + self.margin.right
        };
        let final_y = if is_block {
            self.dimensions.y + outer_height(&self) + self.margin.bottom
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
        // Line breaking has to see the run as it will be drawn, so the transform
        // is applied before anything is measured.
        let text = apply_text_transform(&text, &self.style_node.specified_values);
        let trimmed = text.trim();
        let font_size = match self
            .style_node
            .specified_values
            .get(&crate::css::intern("font-size"))
        {
            Some(Value::Length(v, Unit::Px)) => v.max(1.0),
            // Zero is the only unitless font-size CSS accepts, and it means the
            // text takes no space. Any other bare number is a value this parser
            // failed to attach a unit to, and must not be read as pixels.
            Some(Value::Number(v)) if *v == 0.0 => 0.0,
            _ => 16.0,
        };
        let fonts = crate::font::fonts();
        let line_height = resolved_line_height_px(self.style_node);
        let font_style = resolved_font_style(self.style_node);
        let letter_spacing = resolved_letter_spacing_px(self.style_node);
        let space_w = fonts.advance(' ', font_size, font_style) + letter_spacing;
        let white_space = self
            .style_node
            .specified_values
            .get(&crate::css::intern("white-space"))
            .and_then(|v| if let Value::Keyword(k) = v { Some(&**k as &str) } else { None })
            .unwrap_or("normal");
        // `pre` and `nowrap` both stop wrapping; `pre-line` and `pre-wrap` wrap
        // as usual but keep the source's own line breaks.
        let no_wrap = white_space == "nowrap" || white_space == "pre";
        let preserve_newlines = preserves_newlines(white_space);

        // CSS white-space: normal — whitespace-only text nodes between inline
        // elements collapse to a single inter-element space.  We only insert
        // that space when we are NOT at the start of a line (i.e. there is
        // already inline content to our left).
        let at_line_start = current_x <= container_start_x + 0.5;

        if trimmed.is_empty() {
            // Whitespace-only node: emit a single-space-wide invisible box so
            // the next inline sibling is separated from the previous one.
            if !at_line_start && text.contains(|c: char| c.is_whitespace()) {
                // Sized and placed like any other run: the box spans the line
                // box, with what came before it on the line carried as the
                // first line's indent.
                let indent = (current_x - container_start_x).max(0.0);
                let w = space_w.min((container_width - indent).max(0.0));
                self.text_leading = indent;
                self.dimensions.x = container_start_x + self.margin.left;
                self.dimensions.y = current_y + self.margin.top;
                self.dimensions.width = indent + w;
                self.dimensions.height = line_height;
                let final_x = self.dimensions.x + self.dimensions.width + self.margin.right;
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

        // Only the *first* line of a run starts where the run does: what an
        // inline sibling to its left already used is unavailable on that line
        // and available on every line after it. Measuring the whole run against
        // the leftover width wrapped a heading with a bold lead-in a line early
        // — and every line of every paragraph with an inline element in it.
        let first_line_indent = (current_x - container_start_x).max(0.0)
            + if has_leading_space { space_w } else { 0.0 };

        // `text-wrap-style` adjusts the greedy break, so the run's own word
        // advances are kept while it is measured — but only when a style asks
        // for them, since every ordinary run takes the greedy answer as it is.
        let wrap_style = resolved_wrap_style(self.style_node);
        let restyle_lines = wrap_style != WrapStyle::Auto
            && !no_wrap
            && !preserve_newlines
            && container_width.is_finite();
        let mut word_widths: Vec<f32> = Vec::new();
        let mut any_word_overlong = false;

        let mut lines_count = 1;
        // The indent is part of the first line's width; a break resets it,
        // since the next line starts at the container's own edge.
        let mut line_w: f32 = first_line_indent;
        let mut max_w: f32 = 0.0;
        // A line may only break *between* two words. The leading inter-element
        // space counts toward the width but is not something to break after:
        // treating it as content let the very first word wrap onto a second
        // line, so a tag whose text exactly filled its box came out split in
        // two where the browser keeps it whole.
        let mut word_on_line = false;

        let segments: Vec<&str> = if preserve_newlines {
            trimmed.split('\n').collect()
        } else {
            vec![trimmed]
        };
        for (segment_index, segment) in segments.into_iter().enumerate() {
            if segment_index > 0 {
                // A newline the source kept: close the line whatever is on it.
                max_w = max_w.max(line_w);
                line_w = 0.0;
                lines_count += 1;
                word_on_line = false;
            }
            for word in segment.split_whitespace() {
                let word_w = fonts.measure(word, font_size, font_style, letter_spacing);
                if restyle_lines {
                    word_widths.push(word_w);
                    // A word wider than its container is broken between its own
                    // characters, which is not something a wrap style rearranges.
                    any_word_overlong |= word_w > container_width;
                }

                if no_wrap {
                    if line_w > 0.0 {
                        line_w += space_w;
                    }
                    line_w += word_w;
                    word_on_line = true;
                    continue;
                }

                // Does this word still fit? The space that will be put before
                // it is part of what has to fit: leaving it out of the test but
                // adding it to the line afterwards let a line end up a space
                // wider than its container, so a last word that misses the
                // line by a hair stayed on it — which is one line's difference
                // in the height of every paragraph it happens to.
                let needed = if word_on_line { space_w + word_w } else { word_w };
                if container_width.is_finite()
                    && line_w + needed > container_width + LINE_FIT_EPSILON
                    && word_on_line
                {
                    max_w = max_w.max(line_w);
                    line_w = 0.0;
                    lines_count += 1;
                    word_on_line = false;
                }

                // If a single word is LONGER than the entire container, we must
                // break it char-by-char
                if container_width.is_finite() && word_w > container_width {
                    for c in word.chars() {
                        let char_w = fonts.advance(c, font_size, font_style) + letter_spacing;
                        if line_w + char_w > container_width + LINE_FIT_EPSILON && line_w > 0.0 {
                            max_w = max_w.max(line_w);
                            line_w = 0.0;
                            lines_count += 1;
                        }
                        line_w += char_w;
                    }
                    word_on_line = true;
                } else {
                    if word_on_line {
                        line_w += space_w;
                    }
                    line_w += word_w;
                    word_on_line = true;
                }
            }
        }

        // Trailing inter-element space: allows the following inline sibling to
        // start with a visible gap even though it has no leading whitespace itself.
        if has_trailing_space && line_w > 0.0 {
            line_w += space_w;
        }

        max_w = max_w.max(line_w);

        // Re-break under the run's own `text-wrap-style`. The greedy pass above
        // settled the default and told this one whether the run is the kind a
        // style applies to at all.
        if restyle_lines && !any_word_overlong && word_widths.len() > 1 {
            let starts = break_lines(
                &word_widths,
                space_w,
                first_line_indent,
                container_width,
                wrap_style,
            );
            lines_count = starts.len();
            max_w = 0.0;
            for (i, start) in starts.iter().enumerate() {
                let end = starts.get(i + 1).copied().unwrap_or(word_widths.len());
                let mut w = line_width(&word_widths, space_w, *start, end);
                if i == 0 {
                    w += first_line_indent;
                }
                if i + 1 == starts.len() && has_trailing_space {
                    w += space_w;
                }
                max_w = max_w.max(w);
            }
        }

        // Paint has to break the run in the same places, and the box it is
        // given is only as wide as the widest line — not the room the run had.
        self.wrap_width = if no_wrap { f32::INFINITY } else { container_width };

        // The box spans the whole line box, because that is where its second
        // and later lines live; the first line starts `text_leading` in, which
        // is what an inline sibling to the left of it took plus any collapsed
        // space between the two. Paint reads it the same way.
        self.text_leading = first_line_indent;
        self.dimensions.x = container_start_x + self.margin.left;
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
        Some(v @ Value::Math(_)) => resolve_math_px(v, container_size, vw, vh, node_font_size(sn), resolved_font_style(sn)),
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

/// The line box height for a node, honouring `line-height`.
///
/// A unitless `line-height` is a multiplier of the element's own font size; a
/// length is used as-is; a percentage resolves against the font size. `normal`
/// comes from the font's vertical metrics, not a fixed multiplier — a guessed
/// factor makes every block on a page the wrong height, and the error compounds
/// down a long document.
/// The face an element's text is set in.
///
/// Layout has to know this because a bold face is wider than the regular one at
/// the same size: measuring every run with the regular face made bold headings
/// and brand marks come out short, so the box behind them ended before the text
/// did and whatever followed sat on top of it.
pub fn resolved_font_style(sn: &StyledNode) -> crate::font::FontStyle {
    let sv = &sn.specified_values;
    // CSS font matching with only two weights bundled: a desired weight above
    // 500 takes the heavier face, 500 and below the lighter one. The threshold
    // was 600, so a design system asking for 560 — a real value, and what
    // yunseong.dev's hero links are set in — came out regular.
    let wants_bold = |weight: f32| weight > 500.0;
    let bold = match sv.get(&crate::css::intern("font-weight")) {
        Some(Value::Keyword(k)) => match k.as_ref() {
            "bold" | "bolder" => true,
            other => other.parse::<f32>().is_ok_and(wants_bold),
        },
        Some(Value::Number(v)) => wants_bold(*v),
        Some(Value::Length(v, _)) => wants_bold(*v),
        _ => false,
    };
    let italic = matches!(
        sv.get(&crate::css::intern("font-style")),
        Some(Value::Keyword(k)) if matches!(k.as_ref(), "italic" | "oblique")
    );
    let family = font_family_stack(sv)
        .as_deref()
        .map(generic_family_for)
        .unwrap_or_default();
    crate::font::FontStyle { bold, italic, family, web_family: web_family_for(sv) }
}

/// The element's `font-family` as written, if it has one.
fn font_family_stack(sv: &crate::style::PropertyMap) -> Option<String> {
    match sv.get(&crate::css::intern("font-family")) {
        Some(Value::Keyword(k)) => Some(k.to_string()),
        Some(Value::RawCustomProp(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Which bundled family a `font-family` stack resolves to.
///
/// Font matching walks the stack **in order** and takes the first family the
/// system can satisfy, so the order is the whole answer: a page that writes
/// `"Pretendard Variable", Pretendard, -apple-system, system-ui, ..., sans-serif`
/// gets `system-ui` here, not `sans-serif`, because that is the first name this
/// renderer has a face for. Reading the stack as a set and asking only whether
/// it mentions `serif` or `monospace` anywhere let every such stack fall
/// through to the sans default — and DejaVu Sans, which `system-ui` resolves
/// to, is wide enough that the whole page then laid out at the wrong measure.
///
/// Which names count as satisfiable was measured against the reference
/// renderer, not assumed: `-apple-system`, `BlinkMacSystemFont`, `ui-sans-serif`
/// and the rest of the `ui-*` generics resolve to nothing there and are stepped
/// over, and so is any face by its own name. Only the generic keywords this
/// bundles a face for stop the walk.
pub fn generic_family_for(stack: &str) -> crate::font::GenericFamily {
    stack
        .split(',')
        .find_map(|family| {
            let name = family
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_ascii_lowercase();
            satisfiable_family(&name)
        })
        // A stack this renderer can satisfy nothing in falls to the standard
        // font, which the reference sets to a serif.
        .unwrap_or(crate::font::GenericFamily::Serif)
}

/// The bundled family one `font-family` name asks for, or `None` when this
/// renderer has no face for it and matching moves on to the next name.
fn satisfiable_family(name: &str) -> Option<crate::font::GenericFamily> {
    use crate::font::GenericFamily;
    Some(match name {
        "monospace" => GenericFamily::Mono,
        "serif" => GenericFamily::Serif,
        "system-ui" => GenericFamily::SystemUi,
        // The reference resolves these to the same metrics as `sans-serif`,
        // being metric-compatible with the face it bundles for it.
        "sans-serif" | "arial" | "helvetica" => GenericFamily::Sans,
        _ => return None,
    })
}

/// The registered web-font family an element's `font-family` stack selects.
///
/// The stack is walked in order, exactly as font matching does: the first name
/// the page actually loaded a face for wins, and a stack whose custom faces all
/// failed to load falls through to the bundled ones.
fn web_family_for(sv: &crate::style::PropertyMap) -> Option<u16> {
    if !crate::font::has_web_faces() {
        return None;
    }
    let stack = match sv.get(&crate::css::intern("font-family")) {
        Some(Value::Keyword(k)) => k.to_string(),
        Some(Value::RawCustomProp(s)) => s.to_string(),
        _ => return None,
    };
    stack
        .split(',')
        .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\''))
        .find_map(crate::font::web_family_id)
}

/// Whether a `white-space` value keeps the newlines in the source text.
///
/// `pre-line` is how a page writes a paragraph whose line breaks are part of
/// the copy: it still collapses runs of spaces, but every newline is a break
/// the browser honours. Treating one as ordinary whitespace re-flowed such a
/// paragraph to whatever width it happened to have, so it came out a line short
/// and everything below it moved up.
pub fn preserves_newlines(white_space: &str) -> bool {
    matches!(white_space, "pre" | "pre-line" | "pre-wrap" | "break-spaces")
}

/// The `white-space` an element computes to.
pub fn resolved_white_space(sn: &StyledNode) -> &'static str {
    match sn.specified_values.get(&crate::css::intern("white-space")) {
        Some(Value::Keyword(k)) => match &**k {
            "pre" => "pre",
            "pre-line" => "pre-line",
            "pre-wrap" => "pre-wrap",
            "break-spaces" => "break-spaces",
            "nowrap" => "nowrap",
            _ => "normal",
        },
        _ => "normal",
    }
}

/// `letter-spacing` in pixels. `normal` is zero.
pub fn resolved_letter_spacing_px(sn: &StyledNode) -> f32 {
    match sn.specified_values.get(&crate::css::intern("letter-spacing")) {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Length(v, Unit::Em)) => resolved_font_size_px(sn) * *v,
        Some(Value::Length(v, Unit::Percent)) => resolved_font_size_px(sn) * (*v / 100.0),
        _ => 0.0,
    }
}

pub fn resolved_line_height_px(sn: &StyledNode) -> f32 {
    let font_size = resolved_font_size_px(sn);
    match sn.specified_values.get(&crate::css::intern("line-height")) {
        Some(Value::Length(v, Unit::Px)) => (*v).max(0.0),
        Some(Value::Length(v, Unit::Percent)) => (font_size * (*v / 100.0)).max(0.0),
        Some(Value::Number(v)) => (font_size * *v).max(0.0),
        _ => crate::font::fonts().normal_line_height(font_size, resolved_font_style(sn)),
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

/// Resolve a `calc()`/`clamp()` value that still holds a percentage.
///
/// Expressions without percentages are already folded to pixels during style
/// computation; only the ones needing a containing-block basis reach here.
fn resolve_math_px(
    value: &Value,
    cb: f32,
    vw: f32,
    vh: f32,
    font_size: f32,
    font_style: crate::font::FontStyle,
) -> Option<f32> {
    let Value::Math(expr) = value else { return None };
    expr.resolve(&crate::css::MathContext {
        viewport_width: vw,
        viewport_height: vh,
        font_size,
        font_style,
        root_font_size: font_size,
        percent_basis: Some(cb),
    })
}

/// The computed `font-size` of a node in pixels, for `em`-relative math.
fn node_font_size(sn: &StyledNode) -> f32 {
    match sn.specified_values.get(&crate::css::intern("font-size")) {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 16.0,
    }
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
        Some(v @ Value::Math(_)) => resolve_math_px(v, cw, vw, vh, node_font_size(sn), resolved_font_style(sn)).unwrap_or(0.0),
        _ => 0.0,
    }
}

fn specified_width_percent(sn: &StyledNode) -> Option<f32> {
    match sn.specified_values.get(&crate::css::intern("width")) {
        Some(Value::Length(v, Unit::Percent)) => Some(*v),
        _ => None,
    }
}

/// Lay out one out-of-flow child against `cb`, at `static_y` when it states no
/// offsets, and return it in page coordinates.
fn place_positioned_child<'a>(
    pos_node: &'a StyledNode,
    static_y: f32,
    cb: Rect,
    vw: f32,
    vh: f32,
    intrinsic_cache: &mut IntrinsicSizeCache,
) -> Option<LayoutBox<'a>> {
    // If both left and right are specified and no explicit width, the element
    // stretches to fill the space between them (CSS spec §10.3.7).
    let left_offset = resolve_offset(pos_node, "left", cb.width, vw, vh);
    let right_offset = resolve_offset(pos_node, "right", cb.width, vw, vh);
    let top_offset = resolve_offset(pos_node, "top", cb.height, vw, vh);
    let bottom_offset = resolve_offset(pos_node, "bottom", cb.height, vw, vh);

    let child_explicit_width = pos_node.specified_values.get(&crate::css::intern("width"));
    let child_layout_width = match child_explicit_width {
        Some(Value::Length(v, Unit::Px)) => *v,
        Some(Value::Length(v, Unit::Percent)) => cb.width * (v / 100.0),
        _ => {
            // Both left and right specified without explicit width → stretch.
            if let (Some(l), Some(r)) = (left_offset, right_offset) {
                (cb.width - l - r).max(0.0)
            } else {
                // Shrink-wrap: lay out at max-content width bounded by cb width.
                let max_c = intrinsic_cache.max_content_width(pos_node, vw, vh);
                max_c.min(cb.width)
            }
        }
    };

    // A child that states its own width resolves it against the containing
    // block, so that is what it has to be laid out against: handing it the
    // already-resolved width instead made `width: 40%` mean 40% of 40%, and a
    // wash pinned across half a section came out a fifth of it.
    let container_for_child = if child_explicit_width.is_some() {
        cb.width
    } else {
        child_layout_width
    };
    // Build the child at a temporary origin; we reposition it below.
    let (pc_opt, _, _) = build_layout_tree_with_cb_cached(
        pos_node,
        0.0,
        0.0,
        0.0,
        container_for_child.max(1.0),
        vw,
        vh,
        Some(cb),
        intrinsic_cache,
    );
    let mut pc = pc_opt?;

    // Both `top` and `bottom` with no stated height stretches the box between
    // them, the vertical mirror of the width rule above (CSS 2.2 §10.6.4).
    // Without it an overlay written as `position: absolute; inset: 0` came out
    // its content's height — zero, for the empty element a gradient wash is —
    // and never painted.
    let child_states_height = pc
        .style_node
        .specified_values
        .contains_key(&crate::css::intern("height"));
    if !child_states_height {
        if let (Some(t), Some(b)) = (top_offset, bottom_offset) {
            let stretched = (cb.height - t - b - pc.margin.top - pc.margin.bottom).max(0.0);
            if stretched > outer_height(&pc) {
                pc.dimensions.height = stretched;
                pc.content_box_height = false;
            }
        }
    }
    let target_x = match (left_offset, right_offset) {
        (Some(l), _) => cb.x + l + pc.margin.left,
        (None, Some(r)) => cb.x + cb.width - r - pc.dimensions.width - pc.margin.right,
        (None, None) => cb.x + pc.margin.left, // default to CB origin
    };
    let target_y = match (top_offset, bottom_offset) {
        (Some(t), _) => cb.y + t + pc.margin.top,
        (None, Some(b)) => cb.y + cb.height - b - pc.dimensions.height - pc.margin.bottom,
        // Neither offset given: the box stays where it would have been in flow.
        // Falling back to the containing block's origin instead pulls it to the
        // top of its ancestor, so a hero pinned below a header jumped to the
        // page top.
        (None, None) => static_y + pc.margin.top,
    };

    let dx = target_x - pc.dimensions.x;
    let dy = target_y - pc.dimensions.y;
    offset_layout_box(&mut pc, dx, dy);
    Some(pc)
}

/// Gather every descendant that was placed against an inherited containing
/// block, as `(path from `root`, static position, style node)`.
///
/// The search does not cross into a box that establishes a containing block of
/// its own: what is deferred inside that one is its business, not `root`'s.
fn collect_deferred_positioned<'a>(
    root: &mut LayoutBox<'a>,
    out: &mut Vec<(Vec<usize>, f32, &'a StyledNode)>,
) {
    fn walk<'a>(
        boxes: &mut [LayoutBox<'a>],
        prefix: &mut Vec<usize>,
        out: &mut Vec<(Vec<usize>, f32, &'a StyledNode)>,
    ) {
        for (i, child) in boxes.iter_mut().enumerate() {
            prefix.push(i);
            if let Some(static_y) = child.deferred_static_pos.take() {
                out.push((prefix.clone(), static_y, child.style_node));
            } else {
                let establishes = !matches!(child.position, PositionType::Static);
                if !establishes {
                    walk(&mut child.children, prefix, out);
                }
            }
            prefix.pop();
        }
    }
    let mut prefix = Vec::new();
    walk(&mut root.children, &mut prefix, out);
}

/// The box at `path` under `root`, where each step indexes `children`.
fn child_at_path_mut<'a, 'b>(
    root: &'b mut LayoutBox<'a>,
    path: &[usize],
) -> Option<&'b mut LayoutBox<'a>> {
    let mut cur = root;
    for &i in path {
        cur = cur.children.get_mut(i)?;
    }
    Some(cur)
}

fn get_display_type(sn: &StyledNode) -> DisplayType {
    if let NodeData::Text { .. } = sn.node.data {
        return DisplayType::Inline;
    }
    // Check if this is a form control element first — CSS display:block
    // on <input>/<button>/<select>/<textarea> means "fill width" not "become block".
    // These must always use DisplayType::Input so collect_form_controls finds them.
    let is_form_control = if let NodeData::Element { ref name, .. } = sn.node.data {
        matches!(name.local.to_string().as_str(), "input" | "button" | "select" | "textarea")
    } else {
        false
    };
    // A replaced element keeps its own box whatever `display` the page states:
    // the keyword decides how the box participates in flow, not whether the
    // element still draws its contents. `display: block` on an <img> is how
    // nearly every stylesheet removes the inline baseline gap under a picture,
    // and treating that as "become a plain block" stopped the image being
    // painted at all and left the box with no intrinsic height.
    //
    // An inline `<svg>` is the same: github's buttons put `display: flex` on
    // their icons, and laying one out as an empty flex container shrank it to
    // nothing — the chevron beside "English" in its footer disappeared.
    let is_image = matches!(
        sn.node.data,
        NodeData::Element { ref name, .. }
            if matches!(
                name.local.as_ref(),
                "img" | "svg" | "canvas" | "video" | "iframe" | "embed" | "object"
            )
    );
    if let Some(Value::Keyword(d)) = sn.specified_values.get(&crate::css::intern("display")) {
        match &**d {
            "none" => return DisplayType::Inline,
            _ if is_image => return DisplayType::Image,
            "block" if is_form_control => return DisplayType::Input,
            "block" => return DisplayType::Block,
            "inline-block" if is_form_control => return DisplayType::Input,
            "inline-block" => return blockify_out_of_flow(sn, DisplayType::InlineBlock),
            "flex" => return DisplayType::Flex,
            "grid" => return DisplayType::Grid,
            // `inline-flex` lays its children out exactly like `flex`; the
            // difference is in how the box itself flows, which
            // `is_block_level_for` and `is_shrink_wrap_for` read off the
            // keyword. Falling through to the tag default instead left every
            // Primer button an ordinary inline box, so a "Sign in" pill was
            // drawn the full width of the header.
            "inline-flex" => return DisplayType::Flex,
            "inline-grid" => return DisplayType::Grid,
            _ => {}
        }
    }
    let tag_display = if let NodeData::Element { ref name, .. } = sn.node.data {
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
            // Replaced elements reserve a box from their own width, height and
            // aspect ratio whether or not anything can draw their contents. A
            // page whose hero is a canvas or a video loses that whole box
            // otherwise, and everything below it moves up.
            "img" | "svg" | "canvas" | "video" | "iframe" | "embed" | "object" => {
                DisplayType::Image
            }
            _ => DisplayType::Inline,
        }
    } else {
        DisplayType::Block
    };
    blockify_out_of_flow(sn, tag_display)
}

/// Taking a box out of flow blockifies it (CSS Display §2.7): an absolutely
/// positioned or floated box is block-level whatever its `display` said.
///
/// Every generated box this engine builds is a `<span>`, so `::before { content:
/// ""; position: absolute; inset: 0 }` — the way a page lays a wash, a glow or a
/// disc behind its content — was laid out inline and came out its content's
/// size, which for an empty generated box is nothing at all. github's hero glow
/// and the disc behind its play button are both written that way.
fn blockify_out_of_flow(sn: &StyledNode, d: DisplayType) -> DisplayType {
    if !is_out_of_flow(sn) {
        return d;
    }
    match d {
        DisplayType::Inline | DisplayType::InlineBlock => DisplayType::Block,
        // A replaced element and a form control keep their own type: it says
        // how the box is *drawn*, not how it flows.
        other => other,
    }
}

/// `true` when `position` or `float` takes this box out of normal flow.
fn is_out_of_flow(sn: &StyledNode) -> bool {
    if matches!(
        get_position_type(sn),
        PositionType::Absolute | PositionType::Fixed
    ) {
        return true;
    }
    matches!(
        sn.specified_values.get(&crate::css::intern("float")),
        Some(Value::Keyword(k)) if matches!(&**k, "left" | "right" | "inline-start" | "inline-end")
    )
}

/// Whether a box participates in flow as block-level.
///
/// A replaced element keeps its own `DisplayType` so it still draws, which means
/// that type no longer says how the box flows: `display: block` on an `<img>`
/// must put it on its own line, and reading block-ness off the type alone left
/// such images overlapping the content around them.
fn is_block_level_for(sn: &StyledNode, d: DisplayType) -> bool {
    // An inline-level container flows inline however it lays its children out.
    if is_inline_level_container(sn) {
        return false;
    }
    if is_block_level(d) {
        return true;
    }
    matches!(
        sn.specified_values.get(&crate::css::intern("display")),
        Some(Value::Keyword(k)) if matches!(&**k, "block" | "flex" | "grid" | "list-item" | "table")
    )
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
    // A comment is not content: it takes no box and, being nothing, does not
    // stand between two blocks whose margins collapse. Laying one out as an
    // empty inline broke that collapse, so a comment written between a heading
    // and a paragraph pushed the paragraph down by the heading's whole margin.
    if matches!(
        child.node.data,
        NodeData::Comment { .. } | NodeData::ProcessingInstruction { .. } | NodeData::Doctype { .. }
    ) {
        return true;
    }
    // Then the CSS display property — display:none always hides the element.
    if is_none_display(child) {
        return true;
    }
    if let NodeData::Element { ref name, ref attrs, .. } = child.node.data {
        let t = name.local.to_string();
        // The `hidden` attribute is `display: none` in every UA stylesheet, and
        // pages lean on that default rather than writing the rule themselves —
        // dismissed banners and flash-message templates ship in the markup and
        // stay in it. `<template>` holds inert content that is never rendered.
        if t == "template" {
            return true;
        }
        if attrs.borrow().iter().any(|a| {
            a.name.local.as_ref() == "hidden" && !a.value.as_ref().eq_ignore_ascii_case("until-found")
        }) {
            return true;
        }
        if matches!(
            t.as_str(),
            "head" | "style" | "meta" | "title" | "script" | "link" | "noscript"
        ) {
            return true;
        }
        // An `<svg>` is a replaced element and keeps its box — its size comes
        // from its `width`/`height` attributes and its `viewBox`, and a page
        // that draws its logo as inline SVG loses that whole box otherwise. Its
        // *contents* are a different language and are not laid out as HTML:
        // anything else in the SVG namespace is skipped, subtree and all.
        if name.ns.as_ref() == "http://www.w3.org/2000/svg" && t != "svg" {
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

/// Move a flex or grid item into its place, leaving behind the out-of-flow
/// boxes inside it that were placed against a containing block further up.
///
/// Such an item is laid out at the origin and offset afterwards, so everything
/// measured in its own space has to move — but a descendant already placed in
/// page coordinates has not, and carrying it along moved it twice. github's
/// hero carousel put its video 52px right of where the page has it, exactly the
/// offset of the column it sits in.
fn offset_item_keeping_escapes(layout: &mut LayoutBox, dx: f32, dy: f32) {
    let mut stack: Vec<*mut LayoutBox> = vec![layout as *mut LayoutBox];
    let root: *const LayoutBox = layout as *const LayoutBox;
    while let Some(ptr) = stack.pop() {
        // SAFETY: as in `offset_layout_box`.
        let node = unsafe { &mut *ptr };
        if !std::ptr::eq(node as *const LayoutBox, root) && node.escapes_ancestor_offset {
            continue;
        }
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


    /// A percentage height resolves against the containing block's, and only
    /// when that one is definite. An overlay written as
    /// `position: absolute; height: 100%` was otherwise nothing tall, and the
    /// gradient wash it exists to paint never appeared.
    #[test]
    fn test_percentage_height_resolves_against_the_containing_block() {
        let html = r#"<div id="cb" style="position:relative;width:400px;height:300px">
            <div id="full" style="position:absolute;height:100%;width:50%"></div>
            <div id="half" style="position:absolute;height:50%"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        assert_eq!(find_element_by_id(&layout, "full").expect("full").dimensions.height, 300.0);
        assert_eq!(find_element_by_id(&layout, "half").expect("half").dimensions.height, 150.0);
    }

    /// A containing block still being laid out has no definite height, so a
    /// percentage against it is `auto` — the box takes its content's height.
    #[test]
    fn test_percentage_height_against_an_indefinite_block_is_auto() {
        let html = r#"<div id="outer"><div id="inner" style="height:100%"><div style="height:20px"></div></div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        assert_eq!(
            find_element_by_id(&layout, "inner").expect("inner").dimensions.height,
            20.0
        );
    }

    /// A positioned child states its width against the containing block, so
    /// that is what it has to be laid out against. Handing it the
    /// already-resolved width made `width: 40%` mean 40% of 40%.
    #[test]
    fn test_percentage_width_on_a_positioned_child_is_not_applied_twice() {
        let html = r#"<div id="cb" style="position:relative;width:800px;height:200px">
            <div id="wash" style="position:absolute;left:5%;width:40%;height:40px"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let wash = find_element_by_id(&layout, "wash").expect("wash");
        assert_eq!(wash.dimensions.width, 320.0, "40% of 800");
        assert_eq!(wash.dimensions.x, 40.0, "5% of 800");
    }

    // ── Flex container sizing ─────────────────────────────────────────────────

    /// A stated height is the height of a flex container too. Deriving one from
    /// the content whenever it had not been set yet threw it away, so a
    /// `display: flex` button with `height: 48px` — which is how a design
    /// system builds every button that centres its label — came out however
    /// tall its label made it.
    #[test]
    fn test_a_flex_container_keeps_its_stated_height() {
        let html = r#"<div id="b" style="display:flex;align-items:center;height:48px;padding:6px 20px;border:1px solid #000;box-sizing:border-box"><span>Sign up</span></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let b = find_element_by_id(&layout, "b").expect("b");
        assert_eq!(b.paint_rect().height, 48.0, "the box is the stated 48");
    }

    /// The same for a grid container.
    #[test]
    fn test_a_grid_container_keeps_its_stated_height() {
        let html = r#"<div id="g" style="display:grid;height:120px;grid-template-columns:1fr"><div style="height:10px"></div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        assert_eq!(
            find_element_by_id(&layout, "g").expect("g").paint_rect().height,
            120.0
        );
    }

    /// A single flex line in a container with a definite height fills that
    /// height, so `align-items: center` centres against the container rather
    /// than against the tallest item — which by definition fills the line.
    #[test]
    fn test_a_single_flex_line_fills_a_definite_height() {
        let html = r#"<div id="row" style="display:flex;align-items:center;width:300px;height:100px">
            <div id="a" style="width:80px;height:30px"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let row = find_element_by_id(&layout, "row").expect("row");
        let a = find_element_by_id(&layout, "a").expect("a");
        assert_eq!(
            a.dimensions.y - row.dimensions.y,
            35.0,
            "(100 - 30) / 2 from the container's top"
        );
    }

    /// A form control holds a line of text of its own whatever its `display`
    /// says. Primer sets `display: flex` on its text inputs, and reading the
    /// `display` instead of the element collapsed github's email field to its
    /// own top padding.
    #[test]
    fn test_a_flex_input_still_holds_a_line() {
        let html = r#"<input id="i" style="display:flex;font-size:16px;line-height:1.5;padding:18px 12px 0 18px;box-sizing:content-box">"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let i = find_element_by_id(&layout, "i").expect("input");
        assert!(
            i.paint_rect().height >= 40.0,
            "one 24px line under 18px of top padding, got {}",
            i.paint_rect().height
        );
    }

    // ── Grid item alignment ───────────────────────────────────────────────────

    /// A grid item fills its cell only under `stretch`. `align-items: center`
    /// is how a page puts a portrait beside a taller column of text without
    /// letting the portrait grow to match it.
    #[test]
    fn test_grid_align_items_center_does_not_stretch_the_item() {
        let html = r#"<div id="g" style="display:grid;align-items:center;grid-template-columns:100px 100px;width:200px">
            <div id="short" style="height:40px"></div>
            <div id="tall" style="height:200px"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let short = find_element_by_id(&layout, "short").expect("short");
        let tall = find_element_by_id(&layout, "tall").expect("tall");
        assert_eq!(short.dimensions.height, 40.0, "it keeps its own height");
        assert_eq!(
            short.dimensions.y - tall.dimensions.y,
            80.0,
            "and sits centred in the 200px row: {} vs {}",
            short.dimensions.y,
            tall.dimensions.y
        );
    }

    /// The default is still `stretch`.
    #[test]
    fn test_grid_stretches_its_items_by_default() {
        let html = r#"<div id="g" style="display:grid;grid-template-columns:100px 100px;width:200px">
            <div id="short"></div>
            <div id="tall" style="height:200px"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let short = find_element_by_id(&layout, "short").expect("short");
        assert_eq!(short.dimensions.height, 200.0, "it fills the row");
        assert_eq!(short.dimensions.y, find_element_by_id(&layout, "tall").expect("tall").dimensions.y);
    }

    /// `align-self` on the item overrides the container's `align-items`.
    #[test]
    fn test_grid_align_self_overrides_the_container() {
        let html = r#"<div id="g" style="display:grid;align-items:center;grid-template-columns:100px 100px;width:200px">
            <div id="pinned" style="height:40px;align-self:end"></div>
            <div id="tall" style="height:200px"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let pinned = find_element_by_id(&layout, "pinned").expect("pinned");
        let tall = find_element_by_id(&layout, "tall").expect("tall");
        assert_eq!(
            pinned.dimensions.y - tall.dimensions.y,
            160.0,
            "`end` puts it at the bottom of the row"
        );
    }

    // ── Line breaking ─────────────────────────────────────────────────────────

    /// The space that goes before a word is part of what has to fit. Testing
    /// only the word and adding the space afterwards let a line end up a space
    /// wider than its container, so a last word that misses the line by a hair
    /// stayed on it — one line's difference in the height of every paragraph it
    /// happens to.
    #[test]
    fn test_the_space_before_a_word_counts_toward_the_fit() {
        // Two words whose widths plus one space just exceed the box.
        let (probe, _, _) = layout_from_html(
            r#"<div id="p" style="font:16px sans-serif;white-space:nowrap;position:absolute">aaaa bbbb</div>"#,
            800.0,
            600.0,
        );
        let full = find_element_by_id(&probe, "p").expect("p").dimensions.width;
        // A box one pixel narrower than the pair cannot hold both on one line.
        let html = format!(
            r#"<div id="b" style="font:16px sans-serif;width:{}px">aaaa bbbb</div>"#,
            full - 1.0
        );
        let (layout, _, _) = layout_from_html(&html, 800.0, 600.0);
        let b = find_element_by_id(&layout, "b").expect("b");
        assert!(
            b.dimensions.height > 20.0,
            "the pair should have wrapped to two lines, got height {}",
            b.dimensions.height
        );
        // And a box exactly as wide as the pair holds both.
        let html = format!(
            r#"<div id="b" style="font:16px sans-serif;width:{full}px">aaaa bbbb</div>"#
        );
        let (layout, _, _) = layout_from_html(&html, 800.0, 600.0);
        let b = find_element_by_id(&layout, "b").expect("b");
        assert!(
            b.dimensions.height < 20.0,
            "and at exactly its own width it stays on one line, got height {}",
            b.dimensions.height
        );
    }

    /// Only a run's *first* line starts where the run does. What an inline
    /// sibling to its left already used is unavailable on that line and
    /// available on every line after it; measuring the whole run against the
    /// leftover width wrapped it a line early — in a heading with a bold
    /// lead-in, and in every paragraph with an inline element in it.
    #[test]
    fn test_only_the_first_line_of_a_run_is_indented() {
        let html = r#"<div id="d" style="width:351px;font-size:20px;line-height:28px">
            <span><span>Plan with clarity.</span> Organize everything from high-level roadmaps to everyday tasks.</span>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let d = find_element_by_id(&layout, "d").expect("d");
        assert_eq!(
            d.dimensions.height, 84.0,
            "three lines of 28, not four: got {}",
            d.dimensions.height
        );
    }

    /// The run's box spans the line box, and the indent it carries is what
    /// paint starts its first line at.
    #[test]
    fn test_a_run_after_an_inline_spans_the_line_box() {
        let html = r#"<div id="d" style="width:400px"><span id="p" style="display:inline-block;width:120px">p</span>tail</div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let tail = find_text_box_containing(&layout, "tail").expect("tail");
        assert_eq!(tail.dimensions.x, 0.0, "the box starts at the line's edge");
        assert_eq!(tail.text_leading, 120.0, "and its first line 120 in");
    }

    /// A shrink-to-fit box is sized to its own content, so that content must
    /// not then wrap inside it. The two widths are summed in different orders
    /// and land a few ULPs apart, which an exact `>` turned into a wrapped
    /// button label.
    #[test]
    fn test_a_shrink_to_fit_label_does_not_wrap_in_its_own_box() {
        let html = r#"<div style="display:flex"><a id="btn" style="display:inline-flex;padding:10px 18px;border:1px solid #000;font:16px sans-serif">Try GitHub Copilot</a></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let btn = find_element_by_id(&layout, "btn").expect("btn");
        assert!(
            btn.dimensions.height < 45.0,
            "the label should stay on one line in the box built for it, got height {}",
            btn.dimensions.height
        );
    }

    /// `pre-line` keeps the source's own newlines as line breaks while still
    /// collapsing runs of spaces.
    #[test]
    fn test_pre_line_keeps_the_sources_newlines() {
        let flowed = "<div id=\"d\" style=\"font-size:16px;line-height:20px;width:600px\">one\ntwo\nthree</div>";
        let kept = "<div id=\"d\" style=\"font-size:16px;line-height:20px;width:600px;white-space:pre-line\">one\ntwo\nthree</div>";
        let (a, _, _) = layout_from_html(flowed, 800.0, 600.0);
        let (b, _, _) = layout_from_html(kept, 800.0, 600.0);
        let ha = find_element_by_id(&a, "d").expect("d").dimensions.height;
        let hb = find_element_by_id(&b, "d").expect("d").dimensions.height;
        assert_eq!(ha, 20.0, "without pre-line the three words share a line");
        assert_eq!(hb, 60.0, "with it each is its own line, got {hb}");
    }

    /// Kerning is what a shaper applies and this has to match it, or a line
    /// measures wider than the browser draws it and wraps a word early.
    #[test]
    fn test_kerning_narrows_a_run_that_has_kern_pairs() {
        use crate::font::{FontStyle, GenericFamily};
        let fonts = crate::font::fonts();
        let style = FontStyle { family: GenericFamily::SystemUi, ..FontStyle::regular() };
        let kerned = fonts.measure("AVATAR Yo To We", 16.0, style, 0.0);
        let unkerned: f32 = "AVATAR Yo To We"
            .chars()
            .map(|c| fonts.advance(c, 16.0, style))
            .sum();
        assert!(
            kerned < unkerned,
            "the kern pairs should pull the run in: {kerned} vs {unkerned}"
        );
    }

    // ── Font stack resolution ─────────────────────────────────────────────────

    /// Font matching walks the stack in order and takes the first family it can
    /// satisfy. Reading it as a set — asking only whether `serif` or
    /// `monospace` appears anywhere — sent every modern stack to the sans
    /// default, and `system-ui` resolves to a face wide enough that the whole
    /// page then laid out at the wrong measure.
    #[test]
    fn test_font_stack_takes_the_first_family_it_can_satisfy() {
        use crate::font::GenericFamily;
        assert_eq!(generic_family_for("sans-serif"), GenericFamily::Sans);
        assert_eq!(generic_family_for("serif"), GenericFamily::Serif);
        assert_eq!(generic_family_for("monospace"), GenericFamily::Mono);
        assert_eq!(generic_family_for("system-ui"), GenericFamily::SystemUi);
        // yunseong.dev's stack: `system-ui` comes before `sans-serif`.
        assert_eq!(
            generic_family_for(
                "\"Pretendard Variable\", Pretendard, -apple-system, system-ui, \"Apple SD Gothic Neo\", \"Malgun Gothic\", sans-serif"
            ),
            GenericFamily::SystemUi
        );
        // github.com's: nothing before `sans-serif` can be satisfied here, so
        // `-apple-system` and `BlinkMacSystemFont` are stepped over rather than
        // treated as the system face.
        assert_eq!(
            generic_family_for(
                "\"Mona Sans\", MonaSansFallback, -apple-system, BlinkMacSystemFont, \"Segoe UI\", Helvetica, Arial, sans-serif"
            ),
            GenericFamily::Sans
        );
        // A `ui-*` generic resolves to nothing in the reference, so the walk
        // continues to the plain one.
        assert_eq!(
            generic_family_for("ui-monospace, SFMono-Regular, monospace"),
            GenericFamily::Mono
        );
    }

    /// A face named on its own, with no generic after it, is not something this
    /// renderer has — the standard font stands in, as it does in the reference.
    #[test]
    fn test_unsatisfiable_stack_falls_to_the_standard_font() {
        use crate::font::GenericFamily;
        assert_eq!(generic_family_for("\"Nope Font XYZ\""), GenericFamily::Serif);
        assert_eq!(generic_family_for("Georgia"), GenericFamily::Serif);
    }

    /// The faces are not interchangeable — that is the whole reason the stack
    /// has to resolve correctly.
    #[test]
    fn test_the_bundled_families_have_different_advances() {
        use crate::font::{FontStyle, GenericFamily};
        let width = |family| {
            crate::font::fonts().zero_advance(
                16.0,
                FontStyle { family, ..FontStyle::regular() },
            )
        };
        let sans = width(GenericFamily::Sans);
        let system = width(GenericFamily::SystemUi);
        let mono = width(GenericFamily::Mono);
        assert!(
            system > sans && mono > sans,
            "system-ui and monospace are both wider than sans: {sans} {system} {mono}"
        );
    }

    /// `ch` is a metric of the element's own face, so the same `max-width`
    /// gives a wider measure under `system-ui` than under `sans-serif`.
    #[test]
    fn test_ch_measures_in_the_elements_own_font() {
        let html = r#"<div id="a" style="font-family:system-ui;max-width:20ch">x</div>
            <div id="b" style="font-family:sans-serif;max-width:20ch">x</div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let a = find_element_by_id(&layout, "a").expect("a").dimensions.width;
        let b = find_element_by_id(&layout, "b").expect("b").dimensions.width;
        assert!(
            a > b,
            "20ch of system-ui is wider than 20ch of sans-serif: {a} vs {b}"
        );
    }

    // ── Absolute positioning ──────────────────────────────────────────────────

    /// `top` and `bottom` together with no stated height stretch the box
    /// between them. An overlay written as `position: absolute; inset: 0` is
    /// otherwise its content's height — zero, for the empty element a gradient
    /// wash is — and never paints.
    #[test]
    fn test_absolute_with_top_and_bottom_stretches() {
        let html = r#"<div id="cb" style="position:relative;width:400px;height:300px">
            <div id="fill" style="position:absolute;inset:0;background:#0a8"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let fill = find_element_by_id(&layout, "fill").expect("fill");
        assert_eq!(fill.dimensions.width, 400.0, "stretched across");
        assert_eq!(fill.dimensions.height, 300.0, "and down");
    }

    /// Insets on one axis only still stretch that axis.
    #[test]
    fn test_absolute_stretches_only_the_axis_with_both_offsets() {
        let html = r#"<div id="cb" style="position:relative;width:400px;height:300px">
            <div id="fill" style="position:absolute;top:20px;bottom:40px;left:10px;width:50px"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let fill = find_element_by_id(&layout, "fill").expect("fill");
        assert_eq!(fill.dimensions.height, 240.0, "300 - 20 - 40");
        assert_eq!(fill.dimensions.width, 50.0, "the stated width stands");
    }

    /// A stated height wins over the stretch.
    #[test]
    fn test_stated_height_beats_the_absolute_stretch() {
        let html = r#"<div id="cb" style="position:relative;width:400px;height:300px">
            <div id="fill" style="position:absolute;top:0;bottom:0;height:25px"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let fill = find_element_by_id(&layout, "fill").expect("fill");
        assert_eq!(fill.dimensions.height, 25.0);
    }

    // ── Parent / child margin collapsing ──────────────────────────────────────

    /// A first child's top margin does not vanish into its parent — it becomes
    /// the parent's own top margin, so the space lands above the parent. The
    /// old code dropped it, which pulled the top of every page up by its first
    /// paragraph's margin.
    #[test]
    fn test_first_child_top_margin_becomes_the_parents() {
        let html = r#"<div id="outer"><div id="inner" style="margin-top:30px;height:10px"></div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let outer = find_element_by_id(&layout, "outer").expect("outer");
        let inner = find_element_by_id(&layout, "inner").expect("inner");
        assert_eq!(outer.margin.top, 30.0, "the parent takes the margin on");
        assert_eq!(
            outer.dimensions.y, inner.dimensions.y,
            "and no space is left inside it: outer.y={} inner.y={}",
            outer.dimensions.y, inner.dimensions.y
        );
        assert_eq!(outer.dimensions.y, 30.0, "both start below the margin");
    }

    /// Padding on the parent stops the collapse: the margin stays interior
    /// space and the parent does not move.
    #[test]
    fn test_padding_stops_the_first_child_collapse() {
        let html = r#"<div id="outer" style="padding-top:1px"><div id="inner" style="margin-top:30px;height:10px"></div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let outer = find_element_by_id(&layout, "outer").expect("outer");
        let inner = find_element_by_id(&layout, "inner").expect("inner");
        assert_eq!(outer.margin.top, 0.0);
        assert_eq!(outer.dimensions.y, 0.0);
        assert_eq!(inner.dimensions.y, 31.0, "1px of padding then 30 of margin");
    }

    /// The same at the bottom: a last child's bottom margin becomes the
    /// parent's, so it pushes the parent's next sibling down instead of
    /// disappearing.
    #[test]
    fn test_last_child_bottom_margin_becomes_the_parents() {
        let html = r#"<div id="outer"><div style="margin-bottom:30px;height:10px"></div></div><div id="next" style="height:10px"></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let outer = find_element_by_id(&layout, "outer").expect("outer");
        let next = find_element_by_id(&layout, "next").expect("next");
        assert_eq!(outer.dimensions.height, 10.0, "no interior space is added");
        assert_eq!(outer.margin.bottom, 30.0, "the parent takes the margin on");
        assert_eq!(
            next.dimensions.y, 40.0,
            "so the next sibling starts 30 below the parent, got {}",
            next.dimensions.y
        );
    }

    /// A stated height separates the parent's bottom edge from its last child,
    /// so that margin stays inside.
    #[test]
    fn test_stated_height_stops_the_last_child_collapse() {
        let html = r#"<div id="outer" style="height:100px"><div style="margin-bottom:30px;height:10px"></div></div><div id="next" style="height:10px"></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let outer = find_element_by_id(&layout, "outer").expect("outer");
        let next = find_element_by_id(&layout, "next").expect("next");
        assert_eq!(outer.margin.bottom, 0.0);
        assert_eq!(next.dimensions.y, 100.0, "got {}", next.dimensions.y);
    }

    /// A negative margin pulls a box back over its predecessor. Collapsing two
    /// margins by taking the larger threw the negative one away, so a page that
    /// overlaps two sections on purpose got a gap where the overlap should be.
    #[test]
    fn test_a_negative_margin_pulls_the_box_back() {
        let html = r#"<div id="w" style="width:400px">
            <div id="a" style="height:40px"></div>
            <div id="b" style="height:40px;margin-top:-10%"></div>
            <div id="c" style="height:40px;margin-top:-20px"></div>
            <div id="d" style="height:40px;margin-top:10%"></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let y = |id: &str| find_element_by_id(&layout, id).expect(id).dimensions.y;
        // A percentage margin is a fraction of the containing block's *width*.
        assert_eq!(y("b"), 0.0, "-10% of 400 pulls it right back over `a`");
        assert_eq!(y("c"), 20.0, "and -20px pulls it 20 back over `b`");
        assert_eq!(y("d"), 100.0, "a positive percentage still pushes it down");
        assert_eq!(
            find_element_by_id(&layout, "w").expect("w").dimensions.height,
            140.0
        );
    }

    /// Two margins that meet collapse to the largest positive plus the most
    /// negative, so one of each cancel rather than the positive simply winning.
    #[test]
    fn test_a_positive_and_a_negative_margin_cancel() {
        assert_eq!(collapse_margins(20.0, -10.0), 10.0);
        assert_eq!(collapse_margins(0.0, -40.0), -40.0);
        assert_eq!(collapse_margins(-10.0, -30.0), -30.0);
        assert_eq!(collapse_margins(20.0, 30.0), 30.0);
    }

    /// An `<svg>` with no `viewBox` takes its intrinsic ratio from its width
    /// and height attributes, which is what `height: auto` follows. github
    /// ships `<svg width="2280" height="1200">` as a spacer, and reading no
    /// ratio from it made the block 100px too tall.
    #[test]
    fn test_svg_without_a_viewbox_takes_its_ratio_from_its_attributes() {
        let html = r#"<div style="width:800px"><svg id="s" width="2280" height="1200" style="width:100%;height:auto"></svg></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let svg = find_element_by_id(&layout, "s").expect("svg");
        assert!(
            (svg.dimensions.height - 421.05).abs() < 1.0,
            "800 wide at 2280:1200 is 421 tall, got {}",
            svg.dimensions.height
        );
    }

    // ── Flex intrinsic sizing ─────────────────────────────────────────────────

    /// A row flex container's max-content is the sum of its items plus the gaps
    /// between them, not the widest item. Taking the max sized github's signup
    /// row to whichever of "email field" and "Sign up" was wider, and the other
    /// one hung outside the white box around them.
    #[test]
    fn test_row_flex_max_content_sums_items_and_gaps() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center">
            <div id="row" style="display:flex;gap:8px">
              <div style="width:120px;height:30px"></div>
              <div style="width:120px;height:30px"></div>
            </div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let row = find_element_by_id(&layout, "row").expect("row");
        assert_eq!(
            row.dimensions.width, 248.0,
            "shrink-to-fit row should be 120 + 8 + 120, got {}",
            row.dimensions.width
        );
    }

    /// The sum has to see through items whose own `display` is inline: a flex
    /// item is blockified, so two inline spans still lay side by side and both
    /// count.
    #[test]
    fn test_row_flex_max_content_sums_block_level_items() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center">
            <div id="row" style="display:flex">
              <div style="display:block;width:100px;height:30px"></div>
              <div style="display:block;width:60px;height:30px"></div>
            </div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let row = find_element_by_id(&layout, "row").expect("row");
        assert_eq!(
            row.dimensions.width, 160.0,
            "two block-level flex items sit side by side, got {}",
            row.dimensions.width
        );
    }

    /// A column flex container stacks its items, so its max-content is the
    /// widest one — never their sum.
    #[test]
    fn test_column_flex_max_content_is_the_widest_item() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center">
            <div id="inner" style="display:flex;flex-direction:column;gap:8px">
              <div style="width:120px;height:30px"></div>
              <div style="width:60px;height:30px"></div>
            </div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let inner = find_element_by_id(&layout, "inner").expect("inner");
        assert_eq!(
            inner.dimensions.width, 120.0,
            "a column's shrink-to-fit width is its widest item, got {}",
            inner.dimensions.width
        );
    }

    /// The newlines and indentation between two flex items are an anonymous
    /// item that contains only white space, which is not rendered. Counting
    /// them made a two-item row four items wide and charged it the gap twice
    /// more than it should have.
    #[test]
    fn test_whitespace_between_flex_items_is_not_an_item() {
        let packed = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center"><div id="row" style="display:flex;gap:16px"><div style="width:100px;height:30px"></div><div style="width:100px;height:30px"></div></div></div>"#;
        let spaced = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center">
            <div id="row" style="display:flex;gap:16px">
              <div style="width:100px;height:30px"></div>
              <div style="width:100px;height:30px"></div>
            </div>
          </div>"#;
        let (a, _, _) = layout_from_html(packed, 800.0, 600.0);
        let (b, _, _) = layout_from_html(spaced, 800.0, 600.0);
        let wa = find_element_by_id(&a, "row").expect("row").dimensions.width;
        let wb = find_element_by_id(&b, "row").expect("row").dimensions.width;
        assert_eq!(wa, 216.0, "100 + 16 + 100, got {wa}");
        assert_eq!(
            wa, wb,
            "indenting the markup must not change the layout: {wa} vs {wb}"
        );
    }

    /// `white-space: pre` does not save it: browsers drop the space between two
    /// flex items whatever the property says, so the first item still starts at
    /// the container's content edge.
    #[test]
    fn test_preserved_whitespace_is_still_not_a_flex_item() {
        let html = r#"<div id="row" style="display:flex;white-space:pre;width:400px"> <b>x</b></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let row = find_element_by_id(&layout, "row").expect("row");
        assert_eq!(
            row.children.len(),
            1,
            "the leading space is not an item, got {} children",
            row.children.len()
        );
        assert_eq!(
            row.children[0].dimensions.x, row.dimensions.x,
            "the first real item starts at the content edge"
        );
    }

    /// An out-of-flow child is sized against its containing block, not against
    /// its parent's content, so it adds nothing to the parent's intrinsic
    /// width. github floats its "Enter your email" label over the field with
    /// `position: absolute`; counting it made the field 105px wider than the
    /// field.
    #[test]
    fn test_absolutely_positioned_child_adds_no_intrinsic_width() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center">
            <div id="row" style="display:flex;position:relative">
              <label style="position:absolute;top:0;left:0;width:200px;height:20px"></label>
              <span style="display:block;width:120px;height:30px"></span>
            </div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let row = find_element_by_id(&layout, "row").expect("row");
        assert_eq!(
            row.dimensions.width, 120.0,
            "only the in-flow span sizes the row, got {}",
            row.dimensions.width
        );
    }

    /// The same in ordinary block flow.
    #[test]
    fn test_absolutely_positioned_child_adds_no_intrinsic_width_in_flow() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center">
            <div id="blk" style="position:relative">
              <i style="position:absolute;display:block;width:300px;height:20px"></i>
              <i style="display:block;width:90px;height:30px"></i>
            </div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let blk = find_element_by_id(&layout, "blk").expect("blk");
        assert_eq!(
            blk.dimensions.width, 90.0,
            "the absolute child does not widen the block, got {}",
            blk.dimensions.width
        );
    }

    // ── `box-sizing: border-box` ──────────────────────────────────────────────

    /// Under `border-box` a stated width is the *outer* width, so an intrinsic
    /// measurement must not add the padding on top of it again.
    #[test]
    fn test_border_box_stated_width_is_the_outer_width_in_max_content() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center">
            <div id="row" style="display:flex"><i style="box-sizing:border-box;display:block;width:260px;padding:0 30px;height:30px"></i></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let row = find_element_by_id(&layout, "row").expect("row");
        assert_eq!(
            row.dimensions.width, 260.0,
            "border-box: the padding is inside the 260, got {}",
            row.dimensions.width
        );
    }

    /// Under the default `content-box` the padding sits outside the stated
    /// width and does count.
    #[test]
    fn test_content_box_stated_width_adds_padding_in_max_content() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center">
            <div id="row" style="display:flex"><i style="display:block;width:260px;padding:0 30px;height:30px"></i></div>
          </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let row = find_element_by_id(&layout, "row").expect("row");
        assert_eq!(
            row.dimensions.width, 320.0,
            "content-box: 260 of content plus 60 of padding, got {}",
            row.dimensions.width
        );
    }

    /// The height axis follows the same rule: under `border-box` a stated
    /// height already covers the padding and border, so a 48px button paints
    /// 48px tall rather than 62.
    #[test]
    fn test_border_box_stated_height_covers_padding_and_border() {
        let html = r#"<div id="b" style="box-sizing:border-box;height:48px;padding:6px 20px;border:1px solid #000">Sign up</div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let b = find_element_by_id(&layout, "b").expect("b");
        // A stated height leaves `dimensions.height` as the content box; paint
        // adds the padding and border back. 48 - 12 - 2 = 34.
        assert_eq!(
            b.dimensions.height, 34.0,
            "border-box height should inset padding and border, got {}",
            b.dimensions.height
        );
        assert_eq!(
            b.paint_rect().height,
            48.0,
            "the painted box is the stated 48, got {}",
            b.paint_rect().height
        );
    }

    /// `content-box` is unchanged: the stated height is the content height and
    /// the padding is drawn outside it.
    #[test]
    fn test_content_box_stated_height_excludes_padding() {
        let html = r#"<div id="b" style="height:48px;padding:6px 20px;border:1px solid #000">Sign up</div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let b = find_element_by_id(&layout, "b").expect("b");
        assert_eq!(b.dimensions.height, 48.0);
        assert_eq!(b.paint_rect().height, 62.0);
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

        // The run's box is the line box it lives in: its first line starts
        // after the prefix, but every line after that starts at the container's
        // own edge and has the full width to fill.
        assert!(
            (text.text_leading - 280.0).abs() < 6.0,
            "the first line starts after the prefix and the space between them, \
             got an indent of {}",
            text.text_leading
        );
        assert_eq!(
            text.dimensions.x, 0.0,
            "and the box itself spans the line box, got x={}",
            text.dimensions.x
        );
        assert!(text.dimensions.height > 24.0,
            "inline text should wrap onto multiple lines when the prefix consumes horizontal space, got height={}",
            text.dimensions.height);
        assert!(text.dimensions.height < 60.0,
            "but only two of them: the second line has all 800px, got height={}",
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

        // Two <br>s move the cursor down two line boxes: one ends the first line,
        // the second leaves a blank one. Expressed in terms of the font's own
        // line height rather than a fixed number, so the assertion still means
        // "two lines" if the bundled face changes.
        let line = crate::font::fonts().normal_line_height(16.0, crate::font::FontStyle::regular());
        assert!(
            second.dimensions.y >= first.dimensions.y + line * 2.0 - 1.0,
            "consecutive <br> should create a blank line of vertical space: \
             first.y={}, second.y={}, line height={line}",
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

    /// An inline-block whose only child is out of flow has no content to size
    /// itself from, so it is zero wide and the text after it starts at the
    /// container's edge — what every browser does. The absolutely positioned
    /// child still lays out and paints; it just sizes against its containing
    /// block rather than against this box.
    #[test]
    fn test_inline_box_with_only_positioned_children_is_zero_wide() {
        let html = r#"<div style="width: 160px;"><a id="login-link" style="display: inline-block;"><span style="position: absolute;">로그인</span></a><span id="after">after</span></div>"#;
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

        assert_eq!(
            link.dimensions.width, 0.0,
            "an out-of-flow child contributes no width to its parent, got {}",
            link.dimensions.width
        );
        assert!(
            after.dimensions.x >= link.dimensions.x + link.dimensions.width - 1.0,
            "following inline content still flows after it: link.right={}, after.x={}",
            link.dimensions.x + link.dimensions.width,
            after.dimensions.x
        );
        assert_eq!(
            after.dimensions.x, 0.0,
            "and the text after it starts at the container edge, got {}",
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
        // `.gb_Td` has no stated width, so `dimensions.width` is its border box
        // already: `min-width: 85px` under the default `content-box` gives 85 of
        // content plus its 24px of padding.
        assert!(
            login.dimensions.x + login.dimensions.width <= 800.0,
            "login button should stay inside the viewport, got right edge {}",
            login.dimensions.x + login.dimensions.width
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

        // The width is auto, so `dimensions.width` is already the border box —
        // see the note in `perform_layout`. Under `border-box` sizing a
        // `min-width` is a border-box bound, so the box is exactly 85 wide.
        let border_w = login.dimensions.width;
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

    /// A source that never arrived and an empty `alt` is not rendered at all,
    /// and a stated height does not hold the row open on its own: with the width
    /// auto the box has no area and the whole element collapses. github's
    /// customer logos are SVGs written exactly that way, and the 100px
    /// placeholder drew a hairline frame across a band the reference leaves
    /// blank.
    #[test]
    fn test_a_broken_image_with_only_a_stated_height_collapses_entirely() {
        let html = r#"<img id="a" src="x.png" alt="" style="height: 42px">"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let img = find_element_by_id(&layout, "a").expect("img");
        assert_eq!(outer_width(img), 0.0, "nothing to show is no width");
        assert_eq!(outer_height(img), 0.0, "and a height on its own is no box");
    }

    /// Only an ordinary inline `<img>` collapses. A page that gave the image a
    /// display of its own still gets a box, and the stated height holds — its
    /// width is what an empty box shrinks to. github's customer logos are
    /// `inline-block` SVGs 42px tall, and collapsing those took 62px out of the
    /// page.
    #[test]
    fn test_a_broken_inline_block_image_keeps_its_stated_height() {
        let html = r#"<img id="a" src="x.png" alt="" style="display: inline-block; height: 42px">"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let img = find_element_by_id(&layout, "a").expect("img");
        assert_eq!(outer_width(img), 0.0, "an empty box shrinks to nothing wide");
        assert!(
            (outer_height(img) - 42.0).abs() < 0.5,
            "but the stated height holds, got {}",
            outer_height(img)
        );
    }

    /// A block-level one fills its line the way any block does.
    #[test]
    fn test_a_broken_block_image_fills_its_line() {
        let html = r#"<div style="width: 300px"><img id="a" src="x.png" alt="" style="display: block; height: 42px"></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let img = find_element_by_id(&layout, "a").expect("img");
        assert!(
            (outer_width(img) - 300.0).abs() < 0.5,
            "a block fills its line, got {}",
            outer_width(img)
        );
        assert!(
            (outer_height(img) - 42.0).abs() < 0.5,
            "and keeps its stated height, got {}",
            outer_height(img)
        );
    }

    /// A width the page stated is used as stated: only `auto` collapses.
    #[test]
    fn test_a_broken_image_keeps_a_width_the_page_stated() {
        let html = r#"<img id="a" src="x.png" alt="" style="width: 100px; height: 42px">"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let img = find_element_by_id(&layout, "a").expect("img");
        assert!(
            (outer_width(img) - 100.0).abs() < 0.5,
            "a stated width is kept, got {}",
            outer_width(img)
        );
        assert!(
            (outer_height(img) - 42.0).abs() < 0.5,
            "as is the stated height, got {}",
            outer_height(img)
        );
    }

    // ── Line breaking: `text-wrap-style` ─────────────────────────────────

    /// Word advances from the run's cumulative prefix widths, which is how the
    /// figures below were measured out of Chromium.
    fn words_from_prefixes(prefixes: &[f32], space_w: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let mut prev = 0.0;
        for (i, p) in prefixes.iter().enumerate() {
            out.push(p - prev - if i > 0 { space_w } else { 0.0 });
            prev = *p;
        }
        out
    }

    /// The widths of the lines a break produces.
    fn line_widths(widths: &[f32], space_w: f32, starts: &[usize]) -> Vec<f32> {
        let mut out = Vec::new();
        for (i, start) in starts.iter().enumerate() {
            let end = starts.get(i + 1).copied().unwrap_or(widths.len());
            out.push(super::line_width(widths, space_w, *start, end));
        }
        out
    }

    /// github's customer-story heading, measured out of Chromium at 20px in a
    /// 552px column. Greedy leaves "onboarding" alone on the last line; `pretty`
    /// pulls "automates" down to keep it company.
    #[test]
    fn test_pretty_pulls_a_word_down_to_a_lonely_last_line() {
        let space_w = 5.56;
        let widths = words_from_prefixes(&[140.0, 259.0, 325.0, 374.0, 412.0, 511.0, 617.0], space_w);
        let greedy = break_lines(&widths, space_w, 0.0, 552.0, WrapStyle::Auto);
        assert_eq!(greedy, vec![0, 6], "greedy fills the first line to 511");

        let pretty = break_lines(&widths, space_w, 0.0, 552.0, WrapStyle::Pretty);
        assert_eq!(pretty, vec![0, 5], "pretty breaks a word earlier");
        let lines = line_widths(&widths, space_w, &pretty);
        assert!(
            (lines[0] - 412.0).abs() < 1.0 && (lines[1] - 199.0).abs() < 1.5,
            "and lands on Chromium's 412 / 199, got {lines:?}"
        );
    }

    /// A last line that already holds two words is left alone: `pretty` is about
    /// the orphan, not about evening the lines out.
    #[test]
    fn test_pretty_leaves_a_last_line_that_is_not_an_orphan() {
        let space_w = 4.45;
        // "Write, test, and fix code quickly with GitHub Copilot, from simple
        //  boilerplate to complex features." at 16px in 602px.
        let widths = words_from_prefixes(
            &[41.0, 76.0, 107.0, 128.0, 167.0, 220.0, 253.0, 307.0, 366.0, 403.0, 453.0, 531.0, 549.0, 613.0, 680.0],
            space_w,
        );
        let greedy = break_lines(&widths, space_w, 0.0, 602.0, WrapStyle::Auto);
        let pretty = break_lines(&widths, space_w, 0.0, 602.0, WrapStyle::Pretty);
        assert_eq!(greedy, pretty, "two words on the last line is not an orphan");
    }

    /// github's security heading at 40px in 640px: greedy runs the first line to
    /// 594 and leaves 327 under it, and `balance` evens the two out.
    #[test]
    fn test_balance_evens_the_lines_out() {
        let space_w = 11.1;
        let widths = words_from_prefixes(&[122.0, 325.0, 474.0, 594.0, 705.0, 836.0, 932.0], space_w);
        let greedy = break_lines(&widths, space_w, 0.0, 640.0, WrapStyle::Auto);
        assert_eq!(greedy, vec![0, 4]);

        let balanced = break_lines(&widths, space_w, 0.0, 640.0, WrapStyle::Balance);
        assert_eq!(balanced, vec![0, 3], "the break moves one word earlier");
        let lines = line_widths(&widths, space_w, &balanced);
        assert!(
            (lines[0] - 474.0).abs() < 1.0 && (lines[1] - 447.0).abs() < 1.5,
            "onto Chromium's 474 / 447, got {lines:?}"
        );
    }

    /// Balancing never costs a line — that is the whole constraint on it.
    #[test]
    fn test_balance_keeps_the_line_count() {
        let space_w = 11.1;
        let widths = words_from_prefixes(&[124.0, 249.0, 331.0, 451.0, 496.0, 571.0, 654.0, 696.0, 771.0, 931.0], space_w);
        let greedy = break_lines(&widths, space_w, 0.0, 640.0, WrapStyle::Auto);
        let balanced = break_lines(&widths, space_w, 0.0, 640.0, WrapStyle::Balance);
        assert_eq!(greedy.len(), balanced.len(), "the same two lines");
        assert_eq!(balanced, vec![0, 4]);
        let lines = line_widths(&widths, space_w, &balanced);
        assert!(
            (lines[0] - 451.0).abs() < 1.0 && (lines[1] - 468.0).abs() < 1.5,
            "Chromium's 451 / 468, got {lines:?}"
        );
    }

    /// A run that fits on one line is not something either style touches.
    #[test]
    fn test_a_single_line_run_is_left_alone_by_every_style() {
        let widths = vec![40.0, 30.0, 50.0];
        for style in [WrapStyle::Auto, WrapStyle::Pretty, WrapStyle::Balance] {
            assert_eq!(
                break_lines(&widths, 4.0, 0.0, 800.0, style),
                vec![0],
                "{style:?} must leave a run that fits alone"
            );
        }
    }

    /// The orphan stands when pulling the word down would overflow the line.
    #[test]
    fn test_pretty_gives_up_when_the_pulled_word_will_not_fit() {
        let space_w = 5.0;
        // The orphan is wide enough that the word above cannot join it.
        let widths = vec![90.0, 90.0, 150.0];
        let greedy = break_lines(&widths, space_w, 0.0, 190.0, WrapStyle::Auto);
        assert_eq!(greedy, vec![0, 2], "two fit, the third wraps");
        assert_eq!(
            break_lines(&widths, space_w, 0.0, 190.0, WrapStyle::Pretty),
            greedy,
            "the pair below would be 245 wide, so the orphan stands"
        );
    }

    #[test]
    fn test_image_no_dimensions_gets_default() {
        // Nothing stated, nothing decoded and nothing to say in its place: the
        // image reserves no room at all, which is what a browser gives a source
        // it has not got.
        let html = r#"<img src="x.png">"#;
        let dom = dom::parse_html(html);
        let ss = css::parse_css("");
        let style =
            style::build_style_tree(&dom.document, &ss, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree must be built");
        let img = find_image_box(&layout).expect("img node must be found");
        assert_eq!(outer_width(img), 0.0, "an image with nothing to show is not there");
        assert_eq!(outer_height(img), 0.0);

        // With alt text it is that text: the box is as tall as the text wrapped
        // into the width available to it.
        let html = r#"<div style="width:200px"><img id="a" style="display:block" src="x.png" alt="A long alternative description that has to wrap across several lines to fit"></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let img = find_element_by_id(&layout, "a").expect("img");
        assert!(
            (outer_width(img) - 200.0).abs() < 0.5,
            "a block-level one fills its line, got {}",
            outer_width(img)
        );
        assert!(
            outer_height(img) > 40.0,
            "and is as tall as its alt text wrapped, got {}",
            outer_height(img)
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

    /// A single-line flex row is as tall as its line — `gap` sits *between*
    /// lines, so a lone line gets no gap at all. Keeping the trailing gap made
    /// every flex row one gap too tall, and the error accumulated down a page.
    #[test]
    fn test_flex_single_line_height_excludes_gap() {
        let html = r#"<div id="f" style="display:flex;gap:20px;width:400px;">
            <div id="a" style="width:60px;height:30px;">a</div>
            <div id="b" style="width:60px;height:30px;">b</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let f = find_element_by_id(&layout, "f").expect("flex container");
        assert!(
            (f.dimensions.height - 30.0).abs() < 2.0,
            "single-line flex row should be as tall as its line (30px), got {}",
            f.dimensions.height
        );
    }

    /// Two wrapped lines take one gap between them, not two.
    #[test]
    fn test_flex_wrapped_lines_take_one_gap_between_them() {
        let html = r#"<div id="f" style="display:flex;flex-wrap:wrap;gap:20px;width:150px;">
            <div id="a" style="width:100px;height:30px;">a</div>
            <div id="b" style="width:100px;height:30px;">b</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let f = find_element_by_id(&layout, "f").expect("flex container");
        assert!(
            (f.dimensions.height - 80.0).abs() < 2.0,
            "two 30px lines plus one 20px gap should be 80px tall, got {}",
            f.dimensions.height
        );
    }

    /// A paragraph's UA margin is `1em`, so it tracks the page's font size.
    /// Pinning it to 16px made a page that sets a larger body size come out
    /// short, and the error compounded down a column of prose.
    #[test]
    fn test_paragraph_ua_margin_scales_with_font_size() {
        let html = r#"<div style="font-size:20px"><p id="a">one</p><p id="b">two</p></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("p a");
        let b = find_element_by_id(&layout, "b").expect("p b");

        // Adjacent margins collapse, so the gap between the boxes is one 20px margin.
        let gap = b.dimensions.y - (a.dimensions.y + a.dimensions.height);
        assert!(
            (gap - 20.0).abs() < 2.0,
            "1em margin at 20px font should leave a 20px gap, got {}",
            gap
        );
    }

    /// `h1` is `2em` of its parent, not a fixed 32px.
    #[test]
    fn test_heading_ua_font_size_is_relative_to_parent() {
        let html = r#"<div style="font-size:10px"><h1 id="h">Title</h1></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let h = find_element_by_id(&layout, "h").expect("h1");
        // 2em of 10px = 20px text; its 0.67em margins resolve against that.
        assert!(
            (h.dimensions.height - 20.0).abs() < 6.0,
            "h1 in a 10px context should be about one 20px line tall, got {}",
            h.dimensions.height
        );
        assert!(
            (h.margin.top - 13.4).abs() < 2.0,
            "h1 margin should be 0.67em of its own 20px size, got {}",
            h.margin.top
        );
    }

    /// A bold run is measured with the bold face, which is wider than the
    /// regular one at the same size. Measuring bold text with the regular face
    /// made every heading and brand mark come out short, so the box behind it
    /// ended before the text did.
    #[test]
    fn test_bold_text_measures_wider_than_regular() {
        let fonts = crate::font::fonts();
        let regular = fonts.measure("Yunseong", 16.0, crate::font::FontStyle::regular(), 0.0);
        let bold = fonts.measure(
            "Yunseong",
            16.0,
            crate::font::FontStyle { bold: true, ..crate::font::FontStyle::regular() },
            0.0,
        );
        assert!(
            bold > regular + 2.0,
            "bold should be measurably wider: regular={regular}, bold={bold}"
        );
    }

    /// `letter-spacing` changes how wide a run is, so it has to reach both the
    /// measurement that decides line breaks and the box drawn behind the text.
    #[test]
    fn test_letter_spacing_widens_a_run() {
        let html = r#"<div><span id="a">dev / archive</span><span id="b" style="letter-spacing:2px">dev / archive</span></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("span a");
        let b = find_element_by_id(&layout, "b").expect("span b");
        // 13 characters, 2px after each.
        let delta = b.dimensions.width - a.dimensions.width;
        assert!(
            (delta - 26.0).abs() < 3.0,
            "2px of letter-spacing over 13 characters should add about 26px, got {delta}"
        );
    }

    /// A padded flex item's box has to include its own padding and border once
    /// the flex algorithm is done, or its background stops short of its text and
    /// the next item is drawn on top of it.
    #[test]
    fn test_flex_item_box_includes_its_padding_and_border() {
        let html = r#"<div style="display:flex">
            <span id="a">Yunseong</span>
            <span id="b" style="border-left:1px solid #000;padding-left:10px">dev</span>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let b = find_element_by_id(&layout, "b").expect("span b");
        let text = crate::font::fonts().measure("dev", 16.0, crate::font::FontStyle::regular(), 0.0);
        assert!(
            b.dimensions.width >= text + 10.0,
            "item box should cover text plus its 11px of padding and border: width={}, text={text}",
            b.dimensions.width
        );
    }

    /// A shrink-wrapped flex item is measured at its max-content width, and the
    /// block sizing path then subtracts the item's own margins from whatever it
    /// is handed. Passing max-content alone therefore made an item with a side
    /// margin that much narrower than its own content, and its children were
    /// shrunk to fit a box smaller than they needed.
    #[test]
    fn test_flex_item_with_side_margin_keeps_its_content_width() {
        let html = r#"<div style="display:flex;width:800px">
            <div id="brand" style="display:flex;margin-right:40px">
                <span id="a">Yunseong</span><span id="b">dev</span>
            </div>
            <div id="rest" style="width:56px">x</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let brand = find_element_by_id(&layout, "brand").expect("brand");
        let a = find_element_by_id(&layout, "a").expect("span a");
        let text = crate::font::fonts().measure(
            "Yunseong",
            16.0,
            crate::font::FontStyle::regular(),
            0.0,
        );
        assert!(
            a.dimensions.width >= text - 1.0,
            "the item's own text must not be squeezed: width={}, text={text}",
            a.dimensions.width
        );
        assert!(
            brand.dimensions.width >= a.dimensions.width,
            "the brand must be at least as wide as its first child: {} vs {}",
            brand.dimensions.width, a.dimensions.width
        );
    }

    /// In a column flex container the cross axis is horizontal, so
    /// `align-items: center` centres each item across the container's width.
    /// Measuring items at the full width regardless made every one of them as
    /// wide as its container, which left `center` nothing to centre.
    #[test]
    fn test_column_flex_align_items_center_centres_items() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;align-items:center;width:800px">
            <h2 id="h">ABSOLUTE hero heading</h2>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let h = find_element_by_id(&layout, "h").expect("h2");
        assert!(
            h.dimensions.width < 700.0,
            "the item shrink-wraps rather than filling the column: width={}",
            h.dimensions.width
        );
        let centre = h.dimensions.x + h.dimensions.width / 2.0;
        assert!(
            (centre - 400.0).abs() < 4.0,
            "the item should sit at the middle of the 800px column, centre={centre}"
        );
    }

    /// `align-items: stretch` — the default — still fills the column.
    #[test]
    fn test_column_flex_stretch_still_fills_the_container() {
        let html = r#"<div id="col" style="display:flex;flex-direction:column;width:800px">
            <div id="a">stretched</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("child");
        assert!(
            a.dimensions.width > 700.0,
            "a stretched item fills the column: width={}",
            a.dimensions.width
        );
    }

    /// `inline-flex` lays its children out like `flex` but sizes the box like
    /// an inline-block. Falling through to the tag default left every button
    /// built that way an ordinary inline box, so a "Sign in" pill was drawn the
    /// full width of its header.
    #[test]
    fn test_inline_flex_shrink_wraps_but_still_lays_out_as_flex() {
        let html = r#"<div style="width:800px">
            <a id="btn" style="display:inline-flex;gap:8px"><span id="a">Sign</span><span id="b">in</span></a>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let btn = find_element_by_id(&layout, "btn").expect("button");
        let a = find_element_by_id(&layout, "a").expect("span a");
        let b = find_element_by_id(&layout, "b").expect("span b");

        assert!(
            btn.dimensions.width < 200.0,
            "an inline-flex box shrink-wraps rather than filling its container: width={}",
            btn.dimensions.width
        );
        assert!(
            (a.dimensions.y - b.dimensions.y).abs() < 2.0,
            "its children still lay out as flex items on one line: a.y={}, b.y={}",
            a.dimensions.y, b.dimensions.y
        );
        assert!(
            b.dimensions.x >= a.dimensions.x + a.dimensions.width + 6.0,
            "and the gap between them applies: a ends at {}, b starts at {}",
            a.dimensions.x + a.dimensions.width, b.dimensions.x
        );
    }

    /// A shrink-to-fit box is sized around its content, padding and border
    /// included. Taking the padding off again under `box-sizing: border-box`
    /// made a button exactly its own padding too narrow, so its label was drawn
    /// past the end of its background.
    #[test]
    fn test_shrink_to_fit_button_covers_its_own_label() {
        let html = r#"<div style="width:800px">
            <button id="b" style="box-sizing:border-box;padding:10px 18px;font-size:16px">Sign up for GitHub</button>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let b = find_element_by_id(&layout, "b").expect("button");
        let label = crate::font::fonts().measure(
            "Sign up for GitHub",
            16.0,
            crate::font::FontStyle::regular(),
            0.0,
        );
        assert!(
            b.dimensions.width >= label + 36.0 - 1.0,
            "the box must cover the label plus its 36px of padding: width={}, label={label}",
            b.dimensions.width
        );
    }

    /// A line box reserves an inline-level child's outer size. For the common
    /// auto-sized box `dimensions` already covers padding and border, so adding
    /// them again counted the padding twice and made the line — and the block
    /// around it — that much too tall.
    #[test]
    fn test_line_box_counts_a_padded_inline_child_once() {
        let html = r#"<div id="s" style="padding:12px"><b>label</b><br><button id="b" style="padding:10px 18px;border:0">Press</button></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let s = find_element_by_id(&layout, "s").expect("block");
        let b = find_element_by_id(&layout, "b").expect("button");
        let label_line = b.dimensions.y - (s.dimensions.y + 12.0);
        let expected = 12.0 + label_line + b.dimensions.height + 12.0;
        assert!(
            (s.dimensions.height - expected).abs() < 2.0,
            "the block is its padding plus its two lines: height={}, expected {expected}",
            s.dimensions.height
        );
    }

    /// A form control does not inherit the page's `line-height`: the UA sheet
    /// gives it one of its own, which is why a button inside
    /// `body { line-height: 2 }` is not two lines tall.
    #[test]
    fn test_button_does_not_inherit_the_page_line_height() {
        let html = r#"<div style="line-height:3"><button id="b" style="padding:0;border:0;font-size:16px">Press</button></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let b = find_element_by_id(&layout, "b").expect("button");
        assert!(
            b.dimensions.height < 32.0,
            "the button keeps its own line height, not 3x the font size: height={}",
            b.dimensions.height
        );
    }

    /// A grid or flex container's height is measured from its own top edge.
    /// Measuring from the content edge counted the bottom padding but silently
    /// dropped the top one, so every padded container came out exactly its
    /// `padding-top` short — and a page built from padded entries ended
    /// hundreds of pixels short.
    #[test]
    fn test_container_height_includes_both_paddings() {
        for style in [
            "display:grid;grid-template-columns:38px 1fr;padding:20px 0",
            "display:flex;padding:20px 0",
            "display:flex;flex-direction:column;padding:20px 0",
        ] {
            let html = format!(
                r#"<div id="f" style="{style}"><div id="a" style="height:30px">a</div></div>"#
            );
            let (layout, _, _) = layout_from_html(&html, 800.0, 600.0);
            let f = find_element_by_id(&layout, "f").expect("container");
            let a = find_element_by_id(&layout, "a").expect("item");
            assert!(
                (a.dimensions.y - 20.0).abs() < 1.0,
                "[{style}] the item sits below the top padding, got y={}",
                a.dimensions.y
            );
            assert!(
                (f.dimensions.height - 70.0).abs() < 1.5,
                "[{style}] 20 + 30 + 20 = 70, got {}",
                f.dimensions.height
            );
        }
    }

    /// A flex line reserves an item's outer size. For the common auto-sized item
    /// `dimensions` already covers padding and border, so adding them again made
    /// a row holding a padded pill that much taller than the browser draws it.
    #[test]
    fn test_flex_line_counts_a_padded_item_once() {
        let html = r#"<div id="bar" style="display:flex;justify-content:flex-end;padding:8px">
            <a id="pill" style="display:inline-flex;padding:8px 16px;border:1px solid #000">Sign in</a>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let bar = find_element_by_id(&layout, "bar").expect("bar");
        let pill = find_element_by_id(&layout, "pill").expect("pill");
        assert!(
            (bar.dimensions.height - (pill.dimensions.height + 16.0)).abs() < 1.5,
            "the bar is its padding plus the pill: bar={}, pill={}",
            bar.dimensions.height, pill.dimensions.height
        );
    }

    /// CSS resolves `min-width: auto` on a row flex item to its min-content
    /// width, so shrinking never squeezes a box below the longest word it
    /// holds. Without that floor a row of tags came out with each tag broken
    /// across two lines where the browser keeps every one of them whole.
    #[test]
    fn test_flex_item_does_not_shrink_below_its_longest_word() {
        let html = r#"<div style="display:flex;width:120px">
            <div id="a">Supercalifragilistic</div>
            <div id="b">Expialidocious</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("item a");
        let word = crate::font::fonts().measure(
            "Supercalifragilistic",
            16.0,
            crate::font::FontStyle::regular(),
            0.0,
        );
        assert!(
            a.dimensions.width >= word - 1.0,
            "the item keeps its longest word: width={}, word={word}",
            a.dimensions.width
        );
    }

    /// An author opts out of that floor by stating `min-width` — which is why
    /// `min-width: 0` is such a common idiom on flex children.
    #[test]
    fn test_stated_min_width_opts_out_of_the_automatic_minimum() {
        let html = r#"<div style="display:flex;width:120px">
            <div id="a" style="min-width:0">Supercalifragilistic</div>
            <div id="b" style="min-width:0">Expialidocious</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("item a");
        assert!(
            a.dimensions.width < 120.0,
            "with min-width: 0 the item may shrink freely, got {}",
            a.dimensions.width
        );
    }

    /// A text run gets the line's whole width, not what is left of it: the run
    /// is built with its start x already advanced past the line's content and
    /// takes that off itself. Subtracting it at the call site as well left the
    /// run with half the room it had, so a short word after an inline sibling
    /// wrapped where the browser keeps it whole.
    #[test]
    fn test_text_after_an_inline_sibling_gets_the_rest_of_the_line() {
        let html = r#"<div style="width:400px"><span id="a">####</span><span id="b">Handgloves</span></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let b = find_element_by_id(&layout, "b").expect("span b");
        let one_line = crate::font::fonts().normal_line_height(16.0, crate::font::FontStyle::regular());
        assert!(
            b.dimensions.height < one_line * 1.8,
            "the second run stays on one line: height={}, line={one_line}",
            b.dimensions.height
        );
    }

    /// A line may only break between two words, so the first word of a run
    /// never wraps by itself — not even when an inter-element space precedes
    /// it. Treating that space as content put the word on a line of its own.
    #[test]
    fn test_first_word_of_a_run_never_wraps_alone() {
        let fonts = crate::font::fonts();
        let regular = crate::font::FontStyle::regular();
        let x = fonts.measure("x", 16.0, regular, 0.0);
        let word = fonts.measure("Hand", 16.0, regular, 0.0);
        let space = fonts.advance(' ', 16.0, regular);
        // Wide enough for the word after the first span, too narrow for the
        // collapsed space in front of it.
        let width = x + word + space / 2.0;
        let html = format!(r#"<div id="d" style="width:{width}px"><span>x</span> Hand</div>"#);
        let (layout, _, _) = layout_from_html(&html, 800.0, 600.0);

        let d = find_element_by_id(&layout, "d").expect("div");
        let line = fonts.normal_line_height(16.0, regular);
        assert!(
            d.dimensions.height < line * 1.8,
            "everything stays on one line: height={}, line={line}",
            d.dimensions.height
        );
    }

    /// Layout keeps one space where a text node begins or ends with
    /// whitespace, so the intrinsic measurement has to keep it too. Trimming it
    /// away made a run measure narrower than it lays out, and a date written as
    /// two spans wrapped inside a box built without the space between them.
    #[test]
    fn test_max_content_keeps_the_space_between_two_runs() {
        let html = r#"<div style="width:800px"><p id="p" style="display:inline-block"><span>2026-02</span><span> — Present</span></p></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let p = find_element_by_id(&layout, "p").expect("p");
        let fonts = crate::font::fonts();
        let regular = crate::font::FontStyle::regular();
        let text = fonts.measure("2026-02", 16.0, regular, 0.0)
            + fonts.advance(' ', 16.0, regular)
            + fonts.measure("— Present", 16.0, regular, 0.0);
        assert!(
            p.dimensions.width >= text - 1.0,
            "the box covers both runs and the space between them: width={}, text={text}",
            p.dimensions.width
        );
        let line = fonts.normal_line_height(16.0, regular);
        assert!(
            p.dimensions.height < line * 1.8,
            "so it stays on one line: height={}, line={line}",
            p.dimensions.height
        );
    }

    /// A padded block lays its children out against its *content* width. It
    /// used to hand them its own outer width, so their text ran the full width
    /// of the box instead of stopping at its padding.
    #[test]
    fn test_padded_block_narrows_the_width_its_children_get() {
        let html = r#"<div style="width:800px"><section id="s" style="padding:8px"><div id="a">x</div></section></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("child");
        assert!(
            (a.dimensions.width - 784.0).abs() < 1.0,
            "800 less the section's 16px of padding, got {}",
            a.dimensions.width
        );
        assert!(
            (a.dimensions.x - 8.0).abs() < 1.0,
            "and it starts inside the padding, got x={}",
            a.dimensions.x
        );
    }

    /// A background and a border cover the *border* box. `dimensions` is that
    /// box already when the width was auto, and the content box when it was
    /// stated, so a box with a declared width and padding used to paint its
    /// padding short.
    #[test]
    fn test_paint_rect_is_always_the_border_box() {
        let html = r#"<div style="width:800px">
            <div id="stated" style="width:100px;padding:10px">x</div>
            <div id="auto" style="padding:10px">y</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let stated = find_element_by_id(&layout, "stated").expect("stated");
        assert!(
            (stated.paint_rect().width - 120.0).abs() < 1.0,
            "100px of content plus 20px of padding, got {}",
            stated.paint_rect().width
        );
        let auto = find_element_by_id(&layout, "auto").expect("auto");
        assert!(
            (auto.paint_rect().width - 800.0).abs() < 1.0,
            "an auto width already covers the padding, got {}",
            auto.paint_rect().width
        );
    }

    /// A form control's height is content-driven: one line of the control's own
    /// font plus its padding and border. Pinning it to a fixed 24px made a
    /// padded field several pixels taller than the browser draws it, and the
    /// field is the tallest thing in its row.
    #[test]
    fn test_input_height_comes_from_its_line_and_padding() {
        let html = r#"<div style="width:800px"><input id="f" style="padding:10px 12px;border:1px solid #000"></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let f = find_element_by_id(&layout, "f").expect("input");
        let line = resolved_line_height_px(f.style_node);
        let expected = line + 20.0 + 2.0;
        assert!(
            (f.dimensions.height - expected).abs() < 1.5,
            "one line plus 20px padding and 2px border = {expected}, got {}",
            f.dimensions.height
        );
    }

    /// A control does not inherit the page's font size either, so a page that
    /// sets a large body size does not blow its fields up with it.
    #[test]
    fn test_input_keeps_its_own_font_size() {
        let html = r#"<div style="font-size:40px"><input id="f" style="padding:0;border:0"></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let f = find_element_by_id(&layout, "f").expect("input");
        assert!(
            f.dimensions.height < 25.0,
            "the field keeps its own ~13px font, got height {}",
            f.dimensions.height
        );
    }

    /// An inline `<svg>` is a replaced element and keeps its box whether or not
    /// anything can draw it. Its size comes from its `width`/`height`
    /// attributes; with neither stated the SVG spec's defaults are `100%`, and
    /// the `viewBox` supplies the proportions — which is how a logo with only a
    /// `viewBox` fills its container and takes a square of height with it.
    #[test]
    fn test_inline_svg_reserves_a_box_from_its_viewbox() {
        let html = r#"<div style="width:400px"><svg id="s" viewBox="0 0 24 24"><path d="M0 0h24v24H0z"/></svg></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let s = find_element_by_id(&layout, "s").expect("svg");
        assert!(
            (s.dimensions.width - 400.0).abs() < 1.0 && (s.dimensions.height - 400.0).abs() < 1.0,
            "a 1:1 viewBox with no stated size fills the container and squares off: {}x{}",
            s.dimensions.width, s.dimensions.height
        );
    }

    /// Stated `width` and `height` attributes win, and the contents of the SVG
    /// are not laid out as HTML.
    #[test]
    fn test_inline_svg_takes_its_stated_size_and_lays_out_no_children() {
        let html = r#"<div style="width:400px"><svg id="s" viewBox="0 0 16 16" width="32" height="32"><text>not html</text></svg></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let s = find_element_by_id(&layout, "s").expect("svg");
        assert!(
            (s.dimensions.width - 32.0).abs() < 1.0 && (s.dimensions.height - 32.0).abs() < 1.0,
            "the attributes win: {}x{}",
            s.dimensions.width, s.dimensions.height
        );
        assert!(
            s.children.is_empty(),
            "SVG contents are a different language and are not laid out as HTML"
        );
    }

    /// `<canvas>`, `<video>` and the embedded-content elements are replaced:
    /// their box comes from their `width`/`height` attributes, and the spec's
    /// default for all of them is 300x150. A page whose hero is a canvas lost
    /// that whole box, and everything below it moved up.
    #[test]
    fn test_replaced_media_reserves_its_box() {
        let html = r#"<div style="width:800px">
            <canvas id="c" width="800" height="950"></canvas>
            <video id="v"></video>
            <div id="after">after</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let c = find_element_by_id(&layout, "c").expect("canvas");
        assert!(
            (c.dimensions.width - 800.0).abs() < 1.0 && (c.dimensions.height - 950.0).abs() < 1.0,
            "the canvas takes its attributes: {}x{}",
            c.dimensions.width, c.dimensions.height
        );
        let v = find_element_by_id(&layout, "v").expect("video");
        assert!(
            (v.dimensions.width - 300.0).abs() < 1.0 && (v.dimensions.height - 150.0).abs() < 1.0,
            "a video with no stated size is 300x150: {}x{}",
            v.dimensions.width, v.dimensions.height
        );
        let after = find_element_by_id(&layout, "after").expect("after");
        assert!(
            after.dimensions.y >= 950.0,
            "content below sits past the media, got y={}",
            after.dimensions.y
        );
    }

    /// `aspect-ratio` is not only for images: a media wrapper, a card or a
    /// video container states one and lets its height follow from its width.
    /// Applying it only to replaced elements left every such box the height of
    /// its content — zero, when everything inside it is positioned — and the
    /// whole section below it moved up.
    #[test]
    fn test_aspect_ratio_sizes_an_ordinary_box() {
        for display in ["block", "flex", "grid"] {
            let html = format!(
                r#"<div style="width:800px"><div id="a" style="display:{display};width:100%;aspect-ratio:2/1"><div style="position:absolute">only positioned content</div></div><div id="after">after</div></div>"#
            );
            let (layout, _, _) = layout_from_html(&html, 800.0, 600.0);

            let a = find_element_by_id(&layout, "a").expect("box");
            assert!(
                (a.dimensions.height - 400.0).abs() < 2.0,
                "[{display}] 800 wide at 2:1 is 400 tall, got {}",
                a.dimensions.height
            );
            let after = find_element_by_id(&layout, "after").expect("after");
            assert!(
                after.dimensions.y >= 398.0,
                "[{display}] what follows sits below it, got y={}",
                after.dimensions.y
            );
        }
    }

    /// `aspect-ratio` sizes whichever box `box-sizing` names. Under
    /// `border-box` that is the padding and border included, so applying the
    /// ratio to the content box left github's hero frame — 8px of padding and a
    /// 1px border around a `1000 / 1196` box — short by its own insets.
    #[test]
    fn test_aspect_ratio_follows_box_sizing() {
        let html = r#"<div style="width:800px">
            <div id="bb" style="width:200px;padding:8px;border:1px solid #000;box-sizing:border-box;aspect-ratio:2/1"></div>
            <div id="cb" style="width:200px;padding:8px;border:1px solid #000;box-sizing:content-box;aspect-ratio:2/1"></div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        // border-box: the 200px *is* the border box, so the ratio gives a 100px
        // border box back.
        let bb = find_element_by_id(&layout, "bb").expect("bb");
        assert!(
            (bb.dimensions.height - 100.0).abs() < 0.5,
            "border-box 200 wide at 2:1 is a 100px border box, got {}",
            bb.dimensions.height
        );
        // content-box: the ratio applies to the 200px content box, and the 18px
        // of insets sit outside the resulting 100px.
        let cb = find_element_by_id(&layout, "cb").expect("cb");
        assert!(
            (cb.dimensions.height - 118.0).abs() < 0.5,
            "content-box 200 wide at 2:1 is 100 tall plus 18 of insets, got {}",
            cb.dimensions.height
        );
    }

    /// A height that `aspect-ratio` settles before the children are laid out is
    /// definite, so `height: 100%` inside resolves against it. Without that the
    /// media that fills a ratio-sized frame came out `auto` — zero — and never
    /// painted.
    #[test]
    fn test_percentage_height_resolves_against_a_ratio_sized_parent() {
        let html = r#"<div style="width:800px"><div id="frame" style="position:relative;width:200px;aspect-ratio:2/1"><div id="fill" style="width:100%;height:100%"></div></div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let fill = find_element_by_id(&layout, "fill").expect("fill");
        assert!(
            (fill.dimensions.height - 100.0).abs() < 0.5,
            "100% of a 100px ratio-sized frame is 100px, got {}",
            fill.dimensions.height
        );
    }

    /// Flexing overrides an item's stated main size, and `aspect-ratio` ties the
    /// two axes together — so the cross size has to be re-derived from the size
    /// flexing settled on, not from the one the item asked for.
    #[test]
    fn test_a_shrunk_flex_item_takes_its_height_from_its_final_width() {
        let html = r#"<div style="width:800px"><div style="display:flex;width:400px"><div id="w" style="width:120%;flex:0 1 auto;aspect-ratio:2/1"></div></div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let w = find_element_by_id(&layout, "w").expect("w");
        assert!(
            (w.dimensions.width - 400.0).abs() < 1.0,
            "480 shrinks to the 400px line, got {}",
            w.dimensions.width
        );
        assert!(
            (w.dimensions.height - 200.0).abs() < 1.0,
            "and 400 at 2:1 is 200 tall, not the 240 it asked for, got {}",
            w.dimensions.height
        );
    }

    /// The ratio runs the other way too: stretching settles the cross size, and
    /// an item with nothing of its own along the main axis takes that size from
    /// the ratio instead of collapsing to nothing.
    #[test]
    fn test_a_stretched_ratio_item_takes_its_width_from_its_height() {
        let html = r#"<div style="width:800px"><div style="display:flex;align-items:stretch;height:120px"><div id="r" style="aspect-ratio:2/1"></div></div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let r = find_element_by_id(&layout, "r").expect("r");
        assert!(
            (r.dimensions.width - 240.0).abs() < 1.0,
            "120 tall at 2:1 is 240 wide, got {}",
            r.dimensions.width
        );
    }

    /// A flex or grid item is laid out at the origin and offset into place
    /// afterwards, so everything measured in its own space moves with it — but
    /// an out-of-flow descendant placed against a containing block further up
    /// is already in page coordinates, and carrying it along moves it twice.
    /// github's hero carousel put its video 52px right of where the page has
    /// it, exactly the offset of the column it sits in.
    #[test]
    fn test_an_escaping_absolute_box_does_not_move_with_its_flex_item() {
        let html = r#"<div style="width:800px">
            <div id="cb" style="position:relative;display:flex">
                <div style="width:120px">first</div>
                <div style="width:0">
                    <div id="overlay" style="position:absolute;left:0;top:0;width:40px;height:40px"></div>
                </div>
            </div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let cb = find_element_by_id(&layout, "cb").expect("cb");
        let overlay = find_element_by_id(&layout, "overlay").expect("overlay");
        assert!(
            (overlay.dimensions.x - cb.dimensions.x).abs() < 0.5,
            "`left: 0` puts the overlay on the containing block's own edge ({}), not 120px along \
             with the item it sits in; got {}",
            cb.dimensions.x,
            overlay.dimensions.x
        );
    }

    /// `order` modifies the document order a grid places its items in, the same
    /// way it does for flex. github alternates its feature sections by giving
    /// one column `order: 2` and the other `order: 1`, and ignoring that put
    /// every screenshot on the side the text belongs on.
    #[test]
    fn test_grid_items_are_placed_in_order_modified_document_order() {
        let css = ".g { display: grid; grid-template-columns: 1fr 1fr; }                    #first { order: 2 } #second { order: 1 }";
        let html = r#"<div style="width:800px"><div class="g">
            <div id="first">text</div><div id="second">picture</div>
        </div></div>"#;
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);

        let first = find_element_by_id(&layout, "first").expect("first");
        let second = find_element_by_id(&layout, "second").expect("second");
        assert!(
            first.dimensions.x > second.dimensions.x,
            "`order: 2` puts the first child in the second column: {} vs {}",
            first.dimensions.x,
            second.dimensions.x
        );
    }

    /// Without `order`, document order stands.
    #[test]
    fn test_grid_items_keep_document_order_by_default() {
        let css = ".g { display: grid; grid-template-columns: 1fr 1fr; }";
        let html = r#"<div style="width:800px"><div class="g">
            <div id="first">text</div><div id="second">picture</div>
        </div></div>"#;
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);
        let first = find_element_by_id(&layout, "first").expect("first");
        let second = find_element_by_id(&layout, "second").expect("second");
        assert!(first.dimensions.x < second.dimensions.x);
    }

    /// Everything on a line hangs from one baseline. An atomic inline rests its
    /// bottom margin edge there, so a short one sits *below* the line's top
    /// rather than flush with it — which is where an icon beside a run of text
    /// goes, and where top-aligning it put it three pixels too high.
    #[test]
    fn test_an_atomic_inline_rests_on_the_line_s_baseline() {
        let css = ".flag { display: inline-block; width: 12px; height: 12px }";
        let html = r#"<div style="width:800px;font-size:14px;line-height:21px">
            <div id="line"><i class="flag" id="flag"></i> text beside it</div>
        </div>"#;
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);
        let line = find_element_by_id(&layout, "line").expect("line");
        let flag = find_element_by_id(&layout, "flag").expect("flag");
        let drop = flag.dimensions.y - line.dimensions.y;
        assert!(
            drop > 1.0,
            "a 12px box on a 21px line sits below its top, not flush with it; got {drop}",
        );
        assert!(
            flag.dimensions.y + flag.dimensions.height
                <= line.dimensions.y + line.dimensions.height + 0.5,
            "and it stays inside the line box",
        );
    }

    /// A block's bottom margin is held back to collapse with the next block's
    /// top margin. Real inline content after it forms an anonymous block, which
    /// has no margin to collapse with, so the held margin is space before it.
    #[test]
    fn test_a_block_s_bottom_margin_precedes_inline_content() {
        let css = ".flag { display: inline-block; width: 12px; height: 12px }                   .box { margin-bottom: 8px }";
        let html = r#"<div style="width:800px;font-size:14px;line-height:21px">
            <div class="box" id="above">above</div>
            <i class="flag" id="flag"></i>
        </div>"#;
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);
        let above = find_element_by_id(&layout, "above").expect("above");
        let flag = find_element_by_id(&layout, "flag").expect("flag");
        let line_top = above.dimensions.y + above.dimensions.height + 8.0;
        assert!(
            flag.dimensions.y >= line_top - 0.5,
            "the 8px margin is space before the anonymous block: line top {line_top}, flag at {}",
            flag.dimensions.y,
        );
    }

    /// Whitespace between two blocks is not inline content and must not
    /// interrupt their margins collapsing.
    #[test]
    fn test_whitespace_between_blocks_does_not_break_collapsing() {
        let html = r#"<div style="width:800px">
            <p style="margin:16px 0;height:50px">A</p>
            <p style="margin:16px 0;height:50px">B</p>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        fn blocks<'a>(b: &'a LayoutBox<'a>, out: &mut Vec<&'a LayoutBox<'a>>) {
            for c in &b.children {
                if matches!(c.style_node.node.data, NodeData::Element { ref name, .. } if name.local.as_ref() == "p") {
                    out.push(c);
                }
                blocks(c, out);
            }
        }
        let mut ps = Vec::new();
        blocks(&layout, &mut ps);
        assert_eq!(ps.len(), 2);
        let gap = ps[1].dimensions.y - (ps[0].dimensions.y + ps[0].dimensions.height);
        assert!((gap - 16.0).abs() < 0.5, "16px collapsed, not 32px; got {gap}");
    }

    /// CSS drops whitespace at the start and end of a block's inline content,
    /// so an intrinsic measurement must not keep a space there. A date written
    /// as two spans inside a `<p>` indented in the source carries a
    /// whitespace-only text node on each side of them; counting those made the
    /// box wide enough to push the heading beside it onto a second line.
    #[test]
    fn test_edge_whitespace_is_not_measured() {
        let css = ".row { display: inline-block; font-family: monospace; font-size: 16px }";
        let html = "<div style=\"width:800px\">             <div id=\"tight\" class=\"row\"><span>ab</span> <span>cd</span></div>             <div id=\"padded\" class=\"row\">
    <span>ab</span>
    <span>cd</span>
  </div>           </div>";
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);

        let tight = find_element_by_id(&layout, "tight").expect("tight");
        let padded = find_element_by_id(&layout, "padded").expect("padded");
        assert!(
            (tight.dimensions.width - padded.dimensions.width).abs() < 0.5,
            "source indentation must not widen the box: {} vs {}",
            tight.dimensions.width,
            padded.dimensions.width
        );
    }

    /// What a box contributes to its parent's intrinsic size is bounded by its
    /// own `min-width` and `max-width`. Measuring the labels alone left
    /// github's hero toggle 109px short of the five buttons it holds, and the
    /// last one was clipped away by the container's `overflow: hidden`.
    #[test]
    fn test_intrinsic_width_is_bounded_by_the_box_s_own_constraints() {
        let css = r#"
            .row { display: inline-block; }
            .pill { display: inline-block; min-width: 110px; box-sizing: border-box; }
            .capped { display: inline-block; max-width: 40px; box-sizing: border-box; }
        "#;
        let html = r#"<div style="width:800px">
            <div id="row" class="row"><span class="pill">Code</span><span class="pill">Plan</span></div>
            <div id="cap" class="row"><span class="capped">a very long label indeed</span></div>
        </div>"#;
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);

        let row = find_element_by_id(&layout, "row").expect("row");
        assert!(
            (row.dimensions.width - 220.0).abs() < 1.0,
            "two 110px pills make a 220px row; got {}",
            row.dimensions.width
        );

        let cap = find_element_by_id(&layout, "cap").expect("cap");
        assert!(
            cap.dimensions.width <= 41.0,
            "`max-width` bounds the contribution too; got {}",
            cap.dimensions.width
        );
    }

    /// Taking a box out of flow blockifies it. Every generated box this engine
    /// builds is a `<span>`, so `::before { content: ""; position: absolute;
    /// inset: 0 }` — a wash, a glow, a disc behind an icon — was laid out inline
    /// and came out its content's size, which for an empty one is nothing.
    #[test]
    fn test_an_out_of_flow_inline_box_is_blockified() {
        let html = r#"<div style="width:800px">
            <div id="host" style="position:relative;height:120px">
                <span id="overlay" style="position:absolute;top:0;right:0;bottom:0;left:0"></span>
            </div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let overlay = find_element_by_id(&layout, "overlay").expect("overlay");
        assert_eq!(overlay.display, DisplayType::Block, "an absolute inline box is block-level");
        assert!(
            (overlay.dimensions.width - 800.0).abs() < 0.5,
            "`left: 0; right: 0` stretches it across the block; got {}",
            overlay.dimensions.width
        );
        assert!(
            (overlay.dimensions.height - 120.0).abs() < 0.5,
            "`top: 0; bottom: 0` stretches it down the block; got {}",
            overlay.dimensions.height
        );
    }

    /// A float is out of flow too, and blockifies the same way.
    #[test]
    fn test_a_floated_inline_box_is_blockified() {
        let html = r#"<div style="width:800px"><span id="f" style="float:left;width:60px;height:60px"></span></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let f = find_element_by_id(&layout, "f").expect("float");
        assert_eq!(f.display, DisplayType::Block);
    }

    /// An in-flow inline box stays inline: blockification is about being out of
    /// flow, not about the properties that usually come with it.
    #[test]
    fn test_an_in_flow_inline_box_is_left_alone() {
        let html = r#"<div style="width:800px"><span id="s" style="position:relative">text</span></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let s = find_element_by_id(&layout, "s").expect("span");
        assert_eq!(s.display, DisplayType::Inline);
    }

    /// An absolute box laid out by a *static* parent resolves its percentages
    /// against an ancestor whose height is not settled when that parent runs.
    /// Placed once and left there, `height: 100%` came out zero — which is how
    /// github's hero glow, a `::before` sized exactly that way, went missing.
    #[test]
    fn test_a_percentage_sized_absolute_box_under_a_static_parent() {
        let html = r#"<div style="width:800px">
            <div id="cb" style="position:relative">
                <div>
                    <div id="host" style="height:160px">
                        <div id="wash" style="position:absolute;top:0;left:0;width:100%;height:100%"></div>
                    </div>
                </div>
            </div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let wash = find_element_by_id(&layout, "wash").expect("wash");
        assert!(
            (wash.dimensions.width - 800.0).abs() < 0.5,
            "100% of the containing block's width; got {}",
            wash.dimensions.width
        );
        assert!(
            (wash.dimensions.height - 160.0).abs() < 0.5,
            "100% of the containing block's settled height; got {}",
            wash.dimensions.height
        );
    }

    /// Half of a settled containing block, to show the second placement reads
    /// the percentage rather than just filling the block.
    #[test]
    fn test_a_half_height_absolute_box_under_a_static_parent() {
        let html = r#"<div style="width:800px">
            <div id="cb" style="position:relative">
                <div><div style="height:200px">
                    <div id="wash" style="position:absolute;top:0;left:0;width:50%;height:50%"></div>
                </div></div>
            </div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);
        let wash = find_element_by_id(&layout, "wash").expect("wash");
        assert!((wash.dimensions.width - 400.0).abs() < 0.5, "got {}", wash.dimensions.width);
        assert!((wash.dimensions.height - 100.0).abs() < 0.5, "got {}", wash.dimensions.height);
    }

    /// A generated box is a box, not a text run. `content: ""` with a stated
    /// size is how a page draws a disc behind an icon or a stripe down the side
    /// of a card, and none of that survives being modelled as a text node.
    #[test]
    fn test_a_generated_box_is_sized_by_its_own_properties() {
        let css = r#"
            .disc { --bg: #ffffff2e; position: relative; width: 44px; height: 44px }
            .disc::before { content: ""; background: var(--bg); width: 100%; height: 100%; position: absolute }
            .card { --accent: #3fb950; padding: 12px; position: relative }
            .card::before { content: ""; position: absolute; left: 0; top: 0; bottom: 0; width: 4px; background: var(--accent) }
            .rule::after { content: ""; display: block; height: 4px; width: 120px; background: #a371f7 }
        "#;
        let html = r#"<div style="width:400px">
            <div class="disc" id="disc"></div>
            <div class="card" id="card">a line of text</div>
            <h3 class="rule" id="rule">a heading</h3>
        </div>"#;
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);

        // The disc's generated box fills it, and carries the colour the custom
        // property names.
        let disc = find_element_by_id(&layout, "disc").expect("disc");
        let before = disc.children.first().expect("a generated child");
        assert!(
            (outer_width(before) - 44.0).abs() < 0.5 && (outer_height(before) - 44.0).abs() < 0.5,
            "the generated box is 44x44, got {}x{}",
            outer_width(before),
            outer_height(before)
        );
        assert!(
            matches!(
                before.style_node.specified_values.get(&crate::css::intern("background-color")),
                Some(Value::Color(c)) if c.a > 0
            ),
            "and takes the colour its `var()` names"
        );

        // `top: 0; bottom: 0` stretches against the *padding* box, so a stripe
        // down a padded card is as tall as the card, not as its text.
        let card = find_element_by_id(&layout, "card").expect("card");
        let stripe = card.children.first().expect("a generated child");
        assert!(
            (outer_height(stripe) - outer_height(card)).abs() < 0.5,
            "the stripe covers the card's {} px, got {}",
            outer_height(card),
            outer_height(stripe)
        );

        // A generated box in flow takes its own size and pushes the box it
        // belongs to.
        let rule = find_element_by_id(&layout, "rule").expect("rule");
        let after = rule.children.last().expect("a generated child");
        assert!(
            (outer_height(after) - 4.0).abs() < 0.5 && (outer_width(after) - 120.0).abs() < 0.5,
            "the rule is 120x4, got {}x{}",
            outer_width(after),
            outer_height(after)
        );
    }

    /// A selector's verdict is cached against the element's tag, id and classes,
    /// so anything the cache cannot see has to opt out of it. `:first-child` is
    /// decided by where an element sits among its siblings — caching the first
    /// `.row:first-child` verdict handed it to every `.row` on the page, and
    /// yunseong's project list gave each of its five entries the first one's
    /// zero top padding.
    #[test]
    fn test_a_positional_pseudo_class_is_matched_per_element() {
        let css = r#"
            .row { padding: 32px 0 }
            .row:first-child { padding-top: 0 }
            .row:last-child { padding-bottom: 0 }
        "#;
        let html = r#"<div style="width:800px">
            <div class="row" id="one">one</div>
            <div class="row" id="two">two</div>
            <div class="row" id="three">three</div>
        </div>"#;
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);

        let h = |id: &str| outer_height(find_element_by_id(&layout, id).expect(id));
        // One line of text is ~18px; what matters is which paddings survived.
        let line = h("two") - 64.0;
        assert!(line > 8.0 && line < 40.0, "a single line, got {line}");
        assert!(
            (h("one") - (line + 32.0)).abs() < 0.5,
            "the first row keeps only its bottom padding, got {}",
            h("one")
        );
        assert!(
            (h("three") - (line + 32.0)).abs() < 0.5,
            "the last row keeps only its top padding, got {}",
            h("three")
        );
    }

    /// Paint order among positioned boxes is document order, so an out-of-flow
    /// child — which has to be laid out after the in-flow ones, since its
    /// offsets resolve against a box whose size is not known until then — goes
    /// back where the document put it. Appending them instead let github's hero
    /// carousel paint its video under the two gradients that sit behind it in
    /// the markup.
    #[test]
    fn test_a_positioned_child_keeps_its_place_among_its_siblings() {
        let html = r#"<div id="root" style="position:relative;width:400px;height:200px">
            <div id="under" style="position:absolute;inset:0"></div>
            <div id="flow">in flow</div>
            <div id="over" style="position:absolute;inset:0"></div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let root = find_element_by_id(&layout, "root").expect("root");
        let order: Vec<String> = root
            .children
            .iter()
            .filter_map(|c| match c.style_node.node.data {
                NodeData::Element { ref attrs, .. } => attrs
                    .borrow()
                    .iter()
                    .find(|a| &a.name.local == "id")
                    .map(|a| a.value.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            order,
            vec!["under".to_string(), "flow".to_string(), "over".to_string()],
            "children come back in document order"
        );
    }

    /// `rem` resolves against the *root* element's font size wherever it
    /// appears, `calc()` included. Layout knows the element's own font size but
    /// not the root's, so the fold has to happen in the style pass — resolving
    /// it against the element instead turned github's `calc(100% - 2 * 2rem)`
    /// into 56px of inset where the page asks for 64, and its hero carousel came
    /// out 8px wide of where it belongs.
    #[test]
    fn test_rem_in_calc_resolves_against_the_root_font_size() {
        let css = r#"
            html { font-size: 16px }
            #mix { font-size: 14px; width: calc(2rem + 2em) }
            #inset { font-size: 14px; --w: calc(100% - 2 * 2rem); max-width: var(--w) }
        "#;
        let html = r#"<div style="width:800px"><div id="mix"></div><div id="inset"></div></div>"#;
        let (layout, _, _) = layout_from_html_css(html, css, 800.0, 600.0);

        // 2rem against the root's 16px is 32; 2em against the element's 14px is 28.
        let mix = find_element_by_id(&layout, "mix").expect("mix");
        assert!(
            (outer_width(mix) - 60.0).abs() < 0.5,
            "2rem (32) + 2em (28) is 60, got {}",
            outer_width(mix)
        );
        // And the same through a custom property, which is how a design system
        // writes its gutters.
        let inset = find_element_by_id(&layout, "inset").expect("inset");
        assert!(
            (outer_width(inset) - 736.0).abs() < 0.5,
            "800 less two 2rem gutters is 736, got {}",
            outer_width(inset)
        );
    }

    /// An atomic inline normally lends the line the baseline of the text inside
    /// it. A box that hides its own baseline — `contain: layout`, a scroll
    /// container, or one with nothing in it — rests its whole box on the
    /// baseline instead, so the line still keeps the strut's descender space
    /// underneath. github puts `contain: content` on every link, which is 6px a
    /// row down its footer's link columns.
    #[test]
    fn test_a_contained_atomic_inline_leaves_the_struts_descender_space() {
        let html = r#"<div style="width:800px;font-size:14px;line-height:21px">
            <div id="plain"><span style="display:inline-block">an inline-block</span></div>
            <div id="held"><span style="display:inline-block;contain:content">an inline-block</span></div>
            <div id="empty"><span style="display:inline-block;width:40px;height:40px"></span></div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let plain = outer_height(find_element_by_id(&layout, "plain").expect("plain"));
        let held = outer_height(find_element_by_id(&layout, "held").expect("held"));
        assert!(
            (plain - 21.0).abs() < 0.5,
            "an ordinary inline-block lines its text up with the strut, got {plain}"
        );
        assert!(
            held > plain + 2.0,
            "a contained one leaves the descender space under it: {held} should exceed {plain}"
        );

        // An empty box has no baseline of its own either, so the same rule puts
        // the strut's descender space below its 40px.
        let empty = outer_height(find_element_by_id(&layout, "empty").expect("empty"));
        assert!(
            empty > 42.0,
            "an empty 40px box sits above the baseline, got {empty}"
        );
    }

    /// A row track with a zero flex factor takes none of the free space and
    /// contributes no content of its own, so it is nothing tall. That is how a
    /// page holds a disclosure panel closed while keeping it animatable —
    /// `grid-template-rows: 0fr`, opened by swapping in `1fr`.
    #[test]
    fn test_a_zero_fr_row_collapses_its_item() {
        let html = r#"<div style="width:800px">
            <div id="shut" style="display:grid;grid-template-rows:0fr"><div id="shutinner" style="overflow:hidden"><p style="margin:0">panel text</p></div></div>
            <div id="open" style="display:grid;grid-template-rows:1fr"><div style="overflow:hidden"><p style="margin:0">panel text</p></div></div>
            <div id="after">after</div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let shut = find_element_by_id(&layout, "shut").expect("shut");
        assert!(
            outer_height(shut) < 0.5,
            "a 0fr row is nothing tall, got {}",
            outer_height(shut)
        );
        let inner = find_element_by_id(&layout, "shutinner").expect("shutinner");
        assert!(
            outer_height(inner) < 0.5,
            "and the item stretched into it takes the row's size, got {}",
            outer_height(inner)
        );
        let open = find_element_by_id(&layout, "open").expect("open");
        assert!(
            outer_height(open) > 10.0,
            "a 1fr row is as tall as what is in it, got {}",
            outer_height(open)
        );
    }

    /// A CSS-wide keyword resets a whole shorthand. `flex` is not inherited, so
    /// `flex: unset` means the initial `0 1 auto` — not "leave whatever an
    /// earlier rule said", which is what dropping the declaration amounted to.
    #[test]
    fn test_flex_unset_returns_an_item_to_its_initial_flex() {
        let html = r#"<div style="width:800px"><div style="display:flex">
            <div id="grow" style="flex:1 1 0%"></div>
            <div id="fixed" style="flex:1 1 0%;flex:unset;width:200px"></div>
        </div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let fixed = find_element_by_id(&layout, "fixed").expect("fixed");
        assert!(
            (outer_width(fixed) - 200.0).abs() < 1.0,
            "`flex: unset` leaves the item at its own width, got {}",
            outer_width(fixed)
        );
        let grow = find_element_by_id(&layout, "grow").expect("grow");
        assert!(
            (outer_width(grow) - 600.0).abs() < 1.0,
            "and the growing item takes the rest, got {}",
            outer_width(grow)
        );
    }

    /// `max-width` and `min-width` are lengths like any other: a percentage and
    /// a `calc()` bind exactly as a pixel value does. github's hero carousel is
    /// held to `calc(100% - 2 * 32px)` and, unbounded, ran the full width of the
    /// viewport.
    #[test]
    fn test_width_bounds_accept_percentages_and_calc() {
        let html = r#"<div style="width:800px">
            <div id="pct" style="max-width:50%"></div>
            <div id="calc" style="max-width:calc(100% - 64px)"></div>
            <div id="floor" style="width:100px;min-width:25%"></div>
        </div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        for (id, want) in [("pct", 400.0), ("calc", 736.0), ("floor", 200.0)] {
            let b = find_element_by_id(&layout, id).expect(id);
            assert!(
                (outer_width(b) - want).abs() < 0.5,
                "#{id} should be {want} wide, got {}",
                outer_width(b)
            );
        }
    }

    /// Flow advances past a box's *border* box. A stated height leaves
    /// `dimensions.height` as the content box, so reading it directly handed the
    /// next block a cursor the box's own padding too high.
    #[test]
    fn test_a_stated_height_advances_flow_by_the_border_box() {
        let html = r#"<div style="width:800px"><div id="a" style="height:100px;padding:8px;border:2px solid #000"></div><div id="after">after</div></div>"#;
        let (layout, _, _) = layout_from_html(html, 800.0, 600.0);

        let after = find_element_by_id(&layout, "after").expect("after");
        assert!(
            (after.dimensions.y - 120.0).abs() < 0.5,
            "100 of content plus 16 of padding and 4 of border is 120, got y={}",
            after.dimensions.y
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

    /// `grid-template-columns: auto 1fr` → the auto track hugs its content and
    /// the fr track takes the rest. Sizing `auto` out of the free space instead
    /// left nothing for the `fr`, so a sidebar's main column came out empty.
    #[test]
    fn test_grid_auto_track_hugs_content_leaving_room_for_fr() {
        let html = r#"<div id="grid">
            <div id="a">Nav</div>
            <div id="b">Main</div>
        </div>"#;
        let css_src = "#grid { display: grid; grid-template-columns: auto 1fr; }";
        let (layout, _, _) = layout_from_html_css(html, css_src, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("cell A");
        let b = find_element_by_id(&layout, "b").expect("cell B");

        assert!(
            a.dimensions.width > 0.0 && a.dimensions.width < 200.0,
            "auto column should hug its content, got {}",
            a.dimensions.width
        );
        assert!(
            b.dimensions.width > 600.0,
            "fr column should take the remaining space, got {}",
            b.dimensions.width
        );
        assert!(
            (a.dimensions.y - b.dimensions.y).abs() < 2.0,
            "A and B should share a row: a.y={}, b.y={}",
            a.dimensions.y, b.dimensions.y
        );
    }

    /// With no flexible track to absorb it, `auto auto` still spans the
    /// container rather than leaving the row half empty.
    #[test]
    fn test_grid_auto_tracks_without_fr_fill_container() {
        let html = r#"<div id="grid">
            <div id="a">A</div>
            <div id="b">B</div>
        </div>"#;
        let css_src = "#grid { display: grid; grid-template-columns: auto auto; }";
        let (layout, _, _) = layout_from_html_css(html, css_src, 800.0, 600.0);

        let a = find_element_by_id(&layout, "a").expect("cell A");
        let b = find_element_by_id(&layout, "b").expect("cell B");

        assert!(
            a.dimensions.width + b.dimensions.width > 700.0,
            "auto tracks with no fr beside them should fill the container, got {} + {}",
            a.dimensions.width, b.dimensions.width
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

/// The resolved display type of a styled node, for diagnostics.
pub fn debug_display_type(sn: &StyledNode) -> DisplayType {
    get_display_type(sn)
}



