//! browser-daemon — engine + GUI + HTTP server in one process.
//!
//! Usage:
//!   browser-daemon              # GUI window + HTTP server on :7070
//!   browser-daemon --no-gui     # headless HTTP server only
//!   browser-daemon --port 7071  # custom port

use std::collections::HashMap;

use browser::{engine, layout};
use browser::engine::{EngineCmd, EngineHandle};
use eframe::egui;
use poll_promise::Promise;

// ── CLI argument parsing ───────────────────────────────────────────────────────

#[derive(Debug)]
struct DaemonArgs {
    no_gui: bool,
    port: u16,
}

fn parse_args_from(args: &[&str]) -> DaemonArgs {
    let mut no_gui = false;
    let mut port = 7070u16;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--no-gui" => no_gui = true,
            "--port" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    port = p.parse().unwrap_or(7070);
                }
            }
            _ => {}
        }
        i += 1;
    }
    DaemonArgs { no_gui, port }
}

fn parse_args() -> DaemonArgs {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = raw.iter().map(|s| s.as_str()).collect();
    parse_args_from(&refs)
}


// ── axum HTTP server ──────────────────────────────────────────────────────────

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};

#[derive(serde::Deserialize)]
struct NavigateRequest {
    url: String,
}

#[derive(serde::Deserialize)]
struct ClickRequest {
    x: f32,
    y: f32,
}

#[derive(serde::Deserialize)]
struct TypeRequest {
    text: String,
}

#[derive(serde::Deserialize)]
struct JsRequest {
    script: String,
}

#[derive(serde::Deserialize)]
struct StyleQuery {
    selector: Option<String>,
}

#[derive(serde::Serialize)]
struct StatusResponse {
    url: String,
    loading: bool,
}

#[derive(serde::Serialize)]
struct JsResponse {
    result: String,
}

#[derive(serde::Serialize)]
struct OkResponse {
    ok: bool,
}

/// Helper: run a blocking closure in spawn_blocking, converting a JoinError into an HTTP 500.
macro_rules! blocking {
    ($f:expr) => {
        match tokio::task::spawn_blocking($f).await {
            Ok(v) => v,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };
}

async fn navigate_handler(
    State(handle): State<EngineHandle>,
    Json(req): Json<NavigateRequest>,
) -> impl IntoResponse {
    // `page_to_api_response` is called inside `spawn_blocking` so that any panic it raises
    // is caught by Tokio and converted to a `JoinError`. The `blocking!` macro turns a
    // `JoinError` into an HTTP 500 rather than dropping the TCP connection silently.
    let result = blocking!(move || {
        let page = handle.send_navigate(req.url, 800.0)?;
        let base_url = page.base_url.clone();
        Ok::<_, String>(engine::page_to_api_response(&page, &base_url))
    });
    match result {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn page_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let resp = blocking!(move || handle.send_get_page());
    match resp {
        Some(page) => (StatusCode::OK, Json(page)).into_response(),
        None => (StatusCode::NOT_FOUND, "No page loaded").into_response(),
    }
}

async fn status_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let page_opt = blocking!(move || handle.send_get_page());
    let url = page_opt.map(|p| p.url).unwrap_or_default();
    (StatusCode::OK, Json(StatusResponse { url, loading: false })).into_response()
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok").into_response()
}

async fn click_handler(
    State(handle): State<EngineHandle>,
    Json(req): Json<ClickRequest>,
) -> impl IntoResponse {
    let results = blocking!(move || handle.send_click(req.x, req.y));
    (StatusCode::OK, Json(results)).into_response()
}

async fn type_handler(
    State(handle): State<EngineHandle>,
    Json(req): Json<TypeRequest>,
) -> impl IntoResponse {
    blocking!(move || { let _ = handle.tx.send(EngineCmd::TypeText { text: req.text }); });
    (StatusCode::OK, Json(OkResponse { ok: true })).into_response()
}

async fn js_handler(
    State(handle): State<EngineHandle>,
    Json(req): Json<JsRequest>,
) -> impl IntoResponse {
    let result = blocking!(move || handle.send_evaluate_js(req.script));
    (StatusCode::OK, Json(JsResponse { result })).into_response()
}

async fn screenshot_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let png_opt = blocking!(move || handle.send_screenshot());
    match png_opt {
        Some(bytes) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/png")],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "No page loaded").into_response(),
    }
}

async fn dom_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let text = blocking!(move || handle.send_dom_tree());
    (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "text/plain")], text).into_response()
}

