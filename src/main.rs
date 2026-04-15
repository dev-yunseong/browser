use eframe::egui;
use poll_promise::Promise;
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
pub mod engine;

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

    image_promises: HashMap<String, Promise<Result<(String, Vec<u8>), String>>>,
    is_loading: bool,
    start_time: std::time::Instant,

    /// Headless browser engine — owns all pipeline state.
    engine: engine::BrowserEngine,
}

/// Type alias — the GUI layer calls the pipeline result `StaticPageData` internally.
type StaticPageData = engine::PageResult;

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
            image_promises: HashMap::new(),
            is_loading: false,
            start_time: std::time::Instant::now(),
            engine: engine::BrowserEngine::new(),
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
        self.hovered_id = None;
        self.is_loading = true;
        self.engine.clear_for_new_url();

        let mut cache = self.engine.css_cache.clone();
        self.content_promise = Some(Promise::spawn_thread("fetcher", move || {
            engine::fetch_and_process(&url, &mut cache, &HashMap::new(), None, None, width)
                .map_err(|e| e.to_string())
        }));
    }

    /// Re-render the current page using `engine.js_style_overrides` and `engine.image_cache`.
    fn trigger_re_render(&mut self, ctx: &egui::Context, width: f32) {
        let (body, base_url) = match self.engine.last_page.as_ref() {
            Some(p) => (p.body.clone(), p.base_url.clone()),
            None => return,
        };
        let cache = self.engine.image_cache.clone();
        let mut css_cache = self.engine.css_cache.clone();
        let stylesheet = self.engine.last_stylesheet.clone();
        let overrides = self.engine.js_style_overrides.clone();
        let hovered_id = self.hovered_id.clone();
        let focused_id = self.focused_id.clone();
        let csp_policy = self.engine.current_csp_policy.clone();

        // Use re_render_promise instead of join() to avoid UI blocking
        self.re_render_promise = Some(Promise::spawn_thread("re_render", move || {
            engine::process_html_with_cache(
                &body, &base_url, &cache, &mut css_cache, stylesheet, &overrides,
                hovered_id.as_deref(), focused_id.as_deref(), csp_policy, width,
            )
            .map_err(|e| e.to_string())
        }));

        // Request repaint to check promise
        ctx.request_repaint();
    }

    fn apply_page_data(&mut self, page_data: StaticPageData, ctx: &egui::Context) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [page_data.width as usize, page_data.height as usize],
            &page_data.pixmap_bytes,
        );
        self.texture = Some(ctx.load_texture("page_content", image, Default::default()));
        self.current_links = page_data.links.clone();
        self.current_form_controls = page_data.form_controls.clone();
        self.current_event_handlers = page_data.event_handlers.clone();
        self.current_element_ids = page_data.element_ids.clone();
        self.current_focusable_elements = page_data.focusable_elements.clone();
        // csp_policy is stored on engine.current_csp_policy
    }
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

        // Poll JS event loop tasks (Macro, Micro, rAF, Idle)
        let timestamp = self.start_time.elapsed().as_secs_f64() * 1000.0;
        let mut deadline = None;
        if self.content_promise.is_none() && self.re_render_promise.is_none() {
            deadline = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64 + 16.0);
        }
        
        // Sync focus to JS
        self.engine.js_runtime.set_focused_node_id(self.focused_id.clone());

        let mut needs_re_render = self.engine.js_runtime.tick(Some(timestamp), deadline);

        // Sync focus FROM JS (if changed via element.focus())
        if let Some(js_focused_id) = self.engine.js_runtime.get_focused_node_id() {
            if self.focused_id.as_deref() != Some(&js_focused_id) {
                self.focused_id = Some(js_focused_id);
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
                let overrides = self.engine.js_runtime.get_style_overrides();
                if !overrides.is_empty() {
                    for (id, props) in overrides {
                        self.engine.js_style_overrides.entry(id).or_default().extend(props);
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
                            self.engine.last_stylesheet = Some(stylesheet.clone());
                            self.engine.last_page = Some(page_data.clone());
                            self.apply_page_data(page_data.clone(), ctx);
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
                            // Clone everything we need before releasing the borrow on content_promise
                            let page_data = page_data.clone();
                            let image_urls = page_data.image_urls.clone();
                            let body = page_data.body.clone();
                            self.engine.last_stylesheet = Some(stylesheet.clone());
                            self.engine.current_csp_policy = page_data.csp_policy.clone();
                            self.engine.last_page = Some(page_data.clone());

                            // Release the borrow on content_promise
                            self.content_promise = None;
                            self.is_loading = false;

                            // Update stored GUI state
                            self.form_values.clear();
                            for (i, (_, val)) in page_data.form_controls.iter().enumerate() {
                                self.form_values.insert(i.to_string(), val.clone());
                            }
                            self.apply_page_data(page_data.clone(), ctx);

                            // Start async image fetches
                            for url in &image_urls {
                                if !self.engine.image_cache.contains_key(url) && !self.image_promises.contains_key(url) {
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

                            // Initialize JS runtime and execute page scripts
                            self.engine.init_js_for_page(&body);
                            let overrides = self.engine.js_runtime.get_style_overrides();
                            if !overrides.is_empty() {
                                for (id, props) in overrides {
                                    self.engine.js_style_overrides.entry(id).or_default().extend(props);
                                }
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
                            self.engine.image_cache.insert(url.clone(), bytes.clone());
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
                                        self.engine.js_runtime.trigger_event(id, "click");
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
                        self.engine.js_runtime.execute(script);
                    }
                    if !scripts_to_run.is_empty() {
                        let overrides = self.engine.js_runtime.get_style_overrides();
                        if !overrides.is_empty() {
                            for (id, props) in overrides {
                                self.engine.js_style_overrides.entry(id).or_default().extend(props);
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

#[cfg(test)]
mod main_tests {
    use super::*;

    #[test]
    fn test_collect_css_in_order() {
        let html = r#"
            <html>
                <head>
                    <style>body { color: red; }</style>
                    <link rel="stylesheet" href="style.css">
                </head>
            </html>
        "#;
        let dom = dom::parse_html(html);
        let base_url = Url::parse("https://example.com").unwrap();
        let cache = HashMap::new();
        let mut sources = Vec::new();
        engine::collect_css_in_order(&dom.document, &base_url, &cache, &mut sources);

        assert_eq!(sources.len(), 2);
        match &sources[0] {
            engine::CssSource::Inline(text) => assert!(text.contains("red")),
            _ => panic!("Expected inline style"),
        }
        match &sources[1] {
            engine::CssSource::Remote(url) => assert_eq!(url, "https://example.com/style.css"),
            _ => panic!("Expected remote style"),
        }
    }
}
