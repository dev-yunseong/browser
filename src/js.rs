use v8;
use std::collections::{HashMap, HashSet, VecDeque};
use markup5ever_rcdom::{NodeData, Handle};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;
use serde::{Serialize, Deserialize};
use std::sync::Mutex;
use std::rc::Rc;
use lazy_static::lazy_static;


lazy_static! {
    static ref GLOBAL_STORAGE: Mutex<OriginStorage> = Mutex::new(OriginStorage::load());
}

#[derive(Serialize, Deserialize, Default)]
struct OriginStorage {
    data: HashMap<String, HashMap<String, String>>,
}

impl OriginStorage {
    fn load() -> Self {
        std::fs::read_to_string("storage.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write("storage.json", s);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SiblingDirection { Next, Previous }

struct FetchHandlers {
    resolve: v8::Global<v8::Function>,
    reject: v8::Global<v8::Function>,
}

thread_local! {
    static MACRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce()>>> = RefCell::new(VecDeque::new());
    static MICRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce()>>> = RefCell::new(VecDeque::new());
    static RAF_TASKS: RefCell<VecDeque<Box<dyn FnOnce(f64)>>> = RefCell::new(VecDeque::new());
    static IDLE_TASKS: RefCell<VecDeque<(u32, Box<dyn FnOnce(f64)>)>> = RefCell::new(VecDeque::new());
    static NEXT_IDLE_ID: RefCell<u32> = RefCell::new(1);
    static DOM_ROOT: RefCell<Option<Handle>> = RefCell::new(None);
    static NODE_REGISTRY: RefCell<HashMap<u32, Handle>> = RefCell::new(HashMap::new());
    static REVERSE_NODE_REGISTRY: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
    static DOCUMENT_FRAGMENT_NODE_IDS: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
    static NEXT_NODE_ID: RefCell<u32> = RefCell::new(1);
    static FETCH_REGISTRY: RefCell<HashMap<u32, FetchHandlers>> = RefCell::new(HashMap::new());
    static FETCH_BODY_REGISTRY: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
    static NEXT_FETCH_ID: RefCell<u32> = RefCell::new(1);
    static TASK_SENDER: RefCell<Option<std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>>>> = RefCell::new(None);
    static FOCUSED_NODE: RefCell<Option<String>> = RefCell::new(None);
    static PREVIOUS_FOCUSED_NODE: RefCell<Option<String>> = RefCell::new(None);
    static CURRENT_ORIGIN: RefCell<Option<Url>> = RefCell::new(None);
    static CSP_POLICY: RefCell<Option<CspPolicy>> = RefCell::new(None);
    static LAYOUT_METRICS: RefCell<HashMap<String, LayoutMetrics>> = RefCell::new(HashMap::new());
    static CONSOLE_BUFFER: RefCell<Option<ConsoleBuffer>> = const { RefCell::new(None) };
    static RUN_PENDING: RefCell<Vec<(v8::Global<v8::Function>, Option<f64>)>> = RefCell::new(Vec::new());
    static FETCH_PENDING: RefCell<VecDeque<(v8::Global<v8::Function>, String, bool)>> = RefCell::new(VecDeque::new());
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
    task_receiver: std::sync::mpsc::Receiver<Box<dyn FnOnce() + Send>>,
}

impl JsRuntime {
    pub fn new(
        dom: Option<Handle>,
        base_url: Option<Url>,
        policy: Option<CspPolicy>,
        layout_metrics: Option<HashMap<String, LayoutMetrics>>,
        console_buffer: ConsoleBuffer,
    ) -> Self {
        DOM_ROOT.with(|root| *root.borrow_mut() = dom);
        CURRENT_ORIGIN.with(|origin| *origin.borrow_mut() = base_url);
        CSP_POLICY.with(|p| *p.borrow_mut() = policy);
        LAYOUT_METRICS.with(|metrics| *metrics.borrow_mut() = layout_metrics.unwrap_or_default());
        CONSOLE_BUFFER.with(|cell| *cell.borrow_mut() = Some(console_buffer));

        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let platform = v8::new_default_platform(0, false).make_shared();
            v8::V8::initialize_platform(platform);
            v8::V8::initialize();
        });

        let mut isolate = v8::Isolate::new(v8::CreateParams::default());
        let (task_sender, task_receiver) = std::sync::mpsc::channel();
        TASK_SENDER.with(|s| *s.borrow_mut() = Some(task_sender));

        let global_context;
        {
            let scope = std::pin::pin!(v8::HandleScope::new(&mut isolate));
            let scope = &mut scope.init();
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);
            let global = context.global(scope);

            let has_dom = DOM_ROOT.with(|r| r.borrow().is_some());
            if has_dom {
                register_native_functions(scope, global);

                {
                    let bootstrap = include_str!("js_bootstrap.js");
                    let tc = std::pin::pin!(v8::TryCatch::new(scope));
                    let tc = &mut tc.init();
                    let src = v8::String::new(tc, bootstrap).unwrap();
                    if let Some(script) = v8::Script::compile(tc, src, None) {
                        let _ = script.run(tc);
                    }

                    let url_init = CURRENT_ORIGIN.with(|origin| {
                        if let Some(ref url) = *origin.borrow() {
                            let href = url.to_string();
                            format!(r#"(function() {{
                                var _loc = __aura_create_location(JSON.parse({href_json:?}));
                                document.location = _loc;
                                document.URL = {href_json:?};
                                document.documentURI = {href_json:?};
                                document.baseURI = {href_json:?};
                                window.location = _loc;
                                location = _loc;
                            }})();"#, href_json = serde_json::to_string(&href).unwrap_or_default())
                        } else { String::new() }
                    });
                    if !url_init.is_empty() {
                        let src = v8::String::new(tc, &url_init).unwrap();
                        if let Some(script) = v8::Script::compile(tc, src, None) {
                            let _ = script.run(tc);
                        }
                    }

                    let script_allowed = CSP_POLICY.with(|p| {
                        p.borrow().as_ref().map(|pol| pol.allows_inline_script()).unwrap_or(true)
                    });
                    let flag_init = format!("window.__aura_inline_script_allowed = {};",
                        if script_allowed { "true" } else { "false" });
                    let src = v8::String::new(tc, &flag_init).unwrap();
                    if let Some(script) = v8::Script::compile(tc, src, None) {
                        let _ = script.run(tc);
                    }
                }
            }

