use crate::layout::{LayoutBox, DisplayType, PositionType, Rect as LayoutRect};
use crate::css::{Value, Color, BoxShadow, TransformOp, GradientValue, CssColorStop, LinearDirection};
use crate::matrix::{Matrix3x3, Matrix4x4};
use markup5ever_rcdom::NodeData;
use std::rc::Rc;

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

/// CSS `border-style` values that affect how a `PaintCommand::Border` strokes
/// its outline. `border-style: none` (and `hidden`) suppress the border
/// entirely, so they never reach this enum — `collect_paint_commands` simply
/// does not emit a `Border` command for them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderStyle {
    Solid,
    Dashed,
    Dotted,
}

/// Per-side border data, in `top, right, bottom, left` order.
#[derive(Debug, Clone, PartialEq)]
pub struct BorderSides {
    pub widths: [f32; 4],
    pub colors: [Color; 4],
    pub styles: [BorderStyle; 4],
}

// ── Paint Commands ────────────────────────────────────────────────────────────

/// A single atomic drawing operation. Moved from render.rs so that layer_tree.rs
/// owns the data pipeline (layout → layer tree → paint commands) while render.rs
/// owns the pixel execution (paint commands → Pixmap).
#[derive(Debug, Clone)]
pub enum PaintCommand {
    /// Filled rectangle: (bounds, color, corner-radius)
    Rect(LayoutRect, Color, f32),
    /// Stroked rectangle border: (bounds, stroke-width, color, corner-radius, style)
    Border(LayoutRect, f32, Color, f32, BorderStyle),
    /// Border whose sides differ in width, color or style.
    BorderSides { rect: LayoutRect, sides: Box<BorderSides>, radius: f32 },
    /// Image: layout rect, source URL, object-fit mode, alt text, and the
    /// border-box corner radius its content is clipped to.
    Image { rect: LayoutRect, url: String, object_fit: ObjectFit, alt: String, radius: f32 },
    /// Inline `<svg>` element, serialized to a standalone document whose size
    /// is the content box `rect`.
    Svg { rect: LayoutRect, source: std::sync::Arc<str> },
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
        /// Raw CSS `font-family` value (resolved by `crate::fonts`).
        font_family: std::sync::Arc<str>,
        /// CSS `font-weight` (1..=1000).
        font_weight: u16,
        /// Used line box height in px.
        line_height: f32,
        /// CSS `letter-spacing` in px, added after every character.
        letter_spacing: f32,
    },
    /// Outer box-shadow
    /// Rect, shadow parameters, and the element's border-radius (so the shadow
    /// shape — and, for `inset` shadows, the un-shadowed "hole" — follows the
    /// same rounding as the box itself).
    Shadow(LayoutRect, BoxShadow, f32),
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
    /// CSS `background-image: url(...)` layer. The tile size depends on the
    /// image's intrinsic size, so it is resolved at raster time.
    BackgroundImage {
        url: String,
        /// Painting area (`background-clip` box), in page coordinates.
        clip: LayoutRect,
        /// Corner radius of the clip box.
        radius: f32,
        /// Positioning area (`background-origin` box).
        area: LayoutRect,
        position: crate::background::BackgroundPosition,
        size: crate::background::BackgroundSize,
        repeat_x: bool,
        repeat_y: bool,
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
    /// Clip regions from ancestors (`overflow` other than visible, CSS `clip`)
    /// that apply to this layer. Each layer paints with a fresh clip stack, so
    /// these must be re-applied before its commands run.
    pub ancestor_clips: Vec<ClipRegion>,
}

/// A clip region inherited from an ancestor box, in page coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipRegion {
    pub rect: LayoutRect,
    pub radius: f32,
    /// Whether `position: absolute` descendants are clipped too. True once the
    /// clipping box or a box between it and the descendant is positioned, i.e.
    /// the absolute box's containing block lies inside the clipping box.
    clips_absolute: bool,
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
            ancestor_clips: Vec::new(),
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
/// Clip rect that never cuts anything, for text outside any overflow clip.
const UNBOUNDED_CLIP: LayoutRect = LayoutRect { x: -1.0e7, y: -1.0e7, width: 2.0e7, height: 2.0e7 };

pub struct LayerTreeBuilder;

