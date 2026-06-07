use crate::css::{parse_selector, AttributeMatch, Combinator, Selector};
use lazy_static::lazy_static;
use markup5ever_rcdom::{Handle, NodeData};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroI32;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;
use v8;

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
enum SiblingDirection {
    Next,
    Previous,
}

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
    /// Module source cache for resolve callback during instantiate_module.
    static MODULE_SOURCES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// Maps module identity hash → URL, used by resolve callback when
    /// get_source_url() is unavailable (rusty_v8 v147 limitation).
    static MODULE_ID_TO_URL: RefCell<HashMap<NonZeroI32, String>> = RefCell::new(HashMap::new());
    /// Cache of compiled dependency modules during instantiation.
    /// Prevents re-compilation of the same module in cyclic import graphs.
    static RESOLVED_MODULES: RefCell<HashMap<String, v8::Global<v8::Module>>> =
        RefCell::new(HashMap::new());
    /// Pending form submission requests from JS — checked by engine.
    static FORM_SUBMIT_REQUESTS: RefCell<Vec<u32>> = RefCell::new(Vec::new());
}

unsafe extern "C" fn import_meta_callback(
    context: v8::Local<v8::Context>,
    module: v8::Local<v8::Module>,
    meta: v8::Local<v8::Object>,
) {
    v8::callback_scope!(unsafe scope, context);

    let hash = module.get_identity_hash();
    let url = MODULE_ID_TO_URL
        .with(|map| map.borrow().get(&hash).cloned())
        .unwrap_or_else(|| "unknown".to_string());

    let key = v8::String::new(scope, "url").unwrap();
    let value = v8::String::new(scope, &url).unwrap();
    meta.set(scope, key.into(), value.into());
}

fn host_import_module_dynamically<'s, 'i>(
    scope: &mut v8::PinScope<'s, 'i>,
    _host_defined_options: v8::Local<'s, v8::Data>,
    _resource_name: v8::Local<'s, v8::Value>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let spec_str = specifier.to_rust_string_lossy(scope);

    let resolver = v8::PromiseResolver::new(scope)?;
    let promise = resolver.get_promise(scope);

    let reject = |msg: &str| {
        let err = v8::String::new(scope, msg).unwrap();
        resolver.reject(scope, err.into());
    };

    let resolved_url = CURRENT_ORIGIN.with(|origin| {
        origin
            .borrow()
            .as_ref()
            .and_then(|base| base.join(&spec_str).ok())
            .map(|u| u.to_string())
    });

    let resolved_str = match resolved_url {
        Some(url) => url,
        None => {
            reject(&format!("Failed to resolve module specifier: {spec_str}"));
            return Some(promise);
        }
    };

    let allowed = CSP_POLICY.with(|p| {
        p.borrow()
            .as_ref()
            .map(|pol| {
                let url = Url::parse(&resolved_str)
                    .unwrap_or_else(|_| Url::parse("about:blank").unwrap());
                let base = CURRENT_ORIGIN.with(|o| o.borrow().clone());
                pol.is_allowed("script-src", &url, base.as_ref())
            })
            .unwrap_or(true)
    });

    if !allowed {
        reject("CSP blocked dynamic import");
        return Some(promise);
    }

    let source = MODULE_SOURCES.with(|map| map.borrow().get(&resolved_str).cloned());

    let source = match source {
        Some(s) => s,
        None => match fetch_module_source_with_timeout(&resolved_str) {
            Ok(s) => s,
            Err(e) => {
                reject(&format!("Failed to fetch module: {e}"));
                return Some(promise);
            }
        },
    };

    MODULE_SOURCES.with(|map| {
        map.borrow_mut()
            .insert(resolved_str.clone(), source.clone());
    });

    let code = v8::String::new(scope, &source)?;
    let resource = v8::String::new(scope, &resolved_str)?;
    let origin = v8::ScriptOrigin::new(
        scope,
        resource.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        true,
        None,
    );
    let mut source_compiler = v8::script_compiler::Source::new(code, Some(&origin));

    let module = match v8::script_compiler::compile_module(scope, &mut source_compiler) {
        Some(m) => m,
        None => {
            reject("Module compilation failed");
            return Some(promise);
        }
    };

    MODULE_ID_TO_URL.with(|map| {
        map.borrow_mut()
            .insert(module.get_identity_hash(), resolved_str.clone());
    });
    RESOLVED_MODULES.with(|map| {
        map.borrow_mut()
            .insert(resolved_str.clone(), v8::Global::new(scope, module));
    });

    let instantiate_ok = module
        .instantiate_module(scope, resolve_module_callback)
        .unwrap_or(false);
    if !instantiate_ok {
        reject("Module instantiation failed");
        return Some(promise);
    }

    let eval_result = module.evaluate(scope);
    if eval_result.is_none() || module.get_status() == v8::ModuleStatus::Errored {
        let err_msg = if module.get_status() == v8::ModuleStatus::Errored {
            module
                .get_exception()
                .to_string(scope)
                .map(|s| s.to_rust_string_lossy(scope))
                .unwrap_or_else(|| "Module evaluation error".to_string())
        } else {
            "Module evaluation failed".to_string()
        };
        reject(&err_msg);
        return Some(promise);
    }

    let namespace = module.get_module_namespace();
    resolver.resolve(scope, namespace);

    Some(promise)
}

fn fetch_module_source_with_timeout(url: &str) -> Result<String, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(url)
        .send()?
        .text()
}

fn resolve_module_callback<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    v8::callback_scope!(unsafe scope, context);

    let spec_str = specifier.to_rust_string_lossy(scope);
    let base_url = {
        let hash = referrer.get_identity_hash();
        MODULE_ID_TO_URL.with(|map| map.borrow().get(&hash).cloned())
    };

    let resolved_url = match base_url.as_deref() {
        Some(base) => match Url::parse(base).and_then(|b| b.join(&spec_str)) {
            Ok(url) => url.to_string(),
            Err(_) => return None,
        },
        None => return None,
    };

    let source = MODULE_SOURCES.with(|map| map.borrow().get(&resolved_url).cloned())?;

    // Return cached module if already resolved (handles cyclic imports)
    if let Some(cached) = RESOLVED_MODULES.with(|map| map.borrow().get(&resolved_url).cloned()) {
        return Some(v8::Local::new(scope, &cached));
    }

    let resource = v8::String::new(scope, &resolved_url)?;
    let origin = v8::ScriptOrigin::new(
        scope,
        resource.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        true,
        None,
    );
    let code = v8::String::new(scope, &source)?;
    let mut source_compiler = v8::script_compiler::Source::new(code, Some(&origin));
    let dep_module = v8::script_compiler::compile_module(scope, &mut source_compiler)?;

    let cached = v8::Global::new(scope, dep_module);
    RESOLVED_MODULES.with(|map| {
        map.borrow_mut().insert(resolved_url.clone(), cached);
    });

    MODULE_ID_TO_URL.with(|map| {
        map.borrow_mut()
            .insert(dep_module.get_identity_hash(), resolved_url);
    });

    Some(dep_module)
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

pub fn push_console_entry(level: ConsoleLevel, message: String) {
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
            if parts.is_empty() {
                continue;
            }
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

        if sources.is_empty() {
            return true;
        }

        for source in sources {
            if source == "*" {
                return true;
            }
            if source == "'self'" {
                if let Some(origin) = current_origin {
                    if origin.origin() == url.origin() {
                        return true;
                    }
                }
                continue;
            }
            // Simple string prefix/origin match
            if url.to_string().starts_with(source) {
                return true;
            }
        }
        false
    }

    pub fn allows_inline_script(&self) -> bool {
        if self.script_src.is_empty() {
            return true;
        }
        self.script_src.iter().any(|s| s == "'unsafe-inline'")
    }
}

pub struct JsRuntime {
    global_context: v8::Global<v8::Context>,
    module_loader: ModuleLoader,
    task_receiver: std::sync::mpsc::Receiver<Box<dyn FnOnce() + Send>>,
    isolate: v8::OwnedIsolate,
}

#[derive(Default)]
pub struct ModuleLoader {
    modules: HashMap<Url, ModuleRecord>,
}

