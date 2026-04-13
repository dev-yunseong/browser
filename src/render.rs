use tiny_skia::{Pixmap, Paint, Transform, Stroke, PathBuilder, PixmapPaint};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, Rect as LayoutRect};
use crate::css::Color;
use crate::layer_tree::{LayerTree, LayerTreeBuilder, PaintCommand};
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

fn execute_commands_on_tile(commands: &[PaintCommand], pixmap: &mut Pixmap, tile_rect: LayoutRect, image_cache: &HashMap<String, Vec<u8>>) {
    let tx = -tile_rect.x;
    let ty = -tile_rect.y;
    let transform = Transform::from_translate(tx, ty);

    for cmd in commands {
        match cmd {
            PaintCommand::Rect(r, c, radius) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                if *radius > 0.0 {
                    if let Some(path) = create_rounded_rect_path(*r, *radius) {
                        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, transform, None);
                    }
                } else if let Some(tr) = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height) {
                    pixmap.fill_rect(tr, &paint, transform, None);
                }
            }
            PaintCommand::Border(r, w, c, radius) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                let mut stroke = Stroke::default();
                stroke.width = *w;
                if *radius > 0.0 {
                    if let Some(path) = create_rounded_rect_path(*r, *radius) {
                        pixmap.stroke_path(&path, &paint, &stroke, transform, None);
                    }
                } else if let Some(tr) = tiny_skia::Rect::from_xywh(r.x + w/2.0, r.y + w/2.0, (r.width - w).max(0.0), (r.height - w).max(0.0)) {
                    let mut pb = PathBuilder::new();
                    pb.push_rect(tr);
                    if let Some(path) = pb.finish() {
                        pixmap.stroke_path(&path, &paint, &stroke, transform, None);
                    }
                }
            }
            PaintCommand::Image(r, url) => {
                if let Some(data) = image_cache.get(url) {
                    if let Ok(img) = image::load_from_memory(data) {
                        let rgba = img.to_rgba8();
                        if let Some(mut img_pixmap) = Pixmap::new(rgba.width(), rgba.height()) {
                            img_pixmap.data_mut().copy_from_slice(&rgba);
                            pixmap.draw_pixmap(r.x as i32, r.y as i32, img_pixmap.as_ref(), &PixmapPaint::default(),
                                transform.post_scale(r.width / rgba.width() as f32, r.height / rgba.height() as f32), None);
                        }
                    }
                }
            }
            PaintCommand::Text { rect, text, font_size, color, clip } => {
                let mut adjusted_rect = *rect;
                adjusted_rect.x += tx;
                adjusted_rect.y += ty;
                let mut adjusted_clip = *clip;
                adjusted_clip.x += tx;
                adjusted_clip.y += ty;
                render_text_raw(text.clone(), adjusted_rect, *font_size, color, adjusted_clip, pixmap);
            }
            PaintCommand::Shadow(r, s) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(s.color.r, s.color.g, s.color.b, s.color.a / 2);
                let sx = r.x + *s.offset_x - *s.spread;
                let sy = r.y + *s.offset_y - *s.spread;
                let sw = r.width + (*s.spread * 2.0);
                let sh = r.height + (*s.spread * 2.0);
                if let Some(tr) = tiny_skia::Rect::from_xywh(sx, sy, sw, sh) {
                    pixmap.fill_rect(tr, &paint, transform, None);
                }
            }
        }
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

fn render_text_raw(text: String, rect: LayoutRect, font_size: f32, color: &Color, clip: LayoutRect, pixmap: &mut Pixmap) {
    let trimmed = text.trim();
    if trimmed.is_empty() { return; }
    let font = FONT.get_or_init(|| {
        FontRef::try_from_slice(FONT_DATA).expect("Failed to parse embedded font")
    });
    let scale = PxScale::from(font_size);
    let units = font.units_per_em().unwrap_or(1000.0) as f32;
    let mut current_y = rect.y + (font_size * 0.85);
    let mut current_x = rect.x;
    let space_w = font.h_advance_unscaled(font.glyph_id(' ')) * (scale.x / units);
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
            current_x = rect.x;
            current_y += font_size * 1.4;
        }
        for (gid, adv) in glyphs {
            let glyph = gid.with_scale_and_position(scale, point(current_x, current_y));
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                let bx = bounds.min.x.floor() as i32;
                let by = bounds.min.y.floor() as i32;
                outline.draw(|gx, gy, coverage| {
                    let px = bx + gx as i32;
                    let py = by + gy as i32;
                    let pxf = px as f32;
                    let pyf = py as f32;
                    if pxf >= clip.x && pxf < (clip.x + clip.width) &&
                       pyf >= clip.y && pyf < (clip.y + clip.height) {
                        if px >= 0 && py >= 0 && px < pixmap.width() as i32 && py < pixmap.height() as i32 {
                            blend_glyph_pixel(pixmap, px as u32, py as u32, coverage, color);
                        }
                    }
                });
            }
            current_x += adv;
        }
        current_x += space_w;
    }
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