impl LayerTreeBuilder {
    /// Build a `LayerTree` from the given layout root.
    ///
    /// `viewport` is the full drawable area and becomes the bounds of the root layer.
    pub fn build(layout: &LayoutBox, viewport: LayoutRect) -> LayerTree {
        let mut tree = LayerTree::new();
        let root = Layer::new(0, 0, 1.0, viewport, vec![], Matrix4x4::identity());
        tree.add_layer(root);
        // Text runs are not clipped to the viewport: layer transforms move
        // content after this clip would apply (a box translated from x=900 to
        // x=400 must still paint). Overflow clipping is done by clip masks.
        Self::traverse(layout, &mut tree, 0, UNBOUNDED_CLIP);
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
                /// Layer of the nearest ancestor stacking context: new layers
                /// are ordered by z-index among that layer's children.
                stacking_id: usize,
                clip: LayoutRect,
                clips: Rc<Vec<ClipRegion>>,
            },
            /// Emit a PopClip command into the given layer after children are done.
            PopClip {
                layer_id: usize,
                is_background: bool,
            },
        }

        let mut stack: Vec<Frame> = vec![Frame::Process {
            layout,
            layer_id: current_layer_id,
            stacking_id: current_layer_id,
            clip,
            clips: Rc::new(Vec::new()),
        }];

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

                Frame::Process { layout: frame_layout, layer_id: frame_layer_id, stacking_id: frame_stacking_id, clip: frame_clip, clips: frame_clips } => {
                    let d = frame_layout.dimensions;
                    let (clips, child_clips) = Self::clip_regions_for(frame_layout, frame_clips);

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
                        // A spread or blurred outset shadow is visible even
                        // around an empty box (e.g. a 0px-tall ad iframe).
                        let shadows: Vec<PaintCommand> = Self::box_shadows(frame_layout)
                            .into_iter()
                            .filter(|s| !s.inset && (*s.spread > 0.0 || *s.blur > 0.0))
                            .map(|s| PaintCommand::Shadow(d, s, 0.0))
                            .collect();
                        if !shadows.is_empty() && d.width >= 0.0 && d.height >= 0.0 {
                            let layer = &mut tree.layers[frame_layer_id];
                            for tile in &mut layer.tiles {
                                tile.content_commands.extend(shadows.iter().cloned());
                                tile.dirty = true;
                            }
                            layer.content_commands.extend(shadows);
                        }
                        // An empty clipping box still hides its in-flow children.
                        if Self::has_overflow_hidden(frame_layout) && !frame_layout.children.is_empty() {
                            let push_cmd = PaintCommand::PushClip { rect: d, radius: 0.0 };
                            tree.layers[frame_layer_id].content_commands.push(push_cmd.clone());
                            for tile in &mut tree.layers[frame_layer_id].tiles {
                                tile.content_commands.push(push_cmd.clone());
                                tile.dirty = true;
                            }
                            stack.push(Frame::PopClip { layer_id: frame_layer_id, is_background: false });
                        }
                        // Push children in reverse order so the first child is processed first.
                        for child in frame_layout.children.iter().rev() {
                            stack.push(Frame::Process { layout: child, layer_id: frame_layer_id, stacking_id: frame_stacking_id, clip: next_clip, clips: child_clips.clone() });
                        }
                        continue;
                    }

                    // Check if this box clips its overflow.
                    let overflow_hidden = Self::has_overflow_hidden(frame_layout);
                    let border_radius = border_radius_px(frame_layout);

                    let (triggers, matrix) = Self::detect_triggers(frame_layout);

                    if !triggers.is_empty() {
                        // This box establishes a new compositing layer.
                        let new_id = tree.layers.len();
                        let opacity = frame_layout.get_opacity();
                        let mut new_layer = Layer::new(new_id, frame_layout.z_index, opacity, d, triggers, matrix);
                        new_layer.ancestor_clips = clips.as_ref().clone();
                        tree.add_layer(new_layer);

                        // Record parent → child relationship: access parent index first,
                        // then new_id — both are distinct indices so no aliasing.
                        // Layers are ordered within the nearest stacking context, not
                        // within a positioned `z-index: auto` ancestor (CSS 2.1 §9.9.1).
                        tree.layers[frame_stacking_id].child_layer_ids.push(new_id);
                        let child_stacking_id = if Self::establishes_stacking_context(frame_layout, &tree.layers[new_id].triggers) {
                            new_id
                        } else {
                            frame_stacking_id
                        };

                        // Collect this box's paint commands into the new layer as BACKGROUND.
                        Self::collect_paint_commands(frame_layout, &mut tree.layers[new_id], frame_clip, true);

                        // If overflow:hidden, emit PushClip before children and schedule PopClip after.
                        // Non-layer children paint into content_commands, so the clip goes there.
                        if overflow_hidden && !frame_layout.children.is_empty() {
                            let clip_rect = crate::background::box_rect(frame_layout, crate::background::BoxArea::Padding);
                            let push_cmd = PaintCommand::PushClip { rect: clip_rect, radius: border_radius };
                            tree.layers[new_id].content_commands.push(push_cmd.clone());
                            for tile in &mut tree.layers[new_id].tiles {
                                tile.content_commands.push(push_cmd.clone());
                                tile.dirty = true;
                            }
                            // Schedule PopClip to be emitted after all children finish.
                            stack.push(Frame::PopClip { layer_id: new_id, is_background: false });
                        }

                        // All children belong to the new layer's stacking context.
                        for child in frame_layout.children.iter().rev() {
                            stack.push(Frame::Process { layout: child, layer_id: new_id, stacking_id: child_stacking_id, clip: next_clip, clips: child_clips.clone() });
                        }
                    } else {
                        // No trigger — paint into the current ancestor layer as CONTENT.
                        Self::collect_paint_commands(frame_layout, &mut tree.layers[frame_layer_id], frame_clip, false);

                        // If overflow:hidden, emit PushClip before children and schedule PopClip after.
                        if overflow_hidden && !frame_layout.children.is_empty() {
                            let clip_rect = crate::background::box_rect(frame_layout, crate::background::BoxArea::Padding);
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
                            stack.push(Frame::Process { layout: child, layer_id: frame_layer_id, stacking_id: frame_stacking_id, clip: next_clip, clips: child_clips.clone() });
                        }
                    }
                }
            }
        }
    }

    /// Every `box-shadow` layer of `layout` in paint order: the first CSS layer
    /// is on top, so layers are returned last-to-first.
    fn box_shadows(layout: &LayoutBox) -> Vec<BoxShadow> {
        let sv = &layout.style_node.specified_values;
        match sv.get(&crate::css::intern("box-shadow-layers")) {
            Some(Value::BoxShadowList(list)) => list.iter().rev().cloned().collect(),
            _ => match sv.get(&crate::css::intern("box-shadow")) {
                Some(Value::BoxShadow(shadow)) => vec![shadow.clone()],
                _ => Vec::new(),
            },
        }
    }

    /// Per-side border widths, colors and styles, or `None` when no side
    /// paints. A missing `border-*-color` falls back to `currentColor`.
    fn border_sides(layout: &LayoutBox) -> Option<BorderSides> {
        let sv = &layout.style_node.specified_values;
        let b = layout.border;
        let widths = [b.top, b.right, b.bottom, b.left];
        if widths.iter().all(|w| *w <= 0.0) {
            return None;
        }
        let current_color = match sv.get(&crate::css::intern("color")) {
            Some(Value::Color(c)) => c.clone(),
            _ => Color { r: 0, g: 0, b: 0, a: 255 },
        };
        let mut colors: [Color; 4] = std::array::from_fn(|_| current_color.clone());
        let mut styles = [BorderStyle::Solid; 4];
        let mut widths = widths;
        for (i, side) in ["top", "right", "bottom", "left"].iter().enumerate() {
            let color = sv
                .get(&crate::css::intern(&format!("border-{side}-color")))
                .or_else(|| sv.get(&crate::css::intern("border-color")));
            if let Some(Value::Color(c)) = color {
                colors[i] = c.clone();
            }
            let style = sv
                .get(&crate::css::intern(&format!("border-{side}-style")))
                .or_else(|| sv.get(&crate::css::intern("border-style")));
            match style {
                Some(Value::Keyword(k)) => match k.as_ref() {
                    "dashed" => styles[i] = BorderStyle::Dashed,
                    "dotted" => styles[i] = BorderStyle::Dotted,
                    "none" | "hidden" => widths[i] = 0.0,
                    _ => {}
                },
                // No style declaration at all: keep an explicit width solid.
                _ => {}
            }
        }
        if widths.iter().all(|w| *w <= 0.0) {
            return None;
        }
        Some(BorderSides { widths, colors, styles })
    }

    /// `src` of an `<iframe>` box that loads a network document (`http(s)` or
    /// relative), or `None` for no src, `about:`, `data:`, `javascript:` and
    /// `srcdoc` frames.
    fn iframe_src(layout: &LayoutBox) -> Option<String> {
        let NodeData::Element { ref name, ref attrs, .. } = layout.style_node.node.data else { return None };
        if name.local.as_ref() != "iframe" {
            return None;
        }
        let attrs = attrs.borrow();
        if let Some(srcdoc) = attrs.iter().find(|a| a.name.local.as_ref() == "srcdoc") {
            return Some(srcdoc_paint_src(&srcdoc.value));
        }
        let src = attrs.iter().find(|a| a.name.local.as_ref() == "src")?.value.trim().to_string();
        if src.is_empty() {
            return None;
        }
        if let Some((scheme, _)) = src.split_once(':') {
            let scheme = scheme.to_ascii_lowercase();
            let is_scheme = !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
            if is_scheme && !matches!(scheme.as_str(), "http" | "https") {
                return None;
            }
        }
        Some(src)
    }

    /// Whether `layout` is a `<video>` element box.
    fn is_video_element(layout: &LayoutBox) -> bool {
        matches!(layout.style_node.node.data, NodeData::Element { ref name, .. } if name.local.as_ref() == "video")
    }

    /// Whether `layout` is an `<svg>` element box.
    fn is_svg_element(layout: &LayoutBox) -> bool {
        matches!(layout.style_node.node.data, NodeData::Element { ref name, .. } if name.local.as_ref() == "svg")
    }

    /// Whether a layer-creating box also establishes a stacking context. A
    /// positioned box with `z-index: auto` gets its own layer but does not:
    /// its positioned descendants take part in the enclosing context.
    fn establishes_stacking_context(layout: &LayoutBox, triggers: &[CompositingTrigger]) -> bool {
        let z_index_auto = match layout.style_node.specified_values.get(&crate::css::intern("z-index")) {
            None => true,
            Some(Value::Keyword(k)) => k.as_ref() == "auto",
            Some(_) => false,
        };
        let only_positioning = triggers.iter().all(|t| matches!(t, CompositingTrigger::ZIndex(_)));
        !(only_positioning && z_index_auto)
    }

    /// Returns `true` if this box has `overflow: hidden` set.
    fn has_overflow_hidden(layout: &LayoutBox) -> bool {
        // We do not scroll, so auto/scroll clip like hidden. A single clipped
        // axis still clips the box (Chromium turns the other axis into auto).
        ["overflow", "overflow-x", "overflow-y"].iter().any(|prop| {
            match layout.style_node.specified_values.get(&crate::css::intern(prop)) {
                Some(Value::Keyword(k)) => matches!(k.as_ref(), "hidden" | "clip" | "auto" | "scroll"),
                _ => false,
            }
        })
    }

    /// Clip regions that apply to `layout` itself and to its children.
    ///
    /// Absolute boxes escape clips whose box does not contain their containing
    /// block; fixed boxes escape all of them. The CSS `clip` property (absolute
    /// and fixed boxes only) clips the box itself and its descendants; an
    /// `overflow` clip applies to descendants only.
    fn clip_regions_for(
        layout: &LayoutBox,
        inherited: Rc<Vec<ClipRegion>>,
    ) -> (Rc<Vec<ClipRegion>>, Rc<Vec<ClipRegion>>) {
        let positioned = !matches!(layout.position, PositionType::Static);
        let mut own: Vec<ClipRegion> = match layout.position {
            PositionType::Fixed => Vec::new(),
            PositionType::Absolute => inherited.iter().copied().filter(|c| c.clips_absolute).collect(),
            _ => inherited.as_ref().clone(),
        };
        if positioned {
            // This box becomes the containing block of absolute descendants.
            for c in &mut own {
                c.clips_absolute = true;
            }
        }
        if matches!(layout.position, PositionType::Absolute | PositionType::Fixed) {
            if let Some(rect) = Self::css_clip_rect(layout) {
                own.push(ClipRegion { rect, radius: 0.0, clips_absolute: true });
            }
        }
        let own = if own == *inherited { inherited } else { Rc::new(own) };

        if !Self::has_overflow_hidden(layout) {
            return (own.clone(), own);
        }
        let mut children = own.as_ref().clone();
        children.push(ClipRegion {
            rect: crate::background::box_rect(layout, crate::background::BoxArea::Padding),
            radius: border_radius_px(layout),
            clips_absolute: positioned,
        });
        (own, Rc::new(children))
    }

    /// Resolve CSS `clip: rect(top right bottom left)` against the border box.
    /// `auto` edges keep the border edge.
    fn css_clip_rect(layout: &LayoutBox) -> Option<LayoutRect> {
        let raw = match layout.style_node.specified_values.get(&crate::css::intern("clip"))? {
            Value::Keyword(k) => k.to_string(),
            _ => return None,
        };
        let raw = raw.trim().to_ascii_lowercase();
        let inner = raw.strip_prefix("rect(")?.strip_suffix(')')?;
        let parts: Vec<&str> = inner
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() != 4 {
            return None;
        }
        let d = layout.dimensions;
        let edge = |part: &str, auto: f32| -> Option<f32> {
            if part == "auto" {
                return Some(auto);
            }
            part.trim_end_matches("px").parse::<f32>().ok()
        };
        let top = edge(parts[0], 0.0)?;
        let right = edge(parts[1], d.width)?;
        let bottom = edge(parts[2], d.height)?;
        let left = edge(parts[3], 0.0)?;
        Some(LayoutRect {
            x: d.x + left,
            y: d.y + top,
            width: (right - left).max(0.0),
            height: (bottom - top).max(0.0),
        })
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

        let w = layout.dimensions.width;
        let h = layout.dimensions.height;
        let local = match sv.get(&crate::css::intern("transform")) {
            Some(Value::Transform(ops)) => Some(Self::compute_transform_matrix(ops, w, h)),
            // Function lists the CSS parser keeps as text (e.g. `skew()`).
            Some(Value::Keyword(k)) if k.contains('(') => keyword_transform_matrix(k, w, h),
            _ => None,
        };
        if let Some(local) = local {
            // Apply the transform around `transform-origin` (default: the
            // border box centre): T(origin) · M · T(-origin).
            let (ox, oy) = transform_origin(sv.get(&crate::css::intern("transform-origin")), w, h);
            matrix = Matrix4x4::translate(ox, oy, 0.0)
                .multiply(&local)
                .multiply(&Matrix4x4::translate(-ox, -oy, 0.0));
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

        let radius = border_radius_px(layout);

        let mut commands = Vec::new();

        let box_shadows = Self::box_shadows(layout);

        // Outset box-shadow paints behind the background/border, like a drop
        // shadow cast by the box onto whatever is underneath it.
        for shadow in box_shadows.iter().filter(|s| !s.inset) {
            commands.push(PaintCommand::Shadow(d, shadow.clone(), radius));
        }

        // Background: color first, then image layers bottom-most first.
        let layers = crate::background::background_layers(sv);
        let inner_radius = |area: crate::background::BoxArea| -> f32 {
            match area {
                crate::background::BoxArea::Border => radius,
                crate::background::BoxArea::Padding => (radius - layout.border.left).max(0.0),
                crate::background::BoxArea::Content => (radius - layout.border.left - layout.padding.left).max(0.0),
            }
        };
        // `background-color` is clipped like the bottom-most image layer.
        let color_clip = layers.first().map(|l| l.clip).unwrap_or(crate::background::BoxArea::Border);
        let bg_color = sv.get(&crate::css::intern("background-color"))
            .or_else(|| sv.get(&crate::css::intern("background")));
        if let Some(Value::Color(c)) = bg_color {
            if c.a > 0 {
                let r = crate::background::box_rect(layout, color_clip);
                commands.push(PaintCommand::Rect(r, c.clone(), inner_radius(color_clip)));
            }
        }
        for layer in layers {
            let clip_rect = crate::background::box_rect(layout, layer.clip);
            let clip_radius = inner_radius(layer.clip);
            match layer.image {
                crate::background::LayerImage::Gradient(GradientValue::Linear { direction, stops }) => {
                    commands.push(PaintCommand::LinearGradient { rect: clip_rect, direction, stops, radius: clip_radius });
                }
                crate::background::LayerImage::Gradient(GradientValue::Radial { stops, .. }) => {
                    commands.push(PaintCommand::RadialGradient { rect: clip_rect, stops, radius: clip_radius });
                }
                crate::background::LayerImage::Url(url) => {
                    commands.push(PaintCommand::BackgroundImage {
                        url,
                        clip: clip_rect,
                        radius: clip_radius,
                        area: crate::background::box_rect(layout, layer.origin),
                        position: layer.position,
                        size: layer.size,
                        repeat_x: layer.repeat_x,
                        repeat_y: layer.repeat_y,
                    });
                }
            }
        }

        // Border. `border-style: none`/`hidden` (the initial value) suppresses
        // a side entirely, even when a width/color is set, per spec; layout
        // already resolves such sides to width 0.
        if let Some(border) = Self::border_sides(layout) {
            let uniform = border.widths.iter().all(|w| (*w - border.widths[0]).abs() < 0.01)
                && border.colors.iter().all(|c| *c == border.colors[0])
                && border.styles.iter().all(|st| *st == border.styles[0]);
            if uniform {
                commands.push(PaintCommand::Border(d, border.widths[0], border.colors[0].clone(), radius, border.styles[0]));
            } else {
                commands.push(PaintCommand::BorderSides { rect: d, sides: Box::new(border), radius });
            }
        }

        // Inset box-shadow paints on top of the background (and below any
        // content/border painted after it), simulating a shadow cast onto the
        // box's own interior from its edges.
        for shadow in box_shadows.iter().filter(|s| s.inset) {
            commands.push(PaintCommand::Shadow(d, shadow.clone(), radius));
        }

        // A <video> is black where it has no frame to show (no poster; we do
        // not decode video), as Chromium paints one without a frame.
        if Self::is_video_element(layout) {
            let content = crate::background::box_rect(layout, crate::background::BoxArea::Content);
            if content.width > 0.0 && content.height > 0.0 {
                let black = Color { r: 0, g: 0, b: 0, a: 255 };
                commands.push(PaintCommand::Rect(content, black, inner_radius(crate::background::BoxArea::Content)));
            }
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
                    // A video poster keeps its aspect ratio inside the box.
                    _ if Self::is_video_element(layout) => ObjectFit::Contain,
                    _ => ObjectFit::Fill,
                };
                let alt = layout.alt_text.clone().unwrap_or_default();
                // Replaced content fills the content box, clipped to the
                // correspondingly reduced border radius.
                let content = crate::background::box_rect(layout, crate::background::BoxArea::Content);
                let content_radius = inner_radius(crate::background::BoxArea::Content);
                commands.push(PaintCommand::Image { rect: content, url: url.clone(), object_fit, alt, radius: content_radius });
            } else if Self::is_svg_element(layout) {
                let content = crate::background::box_rect(layout, crate::background::BoxArea::Content);
                if content.width > 0.0 && content.height > 0.0 {
                    let source = crate::svg::serialize_inline(layout.style_node, content.width, content.height);
                    commands.push(PaintCommand::Svg { rect: content, source: source.into() });
                }
            } else if let Some(src) = Self::iframe_src(layout) {
                // The frame's rendered document, if the frames track has put
                // it in the image cache; otherwise nothing (no placeholder).
                let content = crate::background::box_rect(layout, crate::background::BoxArea::Content);
                let (w, h) = (content.width.round() as u32, content.height.round() as u32);
                if w > 0 && h > 0 {
                    commands.push(PaintCommand::Image {
                        rect: content,
                        url: iframe_frame_key(&src, w, h),
                        object_fit: ObjectFit::Fill,
                        alt: String::new(),
                        radius: inner_radius(crate::background::BoxArea::Content),
                    });
                }
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
            let font_weight = crate::fonts::parse_weight(sv.get(&crate::css::intern("font-weight")));
            let bold = font_weight >= 600;
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
                // Line fragments carry only the text shown on their line.
                text: layout.text_fragment.clone().unwrap_or_else(|| contents.borrow().to_string()),
                font_size,
                color,
                clip,
                bold,
                italic,
                text_decoration,
                font_family: text_font_family(sv),
                font_weight,
                // Line fragments are the glyph content area (ascent + descent), so
                // their own height puts the baseline at rect.y + ascent.
                line_height: if layout.text_fragment.is_some() {
                    d.height
                } else {
                    crate::fonts::line_height_px(sv, font_size)
                },
                letter_spacing: crate::fonts::letter_spacing_px(sv, font_size),
            });
        }

        // List item marker (• / ◦ / ▪ / "1." etc.)
        // Painted to the left of the list item's content box, inside the padding-left area.
        if layout.display == DisplayType::ListItem {
            if let Some(ref marker) = layout.list_marker {
                let font_size = match sv.get(&crate::css::intern("font-size")) {
                    Some(Value::Length(v, _)) => *v,
                    _ => 16.0,
                };
                let color = match sv.get(&crate::css::intern("color")) {
                    Some(Value::Color(c)) => c.clone(),
                    _ => crate::css::Color { r: 0, g: 0, b: 0, a: 255 },
                };
                // Place marker ~20px to the left of the content box left edge.
                // The padding-left (~40px) leaves enough room for the marker.
                let marker_x = (d.x - 20.0).max(0.0);
                let line_height = crate::fonts::line_height_px(sv, font_size);
                let marker_rect = crate::layout::Rect {
                    x: marker_x,
                    y: d.y,
                    width: 20.0,
                    height: line_height,
                };
                commands.push(PaintCommand::Text {
                    rect: marker_rect,
                    text: marker.clone(),
                    font_size,
                    color,
                    clip,
                    bold: false,
                    italic: false,
                    text_decoration: 0,
                    font_family: text_font_family(sv),
                    font_weight: 400,
                    line_height,
                    letter_spacing: 0.0,
                });
            }
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
                // Center one `line-height: normal` line box vertically in the button.
                let line_height = crate::fonts::chain_for(sv).metrics(font_size).normal_line_height();
                let mid_y = d.y + d.height / 2.0;
                let text_rect = crate::layout::Rect {
                    x: d.x + layout.padding.left,
                    y: mid_y - line_height / 2.0,
                    width: (d.width - layout.padding.left - layout.padding.right).max(0.0),
                    height: line_height,
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
                    font_family: text_font_family(sv),
                    font_weight: crate::fonts::parse_weight(sv.get(&crate::css::intern("font-weight"))),
                    line_height,
                    letter_spacing: crate::fonts::letter_spacing_px(sv, font_size),
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
                PaintCommand::BorderSides { rect, .. } => *rect,
                PaintCommand::Image { rect, .. } => *rect,
                PaintCommand::Svg { rect, .. } => *rect,
                PaintCommand::Text { rect, .. } => *rect,
                PaintCommand::Shadow(r, ..) => *r,
                PaintCommand::LinearGradient { rect, .. } => *rect,
                PaintCommand::RadialGradient { rect, .. } => *rect,
                PaintCommand::BackgroundImage { clip, .. } => *clip,
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

/// Used `border-radius` in px. A percentage resolves against the smaller
/// border-box side (exact for the common `50%` circle on square boxes; the
/// painter has one circular radius per box, so ellipses become pills).
fn border_radius_px(layout: &LayoutBox) -> f32 {
    match layout.style_node.specified_values.get(&crate::css::intern("border-radius")) {
        Some(Value::Length(v, crate::css::Unit::Percent)) => {
            let d = layout.dimensions;
            (*v / 100.0 * d.width.min(d.height)).max(0.0)
        }
        Some(Value::Length(v, _)) => v.max(0.0),
        _ => 0.0,
    }
}


/// Identity `src` of an `<iframe srcdoc>` frame (with its ordinal among such
/// frames); shared with page script's frame routing.
pub const SRCDOC_FRAME_SRC: &str = "about:srcdoc";

/// Paint-side `src` of an `<iframe srcdoc>` document: `about:srcdoc` plus a
/// hash of the markup, so a rewritten document gets a new cache key.
pub fn srcdoc_paint_src(html: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    html.hash(&mut hasher);
    format!("{SRCDOC_FRAME_SRC}#{:016x}", hasher.finish())
}

/// Image-cache key under which the rendered document of an `<iframe>` with
/// `src` and a `width` x `height` px content box is stored (as PNG bytes).
/// Layer building has no base URL, so `src` may be relative here; the
/// renderer resolves it against the page URL before the cache lookup, so the
/// cached key always carries the absolute URL.
pub fn iframe_frame_key(src: &str, width: u32, height: u32) -> String {
    format!("iframe:{width}x{height}:{src}")
}

/// Split an `iframe_frame_key` into its `(width, height, src)` parts.
pub fn parse_iframe_frame_key(key: &str) -> Option<(u32, u32, &str)> {
    let rest = key.strip_prefix("iframe:")?;
    let (size, src) = rest.split_once(':')?;
    let (w, h) = size.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?, src))
}