struct ModuleRecord {
    source: String,
    requests: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleCompileOutcome {
    pub url: Url,
    pub from_cache: bool,
    pub requests: Vec<String>,
    pub error: Option<String>,
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
        isolate.set_host_initialize_import_meta_object_callback(import_meta_callback);
        isolate.set_host_import_module_dynamically_callback(host_import_module_dynamically);
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
                            let href_json =
                                serde_json::to_string(&href).unwrap_or_else(|_| "\"\"".to_string());
                            format!(
                                r#"(function() {{
                                var href = {href_json};
                                var _loc = __aura_create_location(href);
                                document.location = _loc;
                                document.URL = href;
                                document.documentURI = href;
                                document.baseURI = href;
                                window.location = _loc;
                                location = _loc;
                            }})();"#,
                                href_json = href_json
                            )
                        } else {
                            String::new()
                        }
                    });
                    if !url_init.is_empty() {
                        let src = v8::String::new(tc, &url_init).unwrap();
                        if let Some(script) = v8::Script::compile(tc, src, None) {
                            let _ = script.run(tc);
                        }
                    }

                    let script_allowed = CSP_POLICY.with(|p| {
                        p.borrow()
                            .as_ref()
                            .map(|pol| pol.allows_inline_script())
                            .unwrap_or(true)
                    });
                    let flag_init = format!(
                        "window.__aura_inline_script_allowed = {};",
                        if script_allowed { "true" } else { "false" }
                    );
                    let src = v8::String::new(tc, &flag_init).unwrap();
                    if let Some(script) = v8::Script::compile(tc, src, None) {
                        let _ = script.run(tc);
                    }
                }
            }

            global_context = v8::Global::new(scope, context);
        }

        JsRuntime {
            global_context,
            module_loader: ModuleLoader::default(),
            task_receiver,
            isolate,
        }
    }

    pub fn set_layout_metrics(&mut self, layout_metrics: HashMap<String, LayoutMetrics>) {
        LAYOUT_METRICS.with(|metrics| *metrics.borrow_mut() = layout_metrics);
    }

    pub fn tick(&mut self, timestamp: Option<f64>, deadline_ms: Option<f64>) -> bool {
        const MAX_TASKS_PER_TICK: usize = 32;
        let mut did_work = false;
        if self.sync_focus() {
            did_work = true;
        }

        for _ in 0..MAX_TASKS_PER_TICK {
            match self.task_receiver.try_recv() {
                Ok(task) => MACRO_TASKS.with(|tasks| tasks.borrow_mut().push_back(task)),
                Err(_) => break,
            }
        }

        let macro_task = MACRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
        if let Some(task) = macro_task {
            task();
            did_work = true;
        }

        self.run_pending_callbacks();
        self.run_microtasks();

        if let Some(ts) = timestamp {
            let mut tasks = VecDeque::new();
            RAF_TASKS.with(|t| {
                let mut queue = t.borrow_mut();
                let count = queue.len().min(MAX_TASKS_PER_TICK);
                tasks = queue.drain(..count).collect();
            });
            if !tasks.is_empty() {
                did_work = true;
                for task in tasks {
                    task(ts);
                    self.run_pending_callbacks();
                    self.run_microtasks();
                }
            }
        }

        if let Some(deadline) = deadline_ms {
            loop {
                let task_opt = IDLE_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
                if let Some((_, task)) = task_opt {
                    did_work = true;
                    task(deadline);
                    self.run_pending_callbacks();
                    self.run_microtasks();
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as f64;
                    if now >= deadline {
                        break;
                    }
                } else {
                    break;
                }
            }
        }

        for _ in 0..MAX_TASKS_PER_TICK {
            let pending = FETCH_PENDING.with(|p| p.borrow_mut().pop_front());
            let Some((callback, payload, is_resolve)) = pending else {
                break;
            };
            did_work = true;
            self.call_with_payload(callback, &payload, is_resolve);
            self.run_microtasks();
        }

        did_work
    }

    fn run_pending_callbacks(&mut self) {
        const MAX_PENDING_CALLBACKS_PER_DRAIN: usize = 64;
        let pending: Vec<(v8::Global<v8::Function>, Option<f64>)> = RUN_PENDING.with(|p| {
            let mut pending = p.borrow_mut();
            let count = pending.len().min(MAX_PENDING_CALLBACKS_PER_DRAIN);
            pending.drain(..count).collect()
        });
        if pending.is_empty() {
            return;
        }
        let hs = std::pin::pin!(v8::HandleScope::new(&mut self.isolate));
        let hs = &mut hs.init();
        let local_context = v8::Local::new(hs, &self.global_context);
        let scope = &mut v8::ContextScope::new(hs, local_context);
        for (cb_global, ts_opt) in pending {
            let func = v8::Local::new(scope, &cb_global);
            let undef = v8::undefined(scope);
            if let Some(ts) = ts_opt {
                let ts_val = v8::Number::new(scope, ts);
                let _ = func.call(scope, undef.into(), &[ts_val.into()]);
            } else {
                let _ = func.call(scope, undef.into(), &[]);
            }
        }
    }

    fn call_with_payload(
        &mut self,
        callback: v8::Global<v8::Function>,
        payload: &str,
        is_resolve: bool,
    ) {
        let hs = std::pin::pin!(v8::HandleScope::new(&mut self.isolate));
        let hs = &mut hs.init();
        let local_context = v8::Local::new(hs, &self.global_context);
        let scope = &mut v8::ContextScope::new(hs, local_context);
        let func = v8::Local::new(scope, &callback);
        let undef = v8::undefined(scope);
        if is_resolve {
            let tc2 = std::pin::pin!(v8::TryCatch::new(scope));
            let tc2 = &mut tc2.init();
            let script_src = format!("__aura_make_fetch_response({})", payload);
            let src = v8::String::new(tc2, &script_src).unwrap();
            if let Some(script) = v8::Script::compile(tc2, src, None) {
                if let Some(result) = script.run(tc2) {
                    let _ = func.call(tc2, undef.into(), &[result]);
                }
            }
        } else {
            let msg = v8::String::new(scope, payload).unwrap();
            let _ = func.call(scope, undef.into(), &[msg.into()]);
        }
    }

    fn run_microtasks(&mut self) {
        const MAX_MICROTASKS_PER_DRAIN: usize = 128;
        let mut iterations = 0;
        loop {
            let mut micro_work_done = false;
            while let Some(task) = MICRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front()) {
                if iterations >= MAX_MICROTASKS_PER_DRAIN {
                    push_console_entry(
                        ConsoleLevel::Warn,
                        "JS microtask drain limit reached; remaining tasks deferred".to_string(),
                    );
                    return;
                }
                task();
                micro_work_done = true;
                iterations += 1;
            }
            let hs = std::pin::pin!(v8::HandleScope::new(&mut self.isolate));
            let hs = &mut hs.init();
            let local_context = v8::Local::new(hs, &self.global_context);
            let scope = &mut v8::ContextScope::new(hs, local_context);
            scope.perform_microtask_checkpoint();
            if !micro_work_done {
                break;
            }
        }
    }

    fn sync_focus(&mut self) -> bool {
        let old = PREVIOUS_FOCUSED_NODE.with(|f| (*f.borrow()).clone());
        let new = FOCUSED_NODE.with(|f| (*f.borrow()).clone());
        if old == new {
            return false;
        }
        PREVIOUS_FOCUSED_NODE.with(|f| *f.borrow_mut() = new.clone());
        if let Some(ref old_id) = old {
            self.trigger_event(old_id, "blur");
            self.trigger_event(old_id, "focusout");
        }
        if let Some(ref new_id) = new {
            self.trigger_event(new_id, "focus");
            self.trigger_event(new_id, "focusin");
        }
        true
    }

    pub fn trigger_event(&mut self, target_id: &str, event_type: &str) {
        let native_id = DOM_ROOT.with(|root| {
            if let Some(ref r) = *root.borrow() {
                find_element_by_id(r, target_id).map(register_node)
            } else {
                None
            }
        });
        if let Some(nid) = native_id {
            let code = format!(
                "document.__trigger_event({}, '{}', {{ bubbles: true }})",
                nid, event_type
            );
            self.execute(&code);
        }
    }

    pub fn trigger_event_on_node_id(&mut self, node_id: u32, event_type: &str) {
        let code = format!(
            "document.__trigger_event({}, '{}', {{ bubbles: true }})",
            node_id, event_type
        );
        self.execute(&code);
    }

    pub fn execute(&mut self, source: &str) {
        let outcome = self.execute_with_result(source);
        if let Some(error) = outcome.error {
            eprintln!("[JS Error] execute: {}", error);
        }
    }

    pub fn execute_with_result(&mut self, source: &str) -> EvalOutcome {
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
                    EvalOutcome {
                        result,
                        error: None,
                    }
                }
            },
        }
    }

    pub fn resolve_module_specifier(&self, specifier: &str, referrer: &Url) -> Result<Url, String> {
        referrer
            .join(specifier)
            .or_else(|_| Url::parse(specifier))
            .map_err(|err| format!("Failed to resolve module specifier '{specifier}': {err}"))
    }

    pub fn compile_module_source(&mut self, url: Url, source: String) -> ModuleCompileOutcome {
        if let Some(record) = self.module_loader.modules.get(&url) {
            return ModuleCompileOutcome {
                url,
                from_cache: true,
                requests: record.requests.clone(),
                error: None,
            };
        }

        match self.compile_v8_module(&url, &source) {
            Ok(requests) => {
                self.module_loader.modules.insert(
                    url.clone(),
                    ModuleRecord {
                        source,
                        requests: requests.clone(),
                    },
                );
                ModuleCompileOutcome {
                    url,
                    from_cache: false,
                    requests,
                    error: None,
                }
            }
            Err(error) => ModuleCompileOutcome {
                url,
                from_cache: false,
                requests: Vec::new(),
                error: Some(error),
            },
        }
    }

    pub fn module_cache_len(&self) -> usize {
        self.module_loader.modules.len()
    }

    pub fn cached_module_urls(&self) -> Vec<Url> {
        self.module_loader.modules.keys().cloned().collect()
    }

    pub fn cached_module_source(&self, url: &Url) -> Option<&str> {
        self.module_loader
            .modules
            .get(url)
            .map(|record| record.source.as_str())
    }

    pub fn cached_module_requests(&self, url: &Url) -> Option<&[String]> {
        self.module_loader
            .modules
            .get(url)
            .map(|record| record.requests.as_slice())
    }

    pub fn instantiate_module_graph(&mut self, root_urls: &[Url]) -> Result<(), Vec<String>> {
        if root_urls.is_empty() {
            return Ok(());
        }

        MODULE_SOURCES.with(|map| {
            let mut map = map.borrow_mut();
            map.clear();
            for (url, record) in &self.module_loader.modules {
                map.insert(url.to_string(), record.source.clone());
            }
        });
        MODULE_ID_TO_URL.with(|map| map.borrow_mut().clear());

        let mut errors = Vec::new();

        let hs = std::pin::pin!(v8::HandleScope::new(&mut self.isolate));
        let hs = &mut hs.init();
        let local_context = v8::Local::new(hs, &self.global_context);
        let scope = &mut v8::ContextScope::new(hs, local_context);

        for root_url in root_urls {
            let source = match self.module_loader.modules.get(root_url) {
                Some(record) => record.source.clone(),
                None => {
                    errors.push(format!("Module not compiled: {root_url}"));
                    continue;
                }
            };

            let tc = std::pin::pin!(v8::TryCatch::new(scope));
            let tc = &mut tc.init();

            let code = v8::String::new(tc, &source)
                .ok_or_else(|| "Failed to create V8 module source string".to_string());
            let code = match code {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("{root_url}: {e}"));
                    continue;
                }
            };

            let resource = v8::String::new(tc, root_url.as_str())
                .ok_or_else(|| "Failed to create V8 module resource name".to_string());
            let resource = match resource {
                Ok(r) => r,
                Err(e) => {
                    errors.push(format!("{root_url}: {e}"));
                    continue;
                }
            };

            let origin = v8::ScriptOrigin::new(
                tc,
                resource.into(),
                0,
                0,
                false,
                0,
                None,
                false,
                false,
                true,
                None,
            );
            let mut source_compiler = v8::script_compiler::Source::new(code, Some(&origin));
            let module = match v8::script_compiler::compile_module(tc, &mut source_compiler) {
                Some(m) => m,
                None => {
                    let err_msg = tc
                        .exception()
                        .and_then(|e| e.to_string(tc))
                        .map(|s| s.to_rust_string_lossy(tc))
                        .unwrap_or_else(|| "Unknown compilation error".to_string());
                    errors.push(format!("{root_url}: {err_msg}"));
                    continue;
                }
            };

            MODULE_ID_TO_URL.with(|map| {
                map.borrow_mut()
                    .insert(module.get_identity_hash(), root_url.to_string());
            });

            RESOLVED_MODULES.with(|map| {
                map.borrow_mut()
                    .insert(root_url.to_string(), v8::Global::new(tc, module));
            });

            let result = module.instantiate_module(tc, resolve_module_callback);
            match result {
                Some(true) => {}
                Some(false) | None => {
                    let err_msg = tc
                        .exception()
                        .and_then(|e| e.to_string(tc))
                        .map(|s| s.to_rust_string_lossy(tc))
                        .unwrap_or_else(|| "Unknown instantiation error".to_string());
                    errors.push(format!("{root_url}: {err_msg}"));
                }
            }
        }

        MODULE_SOURCES.with(|map| map.borrow_mut().clear());
        MODULE_ID_TO_URL.with(|map| map.borrow_mut().clear());
        RESOLVED_MODULES.with(|map| map.borrow_mut().clear());

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn evaluate_module_graph(&mut self, root_urls: &[Url]) -> Result<(), Vec<String>> {
        if root_urls.is_empty() {
            return Ok(());
        }

        MODULE_SOURCES.with(|map| {
            let mut map = map.borrow_mut();
            map.clear();
            for (url, record) in &self.module_loader.modules {
                map.insert(url.to_string(), record.source.clone());
            }
        });
        MODULE_ID_TO_URL.with(|map| map.borrow_mut().clear());

        let mut errors = Vec::new();

        let hs = std::pin::pin!(v8::HandleScope::new(&mut self.isolate));
        let hs = &mut hs.init();
        let local_context = v8::Local::new(hs, &self.global_context);
        let scope = &mut v8::ContextScope::new(hs, local_context);

        for root_url in root_urls {
            let source = match self.module_loader.modules.get(root_url) {
                Some(record) => record.source.clone(),
                None => {
                    errors.push(format!("Module not compiled: {root_url}"));
                    continue;
                }
            };

            let tc = std::pin::pin!(v8::TryCatch::new(scope));
            let tc = &mut tc.init();

            let code = v8::String::new(tc, &source)
                .ok_or_else(|| "Failed to create V8 module source string".to_string());
            let code = match code {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("{root_url}: {e}"));
                    continue;
                }
            };

            let resource = v8::String::new(tc, root_url.as_str())
                .ok_or_else(|| "Failed to create V8 module resource name".to_string());
            let resource = match resource {
                Ok(r) => r,
                Err(e) => {
                    errors.push(format!("{root_url}: {e}"));
                    continue;
                }
            };

            let origin = v8::ScriptOrigin::new(
                tc,
                resource.into(),
                0,
                0,
                false,
                0,
                None,
                false,
                false,
                true,
                None,
            );
            let mut source_compiler = v8::script_compiler::Source::new(code, Some(&origin));
            let module = match v8::script_compiler::compile_module(tc, &mut source_compiler) {
                Some(m) => m,
                None => {
                    let err_msg = tc
                        .exception()
                        .and_then(|e| e.to_string(tc))
                        .map(|s| s.to_rust_string_lossy(tc))
                        .unwrap_or_else(|| "Unknown compilation error".to_string());
                    errors.push(format!("{root_url}: {err_msg}"));
                    continue;
                }
            };

            MODULE_ID_TO_URL.with(|map| {
                map.borrow_mut()
                    .insert(module.get_identity_hash(), root_url.to_string());
            });

            // Cache root module so cyclic imports return this instance
            RESOLVED_MODULES.with(|map| {
                map.borrow_mut()
                    .insert(root_url.to_string(), v8::Global::new(tc, module));
            });

            // Instantiate before evaluate — required by V8
            let instantiate_ok = module
                .instantiate_module(tc, resolve_module_callback)
                .unwrap_or(false);
            if !instantiate_ok {
                let err_msg = tc
                    .exception()
                    .and_then(|e| e.to_string(tc))
                    .map(|s| s.to_rust_string_lossy(tc))
                    .unwrap_or_else(|| "Unknown instantiation error".to_string());
                errors.push(format!("{root_url}: {err_msg}"));
                continue;
            }

            let evaluate_result = module.evaluate(tc);
            if evaluate_result.is_none() || module.get_status() == v8::ModuleStatus::Errored {
                let err_msg = tc
                    .exception()
                    .and_then(|e| e.to_string(tc))
                    .map(|s| s.to_rust_string_lossy(tc))
                    .or_else(|| {
                        if module.get_status() == v8::ModuleStatus::Errored {
                            module
                                .get_exception()
                                .to_string(tc)
                                .map(|s| s.to_rust_string_lossy(tc))
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "Unknown evaluation error".to_string());
                errors.push(format!("{root_url}: {err_msg}"));
            }
        }

        MODULE_SOURCES.with(|map| map.borrow_mut().clear());
        MODULE_ID_TO_URL.with(|map| map.borrow_mut().clear());
        RESOLVED_MODULES.with(|map| map.borrow_mut().clear());

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn compile_v8_module(&mut self, url: &Url, source: &str) -> Result<Vec<String>, String> {
        let hs = std::pin::pin!(v8::HandleScope::new(&mut self.isolate));
        let hs = &mut hs.init();
        let local_context = v8::Local::new(hs, &self.global_context);
        let scope = &mut v8::ContextScope::new(hs, local_context);
        let tc = std::pin::pin!(v8::TryCatch::new(scope));
        let tc = &mut tc.init();

        let code = v8::String::new(tc, source)
            .ok_or_else(|| "Failed to create V8 module source string".to_string())?;
        let resource = v8::String::new(tc, url.as_str())
            .ok_or_else(|| "Failed to create V8 module resource name".to_string())?;
        let origin = v8::ScriptOrigin::new(
            tc,
            resource.into(),
            0,
            0,
            false,
            0,
            None,
            false,
            false,
            true,
            None,
        );
        let mut source_compiler = v8::script_compiler::Source::new(code, Some(&origin));
        let module =
            v8::script_compiler::compile_module(tc, &mut source_compiler).ok_or_else(|| {
                tc.exception()
                    .and_then(|e| e.to_string(tc))
                    .map(|s| s.to_rust_string_lossy(tc))
                    .unwrap_or_else(|| "Unknown module compilation error".to_string())
            })?;

        let requests = module.get_module_requests();
        let mut specifiers = Vec::new();
        for i in 0..requests.length() {
            if let Some(data) = requests.get(tc, i) {
                if let Ok(request) = data.try_cast::<v8::ModuleRequest>() {
                    let specifier = request.get_specifier();
                    specifiers.push(specifier.to_rust_string_lossy(tc));
                }
            }
        }
        Ok(specifiers)
    }

    pub fn get_style_overrides(&mut self) -> HashMap<String, HashMap<String, String>> {
        let mut result = HashMap::new();
        let hs = std::pin::pin!(v8::HandleScope::new(&mut self.isolate));
        let hs = &mut hs.init();
        let local_context = v8::Local::new(hs, &self.global_context);
        let scope = &mut v8::ContextScope::new(hs, local_context);
        {
            let tc = std::pin::pin!(v8::TryCatch::new(scope));
            let tc = &mut tc.init();
            let src = v8::String::new(tc, "__aura_style_log.join('####')").unwrap();
            if let Some(script) = v8::Script::compile(tc, src, None) {
                if let Some(val) = script.run(tc) {
                    if let Some(s) = val.to_string(tc) {
                        let s_std = s.to_rust_string_lossy(tc);
                        for entry in s_std.split("####") {
                            let parts: Vec<&str> = entry.splitn(3, "||||").collect();
                            if parts.len() == 3 && !parts[0].is_empty() {
                                result
                                    .entry(parts[0].to_string())
                                    .or_insert_with(HashMap::new)
                                    .insert(parts[1].to_string(), parts[2].to_string());
                            }
                        }
                    }
                }
            }
        }
        {
            let tc2 = std::pin::pin!(v8::TryCatch::new(scope));
            let tc2 = &mut tc2.init();
            let clear = v8::String::new(tc2, "__aura_style_log = [];").unwrap();
            if let Some(script) = v8::Script::compile(tc2, clear, None) {
                let _ = script.run(tc2);
            }
        }
        result
    }

    pub fn get_focused_node_id(&self) -> Option<String> {
        FOCUSED_NODE.with(|f| (*f.borrow()).clone())
    }

    pub fn set_focused_node_id(&mut self, id: Option<String>) {
        FOCUSED_NODE.with(|f| *f.borrow_mut() = id);
    }

    pub fn take_form_submit_requests(&mut self) -> Vec<u32> {
        FORM_SUBMIT_REQUESTS.with(|r| r.borrow_mut().drain(..).collect())
    }
}

