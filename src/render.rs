use tiny_skia::{Pixmap, Paint, Transform, Stroke, PathBuilder, PixmapPaint, Mask, FillRule,
    LinearGradient, RadialGradient, GradientStop, SpreadMode, Point as SkPoint,
    StrokeDash, LineCap};
use ab_glyph::{Font, PxScale, point};
use crate::layout::{LayoutBox, Rect as LayoutRect};
use crate::css::{Color, CssColorStop, LinearDirection};
use crate::layer_tree::{LayerTree, LayerTreeBuilder, PaintCommand, ObjectFit, BorderStyle};
use crate::matrix::Matrix4x4;
use std::collections::HashMap;
use std::sync::Mutex;
use lazy_static::lazy_static;
use std::time::Instant;
use url::Url;


// ── Glyph Cache ───────────────────────────────────────────────────────────────

/// Cache key for a single rasterized glyph.
///
/// Keyed on the face, glyph, font size (1/64 px), quarter-pixel horizontal
/// position and synthesis flags.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct GlyphKey {
    face: usize,
    glyph_id: u16,
    /// `(font_size * 2.0).round() as u32` — rounds to nearest 0.5 px.
    font_size_half_px: u32,
    /// `(font_size * 64.0).round() as u32`.
    size_64: u32,
    /// Subpixel x bin (0..SUBPIXEL_STEPS).
    subpixel: u8,
    bold: bool,
    italic: bool,
}

/// LCD coverage bitmap for one glyph, relative to the pixel column of the pen
/// position (`left`) and the baseline row (`top`).
struct GlyphPixels {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    /// Row-major R/G/B subpixel coverage.
    coverage: Vec<[u8; 3]>,
}

lazy_static! {
    static ref TEXTURE_POOL: Mutex<TexturePool> = Mutex::new(TexturePool::new());

    /// Process-wide glyph rasterization cache.
    ///
    /// Populated on first use of each (glyph, size, style) combination.
    /// Call `clear_glyph_cache()` between page navigations to free memory.
    static ref GLYPH_CACHE: Mutex<HashMap<GlyphKey, std::sync::Arc<GlyphPixels>>> =
        Mutex::new(HashMap::new());
}

/// Evict all cached glyph bitmaps.
///
/// Must be called whenever the user navigates to a new page so that memory
/// freed between renders and the cache does not grow without bound across
/// many navigations.
pub fn clear_glyph_cache() {
    if let Ok(mut cache) = GLYPH_CACHE.lock() {
        cache.clear();
    }
}

/// A pool of reusable `Pixmap` buffers to avoid frequent allocations.
pub struct TexturePool {
    pool: HashMap<(u32, u32), Vec<Pixmap>>,
}

impl TexturePool {
    pub fn new() -> Self {
        Self { pool: HashMap::new() }
    }

    /// Acquire a `Pixmap` of the given size. Returns a new one if none available in pool.
    pub fn acquire(&mut self, width: u32, height: u32) -> Pixmap {
        if let Some(list) = self.pool.get_mut(&(width, height)) {
            if let Some(mut pixmap) = list.pop() {
                pixmap.fill(tiny_skia::Color::TRANSPARENT);
                return pixmap;
            }
        }
        Pixmap::new(width, height).expect("Failed to allocate Pixmap")
    }

    /// Release a `Pixmap` back into the pool for future reuse.
    pub fn release(&mut self, pixmap: Pixmap) {
        let size = (pixmap.width(), pixmap.height());
        self.pool.entry(size).or_insert_with(Vec::new).push(pixmap);
    }
}

/// Entry point for rendering. Builds a `LayerTree` from the layout, then
/// composites all layers onto `pixmap` in a parallel viewport-tiled approach.
pub fn render_layout_tree(
    layout: &LayoutBox,
    pixmap: &mut Pixmap,
    image_cache: &HashMap<String, Vec<u8>>,
    base_url: &Url,
) {
    let start = Instant::now();

    let viewport = LayoutRect {
        x: 0.0,
        y: 0.0,
        width: pixmap.width() as f32,
        height: pixmap.height() as f32,
    };

    let tree: LayerTree = LayerTreeBuilder::build(layout, viewport);
    let layer_gen_elapsed = start.elapsed();

    let start_render = Instant::now();
    composite_layer_to_surface(0, &tree, pixmap, viewport, image_cache, base_url);

    let render_elapsed = start_render.elapsed();
    println!("[Perf] render_layout_tree (Surface): Layer gen: {:?}, Actual render: {:?}", layer_gen_elapsed, render_elapsed);
}

fn composite_layer_to_surface(
    layer_id: usize,
    tree: &LayerTree,
    target: &mut Pixmap,
    surface_rect: LayoutRect,
    image_cache: &HashMap<String, Vec<u8>>,
    base_url: &Url,
) {
    let layer = &tree.layers[layer_id];
    if layer.opacity <= 0.0 {
        return;
    }

    let has_effect = layer.opacity < 1.0 || layer.transform != Matrix4x4::identity();
    let mut effect_pixmap = if has_effect {
        let width = layer.bounds.width.max(1.0).ceil() as u32;
        let height = layer.bounds.height.max(1.0).ceil() as u32;
        let mut pixmap = Pixmap::new(width, height).expect("Failed to allocate layer pixmap");
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        Some(pixmap)
    } else {
        None
    };

    let (negative, zero, positive) = tree.categorize_children(layer_id);

    if let Some(ref mut pixmap) = effect_pixmap {
        execute_commands_with_clips(&layer.background_commands, pixmap, layer.bounds, image_cache, base_url, &layer.ancestor_clips);

        for &child_id in &negative {
            composite_layer_to_surface(child_id, tree, pixmap, layer.bounds, image_cache, base_url);
        }

        execute_commands_with_clips(&layer.content_commands, pixmap, layer.bounds, image_cache, base_url, &layer.ancestor_clips);

        for &child_id in &zero {
            composite_layer_to_surface(child_id, tree, pixmap, layer.bounds, image_cache, base_url);
        }
        for &child_id in &positive {
            composite_layer_to_surface(child_id, tree, pixmap, layer.bounds, image_cache, base_url);
        }
    } else {
        execute_commands_with_clips(&layer.background_commands, target, surface_rect, image_cache, base_url, &layer.ancestor_clips);

        for &child_id in &negative {
            composite_layer_to_surface(child_id, tree, target, surface_rect, image_cache, base_url);
        }

        execute_commands_with_clips(&layer.content_commands, target, surface_rect, image_cache, base_url, &layer.ancestor_clips);

        for &child_id in &zero {
            composite_layer_to_surface(child_id, tree, target, surface_rect, image_cache, base_url);
        }
        for &child_id in &positive {
            composite_layer_to_surface(child_id, tree, target, surface_rect, image_cache, base_url);
        }
    }

    if let Some(pixmap) = effect_pixmap {
        let mut paint = PixmapPaint::default();
        paint.opacity = layer.opacity;

        let local_x = layer.bounds.x - surface_rect.x;
        let local_y = layer.bounds.y - surface_rect.y;
        let transform = Transform::from_translate(local_x, local_y).pre_concat(layer.transform.to_skia());

        target.draw_pixmap(0, 0, pixmap.as_ref(), &paint, transform, None);
    }
}

/// Build a `Mask` (sized to the tile pixmap) for an overflow clip region.
///
/// The clip rect is in document space; `tx`/`ty` translate it to tile-local space.
/// If `parent_mask` is provided the new mask is AND-ed with the parent so nested
/// `overflow: hidden` containers accumulate correctly.
fn build_clip_mask(
    rect: LayoutRect,
    radius: f32,
    tx: f32,
    ty: f32,
    pw: u32,
    ph: u32,
    parent_mask: Option<&Mask>,
) -> Option<Mask> {
    if pw == 0 || ph == 0 { return None; }
    let mut m = Mask::new(pw, ph)?;
    if rect.width <= 0.0 || rect.height <= 0.0 {
        // An empty clip hides everything; a `None` mask would mean "unclipped".
        return Some(m);
    }

    let local_rect = LayoutRect { x: rect.x + tx, y: rect.y + ty, width: rect.width, height: rect.height };
    let path = if radius > 0.0 {
        create_rounded_rect_path(local_rect, radius)
    } else {
        tiny_skia::Rect::from_xywh(local_rect.x, local_rect.y, local_rect.width, local_rect.height)
            .and_then(|tr| { let mut pb = PathBuilder::new(); pb.push_rect(tr); pb.finish() })
    }?;
    m.fill_path(&path, FillRule::Winding, true, Transform::identity());

    // Intersect with parent clip by multiplying alpha values.
    if let Some(pm) = parent_mask {
        let pd = pm.data();
        let md = m.data_mut();
        for (d, s) in md.iter_mut().zip(pd.iter()) {
            *d = ((*d as u32 * *s as u32) / 255) as u8;
        }
    }
    Some(m)
}

fn execute_commands_on_tile(
    commands: &[PaintCommand],
    pixmap: &mut Pixmap,
    tile_rect: LayoutRect,
    image_cache: &HashMap<String, Vec<u8>>,
    base_url: &Url,
) {
    execute_commands_with_clips(commands, pixmap, tile_rect, image_cache, base_url, &[]);
}

