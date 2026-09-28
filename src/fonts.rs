//! Font selection and text metrics.
//!
//! Resolves a CSS `font-family` list plus `font-weight` / `font-style` to real
//! font faces, the way Chromium on Linux does through fontconfig:
//!
//! 1. Each named family in the list is matched against the families found in the
//!    fontconfig font directories (`/usr/share/fonts`, `~/.local/share/fonts`, ...).
//!    Unknown names are skipped.
//! 2. Generic families (`sans-serif`, `serif`, `monospace`, ...) expand to the
//!    fontconfig preference lists from `60-latin.conf`.
//! 3. Inside a family the face is chosen with the CSS Fonts 4 weight matching
//!    algorithm; synthetic bold / italic are flagged when no real face exists.
//! 4. Per-glyph fallback: a character missing from every face of the list is looked
//!    up in CJK-capable faces, then in any installed face, then in the embedded
//!    NanumGothic, which is always present so the engine works without system fonts.
//!
//! Layout (`layout.rs`) and painting (`render.rs`) both measure text through
//! [`FontChain`], so advances used for line breaking match the painted glyphs.

use ab_glyph::{Font, FontRef, GlyphId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::css::Value;

/// NanumGothic Regular, embedded as the last-resort face.
pub const EMBEDDED_FALLBACK: &[u8] = include_bytes!("../assets/fonts/NanumGothic.ttf");

/// Index of a face inside a [`FontDb`].
pub type FaceId = usize;

/// Face id of the embedded NanumGothic in every [`FontDb`].
pub const EMBEDDED_FACE: FaceId = 0;

struct FaceEntry {
    path: Option<PathBuf>,
    index: u32,
    /// Lower-cased family names (all languages, typographic and legacy).
    families: Vec<String>,
    weight: u16,
    italic: bool,
    /// `usWidthClass` (5 = normal).
    stretch: u16,
    font: OnceLock<Option<&'static FontRef<'static>>>,
}

/// A set of font faces discovered on disk plus the embedded fallback.
pub struct FontDb {
    faces: Vec<FaceEntry>,
    chains: Mutex<HashMap<(String, u16, bool), Arc<FontChain>>>,
    fallback: Mutex<HashMap<(char, u16, bool), FaceId>>,
}

/// Generic family name -> fontconfig preference list (from `60-latin.conf`).
fn generic_family(name: &str) -> Option<&'static [&'static str]> {
    const SANS: &[&str] = &[
        "noto sans", "dejavu sans", "verdana", "arial", "albany amt", "luxi sans",
        "nimbus sans l", "nimbus sans", "helvetica", "lucida sans unicode", "tahoma",
        "liberation sans", "ubuntu", "ubuntu sans",
    ];
    const SERIF: &[&str] = &[
        "noto serif", "dejavu serif", "times new roman", "thorndale amt", "luxi serif",
        "nimbus roman no9 l", "nimbus roman", "times", "liberation serif",
    ];
    const MONO: &[&str] = &[
        "noto sans mono", "dejavu sans mono", "inconsolata", "andale mono", "courier new",
        "cumberland amt", "luxi mono", "nimbus mono l", "nimbus mono", "nimbus mono ps",
        "courier", "liberation mono", "ubuntu mono", "ubuntu sans mono",
    ];
    match name {
        "sans-serif" | "system-ui" | "ui-sans-serif" => Some(SANS),
        "serif" | "ui-serif" | "cursive" | "fantasy" => Some(SERIF),
        "monospace" | "ui-monospace" => Some(MONO),
        _ => None,
    }
}

/// Well-known families that fontconfig (`30-metric-aliases.conf`, `45-latin.conf`)
/// substitutes with a generic when they are not installed; Chromium accepts
/// those substitutions (e.g. `helvetica` renders as DejaVu Sans).
fn alias_generic(name: &str) -> Option<&'static str> {
    match name {
        "helvetica" | "arial" | "helvetica neue" | "verdana" | "tahoma" => Some("sans-serif"),
        "times" | "times new roman" | "georgia" => Some("serif"),
        "courier" | "courier new" | "consolas" => Some("monospace"),
        _ => None,
    }
}

