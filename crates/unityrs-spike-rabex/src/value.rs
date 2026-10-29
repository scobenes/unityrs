//! Typetree values through rabex's serde layer.
//!
//! rabex has no raw typetree walker: its only value API is
//! `serde_typetree::Deserializer`, which drives a serde `Visitor`. A plain
//! visitor cannot tell a struct from a `pair` or a `vector<UInt8>` from
//! `TypelessData`, so every visit here is seeded with the node rabex is
//! reading. The node supplies the distinctions UnityPy's `parse_as_dict()`
//! makes (tuple versus dict versus list, bytes versus u8 list, float width);
//! what rabex itself reads (alignment, lengths, which nodes it supports)
//! cannot be changed from outside, and that is what the spike measures.

use std::fmt;

use rabex::typetree::TypeTreeNode;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::ser::{self, SerializeMap, SerializeSeq, SerializeTuple};

/// UnityPy's dict model of one typetree value.
pub enum Val {
    Bool(bool),
    I(i64),
    U(u64),
    /// float32 bit pattern.
    F32(u32),
    /// float64 bit pattern.
    F64(u64),
    Str(String),
    /// A string whose bytes are not valid UTF-8 (rabex visits these as a byte buffer).
    BadStr(Vec<u8>),
    /// `TypelessData`.
    Bytes(Vec<u8>),
    List(Vec<Val>),
    F32s(Vec<u32>),
    F64s(Vec<u64>),
    /// A non-empty `vector<UInt8>` or `vector<char>`.
    U8s(Vec<u8>),
    Tuple(Vec<Val>),
    /// One value per child node, in child order (duplicates included).
    Dict(Vec<Val>),
    /// What rabex visits for a childless struct node (`TypetreeNodeKind::Empty`).
    Unit,
}

/// The element node of a vector-shaped node (`children[0]` is the `Array`).
pub fn element(node: &TypeTreeNode) -> Option<&TypeTreeNode> {
    match node.children.first() {
        Some(a) if a.m_Type == "Array" && a.children.len() == 2 => Some(&a.children[1]),
        _ => None,
    }
}

/// Seeds a visit with the node rabex is about to read.
pub struct Seed<'t>(pub &'t TypeTreeNode);

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Val;

    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Val, D::Error> {
        d.deserialize_any(NodeVisitor(self.0))
    }
}

struct NodeVisitor<'t>(&'t TypeTreeNode);

fn collect<'de, A: SeqAccess<'de>, T: serde::Deserialize<'de>, U>(
    mut seq: A,
    f: impl Fn(T) -> U,
) -> Result<Vec<U>, A::Error> {
    let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(1 << 16));
    while let Some(x) = seq.next_element::<T>()? {
        out.push(f(x));
    }
    Ok(out)
}

