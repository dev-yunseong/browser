use tiny_skia::{Pixmap, Paint, Transform, Stroke, PathBuilder};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, DisplayType};
use crate::css::Value;
use markup5ever_rcdom::NodeData;
use std::collections::HashMap;

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
    render_backgrounds(layout, pixmap);
    // Initial clip is the entire pixmap
    let initial_clip = crate::layout::Rect {
        x: 0.0,
        y: 0.0,
        width: pixmap.width() as f32,
        height: pixmap.height() as f32,
    };
    render_foregrounds(layout, pixmap, image_cache, initial_clip);
}

fn render_backgrounds(layout: &LayoutBox, pixmap: &mut Pixmap) {
    let d = layout.dimensions;
    if d.width < 0.1 || d.height < 0.1 {
        for child in &layout.children { render_backgrounds(child, pixmap); }
        return;
    }

    // ── 1. Box Shadow ──
    if let Some(Value::BoxShadow(shadow)) = layout.style_node.specified_values.get("box-shadow") {
        if !shadow.inset {
            let mut paint = Paint::default();
            paint.set_color_rgba8(shadow.color.r, shadow.color.g, shadow.color.b, shadow.color.a / 2);
            let sx = d.x + shadow.offset_x - shadow.spread;
            let sy = d.y + shadow.offset_y - shadow.spread;
            let sw = d.width + (shadow.spread * 2.0);
            let sh = d.height + (shadow.spread * 2.0);
            if let Some(r) = tiny_skia::Rect::from_xywh(sx, sy, sw, sh) {
                pixmap.fill_rect(r, &paint, Transform::identity(), None);
                if shadow.blur > 0.0 {
                    paint.set_color_rgba8(shadow.color.r, shadow.color.g, shadow.color.b, shadow.color.a / 4);
                    let b = shadow.blur / 2.0;
                    if let Some(r2) = tiny_skia::Rect::from_xywh(sx - b, sy - b, sw + shadow.blur, sh + shadow.blur) {
                        pixmap.fill_rect(r2, &paint, Transform::identity(), None);
                    }
                }
            }
        }
    }

    // ── 2. Background ──
    let bg = layout.style_node.specified_values.get("background-color").or(layout.style_node.specified_values.get("background"));
    if let Some(Value::Color(c)) = bg {
        if c.a > 0 {
            let mut paint = Paint::default();
            paint.set_color_rgba8(c.r, c.g, c.b, c.a);
            if let Some(r) = tiny_skia::Rect::from_xywh(d.x, d.y, d.width, d.height) {
                pixmap.fill_rect(r, &paint, Transform::identity(), None);
            }
        }
    }

    // ── 3. Border ──
    if layout.border.left > 0.0 {
        let mut paint = Paint::default();
        paint.set_color_rgba8(180, 180, 180, 255);
        let mut stroke = Stroke::default();
        let b = layout.border.left;
        stroke.width = b;
        // Draw border INSIDE the border-box
        if let Some(r) = tiny_skia::Rect::from_xywh(d.x + b/2.0, d.y + b/2.0, (d.width - b).max(0.0), (d.height - b).max(0.0)) {
            let mut pb = PathBuilder::new();
            pb.push_rect(r);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
    }

    for child in &layout.children {
        render_backgrounds(child, pixmap);
    }
}

fn render_foregrounds(layout: &LayoutBox, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>, clip: crate::layout::Rect) {
    let d = layout.dimensions;
    if d.width < 0.1 || d.height < 0.1 {
        for child in &layout.children { render_foregrounds(child, pixmap, image_cache, clip); }
        return;
    }

    // ── 4. Image ──
    if layout.display == DisplayType::Image {
        if let Some(ref url) = layout.image_url {
            if let Some(data) = image_cache.get(url) {
                if let Ok(img) = image::load_from_memory(data) {
                    let rgba = img.to_rgba8();
                    if let Some(mut img_pixmap) = Pixmap::new(rgba.width(), rgba.height()) {
                        img_pixmap.data_mut().copy_from_slice(&rgba);
                        // Apply clip by only drawing if within bounds (simplified)
                        pixmap.draw_pixmap(d.x as i32, d.y as i32, img_pixmap.as_ref(), &tiny_skia::PixmapPaint::default(), 
                            Transform::from_scale(d.width / rgba.width() as f32, d.height / rgba.height() as f32), None);
                    }
                }
            }
        }
    }

    // ── 5. Text ──
    if let NodeData::Text { ref contents } = layout.style_node.node.data {
        render_text_wrapped(contents.borrow().to_string(), layout, pixmap, clip);
    }

    // When going to children, the new clip is the intersection of current clip and parent's content area
    let next_clip = clip.intersect(&layout.get_content_rect());

    for child in &layout.children {
        render_foregrounds(child, pixmap, image_cache, next_clip);
    }
}

fn render_text_wrapped(text: String, layout: &LayoutBox, pixmap: &mut Pixmap, clip: crate::layout::Rect) {
    let trimmed = text.trim();
    if trimmed.is_empty() { return; }

    let font_size = match layout.style_node.specified_values.get("font-size") {
        Some(Value::Length(v, _)) => *v,
        _ => 16.0,
    };
    let color = match layout.style_node.specified_values.get("color") {
        Some(Value::Color(c)) => c.clone(),
        _ => crate::css::Color { r: 0, g: 0, b: 0, a: 255 },
    };

    let font = FontRef::try_from_slice(FONT_DATA).unwrap();
    let scale = PxScale::from(font_size);
    let units = font.units_per_em().unwrap_or(1000.0) as f32;

    let mut current_y = layout.dimensions.y + (font_size * 0.85);
    let mut current_x = layout.dimensions.x;
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

        if current_x + word_w > layout.dimensions.x + layout.dimensions.width + 1.0 && current_x > layout.dimensions.x {
            current_x = layout.dimensions.x;
            current_y += font_size * 1.4;
        }

        for (gid, adv) in glyphs {
            let glyph = gid.with_scale_and_position(scale, point(current_x, current_y));
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                let bx = bounds.min.x.floor() as i32;
                let by = bounds.min.y.floor() as i32;
                let pw = pixmap.width() as i32;
                let ph = pixmap.height() as i32;
                
                outline.draw(|gx, gy, coverage| {
                    let px = bx + gx as i32;
                    let py = by + gy as i32;
                    
                    // Apply Clipping check
                    let pxf = px as f32;
                    let pyf = py as f32;
                    if pxf >= clip.x && pxf < (clip.x + clip.width) &&
                       pyf >= clip.y && pyf < (clip.y + clip.height) {
                        if px >= 0 && py >= 0 && px < pw && py < ph {
                            blend_glyph_pixel(pixmap, px as u32, py as u32, coverage, &color);
                        }
                    }
                });
            }
            current_x += adv;
        }
        current_x += space_w;
    }
}

fn blend_glyph_pixel(
    pixmap: &mut Pixmap,
    x: u32,
    y: u32,
    coverage: f32,
    color: &crate::css::Color,
) {
    if coverage <= 0.0 || x >= pixmap.width() || y >= pixmap.height() {
        return;
    }

    let alpha = (coverage * (color.a as f32 / 255.0)).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }

    let index = (y * pixmap.width() + x) as usize;
    let pixels = pixmap.pixels_mut();
    let pixel = &mut pixels[index];
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
    )
    .premultiply();
}