            global_context = v8::Global::new(scope, context);
        }

        JsRuntime {
            isolate,
            global_context,
            task_receiver,
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


fn register_native_functions(scope: &mut v8::ContextScope<v8::HandleScope>, global: v8::Local<v8::Object>) {
    let f = v8::Function::new(scope, console_log).unwrap();
    global.set(scope, v8::String::new(scope, "log").unwrap().into(), f.into());
    let f = v8::Function::new(scope, console_warn).unwrap();
    global.set(scope, v8::String::new(scope, "warn").unwrap().into(), f.into());
    let f = v8::Function::new(scope, console_error).unwrap();
    global.set(scope, v8::String::new(scope, "error").unwrap().into(), f.into());
    let f = v8::Function::new(scope, console_info).unwrap();
    global.set(scope, v8::String::new(scope, "info").unwrap().into(), f.into());
    let f = v8::Function::new(scope, console_debug).unwrap();
    global.set(scope, v8::String::new(scope, "debug").unwrap().into(), f.into());
}

fn console_log(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    console_impl(scope, args, ConsoleLevel::Log);
}
fn console_warn(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    console_impl(scope, args, ConsoleLevel::Warn);
}
fn console_error(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    console_impl(scope, args, ConsoleLevel::Error);
}
fn console_info(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    console_impl(scope, args, ConsoleLevel::Info);
}
fn console_debug(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    console_impl(scope, args, ConsoleLevel::Debug);
}


fn register_fn(scope: &mut v8::ContextScope<v8::HandleScope>, global: v8::Local<v8::Object>,
    name: &str, cb: impl v8::MapFnTo<v8::FunctionCallback>,
) {
    let f = v8::Function::new(scope, cb).unwrap();
    let key = v8::String::new(scope, name).unwrap();
    global.set(scope, key.into(), f.into());
}


fn get_element_by_id_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let id = args.get(0).to_rust_string_lossy(scope);
    let res = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            find_element_by_id(r, &id).map(|h| {
                let tag = if let NodeData::Element { ref name, .. } = h.data { name.local.to_string() } else { String::new() };
                (register_node(h), tag)
            })
        } else { None }
    });
    if let Some((nid, tag)) = res {
        let obj = v8::Object::new(scope);
        obj.set(scope, v8::String::new(scope, "nid").unwrap().into(), v8::Number::new(scope, nid as f64).into());
        obj.set(scope, v8::String::new(scope, "tag").unwrap().into(), v8::String::new(scope, &tag).unwrap().into());
        obj.set(scope, v8::String::new(scope, "kind").unwrap().into(), v8::String::new(scope, "element").unwrap().into());
        rv.set(obj.into());
    } else { rv.set_null(); }
}

fn get_document_element_cb(scope: &mut v8::PinScope, _a: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = DOM_ROOT.with(|root| root.borrow().as_ref().and_then(find_document_element).map(register_node));
    if let Some(n) = nid { rv.set_uint32(n); } else { rv.set_null(); }
}
fn get_head_cb(scope: &mut v8::PinScope, _a: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = DOM_ROOT.with(|root| root.borrow().as_ref().and_then(|d| find_document_surface_element(d, "head")).map(register_node));
    if let Some(n) = nid { rv.set_uint32(n); } else { rv.set_null(); }
}
fn get_body_cb(scope: &mut v8::PinScope, _a: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = DOM_ROOT.with(|root| root.borrow().as_ref().and_then(|d| find_document_surface_element(d, "body")).map(register_node));
    if let Some(n) = nid { rv.set_uint32(n); } else { rv.set_null(); }
}

fn query_selector_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let root_nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let selector = args.get(1).to_rust_string_lossy(scope);
    let found = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            let search = if root_nid == 0 { r.clone() } else { NODE_REGISTRY.with(|reg| reg.borrow().get(&root_nid).cloned()).unwrap_or_else(|| r.clone()) };
            query_selector_first(&search, &selector, root_nid != 0)
        } else { None }
    });
    if let Some(h) = found { rv.set_uint32(register_node(h)); } else { rv.set_null(); }
}

fn query_selector_all_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let root_nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let selector = args.get(1).to_rust_string_lossy(scope);
    let nids = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            let search = if root_nid == 0 { r.clone() } else { NODE_REGISTRY.with(|reg| reg.borrow().get(&root_nid).cloned()).unwrap_or_else(|| r.clone()) };
            query_selector_all_nodes(&search, &selector, root_nid != 0)
        } else { vec![] }
    });
    let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
    let json = format!("[{}]", ids.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","));
    rv.set(v8::String::new(scope, &json).unwrap().into());
}

