//! Phase 1's parse: the document read once, each value checked by its
//! per-value rules as it is read, in document order (`docs/BATCH_SPEC.md`
//! section 10), and the spec's values built (`docs/plans/batch.md` 2.11
//! step 2).
//!
//! The parse is a serde `DeserializeSeed` per field type over
//! `serde_json`'s parser, so the parser's recursion limit stays (a value
//! 128 levels deep is `syntax`) and bounds the main thread's stack.
//!
//! What it holds is bounded by the document's legal parts (review round
//! 1's D3):
//! - after the first per-value error, nothing more is built: every later
//!   value is read as [`Skip`], for `syntax` alone, but for what can
//!   still change the error: `spec_version` anywhere at the top level, and
//!   the counts and byte totals of the arrays and `headers` objects the
//!   error is inside, whose own rules come before their members';
//! - an unknown or repeated member's value is never read into anything;
//! - an array stops being read into anything once its count passes its
//!   cap, which is then its error;
//! - the parts no field caps, the bundles, their `deps` and `headers`, and
//!   `prefer`, go to compact tables ([`Store`]), each string a `Seg` of the
//!   document's text, or of the escape table when the document escaped it.

use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::num::NonZeroU64;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

use super::fields::{self, Array, BUNDLE, Field, MAX_VIEW_BUNDLES, Str, Ty};
use super::model::{Bundle, ESCAPED, Header, PreferEntry};
use super::node::{Held, Member, Name, Node};
use super::store::{Chunks, EMPTY, Seg};
use super::validate;
use super::{BadSpec, Rule, push_token};

/// The most ids of one `deps` list the parse keeps: one past the greatest
/// `max_view_bundles`, so a longer list is still seen too long (`deps`).
const MAX_LIST: usize = 65_537;
const _: () = assert!(MAX_LIST as u64 == MAX_VIEW_BUNDLES + 1);

/// Phase 1's tables, as the parse fills them.
#[derive(Debug, Default)]
pub(super) struct Store {
    pub(super) inputs: Chunks<Bundle>,
    pub(super) dependencies: Chunks<Bundle>,
    /// Each input's `deps`, a run of `ids`.
    pub(super) inputs_deps: Chunks<Seg>,
    /// Each dependency's `deps`, a run of `ids`.
    pub(super) dependencies_deps: Chunks<Seg>,
    /// Every kept `deps` id.
    pub(super) ids: Chunks<Seg>,
    pub(super) headers: Chunks<Header>,
    pub(super) prefer: Chunks<PreferEntry>,
    /// The strings the document escaped.
    pub(super) escaped: String,
    /// Whether `inputs` came before `dependencies`.
    pub(super) inputs_first: bool,
}

/// What the parse found.
pub(super) struct Parsed<'a> {
    /// The top level's members but `spec_version`, in document order, or
    /// `None` when the document is not an object.
    pub(super) top: Option<Vec<Member<'a>>>,
    /// `spec_version`'s own verdict.
    pub(super) version: Result<(), Rule>,
    /// The first per-value error after `spec_version`'s.
    pub(super) err: Option<BadSpec>,
    pub(super) store: Store,
}

/// Parses `text`; the error is `serde_json`'s (`syntax`).
pub(super) fn parse(text: &str) -> Result<Parsed<'_>, serde_json::Error> {
    let mut cx = Cx {
        text,
        ptr: String::new(),
        err: None,
        store: Store {
            inputs_first: true,
            ..Store::default()
        },
        seen_dependencies: false,
        versions: 0,
        version: Err(Rule::Missing),
    };
    let mut de = serde_json::Deserializer::from_str(text);
    let top = Top(&mut cx).deserialize(&mut de)?;
    de.end()?;
    let version = match cx.versions {
        0 => Err(Rule::Missing),
        1 => cx.version,
        _ => Err(Rule::DuplicateMember),
    };
    Ok(Parsed {
        top,
        version,
        err: cx.err,
        store: cx.store,
    })
}

/// The parse's state.
struct Cx<'a> {
    text: &'a str,
    /// The pointer of the value being read.
    ptr: String,
    /// The first per-value error: once set, nothing more is built.
    err: Option<BadSpec>,
    store: Store,
    seen_dependencies: bool,
    /// How many `spec_version` members, and the first one's verdict.
    versions: u32,
    version: Result<(), Rule>,
}

