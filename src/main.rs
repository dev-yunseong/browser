use eframe::egui;
use poll_promise::Promise;
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
    content_promise: Option<Promise<Result<engine::PageResult, String>>>,
    re_render_promise: Option<Promise<Result<engine::PageResult, String>>>,
    /// Bounded JS tick promise — prevents unbounded thread spawns per frame.
    tick_promise: Option<Promise<bool>>,
    /// Pending click result — triggers re-render when a ScriptExecuted click resolves.
    click_promise: Option<Promise<Vec<engine::ClickResult>>>,
    submit_promise: Option<Promise<Option<String>>>,
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
    console_entries: Vec<js::ConsoleEntry>,
    console_panel_open: bool,
    /// Current text in the REPL input field.
    console_input: String,
    /// History of previously evaluated expressions (for arrow-key navigation).
    console_history: Vec<String>,
    /// Index into `console_history` while navigating with arrow keys; `None` means fresh input.
    console_history_index: Option<usize>,
    /// In-flight promise for a console REPL evaluation.
    console_eval_promise: Option<Promise<js::EvalOutcome>>,
    /// True once a page has been successfully rendered (needed to decide whether to re-render after eval).
    has_page: bool,

    /// Engine actor handle — all pipeline work is delegated through this.
    engine: engine::EngineHandle,

    /// The single authoritative viewport width for all layout and re-render operations.
    ///
    /// Set once on each navigation (to 800.0 px) and reused verbatim for every
    /// subsequent re-render triggered by hover, focus, image-load, or JS eval.
    /// This prevents layout jitter caused by measuring `ui.available_width()` from
    /// different egui containers, which varies slightly frame-to-frame.
    viewport_width: f32,
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
            tick_promise: None,
            click_promise: None,
            submit_promise: None,
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
            console_entries: vec![],
            console_panel_open: false,
            console_input: String::new(),
            console_history: vec![],
            console_history_index: None,
            console_eval_promise: None,
            has_page: false,
            engine: engine::EngineHandle::spawn(),
            viewport_width: 800.0,
        }
    }

    fn load_url(&mut self, url: String) {
        let resolved = engine::resolve_url(&url);
        if self.history.is_empty() || self.history[self.history_index] != resolved {
            self.history.truncate(self.history_index + 1);
            self.history.push(resolved.clone());
            self.history_index = self.history.len() - 1;
        }
        self.load_url_direct(resolved);
    }

    fn navigate_back(&mut self) {
        if self.history_index > 0 {
            self.history_index -= 1;
            let url = self.history[self.history_index].clone();
            self.load_url_direct(url);
        }
    }

    fn navigate_forward(&mut self) {
        if self.history_index + 1 < self.history.len() {
            self.history_index += 1;
            let url = self.history[self.history_index].clone();
            self.load_url_direct(url);
        }
    }

    fn load_url_direct(&mut self, url: String) {
        // Fix viewport_width at 800.0 on every navigation so all subsequent
        // re-renders (hover, focus, image-load, JS eval) use the exact same width
        // and never cause layout jitter.
        self.viewport_width = 800.0;
        println!("[Viewport] navigate width={:.1}", self.viewport_width);

        self.url = url.clone();
        self.error = None;
        self.image_promises.clear();
        self.hovered_id = None;
        self.is_loading = true;
        self.has_page = false;

        // Evict the glyph rasterization cache so that memory is freed between
        // navigations and does not grow without bound across many page loads.
        render::clear_glyph_cache();

        let width = self.viewport_width;
        let handle = self.engine.clone();
        self.content_promise = Some(Promise::spawn_thread("fetcher", move || {
            handle.send_navigate(url, width)
        }));
    }

    /// Re-render the current page (e.g. after hover/focus state change).
    ///
    /// Always uses `self.viewport_width` — the single authoritative width set at
    /// navigate time — so that layout is bit-identical across all re-render triggers.
    fn trigger_re_render(&mut self, ctx: &egui::Context, trigger: &str) {
        let width = self.viewport_width;
        println!("[Viewport] re-render trigger={} width={:.1}", trigger, width);

        let handle = self.engine.clone();
        let hovered_id = self.hovered_id.clone();
        let focused_id = self.focused_id.clone();

        self.re_render_promise = Some(Promise::spawn_thread("re_render", move || {
            handle.send_re_render(hovered_id, focused_id, width)
        }));

        ctx.request_repaint();
    }

    fn apply_page_data(&mut self, page_data: engine::PageResult, ctx: &egui::Context) {
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
        self.has_page = true;
    }
}

