//! browser-daemon — engine + GUI + HTTP server in one process.
//!
//! Usage:
//!   browser-daemon              # GUI window + HTTP server on :7070
//!   browser-daemon --no-gui     # headless HTTP server only
//!   browser-daemon --port 7071  # custom port
//!   browser-daemon --viewport-height 1200  # viewport height in CSS px (default 768)

use std::collections::HashMap;

use browser::engine::{EngineCmd, EngineHandle};
use browser::{engine, layout};
use eframe::egui;
use poll_promise::Promise;

// ── CLI argument parsing ───────────────────────────────────────────────────────

#[derive(Debug)]
struct DaemonArgs {
    no_gui: bool,
    port: u16,
    viewport_height: Option<f32>,
}

fn parse_args_from(args: &[&str]) -> DaemonArgs {
    let mut no_gui = false;
    let mut port = 7070u16;
    let mut viewport_height = None;
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
            "--viewport-height" => {
                i += 1;
                viewport_height = args.get(i).and_then(|h| h.parse().ok());
            }
            _ => {}
        }
        i += 1;
    }
    DaemonArgs { no_gui, port, viewport_height }
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
struct ConsoleEvalRequest {
    code: String,
}

#[derive(serde::Deserialize)]
struct TickRequest {
    count: Option<u32>,
    width: Option<f32>,
}

/// `GET /screenshot?mode=viewport|full`. `full` (the default) is the whole
/// page from the document origin; `viewport` is the `width` x
/// `--viewport-height` rectangle at the current document scroll offset, as a
/// Chromium page screenshot.
#[derive(serde::Deserialize, Default)]
struct ScreenshotQuery {
    mode: Option<String>,
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
struct ConsoleEvalResponse {
    result: Option<String>,
    error: Option<String>,
}

#[derive(serde::Serialize)]
struct OkResponse {
    ok: bool,
}

#[derive(serde::Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(serde::Serialize)]
struct TickResponse {
    ticks: u32,
    worked: bool,
    rerendered: bool,
}

#[derive(serde::Serialize)]
struct SubmitResponse {
    url: String,
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

fn engine_error_response(error: engine::EngineRequestError) -> axum::response::Response {
    let status = match error {
        engine::EngineRequestError::Busy => StatusCode::SERVICE_UNAVAILABLE,
        engine::EngineRequestError::Disconnected => StatusCode::INTERNAL_SERVER_ERROR,
        engine::EngineRequestError::Failed(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
        .into_response()
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

/// `POST /back` and `POST /forward`: load the neighbouring session history
/// entry. 404 with an error message when there is none.
async fn traverse(handle: EngineHandle, delta: isize) -> axum::response::Response {
    let result = blocking!(move || {
        let page = handle.send_traverse(delta, 800.0)?;
        Ok::<_, String>(page.map(|page| {
            let base_url = page.base_url.clone();
            engine::page_to_api_response(&page, &base_url)
        }))
    });
    match result {
        Ok(Some(resp)) => (StatusCode::OK, Json(resp)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: if delta < 0 {
                    "Already at the beginning of history.".to_string()
                } else {
                    "Already at the end of history.".to_string()
                },
            }),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn back_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    traverse(handle, -1).await
}

async fn forward_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    traverse(handle, 1).await
}

async fn page_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let resp = blocking!(move || handle.send_get_page_control());
    match resp {
        Ok(Some(page)) => (StatusCode::OK, Json(page)).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "No page loaded").into_response(),
        Err(error) => engine_error_response(error),
    }
}

async fn status_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let page_opt = blocking!(move || handle.send_get_page_control());
    match page_opt {
        Ok(page_opt) => {
            let url = page_opt.map(|p| p.url).unwrap_or_default();
            (
                StatusCode::OK,
                Json(StatusResponse {
                    url,
                    loading: false,
                }),
            )
                .into_response()
        }
        Err(error) => engine_error_response(error),
    }
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
    blocking!(move || {
        let _ = handle.tx.send(EngineCmd::TypeText { text: req.text });
    });
    (StatusCode::OK, Json(OkResponse { ok: true })).into_response()
}

async fn js_handler(
    State(handle): State<EngineHandle>,
    Json(req): Json<JsRequest>,
) -> impl IntoResponse {
    let result = blocking!(move || handle.send_evaluate_js(req.script));
    (StatusCode::OK, Json(JsResponse { result })).into_response()
}

async fn console_eval_handler(
    State(handle): State<EngineHandle>,
    Json(req): Json<ConsoleEvalRequest>,
) -> impl IntoResponse {
    let outcome = blocking!(move || handle.send_console_eval_result(req.code));
    (
        StatusCode::OK,
        Json(ConsoleEvalResponse {
            result: outcome.result,
            error: outcome.error,
        }),
    )
        .into_response()
}

async fn screenshot_handler(
    State(handle): State<EngineHandle>,
    Query(query): Query<ScreenshotQuery>,
) -> impl IntoResponse {
    let viewport = match query.mode.as_deref() {
        None | Some("") | Some("full") | Some("full-page") | Some("full_page") => false,
        Some("viewport") => true,
        Some(other) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("unknown screenshot mode '{}' (expected viewport or full)", other),
            )
                .into_response()
        }
    };
    let png_opt = blocking!(move || if viewport {
        handle.send_screenshot_viewport_control()
    } else {
        handle.send_screenshot_control()
    });
    match png_opt {
        Ok(Some(bytes)) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/png")],
            bytes,
        )
            .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "No page loaded").into_response(),
        Err(error) => engine_error_response(error),
    }
}

