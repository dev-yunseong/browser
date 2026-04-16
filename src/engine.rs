use std::collections::HashMap;
use std::time::Instant;
use url::Url;
use rayon::prelude::*;
use markup5ever_rcdom;

use crate::{dom, css, style, layout, render, js};

// ── Public types ─────────────────────────────────────────────────────────────

/// The result of rendering a page through the full pipeline.
#[derive(Clone, Debug)]
pub struct PageResult {
    pub pixmap_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub links: Vec<(layout::Rect, String)>,
    pub form_controls: Vec<(layout::Rect, String)>,
    pub event_handlers: Vec<(layout::Rect, String)>,
    pub element_ids: Vec<(layout::Rect, String)>,
    pub focusable_elements: Vec<(layout::Rect, String)>,
    pub image_urls: Vec<String>,
    pub body: String,
    pub base_url: Url,
    pub csp_policy: Option<js::CspPolicy>,
}

/// Result of a click action in headless mode.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ClickResult {
    /// A link was clicked; contains the absolute URL.
    Navigate { url: String },
    /// An `onclick` script handler was executed.
    ScriptExecuted,
    /// A focusable element received focus; contains its ID.
    FocusChanged { id: String },
    /// No interactive element at the given position.
    Nothing,
}

// ── HTTP API response types ───────────────────────────────────────────────────

/// A rectangle suitable for JSON serialization in HTTP API responses.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ApiRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A single interactive element returned by GET /elements.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ApiElement {
    pub id: String,
    #[serde(rename = "type")]
    pub element_type: String,
    pub text: String,
    pub href: Option<String>,
    pub rect: ApiRect,
}

/// A form control element included in the page response.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ApiFormControl {
    /// Value of the HTML `name` attribute, or empty string.
    pub name: String,
    /// Tag name: `"input"`, `"textarea"`, or `"select"`.
    pub element_type: String,
    pub rect: ApiRect,
}

/// The full page response returned by HTTP navigation/page endpoints.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ApiPageResponse {
    pub url: String,
    pub title: String,
    pub markdown: String,
    pub elements: Vec<ApiElement>,
    /// Form controls present on the page.
    #[serde(default)]
    pub forms: Vec<ApiFormControl>,
    pub width: u32,
    pub height: u32,
}

/// Convert a `PageResult` into an `ApiPageResponse` for HTTP clients.
/// Title is extracted by scanning the raw HTML for a `<title>` element.
pub fn page_to_api_response(page: &PageResult, base_url: &Url) -> ApiPageResponse {
    let title = extract_title_from_html(&page.body);
    let markdown = markdown_from_html(&page.body);
    let link_texts = extract_link_texts_from_html(&page.body);
    let elements = page
        .links
        .iter()
        .enumerate()
        .map(|(i, (rect, href))| {
            // Match link text by index (same DOM traversal order as collect_links).
            let text = link_texts.get(i).map(|(_, t)| t.clone()).unwrap_or_default();
            ApiElement {
                id: format!("e{}", i),
                element_type: "link".to_string(),
                text,
                href: Some(href.clone()),
                rect: ApiRect {
                    x: if rect.x.is_finite() { rect.x } else { 0.0 },
                    y: if rect.y.is_finite() { rect.y } else { 0.0 },
                    w: if rect.width.is_finite() { rect.width } else { 0.0 },
                    h: if rect.height.is_finite() { rect.height } else { 0.0 },
                },
            }
        })
        .collect();

    let form_meta = extract_form_controls_from_html(&page.body);
    let forms = page
        .form_controls
        .iter()
        .enumerate()
        .map(|(i, (rect, _value))| {
            let (name, element_type) = form_meta
                .get(i)
                .cloned()
                .unwrap_or_else(|| (String::new(), "input".to_string()));
            ApiFormControl {
                name,
                element_type,
                rect: ApiRect {
                    x: if rect.x.is_finite() { rect.x } else { 0.0 },
                    y: if rect.y.is_finite() { rect.y } else { 0.0 },
                    w: if rect.width.is_finite() { rect.width } else { 0.0 },
                    h: if rect.height.is_finite() { rect.height } else { 0.0 },
                },
            }
        })
        .collect();

    ApiPageResponse {
        url: base_url.to_string(),
        title,
        markdown,
        elements,
        forms,
        width: page.width,
        height: page.height,
    }
}

