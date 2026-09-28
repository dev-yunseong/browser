//! `<iframe>` documents.
//!
//! Every frame document runs in its own `BrowserEngine` on its own thread
//! (V8 isolates must be created and dropped in LIFO order per thread, so a
//! child engine never shares a thread with its parent). The parent engine
//! owns a [`FrameHost`] that
//! - finds the `<iframe>` boxes of each parent render ([`collect_frame_boxes`]),
//! - sends each child a load or resize request for the content-box size,
//! - receives the child's viewport capture as PNG bytes, which the parent
//!   stores in its image cache under [`iframe_frame_key`] (the paint side
//!   draws an `<iframe>` box from that cache entry), and
//! - relays `postMessage` between the parent and child documents.
//!
//! Work is bounded: [`MAX_FRAMES`] child documents per top-level page across
//! all nesting levels, [`MAX_DEPTH`] nesting levels, and [`FRAME_BUDGET`] of
//! waiting per navigation.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use url::Url;

use crate::layout;

/// Largest frame viewport side in px; bigger boxes are rendered clipped to it.
pub const MAX_FRAME_SIDE: u32 = 4096;
/// Child documents per top-level page, counted across all nesting levels.
pub const MAX_FRAMES: usize = 8;
/// Nesting levels that load documents: the top-level page is depth 0, its
/// frames depth 1, their frames depth 2. Frames deeper than this stay empty.
pub const MAX_DEPTH: u32 = 2;
/// How long a navigation waits for its frames (load, message round trips and
/// resizes) before returning; frames that finish later are picked up by the
/// next tick or capture.
pub const FRAME_BUDGET: Duration = Duration::from_secs(8);
/// After loading, a frame runs its timers for up to this long before the
/// first capture, so documents that render from `setTimeout` show content.
const FRAME_SETTLE: Duration = Duration::from_millis(1500);
const FRAME_TICK_INTERVAL: Duration = Duration::from_millis(100);
/// How long after its last command a frame keeps running its timers.
const FRAME_IDLE_TICKS: Duration = Duration::from_secs(30);
/// Minimum time between captures sent for timer-driven changes.
const FRAME_IDLE_CAPTURE_INTERVAL: Duration = Duration::from_millis(500);

/// Image-cache key of the rendered document of an `<iframe>` with absolute
/// `src` and a `width` x `height` px content box (the paint side's key).
pub use crate::layer_tree::iframe_frame_key;

// ── Frame threads ─────────────────────────────────────────────────────────────

thread_local! {
    /// Set on frame threads: the document is rendered onto a transparent
    /// canvas so the parent shows through where it paints no background.
    static FRAME_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Whether the current thread renders a frame document.
pub fn is_frame_thread() -> bool {
    FRAME_THREAD.with(|flag| flag.get())
}

// ── Frame boxes ───────────────────────────────────────────────────────────────

/// An `<iframe>` box of a rendered page that loads a network document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameBox {
    /// Absolute `http(s)` URL of the frame document.
    pub src: String,
    /// Position among the page's `<iframe>` elements with the same `src`, in
    /// document order (display: none elements included). Together with `src`
    /// it names the frame across re-renders and in `postMessage` routing.
    pub ordinal: usize,
    /// Content-box size in CSS px, rounded.
    pub width: u32,
    pub height: u32,
    /// The `name` attribute, which becomes the frame's `window.name`.
    pub name: String,
    /// Markup of an `<iframe srcdoc>` frame (`src` is then `about:srcdoc`).
    pub html: Option<String>,
}

impl FrameBox {
    pub fn key(&self) -> String {
        match &self.html {
            Some(html) => iframe_frame_key(&crate::layer_tree::srcdoc_paint_src(html), self.width, self.height),
            None => iframe_frame_key(&self.src, self.width, self.height),
        }
    }
}

