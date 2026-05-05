/*
use boa_engine::{Context, Source};
use std::collections::VecDeque;
use std::cell::RefCell;

// Note: Testing JsRuntime directly requires some setup since it uses thread_local for task queues.
// However, the test should be able to verify if requestIdleCallback is registered and functions correctly.

#[test]
fn test_request_idle_callback_registration() {
    let mut runtime = browser::js::JsRuntime::new(None, None, None, None, browser::js::new_console_buffer());
    
    // Test if requestIdleCallback is available in the global scope
    let code = r#"
        let called = false;
        requestIdleCallback((deadline) => {
            called = true;
            console.log("Idle callback called! Time remaining: " + deadline.timeRemaining());
        });
        called;
    "#;
    
    // Initially not called
    let result = runtime.context.eval(Source::from_bytes(code.as_bytes())).unwrap();
    assert_eq!(result.as_boolean(), Some(false));
    
    // Poll idle tasks with a 50ms deadline
    let deadline = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64 + 50.0;
    runtime.tick(None, Some(deadline));
    
    // Should be called now
    let check_code = "typeof called !== 'undefined' && called === true";
    let result = runtime.context.eval(Source::from_bytes(check_code.as_bytes())).unwrap();
    assert_eq!(result.as_boolean(), Some(true));
}

#[test]
fn test_cancel_idle_callback() {
    let mut runtime = browser::js::JsRuntime::new(None, None, None, None, browser::js::new_console_buffer());
    
    let code = r#"
        let idleCalled = false;
        let id = requestIdleCallback(() => {
            idleCalled = true;
        });
        cancelIdleCallback(id);
        id;
    "#;
    
    runtime.context.eval(Source::from_bytes(code.as_bytes())).unwrap();
    
    // Poll idle tasks
    let deadline = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64 + 50.0;
    runtime.tick(None, Some(deadline));
    
    // Should NOT be called because it was canceled
    let result = runtime.context.eval(Source::from_bytes(b"idleCalled")).unwrap();
    assert_eq!(result.as_boolean(), Some(false));
}
*/
