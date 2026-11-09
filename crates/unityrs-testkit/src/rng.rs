//! A tiny deterministic generator (`SplitMix64`) for "no panic on garbage"
//! tests: every package mutates the builders' output with it, seeded, so a
//! failure reproduces from its seed. No dependency, no global state.

/// `SplitMix64`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    /// A generator from a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % u64::try_from(n).unwrap_or(u64::MAX)).unwrap_or(0)
    }

    /// Flips `count` random bits of `data`.
    pub fn flip_bits(&mut self, data: &mut [u8], count: usize) {
        for _ in 0..count {
            let i = self.below(data.len());
            let bit = self.below(8);
            if let Some(b) = data.get_mut(i) {
                *b ^= 1 << bit;
            }
        }
    }

    /// `data` cut at a random length.
    pub fn truncate(&mut self, data: &mut Vec<u8>) {
        let n = self.below(data.len().saturating_add(1));
        data.truncate(n);
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn deterministic() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        assert_eq!(a.next_u64(), b.next_u64());
        let mut v = vec![0u8; 16];
        a.flip_bits(&mut v, 5);
        let mut w = vec![0u8; 16];
        b.flip_bits(&mut w, 5);
        assert_eq!(v, w);
        assert_eq!(Rng::new(1).below(0), 0);
    }
}