fn register_native_functions(
    scope: &mut v8::ContextScope<v8::HandleScope>,
    global: v8::Local<v8::Object>,
) {
    let f = v8::Function::new(scope, console_log).unwrap();
    global.set(
        scope,
        v8::String::new(scope, "log").unwrap().into(),
        f.into(),
    );
    let f = v8::Function::new(scope, console_warn).unwrap();
    global.set(
        scope,
        v8::String::new(scope, "warn").unwrap().into(),
        f.into(),
    );
    let f = v8::Function::new(scope, console_error).unwrap();
    global.set(
        scope,
        v8::String::new(scope, "error").unwrap().into(),
        f.into(),
    );
    let f = v8::Function::new(scope, console_info).unwrap();
    global.set(
        scope,
        v8::String::new(scope, "info").unwrap().into(),
        f.into(),
    );
    let f = v8::Function::new(scope, console_debug).unwrap();
    global.set(
        scope,
        v8::String::new(scope, "debug").unwrap().into(),
        f.into(),
    );

    register_fn(
        scope,
        global,
        "__aura_get_document_element",
        get_document_element_cb,
    );
    register_fn(scope, global, "__aura_get_head", get_head_cb);
    register_fn(scope, global, "__aura_get_body", get_body_cb);
    register_fn(
        scope,
        global,
        "__aura_get_element_by_id",
        get_element_by_id_cb,
    );
    register_fn(scope, global, "__aura_query_selector", query_selector_cb);
    register_fn(
        scope,
        global,
        "__aura_query_selector_all",
        query_selector_all_cb,
    );
    register_fn(
        scope,
        global,
        "__aura_get_elements_by_class",
        get_elements_by_class_cb,
    );
    register_fn(
        scope,
        global,
        "__aura_get_elements_by_tag",
        get_elements_by_tag_cb,
    );
    register_fn(scope, global, "__aura_get_parent_id", get_parent_id_cb);
    register_fn(
        scope,
        global,
        "__aura_get_next_sibling_id",
        get_next_sibling_cb,
    );
    register_fn(
        scope,
        global,
        "__aura_get_previous_sibling_id",
        get_previous_sibling_cb,
    );
    register_fn(
        scope,
        global,
        "__aura_get_next_element_sibling_id",
        get_next_element_sibling_cb,
    );
    register_fn(
        scope,
        global,
        "__aura_get_previous_element_sibling_id",
        get_previous_element_sibling_cb,
    );
    register_fn(scope, global, "__aura_get_inner_html", get_inner_html_cb);
    register_fn(scope, global, "__aura_get_outer_html", get_outer_html_cb);
    register_fn(scope, global, "__aura_set_inner_html", set_inner_html_cb);
    register_fn(
        scope,
        global,
        "__aura_get_text_content",
        get_text_content_cb,
    );
    register_fn(
        scope,
        global,
        "__aura_set_text_content",
        set_text_content_cb,
    );
    register_fn(scope, global, "__aura_get_attribute", get_attribute_cb);
    register_fn(scope, global, "__aura_set_attribute", set_attribute_cb);
    register_fn(scope, global, "__aura_get_attributes", get_attributes_cb);
    register_fn(scope, global, "__aura_has_attribute", has_attribute_cb);
    register_fn(
        scope,
        global,
        "__aura_remove_attribute",
        remove_attribute_cb,
    );
    register_fn(scope, global, "__aura_create_element", create_element_cb);
    register_fn(
        scope,
        global,
        "__aura_create_text_node",
        create_text_node_cb,
    );
    register_fn(scope, global, "__aura_create_comment", create_comment_cb);
    register_fn(
        scope,
        global,
        "__aura_create_document_fragment",
        create_document_fragment_cb,
    );
    register_fn(scope, global, "__aura_append_child", append_child_cb);
    register_fn(scope, global, "__aura_remove_child", remove_child_cb);
    register_fn(scope, global, "__aura_insert_before", insert_before_cb);
    register_fn(scope, global, "__aura_remove_self", remove_self_cb);
    register_fn(scope, global, "__aura_get_children", get_children_cb);
    register_fn(scope, global, "__aura_get_node_info", get_node_info_cb);
    register_fn(scope, global, "__aura_get_node_type", get_node_type_cb);
    register_fn(scope, global, "__aura_get_node_value", get_node_value_cb);
    register_fn(scope, global, "__aura_set_node_value", set_node_value_cb);
    register_fn(
        scope,
        global,
        "__aura_get_layout_metrics",
        get_layout_metrics_cb,
    );
    register_fn(scope, global, "__aura_get_doctype_id", get_doctype_id_cb);
    register_fn(
        scope,
        global,
        "__aura_get_doctype_info",
        get_doctype_info_cb,
    );
    register_fn(scope, global, "__aura_set_focus", set_focus_cb);
    register_fn(scope, global, "__aura_queue_task", queue_task_cb);
    register_fn(scope, global, "__aura_resolve_url", resolve_url_cb);
    register_fn(
        scope,
        global,
        "__aura_can_execute_script_url",
        can_execute_script_url_cb,
    );
    register_fn(scope, global, "__aura_parse_url", parse_url_cb);
    register_fn(scope, global, "__aura_storage_get", storage_get_cb);
    register_fn(scope, global, "__aura_storage_set", storage_set_cb);
    register_fn(scope, global, "__aura_storage_remove", storage_remove_cb);
    register_fn(scope, global, "__aura_storage_clear", storage_clear_cb);
    register_fn(scope, global, "__aura_fetch", fetch_cb);
    register_fn(scope, global, "setTimeout", setTimeout_cb);
    register_fn(scope, global, "requestAnimationFrame", raf_cb);
    register_fn(scope, global, "requestIdleCallback", ric_cb);
    register_fn(scope, global, "cancelIdleCallback", cic_cb);
    register_fn(scope, global, "__aura_submit_form", submit_form_cb);
}

fn console_log(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    console_impl(scope, args, ConsoleLevel::Log);
}
fn console_warn(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    console_impl(scope, args, ConsoleLevel::Warn);
}
fn console_error(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    console_impl(scope, args, ConsoleLevel::Error);
}
fn console_info(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    console_impl(scope, args, ConsoleLevel::Info);
}
fn console_debug(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    console_impl(scope, args, ConsoleLevel::Debug);
}

fn register_fn(
    scope: &mut v8::ContextScope<v8::HandleScope>,
    global: v8::Local<v8::Object>,
    name: &str,
    cb: impl v8::MapFnTo<v8::FunctionCallback>,
) {
    let f = v8::Function::new(scope, cb).unwrap();
    let key = v8::String::new(scope, name).unwrap();
    global.set(scope, key.into(), f.into());
}

fn get_element_by_id_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let id = args.get(0).to_rust_string_lossy(scope);
    let res = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            find_element_by_id(r, &id).map(|h| {
                let tag = if let NodeData::Element { ref name, .. } = h.data {
                    name.local.to_string()
                } else {
                    String::new()
                };
                (register_node(h), tag)
            })
        } else {
            None
        }
    });
    if let Some((nid, tag)) = res {
        let obj = v8::Object::new(scope);
        obj.set(
            scope,
            v8::String::new(scope, "nid").unwrap().into(),
            v8::Number::new(scope, nid as f64).into(),
        );
        obj.set(
            scope,
            v8::String::new(scope, "tag").unwrap().into(),
            v8::String::new(scope, &tag).unwrap().into(),
        );
        obj.set(
            scope,
            v8::String::new(scope, "kind").unwrap().into(),
            v8::String::new(scope, "element").unwrap().into(),
        );
        rv.set(obj.into());
    } else {
        rv.set_null();
    }
}

fn get_document_element_cb(
    scope: &mut v8::PinScope,
    _a: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = DOM_ROOT.with(|root| {
        root.borrow()
            .as_ref()
            .and_then(find_document_element)
            .map(register_node)
    });
    if let Some(n) = nid {
        rv.set_uint32(n);
    } else {
        rv.set_null();
    }
}
fn get_head_cb(
    scope: &mut v8::PinScope,
    _a: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = DOM_ROOT.with(|root| {
        root.borrow()
            .as_ref()
            .and_then(|d| find_document_surface_element(d, "head"))
            .map(register_node)
    });
    if let Some(n) = nid {
        rv.set_uint32(n);
    } else {
        rv.set_null();
    }
}
fn get_body_cb(
    scope: &mut v8::PinScope,
    _a: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = DOM_ROOT.with(|root| {
        root.borrow()
            .as_ref()
            .and_then(|d| find_document_surface_element(d, "body"))
            .map(register_node)
    });
    if let Some(n) = nid {
        rv.set_uint32(n);
    } else {
        rv.set_null();
    }
}

fn query_selector_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let root_nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let selector = args.get(1).to_rust_string_lossy(scope);
    let found = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            let search = if root_nid == 0 {
                r.clone()
            } else {
                NODE_REGISTRY
                    .with(|reg| reg.borrow().get(&root_nid).cloned())
                    .unwrap_or_else(|| r.clone())
            };
            query_selector_first(&search, &selector, root_nid != 0)
        } else {
            None
        }
    });
    if let Some(h) = found {
        rv.set_uint32(register_node(h));
    } else {
        rv.set_null();
    }
}

fn query_selector_all_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let root_nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let selector = args.get(1).to_rust_string_lossy(scope);
    let nids = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            let search = if root_nid == 0 {
                r.clone()
            } else {
                NODE_REGISTRY
                    .with(|reg| reg.borrow().get(&root_nid).cloned())
                    .unwrap_or_else(|| r.clone())
            };
            query_selector_all_nodes(&search, &selector, root_nid != 0)
        } else {
            vec![]
        }
    });
    let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
    let json = format!(
        "[{}]",
        ids.iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    rv.set(v8::String::new(scope, &json).unwrap().into());
}

fn get_elements_by_class_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let root_nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let cls = args.get(1).to_rust_string_lossy(scope);
    let nids = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            let search = if root_nid == 0 {
                r.clone()
            } else {
                NODE_REGISTRY
                    .with(|reg| reg.borrow().get(&root_nid).cloned())
                    .unwrap_or_else(|| r.clone())
            };
            find_elements_by_class(&search, &cls, root_nid != 0)
        } else {
            vec![]
        }
    });
    let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
    let json = format!(
        "[{}]",
        ids.iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    rv.set(v8::String::new(scope, &json).unwrap().into());
}

fn get_elements_by_tag_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let root_nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let tag = args.get(1).to_rust_string_lossy(scope).to_lowercase();
    let nids = DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            let search = if root_nid == 0 {
                r.clone()
            } else {
                NODE_REGISTRY
                    .with(|reg| reg.borrow().get(&root_nid).cloned())
                    .unwrap_or_else(|| r.clone())
            };
            find_elements_by_tag_name(&search, &tag, root_nid != 0)
        } else {
            vec![]
        }
    });
    let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
    let json = format!(
        "[{}]",
        ids.iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    rv.set(v8::String::new(scope, &json).unwrap().into());
}

fn get_parent_id_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let parent = NODE_REGISTRY.with(|reg| {
        reg.borrow().get(&nid).and_then(|node| {
            let pw = node.parent.take();
            let p = pw.and_then(|w| w.upgrade());
            if let Some(ref ph) = p {
                node.parent.set(Some(Rc::downgrade(ph)));
            }
            p
        })
    });
    if let Some(pn) = parent.map(register_node) {
        rv.set_uint32(pn);
    } else {
        rv.set_null();
    }
}
fn sib_res(
    scope: &mut v8::PinScope,
    rv: &mut v8::ReturnValue<v8::Value>,
    nid: u32,
    dir: SiblingDirection,
    el_only: bool,
) {
    match get_sibling_id(nid, dir, el_only) {
        Some(sn) => rv.set_uint32(sn),
        None => rv.set_null(),
    }
}
fn get_next_sibling_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    sib_res(
        scope,
        &mut rv,
        args.get(0).uint32_value(scope).unwrap_or(0),
        SiblingDirection::Next,
        false,
    );
}
fn get_previous_sibling_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    sib_res(
        scope,
        &mut rv,
        args.get(0).uint32_value(scope).unwrap_or(0),
        SiblingDirection::Previous,
        false,
    );
}
fn get_next_element_sibling_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    sib_res(
        scope,
        &mut rv,
        args.get(0).uint32_value(scope).unwrap_or(0),
        SiblingDirection::Next,
        true,
    );
}
fn get_previous_element_sibling_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    sib_res(
        scope,
        &mut rv,
        args.get(0).uint32_value(scope).unwrap_or(0),
        SiblingDirection::Previous,
        true,
    );
}

fn get_inner_html_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let html = NODE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&nid)
            .map(|n| serialize_inner_html(n))
            .unwrap_or_default()
    });
    rv.set(v8::String::new(scope, &html).unwrap().into());
}
fn get_outer_html_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let html = NODE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&nid)
            .map(|n| {
                let mut o = String::new();
                serialize_node(n, &mut o);
                o
            })
            .unwrap_or_default()
    });
    rv.set(v8::String::new(scope, &html).unwrap().into());
}
fn set_inner_html_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let html = args.get(1).to_rust_string_lossy(scope);
    NODE_REGISTRY.with(|reg| {
        if let Some(node) = reg.borrow().get(&nid) {
            let tag = if let NodeData::Element { ref name, .. } = node.data {
                name.local.to_string()
            } else {
                "div".to_string()
            };
            let frag = parse_html_fragment(&html, &tag);
            let mut ch = node.children.borrow_mut();
            for c in ch.iter() {
                c.parent.set(None);
            }
            ch.clear();
            for c in frag {
                c.parent.set(Some(Rc::downgrade(node)));
                ch.push(c);
            }
        }
    });
}
fn get_text_content_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let t = NODE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&nid)
            .map(|n| match &n.data {
                NodeData::Comment { contents } => contents.to_string(),
                _ => collect_text_content(n),
            })
            .unwrap_or_default()
    });
    rv.set(v8::String::new(scope, &t).unwrap().into());
}
fn set_text_content_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let text = args.get(1).to_rust_string_lossy(scope);
    let node = NODE_REGISTRY.with(|reg| reg.borrow().get(&nid).cloned());
    if let Some(node) = node {
        match &node.data {
            NodeData::Text { contents } => {
                *contents.borrow_mut() = text.as_str().into();
            }
            NodeData::Comment { .. } => {
                use html5ever::tendril::StrTendril;
                use markup5ever_rcdom::Node;
                replace_registered_node(
                    nid,
                    Node::new(NodeData::Comment {
                        contents: StrTendril::from(text.as_str()),
                    }),
                );
            }
            _ => {
                use html5ever::tendril::StrTendril;
                use markup5ever_rcdom::Node;
                let tn = Node::new(NodeData::Text {
                    contents: std::cell::RefCell::new(StrTendril::from(text.as_str())),
                });
                tn.parent.set(Some(Rc::downgrade(&node)));
                node.children.borrow_mut().clear();
                node.children.borrow_mut().push(tn);
            }
        }
    }
}