async fn layout_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let text = blocking!(move || handle.send_layout_tree());
    (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "text/plain")], text).into_response()
}

async fn style_handler(
    State(handle): State<EngineHandle>,
    Query(params): Query<StyleQuery>,
) -> impl IntoResponse {
    let selector = params.selector.unwrap_or_default();
    let style = blocking!(move || handle.send_computed_style(selector));
    (StatusCode::OK, Json(style)).into_response()
}

async fn elements_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let elems = blocking!(move || handle.send_get_elements());
    (StatusCode::OK, Json(elems)).into_response()
}

async fn console_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let entries = blocking!(move || handle.send_get_console());
    (StatusCode::OK, Json(entries)).into_response()
}

/// Submit the first form on the current page by evaluating `document.forms[0].submit()`.
///
/// Note: the JS runtime is a mock, so this is a best-effort stub.
async fn submit_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    blocking!(move || {
        handle.send_evaluate_js("if(document.forms && document.forms[0]) document.forms[0].submit()".to_string())
    });
    (StatusCode::OK, Json(OkResponse { ok: true })).into_response()
}

/// Build the axum Router — extracted for testability.
pub fn build_router(handle: EngineHandle) -> Router {
    Router::new()
        .route("/navigate", post(navigate_handler))
        .route("/page", get(page_handler))
        .route("/status", get(status_handler))
        .route("/health", get(health_handler))
        .route("/click", post(click_handler))
        .route("/type", post(type_handler))
        .route("/js", post(js_handler))
        .route("/screenshot", get(screenshot_handler))
        .route("/dom", get(dom_handler))
        .route("/layout", get(layout_handler))
        .route("/style", get(style_handler))
        .route("/elements", get(elements_handler))
        .route("/console", get(console_handler))
        .route("/submit", post(submit_handler))
        .with_state(handle)
}

async fn run_http_server(handle: EngineHandle, port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("[browser-daemon] HTTP listening on http://{}", addr);
    axum::serve(listener, build_router(handle)).await.unwrap();
}

// ── DaemonBrowserApp — GUI front-end ──────────────────────────────────────────

struct DaemonBrowserApp {
    handle: EngineHandle,
    url: String,
    history: Vec<String>,
    history_index: usize,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    current_links: Vec<(layout::Rect, String)>,
    current_form_controls: Vec<(layout::Rect, String)>,
    current_event_handlers: Vec<(layout::Rect, String)>,
    current_element_ids: Vec<(layout::Rect, String)>,
    current_focusable_elements: Vec<(layout::Rect, String)>,
    hovered_id: Option<String>,
    focused_id: Option<String>,
    is_loading: bool,
    content_promise: Option<Promise<Result<engine::PageResult, String>>>,
    re_render_promise: Option<Promise<Result<engine::PageResult, String>>>,
    /// Bounded JS tick promise — prevents unbounded thread spawns per frame.
    tick_promise: Option<Promise<bool>>,
    /// Pending click result — triggers re-render when a ScriptExecuted click resolves.
    click_promise: Option<Promise<Vec<engine::ClickResult>>>,
    image_promises: HashMap<String, Promise<Result<(String, Vec<u8>), String>>>,
    form_values: HashMap<String, String>,
    start_time: std::time::Instant,
    console_entries: Vec<browser::js::ConsoleEntry>,
    console_panel_open: bool,
}

impl DaemonBrowserApp {
    fn new(cc: &eframe::CreationContext<'_>, handle: EngineHandle) -> Self {
        // Load Korean font (same as BrowserApp)
        let mut fonts = egui::FontDefinitions::default();
        let nanum_data = include_bytes!("../../assets/fonts/NanumGothic.ttf");
        fonts.font_data.insert(
            "nanum".to_owned(),
            egui::FontData::from_static(nanum_data),
        );
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "nanum".to_owned());
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .push("nanum".to_owned());
        cc.egui_ctx.set_fonts(fonts);

        Self {
            handle,
            url: "https://yunseong.dev".to_string(),
            history: vec![],
            history_index: 0,
            texture: None,
            error: None,
            current_links: vec![],
            current_form_controls: vec![],
            current_event_handlers: vec![],
            current_element_ids: vec![],
            current_focusable_elements: vec![],
            hovered_id: None,
            focused_id: None,
            is_loading: false,
            content_promise: None,
            re_render_promise: None,
            tick_promise: None,
            click_promise: None,
            image_promises: HashMap::new(),
            form_values: HashMap::new(),
            start_time: std::time::Instant::now(),
            console_entries: vec![],
            console_panel_open: true,
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

        let handle = self.handle.clone();
        self.content_promise = Some(Promise::spawn_thread("daemon-navigate", move || {
            handle.send_navigate(url, width)
        }));
    }