/// Absolute `src` of an `<iframe>` element that loads a network document:
/// `http(s)` after resolution against `base_url`, no `srcdoc`.
fn frame_src(attrs: &[(String, String)], base_url: &Url) -> Option<String> {
    if attrs.iter().any(|(name, _)| name == "srcdoc") {
        return Some(crate::layer_tree::SRCDOC_FRAME_SRC.to_string());
    }
    let src = attrs.iter().find(|(name, _)| name == "src")?.1.trim();
    if src.is_empty() {
        return None;
    }
    let url = base_url.join(src).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn iframe_attrs(node: &markup5ever_rcdom::Handle) -> Option<Vec<(String, String)>> {
    let markup5ever_rcdom::NodeData::Element { name, attrs, .. } = &node.data else {
        return None;
    };
    if name.local.as_ref() != "iframe" {
        return None;
    }
    Some(
        attrs
            .borrow()
            .iter()
            .map(|a| (a.name.local.to_string(), a.value.to_string()))
            .collect(),
    )
}

/// Frame ordinal of every network `<iframe>` element under `root`, keyed by
/// node address.
fn frame_ordinals(root: &markup5ever_rcdom::Handle, base_url: &Url) -> HashMap<usize, (String, usize)> {
    let mut per_src: HashMap<String, usize> = HashMap::new();
    let mut out = HashMap::new();
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        if let Some(src) = iframe_attrs(&node).and_then(|attrs| frame_src(&attrs, base_url)) {
            let next = per_src.entry(src.clone()).or_insert(0);
            out.insert(std::rc::Rc::as_ptr(&node) as usize, (src, *next));
            *next += 1;
        }
        // Push children in reverse so they pop in document order.
        for child in node.children.borrow().iter().rev() {
            stack.push(child.clone());
        }
    }
    out
}

fn document_root(node: &markup5ever_rcdom::Handle) -> markup5ever_rcdom::Handle {
    let mut current = node.clone();
    loop {
        let parent = current.parent.take();
        current.parent.set(parent.clone());
        match parent.and_then(|weak| weak.upgrade()) {
            Some(up) => current = up,
            None => return current,
        }
    }
}

/// The `<iframe>` boxes of a laid-out page that load a document (network
/// `src` or `srcdoc`), in document order.
pub fn collect_frame_boxes(root: &layout::LayoutBox, base_url: &Url) -> Vec<FrameBox> {
    let ordinals = frame_ordinals(&document_root(&root.style_node.node), base_url);
    if ordinals.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(layout_box) = stack.pop() {
        let node = &layout_box.style_node.node;
        if let Some((src, ordinal)) = ordinals.get(&(std::rc::Rc::as_ptr(node) as usize)) {
            // Rounded like the paint side rounds the key it looks up.
            let content = crate::background::box_rect(layout_box, crate::background::BoxArea::Content);
            let (width, height) = (content.width.round() as u32, content.height.round() as u32);
            let attrs = iframe_attrs(node).unwrap_or_default();
            let attr = |wanted: &str| attrs.iter().find(|(n, _)| n == wanted).map(|(_, v)| v.clone());
            let name = attr("name").unwrap_or_default();
            let html = attr("srcdoc");
            // Zero-size frames still load: their scripts often ask the
            // embedder for a size (ad SafeFrames). A 1px viewport stands in.
            let (width, height) = (width.clamp(1, MAX_FRAME_SIDE), height.clamp(1, MAX_FRAME_SIDE));
            if !out.iter().any(|f: &FrameBox| f.src == *src && f.ordinal == *ordinal) {
                out.push(FrameBox { src: src.clone(), ordinal: *ordinal, width, height, name, html });
            }
        }
        for child in layout_box.children.iter().rev() {
            stack.push(child);
        }
    }
    out
}

// ── Messages ──────────────────────────────────────────────────────────────────

