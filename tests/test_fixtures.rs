use browser::engine;
use std::collections::HashMap;
use url::Url;

fn fixture_path(name: &str) -> String {
    std::fs::canonicalize(format!("tests/fixtures/{}", name))
        .unwrap()
        .to_string_lossy()
        .to_string()
}

fn load_fixture(name: &str) -> (String, Url) {
    let path = fixture_path(name);
    let html = std::fs::read_to_string(&path).expect("failed to read fixture");
    let base_url = Url::from_file_path(&path).expect("failed to construct file:// URL");
    (html, base_url)
}

fn engine_with_fixture(name: &str) -> browser::engine::BrowserEngine {
    let (html, base_url) = load_fixture(name);
    let mut css_cache = HashMap::new();
    let (page, _) = engine::process_html_with_cache(
        &html,
        &base_url,
        &HashMap::new(),
        &mut css_cache,
        None,
        &HashMap::new(),
        None,
        None,
        None,
        800.0,
    )
    .expect("process_html_with_cache");
    let mut engine = browser::engine::BrowserEngine::new();
    engine.init_js_for_page(&page);
    engine
}

fn get_results(engine: &mut browser::engine::BrowserEngine) -> String {
    engine.evaluate_js(
        "JSON.stringify((window.__results || []).map(function(r) { return r.phase + ':' + r.value; }))",
    )
}

fn get_result_phases(engine: &mut browser::engine::BrowserEngine) -> Vec<String> {
    let json = engine.evaluate_js(
        "JSON.stringify((window.__results || []).map(function(r) { return r.phase; }))",
    );
    serde_json::from_str::<Vec<String>>(&json).unwrap_or_default()
}

#[test]
fn test_inline_classic_script_executes_and_modifies_dom() {
    let mut engine = engine_with_fixture("inline-classic.html");
    let text = engine.evaluate_js("document.getElementById('target').textContent");
    assert_eq!(text, "modified-by-classic");
}

#[test]
fn test_inline_classic_ordering_sync_before_defer() {
    let mut engine = engine_with_fixture("inline-classic.html");
    let phases = get_result_phases(&mut engine);
    let sync1_idx = phases.iter().position(|p| p == "classic").unwrap();
    let defer_idx = phases
        .iter()
        .position(|p| p == "classic-defer")
        .unwrap();
    let sync2_idx = phases
        .iter()
        .position(|p| p == "classic-sync2")
        .unwrap();
    assert!(sync1_idx < sync2_idx, "sync1 before sync2");
    assert!(sync2_idx < defer_idx, "sync2 before deferred: {:?}", phases);
}

#[test]
fn test_inline_module_executes_and_modifies_dom() {
    let mut engine = engine_with_fixture("inline-module.html");
    let text = engine.evaluate_js("document.getElementById('target').textContent");
    assert_eq!(text, "modified-by-module");
}

#[test]
fn test_module_defer_semantics_after_classics() {
    let mut engine = engine_with_fixture("inline-module.html");
    let phases = get_result_phases(&mut engine);
    let classic_idx = phases.iter().position(|p| p == "classic").unwrap();
    let module_idx = phases.iter().position(|p| p == "module").unwrap();
    assert!(
        classic_idx < module_idx,
        "classic before module (defer semantics): {:?}",
        phases
    );
}

#[test]
fn test_module_dom_mutations() {
    let mut engine = engine_with_fixture("module-dom-mutation.html");
    assert_eq!(
        engine.evaluate_js("document.getElementById('text-content').textContent"),
        "new-text"
    );
    assert_eq!(
        engine.evaluate_js("document.getElementById('inner-html').innerHTML"),
        "<span>new-inner</span>"
    );
    assert_eq!(
        engine.evaluate_js("document.getElementById('set-attr').getAttribute('data-x')"),
        "new-attr"
    );
    assert_eq!(
        engine.evaluate_js("document.getElementById('form-input').value"),
        "new-value"
    );
    let results = get_results(&mut engine);
    assert!(results.contains("textContent:done"));
    assert!(results.contains("innerHTML:done"));
    assert!(results.contains("setAttribute:done"));
    assert!(results.contains("formValue:done"));
}

#[test]
fn test_module_style_overrides() {
    let engine = engine_with_fixture("module-style-override.html");
    let color = engine
        .js_style_overrides
        .get("test")
        .and_then(|p| p.get("color").cloned());
    let font_size = engine
        .js_style_overrides
        .get("test")
        .and_then(|p| p.get("font-size").cloned());
    let display = engine
        .js_style_overrides
        .get("test")
        .and_then(|p| p.get("display").cloned());
    assert_eq!(color, Some("red".to_string()));
    assert_eq!(font_size, Some("24px".to_string()));
    assert_eq!(display, Some("none".to_string()));
}

#[test]
fn test_module_tick_timer() {
    let mut engine = engine_with_fixture("module-tick-timer.html");
    assert_eq!(engine.evaluate_js("globalThis.__timer_fired"), "false");
    assert_eq!(
        engine.evaluate_js("document.getElementById('timer-target').textContent"),
        "waiting"
    );
    engine.tick_js(Some(20.0), None);
    assert_eq!(engine.evaluate_js("globalThis.__timer_fired"), "true");
    assert_eq!(
        engine.evaluate_js("document.getElementById('timer-target').textContent"),
        "timer-done"
    );
}

