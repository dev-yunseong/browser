use markup5ever_rcdom;
use rayon::prelude::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use url::Url;

use crate::{css, dom, frames, js, layout, render, style};

// ── Public types ─────────────────────────────────────────────────────────────

/// Metadata for a single form control (input, textarea, select).
#[derive(Clone, Debug)]
pub struct FormControlMeta {
    pub name: String,
    pub rect: layout::Rect,
    pub initial_value: String,
}

/// Metadata for a `<form>` element: its action/method attributes and child controls.
#[derive(Clone, Debug)]
pub struct FormMetadata {
    pub action: String,
    pub method: String,
    pub controls: Vec<FormControlMeta>,
}

/// The result of rendering a page through the full pipeline.
#[derive(Clone, Debug)]
pub struct PageResult {
    pub pixmap_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub links: Vec<(layout::Rect, String)>,
    pub form_controls: Vec<(layout::Rect, String)>,
    pub form_buttons: Vec<(layout::Rect, String)>,
    /// Name attributes for form_controls, same order and length.
    pub form_control_names: Vec<String>,
    pub event_handlers: Vec<(layout::Rect, String)>,
    pub element_ids: Vec<(layout::Rect, String)>,
    pub focusable_elements: Vec<(layout::Rect, String)>,
    pub image_urls: Vec<String>,
    pub layout_metrics: HashMap<String, js::LayoutMetrics>,
    pub body: String,
    pub base_url: Url,
    pub csp_policy: Option<js::CspPolicy>,
    /// Form metadata (action, method, and named controls) for the first `<form>` on the page.
    /// `None` if the page has no `<form>` element.
    pub form_metadata: Option<FormMetadata>,
    /// Scrollable document size (layout overflow extent) in CSS px, at least
    /// the viewport width and the laid-out height.
    pub scroll_width: f32,
    pub scroll_height: f32,
    /// Document scroll offset this page was rendered at. `position: fixed`
    /// boxes are painted relative to the viewport at this offset.
    pub scroll_x: f32,
    pub scroll_y: f32,
    /// `<iframe>` boxes that load a network document, with their content-box
    /// size; their rendered documents are painted from `image_cache`.
    pub frames: Vec<frames::FrameBox>,
}

/// Most documents one script-driven navigation may chain through (a page
/// whose script sends it on to another page, and so on).
const MAX_SCRIPT_NAVIGATIONS: usize = 5;

/// Session history: the URLs of top-level documents loaded, in order, and
/// which one is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionHistory {
    entries: Vec<String>,
    index: usize,
}

impl SessionHistory {
    /// Add `url` after the current entry, dropping the forward entries. A
    /// load of the current URL again adds nothing.
    pub fn push(&mut self, url: String) {
        if self.current() == Some(url.as_str()) {
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
        self.entries.push(url);
        self.index = self.entries.len() - 1;
    }

    /// Replace the current entry with `url` (or add it to an empty history).
    pub fn replace(&mut self, url: String) {
        match self.entries.get_mut(self.index) {
            Some(entry) => *entry = url,
            None => self.push(url),
        }
    }

    pub fn current(&self) -> Option<&str> {
        self.entries.get(self.index).map(String::as_str)
    }

    pub fn can_go_back(&self) -> bool {
        !self.entries.is_empty() && self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.entries.len()
    }

    /// Index and URL of the entry `delta` steps from the current one.
    fn entry_at(&self, delta: isize) -> Option<(usize, String)> {
        let index = self.index.checked_add_signed(delta)?;
        self.entries.get(index).map(|url| (index, url.clone()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Session history shared between the engine actor and GUI threads.
pub type SharedHistory = std::sync::Arc<std::sync::Mutex<SessionHistory>>;

/// Result of a click action in headless mode.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ClickResult {
    /// A link was clicked; contains the absolute URL. The caller loads it.
    Navigate { url: String },
    /// Page script started a navigation (`location.href = …`,
    /// `form.submit()`) and the engine already loaded `url`.
    Navigated { url: String },
    /// An `onclick` script handler was executed.
    ScriptExecuted,
    /// A focusable element received focus; contains its ID.
    FocusChanged { id: String },
    /// No interactive element at the given position.
    Nothing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineRequestError {
    Busy,
    Disconnected,
    Failed(String),
}

impl std::fmt::Display for EngineRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineRequestError::Busy => write!(f, "engine busy"),
            EngineRequestError::Disconnected => write!(f, "engine disconnected"),
            EngineRequestError::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for EngineRequestError {}

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
            let text = link_texts
                .get(i)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            ApiElement {
                id: format!("e{}", i),
                element_type: "link".to_string(),
                text,
                href: Some(href.clone()),
                rect: ApiRect {
                    x: if rect.x.is_finite() { rect.x } else { 0.0 },
                    y: if rect.y.is_finite() { rect.y } else { 0.0 },
                    w: if rect.width.is_finite() {
                        rect.width
                    } else {
                        0.0
                    },
                    h: if rect.height.is_finite() {
                        rect.height
                    } else {
                        0.0
                    },
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
                    w: if rect.width.is_finite() {
                        rect.width
                    } else {
                        0.0
                    },
                    h: if rect.height.is_finite() {
                        rect.height
                    } else {
                        0.0
                    },
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

/// Resolve user input to a navigable URL.
/// - URLs with scheme → use as-is
/// - Domain-like input (contains dots, no spaces) → prepend `https://`
/// - Everything else → Google search query
pub fn resolve_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_string();
    }
    if trimmed.contains('.') && !trimmed.contains(' ') {
        return format!("https://{}", trimmed);
    }
    let encoded = url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect::<String>();
    format!("https://www.google.com/search?q={}", encoded)
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
                if matches!(
                    tag_lower,
                    "p" | "br"
                        | "/p"
                        | "div"
                        | "/div"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "/h1"
                        | "/h2"
                        | "/h3"
                        | "/h4"
                        | "/h5"
                        | "/h6"
                        | "li"
                        | "/li"
                        | "tr"
                        | "/tr"
                ) {
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
                let (verb, _) = tag_lower_trimmed
                    .split_once(' ')
                    .unwrap_or((tag_lower_trimmed, ""));
                if matches!(verb, "input" | "textarea" | "select") {
                    // Skip hidden inputs — they are excluded from the layout tree and
                    // must not be included here to keep the index-based zip in sync.
                    let input_type = extract_attr(&tag_str, "type")
                        .unwrap_or_default()
                        .to_lowercase();
                    if input_type == "hidden" {
                        // skip
                    } else {
                        let name = extract_attr(&tag_str, "name").unwrap_or_default();
                        results.push((name, verb.to_string()));
                    }
                }
            }
        } else if in_tag {
            tag_buf.push(ch);
        }
    }

    results
}

/// Recursively extract visible text content from a DOM node's children.
fn extract_text_content(node: &markup5ever_rcdom::Node, out: &mut String) {
    for child in node.children.borrow().iter() {
        match &child.data {
            markup5ever_rcdom::NodeData::Text { contents } => {
                let text = contents.borrow();
                if !text.trim().is_empty() {
                    out.push_str(text.trim());
                }
            }
            markup5ever_rcdom::NodeData::Element { .. } => {
                extract_text_content(child, out);
            }
            _ => {}
        }
    }
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
                debug_assert_eq!(
                    lc, lc_from_lower,
                    "to_lowercase mismatch: tag_ch={:?} expanded to {:?} but lower had {:?}",
                    tag_ch, lc, lc_from_lower
                );
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
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(rest.len());
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
    if let markup5ever_rcdom::NodeData::Element {
        ref name,
        ref attrs,
        ..
    } = handle.data
    {
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
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
    width: f32,
    viewport_height: f32,
) -> Result<(PageResult, css::Stylesheet), Box<dyn std::error::Error + Send + Sync>> {
    let response = reqwest::blocking::get(url_str)?;
    let base_url = response.url().clone();
    let csp_header = response
        .headers()
        .get("content-security-policy")
        .and_then(|h| h.to_str().ok())
        .map(|s| js::CspPolicy::parse(s));

    let body = response.text()?;
    process_html_with_scroll(
        &body,
        &base_url,
        &HashMap::new(),
        css_cache,
        None,
        hovered_id,
        focused_id,
        csp_header,
        width,
        viewport_height,
        (0.0, 0.0),
    )
}

/// Viewport height in CSS px when none is configured (`browser-daemon
/// --viewport-height` overrides it). Used for `vh` units, fixed-position
/// boxes, `window.innerHeight` and viewport screen captures.
pub const DEFAULT_VIEWPORT_HEIGHT: f32 = 768.0;

/// Run the full pipeline on pre-fetched HTML, returning a `PageResult`.
/// The viewport is `width` x `DEFAULT_VIEWPORT_HEIGHT`.
#[allow(clippy::too_many_arguments)]
pub fn process_html_with_cache(
    body: &str,
    base_url: &Url,
    image_cache: &HashMap<String, Vec<u8>>,
    css_cache: &mut HashMap<String, String>,
    cached_stylesheet: Option<css::Stylesheet>,
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
    csp_policy: Option<js::CspPolicy>,
    width: f32,
) -> Result<(PageResult, css::Stylesheet), Box<dyn std::error::Error + Send + Sync>> {
    process_html_with_scroll(
        body,
        base_url,
        image_cache,
        css_cache,
        cached_stylesheet,
        hovered_id,
        focused_id,
        csp_policy,
        width,
        DEFAULT_VIEWPORT_HEIGHT,
        (0.0, 0.0),
    )
}

/// `process_html_with_cache` for a `width` x `viewport_height` viewport
/// scrolled to `scroll` (x, y): `position: fixed` boxes are moved to the
/// viewport at that offset.
#[allow(clippy::too_many_arguments)]
pub fn process_html_with_scroll(
    body: &str,
    base_url: &Url,
    image_cache: &HashMap<String, Vec<u8>>,
    css_cache: &mut HashMap<String, String>,
    cached_stylesheet: Option<css::Stylesheet>,
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
    csp_policy: Option<js::CspPolicy>,
    width: f32,
    viewport_height: f32,
    scroll: (f32, f32),
) -> Result<(PageResult, css::Stylesheet), Box<dyn std::error::Error + Send + Sync>> {
    let start_total = Instant::now();
    let width = width.max(1.0);
    css::set_media_viewport(width, viewport_height);

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
        let fetched_contents: Vec<(String, Option<String>)> = sources
            .into_par_iter()
            .map(|src| match src {
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
                            // Resolve relative url() against the stylesheet, not the document.
                            (crate::background::absolutize_css_urls(&text, &url), Some(url))
                        }
                        Err(e) => {
                            println!("[Error] Parallel Fetch (CSS): {} failed: {}", url, e);
                            (String::new(), None)
                        }
                    }
                }
            })
            .collect();

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
    // Inline styles written by script live in the DOM's style attributes;
    // style.rs still takes a separate override map, which is always empty.
    let style_tree = style::build_style_tree(
        &dom_tree.document,
        &stylesheet,
        None,
        &HashMap::new(),
        hovered_id,
        focused_id,
        csp_policy.as_ref(),
    );
    let style_elapsed = start.elapsed();

    let start = Instant::now();
    let image_sizes = layout::ImageSizes::from_cache(image_cache, Some(base_url));
    let (layout_tree_opt, _, final_y) = layout::build_layout_tree_with_images(
        &style_tree, 0.0, 0.0, width, width, viewport_height, None, image_sizes,
    );
    let mut layout_tree = layout_tree_opt.ok_or("Failed to build layout tree")?;
    let layout_elapsed = start.elapsed();

    let (scroll_x, scroll_y) = scroll;
    if scroll_x != 0.0 || scroll_y != 0.0 {
        shift_fixed_boxes(&mut layout_tree, scroll_x, scroll_y);
    }
    let (scroll_width, scroll_height) = document_scroll_extent(&layout_tree, width, final_y);

    let height = (final_y.ceil() as u32).clamp(600, 16384);
    let w_u32 = width as u32;

    let start = Instant::now();
    let mut pixmap = tiny_skia::Pixmap::new(w_u32, height)
        .ok_or_else(|| format!("Failed to create pixmap with size {}x{}", w_u32, height))?;

    // A frame document without a background shows its parent through it.
    if !frames::is_frame_thread() {
        pixmap.fill(tiny_skia::Color::WHITE);
    }

    let mut links: Vec<(layout::Rect, String)> = Vec::new();
    let mut form_controls = Vec::new();
    let mut form_buttons = Vec::new();
    let mut form_control_names = Vec::new();
    let mut event_handlers = Vec::new();
    let mut element_ids = Vec::new();
    let mut focusable_elements = Vec::new();
    let mut layout_metrics = HashMap::new();
    let mut image_urls: Vec<String>;

    render::render_layout_tree(&layout_tree, &mut pixmap, image_cache, &base_url);

    layout_tree.collect_links(&mut links);
    layout_tree.collect_event_handlers(&mut event_handlers);
    layout_tree.collect_element_ids(&mut element_ids);
    layout_tree.collect_focusable_elements(&mut focusable_elements);
    collect_layout_metrics(&layout_tree, &mut layout_metrics);

    let mut controls_with_nodes = Vec::new();
    layout_tree.collect_form_controls(&mut controls_with_nodes);

    let mut image_urls_raw = Vec::new();
    layout_tree.collect_images(&mut image_urls_raw);
    image_urls = image_urls_raw
        .into_iter()
        .map(|(_, url)| base_url.join(&url).map(|u| u.to_string()).unwrap_or(url))
        .collect();
    crate::background::collect_background_image_urls(&layout_tree, base_url, &mut image_urls);
    let frame_boxes = frames::collect_frame_boxes(&layout_tree, base_url);

    let (form_action, form_method) = layout_tree
        .collect_form_element()
        .unwrap_or_else(|| (String::new(), String::from("get")));
    let mut form_control_metas = Vec::new();

    for (rect, node) in controls_with_nodes {
        let mut val = String::new();
        let mut name = String::new();
        let mut input_type = String::from("text");
        let mut tag = String::new();
        if let markup5ever_rcdom::NodeData::Element { ref attrs, .. } = node.node.data {
            // Determine the tag name — access `name` field via the element data.
            if let markup5ever_rcdom::NodeData::Element { ref name, .. } = node.node.data {
                tag = name.local.to_string();
            }
            for attr in attrs.borrow().iter() {
                let attr_name = attr.name.local.to_string();
                match attr_name.as_str() {
                    "value" if tag == "input" => val = attr.value.to_string(),
                    "name" => name = attr.value.to_string(),
                    "type" => input_type = attr.value.to_string().to_lowercase(),
                    _ => {}
                }
            }
            // For <button>, extract text content from child text nodes
            if tag == "button" {
                val.clear();
                extract_text_content(&node.node, &mut val);
                if input_type.is_empty() || input_type == "text" {
                    input_type = String::from("submit");
                }
            }
        }
        if matches!(input_type.as_str(), "submit" | "button" | "reset") {
            form_buttons.push((rect, val));
        } else {
            form_control_metas.push(FormControlMeta {
                name: name.clone(),
                rect,
                initial_value: val.clone(),
            });
            form_controls.push((rect, val.clone()));
            form_control_names.push(name);
        }
    }

    let form_metadata = if form_action.is_empty() && form_control_metas.is_empty() {
        None
    } else {
        Some(FormMetadata {
            action: form_action,
            method: form_method,
            controls: form_control_metas,
        })
    };

    let render_elapsed = start.elapsed();

    let start = Instant::now();
    let absolute_links = links
        .into_iter()
        .map(|(rect, link)| {
            let abs = base_url.join(&link).map(|u| u.to_string()).unwrap_or(link);
            (rect, abs)
        })
        .collect();

    let pixmap_bytes = pixmap.take();
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
            form_buttons,
            form_control_names,
            event_handlers,
            element_ids,
            focusable_elements,
            image_urls,
            layout_metrics,
            body: body.to_string(),
            base_url: base_url.clone(),
            csp_policy,
            form_metadata,
            scroll_width,
            scroll_height,
            scroll_x,
            scroll_y,
            frames: frame_boxes,
        },
        stylesheet,
    ))
}

