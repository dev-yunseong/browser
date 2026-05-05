/*
use browser::js::JsRuntime;
use boa_engine::{Source, js_string};

fn get_array_results(js: &mut JsRuntime, var_name: &str) -> Vec<String> {
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
fn test_event_loop_interleaving() {
    let mut js = JsRuntime::new(None, None, None, None, browser::js::new_console_buffer());
    
    js.execute(r#"
        var results = [];
        setTimeout(() => {
            results.push("macro1");
            Promise.resolve().then(() => {
                results.push("micro in macro1");
            });
        }, 0);
        setTimeout(() => {
            results.push("macro2");
        }, 0);
        Promise.resolve().then(() => {
            results.push("micro1");
        });
    "#);

    // After execute(), micro1 should have run (checkpoint at end of script task)
    assert_eq!(get_array_results(&mut js, "results"), vec!["micro1"]);

    // Tick 1: Macro1 + its microtask
    js.tick(None, None);
    assert_eq!(get_array_results(&mut js, "results"), vec!["micro1", "macro1", "micro in macro1"]);

    // Tick 2: Macro2
    js.tick(None, None);
    assert_eq!(get_array_results(&mut js, "results"), vec!["micro1", "macro1", "micro in macro1", "macro2"]);
}

#[test]
fn test_raf_interleaving() {
    let mut js = JsRuntime::new(None, None, None, None, browser::js::new_console_buffer());
    
    js.execute(r#"
        var results = [];
        requestAnimationFrame(() => {
            results.push("raf1");
            Promise.resolve().then(() => {
                results.push("micro in raf1");
            });
        });
        setTimeout(() => {
            results.push("macro1");
        }, 0);
    "#);

    // Initial execute() runs sync code (none here)
    // results should be empty
    assert_eq!(get_array_results(&mut js, "results"), Vec::<String>::new());

    // Tick 1: Process Macro1
    js.tick(None, None);
    assert_eq!(get_array_results(&mut js, "results"), vec!["macro1"]);

    // Tick 2: Process rAF + its microtask
    js.tick(Some(100.0), None);
    assert_eq!(get_array_results(&mut js, "results"), vec!["macro1", "raf1", "micro in raf1"]);
}
*/