/// CJK-capable families tried for characters missing from the requested list.
const CJK_FALLBACK: &[&str] = &[
    "noto sans cjk kr", "noto sans kr", "source han sans k", "source han sans kr",
    "malgun gothic", "nanumgothic", "undotum", "baekmuk gulim", "noto sans cjk jp",
    "noto sans cjk sc", "droid sans fallback",
];

/// Split a CSS `font-family` value into lower-cased family names.
pub fn parse_family_list(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in value.split(',') {
        let name = part.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        if name.is_empty() {
            continue;
        }
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Resolve a `font-weight` value to a number in 1..=1000.
pub fn parse_weight(value: Option<&Value>) -> u16 {
    match value {
        Some(Value::Keyword(k)) => match k.as_ref() {
            "bold" | "bolder" => 700,
            "lighter" => 300,
            s => s.parse::<f32>().map(|v| v.clamp(1.0, 1000.0) as u16).unwrap_or(400),
        },
        Some(Value::Number(v)) | Some(Value::Length(v, _)) => v.clamp(1.0, 1000.0) as u16,
        _ => 400,
    }
}

/// `true` for `font-style: italic` or `oblique`.
pub fn parse_italic(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Keyword(k)) if k.starts_with("italic") || k.starts_with("oblique"))
}

/// CSS Fonts 4 weight matching: lower rank is a better match.
fn weight_rank(desired: u16, w: u16) -> (u8, u16) {
    if (400..=500).contains(&desired) {
        if w >= desired && w <= 500 {
            (0, w - desired)
        } else if w < desired {
            (1, desired - w)
        } else {
            (2, w - desired)
        }
    } else if desired < 400 {
        if w <= desired { (0, desired - w) } else { (1, w - desired) }
    } else if w >= desired {
        (0, w - desired)
    } else {
        (1, desired - w)
    }
}

impl FontDb {
    /// Build a database from the given font directories (scanned recursively).
    /// The embedded NanumGothic is always face [`EMBEDDED_FACE`].
    pub fn from_dirs(dirs: &[PathBuf]) -> FontDb {
        let mut faces = vec![FaceEntry {
            path: None,
            index: 0,
            families: vec!["nanumgothic".into(), "나눔고딕".into()],
            weight: 400,
            italic: false,
            stretch: 5,
            font: OnceLock::new(),
        }];
        let mut files = Vec::new();
        for dir in dirs {
            collect_font_files(dir, &mut files, 0);
        }
        files.sort();
        files.dedup();
        for path in files {
            scan_file(&path, &mut faces);
        }
        FontDb {
            faces,
            chains: Mutex::new(HashMap::new()),
            fallback: Mutex::new(HashMap::new()),
        }
    }

