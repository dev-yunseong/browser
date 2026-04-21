use tiny_skia::{Pixmap, Paint, Transform, Stroke, PathBuilder, PixmapPaint, Mask, FillRule,
    LinearGradient, RadialGradient, GradientStop, SpreadMode, Point as SkPoint};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, Rect as LayoutRect};
use crate::css::{Color, CssColorStop, LinearDirection};
use crate::layer_tree::{LayerTree, LayerTreeBuilder, PaintCommand, ObjectFit};
use crate::matrix::Matrix4x4;
use std::collections::HashMap;
use std::sync::Mutex;
use lazy_static::lazy_static;
use rayon::prelude::*;
use std::time::Instant;

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

lazy_static! {
    static ref TEXTURE_POOL: Mutex<TexturePool> = Mutex::new(TexturePool::new());
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
pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
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

    let tile_size = 256.0;
    let tiles_x = (viewport.width / tile_size).ceil() as u32;
    let tiles_y = (viewport.height / tile_size).ceil() as u32;

    let mut viewport_tiles = Vec::new();
    for y in 0..tiles_y {
        for x in 0..tiles_x {
            let rect = LayoutRect {
                x: x as f32 * tile_size,
                y: y as f32 * tile_size,
                width: tile_size.min(viewport.width - x as f32 * tile_size),
                height: tile_size.min(viewport.height - y as f32 * tile_size),
            };
            if rect.width > 0.1 && rect.height > 0.1 {
                viewport_tiles.push(rect);
            }
        }
    }

    // Parallel rendering of viewport tiles
    let rendered_tiles: Vec<(LayoutRect, Pixmap)> = viewport_tiles.into_par_iter().map(|tile_rect| {
        let mut tile_pixmap = Pixmap::new(tile_rect.width as u32, tile_rect.height as u32).unwrap();
        tile_pixmap.fill(tiny_skia::Color::TRANSPARENT);

        composite_layer_to_tile(0, &tree, &mut tile_pixmap, tile_rect, image_cache, LayoutRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 });

        (tile_rect, tile_pixmap)
    }).collect();

    // Composite all tiles onto the final pixmap (on main thread)
    for (rect, tile_pixmap) in rendered_tiles {
        pixmap.draw_pixmap(
            rect.x as i32, rect.y as i32,
            tile_pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None
        );
    }

    let render_elapsed = start_render.elapsed();
    println!("[Perf] render_layout_tree (Tiled): Layer gen: {:?}, Actual render: {:?}", layer_gen_elapsed, render_elapsed);
}