    fn trigger_re_render(&mut self, ctx: &egui::Context, width: f32) {
        let handle = self.handle.clone();
        let hovered_id = self.hovered_id.clone();
        let focused_id = self.focused_id.clone();

        self.re_render_promise = Some(Promise::spawn_thread("daemon-re-render", move || {
            handle.send_re_render(hovered_id, focused_id, width)
        }));
        ctx.request_repaint();
    }

    fn apply_page_data(&mut self, page: engine::PageResult, ctx: &egui::Context) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [page.width as usize, page.height as usize],
            &page.pixmap_bytes,
        );
        self.texture = Some(ctx.load_texture("daemon-page", image, Default::default()));
        self.current_links = page.links;
        self.current_form_controls = page.form_controls;
        self.current_event_handlers = page.event_handlers;
        self.current_element_ids = page.element_ids;
        self.current_focusable_elements = page.focusable_elements;
    }
}

impl eframe::App for DaemonBrowserApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // JS tick — at most one in-flight at a time to prevent unbounded thread spawns.
        let timestamp = self.start_time.elapsed().as_secs_f64() * 1000.0;
        if self.tick_promise.is_none()
            && self.content_promise.is_none()
            && self.re_render_promise.is_none()
        {
            let handle = self.handle.clone();
            self.tick_promise = Some(Promise::spawn_thread("daemon-tick", move || {
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

        self.console_entries = self.handle.send_get_console();

        // Tab navigation
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let focusables = &self.current_focusable_elements;
            if !focusables.is_empty() {
                let current_index = self.focused_id.as_ref().and_then(|id| {
                    focusables.iter().position(|(_, fid)| fid == id)
                });
                let next_index = if ctx.input(|i| i.modifiers.shift) {
                    match current_index {
                        Some(i) if i > 0 => i - 1,
                        _ => focusables.len() - 1,
                    }
                } else {
                    match current_index {
                        Some(i) if i + 1 < focusables.len() => i + 1,
                        _ => 0,
                    }
                };
                self.focused_id = Some(focusables[next_index].1.clone());
                self.trigger_re_render(ctx, 800.0);
            }
        }

        // Browser chrome
        let toolbar_fill = egui::Color32::from_rgb(50, 50, 55);
        let url_bar_fill = egui::Color32::from_rgb(72, 72, 78);

        egui::TopBottomPanel::top("daemon_chrome")
            .frame(
                egui::Frame::none()
                    .fill(toolbar_fill)
                    .inner_margin(egui::Margin::symmetric(8.0, 6.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    let btn_style =
                        |ui: &mut egui::Ui, label: &str, enabled: bool| -> egui::Response {
                            ui.add_enabled(
                                enabled,
                                egui::Button::new(
                                    egui::RichText::new(label)
                                        .color(if enabled {
                                            egui::Color32::WHITE
                                        } else {
                                            egui::Color32::DARK_GRAY
                                        })
                                        .size(14.0),
                                )
                                .fill(egui::Color32::from_rgb(70, 70, 76))
                                .rounding(egui::Rounding::same(4.0))
                                .min_size(egui::vec2(28.0, 28.0)),
                            )
                        };

                    if btn_style(ui, "←", self.history_index > 0).clicked() {
                        self.navigate_back(800.0);
                    }
                    if btn_style(ui, "→", self.history_index + 1 < self.history.len()).clicked() {
                        self.navigate_forward(800.0);
                    }
                    if btn_style(ui, "⟳", true).clicked() {
                        let url = self.url.clone();
                        self.load_url_direct(url, 800.0);
                    }

                    ui.spacing_mut().item_spacing.x = 8.0;

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
                        if resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            let url = self.url.clone();
                            self.load_url(url, 800.0);
                        }
                    });

                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("이동")
                                    .color(egui::Color32::WHITE)
                                    .size(13.0),
                            )
                            .fill(egui::Color32::from_rgb(0, 120, 212))
                            .rounding(egui::Rounding::same(14.0))
                            .min_size(egui::vec2(50.0, 28.0)),
                        )
                        .clicked()
                    {
                        let url = self.url.clone();
                        self.load_url(url, 800.0);
                    }

                    // Daemon badge
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("daemon")
                                .color(egui::Color32::from_rgb(150, 200, 255))
                                .size(11.0),
                        );
                    });
                });

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
            || self.handle.send_clear_console(),
        );

        // Content area
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::WHITE))
            .show(ctx, |ui| {
                // Poll click promise — trigger re-render if onclick fired JS style changes
                if let Some(click_p) = &self.click_promise {
                    if let Some(results) = click_p.ready() {
                        let had_script = results.iter().any(|r| matches!(r, engine::ClickResult::ScriptExecuted));
                        self.click_promise = None;
                        if had_script {
                            self.trigger_re_render(ctx, 800.0);
                        }
                    }
                }

                // Poll re-render promise
                if let Some(promise) = &self.re_render_promise {
                    match promise.ready() {
                        None => ctx.request_repaint(),
                        Some(Err(e)) => {
                            self.error = Some(format!("Re-render error: {}", e));
                            self.re_render_promise = None;
                        }
                        Some(Ok(page)) => {
                            self.apply_page_data(page.clone(), ctx);
                            self.re_render_promise = None;
                        }
                    }
                }

                // Poll content (navigate) promise
                if let Some(promise) = &self.content_promise {
                    match promise.ready() {
                        None => {
                            ui.centered_and_justified(|ui| { ui.spinner(); });
                            ctx.request_repaint();
                        }
                        Some(Err(e)) => {
                            self.error = Some(e.clone());
                            self.content_promise = None;
                            self.is_loading = false;
                        }
                        Some(Ok(page)) => {
                            let page = page.clone();
                            let image_urls = page.image_urls.clone();

                            self.is_loading = false;
                            self.form_values.clear();
                            for (i, (_, val)) in page.form_controls.iter().enumerate() {
                                self.form_values.insert(i.to_string(), val.clone());
                            }
                            self.apply_page_data(page, ctx);
                            self.content_promise = None;

                            // Start async image fetches
                            for url in &image_urls {
                                if !self.image_promises.contains_key(url) {
                                    let url_clone = url.clone();
                                    self.image_promises.insert(
                                        url.clone(),
                                        Promise::spawn_thread("daemon-img", move || {
                                            match reqwest::blocking::get(&url_clone) {
                                                Ok(resp) => match resp.bytes() {
                                                    Ok(bytes) => Ok((url_clone, bytes.to_vec())),
                                                    Err(e) => Err(e.to_string()),
                                                },
                                                Err(e) => Err(e.to_string()),
                                            }
                                        }),
                                    );
                                }
                            }
                        }
                    }
                }

                // Resolved image promises → send bytes to engine actor, trigger re-render
                let mut newly_loaded = false;
                let handle = self.handle.clone();
                self.image_promises.retain(|_url, promise| match promise.ready() {
                    Some(Ok((url, bytes))) => {
                        let _ = handle.tx.send(EngineCmd::LoadImage {
                            url: url.clone(),
                            bytes: bytes.clone(),
                        });
                        newly_loaded = true;
                        false
                    }
                    Some(Err(_)) => false,
                    None => true,
                });
                if newly_loaded {
                    self.trigger_re_render(ctx, 800.0);
                }

                // Error
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
                let texture_info =
                    self.texture.as_ref().map(|t| (t.id(), t.size_vec2()));
                if let Some((texture_id, texture_size)) = texture_info {
                    let mut url_to_load: Option<String> = None;

                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let (rect, response) = ui.allocate_at_least(
                                texture_size,
                                egui::Sense::click(),
                            );
                            ui.painter().image(
                                texture_id,
                                rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );

                            // Form controls overlay
                            for (i, (l_rect, _)) in
                                self.current_form_controls.iter().enumerate()
                            {
                                let val = self.form_values.entry(i.to_string()).or_default();
                                let screen_rect = egui::Rect::from_min_size(
                                    rect.min + egui::vec2(l_rect.x, l_rect.y),
                                    egui::vec2(l_rect.width, l_rect.height),
                                );
                                ui.put(screen_rect, egui::TextEdit::singleline(val).id_source(i));
                            }

                            // Click handling
                            if response.clicked() {
                                if let Some(ptr) = response.interact_pointer_pos() {
                                    let rel = ptr - rect.min;

                                    // Dispatch full click to engine actor — this handles
                                    // JS click events, focus changes, links, and onclick handlers
                                    // in a single coordinated call using actual pixel coordinates.
                                    // Use click_promise so we can check results for re-render.
                                    if self.click_promise.is_none() {
                                        let handle = self.handle.clone();
                                        let rel_x = rel.x;
                                        let rel_y = rel.y;
                                        self.click_promise = Some(Promise::spawn_thread(
                                            "daemon-click",
                                            move || handle.send_click(rel_x, rel_y),
                                        ));
                                    }

                                    // Update GUI-side focused_id from click
                                    let mut new_focus = None;
                                    for (l_rect, id) in &self.current_focusable_elements {
                                        if daemon_hit(rel, l_rect) {
                                            new_focus = Some(id.clone());
                                        }
                                    }
                                    if new_focus != self.focused_id {
                                        self.focused_id = new_focus;
                                        self.trigger_re_render(ctx, 800.0);
                                    }

                                    // GUI-side link navigation
                                    for (l_rect, link) in &self.current_links {
                                        if daemon_hit(rel, l_rect) {
                                            url_to_load = Some(link.clone());
                                            break;
                                        }
                                    }
                                }
                            }

                            // Hover
                            if let Some(ptr) = response.hover_pos() {
                                let rel = ptr - rect.min;
                                let hovering = self.current_links.iter().any(|(r, _)| daemon_hit(rel, r))
                                    || self.current_event_handlers.iter().any(|(r, _)| daemon_hit(rel, r));
                                if hovering {
                                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                }

                                let mut new_hovered_id = None;
                                for (l_rect, id) in self.current_element_ids.iter().rev() {
                                    if daemon_hit(rel, l_rect) {
                                        new_hovered_id = Some(id.clone());
                                        break;
                                    }
                                }
                                if new_hovered_id != self.hovered_id {
                                    self.hovered_id = new_hovered_id;
                                    self.trigger_re_render(ctx, 800.0);
                                }
                            } else if self.hovered_id.is_some() {
                                self.hovered_id = None;
                                self.trigger_re_render(ctx, 800.0);
                            }
                        });

                    if let Some(url) = url_to_load {
                        self.load_url(url, 800.0);
                    }
                }
            });
    }
}

