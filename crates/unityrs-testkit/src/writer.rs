//! An endian-aware byte writer, the builders' only primitive.
//!
//! **Fields.** Every 4- and 8-byte integer a writer writes (`u32`, `i32`,
//! `u64`, `i64`: each count, length, size and offset the builders write,
//! and every other integer of those widths) is a *field*. While
//! [`with_fields`] runs a build on the calling thread, the writers number
//! the fields that build writes, in write order, report each one, and can
//! write another value in place of any of them. G3b's mutator
//! ([`crate::mutate`]) builds the gate's inputs this way, so a field is
//! changed before anything that follows from it is computed: the bytes a
//! codec compresses, the size of a block, the entry that holds a file.
//! Outside [`with_fields`] a writer writes what it is given.

use std::cell::RefCell;

/// Little- or big-endian bytes, appended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ByteWriter {
    buf: Vec<u8>,
    big_endian: bool,
}

impl ByteWriter {
    /// A little-endian writer.
    #[must_use]
    pub fn le() -> Self {
        Self::default()
    }

    /// A big-endian writer.
    #[must_use]
    pub fn be() -> Self {
        Self {
            buf: Vec::new(),
            big_endian: true,
        }
    }

    /// Switches byte order from here on.
    pub fn set_big_endian(&mut self, big: bool) -> &mut Self {
        self.big_endian = big;
        self
    }

    /// Bytes written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The bytes.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }

    /// The bytes so far.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    /// Raw bytes.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// `b` and a NUL.
    pub fn cstr(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self.buf.push(0);
        self
    }

    /// Zero bytes up to a multiple of `n` (UnityPy's `align_stream`, 4 by default).
    pub fn align(&mut self, n: usize) -> &mut Self {
        if n > 1 {
            let pad = (n - self.buf.len() % n) % n;
            self.buf.resize(self.buf.len() + pad, 0);
        }
        self
    }

    /// A `u8`.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    /// A `bool` as one byte.
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(u8::from(v))
    }

    /// An `i8`.
    pub fn i8(&mut self, v: i8) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
}

/// One field a build wrote (module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// 4 or 8 bytes.
    pub width: usize,
    /// Written big-endian.
    pub big_endian: bool,
    /// The value it was given, as its `width` bytes read unsigned.
    pub value: u64,
}

/// The state of [`with_fields`] on one thread.
struct Hook {
    /// Every field written so far.
    written: Vec<Field>,
    /// `(field number, value)` to write instead, sorted by number.
    overrides: Vec<(usize, u64)>,
}

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Runs `build` on this thread with each field it writes numbered from 0 in
/// write order: field `n` is written as `v` where `overrides` holds
/// `(n, v)` (its low `width` bytes), and as given otherwise. Returns what
/// `build` returned and every field it wrote, with the value it was given.
/// A nested call runs its build with a numbering of its own.
pub fn with_fields<T>(overrides: &[(usize, u64)], build: impl FnOnce() -> T) -> (T, Vec<Field>) {
    let mut sorted = overrides.to_vec();
    sorted.sort_unstable();
    let outer = HOOK.with(|h| {
        h.borrow_mut().replace(Hook {
            written: Vec::new(),
            overrides: sorted,
        })
    });
    let out = build();
    let hook = HOOK.with(|h| std::mem::replace(&mut *h.borrow_mut(), outer));
    (out, hook.map(|h| h.written).unwrap_or_default())
}

/// The value field `value` is written as under [`with_fields`], and records it.
fn field(width: usize, big_endian: bool, value: u64) -> u64 {
    HOOK.with(|h| {
        let mut h = h.borrow_mut();
        let Some(hook) = h.as_mut() else {
            return value;
        };
        let n = hook.written.len();
        hook.written.push(Field {
            width,
            big_endian,
            value,
        });
        hook.overrides
            .binary_search_by_key(&n, |o| o.0)
            .ok()
            .and_then(|i| hook.overrides.get(i))
            .map_or(value, |o| o.1)
    })
}