/// Matrix of a 2D CSS transform function list kept as raw text by the CSS
/// parser (it only models translate/scale/rotate/matrix). Supports those plus
/// `skew`, `skewX` and `skewY`; `None` if any function is unknown.
fn keyword_transform_matrix(text: &str, w: f32, h: f32) -> Option<Matrix4x4> {
    let angle = |a: &str| -> Option<f32> {
        let a = a.trim().to_ascii_lowercase();
        if let Some(v) = a.strip_suffix("deg") {
            v.parse::<f32>().ok().map(f32::to_radians)
        } else if let Some(v) = a.strip_suffix("grad") {
            v.parse::<f32>().ok().map(|g| g * std::f32::consts::PI / 200.0)
        } else if let Some(v) = a.strip_suffix("rad") {
            v.parse::<f32>().ok()
        } else if let Some(v) = a.strip_suffix("turn") {
            v.parse::<f32>().ok().map(|t| t * std::f32::consts::TAU)
        } else {
            a.parse::<f32>().ok().filter(|v| *v == 0.0)
        }
    };
    let length = |a: &str, size: f32| -> Option<f32> {
        let a = a.trim();
        if let Some(p) = a.strip_suffix('%') {
            p.parse::<f32>().ok().map(|p| p / 100.0 * size)
        } else {
            a.trim_end_matches("px").parse::<f32>().ok()
        }
    };
    let mut result = Matrix4x4::identity();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let open = rest.find('(')?;
        let close = open + rest[open..].find(')')?;
        let name = rest[..open].trim().to_ascii_lowercase();
        let args: Vec<&str> = rest[open + 1..close]
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|a| !a.is_empty())
            .collect();
        let arg = |i: usize| args.get(i).copied();
        // Row-major [a c e; b d f; 0 0 1].
        let m = |a: f32, b: f32, c: f32, d: f32, e: f32, f: f32| Matrix4x4::from_2d(Matrix3x3([a, c, e, b, d, f, 0.0, 0.0, 1.0]));
        let op = match name.as_str() {
            "translate" => m(1.0, 0.0, 0.0, 1.0, length(arg(0)?, w)?, arg(1).map_or(Some(0.0), |a| length(a, h))?),
            "translatex" => m(1.0, 0.0, 0.0, 1.0, length(arg(0)?, w)?, 0.0),
            "translatey" => m(1.0, 0.0, 0.0, 1.0, 0.0, length(arg(0)?, h)?),
            "scale" => {
                let sx = arg(0)?.parse::<f32>().ok()?;
                let sy = arg(1).map_or(Some(sx), |a| a.parse::<f32>().ok())?;
                m(sx, 0.0, 0.0, sy, 0.0, 0.0)
            }
            "scalex" => m(arg(0)?.parse::<f32>().ok()?, 0.0, 0.0, 1.0, 0.0, 0.0),
            "scaley" => m(1.0, 0.0, 0.0, arg(0)?.parse::<f32>().ok()?, 0.0, 0.0),
            "rotate" | "rotatez" => {
                let (sin, cos) = angle(arg(0)?)?.sin_cos();
                m(cos, sin, -sin, cos, 0.0, 0.0)
            }
            "skew" => {
                let ax = angle(arg(0)?)?;
                let ay = arg(1).map_or(Some(0.0), angle)?;
                m(1.0, ay.tan(), ax.tan(), 1.0, 0.0, 0.0)
            }
            "skewx" => m(1.0, 0.0, angle(arg(0)?)?.tan(), 1.0, 0.0, 0.0),
            "skewy" => m(1.0, angle(arg(0)?)?.tan(), 0.0, 1.0, 0.0, 0.0),
            "matrix" => {
                let v: Vec<f32> = args.iter().filter_map(|a| a.parse::<f32>().ok()).collect();
                if v.len() != 6 {
                    return None;
                }
                m(v[0], v[1], v[2], v[3], v[4], v[5])
            }
            _ => return None,
        };
        result = result.multiply(&op);
        rest = rest[close + 1..].trim_start();
    }
    Some(result)
}

