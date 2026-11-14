// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/ArchiveStorageManager.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/ArchiveStorageDecryptor.cpp. See THIRD_PARTY_LICENSES.
// Ported from RazTools/Studio (MIT): UnityCN.cs via UnityPyBoost. See LICENSE-RAZTOOLS.
// Ported from Razmoth/PGRStudio (MIT): PGR.cs via UnityPy. See LICENSE-RAZMOTH.
//! UnityCN's AES vectors and in-place LZ4-token walk.
//!
//! Tables and AES state live on the stack, or inline in the bundle handle
//! whose charge covers them. Decryption allocates nothing; the caller pays
//! for its mutable block copy. Each byte is visited at most once. Boost's
//! implicit trailing zero exists here as one local byte, never as a write
//! beyond the slice. Reading after it is the documented Tier U refusal.

use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit};

use crate::codec::BlockDecryptor;
use crate::stream::Reader;
use crate::{Error, Unsupported};

/// The per-bundle nibble tables. Debug deliberately omits their contents.
#[derive(Clone)]
pub struct UnityCnDecryptor {
    index: [u8; 16],
    substitute: [u8; 16],
}

impl std::fmt::Debug for UnityCnDecryptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnityCnDecryptor").finish_non_exhaustive()
    }
}

const _: () = crate::send_sync::<UnityCnDecryptor>();

impl UnityCnDecryptor {
    /// Reads the u32 and both vectors before checking the load-local key.
    /// UnityPy 1.25.2 helpers/ArchiveStorageManager.py:67-94.
    pub fn read(r: &mut Reader<'_>, key: Option<&[u8; 16]>) -> Result<Self, Error> {
        r.read_u32()?;
        let (data, vector) = read_vector(r)?;
        let (signature, signature_vector) = read_vector(r)?;
        let key = key.ok_or(Error::UnityCnKeyMissing)?;
        let (decoded, len) = decrypt_key(signature_vector, signature, key)?;
        if len != 16 || &decoded != b"#$unity3dchina!@" {
            return Err(Error::UnityCnSignature);
        }
        let (decoded, len) = decrypt_key(vector, data, key)?;
        if len != 16 {
            return Err(Error::UnityCnSignature);
        }
        let mut nibbles = [0; 32];
        for (pair, byte) in nibbles.as_chunks_mut::<2>().0.iter_mut().zip(decoded) {
            pair.copy_from_slice(&[byte >> 4, byte & 15]);
        }
        let mut index = [0; 16];
        let mut substitute = [0; 16];
        for (to, from) in index.iter_mut().zip(nibbles.iter()) {
            *to = *from;
        }
        for (n, to) in substitute.iter_mut().enumerate() {
            *to = nibbles.get(16 + (n % 4) * 4 + n / 4).copied().unwrap_or(0);
        }
        Ok(Self { index, substitute })
    }

    fn byte(&self, byte: u8, index: usize) -> u8 {
        let get = |table: &[u8; 16], i| table.get(i).copied().unwrap_or(0);
        let b = get(&self.substitute, ((index >> 2) & 3) + 4)
            + get(&self.substitute, index & 3)
            + get(&self.substitute, ((index >> 4) & 3) + 8)
            + get(&self.substitute, ((index >> 6) & 3) + 12);
        (get(&self.index, usize::from(byte & 15)).wrapping_sub(b) & 15)
            | (get(&self.index, usize::from(byte >> 4)).wrapping_sub(b) << 4)
    }

    fn next(&self, bytes: &mut [u8], at: &mut usize, index: &mut usize) -> Result<u8, Error> {
        let value = if let Some(byte) = bytes.get_mut(*at) {
            *byte = self.byte(*byte, *index);
            *byte
        } else if *at == bytes.len() {
            self.byte(0, *index)
        } else {
            return Err(Error::Unsupported(Unsupported::UnityCnWalk));
        };
        *at = at.saturating_add(1);
        *index = index.wrapping_add(1);
        Ok(value)
    }
}

impl BlockDecryptor for UnityCnDecryptor {
    /// UnityPyBoost/ArchiveStorageDecryptor.cpp:6-53 and
    /// UnityPyBoost/ArchiveStorageDecryptor.cpp:55-89, without its out-of-bounds accesses.
    fn decrypt(&self, block: &mut [u8], mut index: usize) -> Result<(), Error> {
        let mut at = 0;
        while at < block.len() {
            let mut sequence_index = index;
            let token = self.next(block, &mut at, &mut sequence_index)?;
            let mut literals = usize::from(token >> 4);
            if literals == 15 {
                loop {
                    let b = self.next(block, &mut at, &mut sequence_index)?;
                    literals = literals.saturating_add(usize::from(b));
                    if b != 255 {
                        break;
                    }
                }
            }
            // A literal jump beyond the end reads nothing there (Tier A).
            at = at.saturating_add(literals);
            if at < block.len() {
                self.next(block, &mut at, &mut sequence_index)?;
                self.next(block, &mut at, &mut sequence_index)?;
                if token & 15 == 15 {
                    loop {
                        if self.next(block, &mut at, &mut sequence_index)? != 255 {
                            break;
                        }
                    }
                }
            }
            index = index.wrapping_add(1);
        }
        Ok(())
    }
}

/// helpers/ArchiveStorageManager.py:25-30: two short reads and a seek.
fn read_vector<'a>(r: &mut Reader<'a>) -> Result<(&'a [u8], &'a [u8]), Error> {
    let data = r.read_bytes_short(16);
    let key = r.read_bytes_short(16);
    r.set_position(r.position() + 1)?;
    Ok((data, key))
}

/// AES **encryption**, then XOR, per helpers/ArchiveStorageManager.py:33-37.
fn decrypt_key(vector: &[u8], data: &[u8], key: &[u8; 16]) -> Result<([u8; 16], usize), Error> {
    // ECB accepts zero blocks. Preserve the effective length of Python's
    // zip result alongside the fixed stack buffer, including empty input.
    if vector.is_empty() {
        return Ok(([0; 16], 0));
    }
    let vector = <[u8; 16]>::try_from(vector).map_err(|_| Error::UnityCnVectorLength {
        length: vector.len(),
    })?;
    let cipher = Aes128::new(&(*key).into());
    let mut block = vector.into();
    cipher.encrypt_block(&mut block);
    let mut out = [0; 16];
    for ((to, from), mask) in out.iter_mut().zip(data).zip(block) {
        *to = from ^ mask;
    }
    Ok((out, data.len().min(16)))
}

#[cfg(test)]
mod tests;
