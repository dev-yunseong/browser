use markup5ever_rcdom;
use rayon::prelude::*;
use std::collections::HashMap;
use std::time::Instant;
use url::Url;

use crate::{css, dom, js, layout, render, style};

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
    js_overrides: &HashMap<String, HashMap<String, String>>,
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
    width: f32,
) -> Result<(PageResult, css::Stylesheet), Box<dyn std::error::Error + Send + Sync>> {
    let response = reqwest::blocking::get(url_str)?;
    let base_url = response.url().clone();
    let csp_header = response
        .headers()
        .get("content-security-policy")
        .and_then(|h| h.to_str().ok())
        .map(|s| js::CspPolicy::parse(s));

    let body = response.text()?;
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
                            (text, Some(url))
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
    let mut form_buttons = Vec::new();
    let mut form_control_names = Vec::new();
    let mut event_handlers = Vec::new();
    let mut element_ids = Vec::new();
    let mut focusable_elements = Vec::new();
    let mut layout_metrics = HashMap::new();
    let image_urls: Vec<String>;

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
        },
        stylesheet,
    ))
}

fn collect_layout_metrics(
    layout_tree: &layout::LayoutBox,
    out: &mut HashMap<String, js::LayoutMetrics>,
) {
    let mut stack = vec![layout_tree];
    while let Some(layout) = stack.pop() {
        if matches!(
            layout.style_node.node.data,
            markup5ever_rcdom::NodeData::Element { .. }
        ) {
            out.insert(
                js::node_path_key(&layout.style_node.node),
                js::LayoutMetrics {
                    x: layout.dimensions.x,
                    y: layout.dimensions.y,
                    width: layout.dimensions.width,
                    height: layout.dimensions.height,
                },
            );
        }
        for child in layout.children.iter().rev() {
            stack.push(child);
        }
    }
}

// ── BrowserEngine ─────────────────────────────────────────────────────────────

/// A headless browser engine with no GUI dependencies.
/// Owns all pipeline state: caches, JS runtime, and last rendered page.
pub struct BrowserEngine {
    pub image_cache: HashMap<String, Vec<u8>>,
    pub css_cache: HashMap<String, String>,
    pub last_stylesheet: Option<css::Stylesheet>,
    pub js_runtime: js::JsRuntime,
    pub console_buffer: js::ConsoleBuffer,
    pub current_csp_policy: Option<js::CspPolicy>,
    pub js_style_overrides: HashMap<String, HashMap<String, String>>,
    /// The most recently rendered page result.
    pub last_page: Option<PageResult>,
}

impl BrowserEngine {
    /// Create a new engine with empty caches and a fresh JS runtime.
    pub fn new_with_console(console_buffer: js::ConsoleBuffer) -> Self {
        Self {
            image_cache: HashMap::new(),
            css_cache: HashMap::new(),
            last_stylesheet: None,
            js_runtime: js::JsRuntime::new(None, None, None, None, console_buffer.clone()),
            console_buffer,
            current_csp_policy: None,
            js_style_overrides: HashMap::new(),
            last_page: None,
        }
    }

    pub fn new() -> Self {
        Self::new_with_console(js::new_console_buffer())
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
        self.init_js_for_page(&page);
        self.refresh_after_image_loads(page, width)
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
        self.js_runtime
            .set_layout_metrics(page.layout_metrics.clone());
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
        if !self.cache_missing_images(&page.image_urls) {
            return Ok(page);
        }

        let refreshed = self.re_render(None, None, width)?;
        Ok(refreshed)
    }

