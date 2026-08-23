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

/// The four corner radii of a box, each horizontal and vertical.
///
/// CSS gives every corner both, and they only coincide when the radius is a
/// length: `border-radius: 50%` on a wide box is a full ellipse, not a stadium.
/// It also lets the four corners differ — `border-radius: 24px 24px 0 0` is how
/// a page rounds the top of a panel and leaves it flush at the bottom — so one
/// radius for the whole box drew a card square where the page wanted it curved.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CornerRadii {
    pub top_left: (f32, f32),
    pub top_right: (f32, f32),
    pub bottom_right: (f32, f32),
    pub bottom_left: (f32, f32),
}

impl CornerRadii {
    pub const NONE: Self = CornerRadii {
        top_left: (0.0, 0.0),
        top_right: (0.0, 0.0),
        bottom_right: (0.0, 0.0),
        bottom_left: (0.0, 0.0),
    };

    /// The same radius on every corner and both axes — what a single length
    /// radius gives.
    pub fn uniform(r: f32) -> Self {
        CornerRadii {
            top_left: (r, r),
            top_right: (r, r),
            bottom_right: (r, r),
            bottom_left: (r, r),
        }
    }

    /// One radius per axis, the same on every corner.
    pub fn elliptical(x: f32, y: f32) -> Self {
        CornerRadii {
            top_left: (x, y),
            top_right: (x, y),
            bottom_right: (x, y),
            bottom_left: (x, y),
        }
    }

    pub fn is_rounded(self) -> bool {
        [self.top_left, self.top_right, self.bottom_right, self.bottom_left]
            .iter()
            .any(|(x, y)| *x > 0.0 && *y > 0.0)
    }
}


/// How the lines of a run sit inside the box the run was measured into.
///
/// `text-align` places *every* line by its own width, so paint needs it: the
/// box carries only where the widest line goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    /// The lines start at the box's own edge — `left`, `start`, and the default.
    Start,
    Center,
    /// `right` and `end`.
    End,
}

impl TextAlign {
    /// The alignment a `text-align` keyword asks for. `justify` is not stretched
    /// here, so it reads as the start edge, which is where its last line sits.
    pub fn from_keyword(k: &str) -> Self {
        match k {
            "center" => TextAlign::Center,
            "right" | "end" => TextAlign::End,
            _ => TextAlign::Start,
        }
    }
}


/// A single atomic drawing operation. Moved from render.rs so that layer_tree.rs
/// owns the data pipeline (layout → layer tree → paint commands) while render.rs
/// owns the pixel execution (paint commands → Pixmap).
#[derive(Debug, Clone)]
pub enum PaintCommand {
    /// Filled rectangle: (bounds, color, corner-radius)
    /// A solid background fill: box, colour, corner radii (horizontal,
    /// vertical), `filter: blur()` radius (0 for none).
    Rect(LayoutRect, Color, CornerRadii, f32),
    /// Stroked rectangle border: (bounds, stroke-width, color, corner-radius)
    Border(LayoutRect, f32, Color, CornerRadii),
    /// Image: layout rect, source URL, object-fit mode, alt text
    Image {
        rect: LayoutRect,
        url: String,
        object_fit: ObjectFit,
        alt: String,
        /// The inherited text colour and size, for the `alt` a browser shows in
        /// place of an image it could not load.
        alt_color: Color,
        alt_font_size: f32,
        /// The line height the alt text wraps on, so paint lays it out the way
        /// layout sized the box for it.
        alt_line_height: f32,
        /// Whether a source that never arrived leaves a frame behind. A browser
        /// draws one only for a box the page sized on *both* axes; one whose
        /// width came from the layout is left blank.
        framed: bool,
    },
    /// An inline `<svg>` subtree, as markup, to be rasterised into `rect`.
    ///
    /// The element's own `color` travels with it because icon sets are drawn
    /// with `fill="currentColor"`, which means nothing on its own.
    Svg { rect: LayoutRect, source: String, current_color: Color },
    /// Text run with clipping rect
    Text {
        rect: LayoutRect,
        text: String,
        font_size: f32,
        /// Line box height, so paint wraps at the same places layout measured.
        line_height: f32,
        /// Space layout reserved before the first glyph of this run.
        leading_space: f32,
        color: Color,
        clip: LayoutRect,
        /// Which face the run is set in — weight, slant and pitch together.
        /// Layout measured with this face, so paint has to draw with it.
        style: crate::font::FontStyle,
        /// `letter-spacing` in pixels, added after every character.
        letter_spacing: f32,
        /// Bitmask: bit 0 = underline, bit 1 = line-through, bit 2 = overline
        text_decoration: u8,
        /// Whether the source's own newlines are line breaks, as `pre-line`
        /// and friends make them. Layout counted its lines that way, so paint
        /// has to draw them that way.
        preserve_newlines: bool,
        /// Where each of the run's lines sits inside the box. Layout places the
        /// box by its widest line; the rest are settled against that here.
        text_align: TextAlign,
        /// `text-wrap-style`, and the width layout laid the run into. Paint has
        /// to break the lines in the same places, and the box is only as wide as
        /// the widest line — not the room the run actually had.
        wrap_style: crate::layout::WrapStyle,
        wrap_width: f32,
    },
    /// Outer box-shadow
    Shadow(LayoutRect, BoxShadow),
    /// CSS `linear-gradient()` background fill.
    LinearGradient {
        rect: LayoutRect,
        direction: LinearDirection,
        stops: Vec<CssColorStop>,
        radius: CornerRadii,
        /// `filter: blur()` radius in pixels, 0 for none.
        blur: f32,
    },
    /// CSS `radial-gradient()` background fill.
    RadialGradient {
        rect: LayoutRect,
        stops: Vec<CssColorStop>,
        /// The `at <x> <y>` centre; `None` is the box's own centre.
        center: Option<(crate::css::TranslateLength, crate::css::TranslateLength)>,
        /// How far the gradient's last stop is from that centre.
        extent: crate::css::RadialExtent,
        radius: CornerRadii,
        /// `filter: blur()` radius in pixels, 0 for none.
        blur: f32,
    },
    /// Push a clip region onto the clip stack.
    /// All subsequent commands are clipped to `rect` (optionally with rounded corners
    /// when `radius` > 0). Paired with `PopClip`.
    PushClip { rect: LayoutRect, radius: CornerRadii },
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
    /// `mask-image` — the layer's own pixels are faded by a gradient once it is
    /// finished, which needs a surface of its own to fade.
    Mask,
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
    /// Whether this layer's element actually establishes a stacking context.
    ///
    /// Every positioned box gets a layer here so paint order can be expressed,
    /// but `position: relative` with `z-index: auto` is *not* a stacking
    /// context: a `z-index: -1` child of it belongs to an ancestor's negative
    /// list and paints below this box's own background. Painting it as an
    /// ordinary negative child instead put github's hero glow on top of the
    /// panel it is supposed to sit behind.
    pub is_stacking_context: bool,
    /// The clip an `overflow: hidden` ancestor puts on this layer, in page
    /// coordinates, when the layer paints outside its own bounds.
    ///
    /// A clip is otherwise expressed as PushClip/PopClip inside one layer's own
    /// command list, which a descendant layer never sees. That only matters for
    /// a layer whose paint reaches past its box — a `filter: blur()` halo —
    /// since anything drawn inside the box is inside the clip anyway. github's
    /// hero glow is blurred by 42px, and the halo escaped the intro section's
    /// clip and washed over the whole band below it.
    pub clip: Option<LayoutRect>,
    /// `mask-image`, when it names a gradient — the shape a page reaches for to
    /// fade a decorative wash in or out. Applied to the finished layer as an
    /// alpha multiply over its own box.
    pub mask: Option<crate::css::GradientValue>,
    /// `mix-blend-mode`, as the name CSS gives it. `None` is the default
    /// `normal`, which is ordinary source-over compositing.
    pub blend_mode: Option<String>,
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
            is_stacking_context: true,
            clip: None,
            mask: None,
            blend_mode: None,
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

