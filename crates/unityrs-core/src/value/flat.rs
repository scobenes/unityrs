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

/// Dropped without recursion and without allocating, each value looked at
/// once, so in time linear in the values held at any depth. The values
/// waiting are a stack of piles, each a value's own vector of items: the
/// innermost is taken from the end one value at a time, and a value with
/// items of its own has them taken out (its vector moves) to become the
/// innermost pile, the rest of the one it came from saved in a value of
/// its own, along with the pile above that (pushed into the slot the value
/// left free). A pile done, the one saved above it resumes. A value of any
/// depth takes the same stack, so a read (and a failed read's partial
/// values) may drop on a thread of any stack size, and the heap never goes
/// above what the values held.
impl Drop for Value {
    fn drop(&mut self) {
        if !self.deep() {
            return;
        }
        let mut pile = self.take_items();
        // The pile the current one came from, saved in a value; below the
        // first, each saved pile's last item is the one saved above it.
        let mut up: Option<Value> = None;
        let mut saved = 0_usize;
        loop {
            let Some(mut value) = pile.pop() else {
                let Some(mut link) = up.take() else {
                    break;
                };
                pile = link.take_items();
                saved -= 1;
                if saved > 0 {
                    up = pile.pop();
                }
                continue;
            };
            if !value.nests() {
                // A scalar or an empty container: nothing below it.
                continue;
            }
            let items = value.take_items();
            drop(value);
            if !pile.is_empty() {
                // The slot `value` left takes the pile saved above.
                if let Some(link) = up.take() {
                    pile.put_back(link);
                }
                up = Some(std::mem::replace(&mut pile, Pile::Values(Vec::new())).into_value());
                saved += 1;
            }
            pile = items;
        }
    }
}

/// Containers open in [`Value::each`] kept in place before any is kept on
/// the heap: most values nest no deeper, so a visit allocates nothing.
const INLINE_OPENS: usize = 16;

/// A stack whose first [`INLINE_OPENS`] items are in place, any more on the
/// heap: a value of any depth takes the same native stack. A slot past the
/// top holds a spent item (`T::default()`, or one popped).
struct Stack<T> {
    inline: [T; INLINE_OPENS],
    len: usize,
    spill: Vec<T>,
}

impl<T: Default> Stack<T> {
    fn new() -> Self {
        Self {
            inline: Default::default(),
            len: 0,
            spill: Vec::new(),
        }
    }

    fn push(&mut self, item: T) {
        match self.inline.get_mut(self.len) {
            Some(slot) => *slot = item,
            None => self.spill.push(item),
        }
        self.len += 1;
    }

    fn last_mut(&mut self) -> Option<&mut T> {
        let at = self.len.checked_sub(1)?;
        match self.inline.get_mut(at) {
            Some(slot) => Some(slot),
            None => self.spill.last_mut(),
        }
    }

    fn pop(&mut self) {
        if self.len > INLINE_OPENS {
            self.spill.pop();
        }
        self.len = self.len.saturating_sub(1);
    }
}

impl crate::pin::Slices for Value {
    /// Its strings, `TypelessData` and `vector<UInt8>` runs, depth first in
    /// order: the containers open above the one visited are iterators on a
    /// stack of their own ([`Stack`]), so a value of any depth takes the
    /// same native stack, and one no deeper than [`INLINE_OPENS`]
    /// allocates nothing.
    fn each(&mut self, f: &mut dyn FnMut(crate::pin::Slice<'_>)) {
        use crate::pin::Slice;
        enum Open<'a> {
            Items(std::slice::IterMut<'a, Value>),
            Entries(std::slice::IterMut<'a, (NodeId, Value)>),
        }
        impl Default for Open<'_> {
            fn default() -> Self {
                Self::Items(std::slice::IterMut::default())
            }
        }
        let mut opens: Stack<Open<'_>> = Stack::new();
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
        timed(Value::List(outer));
    }

    /// A comb 512 levels deep, each level `k` lists of one value beside the
    /// next level: each value is taken apart once, where a drop that walked
    /// back over the levels saved took `k * 512 * 512` steps.
    #[test]
    fn a_deep_comb_drops_in_linear_time() {
        let k = 2_000;
        let mut value = Value::Int(0);
        for _ in 0..512 {
            let mut level: Vec<Value> = (0..k).map(|i| Value::List(vec![Value::Int(i)])).collect();
            level.insert(level.len() / 2, value);
            value = Value::List(level);
        }
        timed(value);
    }

    fn timed(value: Value) {
        let start = std::time::Instant::now();
        drop(value);
        let took = start.elapsed();
        // Linear: well under a second even unoptimized; worse: minutes.
        assert!(
            took < std::time::Duration::from_secs(5),
            "dropped in {took:?}"
        );
    }
}
