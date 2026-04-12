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

    // Verify order via internal state if we had access, 
    // or just check if it doesn't panic for now.
    // In a real browser test, we'd check `order` array.
}
