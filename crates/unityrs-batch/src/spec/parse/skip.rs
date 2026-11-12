//! Values read for `syntax` alone.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};

/// A value read for `syntax` alone and dropped. Unlike serde's
/// `IgnoredAny`, which `serde_json` skips without counting its depth, it
/// goes through `deserialize_any`, so the parser's depth limit holds
/// for it too.
pub(super) struct Skip;

impl<'de> Deserialize<'de> for Skip {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(SkipVisitor)
    }
}

struct SkipVisitor;

impl<'de> Visitor<'de> for SkipVisitor {
    type Value = Skip;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Skip, A::Error> {
        drain_seq(seq).map(|()| Skip)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Skip, A::Error> {
        drain_map(map).map(|()| Skip)
    }
}

/// Reads and drops what a visitor is handed but does not want.
pub(super) fn drain_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
    while seq.next_element::<Skip>()?.is_some() {}
    Ok(())
}

pub(super) fn drain_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<(), A::Error> {
    while map.next_entry::<Skip, Skip>()?.is_some() {}
    Ok(())
}
