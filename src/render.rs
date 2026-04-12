use tiny_skia::{Pixmap, Paint, Transform, Stroke, PathBuilder, PixmapPaint};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, Rect as LayoutRect};
use crate::css::Color;
use crate::layer_tree::{LayerTree, LayerTreeBuilder, PaintCommand, Layer};
use std::collections::HashMap;

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

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
    for layer in tree.sorted_layers() {
        if layer.opacity <= 0.0 {
            continue;
        }
        execute_layer(layer, pixmap, image_cache);
    }
    let render_elapsed = start_render.elapsed();

    println!("[Perf] render_layout_tree: Layer gen: {:?}, Actual render: {:?}", layer_gen_elapsed, render_elapsed);
}

fn execute_layer(layer: &Layer, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
    for cmd in &layer.paint_commands {
        match cmd {
            PaintCommand::Rect(r, c, radius) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                if *radius > 0.0 {
                    if let Some(path) = create_rounded_rect_path(*r, *radius) {
                        pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
                    }
                } else if let Some(tr) = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height) {
                    pixmap.fill_rect(tr, &paint, Transform::identity(), None);
                }
            }
            PaintCommand::Border(r, w, c, radius) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                let mut stroke = Stroke::default();
                stroke.width = *w;
                if *radius > 0.0 {
                    if let Some(path) = create_rounded_rect_path(*r, *radius) {
                        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                    }
                } else if let Some(tr) = tiny_skia::Rect::from_xywh(r.x + w/2.0, r.y + w/2.0, (r.width - w).max(0.0), (r.height - w).max(0.0)) {
                    let mut pb = PathBuilder::new();
                    pb.push_rect(tr);
                    if let Some(path) = pb.finish() {
                        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
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
                                Transform::from_scale(r.width / rgba.width() as f32, r.height / rgba.height() as f32), None);
                        }
                    }
                }
            }
            PaintCommand::Text { rect, text, font_size, color, clip } => {
                render_text_raw(text.clone(), *rect, *font_size, color, *clip, pixmap);
            }
            PaintCommand::Shadow(r, s) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(s.color.r, s.color.g, s.color.b, s.color.a / 2);
                let sx = r.x + s.offset_x - s.spread;
                let sy = r.y + s.offset_y - s.spread;
                let sw = r.width + (s.spread * 2.0);
                let sh = r.height + (s.spread * 2.0);
                if let Some(tr) = tiny_skia::Rect::from_xywh(sx, sy, sw, sh) {
                    pixmap.fill_rect(tr, &paint, Transform::identity(), None);
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