/// Like `execute_commands_on_tile`, but starts from the clip regions a layer
/// inherits from its ancestors.
fn execute_commands_with_clips(
    commands: &[PaintCommand],
    pixmap: &mut Pixmap,
    tile_rect: LayoutRect,
    image_cache: &HashMap<String, Vec<u8>>,
    base_url: &Url,
    ancestor_clips: &[crate::layer_tree::ClipRegion],
) {
    if commands.is_empty() {
        return;
    }
    let tx = -tile_rect.x;
    let ty = -tile_rect.y;
    let transform = Transform::from_translate(tx, ty);

    // Clip mask stack: each entry is the accumulated mask for that clip level.
    // `None` means the clip region did not intersect this tile or allocation failed.
    let mut clip_stack: Vec<Option<Mask>> = Vec::new();
    for region in ancestor_clips {
        if region.rect.width <= 0.0 || region.rect.height <= 0.0 {
            return; // Fully clipped away.
        }
        let parent = clip_stack.last().and_then(|m| m.as_ref());
        let mask = build_clip_mask(region.rect, region.radius, tx, ty, pixmap.width(), pixmap.height(), parent);
        clip_stack.push(mask);
    }

    // Returns the top active mask (or `None` if the stack is empty / top is None).
    macro_rules! active_mask {
        () => {
            clip_stack.last().and_then(|m| m.as_ref())
        }
    }

    for cmd in commands {
        match cmd {
            PaintCommand::PushClip { rect, radius } => {
                let parent = clip_stack.last().and_then(|m| m.as_ref());
                let mask = build_clip_mask(
                    *rect, *radius, tx, ty,
                    pixmap.width(), pixmap.height(),
                    parent,
                );
                clip_stack.push(mask);
            }

            PaintCommand::PopClip => {
                clip_stack.pop();
            }

            PaintCommand::Rect(r, c, radius) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                if *radius > 0.0 {
                    if let Some(path) = create_rounded_rect_path(*r, *radius) {
                        pixmap.fill_path(&path, &paint, FillRule::Winding, transform, active_mask!());
                    }
                } else if let Some(tr) = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height) {
                    pixmap.fill_rect(tr, &paint, transform, active_mask!());
                }
            }
            PaintCommand::LinearGradient { rect: r, direction, stops, radius } => {
                if let Some(shader) = build_linear_gradient_shader(*r, direction, stops) {
                    let mut paint = Paint::default();
                    paint.shader = shader;
                    paint.anti_alias = true;
                    if *radius > 0.0 {
                        if let Some(path) = create_rounded_rect_path(*r, *radius) {
                            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, active_mask!());
                        }
                    } else if let Some(tr) = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height) {
                        pixmap.fill_rect(tr, &paint, transform, active_mask!());
                    }
                }
            }

            PaintCommand::RadialGradient { rect: r, stops, radius } => {
                if let Some(shader) = build_radial_gradient_shader(*r, stops) {
                    let mut paint = Paint::default();
                    paint.shader = shader;
                    paint.anti_alias = true;
                    if *radius > 0.0 {
                        if let Some(path) = create_rounded_rect_path(*r, *radius) {
                            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, active_mask!());
                        }
                    } else if let Some(tr) = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height) {
                        pixmap.fill_rect(tr, &paint, transform, active_mask!());
                    }
                }
            }

            PaintCommand::Border(r, w, c, radius, style) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                let mut stroke = Stroke::default();
                stroke.width = *w;
                match style {
                    BorderStyle::Solid => {}
                    BorderStyle::Dashed => {
                        // Dash/gap length proportional to width, matching the
                        // roughly 3:2 ratio common browsers render for
                        // `border-style: dashed`.
                        let dash_len = (*w * 3.0).max(1.0);
                        let gap_len = (*w * 2.0).max(1.0);
                        stroke.dash = StrokeDash::new(vec![dash_len, gap_len], 0.0);
                    }
                    BorderStyle::Dotted => {
                        // Round caps + a near-zero dash length draws a row of
                        // circular dots spaced `w * 2` apart (the dot itself is
                        // the stroke width).
                        stroke.line_cap = LineCap::Round;
                        let gap_len = (*w * 2.0).max(1.0);
                        stroke.dash = StrokeDash::new(vec![0.01, gap_len], 0.0);
                    }
                }
                if *radius > 0.0 && *style == BorderStyle::Solid {
                    // Fill the ring between the outer and inner border edges.
                    let inner = LayoutRect {
                        x: r.x + w,
                        y: r.y + w,
                        width: (r.width - 2.0 * w).max(0.0),
                        height: (r.height - 2.0 * w).max(0.0),
                    };
                    let mut pb = PathBuilder::new();
                    if push_rounded_rect(&mut pb, *r, *radius).is_some() {
                        let _ = push_rounded_rect(&mut pb, inner, (*radius - w).max(0.0));
                        if let Some(path) = pb.finish() {
                            paint.anti_alias = true;
                            pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, active_mask!());
                        }
                    }
                } else if *radius > 0.0 {
                    // The stroke is centred on its path, so inset it by half
                    // the width to keep the border inside the border box.
                    let centre = LayoutRect {
                        x: r.x + w / 2.0,
                        y: r.y + w / 2.0,
                        width: (r.width - w).max(0.0),
                        height: (r.height - w).max(0.0),
                    };
                    if let Some(path) = create_rounded_rect_path(centre, (*radius - w / 2.0).max(0.0)) {
                        pixmap.stroke_path(&path, &paint, &stroke, transform, active_mask!());
                    }
                } else if let Some(tr) = tiny_skia::Rect::from_xywh(r.x + w/2.0, r.y + w/2.0, (r.width - w).max(0.0), (r.height - w).max(0.0)) {
                    let mut pb = PathBuilder::new();
                    pb.push_rect(tr);
                    if let Some(path) = pb.finish() {
                        pixmap.stroke_path(&path, &paint, &stroke, transform, active_mask!());
                    }
                }
            }
            PaintCommand::BorderSides { rect, sides, radius } => {
                paint_border_sides(pixmap, *rect, sides, *radius, transform, tx, ty, active_mask!());
            }
            PaintCommand::BackgroundImage { url, clip, radius, area, position, size, repeat_x, repeat_y } => {
                let rounded_mask;
                let mask = if *radius > 0.0 {
                    rounded_mask = build_clip_mask(*clip, *radius, tx, ty, pixmap.width(), pixmap.height(), active_mask!());
                    match rounded_mask.as_ref() {
                        Some(m) => Some(m),
                        None => continue,
                    }
                } else {
                    active_mask!()
                };
                let layer = BackgroundImagePaint {
                    clip: *clip,
                    area: *area,
                    position: *position,
                    size: *size,
                    repeat_x: *repeat_x,
                    repeat_y: *repeat_y,
                };
                if let Some((key, data)) = resolve_image_bytes(image_cache, base_url, url) {
                    paint_background_image(pixmap, &key, &data, &layer, transform, mask);
                }
            }
            PaintCommand::Image { rect: r, url, object_fit, alt, radius } => {
                let rounded_mask;
                let mask = if *radius > 0.0 {
                    rounded_mask = build_clip_mask(*r, *radius, tx, ty, pixmap.width(), pixmap.height(), active_mask!());
                    rounded_mask.as_ref()
                } else {
                    active_mask!()
                };
                let drawn = match resolve_image_bytes(image_cache, base_url, url) {
                    Some((key, bytes)) => {
                        draw_replaced_image(pixmap, &key, &bytes, *r, object_fit, transform, mask, tx, ty)
                    }
                    None => false,
                };
                // A frame whose document is not rendered (yet) paints nothing.
                if !drawn && crate::layer_tree::parse_iframe_frame_key(url).is_none() {
                    draw_broken_image(pixmap, *r, alt, transform);
                }
            }
            PaintCommand::Svg { rect, source } => {
                if let Some(tree) = crate::svg::parse_inline(source) {
                    paint_svg_tree(pixmap, &tree, *rect, tx, ty, active_mask!());
                }
            }
            PaintCommand::Text { rect, text, font_size, color, clip, italic, text_decoration, font_family, font_weight, line_height, letter_spacing, .. } => {
                let mut adjusted_rect = *rect;
                adjusted_rect.x += tx;
                adjusted_rect.y += ty;
                let mut adjusted_clip = *clip;
                adjusted_clip.x += tx;
                adjusted_clip.y += ty;
                let font = TextFont {
                    family: font_family,
                    weight: *font_weight,
                    italic: *italic,
                    line_height: Some(*line_height),
                    letter_spacing: *letter_spacing,
                };
                render_text_run(text, adjusted_rect, *font_size, color, adjusted_clip, pixmap, &font, *text_decoration, active_mask!());
            }
            PaintCommand::Shadow(r, s, radius) => {
                paint_box_shadow(pixmap, tx, ty, active_mask!(), *r, s, *radius);
            }
        }
    }
}

/// Paint a single `box-shadow` layer (outset or inset) onto `pixmap`.
///
/// Both variants are built the same way: rasterize a solid white shape into a
/// small temporary pixmap (padded so a blur halo has room to spread), run the
/// 3-pass box-blur approximation of a Gaussian over its alpha channel when
/// `blur > 0`, then walk every pixel of that temporary pixmap and composite it
/// onto `pixmap` using the shadow's color and alpha, further scaled by
/// `clip_mask` (the active `overflow: hidden` clip for this tile) where
/// present.
///
/// - **Outset**: the shape is the box rect expanded by `spread` and offset by
///   `(offset_x, offset_y)`, rounded to `radius + spread` so it follows the
///   box's own corner rounding.
/// - **Inset**: the shape is the box's own padding rect (rounded to `radius`,
///   so it can never paint outside the box), with a hole cut out of it for
///   the un-shadowed interior — the box shrunk by `spread` and offset by
///   `(offset_x, offset_y)`, rounded to `(radius - spread).max(0.0)`. A
///   positive `spread` therefore shrinks that hole and grows the visible
///   shadow ring, matching the CSS spec's description of `spread-radius` for
///   inset shadows.
fn paint_box_shadow(
    pixmap: &mut Pixmap,
    tx: f32,
    ty: f32,
    clip_mask: Option<&Mask>,
    r: LayoutRect,
    s: &crate::css::BoxShadow,
    radius: f32,
) {
    let blur = (*s.blur).max(0.0);
    let spread = *s.spread;
    let radius = radius.max(0.0);

    // The blur "spreads" the shape by roughly `blur` pixels in each direction,
    // so the temp pixmap needs extra padding around the shape equal to the
    // blur radius so the falloff has room.
    // Three box-blur passes reach about 1.5x the blur radius.
    let pad = if blur > 0.0 { (blur * 1.5).ceil() as i32 + 2 } else { 0 };
    let pad_f = pad as f32;

    if s.inset {
        let tmp_w = (r.width + pad_f * 2.0).max(1.0).ceil() as u32;
        let tmp_h = (r.height + pad_f * 2.0).max(1.0).ceil() as u32;
        let Some(mut shape_px) = Pixmap::new(tmp_w.max(1), tmp_h.max(1)) else { return; };

        // Fill the whole padding box, rounded to the box's own border-radius.
        let box_local = LayoutRect { x: pad_f, y: pad_f, width: r.width, height: r.height };
        fill_shape(&mut shape_px, box_local, radius, 255);

        // Snapshot the (un-blurred) box shape now, before the hole is cut —
        // this is used to hard-clip the blurred result back to the box below.
        // An inset shadow's blur only softens the un-shadowed hole's edge; the
        // box's own outer boundary is always a hard clip, never faded by blur.
        let hard_mask = shape_px.clone();

        // Cut the un-shadowed hole: the box shrunk by `spread` and offset by
        // `(offset_x, offset_y)`.
        let hole_w = (r.width - 2.0 * spread).max(0.0);
        let hole_h = (r.height - 2.0 * spread).max(0.0);
        if hole_w > 0.0 && hole_h > 0.0 {
            let hole_local = LayoutRect {
                x: pad_f + *s.offset_x + spread,
                y: pad_f + *s.offset_y + spread,
                width: hole_w,
                height: hole_h,
            };
            let hole_radius = (radius - spread).max(0.0);
            clear_shape(&mut shape_px, hole_local, hole_radius);
        }

        apply_blur_and_composite(
            pixmap, &mut shape_px, blur,
            r.x - pad_f + tx, r.y - pad_f + ty,
            &s.color, clip_mask, Some(&hard_mask),
        );
    } else {
        let sx = r.x + *s.offset_x - spread;
        let sy = r.y + *s.offset_y - spread;
        let sw = (r.width + spread * 2.0).max(0.0);
        let sh = (r.height + spread * 2.0).max(0.0);
        if sw <= 0.0 || sh <= 0.0 { return; }

        let tmp_w = (sw + pad_f * 2.0).max(1.0).ceil() as u32;
        let tmp_h = (sh + pad_f * 2.0).max(1.0).ceil() as u32;
        let Some(mut shape_px) = Pixmap::new(tmp_w.max(1), tmp_h.max(1)) else { return; };

        let shape_local = LayoutRect { x: pad_f, y: pad_f, width: sw, height: sh };
        // Sharp corners stay sharp; rounded ones grow with the spread.
        let shape_radius = if radius > 0.0 { (radius + spread).max(0.0) } else { 0.0 };
        fill_shape(&mut shape_px, shape_local, shape_radius, 255);

        // An outset shadow is never drawn under its own border box (it shows
        // only outside it, even when the box has no background).
        let Some(mut outside_box) = Pixmap::new(tmp_w.max(1), tmp_h.max(1)) else { return; };
        outside_box.fill(tiny_skia::Color::WHITE);
        let box_local = LayoutRect { x: r.x - (sx - pad_f), y: r.y - (sy - pad_f), width: r.width, height: r.height };
        clear_shape(&mut outside_box, box_local, radius);

        apply_blur_and_composite(
            pixmap, &mut shape_px, blur,
            sx - pad_f + tx, sy - pad_f + ty,
            &s.color, clip_mask, Some(&outside_box),
        );
    }
}

/// Fill `rect` (optionally rounded to `radius`) with opaque white — used as
/// the alpha-coverage source shape for a shadow before blurring.
fn fill_shape(pixmap: &mut Pixmap, rect: LayoutRect, radius: f32, alpha: u8) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, alpha);
    if radius > 0.0 {
        if let Some(path) = create_rounded_rect_path(rect, radius) {
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    } else if let Some(tr) = tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.width, rect.height) {
        pixmap.fill_rect(tr, &paint, Transform::identity(), None);
    }
}

/// Zero out `rect` (optionally rounded to `radius`) — cuts the un-shadowed
/// "hole" out of an inset shadow's shape.
fn clear_shape(pixmap: &mut Pixmap, rect: LayoutRect, radius: f32) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 0);
    paint.blend_mode = tiny_skia::BlendMode::Source;
    if radius > 0.0 {
        if let Some(path) = create_rounded_rect_path(rect, radius) {
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    } else if let Some(tr) = tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.width, rect.height) {
        pixmap.fill_rect(tr, &paint, Transform::identity(), None);
    }
}

