//! What a run's next blocks declare, read ahead of its decode, so the run's
//! buffer doubles its room up to where they end rather than growing to each
//! block's end in turn ([`crate::budget::DecodeBuf::set_ahead`]).
//!
//! Before block `k` of a run that began at block `first` is decoded, the
//! read-ahead holds the declared sizes of the blocks after `k` that start,
//! by those sizes, below the read's end, at most `k - first` of them: as
//! many as the buffer holds, so the room can double and no further (the
//! run's first block has nothing to double). Each block is read ahead once
//! (the window only moves on), so a run of `n` blocks reads at most `2n`
//! rows ahead, besides the steps its decodes count (`index`'s module docs):
//! rows whose sizes lie (a run of blocks that each declare nothing) cost no
//! more to read than honest ones.

/// A run's read-ahead (module docs).
#[derive(Debug)]
pub(super) struct Ahead {
    /// The run's first block.
    first: usize,
    /// The read's end, in joined offsets: no block that starts there or
    /// past it is read ahead.
    hi: u64,
    /// The window's end: blocks before it were read ahead.
    to: usize,
    /// What the window's blocks declare.
    bytes: u64,
}

impl Ahead {
    /// The read-ahead of a run from block `first`, for a read that ends at
    /// joined offset `hi`.
    pub(super) fn new(first: usize, hi: u64) -> Self {
        Self {
            first,
            hi,
            to: 0,
            bytes: 0,
        }
    }

    /// What the blocks after block `k` (which starts at joined offset `at`)
    /// declare, as far as it reads ahead (module docs), before block `k` is
    /// decoded; `declared(i)` is block `i`'s declared size, `None` past the
    /// last block. `k` goes up by one from call to call.
    pub(super) fn before(
        &mut self,
        k: usize,
        at: u64,
        declared: impl Fn(usize) -> Option<u64>,
    ) -> u64 {
        let own = declared(k).unwrap_or(0);
        if self.to > k {
            // Block `k` was read ahead: it is decoded now, not ahead.
            self.bytes = self.bytes.saturating_sub(own);
        } else {
            (self.to, self.bytes) = (k + 1, 0);
        }
        let most = k.saturating_sub(self.first);
        while self.to - (k + 1) < most
            && at.saturating_add(own).saturating_add(self.bytes) < self.hi
            && let Some(d) = declared(self.to)
        {
            self.bytes = self.bytes.saturating_add(d);
            self.to += 1;
        }
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::Ahead;

    const K: u64 = 64 << 10;

    /// Twelve blocks of 64 KiB read from block 0 to their end: before block
    /// `k` the read-ahead holds `min(k, 11 - k)` blocks, so a buffer that
    /// holds `k` blocks may double; before block 0 it holds none.
    #[test]
    fn it_holds_as_many_blocks_as_the_run_does() {
        let sizes = [K; 12];
        let declared = |i: usize| sizes.get(i).copied();
        let mut a = Ahead::new(0, 12 * K);
        let got: Vec<u64> = (0..12)
            .map(|k| a.before(k, k as u64 * K, declared) / K)
            .collect();
        assert_eq!(got, [0, 1, 2, 3, 4, 5, 5, 4, 3, 2, 1, 0]);
    }

    /// Blocks that declare nothing are read ahead no further than honest
    /// ones: before block `k` of a run from block `first` the window ends at
    /// `2k - first + 1`, so a run of `n` blocks reads at most `2n` rows ahead.
    #[test]
    fn rows_that_declare_nothing_are_read_ahead_no_further() {
        let sizes = [0_u64; 1000];
        let declared = |i: usize| sizes.get(i).copied();
        let mut a = Ahead::new(3, u64::MAX);
        for k in 3..100 {
            assert_eq!(a.before(k, 0, declared), 0);
            assert_eq!(a.to, 2 * k - 3 + 1, "{k}");
        }
    }

    /// No block that starts at the read's end, by the declared sizes, is
    /// read ahead: a read of the first 8 of 16 blocks holds, before its
    /// last block, nothing, and before block 6 only block 7.
    #[test]
    fn nothing_past_the_reads_end_is_read_ahead() {
        let sizes = [K; 16];
        let declared = |i: usize| sizes.get(i).copied();
        let mut a = Ahead::new(0, 8 * K);
        let got: Vec<u64> = (1..8)
            .map(|k| a.before(k, k as u64 * K, declared) / K)
            .collect();
        assert_eq!(got, [1, 2, 3, 3, 2, 1, 0]);
    }
}
