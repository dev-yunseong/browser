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
pub mod js;

struct BrowserApp {
    url: String,
    history: Vec<String>,
    history_index: usize,
    content_promise: Option<Promise<Result<StaticPageData, String>>>,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    current_links: Vec<(layout::Rect, String)>,
    current_form_controls: Vec<(layout::Rect, String)>,
    current_event_handlers: Vec<(layout::Rect, String)>,
    current_element_ids: Vec<(layout::Rect, String)>,
    hovered_id: Option<String>,
    form_values: HashMap<usize, String>,
    image_cache: HashMap<String, Vec<u8>>,
    image_promises: HashMap<String, Promise<Result<(String, Vec<u8>), String>>>,
    last_body: String,
    last_base_url: Option<Url>,
    js_runtime: js::JsRuntime,
    /// Accumulated JS style overrides (element id → property → value).
    js_style_overrides: HashMap<String, HashMap<String, String>>,
    is_loading: bool,
}

struct StaticPageData {
    pixmap_bytes: Vec<u8>,
    width: u32,
    height: u32,
    links: Vec<(layout::Rect, String)>,
    form_controls: Vec<(layout::Rect, String)>,
    event_handlers: Vec<(layout::Rect, String)>,
    element_ids: Vec<(layout::Rect, String)>,
    image_urls: Vec<String>,
    body: String,
    base_url: Url,
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
            texture: None,
            error: None,
            current_links: vec![],
            current_form_controls: vec![],
            current_event_handlers: vec![],
            current_element_ids: vec![],
            hovered_id: None,
            form_values: HashMap::new(),
            image_cache: HashMap::new(),
            image_promises: HashMap::new(),
            last_body: String::new(),
            last_base_url: None,
            js_runtime: js::JsRuntime::new(None),
            js_style_overrides: HashMap::new(),
            is_loading: false,
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
        self.js_runtime = js::JsRuntime::new(None);
        self.is_loading = true;
        self.hovered_id = None;
        self.content_promise = Some(Promise::spawn_thread("fetcher", move || {
            fetch_and_process(&url, &HashMap::new(), &HashMap::new(), None, width)
                .map_err(|e| e.to_string())
        }));
    }

    /// Re-render the current page using `self.js_style_overrides` and `self.image_cache`.
    fn trigger_re_render(&mut self, ctx: &egui::Context, width: f32) {
        if let Some(base_url) = self.last_base_url.clone() {
            let body = self.last_body.clone();
            let cache = self.image_cache.clone();
            let overrides = self.js_style_overrides.clone();
            let hovered_id = self.hovered_id.clone();

            let join_handle = std::thread::spawn(move || {
                process_html_with_cache(&body, &base_url, &cache, &overrides, hovered_id.as_deref(), width)
            });

            match join_handle.join() {
                Ok(Ok(page_data)) => {
                    self.apply_page_data(page_data, ctx);
                }
                Ok(Err(e)) => {
                    self.error = Some(format!("Re-render error: {}", e));
                }
                Err(_) => {
                    self.error = Some("Render thread panicked".to_string());
                }
            }
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
    }
}

fn collect_css_in_order(handle: &markup5ever_rcdom::Handle, base_url: &Url, css_source: &mut String) {
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
                    if let Ok(resp) = reqwest::blocking::get(&abs_url) {
                        if let Ok(text) = resp.text() {
                            css_source.push_str(&text);
                        }
                    }
                }
            }
        }
    }
    for child in handle.children.borrow().iter() {
        collect_css_in_order(child, base_url, css_source);
    }
}

fn fetch_and_process(
    url_str: &str,
    image_cache: &HashMap<String, Vec<u8>>,
    js_overrides: &HashMap<String, HashMap<String, String>>,
    hovered_id: Option<&str>,
    width: f32,
) -> Result<StaticPageData, Box<dyn Error + Send + Sync>> {
    let response = reqwest::blocking::get(url_str)?;
    let body = response.text()?;
    let base_url = Url::parse(url_str)?;
    process_html_with_cache(&body, &base_url, image_cache, js_overrides, hovered_id, width)
}