/// Paint the `width` x `viewport_height` viewport of `body` scrolled to
/// `scroll`: the document is laid out as for a full render, then moved by
/// `-scroll` so only the visible rectangle is rasterized.
#[allow(clippy::too_many_arguments)]
fn render_viewport(
    body: &str,
    base_url: &Url,
    image_cache: &HashMap<String, Vec<u8>>,
    stylesheet: &css::Stylesheet,
    focused_id: Option<&str>,
    csp_policy: Option<&js::CspPolicy>,
    width: f32,
    viewport_height: f32,
    scroll: (f32, f32),
) -> Option<tiny_skia::Pixmap> {
    let dom_tree = dom::parse_html(body);
    let style_tree = style::build_style_tree(
        &dom_tree.document,
        stylesheet,
        None,
        &HashMap::new(),
        None,
        focused_id,
        csp_policy,
    );
    let image_sizes = layout::ImageSizes::from_cache(image_cache, Some(base_url));
    let (layout_tree, _, _) = layout::build_layout_tree_with_images(
        &style_tree, 0.0, 0.0, width, width, viewport_height, None, image_sizes,
    );
    let mut layout_tree = layout_tree?;
    let (scroll_x, scroll_y) = scroll;
    shift_fixed_boxes(&mut layout_tree, scroll_x, scroll_y);
    translate_subtree(&mut layout_tree, -scroll_x, -scroll_y);
    let mut pixmap = tiny_skia::Pixmap::new(
        width.max(1.0) as u32,
        viewport_height.round().max(1.0) as u32,
    )?;
    pixmap.fill(tiny_skia::Color::WHITE);
    render::render_layout_tree(&layout_tree, &mut pixmap, image_cache, base_url);
    Some(pixmap)
}

/// Move every `position: fixed` box (with its subtree) by `(dx, dy)`.
///
/// Layout places fixed boxes against the viewport at the document origin;
/// with the document scrolled, the viewport sits at the scroll offset.
fn shift_fixed_boxes(root: &mut layout::LayoutBox, dx: f32, dy: f32) {
    let mut stack: Vec<&mut layout::LayoutBox> = vec![root];
    while let Some(node) = stack.pop() {
        if node.position == layout::PositionType::Fixed {
            translate_subtree(node, dx, dy);
            continue;
        }
        stack.extend(node.children.iter_mut());
    }
}

/// Move `root` and every descendant box by `(dx, dy)`. Same job as
/// `layout::offset_layout_box`, without `unsafe`; one of the two goes when
/// that function is made safe.
fn translate_subtree(root: &mut layout::LayoutBox, dx: f32, dy: f32) {
    let mut stack: Vec<&mut layout::LayoutBox> = vec![root];
    while let Some(node) = stack.pop() {
        node.dimensions.x += dx;
        node.dimensions.y += dy;
        stack.extend(node.children.iter_mut());
    }
}

/// Whether `overflow` (or the per-axis longhand) clips on the given axis.
fn overflow_clips(layout: &layout::LayoutBox, axis_prop: &str) -> bool {
    ["overflow", axis_prop].iter().any(|prop| {
        matches!(
            layout.style_node.specified_values.get(&css::intern(prop)),
            Some(css::Value::Keyword(k)) if matches!(k.as_ref(), "hidden" | "clip" | "auto" | "scroll")
        )
    })
}

/// `opacity: 0`: the box and its subtree paint nothing.
fn is_transparent(layout: &layout::LayoutBox) -> bool {
    matches!(
        layout.style_node.specified_values.get(&css::intern("opacity")),
        Some(css::Value::Number(n)) if *n <= 0.0
    )
}

/// `visibility: hidden` (or `collapse`) on the box itself; descendants may
/// still be visible.
fn is_visibility_hidden(layout: &layout::LayoutBox) -> bool {
    matches!(
        layout.style_node.specified_values.get(&css::intern("visibility")),
        Some(css::Value::Keyword(k)) if matches!(k.as_ref(), "hidden" | "collapse")
    )
}

fn element_tag(layout: &layout::LayoutBox) -> Option<String> {
    match &layout.style_node.node.data {
        markup5ever_rcdom::NodeData::Element { name, .. } => Some(name.local.to_string()),
        _ => None,
    }
}

/// Scrollable size of the document: the union of border boxes that are not
/// fixed, not inside a clipping (`overflow` other than visible) box, not
/// `visibility: hidden` and not inside an `opacity: 0` subtree, but at least
/// `viewport_width` x `min_height`. An `overflow` clip on `<html>` or
/// `<body>` applies to the viewport and disables scrolling on that axis.
fn document_scroll_extent(root: &layout::LayoutBox, viewport_width: f32, min_height: f32) -> (f32, f32) {
    let mut right = viewport_width;
    let mut bottom = min_height;
    let mut clip_x = false;
    let mut clip_y = false;
    let mut stack: Vec<&layout::LayoutBox> = vec![root];
    while let Some(node) = stack.pop() {
        if node.position == layout::PositionType::Fixed || is_transparent(node) {
            continue;
        }
        let d = &node.dimensions;
        if d.width > 0.0 && d.height > 0.0 && !is_visibility_hidden(node) {
            right = right.max(d.x + d.width);
            bottom = bottom.max(d.y + d.height);
        }
        let root_element = matches!(element_tag(node).as_deref(), Some("html" | "body"));
        if root_element {
            clip_x |= overflow_clips(node, "overflow-x");
            clip_y |= overflow_clips(node, "overflow-y");
        } else if overflow_clips(node, "overflow-x") || overflow_clips(node, "overflow-y") {
            continue;
        }
        stack.extend(node.children.iter());
    }
    (
        if clip_x { viewport_width } else { right },
        if clip_y { min_height } else { bottom },
    )
}

fn fetch_text_with_timeout(url: &Url) -> Result<String, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(url.as_str())
        .send()?
        .text()
}

fn collect_layout_metrics(
    layout_tree: &layout::LayoutBox,
    out: &mut HashMap<String, js::LayoutMetrics>,
) {
    // Metrics feed getBoundingClientRect, so they include CSS transforms of
    // the box and its ancestors (as the axis-aligned bounds of the
    // transformed border box), matching what is painted.
    let mut stack = vec![(layout_tree, Affine::IDENTITY)];
    while let Some((layout, parent)) = stack.pop() {
        let d = layout.dimensions;
        let transform = match box_transform(layout) {
            Some(own) => parent.then(&own),
            None => parent,
        };
        if matches!(
            layout.style_node.node.data,
            markup5ever_rcdom::NodeData::Element { .. }
        ) {
            let (x, y, width, height) = transform.bounds(d.x, d.y, d.width, d.height);
            out.insert(
                js::node_path_key(&layout.style_node.node),
                js::LayoutMetrics { x, y, width, height },
            );
        }
        for child in layout.children.iter().rev() {
            stack.push((child, transform));
        }
    }
}

