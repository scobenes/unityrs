//! Bounded view of the canonical value model. Data maps stay tagged dicts,
//! so the reserved, untagged $unityrs marker objects cannot be user data.
use super::{Args, Document, Error, Result, document};
use std::fmt::Write as _;
use std::sync::Arc;
use unityrs_core::{Parsed, PrimArray, Value, limits::Meter};
use unityrs_dump::{
    encode::encode_value_into,
    json,
    text::{Sink, Text},
};

#[derive(Clone, Copy, Default)]
struct Cuts {
    bytes: usize,
    items: usize,
    depth: usize,
    binary: usize,
}
impl Cuts {
    fn json(self) -> String {
        format!(
            "{{\"cuts\":{},\"bytes\":{},\"items\":{},\"depth\":{},\"binary\":{}}}",
            self.bytes + self.items + self.depth + self.binary,
            self.bytes,
            self.items,
            self.depth,
            self.binary
        )
    }
}
struct View<'a> {
    parsed: &'a Parsed,
    meter: &'a Arc<Meter>,
    max_items: usize,
    max_depth: usize,
    full: bool,
    cuts: Cuts,
}
struct Limited {
    text: Text,
    cap: usize,
    overflow: bool,
}
impl std::fmt::Write for Limited {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        Sink::push_str(self, s);
        Ok(())
    }
}
impl Sink for Limited {
    fn push_str(&mut self, s: &str) {
        if s.len() > self.cap.saturating_sub(self.text.as_str().len()) {
            self.overflow = true;
        } else if !self.overflow {
            self.text.push_str(s);
        }
    }
    fn push(&mut self, c: char) {
        self.push_str(c.encode_utf8(&mut [0; 4]));
    }
    fn count_work(&mut self, n: usize) -> bool {
        self.text.count_work(n)
    }
    fn meter(&self) -> Option<&Arc<Meter>> {
        self.text.meter()
    }
    fn stop(
        &mut self,
        e: unityrs_core::limits::ChargeError,
    ) -> Option<unityrs_core::limits::ChargeError> {
        self.text.stop(e)
    }
}
fn marker(out: &mut impl Sink, reason: &str, omitted: usize) {
    let _ = write!(
        out,
        "{{\"$unityrs\":\"cut\",\"reason\":\"{reason}\",\"omitted\":{omitted}}}"
    );
}
impl View<'_> {
    fn render(&mut self, value: &Value, depth: usize, cap: usize) -> Result<Document> {
        self.meter.count_decode(8)?;
        let before = self.cuts;
        let mut out = Limited {
            text: Text::on(self.meter),
            cap,
            overflow: false,
        };
        let composite = matches!(
            value,
            Value::Dict(_) | Value::Tuple(_) | Value::List(_) | Value::Prims(_)
        );
        if composite && depth >= self.max_depth {
            self.cuts.depth += 1;
            marker(&mut out, "depth", 1);
        } else {
            match value {
                Value::Bytes(b) | Value::Prims(PrimArray::U8(b)) => self.binary(
                    &mut out,
                    if matches!(value, Value::Bytes(_)) {
                        "bytes"
                    } else {
                        "u8s"
                    },
                    b,
                )?,
                Value::Dict(map) => {
                    out.push_str("{\"t\":\"dict\",\"v\":[");
                    for (i, (key, v)) in map.entries().iter().take(self.max_items).enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push('[');
                        json::write_name(&mut out, self.parsed.key_name(map, *key));
                        out.push(',');
                        self.child(&mut out, v, depth)?;
                        out.push(']');
                        if out.overflow {
                            break;
                        }
                    }
                    out.push(']');
                    if map.len() > self.max_items {
                        self.cuts.items += 1;
                        out.push_str(",\"cut\":");
                        marker(&mut out, "items", map.len() - self.max_items);
                    }
                    out.push('}');
                }
                Value::List(items) => self.sequence(&mut out, items, depth, false)?,
                Value::Tuple(items) => self.sequence(&mut out, items, depth, true)?,
                Value::Prims(run) => self.prims(&mut out, run, depth)?,
                Value::Str(s) if s.as_bytes().len().saturating_mul(6) > cap => {
                    out.overflow = true;
                }
                Value::Str(s) => json::write_name(&mut out, s.as_bytes()),
                _ => encode_value_into(&mut out, self.parsed, value).map_err(Error::input)?,
            }
        }
        if out.overflow {
            self.cuts = before;
            self.cuts.bytes += 1;
            let mut cut = Text::on(self.meter);
            marker(&mut cut, "bytes", 1);
            return document(cut);
        }
        document(out.text)
    }
    fn child(&mut self, out: &mut Limited, value: &Value, depth: usize) -> Result<()> {
        let remaining = out
            .cap
            .saturating_sub(out.text.as_str().len())
            .saturating_sub(128);
        let child = self.render(value, depth + 1, remaining)?;
        out.push_str(&child.json);
        Ok(())
    }
    fn sequence(
        &mut self,
        out: &mut Limited,
        items: &[Value],
        depth: usize,
        tuple: bool,
    ) -> Result<()> {
        if tuple {
            out.push_str("{\"t\":\"tuple\",\"v\":");
        }
        out.push('[');
        for (i, v) in items.iter().take(self.max_items).enumerate() {
            if i > 0 {
                out.push(',');
            }
            self.child(out, v, depth)?;
            if out.overflow {
                break;
            }
        }
        if items.len() > self.max_items {
            self.cuts.items += 1;
            if self.max_items > 0 {
                out.push(',');
            }
            marker(out, "items", items.len() - self.max_items);
        }
        out.push(']');
        if tuple {
            out.push('}');
        }
        Ok(())
    }
    fn binary(&mut self, out: &mut Limited, kind: &str, bytes: &[u8]) -> Result<()> {
        let available = out.cap.saturating_sub(out.text.as_str().len());
        if self.full && bytes.len().saturating_mul(2).saturating_add(32) <= available {
            let _ = write!(out, "{{\"t\":\"{kind}\",\"hex\":\"");
            json::push_hex(out, bytes);
            out.push_str("\"}");
        } else {
            self.meter.count_decode(bytes.len() as u64)?;
            self.cuts.binary += 1;
            let _ = write!(
                out,
                "{{\"$unityrs\":\"bytes\",\"kind\":\"{kind}\",\"length\":{},\"sha256\":\"{}\",\"prefix_hex\":\"",
                bytes.len(),
                json::sha256_hex(bytes)
            );
            json::push_hex(out, bytes.get(..bytes.len().min(64)).unwrap_or_default());
            out.push_str("\"}");
        }
        Ok(())
    }
    fn prims(&mut self, out: &mut Limited, run: &PrimArray, depth: usize) -> Result<()> {
        out.push('[');
        // Primitive runs use the same scalar encoder as every other value.
        macro_rules! emit {
            ($items:expr, $variant:ident) => {
                for (i, v) in $items.iter().take(self.max_items).enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    self.child(out, &Value::$variant((*v).into()), depth)?;
                    if out.overflow {
                        break;
                    }
                }
            };
        }
        match run {
            PrimArray::Bool(v) => emit!(v, Bool),
            PrimArray::I8(v) => emit!(v, Int),
            PrimArray::U16(v) => emit!(v, UInt),
            PrimArray::I16(v) => emit!(v, Int),
            PrimArray::U32(v) => emit!(v, UInt),
            PrimArray::I32(v) => emit!(v, Int),
            PrimArray::U64(v) => emit!(v, UInt),
            PrimArray::I64(v) => emit!(v, Int),
            PrimArray::F32(v) => emit!(v, F32),
            PrimArray::F64(v) => emit!(v, F64),
            PrimArray::U8(_) => {}
        }
        if run.len() > self.max_items {
            self.cuts.items += 1;
            if self.max_items > 0 {
                out.push(',');
            }
            marker(out, "items", run.len() - self.max_items);
        }
        out.push(']');
        Ok(())
    }
}
pub fn show(
    args: &Args<'_>,
    parsed: &Parsed,
    header: &str,
    meter: &Arc<Meter>,
) -> Result<Document> {
    let max = args.number("show", "--max-bytes")?;
    let prefix =
        format!("{{\"format\":\"unityrs.show\",\"version\":1,\"object\":{header},\"value\":");
    let mut view = View {
        parsed,
        meter,
        max_items: args.number("show", "--max-items")?,
        max_depth: args.number("show", "--max-depth")?,
        full: args.value("show", "--bytes") == b"full",
        cuts: Cuts::default(),
    };
    let value = view.render(
        parsed.value(),
        0,
        max.saturating_sub(prefix.len()).saturating_sub(160),
    )?;
    let mut out = Text::on(meter);
    out.push_str(&prefix);
    out.push_str(&value.json);
    let _ = writeln!(out, ",\"truncated\":{}}}", view.cuts.json());
    if out.as_str().len() > max {
        return Err(Error::new(
            "budget",
            "--max-bytes cannot hold the object header and a truncation marker",
        ));
    }
    document(out)
}
