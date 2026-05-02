use crate::layout::{LayoutBox, DisplayType, PositionType, Rect as LayoutRect};
use crate::css::{Value, Color, BoxShadow, TransformOp, GradientValue, CssColorStop, LinearDirection};
use crate::matrix::{Matrix3x3, Matrix4x4};
use markup5ever_rcdom::NodeData;

// ── Object-Fit ────────────────────────────────────────────────────────────────

/// CSS `object-fit` property values controlling how an image fills its layout rect.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectFit {
    /// Stretch to fill (default). Aspect ratio is not preserved.
    Fill,
    /// Scale uniformly to fit inside the rect; letterbox with transparency.
    Contain,
    /// Scale uniformly to fill the rect; crop overflow.
    Cover,
    /// Use intrinsic image size; clip to rect.
    None,
}

// ── Paint Commands ────────────────────────────────────────────────────────────

/// A single atomic drawing operation. Moved from render.rs so that layer_tree.rs
/// owns the data pipeline (layout → layer tree → paint commands) while render.rs
/// owns the pixel execution (paint commands → Pixmap).
#[derive(Debug, Clone)]
pub enum PaintCommand {
    /// Filled rectangle: (bounds, color, corner-radius)
    Rect(LayoutRect, Color, f32),
    /// Stroked rectangle border: (bounds, stroke-width, color, corner-radius)
    Border(LayoutRect, f32, Color, f32),
    /// Image: layout rect, source URL, object-fit mode, alt text
    Image { rect: LayoutRect, url: String, object_fit: ObjectFit, alt: String },
    /// Text run with clipping rect
    Text {
        rect: LayoutRect,
        text: String,
        font_size: f32,
        color: Color,
        clip: LayoutRect,
        /// `true` when `font-weight: bold` (or numeric >= 600)
        bold: bool,
        /// `true` when `font-style: italic` or `oblique`
        italic: bool,
        /// Bitmask: bit 0 = underline, bit 1 = line-through, bit 2 = overline
        text_decoration: u8,
    },
    /// Outer box-shadow
    Shadow(LayoutRect, BoxShadow),
    /// CSS `linear-gradient()` background fill.
    LinearGradient {
        rect: LayoutRect,
        direction: LinearDirection,
        stops: Vec<CssColorStop>,
        radius: f32,
    },
    /// CSS `radial-gradient()` background fill.
    RadialGradient {
        rect: LayoutRect,
        stops: Vec<CssColorStop>,
        radius: f32,
    },
    /// Push a clip region onto the clip stack.
    /// All subsequent commands are clipped to `rect` (optionally with rounded corners
    /// when `radius` > 0). Paired with `PopClip`.
    PushClip { rect: LayoutRect, radius: f32 },
    /// Pop the most recently pushed clip region from the clip stack.
    PopClip,
}

// ── Compositing Triggers ──────────────────────────────────────────────────────

/// CSS properties that cause a `LayoutBox` to establish a compositing layer.
///
/// See: https://developer.mozilla.org/en-US/docs/Web/CSS/CSS_positioned_layout/Understanding_z-index/Stacking_context
#[derive(Debug, Clone, PartialEq)]
pub enum CompositingTrigger {
    /// Non-zero `z-index` (only meaningful on positioned elements in practice).
    ZIndex(i32),
    /// `opacity` < 1.0
    Opacity(f32),
    /// `transform` property with a resolved matrix
    Transform(Matrix4x4),
    /// `position: fixed`
    ///
    /// The layout engine resolves `top`/`left`/`right`/`bottom` against the
    /// viewport (0, 0, vw, vh) so fixed elements are correctly positioned.
    PositionFixed,
    /// `position: sticky`
    ///
    /// Approximated as `position: relative` with offset resolution. True
    /// scroll-threshold behaviour requires a compositor pass (future work).
    PositionSticky,
    /// `will-change` with a value other than `auto`
    WillChange(String),
}

// ── Layer ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Tile {
    pub rect: LayoutRect,
    pub background_commands: Vec<PaintCommand>,
    pub content_commands: Vec<PaintCommand>,
    pub dirty: bool,
}

impl Tile {
    pub fn new(rect: LayoutRect) -> Self {
        Self {
            rect,
            background_commands: Vec::new(),
            content_commands: Vec::new(),
            dirty: true,
        }
    }
}

/// A single compositing layer. Contains the paint commands for all boxes that
/// belong to this layer's stacking context.
#[derive(Debug, Clone)]
pub struct Layer {
    /// Stable identifier — equal to the layer's index in `LayerTree::layers`.
    pub id: usize,
    /// CSS `z-index` of the element that established this layer.
    pub z_index: i32,
    /// CSS `opacity` of the element that established this layer (1.0 = fully opaque).
    pub opacity: f32,
    /// Bounding box of the element that established this layer.
    pub bounds: LayoutRect,
    /// CSS properties that caused this layer to be created.
    pub triggers: Vec<CompositingTrigger>,
    /// Transformation matrix for this layer.
    pub transform: Matrix4x4,
    /// Ordered list of tiles for this layer (256x256 each).
    pub tiles: Vec<Tile>,
    /// Background and borders of the element that established this layer.
    pub background_commands: Vec<PaintCommand>,
    /// In-flow content commands (descendants that don't create layers).
    pub content_commands: Vec<PaintCommand>,
    /// IDs of layers created as direct children of this layer during tree build.
    /// Retained for use by the future compositor (issue #33); not used during
    /// the flat z-index sorted rendering pass implemented in this issue.
    pub child_layer_ids: Vec<usize>,
}