/// Resolve CSS `transform-origin` against a `w` x `h` border box, as an offset
/// from its top-left corner. Defaults to the centre; a single value sets the
/// horizontal position (or the vertical one for `top`/`bottom`).
fn transform_origin(value: Option<&Value>, w: f32, h: f32) -> (f32, f32) {
    let text = match value {
        Some(Value::Keyword(k)) => k.to_string(),
        Some(Value::Length(v, crate::css::Unit::Percent)) => format!("{v}%"),
        Some(Value::Length(v, _)) => format!("{v}px"),
        Some(Value::Number(v)) => format!("{v}px"),
        _ => return (w / 2.0, h / 2.0),
    };
    let tokens: Vec<String> = text.split_whitespace().map(|t| t.to_ascii_lowercase()).collect();
    let resolve = |t: &str, size: f32| -> Option<f32> {
        match t {
            "left" | "top" => Some(0.0),
            "center" => Some(size / 2.0),
            "right" | "bottom" => Some(size),
            _ => {
                if let Some(p) = t.strip_suffix('%') {
                    p.parse::<f32>().ok().map(|p| p / 100.0 * size)
                } else {
                    t.trim_end_matches("px").parse::<f32>().ok()
                }
            }
        }
    };
    let (mut x_tok, mut y_tok) = (tokens.first().map(String::as_str), tokens.get(1).map(String::as_str));
    // Keywords may come in either order (`top left`).
    if matches!(x_tok, Some("top" | "bottom")) || matches!(y_tok, Some("left" | "right")) {
        std::mem::swap(&mut x_tok, &mut y_tok);
    }
    let x = x_tok.and_then(|t| resolve(t, w)).unwrap_or(w / 2.0);
    let y = y_tok.and_then(|t| resolve(t, h)).unwrap_or(h / 2.0);
    (x, y)
}

