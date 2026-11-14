//! Which version's table the parse reads the document by: the value of the
//! top level's first `spec_version` member, read before the parse.
//!
//! The parse checks each value as it reads it, in document order, and a
//! member that only version 2 has (`unitycn_key`, `outputs.formats`) must
//! be `unknown_field` in a version 1 document at its place in that order,
//! wherever `spec_version` stands. So the version is found first, by a
//! scan that reads the top level's member names and skips their values,
//! and stops at the first `spec_version`. Whatever that member holds, and
//! whatever else the document breaks, the parse's own verdicts decide:
//! `spec_version`'s (a repeat, a missing one, a value not 1 or 2) are
//! reported before any other value's, so the table chosen matters only for
//! a document whose version is 1 or 2.

use std::cell::Cell;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, Error as _, MapAccess, Visitor};

use super::super::node::Name;
use super::skip::Skip;

/// The version of `text`'s first top-level `spec_version` member, when it
/// is an integer token; 1 for any other document, which the parse refuses
/// or reads as version 1.
pub(super) fn version(text: &str) -> u64 {
    let found = Cell::new(None);
    let mut de = serde_json::Deserializer::from_str(text);
    // A stop at the member is reported as an error: the rest is never read.
    let _ = de::Deserializer::deserialize_any(&mut de, TopScan(&found));
    found.get().unwrap_or(1)
}

/// The top level, read for its first `spec_version`.
struct TopScan<'c>(&'c Cell<Option<u64>>);

impl<'de> Visitor<'de> for TopScan<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(Name(name)) = map.next_key()? {
            if name == "spec_version" {
                self.0.set(map.next_value_seed(Integer)?);
                return Err(A::Error::custom("found"));
            }
            map.next_value::<Skip>()?;
        }
        Ok(())
    }
}

/// A value: its integer, when it is an unsigned integer token.
struct Integer;

impl<'de> DeserializeSeed<'de> for Integer {
    type Value = Option<u64>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Integer {
    type Value = Option<u64>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a spec version")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(Some(v))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, _: A) -> Result<Self::Value, A::Error> {
        Ok(None)
    }

    fn visit_map<A: MapAccess<'de>>(self, _: A) -> Result<Self::Value, A::Error> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn the_first_top_level_spec_version_wherever_it_stands() {
        assert_eq!(version(r#"{"spec_version":2}"#), 2);
        assert_eq!(
            version(r#"{"a":{"spec_version":2},"b":[1,{}],"spec_version":2}"#),
            2
        );
        assert_eq!(version(r#"{"spec_version":2,"spec_version":1}"#), 2);
        // The rest is never read: the trailing garbage is the parse's.
        assert_eq!(version(r#"{"spec_version":2,}}}"#), 2);
        for other in [
            "",
            "[]",
            "{}",
            r#"{"a":{"spec_version":2}}"#,
            r#"{"spec_version":2.0}"#,
            r#"{"spec_version":"2"}"#,
            r#"{"spec_version":-2}"#,
            r#"{"spec_version":[2]}"#,
            r#"{"x":}"#,
        ] {
            assert_eq!(version(other), 1, "{other}");
        }
        assert_eq!(version(r#"{"spec_version":7}"#), 7);
    }
}
