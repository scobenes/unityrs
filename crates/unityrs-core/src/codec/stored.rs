// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/CompressionHelper.py. See THIRD_PARTY_LICENSES.
//! Compression type 0: [`Stored`], the stored bytes.

use bytes::Bytes;

use super::{BlockCodec, Decoded};
use crate::budget::DecodeBuf;
use crate::error::Error;

/// Compression type 0: the stored bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stored;

const _: () = crate::send_sync::<Stored>();

impl BlockCodec for Stored {
    fn decode(&self, input: &Bytes, _: u32, _: &mut DecodeBuf) -> Result<Decoded, Error> {
        // UnityPy helpers/CompressionHelper.py:239: `lambda cd, _ucs: cd`.
        Ok(Decoded::Input(0..input.len()))
    }
}
