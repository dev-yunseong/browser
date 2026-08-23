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
//! Liberation Serif for `serif`, DejaVu Sans Mono for `monospace`, DejaVu Sans
//! for `system-ui` — established by measuring the same string in both. They are
//! not interchangeable: DejaVu Sans is a good deal wider than Liberation Sans,
//! so a page whose stack ends in `system-ui` lays out at a different measure
//! from one ending in `sans-serif`. A face with different advance widths
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
const SYSTEM_UI: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
const SYSTEM_UI_BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");
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
    SystemUi,
    SystemUiBold,
    Fallback,
    /// A face found on the system, consulted for a character none of the
    /// bundled ones cover. Indexed by load order, which is path order.
    System(u32),
    /// A face the page shipped through `@font-face`, by registration order.
    Web(u32),
}

/// The bundled family a `font-family` stack resolves to.
///
/// These are the generics a browser can actually satisfy here; a stack that
/// names none of them gets `Sans`, as a browser's default does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GenericFamily {
    #[default]
    Sans,
    Serif,
    Mono,
    /// `system-ui`, `-apple-system` and `BlinkMacSystemFont` — the platform's
    /// own UI face, which on the Linux this renders against is DejaVu Sans and
    /// is markedly wider than the `sans-serif` default.
    SystemUi,
}

/// The parts of an element's font that change which face is used.
///
/// Weight is reduced to a boolean because only two weights are bundled; italic
/// is carried here so measurement and paint agree even though it is synthesised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FontStyle {
    pub bold: bool,
    pub italic: bool,
    pub family: GenericFamily,
    /// A registered web-font family, if the element's `font-family` names one.
    /// `None` means the bundled faces.
    pub web_family: Option<u16>,
}

impl FontStyle {
    pub const fn regular() -> Self {
        FontStyle {
            bold: false,
            italic: false,
            family: GenericFamily::Sans,
            web_family: None,
        }
    }

    /// The bundled face this style asks for, before any fallback for missing
    /// glyphs. Web faces are consulted first, in `FontSet::glyph`.
    pub fn face(self) -> FaceId {
        match (self.family, self.bold) {
            (GenericFamily::Mono, true) => FaceId::MonoBold,
            (GenericFamily::Mono, false) => FaceId::Mono,
            (GenericFamily::Serif, true) => FaceId::SerifBold,
            (GenericFamily::Serif, false) => FaceId::Serif,
            (GenericFamily::SystemUi, true) => FaceId::SystemUiBold,
            (GenericFamily::SystemUi, false) => FaceId::SystemUi,
            (GenericFamily::Sans, true) => FaceId::SansBold,
            (GenericFamily::Sans, false) => FaceId::Sans,
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

/// The faces the system has, in a fixed order, loaded the first time a
/// character none of the bundled faces cover asks for one.
///
/// A browser resolves a family it cannot satisfy through the system's own
/// fonts, and the fallback for an uncovered codepoint is the same search. The
/// bundled NanumGothic stays as the last resort so a machine with no fonts at
/// all still draws Hangul, but where a system font exists this engine now picks
/// the one a browser on that machine picks: Chromium here answers a Hangul
/// codepoint with Unifont at a full-em advance, and NanumGothic's 0.94em made
/// every Korean run 5.5% narrow — enough to keep a line the reference wraps.
///
/// The list is walked in path order, which is what makes the choice
/// reproducible. Loading is deferred because most pages never need it, and
/// capped so a machine with a very large font collection cannot be made to read
/// all of it into memory.
fn system_faces() -> &'static [FontRef<'static>] {
    static SYSTEM: OnceLock<Vec<FontRef<'static>>> = OnceLock::new();
    SYSTEM.get_or_init(|| {
        const ROOTS: [&str; 3] = ["/usr/share/fonts", "/usr/local/share/fonts", "/Library/Fonts"];
        const MAX_BYTES: usize = 96 * 1024 * 1024;

        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        for root in ROOTS {
            collect_font_files(std::path::Path::new(root), &mut paths, 0);
        }
        paths.sort();

        let mut faces = Vec::new();
        let mut budget = MAX_BYTES;
        for path in paths {
            let Ok(data) = std::fs::read(&path) else { continue };
            if data.len() > budget {
                break;
            }
            budget -= data.len();
            let leaked: &'static [u8] = Box::leak(data.into_boxed_slice());
            // A collection holds several faces; each is a separate candidate.
            for index in 0..8u32 {
                match FontRef::try_from_slice_and_index(leaked, index) {
                    Ok(face) => faces.push(face),
                    Err(_) => break,
                }
            }
        }
        faces
    })
}

/// Every font file under `dir`, recursively, bounded in depth so a symlink loop
/// cannot walk forever.
fn collect_font_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_files(&path, out, depth + 1);
            continue;
        }
        let is_font = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf") || e.eq_ignore_ascii_case("ttc"))
            .unwrap_or(false);
        if is_font {
            out.push(path);
        }
    }
}

