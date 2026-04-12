use boa_engine::{Context, Source, JsValue, NativeFunction, js_string};
use std::collections::{HashMap, VecDeque};
use markup5ever_rcdom::{NodeData, Handle};
use std::cell::RefCell;
use std::sync::mpsc::{channel, Receiver, Sender};
use url::Url;

thread_local! {
    static MACRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static MICRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static RAF_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context, f64)>>> = RefCell::new(VecDeque::new());
    static IDLE_TASKS: RefCell<VecDeque<(u32, Box<dyn FnOnce(&mut Context, f64)>)>> = RefCell::new(VecDeque::new());
    static NEXT_IDLE_ID: RefCell<u32> = RefCell::new(1);
    static DOM_ROOT: RefCell<Option<Handle>> = RefCell::new(None);
    static NODE_REGISTRY: RefCell<HashMap<u32, Handle>> = RefCell::new(HashMap::new());
    static NEXT_NODE_ID: RefCell<u32> = RefCell::new(1);
    static FETCH_REGISTRY: RefCell<HashMap<u32, (JsValue, JsValue)>> = RefCell::new(HashMap::new());
    static FETCH_BODY_REGISTRY: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
    static NEXT_FETCH_ID: RefCell<u32> = RefCell::new(1);
    static TASK_SENDER: RefCell<Option<Sender<Box<dyn FnOnce(&mut Context) + Send>>>> = RefCell::new(None);
    static FOCUSED_NODE: RefCell<Option<String>> = RefCell::new(None);
    static CURRENT_ORIGIN: RefCell<Option<Url>> = RefCell::new(None);
}

pub struct JsRuntime {
    pub context: Context,
    task_receiver: Receiver<Box<dyn FnOnce(&mut Context) + Send>>,
}

impl JsRuntime {
    pub fn new(dom: Option<Handle>, base_url: Option<Url>) -> Self {
        DOM_ROOT.with(|root| *root.borrow_mut() = dom);
        CURRENT_ORIGIN.with(|origin| *origin.borrow_mut() = base_url);
        let mut context = Context::default();
        let (task_sender, task_receiver) = channel();
        TASK_SENDER.with(|s| *s.borrow_mut() = Some(task_sender));

        // Register native console.log
        let log = NativeFunction::from_copy_closure(|_this, args, context| {
            let mut output = String::new();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 { output.push(' '); }
                if let Ok(s) = arg.to_string(context) {
                    output.push_str(&s.to_std_string_escaped());
                }
            }
            println!("[Aura JS] {}", output);
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("log"), 1, log).unwrap();

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

        // Register native __aura_fetch
        let aura_fetch = NativeFunction::from_copy_closure(|_this, args, context| {
            let url_str = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let resolve = args.get(1).cloned().unwrap_or(JsValue::undefined());
            let reject = args.get(2).cloned().unwrap_or(JsValue::undefined());

            let base_url = CURRENT_ORIGIN.with(|o| (*o.borrow()).clone());
            let target_url = if let Some(base) = base_url.as_ref() {
                base.join(&url_str).unwrap_or_else(|_| Url::parse(&url_str).unwrap_or(base.clone()))
            } else {
                Url::parse(&url_str).unwrap_or_else(|_| Url::parse("about:blank").unwrap())
            };

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

        // Load bootstrap
        let bootstrap = include_str!("js_bootstrap.js");
        let _ = context.eval(Source::from_bytes(bootstrap.as_bytes()));

        Self { context, task_receiver }
    }

    pub fn poll_tasks(&mut self) -> bool {
        let mut did_work = false;

        while let Ok(task) = self.task_receiver.try_recv() {
            MACRO_TASKS.with(|tasks| tasks.borrow_mut().push_back(task));
        }

        let macro_task = MACRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
        if let Some(task) = macro_task {
            task(&mut self.context);
            did_work = true;
            self.run_microtasks();
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
            let code = format!("document.__trigger_event({}, '{}', {{ bubbles: true }})", nid, event_type);
            let _ = self.context.eval(Source::from_bytes(code.as_bytes()));
            self.run_microtasks();
        }
    }

    pub fn poll_raf_tasks(&mut self, timestamp: f64) -> bool {
        let mut did_work = false;
        let mut tasks = VecDeque::new();
        RAF_TASKS.with(|t| tasks = t.borrow_mut().drain(..).collect());

        if !tasks.is_empty() {
            did_work = true;
            for task in tasks {
                task(&mut self.context, timestamp);
                self.run_microtasks();
            }
        }
        did_work
    }

    pub fn poll_idle_tasks(&mut self, deadline_ms: f64) -> bool {
        let mut did_work = false;
        while let Some((_, task)) = IDLE_TASKS.with(|tasks| tasks.borrow_mut().pop_front()) {
            did_work = true;
            task(&mut self.context, deadline_ms);
            self.run_microtasks();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64;
            if now >= deadline_ms { break; }
        }
        did_work
    }

    pub fn execute(&mut self, source: &str) {
        if source.contains("import.meta") || (source.contains("import ") && source.contains(" from ")) { return; }
        let _ = self.context.eval(Source::from_bytes(source.as_bytes()));
        self.run_microtasks();
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

fn register_node(handle: Handle) -> u32 {
    let id = NEXT_NODE_ID.with(|id_cell| {
        let id = *id_cell.borrow();
        *id_cell.borrow_mut() += 1;
        id
    });
    NODE_REGISTRY.with(|reg| reg.borrow_mut().insert(id, handle));
    id
}
