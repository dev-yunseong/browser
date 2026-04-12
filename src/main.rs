use eframe::egui;
use poll_promise::Promise;
use std::error::Error;
use url::Url;
use std::collections::HashMap;

pub mod dom;
pub mod css;
pub mod style;
pub mod layout;
pub mod render;
pub mod layer_tree;
pub mod js;
pub mod matrix;

struct BrowserApp {
    url: String,
    history: Vec<String>,
    history_index: usize,
    content_promise: Option<Promise<Result<(StaticPageData, css::Stylesheet), String>>>,
    re_render_promise: Option<Promise<Result<(StaticPageData, css::Stylesheet), String>>>,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    current_links: Vec<(layout::Rect, String)>,
    current_form_controls: Vec<(layout::Rect, String)>,
    current_event_handlers: Vec<(layout::Rect, String)>,
    current_element_ids: Vec<(layout::Rect, String)>,
    current_focusable_elements: Vec<(layout::Rect, String)>,
    hovered_id: Option<String>,
    focused_id: Option<String>,
    form_values: HashMap<String, String>,

    image_cache: HashMap<String, Vec<u8>>,
    css_cache: HashMap<String, String>,
    last_stylesheet: Option<css::Stylesheet>,
    image_promises: HashMap<String, Promise<Result<(String, Vec<u8>), String>>>,
    last_body: String,
    last_base_url: Option<Url>,
    js_runtime: js::JsRuntime,
    current_csp_policy: Option<js::CspPolicy>,
    /// Accumulated JS style overrides (element id → property → value).
    js_style_overrides: HashMap<String, HashMap<String, String>>,
    is_loading: bool,
    start_time: std::time::Instant,
}

struct StaticPageData {
    pixmap_bytes: Vec<u8>,
    width: u32,
    height: u32,
    links: Vec<(layout::Rect, String)>,
    form_controls: Vec<(layout::Rect, String)>,
    event_handlers: Vec<(layout::Rect, String)>,
    element_ids: Vec<(layout::Rect, String)>,
    focusable_elements: Vec<(layout::Rect, String)>,
    image_urls: Vec<String>,
    body: String,
    base_url: Url,
    csp_policy: Option<js::CspPolicy>,
}

