use boa_engine::{Context, Source, JsValue, NativeFunction, js_string};
use std::collections::{HashMap, VecDeque};
use markup5ever_rcdom::{NodeData, Handle};
use std::cell::RefCell;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;
use serde::{Serialize, Deserialize};
use std::sync::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    static ref GLOBAL_STORAGE: Mutex<OriginStorage> = Mutex::new(OriginStorage::load());
}

#[derive(Serialize, Deserialize, Default)]
struct OriginStorage {
    // origin_string -> { key -> value }
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

thread_local! {
    static MACRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static MICRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static RAF_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context, f64)>>> = RefCell::new(VecDeque::new());
    static IDLE_TASKS: RefCell<VecDeque<(u32, Box<dyn FnOnce(&mut Context, f64)>)>> = RefCell::new(VecDeque::new());
    static NEXT_IDLE_ID: RefCell<u32> = RefCell::new(1);
    static DOM_ROOT: RefCell<Option<Handle>> = RefCell::new(None);
    static NODE_REGISTRY: RefCell<HashMap<u32, Handle>> = RefCell::new(HashMap::new());
    static REVERSE_NODE_REGISTRY: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
    static NEXT_NODE_ID: RefCell<u32> = RefCell::new(1);
    static FETCH_REGISTRY: RefCell<HashMap<u32, (JsValue, JsValue)>> = RefCell::new(HashMap::new());
    static FETCH_BODY_REGISTRY: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
    static NEXT_FETCH_ID: RefCell<u32> = RefCell::new(1);
    static TASK_SENDER: RefCell<Option<Sender<Box<dyn FnOnce(&mut Context) + Send>>>> = RefCell::new(None);
    static FOCUSED_NODE: RefCell<Option<String>> = RefCell::new(None);
    static PREVIOUS_FOCUSED_NODE: RefCell<Option<String>> = RefCell::new(None);
    static CURRENT_ORIGIN: RefCell<Option<Url>> = RefCell::new(None);
    static CSP_POLICY: RefCell<Option<CspPolicy>> = RefCell::new(None);
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

fn register_console_callable(context: &mut Context, name: &'static str, level: ConsoleLevel) {
    let func = NativeFunction::from_copy_closure(move |_this, args, context| {
        let mut output = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                output.push(' ');
            }
            if let Ok(s) = arg.to_string(context) {
                output.push_str(&s.to_std_string_escaped());
            }
        }
        push_console_entry(level.clone(), output);
        Ok(JsValue::undefined())
    });
    context
        .register_global_callable(js_string!(name), 1, func)
        .unwrap();
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
    pub context: Context,
    task_receiver: Receiver<Box<dyn FnOnce(&mut Context) + Send>>,
}

