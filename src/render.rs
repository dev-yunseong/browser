use tiny_skia::{Pixmap, Paint, Rect, Transform, Stroke, PathBuilder, FillRule};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::LayoutBox;
use crate::css::Value;
use markup5ever_rcdom::NodeData;

// Using a bundled font included in the project assets
const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap) {
    // 1. Render Background
    if let Some(Value::Color(c)) = layout.style_node.specified_values.get("background") {
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

    // 2. Render Border (Simplified: draw a stroke around the box)
    if let Some(Value::Keyword(border)) = layout.style_node.specified_values.get("border") {
        if !border.is_empty() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(0, 0, 0, 255); // Default black border
            
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
                    stroke.width = 1.0;
                    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                }
            }
        }
    }

    // 3. Render Text
    if let NodeData::Text { ref contents } = layout.style_node.node.data {
        let text = contents.borrow().to_string();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            render_text(trimmed, layout.dimensions.x, layout.dimensions.y + 15.0, pixmap);
        }
    }

    // Render children
    for child in &layout.children {
        render_layout_tree(child, pixmap);
    }
}

fn render_text(text: &str, x: f32, y: f32, pixmap: &mut Pixmap) {
    let font = FontRef::try_from_slice(FONT_DATA).unwrap();
    let scale = PxScale::from(16.0);
    
    let mut current_x = x;
    for c in text.chars() {
        let glyph_id = font.glyph_id(c);
        let glyph = glyph_id.with_scale_and_position(scale, point(current_x, y));
        
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            let mut mask = tiny_skia::Mask::new(bounds.width() as u32, bounds.height() as u32).unwrap();
            
            outline.draw(|x, y, c| {
                let i = (y * mask.width() as u32 + x) as usize;
                mask.data_mut()[i] = (c * 255.0) as u8;
            });
            
            let mut paint = Paint::default();
            paint.set_color_rgba8(0, 0, 0, 255); // Black text
            
            if let Some(rect) = Rect::from_xywh(bounds.min.x, bounds.min.y, bounds.width(), bounds.height()) {
                pixmap.fill_path(
                    &PathBuilder::from_rect(rect),
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    Some(&mask),
                );
            }
        }
        // ab_glyph v0.2.x uses h_advance_unscaled * scale_factor or similar
        // Or font.glyph_h_advance(glyph_id) * scale.x / units_per_em
        current_x += font.h_advance_unscaled(glyph_id) * (scale.x / font.units_per_em().unwrap() as f32);
    }
}
