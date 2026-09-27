//! CSS backgrounds: parsing of the `background` shorthand and the
//! `background-image` / `-position` / `-size` / `-repeat` / `-clip` / `-origin`
//! longhands, plus the geometry that places one background image tile.
//!
//! Storage model (kept deliberately simple so the rest of the style system does
//! not need new `Value` variants):
//! - `background-color` is a normal parsed value (`Value::Color`, or whatever
//!   `parse_value` returns, e.g. `Value::CssVar`).
//! - `background-image` is `Value::Gradient` when it is a single gradient layer
//!   (what the existing gradient painter reads), otherwise `Value::Keyword`
//!   holding the raw layer list text (`url(a.png)`, `none`, or several layers).
//! - `background-position`, `-size`, `-repeat`, `-clip`, `-origin` are
//!   `Value::Keyword` holding the raw comma-separated layer list text. They are
//!   parsed at paint time by the functions in this module.

use std::sync::Arc;

use crate::css::{intern, parse_color, parse_gradient, parse_value, Declaration, GradientValue, Value};
use crate::layout::{LayoutBox, Rect};
use crate::style::PropertyMap;

// ── Declaration expansion ─────────────────────────────────────────────────────

/// Longhands that the `background` shorthand resets, in the order they are
/// pushed.
const SHORTHAND_LONGHANDS: [&str; 7] = [
    "background-color",
    "background-image",
    "background-position",
    "background-size",
    "background-repeat",
    "background-origin",
    "background-clip",
];

/// Returns `true` for the properties handled by [`push_background_declarations`].
pub fn is_background_property(name: &str) -> bool {
    matches!(
        name,
        "background"
            | "background-image"
            | "background-position"
            | "background-size"
            | "background-repeat"
            | "background-origin"
            | "background-clip"
    )
}

fn is_css_wide_keyword(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer")
}

/// Turn one `background*` declaration (already stripped of `!important`) into
/// the longhand declarations described in the module docs, appending them to
/// `out`.
pub fn push_background_declarations(name: &str, raw: &str, important: bool, out: &mut Vec<Declaration>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    let mut push = |n: &str, value: Value| {
        out.push(Declaration { name: intern(n), value, important });
    };

    if is_css_wide_keyword(raw) {
        if name == "background" {
            for n in SHORTHAND_LONGHANDS {
                push(n, Value::Keyword(intern(&raw.to_ascii_lowercase())));
            }
        } else {
            push(name, Value::Keyword(intern(&raw.to_ascii_lowercase())));
        }
        return;
    }

    // A `var()` reference cannot be expanded before substitution. Keep the
    // old behavior: a single parsed declaration under the original name.
    if raw.contains("var(") {
        push(name, parse_value(raw));
        return;
    }

    match name {
        "background" => {
            let parsed = parse_background_shorthand(raw);
            push("background-color", parsed.color);
            push("background-image", image_list_value(&parsed.images.join(", ")));
            push("background-position", keyword(&parsed.positions.join(", ")));
            push("background-size", keyword(&parsed.sizes.join(", ")));
            push("background-repeat", keyword(&parsed.repeats.join(", ")));
            push("background-origin", keyword(&parsed.origins.join(", ")));
            push("background-clip", keyword(&parsed.clips.join(", ")));
        }
        "background-image" => push(name, image_list_value(raw)),
        _ => push(name, keyword(raw)),
    }
}

fn keyword(s: &str) -> Value {
    Value::Keyword(intern(s.trim()))
}

/// `background-image` value: a lone gradient stays a `Value::Gradient` so the
/// gradient painter keeps working; anything else is kept as raw text.
fn image_list_value(raw: &str) -> Value {
    let raw = raw.trim();
    let layers = split_top_level(raw, ',');
    if layers.len() == 1 {
        let l = layers[0].trim();
        if is_gradient(l) {
            if let Some(g) = parse_gradient(l) {
                return Value::Gradient(g);
            }
        }
    }
    keyword(raw)
}

fn is_gradient(s: &str) -> bool {
    let s = s.trim_start();
    s.starts_with("linear-gradient(")
        || s.starts_with("radial-gradient(")
        || s.starts_with("repeating-linear-gradient(")
        || s.starts_with("repeating-radial-gradient(")
        || s.starts_with("conic-gradient(")
}

/// Split `s` on `sep` at parenthesis depth 0 (so commas inside `rgb()` or
/// `url()` do not split). Quotes are respected too.
pub fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            c if c == sep && depth == 0 => {
                parts.push(std::mem::take(&mut cur));
            }
            c if sep == ' ' && c.is_whitespace() && depth == 0 => {
                parts.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    if sep == ' ' {
        parts.retain(|p| !p.trim().is_empty());
    }
    parts.into_iter().map(|p| p.trim().to_string()).collect()
}