fn get_elements_by_class_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let root_nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let cls = args.get(1).to_rust_string_lossy(scope);
    let nids = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            let search = if root_nid == 0 { r.clone() } else { NODE_REGISTRY.with(|reg| reg.borrow().get(&root_nid).cloned()).unwrap_or_else(|| r.clone()) };
            find_elements_by_class(&search, &cls, root_nid != 0)
        } else { vec![] }
    });
    let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
    let json = format!("[{}]", ids.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","));
    rv.set(v8::String::new(scope, &json).unwrap().into());
}

fn get_elements_by_tag_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let root_nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let tag = args.get(1).to_rust_string_lossy(scope).to_lowercase();
    let nids = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            let search = if root_nid == 0 { r.clone() } else { NODE_REGISTRY.with(|reg| reg.borrow().get(&root_nid).cloned()).unwrap_or_else(|| r.clone()) };
            find_elements_by_tag_name(&search, &tag, root_nid != 0)
        } else { vec![] }
    });
    let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
    let json = format!("[{}]", ids.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","));
    rv.set(v8::String::new(scope, &json).unwrap().into());
}

fn get_parent_id_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let parent = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).and_then(|node| {
        let pw = node.parent.take(); let p = pw.and_then(|w| w.upgrade());
        if let Some(ref ph) = p { node.parent.set(Some(Rc::downgrade(ph))); } p
    }));
    if let Some(pn) = parent.map(register_node) { rv.set_uint32(pn); } else { rv.set_null(); }
}
fn sib_res(scope: &mut v8::PinScope, rv: &mut v8::ReturnValue<v8::Value>, nid: u32, dir: SiblingDirection, el_only: bool) {
    match get_sibling_id(nid, dir, el_only) { Some(sn) => rv.set_uint32(sn), None => rv.set_null() }
}
fn get_next_sibling_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    sib_res(scope, &mut rv, args.get(0).uint32_value(scope).unwrap_or(0), SiblingDirection::Next, false);
}
fn get_previous_sibling_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    sib_res(scope, &mut rv, args.get(0).uint32_value(scope).unwrap_or(0), SiblingDirection::Previous, false);
}
fn get_next_element_sibling_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    sib_res(scope, &mut rv, args.get(0).uint32_value(scope).unwrap_or(0), SiblingDirection::Next, true);
}
fn get_previous_element_sibling_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    sib_res(scope, &mut rv, args.get(0).uint32_value(scope).unwrap_or(0), SiblingDirection::Previous, true);
}

fn get_inner_html_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let html = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).map(|n| serialize_inner_html(n)).unwrap_or_default());
    rv.set(v8::String::new(scope, &html).unwrap().into());
}
fn get_outer_html_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let html = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).map(|n| { let mut o=String::new(); serialize_node(n, &mut o); o }).unwrap_or_default());
    rv.set(v8::String::new(scope, &html).unwrap().into());
}
fn set_inner_html_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let html = args.get(1).to_rust_string_lossy(scope);
    NODE_REGISTRY.with(|reg| {
        if let Some(node) = reg.borrow().get(&nid) {
            let tag = if let NodeData::Element { ref name, .. } = node.data { name.local.to_string() } else { "div".to_string() };
            let frag = parse_html_fragment(&html, &tag);
            let mut ch = node.children.borrow_mut();
            for c in ch.iter() { c.parent.set(None); } ch.clear();
            for c in frag { c.parent.set(Some(Rc::downgrade(node))); ch.push(c); }
        }
    });
}
fn get_text_content_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let t = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).map(|n| match &n.data { NodeData::Comment{contents} => contents.to_string(), _ => collect_text_content(n) }).unwrap_or_default());
    rv.set(v8::String::new(scope, &t).unwrap().into());
}
fn set_text_content_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let text = args.get(1).to_rust_string_lossy(scope);
    let node = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).cloned());
    if let Some(node) = node { match &node.data {
        NodeData::Text{contents} => { *contents.borrow_mut() = text.as_str().into(); }
        NodeData::Comment{..} => {
            use html5ever::tendril::StrTendril; use markup5ever_rcdom::Node;
            replace_registered_node(nid, Node::new(NodeData::Comment{contents:StrTendril::from(text.as_str())}));
        }
        _ => {
            use html5ever::tendril::StrTendril; use markup5ever_rcdom::Node;
            let tn = Node::new(NodeData::Text{contents:std::cell::RefCell::new(StrTendril::from(text.as_str()))});
            tn.parent.set(Some(Rc::downgrade(&node))); node.children.borrow_mut().clear(); node.children.borrow_mut().push(tn);
        }
    }}
}

fn get_attribute_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let name = args.get(1).to_rust_string_lossy(scope);
    let val = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).and_then(|n| {
        if let NodeData::Element{ref attrs,..}=n.data { attrs.borrow().iter().find(|a|a.name.local.to_string()==name).map(|a|a.value.to_string()) } else {None}
    }));
    if let Some(v) = val { rv.set(v8::String::new(scope, &v).unwrap().into()); } else { rv.set_null(); }
}
fn set_attribute_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let name = args.get(1).to_rust_string_lossy(scope);
    let val = args.get(2).to_rust_string_lossy(scope);
    NODE_REGISTRY.with(|reg| if let Some(n)=reg.borrow().get(&nid) { if let NodeData::Element{ref attrs,..}=n.data {
        let mut a=attrs.borrow_mut(); let mut f=false;
        for attr in a.iter_mut() { if attr.name.local.to_string()==name { attr.value=val.clone().into(); f=true; break; } }
        if !f { use html5ever::{QualName,LocalName,ns,Attribute};
            a.push(Attribute{name:QualName::new(None,ns!(html),LocalName::from(name)),value:val.into()}); }
    }});
}
fn get_attributes_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let json = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).map(|n| {
        if let NodeData::Element{ref attrs,..}=n.data { let items:Vec<String>=attrs.borrow().iter().map(|a|
            format!("{{\"name\":{},\"value\":{}}}", serde_json::to_string(&a.name.local.to_string()).unwrap_or_default(), serde_json::to_string(&a.value.to_string()).unwrap_or_default())
        ).collect(); format!("[{}]",items.join(",")) } else {"[]".to_string()}
    }).unwrap_or_else(|| "[]".to_string()));
    rv.set(v8::String::new(scope, &json).unwrap().into());
}
fn has_attribute_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let name = args.get(1).to_rust_string_lossy(scope);
    let f = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).map(|n| {
        if let NodeData::Element{ref attrs,..}=n.data { attrs.borrow().iter().any(|a|a.name.local.to_string()==name) } else {false}
    }).unwrap_or(false));
    rv.set_bool(f);
}
fn remove_attribute_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let name = args.get(1).to_rust_string_lossy(scope);
    NODE_REGISTRY.with(|reg| if let Some(n)=reg.borrow().get(&nid) { if let NodeData::Element{ref attrs,..}=n.data { attrs.borrow_mut().retain(|a|a.name.local.to_string()!=name); }});
}

