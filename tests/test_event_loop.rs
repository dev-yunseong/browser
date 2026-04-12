use browser::js::JsRuntime;

#[test]
fn test_macro_micro_task_order() {
    let mut js = JsRuntime::new(None);
    
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
    js.poll_tasks(); 
    
    // Wait for the thread-spawned setTimeout to send its task
    std::thread::sleep(std::time::Duration::from_millis(50));
    
    // Poll again to run the macro task
    js.poll_tasks();
}

#[test]
fn test_raf_timestamp() {
    let mut js = JsRuntime::new(None);

    js.execute(r#"
        var ts = 0;
        requestAnimationFrame((t) => {
            ts = t;
            log("rAF called with: " + t);
        });
    "#);

    // Initial execute should NOT have run rAF
    js.poll_tasks(); // Runs microtasks

    // Explicitly poll rAF
    js.poll_raf_tasks(1234.5);

    // Check ts via eval
    let val = js.context.eval(boa_engine::Source::from_bytes(b"ts")).unwrap();
    assert_eq!(val.as_number().unwrap(), 1234.5);
}
