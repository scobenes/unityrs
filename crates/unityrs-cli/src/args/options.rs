//! Shared option metadata and the parity parser's ordered lookup.
use super::Dest;

/// Metadata shared by the parity parser and the machine reference.
pub(crate) const fn option(
    name: &'static str,
    value: Option<&'static str>,
    default: Option<&'static str>,
    help: &'static str,
) -> crate::agent::options::OptionSpec {
    crate::agent::options::OptionSpec {
        name,
        value,
        default,
        help,
    }
}
pub(crate) const COMMAND_HELP: crate::agent::options::OptionSpec =
    option("--help", None, None, "Print command help (also -h).");
#[derive(Clone, Copy)]
pub(crate) struct DumpOption {
    pub spec: crate::agent::options::OptionSpec,
    pub(super) dest: Dest,
}
/// `_option_string_actions`, in the order `add_argument` fills it: an
/// ambiguous prefix lists its matches in this order.
pub(crate) const OPTIONS: [DumpOption; 21] = [
    DumpOption {
        spec: option("-h", None, None, "Print command help (also -h)."),
        dest: Dest::Help,
    },
    DumpOption {
        spec: option("--help", None, None, "Print command help (also -h)."),
        dest: Dest::Help,
    },
    DumpOption {
        spec: option(
            "-o",
            Some("PATH"),
            None,
            "Required dump JSONL output, optionally gzip.",
        ),
        dest: Dest::Output,
    },
    DumpOption {
        spec: option(
            "--output",
            Some("PATH"),
            None,
            "Required dump JSONL output, optionally gzip.",
        ),
        dest: Dest::Output,
    },
    DumpOption {
        spec: option(
            "--index",
            None,
            Some("false"),
            "Inputs are index JSON files.",
        ),
        dest: Dest::Index,
    },
    DumpOption {
        spec: option(
            "--kind",
            Some("K[,K...]"),
            Some("bundle,unitypy-sample"),
            "Index kinds to select.",
        ),
        dest: Dest::Kind,
    },
    DumpOption {
        spec: option("--build", Some("B[,B...]"), None, "Index builds to select."),
        dest: Dest::Build,
    },
    DumpOption {
        spec: option(
            "--allow-index-mismatch",
            None,
            Some("false"),
            "Warn rather than refuse an index hash mismatch.",
        ),
        dest: Dest::Allow,
    },
    DumpOption {
        spec: option(
            "--name",
            Some("GLOB"),
            None,
            "Repeatable glob on dump-relative input path.",
        ),
        dest: Dest::Name,
    },
    DumpOption {
        spec: option("--sample", Some("N"), None, "Evenly sample sorted inputs."),
        dest: Dest::Sample,
    },
    DumpOption {
        spec: option("--limit", Some("N"), None, "First N sorted inputs."),
        dest: Dest::Limit,
    },
    DumpOption {
        spec: option(
            "--levels",
            Some("L0,L1,L2,L3"),
            Some("L0,L1,L2,L3"),
            "Canonical levels to write.",
        ),
        dest: Dest::Levels,
    },
    DumpOption {
        spec: option(
            "-j",
            Some("N"),
            Some("1"),
            "Concurrent inputs; values below one mean one.",
        ),
        dest: Dest::Jobs,
    },
    DumpOption {
        spec: option(
            "--jobs",
            Some("N"),
            Some("1"),
            "Concurrent inputs; values below one mean one.",
        ),
        dest: Dest::Jobs,
    },
    DumpOption {
        spec: option("--stats", Some("PATH"), None, "Save timing JSON."),
        dest: Dest::Stats,
    },
    DumpOption {
        spec: option("-q", None, Some("false"), "Suppress per-input progress."),
        dest: Dest::Quiet,
    },
    DumpOption {
        spec: option(
            "--quiet",
            None,
            Some("false"),
            "Suppress per-input progress.",
        ),
        dest: Dest::Quiet,
    },
    DumpOption {
        spec: option(
            "--tpk",
            Some("FILE"),
            None,
            "Schema provider for stripped files.",
        ),
        dest: Dest::Tpk,
    },
    DumpOption {
        spec: option(
            "--reverse-prepass",
            None,
            Some("false"),
            "Visit entries in reverse before dumping.",
        ),
        dest: Dest::LazyReverse,
    },
    DumpOption {
        spec: option(
            "--unbounded",
            Some("KIND[,KIND...]"),
            None,
            "Lift named bounds for parity experiments.",
        ),
        dest: Dest::Unbounded,
    },
    DumpOption {
        spec: option("--mesh", None, Some("false"), "Include mesh digests."),
        dest: Dest::Mesh,
    },
];
// Preserve argparse's significant lookup order without a second option table.
pub(super) fn strings() -> impl Iterator<Item = (&'static str, Dest)> {
    OPTIONS.iter().map(|o| (o.spec.name, o.dest))
}
pub(crate) fn options() -> Vec<crate::agent::options::OptionSpec> {
    let mut entries: Vec<_> = OPTIONS.iter().filter(|o| o.spec.name != "-h").collect();
    // Keep the established reference order independently of argparse's
    // significant abbreviation order.
    entries.sort_by_key(|o| match o.dest {
        Dest::Tpk => 1,
        Dest::Help => 2,
        _ => 0,
    });
    entries.into_iter().map(|o| o.spec).collect()
}
