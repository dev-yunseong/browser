use tiny_skia::{Pixmap, Paint, Transform, Stroke, PathBuilder, PixmapPaint};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, Rect as LayoutRect};
use crate::css::Color;
use crate::layer_tree::{LayerTree, LayerTreeBuilder, PaintCommand};
use std::collections::HashMap;
use std::sync::Mutex;
use lazy_static::lazy_static;

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

use std::time::Instant;

/// Entry point for rendering. Builds a `LayerTree` from the layout, then
/// composites all layers onto `pixmap` in ascending z-index order.
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
    
    // Recursive composition starting from root layer (id=0)
    composite_layer(0, &tree, pixmap, image_cache, LayoutRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 });

    let render_elapsed = start_render.elapsed();
    println!("[Perf] render_layout_tree: Layer gen: {:?}, Actual render: {:?}", layer_gen_elapsed, render_elapsed);
}

fn composite_layer(layer_id: usize, tree: &LayerTree, target: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>, parent_bounds: LayoutRect) {
    let layer = &tree.layers[layer_id];
    if layer.opacity <= 0.0 { return; }

    // 1. Acquire intermediate buffer for this layer
    let mut pool = TEXTURE_POOL.lock().unwrap();
    let mut layer_pixmap = pool.acquire(layer.bounds.width as u32, layer.bounds.height as u32);
    drop(pool);

    // 2. Identify and categorize children
    let (negative, zero, positive) = tree.categorize_children(layer_id);

    // ── CSS 7-Layer Painting Order ──

    // 3. (1/2) Render BACKGROUND of this layer
    execute_commands(&layer.background_commands, &mut layer_pixmap, layer.bounds, image_cache);

    // 4. (3) Render negative z-index child layers
    for &child_id in &negative {
        composite_layer(child_id, tree, &mut layer_pixmap, image_cache, layer.bounds);
    }

    // 5. (4) Render CONTENT of this layer (in-flow descendants)
    execute_commands(&layer.content_commands, &mut layer_pixmap, layer.bounds, image_cache);

    // 6. (5/6) Render zero and positive z-index child layers
    for &child_id in &zero {
        composite_layer(child_id, tree, &mut layer_pixmap, image_cache, layer.bounds);
    }
    for &child_id in &positive {
        composite_layer(child_id, tree, &mut layer_pixmap, image_cache, layer.bounds);
    }

    // 7. Composite this layer's buffer onto the parent target
    let mut paint = PixmapPaint::default();
    paint.opacity = layer.opacity;

    let local_x = layer.bounds.x - parent_bounds.x;
    let local_y = layer.bounds.y - parent_bounds.y;

    let transform = layer.transform.to_skia();
    let final_transform = Transform::from_translate(local_x, local_y).pre_concat(transform);

    target.draw_pixmap(
        0, 0,
        layer_pixmap.as_ref(),
        &paint,
        final_transform,
        None
    );

    // 8. Return buffer to pool
    let mut pool = TEXTURE_POOL.lock().unwrap();
    pool.release(layer_pixmap);
}

fn execute_commands(commands: &[PaintCommand], pixmap: &mut Pixmap, layer_bounds: LayoutRect, image_cache: &HashMap<String, Vec<u8>>) {
    let tx = -layer_bounds.x;
    let ty = -layer_bounds.y;
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

fn execute_tile(tile: &crate::layer_tree::Tile, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>, layer_bounds: LayoutRect) {
    let tx = layer_bounds.x - tile.rect.x;
    let ty = layer_bounds.y - tile.rect.y;
    let transform = Transform::from_translate(tx, ty);

    for cmd in &tile.paint_commands {
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
                // Adjust text rendering for tile offset
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

    // Simple rounded rect implementation
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