fn create_element_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let tag = args.get(0).to_rust_string_lossy(scope);
    let tag = if tag.is_empty() { "div".to_string() } else { tag.to_lowercase() };
    use html5ever::{QualName,LocalName,ns}; use markup5ever_rcdom::Node;
    let n = Node::new(NodeData::Element{name:QualName::new(None,ns!(html),LocalName::from(tag)),attrs:std::cell::RefCell::new(vec![]),template_contents:std::cell::RefCell::new(None),mathml_annotation_xml_integration_point:false});
    rv.set_uint32(register_node(n));
}
fn create_text_node_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    use html5ever::tendril::StrTendril; use markup5ever_rcdom::Node;
    rv.set_uint32(register_node(Node::new(NodeData::Text{contents:std::cell::RefCell::new(StrTendril::from(args.get(0).to_rust_string_lossy(scope).as_str()))})));
}
fn create_comment_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    use html5ever::tendril::StrTendril; use markup5ever_rcdom::Node;
    rv.set_uint32(register_node(Node::new(NodeData::Comment{contents:StrTendril::from(args.get(0).to_rust_string_lossy(scope).as_str())})));
}
fn create_document_fragment_cb(scope: &mut v8::PinScope, _a: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    use markup5ever_rcdom::Node; let n = Node::new(NodeData::Document); let nid=register_node(n); mark_document_fragment_id(nid); rv.set_uint32(nid);
}

fn append_child_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let pn = args.get(0).uint32_value(scope).unwrap_or(0); let cn = args.get(1).uint32_value(scope).unwrap_or(0);
    NODE_REGISTRY.with(|reg| { let reg=reg.borrow();
        if let (Some(p),Some(c))=(reg.get(&pn),reg.get(&cn)) {
            if is_document_fragment_id(cn) { append_fragment_children(p,c); }
            else { detach_node_from_parent(c); c.parent.set(Some(Rc::downgrade(p))); p.children.borrow_mut().push(c.clone()); }
        }
    });
}
fn remove_child_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let pn = args.get(0).uint32_value(scope).unwrap_or(0); let cn = args.get(1).uint32_value(scope).unwrap_or(0);
    NODE_REGISTRY.with(|reg| { let reg=reg.borrow();
        if let (Some(p),Some(c))=(reg.get(&pn),reg.get(&cn)) { let cp=Rc::as_ptr(c) as usize; p.children.borrow_mut().retain(|x|Rc::as_ptr(x) as usize!=cp); c.parent.set(None); }
    });
}
fn insert_before_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let pn = args.get(0).uint32_value(scope).unwrap_or(0); let nn = args.get(1).uint32_value(scope).unwrap_or(0);
    let rn: Option<u32> = if args.get(2).is_null() { None } else { Some(args.get(2).uint32_value(scope).unwrap_or(0)) };
    NODE_REGISTRY.with(|reg| { let reg=reg.borrow();
        if let (Some(p),Some(nc))=(reg.get(&pn),reg.get(&nn)) {
            let pos = rn.and_then(|rv| reg.get(&rv).and_then(|refn| { let rp=Rc::as_ptr(refn) as usize; p.children.borrow().iter().position(|c|Rc::as_ptr(c) as usize==rp) }));
            if is_document_fragment_id(nn) { insert_fragment_children(p,nc,pos); }
            else { detach_node_from_parent(nc); nc.parent.set(Some(Rc::downgrade(p))); let mut ch=p.children.borrow_mut(); if let Some(pos)=pos { ch.insert(pos,nc.clone()); } else { ch.push(nc.clone()); } }
        }
    });
}
fn remove_self_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    NODE_REGISTRY.with(|reg| { let reg=reg.borrow(); if let Some(n)=reg.get(&nid) { let np=Rc::as_ptr(n) as usize;
        if let Some(pw)=n.parent.take() { if let Some(p)=pw.upgrade() { p.children.borrow_mut().retain(|c|Rc::as_ptr(c) as usize!=np); } } }
    });
}

