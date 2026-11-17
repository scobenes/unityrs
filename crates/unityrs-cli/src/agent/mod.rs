//! Versioned agent documents. Operations return charged JSON, never print.
mod cursor;
mod describe;
mod error;
mod inventory;
mod listing;
pub(crate) mod options;
mod render;
mod schema;
pub use error::Error;
use options::Args;
use std::{io::Write, process::ExitCode, sync::Arc};
use unityrs_core::limits::{Charge, Meter};
use unityrs_dump::{
    json,
    text::{Sink, Text},
};
pub type Result<T> = std::result::Result<T, Error>;
// Formatting attacker-controlled names into an error must not make an
// unbounded intermediate String before the charged JSON writer sees it.
fn bounded_message(value: &impl std::fmt::Display) -> String {
    use std::fmt::Write as _;
    struct Message(String);
    impl std::fmt::Write for Message {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            let remaining = 4096usize.saturating_sub(self.0.len());
            let mut end = text.len().min(remaining);
            while !text.is_char_boundary(end) {
                end = end.saturating_sub(1);
            }
            self.0.push_str(text.get(..end).unwrap_or_default());
            if end < text.len() {
                Err(std::fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    let mut message = Message(String::new());
    if message.0.try_reserve_exact(4096).is_err() {
        return String::new();
    }
    let _ = write!(message, "{value}");
    message.0
}
#[derive(Debug)]
pub struct Document {
    pub json: String,
    charge: Charge,
}
impl Document {
    pub(crate) fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }
}
fn document(text: Text) -> Result<Document> {
    let (json, charge) = text.finish()?;
    Ok(Document { json, charge })
}
fn name(out: &mut impl Sink, bytes: &[u8]) {
    // UTF-8 validation plus scanning/escaping (at most six output bytes per
    // input byte), charged before either pass or hex encoding starts.
    if out.count_work(bytes.len().saturating_mul(7)) {
        json::write_name(out, bytes);
    }
}
fn write_error(out: &mut impl Sink, e: &Error) {
    out.push_str("{\"format\":\"unityrs.error\",\"version\":1,\"code\":");
    json::write_str(out, e.code);
    out.push_str(",\"message\":");
    json::write_str(out, &e.message);
    if let Some(detail) = &e.detail {
        out.push_str(",\"detail\":");
        out.push_str(detail.json.trim_end());
    }
    out.push('}');
}
pub(crate) fn error_document(e: &Error) -> Result<Document> {
    let meter = crate::extract::limits::extraction(&unityrs_core::limits::Limits::default());
    let mut out = Text::on(&meter);
    write_error(&mut out, e);
    out.push('\n');
    document(out)
}
/// Executes a new command and returns exactly one compact JSON document.
pub fn execute(command: &str, raw: &[Vec<u8>]) -> Result<Document> {
    execute_confined(command, raw, None)
}
pub(crate) fn execute_confined(
    command: &str,
    raw: &[Vec<u8>],
    roots: Option<Arc<crate::mcp::roots::Roots>>,
) -> Result<Document> {
    let mut args = Args::parse(command, raw)?;
    args.roots = roots;
    if args.get("--help").is_some() {
        return describe::run();
    }
    let result = match command {
        "inspect" => inventory::inspect(&args),
        "show" => inventory::show(&args),
        "describe" => match args.get("--schema") {
            Some(b) => schema::run(std::str::from_utf8(b).map_err(Error::input)?),
            None => describe::run(),
        },
        _ => Err(Error::usage("unknown command")),
    };
    result.map_err(|e| e.for_args(command, &args))
}
fn fixed(value: &serde_json::Value) -> Result<Document> {
    let meter = crate::extract::limits::extraction(&unityrs_core::limits::Limits::default());
    let mut out = Text::on(&meter);
    // These tables are trusted, fixed-size program data.
    out.push_str(&value.to_string());
    out.push('\n');
    document(out)
}
pub fn main(command: &str, raw: &[Vec<u8>]) -> ExitCode {
    let (json, status, _held) = match execute(command, raw) {
        Ok(doc) => (doc.json, 0, Some(doc.charge)),
        Err(e) => {
            // Keep stderr to one physical line even when an input names a newline.
            eprintln!(
                "unityrs {command}: {}: {}",
                e.code,
                e.message.escape_default()
            );
            let status = if e.code == "usage" { 2 } else { 1 };
            match error_document(&e) {
                Ok(doc) => (doc.json, status, Some(doc.charge)),
                Err(_) => (
                    String::from(
                        "{\"format\":\"unityrs.error\",\"version\":1,\"code\":\"budget\",\"message\":\"error response budget refused\"}\n",
                    ),
                    1,
                    None,
                ),
            }
        }
    };
    if std::io::stdout().lock().write_all(json.as_bytes()).is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(status)
}
fn meter(args: &Args<'_>) -> Arc<Meter> {
    crate::extract::limits::extraction(&args.limits)
}
