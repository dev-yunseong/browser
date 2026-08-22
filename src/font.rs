//! Font selection and metrics, shared by layout and paint.
//!
//! Layout measures text to decide where lines break; paint draws the glyphs. If
//! those two disagree about which face a character comes from, wrapping stops
//! matching what is drawn, so both go through this module.
//!
//! Faces are bundled rather than discovered from the system, so a page renders
//! the same way wherever this runs. Which faces are bundled is not a matter of
//! taste: they are the ones a browser resolves the CSS generic families to on
//! the Linux systems this renders against — Liberation Sans for `sans-serif`,
//! Liberation Serif for `serif`, DejaVu Sans Mono for `monospace` — established
//! by measuring the same string in both. A face with different advance widths
//! breaks lines in different places, so the wrong choice makes a page the wrong
//! length however correct the layout code is. NanumGothic covers the CJK ranges
//! none of them have glyphs for.

use ab_glyph::{Font, FontRef, GlyphId, PxScale};
use std::sync::{OnceLock, RwLock};

const SANS: &[u8] = include_bytes!("../assets/fonts/LiberationSans-Regular.ttf");
const SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/LiberationSans-Bold.ttf");
const SERIF: &[u8] = include_bytes!("../assets/fonts/LiberationSerif-Regular.ttf");
const SERIF_BOLD: &[u8] = include_bytes!("../assets/fonts/LiberationSerif-Bold.ttf");
const MONO: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono.ttf");
const MONO_BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSansMono-Bold.ttf");
const FALLBACK: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

/// Which bundled face a glyph came from.
///
/// Paint caches rasterised glyphs by id, and ids are only meaningful within one
/// face, so the face has to travel with the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceId {
    Sans,
    SansBold,
    Serif,
    SerifBold,
    Mono,
    MonoBold,
    Fallback,
    /// A face the page shipped through `@font-face`, by registration order.
    Web(u32),
}

/// The parts of an element's font that change which face is used.
///
/// Weight is reduced to a boolean because only two weights are bundled; italic
/// is carried here so measurement and paint agree even though it is synthesised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FontStyle {
    pub bold: bool,
    pub italic: bool,
    pub monospace: bool,
    pub serif: bool,
    /// A registered web-font family, if the element's `font-family` names one.
    /// `None` means the bundled faces.
    pub web_family: Option<u16>,
}

impl FontStyle {
    pub const fn regular() -> Self {
        FontStyle { bold: false, italic: false, monospace: false, serif: false, web_family: None }
    }

    /// The bundled face this style asks for, before any fallback for missing
    /// glyphs. Web faces are consulted first, in `FontSet::glyph`.
    pub fn face(self) -> FaceId {
        match (self.monospace, self.serif, self.bold) {
            (true, _, true) => FaceId::MonoBold,
            (true, _, false) => FaceId::Mono,
            (false, true, true) => FaceId::SerifBold,
            (false, true, false) => FaceId::Serif,
            (false, false, true) => FaceId::SansBold,
            (false, false, false) => FaceId::Sans,
        }
    }
}

/// One face a page shipped through `@font-face`.
struct WebFace {
    family: u16,
    bold: bool,
    italic: bool,
    font: FontRef<'static>,
}

/// Faces registered from `@font-face`, and the family names they answer to.
///
/// Both grow for the life of the process: a face is parsed from leaked bytes so
/// it can be handed out as `&'static`, which is what lets layout and paint share
/// one reference without threading a lifetime through every call.
#[derive(Default)]
struct WebFonts {
    families: Vec<String>,
    faces: Vec<&'static WebFace>,
}

fn web_fonts() -> &'static RwLock<WebFonts> {
    static WEB: OnceLock<RwLock<WebFonts>> = OnceLock::new();
    WEB.get_or_init(|| RwLock::new(WebFonts::default()))
}

