use tiny_skia::{Pixmap, Paint, Transform, Stroke, PathBuilder};
use ab_glyph::{Font, FontRef, PxScale, point};
use crate::layout::{LayoutBox, DisplayType, Rect as LayoutRect};
use crate::css::{Value, Color};
use markup5ever_rcdom::NodeData;
use std::collections::HashMap;

const FONT_DATA: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

#[derive(Debug, Clone)]
pub enum PaintCommand {
    Rect(LayoutRect, Color),
    Border(LayoutRect, f32, Color),
    Image(LayoutRect, String),
    Text {
        rect: LayoutRect,
        text: String,
        font_size: f32,
        color: Color,
        clip: LayoutRect,
    },
    Shadow(LayoutRect, crate::css::BoxShadow),
}

pub struct DisplayList {
    pub commands: Vec<PaintCommand>,
}

impl DisplayList {
    pub fn new() -> Self {
        Self { commands: Vec::new() }
    }

    pub fn add(&mut self, command: PaintCommand) {
        self.commands.push(command);
    }
}

pub fn render_layout_tree(layout: &LayoutBox, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
    let mut display_list = DisplayList::new();
    
    // Pass 1: Backgrounds and Shadows
    generate_backgrounds(layout, &mut display_list);
    
    // Pass 2: Foregrounds (Images, Text)
    let initial_clip = LayoutRect {
        x: 0.0,
        y: 0.0,
        width: pixmap.width() as f32,
        height: pixmap.height() as f32,
    };
    generate_foregrounds(layout, &mut display_list, initial_clip);

    // Execute Display List
    execute_display_list(&display_list, pixmap, image_cache);
}

fn generate_backgrounds(layout: &LayoutBox, list: &mut DisplayList) {
    let d = layout.dimensions;
    if d.width < 0.1 || d.height < 0.1 {
        for child in &layout.children { generate_backgrounds(child, list); }
        return;
    }

    // ── 1. Box Shadow ──
    if let Some(Value::BoxShadow(shadow)) = layout.style_node.specified_values.get("box-shadow") {
        if !shadow.inset {
            list.add(PaintCommand::Shadow(d, shadow.clone()));
        }
    }

    // ── 2. Background ──
    let bg = layout.style_node.specified_values.get("background-color").or(layout.style_node.specified_values.get("background"));
    if let Some(Value::Color(c)) = bg {
        if c.a > 0 {
            list.add(PaintCommand::Rect(d, c.clone()));
        }
    }

    // ── 3. Border ──
    if layout.border.left > 0.0 {
        let color = match layout.style_node.specified_values.get("border-color") {
            Some(Value::Color(c)) => c.clone(),
            _ => Color { r: 180, g: 180, b: 180, a: 255 },
        };
        list.add(PaintCommand::Border(d, layout.border.left, color));
    }

    for child in &layout.children {
        generate_backgrounds(child, list);
    }
}

fn generate_foregrounds(layout: &LayoutBox, list: &mut DisplayList, clip: LayoutRect) {
    let d = layout.dimensions;
    if d.width < 0.1 || d.height < 0.1 {
        for child in &layout.children { generate_foregrounds(child, list, clip); }
        return;
    }

    // ── 4. Image ──
    if layout.display == DisplayType::Image {
        if let Some(ref url) = layout.image_url {
            list.add(PaintCommand::Image(d, url.clone()));
        }
    }

    // ── 5. Text ──
    if let NodeData::Text { ref contents } = layout.style_node.node.data {
        let font_size = match layout.style_node.specified_values.get("font-size") {
            Some(Value::Length(v, _)) => *v,
            _ => 16.0,
        };
        let color = match layout.style_node.specified_values.get("color") {
            Some(Value::Color(c)) => c.clone(),
            _ => Color { r: 0, g: 0, b: 0, a: 255 },
        };
        list.add(PaintCommand::Text {
            rect: d,
            text: contents.borrow().to_string(),
            font_size,
            color,
            clip,
        });
    }

    let next_clip = clip.intersect(&layout.get_content_rect());
    for child in &layout.children {
        generate_foregrounds(child, list, next_clip);
    }
}

fn execute_display_list(list: &DisplayList, pixmap: &mut Pixmap, image_cache: &HashMap<String, Vec<u8>>) {
    for cmd in &list.commands {
        match cmd {
            PaintCommand::Rect(r, c) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                if let Some(tr) = tiny_skia::Rect::from_xywh(r.x, r.y, r.width, r.height) {
                    pixmap.fill_rect(tr, &paint, Transform::identity(), None);
                }
            }
            PaintCommand::Border(r, w, c) => {
                let mut paint = Paint::default();
                paint.set_color_rgba8(c.r, c.g, c.b, c.a);
                let mut stroke = Stroke::default();
                stroke.width = *w;
                if let Some(tr) = tiny_skia::Rect::from_xywh(r.x + w/2.0, r.y + w/2.0, (r.width - w).max(0.0), (r.height - w).max(0.0)) {
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
                            pixmap.draw_pixmap(r.x as i32, r.y as i32, img_pixmap.as_ref(), &tiny_skia::PixmapPaint::default(), 
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
                    if s.blur > 0.0 {
                        paint.set_color_rgba8(s.color.r, s.color.g, s.color.b, s.color.a / 4);
                        let b = s.blur / 2.0;
                        if let Some(r2) = tiny_skia::Rect::from_xywh(sx - b, sy - b, sw + s.blur, sh + s.blur) {
                            pixmap.fill_rect(r2, &paint, Transform::identity(), None);
                        }
                    }
                }
            }
        }
    }
}

fn render_text_raw(text: String, rect: LayoutRect, font_size: f32, color: &Color, clip: LayoutRect, pixmap: &mut Pixmap) {
    let trimmed = text.trim();
    if trimmed.is_empty() { return; }

    let font = FontRef::try_from_slice(FONT_DATA).unwrap();
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