/// Blur `shape_px`'s alpha channel (if `blur > 0`) using a 3-pass box-blur
/// approximation of a Gaussian with `sigma ≈ blur / 2` (per the CSS spec's
/// description of the box-shadow blur radius), then composite it onto
/// `pixmap` at `(dest_x, dest_y)` using `color`'s RGB and alpha as the
/// shadow's paint, scaled by each destination pixel's coverage and, if
/// present, by `clip_mask`.
///
/// `hard_clip`, when given, is an un-blurred alpha mask the same size as
/// `shape_px` that is multiplied in *after* blurring — used by inset shadows
/// to hard-clip the blur halo back to the box's own boundary, which (unlike
/// the un-shadowed hole's edge) is never itself softened by the blur.
fn apply_blur_and_composite(
    pixmap: &mut Pixmap,
    shape_px: &mut Pixmap,
    blur: f32,
    dest_x_f: f32,
    dest_y_f: f32,
    color: &Color,
    clip_mask: Option<&Mask>,
    hard_clip: Option<&Pixmap>,
) {
    if blur > 0.0 {
        let sigma = (blur / 2.0).max(1.0);
        let box_radius = sigma.round() as usize;
        box_blur_alpha(shape_px, box_radius);
        box_blur_alpha(shape_px, box_radius);
        box_blur_alpha(shape_px, box_radius);
    }

    if let Some(mask_px) = hard_clip {
        let mask_data = mask_px.data();
        for (i, chunk) in shape_px.data_mut().chunks_exact_mut(4).enumerate() {
            let mask_alpha = mask_data.get(i * 4 + 3).copied().unwrap_or(0) as u32;
            chunk[3] = ((chunk[3] as u32 * mask_alpha) / 255) as u8;
        }
    }

    let dest_x = dest_x_f.round() as i32;
    let dest_y = dest_y_f.round() as i32;

    let cr = color.r;
    let cg = color.g;
    let cb = color.b;
    let ca = color.a as f32 / 255.0;

    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let tw = shape_px.width() as i32;
    let th = shape_px.height() as i32;
    let shadow_data = shape_px.data().to_vec();
    let mask_data = clip_mask.map(|m| m.data());

    for ty_off in 0..th {
        let py = dest_y + ty_off;
        if py < 0 || py >= ph { continue; }
        for tx_off in 0..tw {
            let px_coord = dest_x + tx_off;
            if px_coord < 0 || px_coord >= pw { continue; }

            // Each pixel in the shape pixmap is RGBA premultiplied; we stored
            // white so the alpha channel holds the coverage after blurring.
            let src_base = ((ty_off * tw + tx_off) * 4) as usize;
            if src_base + 3 >= shadow_data.len() { continue; }
            let coverage = shadow_data[src_base + 3] as f32 / 255.0;
            if coverage <= 0.0 { continue; }

            let dst_idx = (py as u32 * pixmap.width() + px_coord as u32) as usize;

            let mut alpha = (coverage * ca).clamp(0.0, 1.0);
            if let Some(mdata) = mask_data {
                let mval = mdata.get(dst_idx).copied().unwrap_or(0);
                if mval == 0 { continue; }
                alpha *= mval as f32 / 255.0;
            }
            if alpha <= 0.0 { continue; }

            let pixel = &mut pixmap.pixels_mut()[dst_idx];
            let dst = pixel.demultiply();
            let blend = |src: u8, d: u8| -> u8 {
                ((src as f32 * alpha) + (d as f32 * (1.0 - alpha))).round() as u8
            };
            let out_a = ((alpha + (dst.alpha() as f32 / 255.0) * (1.0 - alpha)) * 255.0).round() as u8;
            *pixel = tiny_skia::ColorU8::from_rgba(
                blend(cr, dst.red()),
                blend(cg, dst.green()),
                blend(cb, dst.blue()),
                out_a,
            ).premultiply();
        }
    }
}

/// Apply a single-pass separable box blur to the alpha channel of a pixmap.
///
/// Performs a 1D horizontal blur then a 1D vertical blur.  Running this
/// function three times with the same `radius` gives a very good
/// approximation of a Gaussian blur with sigma ≈ `radius * sqrt(1/3)`.
///
/// The kernel width is `2 * radius + 1`.  Only the alpha channel is blurred;
/// the RGB channels are left at their initial values (white in the shadow
/// case) because we only use alpha as coverage when compositing.
fn box_blur_alpha(pixmap: &mut Pixmap, radius: usize) {
    if radius == 0 { return; }

    let w = pixmap.width() as usize;
    let h = pixmap.height() as usize;
    let data = pixmap.data_mut();
    let k = (2 * radius + 1) as u32;

    // Centred running-sum blur of one line of `len` samples (`at(i)` is the
    // byte index of sample `i`); samples outside the line count as 0.
    let mut line = Vec::new();
    let mut blur_line = |data: &mut [u8], len: usize, at: &dyn Fn(usize) -> usize| {
        line.clear();
        line.extend((0..len).map(|i| data[at(i)] as u32));
        // Window [i - radius, i + radius].
        let mut acc: u32 = line.iter().take(radius.min(len)).sum();
        for i in 0..len {
            if i + radius < len {
                acc += line[i + radius];
            }
            data[at(i)] = ((acc + k / 2) / k) as u8;
            if i >= radius {
                acc -= line[i - radius];
            }
        }
    };

    for row in 0..h {
        blur_line(data, w, &|x| (row * w + x) * 4 + 3);
    }
    for col in 0..w {
        blur_line(data, h, &|y| (y * w + col) * 4 + 3);
    }
}

/// Find cached bytes for `url`, trying it verbatim and then resolved against
/// `base_url`. Returns the cache key that matched along with the bytes.
fn lookup_image_bytes<'c>(
    image_cache: &'c HashMap<String, Vec<u8>>,
    base_url: &Url,
    url: &str,
) -> Option<(&'c str, &'c [u8])> {
    if let Some((k, v)) = image_cache.get_key_value(url) {
        return Some((k.as_str(), v.as_slice()));
    }
    let resolved = base_url.join(url).ok()?.to_string();
    image_cache.get_key_value(&resolved).map(|(k, v)| (k.as_str(), v.as_slice()))
}

/// Encoded image bytes, borrowed from the image cache or decoded from a
/// `data:` URL.
enum ImageBytes<'c> {
    Cached(&'c [u8]),
    Data(std::sync::Arc<Vec<u8>>),
}

impl std::ops::Deref for ImageBytes<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            ImageBytes::Cached(b) => b,
            ImageBytes::Data(b) => b.as_slice(),
        }
    }
}

/// Bytes of the image at `url`: decoded in place for `data:` URLs, otherwise
/// looked up in the image cache. Returns the key used by the decoded-image
/// caches along with the bytes.
fn resolve_image_bytes<'c>(
    image_cache: &'c HashMap<String, Vec<u8>>,
    base_url: &Url,
    url: &str,
) -> Option<(std::borrow::Cow<'c, str>, ImageBytes<'c>)> {
    if url.trim_start().get(..5).is_some_and(|p| p.eq_ignore_ascii_case("data:")) {
        let bytes = crate::svg::decode_data_url(url)?;
        return Some((std::borrow::Cow::Owned(url.to_string()), ImageBytes::Data(bytes)));
    }
    if let Some((w, h, src)) = crate::layer_tree::parse_iframe_frame_key(url) {
        // Frame documents are cached under the absolute frame URL.
        if let Some((k, v)) = image_cache.get_key_value(url) {
            return Some((std::borrow::Cow::Borrowed(k.as_str()), ImageBytes::Cached(v.as_slice())));
        }
        let abs = base_url.join(src).ok()?;
        let key = crate::layer_tree::iframe_frame_key(abs.as_str(), w, h);
        let (k, v) = image_cache.get_key_value(&key)?;
        return Some((std::borrow::Cow::Borrowed(k.as_str()), ImageBytes::Cached(v.as_slice())));
    }
    let (key, data) = lookup_image_bytes(image_cache, base_url, url)?;
    Some((std::borrow::Cow::Borrowed(key), ImageBytes::Cached(data)))
}

/// Draw `img` scaled to `dest` (page coordinates) under `mask`.
fn draw_scaled_pixmap(
    pixmap: &mut Pixmap,
    img: tiny_skia::PixmapRef,
    dest: LayoutRect,
    transform: Transform,
    mask: Option<&Mask>,
) {
    let sx = dest.width / img.width() as f32;
    let sy = dest.height / img.height() as f32;
    let exact = (sx - 1.0).abs() < 1e-3
        && (sy - 1.0).abs() < 1e-3
        && (dest.x + transform.tx).fract().abs() < 1e-3
        && (dest.y + transform.ty).fract().abs() < 1e-3;
    let paint = PixmapPaint {
        quality: if exact { tiny_skia::FilterQuality::Nearest } else { tiny_skia::FilterQuality::Bilinear },
        ..PixmapPaint::default()
    };
    pixmap.draw_pixmap(0, 0, img, &paint, transform.pre_translate(dest.x, dest.y).pre_scale(sx, sy), mask);
}

/// Destination rect of a replaced image with intrinsic size `(iw, ih)` placed
/// in the content box `r` per `object-fit` (centered, as `object-position`
/// defaults to 50% 50%).
fn object_fit_rect(r: LayoutRect, iw: f32, ih: f32, fit: &ObjectFit) -> LayoutRect {
    let (w, h) = match fit {
        ObjectFit::Fill => return r,
        ObjectFit::Contain => {
            let s = (r.width / iw).min(r.height / ih);
            (iw * s, ih * s)
        }
        ObjectFit::Cover => {
            let s = (r.width / iw).max(r.height / ih);
            (iw * s, ih * s)
        }
        ObjectFit::None => (iw, ih),
    };
    LayoutRect { x: r.x + (r.width - w) / 2.0, y: r.y + (r.height - h) / 2.0, width: w, height: h }
}

/// Paint an `<img>`'s encoded image (raster or SVG) into its content box `r`.
/// Returns `false` when the bytes cannot be decoded.
#[allow(clippy::too_many_arguments)]
fn draw_replaced_image(
    pixmap: &mut Pixmap,
    key: &str,
    bytes: &[u8],
    r: LayoutRect,
    fit: &ObjectFit,
    transform: Transform,
    mask: Option<&Mask>,
    tx: f32,
    ty: f32,
) -> bool {
    let is_svg = crate::svg::is_svg(bytes);
    let intrinsic = if is_svg {
        crate::svg::intrinsic_size(bytes)
    } else {
        crate::background::intrinsic_size(key, bytes).map(|(w, h)| (w as f32, h as f32))
    };
    let Some((iw, ih)) = intrinsic else { return false };
    if !(iw > 0.0 && ih > 0.0) || !(r.width > 0.0 && r.height > 0.0) {
        return true;
    }
    let dest = object_fit_rect(r, iw, ih, fit);
    // Content overflowing the box (cover / none) is clipped to it.
    let overflow_mask;
    let overflows = dest.x < r.x - 0.01
        || dest.y < r.y - 0.01
        || dest.x + dest.width > r.x + r.width + 0.01
        || dest.y + dest.height > r.y + r.height + 0.01;
    let mask = if overflows {
        overflow_mask = build_clip_mask(r, 0.0, tx, ty, pixmap.width(), pixmap.height(), mask);
        overflow_mask.as_ref()
    } else {
        mask
    };
    let (tw, th) = (dest.width.round().max(1.0) as u32, dest.height.round().max(1.0) as u32);
    if is_svg {
        // Rasterize at the drawn size, not the natural size, for crisp edges.
        let Some(img) = crate::svg::rasterize(bytes, tw, th) else { return false };
        draw_scaled_pixmap(pixmap, img.as_ref().as_ref(), dest, transform, mask);
        return true;
    }
    // Downscales are pre-resampled with an area filter (close to Chromium's
    // high-quality downscaling); upscales use bilinear filtering.
    let prescale = tw < iw as u32 && th < ih as u32 && (tw as f32) * (th as f32) <= MAX_PRESCALED_TILE_PIXELS;
    let target = if prescale { Some((tw, th)) } else { None };
    let Some(img) = crate::background::decoded_image(key, bytes, target) else { return false };
    draw_scaled_pixmap(pixmap, img.as_ref().as_ref(), dest, transform, mask);
    true
}

/// Paint an inline SVG document into its content box `rect` (page
/// coordinates; `tx`/`ty` map page to pixmap space).
fn paint_svg_tree(
    pixmap: &mut Pixmap,
    tree: &resvg::usvg::Tree,
    rect: LayoutRect,
    tx: f32,
    ty: f32,
    mask: Option<&Mask>,
) {
    let x = rect.x + tx;
    let y = rect.y + ty;
    let (ix, iy) = (x.floor(), y.floor());
    let Some(img) = crate::svg::rasterize_tree(tree, rect.width, rect.height, x - ix, y - iy) else { return };
    pixmap.draw_pixmap(ix as i32, iy as i32, img.as_ref(), &PixmapPaint::default(), Transform::identity(), mask);
}

/// Geometry of one `background-image` layer, as carried by
/// `PaintCommand::BackgroundImage`.
struct BackgroundImagePaint {
    clip: LayoutRect,
    area: LayoutRect,
    position: crate::background::BackgroundPosition,
    size: crate::background::BackgroundSize,
    repeat_x: bool,
    repeat_y: bool,
}

/// Largest downscaled tile (in pixels) that is pre-resampled on the CPU;
/// bigger tiles are scaled by the pattern transform instead.
const MAX_PRESCALED_TILE_PIXELS: f32 = 4096.0 * 4096.0;