/// What a frame document knows about the document embedding it.
#[derive(Clone)]
pub struct FrameParent {
    pub origin: String,
    pub name: String,
    /// The embedder's console: frame console entries are forwarded to it,
    /// as browser devtools show frame logs with the page's.
    pub console: Option<crate::js::ConsoleBuffer>,
}

impl std::fmt::Debug for FrameParent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameParent").field("origin", &self.origin).field("name", &self.name).finish()
    }
}

/// A `postMessage` queued by page script, as drained from the JS runtime.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct OutgoingMessage {
    /// `"parent"` for `window.parent/top.postMessage`, `"child"` for
    /// `iframe.contentWindow.postMessage`.
    pub to: String,
    /// Target frame of a `"child"` message.
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default)]
    pub ordinal: usize,
    /// The message, JSON-encoded.
    pub data: String,
    #[serde(rename = "targetOrigin", default)]
    pub target_origin: String,
}

/// Whether a message posted with `target_origin` may be delivered to a
/// document of `origin`, as `postMessage` decides.
pub fn target_origin_allows(target_origin: &str, origin: &str) -> bool {
    match target_origin {
        "*" => true,
        "" => false,
        target => Url::parse(target)
            .map(|url| url.origin().ascii_serialization() == origin)
            .unwrap_or(false),
    }
}

/// A message from a child document to its parent.
#[derive(Clone, Debug, PartialEq)]
pub struct IncomingMessage {
    pub src: String,
    pub ordinal: usize,
    pub data: String,
    pub origin: String,
}

// ── Frame host ────────────────────────────────────────────────────────────────

enum FrameCmd {
    /// `html` is set for `<iframe srcdoc>` documents.
    Load { url: String, html: Option<String>, width: u32, height: u32, key: String },
    Resize { width: u32, height: u32, key: String },
    Message { data: String, origin: String },
}

enum FrameEvent {
    /// Viewport capture of the child document (`None` if it failed).
    Rendered { index: usize, key: String, png: Option<Vec<u8>> },
    /// `window.parent.postMessage` from the child document.
    Message { index: usize, data: String, origin: String },
}

struct ChildFrame {
    src: String,
    ordinal: usize,
    /// Markup of a srcdoc frame; a change reloads the child document.
    html: Option<String>,
    tx: mpsc::Sender<FrameCmd>,
    requested: (u32, u32),
    /// Key of the render the child is working on, if any.
    awaiting: Option<String>,
    /// Key of the capture currently in the image cache.
    cached_key: Option<String>,
}

/// Runs a document's frames. Dropping it closes every child's command
/// channel; child threads then drop their engine on their own thread and exit.
pub struct FrameHost {
    /// Depth of the document owning this host (0 = top-level page).
    depth: u32,
    /// Remaining child documents for the whole frame tree of the top-level page.
    budget: Arc<AtomicUsize>,
    children: Vec<ChildFrame>,
    events_tx: mpsc::Sender<FrameEvent>,
    events_rx: mpsc::Receiver<FrameEvent>,
    ready_images: Vec<(usize, String, Vec<u8>)>,
    ready_messages: Vec<IncomingMessage>,
}

impl FrameHost {
    pub fn new_top_level() -> Self {
        Self::new(0, Arc::new(AtomicUsize::new(MAX_FRAMES)))
    }

    fn new(depth: u32, budget: Arc<AtomicUsize>) -> Self {
        let (events_tx, events_rx) = mpsc::channel();
        Self {
            depth,
            budget,
            children: Vec::new(),
            events_tx,
            events_rx,
            ready_images: Vec::new(),
            ready_messages: Vec::new(),
        }
    }

    /// A host with no children for a new document at the same depth. The
    /// top-level page gets a fresh frame budget; nested documents keep
    /// sharing the budget of their top-level page.
    pub fn for_new_document(&self) -> Self {
        let budget = if self.depth == 0 {
            Arc::new(AtomicUsize::new(MAX_FRAMES))
        } else {
            self.budget.clone()
        };
        Self::new(self.depth, budget)
    }