fn get_children_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let ch = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).map(|n| n.children.borrow().iter().cloned().collect::<Vec<_>>()).unwrap_or_default());
    let mut items=vec![];
    for c in ch { let cn=register_node(c.clone()); match &c.data {
        NodeData::Element{ref name,ref attrs,..} => { let tag=name.local.to_string(); let id=attrs.borrow().iter().find(|a|a.name.local.to_string()=="id").map(|a|a.value.to_string()).unwrap_or_default();
            items.push(format!("{{\"nid\":{},\"tag\":\"{}\",\"id\":\"{}\",\"kind\":\"element\"}}",cn,tag,id)); }
        NodeData::Text{..}=>items.push(format!("{{\"nid\":{},\"tag\":\"\",\"id\":\"\",\"kind\":\"text\"}}",cn)),
        NodeData::Comment{..}=>items.push(format!("{{\"nid\":{},\"tag\":\"\",\"id\":\"\",\"kind\":\"comment\"}}",cn)),
        NodeData::Doctype{ref name,..}=>items.push(format!("{{\"nid\":{},\"tag\":\"{}\",\"id\":\"\",\"kind\":\"doctype\"}}",cn,name)),
        NodeData::Document=>items.push(format!("{{\"nid\":{},\"tag\":\"\",\"id\":\"\",\"kind\":\"fragment\"}}",cn)), _=>{}
    }}
    rv.set(v8::String::new(scope, &format!("[{}]",items.join(","))).unwrap().into());
}
fn get_node_info_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let info = NODE_REGISTRY.with(|reg| { let reg=reg.borrow(); if let Some(n)=reg.get(&nid) {
        if is_document_fragment_id(nid) { return Some((String::new(),String::new(),String::new(),"fragment".to_string())); }
        if let NodeData::Element{ref name,ref attrs,..}=n.data { let tag=name.local.to_string(); let ab=attrs.borrow();
            let id=ab.iter().find(|a|a.name.local.to_string()=="id").map(|a|a.value.to_string()).unwrap_or_default();
            let cls=ab.iter().find(|a|a.name.local.to_string()=="class").map(|a|a.value.to_string()).unwrap_or_default();
            return Some((tag,id,cls,"element".to_string())); }
        else if let NodeData::Text{..}=n.data { return Some((String::new(),String::new(),String::new(),"text".to_string())); }
        else if let NodeData::Comment{..}=n.data { return Some((String::new(),String::new(),String::new(),"comment".to_string())); }
        else if let NodeData::Doctype{ref name,..}=n.data { return Some((name.to_string(),String::new(),String::new(),"doctype".to_string())); }
    } None });
    if let Some((tag,id,cls,kind))=info { let o=v8::Object::new(scope);
        o.set(scope, v8::String::new(scope,"tag").unwrap().into(), v8::String::new(scope,&tag).unwrap().into());
        o.set(scope, v8::String::new(scope,"id").unwrap().into(), v8::String::new(scope,&id).unwrap().into());
        o.set(scope, v8::String::new(scope,"class").unwrap().into(), v8::String::new(scope,&cls).unwrap().into());
        o.set(scope, v8::String::new(scope,"kind").unwrap().into(), v8::String::new(scope,&kind).unwrap().into());
        rv.set(o.into()); } else { rv.set_null(); }
}
fn get_node_type_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let nt = NODE_REGISTRY.with(|reg| { if is_document_fragment_id(nid) { 11 } else { reg.borrow().get(&nid).map(|n| match n.data { NodeData::Element{..}=>1,NodeData::Text{..}=>3,NodeData::Comment{..}=>8,NodeData::Doctype{..}=>10,NodeData::Document=>9,_=>0}).unwrap_or(0) }});
    rv.set_int32(nt);
}
fn get_node_value_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let v = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).and_then(|n| match &n.data { NodeData::Text{contents}=>Some(contents.borrow().to_string()), NodeData::Comment{contents}=>Some(contents.to_string()), _=>None }));
    if let Some(s)=v { rv.set(v8::String::new(scope,&s).unwrap().into()); } else { rv.set_null(); }
}
fn set_node_value_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    set_text_content_cb(scope, args, _rv);
}
fn get_layout_metrics_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let m = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).and_then(|n| { let k=node_path_key(n); LAYOUT_METRICS.with(|lm| lm.borrow().get(&k).cloned()) }));
    if let Some(metrics)=m { let o=v8::Object::new(scope);
        o.set(scope, v8::String::new(scope,"x").unwrap().into(), v8::Number::new(scope,metrics.x as f64).into());
        o.set(scope, v8::String::new(scope,"y").unwrap().into(), v8::Number::new(scope,metrics.y as f64).into());
        o.set(scope, v8::String::new(scope,"width").unwrap().into(), v8::Number::new(scope,metrics.width as f64).into());
        o.set(scope, v8::String::new(scope,"height").unwrap().into(), v8::Number::new(scope,metrics.height as f64).into());
        o.set(scope, v8::String::new(scope,"top").unwrap().into(), v8::Number::new(scope,metrics.y as f64).into());
        o.set(scope, v8::String::new(scope,"left").unwrap().into(), v8::Number::new(scope,metrics.x as f64).into());
        o.set(scope, v8::String::new(scope,"right").unwrap().into(), v8::Number::new(scope,metrics.right() as f64).into());
        o.set(scope, v8::String::new(scope,"bottom").unwrap().into(), v8::Number::new(scope,metrics.bottom() as f64).into());
        rv.set(o.into()); } else { rv.set_null(); }
}
fn get_doctype_id_cb(scope: &mut v8::PinScope, _a: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = DOM_ROOT.with(|r| r.borrow().as_ref().and_then(find_document_doctype).map(register_node));
    if let Some(n)=nid { rv.set_uint32(n); } else { rv.set_null(); }
}
fn get_doctype_info_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let info = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).and_then(|n| {
        if let NodeData::Doctype{ref name,ref public_id,ref system_id}=n.data { Some((name.to_string(),public_id.to_string(),system_id.to_string())) } else {None}
    }));
    if let Some((name,pid,sid))=info { let o=v8::Object::new(scope);
        o.set(scope, v8::String::new(scope,"name").unwrap().into(), v8::String::new(scope,&name).unwrap().into());
        o.set(scope, v8::String::new(scope,"publicId").unwrap().into(), v8::String::new(scope,&pid).unwrap().into());
        o.set(scope, v8::String::new(scope,"systemId").unwrap().into(), v8::String::new(scope,&sid).unwrap().into());
        rv.set(o.into()); } else { rv.set_null(); }
}
fn set_focus_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let id = args.get(0).to_rust_string_lossy(scope);
    if id.is_empty() { FOCUSED_NODE.with(|f| *f.borrow_mut() = None); } else { FOCUSED_NODE.with(|f| *f.borrow_mut() = Some(id)); }
}
fn queue_task_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let cb = args.get(0);
    if cb.is_function() {
        let cb_func: v8::Local<v8::Function> = unsafe { std::mem::transmute(cb) };
        let cb_global = v8::Global::new(scope, cb_func);
        MACRO_TASKS.with(|tasks| tasks.borrow_mut().push_back(Box::new(move || {
            RUN_PENDING.with(|p| p.borrow_mut().push((cb_global.clone(), None)));
        })));
    }
}
fn resolve_url_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let input = args.get(0).to_rust_string_lossy(scope);
    let base = args.get(1).to_rust_string_lossy(scope);
    let resolved = if !base.is_empty() { Url::parse(&base).ok().and_then(|b|b.join(&input).ok()).or_else(||Url::parse(&input).ok()) } else { Url::parse(&input).ok() };
    rv.set(v8::String::new(scope, &resolved.map(|u|u.to_string()).unwrap_or(input)).unwrap().into());
}
fn can_execute_script_url_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let url_str = args.get(0).to_rust_string_lossy(scope);
    let base_url = CURRENT_ORIGIN.with(|o| (*o.borrow()).clone());
    let target = if let Some(ref b)=base_url { b.join(&url_str).unwrap_or_else(|_|Url::parse(&url_str).unwrap_or(b.clone())) } else { Url::parse(&url_str).unwrap_or_else(|_|Url::parse("about:blank").unwrap()) };
    let allowed = CSP_POLICY.with(|p| p.borrow().as_ref().map(|pol|pol.is_allowed("script-src",&target,base_url.as_ref())).unwrap_or(true));
    rv.set_bool(allowed);
}
fn parse_url_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let input = args.get(0).to_rust_string_lossy(scope);
    let base = args.get(1).to_rust_string_lossy(scope);
    let base = if base.is_empty() { None } else { Some(base) };
    let parsed = if let Some(ref b)=base { Url::parse(b).ok().and_then(|bu|bu.join(&input).ok()) } else { Url::parse(&input).ok() };
    if let Some(url)=parsed {
        let o=v8::Object::new(scope);
        let s = |k:&str,v:&str| { o.set(scope, v8::String::new(scope,k).unwrap().into(), v8::String::new(scope,v).unwrap().into()); };
        s("href",&url.to_string()); s("hostname",url.host_str().unwrap_or(""));
        s("pathname",url.path()); s("search",&url.query().map(|q|format!("?{}",q)).unwrap_or_default());
        s("hash",&url.fragment().map(|f|format!("#{}",f)).unwrap_or_default());
        s("protocol",&format!("{}:",url.scheme()));
        s("host",&url.host_str().map(|h| if let Some(p)=url.port(){format!("{}:{}",h,p)}else{h.to_string()}).unwrap_or_default());
        s("port",&url.port().map(|p|p.to_string()).unwrap_or_default());
        s("origin",&url.origin().unicode_serialization());
        rv.set(o.into()); } else { rv.set_null(); }
}
fn storage_get_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue<v8::Value>) {
    let key = args.get(0).to_rust_string_lossy(scope);
    let origin = CURRENT_ORIGIN.with(|o| o.borrow().as_ref().map(|u|u.origin().unicode_serialization()).unwrap_or_else(||"null".to_string()));
    let store = GLOBAL_STORAGE.lock().unwrap();
    let val = store.data.get(&origin).and_then(|m|m.get(&key)).cloned().unwrap_or_else(||"null".to_string());
    if val=="null" { rv.set_null(); } else { rv.set(v8::String::new(scope,&val).unwrap().into()); }
}
fn storage_set_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let key = args.get(0).to_rust_string_lossy(scope); let value = args.get(1).to_rust_string_lossy(scope);
    let origin = CURRENT_ORIGIN.with(|o| o.borrow().as_ref().map(|u|u.origin().unicode_serialization()).unwrap_or_else(||"null".to_string()));
    let mut store = GLOBAL_STORAGE.lock().unwrap();
    store.data.entry(origin).or_insert_with(HashMap::new).insert(key,value); store.save();
}
fn storage_remove_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let key = args.get(0).to_rust_string_lossy(scope);
    let origin = CURRENT_ORIGIN.with(|o| o.borrow().as_ref().map(|u|u.origin().unicode_serialization()).unwrap_or_else(||"null".to_string()));
    let mut store = GLOBAL_STORAGE.lock().unwrap();
    if let Some(m)=store.data.get_mut(&origin) { m.remove(&key); store.save(); }
}
fn storage_clear_cb(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {
    let origin = CURRENT_ORIGIN.with(|o| o.borrow().as_ref().map(|u|u.origin().unicode_serialization()).unwrap_or_else(||"null".to_string()));
    let mut store = GLOBAL_STORAGE.lock().unwrap();
    if let Some(m)=store.data.get_mut(&origin) { m.clear(); store.save(); }
}
fn fetch_cb(_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue<v8::Value>) {}

fn console_impl(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, level: ConsoleLevel) {
    let mut output = String::new();
    for i in 0..args.length() {
        if i > 0 { output.push(' '); }
        output.push_str(&args.get(i).to_rust_string_lossy(scope));
    }
    push_console_entry(level, output);
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

// ── DOM Helper Functions ──────────────────────────────────────────────────────

fn find_element_by_tag(root: &Handle, tag: &str) -> Option<Handle> {
    if let NodeData::Element { ref name, .. } = root.data {
        if name.local.to_string() == tag { return Some(root.clone()); }
    }
    for child in root.children.borrow().iter() {
        if let Some(found) = find_element_by_tag(child, tag) { return Some(found); }
    }
    None
}

fn find_direct_child_element_by_tag(root: &Handle, tag: &str) -> Option<Handle> {
    for child in root.children.borrow().iter() {
        if let NodeData::Element { ref name, .. } = child.data {
            if name.local.to_string() == tag { return Some(child.clone()); }
        }
    }
    None
}

fn find_document_element(root: &Handle) -> Option<Handle> {
    find_direct_child_element_by_tag(root, "html")
}

fn find_document_surface_element(root: &Handle, tag: &str) -> Option<Handle> {
    find_document_element(root).and_then(|doc_el| find_direct_child_element_by_tag(&doc_el, tag))
}

fn find_element_by_id(root: &Handle, id: &str) -> Option<Handle> {
    if let NodeData::Element { ref attrs, .. } = root.data {
        for attr in attrs.borrow().iter() {
            if attr.name.local.to_string() == "id" && attr.value.to_string() == id {
                return Some(root.clone());
            }
        }
    }
    for child in root.children.borrow().iter() {
        if let Some(found) = find_element_by_id(child, id) { return Some(found); }
    }
    None
}

fn find_document_doctype(root: &Handle) -> Option<Handle> {
    for child in root.children.borrow().iter() {
        if let NodeData::Doctype { .. } = child.data { return Some(child.clone()); }
    }
    None
}

fn register_node(handle: Handle) -> u32 {
    let ptr = Rc::as_ptr(&handle) as usize;
    if let Some(id) = REVERSE_NODE_REGISTRY.with(|reg| reg.borrow().get(&ptr).cloned()) {
        return id;
    }
    let id = NEXT_NODE_ID.with(|id_cell| {
        let id = *id_cell.borrow();
        *id_cell.borrow_mut() += 1;
        id
    });
    NODE_REGISTRY.with(|reg| reg.borrow_mut().insert(id, handle));
    REVERSE_NODE_REGISTRY.with(|reg| reg.borrow_mut().insert(ptr, id));
    id
}

fn replace_registered_node(id: u32, new_handle: Handle) {
    let parent = NODE_REGISTRY.with(|reg| {
        reg.borrow().get(&id).and_then(|node| node.parent.take().and_then(|weak| weak.upgrade()))
    });
    if let Some(ref parent_handle) = parent {
        let old_ptr = NODE_REGISTRY.with(|reg| reg.borrow().get(&id).map(|node| Rc::as_ptr(node) as usize).unwrap_or(0));
        let mut children = parent_handle.children.borrow_mut();
        if let Some(pos) = children.iter().position(|child| Rc::as_ptr(child) as usize == old_ptr) {
            new_handle.parent.set(Some(Rc::downgrade(parent_handle)));
            children[pos] = new_handle.clone();
        }
    }
    NODE_REGISTRY.with(|reg| {
        if let Some(old_handle) = reg.borrow_mut().insert(id, new_handle.clone()) {
            let old_ptr = Rc::as_ptr(&old_handle) as usize;
            REVERSE_NODE_REGISTRY.with(|reverse| { reverse.borrow_mut().remove(&old_ptr); });
        }
    });
    let new_ptr = Rc::as_ptr(&new_handle) as usize;
    REVERSE_NODE_REGISTRY.with(|reg| reg.borrow_mut().insert(new_ptr, id));
}

fn mark_document_fragment_id(id: u32) {
    DOCUMENT_FRAGMENT_NODE_IDS.with(|ids| { ids.borrow_mut().insert(id); });
}

fn is_document_fragment_id(id: u32) -> bool {
    DOCUMENT_FRAGMENT_NODE_IDS.with(|ids| ids.borrow().contains(&id))
}

fn detach_node_from_parent(node: &Handle) {
    let node_ptr = Rc::as_ptr(node) as usize;
    let parent_weak = node.parent.take();
    if let Some(parent_weak) = parent_weak {
        if let Some(parent) = parent_weak.upgrade() {
            parent.children.borrow_mut().retain(|c| Rc::as_ptr(c) as usize != node_ptr);
        }
    }
}

fn get_parent_handle(node: &Handle) -> Option<Handle> {
    let parent_weak = node.parent.take();
    let parent = parent_weak.and_then(|weak| weak.upgrade());
    if let Some(ref parent_handle) = parent {
        node.parent.set(Some(Rc::downgrade(parent_handle)));
    }
    parent
}

fn get_sibling_id(nid: u32, direction: SiblingDirection, elements_only: bool) -> Option<u32> {
    let node = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).cloned())?;
    let parent = get_parent_handle(&node)?;
    let node_ptr = Rc::as_ptr(&node) as usize;
    let children: Vec<Handle> = parent.children.borrow().iter().cloned().collect();
    let index = children.iter().position(|child| Rc::as_ptr(child) as usize == node_ptr)?;
    match direction {
        SiblingDirection::Next => {
            for sibling in children.iter().skip(index + 1) {
                if elements_only && !matches!(sibling.data, NodeData::Element { .. }) { continue; }
                return Some(register_node(sibling.clone()));
            }
        }
        SiblingDirection::Previous => {
            for sibling in children[..index].iter().rev() {
                if elements_only && !matches!(sibling.data, NodeData::Element { .. }) { continue; }
                return Some(register_node(sibling.clone()));
            }
        }
    }
    None
}