struct ShorthandParts {
    color: Value,
    images: Vec<String>,
    positions: Vec<String>,
    sizes: Vec<String>,
    repeats: Vec<String>,
    origins: Vec<String>,
    clips: Vec<String>,
}

fn is_box_keyword(t: &str) -> bool {
    matches!(t, "border-box" | "padding-box" | "content-box" | "text")
}

fn is_repeat_keyword(t: &str) -> bool {
    matches!(t, "repeat" | "no-repeat" | "repeat-x" | "repeat-y" | "space" | "round")
}

fn is_position_token(t: &str) -> bool {
    matches!(t, "left" | "right" | "top" | "bottom" | "center") || parse_length_pct(t).is_some()
}

fn is_image_token(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    l == "none" || l.starts_with("url(") || is_gradient(&l) || l.starts_with("image-set(")
        || l.starts_with("-webkit-image-set(")
}

/// Put spaces around top-level `/` so `top/1px` tokenizes as `top / 1px`.
fn space_out_slashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c == '/' && depth == 0 {
            out.push_str(" / ");
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_background_shorthand(raw: &str) -> ShorthandParts {
    let layers = split_top_level(raw, ',');
    let mut parts = ShorthandParts {
        color: Value::Keyword(intern("transparent")),
        images: Vec::new(),
        positions: Vec::new(),
        sizes: Vec::new(),
        repeats: Vec::new(),
        origins: Vec::new(),
        clips: Vec::new(),
    };
    let last = layers.len().saturating_sub(1);
    for (i, layer) in layers.iter().enumerate() {
        let tokens = split_top_level(&space_out_slashes(layer), ' ');
        let mut image = "none".to_string();
        let mut position: Vec<String> = Vec::new();
        let mut size: Vec<String> = Vec::new();
        let mut repeat: Vec<String> = Vec::new();
        let mut boxes: Vec<String> = Vec::new();
        let mut after_slash = false;
        for tok in tokens {
            let lower = tok.to_ascii_lowercase();
            if lower == "/" {
                after_slash = true;
                continue;
            }
            if after_slash && (lower == "auto" || lower == "cover" || lower == "contain" || parse_length_pct(&lower).is_some()) {
                size.push(lower);
                continue;
            }
            after_slash = false;
            if is_image_token(&tok) {
                image = tok.clone();
            } else if is_repeat_keyword(&lower) {
                repeat.push(lower);
            } else if is_box_keyword(&lower) {
                boxes.push(lower);
            } else if is_position_token(&lower) {
                position.push(lower);
            } else if matches!(lower.as_str(), "scroll" | "fixed" | "local") {
                // background-attachment: not modelled.
            } else if i == last {
                if let Some(c) = parse_color(&tok) {
                    parts.color = Value::Color(c);
                }
            }
        }
        parts.images.push(image);
        parts.positions.push(if position.is_empty() { "0% 0%".into() } else { position.join(" ") });
        parts.sizes.push(if size.is_empty() { "auto".into() } else { size.join(" ") });
        parts.repeats.push(if repeat.is_empty() { "repeat".into() } else { repeat.join(" ") });
        // One box keyword sets both origin and clip; two set origin then clip.
        let origin = boxes.first().cloned().unwrap_or_else(|| "padding-box".into());
        let clip = boxes.get(1).cloned().or_else(|| boxes.first().cloned()).unwrap_or_else(|| "border-box".into());
        parts.origins.push(origin);
        parts.clips.push(clip);
    }
    parts
}

// ── Value types ───────────────────────────────────────────────────────────────

/// `<length-percentage>` resolved later against a reference size: `px + pct% * ref`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LengthPct {
    pub px: f32,
    pub pct: f32,
}

impl LengthPct {
    pub fn px(v: f32) -> Self {
        LengthPct { px: v, pct: 0.0 }
    }
    pub fn pct(v: f32) -> Self {
        LengthPct { px: 0.0, pct: v }
    }
    pub fn resolve(&self, reference: f32) -> f32 {
        self.px + self.pct / 100.0 * reference
    }
}