#[test]
fn test_module_console_log() {
    let engine = engine_with_fixture("module-console-log.html");
    let entries = engine.console_entries();
    let messages: Vec<&str> = entries.iter().map(|e| e.message.as_str()).collect();
    assert!(
        messages.contains(&"hello from module"),
        "console.log should be captured"
    );
    assert!(
        messages.contains(&"warning from module"),
        "console.warn should be captured"
    );
    assert!(
        messages.contains(&"error from module"),
        "console.error should be captured"
    );
    assert!(
        messages.contains(&"info from module"),
        "console.info should be captured"
    );
}

#[test]
fn test_import_meta_url() {
    let mut engine = engine_with_fixture("import-meta.html");
    let result = get_results(&mut engine);
    assert!(result.contains("import-meta-url:"));
    assert!(result.contains("file://"));
    assert!(result.contains("import-meta.html"));
}

#[test]
fn test_classic_module_hybrid_ordering() {
    let mut engine = engine_with_fixture("classic-module-hybrid.html");
    let phases = get_result_phases(&mut engine);
    let sync1 = phases.iter().position(|p| p == "classic-sync1").unwrap();
    let sync2 = phases.iter().position(|p| p == "classic-sync2").unwrap();
    let classic_deferred = phases
        .iter()
        .position(|p| p == "classic-deferred")
        .unwrap();
    let module = phases.iter().position(|p| p == "module").unwrap();
    let async_module = phases
        .iter()
        .position(|p| p == "async-module")
        .unwrap();

    assert!(sync1 < sync2, "sync1 before sync2");
    assert!(
        sync2 < classic_deferred,
        "sync2 before classic-deferred: {:?}",
        phases
    );
    assert!(
        classic_deferred < module,
        "classic-deferred before module: {:?}",
        phases
    );
    assert!(
        module < async_module,
        "module before async-module: {:?}",
        phases
    );
}

#[test]
fn test_nomodule_fallback() {
    let mut engine = engine_with_fixture("nomodule-fallback.html");
    let phases = get_result_phases(&mut engine);
    assert!(
        !phases.contains(&"nomodule-classic".to_string()),
        "nomodule script should NOT execute when ES modules are supported: {:?}",
        phases
    );
    assert!(
        phases.contains(&"module".to_string()),
        "type=module should execute: {:?}",
        phases
    );
}

#[test]
fn test_defer_async_script_ordering() {
    let mut engine = engine_with_fixture("defer-async-ordering.html");
    let phases = get_result_phases(&mut engine);
    let sync1 = phases.iter().position(|p| p == "sync1").unwrap();
    let sync2 = phases.iter().position(|p| p == "sync2").unwrap();
    let deferred = phases.iter().position(|p| p == "deferred").unwrap();
    let async_s = phases.iter().position(|p| p == "async").unwrap();

    assert!(sync1 < sync2, "sync1 before sync2");
    assert!(
        async_s < sync2,
        "inline async runs synchronously (async only applies to external scripts): {:?}",
        phases
    );
    assert!(sync2 < deferred, "sync2 before deferred: {:?}", phases);
}

#[test]
fn test_fixture_spans_script_types() {
    for fixture in &[
        "inline-classic.html",
        "inline-module.html",
        "module-dom-mutation.html",
        "import-meta.html",
        "classic-module-hybrid.html",
    ] {
        let mut engine = engine_with_fixture(fixture);
        assert!(
            get_results(&mut engine).len() > 0,
            "fixture {} should produce results",
            fixture
        );
    }
}

#[test]
fn test_aura_fetch_cors_bypass() {
    let mut engine = engine_with_fixture("inline-classic.html");

    // 1. Fetch with bypass_cors = false (should fail due to CORS check on cross-origin request)
    engine.evaluate_js(
        "globalThis.fetchResult = null; \
         globalThis.fetchError = null; \
         __aura_fetch( \
             'https://www.google.com/', \
             'GET', \
             '', \
             '', \
             function(r) { globalThis.fetchResult = 'success'; }, \
             function(e) { globalThis.fetchError = String(e); }, \
             false \
         );"
    );

    let mut passed = false;
    for _ in 0..100 {
        engine.tick_js(Some(1.0), None);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let error = engine.evaluate_js("globalThis.fetchError");
        if error.contains("CORS Error") {
            passed = true;
            break;
        }
    }
    assert!(passed, "CORS should be enforced when bypassCors is false");

    // 2. Fetch with bypass_cors = true (should succeed)
    engine.evaluate_js(
        "globalThis.fetchResult = null; \
         globalThis.fetchError = null; \
         __aura_fetch( \
             'https://www.google.com/', \
             'GET', \
             '', \
             '', \
             function(r) { globalThis.fetchResult = 'success'; }, \
             function(e) { globalThis.fetchError = String(e); }, \
             true \
         );"
    );

    let mut passed = false;
    for _ in 0..100 {
        engine.tick_js(Some(1.0), None);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let res = engine.evaluate_js("globalThis.fetchResult");
        if res == "success" {
            passed = true;
            break;
        }
    }
    assert!(passed, "CORS should be bypassed when bypassCors is true");
}