fn process_html_with_cache(
    body: &str,
    base_url: &Url,
    image_cache: &HashMap<String, Vec<u8>>,
    js_overrides: &HashMap<String, HashMap<String, String>>,
    hovered_id: Option<&str>,
    width: f32,
) -> Result<StaticPageData, Box<dyn Error + Send + Sync>> {
    let width = width.max(1.0);
    let dom_tree = dom::parse_html(body);

    // Collect inline + external CSS in order (C6)
    let mut css_source = String::new();
    collect_css_in_order(&dom_tree.document, base_url, &mut css_source);

    let stylesheet = css::parse_css(&css_source);
    let style_tree = style::build_style_tree(&dom_tree.document, &stylesheet, None, js_overrides, hovered_id);

    let (layout_tree, _, final_y) =
        layout::build_layout_tree(&style_tree, 0.0, 0.0, 0.0, width, width, 768.0);

    let height = (final_y.ceil() as u32).clamp(600, 16384);
    let w_u32 = width as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w_u32, height)
        .ok_or_else(|| format!("Failed to create pixmap with size {}x{}", w_u32, height))?;

    pixmap.fill(tiny_skia::Color::WHITE);

    let mut links: Vec<(layout::Rect, String)> = Vec::new();
    let mut form_controls = Vec::new();
    let mut event_handlers = Vec::new();
    let mut element_ids = Vec::new();
    let mut image_urls = Vec::new();

    if let Some(ref root) = layout_tree {
        render::render_layout_tree(root, &mut pixmap, image_cache);

        root.collect_links(&mut links);
        
        root.collect_event_handlers(&mut event_handlers);

        root.collect_element_ids(&mut element_ids);
        
        let mut controls_with_nodes = Vec::new();
        root.collect_form_controls(&mut controls_with_nodes);
        
        let mut image_urls_raw = Vec::new();
        root.collect_images(&mut image_urls_raw);
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
    }

    let absolute_links = links.into_iter().map(|(rect, link)| {
        let abs = base_url.join(&link).map(|u| u.to_string()).unwrap_or(link);
        (rect, abs)
    }).collect();

    Ok(StaticPageData {
        pixmap_bytes: pixmap.data().to_vec(),
        width: width as u32,
        height,
        links: absolute_links,
        form_controls,
        event_handlers,
        element_ids,
        image_urls,
        body: body.to_string(),
        base_url: base_url.clone(),
    })
}

impl eframe::App for BrowserApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                // Periodically run any queued JS macro-tasks (setTimeout, etc.)
                self.js_runtime.run_queued_tasks();
                let overrides = self.js_runtime.get_style_overrides();
                if !overrides.is_empty() {
                    for (id, props) in overrides {
                        self.js_style_overrides.entry(id).or_default().extend(props);
                    }
                    self.trigger_re_render(ctx, ui.available_width());
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
                        Some(Ok(page_data)) => {
                            // Clone what we need before releasing the borrow
                            let body = page_data.body.clone();
                            let base_url = page_data.base_url.clone();
                            let image_urls = page_data.image_urls.clone();
                            let page = StaticPageData {
                                pixmap_bytes: page_data.pixmap_bytes.clone(),
                                width: page_data.width,
                                height: page_data.height,
                                links: page_data.links.clone(),
                                form_controls: page_data.form_controls.clone(),
                                event_handlers: page_data.event_handlers.clone(),
                                element_ids: page_data.element_ids.clone(),
                                image_urls: image_urls.clone(),
                                body: body.clone(),
                                base_url: base_url.clone(),
                            };

                            // Release the borrow on content_promise
                            self.content_promise = None;
                            self.is_loading = false;

                            // Update stored page state
                            self.last_body = body.clone();
                            self.last_base_url = Some(base_url.clone());
                            self.form_values.clear();
                            for (i, (_, val)) in page.form_controls.iter().enumerate() {
                                self.form_values.insert(i, val.clone());
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
                            self.js_runtime = js::JsRuntime::new(Some(dom.document.clone()));
                            let scripts = js::extract_scripts_from_dom(&dom.document);
                            for script in scripts {
                                self.js_runtime.execute(&script);
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
                            let val = self.form_values.entry(i).or_default();
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