/// Parse `12px`, `-3.5px`, `50%`, `0`, `1em`/`1rem` (16px), or a bare number
/// (treated as px, as quirks-mode engines do).
pub fn parse_length_pct(s: &str) -> Option<LengthPct> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let num = |t: &str| t.parse::<f32>().ok();
    if let Some(v) = s.strip_suffix('%') {
        return num(v).map(LengthPct::pct);
    }
    if let Some(v) = s.strip_suffix("px") {
        return num(v).map(LengthPct::px);
    }
    if let Some(v) = s.strip_suffix("rem") {
        return num(v).map(|n| LengthPct::px(n * 16.0));
    }
    if let Some(v) = s.strip_suffix("em") {
        return num(v).map(|n| LengthPct::px(n * 16.0));
    }
    num(s).map(LengthPct::px)
}

/// One axis of `background-position`: an offset from the start edge
/// (left/top) or from the end edge (right/bottom).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionAxis {
    pub from_end: bool,
    pub offset: LengthPct,
}

impl PositionAxis {
    fn start(offset: LengthPct) -> Self {
        PositionAxis { from_end: false, offset }
    }
    fn center() -> Self {
        Self::start(LengthPct::pct(50.0))
    }
    /// Position of the tile's start edge inside the positioning area, where
    /// `free = area_size - tile_size` (percentages resolve against it).
    pub fn resolve(&self, free: f32) -> f32 {
        let o = self.offset.resolve(free);
        if self.from_end { free - o } else { o }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackgroundPosition {
    pub x: PositionAxis,
    pub y: PositionAxis,
}

impl Default for BackgroundPosition {
    fn default() -> Self {
        BackgroundPosition { x: PositionAxis::start(LengthPct::default()), y: PositionAxis::start(LengthPct::default()) }
    }
}

/// Parse one layer of `background-position` (1 to 4 tokens).
pub fn parse_position(s: &str) -> BackgroundPosition {
    let toks: Vec<String> = s.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
    let kw_axis = |t: &str| -> Option<(char, PositionAxis)> {
        match t {
            "left" => Some(('x', PositionAxis::start(LengthPct::default()))),
            "right" => Some(('x', PositionAxis { from_end: true, offset: LengthPct::default() })),
            "top" => Some(('y', PositionAxis::start(LengthPct::default()))),
            "bottom" => Some(('y', PositionAxis { from_end: true, offset: LengthPct::default() })),
            "center" => Some(('c', PositionAxis::center())),
            _ => None,
        }
    };
    let as_axis = |t: &str| -> Option<PositionAxis> {
        kw_axis(t).map(|(_, a)| a).or_else(|| parse_length_pct(t).map(PositionAxis::start))
    };
    let mut pos = BackgroundPosition::default();
    match toks.len() {
        0 => {}
        1 => {
            let t = toks[0].as_str();
            match kw_axis(t) {
                Some(('y', a)) => {
                    pos.x = PositionAxis::center();
                    pos.y = a;
                }
                Some((_, a)) => {
                    pos.x = a;
                    pos.y = PositionAxis::center();
                }
                None => {
                    if let Some(l) = parse_length_pct(t) {
                        pos.x = PositionAxis::start(l);
                        pos.y = PositionAxis::center();
                    }
                }
            }
        }
        2 => {
            let (a, b) = (toks[0].as_str(), toks[1].as_str());
            let swap = matches!(a, "top" | "bottom") || matches!(b, "left" | "right");
            let (xs, ys) = if swap { (b, a) } else { (a, b) };
            if let Some(x) = as_axis(xs) {
                pos.x = x;
            }
            if let Some(y) = as_axis(ys) {
                pos.y = y;
            }
        }
        _ => {
            // 3/4-value syntax: `<edge> [<offset>] <edge> [<offset>]`.
            let mut x: Option<PositionAxis> = None;
            let mut y: Option<PositionAxis> = None;
            let mut pending_center = 0;
            let mut i = 0;
            while i < toks.len() {
                let Some((axis, mut a)) = kw_axis(&toks[i]) else { i += 1; continue };
                if axis != 'c' {
                    if let Some(off) = toks.get(i + 1).and_then(|t| parse_length_pct(t)) {
                        a.offset = off;
                        i += 1;
                    }
                }
                match axis {
                    'x' => x = Some(a),
                    'y' => y = Some(a),
                    _ => pending_center += 1,
                }
                i += 1;
            }
            let _ = pending_center;
            pos.x = x.unwrap_or_else(PositionAxis::center);
            pos.y = y.unwrap_or_else(PositionAxis::center);
        }
    }
    pos
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundSize {
    Cover,
    Contain,
    /// Width and height; `None` means `auto`.
    Explicit(Option<LengthPct>, Option<LengthPct>),
}

impl Default for BackgroundSize {
    fn default() -> Self {
        BackgroundSize::Explicit(None, None)
    }
}

pub fn parse_size(s: &str) -> BackgroundSize {
    let toks: Vec<String> = s.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
    let one = |t: &str| if t == "auto" { None } else { parse_length_pct(t) };
    match toks.as_slice() {
        [t] if t == "cover" => BackgroundSize::Cover,
        [t] if t == "contain" => BackgroundSize::Contain,
        [w] => BackgroundSize::Explicit(one(w), None),
        [w, h, ..] => BackgroundSize::Explicit(one(w), one(h)),
        [] => BackgroundSize::default(),
    }
}

/// `(repeat_x, repeat_y)`. `space` and `round` are approximated as `repeat`.
pub fn parse_repeat(s: &str) -> (bool, bool) {
    let toks: Vec<String> = s.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
    let rep = |t: &str| t != "no-repeat";
    match toks.as_slice() {
        [t] if t == "repeat-x" => (true, false),
        [t] if t == "repeat-y" => (false, true),
        [t] => (rep(t), rep(t)),
        [a, b, ..] => (rep(a), rep(b)),
        [] => (true, true),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxArea {
    Border,
    Padding,
    Content,
}

fn parse_box(s: &str, default: BoxArea) -> BoxArea {
    match s.trim().to_ascii_lowercase().as_str() {
        "border-box" => BoxArea::Border,
        "padding-box" => BoxArea::Padding,
        "content-box" => BoxArea::Content,
        _ => default,
    }
}

/// Extract the URL from `url(...)`, handling quotes.
pub fn parse_url(s: &str) -> Option<String> {
    let s = s.trim();
    let lower = s.get(..4)?.to_ascii_lowercase();
    if lower != "url(" || !s.ends_with(')') {
        return None;
    }
    let inner = s[4..s.len() - 1].trim();
    let inner = inner
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| inner.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(inner);
    if inner.is_empty() { None } else { Some(inner.to_string()) }
}

// ── Layers ────────────────────────────────────────────────────────────────────

/// One background image layer image source.
#[derive(Debug, Clone, PartialEq)]
pub enum LayerImage {
    Url(String),
    Gradient(GradientValue),
}

/// A fully parsed background layer, ready to be placed against a box.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundLayer {
    pub image: LayerImage,
    pub position: BackgroundPosition,
    pub size: BackgroundSize,
    pub repeat_x: bool,
    pub repeat_y: bool,
    pub origin: BoxArea,
    pub clip: BoxArea,
}

/// Best-effort CSS text for a stored value (so values that went through the
/// generic `parse_value` path, e.g. after `var()` substitution, still work).
fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::Keyword(k) => Some(k.to_string()),
        Value::Length(n, unit) => {
            let suffix = match unit {
                crate::css::Unit::Px => "px",
                crate::css::Unit::Percent => "%",
                crate::css::Unit::Em => "em",
                _ => "px",
            };
            Some(format!("{}{}", n, suffix))
        }
        Value::Number(n) => Some(format!("{}", n)),
        _ => None,
    }
}

fn list_item(list: &[String], i: usize) -> Option<&str> {
    if list.is_empty() {
        None
    } else {
        Some(list[i % list.len()].as_str())
    }
}

/// Parse the background image layers of a box from its specified values, in
/// paint order (bottom-most layer first). Only layers with an image are
/// returned.
pub fn background_layers(sv: &PropertyMap) -> Vec<BackgroundLayer> {
    let images: Vec<LayerImage> = match sv.get(&intern("background-image")) {
        Some(Value::Gradient(g)) => vec![LayerImage::Gradient(g.clone())],
        Some(v) => match value_text(v) {
            Some(text) => split_top_level(&text, ',')
                .into_iter()
                .filter_map(|l| {
                    if let Some(u) = parse_url(&l) {
                        Some(LayerImage::Url(u))
                    } else if is_gradient(&l) {
                        parse_gradient(&l).map(LayerImage::Gradient)
                    } else {
                        None
                    }
                })
                .collect(),
            None => Vec::new(),
        },
        None => match sv.get(&intern("background")) {
            // Legacy `background` value that could not be expanded (var()).
            Some(Value::Gradient(g)) => vec![LayerImage::Gradient(g.clone())],
            _ => Vec::new(),
        },
    };
    if images.is_empty() {
        return Vec::new();
    }
    let list = |name: &str| -> Vec<String> {
        sv.get(&intern(name))
            .and_then(value_text)
            .map(|t| split_top_level(&t, ','))
            .unwrap_or_default()
    };
    let positions = list("background-position");
    let sizes = list("background-size");
    let repeats = list("background-repeat");
    let origins = list("background-origin");
    let clips = list("background-clip");

    let mut layers: Vec<BackgroundLayer> = images
        .into_iter()
        .enumerate()
        .map(|(i, image)| {
            let (repeat_x, repeat_y) = list_item(&repeats, i).map(parse_repeat).unwrap_or((true, true));
            BackgroundLayer {
                image,
                position: list_item(&positions, i).map(parse_position).unwrap_or_default(),
                size: list_item(&sizes, i).map(parse_size).unwrap_or_default(),
                repeat_x,
                repeat_y,
                origin: list_item(&origins, i).map(|s| parse_box(s, BoxArea::Padding)).unwrap_or(BoxArea::Padding),
                clip: list_item(&clips, i).map(|s| parse_box(s, BoxArea::Border)).unwrap_or(BoxArea::Border),
            }
        })
        .collect();
    // CSS lists the top-most layer first; paint bottom-most first.
    layers.reverse();
    layers
}

// ── Geometry ──────────────────────────────────────────────────────────────────

/// The box of `layout` selected by `area`. `layout.dimensions` is the border box.
pub fn box_rect(layout: &LayoutBox, area: BoxArea) -> Rect {
    let d = layout.dimensions;
    let b = layout.border;
    let p = layout.padding;
    let inset = |r: Rect, l: f32, t: f32, rt: f32, bt: f32| Rect {
        x: r.x + l,
        y: r.y + t,
        width: (r.width - l - rt).max(0.0),
        height: (r.height - t - bt).max(0.0),
    };
    match area {
        BoxArea::Border => d,
        BoxArea::Padding => inset(d, b.left, b.top, b.right, b.bottom),
        BoxArea::Content => {
            let pb = inset(d, b.left, b.top, b.right, b.bottom);
            inset(pb, p.left, p.top, p.right, p.bottom)
        }
    }
}

/// Size of one tile for an image with intrinsic size `(iw, ih)` in a
/// positioning area of size `(aw, ah)`.
pub fn tile_size(size: BackgroundSize, iw: f32, ih: f32, aw: f32, ah: f32) -> (f32, f32) {
    let (iw, ih) = (iw.max(1.0), ih.max(1.0));
    match size {
        BackgroundSize::Cover => {
            let s = (aw / iw).max(ah / ih);
            (iw * s, ih * s)
        }
        BackgroundSize::Contain => {
            let s = (aw / iw).min(ah / ih);
            (iw * s, ih * s)
        }
        BackgroundSize::Explicit(w, h) => match (w, h) {
            (None, None) => (iw, ih),
            (Some(w), None) => {
                let w = w.resolve(aw);
                (w, w * ih / iw)
            }
            (None, Some(h)) => {
                let h = h.resolve(ah);
                (h * iw / ih, h)
            }
            (Some(w), Some(h)) => (w.resolve(aw), h.resolve(ah)),
        },
    }
}

/// Rect of the anchor tile (the one placed by `background-position`) in page
/// coordinates. Position is snapped to whole pixels as Chromium does.
pub fn anchor_tile(area: Rect, tile_w: f32, tile_h: f32, pos: BackgroundPosition) -> Rect {
    let x = area.x + pos.x.resolve(area.width - tile_w);
    let y = area.y + pos.y.resolve(area.height - tile_h);
    Rect { x: x.round(), y: y.round(), width: tile_w, height: tile_h }
}

// ── URL collection ────────────────────────────────────────────────────────────

/// Collect the (absolute, de-duplicated) URLs of every `url()` background image
/// used by a box in the layout tree, so the engine can fetch them into its
/// image cache.
pub fn collect_background_image_urls(root: &LayoutBox, base_url: &url::Url, out: &mut Vec<String>) {
    let mut seen: std::collections::HashSet<String> = out.iter().cloned().collect();
    let mut stack: Vec<&LayoutBox> = vec![root];
    while let Some(node) = stack.pop() {
        for layer in background_layers(&node.style_node.specified_values) {
            if let LayerImage::Url(u) = layer.image {
                if u.starts_with("data:") {
                    continue;
                }
                let abs = base_url.join(&u).map(|u| u.to_string()).unwrap_or(u);
                if seen.insert(abs.clone()) {
                    out.push(abs);
                }
            }
        }
        for child in node.children.iter().rev() {
            stack.push(child);
        }
    }
}

/// Rewrite relative `url(...)` references in a remote stylesheet to absolute
/// URLs against the stylesheet's own URL, since stylesheets are concatenated
/// before parsing and would otherwise resolve against the document URL.
pub fn absolutize_css_urls(css: &str, sheet_url: &str) -> String {
    let Ok(base) = url::Url::parse(sheet_url) else { return css.to_string() };
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(idx) = find_ci(rest, "url(") {
        out.push_str(&rest[..idx + 4]);
        let after = &rest[idx + 4..];
        let Some(end) = after.find(')') else {
            rest = after;
            break;
        };
        let inner = after[..end].trim();
        let (q, body) = if let Some(b) = inner.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            ("\"", b)
        } else if let Some(b) = inner.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            ("'", b)
        } else {
            ("", inner)
        };
        let lower = body.to_ascii_lowercase();
        let is_absolute = lower.starts_with("data:")
            || lower.starts_with("http:")
            || lower.starts_with("https:")
            || lower.starts_with('#')
            || body.is_empty();
        if is_absolute {
            out.push_str(&after[..end]);
        } else {
            match base.join(body) {
                Ok(u) => {
                    out.push_str(q);
                    out.push_str(u.as_str());
                    out.push_str(q);
                }
                Err(_) => out.push_str(&after[..end]),
            }
        }
        out.push(')');
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

// ── Decoded image cache ───────────────────────────────────────────────────────

lazy_static::lazy_static! {
    /// Decoded, premultiplied images keyed by `(url, byte length, target w, target h)`.
    /// Target `(0, 0)` means intrinsic size.
    static ref DECODED: std::sync::Mutex<std::collections::HashMap<(String, usize, u32, u32), Option<Arc<tiny_skia::Pixmap>>>> =
        std::sync::Mutex::new(std::collections::HashMap::new());
}

const DECODED_CACHE_MAX_ENTRIES: usize = 512;

/// Convert straight-alpha RGBA bytes to the premultiplied form tiny-skia expects.
pub fn premultiply_rgba_in_place(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

fn premultiplied_pixmap(rgba: image::RgbaImage) -> Option<tiny_skia::Pixmap> {
    let (w, h) = rgba.dimensions();
    let mut data = rgba.into_raw();
    premultiply_rgba_in_place(&mut data);
    tiny_skia::Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}

/// Resample RGBA bytes from `sw x sh` to `dw x dh` (both no larger than the
/// source) by averaging the source area each destination pixel covers. Unlike
/// a tent filter, this never reads pixels outside that area, so neighbouring
/// icons in a sprite sheet do not bleed into each other.
fn area_downscale(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    // Per-axis list of (source index, weight) for each destination index.
    fn weights(s: u32, d: u32) -> Vec<Vec<(usize, f32)>> {
        let scale = s as f32 / d as f32;
        (0..d)
            .map(|i| {
                let a = i as f32 * scale;
                let b = (a + scale).min(s as f32);
                let mut w = Vec::new();
                let mut j = a.floor() as u32;
                while (j as f32) < b && j < s {
                    let cover = (b.min(j as f32 + 1.0) - a.max(j as f32)).max(0.0);
                    if cover > 0.0 {
                        w.push((j as usize, cover / scale));
                    }
                    j += 1;
                }
                w
            })
            .collect()
    }
    let wx = weights(sw, dw);
    let wy = weights(sh, dh);
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for (dy, ys) in wy.iter().enumerate() {
        for (dx, xs) in wx.iter().enumerate() {
            let mut acc = [0f32; 4];
            for &(sy, fy) in ys {
                for &(sx, fx) in xs {
                    let o = (sy * sw as usize + sx) * 4;
                    let f = fx * fy;
                    for c in 0..4 {
                        acc[c] += src[o + c] as f32 * f;
                    }
                }
            }
            let o = (dy * dw as usize + dx) * 4;
            let a = acc[3].round().clamp(0.0, 255.0);
            out[o + 3] = a as u8;
            for c in 0..3 {
                // Keep premultiplied invariant: color <= alpha.
                out[o + c] = acc[c].round().clamp(0.0, a) as u8;
            }
        }
    }
    out
}

/// Decode `bytes` (cached per URL) into a premultiplied pixmap, downscaled to
/// `target` when given (see [`area_downscale`]); this matches what Chromium
/// produces for 2x sprite sheets drawn at 1x.
pub fn decoded_image(url: &str, bytes: &[u8], target: Option<(u32, u32)>) -> Option<Arc<tiny_skia::Pixmap>> {
    let (tw, th) = target.unwrap_or((0, 0));
    let key = (url.to_string(), bytes.len(), tw, th);
    if let Some(hit) = DECODED.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let result = (|| {
        let img = image::load_from_memory(bytes).ok()?.to_rgba8();
        // Premultiply before resampling so transparent pixels do not bleed
        // their (arbitrary) color into edges.
        let pre = premultiplied_pixmap(img)?;
        if tw == 0 || th == 0 || (tw == pre.width() && th == pre.height()) {
            return Some(Arc::new(pre));
        }
        let resized = area_downscale(pre.data(), pre.width(), pre.height(), tw, th);
        tiny_skia::Pixmap::from_vec(resized, tiny_skia::IntSize::from_wh(tw, th)?).map(Arc::new)
    })();
    if let Ok(mut c) = DECODED.lock() {
        if c.len() >= DECODED_CACHE_MAX_ENTRIES {
            c.clear();
        }
        c.insert(key, result.clone());
    }
    result
}

/// Intrinsic size of an encoded image (cached decode).
pub fn intrinsic_size(url: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    decoded_image(url, bytes, None).map(|p| (p.width(), p.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decls(name: &str, raw: &str) -> Vec<Declaration> {
        let mut out = Vec::new();
        push_background_declarations(name, raw, false, &mut out);
        out
    }

    fn get<'a>(d: &'a [Declaration], name: &str) -> &'a Value {
        &d.iter().find(|d| d.name.as_ref() == name).unwrap_or_else(|| panic!("missing {name}")).value
    }

    fn kw(v: &Value) -> &str {
        match v {
            Value::Keyword(k) => k.as_ref(),
            other => panic!("expected keyword, got {:?}", other),
        }
    }

    #[test]
    fn test_shorthand_expands_url_position_repeat_color() {
        let d = decls("background", "url(https://x/sp.png) -10px -20px no-repeat #fff");
        assert!(matches!(get(&d, "background-color"), Value::Color(c) if c.r == 255 && c.a == 255));
        assert_eq!(kw(get(&d, "background-image")), "url(https://x/sp.png)");
        assert_eq!(kw(get(&d, "background-position")), "-10px -20px");
        assert_eq!(kw(get(&d, "background-repeat")), "no-repeat");
        assert_eq!(kw(get(&d, "background-size")), "auto");
    }

    #[test]
    fn test_shorthand_color_only_resets_image() {
        let d = decls("background", "#333");
        assert!(matches!(get(&d, "background-color"), Value::Color(c) if c.r == 0x33));
        assert_eq!(kw(get(&d, "background-image")), "none");
    }

    #[test]
    fn test_shorthand_position_slash_size() {
        let d = decls("background", "url(a.png) center/cover no-repeat");
        assert_eq!(kw(get(&d, "background-position")), "center");
        assert_eq!(kw(get(&d, "background-size")), "cover");
        let d = decls("background", "url(a.png) top/1px 22px");
        assert_eq!(kw(get(&d, "background-position")), "top");
        assert_eq!(kw(get(&d, "background-size")), "1px 22px");
    }

    #[test]
    fn test_shorthand_single_gradient_stays_gradient_value() {
        let d = decls("background", "linear-gradient(to right, red, blue)");
        assert!(matches!(get(&d, "background-image"), Value::Gradient(_)));
        assert!(matches!(get(&d, "background-color"), Value::Keyword(k) if k.as_ref() == "transparent"));
    }

    #[test]
    fn test_shorthand_with_var_keeps_legacy_declaration() {
        let d = decls("background", "var(--bg)");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].name.as_ref(), "background");
        assert!(matches!(d[0].value, Value::CssVar { .. }));
    }

    #[test]
    fn test_longhand_position_kept_raw() {
        let d = decls("background-position", "-468px -126px");
        assert_eq!(kw(get(&d, "background-position")), "-468px -126px");
    }

    #[test]
    fn test_parse_url_variants() {
        assert_eq!(parse_url("url(a.png)").as_deref(), Some("a.png"));
        assert_eq!(parse_url("url( \"a b.png\" )").as_deref(), Some("a b.png"));
        assert_eq!(parse_url("URL('x.png')").as_deref(), Some("x.png"));
        assert_eq!(parse_url("none"), None);
    }

    #[test]
    fn test_parse_position_forms() {
        let p = parse_position("-468px -126px");
        assert_eq!(p.x.resolve(100.0), -468.0);
        assert_eq!(p.y.resolve(100.0), -126.0);

        let p = parse_position("center");
        assert_eq!(p.x.resolve(40.0), 20.0);
        assert_eq!(p.y.resolve(40.0), 20.0);

        let p = parse_position("top");
        assert_eq!(p.x.resolve(40.0), 20.0);
        assert_eq!(p.y.resolve(40.0), 0.0);

        let p = parse_position("bottom right");
        assert_eq!(p.x.resolve(30.0), 30.0);
        assert_eq!(p.y.resolve(10.0), 10.0);

        let p = parse_position("50% 100%");
        assert_eq!(p.x.resolve(30.0), 15.0);
        assert_eq!(p.y.resolve(30.0), 30.0);

        let p = parse_position("right 10px top 5px");
        assert_eq!(p.x.resolve(100.0), 90.0);
        assert_eq!(p.y.resolve(100.0), 5.0);
    }

    #[test]
    fn test_parse_size_forms() {
        assert_eq!(parse_size("cover"), BackgroundSize::Cover);
        assert_eq!(parse_size("contain"), BackgroundSize::Contain);
        assert_eq!(parse_size("auto"), BackgroundSize::Explicit(None, None));
        assert_eq!(
            parse_size("484px 476px"),
            BackgroundSize::Explicit(Some(LengthPct::px(484.0)), Some(LengthPct::px(476.0)))
        );
        assert_eq!(parse_size("50%"), BackgroundSize::Explicit(Some(LengthPct::pct(50.0)), None));
    }

    #[test]
    fn test_parse_repeat_forms() {
        assert_eq!(parse_repeat("no-repeat"), (false, false));
        assert_eq!(parse_repeat("repeat-x"), (true, false));
        assert_eq!(parse_repeat("repeat-y"), (false, true));
        assert_eq!(parse_repeat("repeat no-repeat"), (true, false));
        assert_eq!(parse_repeat("repeat"), (true, true));
    }

    #[test]
    fn test_tile_size_rules() {
        // Sprite sheet 968x952 drawn at 484x476.
        let s = parse_size("484px 476px");
        assert_eq!(tile_size(s, 968.0, 952.0, 20.0, 20.0), (484.0, 476.0));
        // auto height keeps aspect ratio.
        assert_eq!(tile_size(parse_size("50px"), 100.0, 40.0, 200.0, 200.0), (50.0, 20.0));
        // cover / contain.
        assert_eq!(tile_size(BackgroundSize::Cover, 10.0, 20.0, 100.0, 100.0), (100.0, 200.0));
        assert_eq!(tile_size(BackgroundSize::Contain, 10.0, 20.0, 100.0, 100.0), (50.0, 100.0));
        // percent resolves against positioning area.
        assert_eq!(tile_size(parse_size("50% 25%"), 1.0, 1.0, 200.0, 80.0), (100.0, 20.0));
    }

    #[test]
    fn test_anchor_tile_percent_uses_free_space() {
        let area = Rect { x: 10.0, y: 20.0, width: 100.0, height: 50.0 };
        let t = anchor_tile(area, 20.0, 10.0, parse_position("100% 50%"));
        assert_eq!((t.x, t.y), (90.0, 40.0));
    }

    #[test]
    fn test_absolutize_css_urls() {
        let css = ".a{background:url(../img/sp.png)} .b{background:url('x.png')} .c{background:url(https://h/abs.png)} .d{background:url(data:image/png;base64,AA)}";
        let out = absolutize_css_urls(css, "https://cdn.example.com/css/main.css");
        assert!(out.contains("url(https://cdn.example.com/img/sp.png)"), "{out}");
        assert!(out.contains("url('https://cdn.example.com/css/x.png')"), "{out}");
        assert!(out.contains("url(https://h/abs.png)"));
        assert!(out.contains("url(data:image/png;base64,AA)"));
    }

    #[test]
    fn test_css_parse_background_longhands_via_stylesheet() {
        let ss = crate::css::parse_css(
            ".i{background-image:url(https://x/sp.png);background-size:484px 476px;background-position:-468px -126px;background-repeat:no-repeat}",
        );
        let rules = ss.all_rules();
        let d = &rules[0].declarations;
        assert_eq!(kw(get(d, "background-image")), "url(https://x/sp.png)");
        assert_eq!(kw(get(d, "background-position")), "-468px -126px");
        assert_eq!(kw(get(d, "background-size")), "484px 476px");
    }

    #[test]
    fn test_multi_layer_order_and_lists() {
        let d = decls("background", "url(top.png) no-repeat, url(bottom.png) repeat-x red");
        let mut map = std::collections::HashMap::new();
        for decl in d {
            map.insert(decl.name.clone(), decl.value.clone());
        }
        let sv = PropertyMap(Arc::new(map));
        let layers = background_layers(&sv);
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].image, LayerImage::Url("bottom.png".into()));
        assert_eq!((layers[0].repeat_x, layers[0].repeat_y), (true, false));
        assert_eq!(layers[1].image, LayerImage::Url("top.png".into()));
        assert_eq!((layers[1].repeat_x, layers[1].repeat_y), (false, false));
    }
}
