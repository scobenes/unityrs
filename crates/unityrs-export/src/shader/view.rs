// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/ShaderConverter.py. See THIRD_PARTY_LICENSES.
//! Borrowed fields for export/ShaderConverter.py:92-372 and array entries
//! for export/ShaderConverter.py:64-68; no intermediate objects.
use super::Error;
use std::sync::Arc;
use unityrs_core::{Parsed, PrimArray, Value};
use unityrs_core::{limits::Meter, typetree::NodeId, walk::write::cleans_to};
#[derive(Clone, Copy)]
pub(super) struct View<'a> {
    pub parsed: &'a Parsed,
    pub value: &'a Value,
    pub meter: &'a Arc<Meter>,
    pub is_none: bool,
}
impl<'a> View<'a> {
    pub fn optional(self, key: &str) -> Result<Option<Self>, Error> {
        let Some(map) = self.value.as_dict() else {
            return Ok(None);
        };
        // Match Parsed::attr's last cleaned child / first raw key semantics,
        // charging each name before cleaning or comparing it, including misses.
        let list = usize::try_from(map.list())
            .ok()
            .and_then(|i| self.parsed.lists().get(i));
        let node = map
            .entries()
            .first()
            .and_then(|(k, _)| k.0.checked_sub(1))
            .map(NodeId);
        let kids = match (list, node) {
            (Some(l), Some(n)) if l.children(n).first() == map.entries().first().map(|e| &e.0) => {
                l.children(n)
            }
            _ => &[],
        };
        let mut found = None;
        if let Some(list) = list.filter(|_| !kids.is_empty()) {
            for child in kids.iter().rev() {
                let name = list.name(*child);
                self.name_work(name, key.as_bytes())?;
                if cleans_to(name, key.as_bytes()) {
                    for (raw, value) in self.parsed.entries(map) {
                        self.name_work(raw, name)?;
                        if raw == name {
                            found = Some((value, *child));
                            break;
                        }
                    }
                    break;
                }
            }
        } else {
            for ((node, _), (name, value)) in map.entries().iter().zip(self.parsed.entries(map)) {
                self.name_work(name, key.as_bytes())?;
                if cleans_to(name, key.as_bytes()) {
                    found = Some((value, *node));
                }
            }
        }
        Ok(found.map(|(value, node)| Self {
            value,
            is_none: matches!(value, Value::Bool(false))
                && list.is_some_and(|list| list.type_name(node) == b"NoneType"),
            ..self
        }))
    }
    fn name_work(self, name: &[u8], key: &[u8]) -> Result<(), Error> {
        // NUL search, cleaning, equality and the visited element.
        self.meter.count_decode(
            (name.len() as u64)
                .saturating_mul(2)
                .saturating_add(key.len() as u64)
                .saturating_add(1),
        )?;
        Ok(())
    }
    pub fn field(self, key: &'static str) -> Result<Self, Error> {
        self.optional(key)?.ok_or(Error::Field(key))
    }
    pub fn int(self) -> Result<i64, Error> {
        self.value.as_i64().ok_or(Error::Field("integer"))
    }
    #[allow(clippy::cast_precision_loss)] // Python converts integer numeric fields to double.
    pub fn num(self) -> Result<f64, Error> {
        self.value
            .as_f64()
            .or_else(|| self.value.as_i64().map(|i| i as f64))
            .ok_or(Error::Field("number"))
    }
    pub fn text(self) -> Result<&'a [u8], Error> {
        if let Value::Str(s) = self.value {
            Ok(s.as_bytes())
        } else {
            Err(Error::Field("string"))
        }
    }
    pub fn seq(self) -> Result<Seq<'a>, Error> {
        Seq::new(self.value)
    }
    pub fn list(self) -> Result<impl Iterator<Item = Self>, Error> {
        let v = match self.value {
            Value::List(v) => v.as_slice(),
            Value::Tuple(v) => v.as_ref(),
            _ => return Err(Error::Field("list")),
        };
        Ok(v.iter().map(move |value| Self {
            parsed: self.parsed,
            value,
            meter: self.meter,
            is_none: false,
        }))
    }
    pub fn empty(self) -> bool {
        match self.value {
            Value::Bytes(v) => v.is_empty(),
            Value::Str(v) => v.as_bytes().is_empty(),
            Value::List(v) => v.is_empty(),
            Value::Tuple(v) => v.is_empty(),
            Value::Prims(v) => v.is_empty(),
            _ => false,
        }
    }
    pub fn boolean(self) -> Result<bool, Error> {
        self.value
            .as_bool()
            .or_else(|| self.value.as_i64().map(|v| v != 0))
            .ok_or(Error::Field("bool"))
    }
}
#[derive(Clone, Copy)]
pub(super) struct Seq<'a>(&'a Value);
impl<'a> Seq<'a> {
    pub fn new(v: &'a Value) -> Result<Self, Error> {
        match v {
            Value::List(_) | Value::Tuple(_) | Value::Prims(_) => Ok(Self(v)),
            _ => Err(Error::Field("array")),
        }
    }
    pub fn len(self) -> usize {
        match self.0 {
            Value::List(v) => v.len(),
            Value::Tuple(v) => v.len(),
            Value::Prims(v) => v.len(),
            _ => 0,
        }
    }
    pub fn int(self, i: usize) -> Result<i64, Error> {
        i64::try_from(self.element(i)?.integer(ArrayUse::Platform)?)
            .map_err(|_| Error::NotImplemented)
    }
    pub fn entry(self, i: usize) -> Result<Entry<'a>, Error> {
        match self.element(i)? {
            Entry::Other(v) if matches!(v, Value::List(_) | Value::Prims(_)) => {
                Self::new(v)?.element(0)
            }
            entry => Ok(entry),
        }
    }
    pub fn segment_count(self, i: usize) -> Result<usize, Error> {
        match self.element(i)? {
            Entry::Other(v) if matches!(v, Value::List(_) | Value::Tuple(_) | Value::Prims(_)) => {
                Ok(Self::new(v)?.len())
            }
            _ => Ok(1),
        }
    }
    pub fn segment(self, i: usize, segment: usize) -> Result<Entry<'a>, Error> {
        match self.element(i)? {
            Entry::Other(v) if matches!(v, Value::List(_) | Value::Tuple(_) | Value::Prims(_)) => {
                Self::new(v)?.element(segment)
            }
            item if segment == 0 => Ok(item),
            _ => Err(Error::Index),
        }
    }
    fn element(self, i: usize) -> Result<Entry<'a>, Error> {
        if i >= self.len() {
            return Err(Error::Index);
        }
        macro_rules! at {
            ($v:expr) => {
                $v.get(i).map(|&n| i128::from(n))
            };
        }
        let n = match self.0 {
            Value::List(v) => return Ok(Entry::value(v.get(i).ok_or(Error::Index)?)),
            Value::Tuple(v) => return Ok(Entry::value(v.get(i).ok_or(Error::Index)?)),
            Value::Prims(p) => match p {
                PrimArray::U8(v) => at!(v),
                PrimArray::I8(v) => at!(v),
                PrimArray::U16(v) => at!(v),
                PrimArray::I16(v) => at!(v),
                PrimArray::U32(v) => at!(v),
                PrimArray::I32(v) => at!(v),
                PrimArray::U64(v) => at!(v),
                PrimArray::I64(v) => at!(v),
                PrimArray::F32(_) | PrimArray::F64(_) => return Ok(Entry::Float),
                PrimArray::Bool(v) => {
                    return v.get(i).copied().map(Entry::Bool).ok_or(Error::Index);
                }
            },
            _ => None,
        };
        n.map(Entry::Int).ok_or(Error::Index)
    }
}

