use tiny_skia::{Pixmap, Paint, Rect, Transform, Stroke, PathBuilder};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, DisplayType};
use crate::css::{Value, Unit};
use markup5ever_rcdom::NodeData;
use std::collections::HashMap;

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
    // 1. Render Background
    let bg_color = layout.style_node.specified_values.get("background-color")
        .or_else(|| layout.style_node.specified_values.get("background"));

    if let Some(Value::Color(c)) = bg_color {
        if c.a > 0 {
            let mut paint = Paint::default();
            paint.set_color_rgba8(c.r, c.g, c.b, c.a);

            if let Some(rect) = Rect::from_xywh(
                layout.dimensions.x,
                layout.dimensions.y,
                layout.dimensions.width,
                layout.dimensions.height,
            ) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
    }

    // 2. Render Border
    let border_width = match layout.style_node.specified_values.get("border-width") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => if let DisplayType::TableCell | DisplayType::Input = layout.display { 1.0 } else { 0.0 }, 
    };
    
    if border_width > 0.0 {
        let border_color = match layout.style_node.specified_values.get("border-color") {
            Some(Value::Color(c)) => c.clone(),
            _ => crate::css::Color { r: 180, g: 180, b: 180, a: 255 },
        };
        
        let mut paint = Paint::default();
        paint.set_color_rgba8(border_color.r, border_color.g, border_color.b, border_color.a);
        
        if let Some(rect) = Rect::from_xywh(
            layout.dimensions.x,
            layout.dimensions.y,
            layout.dimensions.width,
            layout.dimensions.height,
        ) {
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

    // 3. Render Special Elements (Bullets for ListItem)
    if let DisplayType::ListItem = layout.display {
        let mut paint = Paint::default();
        paint.set_color_rgba8(0, 0, 0, 255);
        if let Some(bullet_rect) = Rect::from_xywh(layout.dimensions.x - 12.0, layout.dimensions.y + 8.0, 4.0, 4.0) {
            pixmap.fill_rect(bullet_rect, &paint, Transform::identity(), None);
        }
    }

    // 4. Render Image
    if let DisplayType::Image = layout.display {
        if let Some(ref url) = layout.image_url {
            if let Some(data) = image_cache.get(url) {
                if let Ok(img) = image::load_from_memory(data) {
                    let rgba = img.to_rgba8();
                    let width = rgba.width();
                    let height = rgba.height();
                    
                    if let Some(mut img_pixmap) = Pixmap::new(width, height) {
                        img_pixmap.data_mut().copy_from_slice(&rgba);
                        
                        let scale_x = layout.dimensions.width / width as f32;
                        let scale_y = layout.dimensions.height / height as f32;
                        
                        pixmap.draw_pixmap(
                            layout.dimensions.x as i32,
                            layout.dimensions.y as i32,
                            img_pixmap.as_ref(),
                            &tiny_skia::PixmapPaint::default(),
                            Transform::from_scale(scale_x, scale_y),
                            None
                        );
                    }
                }
            } else {
                let mut paint = Paint::default();
                paint.set_color_rgba8(240, 240, 240, 255);
                if let Some(rect) = Rect::from_xywh(layout.dimensions.x, layout.dimensions.y, layout.dimensions.width, layout.dimensions.height) {
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
        }
    }

    // 5. Render Text
    if let NodeData::Text { ref contents } = layout.style_node.node.data {
        let text = contents.borrow().to_string();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let mut color = match layout.style_node.specified_values.get("color") {
                Some(Value::Color(c)) => c.clone(),
                _ => crate::css::Color { r: 0, g: 0, b: 0, a: 255 },
            };

            let mut is_link = false;
            if layout.link_url.is_some() {
                color = crate::css::Color { r: 0, g: 0, b: 238, a: 255 };
                is_link = true;
            }

            let font_size = match layout.style_node.specified_values.get("font-size") {
                Some(Value::Length(v, Unit::Px)) => *v,
                _ => 16.0,
            };

            render_text_wrapped(trimmed, layout, pixmap, color, font_size, is_link);
        }
    }

    // Render children
    for child in &layout.children {
        render_layout_tree(child, pixmap, image_cache);
    }
}

fn render_text_wrapped(text: &str, layout: &LayoutBox, pixmap: &mut Pixmap, color: crate::css::Color, font_size: f32, is_link: bool) {
    let font = match FontRef::try_from_slice(FONT_DATA) {
        Ok(f) => f,
        Err(_) => return,
    };
    let scale = PxScale::from(font_size);

    let start_x = layout.dimensions.x;
    let mut current_y = layout.dimensions.y + (font_size * 0.85);
    let pix_width = pixmap.width();
    let pix_height = pixmap.height();

    let avg_char_width = 8.0 * (font_size / 16.0);
    let max_chars = (layout.dimensions.width / avg_char_width).max(1.0) as usize;
    
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut current_line = String::new();
    let mut lines = Vec::new();

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

    for line in lines {
        let mut current_x = start_x;
        let line_start_x = current_x;
        for c in line.chars() {
            let glyph_id = font.glyph_id(c);
            let glyph = glyph_id.with_scale_and_position(scale, point(current_x, current_y));
            
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    if coverage > 0.0 {
                        let px = bounds.min.x as i32 + gx as i32 ;
                        let py = bounds.min.y as i32 + gy as i32 ;
                        if px >= 0 && px < pix_width as i32 && py >= 0 && py < pix_height as i32 {
                            let idx = ((py as u32 * pix_width + px as u32) * 4) as usize;
                            let data = pixmap.data_mut();
                            let alpha = (coverage * color.a as f32) as u8;
                            if alpha > 0 {
                                let old_a = data[idx + 3] as f32 / 255.0;
                                let new_a = alpha as f32 / 255.0;
                                let out_a = new_a + old_a * (1.0 - new_a);
                                if out_a > 0.0 {
                                    data[idx] = ((color.r as f32 * new_a + data[idx] as f32 * old_a * (1.0 - new_a)) / out_a) as u8;
                                    data[idx + 1] = ((color.g as f32 * new_a + data[idx + 1] as f32 * old_a * (1.0 - new_a)) / out_a) as u8;
                                    data[idx + 2] = ((color.b as f32 * new_a + data[idx + 2] as f32 * old_a * (1.0 - new_a)) / out_a) as u8;
                                    data[idx + 3] = (out_a * 255.0) as u8;
                                }
                            }
                        }
                    }
                });
            }
            current_x += font.h_advance_unscaled(glyph_id) * (scale.x / font.units_per_em().unwrap_or(1000.0) as f32);
        }

        if is_link {
            let mut paint = Paint::default();
            paint.set_color_rgba8(color.r, color.g, color.b, 255);
            if let Some(line_rect) = Rect::from_xywh(line_start_x, current_y + 2.0, current_x - line_start_x, 1.0) {
                pixmap.fill_rect(line_rect, &paint, Transform::identity(), None);
            }
        }

        current_y += font_size * 1.25; 
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
}
