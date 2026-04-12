use boa_engine::{Context, Source, JsValue, NativeFunction, js_string};
use std::collections::{HashMap, VecDeque};
use markup5ever_rcdom::{NodeData, Handle, Node};
use std::cell::RefCell;
use html5ever::{QualName, LocalName, Namespace};
use std::sync::mpsc::{channel, Receiver, Sender};

thread_local! {
    static MACRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static MICRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static RAF_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context, f64)>>> = RefCell::new(VecDeque::new());
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

        // Register native setTimeout
        let set_timeout = NativeFunction::from_copy_closure(|_this, args, _context| {
            if let Some(callback) = args.get(0).cloned() {
                if let Some(obj) = callback.as_object() {
                    if obj.is_callable() {
                        let ms = args.get(1).and_then(|v| v.as_number()).unwrap_or(0.0);
                        
                        // For now, immediate execution in next macro-task queue
                        // Proper timer management requires JsRuntime to track deadlines.
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
        let aura_fetch = NativeFunction::from_copy_closure(|_this, args, _context| {
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

        // DOM Bridges
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

        let get_parent_id = NativeFunction::from_copy_closure(|_this, args, _context| {
            let node_id = args.get(0).and_then(|v| v.as_number()).unwrap_or(0.0) as u32;
            // Extract the parent Handle before releasing NODE_REGISTRY borrow,
            // then call register_node separately to avoid a double-borrow panic.
            let parent_handle = NODE_REGISTRY.with(|reg| {
                if let Some(node) = reg.borrow().get(&node_id) {
                    node.parent.take().and_then(|p| p.upgrade()).map(|p| {
                        node.parent.set(Some(std::rc::Rc::downgrade(&p)));
                        p
                    })
                } else {
                    None
                }
            });
            let pid = parent_handle.map(register_node);
            Ok(pid.map(JsValue::from).unwrap_or(JsValue::null()))
        });
        context.register_global_callable(js_string!("__aura_get_parent_id"), 1, get_parent_id).unwrap();

        let get_body = NativeFunction::from_copy_closure(|_this, _args, _context| {
            let bid = DOM_ROOT.with(|root_cell| {
                if let Some(ref root) = *root_cell.borrow() {
                    if let Some(body) = find_element_by_tag(root, "body") {
                        return Some(register_node(body));
                    }
                }
                None
            });
            Ok(bid.map(JsValue::from).unwrap_or(JsValue::null()))
        });
        context.register_global_callable(js_string!("__aura_get_body"), 0, get_body).unwrap();

        let create_element = NativeFunction::from_copy_closure(|_this, args, _context| {
            let tag = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let name = QualName::new(None, Namespace::from("http://www.w3.org/1999/xhtml"), LocalName::from(tag));
            let new_node = Node::new(NodeData::Element {
                name,
                attrs: RefCell::new(Vec::new()),
                template_contents: RefCell::new(None),
                mathml_annotation_xml_integration_point: false,
            });
            Ok(JsValue::from(register_node(new_node)))
        });
        context.register_global_callable(js_string!("__aura_create_element"), 1, create_element).unwrap();

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

        // Load the full JS environment
        const BOOTSTRAP: &[u8] = include_bytes!("js_bootstrap.js");
        if let Err(e) = context.eval(Source::from_bytes(BOOTSTRAP)) {
            println!("[Aura JS Bootstrap Error] {}", e);
        }

        Self { context, task_receiver }
    }

    pub fn poll_tasks(&mut self) -> bool {
        let mut did_work = false;

        // 1. Check for tasks from other threads (fetch, etc.)
        while let Ok(task) = self.task_receiver.try_recv() {
            MACRO_TASKS.with(|tasks| tasks.borrow_mut().push_back(task));
        }

        // 2. Execute ONE macro-task
        let macro_task = MACRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
        if let Some(task) = macro_task {
            task(&mut self.context);
            did_work = true;
            
            // 3. Microtask checkpoint after task execution
            self.run_microtasks();
        }

        did_work
    }

    /// Run all pending micro-tasks and Boa jobs.
    fn run_microtasks(&mut self) {
        loop {
            let mut micro_work_done = false;

            // Execute all micro-tasks in our queue
            while let Some(task) = MICRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front()) {
                task(&mut self.context);
                micro_work_done = true;
            }

            // Execute Boa jobs (Promises)
            let _ = self.context.run_jobs();

            if !micro_work_done { break; }
        }
    }

    pub fn trigger_event(&mut self, target_id: &str, event_type: &str) {
        // Find native ID from DOM string ID
        let native_id = DOM_ROOT.with(|root| {
            if let Some(ref r) = *root.borrow() {
                find_element_by_id(r, target_id).map(register_node)
            } else { None }
        });

        if let Some(nid) = native_id {
            let code = format!("document.__trigger_event({}, '{}', {{ bubbles: true }})", nid, event_type);
            let _ = self.context.eval(Source::from_bytes(code.as_bytes()));
            
            // Microtask checkpoint after event execution
            self.run_microtasks();
        }
    }

    pub fn poll_raf_tasks(&mut self, timestamp: f64) -> bool {
        let mut did_work = false;

        // Take a snapshot of current tasks to avoid infinite recursion in same frame
        let mut tasks = VecDeque::new();
        RAF_TASKS.with(|t| tasks = t.borrow_mut().drain(..).collect());

        if !tasks.is_empty() {
            did_work = true;
            for task in tasks {
                task(&mut self.context, timestamp);
                // Microtask checkpoint after each rAF callback
                self.run_microtasks();
            }
        }

        did_work
    }

    pub fn execute(&mut self, source: &str) {
        if source.contains("import.meta") || (source.contains("import ") && source.contains(" from ")) { return; }
        let _ = self.context.eval(Source::from_bytes(source.as_bytes()));
        
        // Microtask checkpoint after script execution
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

fn register_node(handle: Handle) -> u32 {
    NODE_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        // Check if already registered
        for (id, h) in reg.iter() {
            if std::rc::Rc::ptr_eq(h, &handle) { return *id; }
        }
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