fn composite_layer_to_tile(
    layer_id: usize, 
    tree: &LayerTree, 
    target: &mut Pixmap, 
    tile_rect: LayoutRect, 
    image_cache: &HashMap<String, Vec<u8>>, 
    parent_bounds: LayoutRect
) {
    let layer = &tree.layers[layer_id];
    if layer.opacity <= 0.0 { return; }

    // Optimization: only process layer if it intersects with this viewport tile
    if !layer.bounds.intersects(&tile_rect) { return; }

    let has_effect = layer.opacity < 1.0 || layer.transform != Matrix4x4::identity();

    let mut layer_pixmap = if has_effect {
        let mut p = Pixmap::new(tile_rect.width as u32, tile_rect.height as u32).unwrap();
        p.fill(tiny_skia::Color::TRANSPARENT);
        Some(p)
    } else {
        None
    };

    let (negative, zero, positive) = tree.categorize_children(layer_id);

    let mut bg_cmds = Vec::new();
    let mut ct_cmds = Vec::new();
    for tile in &layer.tiles {
        if tile.rect.intersects(&tile_rect) {
            bg_cmds.extend(tile.background_commands.iter().cloned());
            ct_cmds.extend(tile.content_commands.iter().cloned());
        }
    }

    {
        let draw_target = if let Some(ref mut p) = layer_pixmap { p } else { &mut *target };

        // ── CSS 7-Layer Painting Order ──

        // 1. Background
        execute_commands_on_tile(&bg_cmds, draw_target, tile_rect, image_cache);

        // 2. Negative Z
        for &child_id in &negative {
            composite_layer_to_tile(child_id, tree, draw_target, tile_rect, image_cache, layer.bounds);
        }

        // 3. Content
        execute_commands_on_tile(&ct_cmds, draw_target, tile_rect, image_cache);

        // 4. Zero and positive Z
        for &child_id in &zero {
            composite_layer_to_tile(child_id, tree, draw_target, tile_rect, image_cache, layer.bounds);
        }
        for &child_id in &positive {
            composite_layer_to_tile(child_id, tree, draw_target, tile_rect, image_cache, layer.bounds);
        }
    }

    if let Some(p) = layer_pixmap {
        let mut paint = PixmapPaint::default();
        paint.opacity = layer.opacity;

        let local_x = layer.bounds.x - parent_bounds.x;
        let local_y = layer.bounds.y - parent_bounds.y;

        let transform = layer.transform.to_skia();
        let final_transform = Transform::from_translate(local_x - tile_rect.x, local_y - tile_rect.y).pre_concat(transform);

        target.draw_pixmap(0, 0, p.as_ref(), &paint, final_transform, None);
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

fn execute_commands_on_tile(commands: &[PaintCommand], pixmap: &mut Pixmap, tile_rect: LayoutRect, image_cache: &HashMap<String, Vec<u8>>) {
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
            PaintCommand::Image { rect: r, url, object_fit, alt } => {
                let drawn = if let Some(data) = image_cache.get(url) {
                    if let Ok(img) = image::load_from_memory(data) {
                        let rgba = img.to_rgba8();
                        let img_w = rgba.width() as f32;
                        let img_h = rgba.height() as f32;
                        if let Some(mut img_pixmap) = Pixmap::new(rgba.width(), rgba.height()) {
                            img_pixmap.data_mut().copy_from_slice(&rgba);
                            match object_fit {
                                ObjectFit::Fill => {
                                    // Stretch to fill — existing behavior
                                    pixmap.draw_pixmap(r.x as i32, r.y as i32, img_pixmap.as_ref(),
                                        &PixmapPaint::default(),
                                        transform.post_scale(r.width / img_w, r.height / img_h), active_mask!());
                                }
                                ObjectFit::Contain => {
                                    // Scale uniformly to fit inside rect; letterbox with transparency
                                    let s = (r.width / img_w).min(r.height / img_h);
                                    let sw = img_w * s;
                                    let sh = img_h * s;
                                    let ox = r.x + (r.width - sw) / 2.0;
                                    let oy = r.y + (r.height - sh) / 2.0;
                                    pixmap.draw_pixmap(ox as i32, oy as i32, img_pixmap.as_ref(),
                                        &PixmapPaint::default(),
                                        transform.post_scale(s, s), active_mask!());
                                }
                                ObjectFit::Cover => {
                                    // Scale uniformly to fill rect; draw into a temp pixmap to clip overflow
                                    let s = (r.width / img_w).max(r.height / img_h);
                                    let sw = img_w * s;
                                    let sh = img_h * s;
                                    let rw = r.width as u32;
                                    let rh = r.height as u32;
                                    if let Some(mut tmp) = Pixmap::new(rw.max(1), rh.max(1)) {
                                        let local_ox = (r.width - sw) / 2.0;
                                        let local_oy = (r.height - sh) / 2.0;
                                        tmp.draw_pixmap(local_ox as i32, local_oy as i32,
                                            img_pixmap.as_ref(), &PixmapPaint::default(),
                                            Transform::from_scale(s, s), None);
                                        pixmap.draw_pixmap(r.x as i32, r.y as i32, tmp.as_ref(),
                                            &PixmapPaint::default(), transform, active_mask!());
                                    }
                                }
                                ObjectFit::None => {
                                    // Intrinsic size (1:1 scale), clip to rect using a temp pixmap
                                    let rw = r.width as u32;
                                    let rh = r.height as u32;
                                    if let Some(mut tmp) = Pixmap::new(rw.max(1), rh.max(1)) {
                                        let ox = (r.width - img_w) / 2.0;
                                        let oy = (r.height - img_h) / 2.0;
                                        tmp.draw_pixmap(ox as i32, oy as i32, img_pixmap.as_ref(),
                                            &PixmapPaint::default(), Transform::identity(), None);
                                        pixmap.draw_pixmap(r.x as i32, r.y as i32, tmp.as_ref(),
                                            &PixmapPaint::default(), transform, active_mask!());
                                    }
                                }
                            }
                            true
                        } else { false }
                    } else { false }
                } else { false };

                if !drawn {
                    draw_broken_image(pixmap, *r, alt, transform);
                }
            }
            PaintCommand::Text { rect, text, font_size, color, clip, bold, italic, text_decoration } => {
                let mut adjusted_rect = *rect;
                adjusted_rect.x += tx;
                adjusted_rect.y += ty;
                let mut adjusted_clip = *clip;
                adjusted_clip.x += tx;
                adjusted_clip.y += ty;
                render_text_raw(text.clone(), adjusted_rect, *font_size, color, adjusted_clip, pixmap, *bold, *italic, *text_decoration);
            }
            PaintCommand::Shadow(r, s) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(s.color.r, s.color.g, s.color.b, s.color.a / 2);
                let sx = r.x + *s.offset_x - *s.spread;
                let sy = r.y + *s.offset_y - *s.spread;
                let sw = r.width + (*s.spread * 2.0);
                let sh = r.height + (*s.spread * 2.0);
                if let Some(tr) = tiny_skia::Rect::from_xywh(sx, sy, sw, sh) {
                    pixmap.fill_rect(tr, &paint, transform, active_mask!());
                }
            }
        }
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
    let rect = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height)?;
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

