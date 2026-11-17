//! Bounded natural JSON with explicit, collision-safe exceptional values.
//! The optional canonical view preserves the original tagged representation.
use super::{Args, Document, Error, Result, document};
use std::fmt::Write as _;
use std::sync::Arc;
use unityrs_core::{Parsed, PrimArray, Value, limits::Meter};
use unityrs_dump::{
    encode::encode_value_into,
    json,
    text::{Sink, Text},
};

/// Measures a header without retaining its bytes. Work is charged before each
/// emitted fragment, and a refusal is returned before response allocation.
pub struct Length<'a> {
    len: usize,
    meter: &'a Arc<Meter>,
    refused: Option<unityrs_core::limits::ChargeError>,
}
impl<'a> Length<'a> {
    pub fn on(meter: &'a Arc<Meter>) -> Self {
        Self {
            len: 0,
            meter,
            refused: None,
        }
    }
    pub fn finish(self) -> Result<usize> {
        self.refused.map_or(Ok(self.len), |e| Err(e.into()))
    }
}
impl std::fmt::Write for Length<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        Sink::push_str(self, s);
        if self.refused.is_some() {
            Err(std::fmt::Error)
        } else {
            Ok(())
        }
    }
}
impl Sink for Length<'_> {
    fn push_str(&mut self, s: &str) {
        if self.count_work(s.len()) {
            self.len = self.len.saturating_add(s.len());
        }
    }
    fn push(&mut self, c: char) {
        self.push_str(c.encode_utf8(&mut [0; 4]));
    }
    fn count_work(&mut self, n: usize) -> bool {
        if self.refused.is_some() {
            return false;
        }
        match self.meter.count_decode(n as u64) {
            Ok(()) => true,
            Err(e) => {
                self.refused = Some(e);
                false
            }
        }
    }
    fn meter(&self) -> Option<&Arc<Meter>> {
        Some(self.meter)
    }
    fn stop(
        &mut self,
        e: unityrs_core::limits::ChargeError,
    ) -> Option<unityrs_core::limits::ChargeError> {
        self.refused.get_or_insert(e);
        None
    }
}

