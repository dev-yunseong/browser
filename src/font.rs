//! Font selection and metrics, shared by layout and paint.
//!
//! Layout measures text to decide where lines break; paint draws the glyphs. If
//! those two disagree about which face a character comes from, wrapping stops
//! matching what is drawn, so both go through this module.
//!
//! Two faces are bundled. DejaVu Sans is the primary because it is the default
//! `sans-serif` on the Linux systems this renders against, so Latin text picks
//! up the same advance widths a browser would use. NanumGothic covers the CJK
//! ranges DejaVu has no glyphs for.

use ab_glyph::{Font, FontRef, GlyphId, PxScale};
use std::sync::OnceLock;

const PRIMARY: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
const FALLBACK: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

/// Which bundled face a glyph came from.
///
/// Paint caches rasterised glyphs by id, and ids are only meaningful within one
/// face, so the face has to travel with the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceId {
    Primary,
    Fallback,
}

pub struct FontSet {
    primary: FontRef<'static>,
    fallback: FontRef<'static>,
}

impl FontSet {
    pub fn face(&self, id: FaceId) -> &FontRef<'static> {
        match id {
            FaceId::Primary => &self.primary,
            FaceId::Fallback => &self.fallback,
        }
    }

    /// The face that has a glyph for `c`, and that glyph's id.
    ///
    /// `glyph_id` returns 0 (`.notdef`) for a character a face does not cover,
    /// which is what drives the fallback.
    pub fn glyph(&self, c: char) -> (FaceId, GlyphId) {
        let primary = self.primary.glyph_id(c);
        if primary.0 != 0 {
            return (FaceId::Primary, primary);
        }
        let fallback = self.fallback.glyph_id(c);
        if fallback.0 != 0 {
            return (FaceId::Fallback, fallback);
        }
        (FaceId::Primary, primary)
    }

    /// Advance width of `c` at `font_size`, in pixels.
    pub fn advance(&self, c: char, font_size: f32) -> f32 {
        let (face_id, gid) = self.glyph(c);
        let face = self.face(face_id);
        let units = face.units_per_em().unwrap_or(1000.0);
        face.h_advance_unscaled(gid) * (font_size / units)
    }

    /// Advance width of a whole run, with no wrapping applied.
    pub fn measure(&self, text: &str, font_size: f32) -> f32 {
        text.chars().map(|c| self.advance(c, font_size)).sum()
    }

    /// The height of one line when `line-height: normal`.
    ///
    /// Taken from the primary face's own vertical metrics rather than a fixed
    /// multiplier, so line boxes match what a browser using the same face
    /// computes.
    pub fn normal_line_height(&self, font_size: f32) -> f32 {
        let units = self.primary.units_per_em().unwrap_or(1000.0);
        let height = self.primary.height_unscaled() + self.primary.line_gap_unscaled();
        height * (font_size / units)
    }

    /// Advance width of the "0" glyph — the CSS `ch` unit.
    pub fn zero_advance(&self, font_size: f32) -> f32 {
        self.advance('0', font_size)
    }

    /// The font's x-height — the CSS `ex` unit.
    ///
    /// Measured from the "x" glyph's outline; a face with no such glyph falls
    /// back to the half-em the spec names as the default.
    pub fn x_height(&self, font_size: f32) -> f32 {
        use ab_glyph::ScaleFont as _;
        let scaled = self.primary.as_scaled(PxScale::from(font_size));
        let glyph = self.primary.glyph_id('x');
        match self.primary.outline(glyph) {
            Some(outline) => outline.bounds.height() * (font_size / self.primary.units_per_em().unwrap_or(1000.0)),
            None => {
                let _ = scaled;
                font_size * 0.5
            }
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
        primary: FontRef::try_from_slice(PRIMARY).expect("bundled primary font should parse"),
        fallback: FontRef::try_from_slice(FALLBACK).expect("bundled fallback font should parse"),
    })
}