pub struct FontSet {
    sans: FontRef<'static>,
    sans_bold: FontRef<'static>,
    serif: FontRef<'static>,
    serif_bold: FontRef<'static>,
    mono: FontRef<'static>,
    mono_bold: FontRef<'static>,
    system_ui: FontRef<'static>,
    system_ui_bold: FontRef<'static>,
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
            FaceId::SystemUi => &self.system_ui,
            FaceId::SystemUiBold => &self.system_ui_bold,
            FaceId::Fallback => &self.fallback,
            FaceId::System(idx) => system_faces().get(idx as usize).unwrap_or(&self.fallback),
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
        // ASCII is covered by every bundled face, so a miss there is a missing
        // glyph rather than a missing script and the system search would only
        // cost time. Deferring it this way keeps a Latin page from ever
        // touching the disk.
        if !c.is_ascii() {
            for (idx, face) in system_faces().iter().enumerate() {
                let gid = face.glyph_id(c);
                if gid.0 != 0 {
                    return (FaceId::System(idx as u32), gid);
                }
            }
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

    /// The kerning between two adjacent glyphs, in pixels.
    ///
    /// A pair only kerns within one face, so a run that falls back mid-word
    /// gets none across the seam — which is what a shaper does too. Leaving
    /// kerning out entirely made a DejaVu Sans line about a pixel wider than
    /// the browser draws it, and a wrap decision that came down to 1.4px then
    /// went the other way.
    pub fn kern(
        &self,
        prev: Option<(FaceId, GlyphId)>,
        current: (FaceId, GlyphId),
        font_size: f32,
    ) -> f32 {
        let Some((prev_face, prev_gid)) = prev else {
            return 0.0;
        };
        if prev_face != current.0 {
            return 0.0;
        }
        let face = self.face(current.0);
        let units = face.units_per_em().unwrap_or(1000.0);
        face.kern_unscaled(prev_gid, current.1) * (font_size / units)
    }

    /// Advance width of a whole run, with no wrapping applied.
    ///
    /// `letter_spacing` is added after every character, as CSS specifies — the
    /// trailing one included, which is what browsers do.
    pub fn measure(&self, text: &str, font_size: f32, style: FontStyle, letter_spacing: f32) -> f32 {
        let mut total = 0.0;
        let mut prev: Option<(FaceId, GlyphId)> = None;
        for c in text.chars() {
            let (face_id, gid) = self.glyph(c, style);
            total += self.kern(prev, (face_id, gid), font_size);
            let face = self.face(face_id);
            let units = face.units_per_em().unwrap_or(1000.0);
            total += face.h_advance_unscaled(gid) * (font_size / units) + letter_spacing;
            prev = Some((face_id, gid));
        }
        total
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

    /// How much of a line box sits below the baseline, for text of `font_size`
    /// in `style` on a line of `line_height`.
    ///
    /// A line box is the font's ascent and descent plus the leading split
    /// evenly above and below, so this is the room a line always keeps under
    /// the baseline — whatever else is sitting on it.
    pub fn below_baseline(&self, font_size: f32, style: FontStyle, line_height: f32) -> f32 {
        let face_id = style
            .web_family
            .and_then(|f| faces_for(f, style.bold, style.italic).first().map(|(i, _)| FaceId::Web(*i)))
            .unwrap_or_else(|| style.face());
        let face = self.face(face_id);
        let units = face.units_per_em().unwrap_or(1000.0);
        let scale = font_size / units;
        let ascent = face.ascent_unscaled() * scale;
        // `descent_unscaled` is measured downwards from the baseline and so is
        // negative; the half-leading below is what is left of the line once the
        // font's own content area is taken out of it.
        let descent = -face.descent_unscaled() * scale;
        (descent + (line_height - ascent - descent) / 2.0).max(0.0)
    }

    /// Advance width of the "0" glyph — the CSS `ch` unit.
    ///
    /// Both `ch` and `ex` are metrics of *the element's own* font, not of the
    /// default one: a design that caps its prose at `46ch` gets a wider measure
    /// under `system-ui` than under `sans-serif`, and measuring both against
    /// the sans face wrapped one of them a line early.
    pub fn zero_advance(&self, font_size: f32, style: FontStyle) -> f32 {
        self.advance('0', font_size, style)
    }

    /// The font's x-height — the CSS `ex` unit.
    ///
    /// Measured from the "x" glyph's outline; a face with no such glyph falls
    /// back to the half-em the spec names as the default.
    pub fn x_height(&self, font_size: f32, style: FontStyle) -> f32 {
        let face = self.face(style.face());
        let glyph = face.glyph_id('x');
        match face.outline(glyph) {
            Some(outline) => {
                outline.bounds.height() * (font_size / face.units_per_em().unwrap_or(1000.0))
            }
            None => font_size * 0.5,
        }
    }

    /// Scale to use with `ab_glyph` for a given face at `font_size`.
    ///
    /// The faces have different units-per-em, so a shared `PxScale` would draw
    /// one of them at the wrong size.
    /// The `PxScale` that draws `font_size`-pixel glyphs from `face_id`.
    ///
    /// `ab_glyph` measures a `PxScale` against the face's *height* — its ascent
    /// less its descent — not against the em square, so handing it the font size
    /// draws every glyph at `units_per_em / height` of its proper size. On
    /// Liberation Sans that is 2048/2288, so a 16px capital came out 10px tall
    /// where a browser draws it 11: text was spaced correctly and *shaped* a
    /// tenth too small, all the way down every page.
    pub fn scale(&self, face_id: FaceId, font_size: f32) -> PxScale {
        let face = self.face(face_id);
        let units = face.units_per_em().unwrap_or(1000.0);
        let height = face.height_unscaled();
        if units > 0.0 && height > 0.0 {
            PxScale::from(font_size * height / units)
        } else {
            PxScale::from(font_size)
        }
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
        system_ui: FontRef::try_from_slice(SYSTEM_UI).expect("bundled system-ui font should parse"),
        system_ui_bold: FontRef::try_from_slice(SYSTEM_UI_BOLD)
            .expect("bundled system-ui bold font should parse"),
        fallback: FontRef::try_from_slice(FALLBACK).expect("bundled fallback font should parse"),
    })
}

#[cfg(test)]
mod tests {

