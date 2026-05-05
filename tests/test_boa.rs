/*
use boa_engine::{Context, Source, js_string};

fn main() {
    let mut context = Context::default();
    let val = context.eval(Source::from_bytes(b"({})")).unwrap();
    let obj = val.as_object().unwrap();
    let _ = obj.set(js_string!("status"), 200, false, &mut context);
    println!("Success");
}
*/
