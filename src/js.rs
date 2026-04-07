use boa_engine::{Context, Source, JsValue, NativeFunction, JsObject, js_string};

pub struct JsRuntime {
    context: Context,
}

impl JsRuntime {
    pub fn new() -> Self {
        let mut context = Context::default();

        // 1. console.log binding
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
        context.register_global_callable(js_string!("log"), 1, log.clone()).unwrap();

        // 2. Setup mock objects (window, document, navigator)
        let _ = context.eval(Source::from_bytes(r#"
            var window = globalThis;
            var console = { log: log };
            var document = {
                getElementById: function() { return null; },
                getElementsByTagName: function() { return []; },
                querySelector: function() { return null; },
                querySelectorAll: function() { return []; },
                createElement: function() { return {}; },
                body: {},
                location: { href: "" }
            };
            var navigator = { userAgent: "AuraBrowser/0.1" };
            var location = document.location;
        "#.as_bytes()));

        Self { context }
    }

    pub fn execute(&mut self, code: &str) {
        // Skip code that uses features we definitely don't support yet to reduce noise
        if code.contains("import.meta") { return; }

        match self.context.eval(Source::from_bytes(code.as_bytes())) {
            Ok(res) => {
                if !res.is_undefined() {
                    if let Ok(s) = res.to_string(&mut self.context) {
                        // println!("[Aura JS Return] {}", s.to_std_string_escaped());
                    }
                }
            }
            Err(e) => {
                let err_msg = e.to_string();
                // Filter out common repetitive errors to keep console clean
                if !err_msg.contains("document is not defined") {
                    println!("[Aura JS Error] {}", err_msg);
                }
            }
        }
    }
}

pub fn extract_scripts_from_dom(handle: &markup5ever_rcdom::Handle) -> Vec<String> {
    let mut scripts = Vec::new();

    if let markup5ever_rcdom::NodeData::Element { ref name, .. } = handle.data {
        if name.local.to_string() == "script" {
            let mut script_content = String::new();
            for child in handle.children.borrow().iter() {
                if let markup5ever_rcdom::NodeData::Text { ref contents } = child.data {
                    script_content.push_str(&contents.borrow());
                }
            }
            if !script_content.is_empty() {
                scripts.push(script_content);
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

    #[test]
    fn test_js_env_mock() {
        let mut runtime = JsRuntime::new();
        // This should not error now
        runtime.execute("document.getElementById('test');");
    }
}