fn append_fragment_children(parent: &Handle, fragment: &Handle) {
    let fragment_children: Vec<Handle> = fragment.children.borrow_mut().drain(..).collect();
    let mut parent_children = parent.children.borrow_mut();
    for child in fragment_children {
        detach_node_from_parent(&child);
        child.parent.set(Some(Rc::downgrade(parent)));
        parent_children.push(child);
    }
}

fn insert_fragment_children(parent: &Handle, fragment: &Handle, insert_pos: Option<usize>) {
    let fragment_children: Vec<Handle> = fragment.children.borrow_mut().drain(..).collect();
    let mut parent_children = parent.children.borrow_mut();
    let mut pos = insert_pos.unwrap_or(parent_children.len());
    for child in fragment_children {
        detach_node_from_parent(&child);
        child.parent.set(Some(Rc::downgrade(parent)));
        parent_children.insert(pos, child);
        pos += 1;
    }
}

fn parse_html_fragment(html: &str, ctx_tag: &str) -> Vec<Handle> {
    use html5ever::parse_fragment;
    use html5ever::tendril::TendrilSink;
    use html5ever::{QualName, LocalName, ns};
    let ctx_name = QualName::new(None, ns!(html), LocalName::from(ctx_tag));
    let dom = parse_fragment(
        markup5ever_rcdom::RcDom::default(),
        Default::default(), ctx_name, vec![], false,
    ).from_utf8().read_from(&mut html.as_bytes()).unwrap();
    steal_fragment_children(&dom.document)
}