/// Extract the text content of the `<title>` element from raw HTML.
/// Returns an empty string if no title is found.
pub fn extract_title_from_html(html: &str) -> String {
    let lower = html.to_lowercase();
    if let Some(start) = lower.find("<title>") {
        let after = &html[start + 7..];
        if let Some(end) = after.to_lowercase().find("</title>") {
            return after[..end].trim().to_string();
        }
    }
    String::new()
}

/// Naive HTML-to-markdown converter: strips tags, collapses whitespace.
/// Not a full converter — suitable for CLI display of page content.
pub fn markdown_from_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_buf = String::new();

    let mut chars = html.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if ch == '>' && in_tag {
            in_tag = false;
            let tag_lower = tag_buf.to_lowercase();
            let tag_lower = tag_lower.trim();
            if tag_lower == "script" || tag_lower.starts_with("script ") {
                in_script = true;
            } else if tag_lower == "/script" {
                in_script = false;
            } else if tag_lower == "style" || tag_lower.starts_with("style ") {
                in_style = true;
            } else if tag_lower == "/style" {
                in_style = false;
            } else if !in_script && !in_style {
                // Insert newline for block-level tags
                if matches!(tag_lower, "p" | "br" | "/p" | "div" | "/div"
                    | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                    | "/h1" | "/h2" | "/h3" | "/h4" | "/h5" | "/h6"
                    | "li" | "/li" | "tr" | "/tr") {
                    out.push('\n');
                }
            }
        } else if in_tag {
            tag_buf.push(ch);
        } else if !in_script && !in_style {
            out.push(ch);
        }
    }

    // Collapse runs of whitespace (but preserve newlines)
    let mut result = String::with_capacity(out.len());
    let mut last_was_space = false;
    let mut last_was_newline = false;
    for ch in out.chars() {
        if ch == '\n' {
            if !last_was_newline {
                result.push('\n');
            }
            last_was_newline = true;
            last_was_space = false;
        } else if ch.is_whitespace() {
            if !last_was_space && !last_was_newline {
                result.push(' ');
            }
            last_was_space = true;
        } else {
            result.push(ch);
            last_was_space = false;
            last_was_newline = false;
        }
    }
    result.trim().to_string()
}

/// Extract `(href, display_text)` pairs from `<a>` elements in HTML document order.
///
/// Uses the same char-by-char scanner style as `markdown_from_html`. Handles nested
/// inline tags (strips them, keeping inner text) and basic HTML entities.
pub fn extract_link_texts_from_html(html: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    let mut chars = html.chars().peekable();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    // State for being inside an <a> element
    let mut in_anchor = false;
    let mut anchor_href = String::new();
    let mut anchor_text = String::new();
    let mut anchor_depth = 0u32; // nesting depth of tags inside the anchor

    while let Some(ch) = chars.next() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if ch == '>' && in_tag {
            in_tag = false;
            let tag_str = tag_buf.trim().to_string();
            let tag_lower = tag_str.to_lowercase();
            let tag_lower = tag_lower.trim();

            if tag_lower.starts_with("a ") || tag_lower == "a" {
                // Opening <a> tag — extract href
                in_anchor = true;
                anchor_href = extract_attr(&tag_str, "href").unwrap_or_default();
                anchor_text.clear();
                anchor_depth = 0;
            } else if tag_lower == "/a" {
                if in_anchor {
                    let text = decode_html_entities(anchor_text.trim());
                    results.push((anchor_href.clone(), text));
                    in_anchor = false;
                    anchor_href.clear();
                    anchor_text.clear();
                }
            } else if in_anchor {
                // Track nesting depth for inner tags (we don't need to do anything else)
                if tag_lower.starts_with('/') {
                    anchor_depth = anchor_depth.saturating_sub(1);
                } else if !tag_lower.ends_with('/') {
                    anchor_depth += 1;
                }
            }
        } else if in_tag {
            tag_buf.push(ch);
        } else if in_anchor {
            anchor_text.push(ch);
        }
    }

    results
}

