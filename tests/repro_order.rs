use browser::js::JsRuntime;
use boa_engine::{Source, JsValue};

#[test]
fn test_repro_order() {
    let mut js = JsRuntime::new(None, None);
    
    js.execute(r#"
        globalThis.order = [];
        log("Start");
        setTimeout(() => {
            globalThis.order.push("macro");
            log("Macro Task");
        }, 0);
        Promise.resolve().then(() => {
            globalThis.order.push("micro");
            log("Micro Task");
        });
        log("End");
    "#);

    println!("--- After execute ---");
    
    // Poll 1
    js.poll_tasks();
    println!("--- After poll 1 ---");

    // Poll 2
    js.poll_tasks();
    println!("--- After poll 2 ---");

    let val = js.context.eval(Source::from_bytes(b"globalThis.order.join(', ')")).unwrap();
    let order_str = val.to_string(&mut js.context).unwrap().to_std_string_escaped();
    println!("Order: {}", order_str);
    
    // Standard order should be "micro, macro"
    assert_eq!(order_str, "micro, macro");
}
