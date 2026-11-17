use super::roots::Roots;
use crate::{
    agent::options::{BUDGET, OptionSpec},
    args::option,
    extract::files::Files,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
pub struct Config {
    pub roots: Arc<Roots>,
    pub output: Option<Files>,
    pub maxima: BTreeMap<String, u64>,
    pub request_bytes: usize,
    pub response_bytes: usize,
}
pub fn options() -> Vec<OptionSpec> {
    let mut specs = vec![
        option(
            "--root",
            Some("DIR"),
            None,
            "Repeatable held input root; required.",
        ),
        option(
            "--out",
            Some("DIR"),
            None,
            "Held extraction output root; enables unityrs_extract.",
        ),
        option(
            "--max-request-bytes",
            Some("N"),
            Some("1048576"),
            "Maximum bytes per request line, including newline; 256 through 67108864.",
        ),
        option(
            "--max-response-bytes",
            Some("N"),
            Some("4194304"),
            "Maximum response bytes including newline; at least request cap + 1024, at most 67108864.",
        ),
        option(
            "--max-bytes",
            Some("N"),
            Some("65536"),
            "Maximum inspect/show document bytes.",
        ),
        option(
            "--max-items",
            Some("N"),
            Some("256"),
            "Maximum show retained entries.",
        ),
        option(
            "--max-depth",
            Some("N"),
            Some("32"),
            "Maximum show display depth, at most 128.",
        ),
        option(
            "--limit",
            Some("N"),
            Some("200"),
            "Maximum inspect page size, 1 through 10000.",
        ),
    ];
    specs.extend(BUDGET.iter().filter(|s| s.name != "--tpk").copied());
    specs.push(crate::args::COMMAND_HELP);
    specs
}
pub fn help() -> String {
    options()
        .iter()
        .map(|s| format!("{} {}: {}", s.name, s.value.unwrap_or_default(), s.help))
        .collect::<Vec<_>>()
        .join("\n")
}
impl Config {
    pub fn limits(&self) -> unityrs_core::limits::Limits {
        let mut limits = unityrs_core::limits::Limits::default();
        limits.max_input_memory = self.maxima.get("--max-input-memory").copied().unwrap_or(0);
        limits.max_input_bytes = self.maxima.get("--max-input-bytes").copied().unwrap_or(0);
        limits.max_decode_work = self.maxima.get("--max-decode-work").copied().unwrap_or(0);
        limits
    }

    pub fn parse(raw: &[Vec<u8>]) -> Result<Self, Box<dyn std::error::Error>> {
        let specs = options();
        let mut maxima: BTreeMap<String, u64> = specs
            .iter()
            .filter_map(|s| Some((s.name.to_owned(), s.default?.parse().ok()?)))
            .collect();
        let mut roots = Vec::new();
        let mut out = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut iter = raw.iter();
        while let Some(arg) = iter.next() {
            let key = std::str::from_utf8(arg)?;
            let spec = specs
                .iter()
                .find(|s| s.name == key && s.value.is_some())
                .ok_or("unexpected argument")?;
            let value = std::str::from_utf8(iter.next().ok_or("missing argument")?)?;
            if key != "--root" && !seen.insert(spec.name) {
                return Err("duplicate option".into());
            }
            match key {
                "--root" => roots.push(PathBuf::from(value)),
                "--out" => out = Some(PathBuf::from(value)),
                _ => {
                    maxima.insert(key.to_owned(), value.parse()?);
                }
            }
        }
        if roots.is_empty() {
            return Err("at least one --root DIR is required".into());
        }
        let get = |key: &str| maxima.get(key).copied().unwrap_or(0);
        let request_bytes = usize::try_from(get("--max-request-bytes"))?;
        let response_bytes = usize::try_from(get("--max-response-bytes"))?;
        if !(256..=67_108_864).contains(&request_bytes)
            || response_bytes < request_bytes.saturating_add(1024)
            || response_bytes > 67_108_864
        {
            return Err("invalid framing caps: response must be at least request + 1024; caps at most 67108864".into());
        }
        if get("--max-depth") > 128
            || !(1..=10_000).contains(&get("--limit"))
            || get("--max-tree-depth") > u64::from(u32::MAX)
        {
            return Err("invalid display or tree limit".into());
        }
        let roots = Arc::new(Roots::new(&roots)?);
        let output = out
            .map(|path| {
                Files::new(
                    &path,
                    &crate::extract::limits::extraction(&unityrs_core::limits::Limits::default()),
                )
            })
            .transpose()?;
        Ok(Self {
            roots,
            output,
            maxima,
            request_bytes,
            response_bytes,
        })
    }
}
