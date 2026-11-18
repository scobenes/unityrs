//! Metered Unity names and metadata formatting.
use super::{Arc, Extras, Meter, MeteredVec, Result, Text, invalid};
use crate::GltfError;
use std::fmt::{self, Write as _};

pub(crate) fn extra(extras: &mut Extras, meter: &Arc<Meter>, key: &str, value: Text) -> Result<()> {
    meter.count_decode(extras.len() as u64 * key.len() as u64)?;
    if extras
        .as_slice()
        .iter()
        .any(|(k, _)| k.as_bytes() == key.as_bytes())
    {
        return Err(invalid("duplicate collect extra"));
    }
    extras.push((crate::text(meter, key)?, value))?;
    extras
        .as_mut_slice()
        .sort_unstable_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
    Ok(())
}
pub(crate) fn raw_text(meter: &Arc<Meter>, mut bytes: &[u8]) -> Result<Text> {
    meter.count_decode(bytes.len() as u64 * 3)?;
    let mut out = MeteredVec::new(meter);
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(_) => {
                out.extend_from_slice(bytes)?;
                break;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.extend_from_slice(bytes.get(..valid).ok_or_else(|| invalid("text range"))?)?;
                out.extend_from_slice("�".as_bytes())?;
                bytes = bytes
                    .get(valid + e.error_len().unwrap_or(bytes.len() - valid)..)
                    .ok_or_else(|| invalid("text range"))?;
            }
        }
    }
    Ok(Text(out))
}
pub(crate) fn number(meter: &Arc<Meter>, n: impl fmt::Display) -> Result<Text> {
    let mut t = Text(MeteredVec::new(meter));
    append_fmt(&mut t, meter, format_args!("{n}"))?;
    Ok(t)
}
pub(crate) fn append_fmt(t: &mut Text, meter: &Arc<Meter>, args: fmt::Arguments<'_>) -> Result<()> {
    struct Sink<'a> {
        t: &'a mut Text,
        meter: &'a Meter,
        error: Option<GltfError>,
    }
    impl fmt::Write for Sink<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let result = (|| {
                self.meter.count_decode(s.len() as u64)?;
                self.t.0.extend_from_slice(s.as_bytes())?;
                Ok(())
            })();
            if let Err(e) = result {
                self.error = Some(e);
                return Err(fmt::Error);
            }
            Ok(())
        }
    }
    let mut sink = Sink {
        t,
        meter,
        error: None,
    };
    let result = sink.write_fmt(args);
    if result.is_err() {
        return Err(sink.error.unwrap_or_else(|| invalid("metadata formatting")));
    }
    Ok(())
}
