//! The objects whose members the parse reads by rules of their own:
//! `headers`, `prefer` and the sink, and `spec_version`'s value.

use std::borrow::Cow;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

use super::super::fields::{
    DIRECTORY, ENV, MAX_ENDPOINT, MAX_HEADER_BYTES, MAX_HEADER_VALUE, MAX_HEADERS, MAX_HOST,
    OBJECT_STORE, PREFER, REFUSED_HEADERS, Str, Ty,
};
use super::super::formats;
use super::super::model::{Header, PreferEntry};
use super::super::node::{Held, Member, Name, Node};
use super::super::store::{EMPTY, Seg, sort_by_bytes};
use super::super::validate;
use super::super::{Rule, SPEC_VERSION};
use super::{Cx, Skip, Value, drain_map, drain_seq, object};

/// A bundle's `headers` (1.2). Its own rules, at most 32 members whose
/// names and literal values total at most 64 KiB (`range`), come before
/// its members', so the count and the total are kept to the end, after
/// any error. Then each member: `duplicate_member`, its value's `type`, a
/// literal's length (`range`), the name's form or a literal's bytes
/// (`format`), `refused_header`, and an `{"env": NAME}` object's members.
pub(super) fn headers<'de, A: MapAccess<'de>>(
    cx: &mut Cx<'de>,
    mut map: A,
) -> Result<Node<'de>, A::Error> {
    let start = cx.store.headers.len();
    let (mut n, mut total, mut over) = (0u64, 0u64, false);
    let mut names: Vec<Cow<'de, str>> = Vec::new();
    while let Some(Name(name)) = map.next_key()? {
        n += 1;
        total += name.len() as u64;
        if !cx.poisoned() && names.contains(&name) {
            cx.under(&name, |cx| cx.fail(Rule::DuplicateMember));
        }
        if cx.poisoned() {
            total += map.next_value_seed(Len)?;
        } else {
            let v = cx.under(&name, |cx| {
                map.next_value_seed(HeaderValue {
                    cx,
                    name: &name,
                    total: &mut total,
                })
            })?;
            if !cx.poisoned() {
                let name = cx.seg(&name);
                let (value, env) = match v {
                    Node::Str(s) => (cx.seg(&s), false),
                    Node::Obj(m) => {
                        let var = m.into_vec().into_iter().find_map(|(k, v)| match v {
                            Node::Str(s) if k == "env" => Some(s),
                            _ => None,
                        });
                        (var.map_or(EMPTY, |s| cx.seg(&s)), true)
                    }
                    _ => (EMPTY, false),
                };
                cx.store.headers.push(Header { name, value, env });
            }
            names.push(name);
        }
        if !over && (n > MAX_HEADERS || total > MAX_HEADER_BYTES) {
            over = true;
            cx.own(Rule::Range);
        }
    }
    let kept = cx.store.headers.len() - start;
    Ok(Node::Held(Held::Headers(
        Seg::new(start, kept).unwrap_or(EMPTY),
    )))
}

/// A header's value, under its `name`, a literal's length added to
/// `total` (`headers`' own rule).
struct HeaderValue<'c, 'a, 'n> {
    cx: &'c mut Cx<'a>,
    name: &'n str,
    total: &'n mut u64,
}

impl<'de> DeserializeSeed<'de> for HeaderValue<'_, 'de, '_> {
    type Value = Node<'de>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Node<'de>, D::Error> {
        d.deserialize_any(self)
    }
}

impl HeaderValue<'_, '_, '_> {
    fn refused(&self) -> bool {
        REFUSED_HEADERS
            .iter()
            .any(|r| r.eq_ignore_ascii_case(self.name))
    }

    /// Checks a literal before any copy of an escaped one: whether it
    /// passed.
    fn literal(self, s: &str) -> bool {
        *self.total += s.len() as u64;
        let rule = if s.len() as u64 > MAX_HEADER_VALUE {
            Some(Rule::Range)
        } else if !formats::is_token(self.name) || !formats::is_header_value(s) {
            Some(Rule::Format)
        } else if self.refused() {
            Some(Rule::RefusedHeader)
        } else {
            None
        };
        if let Some(r) = rule {
            self.cx.fail(r);
        }
        rule.is_none()
    }
}

impl<'de> Visitor<'de> for HeaderValue<'_, 'de, '_> {
    type Value = Node<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a header value")
    }

    wrong_kinds!(unit, bool, numbers, seq);

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Node<'de>, E> {
        Ok(Node::Str(Cow::Borrowed(if self.literal(v) {
            v
        } else {
            ""
        })))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node<'de>, E> {
        Ok(Node::Str(if self.literal(v) {
            Cow::Owned(v.to_owned())
        } else {
            Cow::Borrowed("")
        }))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Node<'de>, A::Error> {
        if !formats::is_token(self.name) {
            self.cx.fail(Rule::Format);
        } else if self.refused() {
            self.cx.fail(Rule::RefusedHeader);
        }
        if self.cx.poisoned() {
            drain_map(map)?;
            return Ok(Node::Null);
        }
        Ok(Node::Obj(
            object(self.cx, map, &ENV, false)?.into_boxed_slice(),
        ))
    }
}