    fn take_budget(&self) -> bool {
        self.budget
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| left.checked_sub(1))
            .is_ok()
    }

    /// Start loading frames that are new in this render and resize frames
    /// whose content box changed. Never blocks.
    pub fn sync(&mut self, frames: &[FrameBox], parent_origin: &str, console: &crate::js::ConsoleBuffer) {
        if self.depth >= MAX_DEPTH {
            return;
        }
        for frame in frames {
            let key = frame.key();
            let size = (frame.width, frame.height);
            if let Some(child) = self
                .children
                .iter_mut()
                .find(|c| c.src == frame.src && c.ordinal == frame.ordinal)
            {
                if child.html != frame.html {
                    child.html = frame.html.clone();
                    child.requested = size;
                    child.awaiting = Some(key.clone());
                    let _ = child.tx.send(FrameCmd::Load {
                        url: frame.src.clone(),
                        html: frame.html.clone(),
                        width: frame.width,
                        height: frame.height,
                        key,
                    });
                } else if child.requested != size {
                    child.requested = size;
                    child.awaiting = Some(key.clone());
                    let _ = child.tx.send(FrameCmd::Resize { width: frame.width, height: frame.height, key });
                }
                continue;
            }
            if !self.take_budget() {
                continue;
            }
            let index = self.children.len();
            let parent = FrameParent {
                origin: parent_origin.to_string(),
                name: frame.name.clone(),
                console: Some(console.clone()),
            };
            let Some(tx) = spawn_child(index, self.depth + 1, self.budget.clone(), parent, self.events_tx.clone()) else {
                continue;
            };
            let _ = tx.send(FrameCmd::Load {
                url: frame.src.clone(),
                html: frame.html.clone(),
                width: frame.width,
                height: frame.height,
                key: key.clone(),
            });
            self.children.push(ChildFrame {
                src: frame.src.clone(),
                ordinal: frame.ordinal,
                html: frame.html.clone(),
                tx,
                requested: size,
                awaiting: Some(key),
                cached_key: None,
            });
        }
    }

    fn record(&mut self, event: FrameEvent) {
        match event {
            FrameEvent::Rendered { index, key, png } => {
                let Some(child) = self.children.get_mut(index) else { return };
                if child.awaiting.as_deref() == Some(key.as_str()) {
                    child.awaiting = None;
                }
                if let Some(png) = png {
                    self.ready_images.push((index, key, png));
                }
            }
            FrameEvent::Message { index, data, origin } => {
                let Some(child) = self.children.get(index) else { return };
                self.ready_messages.push(IncomingMessage {
                    src: child.src.clone(),
                    ordinal: child.ordinal,
                    data,
                    origin,
                });
            }
        }
    }

    /// Collect finished child work without blocking.
    pub fn poll(&mut self) {
        while let Ok(event) = self.events_rx.try_recv() {
            self.record(event);
        }
    }

    /// Block until a child reports something or `deadline` passes, then
    /// collect everything reported so far.
    pub fn wait_until(&mut self, deadline: Instant) {
        let timeout = deadline.saturating_duration_since(Instant::now());
        if let Ok(event) = self.events_rx.recv_timeout(timeout) {
            self.record(event);
        }
        self.poll();
    }

    /// Whether some child is still loading or re-rendering.
    pub fn has_pending(&self) -> bool {
        self.children.iter().any(|c| c.awaiting.is_some())
    }

    pub fn has_ready_images(&self) -> bool {
        !self.ready_images.is_empty()
    }

    pub fn has_ready_messages(&self) -> bool {
        !self.ready_messages.is_empty()
    }

    /// Move finished captures into `image_cache`, dropping each child's
    /// previous capture. Returns whether anything was stored.
    pub fn store_images(&mut self, image_cache: &mut HashMap<String, Vec<u8>>) -> bool {
        let stored = !self.ready_images.is_empty();
        for (index, key, png) in std::mem::take(&mut self.ready_images) {
            let child = &mut self.children[index];
            if let Some(old) = child.cached_key.replace(key.clone()) {
                if old != key {
                    image_cache.remove(&old);
                }
            }
            image_cache.insert(key, png);
        }
        stored
    }

    pub fn take_messages(&mut self) -> Vec<IncomingMessage> {
        std::mem::take(&mut self.ready_messages)
    }

    /// Deliver `iframe.contentWindow.postMessage` to the child document
    /// loaded for (`src`, `ordinal`). Returns whether such a child exists.
    pub fn post_to_child(&self, src: &str, ordinal: usize, data: String, origin: &str) -> bool {
        let Some(child) = self.children.iter().find(|c| c.src == src && c.ordinal == ordinal) else {
            return false;
        };
        child.tx.send(FrameCmd::Message { data, origin: origin.to_string() }).is_ok()
    }
}