fn get_attribute_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let name = args.get(1).to_rust_string_lossy(scope);
    let val = NODE_REGISTRY.with(|reg| {
        reg.borrow().get(&nid).and_then(|n| {
            if let NodeData::Element { ref attrs, .. } = n.data {
                attrs
                    .borrow()
                    .iter()
                    .find(|a| a.name.local.to_string() == name)
                    .map(|a| a.value.to_string())
            } else {
                None
            }
        })
    });
    if let Some(v) = val {
        rv.set(v8::String::new(scope, &v).unwrap().into());
    } else {
        rv.set_null();
    }
}
fn set_attribute_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let name = args.get(1).to_rust_string_lossy(scope);
    let val = args.get(2).to_rust_string_lossy(scope);
    NODE_REGISTRY.with(|reg| {
        if let Some(n) = reg.borrow().get(&nid) {
            if let NodeData::Element { ref attrs, .. } = n.data {
                let mut a = attrs.borrow_mut();
                let mut f = false;
                for attr in a.iter_mut() {
                    if attr.name.local.to_string() == name {
                        attr.value = val.clone().into();
                        f = true;
                        break;
                    }
                }
                if !f {
                    use html5ever::{ns, Attribute, LocalName, QualName};
                    a.push(Attribute {
                        name: QualName::new(None, ns!(html), LocalName::from(name)),
                        value: val.into(),
                    });
                }
            }
        }
    });
}
fn get_attributes_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let json = NODE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&nid)
            .map(|n| {
                if let NodeData::Element { ref attrs, .. } = n.data {
                    let items: Vec<String> = attrs
                        .borrow()
                        .iter()
                        .map(|a| {
                            format!(
                                "{{\"name\":{},\"value\":{}}}",
                                serde_json::to_string(&a.name.local.to_string())
                                    .unwrap_or_default(),
                                serde_json::to_string(&a.value.to_string()).unwrap_or_default()
                            )
                        })
                        .collect();
                    format!("[{}]", items.join(","))
                } else {
                    "[]".to_string()
                }
            })
            .unwrap_or_else(|| "[]".to_string())
    });
    rv.set(v8::String::new(scope, &json).unwrap().into());
}
fn has_attribute_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let name = args.get(1).to_rust_string_lossy(scope);
    let f = NODE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&nid)
            .map(|n| {
                if let NodeData::Element { ref attrs, .. } = n.data {
                    attrs
                        .borrow()
                        .iter()
                        .any(|a| a.name.local.to_string() == name)
                } else {
                    false
                }
            })
            .unwrap_or(false)
    });
    rv.set_bool(f);
}
fn remove_attribute_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let name = args.get(1).to_rust_string_lossy(scope);
    NODE_REGISTRY.with(|reg| {
        if let Some(n) = reg.borrow().get(&nid) {
            if let NodeData::Element { ref attrs, .. } = n.data {
                attrs
                    .borrow_mut()
                    .retain(|a| a.name.local.to_string() != name);
            }
        }
    });
}

fn create_element_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let tag = args.get(0).to_rust_string_lossy(scope);
    let tag = if tag.is_empty() {
        "div".to_string()
    } else {
        tag.to_lowercase()
    };
    use html5ever::{ns, LocalName, QualName};
    use markup5ever_rcdom::Node;
    let n = Node::new(NodeData::Element {
        name: QualName::new(None, ns!(html), LocalName::from(tag)),
        attrs: std::cell::RefCell::new(vec![]),
        template_contents: std::cell::RefCell::new(None),
        mathml_annotation_xml_integration_point: false,
    });
    rv.set_uint32(register_node(n));
}
fn create_text_node_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    use html5ever::tendril::StrTendril;
    use markup5ever_rcdom::Node;
    rv.set_uint32(register_node(Node::new(NodeData::Text {
        contents: std::cell::RefCell::new(StrTendril::from(
            args.get(0).to_rust_string_lossy(scope).as_str(),
        )),
    })));
}
fn create_comment_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    use html5ever::tendril::StrTendril;
    use markup5ever_rcdom::Node;
    rv.set_uint32(register_node(Node::new(NodeData::Comment {
        contents: StrTendril::from(args.get(0).to_rust_string_lossy(scope).as_str()),
    })));
}
fn create_document_fragment_cb(
    scope: &mut v8::PinScope,
    _a: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    use markup5ever_rcdom::Node;
    let n = Node::new(NodeData::Document);
    let nid = register_node(n);
    mark_document_fragment_id(nid);
    rv.set_uint32(nid);
}

fn append_child_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let pn = args.get(0).uint32_value(scope).unwrap_or(0);
    let cn = args.get(1).uint32_value(scope).unwrap_or(0);
    NODE_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        if let (Some(p), Some(c)) = (reg.get(&pn), reg.get(&cn)) {
            if is_document_fragment_id(cn) {
                append_fragment_children(p, c);
            } else {
                detach_node_from_parent(c);
                c.parent.set(Some(Rc::downgrade(p)));
                p.children.borrow_mut().push(c.clone());
            }
        }
    });
}
fn remove_child_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let pn = args.get(0).uint32_value(scope).unwrap_or(0);
    let cn = args.get(1).uint32_value(scope).unwrap_or(0);
    NODE_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        if let (Some(p), Some(c)) = (reg.get(&pn), reg.get(&cn)) {
            let cp = Rc::as_ptr(c) as usize;
            p.children
                .borrow_mut()
                .retain(|x| Rc::as_ptr(x) as usize != cp);
            c.parent.set(None);
        }
    });
}
fn insert_before_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let pn = args.get(0).uint32_value(scope).unwrap_or(0);
    let nn = args.get(1).uint32_value(scope).unwrap_or(0);
    let rn: Option<u32> = if args.get(2).is_null() {
        None
    } else {
        Some(args.get(2).uint32_value(scope).unwrap_or(0))
    };
    NODE_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        if let (Some(p), Some(nc)) = (reg.get(&pn), reg.get(&nn)) {
            let pos = rn.and_then(|rv| {
                reg.get(&rv).and_then(|refn| {
                    let rp = Rc::as_ptr(refn) as usize;
                    p.children
                        .borrow()
                        .iter()
                        .position(|c| Rc::as_ptr(c) as usize == rp)
                })
            });
            if is_document_fragment_id(nn) {
                insert_fragment_children(p, nc, pos);
            } else {
                detach_node_from_parent(nc);
                nc.parent.set(Some(Rc::downgrade(p)));
                let mut ch = p.children.borrow_mut();
                if let Some(pos) = pos {
                    ch.insert(pos, nc.clone());
                } else {
                    ch.push(nc.clone());
                }
            }
        }
    });
}
fn remove_self_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    NODE_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        if let Some(n) = reg.get(&nid) {
            let np = Rc::as_ptr(n) as usize;
            if let Some(pw) = n.parent.take() {
                if let Some(p) = pw.upgrade() {
                    p.children
                        .borrow_mut()
                        .retain(|c| Rc::as_ptr(c) as usize != np);
                }
            }
        }
    });
}

