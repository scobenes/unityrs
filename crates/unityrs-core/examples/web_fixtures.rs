//! Emit in-code C2 inputs for tools/fixtures/web.py, or native replay summaries.
#[path = "../tests/support/web.rs"]
mod support;
fn main() {
    let inputs = support::inputs();
    if std::env::args().any(|a| a == "--summaries") {
        let answers:Vec<_>=inputs.as_array().unwrap().iter().map(|case| {
            let data=support::unhex(case["hex"].as_str().unwrap());
            serde_json::json!({"memory":support::summary(data.clone(),false),"stream":support::summary(data,true)})
        }).collect();
        println!("{}", serde_json::json!({"inputs":inputs,"answers":answers}));
    } else {
        println!("{inputs}");
    }
}