    /// Execute JavaScript in the current page's runtime and return a result/error.
    pub fn evaluate_js_with_result(&mut self, script: &str) -> js::EvalOutcome {
        let outcome = self.js_runtime.execute_with_result(script);
        let overrides = self.js_runtime.get_style_overrides();
        if !overrides.is_empty() {
            for (id, props) in overrides {
                self.js_style_overrides.entry(id).or_default().extend(props);
            }
        }
        outcome
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

        let scripts = js::extract_script_sources_from_dom(&dom.document, Some(&page.base_url));
        for script in scripts {
            match script {
                js::ScriptSource::InlineClassic(script) => {
                    let allowed = self
                        .current_csp_policy
                        .as_ref()
                        .map(|p| p.allows_inline_script())
                        .unwrap_or(true);
                    if allowed {
                        self.js_runtime.execute(&script);
                    } else {
                        println!("[CSP] Blocked inline script execution");
                    }
                }
                js::ScriptSource::ExternalClassic(url) => {
                    let allowed = self
                        .current_csp_policy
                        .as_ref()
                        .map(|p| p.is_allowed("script-src", &url, Some(&page.base_url)))
                        .unwrap_or(true);
                    if !allowed {
                        println!("[CSP] Blocked external script execution: {}", url);
                        continue;
                    }
                    match reqwest::blocking::get(url.as_str()).and_then(|response| response.text())
                    {
                        Ok(script) => self.js_runtime.execute(&script),
                        Err(err) => {
                            println!("[JS] Failed to load external script {}: {}", url, err)
                        }
                    }
                }
                js::ScriptSource::InlineModule { url, source } => {
                    let allowed = self
                        .current_csp_policy
                        .as_ref()
                        .map(|p| p.allows_inline_script())
                        .unwrap_or(true);
                    if !allowed {
                        println!("[CSP] Blocked inline module compilation: {}", url);
                        continue;
                    }
                    let outcome = self.js_runtime.compile_module_source(url.clone(), source);
                    if let Some(error) = outcome.error {
                        println!("[JS] Failed to compile inline module {}: {}", url, error);
                    }
                }
                js::ScriptSource::ExternalModule(url) => {
                    let outcome =
                        self.load_external_module_with(url.clone(), &page.base_url, |module_url| {
                            reqwest::blocking::get(module_url.as_str())
                                .and_then(|response| response.text())
                        });
                    if let Some(error) = outcome.error {
                        println!("[JS] Failed to load external module {}: {}", url, error);
                    }
                }
            }
        }

        let overrides = self.js_runtime.get_style_overrides();
        if !overrides.is_empty() {
            for (id, props) in overrides {
                self.js_style_overrides.entry(id).or_default().extend(props);
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

    /// Reset all state in preparation for navigating to a new URL.
    pub fn clear_for_new_url(&mut self) {
        self.js_style_overrides.clear();
        self.clear_console();
        let console = self.console_buffer.clone();
        drop_js_runtime_before_create(&mut self.js_runtime, || {
            js::JsRuntime::new(None, None, None, None, console)
        });
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
        self.js_runtime
            .tick(Some(timestamp.unwrap_or(0.0)), deadline)
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
        reply: mpsc::Sender<bool>,
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
                let _ = reply.send(result);
            }
            EngineCmd::EvaluateConsole { script, reply } => {
                let outcome = eng.evaluate_console_repl(&script);
                let _ = reply.send(outcome);
            }
            EngineCmd::Screenshot { reply } => {
                let png = eng.screenshot().and_then(|pm| pm.encode_png().ok());
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
                let needs = eng.tick_js(Some(timestamp), deadline);
                let overrides = eng.get_style_overrides();
                for (id, props) in overrides {
                    eng.js_style_overrides.entry(id).or_default().extend(props);
                }
                let _ = reply.send(needs);
            }
            EngineCmd::Submit { reply } => {
                let _ = reply.send(eng.submit_form());
            }
            EngineCmd::Shutdown => break,
        }
    }
}

/// Cloneable handle used by GUI and HTTP threads to send commands to the engine actor.
#[derive(Clone)]
pub struct EngineHandle {
    pub tx: mpsc::SyncSender<EngineCmd>,
    pub console_buffer: js::ConsoleBuffer,
}

impl EngineHandle {
    /// Create a new engine actor and return a handle to it.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::sync_channel::<EngineCmd>(64);
        let console_buffer = js::new_console_buffer();
        let actor_console = console_buffer.clone();
        std::thread::Builder::new()
            .name("engine-actor".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut eng = BrowserEngine::new_with_console(actor_console);
                run_engine_actor_with_engine(rx, &mut eng);
            })
            .expect("failed to start engine actor thread");
        Self { tx, console_buffer }
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

    pub fn send_get_page(&self) -> Option<ApiPageResponse> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx.send(EngineCmd::GetPage { reply: reply_tx }).ok()?;
        reply_rx.recv().ok()?
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

    pub fn send_tick(&self, timestamp: f64, deadline: Option<f64>) -> bool {
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
            return false;
        }
        reply_rx.recv().unwrap_or(false)
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
    fn test_browser_engine_new() {
        let engine = BrowserEngine::new();
        assert!(engine.last_page.is_none());
        assert!(engine.image_cache.is_empty());
        assert!(engine.css_cache.is_empty());
        assert!(engine.js_style_overrides.is_empty());
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
            &HashMap::new(),
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
            &HashMap::new(),
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
            &HashMap::new(),
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
            &HashMap::new(),
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
            &HashMap::new(),
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
            &HashMap::new(),
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
            &HashMap::new(),
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
            &HashMap::new(),
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
}