impl eframe::App for BrowserApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // JS tick — at most one in-flight at a time to prevent unbounded thread spawns.
        let timestamp = self.start_time.elapsed().as_secs_f64() * 1000.0;
        if self.tick_promise.is_none()
            && self.content_promise.is_none()
            && self.re_render_promise.is_none()
        {
            let handle = self.engine.clone();
            self.tick_promise = Some(Promise::spawn_thread("tick", move || {
                handle.send_tick(timestamp, None)
            }));
        }
        if let Some(tick_p) = &self.tick_promise {
            if let Some(needs) = tick_p.ready() {
                if *needs {
                    ctx.request_repaint();
                }
                self.tick_promise = None;
            }
        }

        self.console_entries = self.engine.send_get_console();

        // Poll console eval promise; trigger re-render if a page is loaded
        if let Some(eval_promise) = &self.console_eval_promise {
            if eval_promise.ready().is_some() {
                self.console_eval_promise = None;
                if self.has_page
                    && self.re_render_promise.is_none()
                    && self.content_promise.is_none()
                {
                    self.trigger_re_render(ctx, "console-eval");
                } else {
                    ctx.request_repaint();
                }
            }
        }

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
                self.trigger_re_render(ctx, "tab-focus");
            }
        }

        // ── Browser chrome ──────────────────────────────────────────────────────────
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
                        self.navigate_back();
                    }
                    if btn_style(ui, "→", self.history_index + 1 < self.history.len()).clicked() {
                        self.navigate_forward();
                    }
                    if btn_style(ui, "⟳", true).clicked() {
                        let url = self.url.clone();
                        self.load_url_direct(url);
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
                            self.load_url(url);
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
                        self.load_url(url);
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

        render_console_panel(
            ctx,
            &mut self.console_panel_open,
            &mut self.console_entries,
            &mut self.console_input,
            &mut self.console_history,
            &mut self.console_history_index,
            || self.engine.send_clear_console(),
            |code| {
                if self.console_eval_promise.is_none() {
                    let handle = self.engine.clone();
                    self.console_eval_promise =
                        Some(Promise::spawn_thread("console_eval", move || {
                            handle.send_console_eval_result(code)
                        }));
                }
            },
        );

        // ── Content area ─────────────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::WHITE))
            .show(ctx, |ui| {
                // Poll click promise — trigger re-render if onclick fired JS style changes
                if let Some(click_p) = &self.click_promise {
                    if let Some(results) = click_p.ready() {
                        let had_script = results.iter().any(|r| matches!(r, engine::ClickResult::ScriptExecuted));
                        self.click_promise = None;
                        if had_script {
                            self.trigger_re_render(ctx, "click-script");
                        }
                    }
                }

                if let Some(submit_p) = &self.submit_promise {
                    if let Some(maybe_url) = submit_p.ready() {
                        let url = maybe_url.clone();
                        self.submit_promise = None;
                        if let Some(url) = url {
                            self.load_url(url);
                        }
                    }
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
                        Some(Ok(page_data)) => {
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
                            self.has_page = false;
                        }
                        Some(Ok(page_data)) => {
                            // Clone everything we need before releasing the borrow on content_promise
                            let page_data = page_data.clone();
                            let image_urls = page_data.image_urls.clone();

                            self.is_loading = false;
                            self.content_promise = None;

                            // Update stored GUI state
                            self.form_values.clear();
                            for (i, (_, val)) in page_data.form_controls.iter().enumerate() {
                                self.form_values.insert(i.to_string(), val.clone());
                            }
                            self.apply_page_data(page_data, ctx);

                            // Start async image fetches
                            for url in &image_urls {
                                if !self.image_promises.contains_key(url) {
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
                        }
                    }
                }

                // Check resolved image promises → send to engine actor, re-render when new images arrive
                let mut newly_loaded = false;
                let engine_handle = self.engine.clone();
                self.image_promises.retain(|_url, promise| {
                    match promise.ready() {
                        Some(Ok((url, bytes))) => {
                            let _ = engine_handle.tx.send(engine::EngineCmd::LoadImage {
                                url: url.clone(),
                                bytes: bytes.clone(),
                            });
                            newly_loaded = true;
                            false
                        }
                        Some(Err(_)) => false,
                        None => true,
                    }
                });
                if newly_loaded {
                    self.trigger_re_render(ctx, "image-load");
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

                        // Enter-key form submission: when the user presses Enter
                        // while a form control is focused, submit the form.
                        if !self.current_form_controls.is_empty()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            && self.submit_promise.is_none()
                            && self.click_promise.is_none()
                            && self.content_promise.is_none()
                        {
                            let handle = self.engine.clone();
                            self.submit_promise = Some(Promise::spawn_thread(
                                "submit",
                                move || handle.send_submit(),
                            ));
                        }

                        // Collect clicks (defer execution to after closure)
                        if response.clicked() {
                            if let Some(ptr) = response.interact_pointer_pos() {
                                let rel = ptr - rect.min;

                                // Dispatch full click to engine actor
                                if self.click_promise.is_none() {
                                    let handle = self.engine.clone();
                                    let rel_x = rel.x;
                                    let rel_y = rel.y;
                                    self.click_promise = Some(Promise::spawn_thread(
                                        "click",
                                        move || handle.send_click(rel_x, rel_y),
                                    ));
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
                                    self.trigger_re_render(ctx, "click-focus");
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
                                self.trigger_re_render(ctx, "hover-enter");
                            }
                        } else if self.hovered_id.is_some() {
                            self.hovered_id = None;
                            self.trigger_re_render(ctx, "hover-leave");
                        }
                    });

                    if let Some(url) = url_to_load {
                        self.load_url(url);
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

fn console_level_label(level: js::ConsoleLevel) -> (&'static str, egui::Color32) {
    match level {
        js::ConsoleLevel::Log => ("LOG", egui::Color32::from_rgb(210, 210, 210)),
        js::ConsoleLevel::Info => ("INFO", egui::Color32::from_rgb(140, 190, 255)),
        js::ConsoleLevel::Warn => ("WARN", egui::Color32::from_rgb(255, 210, 120)),
        js::ConsoleLevel::Error => ("ERR", egui::Color32::from_rgb(255, 120, 120)),
        js::ConsoleLevel::Debug => ("DBG", egui::Color32::from_rgb(180, 180, 180)),
    }
}

fn apply_console_history_navigation(
    ui: &egui::Ui,
    response: &egui::Response,
    input: &mut String,
    history: &[String],
    history_index: &mut Option<usize>,
) {
    if !response.has_focus() || history.is_empty() {
        return;
    }

    if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
        let next_index = history_index
            .map(|index| index.saturating_sub(1))
            .unwrap_or(history.len() - 1);
        *history_index = Some(next_index);
        *input = history[next_index].clone();
    }

    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
        if let Some(index) = *history_index {
            if index + 1 < history.len() {
                let next_index = index + 1;
                *history_index = Some(next_index);
                *input = history[next_index].clone();
            } else {
                *history_index = None;
                input.clear();
            }
        }
    }
}

fn console_submit_requested_flags(has_focus: bool, lost_focus: bool) -> bool {
    has_focus || lost_focus
}

fn console_submit_requested(ui: &egui::Ui, response: &egui::Response) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
        && console_submit_requested_flags(response.has_focus(), response.lost_focus())
}

fn render_console_panel(
    ctx: &egui::Context,
    open: &mut bool,
    entries: &mut Vec<js::ConsoleEntry>,
    input: &mut String,
    history: &mut Vec<String>,
    history_index: &mut Option<usize>,
    mut clear_console: impl FnMut(),
    mut evaluate_console: impl FnMut(String),
) {
    let max_open_height = (ctx.available_rect().height() * 0.45).clamp(180.0, 360.0);
    let default_height = if *open { 200.0 } else { 34.0 };
    egui::TopBottomPanel::bottom("browser_console_panel")
        .resizable(*open)
        .default_height(default_height)
        .min_height(if *open { 120.0 } else { 34.0 })
        .max_height(if *open { max_open_height } else { 34.0 })
        .frame(
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(24, 26, 31))
                .inner_margin(egui::Margin::symmetric(8.0, 6.0)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let toggle = if *open { "▾ Console" } else { "▸ Console" };
                if ui.button(toggle).clicked() {
                    *open = !*open;
                }
                ui.label(
                    egui::RichText::new(format!("{} entries", entries.len()))
                        .color(egui::Color32::GRAY)
                        .small(),
                );
                if ui.add_enabled(!entries.is_empty(), egui::Button::new("Clear")).clicked() {
                    clear_console();
                    entries.clear();
                }
            });

            if !*open {
                return;
            }

            ui.add_space(4.0);
            let scroll_height = (ui.available_height() - 34.0).max(48.0);
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(scroll_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if entries.is_empty() {
                        ui.label(
                            egui::RichText::new("No console output")
                                .color(egui::Color32::GRAY),
                        );
                        return;
                    }

                    for entry in entries.iter() {
                        let (label, color) = console_level_label(entry.level);
                        let message_color = match entry.level {
                            js::ConsoleLevel::Error => color,
                            _ => egui::Color32::WHITE,
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!("[{}]", label))
                                    .color(color)
                                    .monospace(),
                            );
                            ui.label(
                                egui::RichText::new(format!("@{}", entry.timestamp))
                                    .color(egui::Color32::GRAY)
                                    .monospace()
                                    .small(),
                            );
                            ui.label(
                                egui::RichText::new(&entry.message)
                                    .color(message_color)
                                    .monospace(),
                            );
                        });
                    }
                });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("JS >")
                        .color(egui::Color32::from_rgb(140, 190, 255))
                        .monospace(),
                );
                let response = ui.add(
                    egui::TextEdit::singleline(input)
                        .desired_width(f32::INFINITY)
                        .hint_text("Evaluate JavaScript"),
                );
                apply_console_history_navigation(ui, &response, input, history, history_index);

                if console_submit_requested(ui, &response) {
                    let code = input.trim().to_string();
                    if !code.is_empty() {
                        history.push(code.clone());
                        *history_index = None;
                        input.clear();
                        evaluate_console(code);
                    }
                }
            });
        });
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
    use url::Url;

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

    #[test]
    fn test_console_submit_requested_flags_accepts_focus_loss_on_enter() {
        assert!(console_submit_requested_flags(true, false));
        assert!(console_submit_requested_flags(false, true));
        assert!(!console_submit_requested_flags(false, false));
    }
}
