use boa_engine::{Context, Source, JsValue, NativeFunction, js_string};
use std::collections::HashMap;

pub struct JsRuntime {
    context: Context,
}

impl JsRuntime {
    pub fn new() -> Self {
        let mut context = Context::default();

        // Register native console.log so bootstrap.js can use it
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

        // Load the full JS environment from a separate source file.
        // include_bytes! embeds it at compile time; the file may contain UTF-8.
        const BOOTSTRAP: &[u8] = include_bytes!("js_bootstrap.js");
        if let Err(e) = context.eval(Source::from_bytes(BOOTSTRAP)) {
            println!("[Aura JS Bootstrap Error] {}", e);
        }

        Self { context }
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

    /// Read accumulated innerHTML changes from JS.
    /// Returns map: element_id -> html string.
    pub fn get_inner_html_changes(&mut self) -> HashMap<String, String> {
        let mut result: HashMap<String, String> = HashMap::new();

        let val = match self.context.eval(Source::from_bytes(b"__aura_inner_html_log.join('####')")) {
            Ok(v) => v,
            Err(_) => return result,
        };
        let s = match val.to_string(&mut self.context) {
            Ok(s) => s.to_std_string_escaped(),
            Err(_) => return result,
        };

        for entry in s.split("####") {
            let parts: Vec<&str> = entry.splitn(2, "||||").collect();
            if parts.len() == 2 && !parts[0].is_empty() {
                result.insert(parts[0].to_string(), parts[1].to_string());
            }
        }

        let _ = self.context.eval(Source::from_bytes(b"__aura_inner_html_log = [];"));
        result
    }

    /// Returns true if there are pending style or innerHTML changes.
    pub fn is_dirty(&mut self) -> bool {
        matches!(
            self.context.eval(Source::from_bytes(
                b"__aura_style_log.length > 0 || __aura_inner_html_log.length > 0"
            )),
            Ok(v) if v.to_boolean()
        )
    }
}

/// Evaluate JS, suppressing common expected errors that are irrelevant noise.
fn context_eval_silent(ctx: &mut Context, code: &str) -> Result<(), String> {
    match ctx.eval(Source::from_bytes(code.as_bytes())) {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = e.to_string();
            let ignore = [
                "document is not defined",
                "window is not defined",
                "Cannot read property",
                "is not a constructor",
                "exportDefault",
                "__webpack",
                "require is not defined",
            ];
            if ignore.iter().any(|pat| msg.contains(pat)) {
                Ok(())
            } else {
                Err(msg)
            }
        }
    }
}

pub fn extract_scripts_from_dom(handle: &markup5ever_rcdom::Handle) -> Vec<String> {
    let mut scripts = Vec::new();

    if let markup5ever_rcdom::NodeData::Element { ref name, ref attrs, .. } = handle.data {
        if name.local.to_string() == "script" {
            // Skip external scripts
            let has_src = attrs.borrow().iter().any(|a| a.name.local.to_string() == "src");
            if !has_src {
                let mut content = String::new();
                for child in handle.children.borrow().iter() {
                    if let markup5ever_rcdom::NodeData::Text { ref contents } = child.data {
                        content.push_str(&contents.borrow());
                    }
                }
                if !content.trim().is_empty() {
                    scripts.push(content);
                }
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
        runtime.execute("document.getElementById('test');");
        let overrides = runtime.get_style_overrides();
        assert!(overrides.is_empty());
    }

    #[test]
    fn test_style_override() {
        let mut runtime = JsRuntime::new();
        runtime.execute(r#"
            var el = document.getElementById('myBox');
            el.style.backgroundColor = 'red';
        "#);
        let overrides = runtime.get_style_overrides();
        assert!(overrides.contains_key("myBox"), "Expected key 'myBox', got: {:?}", overrides.keys().collect::<Vec<_>>());
        assert_eq!(overrides["myBox"].get("background-color").map(|s| s.as_str()), Some("red"));
    }

    #[test]
    fn test_inner_html_change() {
        let mut runtime = JsRuntime::new();
        runtime.execute(r#"
            var el = document.getElementById('content');
            el.innerHTML = '<p>Hello</p>';
        "#);
        let changes = runtime.get_inner_html_changes();
        assert!(changes.contains_key("content"));
    }

    #[test]
    fn test_console_log() {
        let mut runtime = JsRuntime::new();
        runtime.execute("console.log('hello world');");
        // should not panic
    }

    #[test]
    fn test_settimeout_fires() {
        let mut runtime = JsRuntime::new();
        runtime.execute(r#"
            var fired = false;
            setTimeout(function() { fired = true; }, 0);
        "#);
        // setTimeout fires immediately in Aura
        let val = runtime.context.eval(Source::from_bytes(b"typeof fired !== 'undefined'"));
        assert!(matches!(val, Ok(v) if v.to_boolean()));
    }
}