    /// The fontconfig default directories (plus the Windows font folders on
    /// Windows).  `BROWSER_NO_SYSTEM_FONTS=1` disables
    /// system fonts so only the embedded face is used.
    pub fn system_dirs() -> Vec<PathBuf> {
        if std::env::var_os("BROWSER_NO_SYSTEM_FONTS").is_some() {
            return Vec::new();
        }
        let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
        match std::env::var_os("XDG_DATA_HOME") {
            Some(x) if !x.is_empty() => dirs.push(PathBuf::from(x).join("fonts")),
            _ => {
                if let Some(home) = std::env::var_os("HOME") {
                    dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
                }
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".fonts"));
        }
        // Windows: system fonts and per-user installed fonts.
        if cfg!(windows) {
            let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
            dirs.push(PathBuf::from(windir).join("Fonts"));
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
            }
        }
        dirs
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    /// Weight (`usWeightClass`) of a face.
    pub fn face_weight(&self, id: FaceId) -> u16 {
        self.faces[id].weight
    }

    /// `true` when the face is italic or oblique.
    pub fn face_italic(&self, id: FaceId) -> bool {
        self.faces[id].italic
    }

    /// First family name of a face (lower-cased).
    pub fn face_family(&self, id: FaceId) -> &str {
        &self.faces[id].families[0]
    }

    /// Parsed font for a face, loading (memory-mapping) it on first use.
    pub fn font(&self, id: FaceId) -> Option<&'static FontRef<'static>> {
        let entry = &self.faces[id];
        *entry.font.get_or_init(|| {
            let data: &'static [u8] = match &entry.path {
                None => EMBEDDED_FALLBACK,
                Some(p) => map_file(p)?,
            };
            FontRef::try_from_slice_and_index(data, entry.index)
                .ok()
                .map(|f| &*Box::leak(Box::new(f)))
        })
    }

    fn embedded(&self) -> &'static FontRef<'static> {
        self.font(EMBEDDED_FACE).expect("embedded font must parse")
    }

    /// Best face of `family` for the requested weight / style.
    pub fn match_family(&self, family: &str, weight: u16, italic: bool) -> Option<FaceId> {
        self.faces
            .iter()
            .enumerate()
            .filter(|(_, f)| f.families.iter().any(|n| n == family))
            .min_by_key(|(i, f)| {
                (
                    f.italic != italic,
                    (f.stretch as i32 - 5).unsigned_abs(),
                    weight_rank(weight, f.weight),
                    *i,
                )
            })
            .map(|(i, _)| i)
    }

    /// Resolve a family list to the ordered faces it names.
    fn resolve_faces(&self, families: &[String], weight: u16, italic: bool) -> Vec<FaceId> {
        let mut out: Vec<FaceId> = Vec::new();
        let push = |id: Option<FaceId>, out: &mut Vec<FaceId>| {
            if let Some(id) = id {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        };
        for fam in families {
            if let Some(list) = generic_family(fam) {
                // A generic resolves to its first installed member, like fc-match.
                if let Some(id) = list.iter().find_map(|n| self.match_family(n, weight, italic)) {
                    push(Some(id), &mut out);
                }
            } else if let Some(id) = self.match_family(fam, weight, italic) {
                push(Some(id), &mut out);
            } else if let Some(generic) = alias_generic(fam) {
                // fontconfig aliases well-known PostScript families to a generic.
                let list = generic_family(generic).unwrap();
                push(list.iter().find_map(|n| self.match_family(n, weight, italic)), &mut out);
            }
        }
        if out.is_empty() {
            // Nothing matched: Chromium uses the default (serif) font.
            if let Some(id) = generic_family("serif")
                .unwrap()
                .iter()
                .find_map(|n| self.match_family(n, weight, italic))
            {
                out.push(id);
            }
        }
        out
    }

    /// Face used for `c` when no face of the list has it.
    fn fallback_face(&self, c: char, weight: u16, italic: bool) -> FaceId {
        let key = (c, weight, italic);
        if let Some(id) = self.fallback.lock().ok().and_then(|m| m.get(&key).copied()) {
            return id;
        }
        let has = |id: FaceId| self.font(id).map(|f| f.glyph_id(c).0 != 0).unwrap_or(false);
        let mut found = CJK_FALLBACK
            .iter()
            .filter_map(|n| self.match_family(n, weight, italic))
            .find(|&id| has(id));
        if found.is_none() {
            // Any installed face, preferring the requested style and weight.
            let mut ids: Vec<FaceId> = (1..self.faces.len()).collect();
            ids.sort_by_key(|&i| {
                let f = &self.faces[i];
                (f.italic != italic, (f.stretch as i32 - 5).unsigned_abs(), weight_rank(weight, f.weight), i)
            });
            found = ids.into_iter().find(|&id| has(id));
        }
        let id = found.unwrap_or(EMBEDDED_FACE);
        if let Ok(mut m) = self.fallback.lock() {
            m.insert(key, id);
        }
        id
    }

    /// Cached font chain for a `font-family` value, weight and style.
    pub fn chain(&'static self, family: &str, weight: u16, italic: bool) -> Arc<FontChain> {
        let key = (family.to_string(), weight, italic);
        if let Some(c) = self.chains.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return c;
        }
        let families = parse_family_list(family);
        let faces = self.resolve_faces(&families, weight, italic);
        let chain = Arc::new(FontChain { db: self, faces, weight, italic });
        if let Ok(mut m) = self.chains.lock() {
            m.insert(key, chain.clone());
        }
        chain
    }
}

