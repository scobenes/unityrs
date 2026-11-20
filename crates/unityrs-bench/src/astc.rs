//! ASTC tiles (plan section 2): no ISE writer and no random ASTC bytes, only
//! the oracle-checked blocks recorded in unityrs-texture's
//! `tests/fixtures/astc.json` (written by `tools/fixtures/texture_astc.py`
//! with astc-encoder-py 0.1.12): the blocks of the three images astcenc's
//! encoder made at the footprint (`ramp`, `opaque`, `tiles`) and of its
//! `crafted` image, each kept only when the unityrs LDR decoder's error
//! status says it is no error block.
//!
//! The pool is grouped by [`Class`], and a texture's blocks follow
//! [`PATTERN`], 16 slots of stated proportions: 8 one-partition blocks, 4
//! two-partition, 2 three-partition, 1 four-partition and 1 void extent.
//! Each slot takes its class's next block, cycling. Every texture here
//! holds a multiple of 16 blocks, so the mix is exact at both scales.

use serde_json::Value;

/// The recorded cases, read at build time from the texture crate's tests.
const FIXTURE: &str = include_str!("../../unityrs-texture/tests/fixtures/astc.json");

/// The cases whose blocks the pool takes, by their id's last part.
pub const SOURCES: [&str; 4] = ["ramp", "opaque", "tiles", "crafted"];

/// A block's kind, from its header bits alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// One partition.
    P1,
    /// Two partitions.
    P2,
    /// Three partitions.
    P3,
    /// Four partitions.
    P4,
    /// A void-extent (constant colour) block.
    Void,
}

impl Class {
    /// Every class, in [`Class::name`] order.
    pub const ALL: [Self; 5] = [Self::P1, Self::P2, Self::P3, Self::P4, Self::Void];

    /// The manifest's name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::P1 => "partitions:1",
            Self::P2 => "partitions:2",
            Self::P3 => "partitions:3",
            Self::P4 => "partitions:4",
            Self::Void => "void",
        }
    }

    /// The class of `block`: void extent when its low nine mode bits are
    /// `0x1fc`, else by its partition count (bits 11 and 12, plus one).
    #[must_use]
    pub fn of(block: &[u8; 16]) -> Self {
        let head = u16::from_le_bytes([block[0], block[1]]);
        if head & 0x1ff == 0x1fc {
            return Self::Void;
        }
        match (head >> 11) & 3 {
            0 => Self::P1,
            1 => Self::P2,
            2 => Self::P3,
            _ => Self::P4,
        }
    }
}

/// The 16-slot pattern a texture's blocks follow.
pub const PATTERN: [Class; 16] = {
    use Class::{P1, P2, P3, P4, Void};
    [
        P1, P2, P1, P3, P1, P2, P1, Void, P1, P2, P1, P4, P1, P2, P1, P3,
    ]
};

/// How many of [`PATTERN`]'s slots `class` holds.
#[must_use]
pub fn share(class: Class) -> usize {
    PATTERN.iter().filter(|c| **c == class).count()
}

fn hex(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect()
}

/// Every recorded block of the [`SOURCES`] cases at footprint `bw` x
/// `bh`, in case order, error blocks included.
pub fn recorded(bw: u8, bh: u8) -> Result<Vec<[u8; 16]>, String> {
    let doc: Value = serde_json::from_str(FIXTURE).map_err(|e| format!("astc.json: {e}"))?;
    let cases = doc
        .get("footprint_cases")
        .and_then(Value::as_array)
        .ok_or("astc.json: no footprint_cases")?;
    let mut out = Vec::new();
    for source in SOURCES {
        let id = format!("astc-{bw}x{bh}-{source}");
        let case = cases
            .iter()
            .find(|c| c.get("id").and_then(Value::as_str) == Some(id.as_str()))
            .ok_or_else(|| format!("astc.json: no case {id}"))?;
        let data = case
            .pointer("/data/hex")
            .and_then(Value::as_str)
            .and_then(hex)
            .ok_or_else(|| format!("astc.json: {id} has no hex data"))?;
        out.extend(data.as_chunks::<16>().0.iter().copied());
    }
    Ok(out)
}

/// The pool at footprint `bw` x `bh`: the [`recorded`] blocks the decoder
/// reads as no error block, each once, grouped by [`Class::ALL`]'s order.
pub fn pool(bw: u8, bh: u8) -> Result<Vec<Vec<[u8; 16]>>, String> {
    let blocks = recorded(bw, bh)?;
    let flat: Vec<u8> = blocks.iter().flatten().copied().collect();
    let errors = unityrs_texture::astc_ldr_error_blocks(bw, bh, &flat)
        .ok_or_else(|| format!("no ASTC footprint {bw}x{bh}"))?;
    let mut out = vec![Vec::new(); Class::ALL.len()];
    for (block, error) in blocks.iter().zip(errors) {
        let at = Class::ALL
            .iter()
            .position(|c| *c == Class::of(block))
            .unwrap_or(0);
        if let Some(group) = out.get_mut(at)
            && !error
            && !group.contains(block)
        {
            group.push(*block);
        }
    }
    if let Some(empty) = Class::ALL.iter().zip(&out).find(|(_, g)| g.is_empty()) {
        return Err(format!("ASTC {bw}x{bh}: no legal {} block", empty.0.name()));
    }
    Ok(out)
}

/// `count` blocks at footprint `bw` x `bh`, block `i` from [`PATTERN`]'s
/// slot `i % 16`, each class's blocks taken in turn.
pub fn tile(bw: u8, bh: u8, count: usize) -> Result<Vec<u8>, String> {
    let pool = pool(bw, bh)?;
    let mut next = [0usize; 5];
    let mut out = Vec::with_capacity(count * 16);
    for i in 0..count {
        let class = PATTERN[i % PATTERN.len()];
        let at = Class::ALL.iter().position(|c| *c == class).unwrap_or(0);
        let (Some(group), Some(n)) = (pool.get(at), next.get_mut(at)) else {
            continue;
        };
        if let Some(block) = group.get(*n % group.len()) {
            out.extend_from_slice(block);
        }
        *n += 1;
    }
    Ok(out)
}
