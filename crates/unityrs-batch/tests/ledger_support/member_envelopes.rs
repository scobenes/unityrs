//! Arithmetic edges for the B6c lifetime proof; also runs with rustc --test.
#[cfg(test)]
mod tests {
    fn round(n: u64, b: u64) -> u64 {
        n.div_ceil(b) * b
    }
    #[test]
    fn archive_and_list_rounding_share_only_the_stated_allowances() {
        for raw in 1..=1024_u64 {
            for encoded in [raw, 2 * raw] {
                for b in (1..=2 * encoded + 1024).chain([5003, 65_536, 1_000_003]) {
                    let a = round(encoded + 512, b);
                    // Row fixed <=192; metadata fixed <=67, framed list
                    // header16 and per-list rounding <=b-1. The extra encoded
                    // key bytes are covered by that archive's counted member.
                    assert!(encoded + raw + 192 + 67 + 16 + b - 1 <= 2 * a + encoded);
                }
            }
        }
    }
    #[test]
    #[allow(clippy::many_single_char_names)] // Match the written ledger's algebra.
    fn every_b6c_sort_and_direct_join_envelope_fits_six_s() {
        for b in (1..=8192_u64).chain([65_536, 1_000_003, 1 << 30]) {
            let k = if b < 4096 && 4096 % b != 0 {
                2
            } else {
                b.div_ceil(4096).max(1)
            };
            for n_candidates in [1, 2, 3, 256, 100_000, 1_000_000] {
                for width in [256, 6277, 32_768] {
                    let j = n_candidates * width;
                    let f = round(j + 256, 4096);
                    let c = n_candidates * 4096;
                    for m in [0, 160, 1 << 20, 1 << 24, 1 << 28, 1 << 40] {
                        let s = k * (f + c + m);
                        let runs = if m == 0 { 0 } else { 1 + m / (16 << 20) };
                        let member_sort =
                            3 * j + 64 * n_candidates + 3 * m + (2 * runs + 6) * b + 94 * runs + 90;
                        let rows = 6 * j + 64 * n_candidates + 6 * (b + 15);
                        let handoff = 3 * j + 64 * n_candidates + 4 * m + 6 * b + 90;
                        assert!(
                            member_sort <= 6 * s,
                            "sort b={b} N={n_candidates} J={j} M={m}"
                        );
                        assert!(rows <= 6 * s, "rows b={b} N={n_candidates} J={j}");
                        assert!(
                            handoff <= 6 * s,
                            "handoff b={b} N={n_candidates} J={j} M={m}"
                        );
                    }
                }
            }
        }
    }
}