/// 2D affine map `(x, y) -> (a*x + c*y + e, b*x + d*y + f)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Affine {
    const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    fn translate(x: f32, y: f32) -> Affine {
        Affine { e: x, f: y, ..Affine::IDENTITY }
    }

    /// `self` applied after `inner`: `self.then(inner)(p) = self(inner(p))`.
    fn then(&self, inner: &Affine) -> Affine {
        Affine {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            e: self.a * inner.e + self.c * inner.f + self.e,
            f: self.b * inner.e + self.d * inner.f + self.f,
        }
    }

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// Axis-aligned bounds `(x, y, width, height)` of the mapped rectangle.
    fn bounds(&self, x: f32, y: f32, width: f32, height: f32) -> (f32, f32, f32, f32) {
        if *self == Affine::IDENTITY {
            return (x, y, width, height);
        }
        let corners = [
            self.apply(x, y),
            self.apply(x + width, y),
            self.apply(x, y + height),
            self.apply(x + width, y + height),
        ];
        let min_x = corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let min_y = corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let max_x = corners.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
        let max_y = corners.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        (min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

/// The box's own CSS `transform` in document coordinates, about the default
/// `transform-origin` (the border box center).
fn box_transform(layout: &layout::LayoutBox) -> Option<Affine> {
    let Some(css::Value::Transform(ops)) = layout
        .style_node
        .specified_values
        .get(&css::intern("transform"))
    else {
        return None;
    };
    if ops.is_empty() {
        return None;
    }
    let d = layout.dimensions;
    let mut m = Affine::IDENTITY;
    for op in ops {
        let step = match op {
            css::TransformOp::Translate(x, y) => {
                Affine::translate(x.resolve(d.width), y.resolve(d.height))
            }
            css::TransformOp::Scale(x, y) => Affine { a: x.0, d: y.0, ..Affine::IDENTITY },
            css::TransformOp::Rotate(rad) => {
                let (sin, cos) = rad.0.sin_cos();
                Affine { a: cos, b: sin, c: -sin, d: cos, e: 0.0, f: 0.0 }
            }
            css::TransformOp::Matrix(a, b, c, dd, e, f) => Affine {
                a: a.0,
                b: b.0,
                c: c.0,
                d: dd.0,
                e: e.0,
                f: f.0,
            },
        };
        m = m.then(&step);
    }
    let (ox, oy) = (d.x + d.width / 2.0, d.y + d.height / 2.0);
    Some(
        Affine::translate(ox, oy)
            .then(&m)
            .then(&Affine::translate(-ox, -oy)),
    )
}

// ── BrowserEngine ─────────────────────────────────────────────────────────────

/// A headless browser engine with no GUI dependencies.
/// Owns all pipeline state: caches, JS runtime, and last rendered page.
pub struct BrowserEngine {
    pub image_cache: HashMap<String, Vec<u8>>,
    pub css_cache: HashMap<String, String>,
    pub last_stylesheet: Option<css::Stylesheet>,
    /// Viewport (width, height) `last_stylesheet` was parsed at; `@media`
    /// rules depend on it, so a different size re-parses the sheet.
    stylesheet_viewport: (f32, f32),
    pub js_runtime: js::JsRuntime,
    pub console_buffer: js::ConsoleBuffer,
    pub current_csp_policy: Option<js::CspPolicy>,
    /// The most recently rendered page result.
    pub last_page: Option<PageResult>,
    /// Child engines rendering this document's `<iframe>` documents.
    pub frames: frames::FrameHost,
    /// Set when this engine renders a frame document: its embedder.
    frame_parent: Option<frames::FrameParent>,
    /// Latest time a navigation of this (frame) engine waits for its own
    /// frames; `None` means `frames::FRAME_BUDGET` from the navigation start.
    pub frame_deadline: Option<Instant>,
    /// `iframe.contentWindow.postMessage` calls whose frame has no child yet.
    undelivered_child_messages: Vec<frames::OutgoingMessage>,
    /// `window.parent.postMessage` calls not yet taken by the frame thread.
    parent_outbox: Vec<frames::OutgoingMessage>,
    /// Session history of top-level documents.
    pub history: SharedHistory,
    /// Viewport height in CSS px; the width is passed per render.
    pub viewport_height: f32,
}

impl BrowserEngine {
    /// Create a new engine with empty caches and a fresh JS runtime.
    pub fn new_with_console(console_buffer: js::ConsoleBuffer) -> Self {
        Self {
            image_cache: HashMap::new(),
            css_cache: HashMap::new(),
            last_stylesheet: None,
            stylesheet_viewport: (0.0, 0.0),
            js_runtime: js::JsRuntime::new(None, None, None, None, console_buffer.clone()),
            console_buffer,
            current_csp_policy: None,
            last_page: None,
            frames: frames::FrameHost::new_top_level(),
            frame_parent: None,
            frame_deadline: None,
            undelivered_child_messages: Vec::new(),
            parent_outbox: Vec::new(),
            history: SharedHistory::default(),
            viewport_height: DEFAULT_VIEWPORT_HEIGHT,
        }
    }

    /// An engine for a frame document embedded by `parent`, whose own frames
    /// run under `frames`. Must be created on the frame's thread.
    pub fn new_frame(frames: frames::FrameHost, parent: frames::FrameParent) -> Self {
        let mut engine = Self::new();
        engine.frames = frames;
        engine.frame_parent = Some(parent);
        engine
    }

    pub fn new() -> Self {
        Self::new_with_console(js::new_console_buffer())
    }

    /// Synchronously navigate to a URL and add it to session history.
    /// Follows navigations the page's scripts start while it loads.
    pub fn navigate(&mut self, url_str: &str, width: f32) -> Result<PageResult, String> {
        let page = self.load_document(url_str, width)?;
        self.record_history(&page, false);
        Ok(self.run_script_navigation(width, true)?.unwrap_or(page))
    }

    /// Load the session history entry `delta` steps from the current one
    /// (`-1` is back, `1` is forward). `Ok(None)` when there is no such entry.
    pub fn traverse_history(
        &mut self,
        delta: isize,
        width: f32,
    ) -> Result<Option<PageResult>, String> {
        let target = self.lock_history().entry_at(delta);
        let Some((index, url)) = target else {
            return Ok(None);
        };
        self.lock_history().index = index;
        let page = self.load_document(&url, width)?;
        self.record_history(&page, true);
        Ok(Some(self.run_script_navigation(width, true)?.unwrap_or(page)))
    }

    /// Load the document page script asked for, if any, and the ones that
    /// document's scripts ask for in turn (at most `MAX_SCRIPT_NAVIGATIONS`).
    /// A navigation started while a document loads replaces its history
    /// entry, as Chromium does for client redirects. `Ok(None)` when no
    /// script asked to navigate.
    pub fn run_script_navigation(
        &mut self,
        width: f32,
        during_load: bool,
    ) -> Result<Option<PageResult>, String> {
        let mut replace = during_load;
        let mut loaded = None;
        for _ in 0..MAX_SCRIPT_NAVIGATIONS {
            let Some(request) = self.js_runtime.take_navigation_request() else {
                return Ok(loaded);
            };
            let page = self.load_document(&request.url, width)?;
            self.record_history(&page, replace || request.replace);
            replace = true;
            loaded = Some(page);
        }
        if let Some(request) = self.js_runtime.take_navigation_request() {
            eprintln!("[navigate] dropped script navigation to {} after {} redirects", request.url, MAX_SCRIPT_NAVIGATIONS);
        }
        Ok(loaded)
    }

    /// Width the current page was laid out at, for navigations that page
    /// script starts.
    fn current_width(&self) -> f32 {
        self.last_page.as_ref().map_or(800.0, |page| page.width as f32)
    }

    fn lock_history(&self) -> std::sync::MutexGuard<'_, SessionHistory> {
        self.history.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn record_history(&self, page: &PageResult, replace: bool) {
        let url = page.base_url.to_string();
        let mut history = self.lock_history();
        if replace {
            history.replace(url);
        } else {
            history.push(url);
        }
    }

    /// Fetch `url_str`, run its scripts and render it. Session history is
    /// left to the caller.
    fn load_document(&mut self, url_str: &str, width: f32) -> Result<PageResult, String> {
        self.clear_for_new_url();
        let result = fetch_and_process(
            url_str,
            &mut self.css_cache,
            None,
            None,
            width,
            self.viewport_height,
        )
        .map_err(|e| e.to_string())?;

        let (page, stylesheet) = result;
        self.last_stylesheet = Some(stylesheet);
        self.stylesheet_viewport = (width.max(1.0), self.viewport_height);
        self.current_csp_policy = page.csp_policy.clone();
        self.last_page = Some(page.clone());
        self.init_js_for_page(&page);

        // Drain macro-tasks and requestAnimationFrame callbacks that scripts
        // (e.g. React 18) registered during init_js_for_page.  Without this
        // the RAF queue is never flushed before we read the live DOM, so SPA
        // frameworks that rely on rAF for their first render produce an empty
        // body.  Cap at 10 ticks to avoid infinite render-loop drain.
        for _ in 0..10 {
            if !self.tick_js(Some(0.0), None) {
                break;
            }
        }

        // After JS runs and may have modified the DOM (e.g. React/Vue rendering
        // into #root), serialize the live DOM and re-render using post-JS HTML.
        let js_modified = if let Some(live_html) = self.js_runtime.get_document_html() {
            if let Some(ref mut last) = self.last_page {
                last.body = live_html;
            }
            true
        } else {
            false
        };

        // If JS modified the DOM or images need loading, force a re-render.
        let post_js_page = if js_modified || self.cache_missing_images(&page.image_urls) {
            self.re_render(None, None, width)?
        } else {
            self.frames.sync(&page.frames, &self.document_origin());
            page
        };
        self.settle_frames(post_js_page, width)
    }

    /// Wait (within the frame budget) for the page's frames to load, relaying
    /// their messages and re-rendering as they report, so the page returned
    /// by `navigate` shows its frames.
    fn settle_frames(&mut self, mut page: PageResult, width: f32) -> Result<PageResult, String> {
        let budget_end = Instant::now() + frames::FRAME_BUDGET;
        let deadline = self.frame_deadline.map_or(budget_end, |d| d.min(budget_end));
        while self.frames.has_pending() && Instant::now() < deadline {
            self.frames.wait_until(deadline);
            let mut dirty = self.frames.has_ready_images();
            for _ in 0..5 {
                if !self.tick_js(None, None) {
                    break;
                }
                dirty = true;
            }
            if dirty {
                page = self.re_render(None, None, width)?;
            }
        }
        Ok(page)
    }

    /// Origin of the current document (`"null"` for opaque origins).
    pub fn document_origin(&self) -> String {
        self.last_page
            .as_ref()
            .map(|p| p.base_url.origin().ascii_serialization())
            .unwrap_or_else(|| "null".to_string())
    }

    /// Move `postMessage` calls queued by page script out of the JS runtime:
    /// messages to child frames go to their engines, messages to the parent
    /// wait in `parent_outbox` for the frame thread.
    fn drain_js_outbox(&mut self) {
        let outcome = self.js_runtime.execute_with_result(
            "typeof __aura_frame_take_outbox === 'function' ? __aura_frame_take_outbox() : '[]'",
        );
        let queued: Vec<frames::OutgoingMessage> = outcome
            .result
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let origin = self.document_origin();
        let mut to_children = std::mem::take(&mut self.undelivered_child_messages);
        for message in queued {
            match message.to.as_str() {
                "parent" if self.frame_parent.is_some() => self.parent_outbox.push(message),
                "child" => to_children.push(message),
                _ => {}
            }
        }
        for message in to_children {
            let Some(src) = message.src.clone() else { continue };
            let child_origin = Url::parse(&src)
                .map(|u| u.origin().ascii_serialization())
                .unwrap_or_default();
            if !frames::target_origin_allows(&message.target_origin, &child_origin) {
                continue;
            }
            if !self.frames.post_to_child(&src, message.ordinal, message.data.clone(), &origin)
                && self.undelivered_child_messages.len() < 64
            {
                self.undelivered_child_messages.push(message);
            }
        }
    }

    /// `window.parent.postMessage` calls made by this frame document since
    /// the last call.
    pub fn take_parent_messages(&mut self) -> Vec<frames::OutgoingMessage> {
        self.drain_js_outbox();
        std::mem::take(&mut self.parent_outbox)
    }

    /// Dispatch a `message` event from the parent document on this frame
    /// document's window (`event.source === window.parent`).
    pub fn deliver_parent_message(&mut self, data: &str, origin: &str) {
        self.dispatch_message_event(None, 0, data, origin);
    }

    fn dispatch_message_event(&mut self, src: Option<&str>, ordinal: usize, data: &str, origin: &str) {
        let args = serde_json::json!([src, ordinal, data, origin]);
        self.js_runtime.execute(&format!(
            "if (typeof __aura_frame_receive_message === 'function') __aura_frame_receive_message.apply(null, {args});"
        ));
    }

    /// Dispatch the messages child frames posted to this document.
    fn deliver_child_messages(&mut self) -> bool {
        let messages = self.frames.take_messages();
        let delivered = !messages.is_empty();
        for message in messages {
            self.dispatch_message_event(Some(&message.src), message.ordinal, &message.data, &message.origin);
        }
        delivered
    }

    /// Re-render the current page (e.g. after JS style changes or hover state).
    pub fn re_render(
        &mut self,
        hovered_id: Option<&str>,
        focused_id: Option<&str>,
        width: f32,
    ) -> Result<PageResult, String> {
        // Pick up DOM mutations made by JS since the last render (timers,
        // requestAnimationFrame, event handlers) so re-renders are not stale.
        if let Some(live_html) = self.js_runtime.get_document_html() {
            if let Some(ref mut last) = self.last_page {
                last.body = live_html;
            }
        }
        let (body, base_url) = match &self.last_page {
            Some(p) => (p.body.clone(), p.base_url.clone()),
            None => return Err("No page loaded".into()),
        };
        // Without an explicit focus from the UI, honour element.focus() calls
        // made by page scripts so :focus styles match the live document.
        let js_focused_id = self.js_runtime.get_focused_node_id();
        let focused_id = focused_id.or(js_focused_id.as_deref());

        let mut css_cache = self.css_cache.clone();
        let result = process_html_with_scroll(
            &body,
            &base_url,
            &self.image_cache,
            &mut css_cache,
            // @media rules were resolved against the size the sheet was parsed at.
            self.last_stylesheet.clone().filter(|_| self.stylesheet_viewport == (width, self.viewport_height)),
            hovered_id,
            focused_id,
            self.current_csp_policy.clone(),
            width,
            self.viewport_height,
            self.js_runtime.scroll_position(),
        )
        .map_err(|e| e.to_string())?;

        self.css_cache = css_cache;
        let (page, stylesheet) = result;
        self.last_stylesheet = Some(stylesheet);
        self.stylesheet_viewport = (width.max(1.0), self.viewport_height);
        self.last_page = Some(page.clone());
        self.js_runtime
            .set_layout_metrics(page.layout_metrics.clone());
        self.js_runtime.set_scroll_extent(
            page.scroll_width,
            page.scroll_height,
            page.width as f32,
            self.viewport_height,
        );
        self.frames.sync(&page.frames, &base_url.origin().ascii_serialization());
        self.drain_js_outbox();
        self.refresh_after_image_loads(page, width)
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
                self.js_runtime.set_focused_node_id(Some(id.clone()));
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

        // A handler that set `location.href` or submitted a form replaces
        // the link's own navigation.
        match self.run_script_navigation(page.width as f32, false) {
            Ok(Some(loaded)) => {
                results.push(ClickResult::Navigated {
                    url: loaded.base_url.to_string(),
                });
                return results;
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("[navigate] script navigation failed: {}", error);
                return results;
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

    /// Inject text into the currently focused text-like form control.
    pub fn type_text(&mut self, text: &str) {
        let Some(id) = self.js_runtime.get_focused_node_id() else {
            return;
        };
        let id_json = serde_json::to_string(&id).unwrap_or_else(|_| "\"\"".to_string());
        let text_json = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
        let code = format!(
            r#"
            (function() {{
                var el = document.getElementById({id_json});
                var text = {text_json};
                if (!el || el.disabled) return;
                var tag = el.tagName;
                var type = (el.type || '').toLowerCase();
                if (tag === 'TEXTAREA' || tag === 'INPUT') {{
                    if (type === 'checkbox' || type === 'radio' || type === 'button' || type === 'submit' || type === 'reset') return;
                    el.value = (el.value || '') + text;
                    el.dispatchEvent(new InputEvent('input', {{ bubbles: true, data: text, inputType: 'insertText' }}));
                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                }}
            }})();
            "#
        );
        self.js_runtime.execute(&code);
    }

    fn cache_missing_images_with<F>(&mut self, image_urls: &[String], mut fetch: F) -> bool
    where
        F: FnMut(&str) -> Result<Vec<u8>, String>,
    {
        let mut loaded_any = false;
        for url in image_urls {
            if self.image_cache.contains_key(url) {
                continue;
            }
            if let Ok(bytes) = fetch(url) {
                self.image_cache.insert(url.clone(), bytes);
                loaded_any = true;
            }
        }
        loaded_any
    }

    fn cache_missing_images(&mut self, image_urls: &[String]) -> bool {
        self.cache_missing_images_with(image_urls, |url| {
            let response = reqwest::blocking::get(url).map_err(|e| e.to_string())?;
            let bytes = response.bytes().map_err(|e| e.to_string())?;
            Ok(bytes.to_vec())
        })
    }

    fn refresh_after_image_loads(
        &mut self,
        page: PageResult,
        width: f32,
    ) -> Result<PageResult, String> {
        let loaded_images = self.cache_missing_images(&page.image_urls);
        self.frames.poll();
        let loaded_frames = self.frames.store_images(&mut self.image_cache);
        if !loaded_images && !loaded_frames {
            return Ok(page);
        }

        let refreshed = self.re_render(None, None, width)?;
        Ok(refreshed)
    }

    /// Execute JavaScript in the current page's runtime and return a result/error.
    pub fn evaluate_js_with_result(&mut self, script: &str) -> js::EvalOutcome {
        self.js_runtime.execute_with_result(script)
    }

    /// Execute JavaScript in the current page's runtime (fire-and-forget).
    pub fn evaluate_js(&mut self, script: &str) -> String {
        let outcome = self.evaluate_js_with_result(script);
        outcome.result.or(outcome.error).unwrap_or_default()
    }

    /// Execute JavaScript from the DevTools console REPL.
    /// Echoes the input as `> code` and the result/error as `< value` in the console buffer.
    pub fn evaluate_console_repl(&mut self, script: &str) -> js::EvalOutcome {
        js::append_console_entry(
            &self.console_buffer,
            js::ConsoleLevel::Log,
            format!("> {}", script),
        );
        let outcome = self.evaluate_js_with_result(script);
        if let Some(error) = &outcome.error {
            js::append_console_entry(
                &self.console_buffer,
                js::ConsoleLevel::Error,
                format!("< {}", error),
            );
        } else if let Some(result) = &outcome.result {
            js::append_console_entry(
                &self.console_buffer,
                js::ConsoleLevel::Log,
                format!("< {}", result),
            );
        }
        outcome
    }

    /// Reconstruct a `Pixmap` from the last rendered page's pixel data.
    /// This is the full page from the document origin, `width` px wide.
    pub fn screenshot(&self) -> Option<tiny_skia::Pixmap> {
        let page = self.last_page.as_ref()?;
        tiny_skia::Pixmap::from_vec(
            page.pixmap_bytes.clone(),
            tiny_skia::IntSize::from_wh(page.width, page.height)?,
        )
    }

    /// Current document scroll offset `(scrollX, scrollY)`.
    pub fn scroll_position(&self) -> (f32, f32) {
        self.js_runtime.scroll_position()
    }

    /// Capture the viewport (`width` x `viewport_height`) at the current
    /// document scroll offset, as Chromium's page screenshot does. Re-renders
    /// first when the page scrolled since the last render, so fixed boxes
    /// sit at the new offset.
    pub fn screenshot_viewport(&mut self) -> Option<tiny_skia::Pixmap> {
        let (sx, sy) = self.scroll_position();
        let (rendered_at, width) = {
            let page = self.last_page.as_ref()?;
            ((page.scroll_x, page.scroll_y), page.width)
        };
        self.frames.poll();
        if rendered_at != (sx, sy) || self.frames.has_ready_images() {
            if let Err(error) = self.re_render(None, None, width as f32) {
                eprintln!("[screenshot] re-render at scroll offset failed: {}", error);
            }
        }
        let page = self.last_page.as_ref()?;
        let (sx, sy) = (page.scroll_x.max(0.0).round(), page.scroll_y.max(0.0).round());
        let vh = self.viewport_height.round().max(1.0) as u32;
        // The full-page render already holds the viewport unless the page is
        // scrolled horizontally or the viewport reaches past its bottom.
        if sx > 0.0 || sy as u32 + vh > page.height {
            if let Some(stylesheet) = &self.last_stylesheet {
                let focused_id = self.js_runtime.get_focused_node_id();
                return render_viewport(
                    &page.body,
                    &page.base_url,
                    &self.image_cache,
                    stylesheet,
                    focused_id.as_deref(),
                    self.current_csp_policy.as_ref(),
                    page.width as f32,
                    vh as f32,
                    (sx, sy),
                );
            }
        }
        let mut bytes = crop_viewport(
            &page.pixmap_bytes,
            page.width,
            page.height,
            0,
            sy as u32,
            page.width,
            vh,
        );
        if frames::is_frame_thread() {
            clear_past_source(&mut bytes, page.width, sy as u32, page.height);
        }
        tiny_skia::Pixmap::from_vec(bytes, tiny_skia::IntSize::from_wh(page.width, vh)?)
    }

    /// The serialized live DOM the last render laid out (the fetched HTML
    /// until page script has run).
    pub fn dom_tree(&self) -> String {
        self.last_page
            .as_ref()
            .map(|p| p.body.clone())
            .unwrap_or_default()
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

    /// Construct a form submission URL from the current page's form metadata
    /// (action/method) and live DOM field values via the JS runtime.
    /// Returns Some(url) to navigate to, or None if no form exists.
    pub fn submit_form(&mut self) -> Option<String> {
        let (base_url, action, control_names) = {
            let page = self.last_page.as_ref()?;
            let meta = page.form_metadata.as_ref()?;
            let base_url = page.base_url.clone();
            let action = meta.action.clone();
            let control_names: Vec<String> = meta
                .controls
                .iter()
                .filter_map(|c| {
                    let name = c.name.trim().to_string();
                    if name.is_empty() {
                        None
                    } else {
                        Some(name)
                    }
                })
                .collect();
            (base_url, action, control_names)
        };

        if control_names.is_empty() {
            return Some(base_url.to_string());
        }

        let action_url = if action.is_empty() {
            base_url.clone()
        } else {
            base_url.join(&action).ok()?
        };

        let mut params: Vec<(String, String)> = Vec::new();
        for name in &control_names {
            let escaped_name = name.replace('\\', "\\\\").replace('\'', "\\'");
            let script = format!(
                "(function(){{ var el=document.querySelector('[name=\"{}\"]'); return el?el.value:''; }})()",
                escaped_name
            );
            let value = self.evaluate_js(&script);
            params.push((name.clone(), value));
        }

        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .finish();

        let mut final_url = action_url;
        final_url.set_query(Some(&query));
        Some(final_url.to_string())
    }

    /// Return the computed style properties for a CSS selector.
    /// Stub — full implementation deferred to a follow-up issue.
    pub fn computed_style(&self, _selector: &str) -> HashMap<String, String> {
        HashMap::new()
    }

    pub fn console_entries(&self) -> Vec<js::ConsoleEntry> {
        js::console_entries(&self.console_buffer)
    }

    pub fn clear_console(&self) {
        js::clear_console_buffer(&self.console_buffer);
    }

    /// Re-initialize the JS runtime for the current page.
    /// Parses the DOM from `body`, runs all page scripts (CSP-gated),
    /// and collects any immediate JS style overrides.
    pub fn init_js_for_page(&mut self, page: &PageResult) {
        let dom = dom::parse_html(&page.body);
        let document = dom.document.clone();
        let base_url = page.base_url.clone();
        let policy = self.current_csp_policy.clone();
        let metrics = page.layout_metrics.clone();
        let console = self.console_buffer.clone();

        drop_js_runtime_before_create(&mut self.js_runtime, || {
            js::JsRuntime::new(
                Some(document),
                Some(base_url),
                policy,
                Some(metrics),
                console,
            )
        });

        let viewport_h = self.viewport_height;
        let viewport_w = page.width;
        self.js_runtime.execute(&format!(
            "window.innerHeight = window.outerHeight = screen.height = screen.availHeight = {viewport_h}; \
             window.innerWidth = window.outerWidth = {viewport_w};"
        ));
        if let Some(parent) = &self.frame_parent {
            let args = serde_json::json!([parent.origin, parent.name]);
            self.js_runtime.execute(&format!("__aura_install_frame_parent.apply(null, {args});"));
        }
        self.js_runtime.set_scroll_extent(
            page.scroll_width,
            page.scroll_height,
            page.width as f32,
            viewport_h,
        );

        // Debug hook: a script run before the page's own scripts.
        if let Ok(path) = std::env::var("BROWSER_DEBUG_PRELUDE") {
            if let Ok(prelude) = std::fs::read_to_string(&path) {
                self.js_runtime.execute(&prelude);
            }
        }

        let scripts = js::extract_script_sources_from_dom(&dom.document, Some(&page.base_url));

        // ── Phase-bucketed collections ──────────────────────────────────────
        let mut deferred_classics: Vec<(String, u32)> = Vec::new();
        let mut async_classics: Vec<(String, u32)> = Vec::new();
        let mut deferred_module_urls: Vec<Url> = Vec::new();
        let mut deferred_module_targets: Vec<(Url, u32)> = Vec::new();
        let mut async_module_urls: Vec<Url> = Vec::new();
        let mut async_module_targets: Vec<(Url, u32)> = Vec::new();

        // ── Helper: fetch external classic script source with CSP check ────
        let fetch_classic_source = |this: &mut Self, url: &Url| -> Option<String> {
            let allowed = this
                .current_csp_policy
                .as_ref()
                .map(|p| p.is_allowed("script-src", url, Some(&page.base_url)))
                .unwrap_or(true);
            if !allowed {
                println!("[CSP] Blocked external script execution: {}", url);
                return None;
            }
            match fetch_text_with_timeout(url) {
                Ok(source) => Some(source),
                Err(err) => {
                    println!("[JS] Failed to load external script {}: {}", url, err);
                    None
                }
            }
        };

        // ── Phase 1: execute synchronous classics, bucket the rest ─────────
        for script in scripts {
            match script {
                js::ScriptSource::InlineClassic { source, is_defer, node_id } => {
                    let allowed = self
                        .current_csp_policy
                        .as_ref()
                        .map(|p| p.allows_inline_script())
                        .unwrap_or(true);
                    if !allowed {
                        println!("[CSP] Blocked inline script execution");
                        continue;
                    }
                    if is_defer {
                        deferred_classics.push((source, node_id));
                    } else {
                        eprintln!("[JS DEBUG] Executing inline classic script (len={})", source.len());
                        self.js_runtime.execute_classic_script(&source, node_id);
                        eprintln!("[JS DEBUG] Inline classic script finished");
                    }
                }
                js::ScriptSource::ExternalClassic {
                    url,
                    is_async,
                    is_defer,
                    node_id,
                } => {
                    eprintln!("[JS DEBUG] Fetching external classic script: {}", url);
                    let source = fetch_classic_source(self, &url);
                    eprintln!("[JS DEBUG] External classic script fetch returned");
                    if let Some(source) = source {
                        if is_async {
                            // async wins over defer per HTML spec
                            async_classics.push((source, node_id));
                        } else if is_defer {
                            deferred_classics.push((source, node_id));
                        } else {
                            eprintln!("[JS DEBUG] Executing external classic script: {}", url);
                            self.js_runtime.execute_classic_script(&source, node_id);
                            eprintln!("[JS DEBUG] External classic script finished: {}", url);
                        }
                    }
                }
                js::ScriptSource::InlineModule {
                    url,
                    source,
                    node_id,
                    is_async,
                } => {
                    let allowed = self
                        .current_csp_policy
                        .as_ref()
                        .map(|p| p.allows_inline_script())
                        .unwrap_or(true);
                    if !allowed {
                        println!("[CSP] Blocked inline module compilation: {}", url);
                        continue;
                    }
                    eprintln!("[JS DEBUG] Compiling inline module: {}", url);
                    let outcome = self.js_runtime.compile_module_source(url.clone(), source);
                    eprintln!("[JS DEBUG] Inline module compiled");
                    if outcome.error.is_some() {
                        let error = outcome.error.as_deref().unwrap_or("unknown");
                        println!("[JS] Failed to compile inline module {}: {}", url, error);
                        js::push_console_entry(
                            js::ConsoleLevel::Error,
                            format!("Failed to compile inline module {url}: {error}"),
                        );
                        // Still track for error events
                        deferred_module_targets.push((url.clone(), node_id));
                        continue;
                    }
                    if is_async {
                        async_module_urls.push(url.clone());
                        async_module_targets.push((url, node_id));
                    } else {
                        deferred_module_urls.push(url.clone());
                        deferred_module_targets.push((url, node_id));
                    }
                }
                js::ScriptSource::ExternalModule {
                    url,
                    node_id,
                    is_async,
                } => {
                    eprintln!("[JS DEBUG] Loading external module: {}", url);
                    let outcome = self.load_external_module_with(
                        url.clone(),
                        &page.base_url,
                        fetch_text_with_timeout,
                    );
                    eprintln!("[JS DEBUG] External module loaded");
                    if let Some(error) = &outcome.error {
                        println!("[JS] Failed to load external module {}: {}", url, error);
                        js::push_console_entry(
                            js::ConsoleLevel::Error,
                            format!("Failed to load external module {url}: {error}"),
                        );
                        deferred_module_targets.push((url.clone(), node_id));
                        continue;
                    }
                    if is_async {
                        async_module_urls.push(url.clone());
                        async_module_targets.push((url, node_id));
                    } else {
                        deferred_module_urls.push(url.clone());
                        deferred_module_targets.push((url, node_id));
                    }
                }
            }
        }

        // ── Phase 2: execute deferred classics, then evaluate deferred modules ──
        for (idx, (script, node_id)) in deferred_classics.iter().enumerate() {
            eprintln!("[JS DEBUG] Executing deferred classic script #{} (len={})", idx, script.len());
            self.js_runtime.execute_classic_script(script, *node_id);
            eprintln!("[JS DEBUG] Deferred classic script #{} finished", idx);
        }

        eprintln!("[JS DEBUG] Resolving deferred module dependencies");
        self.resolve_module_dependencies(&page.base_url, &mut deferred_module_urls);
        eprintln!("[JS DEBUG] Deferred module dependencies resolved");
        if !deferred_module_urls.is_empty() {
            eprintln!("[JS DEBUG] Evaluating deferred module graph (count={})", deferred_module_urls.len());
            let eval_result = self.js_runtime.evaluate_module_graph(&deferred_module_urls);
            eprintln!("[JS DEBUG] Deferred module graph evaluation finished");
            if let Err(errors) = eval_result {
                let failed_urls: Vec<&str> =
                    errors.iter().filter_map(|e| e.split(": ").next()).collect();
                for error in &errors {
                    println!("[JS] Module evaluation error: {error}");
                    js::push_console_entry(js::ConsoleLevel::Error, error.clone());
                }
                for (url, node_id) in &deferred_module_targets {
                    if failed_urls.iter().any(|f| url.as_str() == *f) {
                        self.js_runtime.trigger_event_on_node_id(*node_id, "error");
                    }
                }
            }
            for (_, node_id) in &deferred_module_targets {
                self.js_runtime.trigger_event_on_node_id(*node_id, "load");
            }
        }

        // ── Phase 3: execute async classics, then evaluate async modules ──
        for (idx, (script, node_id)) in async_classics.iter().enumerate() {
            eprintln!("[JS DEBUG] Executing async classic script #{}", idx);
            self.js_runtime.execute_classic_script(script, *node_id);
            eprintln!("[JS DEBUG] Async classic script #{} finished", idx);
        }

        eprintln!("[JS DEBUG] Resolving async module dependencies");
        self.resolve_module_dependencies(&page.base_url, &mut async_module_urls);
        eprintln!("[JS DEBUG] Async module dependencies resolved");
        if !async_module_urls.is_empty() {
            eprintln!("[JS DEBUG] Evaluating async module graph (count={})", async_module_urls.len());
            let eval_result = self.js_runtime.evaluate_module_graph(&async_module_urls);
            eprintln!("[JS DEBUG] Async module graph evaluation finished");
            if let Err(errors) = eval_result {
                let failed_urls: Vec<&str> =
                    errors.iter().filter_map(|e| e.split(": ").next()).collect();
                for error in &errors {
                    println!("[JS] Async module evaluation error: {error}");
                    js::push_console_entry(js::ConsoleLevel::Error, error.clone());
                }
                for (url, node_id) in &async_module_targets {
                    if failed_urls.iter().any(|f| url.as_str() == *f) {
                        self.js_runtime.trigger_event_on_node_id(*node_id, "error");
                    }
                }
            }
            for (_, node_id) in &async_module_targets {
                self.js_runtime.trigger_event_on_node_id(*node_id, "load");
            }
        }

    }

    pub fn load_external_module_with<F, E>(
        &mut self,
        url: Url,
        page_base: &Url,
        fetcher: F,
    ) -> js::ModuleCompileOutcome
    where
        F: FnOnce(&Url) -> Result<String, E>,
        E: ToString,
    {
        let allowed = self
            .current_csp_policy
            .as_ref()
            .map(|p| p.is_allowed("script-src", &url, Some(page_base)))
            .unwrap_or(true);
        if !allowed {
            return js::ModuleCompileOutcome {
                url,
                from_cache: false,
                requests: Vec::new(),
                error: Some("Blocked by script-src CSP".to_string()),
            };
        }

        match fetcher(&url) {
            Ok(source) => self.js_runtime.compile_module_source(url, source),
            Err(err) => js::ModuleCompileOutcome {
                url,
                from_cache: false,
                requests: Vec::new(),
                error: Some(err.to_string()),
            },
        }
    }

    fn resolve_module_dependencies(&mut self, page_base: &Url, root_urls: &mut Vec<Url>) {
        let mut idx = 0;
        while idx < root_urls.len() {
            let url = root_urls[idx].clone();
            let requests: Vec<String> = self
                .js_runtime
                .cached_module_requests(&url)
                .map(|r| r.to_vec())
                .unwrap_or_default();

            for specifier in &requests {
                let resolved = match self.js_runtime.resolve_module_specifier(specifier, &url) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                if !root_urls.contains(&resolved) {
                    let outcome = self.load_external_module_with(
                        resolved.clone(),
                        page_base,
                        fetch_text_with_timeout,
                    );
                    if outcome.error.is_none() {
                        root_urls.push(resolved);
                    } else if let Some(error) = outcome.error {
                        println!("[JS] Failed to load module dependency {resolved}: {error}");
                        js::push_console_entry(
                            js::ConsoleLevel::Error,
                            format!("Failed to load module dependency {resolved}: {error}"),
                        );
                    }
                }
            }

            idx += 1;
        }
    }

    /// Reset all state in preparation for navigating to a new URL.
    pub fn clear_for_new_url(&mut self) {
        self.clear_console();
        let console = self.console_buffer.clone();
        drop_js_runtime_before_create(&mut self.js_runtime, || {
            js::JsRuntime::new(None, None, None, None, console)
        });
        self.current_csp_policy = None;
        self.last_stylesheet = None;
        self.last_page = None;
        self.css_cache.clear();
        self.frames = self.frames.for_new_document();
        self.undelivered_child_messages.clear();
        self.parent_outbox.clear();
        // Decoded raster and SVG images of the previous page.
        crate::background::clear_decoded_image_cache();
        crate::svg::clear_svg_caches();
    }

    /// Advance the JS event loop by one tick.
    /// Returns `true` if a re-render is needed.
    /// Also dispatches messages from child frames and reports finished frame
    /// renders as work, so the caller re-renders to show them.
    pub fn tick_js(&mut self, timestamp: Option<f64>, deadline: Option<f64>) -> bool {
        self.frames.poll();
        let delivered = self.deliver_child_messages();
        let worked = self
            .js_runtime
            .tick(Some(timestamp.unwrap_or(0.0)), deadline);
        self.drain_js_outbox();
        worked || delivered || self.frames.has_ready_images()
    }
}

#[inline]
fn drop_js_runtime_before_create(
    slot: &mut js::JsRuntime,
    factory: impl FnOnce() -> js::JsRuntime,
) {
    // SAFETY: We read the old value, drop it, then write a new value.
    // This satisfies V8's requirement that OwnedIsolate instances must be
    // dropped in reverse creation order. If we used normal assignment
    // (slot = factory()), the RHS would create a NEW isolate before the
    // old one is dropped, violating the constraint.
    unsafe {
        let old = std::ptr::read(slot);
        drop(old);
        std::ptr::write(slot, factory());
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Copy the `out_w` x `out_h` rectangle at `(x, y)` out of a premultiplied
/// RGBA buffer; pixels past the source edge are opaque white.
fn crop_viewport(src: &[u8], src_w: u32, src_h: u32, x: u32, y: u32, out_w: u32, out_h: u32) -> Vec<u8> {
    let mut out = vec![255u8; out_w as usize * out_h as usize * 4];
    let copy_w = src_w.saturating_sub(x).min(out_w) as usize;
    if copy_w == 0 {
        return out;
    }
    for row in 0..out_h {
        let sy = y + row;
        if sy >= src_h {
            break;
        }
        let from = (sy as usize * src_w as usize + x as usize) * 4;
        let to = row as usize * out_w as usize * 4;
        out[to..to + copy_w * 4].copy_from_slice(&src[from..from + copy_w * 4]);
    }
    out
}

/// Make the rows of a viewport capture that lie past the end of the rendered
/// canvas transparent (`crop_viewport` pads them white), for frame documents.
fn clear_past_source(bytes: &mut [u8], width: u32, y: u32, src_h: u32) {
    let first_row = src_h.saturating_sub(y) as usize;
    let start = (first_row * width as usize * 4).min(bytes.len());
    bytes[start..].fill(0);
}

#[inline]
fn hit_test(x: f32, y: f32, r: &layout::Rect) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
}

// ── Engine actor ──────────────────────────────────────────────────────────────

use std::sync::mpsc;

/// Commands sent to the engine actor thread.
/// All engine work (including blocking HTTP fetches and JS) happens on that thread.
pub enum EngineCmd {
    Navigate {
        url: String,
        width: f32,
        reply: mpsc::Sender<Result<PageResult, String>>,
    },
    /// Load the session history entry `delta` steps from the current one.
    Traverse {
        delta: isize,
        width: f32,
        reply: mpsc::Sender<Result<Option<PageResult>, String>>,
    },
    ReRender {
        hovered_id: Option<String>,
        focused_id: Option<String>,
        width: f32,
        reply: mpsc::Sender<Result<PageResult, String>>,
    },
    Click {
        x: f32,
        y: f32,
        reply: mpsc::Sender<Vec<ClickResult>>,
    },
    TypeText {
        text: String,
    },
    EvaluateJs {
        script: String,
        reply: mpsc::Sender<String>,
    },
    /// Evaluate JS from the DevTools console REPL; echoes input/output into the console buffer.
    EvaluateConsole {
        script: String,
        reply: mpsc::Sender<js::EvalOutcome>,
    },
    Screenshot {
        reply: mpsc::Sender<Option<Vec<u8>>>,
    },
    /// PNG of the viewport at the current document scroll offset.
    ScreenshotViewport {
        reply: mpsc::Sender<Option<Vec<u8>>>,
    },
    DomTree {
        reply: mpsc::Sender<String>,
    },
    LayoutTree {
        reply: mpsc::Sender<String>,
    },
    ComputedStyle {
        selector: String,
        reply: mpsc::Sender<HashMap<String, String>>,
    },
    GetPage {
        reply: mpsc::Sender<Option<ApiPageResponse>>,
    },
    GetElements {
        reply: mpsc::Sender<Vec<ApiElement>>,
    },
    LoadImage {
        url: String,
        bytes: Vec<u8>,
    },
    Tick {
        timestamp: f64,
        deadline: Option<f64>,
        reply: mpsc::Sender<TickOutcome>,
    },
    /// Submit the current page's form. Constructs the navigation URL from form
    /// metadata (action/method) and current DOM field values.
    /// Returns Some(navigation_url) on success, None if no form exists.
    Submit {
        reply: mpsc::Sender<Option<String>>,
    },
    #[allow(dead_code)]
    Shutdown,
}

/// Run the engine actor — owns `BrowserEngine` exclusively.
/// Processes commands sequentially from the receiver.
/// This guarantees all `reqwest::blocking` calls and `thread_local!` JS state
/// are confined to a single thread.
pub fn run_engine_actor(rx: mpsc::Receiver<EngineCmd>) {
    let mut eng = BrowserEngine::new();
    run_engine_actor_with_engine(rx, &mut eng);
}

fn run_engine_actor_with_engine(rx: mpsc::Receiver<EngineCmd>, eng: &mut BrowserEngine) {
    for cmd in rx {
        match cmd {
            EngineCmd::Navigate { url, width, reply } => {
                let result = eng.navigate(&url, width);
                let _ = reply.send(result);
            }
            EngineCmd::Traverse { delta, width, reply } => {
                let result = eng.traverse_history(delta, width);
                let _ = reply.send(result);
            }
            EngineCmd::ReRender {
                hovered_id,
                focused_id,
                width,
                reply,
            } => {
                let result = eng.re_render(hovered_id.as_deref(), focused_id.as_deref(), width);
                let _ = reply.send(result);
            }
            EngineCmd::Click { x, y, reply } => {
                let result = eng.click(x, y);
                let _ = reply.send(result);
            }
            EngineCmd::TypeText { text } => {
                eng.type_text(&text);
            }
            EngineCmd::EvaluateJs { script, reply } => {
                let result = eng.evaluate_js(&script);
                follow_script_navigation(eng);
                let _ = reply.send(result);
            }
            EngineCmd::EvaluateConsole { script, reply } => {
                let outcome = eng.evaluate_console_repl(&script);
                follow_script_navigation(eng);
                let _ = reply.send(outcome);
            }
            EngineCmd::Screenshot { reply } => {
                let png = eng.screenshot().and_then(|pm| pm.encode_png().ok());
                let _ = reply.send(png);
            }
            EngineCmd::ScreenshotViewport { reply } => {
                let png = eng.screenshot_viewport().and_then(|pm| pm.encode_png().ok());
                let _ = reply.send(png);
            }
            EngineCmd::DomTree { reply } => {
                let _ = reply.send(eng.dom_tree());
            }
            EngineCmd::LayoutTree { reply } => {
                let _ = reply.send(eng.layout_tree());
            }
            EngineCmd::ComputedStyle { selector, reply } => {
                let _ = reply.send(eng.computed_style(&selector));
            }
            EngineCmd::GetPage { reply } => {
                let resp = eng
                    .last_page
                    .as_ref()
                    .map(|p| page_to_api_response(p, &p.base_url.clone()));
                let _ = reply.send(resp);
            }
            EngineCmd::GetElements { reply } => {
                let elems = eng
                    .last_page
                    .as_ref()
                    .map(|p| page_to_api_response(p, &p.base_url.clone()).elements)
                    .unwrap_or_default();
                let _ = reply.send(elems);
            }
            EngineCmd::LoadImage { url, bytes } => {
                eng.image_cache.insert(url, bytes);
            }
            EngineCmd::Tick {
                timestamp,
                deadline,
                reply,
            } => {
                let worked = eng.tick_js(Some(timestamp), deadline) | follow_script_navigation(eng);
                let wake_after = eng.js_runtime.next_wake();
                let _ = reply.send(TickOutcome { worked, wake_after });
            }
            EngineCmd::Submit { reply } => {
                let _ = reply.send(eng.submit_form());
            }
            EngineCmd::Shutdown => break,
        }
    }
}

/// What one event-loop tick did and when the next one is needed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickOutcome {
    /// Script ran (the page may need a re-render).
    pub worked: bool,
    /// Tick again after this long (a timer comes due or a fetch is in
    /// flight); `None` when nothing is pending.
    pub wake_after: Option<Duration>,
}

/// Load the document a script asked for while the actor ran a command that
/// has no page to return. Returns whether a document was loaded.
fn follow_script_navigation(eng: &mut BrowserEngine) -> bool {
    let width = eng.current_width();
    match eng.run_script_navigation(width, false) {
        Ok(loaded) => loaded.is_some(),
        Err(error) => {
            eprintln!("[navigate] script navigation failed: {}", error);
            true
        }
    }
}

/// Cloneable handle used by GUI and HTTP threads to send commands to the engine actor.
#[derive(Clone)]
pub struct EngineHandle {
    pub tx: mpsc::SyncSender<EngineCmd>,
    pub console_buffer: js::ConsoleBuffer,
    /// The engine's session history, for back/forward button state.
    pub history: SharedHistory,
}

const ENGINE_CONTROL_TIMEOUT: Duration = Duration::from_secs(3);

fn recv_control<T>(reply_rx: mpsc::Receiver<T>) -> Result<T, EngineRequestError> {
    match reply_rx.recv_timeout(ENGINE_CONTROL_TIMEOUT) {
        Ok(value) => Ok(value),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(EngineRequestError::Busy),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(EngineRequestError::Disconnected),
    }
}

impl EngineHandle {
    /// Create a new engine actor with the default viewport height.
    pub fn spawn() -> Self {
        Self::spawn_with_viewport_height(DEFAULT_VIEWPORT_HEIGHT)
    }

    /// Create a new engine actor whose viewport is `viewport_height` CSS px
    /// tall, and return a handle to it.
    pub fn spawn_with_viewport_height(viewport_height: f32) -> Self {
        let (tx, rx) = mpsc::sync_channel::<EngineCmd>(64);
        let console_buffer = js::new_console_buffer();
        let actor_console = console_buffer.clone();
        let history = SharedHistory::default();
        let actor_history = history.clone();
        std::thread::Builder::new()
            .name("engine-actor".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut eng = BrowserEngine::new_with_console(actor_console);
                eng.history = actor_history;
                eng.viewport_height = viewport_height.max(1.0);
                run_engine_actor_with_engine(rx, &mut eng);
            })
            .expect("failed to start engine actor thread");
        Self {
            tx,
            console_buffer,
            history,
        }
    }

    /// A copy of the engine's session history.
    pub fn history(&self) -> SessionHistory {
        self.history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Load the session history entry `delta` steps away (`-1` back, `1`
    /// forward). `Ok(None)` when there is no such entry.
    pub fn send_traverse(&self, delta: isize, width: f32) -> Result<Option<PageResult>, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::Traverse {
                delta,
                width,
                reply: reply_tx,
            })
            .map_err(|_| "engine disconnected".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "engine disconnected".to_string())?
    }

    pub fn send_navigate(&self, url: String, width: f32) -> Result<PageResult, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::Navigate {
                url,
                width,
                reply: reply_tx,
            })
            .map_err(|_| "engine disconnected".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "engine disconnected".to_string())?
    }

    pub fn send_re_render(
        &self,
        hovered_id: Option<String>,
        focused_id: Option<String>,
        width: f32,
    ) -> Result<PageResult, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::ReRender {
                hovered_id,
                focused_id,
                width,
                reply: reply_tx,
            })
            .map_err(|_| "engine disconnected".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "engine disconnected".to_string())?
    }

    pub fn send_re_render_control(
        &self,
        hovered_id: Option<String>,
        focused_id: Option<String>,
        width: f32,
    ) -> Result<PageResult, EngineRequestError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::ReRender {
                hovered_id,
                focused_id,
                width,
                reply: reply_tx,
            })
            .map_err(|_| EngineRequestError::Disconnected)?;
        recv_control(reply_rx)?.map_err(EngineRequestError::Failed)
    }

    pub fn send_get_page(&self) -> Option<ApiPageResponse> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx.send(EngineCmd::GetPage { reply: reply_tx }).ok()?;
        reply_rx.recv().ok()?
    }

    pub fn send_get_page_control(&self) -> Result<Option<ApiPageResponse>, EngineRequestError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::GetPage { reply: reply_tx })
            .map_err(|_| EngineRequestError::Disconnected)?;
        recv_control(reply_rx)
    }

    pub fn send_get_console(&self) -> Vec<js::ConsoleEntry> {
        js::console_entries(&self.console_buffer)
    }

    pub fn console_version(&self) -> u64 {
        js::console_version(&self.console_buffer)
    }

    pub fn send_clear_console(&self) {
        js::clear_console_buffer(&self.console_buffer);
    }

    pub fn send_get_elements(&self) -> Vec<ApiElement> {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(EngineCmd::GetElements { reply: reply_tx })
            .is_err()
        {
            return vec![];
        }
        reply_rx.recv().unwrap_or_default()
    }

    pub fn send_click(&self, x: f32, y: f32) -> Vec<ClickResult> {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(EngineCmd::Click {
                x,
                y,
                reply: reply_tx,
            })
            .is_err()
        {
            return vec![];
        }
        reply_rx.recv().unwrap_or_default()
    }

    pub fn send_evaluate_js(&self, script: String) -> String {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(EngineCmd::EvaluateJs {
                script,
                reply: reply_tx,
            })
            .is_err()
        {
            return String::new();
        }
        reply_rx.recv().unwrap_or_default()
    }

    pub fn send_console_eval_result(&self, script: String) -> js::EvalOutcome {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(EngineCmd::EvaluateConsole {
                script,
                reply: reply_tx,
            })
            .is_err()
        {
            return js::EvalOutcome {
                result: None,
                error: Some("engine disconnected".to_string()),
            };
        }
        reply_rx.recv().unwrap_or_default()
    }

    pub fn send_screenshot(&self) -> Option<Vec<u8>> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::Screenshot { reply: reply_tx })
            .ok()?;
        reply_rx.recv().ok()?
    }

    pub fn send_screenshot_control(&self) -> Result<Option<Vec<u8>>, EngineRequestError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::Screenshot { reply: reply_tx })
            .map_err(|_| EngineRequestError::Disconnected)?;
        recv_control(reply_rx)
    }

    /// PNG of the viewport at the current scroll offset (see
    /// `BrowserEngine::screenshot_viewport`).
    pub fn send_screenshot_viewport_control(&self) -> Result<Option<Vec<u8>>, EngineRequestError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::ScreenshotViewport { reply: reply_tx })
            .map_err(|_| EngineRequestError::Disconnected)?;
        recv_control(reply_rx)
    }

    pub fn send_dom_tree(&self) -> String {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(EngineCmd::DomTree { reply: reply_tx })
            .is_err()
        {
            return String::new();
        }
        reply_rx.recv().unwrap_or_default()
    }

    pub fn send_layout_tree(&self) -> String {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(EngineCmd::LayoutTree { reply: reply_tx })
            .is_err()
        {
            return String::new();
        }
        reply_rx.recv().unwrap_or_default()
    }

    pub fn send_computed_style(&self, selector: String) -> HashMap<String, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(EngineCmd::ComputedStyle {
                selector,
                reply: reply_tx,
            })
            .is_err()
        {
            return HashMap::new();
        }
        reply_rx.recv().unwrap_or_default()
    }

    pub fn send_tick(&self, timestamp: f64, deadline: Option<f64>) -> TickOutcome {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .tx
            .send(EngineCmd::Tick {
                timestamp,
                deadline,
                reply: reply_tx,
            })
            .is_err()
        {
            return TickOutcome::default();
        }
        reply_rx.recv().unwrap_or_default()
    }

    pub fn send_tick_control(
        &self,
        timestamp: f64,
        deadline: Option<f64>,
    ) -> Result<bool, EngineRequestError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(EngineCmd::Tick {
                timestamp,
                deadline,
                reply: reply_tx,
            })
            .map_err(|_| EngineRequestError::Disconnected)?;
        recv_control(reply_rx).map(|outcome| outcome.worked)
    }

    pub fn send_submit(&self) -> Option<String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self.tx.send(EngineCmd::Submit { reply: reply_tx }).is_err() {
            return None;
        }
        reply_rx.recv().unwrap_or(None)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_history_push_back_forward() {
        let mut history = SessionHistory::default();
        assert!(!history.can_go_back() && !history.can_go_forward());
        assert_eq!(history.entry_at(-1), None);
        history.push("https://a.com/".into());
        history.push("https://b.com/".into());
        assert!(history.can_go_back());
        assert_eq!(history.entry_at(-1), Some((0, "https://a.com/".to_string())));
        history.index = 0;
        assert!(history.can_go_forward());
        assert_eq!(history.entry_at(1), Some((1, "https://b.com/".to_string())));
        assert_eq!(history.entry_at(2), None);
    }

    #[test]
    fn test_session_history_push_drops_forward_entries_and_skips_reload() {
        let mut history = SessionHistory::default();
        history.push("https://a.com/".into());
        history.push("https://b.com/".into());
        history.index = 0;
        history.push("https://c.com/".into());
        assert_eq!(history.len(), 2);
        assert_eq!(history.current(), Some("https://c.com/"));
        assert!(!history.can_go_forward());
        history.push("https://c.com/".into());
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_session_history_replace_keeps_length() {
        let mut history = SessionHistory::default();
        history.replace("https://a.com/".into());
        assert_eq!(history.current(), Some("https://a.com/"));
        history.push("https://b.com/".into());
        history.replace("https://c.com/".into());
        assert_eq!(history.len(), 2);
        assert_eq!(history.current(), Some("https://c.com/"));
    }

    #[test]
    fn test_location_fragment_change_stays_in_document() {
        let mut engine = engine_with_page_html("<html><body></body></html>");
        let result = engine.evaluate_js(
            "var fired = 0; window.addEventListener('hashchange', function() { fired++; });\
             location.hash = 'top'; String(fired) + ' ' + location.hash + ' ' + history.length",
        );
        assert_eq!(result, "1 #top 2");
        assert_eq!(engine.js_runtime.take_navigation_request(), None);
        engine.evaluate_js("location.href = 'https://example.com/other'");
        assert_eq!(
            engine.js_runtime.take_navigation_request(),
            Some(js::NavigationRequest {
                url: "https://example.com/other".to_string(),
                replace: false,
            })
        );
    }

    #[test]
    fn test_next_wake_waits_for_the_earliest_timer() {
        let mut engine = engine_with_page_html("<html><body></body></html>");
        while engine.tick_js(Some(0.0), None) {}
        assert_eq!(engine.js_runtime.next_wake(), None);
        engine.evaluate_js("setInterval(function() {}, 10000); setTimeout(function() {}, 5000);");
        assert!(!engine.tick_js(Some(0.0), None));
        let wake = engine.js_runtime.next_wake().expect("timers pending");
        assert!(
            wake > Duration::from_secs(4) && wake <= Duration::from_secs(5),
            "{wake:?}"
        );
    }

    #[test]
    fn test_browser_engine_new() {
        let engine = BrowserEngine::new();
        assert!(engine.last_page.is_none());
        assert!(engine.image_cache.is_empty());
        assert!(engine.css_cache.is_empty());
        assert!(engine.last_stylesheet.is_none());
        assert!(engine.current_csp_policy.is_none());
        assert!(engine.console_entries().is_empty());
    }

    #[test]
    fn test_cache_missing_images_with_inserts_only_uncached_urls() {
        let mut engine = BrowserEngine::new();
        engine
            .image_cache
            .insert("https://example.com/already.png".into(), vec![1, 2, 3]);
        let urls = vec![
            "https://example.com/already.png".to_string(),
            "https://example.com/new.png".to_string(),
        ];

        let mut fetched = Vec::new();
        let loaded = engine.cache_missing_images_with(&urls, |url| {
            fetched.push(url.to_string());
            Ok(vec![9, 9, 9])
        });

        assert!(loaded);
        assert_eq!(fetched, vec!["https://example.com/new.png".to_string()]);
        assert_eq!(
            engine.image_cache.get("https://example.com/already.png"),
            Some(&vec![1, 2, 3])
        );
        assert_eq!(
            engine.image_cache.get("https://example.com/new.png"),
            Some(&vec![9, 9, 9])
        );
    }

    #[test]
    fn test_cache_missing_images_with_ignores_fetch_errors() {
        let mut engine = BrowserEngine::new();
        let urls = vec!["https://example.com/missing.png".to_string()];

        let loaded = engine.cache_missing_images_with(&urls, |_url| Err("boom".into()));

        assert!(!loaded);
        assert!(engine.image_cache.is_empty());
    }

    #[test]
    fn test_external_module_fetch_compile_same_origin_and_cache() {
        let mut engine = BrowserEngine::new();
        let page_url = Url::parse("https://example.com/app/index.html").unwrap();
        let module_url = Url::parse("https://example.com/app/main.js").unwrap();
        let source = "import './dep.js'; export const value = 1;".to_string();

        let first = engine.load_external_module_with(module_url.clone(), &page_url, |url| {
            assert_eq!(url, &module_url);
            Ok::<String, String>(source.clone())
        });
        assert_eq!(first.error, None);
        assert!(!first.from_cache);
        assert_eq!(first.requests, vec!["./dep.js".to_string()]);
        assert_eq!(engine.js_runtime.module_cache_len(), 1);

        let second = engine.load_external_module_with(module_url.clone(), &page_url, |_url| {
            Ok::<String, String>("export const value = 2;".to_string())
        });
        assert_eq!(second.error, None);
        assert!(second.from_cache);
        assert_eq!(engine.js_runtime.module_cache_len(), 1);
    }

    #[test]
    fn test_external_module_fetch_compile_reports_fetch_error_without_panic() {
        let mut engine = BrowserEngine::new();
        let page_url = Url::parse("https://example.com/app/index.html").unwrap();
        let module_url = Url::parse("https://example.com/app/missing.js").unwrap();

        let outcome = engine.load_external_module_with(module_url, &page_url, |_url| {
            Err::<String, String>("not found".to_string())
        });

        assert_eq!(outcome.error.as_deref(), Some("not found"));
        assert_eq!(engine.js_runtime.module_cache_len(), 0);
    }

    #[test]
    fn test_external_module_fetch_compile_respects_script_src_csp() {
        let mut engine = BrowserEngine::new();
        engine.current_csp_policy = Some(js::CspPolicy::parse("script-src 'self'"));
        let page_url = Url::parse("https://example.com/app/index.html").unwrap();
        let module_url = Url::parse("https://cdn.example.test/app/main.js").unwrap();

        let outcome = engine.load_external_module_with(module_url, &page_url, |_url| {
            Ok::<String, String>("export const value = 1;".to_string())
        });

        assert_eq!(outcome.error.as_deref(), Some("Blocked by script-src CSP"));
        assert_eq!(engine.js_runtime.module_cache_len(), 0);
    }

    // //     #[test]
    //     fn test_type_text_updates_focused_input_dom_state_and_events() {
    //         let mut engine = BrowserEngine::new();
    //         let base_url = Url::parse("https://example.com/").unwrap();
    //         let mut css_cache = HashMap::new();
    //         let (page, _) = process_html_with_cache(
    //             "<html><body><input id='field' value='a'><script>window.events=[]; var field = document.getElementById('field'); field.addEventListener('input', function(e) { window.events.push('input:' + e.data); }); field.addEventListener('change', function() { window.events.push('change'); });</script></body></html>",
    //             &base_url,
    //             &HashMap::new(),
    //             &mut css_cache,
    //             None,
    //             &HashMap::new(),
    //             None,
    //             None,
    //             None,
    //             800.0,
    //         )
    //         .unwrap();
    //         engine.init_js_for_page(&page);
    //
    //         engine.js_runtime.set_focused_node_id(Some("field".to_string()));
    //         engine.type_text("bc");
    //
    //         let value = engine.evaluate_js("document.getElementById('field').value");
    //         let events = engine.evaluate_js("window.events.join('|')");
    //         assert_eq!(value, "abc");
    //         assert_eq!(events, "input:bc|change");
    //     }

    // //     #[test]
    //     fn test_clear_console_empties_buffer() {
    //         let mut engine = BrowserEngine::new();
    //         engine.evaluate_js("console.log('hello')");
    //         assert_eq!(engine.console_entries().len(), 1);
    //         engine.clear_console();
    //         assert!(engine.console_entries().is_empty());
    //     }

    #[test]
    fn test_console_repl_returns_result_and_console_entries() {
        let mut engine = BrowserEngine::new();
        let outcome = engine.evaluate_console_repl("1 + 1");
        assert_eq!(outcome.result.as_deref(), Some("2"));
        assert_eq!(outcome.error, None);

        let entries = engine.console_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "> 1 + 1");
        assert_eq!(entries[1].message, "< 2");
    }

    #[test]
    fn test_console_repl_returns_error_and_console_entries() {
        let mut engine = BrowserEngine::new();
        let outcome = engine.evaluate_console_repl("missingVariable");
        assert!(outcome.result.is_none());
        assert!(outcome.error.is_some());

        let entries = engine.console_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "> missingVariable");
        assert!(entries[1].message.starts_with("< "));
        assert_eq!(entries[1].level, js::ConsoleLevel::Error);
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
        let html =
            r#"<form><input name="user" type="text"><input name="pass" type="password"></form>"#;
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
        let r = ClickResult::Navigate {
            url: "https://example.com".to_string(),
        };
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
        let r = ClickResult::FocusChanged {
            id: "search-input".to_string(),
        };
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
        assert_eq!(
            extract_attr(tag, "href"),
            Some("https://example.com".to_string())
        );
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

    // ── Viewport width stability regression tests ────────────────────────────────

    /// Verify that `process_html_with_cache` uses the exact width passed in and
    /// returns a `PageResult` whose `width` field matches that value.
    /// This ensures that navigate and re-render paths produce bit-identical layout
    /// when given the same width.
    #[test]
    fn test_process_html_stable_width() {
        use url::Url;
        let html = "<html><body><p>hello</p></body></html>";
        let base = Url::parse("https://example.com").unwrap();
        let mut cache = HashMap::new();

        let navigate_width = 800.0_f32;
        let (page, _ss) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut cache,
            None,
            None,
            None,
            None,
            navigate_width,
        )
        .expect("process_html_with_cache failed");

        assert_eq!(
            page.width, navigate_width as u32,
            "page.width should equal the navigate width"
        );

        // Re-render with the same width: result must be identical.
        let (re_rendered, _) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut cache,
            None,
            None,
            None,
            None,
            navigate_width, // same width — no drift
        )
        .expect("re-render failed");

        assert_eq!(
            page.width, re_rendered.width,
            "re-render width must match navigate width — no viewport drift"
        );
        assert_eq!(
            page.height, re_rendered.height,
            "re-render height must match — no layout churn from width drift"
        );
    }

    /// Verify that a different width produces a different layout, confirming that
    /// the test above is not trivially true.
    #[test]
    fn test_process_html_different_widths_differ() {
        use url::Url;
        // Use enough content that a width difference is likely to change final_y or width.
        let html = "<html><body><p>some content</p></body></html>";
        let base = Url::parse("https://example.com").unwrap();
        let mut cache = HashMap::new();

        let (page_800, _) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut cache,
            None,
            None,
            None,
            None,
            800.0,
        )
        .expect("800 failed");

        let (page_400, _) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut cache,
            None,
            None,
            None,
            None,
            400.0,
        )
        .expect("400 failed");

        assert_ne!(
            page_800.width, page_400.width,
            "different widths must produce different page.width values"
        );
    }

    #[test]
    fn test_form_metadata_extracts_action_method_and_control_names() {
        let html = r#"<html><body><form action="/search" method="post">
            <input name="q" value="hello">
            <input name="msg" value="desc">
        </form></body></html>"#;
        let base = Url::parse("https://example.com").unwrap();
        let mut cache = HashMap::new();
        let (page, _) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut cache,
            None,
            None,
            None,
            None,
            800.0,
        )
        .unwrap();

        let meta = page.form_metadata.expect("form_metadata should be Some");
        assert_eq!(meta.action, "/search");
        assert_eq!(meta.method, "post");
        assert_eq!(meta.controls.len(), 2);

        assert_eq!(meta.controls[0].name, "q");
        assert_eq!(meta.controls[0].initial_value, "hello");

        assert_eq!(meta.controls[1].name, "msg");
        assert_eq!(meta.controls[1].initial_value, "desc");
    }

    #[test]
    fn test_form_metadata_none_when_no_form() {
        let html = "<html><body><p>no form here</p></body></html>";
        let base = Url::parse("https://example.com").unwrap();
        let mut cache = HashMap::new();
        let (page, _) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut cache,
            None,
            None,
            None,
            None,
            800.0,
        )
        .unwrap();

        assert!(page.form_metadata.is_none());
    }

    #[test]
    fn test_form_metadata_default_method_get() {
        let html = r#"<form action="/search"><input name="q" value="test"></form>"#;
        let base = Url::parse("https://example.com").unwrap();
        let mut cache = HashMap::new();
        let (page, _) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut cache,
            None,
            None,
            None,
            None,
            800.0,
        )
        .unwrap();

        let meta = page.form_metadata.unwrap();
        assert_eq!(meta.action, "/search");
        assert_eq!(meta.method, "get");
        assert_eq!(meta.controls.len(), 1);
        assert_eq!(meta.controls[0].name, "q");
    }

    //     #[test]
    // //     fn test_submit_form_constructs_get_url_with_live_values() {
    //         let html = r#"<form action="/search" method="get"><input name="q" value="hello"></form>"#;
    //         let base = Url::parse("https://example.com").unwrap();
    //         let mut cache = HashMap::new();
    //         let (page, _) = process_html_with_cache(
    //             html,
    //             &base,
    //             &HashMap::new(),
    //             &mut cache,
    //             None,
    //             &HashMap::new(),
    //             None,
    //             None,
    //             None,
    //             800.0,
    //         )
    //         .unwrap();
    //
    //         let mut engine = BrowserEngine::new();
    //         engine.init_js_for_page(&page);
    //         engine.last_page = Some(page);
    //
    //         let url = engine.submit_form().expect("submit_form should return URL");
    //         assert!(url.starts_with("https://example.com/search?"));
    //         assert!(url.contains("q=hello"));
    //     }

    #[test]
    fn test_submit_form_none_when_no_form() {
        let html = "<html><body><p>no form</p></body></html>";
        let base = Url::parse("https://example.com").unwrap();
        let mut cache = HashMap::new();
        let (page, _) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut cache,
            None,
            None,
            None,
            None,
            800.0,
        )
        .unwrap();

        let mut engine = BrowserEngine::new();
        engine.last_page = Some(page);
        assert!(engine.submit_form().is_none());
    }

    //     #[test]
    // //     fn test_submit_form_empty_action_uses_base_url() {
    //         let html = r#"<form><input name="q" value="rust"></form>"#;
    //         let base = Url::parse("https://example.com/page").unwrap();
    //         let mut cache = HashMap::new();
    //         let (page, _) = process_html_with_cache(
    //             html,
    //             &base,
    //             &HashMap::new(),
    //             &mut cache,
    //             None,
    //             &HashMap::new(),
    //             None,
    //             None,
    //             None,
    //             800.0,
    //         )
    //         .unwrap();
    //
    //         let mut engine = BrowserEngine::new();
    //         engine.init_js_for_page(&page);
    //         engine.last_page = Some(page);
    //
    //         let url = engine.submit_form().expect("should return URL");
    //         assert!(url.starts_with("https://example.com/page?"));
    //         assert!(url.contains("q=rust"));
    //     }

    #[test]
    fn test_resolve_url_http_passthrough() {
        assert_eq!(resolve_url("http://example.com"), "http://example.com");
        assert_eq!(
            resolve_url("https://example.com/path"),
            "https://example.com/path"
        );
    }

    #[test]
    fn test_resolve_url_domain_like() {
        assert_eq!(resolve_url("google.com"), "https://google.com");
        assert_eq!(resolve_url("example.com/page"), "https://example.com/page");
    }

    #[test]
    fn test_resolve_url_search_query() {
        let result = resolve_url("rust browser engine");
        assert!(result.starts_with("https://www.google.com/search?q="));
        assert!(result.contains("rust"));
        assert!(result.contains("browser"));
    }

    #[test]
    fn test_resolve_url_trims_and_encodes() {
        let result = resolve_url("  hello world  ");
        assert!(result.starts_with("https://www.google.com/search?q=hello+world"));
    }

    // ── async/defer script ordering tests ─────────────────────────────────

    /// Helper: build a PageResult from inline HTML and run init_js_for_page,
    /// then return the engine so callers can inspect the JS runtime state.
    fn engine_with_page_html(html: &str) -> BrowserEngine {
        let base = Url::parse("https://example.com/").unwrap();
        let mut css_cache = HashMap::new();
        let (page, _) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut css_cache,
            None,
            None,
            None,
            None,
            800.0,
        )
        .expect("process_html_with_cache");
        let mut engine = BrowserEngine::new();
        engine.init_js_for_page(&page);
        engine
    }

    /// Re-rendering at a new width re-evaluates `@media` rules, updates
    /// `window.innerWidth` and fires one `resize` event.
    #[test]
    fn test_re_render_at_new_width_reflows_media_rules_and_fires_resize() {
        let html = r#"<html><head><style>
            #a { width: 10px; height: 10px; }
            @media (min-width: 1000px) { #a { width: 50px; } }
        </style></head><body><div id="a"></div>
        <script>window.__resizes = 0; window.addEventListener('resize', () => window.__resizes++);</script>
        </body></html>"#;
        let base = Url::parse("https://example.com/").unwrap();
        let (page, stylesheet) = process_html_with_cache(
            html, &base, &HashMap::new(), &mut HashMap::new(), None, None, None, None, 800.0,
        )
        .expect("process_html_with_cache");
        let mut engine = BrowserEngine::new();
        engine.last_page = Some(page.clone());
        engine.last_stylesheet = Some(stylesheet);
        engine.stylesheet_viewport = (800.0, DEFAULT_VIEWPORT_HEIGHT);
        engine.init_js_for_page(&page);
        engine.re_render(None, None, 800.0).expect("render at 800");
        assert_eq!(engine.evaluate_js("window.innerWidth"), "800");
        assert_eq!(engine.evaluate_js("document.getElementById('a').getBoundingClientRect().width"), "10");

        engine.re_render(None, None, 1200.0).expect("render at 1200");
        engine.tick_js(Some(0.0), None);
        assert_eq!(engine.evaluate_js("window.innerWidth"), "1200");
        assert_eq!(engine.evaluate_js("window.__resizes"), "1");
        assert_eq!(engine.evaluate_js("document.getElementById('a').getBoundingClientRect().width"), "50");
    }

    #[test]
    fn test_classic_inline_scripts_execute_in_dom_order() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <script>window.order = []; window.order.push('first')</script>
                <script>window.order.push('second')</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("JSON.stringify(window.order)");
        assert_eq!(result, r#"["first","second"]"#);
    }

    #[test]
    fn test_defer_classic_executes_after_sync_classics() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <script>window.order = []; window.order.push('sync1')</script>
                <script defer>window.order.push('deferred')</script>
                <script>window.order.push('sync2')</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("JSON.stringify(window.order)");
        assert_eq!(result, r#"["sync1","sync2","deferred"]"#);
    }

    #[test]
    fn test_multiple_deferred_scripts_execute_in_dom_order() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <script>window.order = []</script>
                <script defer>window.order.push('def1')</script>
                <script defer>window.order.push('def2')</script>
                <script defer>window.order.push('def3')</script>
                <script>window.order.push('sync')</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("JSON.stringify(window.order)");
        assert_eq!(result, r#"["sync","def1","def2","def3"]"#);
    }

    #[test]
    fn test_defer_script_without_src_has_no_effect_on_inline() {
        // defer attribute only affects external scripts per HTML spec,
        // but we still support it on inline for completeness.
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <script>window.order = []</script>
                <script>window.order.push('sync1')</script>
                <script defer>window.order.push('should_be_deferred')</script>
                <script>window.order.push('sync2')</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("JSON.stringify(window.order)");
        // defer on inline pushes it to deferred phase
        assert_eq!(result, r#"["sync1","sync2","should_be_deferred"]"#);
    }

    #[test]
    fn test_module_default_defer_executes_after_classics() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <script>window.order = []; window.order.push('sync1')</script>
                <script type="module">window.order.push('module')</script>
                <script>window.order.push('sync2')</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("JSON.stringify(window.order)");
        // modules are defer-by-default: execute after all classic scripts
        assert_eq!(result, r#"["sync1","sync2","module"]"#);
    }

    #[test]
    fn test_async_module_does_not_execute_final_phase() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <script>window.order = []; window.order.push('sync')</script>
                <script type="module" async>window.order.push('async-module')</script>
                <script defer>window.order.push('deferred')</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("JSON.stringify(window.order)");
        // sync first, then deferred classics, then async module last
        assert_eq!(result, r#"["sync","deferred","async-module"]"#);
    }

    #[test]
    fn test_classic_behavior_does_not_regress_no_attrs() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <script>window.x = 1</script>
                <script>window.x = window.x + 2</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("window.x");
        assert_eq!(result, "3");
    }

    #[test]
    fn test_async_and_defer_on_same_script_async_wins() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <script>window.order = []; window.order.push('sync')</script>
                <script async defer>window.order.push('async-wins')</script>
                <script>window.order.push('sync2')</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("JSON.stringify(window.order)");
        // async wins over defer → executes in async phase (after deferred)
        assert_eq!(result, r#"["sync","sync2","async-wins"]"#);
    }

    #[test]
    fn test_module_sets_text_content() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <div id='test'>old</div>
                <script type="module">document.getElementById('test').textContent = 'new';</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("document.getElementById('test').textContent");
        assert_eq!(result, "new");
    }

    #[test]
    fn test_module_sets_inner_html() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <div id='test'>old</div>
                <script type="module">document.getElementById('test').innerHTML = '<span>new</span>';</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("document.getElementById('test').innerHTML");
        assert_eq!(result, "<span>new</span>");
    }

    #[test]
    fn test_module_sets_attribute() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <div id='test' data-foo='old'>text</div>
                <script type="module">document.getElementById('test').setAttribute('data-foo', 'new');</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("document.getElementById('test').getAttribute('data-foo')");
        assert_eq!(result, "new");
    }

    #[test]
    fn test_module_sets_form_value() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <input id='test' value='old'>
                <script type="module">document.getElementById('test').value = 'new';</script>
            </body></html>"#,
        );
        let result = engine.evaluate_js("document.getElementById('test').value");
        assert_eq!(result, "new");
    }

    #[test]
    fn test_module_style_write_lands_in_style_attribute() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <div id='test'>text</div>
                <script type="module">document.getElementById('test').style.color = 'red';</script>
            </body></html>"#,
        );
        assert_eq!(
            engine.evaluate_js("document.getElementById('test').getAttribute('style')"),
            "color: red;"
        );
    }

    #[test]
    fn test_module_settimeout_during_tick() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <div id='test'>old</div>
                <script type="module">
                    globalThis.__timer_fired = false;
                    setTimeout(function() {
                        document.getElementById('test').textContent = 'delayed';
                        globalThis.__timer_fired = true;
                    }, 10);
                </script>
            </body></html>"#,
        );
        assert_eq!(engine.evaluate_js("globalThis.__timer_fired"), "false");
        assert_eq!(
            engine.evaluate_js("document.getElementById('test').textContent"),
            "old"
        );

        // Timers honour their delay, so let the 10ms timer come due first.
        std::thread::sleep(std::time::Duration::from_millis(20));
        engine.tick_js(Some(20.0), None);

        assert_eq!(engine.evaluate_js("globalThis.__timer_fired"), "true");
        assert_eq!(
            engine.evaluate_js("document.getElementById('test').textContent"),
            "delayed"
        );
    }

    #[test]
    fn test_module_multiple_mutations() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <div id='a'>text-a</div>
                <div id='b'>text-b</div>
                <div id='c'>text-c</div>
                <script type="module">
                    document.getElementById('a').textContent = 'new-a';
                    document.getElementById('b').setAttribute('data-x', 'y');
                    document.getElementById('c').style.fontSize = '20px';
                </script>
            </body></html>"#,
        );
        assert_eq!(
            engine.evaluate_js("document.getElementById('a').textContent"),
            "new-a"
        );
        assert_eq!(
            engine.evaluate_js("document.getElementById('b').getAttribute('data-x')"),
            "y"
        );
        assert_eq!(
            engine.evaluate_js("document.getElementById('c').style.fontSize"),
            "20px"
        );
    }

    #[test]
    fn test_module_module_level_execution_integration() {
        let mut engine = engine_with_page_html(
            r#"<html><body>
                <div id='test'>before</div>
                <script type="module">
                    document.getElementById('test').textContent = 'during-module';
                </script>
                <script>globalThis.__classic_ran = true;</script>
            </body></html>"#,
        );
        assert_eq!(engine.evaluate_js("globalThis.__classic_ran"), "true");
        assert_eq!(
            engine.evaluate_js("document.getElementById('test').textContent"),
            "during-module"
        );
    }

    // ── Document scrolling and viewport capture ─────────────────────────────

    /// Like `engine_with_page_html`, but also keeps the rendered page so the
    /// engine can re-render and capture screenshots.
    fn engine_with_rendered_page(html: &str) -> BrowserEngine {
        let base = Url::parse("https://example.com/").unwrap();
        let mut css_cache = HashMap::new();
        let (page, stylesheet) = process_html_with_cache(
            html,
            &base,
            &HashMap::new(),
            &mut css_cache,
            None,
            None,
            None,
            None,
            800.0,
        )
        .expect("process_html_with_cache");
        let mut engine = BrowserEngine::new();
        engine.last_stylesheet = Some(stylesheet);
        engine.last_page = Some(page.clone());
        engine.init_js_for_page(&page);
        engine
    }

    fn scroll_json(engine: &mut BrowserEngine) -> String {
        engine.evaluate_js("JSON.stringify([scrollX, scrollY, pageXOffset, pageYOffset])")
    }

    const WIDE_PAGE: &str = r#"<html><head><style>
        body { margin: 0; }
        #wide { width: 1400px; height: 3000px; }
        #target { margin-left: 900px; margin-top: 50px; width: 100px; height: 20px; }
        #low { margin-top: 2000px; width: 100px; height: 40px; }
    </style></head><body><div id="wide"><div id="target"></div><div id="low"></div></div></body></html>"#;

    #[test]
    fn test_scroll_extent_covers_horizontal_overflow() {
        let mut engine = engine_with_rendered_page(WIDE_PAGE);
        let page = engine.last_page.as_ref().unwrap();
        assert_eq!(page.scroll_width, 1400.0);
        // #target's top margin collapses through #wide, which starts at y=50.
        assert_eq!(page.scroll_height, 3050.0);
        assert_eq!(
            engine.evaluate_js(
                "JSON.stringify([document.documentElement.scrollWidth, document.documentElement.scrollHeight, document.documentElement.clientWidth, document.scrollingElement === document.documentElement])"
            ),
            "[1400,3050,800,true]"
        );
    }

    #[test]
    fn test_scroll_extent_ignores_overflow_hidden_content() {
        let engine = engine_with_rendered_page(
            r#"<html><body style="margin:0">
                <div style="width:300px;height:100px;overflow:hidden"><div style="width:5000px;height:10px"></div></div>
            </body></html>"#,
        );
        assert_eq!(engine.last_page.as_ref().unwrap().scroll_width, 800.0);
    }

    #[test]
    fn test_scroll_extent_ignores_invisible_boxes() {
        let engine = engine_with_rendered_page(
            r#"<html><body style="margin:0">
                <div style="opacity:0"><div style="width:5000px;height:10px"></div></div>
                <div style="visibility:hidden;width:4000px;height:10px"></div>
                <div style="visibility:hidden"><div style="visibility:visible;width:900px;height:10px"></div></div>
            </body></html>"#,
        );
        assert_eq!(engine.last_page.as_ref().unwrap().scroll_width, 900.0);
    }

    #[test]
    fn test_engine_viewport_height_drives_js_and_screen_capture() {
        let mut engine = engine_with_rendered_page("<html><body><div style='height:100vh'></div></body></html>");
        engine.viewport_height = 500.0;
        let page = engine.last_page.clone().unwrap();
        engine.init_js_for_page(&page);
        engine.re_render(None, None, 800.0).unwrap();
        assert_eq!(engine.evaluate_js("String(innerHeight)"), "500");
        assert_eq!(engine.screenshot_viewport().unwrap().height(), 500);
        assert_eq!(engine.last_page.as_ref().unwrap().scroll_height, 500.0);
    }

    #[test]
    fn test_wide_page_renders_viewport_width_only() {
        let mut engine = engine_with_rendered_page(
            r#"<html><body style="margin:0">
                <div style="width:5000px;height:50px"></div>
                <div style="position:absolute;left:1500px;top:10px;width:20px;height:20px;background:#0000ff"></div>
            </body></html>"#,
        );
        let page = engine.last_page.as_ref().unwrap();
        assert_eq!(page.scroll_width, 5000.0);
        assert_eq!(page.pixmap_bytes.len(), 800 * page.height as usize * 4);
        engine.evaluate_js("window.scrollTo(1450, 0)");
        let viewport = engine.screenshot_viewport().unwrap();
        assert_eq!((viewport.width(), viewport.height()), (800, DEFAULT_VIEWPORT_HEIGHT.round() as u32));
        assert_eq!(pixel(&viewport, 55, 15), (0, 0, 255));
        assert_eq!(pixel(&viewport, 45, 15), (255, 255, 255));
    }

    #[test]
    fn test_window_scroll_to_clamps_to_scrollable_range() {
        let mut engine = engine_with_rendered_page(WIDE_PAGE);
        let max_y = 3050.0 - DEFAULT_VIEWPORT_HEIGHT;
        engine.evaluate_js("window.scrollTo(5000, 99999)");
        assert_eq!(
            scroll_json(&mut engine),
            format!("[600,{max_y},600,{max_y}]")
        );
        engine.evaluate_js("window.scrollTo({ top: 10 })");
        assert_eq!(scroll_json(&mut engine), "[600,10,600,10]");
        engine.evaluate_js("window.scrollBy(-100, 5); window.scroll(-1, -1); window.scrollBy({ left: 40 })");
        assert_eq!(scroll_json(&mut engine), "[40,0,40,0]");
        assert_eq!(engine.scroll_position(), (40.0, 0.0));
    }

    #[test]
    fn test_document_element_scroll_offsets_scroll_the_window() {
        let mut engine = engine_with_rendered_page(WIDE_PAGE);
        engine.evaluate_js("document.documentElement.scrollTop = 120; document.documentElement.scrollLeft = 30;");
        assert_eq!(scroll_json(&mut engine), "[30,120,30,120]");
        assert_eq!(
            engine.evaluate_js(
                "JSON.stringify([document.documentElement.scrollTop, document.documentElement.scrollLeft, document.body.scrollTop])"
            ),
            "[120,30,0]"
        );
    }

    #[test]
    fn test_bounding_client_rect_subtracts_scroll_offset() {
        let mut engine = engine_with_rendered_page(WIDE_PAGE);
        engine.evaluate_js("window.scrollTo(100, 30)");
        assert_eq!(
            engine.evaluate_js(
                "var r = document.getElementById('target').getBoundingClientRect(); JSON.stringify([r.x, r.y, r.left, r.top, r.right, r.bottom, r.width])"
            ),
            "[800,20,800,20,900,40,100]"
        );
    }

    #[test]
    fn test_scroll_into_view_alignments() {
        let mut engine = engine_with_rendered_page(WIDE_PAGE);
        // #low border box: y 2070..2110, x 0..100.
        engine.evaluate_js("document.getElementById('low').scrollIntoView()");
        assert_eq!(scroll_json(&mut engine), "[0,2070,0,2070]");
        engine.evaluate_js("window.scrollTo(0, 0); document.getElementById('low').scrollIntoView(false)");
        let end = 2110.0 - DEFAULT_VIEWPORT_HEIGHT;
        assert_eq!(scroll_json(&mut engine), format!("[0,{end},0,{end}]"));
        engine.evaluate_js(
            "window.scrollTo(0, 0); document.getElementById('target').scrollIntoView({ block: 'center', inline: 'center' })",
        );
        let center_y = 60.0 - DEFAULT_VIEWPORT_HEIGHT / 2.0;
        let (x, y) = engine.scroll_position();
        assert_eq!(x, 550.0);
        assert_eq!(y, center_y.max(0.0));
        // `nearest` leaves an already visible element alone.
        engine.evaluate_js("document.getElementById('target').scrollIntoView({ block: 'nearest', inline: 'nearest' })");
        assert_eq!(engine.scroll_position(), (550.0, center_y.max(0.0)));
    }

    #[test]
    fn test_focus_scrolls_element_into_view_nearest_edge() {
        let mut engine = engine_with_rendered_page(WIDE_PAGE);
        // #target spans x 900..1000; the nearest-edge alignment puts its right
        // edge on the viewport's right edge.
        engine.evaluate_js("document.getElementById('target').focus()");
        assert_eq!(scroll_json(&mut engine), "[200,0,200,0]");
        assert_eq!(
            engine.evaluate_js("document.getElementById('target').getBoundingClientRect().right"),
            "800"
        );
    }

    #[test]
    fn test_focus_prevent_scroll_keeps_position() {
        let mut engine = engine_with_rendered_page(WIDE_PAGE);
        engine.evaluate_js("document.getElementById('target').focus({ preventScroll: true })");
        assert_eq!(scroll_json(&mut engine), "[0,0,0,0]");
        assert_eq!(engine.evaluate_js("document.activeElement.id"), "target");
    }

    #[test]
    fn test_scroll_event_fires_once_per_task_and_reaches_window() {
        let mut engine = engine_with_rendered_page(WIDE_PAGE);
        engine.evaluate_js(
            "window.events = []; \
             document.addEventListener('scroll', function() { events.push('document:' + scrollY); }); \
             window.addEventListener('scroll', function() { events.push('window:' + scrollY); }); \
             window.scrollTo(0, 10); window.scrollTo(0, 20); window.scrollTo(0, 20);",
        );
        assert_eq!(engine.evaluate_js("events.length"), "0", "scroll events are async");
        for _ in 0..3 {
            engine.tick_js(Some(0.0), None);
        }
        assert_eq!(
            engine.evaluate_js("JSON.stringify(events)"),
            r#"["document:20","window:20"]"#
        );
        // Scrolling to the current position fires nothing.
        engine.evaluate_js("window.scrollTo(0, 20)");
        engine.tick_js(Some(0.0), None);
        assert_eq!(engine.evaluate_js("events.length"), "2");
    }

    #[test]
    fn test_layout_metrics_include_css_transforms() {
        let mut engine = engine_with_rendered_page(
            r#"<html><body style="margin:0">
                <div id="box" style="position:absolute;left:400px;top:10px;width:200px;height:50px;transform:translateX(-50%)">
                    <div id="inner" style="width:20px;height:10px"></div>
                </div>
            </body></html>"#,
        );
        assert_eq!(
            engine.evaluate_js(
                "var b = document.getElementById('box').getBoundingClientRect(); var i = document.getElementById('inner').getBoundingClientRect(); JSON.stringify([b.x, b.y, b.width, i.x, i.y])"
            ),
            "[300,10,200,300,10]"
        );
    }

    fn pixel(pixmap: &tiny_skia::Pixmap, x: u32, y: u32) -> (u8, u8, u8) {
        let p = pixmap.pixel(x, y).expect("pixel in range");
        (p.red(), p.green(), p.blue())
    }

    const FIXED_PAGE: &str = r#"<html><head><style>
        body { margin: 0; }
        #wide { width: 1200px; height: 3000px; position: relative; }
        #fixed { position: fixed; top: 0; left: 0; width: 40px; height: 40px; background: rgb(255, 0, 0); }
        #marker { position: absolute; left: 300px; top: 600px; width: 20px; height: 20px; background: rgb(0, 0, 255); }
    </style></head><body><div id="wide"><div id="marker"></div></div><div id="fixed"></div></body></html>"#;

    #[test]
    fn test_viewport_screenshot_at_origin_matches_full_page_top() {
        let mut engine = engine_with_rendered_page(FIXED_PAGE);
        let full = engine.screenshot().unwrap();
        let viewport = engine.screenshot_viewport().unwrap();
        assert_eq!(full.width(), 800);
        assert_eq!(viewport.width(), 800);
        assert_eq!(viewport.height(), DEFAULT_VIEWPORT_HEIGHT.round() as u32);
        assert_eq!(pixel(&viewport, 10, 10), (255, 0, 0));
        assert_eq!(pixel(&full, 10, 10), (255, 0, 0));
    }

    #[test]
    fn test_viewport_screenshot_follows_scroll_and_keeps_fixed_boxes_in_view() {
        let mut engine = engine_with_rendered_page(FIXED_PAGE);
        engine.evaluate_js("window.scrollTo(250, 500)");
        let viewport = engine.screenshot_viewport().unwrap();
        // The fixed box stays at the viewport origin.
        assert_eq!(pixel(&viewport, 10, 10), (255, 0, 0));
        // The marker at document (300, 600) shows at viewport (50, 100).
        assert_eq!(pixel(&viewport, 55, 105), (0, 0, 255));
        assert_eq!(pixel(&viewport, 45, 105), (255, 255, 255));
        // Fixed boxes report client rects relative to the viewport.
        assert_eq!(
            engine.evaluate_js(
                "var r = document.getElementById('fixed').getBoundingClientRect(); JSON.stringify([r.x, r.y])"
            ),
            "[0,0]"
        );
        // The full-page capture keeps the document origin and viewport width.
        let full = engine.screenshot().unwrap();
        assert_eq!(full.width(), 800);
        assert_eq!(pixel(&full, 305, 605), (0, 0, 255));
    }

    #[test]
    fn test_crop_viewport_pads_past_the_source_edge_with_white() {
        // 2x2 source: every pixel opaque black.
        let src = [0u8, 0, 0, 255].repeat(4);
        let out = crop_viewport(&src, 2, 2, 1, 1, 2, 2);
        assert_eq!(&out[0..4], &[0, 0, 0, 255]);
        assert_eq!(&out[4..8], &[255, 255, 255, 255]);
        assert_eq!(&out[8..16], &[255u8; 8]);
    }
}
