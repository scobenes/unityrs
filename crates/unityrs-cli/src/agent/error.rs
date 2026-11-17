//! Actionable diagnostics without changing the parity protocols.
use super::{Args, Document, bounded_message, fixed, options};
use std::fmt::Write as _;
use unityrs_core::{LimitError, LimitKind, limits::ChargeError};
use unityrs_dump::text::Sink as _;
#[derive(Debug)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub detail: Option<Box<Document>>,
}
impl Error {
    pub(super) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }
    pub(super) fn formatted(code: &'static str, value: &impl std::fmt::Display) -> Self {
        Self::new(code, bounded_message(value))
    }
    pub(super) fn usage(message: impl Into<String>) -> Self {
        Self::new("usage", message)
    }
    pub(super) fn input(e: impl std::fmt::Display) -> Self {
        Self::formatted("input", &e)
    }
    pub(super) fn at_input(mut self, index: usize, path: &[u8]) -> Self {
        if self.detail.is_some() {
            return self;
        }
        self.message = bounded_message(&format_args!(
            "input {index} {}: {}",
            options::spelling(path),
            self.message
        ));
        self.detail = fixed(&serde_json::json!({"input":index}))
            .ok()
            .map(Box::new);
        self
    }
}
impl From<unityrs_core::Error> for Error {
    fn from(e: unityrs_core::Error) -> Self {
        let limit = limit(Fault::Core(&e));
        let e = unityrs_export::extractor::Error::Core(e);
        Self::new(
            if e.resource_refusal() {
                "budget"
            } else {
                "input"
            },
            bounded_message(&e),
        )
        .with_limit(limit)
    }
}
impl From<unityrs_core::limits::ChargeError> for Error {
    fn from(e: unityrs_core::limits::ChargeError) -> Self {
        let limit = charge_limit(&e);
        Self::formatted("budget", &e).with_limit(limit)
    }
}
impl From<unityrs_core::walk::ParseError> for Error {
    fn from(e: unityrs_core::walk::ParseError) -> Self {
        let limit = limit(Fault::Parse(&e));
        let e = unityrs_export::extractor::Error::Simple(unityrs_export::simple::Error::Parse(e));
        Self::new(
            if e.resource_refusal() {
                "budget"
            } else {
                "input"
            },
            bounded_message(&e),
        )
        .with_limit(limit)
    }
}