/// Extract `(name_attr, element_type)` pairs for form controls in HTML document order.
///
/// Returns one entry per `<input>`, `<textarea>`, or `<select>` tag.
/// The order matches `collect_form_controls` traversal order (DOM order).
pub fn extract_form_controls_from_html(html: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    let mut chars = html.chars().peekable();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    let mut in_script = false;

    while let Some(ch) = chars.next() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else if ch == '>' && in_tag {
            in_tag = false;
            let tag_str = tag_buf.trim().to_string();
            let tag_lower = tag_str.to_lowercase();
            let tag_lower_trimmed = tag_lower.trim();

            if tag_lower_trimmed == "script" || tag_lower_trimmed.starts_with("script ") {
                in_script = true;
            } else if tag_lower_trimmed == "/script" {
                in_script = false;
            } else if !in_script {
                let (verb, _) = tag_lower_trimmed.split_once(' ').unwrap_or((tag_lower_trimmed, ""));
                if matches!(verb, "input" | "textarea" | "select") {
                    let name = extract_attr(&tag_str, "name").unwrap_or_default();
                    results.push((name, verb.to_string()));
                }
            }
        } else if in_tag {
            tag_buf.push(ch);
        }
    }

    results
}

/// Extract the value of an attribute from a raw HTML tag string (e.g., `a href="url" class="x"`).
fn extract_attr(tag: &str, attr_name: &str) -> Option<String> {
    // We need a case-insensitive search that never mixes byte offsets between two strings
    // whose byte lengths might differ (because `to_lowercase()` can expand characters:
    // e.g., İ U+0130 is 1 char / 2 bytes in `tag` but lowercases to "i\u{307}", 2 chars /
    // 3 bytes, in `lower`).
    //
    // Strategy: search `lower` for the attribute pattern to decide *whether* it is present,
    // then walk both `tag` and `lower` together to find the matching byte offset in `tag`.
    // We consume one codepoint from `tag` and its lowercased expansion from `lower` at a time,
    // so the two cursors stay synchronised even when `to_lowercase` changes char or byte counts.
    let lower = tag.to_lowercase();
    let search = format!("{}=", attr_name.to_lowercase());

    // Confirm the attribute exists in the lowercased string.
    let end_in_lower = lower.find(&search)? + search.len();

    // Walk both strings in lock-step: advance `lower_consumed` by the byte length of the
    // lowercased form of each codepoint in `tag` until we have consumed `end_in_lower` bytes
    // of `lower`.  At that point `tag_byte_offset` is the corresponding byte position in `tag`.
    let mut lower_consumed: usize = 0;
    let mut tag_byte_offset: usize = 0;
    let mut lower_chars = lower.char_indices().peekable();

    'outer: for (tag_off, tag_ch) in tag.char_indices() {
        if lower_consumed >= end_in_lower {
            tag_byte_offset = tag_off;
            break 'outer;
        }
        tag_byte_offset = tag_off + tag_ch.len_utf8(); // default: past end of tag
        // Consume the lowercased expansion of `tag_ch` from `lower`.
        for lc in tag_ch.to_lowercase() {
            if let Some((_, lc_from_lower)) = lower_chars.next() {
                debug_assert_eq!(lc, lc_from_lower,
                    "to_lowercase mismatch: tag_ch={:?} expanded to {:?} but lower had {:?}",
                    tag_ch, lc, lc_from_lower);
                lower_consumed += lc.len_utf8();
            }
        }
    }

    if lower_consumed < end_in_lower {
        // We exhausted `tag` before reaching `end_in_lower` — attribute not really there.
        return None;
    }

    let rest = tag[tag_byte_offset..].trim_start();
    if rest.starts_with('"') {
        // Quoted value
        let inner = &rest[1..];
        let end = inner.find('"').unwrap_or(inner.len());
        Some(inner[..end].to_string())
    } else if rest.starts_with('\'') {
        let inner = &rest[1..];
        let end = inner.find('\'').unwrap_or(inner.len());
        Some(inner[..end].to_string())
    } else {
        // Unquoted value
        let end = rest.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

/// Decode common HTML entities in text content.
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Source of a CSS stylesheet, in document order.
#[derive(Clone, Debug)]
pub enum CssSource {
    Inline(String),
    Remote(String), // URL
}

// ── Free pipeline functions ───────────────────────────────────────────────────

/// Walk the DOM and collect CSS sources in document order.
pub fn collect_css_in_order(
    handle: &markup5ever_rcdom::Handle,
    base_url: &Url,
    cache: &HashMap<String, String>,
    sources: &mut Vec<CssSource>,
) {
    if let markup5ever_rcdom::NodeData::Element { ref name, ref attrs, .. } = handle.data {
        let tag = name.local.to_string();
        if tag == "style" {
            let mut inline = String::new();
            for child in handle.children.borrow().iter() {
                if let markup5ever_rcdom::NodeData::Text { ref contents } = child.data {
                    inline.push_str(&contents.borrow());
                }
            }
            if !inline.is_empty() {
                sources.push(CssSource::Inline(inline));
            }
        } else if tag == "link" {
            let mut is_stylesheet = false;
            let mut href = None;
            for attr in attrs.borrow().iter() {
                if attr.name.local.to_string() == "rel" && attr.value.to_string() == "stylesheet" {
                    is_stylesheet = true;
                } else if attr.name.local.to_string() == "href" {
                    href = Some(attr.value.to_string());
                }
            }
            if is_stylesheet {
                if let Some(h) = href {
                    let abs_url = base_url.join(&h).map(|u| u.to_string()).unwrap_or(h);
                    if let Some(cached) = cache.get(&abs_url) {
                        sources.push(CssSource::Inline(cached.clone()));
                    } else {
                        sources.push(CssSource::Remote(abs_url));
                    }
                }
            }
        }
    }
    for child in handle.children.borrow().iter() {
        collect_css_in_order(child, base_url, cache, sources);
    }
}

/// Fetch a URL and run the full pipeline, returning a `PageResult`.
pub fn fetch_and_process(
    url_str: &str,
    css_cache: &mut HashMap<String, String>,
    js_overrides: &HashMap<String, HashMap<String, String>>,
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
    width: f32,
) -> Result<(PageResult, css::Stylesheet), Box<dyn std::error::Error + Send + Sync>> {
    let response = reqwest::blocking::get(url_str)?;
    let csp_header = response
        .headers()
        .get("content-security-policy")
        .and_then(|h| h.to_str().ok())
        .map(|s| js::CspPolicy::parse(s));

    let body = response.text()?;
    let base_url = Url::parse(url_str)?;
    process_html_with_cache(
        &body,
        &base_url,
        &HashMap::new(),
        css_cache,
        None,
        js_overrides,
        hovered_id,
        focused_id,
        csp_header,
        width,
    )
}

/// Run the full pipeline on pre-fetched HTML, returning a `PageResult`.
pub fn process_html_with_cache(
    body: &str,
    base_url: &Url,
    image_cache: &HashMap<String, Vec<u8>>,
    css_cache: &mut HashMap<String, String>,
    cached_stylesheet: Option<css::Stylesheet>,
    js_overrides: &HashMap<String, HashMap<String, String>>,
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
    csp_policy: Option<js::CspPolicy>,
    width: f32,
) -> Result<(PageResult, css::Stylesheet), Box<dyn std::error::Error + Send + Sync>> {
    let start_total = Instant::now();
    let width = width.max(1.0);

    let start = Instant::now();
    let dom_tree = dom::parse_html(body);
    let dom_elapsed = start.elapsed();

    let stylesheet = if let Some(s) = cached_stylesheet {
        s
    } else {
        // 1. Collect all CSS sources (sequential DOM walk)
        let start_collect = Instant::now();
        let mut sources = Vec::new();
        collect_css_in_order(&dom_tree.document, base_url, css_cache, &mut sources);
        println!("  - CSS collect metadata: {:?}", start_collect.elapsed());

        // 2. Fetch all remote sources in parallel
        let fetched_contents: Vec<(String, Option<String>)> =
            sources.into_par_iter().map(|src| match src {
                CssSource::Inline(text) => (text, None),
                CssSource::Remote(url) => {
                    let start_fetch = Instant::now();
                    match reqwest::blocking::get(&url).and_then(|resp| resp.text()) {
                        Ok(text) => {
                            println!(
                                "[Perf] Parallel Fetch (CSS): {} in {:?}",
                                url,
                                start_fetch.elapsed()
                            );
                            (text, Some(url))
                        }
                        Err(e) => {
                            println!("[Error] Parallel Fetch (CSS): {} failed: {}", url, e);
                            (String::new(), None)
                        }
                    }
                }
            }).collect();

        // 3. Assemble and update cache
        let mut final_css = String::new();
        for (content, url_opt) in fetched_contents {
            final_css.push_str(&content);
            if let Some(url) = url_opt {
                css_cache.insert(url, content);
            }
        }

        let start_parse = Instant::now();
        let s = css::parse_css(&final_css);
        println!("  - CSS parse: {:?}", start_parse.elapsed());
        s
    };

    let start = Instant::now();
    let style_tree = style::build_style_tree(
        &dom_tree.document,
        &stylesheet,
        None,
        js_overrides,
        hovered_id,
        focused_id,
        csp_policy.as_ref(),
    );
    let style_elapsed = start.elapsed();

    let start = Instant::now();
    let (layout_tree_opt, _, final_y) =
        layout::build_layout_tree(&style_tree, 0.0, 0.0, 0.0, width, width, 768.0);
    let layout_tree = layout_tree_opt.ok_or("Failed to build layout tree")?;
    let layout_elapsed = start.elapsed();

    let height = (final_y.ceil() as u32).clamp(600, 16384);
    let w_u32 = width as u32;

    let start = Instant::now();
    let mut pixmap = tiny_skia::Pixmap::new(w_u32, height)
        .ok_or_else(|| format!("Failed to create pixmap with size {}x{}", w_u32, height))?;

    pixmap.fill(tiny_skia::Color::WHITE);

    let mut links: Vec<(layout::Rect, String)> = Vec::new();
    let mut form_controls = Vec::new();
    let mut event_handlers = Vec::new();
    let mut element_ids = Vec::new();
    let mut focusable_elements = Vec::new();
    let image_urls: Vec<String>;

    render::render_layout_tree(&layout_tree, &mut pixmap, image_cache);

    layout_tree.collect_links(&mut links);
    layout_tree.collect_event_handlers(&mut event_handlers);
    layout_tree.collect_element_ids(&mut element_ids);
    layout_tree.collect_focusable_elements(&mut focusable_elements);

    let mut controls_with_nodes = Vec::new();
    layout_tree.collect_form_controls(&mut controls_with_nodes);

    let mut image_urls_raw = Vec::new();
    layout_tree.collect_images(&mut image_urls_raw);
    image_urls = image_urls_raw
        .into_iter()
        .map(|(_, url)| base_url.join(&url).map(|u| u.to_string()).unwrap_or(url))
        .collect();

    for (rect, node) in controls_with_nodes {
        let mut val = String::new();
        if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = node.node.data {
            for attr in attrs.borrow().iter() {
                if attr.name.local.to_string() == "value" {
                    val = attr.value.to_string();
                }
            }
        }
        form_controls.push((rect, val));
    }

    let render_elapsed = start.elapsed();

    let start = Instant::now();
    let absolute_links = links
        .into_iter()
        .map(|(rect, link)| {
            let abs = base_url.join(&link).map(|u| u.to_string()).unwrap_or(link);
            (rect, abs)
        })
        .collect();

    let pixmap_bytes = pixmap.data().to_vec();
    let data_copy_elapsed = start.elapsed();

    let total_elapsed = start_total.elapsed();

    println!("[Perf] process_html_with_cache total: {:?}", total_elapsed);
    println!("  - DOM parse: {:?}", dom_elapsed);
    println!("  - Style build: {:?}", style_elapsed);
    println!("  - Layout build: {:?}", layout_elapsed);
    println!("  - Render: {:?}", render_elapsed);
    println!("  - Data copy & Links: {:?}", data_copy_elapsed);

    Ok((
        PageResult {
            pixmap_bytes,
            width: width as u32,
            height,
            links: absolute_links,
            form_controls,
            event_handlers,
            element_ids,
            focusable_elements,
            image_urls,
            body: body.to_string(),
            base_url: base_url.clone(),
            csp_policy,
        },
        stylesheet,
    ))
}

// ── BrowserEngine ─────────────────────────────────────────────────────────────

/// A headless browser engine with no GUI dependencies.
/// Owns all pipeline state: caches, JS runtime, and last rendered page.
pub struct BrowserEngine {
    pub image_cache: HashMap<String, Vec<u8>>,
    pub css_cache: HashMap<String, String>,
    pub last_stylesheet: Option<css::Stylesheet>,
    pub js_runtime: js::JsRuntime,
    pub current_csp_policy: Option<js::CspPolicy>,
    pub js_style_overrides: HashMap<String, HashMap<String, String>>,
    /// The most recently rendered page result.
    pub last_page: Option<PageResult>,
}

impl BrowserEngine {
    /// Create a new engine with empty caches and a fresh JS runtime.
    pub fn new() -> Self {
        Self {
            image_cache: HashMap::new(),
            css_cache: HashMap::new(),
            last_stylesheet: None,
            js_runtime: js::JsRuntime::new(None, None, None),
            current_csp_policy: None,
            js_style_overrides: HashMap::new(),
            last_page: None,
        }
    }

    /// Synchronously navigate to a URL.
    /// Stores the resulting `PageResult` and calls `init_js_for_page`.
    pub fn navigate(&mut self, url_str: &str, width: f32) -> Result<PageResult, String> {
        self.clear_for_new_url();
        let result = fetch_and_process(
            url_str,
            &mut self.css_cache,
            &self.js_style_overrides,
            None,
            None,
            width,
        )
        .map_err(|e| e.to_string())?;

        let (page, stylesheet) = result;
        self.last_stylesheet = Some(stylesheet);
        self.current_csp_policy = page.csp_policy.clone();
        self.last_page = Some(page.clone());
        self.init_js_for_page(&page.body);
        Ok(page)
    }

    /// Re-render the current page (e.g. after JS style changes or hover state).
    pub fn re_render(
        &mut self,
        hovered_id: Option<&str>,
        focused_id: Option<&str>,
        width: f32,
    ) -> Result<PageResult, String> {
        let (body, base_url) = match &self.last_page {
            Some(p) => (p.body.clone(), p.base_url.clone()),
            None => return Err("No page loaded".into()),
        };

        let mut css_cache = self.css_cache.clone();
        let result = process_html_with_cache(
            &body,
            &base_url,
            &self.image_cache,
            &mut css_cache,
            self.last_stylesheet.clone(),
            &self.js_style_overrides,
            hovered_id,
            focused_id,
            self.current_csp_policy.clone(),
            width,
        )
        .map_err(|e| e.to_string())?;

        self.css_cache = css_cache;
        let (page, stylesheet) = result;
        self.last_stylesheet = Some(stylesheet);
        self.last_page = Some(page.clone());
        Ok(page)
    }

    /// Hit-test a click at `(x, y)` against the last rendered page.
    /// Returns all interaction results (links, onclick handlers, focus changes).
    pub fn click(&mut self, x: f32, y: f32) -> Vec<ClickResult> {
        let page = match &self.last_page {
            Some(p) => p.clone(),
            None => return vec![ClickResult::Nothing],
        };

        let mut results = Vec::new();

        // Dispatch standard JS 'click' events for elements with IDs
        for (rect, id) in &page.element_ids {
            if hit_test(x, y, rect) {
                self.js_runtime.trigger_event(id, "click");
            }
        }

        // Focus change
        for (rect, id) in &page.focusable_elements {
            if hit_test(x, y, rect) {
                results.push(ClickResult::FocusChanged { id: id.clone() });
            }
        }

        // onclick attribute handlers
        for (rect, script) in &page.event_handlers {
            if hit_test(x, y, rect) {
                self.js_runtime.execute(script);
                results.push(ClickResult::ScriptExecuted);
            }
        }

        // Collect JS style overrides produced by onclick handlers
        let overrides = self.js_runtime.get_style_overrides();
        if !overrides.is_empty() {
            for (id, props) in overrides {
                self.js_style_overrides.entry(id).or_default().extend(props);
            }
        }

        // Links (navigate)
        for (rect, link) in &page.links {
            if hit_test(x, y, rect) {
                results.push(ClickResult::Navigate { url: link.clone() });
                break;
            }
        }

        if results.is_empty() {
            results.push(ClickResult::Nothing);
        }
        results
    }

    /// Stub: inject text into the currently focused form input.
    /// Full implementation deferred to follow-up issue.
    pub fn type_text(&mut self, _text: &str) {
        println!("[BrowserEngine] type_text: not yet implemented");
    }

    /// Execute JavaScript in the current page's runtime.
    pub fn evaluate_js(&mut self, script: &str) -> String {
        self.js_runtime.execute(script);
        // Return any queued style override info as a simple diagnostic string
        let overrides = self.js_runtime.get_style_overrides();
        if overrides.is_empty() {
            String::new()
        } else {
            format!("style_overrides: {:?}", overrides)
        }
    }

    /// Reconstruct a `Pixmap` from the last rendered page's pixel data.
    pub fn screenshot(&self) -> Option<tiny_skia::Pixmap> {
        let page = self.last_page.as_ref()?;
        tiny_skia::Pixmap::from_vec(
            page.pixmap_bytes.clone(),
            tiny_skia::IntSize::from_wh(page.width, page.height)?,
        )
    }

    /// Return the raw HTML source of the last loaded page.
    /// Full DOM serialization is deferred to a follow-up issue.
    pub fn dom_tree(&self) -> String {
        self.last_page.as_ref().map(|p| p.body.clone()).unwrap_or_default()
    }

    /// Return a summary of the last rendered layout.
    /// Full layout tree dump is deferred to a follow-up issue.
    pub fn layout_tree(&self) -> String {
        match &self.last_page {
            Some(p) => format!(
                "PageResult {{ width: {}, height: {}, links: {}, form_controls: {} }}",
                p.width,
                p.height,
                p.links.len(),
                p.form_controls.len()
            ),
            None => String::new(),
        }
    }

    /// Return the computed style properties for a CSS selector.
    /// Stub — full implementation deferred to a follow-up issue.
    pub fn computed_style(&self, _selector: &str) -> HashMap<String, String> {
        HashMap::new()
    }

    /// Re-initialize the JS runtime for the current page.
    /// Parses the DOM from `body`, runs all page scripts (CSP-gated),
    /// and collects any immediate JS style overrides.
    pub fn init_js_for_page(&mut self, body: &str) {
        let base_url = self.last_page.as_ref().map(|p| p.base_url.clone());
        let dom = dom::parse_html(body);
        self.js_runtime = js::JsRuntime::new(
            Some(dom.document.clone()),
            base_url,
            self.current_csp_policy.clone(),
        );

        let scripts = js::extract_scripts_from_dom(&dom.document);
        let allowed = self
            .current_csp_policy
            .as_ref()
            .map(|p| p.allows_inline_script())
            .unwrap_or(true);

        if allowed {
            for script in scripts {
                self.js_runtime.execute(&script);
            }
        } else {
            println!("[CSP] Blocked inline script execution");
        }

        let overrides = self.js_runtime.get_style_overrides();
        if !overrides.is_empty() {
            for (id, props) in overrides {
                self.js_style_overrides.entry(id).or_default().extend(props);
            }
        }
    }

    /// Reset all state in preparation for navigating to a new URL.
    pub fn clear_for_new_url(&mut self) {
        self.js_style_overrides.clear();
        self.js_runtime = js::JsRuntime::new(None, None, None);
        self.current_csp_policy = None;
        self.last_stylesheet = None;
        self.last_page = None;
        self.css_cache.clear();
    }

    /// Drain JS style overrides produced since the last call.
    pub fn get_style_overrides(&mut self) -> HashMap<String, HashMap<String, String>> {
        self.js_runtime.get_style_overrides()
    }

    /// Advance the JS event loop by one tick.
    /// Returns `true` if a re-render is needed.
    pub fn tick_js(&mut self, timestamp: Option<f64>, deadline: Option<f64>) -> bool {
        self.js_runtime.tick(Some(timestamp.unwrap_or(0.0)), deadline)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[inline]
fn hit_test(x: f32, y: f32, r: &layout::Rect) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_engine_new() {
        let engine = BrowserEngine::new();
        assert!(engine.last_page.is_none());
        assert!(engine.image_cache.is_empty());
        assert!(engine.css_cache.is_empty());
        assert!(engine.js_style_overrides.is_empty());
        assert!(engine.last_stylesheet.is_none());
        assert!(engine.current_csp_policy.is_none());
    }

    #[test]
    fn test_click_on_empty_engine_returns_nothing() {
        let mut engine = BrowserEngine::new();
        let results = engine.click(100.0, 100.0);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], ClickResult::Nothing));
    }

    #[test]
    fn test_screenshot_on_empty_engine_returns_none() {
        let engine = BrowserEngine::new();
        assert!(engine.screenshot().is_none());
    }

    #[test]
    fn test_dom_tree_on_empty_engine() {
        let engine = BrowserEngine::new();
        assert_eq!(engine.dom_tree(), "");
    }

    #[test]
    fn test_layout_tree_on_empty_engine() {
        let engine = BrowserEngine::new();
        assert_eq!(engine.layout_tree(), "");
    }

    #[test]
    fn test_computed_style_stub() {
        let engine = BrowserEngine::new();
        let style = engine.computed_style("body");
        assert!(style.is_empty());
    }

    // ── New tests for link/form extraction and ClickResult serde ────────────────

    #[test]
    fn test_extract_link_texts_basic() {
        let html = r#"<a href="https://example.com">Click here</a>"#;
        let links = extract_link_texts_from_html(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0, "https://example.com");
        assert_eq!(links[0].1, "Click here");
    }

    #[test]
    fn test_extract_link_texts_nested_tags() {
        let html = r#"<a href="/page"><span>Inner text</span></a>"#;
        let links = extract_link_texts_from_html(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0, "/page");
        assert_eq!(links[0].1, "Inner text");
    }

    #[test]
    fn test_extract_link_texts_multiple() {
        let html = r#"<a href="/a">First</a> text <a href="/b">Second</a>"#;
        let links = extract_link_texts_from_html(html);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].1, "First");
        assert_eq!(links[1].1, "Second");
    }

    #[test]
    fn test_extract_link_texts_html_entities() {
        let html = r#"<a href="/x">Rock &amp; Roll</a>"#;
        let links = extract_link_texts_from_html(html);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].1, "Rock & Roll");
    }

    #[test]
    fn test_extract_form_controls_basic() {
        let html = r#"<form><input name="user" type="text"><input name="pass" type="password"></form>"#;
        let controls = extract_form_controls_from_html(html);
        assert_eq!(controls.len(), 2);
        assert_eq!(controls[0].0, "user");
        assert_eq!(controls[0].1, "input");
        assert_eq!(controls[1].0, "pass");
        assert_eq!(controls[1].1, "input");
    }

    #[test]
    fn test_extract_form_controls_select() {
        let html = r#"<select name="role"><option>Admin</option></select>"#;
        let controls = extract_form_controls_from_html(html);
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].0, "role");
        assert_eq!(controls[0].1, "select");
    }

    #[test]
    fn test_click_result_serde_navigate() {
        let r = ClickResult::Navigate { url: "https://example.com".to_string() };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"type\":\"Navigate\""));
        assert!(json.contains("\"url\":\"https://example.com\""));
        let back: ClickResult = serde_json::from_str(&json).expect("deserialize");
        match back {
            ClickResult::Navigate { url } => assert_eq!(url, "https://example.com"),
            _ => panic!("Expected Navigate"),
        }
    }

    #[test]
    fn test_click_result_serde_focus() {
        let r = ClickResult::FocusChanged { id: "search-input".to_string() };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"type\":\"FocusChanged\""));
        assert!(json.contains("\"id\":\"search-input\""));
        let back: ClickResult = serde_json::from_str(&json).expect("deserialize");
        match back {
            ClickResult::FocusChanged { id } => assert_eq!(id, "search-input"),
            _ => panic!("Expected FocusChanged"),
        }
    }

    // ── extract_attr tests ───────────────────────────────────────────────────────

    #[test]
    fn test_extract_attr_basic() {
        let tag = r#"a href="https://example.com" class="link""#;
        assert_eq!(extract_attr(tag, "href"), Some("https://example.com".to_string()));
    }

    #[test]
    fn test_extract_attr_case_insensitive() {
        let tag = r#"INPUT NAME="username" TYPE="text""#;
        assert_eq!(extract_attr(tag, "name"), Some("username".to_string()));
    }

    #[test]
    fn test_extract_attr_unicode_before_attr_no_panic() {
        // İ (U+0130) is 1 char / 2 bytes, but lowercases to "i\u{307}" which is 2 chars / 3 bytes.
        // Placing it before the attribute ensures the old byte-offset mixing would panic or
        // overshoot.  The new implementation must not panic and must return the correct value.
        let tag = "a data-İ=\"x\" href=\"/path\"";
        let result = extract_attr(tag, "href");
        assert_eq!(result, Some("/path".to_string()));
    }

    #[test]
    fn test_extract_attr_missing_returns_none() {
        let tag = r#"img src="photo.jpg""#;
        assert_eq!(extract_attr(tag, "href"), None);
    }

    #[test]
    fn test_extract_attr_single_quoted() {
        let tag = "a href='/page'";
        assert_eq!(extract_attr(tag, "href"), Some("/page".to_string()));
    }

    #[test]
    fn test_extract_attr_unquoted() {
        let tag = "input type=text name=user";
        assert_eq!(extract_attr(tag, "type"), Some("text".to_string()));
    }
}