async fn tick_handler(
    State(handle): State<EngineHandle>,
    Json(req): Json<TickRequest>,
) -> impl IntoResponse {
    let count = req.count.unwrap_or(1).clamp(1, 120);
    let width = req.width.unwrap_or(800.0);
    let result = blocking!(move || {
        let mut worked = false;
        for i in 0..count {
            match handle.send_tick_control(i as f64, None) {
                Ok(did_work) => worked |= did_work,
                Err(error) => return Err(error),
            }
        }
        let rerendered = if worked {
            match handle.send_re_render_control(None, None, width) {
                Ok(_) => true,
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        Ok(TickResponse {
            ticks: count,
            worked,
            rerendered,
        })
    });
    match result {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(error) => engine_error_response(error),
    }
}

async fn dom_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let text = blocking!(move || handle.send_dom_tree());
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        text,
    )
        .into_response()
}

async fn layout_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let text = blocking!(move || handle.send_layout_tree());
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        text,
    )
        .into_response()
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

/// Submit the current page's form by constructing the navigation URL from
/// form metadata (action/method/fields) stored in PageResult.
async fn submit_handler(State(handle): State<EngineHandle>) -> impl IntoResponse {
    let url = blocking!(move || { handle.send_submit() });
    match url {
        Some(nav_url) => (StatusCode::OK, Json(SubmitResponse { url: nav_url })).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(SubmitResponse { url: String::new() }),
        )
            .into_response(),
    }
}