impl JsRuntime {
    pub fn new(
        dom: Option<Handle>,
        base_url: Option<Url>,
        policy: Option<CspPolicy>,
        console_buffer: ConsoleBuffer,
    ) -> Self {
        DOM_ROOT.with(|root| *root.borrow_mut() = dom);
        CURRENT_ORIGIN.with(|origin| *origin.borrow_mut() = base_url);
        CSP_POLICY.with(|p| *p.borrow_mut() = policy);
        CONSOLE_BUFFER.with(|cell| *cell.borrow_mut() = Some(console_buffer));
        let mut context = Context::default();
        let (task_sender, task_receiver) = channel();
        TASK_SENDER.with(|s| *s.borrow_mut() = Some(task_sender));

        register_console_callable(&mut context, "log", ConsoleLevel::Log);
        register_console_callable(&mut context, "warn", ConsoleLevel::Warn);
        register_console_callable(&mut context, "error", ConsoleLevel::Error);
        register_console_callable(&mut context, "info", ConsoleLevel::Info);
        register_console_callable(&mut context, "debug", ConsoleLevel::Debug);

        // Register native setTimeout
        let set_timeout = NativeFunction::from_copy_closure(|_this, args, _context| {
            if let Some(callback) = args.get(0).cloned() {
                if let Some(obj) = callback.as_object() {
                    if obj.is_callable() {
                        let _ms = args.get(1).and_then(|v| v.as_number()).unwrap_or(0.0);
                        
                        // For now, immediate execution in next macro-task queue
                        MACRO_TASKS.with(|tasks| {
                            tasks.borrow_mut().push_back(Box::new(move |ctx| {
                                let _ = callback.as_object().unwrap().call(&JsValue::undefined(), &[], ctx);
                            }));
                        });
                    }
                }
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("setTimeout"), 2, set_timeout).unwrap();

        // Register native requestAnimationFrame
        let raf = NativeFunction::from_copy_closure(|_this, args, _context| {
            if let Some(callback) = args.get(0).cloned() {
                if let Some(obj) = callback.as_object() {
                    if obj.is_callable() {
                        RAF_TASKS.with(|tasks| {
                            tasks.borrow_mut().push_back(Box::new(move |ctx, timestamp| {
                                let _ = callback.as_object().unwrap().call(&JsValue::undefined(), &[JsValue::from(timestamp)], ctx);
                            }));
                        });
                    }
                }
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("requestAnimationFrame"), 1, raf).unwrap();

        // Register native requestIdleCallback
        let ric = NativeFunction::from_copy_closure(|_this, args, _context| {
            if let Some(callback) = args.get(0).cloned() {
                if let Some(obj) = callback.as_object() {
                    if obj.is_callable() {
                        let id = NEXT_IDLE_ID.with(|id_cell| {
                            let id = *id_cell.borrow();
                            *id_cell.borrow_mut() += 1;
                            id
                        });
                        IDLE_TASKS.with(|tasks| {
                            tasks.borrow_mut().push_back((id, Box::new(move |ctx, deadline| {
                                let deadline_obj_val = ctx.eval(Source::from_bytes(b"({})")).unwrap();
                                let deadline_obj = deadline_obj_val.as_object().unwrap();
                                
                                let time_rem = NativeFunction::from_copy_closure(move |_this, _args, _context| {
                                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64;
                                    Ok(JsValue::from((deadline - now).max(0.0)))
                                });
                                
                                ctx.register_global_callable(js_string!("__aura_temp_time_rem"), 0, time_rem).unwrap();
                                let time_rem_fn = ctx.eval(Source::from_bytes(b"__aura_temp_time_rem")).unwrap();
                                
                                deadline_obj.set(js_string!("timeRemaining"), time_rem_fn, false, ctx).unwrap();
                                deadline_obj.set(js_string!("didTimeout"), JsValue::from(false), false, ctx).unwrap();
                                let _ = ctx.eval(Source::from_bytes(b"delete globalThis.__aura_temp_time_rem"));
                                
                                let _ = obj.call(&JsValue::undefined(), &[deadline_obj_val], ctx);
                            })));
                        });
                        return Ok(JsValue::from(id));
                    }
                }
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("requestIdleCallback"), 1, ric).unwrap();

        // Register native cancelIdleCallback
        let cic = NativeFunction::from_copy_closure(|_this, args, _context| {
            let id = args.get(0).and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
            IDLE_TASKS.with(|tasks| {
                tasks.borrow_mut().retain(|(tid, _)| *tid != id);
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("cancelIdleCallback"), 1, cic).unwrap();

        // Register native __aura_set_focus
        let set_focus = NativeFunction::from_copy_closure(|_this, args, _context| {
            let id = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped());
            FOCUSED_NODE.with(|f| *f.borrow_mut() = id);
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_set_focus"), 1, set_focus).unwrap();

        // Register native __aura_get_element_by_id
        let get_element_by_id = NativeFunction::from_copy_closure(|_this, args, _context| {
            let id = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let res = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    find_element_by_id(r, &id).map(|h| {
                        let tag = if let NodeData::Element { ref name, .. } = h.data {
                            name.local.to_string()
                        } else { "".to_string() };
                        (register_node(h), tag)
                    })
                } else { None }
            });
            
            if let Some((nid, tag)) = res {
                use boa_engine::object::ObjectInitializer;
                let mut obj = ObjectInitializer::new(_context);
                obj.property(js_string!("nid"), JsValue::from(nid), boa_engine::property::Attribute::all());
                obj.property(js_string!("tag"), JsValue::from(js_string!(tag)), boa_engine::property::Attribute::all());
                obj.property(js_string!("kind"), JsValue::from(js_string!("element")), boa_engine::property::Attribute::all());
                Ok(obj.build().into())
            } else {
                Ok(JsValue::null())
            }
        });
        context.register_global_callable(js_string!("__aura_get_element_by_id"), 1, get_element_by_id).unwrap();

        // Register native __aura_get_parent_id
        let get_parent_id = NativeFunction::from_copy_closure(|_this, args, _context| {
            let child_nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let parent_nid = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    find_parent_of_node(r, child_nid).map(register_node)
                } else { None }
            });
            Ok(parent_nid.map(JsValue::from).unwrap_or(JsValue::null()))
        });
        context.register_global_callable(js_string!("__aura_get_parent_id"), 1, get_parent_id).unwrap();

        // Register native __aura_get_body
        let get_body = NativeFunction::from_copy_closure(|_this, _args, _context| {
            let body_nid = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    find_element_by_tag(r, "body").map(register_node)
                } else { None }
            });
            Ok(body_nid.map(JsValue::from).unwrap_or(JsValue::null()))
        });
        context.register_global_callable(js_string!("__aura_get_body"), 0, get_body).unwrap();

        // Register native __aura_set_attribute
        let set_attr = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let name = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let value = args.get(2).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            
            NODE_REGISTRY.with(|reg| {
                if let Some(node) = reg.borrow().get(&nid) {
                    if let NodeData::Element { ref attrs, .. } = node.data {
                        let mut attrs = attrs.borrow_mut();
                        let mut found = false;
                        for attr in attrs.iter_mut() {
                            if attr.name.local.to_string() == name {
                                attr.value = value.clone().into();
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            use html5ever::{QualName, LocalName, ns, Attribute};
                            attrs.push(Attribute {
                                name: QualName::new(None, ns!(html), LocalName::from(name)),
                                value: value.into(),
                            });
                        }
                    }
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_set_attribute"), 3, set_attr).unwrap();

        // Register native __aura_storage_get
        let storage_get = NativeFunction::from_copy_closure(|_this, args, _context| {
            let key = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let origin = CURRENT_ORIGIN.with(|o| o.borrow().as_ref().map(|u| u.origin().unicode_serialization()).unwrap_or_else(|| "null".to_string()));
            let store = GLOBAL_STORAGE.lock().unwrap();
            let val = store.data.get(&origin).and_then(|m| m.get(&key)).cloned().unwrap_or_else(|| "null".to_string());
            if val == "null" {
                Ok(JsValue::null())
            } else {
                Ok(JsValue::from(js_string!(val)))
            }
        });
        context.register_global_callable(js_string!("__aura_storage_get"), 1, storage_get).unwrap();

        // Register native __aura_storage_set
        let storage_set = NativeFunction::from_copy_closure(|_this, args, _context| {
            let key = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let value = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let origin = CURRENT_ORIGIN.with(|o| o.borrow().as_ref().map(|u| u.origin().unicode_serialization()).unwrap_or_else(|| "null".to_string()));
            let mut store = GLOBAL_STORAGE.lock().unwrap();
            store.data.entry(origin).or_insert_with(HashMap::new).insert(key, value);
            store.save();
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_storage_set"), 2, storage_set).unwrap();

        // Register native __aura_storage_remove
        let storage_remove = NativeFunction::from_copy_closure(|_this, args, _context| {
            let key = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let origin = CURRENT_ORIGIN.with(|o| o.borrow().as_ref().map(|u| u.origin().unicode_serialization()).unwrap_or_else(|| "null".to_string()));
            let mut store = GLOBAL_STORAGE.lock().unwrap();
            if let Some(m) = store.data.get_mut(&origin) {
                m.remove(&key);
                store.save();
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_storage_remove"), 1, storage_remove).unwrap();

        // Register native __aura_storage_clear
        let storage_clear = NativeFunction::from_copy_closure(|_this, _args, _context| {
            let origin = CURRENT_ORIGIN.with(|o| o.borrow().as_ref().map(|u| u.origin().unicode_serialization()).unwrap_or_else(|| "null".to_string()));
            let mut store = GLOBAL_STORAGE.lock().unwrap();
            if let Some(m) = store.data.get_mut(&origin) {
                m.clear();
                store.save();
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_storage_clear"), 0, storage_clear).unwrap();

        // Register native __aura_queue_task
        let queue_task = NativeFunction::from_copy_closure(|_this, args, _context| {
            if let Some(callback) = args.get(0).cloned() {
                if let Some(obj) = callback.as_object() {
                    if obj.is_callable() {
                        MACRO_TASKS.with(|tasks| {
                            tasks.borrow_mut().push_back(Box::new(move |ctx| {
                                let _ = obj.call(&JsValue::undefined(), &[], ctx);
                            }));
                        });
                    }
                }
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_queue_task"), 1, queue_task).unwrap();

        // Register native __aura_resolve_url
        let resolve_url = NativeFunction::from_copy_closure(|_this, args, _context| {
            let input = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let base = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let resolved = if !base.is_empty() {
                Url::parse(&base)
                    .ok()
                    .and_then(|b| b.join(&input).ok())
                    .or_else(|| Url::parse(&input).ok())
            } else {
                Url::parse(&input).ok()
            };
            let out = resolved.map(|u| u.to_string()).unwrap_or(input);
            Ok(JsValue::from(js_string!(out)))
        });
        context.register_global_callable(js_string!("__aura_resolve_url"), 2, resolve_url).unwrap();

        // Register native __aura_fetch
        let aura_fetch = NativeFunction::from_copy_closure(|_this, args, _context| {
            let url_str = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let resolve = args.get(1).cloned().unwrap_or(JsValue::undefined());
            let reject = args.get(2).cloned().unwrap_or(JsValue::undefined());

            let base_url = CURRENT_ORIGIN.with(|o| (*o.borrow()).clone());
            let target_url = if let Some(base) = base_url.as_ref() {
                base.join(&url_str).unwrap_or_else(|_| Url::parse(&url_str).unwrap_or(base.clone()))
            } else {
                Url::parse(&url_str).unwrap_or_else(|_| Url::parse("about:blank").unwrap())
            };

            // CSP Check
            let allowed = CSP_POLICY.with(|p| {
                if let Some(ref policy) = *p.borrow() {
                    policy.is_allowed("connect-src", &target_url, base_url.as_ref())
                } else {
                    true
                }
            });

            if !allowed {
                if let Some(obj) = reject.as_object() {
                    let _ = obj.call(&JsValue::undefined(), &[JsValue::from(js_string!("CSP Error: connect-src directive blocked this request"))], _context);
                }
                return Ok(JsValue::undefined());
            }

            let fetch_id = NEXT_FETCH_ID.with(|id_cell| {
                let id = *id_cell.borrow();
                *id_cell.borrow_mut() += 1;
                id
            });

            FETCH_REGISTRY.with(|reg| reg.borrow_mut().insert(fetch_id, (resolve, reject)));

            TASK_SENDER.with(|s_cell| {
                if let Some(ref sender) = *s_cell.borrow() {
                    let sender_clone = sender.clone();
                    let origin_str = base_url.as_ref().map(|u| u.origin().unicode_serialization()).unwrap_or_else(|| "null".to_string());
                    let is_cross_origin = base_url.as_ref().map(|u| u.origin() != target_url.origin()).unwrap_or(false);

                    std::thread::spawn(move || {
                        let client = reqwest::blocking::Client::new();
                        let mut req = client.get(target_url.clone());
                        if is_cross_origin {
                            req = req.header("Origin", origin_str.clone());
                        }

                        let res = req.send();
                        match res {
                            Ok(response) => {
                                // CORS Check
                                if is_cross_origin {
                                    let acao = response.headers().get("access-control-allow-origin")
                                        .and_then(|h| h.to_str().ok());
                                    
                                    let allowed = match acao {
                                        Some("*") => true,
                                        Some(val) if val == origin_str => true,
                                        _ => false,
                                    };

                                    if !allowed {
                                        let _ = sender_clone.send(Box::new(move |ctx| {
                                            let (_, reject) = FETCH_REGISTRY.with(|reg| reg.borrow_mut().remove(&fetch_id).unwrap());
                                            if let Some(obj) = reject.as_object() {
                                                let _ = obj.call(&JsValue::undefined(), &[JsValue::from(js_string!("CORS Error: Origin not allowed"))], ctx);
                                            }
                                        }));
                                        return;
                                    }
                                }

                                let body = response.text().unwrap_or_default();
                                let _ = sender_clone.send(Box::new(move |ctx| {
                                    let (resolve, _) = FETCH_REGISTRY.with(|reg| reg.borrow_mut().remove(&fetch_id).unwrap());
                                    FETCH_BODY_REGISTRY.with(|reg| reg.borrow_mut().insert(fetch_id, body));
                                    
                                    if let Some(obj) = resolve.as_object() {
                                        let res_obj_val = ctx.eval(Source::from_bytes(b"({})")).unwrap();
                                        let res_obj = res_obj_val.as_object().unwrap();
                                        
                                        let text_fn = NativeFunction::from_copy_closure(move |_, _, _| {
                                            let body = FETCH_BODY_REGISTRY.with(|reg| reg.borrow_mut().remove(&fetch_id).unwrap_or_default());
                                            Ok(JsValue::from(js_string!(body)))
                                        });
                                        
                                        ctx.register_global_callable(js_string!("__aura_temp_text"), 0, text_fn).unwrap();
                                        let text_fn_obj = ctx.eval(Source::from_bytes(b"__aura_temp_text")).unwrap();
                                        let _ = ctx.eval(Source::from_bytes(b"delete globalThis.__aura_temp_text"));
                                        
                                        res_obj.set(js_string!("text"), text_fn_obj, false, ctx).unwrap();
                                        let _ = obj.call(&JsValue::undefined(), &[res_obj_val], ctx);
                                    }
                                }));
                            }
                            Err(e) => {
                                let err_msg = e.to_string();
                                let _ = sender_clone.send(Box::new(move |ctx| {
                                    let (_, reject) = FETCH_REGISTRY.with(|reg| reg.borrow_mut().remove(&fetch_id).unwrap());
                                    if let Some(obj) = reject.as_object() {
                                        let _ = obj.call(&JsValue::undefined(), &[JsValue::from(js_string!(err_msg))], ctx);
                                    }
                                }));
                            }
                        }
                    });
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_fetch"), 3, aura_fetch).unwrap();

        // ── DOM Query APIs ────────────────────────────────────────────────────

        // __aura_query_selector(root_nid_or_0, selector_str) → nid | null
        let query_selector = NativeFunction::from_copy_closure(|_this, args, _context| {
            let root_nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let selector = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let found = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    let search_root = if root_nid == 0 {
                        r.clone()
                    } else {
                        NODE_REGISTRY.with(|reg| reg.borrow().get(&root_nid).cloned()).unwrap_or_else(|| r.clone())
                    };
                    query_selector_first(&search_root, &selector, root_nid != 0)
                } else { None }
            });
            Ok(found.map(|h| JsValue::from(register_node(h))).unwrap_or(JsValue::null()))
        });
        context.register_global_callable(js_string!("__aura_query_selector"), 2, query_selector).unwrap();

        // __aura_query_selector_all(root_nid_or_0, selector_str) → JSON array of nids
        let query_selector_all = NativeFunction::from_copy_closure(|_this, args, _context| {
            let root_nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let selector = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let nids = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    let search_root = if root_nid == 0 {
                        r.clone()
                    } else {
                        NODE_REGISTRY.with(|reg| reg.borrow().get(&root_nid).cloned()).unwrap_or_else(|| r.clone())
                    };
                    query_selector_all_nodes(&search_root, &selector, root_nid != 0)
                } else { vec![] }
            });
            let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
            let json = format!("[{}]", ids.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","));
            Ok(JsValue::from(js_string!(json)))
        });
        context.register_global_callable(js_string!("__aura_query_selector_all"), 2, query_selector_all).unwrap();

        // __aura_get_elements_by_class(root_nid_or_0, class_name) → JSON array of nids
        let get_by_class = NativeFunction::from_copy_closure(|_this, args, _context| {
            let root_nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let cls = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let nids = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    let search_root = if root_nid == 0 {
                        r.clone()
                    } else {
                        NODE_REGISTRY.with(|reg| reg.borrow().get(&root_nid).cloned()).unwrap_or_else(|| r.clone())
                    };
                    find_elements_by_class(&search_root, &cls, root_nid != 0)
                } else { vec![] }
            });
            let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
            let json = format!("[{}]", ids.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","));
            Ok(JsValue::from(js_string!(json)))
        });
        context.register_global_callable(js_string!("__aura_get_elements_by_class"), 2, get_by_class).unwrap();

        // __aura_get_elements_by_tag(root_nid_or_0, tag_name) → JSON array of nids
        let get_by_tag = NativeFunction::from_copy_closure(|_this, args, _context| {
            let root_nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let tag = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default().to_lowercase();
            let nids = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    let search_root = if root_nid == 0 {
                        r.clone()
                    } else {
                        NODE_REGISTRY.with(|reg| reg.borrow().get(&root_nid).cloned()).unwrap_or_else(|| r.clone())
                    };
                    find_elements_by_tag_name(&search_root, &tag, root_nid != 0)
                } else { vec![] }
            });
            let ids: Vec<u32> = nids.into_iter().map(register_node).collect();
            let json = format!("[{}]", ids.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(","));
            Ok(JsValue::from(js_string!(json)))
        });
        context.register_global_callable(js_string!("__aura_get_elements_by_tag"), 2, get_by_tag).unwrap();

        // ── DOM Mutation APIs ─────────────────────────────────────────────────

        // __aura_create_element(tag) → nid
        let create_element = NativeFunction::from_copy_closure(|_this, args, _context| {
            let tag = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_else(|| "div".to_string()).to_lowercase();
            let nid = DOM_ROOT.with(|root| {
                use html5ever::{QualName, LocalName, ns};
                use markup5ever_rcdom::{Node, NodeData};
                let new_node = Node::new(NodeData::Element {
                    name: QualName::new(None, ns!(html), LocalName::from(tag)),
                    attrs: std::cell::RefCell::new(vec![]),
                    template_contents: std::cell::RefCell::new(None),
                    mathml_annotation_xml_integration_point: false,
                });
                // Store it even without a DOM parent so it can be used
                let nid = register_node(new_node.clone());
                // If there's a DOM root, we register it but don't attach yet
                let _ = root; // suppress warning
                nid
            });
            Ok(JsValue::from(nid))
        });
        context.register_global_callable(js_string!("__aura_create_element"), 1, create_element).unwrap();

        // __aura_create_text_node(text) -> nid
        let create_text_node = NativeFunction::from_copy_closure(|_this, args, _context| {
            use html5ever::tendril::StrTendril;
            use markup5ever_rcdom::{Node, NodeData};

            let text = args.get(0)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let node = Node::new(NodeData::Text {
                contents: std::cell::RefCell::new(StrTendril::from(text.as_str())),
            });
            Ok(JsValue::from(register_node(node)))
        });
        context.register_global_callable(js_string!("__aura_create_text_node"), 1, create_text_node).unwrap();

        // __aura_append_child(parent_nid, child_nid) → void
        let append_child = NativeFunction::from_copy_closure(|_this, args, _context| {
            let parent_nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let child_nid = args.get(1).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let (Some(parent), Some(child)) = (reg.get(&parent_nid), reg.get(&child_nid)) {
                    // Remove from existing parent if any
                    let child_ptr = Rc::as_ptr(child) as usize;
                    if let Some(old_parent_handle) = find_parent_by_ptr_in_dom(child_ptr) {
                        old_parent_handle.children.borrow_mut().retain(|c| Rc::as_ptr(c) as usize != child_ptr);
                    }
                    // Set new parent relationship via Cell::set
                    child.parent.set(Some(Rc::downgrade(parent)));
                    parent.children.borrow_mut().push(child.clone());
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_append_child"), 2, append_child).unwrap();

        // __aura_remove_child(parent_nid, child_nid) → void
        let remove_child = NativeFunction::from_copy_closure(|_this, args, _context| {
            let parent_nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let child_nid = args.get(1).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let (Some(parent), Some(child)) = (reg.get(&parent_nid), reg.get(&child_nid)) {
                    let child_ptr = Rc::as_ptr(child) as usize;
                    parent.children.borrow_mut().retain(|c| Rc::as_ptr(c) as usize != child_ptr);
                    child.parent.set(None);
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_remove_child"), 2, remove_child).unwrap();

        // __aura_insert_before(parent_nid, new_child_nid, ref_nid_or_null) → void
        let insert_before = NativeFunction::from_copy_closure(|_this, args, _context| {
            let parent_nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let new_nid = args.get(1).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let ref_nid = args.get(2).and_then(|v| if v.is_null() { None } else { v.as_number().map(|n| n as u32) });
            NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let (Some(parent), Some(new_child)) = (reg.get(&parent_nid), reg.get(&new_nid)) {
                    // Remove from existing parent
                    let new_ptr = Rc::as_ptr(new_child) as usize;
                    if let Some(old_parent) = find_parent_by_ptr_in_dom(new_ptr) {
                        old_parent.children.borrow_mut().retain(|c| Rc::as_ptr(c) as usize != new_ptr);
                    }
                    new_child.parent.set(Some(Rc::downgrade(parent)));

                    let mut children = parent.children.borrow_mut();
                    if let Some(ref_nid_val) = ref_nid {
                        if let Some(ref_node) = reg.get(&ref_nid_val) {
                            let ref_ptr = Rc::as_ptr(ref_node) as usize;
                            if let Some(pos) = children.iter().position(|c| Rc::as_ptr(c) as usize == ref_ptr) {
                                children.insert(pos, new_child.clone());
                                return;
                            }
                        }
                    }
                    children.push(new_child.clone());
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_insert_before"), 3, insert_before).unwrap();

        // __aura_remove_self(nid) → void
        let remove_self = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    let node_ptr = Rc::as_ptr(node) as usize;
                    // Get parent via Cell::take/set pattern
                    let parent_weak = node.parent.take();
                    if let Some(ref pw) = parent_weak {
                        if let Some(parent) = pw.upgrade() {
                            parent.children.borrow_mut().retain(|c| Rc::as_ptr(c) as usize != node_ptr);
                        }
                    }
                    // parent_weak was taken (None now), leave it as None
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_remove_self"), 1, remove_self).unwrap();

        // __aura_get_inner_html(nid) → string
        let get_inner_html = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let html = NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    serialize_inner_html(node)
                } else {
                    String::new()
                }
            });
            Ok(JsValue::from(js_string!(html)))
        });
        context.register_global_callable(js_string!("__aura_get_inner_html"), 1, get_inner_html).unwrap();

        // __aura_set_inner_html(nid, html_str) → void
        let set_inner_html = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let html_str = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    // Determine context tag for fragment parsing
                    let ctx_tag = if let NodeData::Element { ref name, .. } = node.data {
                        name.local.to_string()
                    } else { "div".to_string() };
                    let fragment_nodes = parse_html_fragment(&html_str, &ctx_tag);
                    // Replace children
                    let mut children = node.children.borrow_mut();
                    children.clear();
                    for child in fragment_nodes {
                        child.parent.set(Some(Rc::downgrade(node)));
                        children.push(child);
                    }
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_set_inner_html"), 2, set_inner_html).unwrap();

        // __aura_get_text_content(nid) → string
        let get_text_content = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let text = NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    collect_text_content(node)
                } else {
                    String::new()
                }
            });
            Ok(JsValue::from(js_string!(text)))
        });
        context.register_global_callable(js_string!("__aura_get_text_content"), 1, get_text_content).unwrap();

        // __aura_set_text_content(nid, text) → void
        let set_text_content = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let text = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    match &node.data {
                        NodeData::Text { contents } => {
                            *contents.borrow_mut() = text.as_str().into();
                        }
                        _ => {
                            use html5ever::tendril::StrTendril;
                            use markup5ever_rcdom::Node;

                            let text_node = Node::new(NodeData::Text {
                                contents: std::cell::RefCell::new(StrTendril::from(text.as_str())),
                            });
                            text_node.parent.set(Some(Rc::downgrade(node)));
                            let mut children = node.children.borrow_mut();
                            children.clear();
                            children.push(text_node);
                        }
                    }
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_set_text_content"), 2, set_text_content).unwrap();

        // __aura_get_attribute(nid, name) → string | null
        let get_attr = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let name = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let val = NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    if let NodeData::Element { ref attrs, .. } = node.data {
                        for attr in attrs.borrow().iter() {
                            if attr.name.local.to_string() == name {
                                return Some(attr.value.to_string());
                            }
                        }
                    }
                }
                None
            });
            Ok(val.map(|v| JsValue::from(js_string!(v))).unwrap_or(JsValue::null()))
        });
        context.register_global_callable(js_string!("__aura_get_attribute"), 2, get_attr).unwrap();

        // __aura_remove_attribute(nid, name) → void
        let remove_attr = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let name = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            NODE_REGISTRY.with(|reg| {
                if let Some(node) = reg.borrow().get(&nid) {
                    if let NodeData::Element { ref attrs, .. } = node.data {
                        attrs.borrow_mut().retain(|a| a.name.local.to_string() != name);
                    }
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_remove_attribute"), 2, remove_attr).unwrap();

        // __aura_has_attribute(nid, name) → bool
        let has_attr = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let name = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let found = NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    if let NodeData::Element { ref attrs, .. } = node.data {
                        return attrs.borrow().iter().any(|a| a.name.local.to_string() == name);
                    }
                }
                false
            });
            Ok(JsValue::from(found))
        });
        context.register_global_callable(js_string!("__aura_has_attribute"), 2, has_attr).unwrap();

