//! SVG support: inline `<svg>` serialization, SVG image rasterization and
//! `data:` URL decoding.
//!
//! Rendering goes through `resvg`, which draws with the same tiny-skia version
//! as `render.rs`. Parsed trees and decoded data URLs are cached by content
//! hash so repeated paints do not re-parse.

use crate::css::Value;
use crate::style::StyledNode;
use markup5ever_rcdom::NodeData;
use resvg::usvg;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use tiny_skia::{Pixmap, Transform};

/// Cache entries kept before a cache is cleared wholesale.
const CACHE_MAX_ENTRIES: usize = 512;

lazy_static::lazy_static! {
    static ref TREES: Mutex<HashMap<u64, Option<Arc<usvg::Tree>>>> = Mutex::new(HashMap::new());
    static ref DATA_URLS: Mutex<HashMap<u64, Option<Arc<Vec<u8>>>>> = Mutex::new(HashMap::new());
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn cache_insert<V>(cache: &Mutex<HashMap<u64, V>>, key: u64, value: V) {
    if let Ok(mut c) = cache.lock() {
        if c.len() >= CACHE_MAX_ENTRIES {
            c.clear();
        }
        c.insert(key, value);
    }
}

/// An `xlink:href`/`href` resolver that only ever loads image data already
/// embedded as a `data:` URL. `usvg::Options::default()` otherwise installs
/// a string resolver that treats any other `href` as a local filesystem
/// path and reads it — so a remote or attacker-supplied SVG could make the
/// engine read arbitrary local files (e.g. `href="/etc/passwd"` or a
/// relative path escaping the page's own directory). Data URLs are still
/// handled by the default data resolver, which only ever sees bytes the
/// SVG document itself carried inline.
fn image_href_resolver() -> usvg::ImageHrefResolver<'static> {
    usvg::ImageHrefResolver {
        resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
        resolve_string: Box::new(|_href: &str, _opts: &usvg::Options| None),
    }
}

// ── Detection ────────────────────────────────────────────────────────────────

/// Whether `bytes` is an SVG document (sniffed from the content, since cached
/// images carry no content type).
pub fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("<svg") {
        return true;
    }
    (t.starts_with("<?xml") || t.starts_with("<!--") || t.starts_with("<!DOCTYPE svg") || t.starts_with("<!doctype svg"))
        && t.contains("<svg")
}

// ── data: URLs ───────────────────────────────────────────────────────────────

/// Decode a `data:` URL (`;base64` or percent-encoded) into its bytes.
pub fn decode_data_url(url: &str) -> Option<Arc<Vec<u8>>> {
    let rest = url.trim().strip_prefix("data:").or_else(|| {
        let t = url.trim();
        (t.len() > 5 && t[..5].eq_ignore_ascii_case("data:")).then(|| &t[5..])
    })?;
    let key = hash_bytes(url.as_bytes());
    if let Some(hit) = DATA_URLS.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let result = (|| {
        let comma = rest.find(',')?;
        let (meta, payload) = (&rest[..comma], &rest[comma + 1..]);
        let is_base64 = meta.split(';').any(|p| p.trim().eq_ignore_ascii_case("base64"));
        let unescaped: Vec<u8> = percent_encoding::percent_decode_str(payload).collect();
        if is_base64 {
            use base64::Engine;
            let clean: Vec<u8> = unescaped.into_iter().filter(|b| !b.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(&clean)
                .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&clean))
                .ok()
        } else {
            Some(unescaped)
        }
    })()
    .map(Arc::new);
    cache_insert(&DATA_URLS, key, result.clone());
    result
}

// ── Parsing and rasterization ────────────────────────────────────────────────

/// Parse (cached by content hash) an SVG document.
pub fn parse(bytes: &[u8]) -> Option<Arc<usvg::Tree>> {
    let key = hash_bytes(bytes);
    if let Some(hit) = TREES.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let mut opt = usvg::Options::default();
    opt.image_href_resolver = image_href_resolver();
    let tree = usvg::Tree::from_data(bytes, &opt).ok().map(Arc::new);
    cache_insert(&TREES, key, tree.clone());
    tree
}

