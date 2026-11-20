//! `unityrs-bench gen --scale full|gate DIR`: writes every fixture and
//! `manifest.json` into DIR (`docs/plans/bench.md` section 2).

use std::path::Path;
use std::process::ExitCode;

use unityrs_bench::{Scale, generate};

const USAGE: &str = "usage: unityrs-bench gen --scale full|gate DIR";

fn gen_into(scale: Scale, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let manifest = generate(scale, |fixture| {
        std::fs::write(dir.join(&fixture.name), &fixture.bytes)
    })
    .map_err(|e| e.to_string())?;
    let mut text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(dir.join("manifest.json"), text).map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match args.as_slice() {
        [cmd, flag, scale, dir] if cmd == "gen" && flag == "--scale" => {
            Scale::parse(scale).map(|s| (s, Path::new(dir)))
        }
        _ => None,
    };
    let Some((scale, dir)) = parsed else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match gen_into(scale, dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("unityrs-bench gen: {why}");
            ExitCode::FAILURE
        }
    }
}