fn collect_font_files(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_files(&path, out, depth + 1);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc" | "otc") {
                out.push(path);
            }
        }
    }
}

fn map_file(path: &Path) -> Option<&'static [u8]> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: font files are treated as read-only; the mapping is leaked so the
    // slice lives for the rest of the process.
    let mmap = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let mmap: &'static memmap2::Mmap = Box::leak(Box::new(mmap));
    Some(&mmap[..])
}

fn scan_file(path: &Path, faces: &mut Vec<FaceEntry>) {
    let Ok(file) = std::fs::File::open(path) else { return };
    // SAFETY: read-only mapping dropped at the end of this function.
    let Ok(mmap) = (unsafe { memmap2::Mmap::map(&file) }) else { return };
    let count = ttf_parser::fonts_in_collection(&mmap).unwrap_or(1);
    for index in 0..count {
        let Ok(face) = ttf_parser::Face::parse(&mmap, index) else { continue };
        let mut families: Vec<String> = Vec::new();
        for id in [ttf_parser::name_id::TYPOGRAPHIC_FAMILY, ttf_parser::name_id::FAMILY] {
            for name in face.names().into_iter().filter(|n| n.name_id == id) {
                if let Some(s) = name.to_string() {
                    let s = s.trim().to_lowercase();
                    if !s.is_empty() && !families.contains(&s) {
                        families.push(s);
                    }
                }
            }
        }
        if families.is_empty() {
            continue;
        }
        faces.push(FaceEntry {
            path: Some(path.to_path_buf()),
            index,
            families,
            weight: face.weight().to_number(),
            italic: face.is_italic() || face.is_oblique(),
            stretch: face.width().to_number(),
            font: OnceLock::new(),
        });
    }
}

/// The process-wide font database (system fonts + embedded fallback).
pub fn db() -> &'static FontDb {
    static DB: OnceLock<FontDb> = OnceLock::new();
    DB.get_or_init(|| FontDb::from_dirs(&FontDb::system_dirs()))
}

/// Font chain for a styled node's `font-family`, `font-weight` and `font-style`.
pub fn chain_for(values: &HashMap<Arc<str>, Value>) -> Arc<FontChain> {
    thread_local! {
        static LAST: std::cell::RefCell<Option<(Arc<str>, u16, bool, Arc<FontChain>)>> =
            const { std::cell::RefCell::new(None) };
    }
    let family: &str = match values.get("font-family") {
        Some(Value::Keyword(k)) => k.as_ref(),
        _ => "serif",
    };
    let weight = parse_weight(values.get("font-weight"));
    let italic = parse_italic(values.get("font-style"));
    LAST.with(|last| {
        if let Some((f, w, i, chain)) = last.borrow().as_ref() {
            if *w == weight && *i == italic && f.as_ref() == family {
                return chain.clone();
            }
        }
        let chain = db().chain(family, weight, italic);
        *last.borrow_mut() = Some((Arc::from(family), weight, italic, chain.clone()));
        chain
    })
}

/// A glyph chosen for one character.
#[derive(Clone, Copy)]
pub struct GlyphChoice {
    pub face: FaceId,
    pub font: &'static FontRef<'static>,
    pub glyph: GlyphId,
    /// Embolden the outline (bold requested, face is not bold).
    pub synth_bold: bool,
    /// Skew the outline (italic requested, face is upright).
    pub synth_italic: bool,
}

/// Rounded font metrics in px, as Chromium's `SimpleFontData` computes them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
}