/// Intrinsic size of an SVG image, in CSS px.
pub fn intrinsic_size(bytes: &[u8]) -> Option<(f32, f32)> {
    let t = parse(bytes)?;
    let s = t.size();
    Some((s.width(), s.height()))
}

/// Render `tree` stretched to `width` x `height` px into a new pixmap whose
/// origin sits at `(frac_x, frac_y)` px inside the pixmap (for subpixel
/// placement). The pixmap is premultiplied RGBA, as tiny-skia expects.
pub fn rasterize_tree(tree: &usvg::Tree, width: f32, height: f32, frac_x: f32, frac_y: f32) -> Option<Pixmap> {
    if !(width > 0.0 && height > 0.0) || !width.is_finite() || !height.is_finite() {
        return None;
    }
    let pw = (width + frac_x).ceil().max(1.0) as u32;
    let ph = (height + frac_y).ceil().max(1.0) as u32;
    if (pw as u64) * (ph as u64) > 8192 * 8192 {
        return None;
    }
    let mut pixmap = Pixmap::new(pw, ph)?;
    let size = tree.size();
    let transform = Transform::from_row(
        width / size.width(),
        0.0,
        0.0,
        height / size.height(),
        frac_x,
        frac_y,
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());
    Some(pixmap)
}

/// Rasterize an SVG image at exactly `width` x `height` px (cached by the
/// content and size), for crisp scaled drawing.
pub fn rasterize(bytes: &[u8], width: u32, height: u32) -> Option<Arc<Pixmap>> {
    lazy_static::lazy_static! {
        static ref RASTERS: Mutex<HashMap<(u64, u32, u32), Option<Arc<Pixmap>>>> = Mutex::new(HashMap::new());
    }
    let key = (hash_bytes(bytes), width, height);
    if let Some(hit) = RASTERS.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let result = parse(bytes)
        .and_then(|t| rasterize_tree(&t, width as f32, height as f32, 0.0, 0.0))
        .map(Arc::new);
    if let Ok(mut c) = RASTERS.lock() {
        if c.len() >= 128 {
            c.clear();
        }
        c.insert(key, result.clone());
    }
    result
}

// ── Inline <svg> serialization ───────────────────────────────────────────────

/// CSS properties that apply to SVG content and are copied from the cascade
/// into a `style` attribute (author CSS beats presentation attributes).
const SVG_STYLE_PROPERTIES: &[&str] = &[
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-miterlimit",
    "opacity",
    "display",
    "visibility",
    "stop-color",
    "stop-opacity",
    "clip-rule",
];

fn escape_attr(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

fn escape_text(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

fn css_color(c: &crate::css::Color) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("rgba({},{},{},{:.4})", c.r, c.g, c.b, c.a as f32 / 255.0)
    }
}

/// CSS text of a cascaded value, or `None` for values SVG cannot use.
fn value_css(v: &Value) -> Option<String> {
    match v {
        Value::Keyword(k) => {
            let k = k.trim();
            if k.is_empty() || k.contains("var(") {
                None
            } else {
                Some(k.to_string())
            }
        }
        Value::Color(c) => Some(css_color(c)),
        Value::Length(n, _) => Some(format!("{}px", n)),
        Value::Number(n) => Some(format!("{}", n)),
        _ => None,
    }
}

/// Serialize the inline `<svg>` element `node` (and its styled subtree) into a
/// standalone SVG document sized `width` x `height` px.
///
/// The cascaded values of SVG properties set by author CSS are written into
/// `style` attributes, the root's `color` resolves `currentColor`, and the
/// root's `width`/`height` are replaced by the used content box size so that
/// `viewBox`/`preserveAspectRatio` map the drawing into the laid-out box.
pub fn serialize_inline(node: &StyledNode, width: f32, height: f32) -> String {
    let mut out = String::with_capacity(512);
    write_element(node, true, width, height, &mut out);
    out
}