impl BrowserApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Load Korean font
        let mut fonts = egui::FontDefinitions::default();
        let nanum_data = include_bytes!("../assets/fonts/NanumGothic.ttf");
        fonts.font_data.insert(
            "nanum".to_owned(),
            egui::FontData::from_static(nanum_data),
        );
        fonts.families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "nanum".to_owned());
        fonts.families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .push("nanum".to_owned());
        cc.egui_ctx.set_fonts(fonts);

        Self {
            url: "https://yunseong.dev".to_string(),
            history: vec![],
            history_index: 0,
            content_promise: None,
            re_render_promise: None,
            texture: None,
            error: None,
            current_links: vec![],
            current_form_controls: vec![],
            current_event_handlers: vec![],
            current_element_ids: vec![],
            current_focusable_elements: vec![],
            hovered_id: None,
            focused_id: None,
            form_values: HashMap::new(),
            image_cache: HashMap::new(),
            css_cache: HashMap::new(),
            last_stylesheet: None,
            image_promises: HashMap::new(),
            last_body: String::new(),
            last_base_url: None,
            js_runtime: js::JsRuntime::new(None, None, None),
            current_csp_policy: None,
            js_style_overrides: HashMap::new(),
            is_loading: false,
            start_time: std::time::Instant::now(),
        }
    }

    fn load_url(&mut self, url: String, width: f32) {
        if self.history.is_empty() || self.history[self.history_index] != url {
            self.history.truncate(self.history_index + 1);
            self.history.push(url.clone());
            self.history_index = self.history.len() - 1;
        }
        self.load_url_direct(url, width);
    }

    fn navigate_back(&mut self, width: f32) {
        if self.history_index > 0 {
            self.history_index -= 1;
            let url = self.history[self.history_index].clone();
            self.load_url_direct(url, width);
        }
    }

    fn navigate_forward(&mut self, width: f32) {
        if self.history_index + 1 < self.history.len() {
            self.history_index += 1;
            let url = self.history[self.history_index].clone();
            self.load_url_direct(url, width);
        }
    }

    fn load_url_direct(&mut self, url: String, width: f32) {
        self.url = url.clone();
        self.error = None;
        self.image_promises.clear();
        self.js_style_overrides.clear();
        let base_url = Url::parse(&url).ok();
        self.js_runtime = js::JsRuntime::new(None, base_url, None);
        self.current_csp_policy = None;
        self.is_loading = true;
        self.hovered_id = None;
        self.last_stylesheet = None; // Reset stylesheet on new URL
        self.css_cache.clear();      // Optional: clear cache or keep it? Let's clear for now to be safe.
        
        let mut cache = self.css_cache.clone();
        self.content_promise = Some(Promise::spawn_thread("fetcher", move || {
            fetch_and_process(&url, &mut cache, &HashMap::new(), None, None, width)
                .map_err(|e| e.to_string())
        }));
    }

    /// Re-render the current page using `self.js_style_overrides` and `self.image_cache`.
    fn trigger_re_render(&mut self, ctx: &egui::Context, width: f32) {
        if let Some(base_url) = self.last_base_url.clone() {
            let body = self.last_body.clone();
            let cache = self.image_cache.clone();
            let mut css_cache = self.css_cache.clone();
            let stylesheet = self.last_stylesheet.clone();
            let overrides = self.js_style_overrides.clone();
            let hovered_id = self.hovered_id.clone();
            let focused_id = self.focused_id.clone();
            let csp_policy = self.current_csp_policy.clone();

            // Use re_render_promise instead of join() to avoid UI blocking
            self.re_render_promise = Some(Promise::spawn_thread("re_render", move || {
                process_html_with_cache(&body, &base_url, &cache, &mut css_cache, stylesheet, &overrides, hovered_id.as_deref(), focused_id.as_deref(), csp_policy, width)
                    .map_err(|e| e.to_string())
            }));
            
            // Request repaint to check promise
            ctx.request_repaint();
        }
    }

    fn apply_page_data(&mut self, page_data: StaticPageData, ctx: &egui::Context) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [page_data.width as usize, page_data.height as usize],
            &page_data.pixmap_bytes,
        );
        self.texture = Some(ctx.load_texture("page_content", image, Default::default()));
        self.current_links = page_data.links;
        self.current_form_controls = page_data.form_controls;
        self.current_event_handlers = page_data.event_handlers;
        self.current_element_ids = page_data.element_ids;
        self.current_focusable_elements = page_data.focusable_elements;
        self.current_csp_policy = page_data.csp_policy;
    }
}

use std::time::Instant;

fn collect_css_in_order(handle: &markup5ever_rcdom::Handle, base_url: &Url, css_source: &mut String, cache: &mut HashMap<String, String>) {
    if let markup5ever_rcdom::NodeData::Element { ref name, ref attrs, .. } = handle.data {
        let tag = name.local.to_string();
        if tag == "style" {
            for child in handle.children.borrow().iter() {
                if let markup5ever_rcdom::NodeData::Text { ref contents } = child.data {
                    css_source.push_str(&contents.borrow());
                }
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
                        css_source.push_str(cached);
                    } else {
                        let start = Instant::now();
                        if let Ok(resp) = reqwest::blocking::get(&abs_url) {
                            let elapsed = start.elapsed();
                            println!("[Perf] Network Fetch (CSS): {} in {:?}", abs_url, elapsed);
                            if let Ok(text) = resp.text() {
                                cache.insert(abs_url, text.clone());
                                css_source.push_str(&text);
                            }
                        } else {
                            println!("[Error] Failed to fetch CSS: {}", abs_url);
                        }
                    }
                }
            }
        }
    }
    for child in handle.children.borrow().iter() {
        collect_css_in_order(child, base_url, css_source, cache);
    }
}

fn fetch_and_process(
    url_str: &str,
    css_cache: &mut HashMap<String, String>,
    js_overrides: &HashMap<String, HashMap<String, String>>,
    hovered_id: Option<&str>,
    focused_id: Option<&str>,
    width: f32,
) -> Result<(StaticPageData, css::Stylesheet), Box<dyn Error + Send + Sync>> {
    let response = reqwest::blocking::get(url_str)?;
    let csp_header = response.headers().get("content-security-policy")
        .and_then(|h| h.to_str().ok())
        .map(|s| js::CspPolicy::parse(s));
    
    let body = response.text()?;
    let base_url = Url::parse(url_str)?;
    process_html_with_cache(&body, &base_url, &HashMap::new(), css_cache, None, js_overrides, hovered_id, focused_id, csp_header, width)
}