    /// The bottom edge of the document's scrollable overflow region, in page
    /// coordinates.
    ///
    /// A page is as tall as the content it can be scrolled to, which CSS
    /// defines as the union of every box's border box — carried through its
    /// ancestors' transforms and cut back by whatever they clip — not merely
    /// where the in-flow cursor stopped. An out-of-flow box below the last
    /// block, or a rotated one whose corner swings past its own edge, makes a
    /// browser's page that much taller; this engine ended its raster at the
    /// flow's end and cut both away.
    ///
    /// A `filter: blur()` halo is deliberately *not* counted. That is ink
    /// overflow: a browser paints it, but it does not lengthen the page.
    pub fn scrollable_overflow_bottom(layout: &LayoutBox) -> f32 {
        /// The lowest point a rect reaches once `xf` has moved it — the corners
        /// are mapped one by one because a rotation puts the lowest one
        /// anywhere.
        fn transformed_bottom(xf: &tiny_skia::Transform, r: LayoutRect) -> f32 {
            let mut pts = [
                tiny_skia::Point::from_xy(r.x, r.y),
                tiny_skia::Point::from_xy(r.x + r.width, r.y),
                tiny_skia::Point::from_xy(r.x, r.y + r.height),
                tiny_skia::Point::from_xy(r.x + r.width, r.y + r.height),
            ];
            xf.map_points(&mut pts);
            pts.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max)
        }