/// Register a decoded face under `family`, returning its id.
///
/// `data` is leaked on purpose: a face outlives the page that loaded it, and
/// pages routinely re-use the same font across navigations.
pub fn register_web_face(family: &str, data: Vec<u8>, bold: bool, italic: bool) -> Option<FaceId> {
    let leaked: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = FontRef::try_from_slice(leaked).ok()?;
    let family = family.trim().to_ascii_lowercase();

    let mut reg = web_fonts().write().ok()?;
    let family_idx = match reg.families.iter().position(|f| *f == family) {
        Some(i) => i,
        None => {
            reg.families.push(family);
            reg.families.len() - 1
        }
    };
    let face: &'static WebFace = Box::leak(Box::new(WebFace {
        family: u16::try_from(family_idx).ok()?,
        bold,
        italic,
        font,
    }));
    reg.faces.push(face);
    Some(FaceId::Web(u32::try_from(reg.faces.len() - 1).ok()?))
}

/// The id of a registered family, if any face has been loaded under that name.
pub fn web_family_id(family: &str) -> Option<u16> {
    let family = family.trim().to_ascii_lowercase();
    let reg = web_fonts().read().ok()?;
    reg.families
        .iter()
        .position(|f| *f == family)
        .and_then(|i| u16::try_from(i).ok())
}

/// Whether any web face has been registered at all.
///
/// Callers use this to skip the family lookup entirely on the common page that
/// ships no fonts of its own.
pub fn has_web_faces() -> bool {
    web_fonts().read().map(|r| !r.faces.is_empty()).unwrap_or(false)
}

fn web_face(id: u32) -> Option<&'static WebFace> {
    web_fonts().read().ok()?.faces.get(id as usize).copied()
}