impl LineMetrics {
    /// `line-height: normal`.
    pub fn normal_line_height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }

    /// Baseline offset from the top of a line box of height `line_height`
    /// (ascent plus half-leading).
    pub fn baseline(&self, line_height: f32) -> f32 {
        (line_height - (self.ascent + self.descent)) / 2.0 + self.ascent
    }
}

/// Ordered faces for one `font-family` / weight / style, with per-glyph fallback.
pub struct FontChain {
    db: &'static FontDb,
    faces: Vec<FaceId>,
    weight: u16,
    italic: bool,
}

impl FontChain {
    /// Faces named by the family list, in order.
    pub fn faces(&self) -> &[FaceId] {
        &self.faces
    }

    /// The primary face (first available family), which defines line metrics.
    pub fn primary(&self) -> FaceId {
        self.faces
            .iter()
            .copied()
            .find(|&id| self.db.font(id).is_some())
            .unwrap_or(EMBEDDED_FACE)
    }

    fn choice(&self, face: FaceId, font: &'static FontRef<'static>, glyph: GlyphId) -> GlyphChoice {
        let entry = &self.db.faces[face];
        GlyphChoice {
            face,
            font,
            glyph,
            synth_bold: self.weight >= 600 && entry.weight < 600,
            synth_italic: self.italic && !entry.italic,
        }
    }

    /// The face and glyph used to draw `c`.
    pub fn glyph(&self, c: char) -> GlyphChoice {
        for &id in &self.faces {
            if let Some(font) = self.db.font(id) {
                let g = font.glyph_id(c);
                if g.0 != 0 {
                    return self.choice(id, font, g);
                }
            }
        }
        if c.is_whitespace() || c.is_control() {
            let id = self.primary();
            let font = self.db.font(id).unwrap_or_else(|| self.db.embedded());
            return self.choice(id, font, font.glyph_id(c));
        }
        let id = self.db.fallback_face(c, self.weight, self.italic);
        let font = self.db.font(id).unwrap_or_else(|| self.db.embedded());
        self.choice(id, font, font.glyph_id(c))
    }

    /// Advance of one character in px.
    pub fn advance(&self, c: char, font_size: f32) -> f32 {
        let g = self.glyph(c);
        let upem = g.font.units_per_em().unwrap_or(1000.0);
        g.font.h_advance_unscaled(g.glyph) * font_size / upem
    }

    /// Glyphs and pen-x offsets for `text` (no line breaking), including
    /// pair kerning between consecutive glyphs of the same face.
    pub fn shape(&self, text: &str, font_size: f32) -> (Vec<(GlyphChoice, f32)>, f32) {
        self.shape_spaced(text, font_size, 0.0)
    }

    /// [`FontChain::shape`] with CSS `letter-spacing` added after every
    /// character, including the last one and spaces (as Chromium does).
    pub fn shape_spaced(
        &self,
        text: &str,
        font_size: f32,
        letter_spacing: f32,
    ) -> (Vec<(GlyphChoice, f32)>, f32) {
        let mut out = Vec::with_capacity(text.len());
        let mut x = 0.0f32;
        let mut prev: Option<GlyphChoice> = None;
        for c in text.chars() {
            let g = self.glyph(c);
            let upem = g.font.units_per_em().unwrap_or(1000.0);
            let scale = font_size / upem;
            if let Some(p) = prev {
                if p.face == g.face {
                    x += g.font.kern_unscaled(p.glyph, g.glyph) * scale;
                }
            }
            out.push((g, x));
            x += g.font.h_advance_unscaled(g.glyph) * scale + letter_spacing;
            prev = Some(g);
        }
        (out, x)
    }

    /// Width of `text` in px (no line breaking).
    pub fn measure(&self, text: &str, font_size: f32) -> f32 {
        self.shape(text, font_size).1
    }

    /// Width of `text` in px with `letter-spacing` applied per character.
    pub fn measure_spaced(&self, text: &str, font_size: f32, letter_spacing: f32) -> f32 {
        self.shape_spaced(text, font_size, letter_spacing).1
    }