/// Build the axum Router — extracted for testability.
pub fn build_router(handle: EngineHandle) -> Router {
    Router::new()
        .route("/navigate", post(navigate_handler))
        .route("/back", post(back_handler))
        .route("/forward", post(forward_handler))
        .route("/page", get(page_handler))
        .route("/status", get(status_handler))
        .route("/health", get(health_handler))
        .route("/click", post(click_handler))
        .route("/type", post(type_handler))
        .route("/js", post(js_handler))
        .route("/console/eval", post(console_eval_handler))
        .route("/tick", post(tick_handler))
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

/// Width kept free for the page scroll area's vertical scrollbar.
const PANEL_SCROLLBAR_ALLOWANCE: f32 = 12.0;
/// Narrowest layout width the GUI asks the engine for.
const MIN_RENDER_WIDTH: f32 = 320.0;
/// How long the panel width must stay unchanged before the page reflows.
const RESIZE_SETTLE: std::time::Duration = std::time::Duration::from_millis(200);

struct DaemonBrowserApp {
    handle: EngineHandle,
    url: String,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    current_links: Vec<(layout::Rect, String)>,
    current_form_controls: Vec<(layout::Rect, String)>,
    current_form_names: Vec<String>,
    current_form_buttons: Vec<(layout::Rect, String)>,
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
    submit_promise: Option<Promise<Option<String>>>,
    image_promises: HashMap<String, Promise<Result<(String, Vec<u8>), String>>>,
    form_values: HashMap<String, String>,
    start_time: std::time::Instant,
    console_entries: Vec<browser::js::ConsoleEntry>,
    console_panel_open: bool,
    console_input: String,
    console_history: Vec<String>,
    console_history_index: Option<usize>,
    console_eval_promise: Option<Promise<browser::js::EvalOutcome>>,
    has_page: bool,
    /// Page layout width in CSS px: the central panel's width, so the page
    /// reflows when the window is resized (the HTTP API keeps 800).
    render_width: f32,
    /// A new panel width waiting to settle before re-rendering at it.
    pending_width: Option<(f32, std::time::Instant)>,
    /// The page must be re-rendered at `render_width` once the render or load
    /// in flight finishes.
    reflow_pending: bool,
}

impl DaemonBrowserApp {
    fn new(cc: &eframe::CreationContext<'_>, handle: EngineHandle) -> Self {
        // Load Korean font (same as BrowserApp)
        let mut fonts = egui::FontDefinitions::default();
        let nanum_data = browser::fonts::EMBEDDED_FALLBACK;
        fonts
            .font_data
            .insert("nanum".to_owned(), egui::FontData::from_static(nanum_data));
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
            texture: None,
            error: None,
            current_links: vec![],
            current_form_controls: vec![],
            current_form_names: vec![],
            current_form_buttons: vec![],
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
            submit_promise: None,
            image_promises: HashMap::new(),
            form_values: HashMap::new(),
            start_time: std::time::Instant::now(),
            console_entries: vec![],
            console_panel_open: false,
            console_input: String::new(),
            console_history: vec![],
            console_history_index: None,
            console_eval_promise: None,
            has_page: false,
            render_width: 800.0,
            pending_width: None,
            reflow_pending: false,
        }
    }

    /// Load `url`; the engine adds it to session history.
    fn load_url(&mut self, url: String, width: f32) {
        let resolved = engine::resolve_url(&url);
        self.url = resolved.clone();
        let handle = self.handle.clone();
        self.start_load(move || handle.send_navigate(resolved, width));
    }

    /// Load the session history entry `delta` steps away.
    fn traverse_history(&mut self, delta: isize, width: f32) {
        let handle = self.handle.clone();
        self.start_load(move || {
            handle
                .send_traverse(delta, width)?
                .ok_or_else(|| "no session history entry there".to_string())
        });
    }

    fn start_load(
        &mut self,
        load: impl FnOnce() -> Result<engine::PageResult, String> + Send + 'static,
    ) {
        self.error = None;
        self.image_promises.clear();
        self.hovered_id = None;
        self.is_loading = true;
        self.has_page = false;
        self.content_promise = Some(Promise::spawn_thread("daemon-navigate", load));
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

    /// Show the engine's current URL, which script navigations, redirects
    /// and history traversal change engine-side.
    fn sync_url_from_history(&mut self) {
        if let Some(url) = self.handle.history().current() {
            self.url = url.to_string();
        }
    }

    fn apply_page_data(&mut self, page: engine::PageResult, ctx: &egui::Context) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [page.width as usize, page.height as usize],
            &page.pixmap_bytes,
        );
        self.texture = Some(ctx.load_texture("daemon-page", image, Default::default()));
        self.current_links = page.links;
        self.current_form_controls = page.form_controls;
        self.current_form_buttons = page.form_buttons;
        self.current_form_names = page.form_control_names;
        self.current_event_handlers = page.event_handlers;
        self.current_element_ids = page.element_ids;
        self.current_focusable_elements = page.focusable_elements;
        self.has_page = true;
    }
}

