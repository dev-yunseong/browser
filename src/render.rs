use tiny_skia::{Pixmap, Paint, Rect, Transform, Stroke, PathBuilder, FillRule};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::LayoutBox;
use crate::css::{Value, Unit};
use markup5ever_rcdom::NodeData;

// Using a bundled font included in the project assets
const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap) {
    // 1. Render Background
    if let Some(Value::Color(c)) = layout.style_node.specified_values.get("background") {
        if c.a > 0 {
            let mut paint = Paint::default();
            paint.set_color_rgba8(c.r, c.g, c.b, c.a);

            if layout.dimensions.width > 0.0 && layout.dimensions.height > 0.0 {
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
    }

    // 2. Render Border
    let border_width = match layout.style_node.specified_values.get("border-width") {
        Some(Value::Length(v, Unit::Px)) => *v,
        _ => 0.0,
    };
    
    if border_width > 0.0 {
        let border_color = match layout.style_node.specified_values.get("border-color") {
            Some(Value::Color(c)) => c.clone(),
            _ => crate::css::Color { r: 0, g: 0, b: 0, a: 255 },
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

    // 3. Render Text (Only for Text nodes)
    if let NodeData::Text { ref contents } = layout.style_node.node.data {
        let text = contents.borrow().to_string();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            let color = match layout.style_node.specified_values.get("color") {
                Some(Value::Color(c)) => c.clone(),
                _ => crate::css::Color { r: 0, g: 0, b: 0, a: 255 },
            };
            let font_size = match layout.style_node.specified_values.get("font-size") {
                Some(Value::Length(v, Unit::Px)) => *v,
                _ => 16.0,
            };
            render_text(trimmed, layout.dimensions.x, layout.dimensions.y, pixmap, color, font_size);
        }
    }

    // Render children
    for child in &layout.children {
        render_layout_tree(child, pixmap);
    }
}

fn render_text(text: &str, x: f32, y: f32, pixmap: &mut Pixmap, color: crate::css::Color, font_size: f32) {
    let font = match FontRef::try_from_slice(FONT_DATA) {
        Ok(f) => f,
        Err(_) => return,
    };
    let scale = PxScale::from(font_size);
    
    let mut current_x = x;
    let baseline_y = y + (font_size * 0.85); // Adjust baseline based on font size
    let pix_width = pixmap.width();
    let pix_height = pixmap.height();

    for c in text.chars() {
        let glyph_id = font.glyph_id(c);
        let glyph = glyph_id.with_scale_and_position(scale, point(current_x, baseline_y));
        
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                if coverage > 0.0 {
                    let px = (bounds.min.x as u32 + gx) as i32;
                    let py = (bounds.min.y as u32 + gy) as i32;
                    
                    if px >= 0 && px < pix_width as i32 && py >= 0 && py < pix_height as i32 {
                        let idx = ((py as u32 * pix_width + px as u32) * 4) as usize;
                        let data = pixmap.data_mut();
                        
                        let alpha = (coverage * color.a as f32) as u8;
                        if alpha > 0 {
                            data[idx] = color.r;
                            data[idx + 1] = color.g;
                            data[idx + 2] = color.b;
                            data[idx + 3] = alpha;
                        }
                    }
                }
            });
        }
        current_x += font.h_advance_unscaled(glyph_id) * (scale.x / font.units_per_em().unwrap_or(1000.0) as f32);
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