    /// Rounded ascent / descent / line gap of the primary face.
    pub fn metrics(&self, font_size: f32) -> LineMetrics {
        let font = self.db.font(self.primary()).unwrap_or_else(|| self.db.embedded());
        let upem = font.units_per_em().unwrap_or(1000.0);
        let s = font_size / upem;
        LineMetrics {
            ascent: (font.ascent_unscaled() * s).round(),
            descent: (-font.descent_unscaled() * s).round(),
            line_gap: (font.line_gap_unscaled() * s).round().max(0.0),
        }
    }
}

/// Resolve `line-height` for a node in px.  `normal` (or absent) uses the
/// primary face metrics, like Chromium.
pub fn line_height_px(values: &HashMap<Arc<str>, Value>, font_size: f32) -> f32 {
    match values.get("line-height") {
        Some(Value::Length(v, crate::css::Unit::Px)) => v.max(0.0),
        Some(Value::Length(v, crate::css::Unit::Em)) => (font_size * v).max(0.0),
        Some(Value::Length(v, crate::css::Unit::Percent)) => (font_size * v / 100.0).max(0.0),
        Some(Value::Number(v)) => (font_size * v).max(0.0),
        _ => chain_for(values).metrics(font_size).normal_line_height(),
    }
}

/// Resolve `letter-spacing` in px (`normal` is 0).
pub fn letter_spacing_px(values: &HashMap<Arc<str>, Value>, font_size: f32) -> f32 {
    match values.get("letter-spacing") {
        Some(Value::Length(v, crate::css::Unit::Px)) => *v,
        Some(Value::Length(v, crate::css::Unit::Em)) => font_size * v,
        Some(Value::Number(v)) if *v == 0.0 => 0.0,
        _ => 0.0,
    }
}

