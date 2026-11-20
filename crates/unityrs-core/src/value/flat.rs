//! A value's depth without recursion: dropped, and its slices visited
//! ([`crate::pin`]), with the same native stack at any depth, so a read
//! runs on a thread of any stack size.

use super::{ListIx, Map, PrimArray, Value};
use crate::typetree::NodeId;

impl Value {
    /// Whether this value holds another value (a non-empty list, tuple or
    /// dict).
    fn nests(&self) -> bool {
        match self {
            Self::List(items) => !items.is_empty(),
            Self::Tuple(items) => !items.is_empty(),
            Self::Dict(map) => !map.entries.is_empty(),
            _ => false,
        }
    }

    /// Whether an item of this value holds values of its own: dropped as
    /// is, it would drop a level below its items.
    fn deep(&self) -> bool {
        match self {
            Self::List(items) => items.iter().any(Self::nests),
            Self::Tuple(items) => items.iter().any(Self::nests),
            Self::Dict(map) => map.entries.iter().any(|(_, v)| v.nests()),
            _ => false,
        }
    }

    /// The values this one holds, taken out of it (no allocation: its own
    /// vector moves).
    fn take_items(&mut self) -> Pile {
        match self {
            Self::List(items) => Pile::Values(std::mem::take(items)),
            Self::Tuple(items) => Pile::Values(std::mem::take(items).into_vec()),
            Self::Dict(map) => Pile::Entries(std::mem::take(&mut map.entries)),
            _ => Pile::Values(Vec::new()),
        }
    }
}

/// Values waiting to be dropped: a value's own items.
enum Pile {
    Values(Vec<Value>),
    Entries(Vec<(NodeId, Value)>),
}

impl Pile {
    fn pop(&mut self) -> Option<Value> {
        match self {
            Self::Values(items) => items.pop(),
            Self::Entries(entries) => entries.pop().map(|(_, v)| v),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Values(items) => items.is_empty(),
            Self::Entries(entries) => entries.is_empty(),
        }
    }

    /// A value holding these, as any other: dropping it drops them.
    fn into_value(self) -> Value {
        match self {
            Self::Values(items) => Value::List(items),
            Self::Entries(entries) => Value::Dict(Map {
                list: ListIx::default(),
                entries,
            }),
        }
    }

    /// Adds `value` where one was just taken: never allocates.
    fn put_back(&mut self, value: Value) {
        match self {
            Self::Values(items) => items.push(value),
            Self::Entries(entries) => entries.push((NodeId(0), value)),
        }
    }
}

/// Dropped without recursion and without allocating, in time linear in
/// the values held: a value whose items hold values of their own is taken
/// apart one value at a time, each value looked at once. The items being
/// dropped are a value's own vector; on reaching a value with items,
/// the rest of that vector goes into the emptied value, which goes where
/// one of its items was taken from, and its items are dropped next. A value
/// of any depth takes the same stack, so a read (and a failed read's
/// partial values) may drop on a thread of any stack size, and the heap
/// never goes above what the values held.
impl Drop for Value {
    fn drop(&mut self) {
        if !self.deep() {
            return;
        }
        let mut pile = self.take_items();
        let mut hand: Option<Value> = None;
        while let Some(mut value) = hand.take().or_else(|| pile.pop()) {
            if !value.nests() {
                // A scalar or an empty container: nothing below it.
                drop(value);
                continue;
            }
            let mut items = value.take_items();
            drop(value);
            if pile.is_empty() {
                pile = items;
                continue;
            }
            // `items` holds a value (the value was deep): taking it frees
            // the slot the rest of `pile` goes into.
            hand = items.pop();
            items.put_back(std::mem::replace(&mut pile, Pile::Values(Vec::new())).into_value());
            pile = items;
        }
    }
}

impl crate::pin::Slices for Value {
    /// Its strings, `TypelessData` and `vector<UInt8>` runs, depth first in
    /// order: the containers open above the one visited are iterators on a
    /// heap stack, so a value of any depth takes the same native stack.
    fn each(&mut self, f: &mut dyn FnMut(crate::pin::Slice<'_>)) {
        use crate::pin::Slice;
        enum Open<'a> {
            Items(std::slice::IterMut<'a, Value>),
            Entries(std::slice::IterMut<'a, (NodeId, Value)>),
        }
        let mut opens: Vec<Open<'_>> = Vec::new();
        let mut next = Some(self);
        loop {
            let value = if let Some(value) = next.take() {
                value
            } else {
                let Some(open) = opens.last_mut() else {
                    break;
                };
                let item = match open {
                    Open::Items(items) => items.next(),
                    Open::Entries(entries) => entries.next().map(|(_, v)| v),
                };
                let Some(item) = item else {
                    opens.pop();
                    continue;
                };
                item
            };
            match value {
                Self::Str(s) => f(Slice::Str(s)),
                Self::Bytes(b) | Self::Prims(PrimArray::U8(b)) => f(Slice::Bytes(b)),
                Self::List(items) => opens.push(Open::Items(items.iter_mut())),
                Self::Tuple(items) => opens.push(Open::Items(items.iter_mut())),
                Self::Dict(m) => opens.push(Open::Entries(m.entries.iter_mut())),
                Self::Bool(_)
                | Self::Int(_)
                | Self::UInt(_)
                | Self::F32(_)
                | Self::F64(_)
                | Self::Prims(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ListIx, Map, Value};
    use crate::typetree::NodeId;

    /// `vector<vector<Inner{int}>>`: `n` empty lists, then `n` lists of one
    /// class each. Each nested value is taken apart once; a drop that looked
    /// over the values still waiting at each one took `n * n` steps.
    #[test]
    fn a_wide_list_of_short_lists_drops_in_linear_time() {
        let n = 200_000;
        let mut outer = Vec::with_capacity(2 * n);
        outer.extend((0..n).map(|_| Value::List(Vec::new())));
        outer.extend((0..n).map(|_| {
            Value::List(vec![Value::Dict(Map {
                list: ListIx::default(),
                entries: vec![(NodeId(0), Value::Int(1))],
            })])
        }));
        let value = Value::List(outer);
        let start = std::time::Instant::now();
        drop(value);
        let took = start.elapsed();
        // Linear: milliseconds even unoptimized; quadratic: hours.
        assert!(
            took < std::time::Duration::from_secs(5),
            "dropped in {took:?}"
        );
    }
}
