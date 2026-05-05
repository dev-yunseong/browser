use v8;
use std::collections::{HashMap, VecDeque};
use markup5ever_rcdom::{NodeData, Handle};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;
use serde::{Serialize, Deserialize};
use std::sync::Mutex;

thread_local! {
    static CONSOLE_BUFFER: RefCell<Option<ConsoleBuffer>> = const { RefCell::new(None) };
}

const MAX_CONSOLE_ENTRIES: usize = 200;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleLevel {
    Log,
    Warn,
    Error,
    Info,
    Debug,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    pub message: String,
    pub timestamp: u64,
}

pub struct ConsoleState {
    entries: Mutex<VecDeque<ConsoleEntry>>,
    version: AtomicU64,
}

pub type ConsoleBuffer = Arc<ConsoleState>;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalOutcome {
    pub result: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct LayoutMetrics {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl LayoutMetrics {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

fn now_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn new_console_buffer() -> ConsoleBuffer {
    Arc::new(ConsoleState {
        entries: Mutex::new(VecDeque::with_capacity(MAX_CONSOLE_ENTRIES)),
        version: AtomicU64::new(0),
    })
}

pub fn append_console_entry(buffer: &ConsoleBuffer, level: ConsoleLevel, message: String) {
    if let Ok(mut entries) = buffer.entries.lock() {
        if entries.len() >= MAX_CONSOLE_ENTRIES {
            entries.pop_front();
        }
        entries.push_back(ConsoleEntry {
            level,
            message,
            timestamp: now_timestamp_ms(),
        });
        buffer.version.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn clear_console_buffer(buffer: &ConsoleBuffer) {
    if let Ok(mut entries) = buffer.entries.lock() {
        entries.clear();
        buffer.version.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn console_entries(buffer: &ConsoleBuffer) -> Vec<ConsoleEntry> {
    buffer
        .entries
        .lock()
        .map(|entries| entries.iter().cloned().collect())
        .unwrap_or_default()
}

pub fn console_version(buffer: &ConsoleBuffer) -> u64 {
    buffer.version.load(Ordering::Relaxed)
}

fn push_console_entry(level: ConsoleLevel, message: String) {
    CONSOLE_BUFFER.with(|cell| {
        if let Some(buffer) = cell.borrow().as_ref() {
            append_console_entry(buffer, level, message);
        }
    });
}

#[derive(Clone, Debug, Default)]
pub struct CspPolicy {
    pub connect_src: Vec<String>,
    pub script_src: Vec<String>,
}

impl CspPolicy {
    pub fn parse(header: &str) -> Self {
        let mut policy = Self::default();
        for directive in header.split(';') {
            let parts: Vec<&str> = directive.trim().split_whitespace().collect();
            if parts.is_empty() { continue; }
            let name = parts[0].to_lowercase();
            let sources: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            
            match name.as_str() {
                "connect-src" => policy.connect_src = sources,
                "script-src" => policy.script_src = sources,
                _ => {}
            }
        }
        policy
    }

    pub fn is_allowed(&self, directive: &str, url: &Url, current_origin: Option<&Url>) -> bool {
        let sources = match directive {
            "connect-src" => &self.connect_src,
            "script-src" => &self.script_src,
            _ => return true,
        };

        if sources.is_empty() { return true; }

        for source in sources {
            if source == "*" { return true; }
            if source == "'self'" {
                if let Some(origin) = current_origin {
                    if origin.origin() == url.origin() { return true; }
                }
                continue;
            }
            // Simple string prefix/origin match
            if url.to_string().starts_with(source) { return true; }
        }
        false
    }

    pub fn allows_inline_script(&self) -> bool {
        if self.script_src.is_empty() { return true; }
        self.script_src.iter().any(|s| s == "'unsafe-inline'")
    }
}

pub struct JsRuntime {
    isolate: v8::OwnedIsolate,
    global_context: v8::Global<v8::Context>,
}

impl JsRuntime {
    pub fn new(
        _dom: Option<Handle>,
        _base_url: Option<Url>,
        _policy: Option<CspPolicy>,
        _layout_metrics: Option<HashMap<String, LayoutMetrics>>,
        console_buffer: ConsoleBuffer,
    ) -> Self {
        CONSOLE_BUFFER.with(|cell| *cell.borrow_mut() = Some(console_buffer));

        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let platform = v8::new_default_platform(0, false).make_shared();
            v8::V8::initialize_platform(platform);
            v8::V8::initialize();
        });

        let mut isolate = v8::Isolate::new(v8::CreateParams::default());

        let global_context;
        {
            let scope = std::pin::pin!(v8::HandleScope::new(&mut isolate));
            let scope = &mut scope.init();
            let context = v8::Context::new(scope, Default::default());
            global_context = v8::Global::new(scope, context);
        }

        JsRuntime {
            isolate,
            global_context,
        }
    }

    pub fn set_layout_metrics(&mut self, _layout_metrics: HashMap<String, LayoutMetrics>) {}

    pub fn tick(&mut self, _timestamp: Option<f64>, _deadline_ms: Option<f64>) -> bool {
        false
    }

    pub fn trigger_event(&mut self, _target_id: &str, _event_type: &str) {}

    pub fn execute(&mut self, source: &str) {
        let outcome = self.execute_with_result(source);
        if let Some(error) = outcome.error {
            eprintln!("[JS Error] execute: {}", error);
        }
    }

    pub fn execute_with_result(&mut self, source: &str) -> EvalOutcome {
        if source.contains("import.meta") || (source.contains("import ") && source.contains(" from ")) {
            return EvalOutcome {
                result: None,
                error: Some("ES module syntax is not supported".to_string()),
            };
        }

        let hs = std::pin::pin!(v8::HandleScope::new(&mut self.isolate));
        let hs = &mut hs.init();
        let local_context = v8::Local::new(hs, &self.global_context);
        let scope = &mut v8::ContextScope::new(hs, local_context);

        let code = match v8::String::new(scope, source) {
            Some(s) => s,
            None => {
                return EvalOutcome {
                    result: None,
                    error: Some("Failed to create V8 string".to_string()),
                }
            }
        };

        let tc = std::pin::pin!(v8::TryCatch::new(scope));
        let tc = &mut tc.init();

        match v8::Script::compile(tc, code, None) {
            None => {
                let error_msg = tc
                    .exception()
                    .and_then(|e| e.to_string(tc))
                    .map(|s| s.to_rust_string_lossy(tc))
                    .unwrap_or_else(|| "Unknown compilation error".to_string());
                EvalOutcome {
                    result: None,
                    error: Some(error_msg),
                }
            }
            Some(script) => match script.run(tc) {
                None => {
                    let error_msg = tc
                        .exception()
                        .and_then(|e| e.to_string(tc))
                        .map(|s| s.to_rust_string_lossy(tc))
                        .unwrap_or_else(|| "Unknown runtime error".to_string());
                    EvalOutcome {
                        result: None,
                        error: Some(error_msg),
                    }
                }
                Some(value) => {
                    let result = if value.is_undefined() {
                        Some("undefined".to_string())
                    } else if value.is_null() {
                        Some("null".to_string())
                    } else {
                        value.to_string(tc).map(|s| s.to_rust_string_lossy(tc))
                    };
                    EvalOutcome { result, error: None }
                }
            },
        }
    }

    pub fn get_style_overrides(&mut self) -> HashMap<String, HashMap<String, String>> {
        HashMap::new()
    }

    pub fn get_focused_node_id(&self) -> Option<String> {
        None
    }

    pub fn set_focused_node_id(&mut self, _id: Option<String>) {}
}

pub fn node_path_key(node: &Handle) -> String {
    let mut indices = Vec::new();
    let mut current = node.clone();

    loop {
        let parent_weak = current.parent.take();
        let Some(parent_weak) = parent_weak else {
            break;
        };
        let Some(parent) = parent_weak.upgrade() else {
            break;
        };

        let current_ptr = std::rc::Rc::as_ptr(&current) as usize;
        let index = parent
            .children
            .borrow()
            .iter()
            .position(|child| std::rc::Rc::as_ptr(child) as usize == current_ptr)
            .unwrap_or(0);
        indices.push(index.to_string());
        current.parent.set(Some(std::rc::Rc::downgrade(&parent)));
        current = parent;
    }

    indices.reverse();
    indices.join("/")
}

pub fn extract_scripts_from_dom(handle: &Handle) -> Vec<String> {
    let mut scripts = Vec::new();
    if let NodeData::Element { ref name, .. } = handle.data {
        if name.local.to_string() == "script" {
            let mut content = String::new();
            for child in handle.children.borrow().iter() {
                if let NodeData::Text { ref contents } = child.data {
                    content.push_str(&contents.borrow());
                }
            }
            if !content.is_empty() {
                scripts.push(content);
            }
        }
    }
    for child in handle.children.borrow().iter() {
        scripts.extend(extract_scripts_from_dom(child));
    }
    scripts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;

    fn make_runtime(html: &str) -> JsRuntime {
        let _dom = dom::parse_html(html);
        JsRuntime::new(None, None, None, None, new_console_buffer())
    }

    #[test]
    fn test_execute_with_result_returns_value() {
        let mut rt = make_runtime("<html><body></body></html>");
        let outcome = rt.execute_with_result("1 + 2");
        assert_eq!(outcome.result.as_deref(), Some("3"));
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn test_execute_with_result_returns_error() {
        let mut rt = make_runtime("<html><body></body></html>");
        let outcome = rt.execute_with_result("missingVariable");
        assert!(outcome.result.is_none());
        assert!(outcome.error.is_some());
    }

    #[test]
    fn test_execute_with_result_json() {
        let mut rt = make_runtime("<html><body></body></html>");
        let outcome = rt.execute_with_result("JSON.stringify({a: 1, b: [2, 3]})");
        assert_eq!(outcome.result.as_deref(), Some("{\"a\":1,\"b\":[2,3]}"));
        assert_eq!(outcome.error, None);
    }
}