impl Cx<'_> {
    fn poisoned(&self) -> bool {
        self.err.is_some()
    }

    /// `rule` broken at the current pointer, if nothing was before.
    fn fail(&mut self, rule: Rule) {
        if self.err.is_none() {
            self.err = Some(BadSpec::new(rule, self.ptr.clone()));
        }
    }

    /// A value's own rule broken at the current pointer: it comes before
    /// whatever its members broke.
    fn own(&mut self, rule: Rule) {
        self.err = Some(BadSpec::new(rule, self.ptr.clone()));
    }

    fn under<T>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> T) -> T {
        let len = self.ptr.len();
        push_token(&mut self.ptr, name);
        let r = f(self);
        self.ptr.truncate(len);
        r
    }

    fn under_index<T>(&mut self, i: u64, f: impl FnOnce(&mut Self) -> T) -> T {
        let len = self.ptr.len();
        let _ = write!(self.ptr, "/{i}");
        let r = f(self);
        self.ptr.truncate(len);
        r
    }

    /// `s` as a `Seg`: of the text when it lies in the text (the parser
    /// borrowed it), else appended to the escape table.
    fn seg(&mut self, s: &str) -> Seg {
        let at = (s.as_ptr() as usize).wrapping_sub(self.text.as_ptr() as usize);
        if at <= self.text.len()
            && s.len() <= self.text.len() - at
            && let Some(seg) = Seg::new(at, s.len())
        {
            return seg;
        }
        let at = ESCAPED + self.store.escaped.len();
        self.store.escaped.push_str(s);
        Seg::new(at, s.len()).unwrap_or(EMPTY)
    }

    /// The string a `Seg` of [`Self::seg`] names.
    fn str(&self, s: Seg) -> &str {
        let r = s.range();
        match r.start.checked_sub(ESCAPED) {
            Some(at) => self.store.escaped.get(at..at + s.len()),
            None => self.text.get(r),
        }
        .unwrap_or_default()
    }
}

/// The visit methods of a JSON kind a field does not take: `type`, and
/// the value read and dropped.
macro_rules! wrong_kinds {
    ($($kind:ident),*) => {
        $(wrong_kinds!(@ $kind);)*
    };
    (@ unit) => {
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            self.cx.fail(Rule::Type);
            Ok(Node::Null)
        }
    };
    (@ bool) => {
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            self.cx.fail(Rule::Type);
            Ok(Node::Null)
        }
    };
    (@ numbers) => {
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            self.cx.fail(Rule::Type);
            Ok(Node::Null)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            self.cx.fail(Rule::Type);
            Ok(Node::Null)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            self.cx.fail(Rule::Type);
            Ok(Node::Null)
        }
    };
    (@ seq) => {
        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
            self.cx.fail(Rule::Type);
            drain_seq(seq)?;
            Ok(Node::Null)
        }
    };
}

mod members;
mod skip;

use members::{Version, headers, prefer, sink};
use skip::{Skip, drain_map, drain_seq};

/// The document: an object, or `None` (`type`, the rule alone).
struct Top<'c, 'a>(&'c mut Cx<'a>);

impl<'de> DeserializeSeed<'de> for Top<'_, 'de> {
    type Value = Option<Vec<Member<'de>>>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Top<'_, 'de> {
    type Value = Option<Vec<Member<'de>>>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        object(self.0, map, &fields::TOP, true).map(Some)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        drain_seq(seq)?;
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }
}

/// An object of `table`'s members, in document order: `duplicate_member`,
/// `unknown_field`, then the value; at its end, `missing` for the first
/// required member, in the table's order, it lacks. At the top level,
/// every `spec_version` member is `spec_version`'s rule's, read even after
/// an error.
fn object<'de, A: MapAccess<'de>>(
    cx: &mut Cx<'de>,
    mut map: A,
    table: &'static [Field],
    top: bool,
) -> Result<Vec<Member<'de>>, A::Error> {
    let mut seen: u64 = 0;
    let mut members = Vec::new();
    while let Some(Name(name)) = map.next_key()? {
        if top && name == "spec_version" {
            let v = map.next_value_seed(Version)?;
            cx.versions = cx.versions.saturating_add(1);
            if cx.versions == 1 {
                cx.version = v;
            }
            continue;
        }
        if cx.poisoned() {
            map.next_value::<Skip>()?;
            continue;
        }
        match table.iter().enumerate().find(|(_, f)| f.name == name) {
            Some((i, _)) if seen & (1 << i) != 0 => {
                cx.under(&name, |cx| cx.fail(Rule::DuplicateMember));
                map.next_value::<Skip>()?;
            }
            None => {
                cx.under(&name, |cx| cx.fail(Rule::UnknownField));
                map.next_value::<Skip>()?;
            }
            Some((i, f)) => {
                seen |= 1 << i;
                let v = cx.under(&name, |cx| {
                    map.next_value_seed(Value {
                        cx,
                        ty: &f.ty,
                        nullable: f.nullable,
                    })
                })?;
                members.push((name, v));
            }
        }
    }
    if !cx.poisoned()
        && let Some((_, f)) = table
            .iter()
            .enumerate()
            .find(|(i, f)| f.required && seen & (1 << i) == 0 && !(top && f.name == "spec_version"))
    {
        cx.under(f.name, |cx| cx.fail(Rule::Missing));
    }
    Ok(members)
}