        // __aura_get_children_nids(nid) → JSON array of {nid, tag, string_id}
        let get_children = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let result = NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    let mut items = Vec::new();
                    for child in node.children.borrow().iter() {
                        let child_nid = register_node(child.clone());
                        match &child.data {
                            NodeData::Element { ref name, ref attrs, .. } => {
                                let tag = name.local.to_string();
                                let id_attr = attrs.borrow().iter()
                                    .find(|a| a.name.local.to_string() == "id")
                                    .map(|a| a.value.to_string())
                                    .unwrap_or_default();
                                items.push(format!("{{\"nid\":{},\"tag\":\"{}\",\"id\":\"{}\",\"kind\":\"element\"}}", child_nid, tag, id_attr));
                            }
                            NodeData::Text { .. } => {
                                items.push(format!("{{\"nid\":{},\"tag\":\"\",\"id\":\"\",\"kind\":\"text\"}}", child_nid));
                            }
                            _ => {}
                        }
                    }
                    items.join(",")
                } else {
                    String::new()
                }
            });
            Ok(JsValue::from(js_string!(format!("[{}]", result))))
        });
        context.register_global_callable(js_string!("__aura_get_children"), 1, get_children).unwrap();

        // __aura_get_node_info(nid) → {tag, id, class} or null
        let get_node_info = NativeFunction::from_copy_closure(|_this, args, context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let info = NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let Some(node) = reg.get(&nid) {
                    if let NodeData::Element { ref name, ref attrs, .. } = node.data {
                        let tag = name.local.to_string();
                        let attrs_b = attrs.borrow();
                        let id = attrs_b.iter().find(|a| a.name.local.to_string() == "id").map(|a| a.value.to_string()).unwrap_or_default();
                        let class = attrs_b.iter().find(|a| a.name.local.to_string() == "class").map(|a| a.value.to_string()).unwrap_or_default();
                        return Some((tag, id, class, "element".to_string()));
                    } else if let NodeData::Text { .. } = node.data {
                        return Some((String::new(), String::new(), String::new(), "text".to_string()));
                    }
                }
                None
            });
            if let Some((tag, id, class, kind)) = info {
                use boa_engine::object::ObjectInitializer;
                let mut obj = ObjectInitializer::new(context);
                obj.property(js_string!("tag"), JsValue::from(js_string!(tag)), boa_engine::property::Attribute::all());
                obj.property(js_string!("id"), JsValue::from(js_string!(id)), boa_engine::property::Attribute::all());
                obj.property(js_string!("class"), JsValue::from(js_string!(class)), boa_engine::property::Attribute::all());
                obj.property(js_string!("kind"), JsValue::from(js_string!(kind)), boa_engine::property::Attribute::all());
                Ok(obj.build().into())
            } else {
                Ok(JsValue::null())
            }
        });
        context.register_global_callable(js_string!("__aura_get_node_info"), 1, get_node_info).unwrap();

        // __aura_get_node_type(nid) -> DOM nodeType integer
        let get_node_type = NativeFunction::from_copy_closure(|_this, args, _context| {
            let nid = args.get(0).and_then(|v| v.as_number()).map(|n| n as u32).unwrap_or(0);
            let node_type = NODE_REGISTRY.with(|reg| {
                reg.borrow().get(&nid).map(|node| match node.data {
                    NodeData::Element { .. } => 1,
                    NodeData::Text { .. } => 3,
                    NodeData::Comment { .. } => 8,
                    NodeData::Document => 9,
                    _ => 0,
                }).unwrap_or(0)
            });
            Ok(JsValue::from(node_type))
        });
        context.register_global_callable(js_string!("__aura_get_node_type"), 1, get_node_type).unwrap();

        // __aura_parse_url(url, base) -> object|null
        let parse_url = NativeFunction::from_copy_closure(|_this, args, context| {
            let url_input = args.get(0)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let base_input = args.get(1)
                .and_then(|v| v.as_string())
                .map(|s| s.to_std_string_escaped());

            let parsed = if let Some(base) = base_input.as_deref() {
                Url::parse(base).ok().and_then(|base_url| base_url.join(&url_input).ok())
            } else {
                Url::parse(&url_input).ok()
            };

            if let Some(url) = parsed {
                use boa_engine::object::ObjectInitializer;
                let href = url.to_string();
                let hostname = url.host_str().unwrap_or("").to_string();
                let pathname = url.path().to_string();
                let search = url.query().map(|q| format!("?{}", q)).unwrap_or_default();
                let hash = url.fragment().map(|f| format!("#{}", f)).unwrap_or_default();
                let protocol = format!("{}:", url.scheme());
                let host = url.host_str().map(|h| {
                    if let Some(port) = url.port() {
                        format!("{}:{}", h, port)
                    } else {
                        h.to_string()
                    }
                }).unwrap_or_default();
                let port = url.port().map(|p| p.to_string()).unwrap_or_default();
                let origin = url.origin().unicode_serialization();

                let mut obj = ObjectInitializer::new(context);
                obj.property(js_string!("href"), JsValue::from(js_string!(href)), boa_engine::property::Attribute::all());
                obj.property(js_string!("hostname"), JsValue::from(js_string!(hostname)), boa_engine::property::Attribute::all());
                obj.property(js_string!("pathname"), JsValue::from(js_string!(pathname)), boa_engine::property::Attribute::all());
                obj.property(js_string!("search"), JsValue::from(js_string!(search)), boa_engine::property::Attribute::all());
                obj.property(js_string!("hash"), JsValue::from(js_string!(hash)), boa_engine::property::Attribute::all());
                obj.property(js_string!("protocol"), JsValue::from(js_string!(protocol)), boa_engine::property::Attribute::all());
                obj.property(js_string!("host"), JsValue::from(js_string!(host)), boa_engine::property::Attribute::all());
                obj.property(js_string!("port"), JsValue::from(js_string!(port)), boa_engine::property::Attribute::all());
                obj.property(js_string!("origin"), JsValue::from(js_string!(origin)), boa_engine::property::Attribute::all());
                Ok(obj.build().into())
            } else {
                Ok(JsValue::null())
            }
        });
        context.register_global_callable(js_string!("__aura_parse_url"), 2, parse_url).unwrap();

        // Load bootstrap
        let bootstrap = include_str!("js_bootstrap.js");
        let _ = context.eval(Source::from_bytes(bootstrap.as_bytes()));

        // Inject the base URL into the JS location and document.location objects
        let url_init = CURRENT_ORIGIN.with(|origin| {
            if let Some(ref url) = *origin.borrow() {
                let href = url.to_string();
                let hostname = url.host_str().unwrap_or("").to_string();
                let pathname = url.path().to_string();
                let search = url.query().map(|q| format!("?{}", q)).unwrap_or_default();
                let hash = url.fragment().map(|f| format!("#{}", f)).unwrap_or_default();
                let protocol = url.scheme().to_string() + ":";
                let host = url.host_str().map(|h| {
                    if let Some(port) = url.port() {
                        format!("{}:{}", h, port)
                    } else {
                        h.to_string()
                    }
                }).unwrap_or_default();
                let port = url.port().map(|p| p.to_string()).unwrap_or_default();
                let origin = url.origin().unicode_serialization();
                format!(
                    r#"(function() {{
                        var _loc = {{
                            href: {href:?},
                            hostname: {hostname:?},
                            pathname: {pathname:?},
                            search: {search:?},
                            hash: {hash:?},
                            protocol: {protocol:?},
                            host: {host:?},
                            port: {port:?},
                            origin: {origin:?},
                        }};
                        document.location = _loc;
                        document.URL = {href:?};
                        document.documentURI = {href:?};
                        document.baseURI = {href:?};
                        window.location = _loc;
                        location = _loc;
                    }})();"#
                )
            } else {
                String::new()
            }
        });
        if !url_init.is_empty() {
            if let Err(e) = context.eval(Source::from_bytes(url_init.as_bytes())) {
                println!("[JS Bootstrap] URL init error: {:?}", e);
            }
        }

        Self { context, task_receiver }
    }

    pub fn tick(&mut self, timestamp: Option<f64>, deadline_ms: Option<f64>) -> bool {
        let mut did_work = false;

        // 0. Synchronize focus and dispatch events
        if self.sync_focus() {
            did_work = true;
        }

        // 1. Drain task_receiver into MACRO_TASKS
        while let Ok(task) = self.task_receiver.try_recv() {
            MACRO_TASKS.with(|tasks| tasks.borrow_mut().push_back(task));
        }

        // 2. Process ONE macro task
        let macro_task = MACRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
        if let Some(task) = macro_task {
            task(&mut self.context);
            did_work = true;
        }

        // 3. Microtask checkpoint
        self.run_microtasks();

        // 4. Rendering step (rAF)
        if let Some(ts) = timestamp {
            let mut tasks = VecDeque::new();
            RAF_TASKS.with(|t| tasks = t.borrow_mut().drain(..).collect());

            if !tasks.is_empty() {
                did_work = true;
                for task in tasks {
                    task(&mut self.context, ts);
                    self.run_microtasks();
                }
            }
        }

        // 5. Idle tasks
        if let Some(deadline) = deadline_ms {
            loop {
                let task_opt = IDLE_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
                if let Some((_, task)) = task_opt {
                    did_work = true;
                    task(&mut self.context, deadline);
                    self.run_microtasks();
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64;
                    if now >= deadline { break; }
                } else {
                    break;
                }
            }
        }

        did_work
    }

    fn run_microtasks(&mut self) {
        loop {
            let mut micro_work_done = false;
            while let Some(task) = MICRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front()) {
                task(&mut self.context);
                micro_work_done = true;
            }
            let _ = self.context.run_jobs();
            if !micro_work_done { break; }
        }
    }

    pub fn trigger_event(&mut self, target_id: &str, event_type: &str) {
        let native_id = DOM_ROOT.with(|root| {
            if let Some(ref r) = *root.borrow() {
                find_element_by_id(r, target_id).map(register_node)
            } else { None }
        });

        if let Some(nid) = native_id {
            let event_type = event_type.to_string();
            MACRO_TASKS.with(|tasks| {
                tasks.borrow_mut().push_back(Box::new(move |ctx| {
                    let code = format!("document.__trigger_event({}, '{}', {{ bubbles: true }})", nid, event_type);
                    let _ = ctx.eval(Source::from_bytes(code.as_bytes()));
                }));
            });
        }
    }

    pub fn execute(&mut self, source: &str) {
        let outcome = self.execute_with_result(source);
        if let Some(error) = outcome.error {
            println!("[JS Error] execute: {}", error);
        }
    }

    pub fn execute_with_result(&mut self, source: &str) -> EvalOutcome {
        if source.contains("import.meta") || (source.contains("import ") && source.contains(" from ")) {
            return EvalOutcome {
                result: None,
                error: Some("ES module syntax is not supported".to_string()),
            };
        }

        let outcome = match self.context.eval(Source::from_bytes(source.as_bytes())) {
            Ok(value) => {
                let result = if value.is_undefined() {
                    Some("undefined".to_string())
                } else if value.is_null() {
                    Some("null".to_string())
                } else {
                    value
                        .to_string(&mut self.context)
                        .ok()
                        .map(|s| s.to_std_string_escaped())
                        .or_else(|| Some(String::new()))
                };
                EvalOutcome { result, error: None }
            }
            Err(error) => {
                let message = error.to_string();
                EvalOutcome {
                    result: None,
                    error: Some(if message.is_empty() {
                        format!("{:?}", error)
                    } else {
                        message
                    }),
                }
            }
        };
        self.run_microtasks();
        outcome
    }

    pub fn get_style_overrides(&mut self) -> HashMap<String, HashMap<String, String>> {
        let mut result = HashMap::new();
        if let Ok(val) = self.context.eval(Source::from_bytes(b"__aura_style_log.join('####')")) {
            if let Ok(s) = val.to_string(&mut self.context) {
                let s_std = s.to_std_string_escaped();
                for entry in s_std.split("####") {
                    let parts: Vec<&str> = entry.splitn(3, "||||").collect();
                    if parts.len() == 3 && !parts[0].is_empty() {
                        result.entry(parts[0].to_string()).or_insert_with(HashMap::new)
                            .insert(parts[1].to_string(), parts[2].to_string());
                    }
                }
            }
        }
        let _ = self.context.eval(Source::from_bytes(b"__aura_style_log = [];"));
        result
    }

    pub fn get_focused_node_id(&self) -> Option<String> {
        FOCUSED_NODE.with(|f| (*f.borrow()).clone())
    }

    pub fn set_focused_node_id(&mut self, id: Option<String>) {
        FOCUSED_NODE.with(|f| *f.borrow_mut() = id);
    }

    pub fn sync_focus(&mut self) -> bool {
        let (old, new) = (
            PREVIOUS_FOCUSED_NODE.with(|f| (*f.borrow()).clone()),
            FOCUSED_NODE.with(|f| (*f.borrow()).clone())
        );

        if old == new { return false; }

        PREVIOUS_FOCUSED_NODE.with(|f| *f.borrow_mut() = new.clone());

        // Blur old element
        if let Some(id) = old {
            let native_id = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    find_element_by_id(r, &id).map(register_node)
                } else { None }
            });
            if let Some(nid) = native_id {
                let code = format!("
                    (function() {{
                        let el = document.getElementById('{}') || __get_or_create_node({});
                        if (el) {{
                            el.dispatchEvent(new Event('blur', {{ bubbles: false }}));
                            el.dispatchEvent(new Event('focusout', {{ bubbles: true }}));
                        }}
                    }})();
                ", id, nid);
                let _ = self.context.eval(Source::from_bytes(code.as_bytes()));
                self.run_microtasks();
            }
        }

        // Focus new element
        if let Some(id) = new {
            let native_id = DOM_ROOT.with(|root| {
                if let Some(ref r) = *root.borrow() {
                    find_element_by_id(r, &id).map(register_node)
                } else { None }
            });
            if let Some(nid) = native_id {
                let code = format!("
                    (function() {{
                        let el = document.getElementById('{}') || __get_or_create_node({});
                        if (el) {{
                            document.activeElement = el;
                            el.dispatchEvent(new Event('focus', {{ bubbles: false }}));
                            el.dispatchEvent(new Event('focusin', {{ bubbles: true }}));
                        }}
                    }})();
                ", id, nid);
                let _ = self.context.eval(Source::from_bytes(code.as_bytes()));
                self.run_microtasks();
            }
        } else {
            let _ = self.context.eval(Source::from_bytes(b"document.activeElement = document.body;"));
            self.run_microtasks();
        }

        true
    }
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