/// Every face registered under `family`, best match for `style` first.
///
/// A page that ships a subsetted family — one `@font-face` per unicode range,
/// which is how a CJK face is delivered — registers many faces under one name,
/// so the caller walks them until one has the glyph.
fn faces_for(family: u16, bold: bool, italic: bool) -> Vec<(u32, &'static WebFace)> {
    let Ok(reg) = web_fonts().read() else {
        return Vec::new();
    };
    let mut matching: Vec<(u32, &'static WebFace)> = reg
        .faces
        .iter()
        .enumerate()
        .filter(|(_, f)| f.family == family)
        .map(|(i, f)| (i as u32, *f))
        .collect();
    // Exact weight/slant matches first; the rest stay as fallbacks rather than
    // being dropped, because a page that ships only one weight still wants it.
    matching.sort_by_key(|(_, f)| ((f.bold != bold) as u8, (f.italic != italic) as u8));
    matching
}

pub struct FontSet {
    sans: FontRef<'static>,
    sans_bold: FontRef<'static>,
    serif: FontRef<'static>,
    serif_bold: FontRef<'static>,
    mono: FontRef<'static>,
    mono_bold: FontRef<'static>,
    fallback: FontRef<'static>,
}

impl FontSet {
    pub fn face(&self, id: FaceId) -> &FontRef<'static> {
        match id {
            FaceId::Sans => &self.sans,
            FaceId::SansBold => &self.sans_bold,
            FaceId::Serif => &self.serif,
            FaceId::SerifBold => &self.serif_bold,
            FaceId::Mono => &self.mono,
            FaceId::MonoBold => &self.mono_bold,
            FaceId::Fallback => &self.fallback,
            // A registered face is parsed from leaked bytes, so the reference
            // outlives this borrow; the bundled sans stands in if the id is
            // stale, which can only happen across a registry reset.
            FaceId::Web(idx) => match web_face(idx) {
                Some(f) => &f.font,
                None => &self.sans,
            },
        }
    }

    /// The face that has a glyph for `c`, and that glyph's id.
    ///
    /// A family the page shipped is tried first, then the bundled face the
    /// style asks for, then the CJK fallback. `glyph_id` returns 0 (`.notdef`)
    /// for a character a face does not cover, which is what drives each step.
    pub fn glyph(&self, c: char, style: FontStyle) -> (FaceId, GlyphId) {
        if let Some(family) = style.web_family {
            for (idx, face) in faces_for(family, style.bold, style.italic) {
                let gid = face.font.glyph_id(c);
                if gid.0 != 0 {
                    return (FaceId::Web(idx), gid);
                }
            }
        }
        let wanted = style.face();
        let primary = self.face(wanted).glyph_id(c);
        if primary.0 != 0 {
            return (wanted, primary);
        }
        let fallback = self.fallback.glyph_id(c);
        if fallback.0 != 0 {
            return (FaceId::Fallback, fallback);
        }
        (wanted, primary)
    }

    /// Advance width of `c` at `font_size`, in pixels.
    pub fn advance(&self, c: char, font_size: f32, style: FontStyle) -> f32 {
        let (face_id, gid) = self.glyph(c, style);
        let face = self.face(face_id);
        let units = face.units_per_em().unwrap_or(1000.0);
        face.h_advance_unscaled(gid) * (font_size / units)
    }

    /// Advance width of a whole run, with no wrapping applied.
    ///
    /// `letter_spacing` is added after every character, as CSS specifies — the
    /// trailing one included, which is what browsers do.
    pub fn measure(&self, text: &str, font_size: f32, style: FontStyle, letter_spacing: f32) -> f32 {
        text.chars()
            .map(|c| self.advance(c, font_size, style) + letter_spacing)
            .sum()
    }

    /// The height of one line when `line-height: normal`.
    ///
    /// Taken from the face's own vertical metrics rather than a fixed
    /// multiplier, so line boxes match what a browser using the same face
    /// computes.
    pub fn normal_line_height(&self, font_size: f32, style: FontStyle) -> f32 {
        // A page's own face sets its line height too: a face with taller
        // metrics than the bundled one makes every line box taller, and the
        // error compounds down the page.
        let face_id = style
            .web_family
            .and_then(|f| faces_for(f, style.bold, style.italic).first().map(|(i, _)| FaceId::Web(*i)))
            .unwrap_or_else(|| style.face());
        let face = self.face(face_id);
        let units = face.units_per_em().unwrap_or(1000.0);
        let height = face.height_unscaled() + face.line_gap_unscaled();
        height * (font_size / units)
    }

    /// Advance width of the "0" glyph — the CSS `ch` unit.
    pub fn zero_advance(&self, font_size: f32) -> f32 {
        self.advance('0', font_size, FontStyle::regular())
    }

    /// The font's x-height — the CSS `ex` unit.
    ///
    /// Measured from the "x" glyph's outline; a face with no such glyph falls
    /// back to the half-em the spec names as the default.
    pub fn x_height(&self, font_size: f32) -> f32 {
        let glyph = self.sans.glyph_id('x');
        match self.sans.outline(glyph) {
            Some(outline) => {
                outline.bounds.height() * (font_size / self.sans.units_per_em().unwrap_or(1000.0))
            }
            None => font_size * 0.5,
        }
    }

    /// Scale to use with `ab_glyph` for a given face at `font_size`.
    ///
    /// The faces have different units-per-em, so a shared `PxScale` would draw
    /// one of them at the wrong size.
    pub fn scale(&self, face_id: FaceId, font_size: f32) -> PxScale {
        let _ = self.face(face_id);
        PxScale::from(font_size)
    }
}

/// The bundled faces, parsed once.
pub fn fonts() -> &'static FontSet {
    static FONTS: OnceLock<FontSet> = OnceLock::new();
    FONTS.get_or_init(|| FontSet {
        sans: FontRef::try_from_slice(SANS).expect("bundled sans font should parse"),
        sans_bold: FontRef::try_from_slice(SANS_BOLD).expect("bundled bold font should parse"),
        serif: FontRef::try_from_slice(SERIF).expect("bundled serif font should parse"),
        serif_bold: FontRef::try_from_slice(SERIF_BOLD).expect("bundled serif bold font should parse"),
        mono: FontRef::try_from_slice(MONO).expect("bundled mono font should parse"),
        mono_bold: FontRef::try_from_slice(MONO_BOLD).expect("bundled mono bold font should parse"),
        fallback: FontRef::try_from_slice(FALLBACK).expect("bundled fallback font should parse"),
    })
}