#[derive(Clone, Copy)]
struct Context<'a> {
    list: &'a unityrs_core::typetree::NodeList,
    node: unityrs_core::typetree::NodeId,
}
impl Context<'_> {
    fn is_map(self) -> bool {
        self.list
            .row(self.node)
            .is_some_and(|row| row.ty.as_bytes() == b"map")
    }
    fn child(self, index: usize) -> Option<Self> {
        self.list.children(self.node).get(index).map(|&node| Self {
            list: self.list,
            node,
        })
    }
    fn element(self) -> Option<Self> {
        let array = if self.list.row(self.node)?.ty.as_bytes() == b"Array" {
            self
        } else {
            let first = self.child(0)?;
            if first.list.row(first.node)?.ty.as_bytes() != b"Array" {
                return None;
            }
            first
        };
        array.child(1)
    }
}

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
    canonical: bool,
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
fn pair(value: &Value) -> Result<(&Value, &Value)> {
    if let Value::Tuple(items) = value
        && let [key, value] = items.as_ref()
    {
        return Ok((key, value));
    }
    Err(Error::new("input", "map element is not a key/value pair"))
}
impl View<'_> {
    fn render(&mut self, value: &Value, depth: usize, cap: usize) -> Result<Document> {
        let context = self.parsed.lists().first().map(|list| Context {
            list,
            node: list.root(),
        });
        self.render_at(value, depth, cap, context)
    }
    fn render_at(
        &mut self,
        value: &Value,
        depth: usize,
        cap: usize,
        context: Option<Context<'_>>,
    ) -> Result<Document> {
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
                Value::Dict(map) => self.dictionary(&mut out, map, depth)?,
                Value::List(items) if !self.canonical && context.is_some_and(Context::is_map) => {
                    self.logical_map(&mut out, items, depth, context)?;
                }
                Value::List(items) => self.sequence(&mut out, items, depth, false, context)?,
                Value::Tuple(items) => self.sequence(&mut out, items, depth, true, context)?,
                Value::Prims(run) => self.prims(&mut out, run, depth)?,
                Value::Str(s) if s.as_bytes().len().saturating_mul(6) > cap => {
                    out.overflow = true;
                }
                Value::Str(s) => json::write_name(&mut out, s.as_bytes()),
                Value::Int(i) if !self.canonical && i.unsigned_abs() > (1_u64 << 53) => {
                    let _ = write!(out, "{{\"t\":\"i64\",\"v\":\"{i}\"}}");
                }
                Value::UInt(i) if !self.canonical && *i > (1_u64 << 53) => {
                    let _ = write!(out, "{{\"t\":\"u64\",\"v\":\"{i}\"}}");
                }
                Value::F32(bits) if !self.canonical && f32::from_bits(*bits).is_finite() => {
                    let _ = write!(out, "{:?}", f32::from_bits(*bits));
                }
                Value::F64(bits) if !self.canonical && f64::from_bits(*bits).is_finite() => {
                    let _ = write!(out, "{:?}", f64::from_bits(*bits));
                }
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
    fn dictionary(
        &mut self,
        out: &mut Limited,
        map: &unityrs_core::value::Map,
        depth: usize,
    ) -> Result<()> {
        let parsed = self.parsed;
        let natural = !self.canonical && self.natural_map(map)?;
        out.push_str(if natural {
            "{"
        } else {
            "{\"t\":\"dict\",\"v\":["
        });
        for (i, (key, v)) in map.entries().iter().take(self.max_items).enumerate() {
            if i > 0 {
                out.push(',');
            }
            if !natural {
                out.push('[');
            }
            json::write_name(out, self.parsed.key_name(map, *key));
            out.push(if natural { ':' } else { ',' });
            let context = parsed
                .lists()
                .get(map.list() as usize)
                .map(|list| Context { list, node: *key });
            self.child(out, v, depth, context)?;
            if !natural {
                out.push(']');
            }
            if out.overflow {
                break;
            }
        }
        if !natural {
            out.push(']');
        }
        if map.len() > self.max_items {
            self.cuts.items += 1;
            out.push_str(",\"cut\":");
            marker(out, "items", map.len() - self.max_items);
        }
        out.push('}');
        Ok(())
    }
    fn logical_map(
        &mut self,
        out: &mut Limited,
        items: &[Value],
        depth: usize,
        context: Option<Context<'_>>,
    ) -> Result<()> {
        let natural = self.natural_pairs(items)?;
        let pair_context = context.and_then(Context::element);
        out.push_str(if natural {
            "{"
        } else {
            "{\"t\":\"map\",\"v\":["
        });
        for (i, item) in items.iter().take(self.max_items).enumerate() {
            let (key, value) = pair(item)?;
            if i > 0 {
                out.push(',');
            }
            if natural {
                if let Value::Str(key) = key {
                    json::write_name(out, key.as_bytes());
                }
                out.push(':');
            } else {
                out.push('[');
                self.child(out, key, depth, pair_context.and_then(|c| c.child(0)))?;
                out.push(',');
            }
            self.child(out, value, depth, pair_context.and_then(|c| c.child(1)))?;
            if !natural {
                out.push(']');
            }
            if out.overflow {
                break;
            }
        }
        if !natural {
            out.push(']');
        }
        if items.len() > self.max_items {
            self.cuts.items += 1;
            out.push_str(",\"cut\":");
            marker(out, "items", items.len() - self.max_items);
        }
        out.push('}');
        Ok(())
    }
    fn natural_pairs(&self, items: &[Value]) -> Result<bool> {
        if items.len() > self.max_items {
            return Ok(false);
        }
        for (i, item) in items.iter().enumerate() {
            let (Value::Str(key), _) = pair(item)? else {
                return Ok(false);
            };
            let key = key.as_bytes();
            self.meter.count_decode(key.len() as u64)?;
            if std::str::from_utf8(key).is_err() || key == b"t" || key == b"$unityrs" {
                return Ok(false);
            }
            for prior in items.iter().take(i) {
                let (Value::Str(prior), _) = pair(prior)? else {
                    return Ok(false);
                };
                self.meter
                    .count_decode(key.len().min(prior.as_bytes().len()) as u64 + 1)?;
                if key == prior.as_bytes() {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    // A clipped map uses the pair wrapper so its item marker is outside data.
    // Compare borrowed names without an uncharged index allocation. Each
    // comparison is charged, so adversarial duplicate detection is bounded.
    fn natural_map(&self, map: &unityrs_core::value::Map) -> Result<bool> {
        if map.len() > self.max_items {
            return Ok(false);
        }
        for (i, (key, _)) in map.entries().iter().enumerate() {
            let name = self.parsed.key_name(map, *key);
            self.meter.count_decode(name.len() as u64)?;
            if std::str::from_utf8(name).is_err() || name == b"t" || name == b"$unityrs" {
                return Ok(false);
            }
            for (prior, _) in map.entries().iter().take(i) {
                let prior = self.parsed.key_name(map, *prior);
                self.meter
                    .count_decode(name.len().min(prior.len()) as u64 + 1)?;
                if name == prior {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    fn child(
        &mut self,
        out: &mut Limited,
        value: &Value,
        depth: usize,
        context: Option<Context<'_>>,
    ) -> Result<()> {
        let remaining = out
            .cap
            .saturating_sub(out.text.as_str().len())
            .saturating_sub(128);
        let child = self.render_at(value, depth + 1, remaining, context)?;
        out.push_str(&child.json);
        Ok(())
    }
    fn sequence(
        &mut self,
        out: &mut Limited,
        items: &[Value],
        depth: usize,
        tuple: bool,
        context: Option<Context<'_>>,
    ) -> Result<()> {
        let tagged = tuple && self.canonical;
        if tagged {
            out.push_str("{\"t\":\"tuple\",\"v\":");
        }
        out.push('[');
        for (i, v) in items.iter().take(self.max_items).enumerate() {
            if i > 0 {
                out.push(',');
            }
            let child = context.and_then(|context| {
                if tuple {
                    context.child(i)
                } else {
                    context.element()
                }
            });
            self.child(out, v, depth, child)?;
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
        if tagged {
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
                    self.child(out, &Value::$variant((*v).into()), depth, None)?;
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
    header_len: usize,
    write_header: impl FnOnce(&mut Text) -> Result<()>,
    meter: &Arc<Meter>,
) -> Result<Document> {
    const PREFIX: &str = "{\"format\":\"unityrs.show\",\"version\":1,\"object\":";
    const VALUE: &str = ",\"value\":";
    const MIN_TAIL: &str = "{\"$unityrs\":\"cut\",\"reason\":\"bytes\",\"omitted\":1},\"truncated\":{\"cuts\":1,\"bytes\":1,\"items\":0,\"depth\":0,\"binary\":0}}\n";
    let max = args.number("show", "--max-bytes")?;
    // Inspect borrowed lengths before making any response-sized allocation.
    let prefix_len = PREFIX
        .len()
        .saturating_add(header_len)
        .saturating_add(VALUE.len());
    if prefix_len.saturating_add(MIN_TAIL.len()) > max {
        return Err(Error::new(
            "budget",
            "--max-bytes cannot hold the object header and a truncation marker",
        ));
    }
    let mut view = View {
        parsed,
        meter,
        max_items: args.number("show", "--max-items")?,
        max_depth: args.number("show", "--max-depth")?,
        full: args.value("show", "--bytes") == b"full",
        canonical: args.value("show", "--value") == b"canonical",
        cuts: Cuts::default(),
    };
    let value = view.render(
        parsed.value(),
        0,
        max.saturating_sub(prefix_len).saturating_sub(160),
    )?;
    let mut out = Text::on(meter);
    out.push_str(PREFIX);
    write_header(&mut out)?;
    out.push_str(VALUE);
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

#[cfg(test)]
mod tests {
    use super::*;
    use unityrs_core::{
        ValueBuilder,
        typetree::{NodeList, NodeRow},
    };
    #[test]
    fn oversized_header_refuses_before_copying_or_rendering() {
        let meter = Meter::unlimited();
        let nodes = NodeList::trusted(vec![NodeRow::new(0, "int", "Base")])
            .unwrap()
            .into_shared()
            .unwrap();
        let builder = ValueBuilder::new(&meter, nodes).unwrap();
        let scalar = builder.scalar(Value::Int(1)).unwrap();
        let parsed = builder.finish(scalar, 0);
        let raw = [
            b"input".to_vec(),
            b"--path-id".to_vec(),
            b"1".to_vec(),
            b"--max-bytes".to_vec(),
            b"512".to_vec(),
        ];
        let args = Args::parse("show", &raw).unwrap();
        let header = format!("{{\"name\":\"{}\"}}", "x".repeat(4 << 20));
        let before = meter.decoded();
        crate::test_alloc::LARGEST.set(Some(0));
        let result = show(
            &args,
            &parsed,
            header.len(),
            |out| {
                out.push_str(&header);
                Ok(())
            },
            &meter,
        );
        let largest = crate::test_alloc::LARGEST.replace(None).unwrap();
        assert_eq!(result.unwrap_err().code, "budget");
        assert!(
            largest < 16_384,
            "small cap copied a huge header: {largest} bytes"
        );
        assert_eq!(meter.decoded(), before, "refuse before rendering the value");
    }
    #[test]
    fn duplicate_and_raw_field_names_preserve_pairs() {
        use unityrs_core::typetree::NodeId;
        let meter = Meter::unlimited();
        for (second_name, expected) in [
            (
                unityrs_core::RawStr::from("key"),
                serde_json::json!({"t":"dict","v":[["key",1],["key",2]]}),
            ),
            (
                unityrs_core::RawStr::from(&b"\xff"[..]),
                serde_json::json!({"t":"dict","v":[["key",1],[{"t":"str","hex":"ff"},2]]}),
            ),
        ] {
            let nodes = NodeList::trusted(vec![
                NodeRow::new(0, "Base", "Base"),
                NodeRow::new(1, "int", "key"),
                NodeRow::new(1, "int", second_name),
            ])
            .unwrap()
            .into_shared()
            .unwrap();
            let builder = ValueBuilder::new(&meter, nodes).unwrap();
            let mut entries = builder.vec();
            entries
                .push_entry(NodeId(1), builder.scalar(Value::Int(1)).unwrap())
                .unwrap();
            entries
                .push_entry(NodeId(2), builder.scalar(Value::Int(2)).unwrap())
                .unwrap();
            let value = builder.map(0, entries);
            let parsed = builder.finish(value, 0);
            let mut view = View {
                parsed: &parsed,
                meter: &meter,
                max_items: 100,
                max_depth: 32,
                full: false,
                canonical: false,
                cuts: Cuts::default(),
            };
            let rendered = view.render(parsed.value(), 0, 10000).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&rendered.json).unwrap(),
                expected
            );
        }
    }
}