use std::sync::OnceLock;
static FONT: OnceLock<FontRef<'static>> = OnceLock::new();

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
    let trimmed = text.trim();
    if trimmed.is_empty() { return; }
    let font = FONT.get_or_init(|| {
        FontRef::try_from_slice(FONT_DATA).expect("Failed to parse embedded font")
    });
    let scale = PxScale::from(font_size);
    let units = font.units_per_em().unwrap_or(1000.0) as f32;
    let baseline_offset = font_size * 0.85;
    let mut current_y = rect.y + baseline_offset;
    let mut current_x = rect.x;
    let space_w = font.h_advance_unscaled(font.glyph_id(' ')) * (scale.x / units);

    // Shear coefficient for italic synthesis: shifts pixels ~12° (tan 12° ≈ 0.213)
    const ITALIC_SHEAR: f32 = 0.213;

    // We track line segments so we can draw decorations per line.
    // Each entry: (line_start_x, line_end_x, baseline_y)
    let mut decoration_lines: Vec<(f32, f32, f32)> = Vec::new();
    let mut line_start_x = current_x;
    let mut line_end_x = current_x;

    for word in trimmed.split_whitespace() {
        let mut word_w = 0.0;
        let mut glyphs = Vec::new();
        for c in word.chars() {
            let gid = font.glyph_id(c);
            let adv = font.h_advance_unscaled(gid) * (scale.x / units);
            glyphs.push((gid, adv));
            word_w += adv;
        }
        if current_x + word_w > rect.x + rect.width + 1.0 && current_x > rect.x {
            // End the current decoration line segment before wrapping.
            decoration_lines.push((line_start_x, line_end_x, current_y));
            current_x = rect.x;
            current_y += font_size * 1.4;
            line_start_x = current_x;
            line_end_x = current_x;
        }
        for (gid, adv) in glyphs {
            let glyph = gid.with_scale_and_position(scale, point(current_x, current_y));
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                let bx = bounds.min.x.floor() as i32;
                let by = bounds.min.y.floor() as i32;

                // Bold: draw glyph pixels shifted by small offsets for stroke thickening.
                let bold_offsets: &[(i32, i32)] = if bold {
                    &[(0, 0), (1, 0), (-1, 0), (0, 1)]
                } else {
                    &[(0, 0)]
                };

                outline.draw(|gx, gy, coverage| {
                    for &(dx, dy) in bold_offsets {
                        let mut px = bx + gx as i32 + dx;
                        let py = by + gy as i32 + dy;
                        let pyf = py as f32;

                        // Italic shear: shift x based on distance from baseline.
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
                });
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