/// Raw CSS `font-family` of a node (`serif` when unset, the initial value).
fn text_font_family(sv: &std::collections::HashMap<std::sync::Arc<str>, Value>) -> std::sync::Arc<str> {
    match sv.get(&crate::css::intern("font-family")) {
        Some(Value::Keyword(k)) => k.clone(),
        _ => crate::css::intern("serif"),
    }
}

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

    fn all_commands(tree: &LayerTree) -> Vec<&PaintCommand> {
        tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .collect()
    }

    #[test]
    fn test_background_shorthand_emits_color_then_image() {
        let tree = build_tree_from_html(
            r#"<div class="ico"></div>"#,
            ".ico{width:20px;height:20px;background:url(https://x/sp.png) -10px -20px/100px 50px no-repeat #fff}",
        );
        let cmds = all_commands(&tree);
        let color_idx = cmds.iter().position(|c| matches!(c, PaintCommand::Rect(_, c, _) if c.r == 255 && c.g == 255));
        let img_idx = cmds.iter().position(|c| matches!(c,
            PaintCommand::BackgroundImage { url, repeat_x: false, repeat_y: false, .. } if url == "https://x/sp.png"));
        assert!(color_idx.is_some() && img_idx.is_some(), "expected color + image commands, got {:?}", cmds);
        assert!(color_idx < img_idx, "background-color must paint below the image");
    }

    #[test]
    fn test_background_image_longhands_emit_command_with_position_and_size() {
        let tree = build_tree_from_html(
            r#"<span class="s"></span>"#,
            ".s{display:inline-block;width:20px;height:20px;background-image:url(sp.png);background-size:484px 476px;background-position:-468px -126px;background-repeat:no-repeat}",
        );
        let cmd = all_commands(&tree).into_iter().find_map(|c| match c {
            PaintCommand::BackgroundImage { position, size, .. } => Some((*position, *size)),
            _ => None,
        });
        let (position, size) = cmd.expect("BackgroundImage command");
        assert_eq!(position.x.resolve(0.0), -468.0);
        assert_eq!(position.y.resolve(0.0), -126.0);
        assert_eq!(crate::background::tile_size(size, 968.0, 952.0, 20.0, 20.0), (484.0, 476.0));
    }

    #[test]
    fn test_background_gradient_shorthand_still_paints_gradient() {
        let tree = build_tree_from_html(
            r#"<div class="g"></div>"#,
            ".g{width:20px;height:20px;background:linear-gradient(to right,red,blue)}",
        );
        assert!(all_commands(&tree).iter().any(|c| matches!(c, PaintCommand::LinearGradient { .. })));
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

    /// An `inset` box-shadow must still emit a `PaintCommand::Shadow`, but
    /// (unlike an outset shadow) it must appear AFTER the background rect,
    /// since it paints on top of the background rather than behind it.
    #[test]
    fn test_inset_box_shadow_appears_after_background() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;background-color:red;box-shadow:inset 0 0 4px #888;">Content</div>"#,
            "",
        );
        let cmds: Vec<&PaintCommand> = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .collect();

        let shadow_idx = cmds.iter().position(|c| matches!(c, PaintCommand::Shadow(..)));
        let rect_idx   = cmds.iter().position(|c| matches!(c, PaintCommand::Rect(..)));

        assert!(shadow_idx.is_some(), "inset box-shadow must still emit a Shadow command");
        assert!(rect_idx.is_some(),   "must have a Rect (background) command");
        assert!(
            shadow_idx.unwrap() > rect_idx.unwrap(),
            "inset Shadow command (idx {}) must come after background Rect (idx {})",
            shadow_idx.unwrap(), rect_idx.unwrap()
        );
    }

    /// A `PaintCommand::Shadow` must carry the element's border-radius so the
    /// shadow shape follows the box's own rounding.
    #[test]
    fn test_box_shadow_command_carries_border_radius() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;background-color:red;border-radius:12px;box-shadow:0 2px 4px #888;">Content</div>"#,
            "",
        );
        let shadow_radius = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .find_map(|cmd| match cmd {
                PaintCommand::Shadow(_, _, radius) => Some(*radius),
                _ => None,
            });
        assert_eq!(shadow_radius, Some(12.0), "Shadow command must carry the element's border-radius");
    }

    /// `border-style: dashed` must emit a `Border` command tagged `Dashed`.
    #[test]
    fn test_border_style_dashed_emits_dashed_border_command() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;border:2px dashed #333;">Content</div>"#,
            "",
        );
        let style = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .find_map(|cmd| match cmd {
                PaintCommand::Border(_, _, _, _, style) => Some(*style),
                _ => None,
            });
        assert_eq!(style, Some(BorderStyle::Dashed), "border:2px dashed must emit a Border command tagged Dashed");
    }

    /// `border-style: dotted` must emit a `Border` command tagged `Dotted`.
    #[test]
    fn test_border_style_dotted_emits_dotted_border_command() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;border:2px dotted #333;">Content</div>"#,
            "",
        );
        let style = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .find_map(|cmd| match cmd {
                PaintCommand::Border(_, _, _, _, style) => Some(*style),
                _ => None,
            });
        assert_eq!(style, Some(BorderStyle::Dotted), "border:2px dotted must emit a Border command tagged Dotted");
    }

    /// `border-style: none` must suppress the `Border` command entirely, even
    /// when a `border-width`/`border-color` is set, matching the CSS spec's
    /// initial value for `border-style`.
    #[test]
    fn test_border_style_none_suppresses_border_command() {
        let tree = build_tree_from_html(
            r#"<div style="width:100px;height:50px;border-width:3px;border-color:red;border-style:none;">Content</div>"#,
            "",
        );
        let has_border = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Border(..)));
        assert!(!has_border, "border-style:none must suppress the Border paint command");
    }

    fn layer_clips_for_text(tree: &LayerTree, marker: &str) -> Vec<ClipRegion> {
        tree.layers
            .iter()
            .find(|l| {
                l.content_commands.iter().chain(l.background_commands.iter()).any(|c| {
                    matches!(c, PaintCommand::Text { text, .. } if text.contains(marker))
                })
            })
            .map(|l| l.ancestor_clips.clone())
            .expect("layer painting the marker text")
    }

    /// An absolute child whose containing block is inside an overflow:hidden
    /// box inherits that clip; one whose containing block is outside does not.
    #[test]
    fn test_absolute_child_inherits_clip_only_through_its_containing_block() {
        let tree = build_tree_from_html(
            r#"<div style="position:relative;width:100px;height:30px;overflow:hidden"><div style="position:absolute;width:300px;height:80px">inside</div></div>
               <div style="width:100px;height:30px;overflow:hidden"><div style="position:absolute;width:300px;height:80px">escapes</div></div>
               <div style="position:relative;width:100px;height:30px;overflow:hidden"><div style="position:fixed;width:300px;height:80px">fixed</div></div>"#,
            "",
        );
        let inside = layer_clips_for_text(&tree, "inside");
        assert_eq!(inside.len(), 1, "absolute child of a positioned clipping box is clipped");
        assert!((inside[0].rect.width - 100.0).abs() < 0.5 && (inside[0].rect.height - 30.0).abs() < 0.5);
        assert!(layer_clips_for_text(&tree, "escapes").is_empty(), "containing block outside the clip");
        assert!(layer_clips_for_text(&tree, "fixed").is_empty(), "fixed boxes escape overflow clips");
    }

    /// `clip: rect(0 0 0 0)` (the `.blind` pattern) yields an empty clip region
    /// for the box itself.
    #[test]
    fn test_css_clip_rect_on_absolute_box_clips_itself() {
        let tree = build_tree_from_html(
            r#"<div style="position:relative"><span style="position:absolute;clip:rect(0 0 0 0);width:1px;height:1px;overflow:hidden">blind label</span></div>"#,
            "",
        );
        let clips = layer_clips_for_text(&tree, "blind");
        assert!(clips.iter().any(|c| c.rect.width == 0.0 && c.rect.height == 0.0), "{clips:?}");
    }

    fn map_point(m: &Matrix4x4, x: f32, y: f32) -> (f32, f32) {
        let mut pts = [tiny_skia::Point::from_xy(x, y)];
        m.to_skia().map_points(&mut pts);
        (pts[0].x, pts[0].y)
    }

    fn transformed_layer(tree: &LayerTree) -> &Layer {
        tree.layers.iter().find(|l| l.triggers.iter().any(|t| matches!(t, CompositingTrigger::Transform(_)))).expect("transform layer")
    }

    /// `rotate(180deg)` turns the box around its centre (the default
    /// `transform-origin`), so its top-left corner lands on its bottom-right.
    #[test]
    fn test_rotate_uses_default_centre_origin() {
        let tree = build_tree_from_html(r#"<div style="width:20px;height:10px;transform:rotate(180deg)"></div>"#, "");
        let (x, y) = map_point(&transformed_layer(&tree).transform, 0.0, 0.0);
        assert!((x - 20.0).abs() < 0.01 && (y - 10.0).abs() < 0.01, "got ({x}, {y})");
    }

    #[test]
    fn test_transform_origin_keywords_and_lengths() {
        let tree = build_tree_from_html(r#"<div style="width:20px;height:10px;transform:rotate(90deg);transform-origin:0 0"></div>"#, "");
        let (x, y) = map_point(&transformed_layer(&tree).transform, 10.0, 0.0);
        assert!((x - 0.0).abs() < 0.01 && (y - 10.0).abs() < 0.01, "origin 0 0: got ({x}, {y})");
        let tree = build_tree_from_html(r#"<div style="width:20px;height:10px;transform:scale(2);transform-origin:right bottom"></div>"#, "");
        let (x, y) = map_point(&transformed_layer(&tree).transform, 20.0, 10.0);
        assert!((x - 20.0).abs() < 0.01 && (y - 10.0).abs() < 0.01, "right bottom is fixed: got ({x}, {y})");
        assert_eq!(transform_origin(Some(&Value::Keyword(css::intern("top"))), 20.0, 10.0), (10.0, 0.0));
        assert_eq!(transform_origin(Some(&Value::Keyword(css::intern("25% 4px"))), 20.0, 10.0), (5.0, 4.0));
    }

    #[test]
    fn test_inline_svg_emits_svg_command_sized_to_content_box() {
        let tree = build_tree_from_html(
            r#"<svg width="24" height="12" viewBox="0 0 2 1" style="padding:2px"><rect width="2" height="1"></rect></svg>"#,
            "",
        );
        let svg = all_commands(&tree).into_iter().find_map(|c| match c {
            PaintCommand::Svg { rect, source } => Some((*rect, source.clone())),
            _ => None,
        }).expect("svg command");
        assert!((svg.0.width - 24.0).abs() < 0.5 && (svg.0.height - 12.0).abs() < 0.5, "{:?}", svg.0);
        assert!(svg.1.contains("<rect"), "{}", svg.1);
    }

    /// A spread shadow around a 0px-tall box still paints (Chromium draws the
    /// 1px ring of an empty ad iframe as a line).
    #[test]
    fn test_zero_height_box_still_emits_spread_shadow() {
        let tree = build_tree_from_html(r#"<div style="width:100px;height:0;box-shadow:0 0 0 1px #e5e5e5"></div>"#, "");
        assert!(all_commands(&tree).iter().any(|c| matches!(c, PaintCommand::Shadow(..))));
    }

    #[test]
    fn test_box_shadow_list_emits_every_layer_bottom_first() {
        let tree = build_tree_from_html(r#"<div style="width:10px;height:10px;box-shadow:0 0 0 1px #111111, 0 2px 2px #222222"></div>"#, "");
        let colors: Vec<u8> = all_commands(&tree).iter().filter_map(|c| match c {
            PaintCommand::Shadow(_, s, _) => Some(s.color.r),
            _ => None,
        }).collect();
        assert_eq!(colors, vec![0x22, 0x11]);
    }

    #[test]
    fn test_per_side_border_emits_border_sides_with_current_color() {
        let tree = build_tree_from_html(
            r#"<div style="color:#00ff00;width:10px;height:10px;border-left:3px solid #0000ff;border-bottom:1px solid"></div>"#,
            "",
        );
        let sides = all_commands(&tree).into_iter().find_map(|c| match c {
            PaintCommand::BorderSides { sides, .. } => Some(sides.clone()),
            _ => None,
        }).expect("border sides command");
        assert_eq!(sides.widths, [0.0, 0.0, 1.0, 3.0]);
        assert_eq!(sides.colors[3].b, 255);
        assert_eq!(sides.colors[2].g, 255, "missing border color is currentColor");
    }

    #[test]
    fn test_iframe_emits_frame_key_image_only_for_network_src() {
        let tree = build_tree_from_html(
            r#"<iframe src="/ad?x=1" style="width:30px;height:20px;border:0"></iframe><iframe style="width:30px;height:20px"></iframe><iframe src="about:blank" style="width:30px;height:20px"></iframe>"#,
            "",
        );
        let urls: Vec<String> = all_commands(&tree).iter().filter_map(|c| match c {
            PaintCommand::Image { url, .. } => Some(url.clone()),
            _ => None,
        }).collect();
        assert_eq!(urls, vec![iframe_frame_key("/ad?x=1", 30, 20)]);
        assert_eq!(parse_iframe_frame_key("iframe:30x20:https://a.b/c:1"), Some((30, 20, "https://a.b/c:1")));
    }

    /// `skew()` is kept as text by the CSS parser and resolved here.
    #[test]
    fn test_skew_transform_is_applied_around_the_centre() {
        let tree = build_tree_from_html(r#"<div style="width:10px;height:10px;transform:skewX(45deg)"></div>"#, "");
        let (x, y) = map_point(&transformed_layer(&tree).transform, 5.0, 10.0);
        assert!((x - 10.0).abs() < 0.01 && (y - 10.0).abs() < 0.01, "got ({x}, {y})");
        let tree = build_tree_from_html(r#"<div style="width:1px;height:15px;transform:skew(-15deg)"></div>"#, "");
        let (x, _) = map_point(&transformed_layer(&tree).transform, 0.5, 0.0);
        assert!((x - (0.5 + 7.5 * 15f32.to_radians().tan())).abs() < 0.01, "top edge leans right: {x}");
    }

    #[test]
    fn test_percent_border_radius_resolves_against_box_size() {
        let tree = build_tree_from_html(r#"<div style="width:120px;height:120px;border-radius:50%;background:#000"></div>"#, "");
        let radius = all_commands(&tree).iter().find_map(|c| match c {
            PaintCommand::Rect(_, _, r) => Some(*r),
            _ => None,
        }).expect("background rect");
        assert!((radius - 60.0).abs() < 0.01, "got {radius}");
    }

    /// A `z-index: 10` box inside a positioned `z-index: auto` parent is
    /// ordered in the root stacking context, above a later positioned sibling
    /// of that parent.
    #[test]
    fn test_z_index_escapes_positioned_z_auto_parent() {
        let tree = build_tree_from_html(
            r#"<div style="position:relative;width:100px;height:20px"><div style="position:relative;z-index:10;width:100px;height:20px">high</div></div>
               <div style="position:absolute;left:0;top:0;width:50px;height:50px">later</div>"#,
            "",
        );
        let layer_with = |marker: &str| {
            tree.layers
                .iter()
                .position(|l| l.background_commands.iter().chain(l.content_commands.iter()).any(|c| {
                    matches!(c, PaintCommand::Text { text, .. } if text.contains(marker))
                }))
                .expect("layer painting the marker")
        };
        let (high, later) = (layer_with("high"), layer_with("later"));
        let (_, zero, positive) = tree.categorize_children(0);
        assert!(positive.contains(&high), "z-index 10 box is a positive child of the root context");
        assert!(zero.contains(&later), "z-index auto box stays in the root's zero list");
    }

    /// Text runs carry no viewport clip: a transform may move text laid out
    /// past the canvas edge back into view.
    #[test]
    fn test_text_clip_is_not_bounded_by_the_viewport() {
        let tree = build_tree_from_html(
            r#"<div style="position:absolute;left:900px;top:0;width:300px;transform:translateX(-80%)">far text</div>"#,
            "",
        );
        let clip = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .find_map(|c| match c {
                PaintCommand::Text { text, clip, .. } if text.contains("far") => Some(*clip),
                _ => None,
            })
            .expect("text command");
        assert!(clip.x + clip.width > 1200.0, "text clip must extend past the canvas: {clip:?}");
    }

    /// An `<iframe srcdoc>` paints its document from the cache key built on a
    /// hash of the markup, so rewriting the markup changes the key.
    #[test]
    fn test_srcdoc_iframe_paints_from_markup_key() {
        let tree = build_tree_from_html(r#"<iframe srcdoc="<p>hi</p>" style="width:100px;height:50px;border:0"></iframe>"#, "");
        let url = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .find_map(|c| match c { PaintCommand::Image { url, .. } => Some(url.clone()), _ => None })
            .expect("iframe image command");
        assert_eq!(url, iframe_frame_key(&srcdoc_paint_src("<p>hi</p>"), 100, 50));
        assert_ne!(srcdoc_paint_src("<p>hi</p>"), srcdoc_paint_src("<p>bye</p>"));
    }

    /// A `<video>` without a poster paints black; with one, the poster is
    /// drawn over it keeping its aspect ratio.
    #[test]
    fn test_video_paints_black_and_its_poster() {
        let tree = build_tree_from_html(
            r#"<video style="width:160px;height:90px"></video><video poster="p.png" style="width:160px;height:90px"></video>"#,
            "",
        );
        let cmds: Vec<&PaintCommand> = tree.layers.iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .collect();
        let black = cmds.iter().filter(|c| matches!(c, PaintCommand::Rect(r, col, _) if col.r == 0 && col.g == 0 && col.b == 0 && col.a == 255 && (r.width - 160.0).abs() < 0.5)).count();
        assert_eq!(black, 2, "both videos paint black");
        assert!(cmds.iter().any(|c| matches!(c, PaintCommand::Image { url, object_fit: ObjectFit::Contain, .. } if url == "p.png")));
    }
}