fn find_element_by_tag(root: &Handle, tag: &str) -> Option<Handle> {
    if let NodeData::Element { ref name, .. } = root.data {
        if name.local.to_string() == tag {
            return Some(root.clone());
        }
    }
    for child in root.children.borrow().iter() {
        if let Some(found) = find_element_by_tag(child, tag) { return Some(found); }
    }
    None
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

fn find_parent_of_node(root: &Handle, target_nid: u32) -> Option<Handle> {
    for child in root.children.borrow().iter() {
        // Check if any child matches the target_nid (needs to be registered first to check nid)
        // Wait, checking by NID is tricky because we might not have it registered.
        // Actually, we can check by pointer equality if we had the handle.
        // But the JS side only gives us NID.
        
        let child_nid = NODE_REGISTRY.with(|reg| {
            for (nid, handle) in reg.borrow().iter() {
                if Rc::ptr_eq(handle, child) { return Some(*nid); }
            }
            None
        });
        
        if let Some(nid) = child_nid {
            if nid == target_nid { return Some(root.clone()); }
        }
        
        if let Some(found) = find_parent_of_node(child, target_nid) { return Some(found); }
    }
    None
}

use std::rc::Rc;

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

// ── CSS Selector Matching ─────────────────────────────────────────────────────

/// Simple CSS selector parser: supports tag, #id, .class, and combinations
/// e.g. "div", "#foo", ".bar", "div.foo", "div#id", ".a.b"
/// Also supports comma-separated selectors: "h1, h2, h3"
fn selector_matches(node: &Handle, selector: &str) -> bool {
    // Comma-separated: match any
    if selector.contains(',') {
        return selector.split(',').any(|s| selector_matches(node, s.trim()));
    }

    if let NodeData::Element { ref name, ref attrs, .. } = node.data {
        let tag = name.local.to_string().to_lowercase();
        let attrs_b = attrs.borrow();
        let id_val = attrs_b.iter().find(|a| a.name.local.to_string() == "id").map(|a| a.value.to_string()).unwrap_or_default();
        let class_val = attrs_b.iter().find(|a| a.name.local.to_string() == "class").map(|a| a.value.to_string()).unwrap_or_default();
        let classes: Vec<&str> = class_val.split_whitespace().collect();

        // Parse compound selector (no spaces — those would be descendant combinators)
        // We handle simple compound selectors: tag.class#id combinations
        let sel = selector.trim();

        // Check for attribute selectors like [type="submit"] - simplified
        if sel.contains('[') {
            return selector_matches_with_attr(node, sel);
        }

        // Split into parts by '#' and '.'
        // Strategy: extract optional tag prefix, then check all id/class parts
        let mut remaining = sel;
        let mut required_tag: Option<&str> = None;

        // Extract tag prefix (before any '.' or '#')
        let prefix_end = remaining.find(|c| c == '.' || c == '#').unwrap_or(remaining.len());
        if prefix_end > 0 {
            let prefix = &remaining[..prefix_end];
            if prefix != "*" {
                required_tag = Some(prefix);
            }
            remaining = &remaining[prefix_end..];
        }

        if let Some(t) = required_tag {
            if tag != t.to_lowercase() {
                return false;
            }
        }

        // Check remaining .class and #id parts
        while !remaining.is_empty() {
            if remaining.starts_with('#') {
                let end = remaining[1..].find(|c| c == '.' || c == '#').map(|p| p + 1).unwrap_or(remaining.len());
                let required_id = &remaining[1..end];
                if id_val != required_id {
                    return false;
                }
                remaining = &remaining[end..];
            } else if remaining.starts_with('.') {
                let end = remaining[1..].find(|c| c == '.' || c == '#').map(|p| p + 1).unwrap_or(remaining.len());
                let required_class = &remaining[1..end];
                if !classes.contains(&required_class) {
                    return false;
                }
                remaining = &remaining[end..];
            } else {
                break;
            }
        }
        true
    } else {
        false
    }
}

fn selector_matches_with_attr(node: &Handle, sel: &str) -> bool {
    if let NodeData::Element { ref name, ref attrs, .. } = node.data {
        let tag = name.local.to_string().to_lowercase();
        let attrs_b = attrs.borrow();

        // Extract tag part before '['
        let bracket_pos = sel.find('[').unwrap_or(sel.len());
        let tag_part = &sel[..bracket_pos];
        if !tag_part.is_empty() && tag_part != "*" && tag != tag_part.to_lowercase() {
            return false;
        }

        // Parse [attr=value] or [attr]
        let bracket_content = &sel[bracket_pos..];
        if let Some(inner) = bracket_content.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if inner.contains('=') {
                let mut parts = inner.splitn(2, '=');
                let attr_name = parts.next().unwrap_or("").trim().trim_matches('"');
                let attr_val = parts.next().unwrap_or("").trim().trim_matches('"');
                return attrs_b.iter().any(|a| a.name.local.to_string() == attr_name && a.value.to_string() == attr_val);
            } else {
                let attr_name = inner.trim();
                return attrs_b.iter().any(|a| a.name.local.to_string() == attr_name);
            }
        }
        false
    } else {
        false
    }
}

