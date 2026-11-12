//! The spec's values as an order-preserving JSON tree (`docs/plans/batch.md`
//! 2.11 step 2). Phase 1's parse (`parse`) builds one for the parts of a
//! spec a field caps, and keeps the rest in compact tables, which a
//! [`Node::Held`] stands for.
//!
//! `serde_json::Value` keeps the last of two members of one name silently
//! and sorts an object's members, so validation could neither refuse a
//! duplicate member (`duplicate_member`) nor report errors in document
//! order (`docs/BATCH_SPEC.md` section 10). This tree keeps every member,
//! duplicates included, in the order the document gives them. It is built
//! by a serde [`Visitor`] over `serde_json`'s parser, so the parser's
//! recursion limit stays (a value 128 levels deep is `syntax`), which
//! bounds the main thread's stack (`docs/plans/batch.md` 2.11, *Threads*).
//!
//! Numbers keep the class the parser gives them: an integer token that
//! fits `u64` or `i64` is [`Node::UInt`] or [`Node::NegInt`]; a token with
//! a fraction or an exponent, or an integer past both, is [`Node::Float`]
//! (`BATCH_SPEC` 1.1: an integer field takes only an integer token).
//!
//! A string with no escape borrows the document's text, so the tree
//! allocates nothing for it.

use std::borrow::Cow;
use std::fmt;

use super::store::Seg;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// One JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Node<'a> {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A non-negative integer token.
    UInt(u64),
    /// A negative integer token.
    NegInt(i64),
    /// Any other number.
    Float(f64),
    /// A string, borrowed from the text where it has no escape.
    Str(Cow<'a, str>),
    /// An array, its items in order.
    Arr(Box<[Node<'a>]>),
    /// An object, every member in document order, duplicates kept.
    Obj(Box<[Member<'a>]>),
    /// A value the parse keeps in the spec's tables.
    Held(Held),
}

/// What a [`Node::Held`] stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// `inputs` or `dependencies`: the bundles are in the store.
    Bundles,
    /// `prefer`: its entries are in the store.
    Prefer,
    /// A bundle's `headers`: its run of the header table.
    Headers(Seg),
    /// A bundle's `deps`: its run of phase 1's id table, at most
    /// `max_view_bundles`' greatest value + 1 ids long.
    Ids(Seg),
}

/// An object's member: its name and its value.
pub type Member<'a> = (Cow<'a, str>, Node<'a>);

#[cfg(test)]
impl<'a> Node<'a> {
    /// Parses `text` into a whole tree (the tests' reference reader). The
    /// error is `serde_json`'s, which the caller reports as `syntax`
    /// without its message (it may quote a value, `docs/plans/extract.md`
    /// E42).
    pub fn parse(text: &'a str) -> Result<Self, serde_json::Error> {
        let mut de = serde_json::Deserializer::from_str(text);
        let node = Self::deserialize(&mut de)?;
        de.end()?;
        Ok(node)
    }
}

impl<'de> Deserialize<'de> for Node<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NodeVisitor)
    }
}

/// A member's name, borrowed where it has no escape.
pub struct Name<'a>(pub Cow<'a, str>);

impl<'de> Deserialize<'de> for Name<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(NameVisitor)
    }
}

struct NameVisitor;

impl<'de> Visitor<'de> for NameVisitor {
    type Value = Name<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a member name")
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Name<'de>, E> {
        Ok(Name(Cow::Borrowed(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Name<'de>, E> {
        Ok(Name(Cow::Owned(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Name<'de>, E> {
        Ok(Name(Cow::Owned(v)))
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
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
        // serde_json gives a non-negative integer as u64; keep one form.
        Ok(u64::try_from(v).map_or(Node::NegInt(v), Node::UInt))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node<'de>, E> {
        Ok(Node::Float(v))
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Node<'de>, E> {
        Ok(Node::Str(Cow::Borrowed(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node<'de>, E> {
        Ok(Node::Str(Cow::Owned(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Node<'de>, E> {
        Ok(Node::Str(Cow::Owned(v)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node<'de>, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<Node>()? {
            items.push(item);
        }
        Ok(Node::Arr(items.into_boxed_slice()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node<'de>, A::Error> {
        let mut members = Vec::new();
        while let Some(Name(name)) = map.next_key::<Name<'de>>()? {
            let value = map.next_value::<Node<'de>>()?;
            members.push((name, value));
        }
        Ok(Node::Obj(members.into_boxed_slice()))
    }
}

#[cfg(test)]
mod tests {
    use super::Node;

    #[test]
    fn duplicates_and_order_are_kept() {
        let n = Node::parse(r#"{"b":1,"a":2,"b":3}"#).unwrap();
        let Node::Obj(m) = n else { panic!() };
        let names: Vec<&str> = m.iter().map(|(k, _)| &**k).collect();
        assert_eq!(names, ["b", "a", "b"]);
        assert!(
            m.iter()
                .all(|(k, _)| matches!(k, std::borrow::Cow::Borrowed(_)))
        );
        assert_eq!(m[2].1, Node::UInt(3));
    }

    #[test]
    fn numbers_keep_their_class() {
        let n = Node::parse("[0, -1, 1.0, 1e2, 18446744073709551616, 9007199254740993]").unwrap();
        let Node::Arr(a) = n else { panic!() };
        assert_eq!(a[0], Node::UInt(0));
        assert_eq!(a[1], Node::NegInt(-1));
        assert!(matches!(a[2], Node::Float(_)));
        assert!(matches!(a[3], Node::Float(_)));
        assert!(matches!(a[4], Node::Float(_)));
        assert_eq!(a[5], Node::UInt(9_007_199_254_740_993));
    }

    #[test]
    fn the_recursion_limit_stays() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(Node::parse(&deep).is_err());
        let ok = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(Node::parse(&ok).is_ok());
    }

    #[test]
    fn escaped_strings_are_owned_and_others_borrowed() {
        let n = Node::parse(r#"{"a\u0062":"c\n","d":"e"}"#).unwrap();
        let Node::Obj(m) = n else { panic!() };
        assert_eq!(&*m[0].0, "ab");
        assert!(matches!(m[0].0, std::borrow::Cow::Owned(_)));
        assert_eq!(m[0].1, Node::Str("c\n".into()));
        assert!(matches!(
            &m[1].1,
            Node::Str(std::borrow::Cow::Borrowed("e"))
        ));
    }

    #[test]
    fn trailing_text_is_an_error() {
        assert!(Node::parse("{} x").is_err());
        assert!(Node::parse("{} \n").is_ok());
    }
}