impl Layer {
    fn new(id: usize, z_index: i32, opacity: f32, bounds: LayoutRect, triggers: Vec<CompositingTrigger>, transform: Matrix4x4) -> Self {
        // Guard against non-finite dimensions (e.g. INFINITY from unconstrained flex
        // measurement) which would cause the tile loop below to spin forever.
        let bounds = LayoutRect {
            x: if bounds.x.is_finite() { bounds.x } else { 0.0 },
            y: if bounds.y.is_finite() { bounds.y } else { 0.0 },
            width:  if bounds.width.is_finite()  { bounds.width.max(0.0)  } else { 0.0 },
            height: if bounds.height.is_finite() { bounds.height.max(0.0) } else { 0.0 },
        };
        let mut tiles = Vec::new();
        let tile_size = 256.0;
        
        // Only subdivide the root layer or large layers. 
        // Small layers (most divs) get 1 tile.
        let mut y = bounds.y;
        while y < (bounds.y + bounds.height).max(y + 1.0) {
            let mut x = bounds.x;
            while x < (bounds.x + bounds.width).max(x + 1.0) {
                let w = (bounds.x + bounds.width - x).max(0.0).min(tile_size);
                let h = (bounds.y + bounds.height - y).max(0.0).min(tile_size);
                tiles.push(Tile::new(LayoutRect { x, y, width: w.max(1.0), height: h.max(1.0) }));
                x += tile_size;
                if x >= bounds.x + bounds.width && bounds.width > 0.0 { break; }
                if bounds.width <= 0.0 { break; }
            }
            y += tile_size;
            if y >= bounds.y + bounds.height && bounds.height > 0.0 { break; }
            if bounds.height <= 0.0 { break; }
        }

        Self {
            id,
            z_index,
            opacity,
            bounds,
            triggers,
            transform,
            tiles,
            background_commands: Vec::new(),
            content_commands: Vec::new(),
            child_layer_ids: Vec::new(),
        }
    }
}

// ── LayerTree ─────────────────────────────────────────────────────────────────

/// A flat collection of compositing layers built from a `LayoutBox` tree.
///
/// `layers[0]` is always the root layer (z_index = 0, opacity = 1.0).
pub struct LayerTree {
    /// All layers in creation order. Index == `Layer::id`.
    pub layers: Vec<Layer>,
}

impl LayerTree {
    fn new() -> Self {
        Self { layers: Vec::new() }
    }

    /// Append a layer and return its assigned id.
    fn add_layer(&mut self, layer: Layer) -> usize {
        let id = layer.id;
        self.layers.push(layer);
        id
    }

    /// Returns references to all layers sorted by `z_index` (ascending).
    /// This is the order in which layers must be composited to produce correct
    /// painter's-algorithm rendering.
    pub fn sorted_layers(&self) -> Vec<&Layer> {
        let mut refs: Vec<&Layer> = self.layers.iter().collect();
        refs.sort_by_key(|l| l.z_index);
        refs
    }

    /// Categorize child layers of a parent into negative, zero, and positive z-indices.
    pub fn categorize_children(&self, parent_id: usize) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
        let parent = &self.layers[parent_id];
        let mut negative = Vec::new();
        let mut zero = Vec::new();
        let mut positive = Vec::new();

        for &child_id in &parent.child_layer_ids {
            let child = &self.layers[child_id];
            if child.z_index < 0 {
                negative.push(child_id);
            } else if child.z_index == 0 {
                zero.push(child_id);
            } else {
                positive.push(child_id);
            }
        }

        // Sort negative and positive by z_index
        negative.sort_by_key(|&id| self.layers[id].z_index);
        positive.sort_by_key(|&id| self.layers[id].z_index);

        (negative, zero, positive)
    }
}

// ── LayerTreeBuilder ──────────────────────────────────────────────────────────

/// Traverses a `LayoutBox` tree and produces a `LayerTree`.
///
/// Each box that carries a compositing trigger establishes a new `Layer`;
/// all other boxes paint into the current ancestor layer.
pub struct LayerTreeBuilder;

impl LayerTreeBuilder {
    /// Build a `LayerTree` from the given layout root.
    ///
    /// `viewport` is the full drawable area and becomes the bounds of the root layer.
    pub fn build(layout: &LayoutBox, viewport: LayoutRect) -> LayerTree {
        let mut tree = LayerTree::new();
        let root = Layer::new(0, 0, 1.0, viewport, vec![], Matrix4x4::identity());
        tree.add_layer(root);
        Self::traverse(layout, &mut tree, 0, viewport);
        tree
    }

    /// Iterative traversal (replaces the formerly recursive implementation).
    ///
    /// Assigns each `LayoutBox` to either a new layer (if it has compositing
    /// triggers) or the current ancestor layer.  Uses an explicit stack so that
    /// deeply nested DOM trees do not cause a stack overflow.
    ///
    /// Sequential index-based accesses to `tree.layers` are used deliberately:
    /// each write touches a single distinct index, so there is no simultaneous
    /// aliasing of two entries.
    fn traverse(layout: &LayoutBox, tree: &mut LayerTree, current_layer_id: usize, clip: LayoutRect) {
        enum Frame<'f> {
            /// Process a layout box and push its children.
            Process {
                layout: &'f LayoutBox<'f>,
                layer_id: usize,
                clip: LayoutRect,
            },
            /// Emit a PopClip command into the given layer after children are done.
            PopClip {
                layer_id: usize,
                is_background: bool,
            },
        }

        let mut stack: Vec<Frame> = vec![Frame::Process { layout, layer_id: current_layer_id, clip }];

