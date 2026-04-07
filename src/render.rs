use tiny_skia::{Pixmap, Paint, Rect, Transform, Color};
use crate::layout::LayoutBox;
use crate::css::Value;

pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap) {
    // Fill background if it exists
    if let Some(Value::Color(c)) = layout.style_node.specified_values.get("background") {
        let (r, g, b, a) = parse_color(c);
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, a);

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

    // A real browser would render text here using `ab_glyph` or similar

    // Render children
    for child in &layout.children {
        render_layout_tree(child, pixmap);
    }
}

fn parse_color(color_str: &str) -> (u8, u8, u8, u8) {
    if color_str == "#eee" {
        (238, 238, 238, 255)
    } else if color_str == "red" {
        (255, 0, 0, 255)
    } else if color_str == "blue" {
        (0, 0, 255, 255)
    } else {
        (255, 255, 255, 0) // transparent
    }
}