fn spawn_child(
    index: usize,
    depth: u32,
    budget: Arc<AtomicUsize>,
    parent: FrameParent,
    events: mpsc::Sender<FrameEvent>,
) -> Option<mpsc::Sender<FrameCmd>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("frame-{depth}-{index}"))
        .stack_size(16 * 1024 * 1024)
        .spawn(move || run_child(index, depth, budget, parent, rx, events))
        .ok()?;
    Some(tx)
}

/// Move the frame document's console entries to the embedder's console,
/// labelled with the frame URL.
fn forward_console(engine: &crate::engine::BrowserEngine, parent: &Option<crate::js::ConsoleBuffer>, label: &str) {
    let Some(parent) = parent else { return };
    let entries = engine.console_entries();
    if entries.is_empty() {
        return;
    }
    engine.clear_console();
    for entry in entries {
        crate::js::append_console_entry(parent, entry.level, format!("[frame {label}] {}", entry.message));
    }
}

/// Frame thread body: owns the child engine for its whole life.
fn run_child(
    index: usize,
    depth: u32,
    budget: Arc<AtomicUsize>,
    parent: FrameParent,
    rx: mpsc::Receiver<FrameCmd>,
    events: mpsc::Sender<FrameEvent>,
) {
    FRAME_THREAD.with(|flag| flag.set(true));
    let parent_origin = parent.origin.clone();
    let parent_console = parent.console.clone();
    let mut frame_label = String::new();
    let mut engine = crate::engine::BrowserEngine::new_frame(FrameHost::new(depth, budget), parent);
    let forward = |engine: &mut crate::engine::BrowserEngine| {
        let origin = engine.document_origin();
        for message in engine.take_parent_messages() {
            if target_origin_allows(&message.target_origin, &parent_origin) {
                let _ = events.send(FrameEvent::Message { index, data: message.data, origin: origin.clone() });
            }
        }
    };
    let mut width = 0u32;
    let mut current_key = String::new();
    let mut last_command = Instant::now();
    let mut last_capture = Instant::now();
    let mut dirty = false;
    loop {
        let cmd = match rx.recv_timeout(FRAME_TICK_INTERVAL) {
            Ok(cmd) => cmd,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // No command: keep the document's timers running for a while,
                // as a browser does, and report what they change.
                if engine.last_page.is_none() || last_command.elapsed() > FRAME_IDLE_TICKS {
                    continue;
                }
                dirty |= engine.tick_js(None, None);
                forward(&mut engine);
                forward_console(&engine, &parent_console, &frame_label);
                if dirty && last_capture.elapsed() >= FRAME_IDLE_CAPTURE_INTERVAL {
                    dirty = false;
                    last_capture = Instant::now();
                    let _ = engine.re_render(None, None, width as f32);
                    let png = engine.screenshot_viewport().and_then(|pixmap| pixmap.encode_png().ok());
                    if events.send(FrameEvent::Rendered { index, key: current_key.clone(), png }).is_err() {
                        break;
                    }
                }
                continue;
            }
        };
        last_command = Instant::now();
        match cmd {
            FrameCmd::Load { url, html, width: w, height: h, key } => {
                (width, current_key) = (w, key);
                frame_label = url.clone();
                engine.viewport_height = (h as f32).max(1.0);
                engine.frame_deadline = Some(Instant::now() + FRAME_BUDGET);
                let loaded = match &html {
                    // A srcdoc document has its embedder's URL as base and origin.
                    Some(html) => match Url::parse(&parent_origin).and_then(|u| u.join("/")) {
                        Ok(base) => engine.load_html_document(html, &base, w as f32).map(|_| ()),
                        Err(error) => Err(error.to_string()),
                    },
                    None => engine.navigate(&url, w as f32).map(|_| ()),
                };
                if let Err(error) = loaded {
                    eprintln!("[frames] {url} failed to load: {error}");
                }
                forward(&mut engine);
                settle_child(&mut engine, width, &forward);
            }
            FrameCmd::Resize { width: w, height: h, key } => {
                (width, current_key) = (w, key);
                engine.viewport_height = (h as f32).max(1.0);
                if let Err(error) = engine.re_render(None, None, w as f32) {
                    eprintln!("[frames] re-render failed: {error}");
                }
            }
            FrameCmd::Message { data, origin } => {
                engine.deliver_parent_message(&data, &origin);
                let mut worked = false;
                for _ in 0..5 {
                    if !engine.tick_js(None, None) {
                        break;
                    }
                    worked = true;
                }
                forward(&mut engine);
                if !worked || engine.last_page.is_none() {
                    continue;
                }
                let _ = engine.re_render(None, None, width as f32);
            }
        }
        forward_console(&engine, &parent_console, &frame_label);
        dirty = false;
        last_capture = Instant::now();
        let png = engine.screenshot_viewport().and_then(|pixmap| pixmap.encode_png().ok());
        let rendered = FrameEvent::Rendered { index, key: current_key.clone(), png };
        if events.send(rendered).is_err() {
            break;
        }
    }
}