macro_rules! field {
    ($($name:ident: $t:ty, $u:ty),*) => {
        impl ByteWriter {
            $(
                #[doc = concat!("A `", stringify!($t), "` in the writer's byte order: a field (module docs).")]
                pub fn $name(&mut self, v: $t) -> &mut Self {
                    let width = std::mem::size_of::<$t>();
                    let bits = <$u>::from_ne_bytes(v.to_ne_bytes());
                    let w = field(width, self.big_endian, u64::from(bits));
                    // The low `width` bytes of the value to write.
                    let low = w.to_le_bytes();
                    let mut b = [0u8; std::mem::size_of::<$t>()];
                    b.copy_from_slice(&low[..width]);
                    let v = <$u>::from_le_bytes(b);
                    if self.big_endian {
                        self.buf.extend_from_slice(&v.to_be_bytes());
                    } else {
                        self.buf.extend_from_slice(&v.to_le_bytes());
                    }
                    self
                }
            )*
        }
    };
}

field!(u32: u32, u32, i32: i32, u32, u64: u64, u64, i64: i64, u64);

macro_rules! put {
    ($($name:ident: $t:ty),*) => {
        impl ByteWriter {
            $(
                #[doc = concat!("A `", stringify!($t), "` in the writer's byte order.")]
                pub fn $name(&mut self, v: $t) -> &mut Self {
                    if self.big_endian {
                        self.buf.extend_from_slice(&v.to_be_bytes());
                    } else {
                        self.buf.extend_from_slice(&v.to_le_bytes());
                    }
                    self
                }
            )*
        }
    };
}

put!(u16: u16, i16: i16, f32: f32, f64: f64);

#[cfg(test)]
mod tests {
    use super::{ByteWriter, Field, with_fields};

    #[test]
    fn fields_are_numbered_reported_and_overridden() {
        let build = || {
            let mut w = ByteWriter::le();
            w.u32(5)
                .u16(7)
                .i64(-1)
                .set_big_endian(true)
                .i32(-2)
                .f32(1.0);
            w.into_inner()
        };
        let (plain, fields) = with_fields(&[], build);
        assert_eq!(plain, build(), "no override, no change");
        assert_eq!(
            fields,
            [
                Field {
                    width: 4,
                    big_endian: false,
                    value: 5
                },
                Field {
                    width: 8,
                    big_endian: false,
                    value: u64::MAX
                },
                Field {
                    width: 4,
                    big_endian: true,
                    value: 0xFFFF_FFFE
                },
            ]
        );
        // The second and third fields replaced; the u16 and the f32 are no fields.
        let (b, _) = with_fields(&[(2, 0x0102_0304), (1, 9)], build);
        let mut want = vec![5, 0, 0, 0, 7, 0, 9, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4];
        want.extend_from_slice(&1.0f32.to_be_bytes());
        assert_eq!(b, want);
        // A value wider than its field keeps its low bytes; outside the hook
        // nothing is recorded or replaced.
        let (b, _) = with_fields(&[(0, 0x1_0000_0006)], build);
        assert_eq!(&b[..4], &[6, 0, 0, 0]);
        assert_eq!(build(), plain);
    }

    #[test]
    fn a_nested_build_numbers_its_own_fields() {
        let (outer, fields) = with_fields(&[(1, 8)], || {
            let mut w = ByteWriter::le();
            w.u32(1);
            let (inner, inner_fields) = with_fields(&[(0, 4)], || {
                let mut v = ByteWriter::le();
                v.u32(2);
                v.into_inner()
            });
            assert_eq!(inner, [4, 0, 0, 0]);
            assert_eq!(inner_fields.len(), 1);
            w.u32(3);
            w.into_inner()
        });
        assert_eq!(outer, [1, 0, 0, 0, 8, 0, 0, 0]);
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn order_and_align() {
        let mut w = ByteWriter::be();
        w.u8(1)
            .align(4)
            .u32(0x0102_0304)
            .set_big_endian(false)
            .u16(0x0506)
            .cstr(b"ab");
        assert_eq!(
            w.into_inner(),
            [1, 0, 0, 0, 1, 2, 3, 4, 6, 5, b'a', b'b', 0]
        );
    }
}