impl Error {
    fn with_limit(mut self, limit: Option<LimitError>) -> Self {
        if self.code != "budget" {
            return self;
        }
        let action = if let Some(limit) = limit {
            let option = match limit.kind() {
                LimitKind::InputMemory => Some("--max-input-memory"),
                LimitKind::InputBytes => Some("--max-input-bytes"),
                LimitKind::DecodeWork => Some("--max-decode-work"),
                LimitKind::TreeDepth => Some("--max-tree-depth"),
                _ => None,
            };
            option.map_or_else(
                || {
                    format!(
                        "fixed reader bound (current: {}); no CLI option raises it",
                        limit.max()
                    )
                },
                |key| format!("raise {key} (current: {})", limit.max()),
            )
        } else {
            "memory allocation refused; reduce input or response size, or free host memory"
                .to_owned()
        };
        self.message = bounded_message(&format_args!("{action}: {}", self.message));
        self
    }
    pub(super) fn at_object(
        self,
        input: usize,
        path: &[u8],
        file_index: usize,
        path_id: i64,
    ) -> Self {
        let mut error = self.at_input(input, path);
        error.message = bounded_message(&format_args!(
            "file_index {file_index}, path_id {path_id}: {}",
            error.message
        ));
        error.detail =
            fixed(&serde_json::json!({"input":input,"file_index":file_index,"path_id":path_id}))
                .ok()
                .map(Box::new);
        error
    }
    pub(super) fn missing_tree(mut self, args: &Args<'_>) -> Self {
        let current = args
            .get("--tpk")
            .map_or_else(|| "absent".to_owned(), options::spelling);
        self.message = bounded_message(&format_args!(
            "{}; supply a matching --tpk FILE (current: {current})",
            self.message
        ));
        self
    }
    pub(crate) fn tpk(error: &unityrs_tpk::TpkError) -> Self {
        use unityrs_tpk::TpkError;
        match error {
            TpkError::Resource(e) => Self::from(e.clone()),
            TpkError::TooLarge { max, .. } => Self::new(
                "budget",
                bounded_message(&format_args!(
                    "fixed provider bound (current: {max}); no CLI option raises it; choose a smaller --tpk provider: {error}"
                )),
            ),
            TpkError::Allocator(_) => Self::formatted("budget", &error).with_limit(None),
            TpkError::Io(io) if io.kind() == std::io::ErrorKind::OutOfMemory => {
                Self::formatted("budget", &error).with_limit(None)
            }
            _ => Self::input(error),
        }
    }
    pub(super) fn at_tpk(mut self, path: &[u8]) -> Self {
        self.message = bounded_message(&format_args!(
            "shared provider --tpk {}: {}",
            options::spelling(path),
            self.message
        ));
        let meter = crate::extract::limits::extraction(&unityrs_core::limits::Limits::default());
        let mut text = unityrs_dump::text::Text::on(&meter);
        text.push_str("{\"tpk\":");
        super::listing::string(&mut text, path, true);
        text.push('}');
        self.detail = super::document(text).ok().map(Box::new);
        self
    }
    pub(super) fn response_cap(message: &str, current: usize) -> Self {
        Self::new(
            "budget",
            format!("raise --max-bytes (current: {current}): {message}"),
        )
    }
    pub(super) fn for_args(mut self, command: &str, args: &Args<'_>) -> Self {
        if self.code == "bad_cursor" {
            self.message = bounded_message(&format_args!(
                "--cursor {}: restart inspect without --cursor using the current inputs and options; {}",
                options::spelling(args.get("--cursor").unwrap_or_default()),
                self.message
            ));
        }
        if matches!(self.code, "not_found" | "ambiguous")
            || (self.code == "budget" && self.detail.is_none())
            || self.code == "bad_cursor"
        {
            let selected = args.file_index().ok().flatten().map(|(index, _)| index);
            let inputs = args
                .inputs
                .iter()
                .enumerate()
                .filter(|(index, _)| selected.is_none_or(|selected| selected == *index))
                .take(4)
                .map(|(index, path)| format!("input {index} {}", options::spelling(path)))
                .collect::<Vec<_>>()
                .join(", ");
            let mut context = format!("{command}: {inputs}");
            if selected.is_none() && args.inputs.len() > 4 {
                context.push_str(", ...");
            }
            for key in ["--path-id", "--file", "--file-index"] {
                if let Some(raw) = args.get(key) {
                    let _ = write!(context, " {key} {}", options::spelling(raw));
                }
            }
            self.message = bounded_message(&format_args!("{context}: {}", self.message));
        }
        self
    }
}
fn charge_limit(error: &ChargeError) -> Option<LimitError> {
    if let ChargeError::Limit(limit) = error {
        Some(*limit)
    } else {
        None
    }
}
enum Fault<'a> {
    Core(&'a unityrs_core::Error),
    Parse(&'a unityrs_core::walk::ParseError),
    Tree(&'a unityrs_core::typetree::TreeError),
    Deref(&'a unityrs_core::env::DerefError),
}
fn limit(mut fault: Fault<'_>) -> Option<LimitError> {
    use unityrs_core::{
        Error as Core,
        detect::DetectError,
        env::DerefError,
        typetree::TreeError,
        walk::{ParseError, ReadDetail},
    };
    loop {
        fault = match fault {
            Fault::Core(error) => match error {
                Core::Limit(limit)
                | Core::Detect(DetectError::InflateLimit(limit) | DetectError::Work(limit)) => {
                    return Some(*limit);
                }
                Core::Entry { source, .. } => Fault::Core(source),
                Core::AssetBundle(error) => Fault::Parse(error),
                Core::Tree(error) => Fault::Tree(error),
                Core::Deref(error) => Fault::Deref(error),
                _ => return None,
            },
            Fault::Parse(error) => match error {
                ParseError::Read(error) => match &error.detail {
                    ReadDetail::Limit(limit) => return Some(*limit),
                    ReadDetail::Charge(error) => return charge_limit(error),
                    ReadDetail::Tree(error) => Fault::Tree(error),
                    _ => return None,
                },
                ParseError::Tree(error) => Fault::Tree(error),
                ParseError::Deref(error) => Fault::Deref(error),
                ParseError::Provider(error) => return error.limit,
                _ => return None,
            },
            Fault::Tree(error) => {
                return match error {
                    TreeError::Limit(limit) => Some(*limit),
                    TreeError::Charge(error) => charge_limit(error),
                    _ => None,
                };
            }
            Fault::Deref(DerefError::Load(error)) => Fault::Core(error),
            Fault::Deref(_) => return None,
        };
    }
}