#[inline]
fn daemon_hit(rel: egui::Vec2, r: &layout::Rect) -> bool {
    rel.x >= r.x && rel.x <= r.x + r.width && rel.y >= r.y && rel.y <= r.y + r.height
}

fn console_level_label(level: browser::js::ConsoleLevel) -> (&'static str, egui::Color32) {
    match level {
        browser::js::ConsoleLevel::Log => ("LOG", egui::Color32::from_rgb(210, 210, 210)),
        browser::js::ConsoleLevel::Info => ("INFO", egui::Color32::from_rgb(140, 190, 255)),
        browser::js::ConsoleLevel::Warn => ("WARN", egui::Color32::from_rgb(255, 210, 120)),
        browser::js::ConsoleLevel::Error => ("ERR", egui::Color32::from_rgb(255, 120, 120)),
        browser::js::ConsoleLevel::Debug => ("DBG", egui::Color32::from_rgb(180, 180, 180)),
    }
}

fn render_console_panel(
    ctx: &egui::Context,
    open: &mut bool,
    entries: &mut Vec<browser::js::ConsoleEntry>,
    mut clear_console: impl FnMut(),
) {
    let default_height = if *open { 180.0 } else { 32.0 };
    egui::TopBottomPanel::bottom("daemon_console_panel")
        .resizable(*open)
        .default_height(default_height)
        .min_height(default_height)
        .max_height(if *open { 260.0 } else { 32.0 })
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
                if ui.button("Clear").clicked() {
                    clear_console();
                    entries.clear();
                }
            });

            if !*open {
                return;
            }

            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
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
                                    .color(egui::Color32::WHITE)
                                    .monospace(),
                            );
                        });
                    }
                });
        });
}