fn process_html_with_cache(
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
) -> Result<(StaticPageData, css::Stylesheet), Box<dyn Error + Send + Sync>> {
    let start_total = Instant::now();
    let width = width.max(1.0);
    
    let start = Instant::now();
    let dom_tree = dom::parse_html(body);
    let dom_elapsed = start.elapsed();

    let stylesheet = if let Some(s) = cached_stylesheet {
        s
    } else {
        // Collect inline + external CSS in order (C6)
        let start = Instant::now();
        let mut css_source = String::new();
        collect_css_in_order(&dom_tree.document, base_url, &mut css_source, css_cache);
        let css_collect_elapsed = start.elapsed();
        println!("  - CSS collect: {:?}", css_collect_elapsed);

        let start = Instant::now();
        let s = css::parse_css(&css_source);
        let css_parse_elapsed = start.elapsed();
        println!("  - CSS parse: {:?}", css_parse_elapsed);
        s
    };

    let start = Instant::now();
    let style_tree = style::build_style_tree(&dom_tree.document, &stylesheet, None, js_overrides, hovered_id, focused_id, csp_policy.as_ref());
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
    image_urls = image_urls_raw.into_iter().map(|(_, url)| {
        base_url.join(&url).map(|u| u.to_string()).unwrap_or(url)
    }).collect();

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
    let absolute_links = links.into_iter().map(|(rect, link)| {
        let abs = base_url.join(&link).map(|u| u.to_string()).unwrap_or(link);
        (rect, abs)
    }).collect();

    let pixmap_bytes = pixmap.data().to_vec();
    let data_copy_elapsed = start.elapsed();

    let total_elapsed = start_total.elapsed();
    
    println!("[Perf] process_html_with_cache total: {:?}", total_elapsed);
    println!("  - DOM parse: {:?}", dom_elapsed);
    println!("  - Style build: {:?}", style_elapsed);
    println!("  - Layout build: {:?}", layout_elapsed);
    println!("  - Render: {:?}", render_elapsed);
    println!("  - Data copy & Links: {:?}", data_copy_elapsed);

    Ok((StaticPageData {
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
    }, stylesheet))
}

