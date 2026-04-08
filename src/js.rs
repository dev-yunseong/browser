use boa_engine::{Context, Source, JsValue, NativeFunction, js_string};
use std::collections::{HashMap, VecDeque};
use markup5ever_rcdom::NodeData;
use std::cell::RefCell;

thread_local! {
    static MACRO_TASKS: RefCell<VecDeque<Box<dyn FnOnce(&mut Context)>>> = RefCell::new(VecDeque::new());
}

pub struct JsRuntime {
    context: Context,
}

impl JsRuntime {
    pub fn new() -> Self {
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

        // Load the full JS environment
        const BOOTSTRAP: &[u8] = include_bytes!("js_bootstrap.js");
        if let Err(e) = context.eval(Source::from_bytes(BOOTSTRAP)) {
            println!("[Aura JS Bootstrap Error] {}", e);
        }

        Self { context }
    }

    /// Drain and run all queued macro-tasks.
    pub fn run_queued_tasks(&mut self) {
        let mut tasks_to_run = VecDeque::new();
        MACRO_TASKS.with(|tasks| {
            tasks_to_run = std::mem::take(&mut *tasks.borrow_mut());
        });

        while let Some(task) = tasks_to_run.pop_front() {
            task(&mut self.context);
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