impl eframe::App for DaemonBrowserApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // JS tick — at most one in-flight at a time to prevent unbounded thread spawns.
        let timestamp = self.start_time.elapsed().as_secs_f64() * 1000.0;
        if let Some(tick_p) = &self.tick_promise {
            if tick_p.ready().is_some() {
                self.tick_promise = None;
            }
        }
        // The tick thread wakes the UI again only when the page has work: at
        // once after script ran, when the next timer is due, or to poll an
        // in-flight fetch. With nothing pending the UI sleeps until input.
        if self.tick_promise.is_none()
            && self.content_promise.is_none()
            && self.re_render_promise.is_none()
        {
            let handle = self.handle.clone();
            let ctx = ctx.clone();
            self.tick_promise = Some(Promise::spawn_thread("daemon-tick", move || {
                let outcome = handle.send_tick(timestamp, None);
                if outcome.worked {
                    ctx.request_repaint();
                } else if let Some(wake) = outcome.wake_after {
                    ctx.request_repaint_after(wake);
                }
                outcome.worked
            }));
        }

        self.console_entries = self.handle.send_get_console();
        if let Some(eval_promise) = &self.console_eval_promise {
            if eval_promise.ready().is_some() {
                self.console_eval_promise = None;
                if self.has_page
                    && self.re_render_promise.is_none()
                    && self.content_promise.is_none()
                {
                    self.trigger_re_render(ctx, self.render_width);
                } else {
                    ctx.request_repaint();
                }
            }
        }

        // Tab navigation
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let focusables = &self.current_focusable_elements;
            if !focusables.is_empty() {
                let current_index = self
                    .focused_id
                    .as_ref()
                    .and_then(|id| focusables.iter().position(|(_, fid)| fid == id));
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
                self.trigger_re_render(ctx, self.render_width);
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

                    let history = self.handle.history();
                    if btn_style(ui, "←", history.can_go_back()).clicked() {
                        self.traverse_history(-1, self.render_width);
                    }
                    if btn_style(ui, "→", history.can_go_forward()).clicked() {
                        self.traverse_history(1, self.render_width);
                    }
                    if btn_style(ui, "⟳", true).clicked() {
                        let url = self.url.clone();
                        self.load_url(url, self.render_width);
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
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let url = self.url.clone();
                            self.load_url(url, self.render_width);
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
                        self.load_url(url, self.render_width);
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
            &mut self.console_input,
            &mut self.console_history,
            &mut self.console_history_index,
            || self.handle.send_clear_console(),
            |code| {
                if self.console_eval_promise.is_none() {
                    let handle = self.handle.clone();
                    self.console_eval_promise =
                        Some(Promise::spawn_thread("daemon-console-eval", move || {
                            handle.send_console_eval_result(code)
                        }));
                }
            },
        );

        // Content area
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::WHITE))
            .show(ctx, |ui| {
                // Poll click promise — trigger re-render if onclick fired JS style changes
                if let Some(click_p) = &self.click_promise {
                    if let Some(results) = click_p.ready() {
                        let navigated = results
                            .iter()
                            .any(|r| matches!(r, engine::ClickResult::Navigated { .. }));
                        let had_script = navigated
                            || results
                                .iter()
                                .any(|r| matches!(r, engine::ClickResult::ScriptExecuted));
                        self.click_promise = None;
                        if navigated {
                            self.sync_url_from_history();
                        }
                        if had_script {
                            self.trigger_re_render(ctx, self.render_width);
                        }
                    }
                }

                if let Some(submit_p) = &self.submit_promise {
                    if let Some(maybe_url) = submit_p.ready() {
                        let url = maybe_url.clone();
                        self.submit_promise = None;
                        if let Some(url) = url {
                            self.load_url(url, self.render_width);
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
                if self.reflow_pending && self.has_page && self.re_render_promise.is_none() && self.content_promise.is_none() {
                    self.reflow_pending = false;
                    self.trigger_re_render(ctx, self.render_width);
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
                            self.has_page = false;
                        }
                        Some(Ok(page)) => {
                            let page = page.clone();
                            let image_urls = page.image_urls.clone();

                            self.is_loading = false;
                            self.sync_url_from_history();
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
                                            match browser::engine::http_client().get(&url_clone).send() {
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
                    self.trigger_re_render(ctx, self.render_width);
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

                // Reflow the page to the panel width once a resize settles.
                let panel_width = (ui.available_width() - PANEL_SCROLLBAR_ALLOWANCE)
                    .floor()
                    .max(MIN_RENDER_WIDTH);
                if (panel_width - self.render_width).abs() >= 1.0 {
                    let settled = match self.pending_width {
                        Some((w, since)) if (w - panel_width).abs() < 1.0 => {
                            since.elapsed() >= RESIZE_SETTLE
                        }
                        _ => {
                            self.pending_width = Some((panel_width, std::time::Instant::now()));
                            false
                        }
                    };
                    if settled {
                        // Adopt the width now; if a render or load is in
                        // flight (pages with timers re-render constantly),
                        // reflow as soon as it finishes instead of waiting
                        // for an idle moment that may never come.
                        self.render_width = panel_width;
                        self.pending_width = None;
                        if self.has_page && self.re_render_promise.is_none() && self.content_promise.is_none() {
                            self.trigger_re_render(ctx, self.render_width);
                        } else {
                            self.reflow_pending = true;
                        }
                    } else {
                        ctx.request_repaint_after(RESIZE_SETTLE);
                    }
                } else {
                    self.pending_width = None;
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

                            // Form buttons overlay
                            for (l_rect, label) in &self.current_form_buttons {
                                let screen_rect = egui::Rect::from_min_size(
                                    rect.min + egui::vec2(l_rect.x, l_rect.y),
                                    egui::vec2(l_rect.width.max(40.0), l_rect.height.max(16.0)),
                                );
                                let resp = ui.put(screen_rect, egui::Button::new(label.as_str()));
                                if resp.clicked() && self.submit_promise.is_none() && self.content_promise.is_none() {
                                    let handle = self.handle.clone();
                                    self.submit_promise = Some(Promise::spawn_thread(
                                        "daemon-submit-btn",
                                        move || handle.send_submit(),
                                    ));
                                }
                            }

                            if !self.current_form_controls.is_empty()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                && self.submit_promise.is_none()
                                && self.click_promise.is_none()
                                && self.content_promise.is_none()
                            {
                                let handle = self.handle.clone();
                                let names = self.current_form_names.clone();
                                let values: Vec<String> = (0..self.current_form_controls.len())
                                    .map(|i| self.form_values.get(&i.to_string()).cloned().unwrap_or_default())
                                    .collect();
                                self.submit_promise = Some(Promise::spawn_thread(
                                    "daemon-submit",
                                    move || {
                                        for (name, val) in names.iter().zip(values.iter()) {
                                            if name.is_empty() { continue; }
                                            let escaped_name = name.replace('\\', "\\\\").replace('\'', "\\'");
                                            let escaped_val = val.replace('\\', "\\\\").replace('\'', "\\'");
                                            let script = format!(
                                                "(function(){{ var el=document.querySelector('[name=\"{}\"]'); if(el)el.value=\"{}\"; }})()",
                                                escaped_name, escaped_val
                                            );
                                            handle.send_evaluate_js(script);
                                        }
                                        handle.send_submit()
                                    },
                                ));
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
                                        self.trigger_re_render(ctx, self.render_width);
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
                                    self.trigger_re_render(ctx, self.render_width);
                                }
                            } else if self.hovered_id.is_some() {
                                self.hovered_id = None;
                                self.trigger_re_render(ctx, self.render_width);
                            }
                        });

                    if let Some(url) = url_to_load {
                        self.load_url(url, self.render_width);
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
    entries: &mut Vec<browser::js::ConsoleEntry>,
    input: &mut String,
    history: &mut Vec<String>,
    history_index: &mut Option<usize>,
    mut clear_console: impl FnMut(),
    mut evaluate_console: impl FnMut(String),
) {
    let max_open_height = (ctx.available_rect().height() * 0.45).clamp(180.0, 360.0);
    let default_height = if *open { 200.0 } else { 34.0 };
    egui::TopBottomPanel::bottom("daemon_console_panel")
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
                if ui
                    .add_enabled(!entries.is_empty(), egui::Button::new("Clear"))
                    .clicked()
                {
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
                            egui::RichText::new("No console output").color(egui::Color32::GRAY),
                        );
                        return;
                    }

                    for entry in entries.iter() {
                        let (label, color) = console_level_label(entry.level);
                        let message_color = match entry.level {
                            browser::js::ConsoleLevel::Error => color,
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

// ── Main entry point ──────────────────────────────────────────────────────────

fn main() {
    let args = parse_args();

    // Spawn the engine actor thread and get a cloneable handle to it.
    let handle = EngineHandle::spawn_with_viewport_height(
        args.viewport_height
            .unwrap_or(engine::DEFAULT_VIEWPORT_HEIGHT),
    );

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
        println!(
            "[browser-daemon] Running headless on http://127.0.0.1:{}",
            port
        );
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
    use tower::ServiceExt;

    // ── Arg parsing ──────────────────────────────────────────────────────────

    #[test]
    fn test_parse_args_defaults() {
        let args = parse_args_from(&[]);
        assert!(!args.no_gui);
        assert_eq!(args.port, 7070);
    }

    #[test]
    fn test_parse_args_viewport_height() {
        assert_eq!(parse_args_from(&[]).viewport_height, None);
        let args = parse_args_from(&["--no-gui", "--viewport-height", "1200"]);
        assert_eq!(args.viewport_height, Some(1200.0));
        assert!(args.no_gui);
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
        handle
            .tx
            .send(EngineCmd::GetPage { reply: reply_tx })
            .unwrap();
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
        let outcome = handle.send_tick(0.0, None);
        assert!(!outcome.worked);
        assert_eq!(outcome.wake_after, None);
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
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
    async fn test_http_screenshot_modes() {
        for (uri, expected) in [
            ("/screenshot?mode=viewport", StatusCode::NOT_FOUND),
            ("/screenshot?mode=full", StatusCode::NOT_FOUND),
            ("/screenshot?mode=sideways", StatusCode::BAD_REQUEST),
        ] {
            let app = build_router(make_test_handle());
            let req = axum::http::Request::builder()
                .method("GET")
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), expected, "{uri}");
        }
    }

    #[tokio::test]
    async fn test_http_tick_returns_status() {
        let handle = make_test_handle();
        let app = build_router(handle);
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/tick")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(r#"{"count":2}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ticks"], 2);
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    //     #[tokio::test]
    //     async fn test_http_console_returns_entries() {
    //         let handle = make_test_handle();
    //         handle.send_evaluate_js("console.warn('watch out')".to_string());
    //
    //         let app = build_router(handle);
    //         let req = axum::http::Request::builder()
    //             .method("GET")
    //             .uri("/console")
    //             .body(axum::body::Body::empty())
    //             .unwrap();
    //         let resp = app.oneshot(req).await.unwrap();
    //         assert_eq!(resp.status(), StatusCode::OK);
    //         let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    //         let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    //         let entries = json.as_array().unwrap();
    //         assert_eq!(entries.len(), 1);
    //         assert_eq!(entries[0]["level"], "warn");
    //         assert_eq!(entries[0]["message"], "watch out");
    //     }

    //     #[tokio::test]
    //     async fn test_http_console_eval_returns_result() {
    //         let handle = make_test_handle();
    //         let app = build_router(handle);
    //         let req = axum::http::Request::builder()
    //             .method("POST")
    //             .uri("/console/eval")
    //             .header(axum::http::header::CONTENT_TYPE, "application/json")
    //             .body(axum::body::Body::from(r#"{"code":"1+1"}"#))
    //             .unwrap();
    //         let resp = app.oneshot(req).await.unwrap();
    //         assert_eq!(resp.status(), StatusCode::OK);
    //         let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    //         let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    //         assert_eq!(json["result"], "2");
    //         assert!(json["error"].is_null());
    //     }

    //     #[tokio::test]
    //     async fn test_http_console_eval_returns_error() {
    //         let handle = make_test_handle();
    //         let app = build_router(handle);
    //         let req = axum::http::Request::builder()
    //             .method("POST")
    //             .uri("/console/eval")
    //             .header(axum::http::header::CONTENT_TYPE, "application/json")
    //             .body(axum::body::Body::from(r#"{"code":"missingVariable"}"#))
    //             .unwrap();
    //         let resp = app.oneshot(req).await.unwrap();
    //         assert_eq!(resp.status(), StatusCode::OK);
    //         let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    //         let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    //         assert!(json["result"].is_null());
    //         assert!(json["error"].is_string());
    //     }

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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_object().unwrap().is_empty());
    }

    // ── Session history ──────────────────────────────────────────────────────

    /// Serve `pages` (path → HTML) over HTTP on a local port until the test
    /// process exits; unknown paths get an empty page. Returns the origin.
    fn serve_pages(pages: Vec<(&'static str, &'static str)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                loop {
                    let mut header = String::new();
                    match reader.read_line(&mut header) {
                        Ok(n) if n > 2 => continue,
                        _ => break,
                    }
                }
                let target = request_line.split_whitespace().nth(1).unwrap_or("/");
                let path = target.split('?').next().unwrap_or("/");
                let body = pages
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, html)| *html)
                    .unwrap_or("<html><body></body></html>");
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        origin
    }

    async fn request_json(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let builder = axum::http::Request::builder().method(method).uri(uri);
        let req = match body {
            Some(json) => builder
                .header("content-type", "application/json")
                .body(axum::body::Body::from(json.to_string()))
                .unwrap(),
            None => builder.body(axum::body::Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    async fn current_url(app: &Router) -> String {
        let (_, json) = request_json(app, "GET", "/status", None).await;
        json["url"].as_str().unwrap_or_default().to_string()
    }

    const PAGE_A: &str = r#"<html><body><a href="/b" style="display:block;height:40px">to b</a>
        <button id="go" onclick="location.href='/c'">go</button>
        <form action="/search"><input name="q" value="rust lang"></form></body></html>"#;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_http_back_forward_follow_navigate_and_click() {
        let origin = serve_pages(vec![
            ("/a", PAGE_A),
            ("/b", "<html><body><p>b</p></body></html>"),
        ]);
        let app = build_router(make_test_handle());

        let (status, json) = request_json(&app, "POST", "/back", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["error"], "Already at the beginning of history.");

        let (status, _) = request_json(
            &app,
            "POST",
            "/navigate",
            Some(serde_json::json!({ "url": format!("{origin}/a") })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // A link click answers Navigate; the client then loads it, as
        // browser-cli does.
        let (_, clicks) = request_json(
            &app,
            "POST",
            "/click",
            Some(serde_json::json!({ "x": 10.0, "y": 20.0 })),
        )
        .await;
        let link = clicks
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["type"] == "Navigate")
            .expect("link click navigates")["url"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(link, format!("{origin}/b"));
        request_json(&app, "POST", "/navigate", Some(serde_json::json!({ "url": link }))).await;
        assert_eq!(current_url(&app).await, format!("{origin}/b"));

        let (status, json) = request_json(&app, "POST", "/back", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["url"], format!("{origin}/a"));
        assert_eq!(current_url(&app).await, format!("{origin}/a"));

        let (status, json) = request_json(&app, "POST", "/forward", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["url"], format!("{origin}/b"));

        let (status, json) = request_json(&app, "POST", "/forward", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["error"], "Already at the end of history.");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_http_script_navigations_add_history_entries() {
        let origin = serve_pages(vec![
            ("/a", PAGE_A),
            ("/c", "<html><body><p>c</p></body></html>"),
            (
                "/redirect",
                "<html><body><script>location.replace('/c')</script></body></html>",
            ),
        ]);
        let app = build_router(make_test_handle());
        let navigate = |path: &str| serde_json::json!({ "url": format!("{origin}{path}") });

        // location.href from script run through /js.
        request_json(&app, "POST", "/navigate", Some(navigate("/a"))).await;
        request_json(
            &app,
            "POST",
            "/js",
            Some(serde_json::json!({ "script": "location.href = '/c'" })),
        )
        .await;
        assert_eq!(current_url(&app).await, format!("{origin}/c"));
        let (_, json) = request_json(&app, "POST", "/back", None).await;
        assert_eq!(json["url"], format!("{origin}/a"));

        // form.submit() loads the action with the fields as a query.
        request_json(
            &app,
            "POST",
            "/js",
            Some(serde_json::json!({ "script": "document.querySelector('form').submit()" })),
        )
        .await;
        assert_eq!(current_url(&app).await, format!("{origin}/search?q=rust+lang"));
        let (_, json) = request_json(&app, "POST", "/back", None).await;
        assert_eq!(json["url"], format!("{origin}/a"));

        // A redirect while the page loads replaces its entry: back skips it.
        request_json(&app, "POST", "/navigate", Some(navigate("/redirect"))).await;
        assert_eq!(current_url(&app).await, format!("{origin}/c"));
        let (_, json) = request_json(&app, "POST", "/back", None).await;
        assert_eq!(json["url"], format!("{origin}/a"));
    }
}
