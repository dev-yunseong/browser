# Plan: Improved Parent-Child Rendering Containment

## Goals
1. Add `get_content_rect` to `LayoutBox` to calculate the inner area (excluding padding and borders).
2. Implement clipping in `src/render.rs` so children are contained within their parent's content area.
3. Refine border and background rendering for precision.
4. Ensure no regressions and verify with `cargo check`.

## 1. Layout Improvements (src/layout.rs)

### 1.1 Add `Rect::intersect`
Implement a method to calculate the intersection of two rectangles. This is essential for nested clipping.
```rust
impl Rect {
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let x2 = (self.x + self.width).min(other.x + other.width);
        let y2 = (self.y + self.height).min(other.y + other.height);
        Rect {
            x,
            y,
            width: (x2 - x).max(0.0),
            height: (y2 - y).max(0.0),
        }
    }
}
```

### 1.2 Add `LayoutBox::get_content_rect`
Return the rectangle representing the content area.
```rust
pub fn get_content_rect(&self) -> Rect {
    Rect {
        x: self.dimensions.x + self.border.left + self.padding.left,
        y: self.dimensions.y + self.border.top + self.padding.top,
        width: (self.dimensions.width - self.border.left - self.border.right - self.padding.left - self.padding.right).max(0.0),
        height: (self.dimensions.height - self.border.top - self.border.bottom - self.padding.top - self.padding.bottom).max(0.0),
    }
}
```

## 2. Rendering Improvements (src/render.rs)

### 2.1 Update `render_foregrounds` for Clipping
Add a `clip: Option<crate::layout::Rect>` parameter to `render_foregrounds`.

- For each `LayoutBox`:
  1. Calculate its `content_rect = self.get_content_rect()`.
  2. When rendering children, calculate `new_clip`:
     - If `clip` is `Some(current)`, `new_clip = Some(current.intersect(&content_rect))`.
     - If `clip` is `None`, `new_clip = Some(content_rect)`.
  3. Pass `new_clip` to recursive `render_foregrounds` calls.

### 2.2 Update `render_text_wrapped` for Clipping
Add `clip: Option<crate::layout::Rect>` to `render_text_wrapped`.
In the inner pixel loop of `outline.draw`:
```rust
outline.draw(|gx, gy, coverage| {
    let px = bx + gx as i32;
    let py = by + gy as i32;
    // Check against clip
    if let Some(c) = clip {
        if (px as f32) < c.x || (px as f32) >= c.x + c.width || (py as f32) < c.y || (py as f32) >= c.y + c.height {
            return;
        }
    }
    if px >= 0 && py >= 0 && px < pw && py < ph {
        blend_glyph_pixel(pixmap, px as u32, py as u32, coverage, &color);
    }
});
```

### 2.3 Precise Borders
Modify `render_backgrounds` to draw borders *inside* the border-box.
Instead of:
```rust
if let Some(r) = Rect::from_xywh(d.x, d.y, d.width, d.height) { ... stroke ... }
```
Use an inset rectangle:
```rust
let b = layout.border.left; // assuming uniform for now as per current layout.rs
if let Some(r) = tiny_skia::Rect::from_xywh(d.x + b/2.0, d.y + b/2.0, (d.width - b).max(0.0), (d.height - b).max(0.0)) {
    // stroke with width b
}
```

### 2.4 Image Clipping
For `DisplayType::Image`, if `clip` is present, use `tiny_skia::Mask::from_rect` to apply clipping to `draw_pixmap`.

## 3. Verification Plan
1. Run `cargo check` to verify types and syntax.
2. Ensure no regression in existing tests (`cargo test`).
3. (Visual) `cargo run` and check if elements are contained correctly.