        while let Some(frame) = stack.pop() {
            match frame {
                Frame::PopClip { layer_id, is_background } => {
                    let cmd = PaintCommand::PopClip;
                    if is_background {
                        tree.layers[layer_id].background_commands.push(cmd.clone());
                    } else {
                        tree.layers[layer_id].content_commands.push(cmd.clone());
                    }
                    // Also add to overlapping tiles
                    for tile in &mut tree.layers[layer_id].tiles {
                        if is_background {
                            tile.background_commands.push(cmd.clone());
                        } else {
                            tile.content_commands.push(cmd.clone());
                        }
                        tile.dirty = true;
                    }
                }

                Frame::Process { layout: frame_layout, layer_id: frame_layer_id, clip: frame_clip } => {
                    let d = frame_layout.dimensions;

                    // Per CSS spec, the default `overflow: visible` means content (in particular
                    // text) must NOT be clipped to its ancestors' content boxes. Only boxes with
                    // `overflow: hidden|clip|auto|scroll` establish a clipping region — and those
                    // are handled via PushClip/PopClip + the mask stack in render.rs.
                    //
                    // Therefore, `next_clip` is propagated unchanged to descendants; it represents
                    // a conservative paint-order hint (current layer's drawable region) rather than
                    // a mandatory per-glyph clip.
                    let next_clip = frame_clip;

                    // Skip zero-sized boxes but still visit children.
                    if d.width < 0.1 || d.height < 0.1 {
                        // Push children in reverse order so the first child is processed first.
                        for child in frame_layout.children.iter().rev() {
                            stack.push(Frame::Process { layout: child, layer_id: frame_layer_id, clip: next_clip });
                        }
                        continue;
                    }

                    // Check if this box clips its overflow.
                    let overflow_hidden = Self::has_overflow_hidden(frame_layout);
                    let border_radius = match frame_layout.style_node.specified_values.get(&crate::css::intern("border-radius")) {
                        Some(Value::Length(v, _)) => *v,
                        _ => 0.0,
                    };

                    let (triggers, matrix) = Self::detect_triggers(frame_layout);

                    if !triggers.is_empty() {
                        // This box establishes a new compositing layer.
                        let new_id = tree.layers.len();
                        let opacity = frame_layout.get_opacity();
                        let new_layer = Layer::new(new_id, frame_layout.z_index, opacity, d, triggers, matrix);
                        tree.add_layer(new_layer);

                        // Record parent → child relationship: access parent index first,
                        // then new_id — both are distinct indices so no aliasing.
                        tree.layers[frame_layer_id].child_layer_ids.push(new_id);

                        // Collect this box's paint commands into the new layer as BACKGROUND.
                        Self::collect_paint_commands(frame_layout, &mut tree.layers[new_id], frame_clip, true);

                        // If overflow:hidden, emit PushClip before children and schedule PopClip after.
                        if overflow_hidden && !frame_layout.children.is_empty() {
                            let clip_rect = frame_layout.dimensions;
                            let push_cmd = PaintCommand::PushClip { rect: clip_rect, radius: border_radius };
                            tree.layers[new_id].background_commands.push(push_cmd.clone());
                            for tile in &mut tree.layers[new_id].tiles {
                                tile.background_commands.push(push_cmd.clone());
                                tile.dirty = true;
                            }
                            // Schedule PopClip to be emitted after all children finish.
                            stack.push(Frame::PopClip { layer_id: new_id, is_background: true });
                        }

                        // All children belong to the new layer's stacking context.
                        for child in frame_layout.children.iter().rev() {
                            stack.push(Frame::Process { layout: child, layer_id: new_id, clip: next_clip });
                        }
                    } else {
                        // No trigger — paint into the current ancestor layer as CONTENT.
                        Self::collect_paint_commands(frame_layout, &mut tree.layers[frame_layer_id], frame_clip, false);

                        // If overflow:hidden, emit PushClip before children and schedule PopClip after.
                        if overflow_hidden && !frame_layout.children.is_empty() {
                            let clip_rect = frame_layout.dimensions;
                            let push_cmd = PaintCommand::PushClip { rect: clip_rect, radius: border_radius };
                            tree.layers[frame_layer_id].content_commands.push(push_cmd.clone());
                            for tile in &mut tree.layers[frame_layer_id].tiles {
                                tile.content_commands.push(push_cmd.clone());
                                tile.dirty = true;
                            }
                            // Schedule PopClip to be emitted after all children finish.
                            stack.push(Frame::PopClip { layer_id: frame_layer_id, is_background: false });
                        }

                        for child in frame_layout.children.iter().rev() {
                            stack.push(Frame::Process { layout: child, layer_id: frame_layer_id, clip: next_clip });
                        }
                    }
                }
            }
        }
    }

    /// Returns `true` if this box has `overflow: hidden` set.
    fn has_overflow_hidden(layout: &LayoutBox) -> bool {
        match layout.style_node.specified_values.get(&crate::css::intern("overflow")) {
            Some(Value::Keyword(k)) => **k == *"hidden",
            _ => false,
        }
    }

    /// Inspect a `LayoutBox`'s CSS properties and return the list of
    /// compositing triggers it carries.
    ///
    /// BFC-establishing properties (InlineBlock, Flex, TableCell, overflow:hidden)
    /// are intentionally excluded — BFC != compositing layer per the CSS spec.
    fn detect_triggers(layout: &LayoutBox) -> (Vec<CompositingTrigger>, Matrix4x4) {
        let mut triggers = Vec::new();
        let sv = &layout.style_node.specified_values;
        let mut matrix = Matrix4x4::identity();

        // CSS spec: any non-static positioned element establishes a stacking context.
        // We always emit a ZIndex trigger for positioned elements (even when z-index is 0/auto)
        // so they get their own layer and participate in the correct stacking order.
        let is_positioned = !matches!(layout.position, PositionType::Static);
        if layout.z_index != 0 || is_positioned {
            triggers.push(CompositingTrigger::ZIndex(layout.z_index));
        }

        let opacity = layout.get_opacity();
        if opacity < 1.0 {
            triggers.push(CompositingTrigger::Opacity(opacity));
        }

        if let Some(Value::Transform(ops)) = sv.get(&crate::css::intern("transform")) {
            let w = layout.dimensions.width;
            let h = layout.dimensions.height;
            matrix = Self::compute_transform_matrix(ops, w, h);
            triggers.push(CompositingTrigger::Transform(matrix));
        }

        match layout.position {
            PositionType::Fixed => triggers.push(CompositingTrigger::PositionFixed),
            PositionType::Sticky => triggers.push(CompositingTrigger::PositionSticky),
            _ => {}
        }

        if let Some(Value::Keyword(k)) = sv.get(&crate::css::intern("will-change")) {
            if **k != *"auto" {
                triggers.push(CompositingTrigger::WillChange(k.to_string()));
            }
        }

        (triggers, matrix)
    }

    fn compute_transform_matrix(ops: &[TransformOp], elem_width: f32, elem_height: f32) -> Matrix4x4 {
        let mut result = Matrix4x4::identity();
        for op in ops {
            let m = match op {
                TransformOp::Translate(x, y) => {
                    Matrix4x4::translate(x.resolve(elem_width), y.resolve(elem_height), 0.0)
                }
                TransformOp::Scale(x, y) => Matrix4x4::from_2d(Matrix3x3::scale(x.0, y.0)),
                TransformOp::Rotate(rad) => Matrix4x4::from_2d(Matrix3x3::rotate(rad.0)),
                TransformOp::Matrix(a, b, c, d, e, f) => Matrix4x4::from_2d(Matrix3x3([a.0, c.0, e.0, b.0, d.0, f.0, 0.0, 0.0, 1.0])),
            };
            result = result.multiply(&m);
        }
        result
    }

    /// Emit paint commands for a single `LayoutBox` (not its children) into `layer`.
    ///
    /// Covers: box-shadow, background, border, images, and text.
    fn collect_paint_commands(layout: &LayoutBox, layer: &mut Layer, clip: LayoutRect, is_root_of_layer: bool) {
        let d = layout.dimensions;
        let sv = &layout.style_node.specified_values;

        let radius = match sv.get(&crate::css::intern("border-radius")) {
            Some(Value::Length(v, _)) => *v,
            _ => 0.0,
        };

        let mut commands = Vec::new();

        // Box shadow (outer only)
        if let Some(Value::BoxShadow(shadow)) = sv.get(&crate::css::intern("box-shadow")) {
            if !shadow.inset {
                commands.push(PaintCommand::Shadow(d, shadow.clone()));
            }
        }

        // Background
        let bg = sv.get(&crate::css::intern("background-color"))
            .or_else(|| sv.get(&crate::css::intern("background")))
            .or_else(|| sv.get(&crate::css::intern("background-image")));
        match bg {
            Some(Value::Color(c)) if c.a > 0 => {
                commands.push(PaintCommand::Rect(d, c.clone(), radius));
            }
            Some(Value::Gradient(GradientValue::Linear { direction, stops })) => {
                commands.push(PaintCommand::LinearGradient {
                    rect: d,
                    direction: direction.clone(),
                    stops: stops.clone(),
                    radius,
                });
            }
            Some(Value::Gradient(GradientValue::Radial { stops, .. })) => {
                commands.push(PaintCommand::RadialGradient {
                    rect: d,
                    stops: stops.clone(),
                    radius,
                });
            }
            _ => {}
        }
        // Also check background-image for gradients (separate from background-color)
        if matches!(bg, Some(Value::Color(_)) | None) {
            let bg_img = sv.get(&crate::css::intern("background-image"));
            match bg_img {
                Some(Value::Gradient(GradientValue::Linear { direction, stops })) => {
                    commands.push(PaintCommand::LinearGradient {
                        rect: d,
                        direction: direction.clone(),
                        stops: stops.clone(),
                        radius,
                    });
                }
                Some(Value::Gradient(GradientValue::Radial { stops, .. })) => {
                    commands.push(PaintCommand::RadialGradient {
                        rect: d,
                        stops: stops.clone(),
                        radius,
                    });
                }
                _ => {}
            }
        }

        // Border
        if layout.border.left > 0.0 {
            let color = match sv.get(&crate::css::intern("border-color")) {
                Some(Value::Color(c)) => c.clone(),
                _ => Color { r: 180, g: 180, b: 180, a: 255 },
            };
            commands.push(PaintCommand::Border(d, layout.border.left, color, radius));
        }

        // Image
        if layout.display == DisplayType::Image {
            if let Some(ref url) = layout.image_url {
                let object_fit = match sv.get(&crate::css::intern("object-fit")) {
                    Some(Value::Keyword(k)) => match k.as_ref() {
                        "contain" => ObjectFit::Contain,
                        "cover"   => ObjectFit::Cover,
                        "none"    => ObjectFit::None,
                        _         => ObjectFit::Fill,
                    },
                    _ => ObjectFit::Fill,
                };
                let alt = layout.alt_text.clone().unwrap_or_default();
                commands.push(PaintCommand::Image { rect: d, url: url.clone(), object_fit, alt });
            }
        }

        // Text
        if let NodeData::Text { ref contents } = layout.style_node.node.data {
            let font_size = match sv.get(&crate::css::intern("font-size")) {
                Some(Value::Length(v, _)) => *v,
                _ => 16.0,
            };
            let color = match sv.get(&crate::css::intern("color")) {
                Some(Value::Color(c)) => c.clone(),
                _ => Color { r: 0, g: 0, b: 0, a: 255 },
            };
            let bold = match sv.get(&crate::css::intern("font-weight")) {
                Some(Value::Keyword(k)) => matches!(k.as_ref(), "bold" | "bolder"),
                Some(Value::Length(v, _)) => *v >= 600.0,
                _ => false,
            };
            let italic = match sv.get(&crate::css::intern("font-style")) {
                Some(Value::Keyword(k)) => matches!(k.as_ref(), "italic" | "oblique"),
                _ => false,
            };
            let text_decoration: u8 = match sv.get(&crate::css::intern("text-decoration")) {
                Some(Value::Keyword(k)) => match k.as_ref() {
                    "underline"    => 0b001,
                    "line-through" => 0b010,
                    "overline"     => 0b100,
                    _              => 0,
                },
                _ => 0,
            };
            commands.push(PaintCommand::Text {
                rect: d,
                text: contents.borrow().to_string(),
                font_size,
                color,
                clip,
                bold,
                italic,
                text_decoration,
            });
        }

        // Input button label
        // <input type="submit|button|reset"> carries its visible label in the
        // `input_label` field (populated in LayoutBox::new from the `value` attribute).
        // The label is rendered centered inside the button rect.
        if let Some(ref label) = layout.input_label {
            if !label.is_empty() {
                let font_size = match sv.get(&crate::css::intern("font-size")) {
                    Some(Value::Length(v, _)) => *v,
                    _ => 13.0, // browser default for buttons
                };
                let color = match sv.get(&crate::css::intern("color")) {
                    Some(Value::Color(c)) => c.clone(),
                    _ => Color { r: 0, g: 0, b: 0, a: 255 },
                };
                // Center the text vertically by offsetting the rect so the baseline
                // lands near the mid-point.  The text renderer places the baseline at
                // rect.y + font_size * 0.85, so shift rect.y so that baseline falls
                // at d.y + d.height / 2.0.
                // baseline = rect_y + font_size * 0.85  =>  rect_y = mid - font_size * 0.85
                let mid_y = d.y + d.height / 2.0;
                let text_rect = crate::layout::Rect {
                    x: d.x + layout.padding.left,
                    y: mid_y - font_size * 0.85,
                    width: (d.width - layout.padding.left - layout.padding.right).max(0.0),
                    height: font_size,
                };
                commands.push(PaintCommand::Text {
                    rect: text_rect,
                    text: label.clone(),
                    font_size,
                    color,
                    clip: d, // clip to the button bounds
                    bold: false,
                    italic: false,
                    text_decoration: 0,
                });
            }
        }

        // Distribute commands to correct list
        if is_root_of_layer {
            layer.background_commands.extend(commands.clone());
        } else {
            layer.content_commands.extend(commands.clone());
        }

        // Also distribute to overlapping tiles
        for cmd in commands {
            // PushClip/PopClip are emitted directly in traverse(); they never appear in
            // the `commands` Vec built by collect_paint_commands. Handle them defensively.
            let cmd_rect = match &cmd {
                PaintCommand::Rect(r, ..) => *r,
                PaintCommand::Border(r, ..) => *r,
                PaintCommand::Image { rect, .. } => *rect,
                PaintCommand::Text { rect, .. } => *rect,
                PaintCommand::Shadow(r, ..) => *r,
                PaintCommand::LinearGradient { rect, .. } => *rect,
                PaintCommand::RadialGradient { rect, .. } => *rect,
                PaintCommand::PushClip { rect, .. } => *rect,
                PaintCommand::PopClip => continue,
            };

            for tile in &mut layer.tiles {
                if tile.rect.intersects(&cmd_rect) {
                    if is_root_of_layer {
                        tile.background_commands.push(cmd.clone());
                    } else {
                        tile.content_commands.push(cmd.clone());
                    }
                    tile.dirty = true;
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;
    use crate::css;
    use crate::style;
    use crate::layout::{build_layout_tree, Rect};
    use std::collections::HashMap;

    fn viewport() -> LayoutRect {
        Rect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 }
    }

    fn build_tree_from_html(html: &str, extra_css: &str) -> LayerTree {
        let dom = dom::parse_html(html);
        let stylesheet = css::parse_css(extra_css);
        let style_tree = style::build_style_tree(&dom.document, &stylesheet, None, &HashMap::new(), None, None, None);
        let (layout_opt, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout_opt.expect("layout tree should be built");
        LayerTreeBuilder::build(&layout, viewport())
    }

    #[test]
    fn test_root_layer_always_created() {
        let tree = build_tree_from_html("<div>Hello</div>", "");
        assert!(!tree.layers.is_empty(), "at least one layer must exist");
        assert_eq!(tree.layers[0].id, 0, "root layer id must be 0");
        assert_eq!(tree.layers[0].z_index, 0, "root layer z_index must be 0");
        assert!((tree.layers[0].opacity - 1.0).abs() < f32::EPSILON, "root opacity must be 1.0");
    }

    #[test]
    fn test_no_trigger_plain_div() {
        // A plain div with no compositing properties should not create extra layers.
        let tree = build_tree_from_html(
            r#"<div style="width:100px; height:50px; background-color:red;">Content</div>"#,
            "",
        );
        assert_eq!(tree.layers.len(), 1, "plain div should not create extra layers");
    }

    #[test]
    fn test_opacity_trigger_creates_new_layer() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px; height:50px; opacity:0.5;">Content</div>"#,
            "",
        );
        // Root layer + at least one layer for the opacity element
        assert!(tree.layers.len() >= 2, "opacity element should create a new layer");
        let opacity_layer = tree.layers.iter().find(|l| l.id != 0).expect("should have child layer");
        assert!(
            opacity_layer.triggers.iter().any(|t| matches!(t, CompositingTrigger::Opacity(_))),
            "layer should have Opacity trigger"
        );
    }

    #[test]
    fn test_z_index_trigger_creates_new_layer() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px; height:50px; z-index:5;">Content</div>"#,
            "",
        );
        assert!(tree.layers.len() >= 2, "z-index element should create a new layer");
        let z_layer = tree.layers.iter().find(|l| l.id != 0).expect("should have child layer");
        assert!(
            z_layer.triggers.iter().any(|t| *t == CompositingTrigger::ZIndex(5)),
            "layer should have ZIndex(5) trigger"
        );
    }

    #[test]
    fn test_sorted_layers_by_z_index() {
        // Create a tree with known z-index values via nested elements
        let tree = build_tree_from_html(
            r#"<div>
                <div style="width:50px;height:50px;z-index:3;">A</div>
                <div style="width:50px;height:50px;z-index:1;">B</div>
                <div style="width:50px;height:50px;z-index:2;">C</div>
            </div>"#,
            "",
        );
        let sorted = tree.sorted_layers();
        for pair in sorted.windows(2) {
            assert!(pair[0].z_index <= pair[1].z_index, "layers must be in ascending z-index order");
        }
    }

    #[test]
    fn test_will_change_trigger_creates_new_layer() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;will-change:transform;">Content</div>"#,
            "",
        );
        assert!(tree.layers.len() >= 2, "will-change element should create a new layer");
        let wc_layer = tree.layers.iter().find(|l| l.id != 0).expect("should have child layer");
        assert!(
            wc_layer.triggers.iter().any(|t| matches!(t, CompositingTrigger::WillChange(_))),
            "layer should have WillChange trigger"
        );
    }

    #[test]
    fn test_transform_trigger_creates_layer_with_matrix() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px; height:50px; transform:translate(50px, 100px);">Content</div>"#,
            "",
        );
        assert!(tree.layers.len() >= 2);
        let t_layer = tree.layers.iter().find(|l| l.id != 0).expect("child layer");
        
        let mut found_transform = false;
        for trigger in &t_layer.triggers {
            if let CompositingTrigger::Transform(m) = trigger {
                found_transform = true;
                // Matrix translation part should match 50, 100
                assert_eq!(m.0[3], 50.0);
                assert_eq!(m.0[7], 100.0);
            }
        }
        assert!(found_transform, "layer must have Transform trigger with matrix");
        assert_eq!(t_layer.transform.0[3], 50.0);
        assert_eq!(t_layer.transform.0[7], 100.0);
    }

    #[test]
    fn test_transform_translate_y_percent_resolves_against_element_height() {
        // translateY(-50%) on a 100px-tall element should produce ty = -50px in the matrix.
        let tree = build_tree_from_html(
            r#"<div style="width:200px; height:100px; transform:translateY(-50%);">Content</div>"#,
            "",
        );
        assert!(tree.layers.len() >= 2, "element with transform should create a new layer");
        let t_layer = tree.layers.iter().find(|l| l.id != 0).expect("child layer");
        // Matrix row-major: translation is at indices [3] (tx) and [7] (ty).
        // translateY(-50%) on height=100 should resolve to ty = -50.
        let ty = t_layer.transform.0[7];
        assert!((ty - (-50.0)).abs() < 1.0,
            "translateY(-50%) on height:100px should produce ty=-50, got {}", ty);
    }

    #[test]
    fn test_transform_translate_x_percent_resolves_against_element_width() {
        // translateX(50%) on a 200px-wide element should produce tx = 100px in the matrix.
        let tree = build_tree_from_html(
            r#"<div style="width:200px; height:100px; transform:translateX(50%);">Content</div>"#,
            "",
        );
        assert!(tree.layers.len() >= 2, "element with transform should create a new layer");
        let t_layer = tree.layers.iter().find(|l| l.id != 0).expect("child layer");
        let tx = t_layer.transform.0[3];
        assert!((tx - 100.0).abs() < 1.0,
            "translateX(50%) on width:200px should produce tx=100, got {}", tx);
    }

    #[test]
    fn test_transform_translate_px_both_axes() {
        // translate(10px, 20px) should produce tx=10, ty=20 in the matrix.
        let tree = build_tree_from_html(
            r#"<div style="width:100px; height:50px; transform:translate(10px, 20px);">Content</div>"#,
            "",
        );
        assert!(tree.layers.len() >= 2, "element with transform should create a new layer");
        let t_layer = tree.layers.iter().find(|l| l.id != 0).expect("child layer");
        let tx = t_layer.transform.0[3];
        let ty = t_layer.transform.0[7];
        assert!((tx - 10.0).abs() < 0.5,
            "translate(10px, 20px) should produce tx=10, got {}", tx);
        assert!((ty - 20.0).abs() < 0.5,
            "translate(10px, 20px) should produce ty=20, got {}", ty);
    }

    #[test]
    fn test_transform_scale_creates_matrix() {
        // scale(1.5) should produce a scale matrix with sx=sy=1.5.
        let tree = build_tree_from_html(
            r#"<div style="width:100px; height:50px; transform:scale(1.5);">Content</div>"#,
            "",
        );
        assert!(tree.layers.len() >= 2, "element with transform should create a new layer");
        let t_layer = tree.layers.iter().find(|l| l.id != 0).expect("child layer");
        // For a scale matrix, indices [0] and [5] hold sx and sy respectively.
        let sx = t_layer.transform.0[0];
        let sy = t_layer.transform.0[5];
        assert!((sx - 1.5).abs() < 0.01, "scale(1.5) should produce sx=1.5, got {}", sx);
        assert!((sy - 1.5).abs() < 0.01, "scale(1.5) should produce sy=1.5, got {}", sy);
    }

    #[test]
    fn test_image_paint_command_carries_object_fit_and_alt() {
        let tree = build_tree_from_html(
            r#"<img src="x.png" alt="test" style="width:100px;height:100px;object-fit:cover;">"#,
            "",
        );
        let has_cover = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .any(|cmd| matches!(
                cmd,
                PaintCommand::Image { object_fit: ObjectFit::Cover, alt, .. } if alt == "test"
            ));
        assert!(has_cover, "expected PaintCommand::Image with ObjectFit::Cover and alt='test'");
    }

    #[test]
    fn test_image_paint_command_default_fill() {
        let tree = build_tree_from_html(
            r#"<img src="photo.jpg" alt="" style="width:200px;height:150px;">"#,
            "",
        );
        let has_fill = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Image { object_fit: ObjectFit::Fill, .. }));
        assert!(has_fill, "image with no object-fit must default to ObjectFit::Fill");
    }

    /// A plain div without `overflow: hidden` must not emit any PushClip/PopClip commands.
    #[test]
    fn test_no_clip_commands_for_visible_overflow() {
        let tree = build_tree_from_html(
            r#"<div style="width:200px;height:100px;background-color:red;">
                <div style="width:200px;height:200px;background-color:blue;">tall child</div>
            </div>"#,
            "",
        );
        let has_push_clip = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::PushClip { .. }));
        assert!(!has_push_clip, "overflow:visible (default) must not emit PushClip");
    }

    /// A div with `overflow: hidden` must emit a PushClip command followed by a PopClip.
    #[test]
    fn test_overflow_hidden_emits_push_pop_clip() {
        let tree = build_tree_from_html(
            r#"<div style="width:200px;height:100px;overflow:hidden;background-color:red;">
                <div style="width:200px;height:200px;background-color:blue;">tall child</div>
            </div>"#,
            "",
        );
        let all_cmds: Vec<&PaintCommand> = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .collect();

        let push_count = all_cmds.iter().filter(|c| matches!(c, PaintCommand::PushClip { .. })).count();
        let pop_count  = all_cmds.iter().filter(|c| matches!(c, PaintCommand::PopClip)).count();

        assert!(push_count >= 1, "overflow:hidden must emit at least one PushClip; got {}", push_count);
        assert_eq!(push_count, pop_count, "PushClip and PopClip must be balanced");
    }

    /// A div with `overflow: hidden` + `border-radius` must emit a PushClip with non-zero radius.
    #[test]
    fn test_overflow_hidden_with_border_radius_emits_rounded_clip() {
        let tree = build_tree_from_html(
            r#"<div style="width:200px;height:100px;overflow:hidden;border-radius:8px;">
                <div style="width:300px;height:300px;background-color:blue;">overflow child</div>
            </div>"#,
            "",
        );
        let has_rounded_push = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::PushClip { radius, .. } if *radius > 0.0));
        assert!(has_rounded_push, "overflow:hidden + border-radius must emit PushClip with radius > 0");
    }

    /// A positioned element (position:relative) without explicit z-index must establish a layer.
    #[test]
    fn test_positioned_element_creates_layer() {
        let tree = build_tree_from_html(
            r#"<div style="width:200px;height:200px;background-color:white;">
                <div style="position:relative;width:100px;height:100px;background-color:red;">content</div>
            </div>"#,
            "",
        );
        // The positioned child must create its own layer (z-index: auto = 0 but still positioned).
        assert!(tree.layers.len() >= 2, "positioned element should create a new layer; got {} layers", tree.layers.len());
        let pos_layer = tree.layers.iter().find(|l| l.id != 0).expect("should have child layer");
        assert!(
            pos_layer.triggers.iter().any(|t| matches!(t, CompositingTrigger::ZIndex(_))),
            "positioned layer must have ZIndex trigger"
        );
    }

    /// A modal with z-index:1000 must produce a layer with z_index == 1000.
    #[test]
    fn test_modal_high_z_index_creates_correct_layer() {
        let tree = build_tree_from_html(
            r#"<div>
                <div style="width:800px;height:600px;background-color:white;">page content</div>
                <div style="position:absolute;top:100px;left:100px;width:400px;height:300px;z-index:1000;background-color:gray;">modal</div>
            </div>"#,
            "",
        );
        // Modal must create a layer with z_index = 1000
        let modal_layer = tree.layers.iter().find(|l| l.z_index == 1000);
        assert!(modal_layer.is_some(), "modal with z-index:1000 must create a layer with z_index=1000");
    }

    /// Negative z-index element must be categorized as a negative child.
    #[test]
    fn test_negative_z_index_categorized_correctly() {
        let tree = build_tree_from_html(
            r#"<div style="position:relative;width:200px;height:200px;">
                <div style="position:absolute;z-index:-1;width:100px;height:100px;background-color:blue;">behind</div>
                <div style="width:100px;height:50px;background-color:red;">front</div>
            </div>"#,
            "",
        );
        // Find the layer with z_index = -1
        let neg_layer = tree.layers.iter().find(|l| l.z_index == -1);
        assert!(neg_layer.is_some(), "negative z-index element must create a layer with z_index=-1");

        // That layer must be a negative child of its parent
        let neg_id = neg_layer.unwrap().id;
        // Find a parent layer that has neg_id as a child
        let parent_has_neg = tree.layers.iter().any(|l| l.child_layer_ids.contains(&neg_id));
        assert!(parent_has_neg, "negative z-index layer must be registered as child of a parent layer");

        // Verify categorize_children puts it in the negative bucket
        let found_in_negative = tree.layers.iter().any(|l| {
            let (neg, _, _) = tree.categorize_children(l.id);
            neg.contains(&neg_id)
        });
        assert!(found_in_negative, "negative z-index layer must appear in negative bucket of categorize_children");
    }

    /// Layers sorted by z-index must produce a stable ascending order within a stacking context.
    #[test]
    fn test_stacking_context_paint_order_stable() {
        let tree = build_tree_from_html(
            r#"<div style="position:relative;">
                <div style="position:absolute;z-index:2;width:50px;height:50px;">z2</div>
                <div style="position:absolute;z-index:1;width:50px;height:50px;">z1</div>
                <div style="position:absolute;z-index:3;width:50px;height:50px;">z3</div>
            </div>"#,
            "",
        );
        let sorted = tree.sorted_layers();
        for pair in sorted.windows(2) {
            assert!(pair[0].z_index <= pair[1].z_index,
                "layers must be in ascending z-index order: {} vs {}", pair[0].z_index, pair[1].z_index);
        }
        // Confirm all 3 z-index layers were created (plus root + the relative container)
        let z_indices: Vec<i32> = tree.layers.iter().map(|l| l.z_index).collect();
        assert!(z_indices.contains(&1), "z-index:1 layer must exist");
        assert!(z_indices.contains(&2), "z-index:2 layer must exist");
        assert!(z_indices.contains(&3), "z-index:3 layer must exist");
    }

    /// Normal boxes without overflow:hidden must not be affected (regression guard).
    #[test]
    fn test_overflow_hidden_does_not_affect_sibling_boxes() {
        let tree = build_tree_from_html(
            r#"<div>
                <div style="width:100px;height:50px;overflow:hidden;background:red;">
                    <span>clipped</span>
                </div>
                <div style="width:100px;height:50px;background:green;">
                    <span>not clipped</span>
                </div>
            </div>"#,
            "",
        );
        // Verify there is exactly one PushClip (from the overflow:hidden div only).
        let push_count = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .filter(|c| matches!(c, PaintCommand::PushClip { .. }))
            .count();
        assert_eq!(push_count, 1, "only the overflow:hidden div should emit PushClip; got {}", push_count);
    }

    /// `<input type="submit" value="Google Search">` must produce a Text paint command
    /// whose text matches the value attribute.
    #[test]
    fn test_input_submit_emits_label_text_command() {
        let tree = build_tree_from_html(
            r#"<form><input type="submit" value="Google Search" style="width:160px;height:30px;"></form>"#,
            "",
        );
        let label_cmd = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .find(|cmd| matches!(cmd, PaintCommand::Text { text, .. } if text == "Google Search"));
        assert!(label_cmd.is_some(), "input[type=submit] must emit a Text paint command with 'Google Search'");
    }

    /// `<input type="button" value="Click me">` must produce a Text paint command.
    #[test]
    fn test_input_button_emits_label_text_command() {
        let tree = build_tree_from_html(
            r#"<input type="button" value="Click me" style="width:100px;height:30px;">"#,
            "",
        );
        let label_cmd = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .find(|cmd| matches!(cmd, PaintCommand::Text { text, .. } if text == "Click me"));
        assert!(label_cmd.is_some(), "input[type=button] must emit a Text paint command with 'Click me'");
    }

    /// `<input type="submit">` without a value attribute must default to "Submit".
    #[test]
    fn test_input_submit_default_label() {
        let tree = build_tree_from_html(
            r#"<input type="submit" style="width:100px;height:30px;">"#,
            "",
        );
        let label_cmd = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .find(|cmd| matches!(cmd, PaintCommand::Text { text, .. } if text == "Submit"));
        assert!(label_cmd.is_some(), "input[type=submit] without value must default to 'Submit' label");
    }

    /// `<input type="text">` must NOT emit an extra Text command (only the egui overlay handles it).
    #[test]
    fn test_input_text_does_not_emit_label_command() {
        let tree = build_tree_from_html(
            r#"<input type="text" value="user input" style="width:200px;height:30px;">"#,
            "",
        );
        // The "user input" text must NOT appear as a Text paint command.
        let found = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Text { text, .. } if text == "user input"));
        assert!(!found, "input[type=text] must not emit a Text paint command for value");
    }

    /// `<input style="border-radius: 24px">` must emit a Rect command with radius > 0.
    #[test]
    fn test_input_border_radius_emits_rounded_rect() {
        let tree = build_tree_from_html(
            r#"<input type="text" style="border-radius: 24px; width: 200px; height: 40px;">"#,
            "",
        );
        let has_rounded = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r) if *r > 0.0));
        assert!(has_rounded, "input with border-radius:24px must emit Rect with radius > 0");
    }

    /// `<input style="border-radius: 0">` must NOT emit a rounded Rect.
    #[test]
    fn test_input_no_border_radius_emits_flat_rect() {
        let tree = build_tree_from_html(
            r#"<input type="text" style="border-radius: 0px; width: 200px; height: 40px;">"#,
            "",
        );
        let has_rounded = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r) if *r > 0.0));
        assert!(!has_rounded, "input with border-radius:0 must not emit Rect with radius > 0");
    }

    /// `<button style="border-radius: 8px">` must emit a Rect command with radius > 0.
    #[test]
    fn test_button_border_radius_emits_rounded_rect() {
        let tree = build_tree_from_html(
            r#"<button style="border-radius: 8px; width: 100px; height: 36px;">Click</button>"#,
            "",
        );
        let has_rounded = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r) if *r > 0.0));
        assert!(has_rounded, "button with border-radius:8px must emit Rect with radius > 0");
    }

    /// `border-radius` applied via an external stylesheet (not inline) must also round the input.
    #[test]
    fn test_input_border_radius_from_stylesheet() {
        let tree = build_tree_from_html(
            r#"<input type="text" style="width: 200px; height: 40px;">"#,
            "input { border-radius: 24px; }",
        );
        let has_rounded = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r) if *r > 0.0));
        assert!(has_rounded, "input with stylesheet border-radius:24px must emit Rect with radius > 0");
    }

    /// `border-radius` applied via attribute selector `input[name=q]` must round the input.
    #[test]
    fn test_input_border_radius_from_attr_selector() {
        let tree = build_tree_from_html(
            r#"<input type="text" name="q" style="width: 200px; height: 40px;">"#,
            r#"input[name="q"] { border-radius: 24px; }"#,
        );
        let has_rounded = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r) if *r > 0.0));
        assert!(has_rounded, "input matched by attr selector with border-radius:24px must emit Rect with radius > 0");
    }

    /// `border-radius: 24px 24px 24px 24px` (four values) must still produce a rounded rect.
    #[test]
    fn test_input_border_radius_multi_value() {
        let tree = build_tree_from_html(
            r#"<input type="text" style="border-radius: 24px 24px 24px 24px; width: 200px; height: 40px;">"#,
            "",
        );
        let has_rounded = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r) if *r > 0.0));
        assert!(has_rounded, "input with border-radius:24px 24px 24px 24px must emit Rect with radius > 0");
    }

    /// An element with `box-shadow` must emit a `PaintCommand::Shadow` before the background rect.
    #[test]
    fn test_box_shadow_emits_shadow_command() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;background-color:white;box-shadow:0 2px 8px rgba(0,0,0,0.15);">Content</div>"#,
            "",
        );
        let has_shadow = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Shadow(..)));
        assert!(has_shadow, "element with box-shadow must emit a PaintCommand::Shadow");
    }

    /// An element with `box-shadow: none` must NOT emit a `PaintCommand::Shadow`.
    #[test]
    fn test_box_shadow_none_does_not_emit_shadow_command() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;background-color:white;box-shadow:none;">Content</div>"#,
            "",
        );
        let has_shadow = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Shadow(..)));
        assert!(!has_shadow, "element with box-shadow:none must not emit PaintCommand::Shadow");
    }

    /// Shadow command must appear BEFORE the background rect command (painter's algorithm).
    #[test]
    fn test_box_shadow_command_appears_before_background() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;background-color:red;box-shadow:2px 2px 4px #888;">Content</div>"#,
            "",
        );
        // Collect all commands from one layer into a flat ordered list.
        let cmds: Vec<&PaintCommand> = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .collect();

        let shadow_idx = cmds.iter().position(|c| matches!(c, PaintCommand::Shadow(..)));
        let rect_idx   = cmds.iter().position(|c| matches!(c, PaintCommand::Rect(..)));

        assert!(shadow_idx.is_some(), "must have a Shadow command");
        assert!(rect_idx.is_some(),   "must have a Rect (background) command");
        assert!(
            shadow_idx.unwrap() < rect_idx.unwrap(),
            "Shadow command (idx {}) must come before background Rect (idx {})",
            shadow_idx.unwrap(), rect_idx.unwrap()
        );
    }
}