/// Width of `text` for a styled node: its font chain, kerning and
/// `letter-spacing` after every character.  Layout and painting share this.
pub fn text_width(values: &HashMap<Arc<str>, Value>, text: &str, font_size: f32) -> f32 {
    let ls = letter_spacing_px(values, font_size);
    chain_for(values).measure_spaced(text, font_size, ls)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded_only() -> &'static FontDb {
        Box::leak(Box::new(FontDb::from_dirs(&[])))
    }

    fn system() -> &'static FontDb {
        db()
    }

    #[test]
    fn parses_family_lists() {
        let v = parse_family_list(
            "-apple-system, BlinkMacSystemFont, Malgun Gothic, \"맑은 고딕\", helvetica, 'Apple SD  Gothic Neo', sans-serif",
        );
        assert_eq!(
            v,
            vec!["-apple-system", "blinkmacsystemfont", "malgun gothic", "맑은 고딕", "helvetica", "apple sd gothic neo", "sans-serif"]
        );
    }

    #[test]
    fn parses_weights() {
        use crate::css::{intern, Unit};
        assert_eq!(parse_weight(None), 400);
        assert_eq!(parse_weight(Some(&Value::Keyword(intern("bold")))), 700);
        assert_eq!(parse_weight(Some(&Value::Keyword(intern("normal")))), 400);
        assert_eq!(parse_weight(Some(&Value::Number(600.0))), 600);
        assert_eq!(parse_weight(Some(&Value::Length(500.0, Unit::Px))), 500);
    }

    #[test]
    fn weight_matching_follows_css_fonts_4() {
        let pick = |desired: u16, avail: &[u16]| {
            *avail.iter().min_by_key(|&&w| weight_rank(desired, w)).unwrap()
        };
        assert_eq!(pick(700, &[400, 700]), 700);
        assert_eq!(pick(600, &[400, 700]), 700);
        assert_eq!(pick(500, &[400, 700]), 400);
        assert_eq!(pick(400, &[300, 500, 700]), 500);
        assert_eq!(pick(300, &[400, 700]), 400);
        assert_eq!(pick(900, &[400, 700]), 700);
    }

    #[test]
    fn embedded_face_is_the_final_fallback() {
        let db = embedded_only();
        assert_eq!(db.face_count(), 1);
        let chain = db.chain("helvetica, Malgun Gothic, sans-serif", 400, false);
        assert!(chain.faces().is_empty());
        assert_eq!(chain.primary(), EMBEDDED_FACE);
        let g = chain.glyph('가');
        assert_eq!(g.face, EMBEDDED_FACE);
        assert_ne!(g.glyph.0, 0);
        assert!(chain.measure("가나다 abc", 16.0) > 0.0);
    }

    #[test]
    fn synthesizes_bold_and_italic_without_real_faces() {
        let db = embedded_only();
        let g = db.chain("sans-serif", 700, false).glyph('A');
        assert!(g.synth_bold);
        assert!(!g.synth_italic);
        let g = db.chain("sans-serif", 400, true).glyph('A');
        assert!(!g.synth_bold);
        assert!(g.synth_italic);
        let g = db.chain("sans-serif", 500, false).glyph('A');
        assert!(!g.synth_bold);
    }

    #[test]
    fn embedded_family_name_matches() {
        let db = embedded_only();
        let chain = db.chain("NanumGothic", 400, false);
        assert_eq!(chain.faces(), &[EMBEDDED_FACE]);
    }

    #[test]
    fn metrics_are_rounded_and_consistent() {
        let db = embedded_only();
        let m = db.chain("x", 400, false).metrics(16.0);
        assert_eq!(m.ascent, m.ascent.round());
        assert!(m.ascent > 0.0 && m.descent > 0.0);
        assert!(m.normal_line_height() >= 16.0);
        assert!((m.baseline(m.ascent + m.descent) - m.ascent).abs() < 1e-4);
    }

    #[test]
    fn shape_width_matches_measure_and_advances() {
        let db = embedded_only();
        let chain = db.chain("x", 400, false);
        let (glyphs, w) = chain.shape("ab가", 20.0);
        assert_eq!(glyphs.len(), 3);
        assert!(glyphs[1].1 > 0.0 && glyphs[2].1 > glyphs[1].1);
        assert_eq!(w, chain.measure("ab가", 20.0));
    }

    #[test]
    fn letter_spacing_is_added_after_every_character() {
        let db = embedded_only();
        let chain = db.chain("x", 400, false);
        let plain = chain.measure("ab c", 16.0);
        let spaced = chain.measure_spaced("ab c", 16.0, -0.5);
        assert!((plain - 2.0 - spaced).abs() < 1e-3, "{plain} {spaced}");
        let (glyphs, _) = chain.shape_spaced("ab", 16.0, 3.0);
        let (plain_glyphs, _) = chain.shape("ab", 16.0);
        assert!((glyphs[1].1 - plain_glyphs[1].1 - 3.0).abs() < 1e-3);
    }

    /// Uses Malgun Gothic when installed (as on the Naver parity machine);
    /// skipped otherwise.
    #[test]
    fn malgun_gothic_regular_and_bold_faces() {
        let db = system();
        let Some(regular) = db.match_family("malgun gothic", 400, false) else { return };
        let chain = db.chain("-apple-system, BlinkMacSystemFont, Malgun Gothic, 맑은 고딕, helvetica, sans-serif", 400, false);
        assert_eq!(chain.primary(), regular);
        let korean = db.chain("맑은 고딕", 400, false);
        assert_eq!(korean.primary(), regular);
        let bold = db.chain("Malgun Gothic", 700, false);
        if db.face_weight(bold.primary()) >= 600 {
            assert_ne!(bold.primary(), regular);
            assert!(!bold.glyph('가').synth_bold);
        } else {
            assert!(bold.glyph('가').synth_bold);
        }
    }

    /// A Latin-only face in the list falls back per glyph for Hangul.
    #[test]
    fn hangul_falls_back_from_latin_only_face() {
        let db = system();
        let chain = db.chain("DejaVu Sans", 400, false);
        let Some(&first) = chain.faces().first() else { return };
        assert_eq!(chain.glyph('A').face, first);
        let g = chain.glyph('한');
        assert_ne!(g.face, first);
        assert_ne!(g.glyph.0, 0);
    }
}