/// A header value's bytes toward `headers`' total: a literal's length.
struct Len;

impl<'de> DeserializeSeed<'de> for Len {
    type Value = u64;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<u64, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Len {
    type Value = u64;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a header value")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
        Ok(v.len() as u64)
    }

    fn visit_unit<E: de::Error>(self) -> Result<u64, E> {
        Ok(0)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<u64, E> {
        Ok(0)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<u64, E> {
        Ok(0)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<u64, E> {
        Ok(0)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<u64, E> {
        Ok(0)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<u64, A::Error> {
        drain_seq(seq).map(|()| 0)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<u64, A::Error> {
        drain_map(map).map(|()| 0)
    }
}

/// A [`PREFER`] object.
static PREFER_TY: Ty = Ty::Object(&PREFER);

/// `prefer`: output key to Prefer, each entry to the store. A repeated
/// key is `duplicate_member` at the later entry, found by sorting the
/// keys at the end, so it replaces an error found in a later entry.
pub(super) fn prefer<'de, A: MapAccess<'de>>(
    cx: &mut Cx<'de>,
    mut map: A,
) -> Result<Node<'de>, A::Error> {
    let start = cx.store.prefer.len();
    let mut failed_at = None;
    while let Some(Name(key)) = map.next_key()? {
        if cx.poisoned() {
            map.next_value::<Skip>()?;
            continue;
        }
        let v = cx.under(&key, |cx| {
            map.next_value_seed(Value {
                cx,
                ty: &PREFER_TY,
                nullable: false,
            })
        })?;
        let mut e = PreferEntry {
            key: cx.seg(&key),
            input: None,
            md5: None,
        };
        if cx.poisoned() {
            failed_at = Some(cx.store.prefer.len() - start);
        } else if let Node::Obj(m) = v {
            for (name, v) in m.into_vec() {
                match (&*name, v) {
                    ("input", Node::Str(s)) => e.input = Some(cx.seg(&s)),
                    ("md5", Node::Str(s)) => e.md5 = Some(cx.seg(&s)),
                    _ => {}
                }
            }
        }
        cx.store.prefer.push(e);
    }
    if let Some(d) = first_repeated_key(cx, start)
        && failed_at.is_none_or(|f| d <= f)
        && let Some(e) = cx.store.prefer.get(start + d)
    {
        let key = cx.str(e.key).to_owned();
        cx.under(&key, |cx| cx.own(Rule::DuplicateMember));
    }
    Ok(Node::Held(Held::Prefer))
}

/// The first `prefer` entry from `start`, in document order, whose key an
/// earlier entry has.
fn first_repeated_key(cx: &Cx<'_>, start: usize) -> Option<usize> {
    let n = cx.store.prefer.len().checked_sub(start)?;
    let key = |i: u32| {
        cx.store
            .prefer
            .get(start + i as usize)
            .map_or("", |e| cx.str(e.key))
    };
    // Exactly sized: a prefer entry's index fits `u32` (a 64 MiB document
    // holds fewer than 2^32 of anything).
    let n = u32::try_from(n).unwrap_or(u32::MAX);
    sort_by_bytes(n, |i| key(i).as_bytes())
        .windows(2)
        .filter_map(|w| match *w {
            [a, b] if key(a) == key(b) => Some(b as usize),
            _ => None,
        })
        .min()
}

/// The sink. Its table is its first `kind` member's, wherever that member
/// is, so its members are gathered (as scalars: no sink field is an array
/// or object) and checked at its end. Gathering stops at the first member
/// that repeats a name or is in neither kind's table, which is an error
/// whatever the kind; after it, only the first `kind` is still looked for.
pub(super) fn sink<'de, A: MapAccess<'de>>(
    cx: &mut Cx<'de>,
    mut map: A,
) -> Result<Node<'de>, A::Error> {
    let mut members: Vec<Member<'de>> = Vec::new();
    let mut kind: Option<Node<'de>> = None;
    let mut stopped = false;
    while let Some(Name(name)) = map.next_key()? {
        if stopped {
            if kind.is_none() && name == "kind" {
                kind = Some(map.next_value_seed(Scalar { name: &name })?);
            } else {
                map.next_value::<Skip>()?;
            }
            continue;
        }
        let known = DIRECTORY
            .iter()
            .chain(&OBJECT_STORE)
            .any(|f| f.name == name);
        let repeat = members.iter().any(|(k, _)| *k == name);
        let v = map.next_value_seed(Scalar { name: &name })?;
        if name == "kind" && kind.is_none() {
            kind = Some(v.clone());
        }
        stopped = repeat || !known;
        members.push((name, v));
    }
    if let Err(e) = validate::sink(&cx.ptr, &members, kind.as_ref()) {
        cx.err.get_or_insert(e);
    }
    Ok(Node::Obj(members.into_boxed_slice()))
}

/// A sink member's value, under its `name`: a scalar, or an empty array
/// standing for any array or object (`type` for every sink field).
struct Scalar<'n> {
    name: &'n str,
}

/// The longest sink kind's name.
const LONGEST_KIND: usize = 12;
const _: () = assert!(LONGEST_KIND == "object_store".len());

impl Scalar<'_> {
    /// A short string that stands, before any copy, for an escaped `kind`
    /// or `endpoint` its checks refuse whatever the rest of it holds, and
    /// that they refuse by the same rule: a `kind` longer than every kind's
    /// name, an `endpoint` not ASCII, or past its cap or its host's cap
    /// as written (`formats`), and any other sink string past its byte cap
    /// (`bucket`, `region`, `prefix`: `range`, which `str_rule` checks
    /// first), so a refused sink string is never copied.
    fn stand_in(&self, v: &str) -> Option<Cow<'static, str>> {
        match self.name {
            "kind" if v.len() > LONGEST_KIND => Some(Cow::Borrowed("?")),
            "endpoint" if !v.is_ascii() => Some(Cow::Borrowed("\u{80}")),
            "endpoint"
                if v.len() as u64 > MAX_ENDPOINT
                    || formats::written_host_len(v) as u64 > MAX_HOST =>
            {
                Some(Cow::Owned(
                    " ".repeat(usize::try_from(MAX_ENDPOINT + 1).unwrap_or(1025)),
                ))
            }
            _ => capped(self.name, v),
        }
    }
}

/// A run of spaces one byte past the cap of the sink string `name`, when
/// `v` passes it: the same `range` (`str_rule` checks the length before
/// the form), at most 513 bytes, in place of a copy of `v`.
fn capped(name: &str, v: &str) -> Option<Cow<'static, str>> {
    let field = OBJECT_STORE
        .iter()
        .chain(&DIRECTORY)
        .find(|f| f.name == name)?;
    match field.ty {
        Ty::Str(Str { max: Some(max), .. }) if v.len() as u64 > max => {
            Some(Cow::Owned(" ".repeat(usize::try_from(max).ok()? + 1)))
        }
        _ => None,
    }
}