        struct Frame<'f> {
            layout: &'f LayoutBox<'f>,
            /// The ancestors' transforms, composed, in page space.
            xf: tiny_skia::Transform,
            /// The lowest edge the clipping ancestors still allow.
            clip_bottom: f32,
        }

        let mut bottom = 0.0_f32;
        let mut stack = vec![Frame {
            layout,
            xf: tiny_skia::Transform::identity(),
            clip_bottom: f32::INFINITY,
        }];

        while let Some(frame) = stack.pop() {
            let b = frame.layout;
            // A fixed box stays where it is however far the page scrolls, so it
            // never lengthens what there is to scroll through.
            if b.position == PositionType::Fixed || Self::is_clipped_away(b) {
                continue;
            }

            let d = b.paint_rect();

            let mut child_xf = frame.xf;
            if let Some(Value::Transform(ops)) = b
                .style_node
                .specified_values
                .get(&crate::css::intern("transform"))
            {
                // `compute_transform_matrix` states the matrix about the box's
                // own origin — the same one the painter uses — so it is moved
                // into page space before it composes with the ancestors'.
                let m = Self::compute_transform_matrix(ops, b.dimensions.width, b.dimensions.height);
                let local = tiny_skia::Transform::from_translate(b.dimensions.x, b.dimensions.y)
                    .pre_concat(m.to_skia())
                    .pre_translate(-b.dimensions.x, -b.dimensions.y);
                child_xf = frame.xf.pre_concat(local);
            }

            // A box's own transform moves the box as well as its descendants,
            // so its own edge is measured through `child_xf` too.
            let own = transformed_bottom(&child_xf, d).min(frame.clip_bottom);
            if own.is_finite() {
                bottom = bottom.max(own);
            }

            let mut child_clip = frame.clip_bottom;
            if Self::has_overflow_hidden(b) {
                // Overflow clips to the padding box, and the clip travels with
                // the box, so it is the *transformed* edge that bounds the
                // descendants.
                let pad_box = LayoutRect {
                    x: d.x + b.border.left,
                    y: d.y + b.border.top,
                    width: (d.width - b.border.left - b.border.right).max(0.0),
                    height: (d.height - b.border.top - b.border.bottom).max(0.0),
                };
                child_clip = child_clip.min(transformed_bottom(&child_xf, pad_box));
            }

            for child in &b.children {
                stack.push(Frame {
                    layout: child,
                    xf: child_xf,
                    clip_bottom: child_clip,
                });
            }
        }

        bottom
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

                    // A subtree clipped to nothing paints nothing. The
                    // `clip: rect(0, 0, 0, 0)` + `position: absolute` pair is how
                    // screen-reader-only text is kept out of the visual page, and
                    // sites lean on it heavily for skip links and icon labels.
                    if Self::is_clipped_away(frame_layout) {
                        continue;
                    }

                    let overflow_hidden = Self::has_overflow_hidden(frame_layout);

                    // Per CSS spec, the default `overflow: visible` means content (in
                    // particular text) must NOT be clipped to its ancestors' content
                    // boxes, so `next_clip` is passed down untouched through every
                    // ordinary box. Only `overflow: hidden|clip|auto|scroll`
                    // establishes a clipping region, and there the region narrows for
                    // the whole subtree. Backgrounds and borders honour that through
                    // the PushClip/PopClip mask stack in render.rs, but a text run
                    // carries its own clip rect and never consults that stack — so
                    // without narrowing it here, a clipped panel's text paints right
                    // over whatever follows it.
                    let next_clip = if overflow_hidden {
                        Self::intersect_rects(frame_clip, d)
                    } else {
                        frame_clip
                    };

                    // A zero-sized box still has its children visited, even when it
                    // clips. Dropping the subtree would be right if the box were
                    // genuinely zero-sized, but a box this engine sized wrongly is
                    // far more common than the `width: 0; height: 0; overflow:
                    // hidden` idiom, and losing a whole subtree to a sizing bug is
                    // much worse than painting something a browser would clip.
                    // Skip zero-sized boxes but still visit children.
                    if d.width < 0.1 || d.height < 0.1 {
                        // A box that is zero along one axis and real along the
                        // other meant it: `grid-template-rows: 0fr` with an
                        // `overflow: hidden` child is how every modern
                        // disclosure holds its panel shut, and leaving that
                        // panel unclipped paints its whole text over whatever
                        // follows. A box that measured zero on *both* axes is
                        // the sizing bug the rule above guards against, and
                        // still paints.
                        let deliberate = d.width >= 0.1 || d.height >= 0.1;
                        if overflow_hidden && deliberate && !frame_layout.children.is_empty() {
                            let push_cmd = PaintCommand::PushClip {
                                rect: d,
                                radius: CornerRadii::NONE,
                            };
                            tree.layers[frame_layer_id].content_commands.push(push_cmd.clone());
                            for tile in &mut tree.layers[frame_layer_id].tiles {
                                tile.content_commands.push(push_cmd.clone());
                                tile.dirty = true;
                            }
                            stack.push(Frame::PopClip { layer_id: frame_layer_id, is_background: false });
                        }
                        // Push children in reverse order so the first child is processed first.
                        for child in frame_layout.children.iter().rev() {
                            stack.push(Frame::Process { layout: child, layer_id: frame_layer_id, clip: next_clip });
                        }
                        continue;
                    }

                    let border_radius = Self::read_corner_radii(
                        &frame_layout.style_node.specified_values,
                        d.width,
                        d.height,
                    );

                    let (triggers, matrix) = Self::detect_triggers(frame_layout);

                    if !triggers.is_empty() {
                        // This box establishes a new compositing layer.
                        let new_id = tree.layers.len();
                        let opacity = frame_layout.get_opacity();
                        let mut new_layer = Layer::new(new_id, frame_layout.z_index, opacity, d, triggers, matrix);
                        new_layer.mask = Self::read_mask(frame_layout);
                        new_layer.blend_mode = Self::read_blend_mode(frame_layout);
                        new_layer.is_stacking_context = Self::establishes_stacking_context(frame_layout);
                        // A clipping ancestor narrows `frame_clip`; anything
                        // still at the viewport is unclipped. Only a layer whose
                        // paint spreads past its own box needs the mask.
                        let root_clip = tree.layers[0].bounds;
                        let clipped = frame_clip.x > root_clip.x + 0.5
                            || frame_clip.y > root_clip.y + 0.5
                            || frame_clip.x + frame_clip.width < root_clip.x + root_clip.width - 0.5
                            || frame_clip.y + frame_clip.height
                                < root_clip.y + root_clip.height - 0.5;
                        new_layer.clip = (clipped && Self::spreads_past_its_box(frame_layout))
                            .then_some(frame_clip);
                        tree.add_layer(new_layer);

                        // Record parent → child relationship: access parent index first,
                        // then new_id — both are distinct indices so no aliasing.
                        tree.layers[frame_layer_id].child_layer_ids.push(new_id);

                        // Collect this box's paint commands into the new layer as BACKGROUND.
                        if !Self::is_visibility_hidden(frame_layout) {
                            Self::collect_paint_commands(frame_layout, &mut tree.layers[new_id], frame_clip, true);
                        }

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
                        if !Self::is_visibility_hidden(frame_layout) {
                            Self::collect_paint_commands(frame_layout, &mut tree.layers[frame_layer_id], frame_clip, false);
                        }

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

    /// The overlap of two rects, or an empty rect at `a`'s origin when they miss.
    ///
    /// Used to narrow the text clip region at every clipping box, so a run
    /// inherits the intersection of every clip above it rather than only the
    /// nearest one.
    fn intersect_rects(a: LayoutRect, b: LayoutRect) -> LayoutRect {
        let x0 = a.x.max(b.x);
        let y0 = a.y.max(b.y);
        let x1 = (a.x + a.width).min(b.x + b.width);
        let y1 = (a.y + a.height).min(b.y + b.height);
        LayoutRect {
            x: x0,
            y: y0,
            width: (x1 - x0).max(0.0),
            height: (y1 - y0).max(0.0),
        }
    }

    /// Returns `true` if this box has `overflow: hidden` set.
    ///
    /// `overflow-x` / `overflow-y` count too: a box that clips on either axis is
    /// clipped here, since this engine has no separate per-axis clip.
    /// Whether the page sized this box on both axes.
    ///
    /// A browser frames an image it could not fetch only when it did — a box
    /// whose width came from the layout is left blank however tall it was told
    /// to be.
    fn states_both_axes(layout: &LayoutBox) -> bool {
        let sv = &layout.style_node.specified_values;
        ["width", "height"].iter().all(|prop| {
            matches!(
                sv.get(&crate::css::intern(prop)),
                Some(Value::Length(v, _)) if *v > 0.0
            )
        })
    }

    fn has_overflow_hidden(layout: &LayoutBox) -> bool {
        let sv = &layout.style_node.specified_values;
        for prop in ["overflow", "overflow-x", "overflow-y"] {
            if let Some(Value::Keyword(k)) = sv.get(&crate::css::intern(prop)) {
                if matches!(&**k, "hidden" | "clip" | "scroll" | "auto") {
                    return true;
                }
            }
        }
        false
    }

    /// Returns `true` if `clip-path: inset(...)` leaves no area.
    ///
    /// `clip-path: inset(50%)` is the modern replacement for the `clip: rect(0,
    /// 0, 0, 0)` visually-hidden idiom, so the degenerate case is worth
    /// recognising even though general clip-path shapes are not supported.
    fn is_clip_path_empty(layout: &LayoutBox) -> bool {
        let Some(Value::Keyword(k)) = layout.style_node.specified_values.get(&crate::css::intern("clip-path")) else {
            return false;
        };
        let text = k.to_lowercase();

        // `clip-path: rect(top right bottom left)` — the modern spelling of the
        // visually-hidden idiom, and the one large sites have moved to.
        if let Some(args) = text.strip_prefix("rect(").and_then(|r| r.strip_suffix(')')) {
            let edges: Vec<f32> = args
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|p| !p.is_empty())
                .take(4)
                .filter_map(|p| p.trim_end_matches("px").parse::<f32>().ok())
                .collect();
            return matches!(edges.as_slice(), [top, right, bottom, left] if right - left <= 0.0 || bottom - top <= 0.0);
        }

        let Some(args) = text.strip_prefix("inset(").and_then(|r| r.strip_suffix(')')) else {
            return false;
        };
        let percents: Vec<f32> = args
            .split_whitespace()
            .take_while(|p| *p != "round")
            .filter_map(|p| p.strip_suffix('%')?.parse::<f32>().ok())
            .collect();
        // inset() takes 1-4 values in the usual top/right/bottom/left order.
        let (top, right, bottom, left) = match percents.as_slice() {
            [all] => (*all, *all, *all, *all),
            [tb, lr] => (*tb, *lr, *tb, *lr),
            [t, lr, b] => (*t, *lr, *b, *lr),
            [t, r, b, l] => (*t, *r, *b, *l),
            _ => return false,
        };
        top + bottom >= 100.0 || left + right >= 100.0
    }

    /// Returns `true` if `visibility: hidden` hides this box's own painting.
    ///
    /// The box keeps its layout position and its children are still visited,
    /// because `visibility` is inherited and a descendant can set it back to
    /// `visible`.
    fn is_visibility_hidden(layout: &LayoutBox) -> bool {
        matches!(
            layout.style_node.specified_values.get(&crate::css::intern("visibility")),
            Some(Value::Keyword(k)) if matches!(&**k, "hidden" | "collapse")
        )
    }

    /// Returns `true` if `clip` reduces this box to an empty region.
    ///
    /// Only the degenerate case is handled, because that is the one that carries
    /// meaning on real pages: `clip: rect(0, 0, 0, 0)` is the visually-hidden
    /// idiom. A non-empty `clip` rect is rare and is left unclipped rather than
    /// guessed at.
    fn is_clipped_away(layout: &LayoutBox) -> bool {
        if Self::is_clip_path_empty(layout) {
            return true;
        }
        let Some(Value::Keyword(k)) = layout.style_node.specified_values.get(&crate::css::intern("clip")) else {
            return false;
        };
        let text = k.to_lowercase();
        let Some(args) = text.strip_prefix("rect(").and_then(|r| r.strip_suffix(')')) else {
            return false;
        };
        let nums: Vec<f32> = args
            .split(|c| c == ',' || c == ' ')
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.trim_end_matches("px").parse::<f32>().ok())
            .collect();
        // rect(top, right, bottom, left): empty when the edges cross or meet.
        matches!(nums.as_slice(), [top, right, bottom, left] if right - left <= 0.0 || bottom - top <= 0.0)
    }

    /// Whether this box's paint reaches outside its own border box, which is
    /// what makes an ancestor's clip visible on it. Today that means a blur.
    fn spreads_past_its_box(layout: &LayoutBox) -> bool {
        matches!(
            layout
                .style_node
                .specified_values
                .get(&crate::css::intern("filter-blur")),
            Some(Value::Length(v, crate::css::Unit::Px)) if *v > 0.0
        ) || matches!(
            layout
                .style_node
                .specified_values
                .get(&crate::css::intern("filter-blur")),
            Some(Value::Number(v)) if *v > 0.0
        )
    }

    /// Whether this box establishes a stacking context, as opposed to merely
    /// getting a layer so its paint order can be expressed.
    ///
    /// The distinction only matters for a negative-`z-index` child: it belongs
    /// to the nearest *stacking context* above it, so under a box that is only
    /// positioned it paints below that box's own background.
    fn establishes_stacking_context(layout: &LayoutBox) -> bool {
        let sv = &layout.style_node.specified_values;

        // A positioned box is a stacking context only with a numeric z-index;
        // `fixed` and `sticky` always are.
        if matches!(layout.position, PositionType::Fixed | PositionType::Sticky) {
            return true;
        }
        let stated_z = matches!(sv.get(&crate::css::intern("z-index")), Some(Value::Number(_)));
        if stated_z && !matches!(layout.position, PositionType::Static) {
            return true;
        }
        if layout.get_opacity() < 1.0 {
            return true;
        }
        if sv.contains_key(&crate::css::intern("transform"))
            || sv.contains_key(&crate::css::intern("filter"))
            || sv.contains_key(&crate::css::intern("filter-blur"))
            || sv.contains_key(&crate::css::intern("mask-image"))
            || sv.contains_key(&crate::css::intern("will-change"))
            || sv.contains_key(&crate::css::intern("perspective"))
        {
            return true;
        }
        if let Some(Value::Keyword(k)) = sv.get(&crate::css::intern("mix-blend-mode")) {
            if &**k != "normal" {
                return true;
            }
        }
        if let Some(Value::Keyword(k)) = sv.get(&crate::css::intern("isolation")) {
            if &**k == "isolate" {
                return true;
            }
        }
        if let Some(Value::Keyword(k)) = sv.get(&crate::css::intern("contain")) {
            if k.split_whitespace().any(|w| matches!(w, "paint" | "strict" | "content")) {
                return true;
            }
        }
        // A grid or flex item with a numeric z-index is one too, even unpositioned.
        stated_z
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

        if Self::read_mask(layout).is_some() || Self::read_blend_mode(layout).is_some() {
            triggers.push(CompositingTrigger::Mask);
        }

        (triggers, matrix)
    }

    /// The gradient a box's `mask-image` names, if any.
    ///
    /// Only the gradient forms are read: an image mask needs the bytes decoded,
    /// while a gradient is the shape pages actually use to fade a wash out at
    /// its own edge — github's hero carousel hides the top of its background
    /// gradients this way, and without the mask they ran the full height of the
    /// section.
    fn read_mask(layout: &LayoutBox) -> Option<crate::css::GradientValue> {
        let sv = &layout.style_node.specified_values;
        for prop in ["mask-image", "-webkit-mask-image"] {
            if let Some(Value::Gradient(g)) = sv.get(&crate::css::intern(prop)) {
                return Some(g.clone());
            }
        }
        None
    }

    /// The `mix-blend-mode` a box asks for, other than the default `normal`.
    ///
    /// A decorative wash is very often set to add rather than cover —
    /// `plus-lighter` over a dark section is how a page makes a glow read as
    /// light rather than as paint — and compositing it normally leaves it about
    /// half as bright as the page intends.
    fn read_blend_mode(layout: &LayoutBox) -> Option<String> {
        match layout
            .style_node
            .specified_values
            .get(&crate::css::intern("mix-blend-mode"))
        {
            Some(Value::Keyword(k)) if **k != *"normal" => Some(k.to_string()),
            _ => None,
        }
    }

    /// The four corner radii of a box, resolved against its own size.
    ///
    /// Each corner has its own longhand, which the `border-radius` shorthand
    /// expands into. A percentage is a fraction of the box on each axis
    /// separately, which is what makes `border-radius: 50%` an ellipse.
    fn read_corner_radii(sv: &crate::style::PropertyMap, width: f32, height: f32) -> CornerRadii {
        let axis = |v: &Value, extent: f32| -> f32 {
            match v {
                Value::Length(n, crate::css::Unit::Percent) => extent * (n / 100.0),
                Value::Length(n, _) => *n,
                _ => 0.0,
            }
        };
        let corner = |prop: &str| -> (f32, f32) {
            match sv.get(&crate::css::intern(prop)) {
                // `12px 8px` on one corner: the horizontal radius then the
                // vertical one, kept as a pair the parser could not fold.
                Some(Value::Keyword(k)) => {
                    let mut parts = k.split_whitespace();
                    let x = parts.next().and_then(crate::css::parse_length);
                    let y = parts.next().and_then(crate::css::parse_length);
                    match (x, y) {
                        (Some(x), Some(y)) => (axis(&x, width), axis(&y, height)),
                        (Some(x), None) => (axis(&x, width), axis(&x, height)),
                        _ => (0.0, 0.0),
                    }
                }
                Some(v) => (axis(v, width), axis(v, height)),
                None => (0.0, 0.0),
            }
        };
        CornerRadii {
            top_left: corner("border-top-left-radius"),
            top_right: corner("border-top-right-radius"),
            bottom_right: corner("border-bottom-right-radius"),
            bottom_left: corner("border-bottom-left-radius"),
        }
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
        // `transform-origin` defaults to the box's centre, so the whole list is
        // applied about that point. Applying it about the top-left corner
        // instead swings a rotated box out of its own place — the further from
        // the corner, the further out — and a wash a page rotates ended up in
        // a different part of the section from the one it was written for.
        let cx = elem_width / 2.0;
        let cy = elem_height / 2.0;
        Matrix4x4::translate(cx, cy, 0.0)
            .multiply(&result)
            .multiply(&Matrix4x4::translate(-cx, -cy, 0.0))
    }

    /// Emit paint commands for a single `LayoutBox` (not its children) into `layer`.
    ///
    /// Covers: box-shadow, background, border, images, and text.
    fn collect_paint_commands(layout: &LayoutBox, layer: &mut Layer, clip: LayoutRect, is_root_of_layer: bool) {
        // The border box, which is what a background and a border cover.
        let d = layout.paint_rect();
        let sv = &layout.style_node.specified_values;

        // A percentage radius is a fraction of the box, not a pixel count:
        // `border-radius: 50%` on a wide box is a pill, and reading the 50 as
        // pixels drew a barely-rounded rectangle where the page wanted a soft
        // blob. (CSS makes each corner an ellipse — a percentage of the width
        // horizontally and of the height vertically.)
        let radius = Self::read_corner_radii(sv, d.width, d.height);

        let mut commands = Vec::new();

        // Box shadow (outer only)
        if let Some(Value::BoxShadow(shadow)) = sv.get(&crate::css::intern("box-shadow")) {
            if !shadow.inset {
                commands.push(PaintCommand::Shadow(d, shadow.clone()));
            }
        }

        // `filter: blur()` is applied to this element's own background fill.
        // The decorative washes a design system builds out of a gradient under
        // a heavy blur are empty boxes, so blurring the fill is the whole
        // effect; a filtered element with content of its own keeps that content
        // sharp, which is noted rather than modelled.
        let blur = match sv.get(&crate::css::intern("filter-blur")) {
            Some(Value::Length(v, crate::css::Unit::Px)) => v.max(0.0),
            Some(Value::Number(v)) => v.max(0.0),
            _ => 0.0,
        };

        // Background
        let bg = sv.get(&crate::css::intern("background-color"))
            .or_else(|| sv.get(&crate::css::intern("background")))
            .or_else(|| sv.get(&crate::css::intern("background-image")));
        match bg {
            Some(Value::Color(c)) if c.a > 0 => {
                commands.push(PaintCommand::Rect(d, c.clone(), radius, blur));
            }
            Some(Value::Gradient(GradientValue::Linear { direction, stops })) => {
                commands.push(PaintCommand::LinearGradient {
                    rect: d,
                    direction: direction.clone(),
                    stops: stops.clone(),
                    radius,
                    blur,
                });
            }
            Some(Value::Gradient(GradientValue::Radial { stops, center, extent, .. })) => {
                commands.push(PaintCommand::RadialGradient {
                    rect: d,
                    stops: stops.clone(),
                    center: *center,
                    extent: *extent,
                    radius,
                    blur,
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
                        blur,
                    });
                }
                Some(Value::Gradient(GradientValue::Radial { stops, center, extent, .. })) => {
                    commands.push(PaintCommand::RadialGradient {
                        rect: d,
                        stops: stops.clone(),
                        center: *center,
                        extent: *extent,
                        radius,
                        blur,
                    });
                }
                _ => {}
            }
        }

        // Border
        //
        // A uniform border on a rounded box is stroked as one rounded path so the
        // corners stay round. Every other case is painted edge by edge: a box may
        // carry a rule on one side only, or a different colour per side, and a
        // single stroke can express neither.
        {
            // An unset border colour is `currentColor`, not a grey of this
            // renderer's choosing — the rounded path below was drawing a light
            // grey ring where the page asked for one in its own text colour.
            let uniform_color = || match sv.get(&crate::css::intern("border-color")) {
                Some(Value::Color(c)) => c.clone(),
                _ => match sv.get(&crate::css::intern("color")) {
                    Some(Value::Color(c)) => c.clone(),
                    _ => Color { r: 0, g: 0, b: 0, a: 255 },
                },
            };
            let side_color = |side: &str| match sv.get(&crate::css::intern(&format!("border-{side}-color"))) {
                Some(Value::Color(c)) => c.clone(),
                // An edge with no colour of its own takes the element's text
                // colour, as `border-color`'s initial value `currentColor` says.
                _ => match sv.get(&crate::css::intern("border-color")) {
                    Some(Value::Color(c)) => c.clone(),
                    _ => match sv.get(&crate::css::intern("color")) {
                        Some(Value::Color(c)) => c.clone(),
                        _ => Color { r: 0, g: 0, b: 0, a: 255 },
                    },
                },
            };
            let b = &layout.border;
            let uniform = b.top == b.right && b.right == b.bottom && b.bottom == b.left;

            if uniform && b.top > 0.0 && radius.is_rounded() {
                commands.push(PaintCommand::Border(d, b.top, uniform_color(), radius));
            } else {
                // Edges are drawn as filled rectangles that meet at the corners.
                // Mitring is skipped: at the 1-2px widths pages actually use, the
                // overlap is a single corner pixel.
                // Snap each edge to the pixel grid. A hairline at a fractional
                // offset otherwise spreads its coverage over two rows and comes
                // out as a pale smear instead of the colour the page asked for —
                // and a design built on hairlines is then wrong everywhere.
                let snap = |rect: crate::layout::Rect| crate::layout::Rect {
                    x: rect.x.round(),
                    y: rect.y.round(),
                    width: rect.width.round().max(if rect.width > 0.0 { 1.0 } else { 0.0 }),
                    height: rect.height.round().max(if rect.height > 0.0 { 1.0 } else { 0.0 }),
                };
                let edges: [(f32, crate::layout::Rect, &str); 4] = [
                    (b.top, crate::layout::Rect { x: d.x, y: d.y, width: d.width, height: b.top }, "top"),
                    (b.right, crate::layout::Rect { x: d.x + d.width - b.right, y: d.y, width: b.right, height: d.height }, "right"),
                    (b.bottom, crate::layout::Rect { x: d.x, y: d.y + d.height - b.bottom, width: d.width, height: b.bottom }, "bottom"),
                    (b.left, crate::layout::Rect { x: d.x, y: d.y, width: b.left, height: d.height }, "left"),
                ];
                for (width, rect, side) in edges {
                    if width > 0.0 && rect.width > 0.0 && rect.height > 0.0 {
                        commands.push(PaintCommand::Rect(snap(rect), side_color(side), CornerRadii::NONE, 0.0));
                    }
                }
            }
        }

        // Outline: drawn just outside the border box, and taking no space in
        // layout — which is what separates it from a border.
        {
            let width = match sv.get(&crate::css::intern("outline-width")) {
                Some(Value::Length(v, _)) => *v,
                Some(Value::Number(v)) => *v,
                _ => 0.0,
            };
            let style_is_none = matches!(
                sv.get(&crate::css::intern("outline-style")),
                Some(Value::Keyword(k)) if matches!(&**k, "none" | "hidden")
            );
            if width > 0.0 && !style_is_none {
                let color = match sv.get(&crate::css::intern("outline-color")) {
                    Some(Value::Color(c)) => c.clone(),
                    _ => match sv.get(&crate::css::intern("color")) {
                        Some(Value::Color(c)) => c.clone(),
                        _ => Color { r: 0, g: 0, b: 0, a: 255 },
                    },
                };
                let offset = match sv.get(&crate::css::intern("outline-offset")) {
                    Some(Value::Length(v, _)) => *v,
                    _ => 0.0,
                };
                let o = offset + width;
                let outer = crate::layout::Rect {
                    x: (d.x - o).round(),
                    y: (d.y - o).round(),
                    width: (d.width + o * 2.0).round(),
                    height: (d.height + o * 2.0).round(),
                };
                let w = width.round().max(1.0);
                for rect in [
                    crate::layout::Rect { x: outer.x, y: outer.y, width: outer.width, height: w },
                    crate::layout::Rect { x: outer.x, y: outer.y + outer.height - w, width: outer.width, height: w },
                    crate::layout::Rect { x: outer.x, y: outer.y, width: w, height: outer.height },
                    crate::layout::Rect { x: outer.x + outer.width - w, y: outer.y, width: w, height: outer.height },
                ] {
                    if rect.width > 0.0 && rect.height > 0.0 {
                        commands.push(PaintCommand::Rect(rect, color.clone(), CornerRadii::NONE, 0.0));
                    }
                }
            }
        }

        // Inline SVG: hand the subtree to the rasteriser as its own document.
        if let markup5ever_rcdom::NodeData::Element { ref name, .. } = layout.style_node.node.data {
            if name.local.as_ref() == "svg" && d.width >= 1.0 && d.height >= 1.0 {
                let current_color = match sv.get(&crate::css::intern("color")) {
                    Some(Value::Color(c)) => c.clone(),
                    _ => Color { r: 0, g: 0, b: 0, a: 255 },
                };
                commands.push(PaintCommand::Svg {
                    rect: d,
                    source: crate::js::serialize_outer_html(&layout.style_node.node),
                    current_color,
                });
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
                    _ => ObjectFit::Fill,
                };
                let alt = layout.alt_text.clone().unwrap_or_default();
                let alt_color = match sv.get(&crate::css::intern("color")) {
                    Some(Value::Color(c)) => c.clone(),
                    _ => Color { r: 0, g: 0, b: 0, a: 255 },
                };
                let alt_font_size = match sv.get(&crate::css::intern("font-size")) {
                    Some(Value::Length(v, _)) => *v,
                    _ => 16.0,
                };
                commands.push(PaintCommand::Image {
                    rect: d,
                    url: url.clone(),
                    object_fit,
                    alt,
                    alt_color,
                    alt_font_size,
                    alt_line_height: crate::layout::resolved_line_height_px(layout.style_node),
                    framed: Self::states_both_axes(layout),
                });
            }
        }

        // Text
        if let NodeData::Text { ref contents } = layout.style_node.node.data {
            let font_size = match sv.get(&crate::css::intern("font-size")) {
                Some(Value::Length(v, _)) => *v,
                // Zero is the only unitless font-size CSS accepts; any other bare
                // number is an unparsed value, not a pixel size.
                Some(Value::Number(v)) if *v == 0.0 => 0.0,
                _ => 16.0,
            };
            let color = match sv.get(&crate::css::intern("color")) {
                Some(Value::Color(c)) => c.clone(),
                _ => Color { r: 0, g: 0, b: 0, a: 255 },
            };
            // Read the face and spacing from the same helpers layout measured
            // with, so paint cannot drift from the widths that decided the
            // line breaks.
            let font_style = crate::layout::resolved_font_style(layout.style_node);
            let letter_spacing = crate::layout::resolved_letter_spacing_px(layout.style_node);
            let text_decoration: u8 = match sv.get(&crate::css::intern("text-decoration")) {
                Some(Value::Keyword(k)) => match k.as_ref() {
                    "underline"    => 0b001,
                    "line-through" => 0b010,
                    "overline"     => 0b100,
                    _              => 0,
                },
                _ => 0,
            };
            // `font-size: 0` is a hiding idiom (and the way inline-block gaps are
            // collapsed); there is no glyph to draw at zero pixels.
            if font_size >= 0.5 {
                commands.push(PaintCommand::Text {
                    rect: d,
                    text: crate::layout::apply_text_transform(&contents.borrow(), sv),
                    font_size,
                    line_height: crate::layout::resolved_line_height_px(layout.style_node),
                    leading_space: layout.text_leading,
                    color,
                    clip,
                    style: font_style,
                    letter_spacing,
                    text_decoration,
                    preserve_newlines: crate::layout::preserves_newlines(
                        crate::layout::resolved_white_space(layout.style_node),
                    ),
                    // `text-align` inherits, so the run carries the alignment of
                    // whatever block it ended up in.
                    text_align: match sv.get(&crate::css::intern("text-align")) {
                        Some(Value::Keyword(k)) => TextAlign::from_keyword(k),
                        _ => TextAlign::Start,
                    },
                    wrap_style: crate::layout::resolved_wrap_style(layout.style_node),
                    wrap_width: layout.wrap_width,
                });
            }
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
                let marker_rect = crate::layout::Rect {
                    x: marker_x,
                    y: d.y,
                    width: 20.0,
                    height: font_size,
                };
                commands.push(PaintCommand::Text {
                    rect: marker_rect,
                    text: marker.clone(),
                    font_size,
                    line_height: crate::layout::resolved_line_height_px(layout.style_node),
                    leading_space: 0.0,
                    color,
                    clip,
                    style: crate::font::FontStyle::regular(),
                    letter_spacing: 0.0,
                    text_decoration: 0,
                    preserve_newlines: false,
                    // A marker, a control's label, its value and its
                    // placeholder are each one line in a box this places, so
                    // there is nothing to settle them against and nothing to
                    // re-break.
                    text_align: TextAlign::Start,
                    wrap_style: crate::layout::WrapStyle::Auto,
                    wrap_width: f32::INFINITY,
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
                    line_height: crate::layout::resolved_line_height_px(layout.style_node),
                    leading_space: 0.0,
                    color,
                    clip: d, // clip to the button bounds
                    style: crate::font::FontStyle::regular(),
                    letter_spacing: 0.0,
                    text_decoration: 0,
                    preserve_newlines: false,
                    // A marker, a control's label, its value and its
                    // placeholder are each one line in a box this places, so
                    // there is nothing to settle them against and nothing to
                    // re-break.
                    text_align: TextAlign::Start,
                    wrap_style: crate::layout::WrapStyle::Auto,
                    wrap_width: f32::INFINITY,
                });
            }
        }

        // A field with something in it draws that, in the field's own colour and
        // on the same line the placeholder would sit on.
        if let Some(ref value) = layout.input_value {
            let font_size = match sv.get(&crate::css::intern("font-size")) {
                Some(Value::Length(v, _)) => *v,
                _ => 13.0,
            };
            let content_top = d.y + layout.padding.top + layout.border.top;
            let content_h = (d.height
                - layout.padding.top
                - layout.padding.bottom
                - layout.border.top
                - layout.border.bottom)
                .max(font_size);
            commands.push(PaintCommand::Text {
                rect: crate::layout::Rect {
                    x: d.x + layout.padding.left + layout.border.left,
                    y: content_top,
                    width: (d.width - layout.padding.left - layout.padding.right).max(0.0),
                    height: content_h,
                },
                text: value.clone(),
                font_size,
                line_height: content_h,
                leading_space: 0.0,
                color: match sv.get(&crate::css::intern("color")) {
                    Some(Value::Color(c)) => c.clone(),
                    _ => Color { r: 0, g: 0, b: 0, a: 255 },
                },
                clip: d,
                style: crate::layout::resolved_font_style(layout.style_node),
                letter_spacing: crate::layout::resolved_letter_spacing_px(layout.style_node),
                text_decoration: 0,
                preserve_newlines: false,
                text_align: TextAlign::Start,
                wrap_style: crate::layout::WrapStyle::Auto,
                wrap_width: f32::INFINITY,
            });
        }

        // An empty text field shows its placeholder, muted, the way a browser
        // draws it. A field left blank where a page put prompt text reads as a
        // broken control.
        if let Some(ref text) = layout.input_placeholder {
            // `::placeholder` declarations ride on the field itself under a
            // prefix — the text has no element of its own to hang them on.
            let ph = |prop: &str| {
                sv.get(&crate::css::intern(&format!(
                    "{}{prop}",
                    crate::style::PLACEHOLDER_PREFIX
                )))
            };
            // A page that replaces the placeholder with its own floating label
            // hides it with `opacity: 0`; drawing it anyway left two strings on
            // top of each other.
            let opacity = match ph("opacity") {
                Some(Value::Number(v)) => *v,
                Some(Value::Length(v, _)) => *v,
                _ => 1.0,
            };
            if !text.is_empty() && opacity > 0.01 {
                let font_size = match ph("font-size").or_else(|| sv.get(&crate::css::intern("font-size"))) {
                    Some(Value::Length(v, _)) => *v,
                    _ => 13.0,
                };
                let mut color = match ph("color") {
                    Some(Value::Color(c)) => c.clone(),
                    _ => Color { r: 117, g: 117, b: 117, a: 255 },
                };
                color.a = (color.a as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
                // A single-line field's content box *is* the line its text sits
                // on, so handing the renderer that box lets its own half-leading
                // split centre the placeholder exactly where the value would go.
                // Placing it from a fixed fraction of the font size put it at
                // the top of a padded field, on top of the floating label a page
                // draws over it.
                let content_top = d.y + layout.padding.top + layout.border.top;
                let content_h = (d.height
                    - layout.padding.top
                    - layout.padding.bottom
                    - layout.border.top
                    - layout.border.bottom)
                    .max(font_size);
                let text_rect = crate::layout::Rect {
                    x: d.x + layout.padding.left + layout.border.left,
                    y: content_top,
                    width: (d.width - layout.padding.left - layout.padding.right).max(0.0),
                    height: content_h,
                };
                commands.push(PaintCommand::Text {
                    rect: text_rect,
                    text: text.clone(),
                    font_size,
                    line_height: content_h,
                    leading_space: 0.0,
                    color,
                    clip: d,
                    // The field's own face and spacing, not the default one: a
                    // placeholder is drawn in the text the field would show.
                    style: crate::layout::resolved_font_style(layout.style_node),
                    letter_spacing: crate::layout::resolved_letter_spacing_px(layout.style_node),
                    text_decoration: 0,
                    preserve_newlines: false,
                    // A marker, a control's label, its value and its
                    // placeholder are each one line in a box this places, so
                    // there is nothing to settle them against and nothing to
                    // re-break.
                    text_align: TextAlign::Start,
                    wrap_style: crate::layout::WrapStyle::Auto,
                    wrap_width: f32::INFINITY,
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
                PaintCommand::Svg { rect, .. } => *rect,
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

    /// Collect every string a layer tree draws.
    fn painted_text(tree: &LayerTree) -> Vec<String> {
        let mut out = Vec::new();
        for layer in &tree.layers {
            for cmd in layer
                .background_commands
                .iter()
                .chain(layer.content_commands.iter())
            {
                if let PaintCommand::Text { text, .. } = cmd {
                    out.push(text.clone());
                }
            }
        }
        out
    }

    /// A page that replaces a field's placeholder with its own floating label
    /// hides the placeholder with `::placeholder { opacity: 0 }`. Drawing it
    /// anyway left github's hero with "you@domain.com" and "Enter your email"
    /// on top of each other.
    #[test]
    fn test_transparent_placeholder_is_not_drawn() {
        let html = r#"<input placeholder="you@domain.com">"#;
        let shown = build_tree_from_html(html, "");
        let hidden = build_tree_from_html(html, "input::placeholder { opacity: 0 }");
        assert!(
            painted_text(&shown).iter().any(|t| t == "you@domain.com"),
            "the placeholder is drawn by default"
        );
        assert!(
            !painted_text(&hidden).iter().any(|t| t == "you@domain.com"),
            "an `opacity: 0` placeholder is not drawn"
        );
    }

    /// A placeholder colour the page states is the one that gets drawn.
    #[test]
    fn test_placeholder_colour_comes_from_the_rule() {
        let tree = build_tree_from_html(
            r#"<input placeholder="hint">"#,
            "input::placeholder { color: #ff0000 }",
        );
        let colour = tree
            .layers
            .iter()
            .flat_map(|l| l.background_commands.iter().chain(l.content_commands.iter()))
            .find_map(|cmd| match cmd {
                PaintCommand::Text { text, color, .. } if text == "hint" => Some(color.clone()),
                _ => None,
            })
            .expect("placeholder text command");
        assert_eq!((colour.r, colour.g, colour.b), (255, 0, 0));
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
            .any(|cmd| matches!(cmd, PaintCommand::PushClip { radius, .. } if radius.is_rounded()));
        assert!(has_rounded_push, "overflow:hidden + border-radius must emit PushClip with radius > 0");
    }

    /// Text inside an `overflow: hidden` box carries its own clip rect and never
    /// consults the mask stack, so the clip has to reach it through `next_clip`.
    #[test]
    fn test_overflow_hidden_narrows_the_text_clip() {
        let tree = build_tree_from_html(
            r#"<div style="width:200px;height:40px;overflow:hidden;">
                <div style="height:400px;">a very tall run of text</div>
            </div>"#,
            "",
        );
        let clips: Vec<LayoutRect> = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .filter_map(|cmd| match cmd {
                PaintCommand::Text { clip, .. } => Some(*clip),
                _ => None,
            })
            .collect();
        assert!(!clips.is_empty(), "expected a text run inside the clipping box");
        for clip in clips {
            assert!(
                clip.height <= 40.0 + 0.01,
                "text clip must be narrowed to the clipping box, got height {}",
                clip.height,
            );
        }
    }

    /// The animatable disclosure: `grid-template-rows: 0fr` collapses the row to
    /// nothing and the `overflow: hidden` child holds the panel shut. The panel's
    /// text must be clipped away entirely, not painted over what follows.
    #[test]
    fn test_zero_height_overflow_hidden_clips_its_panel_away() {
        let tree = build_tree_from_html(
            r#"<div style="display:grid;grid-template-rows:0fr;">
                <div style="overflow:hidden;">
                    <div>panel text that must not paint</div>
                </div>
            </div>"#,
            "",
        );
        let cmds: Vec<&PaintCommand> = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .collect();

        for cmd in &cmds {
            if let PaintCommand::Text { clip, .. } = cmd {
                assert!(
                    clip.height < 0.01,
                    "a panel held at 0fr must clip its text away, got clip height {}",
                    clip.height,
                );
            }
        }

        let push_count = cmds.iter().filter(|c| matches!(c, PaintCommand::PushClip { .. })).count();
        let pop_count = cmds.iter().filter(|c| matches!(c, PaintCommand::PopClip)).count();
        assert!(push_count >= 1, "a zero-height clipping box must still push its clip");
        assert_eq!(push_count, pop_count, "PushClip and PopClip must be balanced");
    }

    /// Nested clipping boxes intersect: the inner run sees the overlap of both,
    /// not just the nearest one.
    #[test]
    fn test_nested_clips_intersect() {
        let tree = build_tree_from_html(
            r#"<div style="width:300px;height:30px;overflow:hidden;">
                <div style="width:300px;height:200px;overflow:hidden;">
                    <div>text under two clips</div>
                </div>
            </div>"#,
            "",
        );
        let clips: Vec<LayoutRect> = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .filter_map(|cmd| match cmd {
                PaintCommand::Text { clip, .. } => Some(*clip),
                _ => None,
            })
            .collect();
        assert!(!clips.is_empty(), "expected a text run inside the nested clips");
        for clip in clips {
            assert!(
                clip.height <= 30.0 + 0.01,
                "the outer 30px clip must survive the inner 200px one, got {}",
                clip.height,
            );
        }
    }

    /// A blurred box paints well outside its own bounds, and an `overflow:
    /// hidden` ancestor clips that halo. A box that composites on its own —
    /// because it is positioned, transformed or blended — never sees the
    /// PushClip in its ancestor's command list, so the clip has to travel with
    /// the layer: github's hero glow is blurred by 42px, scaled and blended,
    /// and its halo escaped the intro section and washed over the band below.
    #[test]
    fn test_a_blurred_layer_carries_its_ancestor_s_clip() {
        let tree = build_tree_from_html(
            r#"<div style="width:400px;height:200px;overflow:hidden">
                 <div style="position:relative;width:400px;height:200px;filter:blur(40px);background-color:#9a7cff"></div>
               </div>"#,
            "",
        );
        let blurred = tree.layers.iter()
            .find(|l| l.bounds.width == 400.0 && l.bounds.height == 200.0 && l.clip.is_some())
            .expect("the blurred layer must carry the clip");
        let clip = blurred.clip.expect("clip");
        assert!(
            clip.height <= 200.0 + 0.5,
            "the clip is the ancestor's box, got height {}",
            clip.height
        );
    }

    /// A layer that paints only inside its own box needs no mask: whatever it
    /// draws is inside the clip already, and building one per layer costs.
    #[test]
    fn test_an_unblurred_layer_carries_no_clip() {
        let tree = build_tree_from_html(
            r#"<div style="width:400px;height:200px;overflow:hidden">
                 <div style="position:relative;width:400px;height:200px;opacity:0.5;background-color:#9a7cff"></div>
               </div>"#,
            "",
        );
        assert!(
            tree.layers.iter().all(|l| l.clip.is_none()),
            "only a layer whose paint spreads past its box needs the mask",
        );
    }

    /// `position: relative` with `z-index: auto` gets a layer here so paint
    /// order can be expressed, but it is *not* a stacking context: a negative
    /// `z-index` child of it belongs further up and paints below this box's own
    /// background. Treating it as one put github's hero glow on top of the
    /// panel the page has it sitting behind.
    #[test]
    fn test_a_merely_positioned_box_is_not_a_stacking_context() {
        let tree = build_tree_from_html(
            r#"<div id="host" style="position:relative;width:200px;height:100px;background-color:#151a22">
                 <div style="position:absolute;inset:0;z-index:-1;background-color:#9a7cff"></div>
               </div>"#,
            "",
        );
        let host = tree.layers.iter()
            .find(|l| l.bounds.width == 200.0 && l.bounds.height == 100.0)
            .expect("the positioned box's layer");
        assert!(!host.is_stacking_context, "z-index: auto does not make a stacking context");
    }

    /// A numeric `z-index` on a positioned box does make one, and then a
    /// negative child paints above that box's own background.
    #[test]
    fn test_a_numeric_z_index_makes_a_stacking_context() {
        let tree = build_tree_from_html(
            r#"<div id="host" style="position:relative;z-index:0;width:200px;height:100px;background-color:#151a22">
                 <div style="position:absolute;inset:0;z-index:-1;background-color:#9a7cff"></div>
               </div>"#,
            "",
        );
        let host = tree.layers.iter()
            .find(|l| l.bounds.width == 200.0 && l.bounds.height == 100.0)
            .expect("the positioned box's layer");
        assert!(host.is_stacking_context, "a stated z-index makes a stacking context");
    }

    /// So do the properties that isolate a subtree, whatever the position is.
    #[test]
    fn test_isolating_properties_make_a_stacking_context() {
        for style in [
            "opacity:0.5",
            "transform:translateX(1px)",
            "filter:blur(2px)",
            "mix-blend-mode:multiply",
            "isolation:isolate",
            "contain:paint",
        ] {
            let html = format!(
                r#"<div style="position:relative;width:200px;height:100px;{style}">
                     <div style="position:absolute;inset:0;z-index:-1;background-color:#9a7cff"></div>
                   </div>"#
            );
            let tree = build_tree_from_html(&html, "");
            let host = tree.layers.iter()
                .find(|l| l.bounds.width == 200.0 && l.bounds.height == 100.0)
                .unwrap_or_else(|| panic!("no layer for {style}"));
            assert!(host.is_stacking_context, "{style} must establish a stacking context");
        }
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
    fn test_a_field_paints_the_value_it_holds() {
        // The raster is what a screenshot and every headless render show, so a
        // field's value has to be painted into it. A GUI that overlays a real
        // text widget draws its own background over this, so the two cannot
        // double up.
        let tree = build_tree_from_html(
            r#"<input type="text" value="user input" style="width:200px;height:30px;">"#,
            "",
        );
        let found = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Text { text, .. } if text == "user input"));
        assert!(found, "a field with a value must paint it");
    }

    /// A field showing its value does not also show its placeholder.
    #[test]
    fn test_a_filled_field_does_not_also_paint_its_placeholder() {
        let tree = build_tree_from_html(
            r#"<input type="text" value="typed" placeholder="hint" style="width:200px;height:30px;">"#,
            "",
        );
        let texts: Vec<&str> = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .filter_map(|cmd| match cmd {
                PaintCommand::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"typed"));
        assert!(!texts.contains(&"hint"), "the placeholder is hidden once something is typed");
    }

    /// A hidden field paints nothing at all.
    #[test]
    fn test_a_hidden_field_paints_no_value() {
        let tree = build_tree_from_html(
            r#"<input type="hidden" name="source" value="form-home-signup">"#,
            "",
        );
        let found = tree.layers.iter()
            .flat_map(|l| l.content_commands.iter().chain(l.background_commands.iter()))
            .any(|cmd| matches!(cmd, PaintCommand::Text { text, .. } if text == "form-home-signup"));
        assert!(!found, "a hidden field has nothing to draw");
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
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r, _) if r.is_rounded()));
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
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r, _) if r.is_rounded()));
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
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r, _) if r.is_rounded()));
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
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r, _) if r.is_rounded()));
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
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r, _) if r.is_rounded()));
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
            .any(|cmd| matches!(cmd, PaintCommand::Rect(_, _, r, _) if r.is_rounded()));
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


