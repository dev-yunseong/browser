use browser::js::JsRuntime;
use browser::dom;
use boa_engine::Source;

#[test]
fn test_focus_events() {
    let html = r#"
        <div id="a" tabindex="0"></div>
        <div id="b" tabindex="0"></div>
    "#;
    let dom = dom::parse_html(html);
    let mut js = JsRuntime::new(Some(dom.document.clone()), None, None, browser::js::new_console_buffer());
    
    js.execute(r#"
        globalThis.log_output = [];
        let a = document.getElementById('a');
        let b = document.getElementById('b');
        a.onfocus = () => log_output.push('a focus');
        a.onblur = () => log_output.push('a blur');
        b.onfocus = () => log_output.push('b focus');
        b.onblur = () => log_output.push('b blur');
    "#);

    // Initial state: nothing focused
    assert_eq!(js.get_focused_node_id(), None);

    // Focus A via Rust
    js.set_focused_node_id(Some("a".to_string()));
    js.tick(None, None);
    
    assert_eq!(get_array_results(&mut js, "log_output"), vec!["a focus"]);
}

fn get_array_results(js: &mut JsRuntime, var_name: &str) -> Vec<String> {
    use boa_engine::js_string;
    let val = js.context.eval(Source::from_bytes(var_name.as_bytes())).unwrap();
    let array = val.as_object().unwrap();
    let length = array.get(js_string!("length"), &mut js.context).unwrap().as_number().unwrap() as usize;
    let mut results = Vec::new();
    for i in 0..length {
        let v = array.get(i, &mut js.context).unwrap();
        results.push(v.as_string().unwrap().to_std_string_escaped());
    }
    results
}

#[test]
fn test_focus_blur_interleaving() {
    let html = r#"
        <div id="a" tabindex="0"></div>
        <div id="b" tabindex="0"></div>
    "#;
    let dom = dom::parse_html(html);
    let mut js = JsRuntime::new(Some(dom.document.clone()), None, None, browser::js::new_console_buffer());
    
    js.execute(r#"
        globalThis.log_output = [];
        let a = document.getElementById('a');
        let b = document.getElementById('b');
        a.addEventListener('focus', () => log_output.push('a focus'));
        a.addEventListener('blur', () => log_output.push('a blur'));
        b.addEventListener('focus', () => log_output.push('b focus'));
        b.addEventListener('blur', () => log_output.push('b blur'));
    "#);

    // 1. Focus A
    js.set_focused_node_id(Some("a".to_string()));
    js.tick(None, None);
    assert_eq!(get_array_results(&mut js, "log_output"), vec!["a focus"]);

    // 2. Focus B (should blur A first)
    js.set_focused_node_id(Some("b".to_string()));
    js.tick(None, None);
    assert_eq!(get_array_results(&mut js, "log_output"), vec!["a focus", "a blur", "b focus"]);

    // 3. Blur B
    js.set_focused_node_id(None);
    js.tick(None, None);
    assert_eq!(get_array_results(&mut js, "log_output"), vec!["a focus", "a blur", "b focus", "b blur"]);
}