fn steal_fragment_children(doc: &Handle) -> Vec<Handle> {
    for child in doc.children.borrow().iter() {
        if let NodeData::Element { .. } = child.data {
            let children: Vec<Handle> = child.children.borrow_mut().drain(..).collect();
            for c in &children { c.parent.set(None); }
            return children;
        }
    }
    vec![]
}

fn serialize_inner_html(node: &Handle) -> String {
    let mut out = String::new();
    for child in node.children.borrow().iter() {
        serialize_node(child, &mut out);
    }
    out
}

fn serialize_node(node: &Handle, out: &mut String) {
    match &node.data {
        NodeData::Element { ref name, ref attrs, .. } => {
            let tag = name.local.to_string();
            out.push('<'); out.push_str(&tag);
            for attr in attrs.borrow().iter() {
                out.push(' ');
                out.push_str(&attr.name.local.to_string());
                out.push_str("=\"");
                out.push_str(&html_escape(&attr.value.to_string()));
                out.push('"');
            }
            out.push('>');
            for child in node.children.borrow().iter() { serialize_node(child, out); }
            out.push_str("</"); out.push_str(&tag); out.push('>');
        }
        NodeData::Text { ref contents } => {
            out.push_str(&html_escape(&contents.borrow()));
        }
        NodeData::Comment { ref contents } => {
            out.push_str("<!--"); out.push_str(contents); out.push_str("-->");
        }
        NodeData::Doctype { ref name, ref public_id, ref system_id } => {
            out.push_str("<!DOCTYPE "); out.push_str(name);
            if !public_id.is_empty() { out.push_str(" PUBLIC \""); out.push_str(public_id); out.push('"');
                if !system_id.is_empty() { out.push_str(" \""); out.push_str(system_id); out.push('"'); }
            }
            out.push('>');
        }
        NodeData::Document => {
            for child in node.children.borrow().iter() { serialize_node(child, out); }
        }
        _ => {}
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn collect_text_content(node: &Handle) -> String {
    let mut text = String::new();
    match &node.data {
        NodeData::Text { ref contents } => { text.push_str(&contents.borrow()); }
        _ => {
            for child in node.children.borrow().iter() {
                text.push_str(&collect_text_content(child));
            }
        }
    }
    text
}



fn query_selector_first(_root: &Handle, _selector: &str, _skip_root: bool) -> Option<Handle> {
    // TODO: implement CSS selector matching
    None
}
fn query_selector_all_nodes(_root: &Handle, _selector: &str, _skip_root: bool) -> Vec<Handle> {
    // TODO: implement CSS selector matching
    vec![]
}
fn find_elements_by_class(root: &Handle, cls: &str, skip_root: bool) -> Vec<Handle> {
    let mut out = vec![];
    if !skip_root {
        if let NodeData::Element { ref attrs, .. } = root.data {
            let classes: Vec<String> = attrs.borrow().iter()
                .find(|a| a.name.local.to_string() == "class")
                .map(|a| a.value.to_string().split_whitespace().map(|s| s.to_string()).collect())
                .unwrap_or_default();
            if classes.contains(&cls.to_string()) { out.push(root.clone()); }
        }
    }
    for child in root.children.borrow().iter() {
        out.extend(find_elements_by_class(child, cls, false));
    }
    out
}
fn find_elements_by_tag_name(root: &Handle, tag: &str, skip_root: bool) -> Vec<Handle> {
    let mut out = vec![];
    if !skip_root {
        if let NodeData::Element { ref name, .. } = root.data {
            if name.local.to_string() == tag { out.push(root.clone()); }
        }
    }
    for child in root.children.borrow().iter() {
        out.extend(find_elements_by_tag_name(child, tag, false));
    }
    out
}