/// Run the freshly loaded child's timers for a short while (re-rendering
/// when they change the document), forwarding its messages to the parent.
fn settle_child(
    engine: &mut crate::engine::BrowserEngine,
    width: u32,
    forward: &dyn Fn(&mut crate::engine::BrowserEngine),
) {
    if engine.last_page.is_none() {
        return;
    }
    let until = Instant::now() + FRAME_SETTLE;
    let mut dirty = false;
    while Instant::now() < until {
        std::thread::sleep(FRAME_TICK_INTERVAL);
        dirty |= engine.tick_js(None, None);
        forward(engine);
    }
    if dirty {
        let _ = engine.re_render(None, None, width as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_key_format() {
        assert_eq!(
            iframe_frame_key("https://shopsquare.naver.com/", 831, 560),
            "iframe:831x560:https://shopsquare.naver.com/"
        );
    }

    #[test]
    fn test_target_origin_allows() {
        assert!(target_origin_allows("*", "https://a.com"));
        assert!(target_origin_allows("https://a.com/path", "https://a.com"));
        assert!(!target_origin_allows("https://b.com", "https://a.com"));
        assert!(!target_origin_allows("", "https://a.com"));
    }

    /// Frame console entries move to the embedder's console with the frame
    /// URL, and leave the frame's own console empty.
    #[test]
    fn test_forward_console_labels_entries_and_clears_the_frame_console() {
        let engine = crate::engine::BrowserEngine::new();
        crate::js::append_console_entry(&engine.console_buffer, crate::js::ConsoleLevel::Error, "boom".to_string());
        let parent = crate::js::new_console_buffer();
        forward_console(&engine, &Some(parent.clone()), "https://ad.example/frame");
        let entries = crate::js::console_entries(&parent);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "[frame https://ad.example/frame] boom");
        assert!(engine.console_entries().is_empty());
    }
}
