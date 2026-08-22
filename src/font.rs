//! Font selection and metrics, shared by layout and paint.
//!
//! Layout measures text to decide where lines break; paint draws the glyphs. If
//! those two disagree about which face a character comes from, wrapping stops
//! matching what is drawn, so both go through this module.
//!
//! Faces are bundled rather than discovered from the system, so a page renders
//! the same way wherever this runs. DejaVu Sans is the primary because it is the
//! default `sans-serif` on the Linux systems this renders against, so Latin text
//! picks up the same advance widths a browser would use; its bold and monospace
//! companions are bundled for the same reason. NanumGothic covers the CJK ranges
//! DejaVu has no glyphs for.

use ab_glyph::{Font, FontRef, GlyphId, PxScale};
use std::sync::OnceLock;

const SANS: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
const SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");
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
    Mono,
    MonoBold,
    Fallback,
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
}

impl FontStyle {
    pub const fn regular() -> Self {
        FontStyle { bold: false, italic: false, monospace: false }
    }

    /// The face this style asks for, before any fallback for missing glyphs.
    pub fn face(self) -> FaceId {
        match (self.monospace, self.bold) {
            (true, true) => FaceId::MonoBold,
            (true, false) => FaceId::Mono,
            (false, true) => FaceId::SansBold,
            (false, false) => FaceId::Sans,
        }
    }
}

pub struct FontSet {
    sans: FontRef<'static>,
    sans_bold: FontRef<'static>,
    mono: FontRef<'static>,
    mono_bold: FontRef<'static>,
    fallback: FontRef<'static>,
}

impl FontSet {
    pub fn face(&self, id: FaceId) -> &FontRef<'static> {
        match id {
            FaceId::Sans => &self.sans,
            FaceId::SansBold => &self.sans_bold,
            FaceId::Mono => &self.mono,
            FaceId::MonoBold => &self.mono_bold,
            FaceId::Fallback => &self.fallback,
        }
    }

    /// The face that has a glyph for `c`, and that glyph's id.
    ///
    /// `glyph_id` returns 0 (`.notdef`) for a character a face does not cover,
    /// which is what drives the fallback.
    pub fn glyph(&self, c: char, style: FontStyle) -> (FaceId, GlyphId) {
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
        let face = self.face(style.face());
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
        mono: FontRef::try_from_slice(MONO).expect("bundled mono font should parse"),
        mono_bold: FontRef::try_from_slice(MONO_BOLD).expect("bundled mono bold font should parse"),
        fallback: FontRef::try_from_slice(FALLBACK).expect("bundled fallback font should parse"),
    })
}