fn write_element(node: &StyledNode, is_root: bool, width: f32, height: f32, out: &mut String) {
    match node.node.data {
        NodeData::Element { ref name, ref attrs, .. } => {
            let tag = name.local.as_ref();
            out.push('<');
            out.push_str(tag);
            if is_root {
                out.push_str(" xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
                out.push_str(&format!(" width=\"{}\" height=\"{}\"", width, height));
            }
            let mut inline_style = String::new();
            for a in attrs.borrow().iter() {
                let local = a.name.local.as_ref();
                let qualified = match a.name.prefix {
                    Some(ref p) => format!("{}:{}", p.as_ref(), local),
                    None => local.to_string(),
                };
                if qualified == "xmlns" || qualified.starts_with("xmlns:") {
                    continue;
                }
                if is_root && matches!(local, "width" | "height" | "x" | "y") && a.name.prefix.is_none() {
                    continue;
                }
                if local == "style" && a.name.prefix.is_none() {
                    inline_style = a.value.to_string();
                    continue;
                }
                out.push(' ');
                out.push_str(&qualified);
                out.push_str("=\"");
                escape_attr(&a.value, out);
                out.push('"');
            }
            // Cascaded author CSS (the inline style attribute is already part
            // of the cascade, so it is replaced rather than repeated).
            let sv = &node.specified_values;
            let mut style = String::new();
            for prop in SVG_STYLE_PROPERTIES {
                if let Some(v) = sv.get(&crate::css::intern(prop)).and_then(value_css) {
                    if *prop == "display" && v != "none" {
                        continue;
                    }
                    style.push_str(&format!("{}:{};", prop, v));
                }
            }
            if is_root {
                if let Some(Value::Color(c)) = sv.get(&crate::css::intern("color")) {
                    style.push_str(&format!("color:{};", css_color(c)));
                }
                // Layer opacity is applied by the compositor, not by the SVG.
                style = style
                    .split(';')
                    .filter(|d| !d.is_empty() && !d.starts_with("opacity:") && !d.starts_with("display:"))
                    .map(|d| format!("{};", d))
                    .collect();
            } else if !inline_style.trim().is_empty() {
                // Keep inline declarations the cascade does not model; the
                // cascaded values follow and win.
                let mut s = inline_style.trim().trim_end_matches(';').to_string();
                s.push(';');
                s.push_str(&style);
                style = s;
            }
            if !style.is_empty() {
                out.push_str(" style=\"");
                escape_attr(&style, out);
                out.push('"');
            }
            out.push('>');
            for child in &node.children {
                write_element(child, false, width, height, out);
            }
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
        NodeData::Text { ref contents } => escape_text(&contents.borrow(), out),
        _ => {}
    }
}

/// Parse (cached) the serialized inline SVG document.
pub fn parse_inline(source: &str) -> Option<Arc<usvg::Tree>> {
    parse(source.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_data_url_base64_and_percent() {
        let b = decode_data_url("data:text/plain;base64,aGVsbG8=").unwrap();
        assert_eq!(b.as_slice(), b"hello");
        let p = decode_data_url("data:image/svg+xml,%3Csvg%20a%3D%22b%22%2F%3E").unwrap();
        assert_eq!(p.as_slice(), b"<svg a=\"b\"/>");
        let u = decode_data_url("data:image/svg+xml;utf8,<svg></svg>").unwrap();
        assert_eq!(u.as_slice(), b"<svg></svg>");
        assert!(decode_data_url("https://x/y.png").is_none());
    }

    #[test]
    fn test_is_svg_sniffs_content() {
        assert!(is_svg(b"  <svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(is_svg(b"<?xml version=\"1.0\"?>\n<svg></svg>"));
        assert!(!is_svg(b"\x89PNG\r\n"));
    }

    #[test]
    fn test_rasterize_fills_requested_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10"><rect width="10" height="10" fill="#ff0000"/></svg>"##;
        assert_eq!(intrinsic_size(svg), Some((10.0, 10.0)));
        let p = rasterize(svg, 40, 20).unwrap();
        assert_eq!((p.width(), p.height()), (40, 20));
        let px = p.pixel(39, 19).unwrap();
        assert_eq!((px.red(), px.alpha()), (255, 255));
    }

    /// A local (non-`data:`) `xlink:href` must never be read from disk: a
    /// remote or attacker-supplied SVG could otherwise read arbitrary local
    /// files through an `<image>` element. Only `data:` URLs may load image
    /// bytes.
    #[test]
    fn test_local_file_href_is_never_resolved() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("browser_svg_href_test_{}_{}.png", std::process::id(), line!()));
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]));
        img.save(&path).expect("write temp png");
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image xlink:href="{}" width="4" height="4"/></svg>"##,
            path.display()
        );
        let tree = parse(svg.as_bytes());
        let _ = std::fs::remove_file(&path);
        let tree = tree.expect("svg document itself still parses");
        let p = rasterize_tree(&tree, 4.0, 4.0, 0.0, 0.0).unwrap();
        let px = p.pixel(2, 2).unwrap();
        assert_eq!(
            (px.red(), px.green(), px.blue(), px.alpha()),
            (0, 0, 0, 0),
            "local file href must be ignored, not rasterized"
        );
    }

    /// `data:` URLs must keep working: they carry their own bytes inline
    /// and never touch the filesystem. (Uses a nested `image/svg+xml` data
    /// URL rather than a raster format, since this build's `resvg` has
    /// `default-features = false` and does not decode raster image
    /// formats regardless of the href resolver.)
    #[test]
    fn test_data_url_href_is_still_resolved() {
        use base64::Engine;
        let inner = r##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="#00ff00"/></svg>"##;
        let encoded = base64::engine::general_purpose::STANDARD.encode(inner);
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="2" height="2"><image xlink:href="data:image/svg+xml;base64,{}" width="2" height="2"/></svg>"##,
            encoded
        );
        let tree = parse(svg.as_bytes()).expect("svg parses");
        let p = rasterize_tree(&tree, 2.0, 2.0, 0.0, 0.0).unwrap();
        let px = p.pixel(1, 1).unwrap();
        assert_eq!((px.red(), px.green(), px.blue(), px.alpha()), (0, 255, 0, 255));
    }

    fn styled_svg(html: &str, css: &str) -> (StyledNode, ()) {
        let dom = crate::dom::parse_html(html);
        let sheet = crate::css::parse_css(css);
        let tree = crate::style::build_style_tree(&dom.document, &sheet, None, &HashMap::new(), None, None, None);
        (tree, ())
    }

    fn find_tag<'a>(n: &'a StyledNode, tag: &str) -> Option<&'a StyledNode> {
        if let NodeData::Element { ref name, .. } = n.node.data {
            if name.local.as_ref() == tag {
                return Some(n);
            }
        }
        n.children.iter().find_map(|c| find_tag(c, tag))
    }

    #[test]
    fn test_serialize_inline_applies_author_fill_and_current_color() {
        let (root, _) = styled_svg(
            r#"<html><body><span style="color:#112233"><svg viewBox="0 0 24 24" fill="none" width="5"><path d="M0 0h24v24H0z" stroke="currentColor"></path></svg></span></body></html>"#,
            ".x{} svg{fill:#03c75a}",
        );
        let svg = find_tag(&root, "svg").expect("svg styled node");
        let s = serialize_inline(svg, 24.0, 12.0);
        assert!(s.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""), "{s}");
        assert!(s.contains("width=\"24\" height=\"12\""), "{s}");
        assert!(!s.contains("width=\"5\""), "{s}");
        assert!(s.contains("fill:#03c75a"), "{s}");
        assert!(s.contains("color:#112233"), "{s}");
        assert!(s.contains("viewBox=\"0 0 24 24\""), "{s}");
        let tree = parse_inline(&s).expect("serialized svg parses");
        let p = rasterize_tree(&tree, 24.0, 12.0, 0.0, 0.0).unwrap();
        // The path is filled green by the CSS rule despite fill="none".
        let px = p.pixel(12, 6).unwrap();
        assert_eq!((px.red(), px.green(), px.blue()), (0x03, 0xc7, 0x5a));
    }
}