    /// A glyph has to be rasterised at the size it was measured at.
    ///
    /// `ab_glyph` measures a `PxScale` against the face's *height* — its ascent
    /// less its descent — not against the em square, so handing it the font size
    /// draws every glyph at `units_per_em / height` of its proper size. On
    /// Liberation Sans that is 2048/2288: a 16px capital came out 10px tall
    /// where a browser draws it 11, and every page was set in text spaced
    /// correctly and shaped a tenth too small.
    #[test]
    fn a_glyph_is_rasterised_at_the_size_it_was_measured_at() {
        use ab_glyph::Font;
        let f = fonts();
        for size in [16.0_f32, 24.0, 40.0] {
            let face = f.face(FaceId::Sans);
            let units = face.units_per_em().expect("the face states its em square");
            let gid = face.glyph_id('H');
            let outlined = face
                .outline_glyph(gid.with_scale_and_position(
                    f.scale(FaceId::Sans, size),
                    ab_glyph::point(0.0, 0.0),
                ))
                .expect("H must outline");
            let drawn = outlined.px_bounds().height();
            // The face's own cap height, scaled by the em square as CSS asks.
            let cap = face
                .outline(gid)
                .expect("H must have an outline")
                .bounds
                .height()
                .abs()
                * (size / units);
            // `px_bounds` is the whole pixels the ink touches, so it is up to a
            // pixel taller than the outline itself.
            assert!(
                drawn >= cap && drawn <= cap + 1.5,
                "{size}px capital must rasterise {cap:.2}px tall, got {drawn:.2}"
            );
        }
    }