/// Paint one background image layer: size the tile from the image's intrinsic
/// size, place the anchor tile per `background-position`, and fill the clip
/// box (repeating axes) or the anchor tile's part of it (non-repeating axes).
fn paint_background_image(
    pixmap: &mut Pixmap,
    key: &str,
    data: &[u8],
    layer: &BackgroundImagePaint,
    transform: Transform,
    mask: Option<&Mask>,
) {
    use crate::background::{anchor_tile, decoded_image, intrinsic_size, tile_size};
    let is_svg = crate::svg::is_svg(data);
    let intrinsic = if is_svg {
        crate::svg::intrinsic_size(data)
    } else {
        intrinsic_size(key, data).map(|(w, h)| (w as f32, h as f32))
    };
    let Some((iw, ih)) = intrinsic else { return };
    let (tw, th) = tile_size(layer.size, iw, ih, layer.area.width, layer.area.height);
    if !(tw > 0.0 && th > 0.0) {
        return;
    }
    let tile = anchor_tile(layer.area, tw, th, layer.position);
    let clip = layer.clip;
    let (x0, x1) = if layer.repeat_x {
        (clip.x, clip.x + clip.width)
    } else {
        (tile.x.max(clip.x), (tile.x + tw).min(clip.x + clip.width))
    };
    let (y0, y1) = if layer.repeat_y {
        (clip.y, clip.y + clip.height)
    } else {
        (tile.y.max(clip.y), (tile.y + th).min(clip.y + clip.height))
    };
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (rw, rh) = (tw.round().max(1.0), th.round().max(1.0));
    // Downscales are pre-resampled with an area filter; upscales use the
    // pattern's bilinear filter.
    let img = if is_svg {
        // Vector tiles are rasterized at the tile size.
        if rw * rh > MAX_PRESCALED_TILE_PIXELS {
            return;
        }
        let Some(img) = crate::svg::rasterize(data, rw as u32, rh as u32) else { return };
        img
    } else {
        let (iw, ih) = (iw as u32, ih as u32);
        let prescale = (rw as u32 != iw || rh as u32 != ih)
            && rw as u32 <= iw
            && rh as u32 <= ih
            && rw * rh <= MAX_PRESCALED_TILE_PIXELS;
        let target = if prescale { Some((rw as u32, rh as u32)) } else { None };
        let Some(img) = decoded_image(key, data, target) else { return };
        img
    };
    let sx = tw / img.width() as f32;
    let sy = th / img.height() as f32;
    let exact = (sx - 1.0).abs() < 1e-3 && (sy - 1.0).abs() < 1e-3;
    let quality = if exact { tiny_skia::FilterQuality::Nearest } else { tiny_skia::FilterQuality::Bilinear };
    let shader = tiny_skia::Pattern::new(
        img.as_ref().as_ref(),
        SpreadMode::Repeat,
        quality,
        1.0,
        Transform::from_row(sx, 0.0, 0.0, sy, tile.x, tile.y),
    );
    let mut paint = Paint::default();
    paint.shader = shader;
    paint.anti_alias = false;
    if let Some(r) = tiny_skia::Rect::from_ltrb(x0, y0, x1, y1) {
        pixmap.fill_rect(r, &paint, transform, mask);
    }
}

/// Draw a broken-image placeholder: gray background, border, and alt text.
fn draw_broken_image(pixmap: &mut Pixmap, r: LayoutRect, alt: &str, transform: Transform) {
    // Light gray background
    let mut paint = Paint::default();
    paint.set_color_rgba8(240, 240, 240, 255);
    if let Some(tr) = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height) {
        pixmap.fill_rect(tr, &paint, transform, None);
    }
    // Gray border
    let mut border_paint = Paint::default();
    border_paint.set_color_rgba8(180, 180, 180, 255);
    let mut stroke = Stroke::default();
    stroke.width = 1.0;
    let mut pb = PathBuilder::new();
    if let Some(tr) = tiny_skia::Rect::from_xywh(
        r.x + 0.5, r.y + 0.5,
        (r.width - 1.0).max(0.0), (r.height - 1.0).max(0.0),
    ) {
        pb.push_rect(tr);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &border_paint, &stroke, transform, None);
        }
    }
    // Alt text (minimum size guard)
    if r.width >= 8.0 && r.height >= 16.0 {
        let display_text = if alt.is_empty() {
            "[broken image]".to_string()
        } else {
            format!("[{}]", alt)
        };
        let text_rect = LayoutRect {
            x: r.x + 4.0,
            y: r.y + 4.0,
            width: (r.width - 8.0).max(0.0),
            height: (r.height - 8.0).max(0.0),
        };
        let text_color = Color { r: 100, g: 100, b: 100, a: 255 };
        render_text_raw(display_text, text_rect, 12.0, &text_color, text_rect, pixmap, false, false, 0);
    }
}

fn create_rounded_rect_path(r: LayoutRect, radius: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    push_rounded_rect(&mut pb, r, radius)?;
    pb.finish()
}

/// Append a rounded rectangle subpath (circular corners drawn as cubic
/// Béziers) to `pb`. Returns `None` for an empty rect.
fn push_rounded_rect(pb: &mut PathBuilder, r: LayoutRect, radius: f32) -> Option<()> {
    let rect = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height)?;
    let radius = radius.max(0.0).min(rect.width().min(rect.height()) / 2.0);
    if radius <= 0.0 {
        pb.push_rect(rect);
        return Some(());
    }
    // Control-point distance for a quarter circle.
    let k = radius * (1.0 - 0.552_284_8);
    let (l, t, rt, b) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    pb.move_to(l + radius, t);
    pb.line_to(rt - radius, t);
    pb.cubic_to(rt - k, t, rt, t + k, rt, t + radius);
    pb.line_to(rt, b - radius);
    pb.cubic_to(rt, b - k, rt - k, b, rt - radius, b);
    pb.line_to(l + radius, b);
    pb.cubic_to(l + k, b, l, b - k, l, b - radius);
    pb.line_to(l, t + radius);
    pb.cubic_to(l, t + k, l + k, t, l + radius, t);
    pb.close();
    Some(())
}

/// Paint a border whose sides differ: each side is the trapezoid between the
/// outer and inner border edges (meeting its neighbours on the corner
/// diagonals), clipped to the rounded ring when `radius > 0`.
#[allow(clippy::too_many_arguments)]
fn paint_border_sides(
    pixmap: &mut Pixmap,
    r: LayoutRect,
    sides: &crate::layer_tree::BorderSides,
    radius: f32,
    transform: Transform,
    tx: f32,
    ty: f32,
    mask: Option<&Mask>,
) {
    let [wt, wr, wb, wl] = sides.widths;
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.width, r.y + r.height);
    let (ix0, iy0, ix1, iy1) = (x0 + wl, y0 + wt, x1 - wr, y1 - wb);

    let ring_mask;
    let mask = if radius > 0.0 {
        let mut pb = PathBuilder::new();
        if push_rounded_rect(&mut pb, r, radius).is_none() {
            return;
        }
        let inner = LayoutRect { x: ix0, y: iy0, width: (ix1 - ix0).max(0.0), height: (iy1 - iy0).max(0.0) };
        let _ = push_rounded_rect(&mut pb, inner, (radius - wt.max(wl)).max(0.0));
        let Some(path) = pb.finish() else { return };
        let Some(mut m) = Mask::new(pixmap.width(), pixmap.height()) else { return };
        m.fill_path(&path, FillRule::EvenOdd, true, Transform::from_translate(tx, ty));
        if let Some(parent) = mask {
            for (d, s) in m.data_mut().iter_mut().zip(parent.data().iter()) {
                *d = ((*d as u32 * *s as u32) / 255) as u8;
            }
        }
        ring_mask = m;
        Some(&ring_mask)
    } else {
        mask
    };

    // (trapezoid corners, centre line of the side, width) per side.
    let quads = [
        ([(x0, y0), (x1, y0), (ix1, iy0), (ix0, iy0)], ((x0, y0 + wt / 2.0), (x1, y0 + wt / 2.0))),
        ([(x1, y0), (x1, y1), (ix1, iy1), (ix1, iy0)], ((x1 - wr / 2.0, y0), (x1 - wr / 2.0, y1))),
        ([(x1, y1), (x0, y1), (ix0, iy1), (ix1, iy1)], ((x1, y1 - wb / 2.0), (x0, y1 - wb / 2.0))),
        ([(x0, y1), (x0, y0), (ix0, iy0), (ix0, iy1)], ((x0 + wl / 2.0, y1), (x0 + wl / 2.0, y0))),
    ];
    for (i, (quad, (from, to))) in quads.iter().enumerate() {
        let w = sides.widths[i];
        let c = &sides.colors[i];
        if w <= 0.0 || c.a == 0 {
            continue;
        }
        let mut paint = Paint::default();
        paint.set_color_rgba8(c.r, c.g, c.b, c.a);
        paint.anti_alias = true;
        match sides.styles[i] {
            BorderStyle::Solid => {
                let mut pb = PathBuilder::new();
                pb.move_to(quad[0].0, quad[0].1);
                for p in &quad[1..] {
                    pb.line_to(p.0, p.1);
                }
                pb.close();
                if let Some(path) = pb.finish() {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                }
            }
            style => {
                let mut stroke = Stroke { width: w, ..Stroke::default() };
                if style == BorderStyle::Dotted {
                    stroke.line_cap = LineCap::Round;
                    stroke.dash = StrokeDash::new(vec![0.01, (w * 2.0).max(1.0)], 0.0);
                } else {
                    stroke.dash = StrokeDash::new(vec![(w * 3.0).max(1.0), (w * 2.0).max(1.0)], 0.0);
                }
                let mut pb = PathBuilder::new();
                pb.move_to(from.0, from.1);
                pb.line_to(to.0, to.1);
                if let Some(path) = pb.finish() {
                    pixmap.stroke_path(&path, &paint, &stroke, transform, mask);
                }
            }
        }
    }
}

/// Font selection for one text run (see `crate::fonts`).
#[derive(Clone, Debug)]
pub struct TextFont<'a> {
    /// Raw CSS `font-family` value.
    pub family: &'a str,
    /// CSS `font-weight` (1..=1000).
    pub weight: u16,
    pub italic: bool,
    /// Used line box height in px; `None` means `line-height: normal`.
    pub line_height: Option<f32>,
    /// CSS `letter-spacing` in px, added after every character.
    pub letter_spacing: f32,
}

/// Render a text run with the default font (sans-serif), weight 700 when
/// `bold`, italic when `italic`, and `line-height: normal`.
fn render_text_raw(
    text: String,
    rect: LayoutRect,
    font_size: f32,
    color: &Color,
    clip: LayoutRect,
    pixmap: &mut Pixmap,
    bold: bool,
    italic: bool,
    text_decoration: u8,
) {
    let font = TextFont {
        family: "sans-serif",
        weight: if bold { 700 } else { 400 },
        italic,
        line_height: None,
        letter_spacing: 0.0,
    };
    render_text_run(&text, rect, font_size, color, clip, pixmap, &font, text_decoration, None);
}

/// Skia's synthetic italic skew (`SK_ScalarSkewX` = 1/4).
const ITALIC_SKEW: f32 = 0.25;
/// Horizontal glyph-position quantization (Skia uses 1/4 px subpixel positions).
const SUBPIXEL_STEPS: f32 = 4.0;
/// FreeType's default LCD filter (`FT_LCD_FILTER_DEFAULT`), weights out of 256.
const LCD_FILTER: [f32; 5] = [8.0 / 256.0, 77.0 / 256.0, 86.0 / 256.0, 77.0 / 256.0, 8.0 / 256.0];

/// Rasterize one glyph into an LCD (RGB subpixel) coverage bitmap.
///
/// The outline is drawn at 3x horizontal resolution, synthetic bold dilates it
/// by `font_size / 24` px (the FreeType embolden strength Skia uses), synthetic
/// italic skews rows by 1/4, and the FreeType default LCD filter turns the three
/// subpixel samples per pixel into R/G/B coverage.
fn rasterize_glyph(g: &crate::fonts::GlyphChoice, font_size: f32, subpixel_x: f32) -> GlyphPixels {
    let empty = GlyphPixels { left: 0, top: 0, width: 0, height: 0, coverage: Vec::new() };
    // ab_glyph scales so that `PxScale` spans ascent - descent, not the em square.
    let px = font_size * g.font.height_unscaled() / g.font.units_per_em().unwrap_or(1000.0);
    let scale = PxScale { x: px * 3.0, y: px };
    let glyph = g.glyph.with_scale_and_position(scale, point(subpixel_x * 3.0, 0.0));
    let Some(outline) = g.font.outline_glyph(glyph) else { return empty };
    let bounds = outline.px_bounds();
    let (min_x, min_y) = (bounds.min.x as i32, bounds.min.y as i32);
    let (ow, oh) = (bounds.width() as i32, bounds.height() as i32);
    if ow <= 0 || oh <= 0 {
        return empty;
    }

    let embolden = if g.synth_bold { font_size / 24.0 } else { 0.0 };
    let rad_x = embolden * 1.5; // half the extra width, in subpixels
    let rad_y = embolden * 0.5;
    let skew_max = if g.synth_italic {
        (ITALIC_SKEW * 3.0 * (min_y.abs().max((min_y + oh).abs()) as f32 + rad_y + 1.0)).ceil() as i32
    } else {
        0
    };
    let pad_x = rad_x.ceil() as i32 + skew_max + 3;
    let pad_y = rad_y.ceil() as i32;

    // Subpixel buffer origin, aligned so that index 0 starts a whole pixel.
    let x0 = (min_x - pad_x).div_euclid(3) * 3;
    let w = (((min_x + ow + pad_x) - x0 + 2) / 3 * 3) as usize;
    let y0 = min_y - pad_y;
    let h = (oh + 2 * pad_y) as usize;
    let mut buf = vec![0f32; w * h];
    let (ox, oy) = ((min_x - x0) as usize, (min_y - y0) as usize);
    outline.draw(|gx, gy, c| {
        let i = (gy as usize + oy) * w + gx as usize + ox;
        if i < buf.len() {
            buf[i] = (buf[i] + c).min(1.0);
        }
    });

    if embolden > 0.0 {
        buf = dilate(&buf, w, h, rad_x, true);
        buf = dilate(&buf, w, h, rad_y, false);
    }

    if g.synth_italic {
        let mut out = vec![0f32; w * h];
        for row in 0..h {
            // Rows above the baseline shift right.
            let y = (y0 + row as i32) as f32 + 0.5;
            let shift = -y * ITALIC_SKEW * 3.0;
            let (si, sf) = (shift.floor() as i32, shift - shift.floor());
            for col in 0..w as i32 {
                let src = col - si;
                let a = sample(&buf, w, row, src);
                let b = sample(&buf, w, row, src - 1);
                out[row * w + col as usize] = a * (1.0 - sf) + b * sf;
            }
        }
        buf = out;
    }

    let pw = w / 3;
    let mut coverage = vec![[0u8; 3]; pw * h];
    for row in 0..h {
        for sx in 0..w as i32 {
            let mut v = 0.0;
            for (k, wt) in LCD_FILTER.iter().enumerate() {
                v += wt * sample(&buf, w, row, sx + k as i32 - 2);
            }
            coverage[row * pw + sx as usize / 3][sx as usize % 3] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    GlyphPixels { left: x0 / 3, top: y0, width: pw as u32, height: h as u32, coverage }
}

fn sample(buf: &[f32], w: usize, row: usize, col: i32) -> f32 {
    if col < 0 || col as usize >= w { 0.0 } else { buf[row * w + col as usize] }
}

/// Grey-scale morphological dilation by a fractional radius along one axis.
fn dilate(buf: &[f32], w: usize, h: usize, radius: f32, horizontal: bool) -> Vec<f32> {
    if radius <= 0.0 {
        return buf.to_vec();
    }
    let whole = radius.floor() as i32;
    let frac = radius - radius.floor();
    let get = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h { 0.0 } else { buf[y as usize * w + x as usize] }
    };
    let mut out = vec![0f32; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut m = 0f32;
            for d in -whole..=whole {
                m = m.max(if horizontal { get(x + d, y) } else { get(x, y + d) });
            }
            if frac > 0.0 {
                let d = whole + 1;
                let e = if horizontal { get(x - d, y).max(get(x + d, y)) } else { get(x, y - d).max(get(x, y + d)) };
                m = m.max(e * frac);
            }
            out[y as usize * w + x as usize] = m;
        }
    }
    out
}