fn query_selector_first(root: &Handle, selector: &str, skip_root: bool) -> Option<Handle> {
    // Handle descendant combinator: "ancestor descendant"
    // We do a simple two-pass: find all ancestors, then find descendants
    let sel = selector.trim();

    // Check for descendant combinator (space not inside brackets)
    if let Some((ancestor_sel, desc_sel)) = split_descendant_selector(sel) {
        // Find all elements matching ancestor_sel, then search their subtrees
        let ancestors = query_selector_all_nodes(root, ancestor_sel, skip_root);
        for ancestor in ancestors {
            if let Some(found) = query_selector_first(&ancestor, desc_sel, true) {
                return Some(found);
            }
        }
        return None;
    }

    if !skip_root && selector_matches(root, sel) {
        return Some(root.clone());
    }
    for child in root.children.borrow().iter() {
        if let Some(found) = query_selector_first(child, sel, false) {
            return Some(found);
        }
    }
    None
}

fn query_selector_all_nodes(root: &Handle, selector: &str, skip_root: bool) -> Vec<Handle> {
    let mut results = Vec::new();
    let sel = selector.trim();

    if let Some((ancestor_sel, desc_sel)) = split_descendant_selector(sel) {
        let ancestors = query_selector_all_nodes(root, ancestor_sel, skip_root);
        for ancestor in ancestors {
            let mut desc = query_selector_all_nodes(&ancestor, desc_sel, true);
            results.append(&mut desc);
        }
        return results;
    }

    if !skip_root && selector_matches(root, sel) {
        results.push(root.clone());
    }
    for child in root.children.borrow().iter() {
        let mut found = query_selector_all_nodes(child, sel, false);
        results.append(&mut found);
    }
    results
}