// ── Main entry point ──────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // Spawn the engine actor thread and get a cloneable handle to it.
    let handle = EngineHandle::spawn();

    // HTTP server thread — runs its own tokio runtime
    let handle_for_http = handle.clone();
    let port = args.port;
    std::thread::Builder::new()
        .name("http-server".into())
        .spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            rt.block_on(run_http_server(handle_for_http, port));
        })
        .expect("failed to start HTTP server thread");

    if args.no_gui {
        println!("[browser-daemon] Running headless on http://127.0.0.1:{}", port);
        println!("[browser-daemon] Press Ctrl+C to stop.");
        // Block main thread forever; signal handlers (Ctrl+C) terminate the process
        loop {
            std::thread::park();
        }
    } else {
        // eframe requires the GUI on the main thread (OS requirement on most platforms)
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1024.0, 768.0])
                .with_title("browser-daemon"),
            ..Default::default()
        };
        eframe::run_native(
            "browser-daemon",
            options,
            Box::new(|cc| Ok(Box::new(DaemonBrowserApp::new(cc, handle)))),
        )
        .expect("eframe error");
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use browser::engine::run_engine_actor;
    use tower::ServiceExt;

    // ── Arg parsing ──────────────────────────────────────────────────────────

    #[test]
    fn test_parse_args_defaults() {
        let args = parse_args_from(&[]);
        assert!(!args.no_gui);
        assert_eq!(args.port, 7070);
    }

    #[test]
    fn test_parse_args_no_gui() {
        let args = parse_args_from(&["--no-gui"]);
        assert!(args.no_gui);
    }

    #[test]
    fn test_parse_args_custom_port() {
        let args = parse_args_from(&["--port", "8080"]);
        assert_eq!(args.port, 8080);
    }

    #[test]
    fn test_parse_args_combined() {
        let args = parse_args_from(&["--no-gui", "--port", "9000"]);
        assert!(args.no_gui);
        assert_eq!(args.port, 9000);
    }

    // ── Engine actor ─────────────────────────────────────────────────────────

    fn make_test_handle() -> EngineHandle {
        EngineHandle::spawn()
    }

    #[test]
    fn test_engine_actor_get_page_empty() {
        let handle = make_test_handle();
        let (reply_tx, reply_rx) = mpsc::channel();
        handle.tx.send(EngineCmd::GetPage { reply: reply_tx }).unwrap();
        let result = reply_rx.recv().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_engine_actor_get_elements_empty() {
        let handle = make_test_handle();
        let elems = handle.send_get_elements();
        assert!(elems.is_empty());
    }

    #[test]
    fn test_engine_actor_dom_tree_empty() {
        let handle = make_test_handle();
        let dom = handle.send_dom_tree();
        assert!(dom.is_empty());
    }

    #[test]
    fn test_engine_actor_layout_tree_empty() {
        let handle = make_test_handle();
        let layout = handle.send_layout_tree();
        assert!(layout.is_empty());
    }

    #[test]
    fn test_engine_actor_computed_style_empty() {
        let handle = make_test_handle();
        let style = handle.send_computed_style("body".to_string());
        assert!(style.is_empty());
    }

    #[test]
    fn test_engine_actor_screenshot_empty() {
        let handle = make_test_handle();
        let result = handle.send_screenshot();
        assert!(result.is_none());
    }

    #[test]
    fn test_engine_actor_click_empty() {
        let handle = make_test_handle();
        let results = handle.send_click(0.0, 0.0);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], engine::ClickResult::Nothing));
    }

    #[test]
    fn test_engine_actor_tick_empty() {
        let handle = make_test_handle();
        let needs = handle.send_tick(0.0, None);
        assert!(!needs);
    }

    #[test]
    fn test_engine_actor_load_image() {
        let handle = make_test_handle();
        // Just verify it doesn't panic
        handle
            .tx
            .send(EngineCmd::LoadImage {
                url: "https://example.com/img.png".into(),
                bytes: vec![0u8; 100],
            })
            .unwrap();
        // Give actor a moment to process
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // ── HTTP endpoints ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_http_status_no_page() {
        let handle = make_test_handle();
        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/status")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["loading"], false);
    }

    #[tokio::test]
    async fn test_http_health_returns_ok() {
        let handle = make_test_handle();
        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn test_http_page_no_page_returns_404() {
        let handle = make_test_handle();
        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/page")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_http_screenshot_no_page_returns_404() {
        let handle = make_test_handle();
        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/screenshot")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_http_elements_empty() {
        let handle = make_test_handle();
        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/elements")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_http_console_returns_entries() {
        let handle = make_test_handle();
        handle.send_evaluate_js("console.warn('watch out')".to_string());

        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/console")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json.as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["level"], "warn");
        assert_eq!(entries[0]["message"], "watch out");
    }

    #[tokio::test]
    async fn test_http_dom_empty() {
        let handle = make_test_handle();
        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/dom")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_http_style_empty() {
        let handle = make_test_handle();
        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/style?selector=body")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_object().unwrap().is_empty());
    }
}
