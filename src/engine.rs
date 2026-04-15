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
#[derive(Clone, Debug)]
pub enum ClickResult {
    /// A link was clicked; contains the absolute URL.
    Navigate(String),
    /// An `onclick` script handler was executed.
    ScriptExecuted,
    /// A focusable element received focus; contains its ID.
    FocusChanged(String),
    /// No interactive element at the given position.
    Nothing,
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
                results.push(ClickResult::FocusChanged(id.clone()));
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
                results.push(ClickResult::Navigate(link.clone()));
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
}