#[cfg(test)]
mod scrollable_overflow_tests {
    use super::*;
    use crate::layout::build_layout_tree;
    use std::collections::HashMap;

    /// The bottom of the document's scrollable overflow region for `html`.
    fn overflow_bottom(html: &str) -> f32 {
        let dom = crate::dom::parse_html(html);
        let stylesheet = crate::css::parse_css("");
        let style_tree = crate::style::build_style_tree(
            &dom.document,
            &stylesheet,
            None,
            &HashMap::new(),
            None,
            None,
            None,
        );
        let (layout, _, _) = build_layout_tree(&style_tree, 0.0, 0.0, 0.0, 800.0, 800.0, 600.0);
        let layout = layout.expect("layout tree should be built");
        LayerTreeBuilder::scrollable_overflow_bottom(&layout)
    }

    #[test]
    fn an_out_of_flow_box_below_the_flow_lengthens_the_page() {
        let bottom = overflow_bottom(
            r#"<body style="height: 200px"><div style="position: absolute; top: 300px; width: 50px; height: 50px"></div></body>"#,
        );
        assert!(
            (bottom - 350.0).abs() < 0.5,
            "the page must reach the absolute box's bottom at 350, got {bottom}"
        );
    }

    #[test]
    fn a_rotated_box_lengthens_the_page_by_the_corner_it_swings_out() {
        // 300x120 turned 45 degrees about its own centre at y = 160 reaches
        // (300 + 120) * sin 45 / 2 = 148.49 below it.
        let bottom = overflow_bottom(
            r#"<body style="height: 200px"><div style="position: absolute; top: 100px; left: 100px; width: 300px; height: 120px; transform: rotate(-45deg)"></div></body>"#,
        );
        assert!(
            (bottom - 308.49).abs() < 0.5,
            "the rotated corner must reach 308.49, got {bottom}"
        );
    }