/// Split "ancestor descendant" into (ancestor_sel, descendant_sel).
/// Returns None if there's no descendant combinator (space outside brackets/parens).
fn split_descendant_selector(sel: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let bytes = sel.as_bytes();
    let mut last_space: Option<usize> = None;

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b' ' if depth == 0 => {
                last_space = Some(i);
            }
            _ => {}
        }
    }

    if let Some(pos) = last_space {
        let ancestor = sel[..pos].trim_end();
        let desc = sel[pos + 1..].trim_start();
        if !ancestor.is_empty() && !desc.is_empty() {
            return Some((ancestor, desc));
        }
    }
    None
}

fn find_elements_by_class(root: &Handle, cls: &str, skip_root: bool) -> Vec<Handle> {
    let mut results = Vec::new();
    if !skip_root {
        if let NodeData::Element { ref attrs, .. } = root.data {
            let class_val = attrs.borrow().iter()
                .find(|a| a.name.local.to_string() == "class")
                .map(|a| a.value.to_string())
                .unwrap_or_default();
            if class_val.split_whitespace().any(|c| c == cls) {
                results.push(root.clone());
            }
        }
    }
    for child in root.children.borrow().iter() {
        let mut found = find_elements_by_class(child, cls, false);
        results.append(&mut found);
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
        let mut found = find_elements_by_tag_name(child, tag, false);
        results.append(&mut found);
    }
    results
}

/// Find the parent of a node identified by its raw pointer, searching the DOM from root.
fn find_parent_by_ptr_in_dom(child_ptr: usize) -> Option<Handle> {
    DOM_ROOT.with(|root| {
        if let Some(ref r) = *root.borrow() {
            find_parent_by_ptr(r, child_ptr)
        } else {
            None
        }
    })
}

fn find_parent_by_ptr(node: &Handle, child_ptr: usize) -> Option<Handle> {
    for child in node.children.borrow().iter() {
        if Rc::as_ptr(child) as usize == child_ptr {
            return Some(node.clone());
        }
        if let Some(found) = find_parent_by_ptr(child, child_ptr) {
            return Some(found);
        }
    }
    None
}

/// Serialize inner HTML of a node (children only, not the node itself).
fn serialize_inner_html(node: &Handle) -> String {
    let mut out = String::new();
    for child in node.children.borrow().iter() {
        serialize_node(child, &mut out);
    }
    out
}

