use browser::js::JsRuntime;

#[test]
fn test_macro_micro_task_order() {
    let mut js = JsRuntime::new(None, None, None);
    
    // This script uses standard Promise (micro) and our setTimeout (macro)
    js.execute(r#"
        var order = [];
        log("Start");
        setTimeout(() => {
            order.push("macro");
            log("Macro Task");
        }, 0);
        Promise.resolve().then(() => {
            order.push("micro");
            log("Micro Task");
        });
        log("End");
    "#);

    // Initial execution should have run synchronous code
    // Standard order: Sync -> Micro -> Macro
    
    // Poll to run microtasks (checkpoint after script)
    js.tick(None, None); 
    
    // Wait for the thread-spawned setTimeout to send its task
    std::thread::sleep(std::time::Duration::from_millis(50));
    
    // Poll again to run the macro task
    js.tick(None, None);
}

#[test]
fn test_raf_timestamp() {
    let mut js = JsRuntime::new(None, None, None);

    js.execute(r#"
        var ts = 0;
        requestAnimationFrame((t) => {
            ts = t;
            log("rAF called with: " + t);
        });
    "#);

    // Initial execute should NOT have run rAF
    js.tick(None, None); // Runs microtasks

    // Explicitly poll rAF
    js.tick(Some(1234.5), None);

    // Check ts via eval
    let val = js.context.eval(boa_engine::Source::from_bytes(b"ts")).unwrap();
    assert_eq!(val.as_number().unwrap(), 1234.5);
}