/// One value of type `ty`.
struct Value<'c, 'a> {
    cx: &'c mut Cx<'a>,
    ty: &'static Ty,
    nullable: bool,
}

impl<'de> DeserializeSeed<'de> for Value<'_, 'de> {
    type Value = Node<'de>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Node<'de>, D::Error> {
        if self.cx.poisoned() {
            Skip::deserialize(d)?;
            return Ok(Node::Null);
        }
        d.deserialize_any(self)
    }
}

impl<'de> Value<'_, 'de> {
    fn number(self, n: Node<'de>) -> Node<'de> {
        match *self.ty {
            Ty::Int { min, max } => {
                if let Some(r) = validate::int_rule(&n, min, max) {
                    self.cx.fail(r);
                }
            }
            _ => self.cx.fail(Rule::Type),
        }
        n
    }

    /// Checks a string value where `serde_json` hands it over, before any
    /// copy is made of an escaped one: whether it passed.
    fn string(self, s: &str) -> bool {
        let rule = match self.ty {
            Ty::Str(st) => validate::str_rule(s, st),
            _ => Some(Rule::Type),
        };
        if let Some(r) = rule {
            self.cx.fail(r);
        }
        rule.is_none()
    }
}

impl<'de> Visitor<'de> for Value<'_, 'de> {
    type Value = Node<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Node<'de>, E> {
        if !self.nullable {
            self.cx.fail(Rule::Type);
        }
        Ok(Node::Null)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Node<'de>, E> {
        if !matches!(self.ty, Ty::Bool) {
            self.cx.fail(Rule::Type);
        }
        Ok(Node::Bool(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node<'de>, E> {
        Ok(self.number(Node::UInt(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node<'de>, E> {
        Ok(self.number(u64::try_from(v).map_or(Node::NegInt(v), Node::UInt)))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node<'de>, E> {
        Ok(self.number(Node::Float(v)))
    }

    // A refused string is never kept: the parse builds nothing after its
    // first error, so the node stands for it alone.
    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Node<'de>, E> {
        Ok(Node::Str(Cow::Borrowed(if self.string(v) {
            v
        } else {
            ""
        })))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node<'de>, E> {
        Ok(Node::Str(if self.string(v) {
            Cow::Owned(v.to_owned())
        } else {
            Cow::Borrowed("")
        }))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Node<'de>, A::Error> {
        match *self.ty {
            Ty::Array(a) => array(self.cx, seq, a),
            Ty::Deps => deps(self.cx, seq),
            Ty::Bundles(a, inputs) => bundles(self.cx, seq, a, inputs),
            _ => {
                self.cx.fail(Rule::Type);
                drain_seq(seq)?;
                Ok(Node::Null)
            }
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Node<'de>, A::Error> {
        match *self.ty {
            Ty::Object(table) => Ok(Node::Obj(
                object(self.cx, map, table, false)?.into_boxed_slice(),
            )),
            Ty::Prefer => prefer(self.cx, map),
            Ty::Headers => headers(self.cx, map),
            Ty::Sink => sink(self.cx, map),
            _ => {
                self.cx.fail(Rule::Type);
                drain_map(map)?;
                Ok(Node::Null)
            }
        }
    }
}

/// An array's items, each read by `item` (which says whether there was
/// one) while no error is found and the count is within `max`; every
/// other item is read as `Skip` and counted. The array's own rule,
/// its count in `min..=max`, comes before what its items break, so it
/// replaces their error; a count past `max` is known, and is the error,
/// as soon as the item past it is read.
fn items<'de, A: SeqAccess<'de>>(
    cx: &mut Cx<'de>,
    seq: &mut A,
    min: u64,
    max: Option<u64>,
    mut item: impl FnMut(&mut Cx<'de>, &mut A) -> Result<bool, A::Error>,
) -> Result<(), A::Error> {
    let mut n = 0u64;
    let mut over = false;
    loop {
        let got = if cx.poisoned() || max.is_some_and(|m| n >= m) {
            seq.next_element::<Skip>()?.is_some()
        } else {
            cx.under_index(n, |cx| item(cx, seq))?
        };
        if !got {
            break;
        }
        n += 1;
        if !over && max.is_some_and(|m| n > m) {
            over = true;
            cx.own(Rule::Range);
        }
    }
    if !over && n < min {
        cx.own(Rule::Range);
    }
    Ok(())
}

/// An array a field caps: its items in a tree. A repeat in a `unique`
/// array is `range`, at the later item.
fn array<'de, A: SeqAccess<'de>>(
    cx: &mut Cx<'de>,
    mut seq: A,
    a: &'static Array,
) -> Result<Node<'de>, A::Error> {
    let mut out: Vec<Node<'de>> = Vec::new();
    items(cx, &mut seq, a.min, a.max, |cx, seq| {
        let Some(v) = seq.next_element_seed(Value {
            cx: &mut *cx,
            ty: &a.item,
            nullable: false,
        })?
        else {
            return Ok(false);
        };
        if a.unique
            && let Node::Str(s) = &v
            && out.iter().any(|o| matches!(o, Node::Str(t) if t == s))
        {
            cx.fail(Rule::Range);
        }
        out.push(v);
        Ok(true)
    })?;
    Ok(Node::Arr(out.into_boxed_slice()))
}

/// Any string, a `deps` id.
static ANY_STRING: Ty = Ty::Str(Str {
    min: 0,
    max: None,
    values: &[],
    format: None,
});

/// A bundle's `deps`: strings of any count, at most [`MAX_LIST`] kept in
/// the id table.
fn deps<'de, A: SeqAccess<'de>>(cx: &mut Cx<'de>, mut seq: A) -> Result<Node<'de>, A::Error> {
    let start = cx.store.ids.len();
    let mut kept = 0;
    items(cx, &mut seq, 0, None, |cx, seq| {
        let Some(v) = seq.next_element_seed(Value {
            cx: &mut *cx,
            ty: &ANY_STRING,
            nullable: false,
        })?
        else {
            return Ok(false);
        };
        if let Node::Str(s) = v
            && !cx.poisoned()
            && kept < MAX_LIST
        {
            let seg = cx.seg(&s);
            cx.store.ids.push(seg);
            kept += 1;
        }
        Ok(true)
    })?;
    Ok(Node::Held(Held::Ids(
        Seg::new(start, kept).unwrap_or(EMPTY),
    )))
}

/// A [`BUNDLE`] object.
static BUNDLE_TY: Ty = Ty::Object(&BUNDLE);

/// `inputs` or `dependencies`: each bundle, once read whole without an
/// error, goes to the store.
fn bundles<'de, A: SeqAccess<'de>>(
    cx: &mut Cx<'de>,
    mut seq: A,
    a: &'static Array,
    inputs: bool,
) -> Result<Node<'de>, A::Error> {
    if inputs {
        cx.store.inputs_first = !cx.seen_dependencies;
    } else {
        cx.seen_dependencies = true;
    }
    items(cx, &mut seq, a.min, a.max, |cx, seq| {
        let Some(v) = seq.next_element_seed(Value {
            cx: &mut *cx,
            ty: &BUNDLE_TY,
            nullable: false,
        })?
        else {
            return Ok(false);
        };
        if let Node::Obj(m) = v
            && !cx.poisoned()
        {
            bundle(cx, m.into_vec(), inputs);
        }
        Ok(true)
    })?;
    Ok(Node::Held(Held::Bundles))
}

/// Stores a bundle the parse accepted.
fn bundle<'de>(cx: &mut Cx<'de>, members: Vec<Member<'de>>, inputs: bool) {
    let mut b = Bundle {
        id: EMPTY,
        path: None,
        url: None,
        sha256: None,
        size1: None,
        headers: None,
    };
    let mut deps = EMPTY;
    for (name, v) in members {
        match (&*name, v) {
            ("id", Node::Str(s)) => b.id = cx.seg(&s),
            ("path", Node::Str(s)) => b.path = Some(cx.seg(&s)),
            ("url", Node::Str(s)) => b.url = Some(cx.seg(&s)),
            ("sha256", Node::Str(s)) => b.sha256 = Some(cx.seg(&s)),
            ("size", Node::UInt(v)) => b.size1 = v.checked_add(1).and_then(NonZeroU64::new),
            ("headers", Node::Held(Held::Headers(h))) => b.headers = Some(h),
            ("deps", Node::Held(Held::Ids(d))) => deps = d,
            _ => {}
        }
    }
    if inputs {
        cx.store.inputs.push(b);
        cx.store.inputs_deps.push(deps);
    } else {
        cx.store.dependencies.push(b);
        cx.store.dependencies_deps.push(deps);
    }
}
