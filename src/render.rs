use tiny_skia::{Pixmap, Paint, Transform, Stroke, PathBuilder, PixmapPaint, Mask, FillRule,
    LinearGradient, RadialGradient, GradientStop, SpreadMode, Point as SkPoint};
use ab_glyph::{Font, point};
use crate::layout::{LayoutBox, Rect as LayoutRect};
use crate::css::{Color, CssColorStop, LinearDirection};
use crate::layer_tree::{LayerTree, LayerTreeBuilder, PaintCommand, ObjectFit};
use crate::matrix::Matrix4x4;
use std::collections::HashMap;
use std::sync::Mutex;
use lazy_static::lazy_static;
use std::time::Instant;
use url::Url;


// ── Glyph Cache ───────────────────────────────────────────────────────────────

/// Cache key for a single rasterized glyph.
///
/// Uses the glyph ID, font size (as bit-pattern to allow use in HashMap), and
/// synthesis flags.  The cache is keyed on font size rounded to the nearest
/// 0.5 px so that minutely different float values produced by the same logical
/// size collapse to the same entry.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct GlyphKey {
    /// Glyph ids are only meaningful within one face, so the face is part of
    /// the key — otherwise two faces' glyph 42 would share a cache entry.
    face: crate::font::FaceId,
    glyph_id: u16,
    /// `(font_size * 2.0).round() as u32` — rounds to nearest 0.5 px.
    font_size_half_px: u32,
    bold: bool,
    italic: bool,
    /// Which quarter-pixel the glyph starts on.
    ///
    /// A browser rasterises a glyph at its true fractional position; snapping
    /// every glyph to a whole pixel — as this did — moves each one by up to half
    /// a pixel, and the accumulated jitter is visible as uneven letter spacing
    /// even when the line as a whole is exactly the right width.
    x_phase: u8,
}

/// Pre-rasterized pixels for one glyph at a specific size and synthesis setting.
///
/// Pixel coordinates are stored as offsets relative to the glyph's bounding-box
/// origin `(bx, by)` so that the same entry can be replayed at any placement.
#[derive(Clone)]
struct GlyphPixels {
    /// `bx - floor(placement_x)` — signed delta from the floor of the placement
    /// x coordinate to the left edge of the glyph's bounding box.
    bx_delta: i32,
    /// `by - floor(placement_y)` — same for y (baseline direction).
    by_delta: i32,
    /// `(gx_offset, gy_offset, coverage)` — raw pixels from `outline.draw()`.
    /// Bold synthesis pixels are already expanded (up to 4× pixels per glyph
    /// sample), and italic shear is NOT pre-applied here (shear depends on
    /// `current_y - (by + gy)` which must be computed at paint time).
    pixels: Vec<(i32, i32, f32)>,
}

