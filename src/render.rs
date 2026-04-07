use tiny_skia::{Pixmap, Paint, Rect, Transform, Stroke, PathBuilder, FillRule};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, DisplayType};
use crate::css::{Value, Unit};
use markup5ever_rcdom::NodeData;
use std::collections::HashMap;

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
    let d = layout.dimensions;

    // 가시성이 없는 박스는 렌더링 스킵 (잔상 및 검은 블록 방지)
    if d.width < 0.1 || d.height < 0.1 {
        for child in &layout.children { render_layout_tree(child, pixmap, image_cache); }
        return;
    }

    // ── 1. Box Shadow ──
    if let Some(Value::BoxShadow(shadow)) = layout.style_node.specified_values.get("box-shadow") {
        if !shadow.inset {
            let mut paint = Paint::default();
            paint.set_color_rgba8(shadow.color.r, shadow.color.g, shadow.color.b, shadow.color.a / 3);
            if let Some(r) = Rect::from_xywh(d.x + shadow.offset_x, d.y + shadow.offset_y, d.width, d.height) {
                pixmap.fill_rect(r, &paint, Transform::identity(), None);
            }
        }
    }

    // ── 2. Background ──
    let bg = layout.style_node.specified_values.get("background-color").or(layout.style_node.specified_values.get("background"));
    if let Some(Value::Color(c)) = bg {
        if c.a > 0 {
            let mut paint = Paint::default();
            paint.set_color_rgba8(c.r, c.g, c.b, c.a);
            if let Some(r) = Rect::from_xywh(d.x, d.y, d.width, d.height) {
                pixmap.fill_rect(r, &paint, Transform::identity(), None);
            }
        }
    }

    // ── 3. Border ──
    if layout.border.left > 0.0 {
        let mut paint = Paint::default();
        paint.set_color_rgba8(180, 180, 180, 255); // 기본 회색 테두리
        let mut stroke = Stroke::default();
        stroke.width = layout.border.left;
        if let Some(r) = Rect::from_xywh(d.x, d.y, d.width, d.height) {
            let mut pb = PathBuilder::new();
            pb.push_rect(r);
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
    }

    // ── 4. Image ──
    if layout.display == DisplayType::Image {
        if let Some(ref url) = layout.image_url {
            if let Some(data) = image_cache.get(url) {
                if let Ok(img) = image::load_from_memory(data) {
                    let rgba = img.to_rgba8();
                    if let Some(mut img_pixmap) = Pixmap::new(rgba.width(), rgba.height()) {
                        img_pixmap.data_mut().copy_from_slice(&rgba);
                        pixmap.draw_pixmap(d.x as i32, d.y as i32, img_pixmap.as_ref(), &tiny_skia::PixmapPaint::default(), 
                            Transform::from_scale(d.width / rgba.width() as f32, d.height / rgba.height() as f32), None);
                    }
                }
            }
        }
    }

    // ── 5. Text ──
    if let NodeData::Text { ref contents } = layout.style_node.node.data {
        render_text_wrapped(contents.borrow().to_string(), layout, pixmap);
    }

    // ── 6. Children ──
    for child in &layout.children {
        render_layout_tree(child, pixmap, image_cache);
    }
}

fn render_text_wrapped(text: String, layout: &LayoutBox, pixmap: &mut Pixmap) {
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

    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    paint.anti_alias = true;

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

        if current_x + word_width_check(word_w, layout, current_x) > layout.dimensions.x + layout.dimensions.width + 1.0 && current_x > layout.dimensions.x {
            current_x = layout.dimensions.x;
            current_y += font_size * 1.4;
        }

        for (gid, adv) in glyphs {
            let glyph = gid.with_scale_and_position(scale, point(current_x, current_y));
            if let Some(outline) = font.outline_glyph(glyph) {
                let mut pb = PathBuilder::new();
                outline.draw(|gx, gy, coverage| {
                    if coverage > 0.1 {
                        if let Some(r) = Rect::from_xywh(gx as f32, gy as f32, 1.0, 1.0) {
                            pb.push_rect(r);
                        }
                    }
                });
                if let Some(path) = pb.finish() {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                }
            }
            current_x += adv;
        }
        current_x += space_w;
    }
}

fn word_width_check(w: f32, _l: &LayoutBox, _cx: f32) -> f32 { w }
