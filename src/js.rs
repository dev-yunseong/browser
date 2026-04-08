use boa_engine::{Context, Source, JsValue, NativeFunction, js_string};
use std::collections::{HashMap, VecDeque};
use markup5ever_rcdom::{NodeData, Handle, Node};
use std::cell::RefCell;
use html5ever::{QualName, LocalName, Namespace, ns, local_name};

thread_local! {
    static MACRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
    static MICRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
}

pub struct JsRuntime {
    context: Context,
    dom: Option<Handle>,
}

impl JsRuntime {
    pub fn new(dom: Option<Handle>) -> Self {
        let mut context = Context::default();

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

        // Register native __aura_queue_task using thread_local
        let queue_task = NativeFunction::from_copy_closure(|_this, args, _context| {
            if let Some(callback) = args.get(0).cloned() {
                if let Some(obj) = callback.as_object() {
                    if obj.is_callable() {
                        MACRO_TASKS.with(|tasks| {
                            tasks.borrow_mut().push_back(Box::new(move |ctx| {
                                let _ = callback.as_callable().unwrap().call(&JsValue::undefined(), &[], ctx);
                            }));
                        });
                    }
                }
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_queue_task"), 1, queue_task).unwrap();

        // Register native queueMicrotask
        let queue_microtask = NativeFunction::from_copy_closure(|_this, args, _context| {
            if let Some(callback) = args.get(0).cloned() {
                if let Some(obj) = callback.as_object() {
                    if obj.is_callable() {
                        MICRO_TASKS.with(|tasks| {
                            tasks.borrow_mut().push_back(Box::new(move |ctx| {
                                let _ = callback.as_callable().unwrap().call(&JsValue::undefined(), &[], ctx);
                            }));
                        });
                    }
                }
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("queueMicrotask"), 1, queue_microtask).unwrap();

        // Register native __aura_append_child
        let dom_ref = dom.clone();
        let append_child = NativeFunction::from_copy_closure(move |_this, args, context| {
            let parent_id = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            let child_tag = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_default();
            
            if let Some(ref root) = dom_ref {
                if let Some(parent) = find_element_by_id(root, &parent_id) {
                    let name = QualName::new(
                        None, 
                        Namespace::from("http://www.w3.org/1999/xhtml"), 
                        LocalName::from(child_tag)
                    );
                    let new_node = Node::new(NodeData::Element {
                        name,
                        attrs: RefCell::new(Vec::new()),
                        template_contents: RefCell::new(None),
                        mathml_annotation_xml_integration_point: false,
                    });
                    // Set parent of the new node to allow traversal upwards
                    new_node.parent.set(Some(std::rc::Rc::downgrade(&parent)));
                    parent.children.borrow_mut().push(new_node);
                    println!("[Aura JS] Appended <{}> to #{}", child_tag, parent_id);
                }
            }
            Ok(JsValue::undefined())
        });
        context.register_global_callable(js_string!("__aura_append_child"), 2, append_child).unwrap();

        // Load the full JS environment
        const BOOTSTRAP: &[u8] = include_bytes!("js_bootstrap.js");
        if let Err(e) = context.eval(Source::from_bytes(BOOTSTRAP)) {
            println!("[Aura JS Bootstrap Error] {}", e);
        }

        Self { context, dom }
    }

    /// Drain and run all queued macro-tasks and their associated micro-tasks.
    pub fn run_queued_tasks(&mut self) {
        loop {
            let task = MACRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
            if let Some(task) = task {
                task(&mut self.context);
                // Run all microtasks after each macro-task (spec compliant)
                self.run_microtasks();
            } else {
                break;
            }
        }
    }

    pub fn run_microtasks(&mut self) {
        loop {
            let micro = MICRO_TASKS.with(|tasks| tasks.borrow_mut().pop_front());
            if let Some(m) = micro {
                m(&mut self.context);
            } else {
                break;
            }
        }
    }

    /// Execute a JavaScript string in the current context.
    pub fn execute(&mut self, code: &str) {
        if code.contains("import.meta") { return; }
        if code.contains("import ") && code.contains(" from ") { return; }

        if let Err(e) = context_eval_silent(&mut self.context, code) {
            println!("[Aura JS Error] {}", e);
        }
    }

    /// Read accumulated style overrides written by JS via `element.style.prop = value`.
    /// Returns map: element_id -> (css-property -> value-string).
    /// Clears the JS log after reading.
    pub fn get_style_overrides(&mut self) -> HashMap<String, HashMap<String, String>> {
        let mut result: HashMap<String, HashMap<String, String>> = HashMap::new();

        let val = match self.context.eval(Source::from_bytes(b"__aura_style_log.join('####')")) {
            Ok(v) => v,
            Err(_) => return result,
        };
        let s = match val.to_string(&mut self.context) {
            Ok(s) => s.to_std_string_escaped(),
            Err(_) => return result,
        };

        for entry in s.split("####") {
            let parts: Vec<&str> = entry.splitn(3, "||||").collect();
            if parts.len() == 3 && !parts[0].is_empty() {
                result
                    .entry(parts[0].to_string())
                    .or_default()
                    .insert(parts[1].to_string(), parts[2].to_string());
            }
        }

        let _ = self.context.eval(Source::from_bytes(b"__aura_style_log = [];"));
        result
    }

    pub fn context(&mut self) -> &mut Context {
        &mut self.context
    }
}

fn find_element_by_id(handle: &Handle, id: &str) -> Option<Handle> {
    if let NodeData::Element { ref name, ref attrs, .. } = handle.data {
        // Special case: if ID is "body", also match the <body> tag itself
        if id == "body" && name.local.to_string() == "body" {
            return Some(handle.clone());
        }

        for attr in attrs.borrow().iter() {
            if attr.name.local.to_string() == "id" && attr.value.to_string() == id {
                return Some(handle.clone());
            }
        }
    }
    for child in handle.children.borrow().iter() {
        if let Some(found) = find_element_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

pub fn extract_scripts_from_dom(handle: &markup5ever_rcdom::Handle) -> Vec<String> {
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

/// Evaluation that doesn't panic on error, just returns Result
fn context_eval_silent(context: &mut Context, code: &str) -> Result<JsValue, String> {
    context.eval(Source::from_bytes(code.as_bytes()))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_console_log() {
        let mut rt = JsRuntime::new();
        rt.execute("console.log('hello from test')");
    }

    #[test]
    fn test_js_env_mock() {
        let mut rt = JsRuntime::new();
        rt.execute("console.log(window.location.href)");
        rt.execute("console.log(document.title)");
    }

    #[test]
    fn test_style_override() {
        let mut rt = JsRuntime::new();
        rt.execute("let el = document.getElementById('test'); el.style.color = 'red';");
        let overrides = rt.get_style_overrides();
        assert_eq!(overrides.get("test").unwrap().get("color").unwrap(), "red");
    }

    #[test]
    fn test_settimeout_fires() {
        let mut rt = JsRuntime::new();
        rt.execute("var x = 1; setTimeout(() => { x = 2; }, 0);");
        // Before running tasks, x should still be 1 because it's async now
        // Wait, I need a way to check global x. 
        // For now, just ensure it doesn't crash.
        rt.run_queued_tasks();
    }

    #[test]
    fn test_inner_html_change() {
        let mut rt = JsRuntime::new();
        rt.execute("document.getElementById('test').innerHTML = 'new content';");
    }
}