fn get_children_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let ch = NODE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&nid)
            .map(|n| n.children.borrow().iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let mut items = vec![];
    for c in ch {
        let cn = register_node(c.clone());
        match &c.data {
            NodeData::Element {
                ref name,
                ref attrs,
                ..
            } => {
                let tag = name.local.to_string();
                let id = attrs
                    .borrow()
                    .iter()
                    .find(|a| a.name.local.to_string() == "id")
                    .map(|a| a.value.to_string())
                    .unwrap_or_default();
                items.push(format!(
                    "{{\"nid\":{},\"tag\":\"{}\",\"id\":\"{}\",\"kind\":\"element\"}}",
                    cn, tag, id
                ));
            }
            NodeData::Text { .. } => items.push(format!(
                "{{\"nid\":{},\"tag\":\"\",\"id\":\"\",\"kind\":\"text\"}}",
                cn
            )),
            NodeData::Comment { .. } => items.push(format!(
                "{{\"nid\":{},\"tag\":\"\",\"id\":\"\",\"kind\":\"comment\"}}",
                cn
            )),
            NodeData::Doctype { ref name, .. } => items.push(format!(
                "{{\"nid\":{},\"tag\":\"{}\",\"id\":\"\",\"kind\":\"doctype\"}}",
                cn, name
            )),
            NodeData::Document => items.push(format!(
                "{{\"nid\":{},\"tag\":\"\",\"id\":\"\",\"kind\":\"fragment\"}}",
                cn
            )),
            _ => {}
        }
    }
    rv.set(
        v8::String::new(scope, &format!("[{}]", items.join(",")))
            .unwrap()
            .into(),
    );
}
fn get_node_info_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let info = NODE_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        if let Some(n) = reg.get(&nid) {
            if is_document_fragment_id(nid) {
                return Some((
                    String::new(),
                    String::new(),
                    String::new(),
                    "fragment".to_string(),
                ));
            }
            if let NodeData::Element {
                ref name,
                ref attrs,
                ..
            } = n.data
            {
                let tag = name.local.to_string();
                let ab = attrs.borrow();
                let id = ab
                    .iter()
                    .find(|a| a.name.local.to_string() == "id")
                    .map(|a| a.value.to_string())
                    .unwrap_or_default();
                let cls = ab
                    .iter()
                    .find(|a| a.name.local.to_string() == "class")
                    .map(|a| a.value.to_string())
                    .unwrap_or_default();
                return Some((tag, id, cls, "element".to_string()));
            } else if let NodeData::Text { .. } = n.data {
                return Some((
                    String::new(),
                    String::new(),
                    String::new(),
                    "text".to_string(),
                ));
            } else if let NodeData::Comment { .. } = n.data {
                return Some((
                    String::new(),
                    String::new(),
                    String::new(),
                    "comment".to_string(),
                ));
            } else if let NodeData::Doctype { ref name, .. } = n.data {
                return Some((
                    name.to_string(),
                    String::new(),
                    String::new(),
                    "doctype".to_string(),
                ));
            }
        }
        None
    });
    if let Some((tag, id, cls, kind)) = info {
        let o = v8::Object::new(scope);
        o.set(
            scope,
            v8::String::new(scope, "tag").unwrap().into(),
            v8::String::new(scope, &tag).unwrap().into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "id").unwrap().into(),
            v8::String::new(scope, &id).unwrap().into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "class").unwrap().into(),
            v8::String::new(scope, &cls).unwrap().into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "kind").unwrap().into(),
            v8::String::new(scope, &kind).unwrap().into(),
        );
        rv.set(o.into());
    } else {
        rv.set_null();
    }
}
fn get_node_type_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let nt = NODE_REGISTRY.with(|reg| {
        if is_document_fragment_id(nid) {
            11
        } else {
            reg.borrow()
                .get(&nid)
                .map(|n| match n.data {
                    NodeData::Element { .. } => 1,
                    NodeData::Text { .. } => 3,
                    NodeData::Comment { .. } => 8,
                    NodeData::Doctype { .. } => 10,
                    NodeData::Document => 9,
                    _ => 0,
                })
                .unwrap_or(0)
        }
    });
    rv.set_int32(nt);
}
fn get_node_value_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let v = NODE_REGISTRY.with(|reg| {
        reg.borrow().get(&nid).and_then(|n| match &n.data {
            NodeData::Text { contents } => Some(contents.borrow().to_string()),
            NodeData::Comment { contents } => Some(contents.to_string()),
            _ => None,
        })
    });
    if let Some(s) = v {
        rv.set(v8::String::new(scope, &s).unwrap().into());
    } else {
        rv.set_null();
    }
}
fn set_node_value_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    set_text_content_cb(scope, args, _rv);
}
fn get_layout_metrics_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let m = NODE_REGISTRY.with(|reg| {
        reg.borrow().get(&nid).and_then(|n| {
            let k = node_path_key(n);
            LAYOUT_METRICS.with(|lm| lm.borrow().get(&k).cloned())
        })
    });
    if let Some(metrics) = m {
        let o = v8::Object::new(scope);
        o.set(
            scope,
            v8::String::new(scope, "x").unwrap().into(),
            v8::Number::new(scope, metrics.x as f64).into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "y").unwrap().into(),
            v8::Number::new(scope, metrics.y as f64).into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "width").unwrap().into(),
            v8::Number::new(scope, metrics.width as f64).into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "height").unwrap().into(),
            v8::Number::new(scope, metrics.height as f64).into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "top").unwrap().into(),
            v8::Number::new(scope, metrics.y as f64).into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "left").unwrap().into(),
            v8::Number::new(scope, metrics.x as f64).into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "right").unwrap().into(),
            v8::Number::new(scope, metrics.right() as f64).into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "bottom").unwrap().into(),
            v8::Number::new(scope, metrics.bottom() as f64).into(),
        );
        rv.set(o.into());
    } else {
        rv.set_null();
    }
}
fn get_doctype_id_cb(
    scope: &mut v8::PinScope,
    _a: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = DOM_ROOT.with(|r| {
        r.borrow()
            .as_ref()
            .and_then(find_document_doctype)
            .map(register_node)
    });
    if let Some(n) = nid {
        rv.set_uint32(n);
    } else {
        rv.set_null();
    }
}
fn get_doctype_info_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    let info = NODE_REGISTRY.with(|reg| {
        reg.borrow().get(&nid).and_then(|n| {
            if let NodeData::Doctype {
                ref name,
                ref public_id,
                ref system_id,
            } = n.data
            {
                Some((
                    name.to_string(),
                    public_id.to_string(),
                    system_id.to_string(),
                ))
            } else {
                None
            }
        })
    });
    if let Some((name, pid, sid)) = info {
        let o = v8::Object::new(scope);
        o.set(
            scope,
            v8::String::new(scope, "name").unwrap().into(),
            v8::String::new(scope, &name).unwrap().into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "publicId").unwrap().into(),
            v8::String::new(scope, &pid).unwrap().into(),
        );
        o.set(
            scope,
            v8::String::new(scope, "systemId").unwrap().into(),
            v8::String::new(scope, &sid).unwrap().into(),
        );
        rv.set(o.into());
    } else {
        rv.set_null();
    }
}
fn set_focus_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let id = args.get(0).to_rust_string_lossy(scope);
    if id.is_empty() {
        FOCUSED_NODE.with(|f| *f.borrow_mut() = None);
    } else {
        FOCUSED_NODE.with(|f| *f.borrow_mut() = Some(id));
    }
}
fn submit_form_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let nid = args.get(0).uint32_value(scope).unwrap_or(0);
    if nid > 0 {
        FORM_SUBMIT_REQUESTS.with(|r| r.borrow_mut().push(nid));
    }
}
fn queue_task_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let cb = args.get(0);
    if cb.is_function() {
        let cb_func: v8::Local<v8::Function> = unsafe { std::mem::transmute(cb) };
        let cb_global = v8::Global::new(scope, cb_func);
        MACRO_TASKS.with(|tasks| {
            tasks.borrow_mut().push_back(Box::new(move || {
                RUN_PENDING.with(|p| p.borrow_mut().push((cb_global.clone(), None)));
            }))
        });
    }
}
fn resolve_url_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let input = args.get(0).to_rust_string_lossy(scope);
    let base_arg = args.get(1);
    let base = if base_arg.is_null() || base_arg.is_undefined() {
        String::new()
    } else {
        base_arg.to_rust_string_lossy(scope)
    };
    let resolved = if !base.is_empty() {
        Url::parse(&base)
            .ok()
            .and_then(|b| b.join(&input).ok())
            .or_else(|| Url::parse(&input).ok())
    } else {
        Url::parse(&input).ok()
    };
    rv.set(
        v8::String::new(scope, &resolved.map(|u| u.to_string()).unwrap_or(input))
            .unwrap()
            .into(),
    );
}
fn can_execute_script_url_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let url_str = args.get(0).to_rust_string_lossy(scope);
    let base_url = CURRENT_ORIGIN.with(|o| (*o.borrow()).clone());
    let target = if let Some(ref b) = base_url {
        b.join(&url_str)
            .unwrap_or_else(|_| Url::parse(&url_str).unwrap_or(b.clone()))
    } else {
        Url::parse(&url_str).unwrap_or_else(|_| Url::parse("about:blank").unwrap())
    };
    let allowed = CSP_POLICY.with(|p| {
        p.borrow()
            .as_ref()
            .map(|pol| pol.is_allowed("script-src", &target, base_url.as_ref()))
            .unwrap_or(true)
    });
    rv.set_bool(allowed);
}
fn parse_url_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let input = args.get(0).to_rust_string_lossy(scope);
    let base_arg = args.get(1);
    let base = if base_arg.is_null() || base_arg.is_undefined() {
        String::new()
    } else {
        base_arg.to_rust_string_lossy(scope)
    };
    let base = if base.is_empty() { None } else { Some(base) };
    let parsed = if let Some(ref b) = base {
        Url::parse(b).ok().and_then(|bu| bu.join(&input).ok())
    } else {
        Url::parse(&input).ok()
    };
    if let Some(url) = parsed {
        let o = v8::Object::new(scope);
        let s = |k: &str, v: &str| {
            o.set(
                scope,
                v8::String::new(scope, k).unwrap().into(),
                v8::String::new(scope, v).unwrap().into(),
            );
        };
        s("href", &url.to_string());
        s("hostname", url.host_str().unwrap_or(""));
        s("pathname", url.path());
        s(
            "search",
            &url.query().map(|q| format!("?{}", q)).unwrap_or_default(),
        );
        s(
            "hash",
            &url.fragment()
                .map(|f| format!("#{}", f))
                .unwrap_or_default(),
        );
        s("protocol", &format!("{}:", url.scheme()));
        s(
            "host",
            &url.host_str()
                .map(|h| {
                    if let Some(p) = url.port() {
                        format!("{}:{}", h, p)
                    } else {
                        h.to_string()
                    }
                })
                .unwrap_or_default(),
        );
        s(
            "port",
            &url.port().map(|p| p.to_string()).unwrap_or_default(),
        );
        s("origin", &url.origin().unicode_serialization());
        rv.set(o.into());
    } else {
        rv.set_null();
    }
}
fn storage_get_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let key = args.get(0).to_rust_string_lossy(scope);
    let origin = CURRENT_ORIGIN.with(|o| {
        o.borrow()
            .as_ref()
            .map(|u| u.origin().unicode_serialization())
            .unwrap_or_else(|| "null".to_string())
    });
    let store = GLOBAL_STORAGE.lock().unwrap();
    let val = store
        .data
        .get(&origin)
        .and_then(|m| m.get(&key))
        .cloned()
        .unwrap_or_else(|| "null".to_string());
    if val == "null" {
        rv.set_null();
    } else {
        rv.set(v8::String::new(scope, &val).unwrap().into());
    }
}
fn storage_set_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let key = args.get(0).to_rust_string_lossy(scope);
    let value = args.get(1).to_rust_string_lossy(scope);
    let origin = CURRENT_ORIGIN.with(|o| {
        o.borrow()
            .as_ref()
            .map(|u| u.origin().unicode_serialization())
            .unwrap_or_else(|| "null".to_string())
    });
    let mut store = GLOBAL_STORAGE.lock().unwrap();
    store
        .data
        .entry(origin)
        .or_insert_with(HashMap::new)
        .insert(key, value);
    store.save();
}
fn storage_remove_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let key = args.get(0).to_rust_string_lossy(scope);
    let origin = CURRENT_ORIGIN.with(|o| {
        o.borrow()
            .as_ref()
            .map(|u| u.origin().unicode_serialization())
            .unwrap_or_else(|| "null".to_string())
    });
    let mut store = GLOBAL_STORAGE.lock().unwrap();
    if let Some(m) = store.data.get_mut(&origin) {
        m.remove(&key);
        store.save();
    }
}
fn storage_clear_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let origin = CURRENT_ORIGIN.with(|o| {
        o.borrow()
            .as_ref()
            .map(|u| u.origin().unicode_serialization())
            .unwrap_or_else(|| "null".to_string())
    });
    let mut store = GLOBAL_STORAGE.lock().unwrap();
    if let Some(m) = store.data.get_mut(&origin) {
        m.clear();
        store.save();
    }
}
fn fetch_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let url_str = args.get(0).to_rust_string_lossy(scope);
    let method_str = args.get(1).to_rust_string_lossy(scope);
    let method_str = if method_str.is_empty() {
        "GET".to_string()
    } else {
        method_str
    };
    let headers_json = args.get(2).to_rust_string_lossy(scope);
    let headers_json = if headers_json.is_empty() {
        "{}".to_string()
    } else {
        headers_json
    };
    let body = args.get(3).to_rust_string_lossy(scope);
    let resolve_val = args.get(4);
    let reject_val = args.get(5);

    let base_url = CURRENT_ORIGIN.with(|o| (*o.borrow()).clone());
    let target_url = if let Some(ref base) = base_url {
        base.join(&url_str)
            .unwrap_or_else(|_| Url::parse(&url_str).unwrap_or(base.clone()))
    } else {
        Url::parse(&url_str).unwrap_or_else(|_| Url::parse("about:blank").unwrap())
    };

    let allowed = CSP_POLICY.with(|p| {
        p.borrow()
            .as_ref()
            .map(|pol| pol.is_allowed("connect-src", &target_url, base_url.as_ref()))
            .unwrap_or(true)
    });

    if !allowed {
        if reject_val.is_function() {
            let rej: v8::Local<v8::Function> = unsafe { std::mem::transmute(reject_val) };
            let msg = v8::String::new(
                scope,
                "CSP Error: connect-src directive blocked this request",
            )
            .unwrap();
            let undef = v8::undefined(scope);
            let _ = rej.call(scope, undef.into(), &[msg.into()]);
        }
        return;
    }

    let fetch_id = NEXT_FETCH_ID.with(|id_cell| {
        let id = *id_cell.borrow();
        *id_cell.borrow_mut() += 1;
        id
    });

    if resolve_val.is_function() && reject_val.is_function() {
        let res: v8::Local<v8::Function> = unsafe { std::mem::transmute(resolve_val) };
        let rej: v8::Local<v8::Function> = unsafe { std::mem::transmute(reject_val) };
        FETCH_REGISTRY.with(|reg| {
            reg.borrow_mut().insert(
                fetch_id,
                FetchHandlers {
                    resolve: v8::Global::new(scope, res),
                    reject: v8::Global::new(scope, rej),
                },
            );
        });
    }

    TASK_SENDER.with(|s_cell| {
        if let Some(ref sender) = *s_cell.borrow() {
            let sender_clone = sender.clone();
            let origin_str = base_url.as_ref().map(|u| u.origin().unicode_serialization()).unwrap_or_else(|| "null".to_string());
            let is_cross_origin = base_url.as_ref().map(|u| u.origin() != target_url.origin()).unwrap_or(false);

            std::thread::spawn(move || {
                let client = reqwest::blocking::Client::new();
                let method = reqwest::Method::from_bytes(method_str.as_bytes()).unwrap_or(reqwest::Method::GET);
                let mut req = client.request(method.clone(), target_url.clone());
                if is_cross_origin { req = req.header("Origin", origin_str.clone()); }
                if let Ok(headers) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&headers_json) {
                    for (name, value) in headers {
                        if let Some(value_str) = value.as_str() { req = req.header(name.as_str(), value_str); }
                    }
                }
                if method != reqwest::Method::GET && method != reqwest::Method::HEAD && !body.is_empty() {
                    req = req.body(body.clone());
                }
                match req.send() {
                    Ok(response) => {
                        let response_url = response.url().to_string();
                        let status = response.status().as_u16();
                        let status_text = response.status().canonical_reason().unwrap_or("").to_string();
                        if is_cross_origin {
                            let acao = response.headers().get("access-control-allow-origin").and_then(|h| h.to_str().ok());
                            let allowed = match acao { Some("*") => true, Some(val) if val == origin_str => true, _ => false };
                            if !allowed {
                                let _ = sender_clone.send(Box::new(move || {
                                    FETCH_REGISTRY.with(|reg| {
                                        if let Some(handlers) = reg.borrow_mut().remove(&fetch_id) {
                                            FETCH_PENDING.with(|p| p.borrow_mut().push_back((handlers.reject, "CORS Error: Origin not allowed".to_string(), false)));
                                        }
                                    });
                                }));
                                return;
                            }
                        }
                        let mut headers = serde_json::Map::new();
                        for (name, value) in response.headers().iter() {
                            if let Ok(value_str) = value.to_str() {
                                headers.insert(name.as_str().to_string(), serde_json::Value::String(value_str.to_string()));
                            }
                        }
                        let resp_body = response.text().unwrap_or_default();
                        let payload = serde_json::json!({
                            "url": response_url, "status": status, "statusText": status_text,
                            "ok": (200..=299).contains(&status), "headers": headers, "body": resp_body,
                            "type": "basic", "redirected": false
                        });
                        let payload_js = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
                        let _ = sender_clone.send(Box::new(move || {
                            FETCH_REGISTRY.with(|reg| {
                                if let Some(handlers) = reg.borrow_mut().remove(&fetch_id) {
                                    FETCH_PENDING.with(|p| p.borrow_mut().push_back((handlers.resolve, payload_js, true)));
                                }
                            });
                        }));
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        let _ = sender_clone.send(Box::new(move || {
                            FETCH_REGISTRY.with(|reg| {
                                if let Some(handlers) = reg.borrow_mut().remove(&fetch_id) {
                                    FETCH_PENDING.with(|p| p.borrow_mut().push_back((handlers.reject, err_msg, false)));
                                }
                            });
                        }));
                    }
                }
            });
        }
    });
}

fn setTimeout_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let cb = args.get(0);
    if cb.is_function() {
        let cb_func: v8::Local<v8::Function> = unsafe { std::mem::transmute(cb) };
        let cb_global = v8::Global::new(scope, cb_func);
        MACRO_TASKS.with(|tasks| {
            tasks.borrow_mut().push_back(Box::new(move || {
                RUN_PENDING.with(|p| p.borrow_mut().push((cb_global.clone(), None)));
            }))
        });
    }
}

fn raf_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let cb = args.get(0);
    if cb.is_function() {
        let cb_func: v8::Local<v8::Function> = unsafe { std::mem::transmute(cb) };
        let cb_global = v8::Global::new(scope, cb_func);
        RAF_TASKS.with(|tasks| {
            tasks.borrow_mut().push_back(Box::new(move |timestamp| {
                RUN_PENDING.with(|p| p.borrow_mut().push((cb_global.clone(), Some(timestamp))));
            }))
        });
    }
}

fn ric_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let cb = args.get(0);
    if cb.is_function() {
        let cb_func: v8::Local<v8::Function> = unsafe { std::mem::transmute(cb) };
        let cb_global = v8::Global::new(scope, cb_func);
        let id = NEXT_IDLE_ID.with(|id_cell| {
            let id = *id_cell.borrow();
            *id_cell.borrow_mut() += 1;
            id
        });
        IDLE_TASKS.with(|tasks| {
            tasks.borrow_mut().push_back((
                id,
                Box::new(move |deadline| {
                    RUN_PENDING.with(|p| p.borrow_mut().push((cb_global.clone(), Some(deadline))));
                }),
            ))
        });
        rv.set_uint32(id);
    }
}

fn cic_cb(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let id = args.get(0).uint32_value(scope).unwrap_or(0);
    IDLE_TASKS.with(|tasks| tasks.borrow_mut().retain(|(tid, _)| *tid != id));
}

