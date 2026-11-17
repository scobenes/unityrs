//! Tables used by both the parser and the machine reference.
use super::{Error, Result};
use unityrs_core::limits::Limits;

#[derive(Clone, Copy)]
pub struct OptionSpec {
    pub name: &'static str,
    pub value: Option<&'static str>,
    pub default: Option<&'static str>,
    pub help: &'static str,
}
const fn opt(
    name: &'static str,
    value: &'static str,
    default: Option<&'static str>,
    help: &'static str,
) -> OptionSpec {
    OptionSpec {
        name,
        value: Some(value),
        default,
        help,
    }
}
pub const BUDGET: &[OptionSpec] = &[
    opt(
        "--max-input-memory",
        "N",
        Some("8589934592"),
        "Charged bytes per input and response.",
    ),
    opt(
        "--max-input-bytes",
        "N",
        Some("4294967296"),
        "Source bytes per input.",
    ),
    opt(
        "--max-decode-work",
        "N",
        Some("34359738368"),
        "Work bytes per input and response.",
    ),
    opt(
        "--max-tree-depth",
        "N",
        Some("256"),
        "Reader tree and graph depth.",
    ),
    opt(
        "--tpk",
        "FILE",
        None,
        "Optional schema provider for stripped trees.",
    ),
];
pub const INSPECT: &[OptionSpec] = &[
    opt(
        "--class",
        "CLASS",
        None,
        "Class name or integer ID; repeat for a union.",
    ),
    opt(
        "--name",
        "GLOB",
        None,
        "Object name, using dump's case-sensitive glob syntax; repeat for a union.",
    ),
    opt("--limit", "N", Some("200"), "Page size, 1 through 10000."),
    opt(
        "--cursor",
        "TOKEN",
        None,
        "Continue this inventory with the same inputs and filters.",
    ),
];
pub const SHOW: &[OptionSpec] = &[
    opt("--path-id", "N", None, "Required signed object path ID."),
    opt(
        "--file",
        "NAME",
        None,
        "Exact SerializedFile name to disambiguate the path ID.",
    ),
    opt(
        "--max-bytes",
        "N",
        Some("65536"),
        "Maximum compact JSON document bytes, including its newline.",
    ),
    opt(
        "--max-items",
        "N",
        Some("256"),
        "Maximum retained entries in each array or map.",
    ),
    opt(
        "--max-depth",
        "N",
        Some("32"),
        "Maximum value nesting, 0 through 128.",
    ),
    opt(
        "--value",
        "natural|canonical",
        Some("natural"),
        "Value JSON representation; canonical preserves tagged types.",
    ),
    opt(
        "--bytes",
        "summary|full",
        Some("summary"),
        "Byte runs are summaries unless full hex fits.",
    ),
];
pub const DESCRIBE: &[OptionSpec] = &[opt(
    "--schema",
    "inspect|show|describe|error",
    None,
    "Print the named draft 2020-12 output schema.",
)];
pub const HELP: OptionSpec = OptionSpec {
    name: "--help",
    value: None,
    default: None,
    help: "Return the command description as JSON (also -h).",
};
pub fn specs(command: &str) -> Vec<OptionSpec> {
    let mut all = match command {
        "inspect" => INSPECT,
        "show" => SHOW,
        _ => DESCRIBE,
    }
    .to_vec();
    if command != "describe" {
        all.extend_from_slice(BUDGET);
    }
    all.push(HELP);
    all
}
pub struct Args<'a> {
    pub inputs: Vec<&'a [u8]>,
    pub options: Vec<(&'static str, &'a [u8])>,
    pub limits: Limits,
}
/// Bound raw argument bytes before escaping or allocating a diagnostic.
pub(super) fn spelling(raw: &[u8]) -> String {
    let mut text = String::from("'");
    for byte in raw.iter().take(96) {
        text.extend(std::ascii::escape_default(*byte).map(char::from));
    }
    if raw.len() > 96 {
        text.push_str("...");
    }
    text.push('\'');
    text
}
pub(super) fn bad_value(key: &str, raw: &[u8], expected: &str) -> Error {
    Error::usage(format!(
        "invalid {key} value {}; expected {expected}",
        spelling(raw)
    ))
}
fn unsigned<T: std::str::FromStr>(key: &str, raw: &[u8], expected: &str) -> Result<T> {
    std::str::from_utf8(raw)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad_value(key, raw, expected))
}
impl<'a> Args<'a> {
    pub fn parse(command: &str, raw: &'a [Vec<u8>]) -> Result<Self> {
        let specs = specs(command);
        let mut options = Vec::new();
        let mut inputs = Vec::new();
        let mut limits = Limits::default();
        let mut positional = false;
        let mut iter = raw.iter();
        while let Some(arg) = iter.next() {
            if !positional && arg == b"--" {
                positional = true;
                continue;
            }
            if !positional && arg.starts_with(b"-") {
                let key = if arg == b"-h" {
                    b"--help".as_slice()
                } else {
                    arg
                };
                let spec = specs
                    .iter()
                    .find(|s| s.name.as_bytes() == key)
                    .ok_or_else(|| Error::usage(format!("unknown option {}", spelling(arg))))?;
                let value = if spec.value.is_some() {
                    iter.next()
                        .ok_or_else(|| Error::usage(format!("missing value for {}", spec.name)))?
                        .as_slice()
                } else {
                    b"true"
                };
                if !matches!(spec.name, "--class" | "--name")
                    && options.iter().any(|(k, _)| *k == spec.name)
                {
                    return Err(Error::usage(format!("duplicate option {}", spec.name)));
                }
                options.push((spec.name, value));
                match spec.name {
                    "--max-input-memory" | "--max-input-bytes" | "--max-decode-work" => {
                        let n = unsigned(
                            spec.name,
                            value,
                            "an integer from 0 through 18446744073709551615",
                        )?;
                        match spec.name {
                            "--max-input-memory" => limits.max_input_memory = n,
                            "--max-input-bytes" => limits.max_input_bytes = n,
                            _ => limits.max_decode_work = n,
                        }
                    }
                    "--max-tree-depth" => {
                        limits.max_tree_depth =
                            unsigned(spec.name, value, "an integer from 0 through 4294967295")?;
                    }
                    _ => {}
                }
            } else {
                inputs.push(arg.as_slice());
            }
        }
        let args = Self {
            inputs,
            options,
            limits,
        };
        args.validate(command)?;
        Ok(args)
    }
    fn validate(&self, command: &str) -> Result<()> {
        if self.get("--help").is_some() {
            return Ok(());
        }
        if command == "describe" {
            if let Some(input) = self.inputs.first() {
                return Err(Error::usage(format!(
                    "describe takes no INPUT; unexpected INPUT {}",
                    spelling(input)
                )));
            }
            if let Some(value) = self.get("--schema")
                && !matches!(value, b"inspect" | b"show" | b"describe" | b"error")
            {
                return Err(bad_value(
                    "--schema",
                    value,
                    "inspect, show, describe or error",
                ));
            }
        } else if self.inputs.is_empty() {
            return Err(Error::usage("missing INPUT"));
        }
        if command == "inspect" {
            let n = self.number(command, "--limit")?;
            if !(1..=10_000).contains(&n) {
                return Err(bad_value(
                    "--limit",
                    self.value(command, "--limit"),
                    "an integer from 1 through 10000",
                ));
            }
        }
        if command == "show" {
            self.path_id()?;
            for key in ["--max-bytes", "--max-items", "--max-depth"] {
                self.number(command, key)?;
            }
            if self.number(command, "--max-depth")? > 128 {
                return Err(bad_value(
                    "--max-depth",
                    self.value(command, "--max-depth"),
                    "an integer from 0 through 128",
                ));
            }
            if !matches!(self.value(command, "--bytes"), b"summary" | b"full") {
                return Err(bad_value(
                    "--bytes",
                    self.value(command, "--bytes"),
                    "summary or full",
                ));
            }
            if !matches!(self.value(command, "--value"), b"natural" | b"canonical") {
                return Err(bad_value(
                    "--value",
                    self.value(command, "--value"),
                    "natural or canonical",
                ));
            }
        }
        Ok(())
    }
    pub fn get(&self, key: &str) -> Option<&'a [u8]> {
        self.options
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
    }
    pub fn value(&self, command: &str, key: &str) -> &[u8] {
        self.get(key).unwrap_or_else(|| {
            specs(command)
                .iter()
                .find(|s| s.name == key)
                .and_then(|s| s.default)
                .unwrap_or("")
                .as_bytes()
        })
    }
    pub fn number(&self, command: &str, key: &str) -> Result<usize> {
        let expected = match key {
            "--limit" => "an integer from 1 through 10000".to_owned(),
            "--max-depth" => "an integer from 0 through 128".to_owned(),
            _ => format!("an integer from 0 through {}", usize::MAX),
        };
        unsigned(key, self.value(command, key), &expected)
    }
    pub fn path_id(&self) -> Result<i64> {
        let raw = self.get("--path-id").ok_or_else(|| Error::usage("required option --path-id is missing; expected a signed integer from -9223372036854775808 through 9223372036854775807"))?;
        std::str::from_utf8(raw)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| {
                bad_value(
                    "--path-id",
                    raw,
                    "a signed integer from -9223372036854775808 through 9223372036854775807",
                )
            })
    }
}