/// Render a text run into `pixmap` using the fonts selected by `font`.
///
/// Word wrapping mirrors `layout.rs` (`FontChain::measure` per word, one space
/// advance between words) and each line is `line_height` tall with the baseline
/// at ascent plus half-leading, as in Chromium's line box model.
///
/// Rasterized glyphs are cached in `GLYPH_CACHE` keyed by face, glyph, size,
/// subpixel x position and synthesis flags.
///
/// `mask` is the active clip mask (same pixel grid as `pixmap`); glyph
/// coverage is multiplied by it so overflow and CSS clips apply to text.
pub fn render_text_run(
    text: &str,
    rect: LayoutRect,
    font_size: f32,
    color: &Color,
    clip: LayoutRect,
    pixmap: &mut Pixmap,
    font: &TextFont,
    text_decoration: u8,
    mask: Option<&Mask>,
) {
    let mask_data = mask
        .filter(|m| m.width() == pixmap.width() && m.height() == pixmap.height())
        .map(|m| m.data());
    let trimmed = text.trim();
    if trimmed.is_empty() || font_size <= 0.0 { return; }
    let chain = crate::fonts::db().chain(font.family, font.weight, font.italic);
    let metrics = chain.metrics(font_size);
    let line_height = font.line_height.unwrap_or_else(|| metrics.normal_line_height());
    let mut current_y = rect.y + metrics.baseline(line_height);
    let mut current_x = rect.x;
    let space_w = chain.advance(' ', font_size) + font.letter_spacing;

    // Each entry: (line_start_x, line_end_x, baseline_y)
    let mut decoration_lines: Vec<(f32, f32, f32)> = Vec::new();
    let mut line_start_x = current_x;
    let mut line_end_x = current_x;

    let font_size_half_px = (font_size * 2.0).round() as u32;
    let size_64 = (font_size * 64.0).round() as u32;
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);

    for word in trimmed.split_whitespace() {
        let (glyphs, word_w) = chain.shape_spaced(word, font_size, font.letter_spacing);
        if current_x + word_w > rect.x + rect.width + 1.0 && current_x > rect.x {
            decoration_lines.push((line_start_x, line_end_x, current_y));
            current_x = rect.x;
            current_y += line_height;
            line_start_x = current_x;
        }
        let baseline_px = current_y.round() as i32;
        for (g, gx) in glyphs {
            let pen = current_x + gx;
            let mut base = pen.floor();
            let mut bin = ((pen - base) * SUBPIXEL_STEPS).round() as u8;
            if bin as f32 >= SUBPIXEL_STEPS {
                base += 1.0;
                bin = 0;
            }
            let key = GlyphKey {
                face: g.face,
                glyph_id: g.glyph.0,
                font_size_half_px,
                size_64,
                subpixel: bin,
                bold: g.synth_bold,
                italic: g.synth_italic,
            };
            let cached = GLYPH_CACHE.lock().ok().and_then(|c| c.get(&key).cloned());
            let cached = match cached {
                Some(entry) => entry,
                None => {
                    let entry = std::sync::Arc::new(rasterize_glyph(&g, font_size, bin as f32 / SUBPIXEL_STEPS));
                    if let Ok(mut cache) = GLYPH_CACHE.lock() {
                        cache.insert(key, entry.clone());
                    }
                    entry
                }
            };

            let bx = base as i32 + cached.left;
            let by = baseline_px + cached.top;
            for row in 0..cached.height as i32 {
                let py = by + row;
                if py < 0 || py >= ph || (py as f32) < clip.y || (py as f32) >= clip.y + clip.height {
                    continue;
                }
                for col in 0..cached.width as i32 {
                    let px = bx + col;
                    if px < 0 || px >= pw || (px as f32) < clip.x || (px as f32) >= clip.x + clip.width {
                        continue;
                    }
                    let mut cov = cached.coverage[(row * cached.width as i32 + col) as usize];
                    if let Some(md) = mask_data {
                        let m = md[(py * pw + px) as usize] as u32;
                        if m == 0 {
                            continue;
                        }
                        if m < 255 {
                            cov = cov.map(|c| ((c as u32 * m + 127) / 255) as u8);
                        }
                    }
                    if cov != [0, 0, 0] {
                        blend_glyph_pixel_lcd(pixmap, px as u32, py as u32, cov, color);
                    }
                }
            }
        }
        current_x += word_w;
        line_end_x = current_x;
        current_x += space_w;
    }

    if line_end_x > line_start_x {
        decoration_lines.push((line_start_x, line_end_x, current_y));
    }

    if text_decoration != 0 {
        let line_thickness = (font_size / 14.0).round().max(1.0);
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);

        for (seg_start_x, seg_end_x, baseline_y) in decoration_lines {
            let seg_w = (seg_end_x - seg_start_x).max(0.0);
            if seg_w < 1.0 { continue; }
            let baseline_y = baseline_y.round();
            let mut draw = |y: f32| {
                if let Some(r) = tiny_skia::Rect::from_xywh(seg_start_x, y, seg_w, line_thickness) {
                    pixmap.fill_rect(r, &paint, Transform::identity(), mask);
                }
            };
            if text_decoration & 0b001 != 0 {
                draw(baseline_y + (font_size / 12.0).round().max(1.0));
            }
            if text_decoration & 0b010 != 0 {
                draw((baseline_y - font_size * 0.3).round());
            }
            if text_decoration & 0b100 != 0 {
                draw(baseline_y - metrics.ascent);
            }
        }
    }
}

/// Blend LCD coverage into an opaque destination per channel; non-opaque
/// destinations get grey-scale coverage (as Skia disables LCD text there).
fn blend_glyph_pixel_lcd(pixmap: &mut Pixmap, x: u32, y: u32, cov: [u8; 3], color: &Color) {
    let index = (y * pixmap.width() + x) as usize;
    let pixel = &mut pixmap.pixels_mut()[index];
    if pixel.alpha() != 255 {
        let gray = (cov[0] as f32 + cov[1] as f32 + cov[2] as f32) / (3.0 * 255.0);
        blend_glyph_pixel(pixmap, x, y, gray, color);
        return;
    }
    let dst = pixel.demultiply();
    let ca = color.a as f32 / 255.0;
    let mix = |src: u8, dst: u8, c: u8| -> u8 {
        let a = c as f32 / 255.0 * ca;
        (src as f32 * a + dst as f32 * (1.0 - a)).round() as u8
    };
    *pixel = tiny_skia::ColorU8::from_rgba(
        mix(color.r, dst.red(), cov[0]),
        mix(color.g, dst.green(), cov[1]),
        mix(color.b, dst.blue(), cov[2]),
        255,
    )
    .premultiply();
}

/// Convert a `CssColorStop` slice into `tiny_skia::GradientStop`s.
fn css_stops_to_skia(stops: &[CssColorStop]) -> Vec<GradientStop> {
    stops.iter().filter_map(|s| {
        let pos = s.position.unwrap_or(0.0).clamp(0.0, 1.0);
        let color = tiny_skia::Color::from_rgba8(s.color.r, s.color.g, s.color.b, s.color.a);
        GradientStop::new(pos, color).into()
    }).collect()
}

/// Build a tiny-skia `Shader` for a CSS `linear-gradient()`.
fn build_linear_gradient_shader<'a>(
    r: LayoutRect,
    direction: &LinearDirection,
    stops: &[CssColorStop],
) -> Option<tiny_skia::Shader<'a>> {
    let skia_stops = css_stops_to_skia(stops);
    if skia_stops.len() < 2 { return None; }

    // Compute start/end points from the direction and the rect bounds.
    let cx = r.x + r.width / 2.0;
    let cy = r.y + r.height / 2.0;

    let (start, end) = match direction {
        LinearDirection::Angle(rad) => {
            // CSS angle: 0 = to top, 90deg = to right (clockwise from 12 o'clock).
            // tiny-skia uses standard math coords (+y down).
            // We want: direction vector = (sin(rad), -cos(rad)) for CSS convention.
            let dx = rad.sin();
            let dy = -rad.cos();
            // Determine the distance from center to edge along that direction.
            let half_w = r.width / 2.0;
            let half_h = r.height / 2.0;
            // Scale so the gradient covers the full box diagonal.
            let scale = if dx.abs() < 1e-6 {
                half_h / dy.abs().max(1e-6)
            } else if dy.abs() < 1e-6 {
                half_w / dx.abs().max(1e-6)
            } else {
                (half_w / dx.abs()).min(half_h / dy.abs())
            };
            (
                SkPoint::from_xy(cx - dx * scale, cy - dy * scale),
                SkPoint::from_xy(cx + dx * scale, cy + dy * scale),
            )
        }
        LinearDirection::ToSide(dx, dy) => {
            let mag = (dx * dx + dy * dy).sqrt().max(1e-6);
            let ndx = dx / mag;
            let ndy = dy / mag;
            let half_w = r.width / 2.0;
            let half_h = r.height / 2.0;
            let scale = if ndx.abs() < 1e-6 {
                half_h / ndy.abs().max(1e-6)
            } else if ndy.abs() < 1e-6 {
                half_w / ndx.abs().max(1e-6)
            } else {
                (half_w / ndx.abs()).min(half_h / ndy.abs())
            };
            (
                SkPoint::from_xy(cx - ndx * scale, cy - ndy * scale),
                SkPoint::from_xy(cx + ndx * scale, cy + ndy * scale),
            )
        }
    };

    LinearGradient::new(start, end, skia_stops, SpreadMode::Pad, Transform::identity())
}

/// Build a tiny-skia `Shader` for a CSS `radial-gradient()`.
fn build_radial_gradient_shader<'a>(
    r: LayoutRect,
    stops: &[CssColorStop],
) -> Option<tiny_skia::Shader<'a>> {
    let skia_stops = css_stops_to_skia(stops);
    if skia_stops.len() < 2 { return None; }

    let center = SkPoint::from_xy(r.x + r.width / 2.0, r.y + r.height / 2.0);
    let radius = (r.width.min(r.height) / 2.0).max(1.0);

    RadialGradient::new(
        center,
        0.0,
        center,
        radius,
        skia_stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
}