fn serialize_node(node: &Handle, out: &mut String) {
    match &node.data {
        NodeData::Text { ref contents } => {
            out.push_str(&html_escape(&contents.borrow().to_string()));
        }
        NodeData::Element { ref name, ref attrs, .. } => {
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
            // Self-closing void elements
            if !matches!(tag.as_str(), "area"|"base"|"br"|"col"|"embed"|"hr"|"img"|"input"|"link"|"meta"|"param"|"source"|"track"|"wbr") {
                for child in node.children.borrow().iter() {
                    serialize_node(child, out);
                }
                out.push_str("</");
                out.push_str(&tag);
                out.push('>');
            }
        }
        NodeData::Comment { ref contents } => {
            out.push_str("<!--");
            out.push_str(contents);
            out.push_str("-->");
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

/// Collect all text content (recursively) from a node.
fn collect_text_content(node: &Handle) -> String {
    let mut out = String::new();
    match &node.data {
        NodeData::Text { ref contents } => {
            out.push_str(&contents.borrow().to_string());
        }
        NodeData::Element { .. } => {
            for child in node.children.borrow().iter() {
                out.push_str(&collect_text_content(child));
            }
        }
        _ => {}
    }
    out
}

/// Parse an HTML fragment string into a vec of Handle nodes.
/// Uses html5ever's fragment parsing.
fn parse_html_fragment(html: &str, _ctx_tag: &str) -> Vec<Handle> {
    use html5ever::parse_fragment;
    use html5ever::tendril::TendrilSink;
    use html5ever::{QualName, LocalName, ns};

    let ctx_name = QualName::new(None, ns!(html), LocalName::from(_ctx_tag));
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

    // The fragment parser puts the content as children of the context element,
    // which is the first child of the document.
    // IMPORTANT: We must extract the nodes from the DOM tree BEFORE `dom` is
    // dropped, because `Node::drop` calls `mem::take` on all descendants'
    // children to avoid deep stack recursion. If we just clone the handles
    // and let `dom` drop naturally, the extracted nodes would have their
    // children cleared by the drop implementation.
    // Solution: steal (take) the children out of the DOM tree before drop.
    let nodes = steal_fragment_children(&dom.document);
    // dom is dropped here but since we've already cleared the tree, the custom
    // Drop impl won't find any children to clear (they're now owned by `nodes`).
    nodes
}

/// Steal the fragment children from the DOM tree by removing them from their
/// parent nodes. This prevents `Node::drop` from clearing their children.
fn steal_fragment_children(doc: &Handle) -> Vec<Handle> {
    // Structure: document → [context_element] → [fragment nodes]
    // We need to take ownership of the fragment nodes out of context_element.
    for child in doc.children.borrow().iter() {
        if let NodeData::Element { .. } = child.data {
            // This is the context element. Take its children.
            let children: Vec<Handle> = child.children.borrow_mut().drain(..).collect();
            // Clear the parent pointers so they don't point into the dying dom
            for c in &children {
                c.parent.set(None);
            }
            return children;
        }
    }
    vec![]
}


// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;

    fn make_runtime(html: &str) -> JsRuntime {
        let dom = dom::parse_html(html);
        JsRuntime::new(Some(dom.document), None, None, new_console_buffer())
    }

    fn eval(rt: &mut JsRuntime, code: &str) -> String {
        if let Ok(val) = rt.context.eval(Source::from_bytes(code.as_bytes())) {
            if let Ok(s) = val.to_string(&mut rt.context) {
                return s.to_std_string_escaped();
            }
        }
        String::new()
    }

    #[test]
    fn test_console_methods_are_captured_with_levels() {
        let buffer = new_console_buffer();
        let dom = dom::parse_html("<html><body></body></html>");
        let mut rt = JsRuntime::new(Some(dom.document), None, None, buffer.clone());

        rt.execute("console.log('hello', 1); console.warn('careful'); console.error('boom');");

        let entries = console_entries(&buffer);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].level, ConsoleLevel::Log);
        assert_eq!(entries[0].message, "hello 1");
        assert_eq!(entries[1].level, ConsoleLevel::Warn);
        assert_eq!(entries[1].message, "careful");
        assert_eq!(entries[2].level, ConsoleLevel::Error);
        assert_eq!(entries[2].message, "boom");
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
    fn test_query_selector_h1() {
        let mut rt = make_runtime("<html><body><h1>Hello</h1></body></html>");
        let result = eval(&mut rt, "document.querySelector('h1') !== null ? 'found' : 'null'");
        assert_eq!(result, "found");
    }

    #[test]
    fn test_query_selector_returns_null_for_missing() {
        let mut rt = make_runtime("<html><body><p>text</p></body></html>");
        let result = eval(&mut rt, "document.querySelector('h1') === null ? 'null' : 'found'");
        assert_eq!(result, "null");
    }

    #[test]
    fn test_text_content_getter() {
        let mut rt = make_runtime("<html><body><h1>Hello World</h1></body></html>");
        let result = eval(&mut rt, "document.querySelector('h1').textContent");
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_get_element_by_id() {
        let mut rt = make_runtime("<html><body><div id='main'>content</div></body></html>");
        let result = eval(&mut rt, "document.getElementById('main') !== null ? 'found' : 'null'");
        assert_eq!(result, "found");
    }

    #[test]
    fn test_query_selector_all_count() {
        let mut rt = make_runtime("<html><body><p>a</p><p>b</p><p>c</p></body></html>");
        let result = eval(&mut rt, "document.querySelectorAll('p').length");
        assert_eq!(result, "3");
    }

    #[test]
    fn test_get_attribute() {
        let mut rt = make_runtime("<html><body><a href='/path' id='link1'>link</a></body></html>");
        let result = eval(&mut rt, "document.querySelector('a').getAttribute('href')");
        assert_eq!(result, "/path");
    }

    #[test]
    fn test_get_attribute_missing_returns_null() {
        let mut rt = make_runtime("<html><body><a>link</a></body></html>");
        let result = eval(&mut rt, "document.querySelector('a').getAttribute('href') === null ? 'null' : 'found'");
        assert_eq!(result, "null");
    }

    #[test]
    fn test_class_list_add_contains() {
        let mut rt = make_runtime("<html><body><div id='el'>x</div></body></html>");
        let result = eval(&mut rt,
            "var el = document.getElementById('el'); el.classList.add('active'); el.classList.contains('active')");
        assert_eq!(result, "true");
    }

    #[test]
    fn test_class_list_remove() {
        let mut rt = make_runtime("<html><body><div id='el' class='active foo'>x</div></body></html>");
        let result = eval(&mut rt,
            "var el = document.getElementById('el'); el.classList.remove('active'); el.classList.contains('active')");
        assert_eq!(result, "false");
    }

    #[test]
    fn test_class_list_toggle_adds() {
        let mut rt = make_runtime("<html><body><div id='el'>x</div></body></html>");
        let result = eval(&mut rt,
            "var el = document.getElementById('el'); el.classList.toggle('visible'); el.classList.contains('visible')");
        assert_eq!(result, "true");
    }

    #[test]
    fn test_class_list_toggle_removes() {
        let mut rt = make_runtime("<html><body><div id='el' class='visible'>x</div></body></html>");
        let result = eval(&mut rt,
            "var el = document.getElementById('el'); el.classList.toggle('visible'); el.classList.contains('visible')");
        assert_eq!(result, "false");
    }

    #[test]
    fn test_get_elements_by_class_name() {
        let mut rt = make_runtime("<html><body><div class='card'>a</div><div class='card'>b</div><p>c</p></body></html>");
        let result = eval(&mut rt, "document.getElementsByClassName('card').length");
        assert_eq!(result, "2");
    }

    #[test]
    fn test_get_elements_by_tag_name() {
        let mut rt = make_runtime("<html><body><span>a</span><span>b</span></body></html>");
        let result = eval(&mut rt, "document.getElementsByTagName('span').length");
        assert_eq!(result, "2");
    }

    #[test]
    fn test_add_event_listener_fires() {
        let mut rt = make_runtime("<html><body><button id='btn'>click</button></body></html>");
        eval(&mut rt, "var clicked = false; var btn = document.getElementById('btn'); btn.addEventListener('click', function() { clicked = true; });");
        eval(&mut rt, "btn.dispatchEvent(new Event('click'))");
        let result = eval(&mut rt, "clicked");
        assert_eq!(result, "true");
    }

    #[test]
    fn test_document_add_event_listener_domcontentloaded() {
        let mut rt = make_runtime("<html><body></body></html>");
        // DOMContentLoaded fires synchronously when addEventListener is called
        let result = eval(&mut rt,
            "var fired = false; document.addEventListener('DOMContentLoaded', function() { fired = true; }); fired");
        assert_eq!(result, "true");
    }

    #[test]
    fn test_selector_matches_class() {
        let mut rt = make_runtime("<html><body><div class='hero'>content</div></body></html>");
        let result = eval(&mut rt, "document.querySelector('.hero') !== null ? 'found' : 'null'");
        assert_eq!(result, "found");
    }

    #[test]
    fn test_selector_matches_id() {
        let mut rt = make_runtime("<html><body><section id='about'>text</section></body></html>");
        let result = eval(&mut rt, "document.querySelector('#about') !== null ? 'found' : 'null'");
        assert_eq!(result, "found");
    }

    #[test]
    fn test_inner_html_getter() {
        let mut rt = make_runtime("<html><body><div id='app'><p>hello</p></div></body></html>");
        let result = eval(&mut rt, "document.getElementById('app').innerHTML");
        assert!(result.contains("hello"), "Expected innerHTML to contain 'hello', got: {:?}", result);
    }

    #[test]
    fn test_parse_html_fragment_text_content() {
        // Test that parse_html_fragment preserves text content
        let nodes = parse_html_fragment("<p>hello world</p>", "div");
        assert_eq!(nodes.len(), 1, "Expected 1 fragment node");

        if let NodeData::Element { ref name, .. } = nodes[0].data {
            assert_eq!(name.local.to_string(), "p");
        }

        let text = collect_text_content(&nodes[0]);
        assert_eq!(text, "hello world", "Text content should be preserved after fragment parse");
    }

    #[test]
    fn test_inner_html_setter() {
        let mut rt = make_runtime("<html><body><div id='app'></div></body></html>");
        // Set innerHTML
        eval(&mut rt, "document.getElementById('app').innerHTML = '<p>hello</p>';");
        // Check via the getter that the p is now there
        let result = eval(&mut rt, "document.getElementById('app').innerHTML");
        assert!(result.contains("hello"), "Expected innerHTML to contain 'hello', got: {:?}", result);
        // Also check querySelector works after mutation
        let found = eval(&mut rt, "document.querySelector('#app p') !== null ? 'found' : 'null'");
        assert_eq!(found, "found", "Expected querySelector('#app p') to find element after innerHTML set");
    }

    // ── New runtime parity tests (#113) ──────────────────────────────────────

    #[test]
    fn test_window_inner_dimensions() {
        let mut rt = make_runtime("<html><body></body></html>");
        let w = eval(&mut rt, "window.innerWidth");
        let h = eval(&mut rt, "window.innerHeight");
        assert_eq!(w, "800");
        assert_eq!(h, "600");
    }

    #[test]
    fn test_navigator_user_agent_chrome() {
        let mut rt = make_runtime("<html><body></body></html>");
        let ua = eval(&mut rt, "navigator.userAgent");
        assert!(ua.contains("Mozilla"), "Expected Chrome-like UA, got: {}", ua);
        assert!(ua.contains("AppleWebKit"), "Expected WebKit UA, got: {}", ua);
    }

    #[test]
    fn test_navigator_platform() {
        let mut rt = make_runtime("<html><body></body></html>");
        let platform = eval(&mut rt, "navigator.platform");
        assert!(!platform.is_empty(), "Expected non-empty platform");
    }

    #[test]
    fn test_history_object() {
        let mut rt = make_runtime("<html><body></body></html>");
        let result = eval(&mut rt, "typeof window.history");
        assert_eq!(result, "object");
        let push = eval(&mut rt, "typeof window.history.pushState");
        assert_eq!(push, "function");
        // pushState should not throw
        let ok = eval(&mut rt, "(function() { try { history.pushState(null,'','/path'); return 'ok'; } catch(e) { return 'err:'+e; } })()");
        assert_eq!(ok, "ok");
    }

    #[test]
    fn test_history_push_and_replace_state_updates_location() {
        let url = url::Url::parse("https://example.com:8443/app/index.html").unwrap();
        let dom = crate::dom::parse_html("<html><body></body></html>");
        let mut rt = JsRuntime::new(Some(dom.document), Some(url), None, new_console_buffer());

        let result = eval(&mut rt, r#"
            (function() {
                history.pushState({page: 1}, '', '/dashboard?tab=home#top');
                var afterPush = [
                    history.length,
                    JSON.stringify(history.state),
                    location.href,
                    location.origin,
                    document.URL
                ].join('|');

                history.replaceState({page: 2}, '', 'settings');
                var afterReplace = [
                    history.length,
                    JSON.stringify(history.state),
                    location.href,
                    location.origin,
                    document.baseURI
                ].join('|');

                return afterPush + '||' + afterReplace;
            })()
        "#);

        assert_eq!(
            result,
            "2|{\"page\":1}|https://example.com:8443/dashboard?tab=home#top|https://example.com:8443|https://example.com:8443/dashboard?tab=home#top||2|{\"page\":2}|https://example.com:8443/settings|https://example.com:8443|https://example.com:8443/settings"
        );
    }

    #[test]
    fn test_create_text_node_append_and_insert() {
        let mut rt = make_runtime("<html><body><div id='app'></div></body></html>");
        let result = eval(&mut rt, r#"
            (function() {
                var app = document.getElementById('app');
                var tail = document.createElement('span');
                tail.textContent = 'tail';
                app.appendChild(tail);
                var head = document.createTextNode('head');
                app.insertBefore(head, tail);
                return [
                    app.childNodes.length,
                    app.firstChild.nodeType,
                    app.lastChild.nodeType,
                    app.textContent,
                    app.children.length
                ].join(':');
            })()
        "#);

        assert_eq!(result, "2:3:1:headtail:1");
    }

    #[test]
    fn test_performance_now() {
        let mut rt = make_runtime("<html><body></body></html>");
        let result = eval(&mut rt, "typeof window.performance.now()");
        assert_eq!(result, "number");
    }

    #[test]
    fn test_document_cookie_empty_string() {
        let mut rt = make_runtime("<html><body></body></html>");
        let cookie = eval(&mut rt, "document.cookie");
        assert_eq!(cookie, "", "Expected empty cookie string");
        // Writing should not throw
        let ok = eval(&mut rt, "(function() { try { document.cookie = 'test=1'; return 'ok'; } catch(e) { return 'err'; } })()");
        assert_eq!(ok, "ok");
    }

    #[test]
    fn test_session_storage_get_set() {
        let mut rt = make_runtime("<html><body></body></html>");
        eval(&mut rt, "sessionStorage.setItem('k', 'v')");
        let val = eval(&mut rt, "sessionStorage.getItem('k')");
        assert_eq!(val, "v");
    }

    #[test]
    fn test_get_computed_style_stub() {
        let mut rt = make_runtime("<html><body><div id='el'>x</div></body></html>");
        let result = eval(&mut rt, "(function() { var el = document.getElementById('el'); var s = window.getComputedStyle(el); return typeof s; })()");
        assert_eq!(result, "object");
    }

    #[test]
    fn test_node_constants() {
        let mut rt = make_runtime("<html><body></body></html>");
        let elem = eval(&mut rt, "Node.ELEMENT_NODE");
        assert_eq!(elem, "1");
        let text = eval(&mut rt, "Node.TEXT_NODE");
        assert_eq!(text, "3");
        let comment = eval(&mut rt, "Node.COMMENT_NODE");
        assert_eq!(comment, "8");
    }

    #[test]
    fn test_element_node_type() {
        let mut rt = make_runtime("<html><body><div id='el'>x</div></body></html>");
        let result = eval(&mut rt, "document.getElementById('el').nodeType");
        assert_eq!(result, "1");
    }

    #[test]
    fn test_image_constructor() {
        let mut rt = make_runtime("<html><body></body></html>");
        // new Image() should not throw
        let result = eval(&mut rt, "(function() { try { var img = new Image(100,50); return typeof img; } catch(e) { return 'err:'+e; } })()");
        assert_eq!(result, "object");
    }

    #[test]
    fn test_xml_http_request_stub() {
        let mut rt = make_runtime("<html><body></body></html>");
        let result = eval(&mut rt, "(function() { try { var xhr = new XMLHttpRequest(); xhr.open('GET', '/test'); return 'ok'; } catch(e) { return 'err:'+e; } })()");
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_match_media_returns_object() {
        let mut rt = make_runtime("<html><body></body></html>");
        let result = eval(&mut rt, "(function() { var mm = window.matchMedia('(max-width: 800px)'); return typeof mm.matches; })()");
        assert_eq!(result, "boolean");
    }

    #[test]
    fn test_window_screen_properties() {
        let mut rt = make_runtime("<html><body></body></html>");
        let w = eval(&mut rt, "window.screen.width");
        let h = eval(&mut rt, "window.screen.height");
        assert_eq!(w, "800");
        assert_eq!(h, "600");
    }

    #[test]
    fn test_device_pixel_ratio() {
        let mut rt = make_runtime("<html><body></body></html>");
        let dpr = eval(&mut rt, "window.devicePixelRatio");
        assert_eq!(dpr, "1");
    }

    #[test]
    fn test_window_location_set_from_url() {
        let url = url::Url::parse("https://www.example.com:8443/path?q=1#hash").unwrap();
        let dom = crate::dom::parse_html("<html><body></body></html>");
        let mut rt = JsRuntime::new(Some(dom.document), Some(url), None, new_console_buffer());
        let href = if let Ok(val) = rt.context.eval(Source::from_bytes(b"window.location.href")) {
            if let Ok(s) = val.to_string(&mut rt.context) { s.to_std_string_escaped() } else { String::new() }
        } else { String::new() };
        assert_eq!(href, "https://www.example.com:8443/path?q=1#hash");
        let hostname = if let Ok(val) = rt.context.eval(Source::from_bytes(b"window.location.hostname")) {
            if let Ok(s) = val.to_string(&mut rt.context) { s.to_std_string_escaped() } else { String::new() }
        } else { String::new() };
        assert_eq!(hostname, "www.example.com");
        let origin = if let Ok(val) = rt.context.eval(Source::from_bytes(b"window.location.origin")) {
            if let Ok(s) = val.to_string(&mut rt.context) { s.to_std_string_escaped() } else { String::new() }
        } else { String::new() };
        assert_eq!(origin, "https://www.example.com:8443");
    }

    #[test]
    fn test_keyboard_event_constructor() {
        let mut rt = make_runtime("<html><body></body></html>");
        let result = eval(&mut rt, "(function() { var e = new KeyboardEvent('keydown', {key: 'Enter', keyCode: 13}); return e.key + ':' + e.keyCode; })()");
        assert_eq!(result, "Enter:13");
    }

    #[test]
    fn test_url_constructor() {
        let mut rt = make_runtime("<html><body></body></html>");
        let result = eval(&mut rt, "(function() { var u = new URL('/path?q=1', 'https://example.com:8443/base/index.html'); return u.href + '|' + u.origin; })()");
        assert_eq!(result, "https://example.com:8443/path?q=1|https://example.com:8443");
    }

    #[test]
    fn test_abort_controller() {
        let mut rt = make_runtime("<html><body></body></html>");
        let result = eval(&mut rt, "(function() { var ctrl = new AbortController(); ctrl.abort(); return ctrl.signal.aborted ? 'aborted' : 'not'; })()");
        assert_eq!(result, "aborted");
    }

    #[test]
    fn test_window_crypto_get_random_values() {
        let mut rt = make_runtime("<html><body></body></html>");
        let result = eval(&mut rt, "(function() { try { var arr = new Uint8Array(4); window.crypto.getRandomValues(arr); return 'ok'; } catch(e) { return 'err:'+e; } })()");
        assert_eq!(result, "ok");
    }
}