impl eframe::App for BrowserApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle Tab navigation
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let focusables = &self.current_focusable_elements;
            if !focusables.is_empty() {
                let current_index = self.focused_id.as_ref()
                    .and_then(|id| focusables.iter().position(|(_, fid)| fid == id));
                
                let next_index = if ctx.input(|i| i.modifiers.shift) {
                    // Shift+Tab: Backward
                    match current_index {
                        Some(i) if i > 0 => i - 1,
                        _ => focusables.len() - 1,
                    }
                } else {
                    // Tab: Forward
                    match current_index {
                        Some(i) if i + 1 < focusables.len() => i + 1,
                        _ => 0,
                    }
                };
                
                self.focused_id = Some(focusables[next_index].1.clone());
                self.trigger_re_render(ctx, 800.0);
            }
        }

        // Poll JS event loop tasks
        let mut needs_re_render = self.js_runtime.poll_tasks();

        // Sync focus from JS
        if let Some(js_focused_id) = self.js_runtime.get_focused_node_id() {
            if self.focused_id.as_deref() != Some(&js_focused_id) {
                self.focused_id = Some(js_focused_id);
                needs_re_render = true;
            }
        }

        // Run animation frames
        let timestamp = self.start_time.elapsed().as_secs_f64() * 1000.0;
        if self.js_runtime.poll_raf_tasks(timestamp) {
            needs_re_render = true;
        }

        // Run idle tasks if no work was done and no long-running promises are active
        if !needs_re_render && self.content_promise.is_none() && self.re_render_promise.is_none() {
            let deadline = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64 + 50.0;
            if self.js_runtime.poll_idle_tasks(deadline) {
                needs_re_render = true;
            }
        }

        if needs_re_render {
            self.trigger_re_render(ctx, 800.0); // Using standard width
        }

        // ── Browser chrome ──────────────────────────────────────────────────
        let toolbar_fill = egui::Color32::from_rgb(50, 50, 55);
        let url_bar_fill = egui::Color32::from_rgb(72, 72, 78);

        egui::TopBottomPanel::top("browser_chrome")
            .frame(egui::Frame::none()
                .fill(toolbar_fill)
                .inner_margin(egui::Margin::symmetric(8.0, 6.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    // Back / Forward / Refresh buttons
                    let btn_style = |ui: &mut egui::Ui, label: &str, enabled: bool| -> egui::Response {
                        ui.add_enabled(
                            enabled,
                            egui::Button::new(
                                egui::RichText::new(label)
                                    .color(if enabled { egui::Color32::WHITE } else { egui::Color32::DARK_GRAY })
                                    .size(14.0)
                            )
                            .fill(egui::Color32::from_rgb(70, 70, 76))
                            .rounding(egui::Rounding::same(4.0))
                            .min_size(egui::vec2(28.0, 28.0)),
                        )
                    };

                    if btn_style(ui, "←", self.history_index > 0).clicked() {
                        self.navigate_back(ui.available_width());
                    }
                    if btn_style(ui, "→", self.history_index + 1 < self.history.len()).clicked() {
                        self.navigate_forward(ui.available_width());
                    }
                    if btn_style(ui, "⟳", true).clicked() {
                        let url = self.url.clone();
                        self.load_url_direct(url, ui.available_width());
                    }

                    ui.spacing_mut().item_spacing.x = 8.0;

                    // URL bar
                    let url_frame = egui::Frame::none()
                        .fill(url_bar_fill)
                        .rounding(egui::Rounding::same(14.0))
                        .inner_margin(egui::Margin::symmetric(10.0, 4.0));

                    url_frame.show(ui, |ui| {
                        ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                        let edit = egui::TextEdit::singleline(&mut self.url)
                            .desired_width(ui.available_width() - 60.0)
                            .frame(false)
                            .font(egui::TextStyle::Monospace);
                        let resp = ui.add(edit);
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let url = self.url.clone();
                            let width = ui.available_width();
                            if width >= 1.0 {
                                self.load_url(url, width);
                            }
                        }
                    });

                    if ui.add(
                        egui::Button::new(
                            egui::RichText::new("이동").color(egui::Color32::WHITE).size(13.0)
                        )
                        .fill(egui::Color32::from_rgb(0, 120, 212))
                        .rounding(egui::Rounding::same(14.0))
                        .min_size(egui::vec2(50.0, 28.0)),
                    ).clicked() {
                        let url = self.url.clone();
                        let width = ui.available_width();
                        if width >= 1.0 {
                            self.load_url(url, width);
                        }
                    }
                });

                // Loading progress bar
                if self.is_loading {
                    let progress = egui::ProgressBar::new(f32::INFINITY)
                        .animate(true)
                        .desired_width(ui.available_width())
                        .desired_height(3.0);
                    ui.add(progress);
                }
            });

        // ── Content area ────────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::WHITE))
            .show(ctx, |ui| {
                let overrides = self.js_runtime.get_style_overrides();
                if !overrides.is_empty() {
                    for (id, props) in overrides {
                        self.js_style_overrides.entry(id).or_default().extend(props);
                    }
                    self.trigger_re_render(ctx, ui.available_width());
                }

                // Handle pending re-render promise
                if let Some(promise) = &self.re_render_promise {
                    match promise.ready() {
                        None => {
                            ctx.request_repaint();
                        }
                        Some(Err(e)) => {
                            self.error = Some(format!("Re-render error: {}", e));
                            self.re_render_promise = None;
                        }
                        Some(Ok((page_data, stylesheet))) => {
                            self.last_stylesheet = Some(stylesheet.clone());
                            let page = StaticPageData {
                                pixmap_bytes: page_data.pixmap_bytes.clone(),
                                width: page_data.width,
                                height: page_data.height,
                                links: page_data.links.clone(),
                                form_controls: page_data.form_controls.clone(),
                                event_handlers: page_data.event_handlers.clone(),
                                element_ids: page_data.element_ids.clone(),
                                focusable_elements: page_data.focusable_elements.clone(),
                                image_urls: page_data.image_urls.clone(),
                                body: page_data.body.clone(),
                                base_url: page_data.base_url.clone(),
                                csp_policy: page_data.csp_policy.clone(),
                            };
                            self.apply_page_data(page, ctx);
                            self.re_render_promise = None;
                        }
                    }
                }

                // Handle pending page-load promise
                if let Some(promise) = &self.content_promise {
                    match promise.ready() {
                        None => {
                            ui.centered_and_justified(|ui| {
                                ui.spinner();
                            });
                            ctx.request_repaint();
                        }
                        Some(Err(e)) => {
                            self.error = Some(e.clone());
                            self.content_promise = None;
                            self.is_loading = false;
                        }
                        Some(Ok((page_data, stylesheet))) => {
                            // Clone what we need before releasing the borrow
                            let body = page_data.body.clone();
                            let base_url = page_data.base_url.clone();
                            let image_urls = page_data.image_urls.clone();
                            self.last_stylesheet = Some(stylesheet.clone());
                            
                            let page = StaticPageData {
                                pixmap_bytes: page_data.pixmap_bytes.clone(),
                                width: page_data.width,
                                height: page_data.height,
                                links: page_data.links.clone(),
                                form_controls: page_data.form_controls.clone(),
                                event_handlers: page_data.event_handlers.clone(),
                                element_ids: page_data.element_ids.clone(),
                                focusable_elements: page_data.focusable_elements.clone(),
                                image_urls: image_urls.clone(),
                                body: body.clone(),
                                base_url: base_url.clone(),
                                csp_policy: page_data.csp_policy.clone(),
                            };

                            // Release the borrow on content_promise
                            self.content_promise = None;
                            self.is_loading = false;

                            // Update stored page state
                            self.last_body = body.clone();
                            self.last_base_url = Some(base_url.clone());
                            self.form_values.clear();
                            for (i, (_, val)) in page.form_controls.iter().enumerate() {
                                self.form_values.insert(i.to_string(), val.clone());
                            }
                            self.apply_page_data(page, ctx);

                            // Start async image fetches
                            for url in &image_urls {
                                if !self.image_cache.contains_key(url) && !self.image_promises.contains_key(url) {
                                    let url_clone = url.clone();
                                    self.image_promises.insert(url.clone(), Promise::spawn_thread("img_fetcher", move || {
                                        match reqwest::blocking::get(&url_clone) {
                                            Ok(resp) => match resp.bytes() {
                                                Ok(bytes) => Ok((url_clone, bytes.to_vec())),
                                                Err(e) => Err(e.to_string()),
                                            },
                                            Err(e) => Err(e.to_string()),
                                        }
                                    }));
                                }
                            }

                            // Execute page scripts and apply any JS-driven style changes
                            let dom = dom::parse_html(&body);
                            self.js_runtime = js::JsRuntime::new(Some(dom.document.clone()), self.last_base_url.clone(), self.current_csp_policy.clone());
                            let scripts = js::extract_scripts_from_dom(&dom.document);
                            
                            let allowed = self.current_csp_policy.as_ref().map(|p| p.allows_inline_script()).unwrap_or(true);
                            if allowed {
                                for script in scripts {
                                    self.js_runtime.execute(&script);
                                }
                            } else {
                                println!("[CSP] Blocked inline script execution");
                            }
                            let overrides = self.js_runtime.get_style_overrides();
                            if !overrides.is_empty() {
                                self.js_style_overrides = overrides;
                                self.trigger_re_render(ctx, ui.available_width());
                            }
                        }
                    }
                }

                // Check resolved image promises → re-render when new images arrive
                let mut newly_loaded = false;
                self.image_promises.retain(|_url, promise| {
                    match promise.ready() {
                        Some(Ok((url, bytes))) => {
                            self.image_cache.insert(url.clone(), bytes.clone());
                            newly_loaded = true;
                            false
                        }
                        Some(Err(_)) => false,
                        None => true,
                    }
                });
                if newly_loaded {
                    let width = ui.available_width();
                    self.trigger_re_render(ctx, width);
                }

                // Error message
                if let Some(err) = &self.error {
                    ui.add_space(20.0);
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 50, 50),
                            format!("페이지를 불러올 수 없습니다: {}", err),
                        );
                    });
                }

                // Render page texture + interactive overlay
                let texture_info = self.texture.as_ref().map(|t| (t.id(), t.size_vec2()));
                if let Some((texture_id, texture_size)) = texture_info {
                    let mut url_to_load: Option<String> = None;
                    let mut scripts_to_run: Vec<String> = Vec::new();

                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let (rect, response) = ui.allocate_at_least(texture_size, egui::Sense::click());
                            ui.painter().image(
                                texture_id,
                                rect,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );

                        // Overlay interactive form controls
                        for (i, (l_rect, _)) in self.current_form_controls.iter().enumerate() {
                            let val = self.form_values.entry(i.to_string()).or_default();
                            let screen_rect = egui::Rect::from_min_size(
                                rect.min + egui::vec2(l_rect.x, l_rect.y),
                                egui::vec2(l_rect.width, l_rect.height),
                            );
                            ui.put(screen_rect, egui::TextEdit::singleline(val).id_source(i));
                        }

                        // Collect clicks (defer execution to after closure)
                        if response.clicked() {
                            if let Some(ptr) = response.interact_pointer_pos() {
                                let rel = ptr - rect.min;
                                
                                // Dispatch standard JS 'click' events for elements with IDs
                                for (l_rect, id) in &self.current_element_ids {
                                    if hit(rel, l_rect) {
                                        self.js_runtime.trigger_event(id, "click");
                                    }
                                }

                                // Update focused_id on click
                                let mut new_focus = None;
                                for (l_rect, id) in &self.current_focusable_elements {
                                    if hit(rel, l_rect) {
                                        new_focus = Some(id.clone());
                                    }
                                }
                                if new_focus != self.focused_id {
                                    self.focused_id = new_focus;
                                    self.trigger_re_render(ctx, ui.available_width());
                                }

                                for (l_rect, script) in &self.current_event_handlers {
                                    if hit(rel, l_rect) {
                                        scripts_to_run.push(script.clone());
                                    }
                                }
                                for (l_rect, link) in &self.current_links {
                                    if hit(rel, l_rect) {
                                        url_to_load = Some(link.clone());
                                        break;
                                    }
                                }
                            }
                        }

                        // Cursor: pointer on links and event handlers
                        if let Some(ptr) = response.hover_pos() {
                            let rel = ptr - rect.min;
                            let hovering = self.current_links.iter().any(|(r, _)| hit(rel, r))
                                || self.current_event_handlers.iter().any(|(r, _)| hit(rel, r));
                            if hovering {
                                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                            }

                            // Update hovered ID and re-render if changed
                            let mut new_hovered_id = None;
                            // Search in reverse to find the innermost (top-most in layout) element
                            for (l_rect, id) in self.current_element_ids.iter().rev() {
                                if hit(rel, l_rect) {
                                    new_hovered_id = Some(id.clone());
                                    break;
                                }
                            }
                            if new_hovered_id != self.hovered_id {
                                self.hovered_id = new_hovered_id;
                                let width = ui.available_width();
                                self.trigger_re_render(ctx, width);
                            }
                        } else if self.hovered_id.is_some() {
                            self.hovered_id = None;
                            let width = ui.available_width();
                            self.trigger_re_render(ctx, width);
                        }
                    });

                    // Execute collected onclick scripts and apply style changes
                    for script in &scripts_to_run {
                        println!("[JS Event] onclick: {}", &script[..script.len().min(80)]);
                        self.js_runtime.execute(script);
                    }
                    if !scripts_to_run.is_empty() {
                        let overrides = self.js_runtime.get_style_overrides();
                        if !overrides.is_empty() {
                            for (id, props) in overrides {
                                self.js_style_overrides.entry(id).or_default().extend(props);
                            }
                            self.trigger_re_render(ctx, ui.available_width());
                        }
                    }

                    if let Some(url) = url_to_load {
                        let width = ui.available_width();
                        self.load_url(url, width);
                    }
                }
            });
    }
}

/// Hit-test a relative pointer position against a layout rect.
#[inline]
fn hit(rel: egui::Vec2, r: &layout::Rect) -> bool {
    rel.x >= r.x && rel.x <= r.x + r.width && rel.y >= r.y && rel.y <= r.y + r.height
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 768.0])
            .with_title("Browser"),
        ..Default::default()
    };
    eframe::run_native(
        "Browser",
        options,
        Box::new(|cc| Ok(Box::new(BrowserApp::new(cc)))),
    )
}
