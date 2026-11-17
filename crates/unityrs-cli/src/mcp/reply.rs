//! Serialize tool documents directly twice, without reparsing them into a
//! second JSON tree or allocating a quoted copy. The original meter pays for
//! the framing buffer and escaping work before each write.
use super::{config::Config, tools};
use serde_json::Value;
use std::{
    io::{self, Write},
    sync::Arc,
};
use unityrs_core::limits::Meter;
use unityrs_mesh::obj::{ChargedBytes, MeteredWrite};
pub enum Document {
    Agent(crate::agent::Document),
    Extract(ChargedBytes),
    Fixed(String),
}
pub struct Tool {
    pub document: Document,
    pub failed: bool,
}
impl Tool {
    fn text(&self) -> io::Result<&str> {
        let text = match &self.document {
            Document::Agent(d) => d.json.as_str(),
            Document::Extract(d) => {
                d.meter()
                    .count_decode(d.as_bytes().len() as u64)
                    .map_err(io::Error::other)?;
                std::str::from_utf8(d.as_bytes()).map_err(io::Error::other)?
            }
            Document::Fixed(d) => d.as_str(),
        };
        Ok(text.trim_end())
    }
    fn meter(&self, config: &Config) -> Arc<Meter> {
        match &self.document {
            Document::Agent(d) => d.meter().clone(),
            Document::Extract(d) => d.meter().clone(),
            Document::Fixed(_) => crate::extract::limits::extraction(&config.limits()),
        }
    }
}
pub enum Response {
    Json(Value),
    Tool { id: Value, result: Tool },
}
impl Response {
    fn id(&self) -> Value {
        match self {
            Self::Json(v) => v.get("id").cloned().unwrap_or(Value::Null),
            Self::Tool { id, .. } => id.clone(),
        }
    }
    fn encode(&self, out: &mut impl Write) -> io::Result<()> {
        match self {
            Self::Json(value) => serde_json::to_writer(out, value).map_err(io::Error::other),
            Self::Tool { id, result } => {
                out.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":")?;
                serde_json::to_writer(&mut *out, id)?;
                out.write_all(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":")?;
                serde_json::to_writer(&mut *out, result.text()?)?;
                out.write_all(b"}],\"structuredContent\":")?;
                out.write_all(result.text()?.as_bytes())?;
                out.write_all(if result.failed {
                    b",\"isError\":true}}"
                } else {
                    b",\"isError\":false}}"
                })
            }
        }
    }
    pub fn write(&self, config: &Config, output: &mut impl Write) -> io::Result<()> {
        let meter = match self {
            Self::Tool { result, .. } => result.meter(config),
            Self::Json(_) => crate::extract::limits::extraction(&config.limits()),
        };
        let mut encoded = Limited {
            bytes: ChargedBytes::new(&meter),
            cap: config.response_bytes.saturating_sub(1),
        };
        if self.encode(&mut encoded).is_err() {
            // Discard the refused staging buffer before constructing a small
            // error on the independent error budget, as A1a's CLI does.
            drop(encoded);
            let meter =
                crate::extract::limits::extraction(&unityrs_core::limits::Limits::default());
            encoded = Limited {
                bytes: ChargedBytes::new(&meter),
                cap: config.response_bytes.saturating_sub(1),
            };
            let refusal = if matches!(self, Self::Tool { .. }) {
                Self::Tool {
                    id: self.id(),
                    result: tools::failure(
                        "budget",
                        "Response exceeds server response memory, work or byte ceiling",
                    ),
                }
            } else {
                super::error(
                    self.id(),
                    -32603,
                    "Response exceeds server response memory, work or byte ceiling",
                )
            };
            refusal.encode(&mut encoded)?;
        }
        output.write_all(encoded.bytes.as_bytes())?;
        output.write_all(b"\n")?;
        output.flush()
    }
}
struct Limited {
    bytes: ChargedBytes,
    cap: usize,
}
impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.as_bytes().len().saturating_add(bytes.len()) > self.cap {
            return Err(io::Error::other("response cap"));
        }
        self.bytes.write_bytes(bytes).map_err(io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
