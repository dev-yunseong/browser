use boa_engine::{Context, Source, JsValue, NativeFunction, js_string, JsObject};
use std::collections::{HashMap, VecDeque};
use markup5ever_rcdom::{NodeData, Handle};
use std::cell::RefCell;
use std::sync::mpsc::{channel, Receiver, Sender};

thread_local! {
    static MACRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static MICRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static DOM_ROOT: RefCell<Option<Handle>> = RefCell::new(None);
    static NODE_REGISTRY: RefCell<HashMap<u32, Handle>> = RefCell::new(HashMap::new());
    static NEXT_NODE_ID: RefCell<u32> = RefCell::new(1);
    static FETCH_REGISTRY: RefCell<HashMap<u32, (JsValue, JsValue)>> = RefCell::new(HashMap::new());
    static NEXT_FETCH_ID: RefCell<u32> = RefCell::new(1);
    static TASK_SENDER: RefCell<Option<Sender<Box<dyn FnOnce(&mut Context) + Send>>>> = RefCell::new(None);
}

pub struct JsRuntime {
    pub context: Context,
    task_receiver: Receiver<Box<dyn FnOnce(&mut Context) + Send>>,
}

impl JsRuntime {
    pub fn new(dom: Option<Handle>) -> Self {
        DOM_ROOT.with(|root| *root.borrow_mut() = dom);
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
            let url = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let resolve = args.get(1).cloned().unwrap_or(JsValue::undefined());
            let reject = args.get(2).cloned().unwrap_or(JsValue::undefined());

            let fetch_id = NEXT_FETCH_ID.with(|id_cell| {
                let id = *id_cell.borrow();
                *id_cell.borrow_mut() += 1;
                id
            });

            FETCH_REGISTRY.with(|reg| reg.borrow_mut().insert(fetch_id, (resolve, reject)));

            TASK_SENDER.with(|s_cell| {
                if let Some(ref sender) = *s_cell.borrow() {
                    let sender_clone = sender.clone();
                    std::thread::spawn(move || {
                        let result = reqwest::blocking::get(&url);
                        match result {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                let ok = resp.status().is_success();
                                let body = resp.text().unwrap_or_default();
                                let task: Box<dyn FnOnce(&mut Context) + Send> = Box::new(move |ctx| {
                                    if let Some((resolve, _)) = FETCH_REGISTRY.with(|reg| reg.borrow_mut().remove(&fetch_id)) {
                                        if let Some(obj) = resolve.as_object() {
                                            if let Ok(js_resp_val) = ctx.eval(Source::from_bytes(b"({})")) {
                                                if let Some(js_resp) = js_resp_val.as_object() {
                                                    let _ = js_resp.set(js_string!("status"), JsValue::from(status), false, ctx);
                                                    let _ = js_resp.set(js_string!("ok"), JsValue::from(ok), false, ctx);
                                                    let _ = js_resp.set(js_string!("_body"), js_string!(body), false, ctx);
                                                    let _ = obj.call(&JsValue::undefined(), &[js_resp_val], ctx);
                                                }
                                            }
                                        }
                                    }
                                });
                                let _ = sender_clone.send(task);
                            }
                            Err(e) => {
                                let err_msg = e.to_string();
                                let task: Box<dyn FnOnce(&mut Context) + Send> = Box::new(move |ctx| {
                                    if let Some((_, reject)) = FETCH_REGISTRY.with(|reg| reg.borrow_mut().remove(&fetch_id)) {
                                        if let Some(obj) = reject.as_object() {
                                            let _ = obj.call(&JsValue::undefined(), &[JsValue::from(js_string!(err_msg))], ctx);
                                        }
                                    }
                                });
                                let _ = sender_clone.send(task);
                            }
                        }
                    });
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_fetch"), 3, aura_fetch).unwrap();

        // Register native queueMicrotask
        let queue_microtask = NativeFunction::from_copy_closure(|_this, args, _context| {
            if let Some(callback) = args.get(0).cloned() {
                if let Some(obj) = callback.as_object() {
                    if obj.is_callable() {
                        MICRO_TASKS.with(|tasks| {
                            tasks.borrow_mut().push_back(Box::new(move |ctx| {
                                let _ = obj.call(&JsValue::undefined(), &[], ctx);
                            }));
                        });
                    }
                }
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("queueMicrotask"), 1, queue_microtask).unwrap();

        // --- DOM Bridges ---
        let get_el_by_id = NativeFunction::from_copy_closure(|_this, args, _context| {
            let id_str = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let id = DOM_ROOT.with(|root_cell| {
                if let Some(ref root) = *root_cell.borrow() {
                    if let Some(handle) = find_element_by_id(root, &id_str) {
                        return Some(register_node(handle));
                    }
                }
                None
            });
            Ok(id.map(JsValue::from).unwrap_or(JsValue::null()))
        });
        context.register_global_callable(js_string!("__aura_get_element_by_id"), 1, get_el_by_id).unwrap();

        let append_child = NativeFunction::from_copy_closure(|_this, args, _context| {
            let parent_id = args.get(0).and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
            let child_id = args.get(1).and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
            NODE_REGISTRY.with(|reg| {
                let reg = reg.borrow();
                if let (Some(parent), Some(child)) = (reg.get(&parent_id), reg.get(&child_id)) {
                    child.parent.set(Some(std::rc::Rc::downgrade(&parent)));
                    parent.children.borrow_mut().push(child.clone());
                }
            });
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_append_child"), 2, append_child).unwrap();

        const BOOTSTRAP: &[u8] = include_bytes!("js_bootstrap.js");
        if let Err(e) = context.eval(Source::from_bytes(BOOTSTRAP)) {
            println!("[Aura JS Bootstrap Error] {}", e);
        }

        Self { context, task_receiver }
    }

    pub fn run_queued_tasks(&mut self) {
        while let Ok(task) = self.task_receiver.try_recv() { task(&mut self.context); }
        loop {
            let task = MACRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
            if let Some(task) = task {
                task(&mut self.context);
                self.run_microtasks();
            } else {
                break;
            }
        }
        let _ = self.context.run_jobs();
    }

    pub fn run_microtasks(&mut self) {
        loop {
            let micro = MICRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
            if let Some(m) = micro { m(&mut self.context); } else { break; }
        }
    }

    pub fn execute(&mut self, code: &str) {
        if code.contains("import.meta") || (code.contains("import ") && code.contains(" from ")) { return; }
        let _ = self.context.eval(Source::from_bytes(code.as_bytes()));
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
    NODE_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        let id = NEXT_NODE_ID.with(|id_cell| {
            let id = *id_cell.borrow();
            *id_cell.borrow_mut() += 1;
            id
        });
        reg.insert(id, handle);
        id
    })
}

pub fn extract_scripts_from_dom(handle: &Handle) -> Vec<String> {
    let mut scripts = Vec::new();
    if let NodeData::Element { ref name, .. } = handle.data {
        if name.local.to_string() == "script" {
            for child in handle.children.borrow().iter() {
                if let NodeData::Text { ref contents } = child.data {
                    scripts.push(contents.borrow().to_string());
                }
            }
        }
    }
    for child in handle.children.borrow().iter() {
        scripts.extend(extract_scripts_from_dom(child));
    }
    scripts
}
