//! An endian-aware byte writer, the builders' only primitive.

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

put!(u16: u16, i16: i16, u32: u32, i32: i32, u64: u64, i64: i64, f32: f32, f64: f64);

#[cfg(test)]
mod tests {
    use super::ByteWriter;

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
