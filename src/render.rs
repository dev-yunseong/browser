use tiny_skia::{Pixmap, Paint, Rect, Transform, Stroke, PathBuilder, FillRule};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, DisplayType};
use crate::css::{Value, Unit, BoxShadow};
use markup5ever_rcdom::NodeData;
use std::collections::HashMap;

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
    let d = layout.dimensions;

    // ── 1. Box shadow (drawn behind everything) ───────────────────────────────
    if let Some(Value::BoxShadow(shadow)) = layout.style_node.specified_values.get("box-shadow") {
        if !shadow.inset {
            render_box_shadow(pixmap, d.x, d.y, d.width, d.height, shadow);
        }
    }

    // ── 2. Background ─────────────────────────────────────────────────────────
    let bg_color = layout.style_node.specified_values.get("background-color")
        .or_else(|| layout.style_node.specified_values.get("background"));

    if let Some(Value::Color(c)) = bg_color {
        if c.a > 0 {
            let mut paint = Paint::default();
            paint.set_color_rgba8(c.r, c.g, c.b, c.a);
            paint.anti_alias = true;

            let radius = get_border_radius(layout, d);
            if radius > 0.0 {
                if let Some(path) = build_rounded_rect_path(d.x, d.y, d.width, d.height, radius) {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                }
            } else if let Some(rect) = Rect::from_xywh(d.x, d.y, d.width, d.height) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
    }

    // ── 3. Border ─────────────────────────────────────────────────────────────
    let border_width = match layout.style_node.specified_values.get("border-width") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => match layout.display {
            DisplayType::TableCell | DisplayType::Input => 1.0,
            _ => 0.0,
        },
    };

    if border_width > 0.0 {
        let border_color = match layout.style_node.specified_values.get("border-color") {
            Some(Value::Color(c)) => c.clone(),
            _ => crate::css::Color { r: 180, g: 180, b: 180, a: 255 },
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(border_color.r, border_color.g, border_color.b, border_color.a);
        paint.anti_alias = true;

        let radius = get_border_radius(layout, d);
        if radius > 0.0 {
            if let Some(path) = build_rounded_rect_path(d.x, d.y, d.width, d.height, radius) {
                let mut stroke = Stroke::default();
                stroke.width = border_width;
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        } else if let Some(rect) = Rect::from_xywh(d.x, d.y, d.width, d.height) {
            let mut pb = PathBuilder::new();
            pb.move_to(rect.left(), rect.top());
            pb.line_to(rect.right(), rect.top());
            pb.line_to(rect.right(), rect.bottom());
            pb.line_to(rect.left(), rect.bottom());
            pb.close();
            if let Some(path) = pb.finish() {
                let mut stroke = Stroke::default();
                stroke.width = border_width;
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
    }

    // ── 4. List item bullet ───────────────────────────────────────────────────
    if let DisplayType::ListItem = layout.display {
        let mut paint = Paint::default();
        paint.set_color_rgba8(0, 0, 0, 200);
        if let Some(bullet) = Rect::from_xywh(d.x - 14.0, d.y + 8.0, 5.0, 5.0) {
            pixmap.fill_rect(bullet, &paint, Transform::identity(), None);
        }
    }

    // ── 5. Image ──────────────────────────────────────────────────────────────
    if let DisplayType::Image = layout.display {
        render_image(layout, pixmap, image_cache);
    }

    // ── 6. Text ───────────────────────────────────────────────────────────────
    if let NodeData::Text { ref contents } = layout.style_node.node.data {
        let text = contents.borrow().to_string();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let color = match layout.style_node.specified_values.get("color") {
                Some(Value::Color(c)) => c.clone(),
                _ => crate::css::Color { r: 0, g: 0, b: 0, a: 255 },
            };
            let is_link = layout.link_url.is_some();
            let final_color = if is_link {
                crate::css::Color { r: 0, g: 0, b: 238, a: 255 }
            } else {
                color
            };
            let font_size = match layout.style_node.specified_values.get("font-size") {
                Some(Value::Length(v, Unit::Px)) => *v,
                Some(Value::Length(v, Unit::Em)) => v * 16.0,
                _ => 16.0,
            };
            render_text_wrapped(trimmed, layout, pixmap, final_color, font_size, is_link);
        }
    }

    // ── 7. Render children ────────────────────────────────────────────────────
    for child in &layout.children {
        render_layout_tree(child, pixmap, image_cache);
    }
}

fn get_border_radius(layout: &LayoutBox, d: crate::layout::Rect) -> f32 {
    match layout.style_node.specified_values.get("border-radius") {
        Some(Value::Length(v, Unit::Px)) => v.min(d.width / 2.0).min(d.height / 2.0),
        Some(Value::Length(v, Unit::Percent)) => {
            (d.width.min(d.height) * (v / 100.0)).min(d.width / 2.0).min(d.height / 2.0)
        }
        _ => 0.0,
    }
}

/// Builds a rounded rectangle path using cubic Bézier curves.
pub fn build_rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    if w <= 0.0 || h <= 0.0 { return None; }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    // Bézier control point ratio for approximating a quarter-circle
    let k = 0.5522848;
    let kr = k * r;

    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    // top-right
    pb.cubic_to(x + w - r + kr, y, x + w, y + r - kr, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    // bottom-right
    pb.cubic_to(x + w, y + h - r + kr, x + w - r + kr, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    // bottom-left
    pb.cubic_to(x + r - kr, y + h, x, y + h - r + kr, x, y + h - r);
    pb.line_to(x, y + r);
    // top-left
    pb.cubic_to(x, y + r - kr, x + r - kr, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Approximate box-shadow with layered semi-transparent rects.
fn render_box_shadow(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, shadow: &BoxShadow) {
    let steps = 6u32;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let spread = shadow.blur * t;
        let alpha = ((1.0 - t) * shadow.color.a as f32 / steps as f32) as u8;
        if alpha == 0 { continue; }

        let mut paint = Paint::default();
        paint.set_color_rgba8(shadow.color.r, shadow.color.g, shadow.color.b, alpha);
        paint.anti_alias = true;

        let sx = x + shadow.offset_x - spread;
        let sy = y + shadow.offset_y - spread;
        let sw = w + spread * 2.0 + shadow.spread * 2.0;
        let sh = h + spread * 2.0 + shadow.spread * 2.0;

        let blur_radius = (shadow.blur * (1.0 - t * 0.5)).max(0.0);
        if blur_radius > 0.5 {
            if let Some(path) = build_rounded_rect_path(sx, sy, sw, sh, blur_radius) {
                pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
            }
        } else if let Some(rect) = Rect::from_xywh(sx, sy, sw, sh) {
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }
}

fn render_image(layout: &LayoutBox, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
    let d = layout.dimensions;
    if let Some(ref url) = layout.image_url {
        if let Some(data) = image_cache.get(url) {
            if let Ok(img) = image::load_from_memory(data) {
                let rgba = img.to_rgba8();
                let iw = rgba.width();
                let ih = rgba.height();
                if iw == 0 || ih == 0 { return; }
                if let Some(mut img_pixmap) = Pixmap::new(iw, ih) {
                    img_pixmap.data_mut().copy_from_slice(&rgba);
                    let scale_x = d.width / iw as f32;
                    let scale_y = d.height / ih as f32;
                    pixmap.draw_pixmap(
                        d.x as i32,
                        d.y as i32,
                        img_pixmap.as_ref(),
                        &tiny_skia::PixmapPaint::default(),
                        Transform::from_scale(scale_x, scale_y),
                        None,
                    );
                }
                return;
            }
        }
    }
    // Placeholder for not-yet-loaded images
    let mut paint = Paint::default();
    paint.set_color_rgba8(230, 230, 230, 255);
    if let Some(rect) = Rect::from_xywh(d.x, d.y, d.width, d.height) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
    // Draw a simple "broken image" X
    let mut paint2 = Paint::default();
    paint2.set_color_rgba8(180, 180, 180, 255);
    let cx = d.x + d.width / 2.0;
    let cy = d.y + d.height / 2.0;
    let s = (d.width.min(d.height) * 0.2).min(20.0);
    if let Some(rect) = Rect::from_xywh(cx - s * 0.1, cy - s, s * 0.2, s * 2.0) {
        pixmap.fill_rect(rect, &paint2, Transform::identity(), None);
    }
}

fn render_text_wrapped(
    text: &str,
    layout: &LayoutBox,
    pixmap: &mut Pixmap,
    color: crate::css::Color,
    font_size: f32,
    is_link: bool,
) {
    let font = match FontRef::try_from_slice(FONT_DATA) {
        Ok(f) => f,
        Err(_) => return,
    };
    let scale = PxScale::from(font_size);
    let units = font.units_per_em().unwrap_or(1000.0) as f32;

    let start_x = layout.dimensions.x;
    let mut current_y = layout.dimensions.y + (font_size * 0.85);
    let pix_w = pixmap.width() as i32;
    let pix_h = pixmap.height() as i32;

    let avg_char_width = 8.0 * (font_size / 16.0);
    let max_chars = (layout.dimensions.width / avg_char_width).max(1.0) as usize;

    // Word-wrap
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut current_line = String::new();
    let mut lines: Vec<String> = Vec::new();

    for word in words {
        if current_line.len() + word.len() + 1 > max_chars && !current_line.is_empty() {
            lines.push(current_line.clone());
            current_line = word.to_string();
        } else {
            if !current_line.is_empty() { current_line.push(' '); }
            current_line.push_str(word);
        }
    }
    if !current_line.is_empty() { lines.push(current_line); }

    for line in &lines {
        let mut current_x = start_x;
        let line_start_x = current_x;
        let mut line_end_x = current_x;

        for c in line.chars() {
            let glyph_id = font.glyph_id(c);
            let glyph = glyph_id.with_scale_and_position(scale, point(current_x, current_y));

            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    if coverage > 0.01 {
                        let px = bounds.min.x as i32 + gx as i32;
                        let py = bounds.min.y as i32 + gy as i32;
                        if px >= 0 && px < pix_w && py >= 0 && py < pix_h {
                            let idx = ((py as u32 * pix_w as u32 + px as u32) * 4) as usize;
                            let data = pixmap.data_mut();
                            let alpha = (coverage * color.a as f32) as u8;
                            if alpha > 0 {
                                let old_a = data[idx + 3] as f32 / 255.0;
                                let new_a = alpha as f32 / 255.0;
                                let out_a = new_a + old_a * (1.0 - new_a);
                                if out_a > 0.0 {
                                    data[idx]     = ((color.r as f32 * new_a + data[idx]     as f32 * old_a * (1.0 - new_a)) / out_a) as u8;
                                    data[idx + 1] = ((color.g as f32 * new_a + data[idx + 1] as f32 * old_a * (1.0 - new_a)) / out_a) as u8;
                                    data[idx + 2] = ((color.b as f32 * new_a + data[idx + 2] as f32 * old_a * (1.0 - new_a)) / out_a) as u8;
                                    data[idx + 3] = (out_a * 255.0) as u8;
                                }
                            }
                        }
                    }
                });
            }
            let advance = font.h_advance_unscaled(glyph_id) * (scale.x / units);
            current_x += advance;
            line_end_x = current_x;
        }

        // Underline for links
        if is_link {
            let mut paint = Paint::default();
            paint.set_color_rgba8(color.r, color.g, color.b, 200);
            if let Some(line_rect) = Rect::from_xywh(
                line_start_x,
                current_y + 2.0,
                (line_end_x - line_start_x).max(1.0),
                1.0,
            ) {
                pixmap.fill_rect(line_rect, &paint, Transform::identity(), None);
            }
        }

        current_y += font_size * 1.4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_loading() {
        let font = FontRef::try_from_slice(FONT_DATA);
        assert!(font.is_ok());
    }

    #[test]
    fn test_rounded_rect_path() {
        let path = build_rounded_rect_path(0.0, 0.0, 100.0, 50.0, 8.0);
        assert!(path.is_some());
    }

    #[test]
    fn test_rounded_rect_path_zero_radius() {
        let path = build_rounded_rect_path(0.0, 0.0, 100.0, 50.0, 0.0);
        assert!(path.is_some());
    }
}
