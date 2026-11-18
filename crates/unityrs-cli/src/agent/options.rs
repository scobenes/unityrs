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
        "Maximum charged memory in bytes per input and for the separate response or extraction budget.",
    ),
    opt(
        "--max-input-bytes",
        "N",
        Some("4294967296"),
        "Maximum source file size in bytes per input, checked before and during loading.",
    ),
    opt(
        "--max-decode-work",
        "N",
        Some("34359738368"),
        "Maximum cumulative work in byte units per input and for response rendering or extraction, including decoding and hashing.",
    ),
    opt(
        "--max-tree-depth",
        "N",
        Some("256"),
        "Maximum compiled typetree nesting depth in levels; separate from display depth and fixed runtime reader limits.",
    ),
    opt(
        "--tpk",
        "FILE",
        None,
        "Optional local TPK schema-provider file for assets with stripped typetrees; no provider is loaded by default.",
    ),
];
pub const INSPECT: &[OptionSpec] = &[
    opt(
        "--class",
        "CLASS",
        None,
        "Class names or integer IDs to include; multiple values form a union, intersected with any name filters. Omit to include all classes.",
    ),
    opt(
        "--name",
        "GLOB",
        None,
        "Case-sensitive object-name globs (*, ?, [abc], [!abc]); multiple values form a union, intersected with any class filters. Omit to include unnamed objects too.",
    ),
    OptionSpec {
        name: "--external-paths",
        value: None,
        default: None,
        help: "Include SerializedFile external paths in PPtr file-ID order on each page when present.",
    },
    opt(
        "--limit",
        "N",
        Some("200"),
        "Maximum objects returned per page, 1 through 10000; the response byte cap may reduce the page further.",
    ),
    opt(
        "--max-bytes",
        "N",
        Some("65536"),
        "Maximum compact JSON document bytes, including its newline; strings use bounded previews.",
    ),
    opt(
        "--cursor",
        "TOKEN",
        None,
        "Pass the previous response's non-null next_cursor string unchanged; omit for the first page and stop when next_cursor is null. Keep input spellings, order, contents, filters, --external-paths and explicitly supplied budget/provider options unchanged.",
    ),
];
pub const SHOW: &[OptionSpec] = &[
    opt(
        "--path-id",
        "N",
        None,
        "Required signed object path_id from inspect; use a file selector if several files contain the same ID.",
    ),
    opt(
        "--file",
        "NAME",
        None,
        "Optional exact SerializedFile name from inspect's file field, not an input path; use file_index if names repeat. If both selectors are supplied, both must match.",
    ),
    opt(
        "--file-index",
        "INPUT:FILE",
        None,
        "Optional INPUT:FILE string using inspect's zero-based input and file_index numbers (for example 0:2); selects one exact file occurrence even when names repeat. Other inputs are not loaded.",
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
        "Maximum retained data entries per array or map; zero shows only a summary and cuts are marked.",
    ),
    opt(
        "--max-depth",
        "N",
        Some("32"),
        "Maximum displayed value nesting depth in levels, 0 through 128, with root at zero; deeper subtrees are replaced by cut markers.",
    ),
    opt(
        "--value",
        "natural|canonical",
        Some("natural"),
        "Natural emits ordinary JSON with tags for exceptional values; canonical uses the dump encoder's tagged dictionaries, tuples and float bits. Untruncated canonical values match the dump encoder.",
    ),
    opt(
        "--bytes",
        "summary|full",
        Some("summary"),
        "In the natural view, summary returns byte length, SHA-256 and a hex prefix; full returns all hex only when it fits the byte cap. Canonical ignores this option and uses the dump's raw-byte rule.",
    ),
];
pub const DESCRIBE: &[OptionSpec] = &[opt(
    "--schema",
    "inspect|show|describe|error",
    None,
    "Return the named draft 2020-12 output JSON Schema; omit to get the command and option reference instead.",
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
    pub roots: Option<std::sync::Arc<crate::mcp::roots::Roots>>,
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
                    return Err(Error::usage(format!(
                        "duplicate option {} value {}; first value {}",
                        spec.name,
                        spelling(value),
                        spelling(
                            options
                                .iter()
                                .find(|(key, _)| *key == spec.name)
                                .map_or(b"", |(_, value)| *value)
                        )
                    )));
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
            roots: None,
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
            self.number(command, "--max-bytes")?;
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
            self.file_index()?;
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
    pub fn file_index(&self) -> Result<Option<(usize, usize)>> {
        let Some(raw) = self.get("--file-index") else {
            return Ok(None);
        };
        let bad = || {
            bad_value(
                "--file-index",
                raw,
                "INPUT:FILE (two zero-based integer indices)",
            )
        };
        let (input, file) = std::str::from_utf8(raw)
            .ok()
            .and_then(|s| s.split_once(':'))
            .ok_or_else(bad)?;
        if ![input, file]
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(bad());
        }
        Ok(Some((
            input.parse().map_err(|_| bad())?,
            file.parse().map_err(|_| bad())?,
        )))
    }
}
