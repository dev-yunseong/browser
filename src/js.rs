use boa_engine::{Context, Source, JsValue, NativeFunction, js_string};

pub struct JsRuntime {
    context: Context,
}

impl JsRuntime {
    pub fn new() -> Self {
        let mut context = Context::default();

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

        // Use the discovered register_global_callable
        context.register_global_callable(js_string!("log"), 1, log).unwrap();
        
        // Also try to setup a simple console.log in JS
        let _ = context.eval(Source::from_bytes("var console = { log: log };".as_bytes()));

        Self { context }
    }

    pub fn execute(&mut self, code: &str) {
        match self.context.eval(Source::from_bytes(code.as_bytes())) {
            Ok(res) => {
                if !res.is_undefined() {
                    if let Ok(s) = res.to_string(&mut self.context) {
                        println!("[Aura JS Return] {}", s.to_std_string_escaped());
                    }
                }
            }
            Err(e) => {
                println!("[Aura JS Error] {}", e.to_string());
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
    fn test_js_execution() {
        let mut runtime = JsRuntime::new();
        runtime.execute("console.log('Test success');");
    }

    #[test]
    fn test_js_return_value() {
        let mut runtime = JsRuntime::new();
        runtime.execute("1 + 1");
    }
}