#[derive(Clone, Copy)]
pub(super) enum ArrayUse {
    Decompressed,
    Platform,
}
#[derive(Clone, Copy)]
pub(super) enum Entry<'a> {
    Int(i128),
    Bool(bool),
    Other(&'a Value),
    Float,
}
impl<'a> Entry<'a> {
    fn value(value: &'a Value) -> Self {
        if let Value::Bool(n) = value {
            return Self::Bool(*n);
        }
        value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
            .map_or(Self::Other(value), Self::Int)
    }
    fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) | Self::Other(Value::Int(_) | Value::UInt(_)) => "int",
            Self::Float | Self::Other(Value::F32(_) | Value::F64(_)) => "float",
            Self::Other(Value::Str(_)) => "str",
            Self::Other(Value::List(_) | Value::Prims(_)) => "list",
            Self::Other(Value::Tuple(_)) => "tuple",
            Self::Other(Value::Bytes(_)) => "bytes",
            Self::Bool(_) | Self::Other(Value::Bool(_)) => "bool",
            Self::Other(Value::Dict(_)) => "UnknownObject",
        }
    }
    /// Evaluate offset + length before checking the slice's integer indices,
    /// as in export/ShaderConverter.py:79-80.
    pub fn slice(self, length: Self, len: usize) -> Result<std::ops::Range<usize>, Error> {
        let left = self.type_name();
        let right = length.type_name();
        if matches!(left, "str" | "list" | "tuple") {
            if left != right {
                return Err(Error::ConcatType {
                    container: left,
                    other: right,
                });
            }
        } else if !matches!(left, "int" | "float" | "bool")
            || !matches!(right, "int" | "float" | "bool")
        {
            return Err(Error::AddType { left, right });
        }
        if let (Some(offset), Some(length)) = (self.index(), length.index()) {
            return Ok(super::compressed_range(len, offset, length));
        }
        Err(Error::PythonType(
            "slice indices must be integers or None or have an __index__ method",
        ))
    }
    // Python bools keep their type name but implement integer arithmetic and
    // __index__, including slice bounds, LZ4 sizes and platform enum lookup.
    fn index(&self) -> Option<i128> {
        match self {
            Self::Int(n) => Some(*n),
            Self::Bool(n) => Some(i128::from(*n)),
            _ => None,
        }
    }
    pub fn integer(self, usage: ArrayUse) -> Result<i128, Error> {
        if let Some(n) = self.index() {
            return Ok(n);
        }
        match usage {
            ArrayUse::Platform => Err(Error::NotImplemented),
            ArrayUse::Decompressed => Err(Error::IntegerType(self.type_name())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unityrs_core::Bytes;
    #[test]
    fn tuple_entries_are_not_unwrapped() {
        for tuple in [
            Box::new([]) as Box<[Value]>,
            Box::new([Value::Int(6), Value::Int(999)]),
        ] {
            let value = Value::List(vec![Value::Tuple(tuple)]);
            let entry = Seq::new(&value).unwrap().entry(0).unwrap();
            let error = entry.slice(Entry::Int(1), 10).unwrap_err();
            assert_eq!(error.exception(), Some("TypeError"));
            assert_eq!(
                error.to_string(),
                "can only concatenate tuple (not \"int\") to tuple"
            );
        }
        // A tuple used as the outer sequence still supports ordinary indexing.
        let value = Value::Tuple(Box::new([Value::Int(6)]));
        assert_eq!(Seq::new(&value).unwrap().int(0).unwrap(), 6);
    }
    #[test]
    fn bool_entries_keep_their_type_and_integer_behavior() {
        for n in [false, true] {
            for value in [
                Value::List(vec![Value::Bool(n)]),
                Value::List(vec![Value::List(vec![Value::Bool(n)])]),
                Value::Prims(PrimArray::Bool(Box::new([n]))),
                Value::List(vec![Value::Prims(PrimArray::Bool(Box::new([n])))]),
            ] {
                let entry = || Seq::new(&value).unwrap().entry(0).unwrap();
                let text = Value::Str("bad".into());
                let error = entry().slice(Entry::value(&text), 10).unwrap_err();
                assert_eq!(error.exception(), Some("TypeError"));
                assert_eq!(
                    error.to_string(),
                    "unsupported operand type(s) for +: 'bool' and 'str'"
                );
                let error = Entry::value(&text).slice(entry(), 10).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    "can only concatenate str (not \"bool\") to str"
                );
                for usage in [ArrayUse::Decompressed, ArrayUse::Platform] {
                    assert_eq!(entry().integer(usage).unwrap(), i128::from(n));
                }
                let start = usize::from(n);
                assert_eq!(entry().slice(Entry::Int(1), 10).unwrap(), start..start + 1);
                assert_eq!(Entry::Int(1).slice(entry(), 10).unwrap(), 1..1 + start);
                assert_eq!(entry().slice(entry(), 10).unwrap(), start..2 * start);
                assert_eq!(
                    entry().slice(Entry::Float, 10).unwrap_err().to_string(),
                    "slice indices must be integers or None or have an __index__ method"
                );
            }
            let value = Value::Prims(PrimArray::Bool(Box::new([n])));
            assert_eq!(Seq::new(&value).unwrap().int(0).unwrap(), i64::from(n));
        }
    }
    #[test]
    fn missing_array_elements_have_the_reference_index_error() {
        for value in [
            Value::List(Vec::new()),
            Value::Tuple(Box::new([])),
            Value::Prims(PrimArray::U8(Bytes::new())),
            Value::Prims(PrimArray::I32(Box::new([]))),
            Value::Prims(PrimArray::F32(Box::new([]))),
            Value::List(vec![Value::List(Vec::new())]),
            Value::List(vec![Value::Prims(PrimArray::U8(Bytes::new()))]),
        ] {
            let Err(error) = Seq::new(&value).unwrap().entry(0) else {
                panic!("missing entry was accepted")
            };
            assert_eq!(error.exception(), Some("IndexError"));
            assert_eq!(error.to_string(), "list index out of range");
        }
    }
}