    /// The advance and the ink have to agree about the size, or text is spaced
    /// for one size and drawn at another.
    #[test]
    fn the_scale_matches_the_advance_the_same_size_measures() {
        use ab_glyph::{Font, ScaleFont};
        let f = fonts();
        let face = f.face(FaceId::Sans);
        let scaled = face.as_scaled(f.scale(FaceId::Sans, 16.0));
        let gid = face.glyph_id('H');
        assert!(
            (scaled.h_advance(gid) - f.advance('H', 16.0, FontStyle::regular())).abs() < 0.01,
            "the rasteriser's advance must be the one layout measured: {} vs {}",
            scaled.h_advance(gid),
            f.advance('H', 16.0, FontStyle::regular())
        );
    }
    use super::*;

    /// A character none of the bundled faces cover is answered by whichever
    /// face has it, and the glyph that comes back is a real one.
    #[test]
    fn a_hangul_codepoint_resolves_to_a_face_that_has_it() {
        let (face_id, gid) = fonts().glyph('한', FontStyle::regular());
        assert_ne!(gid.0, 0, "the fallback search must find a face with the glyph");
        assert!(
            !matches!(face_id, FaceId::Sans),
            "the bundled sans has no Hangul; something else must answer, got {face_id:?}",
        );
        assert!(fonts().advance('한', 16.0, FontStyle::regular()) > 0.0);
    }

    /// Every bundled face covers ASCII, so a Latin page never reaches the
    /// system search — which is what keeps it from ever touching the disk.
    #[test]
    fn ascii_never_leaves_the_bundled_faces() {
        for c in ['a', 'Z', '0', ' ', '@'] {
            let (face_id, gid) = fonts().glyph(c, FontStyle::regular());
            assert_eq!(face_id, FaceId::Sans, "{c:?} must come from the bundled sans");
            assert_ne!(gid.0, 0);
        }
    }

    /// The search is stable: the same character resolves to the same face every
    /// time, so a page lays out identically from one render to the next.
    #[test]
    fn the_fallback_search_is_stable() {
        let first = fonts().glyph('한', FontStyle::regular());
        for _ in 0..4 {
            assert_eq!(fonts().glyph('한', FontStyle::regular()), first);
        }
    }

    /// Measuring a run goes through the same search a single character does, so
    /// layout and paint agree about which face drew what.
    #[test]
    fn measuring_a_mixed_run_matches_its_characters() {
        let style = FontStyle::regular();
        let text = "a한b";
        let summed: f32 = text.chars().map(|c| fonts().advance(c, 16.0, style)).sum();
        let measured = fonts().measure(text, 16.0, style, 0.0);
        assert!(
            (summed - measured).abs() < 0.5,
            "measure() and advance() must agree: {summed} vs {measured}",
        );
    }
}