impl<'de> Visitor<'de> for NodeVisitor<'_> {
    type Value = Val;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "a typetree value for {} {}",
            self.0.m_Type, self.0.m_Name
        )
    }

    fn visit_bool<E>(self, v: bool) -> Result<Val, E> {
        Ok(Val::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Val, E> {
        Ok(Val::I(v))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Val, E> {
        Ok(i64::try_from(v).map_or(Val::U(v), Val::I))
    }
    fn visit_f32<E>(self, v: f32) -> Result<Val, E> {
        Ok(Val::F32(v.to_bits()))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Val, E> {
        Ok(Val::F64(v.to_bits()))
    }
    /// rabex reads a `char` node as one byte widened to a `char`; UnityPy reads it as a u8.
    fn visit_char<E>(self, v: char) -> Result<Val, E> {
        Ok(Val::I(i64::from(u32::from(v))))
    }
    fn visit_str<E>(self, v: &str) -> Result<Val, E> {
        Ok(Val::Str(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Val, E> {
        Ok(Val::Str(v))
    }
    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Val, E> {
        Ok(Val::BadStr(v))
    }
    fn visit_unit<E>(self) -> Result<Val, E> {
        Ok(Val::Unit)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Val, A::Error> {
        let node = self.0;
        if node.m_Type == "TypelessData" {
            return collect(seq, |b: u8| b).map(Val::Bytes);
        }
        let elem =
            element(node).ok_or_else(|| de::Error::custom("sequence under a non-vector node"))?;
        let packed = match elem.m_Type.as_str() {
            "float" => Val::F32s(collect(seq, f32::to_bits)?),
            "double" => Val::F64s(collect(seq, f64::to_bits)?),
            "UInt8" => Val::U8s(collect(seq, |b: u8| b)?),
            #[allow(clippy::cast_possible_truncation)] // rabex widened a byte
            "char" => Val::U8s(collect(seq, |c: char| u32::from(c) as u8)?),
            _ => {
                let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(1 << 16));
                while let Some(v) = seq.next_element_seed(Seed(elem))? {
                    items.push(v);
                }
                return Ok(Val::List(items));
            }
        };
        let empty = match &packed {
            Val::F32s(v) => v.is_empty(),
            Val::F64s(v) => v.is_empty(),
            Val::U8s(v) => v.is_empty(),
            _ => false,
        };
        Ok(if empty { Val::List(Vec::new()) } else { packed })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Val, A::Error> {
        let node = self.0;
        if node.m_Type == "map" {
            let pair = element(node).ok_or_else(|| de::Error::custom("map without an Array"))?;
            let [key, value] = pair.children.as_slice() else {
                return Err(de::Error::custom("map pair without two children"));
            };
            let mut items = Vec::new();
            while let Some(k) = map.next_key_seed(Seed(key))? {
                let v = map.next_value_seed(Seed(value))?;
                items.push(Val::Tuple(vec![k, v]));
            }
            return Ok(Val::List(items));
        }
        let mut vals = Vec::with_capacity(node.children.len());
        for child in &node.children {
            if map.next_key::<IgnoredAny>()?.is_none() {
                return Err(de::Error::custom("struct ended before its children"));
            }
            vals.push(map.next_value_seed(Seed(child))?);
        }
        if map.next_key::<IgnoredAny>()?.is_some() {
            return Err(de::Error::custom("struct has more entries than children"));
        }
        Ok(if node.m_Type == "pair" && vals.len() == 2 {
            Val::Tuple(vals)
        } else {
            Val::Dict(vals)
        })
    }
}

// ------------------------------------------------------------------ resave (L3)

/// A value paired with its node, serialised back through rabex's serializer.
pub struct Ser<'a> {
    pub v: &'a Val,
    pub node: &'a TypeTreeNode,
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // narrowing back what was read
fn ser_int<S: ser::Serializer>(s: S, i: i64, ty: &str) -> Result<S::Ok, S::Error> {
    match ty {
        "SInt8" => s.serialize_i8(i as i8),
        "SInt16" | "short" => s.serialize_i16(i as i16),
        "SInt32" | "int" => s.serialize_i32(i as i32),
        "SInt64" | "long long" => s.serialize_i64(i),
        "UInt8" => s.serialize_u8(i as u8),
        "char" => s.serialize_char(char::from(i as u8)),
        "UInt16" | "unsigned short" => s.serialize_u16(i as u16),
        "UInt32" | "unsigned int" | "Type*" => s.serialize_u32(i as u32),
        "UInt64" | "unsigned long long" | "FileSize" => s.serialize_u64(i as u64),
        _ => Err(ser::Error::custom(format!("integer under node type {ty}"))),
    }
}

impl serde::Serialize for Ser<'_> {
    fn serialize<S: ser::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let node = self.node;
        let elem =
            || element(node).ok_or_else(|| ser::Error::custom("list under a non-vector node"));
        match self.v {
            Val::Bool(b) => s.serialize_bool(*b),
            Val::I(i) => ser_int(s, *i, &node.m_Type),
            Val::U(u) => s.serialize_u64(*u),
            Val::F32(b) => s.serialize_f32(f32::from_bits(*b)),
            Val::F64(b) => s.serialize_f64(f64::from_bits(*b)),
            Val::Str(v) => s.serialize_str(v),
            Val::BadStr(b) => s.serialize_bytes(b),
            Val::Unit => s.serialize_unit(),
            Val::Bytes(b) => {
                let mut seq = s.serialize_seq(Some(b.len()))?;
                for x in b {
                    seq.serialize_element(x)?;
                }
                seq.end()
            }
            Val::U8s(b) => {
                let is_char = elem()?.m_Type == "char";
                let mut seq = s.serialize_seq(Some(b.len()))?;
                for &x in b {
                    if is_char {
                        seq.serialize_element(&char::from(x))?;
                    } else {
                        seq.serialize_element(&x)?;
                    }
                }
                seq.end()
            }
            Val::F32s(v) => {
                let mut seq = s.serialize_seq(Some(v.len()))?;
                for &x in v {
                    seq.serialize_element(&f32::from_bits(x))?;
                }
                seq.end()
            }
            Val::F64s(v) => {
                let mut seq = s.serialize_seq(Some(v.len()))?;
                for &x in v {
                    seq.serialize_element(&f64::from_bits(x))?;
                }
                seq.end()
            }
            Val::List(items) => {
                let e = elem()?;
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for v in items {
                    seq.serialize_element(&Ser { v, node: e })?;
                }
                seq.end()
            }
            Val::Tuple(items) => {
                let mut t = s.serialize_tuple(items.len())?;
                for (v, child) in items.iter().zip(&node.children) {
                    t.serialize_element(&Ser { v, node: child })?;
                }
                t.end()
            }
            Val::Dict(vals) => {
                let mut m = s.serialize_map(Some(vals.len()))?;
                for (v, child) in vals.iter().zip(&node.children) {
                    m.serialize_entry(&child.m_Name, &Ser { v, node: child })?;
                }
                m.end()
            }
        }
    }
}