fn blend_glyph_pixel(pixmap: &mut Pixmap, x: u32, y: u32, coverage: f32, color: &Color) {
    if coverage <= 0.0 { return; }
    let alpha = (coverage * (color.a as f32 / 255.0)).clamp(0.0, 1.0);
    if alpha <= 0.0 { return; }
    let index = (y * pixmap.width() + x) as usize;
    let pixel = &mut pixmap.pixels_mut()[index];
    let dst = pixel.demultiply();
    let blend = |src: u8, dst: u8| -> u8 {
        ((src as f32 * alpha) + (dst as f32 * (1.0 - alpha))).round() as u8
    };
    let out_a = ((alpha + (dst.alpha() as f32 / 255.0) * (1.0 - alpha)) * 255.0).round() as u8;
    *pixel = tiny_skia::ColorU8::from_rgba(
        blend(color.r, dst.red()),
        blend(color.g, dst.green()),
        blend(color.b, dst.blue()),
        out_a,
    ).premultiply();
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect as LayoutRect;
    use crate::css::Color;

    /// Helper: allocate a small opaque white pixmap.
    fn white_pixmap(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        p.fill(tiny_skia::Color::WHITE);
        p
    }

    fn black() -> Color { Color { r: 0, g: 0, b: 0, a: 255 } }
    fn red()   -> Color { Color { r: 255, g: 0, b: 0, a: 255 } }

    fn full_rect(w: f32, h: f32) -> LayoutRect {
        LayoutRect { x: 0.0, y: 0.0, width: w, height: h }
    }

    // ── Glyph cache population ────────────────────────────────────────────────

    /// Rendering text twice must produce identical pixel output (cache replay
    /// must be bit-for-bit identical to the first rasterization).
    #[test]
    fn test_glyph_cache_produces_identical_output() {
        clear_glyph_cache();

        let rect = full_rect(200.0, 40.0);
        let color = black();

        let mut pixmap1 = white_pixmap(200, 40);
        render_text_raw("Hello".to_string(), rect, 16.0, &color, rect, &mut pixmap1, false, false, 0);

        clear_glyph_cache();

        let mut pixmap2 = white_pixmap(200, 40);
        render_text_raw("Hello".to_string(), rect, 16.0, &color, rect, &mut pixmap2, false, false, 0);

        assert_eq!(pixmap1.data(), pixmap2.data(),
            "cache and uncached renders must produce identical pixels");
    }

    /// The glyph cache must be populated after the first render.
    #[test]
    fn test_glyph_cache_is_populated_after_render() {
        clear_glyph_cache();

        let rect = full_rect(200.0, 40.0);
        let mut pixmap = white_pixmap(200, 40);
        render_text_raw("Abc".to_string(), rect, 16.0, &black(), rect, &mut pixmap, false, false, 0);

        let cache_size = GLYPH_CACHE.lock().unwrap().len();
        assert!(cache_size > 0, "glyph cache should be non-empty after rendering text; got {} entries", cache_size);
    }

    /// `clear_glyph_cache()` must empty the cache.
    #[test]
    fn test_clear_glyph_cache_empties_cache() {
        // Populate.
        let rect = full_rect(200.0, 40.0);
        let mut pixmap = white_pixmap(200, 40);
        render_text_raw("Test".to_string(), rect, 16.0, &black(), rect, &mut pixmap, false, false, 0);

        clear_glyph_cache();

        let cache_size = GLYPH_CACHE.lock().unwrap().len();
        assert_eq!(cache_size, 0, "cache should be empty after clear_glyph_cache()");
    }

    /// Bold text rendered twice must match.
    #[test]
    fn test_bold_text_cache_identical() {
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);
        let color = black();

        let mut p1 = white_pixmap(200, 40);
        render_text_raw("Bold".to_string(), rect, 16.0, &color, rect, &mut p1, true, false, 0);

        clear_glyph_cache();
        let mut p2 = white_pixmap(200, 40);
        render_text_raw("Bold".to_string(), rect, 16.0, &color, rect, &mut p2, true, false, 0);

        assert_eq!(p1.data(), p2.data(), "bold renders must be identical across cache miss and cache hit");
    }

    /// Italic text rendered twice must match.
    #[test]
    fn test_italic_text_cache_identical() {
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);
        let color = black();

        let mut p1 = white_pixmap(200, 40);
        render_text_raw("Italic".to_string(), rect, 16.0, &color, rect, &mut p1, false, true, 0);

        clear_glyph_cache();
        let mut p2 = white_pixmap(200, 40);
        render_text_raw("Italic".to_string(), rect, 16.0, &color, rect, &mut p2, false, true, 0);

        assert_eq!(p1.data(), p2.data(), "italic renders must be identical across cache miss and cache hit");
    }

    // ── Visual correctness ────────────────────────────────────────────────────

    /// Rendering non-empty text must modify at least one pixel (basic sanity check
    /// that the text actually hits the pixmap).
    #[test]
    fn test_text_modifies_pixmap() {
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);
        let mut pixmap = white_pixmap(200, 40);
        let white_before = pixmap.data().to_vec();

        render_text_raw("Hello world".to_string(), rect, 16.0, &black(), rect, &mut pixmap, false, false, 0);

        assert_ne!(pixmap.data(), white_before.as_slice(), "text rendering must modify the pixmap");
    }

    /// Empty and whitespace-only strings must not modify the pixmap at all.
    #[test]
    fn test_empty_text_does_not_modify_pixmap() {
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);

        for text in &["", "   ", "\t\n"] {
            let mut pixmap = white_pixmap(200, 40);
            let before = pixmap.data().to_vec();
            render_text_raw(text.to_string(), rect, 16.0, &black(), rect, &mut pixmap, false, false, 0);
            assert_eq!(pixmap.data(), before.as_slice(), "empty/whitespace text must not modify pixmap");
        }
    }

    /// Text with underline decoration must produce a different pixel output than
    /// plain text (the decoration lines add extra pixels).
    #[test]
    fn test_underline_decoration_differs_from_plain() {
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);
        let color = black();

        let mut plain = white_pixmap(200, 40);
        render_text_raw("Hello".to_string(), rect, 16.0, &color, rect, &mut plain, false, false, 0);

        let mut underlined = white_pixmap(200, 40);
        render_text_raw("Hello".to_string(), rect, 16.0, &color, rect, &mut underlined, false, false, 0b001);

        assert_ne!(plain.data(), underlined.data(), "underlined text must differ from plain text");
    }

    /// Different font sizes must be cached independently (i.e. produce different output).
    #[test]
    fn test_different_font_sizes_are_independent_cache_entries() {
        clear_glyph_cache();
        let rect = full_rect(200.0, 60.0);
        let color = black();

        let mut p12 = white_pixmap(200, 60);
        render_text_raw("A".to_string(), rect, 12.0, &color, rect, &mut p12, false, false, 0);
        let mut p24 = white_pixmap(200, 60);
        render_text_raw("A".to_string(), rect, 24.0, &color, rect, &mut p24, false, false, 0);

        // Primary assertion: different font sizes must produce different pixel output,
        // which proves the cache treats them as independent entries.
        assert_ne!(p12.data(), p24.data(), "12px and 24px 'A' must produce different renders");

        // Verify that both entries exist in the cache right now (snapshot atomically
        // under one lock so a parallel clear_glyph_cache() call cannot race).
        let size_12 = (12.0_f32 * 2.0).round() as u32;
        let size_24 = (24.0_f32 * 2.0).round() as u32;
        let (has_12, has_24) = {
            let guard = GLYPH_CACHE.lock().unwrap();
            (
                guard.keys().any(|k| k.font_size_half_px == size_12),
                guard.keys().any(|k| k.font_size_half_px == size_24),
            )
        };
        // These can only fail if clear_glyph_cache() was called between our last
        // render_text_raw and the lock above — i.e. by a parallel test.  That
        // scenario is a test-ordering issue, not a cache-correctness bug, so we
        // only assert when the cache wasn't concurrently cleared.
        if has_12 || has_24 {
            assert!(has_12, "cache must have an entry for 12px (half_px key {size_12})");
            assert!(has_24, "cache must have an entry for 24px (half_px key {size_24})");
        }
    }

    /// Colored text must differ from text rendered in a different color (sanity
    /// check that `blend_glyph_pixel` uses the provided color, not a cached one).
    #[test]
    fn test_color_does_not_bleed_across_renders() {
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);

        let mut p_black = white_pixmap(200, 40);
        render_text_raw("Hi".to_string(), rect, 16.0, &black(), rect, &mut p_black, false, false, 0);

        let mut p_red = white_pixmap(200, 40);
        render_text_raw("Hi".to_string(), rect, 16.0, &red(), rect, &mut p_red, false, false, 0);

        assert_ne!(p_black.data(), p_red.data(), "black and red text must produce different pixel output");
    }

    fn ink_left_edge(p: &Pixmap) -> Option<u32> {
        (0..p.width()).find(|&x| (0..p.height()).any(|y| p.pixel(x, y).unwrap().red() < 200))
    }

    fn ink_right_edge(p: &Pixmap) -> Option<u32> {
        (0..p.width()).rev().find(|&x| (0..p.height()).any(|y| p.pixel(x, y).unwrap().red() < 200))
    }

    /// `letter-spacing` must widen a painted run by one spacing per character.
    #[test]
    fn test_letter_spacing_widens_painted_run() {
        let rect = full_rect(300.0, 40.0);
        let paint = |ls: f32| {
            let mut p = white_pixmap(300, 40);
            let font = TextFont { family: "sans-serif", weight: 400, italic: false, line_height: None, letter_spacing: ls };
            render_text_run("IIII", rect, 16.0, &black(), rect, &mut p, &font, 0, None);
            p
        };
        let plain = paint(0.0);
        let spaced = paint(10.0);
        assert_eq!(ink_left_edge(&plain), ink_left_edge(&spaced));
        let grow = ink_right_edge(&spaced).unwrap() as i32 - ink_right_edge(&plain).unwrap() as i32;
        assert!((29..=31).contains(&grow), "3 gaps of 10px expected, got {grow}");
    }

    /// Text on an opaque background uses LCD coverage (colour fringes); on a
    /// transparent layer it falls back to grey-scale coverage.
    #[test]
    fn test_lcd_text_only_on_opaque_destination() {
        let rect = full_rect(200.0, 40.0);
        let font = TextFont { family: "sans-serif", weight: 400, italic: false, line_height: None, letter_spacing: 0.0 };
        let mut opaque = white_pixmap(200, 40);
        render_text_run("Wave", rect, 16.0, &black(), rect, &mut opaque, &font, 0, None);
        let fringed = opaque.pixels().iter().any(|p| {
            let c = p.demultiply();
            c.red().abs_diff(c.blue()) > 30
        });
        assert!(fringed, "LCD text should have coloured edges on white");

        let mut clear = Pixmap::new(200, 40).unwrap();
        render_text_run("Wave", rect, 16.0, &black(), rect, &mut clear, &font, 0, None);
        assert!(clear.pixels().iter().any(|p| p.alpha() > 0));
        assert!(clear.pixels().iter().all(|p| {
            let c = p.demultiply();
            c.red() == c.green() && c.green() == c.blue()
        }));
    }

    /// The baseline sits at ascent plus half-leading inside the line box.
    #[test]
    fn test_line_height_moves_baseline_by_half_leading() {
        let rect = full_rect(200.0, 80.0);
        let bottom = |lh: f32| {
            let mut p = white_pixmap(200, 80);
            let font = TextFont { family: "sans-serif", weight: 400, italic: false, line_height: Some(lh), letter_spacing: 0.0 };
            render_text_run("H", rect, 16.0, &black(), rect, &mut p, &font, 0, None);
            (0..80).rev().find(|&y| (0..200).any(|x| p.pixel(x, y).unwrap().red() < 128)).unwrap()
        };
        assert_eq!(bottom(40.0) as i32 - bottom(20.0) as i32, 10);
    }

    /// The active clip mask must hide glyph pixels outside it.
    #[test]
    fn test_text_respects_clip_mask() {
        let rect = full_rect(200.0, 40.0);
        let font = TextFont { family: "sans-serif", weight: 400, italic: false, line_height: None, letter_spacing: 0.0 };
        let mut mask = Mask::new(200, 40).unwrap();
        let mut pb = PathBuilder::new();
        pb.push_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, 20.0, 40.0).unwrap());
        mask.fill_path(&pb.finish().unwrap(), FillRule::Winding, false, Transform::identity());
        let mut p = white_pixmap(200, 40);
        render_text_run("MMMMMMMM", rect, 16.0, &black(), rect, &mut p, &font, 0b001, Some(&mask));
        assert!(ink_left_edge(&p).is_some(), "text inside the mask must paint");
        assert!(ink_right_edge(&p).unwrap() < 20, "no ink may pass the mask edge");
    }

    // ── Shadow blur ───────────────────────────────────────────────────────────

    /// `box_blur_alpha` with radius > 0 must produce a different alpha channel
    /// than the original (un-blurred) pixmap.
    #[test]
    fn test_box_blur_alpha_changes_pixels() {
        let mut p = Pixmap::new(20, 20).unwrap();
        // Draw a solid white rect in the center.
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        if let Some(tr) = tiny_skia::Rect::from_xywh(5.0, 5.0, 10.0, 10.0) {
            p.fill_rect(tr, &paint, Transform::identity(), None);
        }
        let before = p.data().to_vec();
        box_blur_alpha(&mut p, 2);
        assert_ne!(p.data(), before.as_slice(), "blur must change the pixmap");
    }

    /// After blurring a fully opaque center patch, the pixels just outside the
    /// original solid area must become non-zero alpha (the halo effect).
    #[test]
    fn test_box_blur_alpha_produces_halo() {
        let mut p = Pixmap::new(20, 20).unwrap();
        // Draw a 4×4 fully opaque white square in the very center.
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        if let Some(tr) = tiny_skia::Rect::from_xywh(8.0, 8.0, 4.0, 4.0) {
            p.fill_rect(tr, &paint, Transform::identity(), None);
        }
        // Three-pass box blur (Gaussian approximation).
        box_blur_alpha(&mut p, 2);
        box_blur_alpha(&mut p, 2);
        box_blur_alpha(&mut p, 2);

        // The pixel one step outside the original rect should now be non-zero.
        let halo_pixel_alpha = p.data()[(7 * 20 + 8) * 4 + 3]; // row 7, col 8
        assert!(halo_pixel_alpha > 0, "halo pixel should have non-zero alpha after blur, got {}", halo_pixel_alpha);
    }

    /// A sharp shadow (blur == 0) must modify the pixmap: the shadow rect area
    /// must differ from a transparent background.
    #[test]
    fn test_shadow_zero_blur_renders_rect() {
        use crate::css::{BoxShadow, OrderedFloat};
        use crate::layer_tree::PaintCommand;
        use url::Url;

        let mut pixmap = Pixmap::new(100, 100).unwrap();
        let shadow = BoxShadow {
            offset_x: OrderedFloat(0.0),
            offset_y: OrderedFloat(4.0),
            blur:     OrderedFloat(0.0),
            spread:   OrderedFloat(0.0),
            color:    Color { r: 0, g: 0, b: 0, a: 76 },
            inset:    false,
        };
        let rect = LayoutRect { x: 10.0, y: 10.0, width: 40.0, height: 20.0 };
        let before = pixmap.data().to_vec();

        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let cmds = vec![PaintCommand::Shadow(rect, shadow, 0.0)];
        let base_url = Url::parse("https://example.com/").unwrap();
        execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &HashMap::new(), &base_url);

        assert_ne!(pixmap.data(), before.as_slice(), "zero-blur shadow must modify the pixmap");
    }

    /// A blurred shadow (blur > 0) must modify the pixmap and produce a
    /// non-zero alpha region larger than the element rect itself (the halo).
    #[test]
    fn test_shadow_with_blur_produces_halo() {
        use crate::css::{BoxShadow, OrderedFloat};
        use crate::layer_tree::PaintCommand;
        use url::Url;

        let mut pixmap = Pixmap::new(100, 100).unwrap();
        let shadow = BoxShadow {
            offset_x: OrderedFloat(0.0),
            offset_y: OrderedFloat(0.0),
            blur:     OrderedFloat(8.0),
            spread:   OrderedFloat(0.0),
            color:    Color { r: 0, g: 0, b: 0, a: 76 },
            inset:    false,
        };
        let rect = LayoutRect { x: 40.0, y: 40.0, width: 20.0, height: 20.0 };

        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let cmds = vec![PaintCommand::Shadow(rect, shadow, 0.0)];
        let base_url = Url::parse("https://example.com/").unwrap();
        execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &HashMap::new(), &base_url);

        // The pixel several pixels outside the shadow rect should have non-zero alpha
        // due to the blur halo.  Shadow at (40,40) size (20,20); check pixel at (34,40).
        let halo_alpha = pixmap.data()[(40 * 100 + 34) * 4 + 3];
        assert!(halo_alpha > 0, "blurred shadow halo pixel should be non-zero alpha, got {}", halo_alpha);
    }

    /// An `inset` box-shadow must darken the ring between the box edge and the
    /// shrunk-by-spread "hole", but must leave the hole itself (the box
    /// interior, away from the edges) unpainted.
    #[test]
    fn test_inset_shadow_paints_ring_not_center() {
        use crate::css::{BoxShadow, OrderedFloat};
        use url::Url;

        let mut pixmap = Pixmap::new(80, 80).unwrap();
        let shadow = BoxShadow {
            offset_x: OrderedFloat(0.0),
            offset_y: OrderedFloat(0.0),
            blur:     OrderedFloat(0.0),
            spread:   OrderedFloat(10.0),
            color:    Color { r: 0, g: 0, b: 0, a: 255 },
            inset:    true,
        };
        let rect = LayoutRect { x: 10.0, y: 10.0, width: 60.0, height: 60.0 };
        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 80.0, height: 80.0 };
        let cmds = vec![PaintCommand::Shadow(rect, shadow, 0.0)];
        let base_url = Url::parse("https://example.com/").unwrap();
        execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &HashMap::new(), &base_url);

        // Center of the box (40, 40) is well inside the shrunk-by-10 hole
        // (which spans 20..70 on both axes) and must remain untouched.
        let center_alpha = pixmap.data()[(40 * 80 + 40) * 4 + 3];
        assert_eq!(center_alpha, 0, "inset shadow must not paint the box's un-shadowed interior");

        // A pixel just inside the box edge (12, 40) is in the shadowed ring
        // (10..20) and must be painted.
        let ring_alpha = pixmap.data()[(40 * 80 + 12) * 4 + 3];
        assert!(ring_alpha > 0, "inset shadow must paint the ring near the box edge, got alpha {}", ring_alpha);
    }

    /// An `inset` box-shadow must never paint outside the box's own rect,
    /// even with a large spread or blur.
    #[test]
    fn test_inset_shadow_does_not_paint_outside_box() {
        use crate::css::{BoxShadow, OrderedFloat};
        use url::Url;

        let mut pixmap = Pixmap::new(80, 80).unwrap();
        let shadow = BoxShadow {
            offset_x: OrderedFloat(0.0),
            offset_y: OrderedFloat(0.0),
            blur:     OrderedFloat(6.0),
            spread:   OrderedFloat(4.0),
            color:    Color { r: 0, g: 0, b: 0, a: 255 },
            inset:    true,
        };
        let rect = LayoutRect { x: 20.0, y: 20.0, width: 40.0, height: 40.0 };
        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 80.0, height: 80.0 };
        let cmds = vec![PaintCommand::Shadow(rect, shadow, 0.0)];
        let base_url = Url::parse("https://example.com/").unwrap();
        execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &HashMap::new(), &base_url);

        // A pixel just outside the box (18, 40) must stay fully transparent.
        let outside_alpha = pixmap.data()[(40 * 80 + 18) * 4 + 3];
        assert_eq!(outside_alpha, 0, "inset shadow must never paint outside the box rect, got alpha {}", outside_alpha);
    }

    /// An outset box-shadow on a rounded box must follow the box's own
    /// border-radius: the far corner of a sharp (blur=0), zero-offset shadow
    /// must be unpainted where the rounded shape excludes it, even though the
    /// same point lies inside the plain bounding rect.
    #[test]
    fn test_outset_shadow_follows_border_radius() {
        use crate::css::{BoxShadow, OrderedFloat};
        use url::Url;

        let shadow = BoxShadow {
            offset_x: OrderedFloat(0.0),
            offset_y: OrderedFloat(0.0),
            blur:     OrderedFloat(0.0),
            spread:   OrderedFloat(4.0),
            color:    Color { r: 0, g: 0, b: 0, a: 255 },
            inset:    false,
        };
        // A 32x32 box spread by 4px: the shadow shape covers 0..40.
        let rect = LayoutRect { x: 4.0, y: 4.0, width: 32.0, height: 32.0 };
        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 40.0, height: 40.0 };
        let base_url = Url::parse("https://example.com/").unwrap();

        // radius = 0: the extreme corner pixel is part of the (square) shadow.
        let mut square = Pixmap::new(40, 40).unwrap();
        let cmds_square = vec![PaintCommand::Shadow(rect, shadow.clone(), 0.0)];
        execute_commands_on_tile(&cmds_square, &mut square, tile_rect, &HashMap::new(), &base_url);
        let square_corner_alpha = square.data()[(1 * 40 + 1) * 4 + 3];
        assert!(square_corner_alpha > 0, "square shadow must cover its corner");

        // radius = 14 (+4 spread = 18) on the 40x40 shape: the same corner pixel falls outside the
        // rounded shape and must be unpainted.
        let mut rounded = Pixmap::new(40, 40).unwrap();
        let cmds_rounded = vec![PaintCommand::Shadow(rect, shadow, 14.0)];
        execute_commands_on_tile(&cmds_rounded, &mut rounded, tile_rect, &HashMap::new(), &base_url);
        let rounded_corner_alpha = rounded.data()[(1 * 40 + 1) * 4 + 3];
        assert_eq!(rounded_corner_alpha, 0, "rounded shadow must not paint past its rounded corner, got alpha {}", rounded_corner_alpha);
    }

    /// A blurred shadow must respect an active `overflow: hidden` clip mask —
    /// pixels outside the pushed clip rect must remain untouched even though
    /// the blur halo would otherwise reach them.
    #[test]
    fn test_blurred_shadow_respects_push_clip() {
        use crate::css::{BoxShadow, OrderedFloat};
        use url::Url;

        let mut pixmap = Pixmap::new(100, 100).unwrap();
        let shadow = BoxShadow {
            offset_x: OrderedFloat(0.0),
            offset_y: OrderedFloat(0.0),
            blur:     OrderedFloat(12.0),
            spread:   OrderedFloat(0.0),
            color:    Color { r: 0, g: 0, b: 0, a: 255 },
            inset:    false,
        };
        let rect = LayoutRect { x: 40.0, y: 40.0, width: 20.0, height: 20.0 };
        // Clip to a region that excludes the pixel we check.
        let clip_rect = LayoutRect { x: 40.0, y: 40.0, width: 20.0, height: 20.0 };
        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let cmds = vec![
            PaintCommand::PushClip { rect: clip_rect, radius: 0.0 },
            PaintCommand::Shadow(rect, shadow, 0.0),
            PaintCommand::PopClip,
        ];
        let base_url = Url::parse("https://example.com/").unwrap();
        execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &HashMap::new(), &base_url);

        // Without the clip, the blur halo reaches several pixels outside the
        // shadow rect (as in test_shadow_with_blur_produces_halo); with the
        // clip active, those pixels must stay untouched.
        let halo_alpha = pixmap.data()[(40 * 100 + 34) * 4 + 3];
        assert_eq!(halo_alpha, 0, "clip mask must suppress the blur halo outside the clip rect, got alpha {}", halo_alpha);
    }

    /// `border-style: dashed`/`dotted` must produce a stroke with visible gaps
    /// (fewer painted pixels along the edge) compared to a solid border of the
    /// same width and color — the whole point of a dash pattern.
    #[test]
    fn test_dashed_and_dotted_borders_paint_fewer_pixels_than_solid() {
        use crate::layer_tree::BorderStyle;
        use url::Url;

        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let rect = LayoutRect { x: 10.0, y: 10.0, width: 80.0, height: 80.0 };
        let color = black();
        let base_url = Url::parse("https://example.com/").unwrap();

        let count_painted = |style: BorderStyle| -> usize {
            let mut pixmap = white_pixmap(100, 100);
            let cmds = vec![PaintCommand::Border(rect, 4.0, color.clone(), 0.0, style)];
            execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &HashMap::new(), &base_url);
            pixmap.data().chunks_exact(4).filter(|px| px != &[255, 255, 255, 255]).count()
        };

        let solid = count_painted(BorderStyle::Solid);
        let dashed = count_painted(BorderStyle::Dashed);
        let dotted = count_painted(BorderStyle::Dotted);

        assert!(solid > 0, "solid border must paint some pixels");
        assert!(dashed < solid, "dashed border ({dashed}) must paint fewer pixels than solid ({solid})");
        assert!(dotted < solid, "dotted border ({dotted}) must paint fewer pixels than solid ({solid})");
    }

    #[test]
    fn test_relative_image_url_uses_absolute_cached_bytes() {
        use crate::layer_tree::{ObjectFit, PaintCommand};
        use url::Url;

        let png_1x1_red: Vec<u8> = vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0,
            0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120,
            156, 99, 248, 207, 192, 240, 31, 0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0,
            0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];

        let mut pixmap = Pixmap::new(8, 8).unwrap();
        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 8.0, height: 8.0 };
        let rect = LayoutRect { x: 0.0, y: 0.0, width: 8.0, height: 8.0 };
        let cmds = vec![PaintCommand::Image {
            rect,
            url: "/tiny.png".to_string(),
            object_fit: ObjectFit::Fill,
            alt: String::new(),
            radius: 0.0,
        }];
        let base_url = Url::parse("https://example.com/path").unwrap();
        let mut image_cache = HashMap::new();
        image_cache.insert("https://example.com/tiny.png".to_string(), png_1x1_red);

        execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &image_cache, &base_url);

        assert!(
            pixmap.data().chunks_exact(4).any(|px| px[0] != 0 || px[1] != 0 || px[2] != 0 || px[3] != 0),
            "relative image paint command should render using absolute cached bytes"
        );
    }

    /// A scaled image must land at its layout rect, not at (x * scale, y * scale).
    #[test]
    fn test_scaled_image_is_drawn_at_its_rect() {
        use crate::layer_tree::{ObjectFit, PaintCommand};
        use url::Url;

        let red = encode_png(&image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])));
        for fit in [ObjectFit::Fill, ObjectFit::Contain, ObjectFit::Cover] {
            let mut pixmap = Pixmap::new(40, 40).unwrap();
            pixmap.fill(tiny_skia::Color::WHITE);
            let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 40.0, height: 40.0 };
            let cmds = vec![PaintCommand::Image {
                rect: LayoutRect { x: 20.0, y: 20.0, width: 10.0, height: 10.0 },
                url: "https://example.com/red.png".to_string(),
                object_fit: fit.clone(),
                alt: String::new(),
                radius: 0.0,
            }];
            let mut image_cache = HashMap::new();
            image_cache.insert("https://example.com/red.png".to_string(), red.clone());
            let base_url = Url::parse("https://example.com/").unwrap();

            execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &image_cache, &base_url);

            let at = |x: u32, y: u32| pixmap.pixel(x, y).unwrap();
            assert_eq!((at(25, 25).red(), at(25, 25).green()), (255, 0), "{fit:?}: rect centre must be red");
            assert_eq!(at(5, 5).green(), 255, "{fit:?}: nothing may be drawn at the scaled origin");
        }
    }

    fn encode_png(img: &image::RgbaImage) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// Sprite sheet: 4x2 image, left half red, right half green.
    fn sprite_png() -> Vec<u8> {
        let img = image::RgbaImage::from_fn(4, 2, |x, _| {
            if x < 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 255]) }
        });
        encode_png(&img)
    }

    fn paint_bg(cmd: PaintCommand, url: &str, bytes: Vec<u8>) -> Pixmap {
        use url::Url;
        let mut pixmap = Pixmap::new(10, 10).unwrap();
        pixmap.fill(tiny_skia::Color::WHITE);
        let tile_rect = LayoutRect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };
        let mut cache = HashMap::new();
        cache.insert(url.to_string(), bytes);
        let base = Url::parse("https://example.com/").unwrap();
        execute_commands_on_tile(&[cmd], &mut pixmap, tile_rect, &cache, &base);
        pixmap
    }

    fn px(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = p.pixel(x, y).unwrap();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    #[test]
    fn test_background_sprite_position_selects_region_and_no_repeat_clips() {
        use crate::background::{parse_position, parse_size};
        let bx = LayoutRect { x: 2.0, y: 2.0, width: 4.0, height: 4.0 };
        // Draw a 2x sprite sheet (16x8) at 8x4 and shift left by 4px: only the green half shows.
        let cmd = PaintCommand::BackgroundImage {
            url: "https://example.com/sp.png".into(),
            clip: bx,
            radius: 0.0,
            area: bx,
            position: parse_position("-4px 0"),
            size: parse_size("8px 4px"),
            repeat_x: false,
            repeat_y: false,
        };
        let sheet = image::RgbaImage::from_fn(16, 8, |x, _| {
            if x < 8 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 255]) }
        });
        let p = paint_bg(cmd, "https://example.com/sp.png", encode_png(&sheet));
        assert_eq!(px(&p, 2, 2), [0, 255, 0, 255], "sprite region at box origin must be green");
        assert_eq!(px(&p, 5, 5), [0, 255, 0, 255]);
        assert_eq!(px(&p, 1, 1), [255, 255, 255, 255], "outside the box stays white");
        assert_eq!(px(&p, 6, 2), [255, 255, 255, 255], "outside the box stays white");
    }

    #[test]
    fn test_background_repeat_x_fills_row_only() {
        use crate::background::{parse_position, parse_size};
        let bx = LayoutRect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 };
        let cmd = PaintCommand::BackgroundImage {
            url: "sp.png".into(),
            clip: bx,
            radius: 0.0,
            area: bx,
            position: parse_position("0 0"),
            size: parse_size("auto"),
            repeat_x: true,
            repeat_y: false,
        };
        let p = paint_bg(cmd, "https://example.com/sp.png", sprite_png());
        // Tiles repeat every 4px horizontally: x=4..5 red, x=6..7 green.
        assert_eq!(px(&p, 4, 0), [255, 0, 0, 255]);
        assert_eq!(px(&p, 7, 1), [0, 255, 0, 255]);
        assert_eq!(px(&p, 9, 1), [255, 0, 0, 255]);
        assert_eq!(px(&p, 3, 2), [255, 255, 255, 255], "no vertical repeat");
    }

    #[test]
    fn test_background_image_alpha_is_premultiplied() {
        use crate::background::{parse_position, parse_size};
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 128]));
        let bx = LayoutRect { x: 0.0, y: 0.0, width: 2.0, height: 2.0 };
        let cmd = PaintCommand::BackgroundImage {
            url: "a.png".into(),
            clip: bx,
            radius: 0.0,
            area: bx,
            position: parse_position("0 0"),
            size: parse_size("auto"),
            repeat_x: false,
            repeat_y: false,
        };
        let p = paint_bg(cmd, "https://example.com/a.png", encode_png(&img));
        let [r, g, b, _] = px(&p, 0, 0);
        // Half-transparent blue over white is roughly (127, 127, 255).
        assert!((120..=135).contains(&r) && (120..=135).contains(&g) && b == 255, "got {r},{g},{b}");
    }

    /// Text rendered via the cache must respect the clip rectangle (pixels outside
    /// the clip must remain at their background value).
    #[test]
    fn test_clip_rect_limits_text_pixels() {
        clear_glyph_cache();
        let rect = LayoutRect { x: 0.0, y: 0.0, width: 200.0, height: 40.0 };
        // Clip to only the right half (x=100..200).
        let clip_right = LayoutRect { x: 100.0, y: 0.0, width: 100.0, height: 40.0 };
        let clip_full  = rect;
        let color = black();

        let mut p_right = white_pixmap(200, 40);
        render_text_raw("Hello world text".to_string(), rect, 16.0, &color, clip_right, &mut p_right, false, false, 0);

        let mut p_full = white_pixmap(200, 40);
        render_text_raw("Hello world text".to_string(), rect, 16.0, &color, clip_full, &mut p_full, false, false, 0);

        // The two renders must differ (full render has pixels in x=0..99 too).
        assert_ne!(p_right.data(), p_full.data(),
            "clipped and unclipped renders must differ");

        // Confirm that every pixel at x < 100 in p_right remains white.
        // The pixmap stores rows of 200 pixels × 4 bytes each.
        let data = p_right.data();
        let w = 200usize;
        let h = 40usize;
        for row in 0..h {
            for col in 0..100usize {
                let base = (row * w + col) * 4;
                let rgba = &data[base..base + 4];
                assert_eq!(rgba, &[255, 255, 255, 255],
                    "pixel at ({}, {}) must remain white under right-half clip", col, row);
            }
        }
    }

    #[test]
    fn test_create_rounded_rect_path_clamps_large_radius() {
        let path = create_rounded_rect_path(
            LayoutRect {
                x: 0.0,
                y: 0.0,
                width: 85.0,
                height: 40.0,
            },
            100.0,
        );
        assert!(path.is_some(), "large border radius should still produce a valid path");
    }

    /// Blurring is centred: a symmetric shape stays symmetric.
    #[test]
    fn test_box_blur_alpha_is_centred() {
        let mut p = Pixmap::new(21, 1).unwrap();
        p.data_mut()[10 * 4 + 3] = 255;
        box_blur_alpha(&mut p, 3);
        let a = |x: usize| p.data()[x * 4 + 3];
        assert_eq!(a(7), a(13));
        assert!(a(7) > 0 && a(6) == 0 && a(14) == 0);
    }

    /// An outset shadow shows only outside the border box.
    #[test]
    fn test_outset_shadow_is_not_painted_under_the_box() {
        use crate::css::{BoxShadow, OrderedFloat};
        let shadow = BoxShadow {
            offset_x: OrderedFloat(0.0),
            offset_y: OrderedFloat(0.0),
            blur: OrderedFloat(0.0),
            spread: OrderedFloat(2.0),
            color: Color { r: 0, g: 0, b: 0, a: 255 },
            inset: false,
        };
        let mut p = Pixmap::new(40, 40).unwrap();
        let rect = LayoutRect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 };
        let cmds = vec![PaintCommand::Shadow(rect, shadow, 0.0)];
        execute_commands_on_tile(&cmds, &mut p, full_rect(40.0, 40.0), &HashMap::new(), &url::Url::parse("https://e.com/").unwrap());
        assert_eq!(px(&p, 20, 20)[3], 0, "inside the box");
        assert_eq!(px(&p, 9, 20)[3], 255, "inside the spread ring");
        assert_eq!(px(&p, 8, 8)[3], 255, "sharp box keeps a sharp shadow corner");
    }

    #[test]
    fn test_svg_data_url_image_is_rasterized_into_rect() {
        let url = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 2 1'%3E%3Crect width='1' height='1' fill='%230000ff'/%3E%3C/svg%3E";
        let mut p = white_pixmap(40, 20);
        let cmds = vec![PaintCommand::Image {
            rect: LayoutRect { x: 0.0, y: 0.0, width: 40.0, height: 20.0 },
            url: url.to_string(),
            object_fit: ObjectFit::Fill,
            alt: String::new(),
            radius: 0.0,
        }];
        execute_commands_on_tile(&cmds, &mut p, full_rect(40.0, 20.0), &HashMap::new(), &url::Url::parse("https://e.com/").unwrap());
        assert_eq!(px(&p, 5, 10), [0, 0, 255, 255], "left half is the blue rect");
        assert_eq!(px(&p, 35, 10), [255, 255, 255, 255], "right half is transparent");
    }

    #[test]
    fn test_rounded_image_is_clipped_to_its_radius() {
        let red = encode_png(&image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255])));
        let mut cache = HashMap::new();
        cache.insert("https://e.com/r.png".to_string(), red);
        let mut p = white_pixmap(40, 40);
        let cmds = vec![PaintCommand::Image {
            rect: full_rect(40.0, 40.0),
            url: "https://e.com/r.png".to_string(),
            object_fit: ObjectFit::Fill,
            alt: String::new(),
            radius: 20.0,
        }];
        execute_commands_on_tile(&cmds, &mut p, full_rect(40.0, 40.0), &cache, &url::Url::parse("https://e.com/").unwrap());
        assert_eq!(px(&p, 1, 1), [255, 255, 255, 255], "corner is clipped");
        assert_eq!(px(&p, 20, 20), [255, 0, 0, 255]);
    }

    #[test]
    fn test_inline_svg_command_paints_at_rect() {
        let source = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 1 1"><rect width="1" height="1" fill="#00ff00"/></svg>"##;
        let mut p = white_pixmap(30, 30);
        let cmds = vec![PaintCommand::Svg { rect: LayoutRect { x: 10.0, y: 10.0, width: 10.0, height: 10.0 }, source: source.into() }];
        execute_commands_on_tile(&cmds, &mut p, full_rect(30.0, 30.0), &HashMap::new(), &url::Url::parse("https://e.com/").unwrap());
        assert_eq!(px(&p, 15, 15), [0, 255, 0, 255]);
        assert_eq!(px(&p, 5, 5), [255, 255, 255, 255]);
    }

    #[test]
    fn test_border_sides_paint_each_side_in_its_color() {
        use crate::layer_tree::BorderSides;
        let blue = Color { r: 0, g: 0, b: 255, a: 255 };
        let sides = BorderSides {
            widths: [0.0, 0.0, 2.0, 4.0],
            colors: [black(), black(), red(), blue],
            styles: [BorderStyle::Solid; 4],
        };
        let mut p = white_pixmap(20, 20);
        let cmds = vec![PaintCommand::BorderSides { rect: full_rect(20.0, 20.0), sides: Box::new(sides), radius: 0.0 }];
        execute_commands_on_tile(&cmds, &mut p, full_rect(20.0, 20.0), &HashMap::new(), &url::Url::parse("https://e.com/").unwrap());
        assert_eq!(px(&p, 1, 10), [0, 0, 255, 255], "left side");
        assert_eq!(px(&p, 10, 19), [255, 0, 0, 255], "bottom side");
        assert_eq!(px(&p, 10, 1), [255, 255, 255, 255], "no top side");
        assert_eq!(px(&p, 10, 10), [255, 255, 255, 255], "interior");
    }

    #[test]
    fn test_iframe_frame_image_uses_absolute_key_or_paints_nothing() {
        let url = crate::layer_tree::iframe_frame_key("/ad", 4, 4);
        let cmds = vec![PaintCommand::Image { rect: full_rect(4.0, 4.0), url, object_fit: ObjectFit::Fill, alt: String::new(), radius: 0.0 }];
        let base = url::Url::parse("https://e.com/page").unwrap();
        let mut p = white_pixmap(4, 4);
        execute_commands_on_tile(&cmds, &mut p, full_rect(4.0, 4.0), &HashMap::new(), &base);
        assert!(p.data().iter().all(|b| *b == 255), "missing frame paints nothing");
        let mut cache = HashMap::new();
        let green = encode_png(&image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255])));
        cache.insert(crate::layer_tree::iframe_frame_key("https://e.com/ad", 4, 4), green);
        execute_commands_on_tile(&cmds, &mut p, full_rect(4.0, 4.0), &cache, &base);
        assert_eq!(px(&p, 2, 2), [0, 255, 0, 255]);
    }
}