fn console_impl(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    level: ConsoleLevel,
) {
    let mut output = String::new();
    for i in 0..args.length() {
        if i > 0 {
            output.push(' ');
        }
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
    extract_script_sources_from_dom(handle, None)
        .into_iter()
        .filter_map(|source| match source {
            ScriptSource::InlineClassic {
                source,
                is_defer: false,
            } => Some(source),
            ScriptSource::InlineClassic { is_defer: true, .. }
            | ScriptSource::ExternalClassic { .. }
            | ScriptSource::InlineModule { .. }
            | ScriptSource::ExternalModule { .. } => None,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptSource {
    /// Inline classic scripts are always synchronous in the HTML spec (neither async nor defer
    /// should apply), but we track `is_defer` so that inline test cases can exercise defer semantics.
    InlineClassic { source: String, is_defer: bool },
    ExternalClassic {
        url: Url,
        is_async: bool,
        is_defer: bool,
    },
    /// Modules are defer-by-default. `is_async` overrides; explicit `defer` attribute has no effect.
    InlineModule {
        url: Url,
        source: String,
        node_id: u32,
        is_async: bool,
    },
    ExternalModule {
        url: Url,
        node_id: u32,
        is_async: bool,
    },
}

pub fn extract_script_sources_from_dom(
    handle: &Handle,
    base_url: Option<&Url>,
) -> Vec<ScriptSource> {
    fn walk(
        handle: &Handle,
        base_url: Option<&Url>,
        scripts: &mut Vec<ScriptSource>,
        inline_module_id: &mut usize,
    ) {
        if let NodeData::Element {
            ref name,
            ref attrs,
            ..
        } = handle.data
        {
            if name.local.to_string() == "script" {
                let mut src = None;
                let mut script_type = None;
                let mut has_nomodule = false;
                let mut is_async = false;
                let mut is_defer = false;
                for attr in attrs.borrow().iter() {
                    let attr_name = attr.name.local.to_string();
                    if attr_name == "src" {
                        src = Some(attr.value.to_string());
                    } else if attr_name == "type" {
                        script_type = Some(attr.value.to_string().trim().to_lowercase());
                    } else if attr_name == "nomodule" {
                        has_nomodule = true;
                    } else if attr_name == "async" {
                        is_async = true;
                    } else if attr_name == "defer" {
                        is_defer = true;
                    }
                }

                if has_nomodule {
                    return;
                }

                let is_module = matches!(script_type.as_deref(), Some("module"));
                let is_classic = match script_type.as_deref() {
                    None
                    | Some("")
                    | Some("text/javascript")
                    | Some("application/javascript")
                    | Some("classic") => true,
                    _ => false,
                };

                if let Some(src) = src {
                    if let Some(base) = base_url {
                        if let Ok(url) = base.join(&src).or_else(|_| Url::parse(&src)) {
                            if is_module {
                                let node_id = register_node(handle.clone());
                                scripts.push(ScriptSource::ExternalModule {
                                    url,
                                    node_id,
                                    is_async,
                                });
                            } else if is_classic {
                                scripts.push(ScriptSource::ExternalClassic {
                                    url,
                                    is_async,
                                    is_defer,
                                });
                            }
                        }
                    }
                } else {
                    let mut content = String::new();
                    for child in handle.children.borrow().iter() {
                        if let NodeData::Text { ref contents } = child.data {
                            content.push_str(&contents.borrow());
                        }
                    }
                    if !content.is_empty() {
                        if is_module {
                            if let Some(base) = base_url {
                                *inline_module_id += 1;
                                let mut url = base.clone();
                                url.set_fragment(Some(&format!(
                                    "inline-module-{}",
                                    inline_module_id
                                )));
                                let node_id = register_node(handle.clone());
                                scripts.push(ScriptSource::InlineModule {
                                    url,
                                    source: content,
                                    node_id,
                                    is_async,
                                });
                            }
                        } else if is_classic {
                            scripts.push(ScriptSource::InlineClassic {
                                source: content,
                                is_defer,
                            });
                        }
                    }
                }

                return;
            }
        }

        for child in handle.children.borrow().iter() {
            walk(child, base_url, scripts, inline_module_id);
        }
    }

    let mut scripts = Vec::new();
    let mut inline_module_id = 0;
    walk(handle, base_url, &mut scripts, &mut inline_module_id);
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

    fn make_dom_runtime(html: &str, url: &str) -> JsRuntime {
        let dom = dom::parse_html(html);
        JsRuntime::new(
            Some(dom.document.clone()),
            Some(Url::parse(url).unwrap()),
            None,
            None,
            new_console_buffer(),
        )
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

    #[test]
    fn test_execute_with_result_does_not_prescan_import_text() {
        let mut rt = make_runtime("<html><body></body></html>");
        let outcome = rt.execute_with_result(r#""import value from './dep.js'""#);
        assert_eq!(
            outcome.result.as_deref(),
            Some("import value from './dep.js'")
        );
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn test_dom_runtime_initializes_location_and_url() {
        let mut rt = make_dom_runtime(
            "<html><body></body></html>",
            "https://example.com/path/page.html",
        );
        let outcome = rt.execute_with_result(
            r#"JSON.stringify({
                href: location.href,
                baseURI: document.baseURI,
                resolved: new URL('/asset.js', location.href).href
            })"#,
        );
        assert_eq!(outcome.error, None);
        assert_eq!(
            outcome.result.as_deref(),
            Some("{\"href\":\"https://example.com/path/page.html\",\"baseURI\":\"https://example.com/path/page.html\",\"resolved\":\"https://example.com/asset.js\"}")
        );
    }

    #[test]
    fn test_script_src_getter_resolves_relative_url() {
        let mut rt = make_dom_runtime(
            r#"<html><head><script src="/bundle.js"></script></head><body></body></html>"#,
            "https://example.com/app/",
        );
        let outcome = rt.execute_with_result("document.scripts[0].src");
        assert_eq!(outcome.error, None);
        assert_eq!(
            outcome.result.as_deref(),
            Some("https://example.com/bundle.js")
        );
    }

    #[test]
    fn test_extract_script_sources_in_dom_order() {
        let dom = dom::parse_html(
            r#"<html><head>
                <script>window.a = 1;</script>
                <script src="/bundle.js"></script>
                <script type="module" src="/module.js"></script>
            </head></html>"#,
        );
        let base = Url::parse("https://example.com/app/").unwrap();
        let sources = extract_script_sources_from_dom(&dom.document, Some(&base));
        assert_eq!(sources.len(), 3);
        assert!(
            matches!(&sources[0], ScriptSource::InlineClassic { source: s, is_defer: false } if s == "window.a = 1;")
        );
        assert!(
            matches!(&sources[1], ScriptSource::ExternalClassic { url, is_async: false, is_defer: false } if url.as_str() == "https://example.com/bundle.js")
        );
        assert!(
            matches!(&sources[2], ScriptSource::ExternalModule { url, is_async: false, .. } if url.as_str() == "https://example.com/module.js")
        );
    }

    #[test]
    fn test_extract_script_sources_skips_nomodule_scripts() {
        let base = Url::parse("https://example.com/app/").unwrap();

        let module_dom = dom::parse_html(
            r#"<html><head><script type="module" src="/m.js"></script></head></html>"#,
        );
        let module_sources = extract_script_sources_from_dom(&module_dom.document, Some(&base));
        assert_eq!(module_sources.len(), 1);
        assert!(
            matches!(&module_sources[0], ScriptSource::ExternalModule { url, is_async: false, .. } if url.as_str() == "https://example.com/m.js")
        );

        let nomodule_dom =
            dom::parse_html(r#"<html><head><script nomodule>window.x=1</script></head></html>"#);
        let nomodule_sources = extract_script_sources_from_dom(&nomodule_dom.document, Some(&base));
        assert!(nomodule_sources.is_empty());

        let classic_dom =
            dom::parse_html(r#"<html><head><script>window.x=1</script></head></html>"#);
        let classic_sources = extract_script_sources_from_dom(&classic_dom.document, Some(&base));
        assert_eq!(
            classic_sources,
            vec![ScriptSource::InlineClassic {
                source: "window.x=1".to_string(),
                is_defer: false
            }]
        );
    }

    #[test]
    fn test_compile_module_source_caches_by_url() {
        let mut rt = make_runtime("<html><body></body></html>");
        let url = Url::parse("https://example.com/app/module.js").unwrap();

        let first = rt.compile_module_source(
            url.clone(),
            "import value from './dep.js'; export const answer = value;".to_string(),
        );
        assert_eq!(first.error, None);
        assert!(!first.from_cache);
        assert_eq!(first.requests, vec!["./dep.js".to_string()]);
        assert_eq!(rt.module_cache_len(), 1);

        let second = rt.compile_module_source(url.clone(), "export const answer = 42;".to_string());
        assert_eq!(second.error, None);
        assert!(second.from_cache);
        assert_eq!(second.requests, vec!["./dep.js".to_string()]);
        assert_eq!(
            rt.cached_module_source(&url),
            Some("import value from './dep.js'; export const answer = value;")
        );
        assert_eq!(rt.module_cache_len(), 1);
    }

    #[test]
    fn test_compile_module_source_reports_error_without_cache() {
        let mut rt = make_runtime("<html><body></body></html>");
        let url = Url::parse("https://example.com/app/bad.js").unwrap();
        let outcome = rt.compile_module_source(url, "export const = ;".to_string());
        assert!(outcome.error.is_some());
        assert_eq!(rt.module_cache_len(), 0);
    }

    #[test]
    fn test_resolve_module_specifier_uses_referrer() {
        let rt = make_runtime("<html><body></body></html>");
        let referrer = Url::parse("https://example.com/app/modules/main.js").unwrap();
        let resolved = rt.resolve_module_specifier("./dep.js", &referrer).unwrap();
        assert_eq!(resolved.as_str(), "https://example.com/app/modules/dep.js");
    }

    #[test]
    fn test_cached_module_requests_returns_import_specifiers() {
        let mut rt = make_runtime("<html><body></body></html>");
        let url = Url::parse("https://example.com/main.js").unwrap();
        rt.compile_module_source(
            url.clone(),
            "import { a } from './a.js'; import { b } from './b.js';".to_string(),
        );
        let requests = rt.cached_module_requests(&url).unwrap();
        assert_eq!(requests, &["./a.js", "./b.js"]);
    }

    #[test]
    fn test_instantiate_module_graph_single_module() {
        let mut rt = make_runtime("<html><body></body></html>");
        let url = Url::parse("https://example.com/main.js").unwrap();
        rt.compile_module_source(url.clone(), "export const answer = 42;".to_string());
        let result = rt.instantiate_module_graph(&[url]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_instantiate_module_graph_nested_imports() {
        let mut rt = make_runtime("<html><body></body></html>");

        let dep_url = Url::parse("https://example.com/dep.js").unwrap();
        rt.compile_module_source(dep_url.clone(), "export const value = 1;".to_string());

        let main_url = Url::parse("https://example.com/main.js").unwrap();
        rt.compile_module_source(
            main_url.clone(),
            "import { value } from './dep.js'; export const doubled = value * 2;".to_string(),
        );

        let result = rt.instantiate_module_graph(&[main_url]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_instantiate_module_graph_missing_dependency() {
        let mut rt = make_runtime("<html><body></body></html>");
        let main_url = Url::parse("https://example.com/main.js").unwrap();
        rt.compile_module_source(
            main_url.clone(),
            "import { x } from './missing.js';".to_string(),
        );
        let result = rt.instantiate_module_graph(&[main_url]);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("main.js"));
    }

    #[test]
    fn test_evaluate_module_graph_runs_top_level_code() {
        let mut rt = make_runtime("<html><body></body></html>");
        let url = Url::parse("https://example.com/main.js").unwrap();
        rt.compile_module_source(
            url.clone(),
            "globalThis.__module_loaded = true; export const x = 1;".to_string(),
        );
        rt.evaluate_module_graph(&[url.clone()]).unwrap();

        let outcome = rt.execute_with_result("globalThis.__module_loaded");
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_evaluate_module_graph_nested_imports() {
        let mut rt = make_runtime("<html><body></body></html>");

        let dep_url = Url::parse("https://example.com/dep.js").unwrap();
        rt.compile_module_source(
            dep_url.clone(),
            "export const greeting = 'hello';".to_string(),
        );

        let main_url = Url::parse("https://example.com/main.js").unwrap();
        rt.compile_module_source(
            main_url.clone(),
            "import { greeting } from './dep.js'; globalThis.__greeting = greeting;".to_string(),
        );

        rt.evaluate_module_graph(&[main_url.clone()]).unwrap();

        let outcome = rt.execute_with_result("globalThis.__greeting");
        assert_eq!(outcome.result.as_deref(), Some("hello"));
    }

    #[test]
    fn test_evaluate_module_graph_cyclic_imports() {
        let mut rt = make_runtime("<html><body></body></html>");

        let a_url = Url::parse("https://example.com/a.js").unwrap();
        rt.compile_module_source(
            a_url.clone(),
            "import { bValue } from './b.js'; export const aValue = 'a'; globalThis.__readBValue = () => bValue;"
                .to_string(),
        );

        let b_url = Url::parse("https://example.com/b.js").unwrap();
        rt.compile_module_source(
            b_url.clone(),
            "import { aValue } from './a.js'; export const bValue = 'b'; globalThis.__readAValue = () => aValue;"
                .to_string(),
        );

        rt.evaluate_module_graph(&[a_url.clone()]).unwrap();

        let outcome = rt.execute_with_result("globalThis.__readBValue()");
        assert_eq!(outcome.result.as_deref(), Some("b"));

        let outcome = rt.execute_with_result("globalThis.__readAValue()");
        assert_eq!(outcome.result.as_deref(), Some("a"));
    }

    #[test]
    fn test_evaluate_module_graph_reports_runtime_error() {
        let mut rt = make_runtime("<html><body></body></html>");
        let url = Url::parse("https://example.com/bad.js").unwrap();
        rt.compile_module_source(url.clone(), "throw new Error('module error');".to_string());

        let result = rt.evaluate_module_graph(&[url.clone()]);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("bad.js"));
    }

    #[test]
    fn test_evaluate_module_graph_evaluation_order() {
        let mut rt = make_runtime("<html><body></body></html>");

        let a_url = Url::parse("https://example.com/a.js").unwrap();
        rt.compile_module_source(
            a_url.clone(),
            "import './b.js'; globalThis.__eval_order = (globalThis.__eval_order || '') + 'a';"
                .to_string(),
        );

        let b_url = Url::parse("https://example.com/b.js").unwrap();
        rt.compile_module_source(
            b_url.clone(),
            "globalThis.__eval_order = (globalThis.__eval_order || '') + 'b';".to_string(),
        );

        rt.evaluate_module_graph(&[a_url.clone()]).unwrap();

        let outcome = rt.execute_with_result("globalThis.__eval_order");
        assert_eq!(outcome.result.as_deref(), Some("ba"));
    }

    #[test]
    fn test_import_meta_url_inline_module() {
        let mut rt = make_runtime("<html><body></body></html>");
        let url = Url::parse("https://example.com/app/main.js").unwrap();

        rt.compile_module_source(
            url.clone(),
            "globalThis.__meta_url = import.meta.url;".to_string(),
        );
        rt.evaluate_module_graph(&[url.clone()]).unwrap();

        let outcome = rt.execute_with_result("globalThis.__meta_url");
        assert_eq!(outcome.error, None);
        assert_eq!(
            outcome.result.as_deref(),
            Some("https://example.com/app/main.js")
        );
    }

    #[test]
    fn test_import_meta_url_external_module() {
        let mut rt = make_runtime("<html><body></body></html>");
        let url = Url::parse("https://cdn.example.com/lib/module.mjs").unwrap();

        rt.compile_module_source(
            url.clone(),
            "globalThis.__cdn_meta_url = import.meta.url;".to_string(),
        );
        rt.evaluate_module_graph(&[url.clone()]).unwrap();

        let outcome = rt.execute_with_result("globalThis.__cdn_meta_url");
        assert_eq!(outcome.error, None);
        assert_eq!(
            outcome.result.as_deref(),
            Some("https://cdn.example.com/lib/module.mjs")
        );
    }

    #[test]
    fn test_import_meta_url_dependency_module() {
        let mut rt = make_runtime("<html><body></body></html>");

        let dep_url = Url::parse("https://example.com/dep.js").unwrap();
        rt.compile_module_source(
            dep_url.clone(),
            "globalThis.__dep_meta_url = import.meta.url;".to_string(),
        );

        let main_url = Url::parse("https://example.com/main.js").unwrap();
        rt.compile_module_source(main_url.clone(), "import './dep.js';".to_string());

        rt.evaluate_module_graph(&[main_url]).unwrap();

        let outcome = rt.execute_with_result("globalThis.__dep_meta_url");
        assert_eq!(outcome.error, None);
        assert_eq!(
            outcome.result.as_deref(),
            Some("https://example.com/dep.js")
        );
    }

    #[test]
    fn test_import_meta_url_with_imports() {
        let mut rt = make_runtime("<html><body></body></html>");

        let lib_url = Url::parse("https://example.com/lib/helpers.js").unwrap();
        rt.compile_module_source(
            lib_url.clone(),
            "export const version = '1.0'; globalThis.__helpers_meta = import.meta.url;"
                .to_string(),
        );

        let app_url = Url::parse("https://example.com/app/main.js").unwrap();
        rt.compile_module_source(
            app_url.clone(),
            "import { version } from '../lib/helpers.js'; globalThis.__app_meta = import.meta.url; globalThis.__version = version;".to_string(),
        );

        rt.evaluate_module_graph(&[app_url]).unwrap();

        let app_meta = rt.execute_with_result("globalThis.__app_meta");
        assert_eq!(app_meta.error, None);
        assert_eq!(
            app_meta.result.as_deref(),
            Some("https://example.com/app/main.js")
        );

        let helpers_meta = rt.execute_with_result("globalThis.__helpers_meta");
        assert_eq!(helpers_meta.error, None);
        assert_eq!(
            helpers_meta.result.as_deref(),
            Some("https://example.com/lib/helpers.js")
        );

        let version = rt.execute_with_result("globalThis.__version");
        assert_eq!(version.error, None);
        assert_eq!(version.result.as_deref(), Some("1.0"));
    }

    #[test]
    fn test_dynamic_import_resolves_with_namespace() {
        let mut rt = make_runtime("<html><body></body></html>");
        CURRENT_ORIGIN.with(|o| {
            *o.borrow_mut() = Some(Url::parse("https://example.com/").unwrap());
        });

        MODULE_SOURCES.with(|map| {
            map.borrow_mut().insert(
                "https://example.com/answer.js".to_string(),
                "export const answer = 42;".to_string(),
            );
        });

        rt.execute(
            "(async () => { const mod = await import('https://example.com/answer.js'); globalThis.__answer = mod.answer; })()",
        );

        rt.tick(Some(0.0), None);

        let outcome = rt.execute_with_result("globalThis.__answer");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("42"));
    }

    #[test]
    fn test_dynamic_import_rejects_on_eval_error() {
        let mut rt = make_runtime("<html><body></body></html>");
        CURRENT_ORIGIN.with(|o| {
            *o.borrow_mut() = Some(Url::parse("https://example.com/").unwrap());
        });

        MODULE_SOURCES.with(|map| {
            map.borrow_mut().insert(
                "https://example.com/bad.js".to_string(),
                "throw new Error('module error');".to_string(),
            );
        });

        rt.execute(
            "(async () => { try { await import('https://example.com/bad.js'); globalThis.__error = 'no error'; } catch(e) { globalThis.__error = e.toString(); } })()",
        );

        rt.tick(Some(0.0), None);

        let outcome = rt.execute_with_result("globalThis.__error");
        assert_eq!(outcome.error, None);
        assert!(outcome
            .result
            .as_deref()
            .unwrap_or("")
            .contains("module error"));
    }

    #[test]
    fn test_dynamic_import_resolve_failure_rejects() {
        let mut rt = make_runtime("<html><body></body></html>");

        rt.execute(
            "(async () => { try { await import('./nonexistent.js'); globalThis.__err = 'no error'; } catch(e) { globalThis.__err = 'got error'; } })()",
        );

        rt.tick(Some(0.0), None);

        let outcome = rt.execute_with_result("globalThis.__err");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("got error"));
    }

    #[test]
    fn test_dynamic_import_returns_object() {
        let mut rt = make_runtime("<html><body></body></html>");
        CURRENT_ORIGIN.with(|o| {
            *o.borrow_mut() = Some(Url::parse("https://example.com/").unwrap());
        });

        MODULE_SOURCES.with(|map| {
            map.borrow_mut().insert(
                "https://example.com/answer.js".to_string(),
                "export const answer = 42;".to_string(),
            );
        });

        let outcome = rt.execute_with_result("typeof import('https://example.com/answer.js')");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("object"));
    }

    #[test]
    fn test_character_data_constructor_exists() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("typeof CharacterData");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("function"));
    }

    #[test]
    fn test_character_data_is_window_property() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("typeof window.CharacterData");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("function"));
    }

    #[test]
    fn test_character_data_prototype_instanceof_node() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("CharacterData.prototype instanceof Node");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_text_node_is_instanceof_character_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome =
            rt.execute_with_result("document.createTextNode('x') instanceof CharacterData");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_text_node_is_instanceof_node() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("document.createTextNode('x') instanceof Node");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_comment_is_instanceof_character_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome =
            rt.execute_with_result("document.createComment('x') instanceof CharacterData");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_comment_is_instanceof_node() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("document.createComment('x') instanceof Node");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_text_prototype_instanceof_character_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("Text.prototype instanceof CharacterData");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_comment_prototype_instanceof_character_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("Comment.prototype instanceof CharacterData");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_text_node_has_data_property() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("document.createTextNode('hello').data");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("hello"));
    }

    #[test]
    fn test_text_node_has_length_property() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("document.createTextNode('hello').length");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("5"));
    }

    #[test]
    fn test_character_data_append_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result(
            "var n = document.createTextNode('hello'); n.appendData(' world'); n.data",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("hello world"));
    }

    #[test]
    fn test_character_data_delete_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result(
            "var n = document.createTextNode('hello'); n.deleteData(1, 3); n.data",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("ho"));
    }

    #[test]
    fn test_character_data_insert_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result(
            "var n = document.createTextNode('ho'); n.insertData(1, 'ell'); n.data",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("hello"));
    }

    #[test]
    fn test_character_data_replace_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome = rt.execute_with_result(
            "var n = document.createTextNode('hello world'); n.replaceData(0, 5, 'goodbye'); n.data",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("goodbye world"));
    }

    #[test]
    fn test_character_data_substring_data() {
        let mut rt = make_dom_runtime("<html><body>hello</body></html>", "https://example.com/");
        let outcome =
            rt.execute_with_result("document.createTextNode('hello world').substringData(0, 5)");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("hello"));
    }

    #[test]
    fn test_onsubmit_handler_assignment_and_dispatch() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'></form></body></html>"#,
            "https://example.com/",
        );
        rt.execute(
            "var form = document.getElementById('f'); \
             form.onsubmit = function(e) { e.preventDefault(); window.__handled = true; };",
        );
        rt.trigger_event("f", "submit");
        let outcome = rt.execute_with_result("window.__handled");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_onclick_handler_assignment_and_dispatch() {
        let mut rt = make_dom_runtime(
            r#"<html><body><button id='btn'></button></body></html>"#,
            "https://example.com/",
        );
        rt.execute(
            "var btn = document.getElementById('btn'); \
             btn.onclick = function() { window.__clicked = true; };",
        );
        rt.trigger_event("btn", "click");
        let outcome = rt.execute_with_result("window.__clicked");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_onload_handler_dispatch() {
        let mut rt = make_dom_runtime(r#"<html><body></body></html>"#, "https://example.com/");
        rt.execute(
            "window.onload = function() { window.__loadFired = true; }; \
             var ev = new Event('load', { bubbles: false }); \
             window.dispatchEvent(ev);",
        );
        let outcome = rt.execute_with_result("window.__loadFired");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_html_form_element_submit_method_exists() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'></form></body></html>"#,
            "https://example.com/",
        );
        let outcome = rt.execute_with_result("typeof document.getElementById('f').submit");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("function"));
    }

    #[test]
    fn test_html_form_element_reset_method_exists() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'></form></body></html>"#,
            "https://example.com/",
        );
        let outcome = rt.execute_with_result("typeof document.getElementById('f').reset");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("function"));
    }

    #[test]
    fn test_document_forms_returns_actual_forms() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f1'></form><form id='f2'></form></body></html>"#,
            "https://example.com/",
        );
        let outcome = rt.execute_with_result("document.forms.length");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("2"));
    }

    #[test]
    fn test_form_elements_returns_controls() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'><input name='a'><input name='b'></form></body></html>"#,
            "https://example.com/",
        );
        let outcome = rt.execute_with_result("document.getElementById('f').elements.length");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("2"));
    }

    #[test]
    fn test_form_submit_calls_onsubmit_handler() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'><input name='q' value='test'></form></body></html>"#,
            "https://example.com/",
        );
        rt.execute(
            "var form = document.getElementById('f'); \
             form.onsubmit = function(e) { e.preventDefault(); window.__formSubmitted = true; }; \
             form.submit();",
        );
        let outcome = rt.execute_with_result("window.__formSubmitted");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_form_reset_resets_input_values() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'><input name='q' value='default'></form></body></html>"#,
            "https://example.com/",
        );
        let outcome = rt.execute_with_result(
            "var form = document.getElementById('f'); \
             typeof form.reset === 'function' && \
             form.querySelector('[name=\"q\"]') !== null",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_form_request_submit_triggers_handler() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'></form></body></html>"#,
            "https://example.com/",
        );
        rt.execute(
            "var form = document.getElementById('f'); \
             form.onsubmit = function(e) { e.preventDefault(); window.__reqSubmitted = true; }; \
             form.requestSubmit();",
        );
        let outcome = rt.execute_with_result("window.__reqSubmitted");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_handler_property_returns_null_when_not_set() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'></form></body></html>"#,
            "https://example.com/",
        );
        let outcome = rt.execute_with_result("document.getElementById('f').onsubmit === null");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_submit_preserves_default_prevented_state() {
        let mut rt = make_dom_runtime(
            r#"<html><body><form id='f'></form></body></html>"#,
            "https://example.com/",
        );
        rt.execute(
            "var form = document.getElementById('f'); \
             var prevented = false; \
             form.addEventListener('submit', function(e) { e.preventDefault(); prevented = e.defaultPrevented; }); \
             form.requestSubmit();",
        );
        let outcome = rt.execute_with_result("prevented");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_oninput_handler_dispatch() {
        let mut rt = make_dom_runtime(
            r#"<html><body><input id='inp' type='text'></body></html>"#,
            "https://example.com/",
        );
        rt.execute(
            "var inp = document.getElementById('inp'); \
             inp.oninput = function() { window.__inputFired = true; };",
        );
        rt.trigger_event("inp", "input");
        let outcome = rt.execute_with_result("window.__inputFired");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_onkeydown_handler_dispatch() {
        let mut rt = make_dom_runtime(
            r#"<html><body><input id='inp' type='text'></body></html>"#,
            "https://example.com/",
        );
        rt.execute(
            "var inp = document.getElementById('inp'); \
             inp.onkeydown = function() { window.__keyFired = true; };",
        );
        rt.trigger_event("inp", "keydown");
        let outcome = rt.execute_with_result("window.__keyFired");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_form_create_element_creates_html_form_element() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result(
            "var f = document.createElement('form'); typeof f.submit === 'function'",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_iframe_element_has_content_document() {
        let mut rt = make_dom_runtime(
            "<html><body><iframe id='f'></iframe></body></html>",
            "https://example.com/",
        );
        let outcome =
            rt.execute_with_result("document.getElementById('f').contentDocument !== null");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_iframe_content_window_has_document() {
        let mut rt = make_dom_runtime(
            "<html><body><iframe id='f'></iframe></body></html>",
            "https://example.com/",
        );
        let outcome =
            rt.execute_with_result("document.getElementById('f').contentWindow.document !== null");
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_iframe_content_document_create_element() {
        let mut rt = make_dom_runtime(
            "<html><body><iframe id='f'></iframe></body></html>",
            "https://example.com/",
        );
        let outcome = rt.execute_with_result(
            "var iframe = document.getElementById('f'); \
             var doc = iframe.contentDocument; \
             var s = doc.createElement('script'); \
             s !== null",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_iframe_append_child_to_content_document_head() {
        let mut rt = make_dom_runtime(
            "<html><body><iframe id='f'></iframe></body></html>",
            "https://example.com/",
        );
        let outcome = rt.execute_with_result(
            "var iframe = document.getElementById('f'); \
             var doc = iframe.contentDocument; \
             var s = doc.createElement('script'); \
             doc.head.appendChild(s); \
             s.parentNode !== null",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_iframe_content_document_has_body() {
        let mut rt = make_dom_runtime(
            "<html><body><iframe id='f'></iframe></body></html>",
            "https://example.com/",
        );
        let outcome = rt.execute_with_result(
            "var iframe = document.getElementById('f'); \
             iframe.contentDocument.body !== null",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_iframe_content_window_self_reference() {
        let mut rt = make_dom_runtime(
            "<html><body><iframe id='f'></iframe></body></html>",
            "https://example.com/",
        );
        let outcome = rt.execute_with_result(
            "var cw = document.getElementById('f').contentWindow; \
             cw === cw.self",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_iframe_content_window_frame_element() {
        let mut rt = make_dom_runtime(
            "<html><body><iframe id='f'></iframe></body></html>",
            "https://example.com/",
        );
        let outcome = rt.execute_with_result(
            "var iframe = document.getElementById('f'); \
             iframe.contentWindow.frameElement === iframe",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_create_element_iframe_has_content_document() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result(
            "var iframe = document.createElement('iframe'); \
             iframe.contentDocument !== null",
        );
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_shadow_root_is_function() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("typeof ShadowRoot");
        assert_eq!(outcome.result.as_deref(), Some("function"));
    }

    #[test]
    fn test_intersection_observer_is_function() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("typeof IntersectionObserver");
        assert_eq!(outcome.result.as_deref(), Some("function"));
    }

    #[test]
    fn test_resize_observer_is_function() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("typeof ResizeObserver");
        assert_eq!(outcome.result.as_deref(), Some("function"));
    }

    #[test]
    fn test_mutation_observer_is_function() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("typeof MutationObserver");
        assert_eq!(outcome.result.as_deref(), Some("function"));
    }

    #[test]
    fn test_intersection_observer_has_take_records() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result(
            "var io = new IntersectionObserver(function() {}); \
             typeof io.takeRecords === 'function' && io.takeRecords().length === 0",
        );
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_mutation_observer_take_records_returns_array() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result(
            "var mo = new MutationObserver(function() {}); \
             Array.isArray(mo.takeRecords())",
        );
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_get_root_node_returns_document_for_element() {
        let mut rt = make_dom_runtime(
            "<html><body><div id='test'></div></body></html>",
            "https://example.com/",
        );
        let outcome =
            rt.execute_with_result("document.getElementById('test').getRootNode() === document");
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_get_root_node_returns_document_for_body() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("document.body.getRootNode() === document");
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_get_root_node_on_document_element() {
        let mut rt = make_dom_runtime("<html><body></body></html>", "https://example.com/");
        let outcome = rt.execute_with_result("document.documentElement.getRootNode() === document");
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }

    #[test]
    fn test_node_has_owner_document() {
        let mut rt = make_dom_runtime(
            "<html><body><div id='test'></div></body></html>",
            "https://example.com/",
        );
        let outcome =
            rt.execute_with_result("document.getElementById('test').ownerDocument === document");
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }
}

// ── DOM Helper Functions ──────────────────────────────────────────────────────

fn find_element_by_tag(root: &Handle, tag: &str) -> Option<Handle> {
    if let NodeData::Element { ref name, .. } = root.data {
        if name.local.to_string() == tag {
            return Some(root.clone());
        }
    }
    for child in root.children.borrow().iter() {
        if let Some(found) = find_element_by_tag(child, tag) {
            return Some(found);
        }
    }
    None
}

fn find_direct_child_element_by_tag(root: &Handle, tag: &str) -> Option<Handle> {
    for child in root.children.borrow().iter() {
        if let NodeData::Element { ref name, .. } = child.data {
            if name.local.to_string() == tag {
                return Some(child.clone());
            }
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
        if let Some(found) = find_element_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

fn find_document_doctype(root: &Handle) -> Option<Handle> {
    for child in root.children.borrow().iter() {
        if let NodeData::Doctype { .. } = child.data {
            return Some(child.clone());
        }
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
        reg.borrow()
            .get(&id)
            .and_then(|node| node.parent.take().and_then(|weak| weak.upgrade()))
    });
    if let Some(ref parent_handle) = parent {
        let old_ptr = NODE_REGISTRY.with(|reg| {
            reg.borrow()
                .get(&id)
                .map(|node| Rc::as_ptr(node) as usize)
                .unwrap_or(0)
        });
        let mut children = parent_handle.children.borrow_mut();
        if let Some(pos) = children
            .iter()
            .position(|child| Rc::as_ptr(child) as usize == old_ptr)
        {
            new_handle.parent.set(Some(Rc::downgrade(parent_handle)));
            children[pos] = new_handle.clone();
        }
    }
    NODE_REGISTRY.with(|reg| {
        if let Some(old_handle) = reg.borrow_mut().insert(id, new_handle.clone()) {
            let old_ptr = Rc::as_ptr(&old_handle) as usize;
            REVERSE_NODE_REGISTRY.with(|reverse| {
                reverse.borrow_mut().remove(&old_ptr);
            });
        }
    });
    let new_ptr = Rc::as_ptr(&new_handle) as usize;
    REVERSE_NODE_REGISTRY.with(|reg| reg.borrow_mut().insert(new_ptr, id));
}

fn mark_document_fragment_id(id: u32) {
    DOCUMENT_FRAGMENT_NODE_IDS.with(|ids| {
        ids.borrow_mut().insert(id);
    });
}

fn is_document_fragment_id(id: u32) -> bool {
    DOCUMENT_FRAGMENT_NODE_IDS.with(|ids| ids.borrow().contains(&id))
}

fn detach_node_from_parent(node: &Handle) {
    let node_ptr = Rc::as_ptr(node) as usize;
    let parent_weak = node.parent.take();
    if let Some(parent_weak) = parent_weak {
        if let Some(parent) = parent_weak.upgrade() {
            parent
                .children
                .borrow_mut()
                .retain(|c| Rc::as_ptr(c) as usize != node_ptr);
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
    let index = children
        .iter()
        .position(|child| Rc::as_ptr(child) as usize == node_ptr)?;
    match direction {
        SiblingDirection::Next => {
            for sibling in children.iter().skip(index + 1) {
                if elements_only && !matches!(sibling.data, NodeData::Element { .. }) {
                    continue;
                }
                return Some(register_node(sibling.clone()));
            }
        }
        SiblingDirection::Previous => {
            for sibling in children[..index].iter().rev() {
                if elements_only && !matches!(sibling.data, NodeData::Element { .. }) {
                    continue;
                }
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
    use html5ever::{ns, LocalName, QualName};
    let ctx_name = QualName::new(None, ns!(html), LocalName::from(ctx_tag));
    let dom = parse_fragment(
        markup5ever_rcdom::RcDom::default(),
        Default::default(),
        ctx_name,
        vec![],
        false,
    )
    .from_utf8()
    .read_from(&mut html.as_bytes())
    .unwrap();
    steal_fragment_children(&dom.document)
}

fn steal_fragment_children(doc: &Handle) -> Vec<Handle> {
    for child in doc.children.borrow().iter() {
        if let NodeData::Element { .. } = child.data {
            let children: Vec<Handle> = child.children.borrow_mut().drain(..).collect();
            for c in &children {
                c.parent.set(None);
            }
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
        NodeData::Element {
            ref name,
            ref attrs,
            ..
        } => {
            let tag = name.local.to_string();
            out.push('<');
            out.push_str(&tag);
            for attr in attrs.borrow().iter() {
                out.push(' ');
                out.push_str(&attr.name.local.to_string());
                out.push_str("=\"");
                out.push_str(&html_escape(&attr.value.to_string()));
                out.push('"');
            }
            out.push('>');
            for child in node.children.borrow().iter() {
                serialize_node(child, out);
            }
            out.push_str("</");
            out.push_str(&tag);
            out.push('>');
        }
        NodeData::Text { ref contents } => {
            out.push_str(&html_escape(&contents.borrow()));
        }
        NodeData::Comment { ref contents } => {
            out.push_str("<!--");
            out.push_str(contents);
            out.push_str("-->");
        }
        NodeData::Doctype {
            ref name,
            ref public_id,
            ref system_id,
        } => {
            out.push_str("<!DOCTYPE ");
            out.push_str(name);
            if !public_id.is_empty() {
                out.push_str(" PUBLIC \"");
                out.push_str(public_id);
                out.push('"');
                if !system_id.is_empty() {
                    out.push_str(" \"");
                    out.push_str(system_id);
                    out.push('"');
                }
            }
            out.push('>');
        }
        NodeData::Document => {
            for child in node.children.borrow().iter() {
                serialize_node(child, out);
            }
        }
        _ => {}
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn collect_text_content(node: &Handle) -> String {
    let mut text = String::new();
    match &node.data {
        NodeData::Text { ref contents } => {
            text.push_str(&contents.borrow());
        }
        _ => {
            for child in node.children.borrow().iter() {
                text.push_str(&collect_text_content(child));
            }
        }
    }
    text
}

fn split_selector_groups(selector: &str) -> Vec<&str> {
    let mut groups = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (idx, ch) in selector.char_indices() {
        match ch {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                let part = selector[start..idx].trim();
                if !part.is_empty() {
                    groups.push(part);
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    let tail = selector[start..].trim();
    if !tail.is_empty() {
        groups.push(tail);
    }
    groups
}

fn selector_is_supported_for_dom_queries(selector: &Selector) -> bool {
    if selector.pseudo_class.is_some() || selector.pseudo_element.is_some() {
        return false;
    }
    selector
        .ancestor
        .as_deref()
        .map(selector_is_supported_for_dom_queries)
        .unwrap_or(true)
}

fn selector_subject_matches_handle(node: &Handle, selector: &Selector) -> bool {
    let NodeData::Element {
        ref name,
        ref attrs,
        ..
    } = node.data
    else {
        return false;
    };
    let has_constraint = selector.tag.is_some()
        || selector.id.is_some()
        || !selector.class.is_empty()
        || !selector.attributes.is_empty();
    if !has_constraint {
        return false;
    }
    let tag = name.local.to_string().to_lowercase();
    if let Some(ref required_tag) = selector.tag {
        if tag != required_tag.to_lowercase() {
            return false;
        }
    }
    let attrs_ref = attrs.borrow();
    let id_val = attrs_ref
        .iter()
        .find(|a| a.name.local.to_string() == "id")
        .map(|a| a.value.to_string());
    if let Some(ref required_id) = selector.id {
        if id_val.as_deref() != Some(required_id.as_str()) {
            return false;
        }
    }
    let class_val = attrs_ref
        .iter()
        .find(|a| a.name.local.to_string() == "class")
        .map(|a| a.value.to_string())
        .unwrap_or_default();
    let classes: Vec<&str> = class_val.split_whitespace().collect();
    for required_class in &selector.class {
        if !classes.contains(&required_class.as_str()) {
            return false;
        }
    }
    for attr_sel in &selector.attributes {
        let matched = attrs_ref.iter().any(|attr| {
            if attr.name.local.to_string() != attr_sel.name {
                return false;
            }
            match &attr_sel.value {
                AttributeMatch::Exists => true,
                AttributeMatch::Equals(expected) => attr.value.to_string() == *expected,
            }
        });
        if !matched {
            return false;
        }
    }
    true
}

fn previous_element_siblings(node: &Handle) -> Vec<Handle> {
    let Some(parent) = get_parent_handle(node) else {
        return Vec::new();
    };
    let node_ptr = Rc::as_ptr(node) as usize;
    let mut siblings = Vec::new();
    for child in parent.children.borrow().iter() {
        if Rc::as_ptr(child) as usize == node_ptr {
            break;
        }
        if matches!(child.data, NodeData::Element { .. }) {
            siblings.push(child.clone());
        }
    }
    siblings
}

fn selector_matches_parsed_handle(node: &Handle, selector: &Selector) -> bool {
    if !selector_is_supported_for_dom_queries(selector)
        || !selector_subject_matches_handle(node, selector)
    {
        return false;
    }
    let Some(ref ancestor_sel) = selector.ancestor else {
        return true;
    };
    let combinator = selector
        .combinator
        .as_ref()
        .unwrap_or(&Combinator::Descendant);
    match combinator {
        Combinator::Descendant => {
            let mut current = get_parent_handle(node);
            while let Some(parent) = current {
                if matches!(parent.data, NodeData::Element { .. })
                    && selector_matches_parsed_handle(&parent, ancestor_sel)
                {
                    return true;
                }
                current = get_parent_handle(&parent);
            }
            false
        }
        Combinator::Child => get_parent_handle(node)
            .filter(|parent| matches!(parent.data, NodeData::Element { .. }))
            .map(|parent| selector_matches_parsed_handle(&parent, ancestor_sel))
            .unwrap_or(false),
        Combinator::NextSibling => previous_element_siblings(node)
            .into_iter()
            .rev()
            .next()
            .map(|sibling| selector_matches_parsed_handle(&sibling, ancestor_sel))
            .unwrap_or(false),
        Combinator::SubsequentSibling => previous_element_siblings(node)
            .into_iter()
            .rev()
            .any(|sibling| selector_matches_parsed_handle(&sibling, ancestor_sel)),
    }
}

fn selector_matches_handle(node: &Handle, selector: &str) -> bool {
    let groups = split_selector_groups(selector);
    if groups.is_empty() {
        return false;
    }
    groups.into_iter().any(|group| {
        let parsed = parse_selector(group);
        selector_matches_parsed_handle(node, &parsed)
    })
}

fn query_selector_first(root: &Handle, selector: &str, skip_root: bool) -> Option<Handle> {
    if !skip_root && selector_matches_handle(root, selector) {
        return Some(root.clone());
    }
    for child in root.children.borrow().iter() {
        if let Some(found) = query_selector_first(child, selector, false) {
            return Some(found);
        }
    }
    None
}

fn query_selector_all_nodes(root: &Handle, selector: &str, skip_root: bool) -> Vec<Handle> {
    let mut results = Vec::new();
    if !skip_root && selector_matches_handle(root, selector) {
        results.push(root.clone());
    }
    for child in root.children.borrow().iter() {
        results.append(&mut query_selector_all_nodes(child, selector, false));
    }
    results
}

fn find_elements_by_class(root: &Handle, cls: &str, skip_root: bool) -> Vec<Handle> {
    let mut results = Vec::new();
    if !skip_root {
        if let NodeData::Element { ref attrs, .. } = root.data {
            let class_val = attrs
                .borrow()
                .iter()
                .find(|a| a.name.local.to_string() == "class")
                .map(|a| a.value.to_string())
                .unwrap_or_default();
            if class_val.split_whitespace().any(|c| c == cls) {
                results.push(root.clone());
            }
        }
    }
    for child in root.children.borrow().iter() {
        results.append(&mut find_elements_by_class(child, cls, false));
    }
    results
}

fn find_elements_by_tag_name(root: &Handle, tag: &str, skip_root: bool) -> Vec<Handle> {
    let mut results = Vec::new();
    if !skip_root {
        if let NodeData::Element { ref name, .. } = root.data {
            if tag == "*" || name.local.to_string().to_lowercase() == tag {
                results.push(root.clone());
            }
        }
    }
    for child in root.children.borrow().iter() {
        results.append(&mut find_elements_by_tag_name(child, tag, false));
    }
    results
}