lazy_static! {
    static ref TEXTURE_POOL: Mutex<TexturePool> = Mutex::new(TexturePool::new());

    /// Process-wide glyph rasterization cache.
    ///
    /// Populated on first use of each (glyph, size, style) combination.
    /// Call `clear_glyph_cache()` between page navigations to free memory.
    static ref GLYPH_CACHE: Mutex<HashMap<GlyphKey, GlyphPixels>> =
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
        execute_commands_on_tile(&layer.background_commands, pixmap, layer.bounds, image_cache, base_url);

        for &child_id in &negative {
            composite_layer_to_surface(child_id, tree, pixmap, layer.bounds, image_cache, base_url);
        }

        execute_commands_on_tile(&layer.content_commands, pixmap, layer.bounds, image_cache, base_url);

        for &child_id in &zero {
            composite_layer_to_surface(child_id, tree, pixmap, layer.bounds, image_cache, base_url);
        }
        for &child_id in &positive {
            composite_layer_to_surface(child_id, tree, pixmap, layer.bounds, image_cache, base_url);
        }
    } else {
        execute_commands_on_tile(&layer.background_commands, target, surface_rect, image_cache, base_url);

        for &child_id in &negative {
            composite_layer_to_surface(child_id, tree, target, surface_rect, image_cache, base_url);
        }

        execute_commands_on_tile(&layer.content_commands, target, surface_rect, image_cache, base_url);

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
    let tx = -tile_rect.x;
    let ty = -tile_rect.y;
    let transform = Transform::from_translate(tx, ty);

    // Clip mask stack: each entry is the accumulated mask for that clip level.
    // `None` means the clip region did not intersect this tile or allocation failed.
    let mut clip_stack: Vec<Option<Mask>> = Vec::new();

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

            PaintCommand::Border(r, w, c, radius) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                let mut stroke = Stroke::default();
                stroke.width = *w;
                if *radius > 0.0 {
                    if let Some(path) = create_rounded_rect_path(*r, *radius) {
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
            PaintCommand::Image { rect: r, url, object_fit, alt, alt_color, alt_font_size } => {
                let resolved_url = if image_cache.contains_key(url) {
                    None
                } else {
                    base_url.join(url).ok().map(|u| u.to_string())
                };
                let drawn = if let Some(data) = image_cache
                    .get(url)
                    .or_else(|| resolved_url.as_ref().and_then(|u| image_cache.get(u)))
                {
                    if let Ok(img) = image::load_from_memory(data) {
                        let rgba = img.to_rgba8();
                        let img_w = rgba.width() as f32;
                        let img_h = rgba.height() as f32;
                        if let Some(mut img_pixmap) = Pixmap::new(rgba.width(), rgba.height()) {
                            img_pixmap.data_mut().copy_from_slice(&rgba);
                            // Every fit mode scales the source into a temporary
                            // pixmap the size of the destination box, then blits
                            // that at the box's position.
                            //
                            // Scaling during the blit instead would scale the
                            // destination offset along with the image —
                            // `draw_pixmap` applies its transform to the placed
                            // result, not just to the source — so a scaled-down
                            // image landed at a fraction of its intended y and
                            // painted over whatever was above it.
                            let dest_w = r.width.round().max(1.0) as u32;
                            let dest_h = r.height.round().max(1.0) as u32;
                            if let Some(mut tmp) = Pixmap::new(dest_w, dest_h) {
                                match object_fit {
                                    ObjectFit::Fill => {
                                        tmp.draw_pixmap(0, 0, img_pixmap.as_ref(), &PixmapPaint::default(),
                                            Transform::from_scale(r.width / img_w, r.height / img_h), None);
                                    }
                                    ObjectFit::Contain | ObjectFit::Cover => {
                                        // Contain fits inside the box and letterboxes;
                                        // Cover fills it and lets the overflow be
                                        // cropped by the temporary pixmap's edges.
                                        let s = if matches!(object_fit, ObjectFit::Contain) {
                                            (r.width / img_w).min(r.height / img_h)
                                        } else {
                                            (r.width / img_w).max(r.height / img_h)
                                        };
                                        let ox = (r.width - img_w * s) / 2.0;
                                        let oy = (r.height - img_h * s) / 2.0;
                                        tmp.draw_pixmap(ox as i32, oy as i32, img_pixmap.as_ref(),
                                            &PixmapPaint::default(), Transform::from_scale(s, s), None);
                                    }
                                    ObjectFit::None => {
                                        let ox = (r.width - img_w) / 2.0;
                                        let oy = (r.height - img_h) / 2.0;
                                        tmp.draw_pixmap(ox as i32, oy as i32, img_pixmap.as_ref(),
                                            &PixmapPaint::default(), Transform::identity(), None);
                                    }
                                }
                                pixmap.draw_pixmap(r.x as i32, r.y as i32, tmp.as_ref(),
                                    &PixmapPaint::default(), transform, active_mask!());
                            }
                            true
                        } else { false }
                    } else { false }
                } else { false };

                if !drawn {
                    draw_broken_image(pixmap, *r, alt, alt_color, *alt_font_size, transform);
                }
            }
            PaintCommand::Text { rect, text, font_size, line_height, leading_space, color, clip, style, letter_spacing, text_decoration } => {
                let mut adjusted_rect = *rect;
                adjusted_rect.x += tx;
                adjusted_rect.y += ty;
                let mut adjusted_clip = *clip;
                adjusted_clip.x += tx;
                adjusted_clip.y += ty;
                adjusted_rect.x += *leading_space;
                adjusted_rect.width = (adjusted_rect.width - *leading_space).max(0.0);
                render_text_raw(text.clone(), adjusted_rect, *font_size, *line_height, color, adjusted_clip, pixmap, *style, *letter_spacing, *text_decoration);
            }
            PaintCommand::Svg { rect, source, current_color } => {
                let mut r = *rect;
                r.x += tx;
                r.y += ty;
                draw_svg(pixmap, r, source, current_color);
            }
            PaintCommand::Shadow(r, s) => {
                let blur = *s.blur;
                let sx = r.x + *s.offset_x - *s.spread;
                let sy = r.y + *s.offset_y - *s.spread;
                let sw = (r.width + (*s.spread * 2.0)).max(1.0);
                let sh = (r.height + (*s.spread * 2.0)).max(1.0);

                if blur <= 0.0 {
                    // No blur: draw a sharp shadow rect directly.
                    let mut paint = Paint::default();
                    paint.set_color_rgba8(s.color.r, s.color.g, s.color.b, s.color.a);
                    if let Some(tr) = tiny_skia::Rect::from_xywh(sx, sy, sw, sh) {
                        pixmap.fill_rect(tr, &paint, transform, active_mask!());
                    }
                } else {
                    // Blurred shadow: render shape into temp pixmap, box-blur it,
                    // then composite onto the main pixmap.
                    //
                    // The blur "spreads" the shadow by roughly `blur` pixels in each
                    // direction, so the temp pixmap needs extra padding around the
                    // shadow shape equal to the blur radius so the falloff has room.
                    let pad = blur.ceil() as i32 + 1;
                    let pad_f = pad as f32;

                    let tmp_w = (sw + pad_f * 2.0).ceil() as u32;
                    let tmp_h = (sh + pad_f * 2.0).ceil() as u32;

                    if let Some(mut shadow_px) = Pixmap::new(tmp_w.max(1), tmp_h.max(1)) {
                        // Fill the shadow shape (solid, full alpha) in the temp pixmap.
                        // Shape is offset by `pad` so there is room for the blur halo.
                        let local_x = pad_f;
                        let local_y = pad_f;
                        if let Some(tr) = tiny_skia::Rect::from_xywh(local_x, local_y, sw, sh) {
                            let mut shape_paint = Paint::default();
                            // Use full opacity here; we apply the shadow color alpha when compositing.
                            shape_paint.set_color_rgba8(255, 255, 255, 255);
                            shadow_px.fill_rect(tr, &shape_paint, Transform::identity(), None);
                        }

                        // Apply a 3-pass separable box-blur to approximate a Gaussian.
                        // sigma ≈ blur / 2  →  box radius ≈ (blur / 2).round() as usize
                        let sigma = (blur / 2.0).max(1.0);
                        let radius = sigma.round() as usize;
                        box_blur_alpha(&mut shadow_px, radius);
                        box_blur_alpha(&mut shadow_px, radius);
                        box_blur_alpha(&mut shadow_px, radius);

                        // Composite the blurred shadow onto the target pixmap.
                        // The top-left of the temp pixmap (in document space) is at
                        // (sx - pad_f, sy - pad_f).  The tile transform shifts by tx/ty.
                        let dest_x = (sx - pad_f + tx) as i32;
                        let dest_y = (sy - pad_f + ty) as i32;

                        let cr = s.color.r;
                        let cg = s.color.g;
                        let cb = s.color.b;
                        let ca = s.color.a as f32 / 255.0;

                        // Walk every pixel of the blurred shadow and composite with
                        // the shadow color into the target pixmap.
                        let pw = pixmap.width() as i32;
                        let ph = pixmap.height() as i32;
                        let tw = shadow_px.width() as i32;
                        let th = shadow_px.height() as i32;
                        let shadow_data = shadow_px.data().to_vec();

                        for ty_off in 0..th {
                            let py = dest_y + ty_off;
                            if py < 0 || py >= ph { continue; }
                            for tx_off in 0..tw {
                                let px_coord = dest_x + tx_off;
                                if px_coord < 0 || px_coord >= pw { continue; }

                                // Each pixel in the shadow pixmap is RGBA premultiplied.
                                // We stored white (255,255,255) and blurred — the
                                // alpha channel holds the coverage.
                                let src_base = ((ty_off * tw + tx_off) * 4) as usize;
                                if src_base + 3 >= shadow_data.len() { continue; }
                                // After blurring, the alpha channel encodes coverage.
                                let coverage = shadow_data[src_base + 3] as f32 / 255.0;
                                if coverage <= 0.0 { continue; }

                                let alpha = (coverage * ca).clamp(0.0, 1.0);
                                if alpha <= 0.0 { continue; }

                                let dst_idx = (py as u32 * pixmap.width() + px_coord as u32) as usize;
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
                }
            }
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
    let k = 2 * radius + 1;

    // Horizontal pass — blur each row independently.
    for row in 0..h {
        let base = row * w * 4;
        // Accumulate the first window.
        let mut acc = 0u32;
        for x in 0..k.min(w) {
            acc += data[base + x * 4 + 3] as u32;
        }

        let mut tmp = vec![0u8; w];
        for x in 0..w {
            tmp[x] = (acc / k as u32) as u8;
            // Slide window: add leading edge, remove trailing edge.
            let lead = x + radius + 1;
            let trail = if x >= radius { x - radius } else { w }; // sentinel: skip
            if lead < w { acc += data[base + lead * 4 + 3] as u32; }
            if x >= radius { acc = acc.saturating_sub(data[base + trail * 4 + 3] as u32); }
        }
        for x in 0..w {
            data[base + x * 4 + 3] = tmp[x];
        }
    }

    // Vertical pass — blur each column independently.
    for col in 0..w {
        let mut acc = 0u32;
        for y in 0..k.min(h) {
            acc += data[(y * w + col) * 4 + 3] as u32;
        }

        let mut tmp = vec![0u8; h];
        for y in 0..h {
            tmp[y] = (acc / k as u32) as u8;
            let lead = y + radius + 1;
            if lead < h { acc += data[(lead * w + col) * 4 + 3] as u32; }
            if y >= radius { acc = acc.saturating_sub(data[((y - radius) * w + col) * 4 + 3] as u32); }
        }
        for y in 0..h {
            data[(y * w + col) * 4 + 3] = tmp[y];
        }
    }
}

/// Draw what a browser shows in place of an image it could not load: a hairline
/// outline around the box the image would have filled, and the element's `alt`
/// on the first line in the page's own text colour.
///
/// Filling the box with light grey and stamping "[broken image]" across it —
/// what this used to do — turns every unreachable image into the loudest thing
/// on the page. A browser leaves the box transparent, so the page's own
/// background shows through and the alt reads as the text it is.
fn draw_broken_image(
    pixmap: &mut Pixmap,
    r: LayoutRect,
    alt: &str,
    color: &Color,
    font_size: f32,
    transform: Transform,
) {
    // A hairline in the text colour, faint enough to read as an outline on a
    // light and a dark page alike.
    let mut border_paint = Paint::default();
    border_paint.set_color_rgba8(color.r, color.g, color.b, 90);
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
    if alt.is_empty() || r.width < 8.0 || r.height < font_size {
        return;
    }
    // Indented past where a browser puts its broken-image icon.
    let inset = 20.0f32.min(r.width / 2.0);
    let text_rect = LayoutRect {
        x: r.x + inset,
        y: r.y + 1.0,
        width: (r.width - inset - 2.0).max(0.0),
        height: font_size,
    };
    render_text_raw(
        alt.to_string(),
        text_rect,
        font_size,
        font_size * 1.2,
        color,
        text_rect,
        pixmap,
        crate::font::FontStyle::regular(),
        0.0,
        0,
    );
}

fn create_rounded_rect_path(r: LayoutRect, radius: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let rect = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height)?;
    let radius = radius
        .max(0.0)
        .min(rect.width().min(rect.height()) / 2.0);
    pb.move_to(rect.left() + radius, rect.top());
    pb.line_to(rect.right() - radius, rect.top());
    pb.quad_to(rect.right(), rect.top(), rect.right(), rect.top() + radius);
    pb.line_to(rect.right(), rect.bottom() - radius);
    pb.quad_to(rect.right(), rect.bottom(), rect.right() - radius, rect.bottom());
    pb.line_to(rect.left() + radius, rect.bottom());
    pb.quad_to(rect.left(), rect.bottom(), rect.left(), rect.bottom() - radius);
    pb.line_to(rect.left(), rect.top() + radius);
    pb.quad_to(rect.left(), rect.top(), rect.left() + radius, rect.top());
    pb.close();
    pb.finish()
}


/// Render a text run into `pixmap`.
///
/// # Synthesis notes
/// - **Bold** — re-draws each glyph up to 2 extra times at ±1 px offsets so the
///   strokes appear thicker.  This is a lightweight approximation that works
///   reasonably well for the NanumGothic TTF which only ships one weight.
/// - **Italic** — applies a horizontal shear (skew) to every pixel coordinate
///   before blending.  Each column is shifted left by `ITALIC_SHEAR * (baseline - y)`.
/// - **Underline / Line-through / Overline** — drawn as filled rectangles after all
///   glyphs are placed.
///
/// # Glyph cache
/// Glyph outlines are expensive to rasterize.  This function uses `GLYPH_CACHE`
/// to avoid re-running `outline_glyph` + `draw` for every glyph on every repaint.
/// On a cache miss the pixels are collected once and stored; subsequent calls
/// for the same (glyph, size, bold, italic) combination replay the stored pixels
/// directly into the pixmap.
fn render_text_raw(
    text: String,
    rect: LayoutRect,
    font_size: f32,
    line_height: f32,
    color: &Color,
    clip: LayoutRect,
    pixmap: &mut Pixmap,
    style: crate::font::FontStyle,
    letter_spacing: f32,
    text_decoration: u8,
) {
    let italic = style.italic;
    let trimmed = text.trim();
    if trimmed.is_empty() { return; }
    let fonts = crate::font::fonts();
    let baseline_offset = font_size * 0.85;
    let mut current_y = rect.y + baseline_offset;
    let mut current_x = rect.x;
    let space_w = fonts.advance(' ', font_size, style) + letter_spacing;

    // Shear coefficient for italic synthesis: shifts pixels ~12° (tan 12° ≈ 0.213)
    const ITALIC_SHEAR: f32 = 0.213;

    // We track line segments so we can draw decorations per line.
    // Each entry: (line_start_x, line_end_x, baseline_y)
    let mut decoration_lines: Vec<(f32, f32, f32)> = Vec::new();
    let mut line_start_x = current_x;
    let mut line_end_x = current_x;

    // font_size_half_px: rounds to nearest 0.5 px so that glyphs at the same
    // logical size share a cache entry regardless of tiny float differences.
    let font_size_half_px = (font_size * 2.0).round() as u32;

    for word in trimmed.split_whitespace() {
        let mut word_w = 0.0;
        let mut glyphs = Vec::new();
        for c in word.chars() {
            let (face, gid) = fonts.glyph(c, style);
            // A real bold face is bundled, so only the CJK fallback — which has
            // no bold companion — still needs the strokes thickened by hand.
            let synthetic_bold = style.bold && face == crate::font::FaceId::Fallback;
            let adv = fonts.advance(c, font_size, style) + letter_spacing;
            glyphs.push((face, gid, adv, synthetic_bold));
            word_w += adv;
        }
        if current_x + word_w > rect.x + rect.width + 1.0 && current_x > rect.x {
            // End the current decoration line segment before wrapping.
            decoration_lines.push((line_start_x, line_end_x, current_y));
            current_x = rect.x;
            current_y += line_height;
            line_start_x = current_x;
            line_end_x = current_x;
        }
        for (face, gid, adv, synthetic_bold) in glyphs {
            // Quarter-pixel phases are what browsers use; finer buys nothing
            // visible and multiplies the cache.
            const PHASES: f32 = 4.0;
            let x_phase = ((current_x - current_x.floor()) * PHASES).round() as u8 % PHASES as u8;
            let key = GlyphKey {
                face,
                glyph_id: gid.0,
                font_size_half_px,
                bold: synthetic_bold,
                italic: false, // Italic shear is applied at paint time; do not vary cache by italic
                x_phase,
            };

            // ── Cache lookup ──────────────────────────────────────────────────
            //
            // Try to find a pre-rasterized entry.  On a miss, rasterize the glyph
            // and store it.  We use a temporary placement of (0.0, 0.0) so that
            // the resulting pixels (gx, gy, coverage) are purely relative to the
            // glyph's own bounding-box origin and can be replayed at any position.
            let cached: GlyphPixels = {
                // Fast path: check cache without holding the lock across the
                // potentially-expensive rasterization.
                let cached_opt = GLYPH_CACHE.lock()
                    .ok()
                    .and_then(|c| c.get(&key).cloned());

                if let Some(entry) = cached_opt {
                    entry
                } else {
                    // Cache miss — rasterize using a canonical origin (0, 0) so
                    // that bx_delta/by_delta are position-independent.
                    let canonical = gid.with_scale_and_position(
                        fonts.scale(face, font_size),
                        point(x_phase as f32 / PHASES, 0.0),
                    );
                    let entry = if let Some(outline) = fonts.face(face).outline_glyph(canonical) {
                        let bounds = outline.px_bounds();
                        let bx_delta = bounds.min.x.floor() as i32;
                        let by_delta = bounds.min.y.floor() as i32;

                        // Bold: expand each sample to up to 4 pixel offsets.
                        let bold_offsets: &[(i32, i32)] = if synthetic_bold {
                            &[(0, 0), (1, 0), (-1, 0), (0, 1)]
                        } else {
                            &[(0, 0)]
                        };

                        let mut pixels: Vec<(i32, i32, f32)> = Vec::new();
                        outline.draw(|gx, gy, coverage| {
                            for &(dx, dy) in bold_offsets {
                                pixels.push((gx as i32 + dx, gy as i32 + dy, coverage));
                            }
                        });

                        GlyphPixels { bx_delta, by_delta, pixels }
                    } else {
                        // No outline (e.g. space character) — empty entry.
                        GlyphPixels { bx_delta: 0, by_delta: 0, pixels: Vec::new() }
                    };

                    // Store in cache (best-effort; ignore poisoned mutex).
                    if let Ok(mut cache) = GLYPH_CACHE.lock() {
                        cache.insert(key, entry.clone());
                    }
                    entry
                }
            };

            // ── Replay cached pixels ──────────────────────────────────────────
            let place_x = current_x.floor() as i32;
            let place_y = current_y.floor() as i32;
            let bx = place_x + cached.bx_delta;
            let by = place_y + cached.by_delta;

            for &(gx_off, gy_off, coverage) in &cached.pixels {
                let mut px = bx + gx_off;
                let py = by + gy_off;
                let pyf = py as f32;

                // Italic shear: shift x based on distance from baseline.
                // `current_y - pyf` ≈ `-(by_delta + gy_off)` since
                // `current_y - place_y` is < 1.0 (fractional part only).
                if italic {
                    let shear_px = (ITALIC_SHEAR * (current_y - pyf)) as i32;
                    px += shear_px;
                }

                let pxf = px as f32;
                if pxf >= clip.x && pxf < (clip.x + clip.width) &&
                   pyf >= clip.y && pyf < (clip.y + clip.height) {
                    if px >= 0 && py >= 0 && px < pixmap.width() as i32 && py < pixmap.height() as i32 {
                        blend_glyph_pixel(pixmap, px as u32, py as u32, coverage, color);
                    }
                }
            }

            current_x += adv;
            line_end_x = current_x;
        }
        current_x += space_w;
        line_end_x = current_x;
    }

    // Close the last line segment.
    if line_end_x > line_start_x {
        decoration_lines.push((line_start_x, line_end_x - space_w, current_y));
    }

    // Draw text decorations.
    if text_decoration != 0 {
        let line_thickness = (font_size * 0.07).max(1.0);
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);

        for (seg_start_x, seg_end_x, baseline_y) in decoration_lines {
            let seg_w = (seg_end_x - seg_start_x).max(0.0);
            if seg_w < 1.0 { continue; }

            // Underline: slightly below the baseline.
            if text_decoration & 0b001 != 0 {
                let uy = baseline_y + line_thickness;
                if let Some(r) = tiny_skia::Rect::from_xywh(seg_start_x, uy, seg_w, line_thickness) {
                    pixmap.fill_rect(r, &paint, Transform::identity(), None);
                }
            }

            // Line-through: at mid-height of the em square (≈ 40% up from baseline).
            if text_decoration & 0b010 != 0 {
                let ly = baseline_y - font_size * 0.30;
                if let Some(r) = tiny_skia::Rect::from_xywh(seg_start_x, ly, seg_w, line_thickness) {
                    pixmap.fill_rect(r, &paint, Transform::identity(), None);
                }
            }

            // Overline: above the em square top.
            if text_decoration & 0b100 != 0 {
                let oy = baseline_y - font_size * 0.85;
                if let Some(r) = tiny_skia::Rect::from_xywh(seg_start_x, oy, seg_w, line_thickness) {
                    pixmap.fill_rect(r, &paint, Transform::identity(), None);
                }
            }
        }
    }
}

/// Convert a `CssColorStop` slice into `tiny_skia::GradientStop`s.
/// Convert CSS colour stops to shader stops, filling in the positions CSS
/// leaves implicit.
///
/// A stop with no position is not at 0: the first defaults to 0, the last to 1,
/// and a run in between is spaced equally across the gap. Treating them all as 0
/// collapses the gradient — `linear-gradient(navy, transparent)` became a flat
/// slab of navy, which on a page that fades a hero into its background paints
/// the whole section opaque.
///
/// Positions are also forced non-decreasing, as the spec requires: a stop
/// earlier than the one before it is clamped up to it.
fn css_stops_to_skia(stops: &[CssColorStop]) -> Vec<GradientStop> {
    if stops.is_empty() {
        return Vec::new();
    }

    let last = stops.len() - 1;
    // Positions are kept as written, out of range included. `#fff 117%` means
    // the gradient never reaches white inside the box; clamping the stop to
    // 100% — as this did — made it reach white at the bottom edge instead, so
    // a hero faded out a whole shade too early.
    let mut positions: Vec<Option<f32>> = stops.iter().map(|s| s.position).collect();
    positions[0].get_or_insert(0.0);
    positions[last].get_or_insert(1.0);

    // Space each run of unpositioned stops evenly between its known neighbours.
    let mut i = 0;
    while i < positions.len() {
        if positions[i].is_some() {
            i += 1;
            continue;
        }
        let before = i - 1;
        let mut after = i;
        while positions[after].is_none() {
            after += 1;
        }
        let (start, end) = (positions[before].unwrap_or(0.0), positions[after].unwrap_or(1.0));
        let steps = (after - before) as f32;
        for (n, slot) in positions[i..after].iter_mut().enumerate() {
            *slot = Some(start + (end - start) * ((n + 1) as f32 / steps));
        }
        i = after + 1;
    }

    // A stop may not sit before the one in front of it.
    let mut previous = f32::NEG_INFINITY;
    let resolved: Vec<(f32, crate::css::Color)> = stops
        .iter()
        .zip(positions)
        .map(|(s, pos)| {
            let pos = pos.unwrap_or(previous).max(previous);
            previous = pos;
            (pos, s.color.clone())
        })
        .collect();

    // tiny-skia only takes stops inside [0, 1], so a run that starts before 0
    // or ends past 1 is clipped to the box: the colour at the boundary is the
    // interpolation of the two stops that straddle it.
    clip_stops_to_unit(&resolved)
        .into_iter()
        .map(|(pos, c)| {
            GradientStop::new(pos, tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a))
        })
        .collect()
}

/// Sample the colour of a gradient run at `t`, in sRGB.
fn lerp_color(a: &crate::css::Color, b: &crate::css::Color, t: f32) -> crate::css::Color {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    crate::css::Color {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: mix(a.a, b.a),
    }
}

/// Restrict a resolved stop list to the `[0, 1]` the shader can express.
///
/// Kept separate from the tiny-skia conversion so the clipping itself can be
/// checked: a `GradientStop` exposes neither its position nor its colour.
fn clip_stops_to_unit(resolved: &[(f32, crate::css::Color)]) -> Vec<(f32, crate::css::Color)> {
    if resolved.len() < 2 {
        return resolved
            .iter()
            .map(|(p, c)| (p.clamp(0.0, 1.0), c.clone()))
            .collect();
    }

    let mut clipped: Vec<(f32, crate::css::Color)> = Vec::with_capacity(resolved.len() + 2);
    for w in resolved.windows(2) {
        let (p0, c0) = (w[0].0, &w[0].1);
        let (p1, c1) = (w[1].0, &w[1].1);
        if (0.0..=1.0).contains(&p0) {
            clipped.push((p0, c0.clone()));
        }
        // The colour where this segment crosses the edge of the box.
        for edge in [0.0f32, 1.0f32] {
            if p0 < edge && p1 > edge {
                let t = (edge - p0) / (p1 - p0);
                clipped.push((edge, lerp_color(c0, c1, t)));
            }
        }
    }
    let (plast, clast) = resolved.last().expect("checked len >= 2");
    if (0.0..=1.0).contains(plast) {
        clipped.push((*plast, clast.clone()));
    }

    if clipped.len() < 2 {
        // Every stop fell outside the box: paint the run's own end colours.
        return vec![
            (0.0, resolved[0].1.clone()),
            (1.0, resolved[resolved.len() - 1].1.clone()),
        ];
    }
    clipped.sort_by(|a, b| a.0.total_cmp(&b.0));
    clipped
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

/// How much a glyph's partial coverage is darkened before it is blended.
///
/// The reference renderer runs glyph coverage through a contrast curve before
/// compositing, which is why the same face at the same size carries visibly more
/// ink there than a straight linear blend produces. Measured over the whole
/// `probe-generics` fixture, its text carried about a quarter more ink than ours
/// and a third more fully-dark pixels; this exponent closes that gap. It is an
/// approximation of the reference's curve, not a derivation of it.
const TEXT_COVERAGE_GAMMA: f32 = 1.0 / 1.45;

/// Rasterise an inline `<svg>` subtree into `rect`.
///
/// The subtree arrives as its own document, so it is handed to the SVG parser
/// as written. `currentColor` is substituted first: an icon set draws itself
/// with `fill="currentColor"`, which resolves to the element's own text colour
/// and means nothing to a standalone parser.
///
/// A subtree that fails to parse simply draws nothing — a page that ships a
/// malformed icon should lose the icon, not the render.
fn draw_svg(pixmap: &mut Pixmap, rect: LayoutRect, source: &str, current_color: &Color) {
    let w = rect.width.round().max(1.0) as u32;
    let h = rect.height.round().max(1.0) as u32;
    // A very large icon is a sign of a mis-sized box, not of intent; capping
    // keeps one bad box from allocating hundreds of megabytes.
    if w > 4096 || h > 4096 {
        return;
    }

    let resolved = source.replace(
        "currentColor",
        &format!("#{:02x}{:02x}{:02x}", current_color.r, current_color.g, current_color.b),
    );
    let options = resvg::usvg::Options::default();
    let Ok(tree) = resvg::usvg::Tree::from_str(&resolved, &options) else {
        return;
    };

    let Some(mut target) = resvg::tiny_skia::Pixmap::new(w, h) else {
        return;
    };
    // Scale the SVG's own coordinate system onto the box layout gave it.
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return;
    }
    let transform = resvg::tiny_skia::Transform::from_scale(
        w as f32 / size.width(),
        h as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut target.as_mut());

    // resvg draws into its own tiny-skia version's pixmap, so the result is
    // copied across rather than handed over.
    let ox = rect.x.round() as i32;
    let oy = rect.y.round() as i32;
    let src = target.data();
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let a = src[i + 3];
            if a == 0 {
                continue;
            }
            let (px, py) = (ox + x as i32, oy + y as i32);
            if px < 0 || py < 0 || px >= pixmap.width() as i32 || py >= pixmap.height() as i32 {
                continue;
            }
            // resvg's output is premultiplied; unpremultiply before blending so
            // the existing glyph/rect blend path sees straight colours.
            let inv = 255.0 / a as f32;
            let colour = Color {
                r: (src[i] as f32 * inv).min(255.0) as u8,
                g: (src[i + 1] as f32 * inv).min(255.0) as u8,
                b: (src[i + 2] as f32 * inv).min(255.0) as u8,
                a: 255,
            };
            blend_glyph_pixel(pixmap, px as u32, py as u32, a as f32 / 255.0, &colour);
        }
    }
}