    #[test]
    fn a_page_with_nothing_outside_the_flow_is_as_tall_as_the_flow() {
        let bottom = overflow_bottom(r#"<body style="height: 200px"></body>"#);
        assert!(
            (bottom - 200.0).abs() < 0.5,
            "the page must stop at the flow's end at 200, got {bottom}"
        );
    }

    #[test]
    fn a_clipping_ancestor_holds_the_page_at_its_own_edge() {
        let bottom = overflow_bottom(
            r#"<body style="height: 200px"><div style="position: absolute; top: 0; width: 100px; height: 100px; overflow: hidden"><div style="height: 400px"></div></div></body>"#,
        );
        assert!(
            (bottom - 200.0).abs() < 0.5,
            "the clipped child must not reach past the flow's 200, got {bottom}"
        );
    }

    #[test]
    fn a_fixed_box_never_lengthens_the_page() {
        // A fixed box stays put however far the page scrolls, so there is
        // nothing extra to scroll to.
        let bottom = overflow_bottom(
            r#"<body style="height: 200px"><div style="position: fixed; top: 500px; width: 50px; height: 50px"></div></body>"#,
        );
        assert!(
            (bottom - 200.0).abs() < 0.5,
            "a fixed box must leave the page at 200, got {bottom}"
        );
    }

    #[test]
    fn an_ancestor_s_transform_carries_its_descendants() {
        // The inner box ends at 200 in its parent's own coordinates; the
        // parent's 100px shift puts it at 300.
        let bottom = overflow_bottom(
            r#"<body style="height: 100px"><div style="position: absolute; top: 0; width: 100px; height: 200px; transform: translateY(100px)"><div style="height: 200px"></div></div></body>"#,
        );
        assert!(
            (bottom - 300.0).abs() < 0.5,
            "the shifted descendant must reach 300, got {bottom}"
        );
    }
}
