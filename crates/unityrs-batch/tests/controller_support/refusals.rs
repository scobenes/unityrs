//! Addendum 3 / CR10: exact allocating functions, independent of allocation
//! ordinals and sizes. Each entry must still abort in every named scope on
//! its platform. Every other refusal must return `host_memory`/74.

#[derive(Clone, Copy)]
pub enum Platform {
    All,
    Darwin,
}

pub struct Exclusion {
    pub scopes: &'static [&'static str],
    pub function: &'static str,
    pub platform: Platform,
    pub name: &'static str,
    /// A third-party allocation: the frame the raw trace must also show,
    /// since `innermost` keys on our caller and skips the registry's frames.
    pub frame: Option<&'static str>,
}

impl Exclusion {
    pub fn applies(&self, scope: &str) -> bool {
        self.scopes.contains(&scope)
            && match self.platform {
                Platform::All => true,
                Platform::Darwin => cfg!(target_vendor = "apple"),
            }
    }
}

pub const INVENTORY: &[Exclusion] = &[
    Exclusion {
        scopes: &["peek", "attempt", "complete"],
        function: "unityrs_core::budget::charged_bytes",
        platform: Platform::All,
        frame: None,
        name: "Bytes::from_owner owner box",
    },
    Exclusion {
        scopes: &["peek", "attempt", "complete"],
        function: "unityrs_core::files::RawFile::with_reader",
        platform: Platform::All,
        frame: None,
        name: "RawFile Arc",
    },
    Exclusion {
        scopes: &["peek"],
        function: "unityrs_export::ngui::classify::Reason::plain",
        platform: Platform::All,
        frame: None,
        name: "owned NGUI reason",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::files::bundle::BundleFile::open_with_reader",
        platform: Platform::All,
        frame: None,
        name: "parsed-container cyclic Arc",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::env::Environment::new",
        platform: Platform::All,
        frame: None,
        name: "Environment and DepCache Arcs",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::limits::meter::Meter::with_decode_work",
        platform: Platform::All,
        frame: None,
        name: "meter and account Arcs",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::env::Environment::try_load_dep",
        platform: Platform::All,
        frame: None,
        name: "dependency Arc",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::env::registry::Registry::new_slot",
        platform: Platform::All,
        frame: None,
        name: "registry path owner and Slot Arcs in prepare",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::env::registry::Registry::apply",
        platform: Platform::All,
        frame: None,
        name: "registry maps",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::files::bundle::index::RunIndex::publish",
        platform: Platform::All,
        frame: None,
        name: "run-index BTreeMap",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::env::registry::lru::Lru::use_key",
        platform: Platform::All,
        frame: None,
        name: "dependency-cache HashMap in touch_paid",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_export::ngui::read::head_rows",
        platform: Platform::All,
        frame: None,
        name: "Reader head-row Vecs",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::typetree::rows::NodeRow::new",
        platform: Platform::All,
        frame: None,
        name: "Reader head-row type and name copies",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::limits::meter::Meter::unlimited",
        platform: Platform::All,
        frame: None,
        name: "head-tree meter Arcs through NodeList::trusted",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_export::ngui::read::Reader::new",
        platform: Platform::All,
        frame: None,
        name: "head NodeList and Tree Arcs",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::typetree::tree::Tree::compile_inner",
        platform: Platform::All,
        frame: None,
        name: "compiled Tree Arc",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_export::ngui::keys::Template::clone",
        platform: Platform::All,
        frame: None,
        name: "Attempt::plan template piece Vec clone",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_export::ngui::keys::Piece::clone",
        platform: Platform::All,
        frame: None,
        name: "Attempt::plan template text Vec clones",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::files::bundle::BundleFile::cached",
        platform: Platform::Darwin,
        frame: None,
        name: "Darwin lazy mutex box in read_span_pinned",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        function: "unityrs_core::env::held::lock",
        platform: Platform::Darwin,
        frame: None,
        name: "Darwin lazy mutex box in DepCache::touch_paid",
    },
    // The one admitted third-party frame: tokio's `spawn_blocking`
    // allocates each task's cell with `Box::new`, which aborts on refusal.
    // There is no fallible submission in tokio's API, and a pool task is how
    // every blocking write is kept off the controller (`docs/plans/batch.md`
    // 2.11), so the refusal is the documented abort (A13: status 134 from
    // step 4). Everything else `Execution::spawn` allocates is fallible, and
    // the prewarm leaves tokio no thread to start or queue to grow.
    Exclusion {
        scopes: SPAWNING,
        function: "unityrs_batch::execution::Execution::spawn",
        platform: Platform::All,
        frame: Some("tokio::runtime::task::core::Cell"),
        name: "tokio spawn_blocking task cell",
    },
];
/// The scopes whose armed span submits pool tasks.
const SPAWNING: &[&str] = &["stage", "peek", "attempt", "failure", "complete", "archive"];

impl Exclusion {
    /// Whether a refusal's trace, whose innermost owner is `function`, is
    /// this entry's: ours by name, a third party's by name and frame.
    pub fn admits(&self, function: Option<&str>, trace: &str) -> bool {
        function == Some(self.function)
            && match self.frame {
                Some(frame) => trace.contains(frame),
                None => {
                    self.function.starts_with("unityrs_core::")
                        || self.function.starts_with("unityrs_export::")
                }
            }
    }
}