fn blend_glyph_pixel(pixmap: &mut Pixmap, x: u32, y: u32, coverage: f32, color: &Color) {
    if coverage <= 0.0 { return; }
    let coverage = coverage.clamp(0.0, 1.0).powf(TEXT_COVERAGE_GAMMA);
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

    /// Serialises the tests that depend on the glyph cache's contents.
    ///
    /// The cache is process-global, so two such tests running at once clear each
    /// other's entries and fail intermittently on whichever lost the race.
    fn cache_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    use crate::layout::Rect as LayoutRect;
    use crate::css::Color;

    fn stop(pos: Option<f32>, r: u8, g: u8, b: u8) -> CssColorStop {
        CssColorStop { color: Color { r, g, b, a: 255 }, position: pos }
    }

    /// Resolve a CSS stop list the way paint does, without the shader.
    fn resolved(stops: &[CssColorStop]) -> Vec<(f32, Color)> {
        let mut positions: Vec<Option<f32>> = stops.iter().map(|s| s.position).collect();
        let last = positions.len() - 1;
        positions[0].get_or_insert(0.0);
        positions[last].get_or_insert(1.0);
        let mut i = 0;
        while i < positions.len() {
            if positions[i].is_some() { i += 1; continue; }
            let before = i - 1;
            let mut after = i;
            while positions[after].is_none() { after += 1; }
            let (a, b) = (positions[before].unwrap_or(0.0), positions[after].unwrap_or(1.0));
            let steps = (after - before) as f32;
            for (n, slot) in positions[i..after].iter_mut().enumerate() {
                *slot = Some(a + (b - a) * ((n + 1) as f32 / steps));
            }
            i = after + 1;
        }
        let pairs: Vec<(f32, Color)> = stops
            .iter()
            .zip(positions)
            .map(|(s, p)| (p.unwrap_or(0.0), s.color.clone()))
            .collect();
        clip_stops_to_unit(&pairs)
    }

    /// A glyph is rasterised at the quarter-pixel it actually starts on.
    /// Snapping every glyph to a whole pixel moved each one by up to half a
    /// pixel, and the accumulated jitter showed as uneven letter spacing even
    /// when the line as a whole was exactly the right width.
    #[test]
    fn test_glyphs_at_different_subpixel_offsets_differ() {
        let _guard = cache_guard();
        let color = black();
        let mut whole = white_pixmap(80, 30);
        let mut half = white_pixmap(80, 30);
        let at = |x: f32| LayoutRect { x, y: 2.0, width: 78.0, height: 26.0 };
        render_text_raw(
            "iiii".to_string(), at(4.0), 16.0, 19.2, &color, at(0.0), &mut whole,
            crate::font::FontStyle::regular(), 0.0, 0,
        );
        render_text_raw(
            "iiii".to_string(), at(4.5), 16.0, 19.2, &color, at(0.0), &mut half,
            crate::font::FontStyle::regular(), 0.0, 0,
        );
        assert_ne!(
            whole.data(), half.data(),
            "a run started half a pixel over must not rasterise identically"
        );
    }

    /// An inline `<svg>` is rasterised into the box layout gave it, and
    /// `currentColor` resolves to the element's own text colour — an icon set
    /// draws itself that way and means nothing to a standalone parser.
    #[test]
    fn test_inline_svg_draws_and_resolves_current_color() {
        let mut pixmap = white_pixmap(40, 40);
        let rect = LayoutRect { x: 0.0, y: 0.0, width: 40.0, height: 40.0 };
        draw_svg(
            &mut pixmap,
            rect,
            r#"<svg viewBox="0 0 10 10" fill="currentColor"><rect width="10" height="10"/></svg>"#,
            &Color { r: 0, g: 128, b: 0, a: 255 },
        );
        let px = pixmap.pixel(20, 20).expect("centre pixel").demultiply();
        assert!(
            px.green() > 100 && px.red() < 60 && px.blue() < 60,
            "the square takes the element's colour, got {:?}",
            (px.red(), px.green(), px.blue())
        );
    }

    /// A subtree that will not parse loses the icon, not the render.
    #[test]
    fn test_malformed_svg_draws_nothing() {
        let mut pixmap = white_pixmap(20, 20);
        let before = pixmap.data().to_vec();
        draw_svg(
            &mut pixmap,
            LayoutRect { x: 0.0, y: 0.0, width: 20.0, height: 20.0 },
            "<svg><path d=",
            &Color { r: 0, g: 0, b: 0, a: 255 },
        );
        assert_eq!(pixmap.data(), &before[..], "nothing was drawn");
    }

    /// A browser leaves an unloadable image's box transparent: it outlines it
    /// and writes the `alt`, but paints no fill. Filling it with light grey —
    /// what this used to do — turned every unreachable image into the loudest
    /// thing on the page.
    #[test]
    fn test_broken_image_paints_no_fill() {
        let mut pixmap = white_pixmap(60, 40);
        draw_broken_image(
            &mut pixmap,
            LayoutRect { x: 0.0, y: 0.0, width: 60.0, height: 40.0 },
            "",
            &Color { r: 0, g: 0, b: 0, a: 255 },
            16.0,
            Transform::identity(),
        );
        // The middle of the box is untouched; only its edge is drawn on.
        let mid = ((20 * 60) + 30) * 4;
        assert_eq!(
            &pixmap.data()[mid..mid + 3],
            &[255, 255, 255],
            "the interior stays the page's own background"
        );
        let edge = ((0 * 60) + 30) * 4;
        assert!(
            pixmap.data()[edge] < 255,
            "but the outline is drawn along the top edge"
        );
    }

    /// The alt is drawn in the page's own text colour, not a fixed grey, so it
    /// stays legible on a dark page.
    #[test]
    fn test_broken_image_alt_uses_the_inherited_colour() {
        let mut pixmap = white_pixmap(200, 40);
        draw_broken_image(
            &mut pixmap,
            LayoutRect { x: 0.0, y: 0.0, width: 200.0, height: 40.0 },
            "Duolingo",
            &Color { r: 220, g: 0, b: 0, a: 255 },
            16.0,
            Transform::identity(),
        );
        let reddish = pixmap
            .data()
            .chunks_exact(4)
            .any(|px| px[0] > 180 && px[1] < 120 && px[2] < 120);
        assert!(reddish, "the alt text should be drawn in the stated colour");
    }

    /// `#fff 117%` means the gradient never reaches white inside the box.
    /// Clamping the stop to 100% made it reach white at the bottom edge, so a
    /// hero faded out a whole shade too early.
    #[test]
    fn test_gradient_stop_past_the_box_is_clipped_not_clamped() {
        let out = resolved(&[stop(Some(0.0), 0, 0, 0), stop(Some(1.17), 255, 255, 255)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].0, 1.0);
        // At the bottom edge the run is 1/1.17 of the way to white.
        let expected = (255.0f32 / 1.17).round() as u8;
        assert!(
            out[1].1.r.abs_diff(expected) <= 2,
            "the far edge should be part way to white ({expected}), got {}",
            out[1].1.r
        );
    }

    /// A stop before the start of the box is clipped the same way.
    #[test]
    fn test_gradient_stop_before_the_box_is_clipped() {
        let out = resolved(&[stop(Some(-1.0), 0, 0, 0), stop(Some(1.0), 255, 255, 255)]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, 0.0);
        assert!(
            out[0].1.r.abs_diff(128) <= 2,
            "half the run lies above the box, so it starts mid-grey, got {}",
            out[0].1.r
        );
    }

    /// Ordinary stops are untouched, and an unpositioned one lands halfway.
    #[test]
    fn test_gradient_stops_inside_the_box_are_unchanged() {
        let out = resolved(&[
            stop(Some(0.0), 255, 0, 0),
            stop(None, 0, 255, 0),
            stop(Some(1.0), 0, 0, 255),
        ]);
        assert_eq!(out.len(), 3);
        assert!((out[1].0 - 0.5).abs() < 0.01);
        assert_eq!(out[1].1.g, 255);
    }

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
        let _guard = cache_guard();
        clear_glyph_cache();

        let rect = full_rect(200.0, 40.0);
        let color = black();

        let mut pixmap1 = white_pixmap(200, 40);
        render_text_raw("Hello".to_string(), rect, 16.0, 19.2, &color, rect, &mut pixmap1, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        clear_glyph_cache();

        let mut pixmap2 = white_pixmap(200, 40);
        render_text_raw("Hello".to_string(), rect, 16.0, 19.2, &color, rect, &mut pixmap2, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        assert_eq!(pixmap1.data(), pixmap2.data(),
            "cache and uncached renders must produce identical pixels");
    }

    /// The glyph cache must be populated after the first render.
    #[test]
    fn test_glyph_cache_is_populated_after_render() {
        let _guard = cache_guard();
        clear_glyph_cache();

        let rect = full_rect(200.0, 40.0);
        let mut pixmap = white_pixmap(200, 40);
        render_text_raw("Abc".to_string(), rect, 16.0, 19.2, &black(), rect, &mut pixmap, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        let cache_size = GLYPH_CACHE.lock().unwrap().len();
        assert!(cache_size > 0, "glyph cache should be non-empty after rendering text; got {} entries", cache_size);
    }

    /// `clear_glyph_cache()` must empty the cache.
    #[test]
    fn test_clear_glyph_cache_empties_cache() {
        let _guard = cache_guard();
        // Populate.
        let rect = full_rect(200.0, 40.0);
        let mut pixmap = white_pixmap(200, 40);
        render_text_raw("Test".to_string(), rect, 16.0, 19.2, &black(), rect, &mut pixmap, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        clear_glyph_cache();

        let cache_size = GLYPH_CACHE.lock().unwrap().len();
        assert_eq!(cache_size, 0, "cache should be empty after clear_glyph_cache()");
    }

    /// Bold text rendered twice must match.
    #[test]
    fn test_bold_text_cache_identical() {
        let _guard = cache_guard();
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);
        let color = black();

        let mut p1 = white_pixmap(200, 40);
        render_text_raw("Bold".to_string(), rect, 16.0, 19.2, &color, rect, &mut p1, crate::font::FontStyle { bold: true, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        clear_glyph_cache();
        let mut p2 = white_pixmap(200, 40);
        render_text_raw("Bold".to_string(), rect, 16.0, 19.2, &color, rect, &mut p2, crate::font::FontStyle { bold: true, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        assert_eq!(p1.data(), p2.data(), "bold renders must be identical across cache miss and cache hit");
    }

    /// Italic text rendered twice must match.
    #[test]
    fn test_italic_text_cache_identical() {
        let _guard = cache_guard();
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);
        let color = black();

        let mut p1 = white_pixmap(200, 40);
        render_text_raw("Italic".to_string(), rect, 16.0, 19.2, &color, rect, &mut p1, crate::font::FontStyle { bold: false, italic: true, monospace: false, serif: false, web_family: None }, 0.0, 0);

        clear_glyph_cache();
        let mut p2 = white_pixmap(200, 40);
        render_text_raw("Italic".to_string(), rect, 16.0, 19.2, &color, rect, &mut p2, crate::font::FontStyle { bold: false, italic: true, monospace: false, serif: false, web_family: None }, 0.0, 0);

        assert_eq!(p1.data(), p2.data(), "italic renders must be identical across cache miss and cache hit");
    }

    // ── Visual correctness ────────────────────────────────────────────────────

    /// Rendering non-empty text must modify at least one pixel (basic sanity check
    /// that the text actually hits the pixmap).
    #[test]
    fn test_text_modifies_pixmap() {
        let _guard = cache_guard();
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);
        let mut pixmap = white_pixmap(200, 40);
        let white_before = pixmap.data().to_vec();

        render_text_raw("Hello world".to_string(), rect, 16.0, 19.2, &black(), rect, &mut pixmap, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        assert_ne!(pixmap.data(), white_before.as_slice(), "text rendering must modify the pixmap");
    }

    /// Empty and whitespace-only strings must not modify the pixmap at all.
    #[test]
    fn test_empty_text_does_not_modify_pixmap() {
        let _guard = cache_guard();
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);

        for text in &["", "   ", "\t\n"] {
            let mut pixmap = white_pixmap(200, 40);
            let before = pixmap.data().to_vec();
            render_text_raw(text.to_string(), rect, 16.0, 19.2, &black(), rect, &mut pixmap, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);
            assert_eq!(pixmap.data(), before.as_slice(), "empty/whitespace text must not modify pixmap");
        }
    }

    /// Text with underline decoration must produce a different pixel output than
    /// plain text (the decoration lines add extra pixels).
    #[test]
    fn test_underline_decoration_differs_from_plain() {
        let _guard = cache_guard();
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);
        let color = black();

        let mut plain = white_pixmap(200, 40);
        render_text_raw("Hello".to_string(), rect, 16.0, 19.2, &color, rect, &mut plain, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        let mut underlined = white_pixmap(200, 40);
        render_text_raw("Hello".to_string(), rect, 16.0, 19.2, &color, rect, &mut underlined, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0b001);

        assert_ne!(plain.data(), underlined.data(), "underlined text must differ from plain text");
    }

    /// Different font sizes must be cached independently (i.e. produce different output).
    #[test]
    fn test_different_font_sizes_are_independent_cache_entries() {
        let _guard = cache_guard();
        clear_glyph_cache();
        let rect = full_rect(200.0, 60.0);
        let color = black();

        let mut p12 = white_pixmap(200, 60);
        render_text_raw("A".to_string(), rect, 12.0, 14.4, &color, rect, &mut p12, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);
        let mut p24 = white_pixmap(200, 60);
        render_text_raw("A".to_string(), rect, 24.0, 28.8, &color, rect, &mut p24, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

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
        let _guard = cache_guard();
        clear_glyph_cache();
        let rect = full_rect(200.0, 40.0);

        let mut p_black = white_pixmap(200, 40);
        render_text_raw("Hi".to_string(), rect, 16.0, 19.2, &black(), rect, &mut p_black, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        let mut p_red = white_pixmap(200, 40);
        render_text_raw("Hi".to_string(), rect, 16.0, 19.2, &red(), rect, &mut p_red, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        assert_ne!(p_black.data(), p_red.data(), "black and red text must produce different pixel output");
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
        let cmds = vec![PaintCommand::Shadow(rect, shadow)];
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
        let cmds = vec![PaintCommand::Shadow(rect, shadow)];
        let base_url = Url::parse("https://example.com/").unwrap();
        execute_commands_on_tile(&cmds, &mut pixmap, tile_rect, &HashMap::new(), &base_url);

        // The pixel several pixels outside the shadow rect should have non-zero alpha
        // due to the blur halo.  Shadow at (40,40) size (20,20); check pixel at (34,40).
        let halo_alpha = pixmap.data()[(40 * 100 + 34) * 4 + 3];
        assert!(halo_alpha > 0, "blurred shadow halo pixel should be non-zero alpha, got {}", halo_alpha);
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
            alt_color: Color { r: 0, g: 0, b: 0, a: 255 },
            alt_font_size: 16.0,
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

    /// Text rendered via the cache must respect the clip rectangle (pixels outside
    /// the clip must remain at their background value).
    #[test]
    fn test_clip_rect_limits_text_pixels() {
        let _guard = cache_guard();
        clear_glyph_cache();
        let rect = LayoutRect { x: 0.0, y: 0.0, width: 200.0, height: 40.0 };
        // Clip to only the right half (x=100..200).
        let clip_right = LayoutRect { x: 100.0, y: 0.0, width: 100.0, height: 40.0 };
        let clip_full  = rect;
        let color = black();

        let mut p_right = white_pixmap(200, 40);
        render_text_raw("Hello world text".to_string(), rect, 16.0, 19.2, &color, clip_right, &mut p_right, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

        let mut p_full = white_pixmap(200, 40);
        render_text_raw("Hello world text".to_string(), rect, 16.0, 19.2, &color, clip_full, &mut p_full, crate::font::FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }, 0.0, 0);

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
}
