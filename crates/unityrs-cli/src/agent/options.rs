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
impl<'a> Args<'a> {
    pub fn parse(command: &str, raw: &'a [Vec<u8>]) -> Result<Self> {
        let specs = specs(command);
        let mut options = Vec::new();
        let mut inputs = Vec::new();
        let mut budgets = Vec::new();
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
                    .ok_or_else(|| Error::usage("unknown option"))?;
                let value = if spec.value.is_some() {
                    iter.next()
                        .ok_or_else(|| Error::usage("missing option value"))?
                        .as_slice()
                } else {
                    b"true"
                };
                if !matches!(spec.name, "--class" | "--name")
                    && options.iter().any(|(k, _)| *k == spec.name)
                {
                    return Err(Error::usage("duplicate option"));
                }
                options.push((spec.name, value));
                if BUDGET.iter().any(|s| s.name == spec.name) {
                    budgets.push(spec.name.as_bytes().to_vec());
                    budgets.push(value.to_vec());
                }
            } else {
                inputs.push(arg.as_slice());
            }
        }
        let limits = crate::extract::limits::parse(&budgets)
            .map_err(|_| Error::usage("invalid budget value"))?;
        let args = Self {
            inputs,
            options,
            limits,
        };
        if args.get("--help").is_some() {
            return Ok(args);
        }
        if command == "describe" {
            if !args.inputs.is_empty() {
                return Err(Error::usage("describe takes no inputs"));
            }
        } else if args.inputs.is_empty() {
            return Err(Error::usage("missing INPUT"));
        }
        if command == "inspect" {
            let n = args.number(command, "--limit")?;
            if !(1..=10_000).contains(&n) {
                return Err(Error::usage("--limit must be 1 through 10000"));
            }
        }
        if command == "show" {
            args.path_id()?;
            for key in ["--max-bytes", "--max-items", "--max-depth"] {
                args.number(command, key)?;
            }
            if args.number(command, "--max-depth")? > 128 {
                return Err(Error::usage("--max-depth must be at most 128"));
            }
            if !matches!(args.value(command, "--bytes"), b"summary" | b"full") {
                return Err(Error::usage("--bytes must be summary or full"));
            }
        }
        Ok(args)
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
        std::str::from_utf8(self.value(command, key))
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::usage(format!("invalid {key}")))
    }
    pub fn path_id(&self) -> Result<i64> {
        self.get("--path-id")
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::usage("missing or invalid --path-id"))
    }
}