/// Only function symbols count. Generic arguments may mention our types in
/// std/alloc functions; trait implementations supplied by std (e.g. slice's
/// `ConvertVec` for `Piece`) are also foreign, as their source location shows.
/// Cargo's sweep profile `debug = "limited"` keeps qualified inline function names.
pub fn innermost(stderr: &str) -> Option<String> {
    let trace = stderr.split_once("REFUSED_ALLOCATION ")?.1;
    let (trace, _) = trace.split_once("END_REFUSED_ALLOCATION")?;
    let mut lines = trace.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((index, symbol)) = line.trim().split_once(": ") else {
            continue;
        };
        if index.parse::<usize>().is_err() {
            continue;
        }
        if lines.peek().is_some_and(|source| {
            let source = source.trim();
            source.starts_with("at ")
                && (source.contains("/rustc/") || source.contains("/.cargo/registry/"))
        }) {
            continue;
        }
        let Some(function) = function_key(symbol) else {
            continue;
        };
        // These are the test's allocator plumbing, not the allocation's
        // caller. No other batch/CLI function may be skipped or excluded.
        if function.starts_with("unityrs_batch::alloc::Counting::")
            || function == "unityrs_batch::alloc::no_unwind"
        {
            continue;
        }
        return Some(function);
    }
    None
}

fn function_key(symbol: &str) -> Option<String> {
    let symbol = if let Some(inner) = symbol.strip_prefix('<') {
        let mut depth = 1;
        let end = inner.char_indices().find_map(|(i, c)| {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i)
        })?;
        let owner = inner[..end].split(" as ").next()?;
        format!("{owner}{}", &inner[end + 1..])
    } else {
        symbol.to_owned()
    };
    if ![
        "unityrs_core::",
        "unityrs_export::",
        "unityrs_batch::",
        "unityrs_cli::",
        "unityrs::",
    ]
    .iter()
    .any(|prefix| symbol.starts_with(prefix))
    {
        return None;
    }
    // Monomorphizations and closure numbers are not distinct functions.
    let mut depth = 0;
    let mut key = String::new();
    for c in symbol.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 => key.push(c),
            _ => {}
        }
    }
    Some(
        key.split("::{closure")
            .next()?
            .trim_end_matches(':')
            .to_owned(),
    )
}

#[test]
fn the_innermost_owner_cannot_be_hidden_by_an_allowed_ancestor() {
    let trace = "REFUSED_ALLOCATION site=99 size=64\n\
        0: <controller_alloc::Refuse as unityrs_batch::alloc::Hook>::refuses\n\
        1: unityrs_batch::alloc::no_unwind::<bool, controller_alloc::Refuse>\n\
        2: <unityrs_batch::alloc::Counting<controller_alloc::Refuse> as core::alloc::global::GlobalAlloc>::alloc\n\
        3: <alloc::sync::Arc<unityrs_core::env::Environment>>::new\n\
        4: unityrs_batch::attempt::run_reserved\n\
        5: <unityrs_core::env::Environment>::new\n\
        END_REFUSED_ALLOCATION";
    assert_eq!(
        innermost(trace).as_deref(),
        Some("unityrs_batch::attempt::run_reserved")
    );
    assert_eq!(
        innermost(&trace.replace("unityrs_batch::attempt", "unityrs_cli::batch")).as_deref(),
        Some("unityrs_cli::batch::run_reserved")
    );
    // The CLI package's production binary is named `unityrs`.
    assert_eq!(
        innermost(&trace.replace("unityrs_batch::attempt", "unityrs::batch")).as_deref(),
        Some("unityrs::batch::run_reserved")
    );
}

#[test]
fn inline_functions_are_keys_but_std_implementations_and_type_mentions_are_not() {
    let trace = "REFUSED_ALLOCATION site=3 size=8\n\
        0: <unityrs_export::ngui::keys::Piece as <[_]>::to_vec_in::ConvertVec>::to_vec::<alloc::alloc::Global>\n\
                     at /rustc/hash/library/alloc/src/slice.rs:448:29\n\
        1: <unityrs_export::ngui::keys::Template as core::clone::Clone>::clone\n\
                     at /work/crates/unityrs-export/src/ngui/keys.rs:33:10\n\
        2: <unityrs_export::ngui::attempt::Attempt>::plan\n\
        END_REFUSED_ALLOCATION";
    assert_eq!(
        innermost(trace).as_deref(),
        Some("unityrs_export::ngui::keys::Template::clone")
    );
    assert_eq!(
        function_key(
            "unityrs_core::env::held::lock::<unityrs_core::env::registry::lru::Lru>::{closure#7}"
        )
        .as_deref(),
        Some("unityrs_core::env::held::lock")
    );
    assert_eq!(
        innermost("REFUSED_ALLOCATION site=1 size=64\n0: <unknown>\nEND_REFUSED_ALLOCATION"),
        None
    );
    assert_eq!(innermost("0: unityrs_core::env::Environment::new"), None);
}
