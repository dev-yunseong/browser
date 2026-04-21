/// Performance test: large Bootstrap-like CSS with many complex selectors.
/// Must complete within a reasonable time (< 30s even in debug mode).

use browser::engine::process_html_with_cache;
use std::collections::HashMap;
use url::Url;

fn base_url() -> Url { Url::parse("https://test.com/").unwrap() }

fn run(html: &str) -> browser::engine::PageResult {
    let image_cache = HashMap::new();
    let mut css_cache = HashMap::new();
    let js_overrides = HashMap::new();
    let (result, _) = process_html_with_cache(
        html, &base_url(), &image_cache, &mut css_cache,
        None, &js_overrides, None, None, None, 800.0,
    ).expect("pipeline should not fail");
    result
}

#[test]
fn test_large_css_does_not_hang() {
    use std::time::Instant;
    
    // 2700 rules — simulating Bootstrap-like CSS scale:
    // 2000 simple class rules + 500 complex descendant rules + 200 tag rules
    let mut css = String::new();
    for i in 0..2000_usize {
        css.push_str(&format!(".rule-{} {{ color: red; margin: {}px 0; }}\n", i, i % 20));
    }
    for i in 0..500_usize {
        css.push_str(&format!(".parent-{} .child-{} {{ background: #fff; }}\n", i % 100, i % 100));
    }
    for i in 0..200_usize {
        css.push_str(&format!("div.variant-{} {{ font-size: {}px; }}\n", i % 10, 14 + i % 10));
    }
    
    // 500 elements with mixed classes
    let elements: String = (0..500_usize).map(|i| {
        format!("<div class=\"parent-{} rule-{} variant-{}\"><p class=\"child-{}\">Para {}</p></div>",
                i % 100, i % 2000, i % 10, i % 100, i)
    }).collect();
    
    let html = format!("<style>{}</style>{}", css, elements);
    
    let start = Instant::now();
    let result = run(&html);
    let elapsed = start.elapsed();
    
    println!("Large CSS pipeline: {:?}", elapsed);
    assert!(result.height > 0);
    // Should complete in under 30s even in debug mode
    assert!(elapsed.as_secs() < 30, "Pipeline took too long: {:?}", elapsed);
}

#[test]
fn test_render_time_scaling() {
    use std::time::Instant;
    
    // Test with 50 elements
    let elements_50: String = (0..50_usize).map(|i| {
        format!("<div class=\"parent-{} rule-{} variant-{}\"><p class=\"child-{}\">Para {}</p></div>",
                i % 100, i % 2000, i % 10, i % 100, i)
    }).collect();
    let html_50 = format!("<style>.parent-1 .child-1 {{ background: white; }}</style>{}", elements_50);
    
    let start = Instant::now();
    let result_50 = run(&html_50);
    let elapsed_50 = start.elapsed();
    println!("50 elements: {:?}, height: {}", elapsed_50, result_50.height);
    
    // Test with 100 elements
    let elements_100: String = (0..100_usize).map(|i| {
        format!("<div><p>Para {}</p></div>", i)
    }).collect();
    let html_100 = elements_100;
    
    let start = Instant::now();
    let result_100 = run(&html_100);
    let elapsed_100 = start.elapsed();
    println!("100 elements (no extra CSS): {:?}, height: {}", elapsed_100, result_100.height);
    
    assert!(result_50.height > 0);
    assert!(result_100.height > 0);
}