impl<'de> DeserializeSeed<'de> for Scalar<'_> {
    type Value = Node<'de>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Node<'de>, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Scalar<'_> {
    type Value = Node<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Node<'de>, E> {
        Ok(Node::Null)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Node<'de>, E> {
        Ok(Node::Bool(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node<'de>, E> {
        Ok(Node::UInt(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node<'de>, E> {
        Ok(u64::try_from(v).map_or(Node::NegInt(v), Node::UInt))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node<'de>, E> {
        Ok(Node::Float(v))
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Node<'de>, E> {
        Ok(Node::Str(Cow::Borrowed(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node<'de>, E> {
        Ok(Node::Str(
            self.stand_in(v).unwrap_or_else(|| Cow::Owned(v.to_owned())),
        ))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Node<'de>, A::Error> {
        drain_seq(seq)?;
        Ok(Node::Arr(Box::new([])))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Node<'de>, A::Error> {
        drain_map(map)?;
        Ok(Node::Arr(Box::new([])))
    }
}

/// `spec_version`'s verdict, by its token (`docs/BATCH_SPEC.md`'s
/// conventions): the integer token 1 is version 1; any other integer token
/// that fits in 64 bits, which `serde_json` hands over as an integer, is
/// `version`; every other number (a fraction or an exponent, `-0`, an
/// integer token too long for 64 bits), which it hands over as a double,
/// and every other value, is `type`.
pub(super) struct Version;

impl<'de> DeserializeSeed<'de> for Version {
    type Value = Result<(), Rule>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Version {
    type Value = Result<(), Rule>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a spec version")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(if v == SPEC_VERSION {
            Ok(())
        } else {
            Err(Rule::Version)
        })
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        match u64::try_from(v) {
            Ok(v) => self.visit_u64(v),
            Err(_) => Ok(Err(Rule::Version)),
        }
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(Err(Rule::Type))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(Err(Rule::Type))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(Err(Rule::Type))
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(Err(Rule::Type))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        drain_seq(seq).map(|()| Err(Rule::Type))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        drain_map(map).map(|()| Err(Rule::Type))
    }
}
