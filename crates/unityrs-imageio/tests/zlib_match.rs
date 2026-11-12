//! Byte equality with pristine zlib 1.2.13 at Pillow's settings. Run with
//! `cargo test -p unityrs-imageio --features test-reference --test zlib_match`.
//! These catch changed candidate selection, mismatch offsets, `MAX_MATCH`
//! truncation, window sliding and dependence on streaming boundaries.
#![cfg(feature = "test-reference")]

use unityrs_imageio::test_reference::assert_deflate_equal;

fn random(len: usize, mut seed: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed.to_le_bytes()[0]
        })
        .collect()
}

fn check(data: &[u8]) {
    assert_deflate_equal(data, 65536, 65536);
    // Input and output edges inside matches and across window slides.
    assert_deflate_equal(data, 257, 509);
}

#[test]
fn random_zero_and_periodic_at_match_and_window_edges() {
    for len in [
        0, 1, 2, 3, 7, 8, 9, 255, 256, 257, 258, 259, 261, 262, 263, 32767, 32768, 32769, 65535,
        65536, 65537, 131_333,
    ] {
        check(&random(len, 0x1234_5678_9abc_def0));
        check(&vec![0; len]);
        for period in [1, 2, 3, 7, 8, 17, 257, 258, 263] {
            let pattern = random(period, 0x1020_3040_5060_7080);
            let data: Vec<_> = pattern.into_iter().cycle().take(len).collect();
            check(&data);
        }
    }
}

#[test]
fn short_and_long_runs_with_mismatches_at_every_match_offset() {
    let mut data = Vec::new();
    for run in 1..=520 {
        data.extend(std::iter::repeat_n(0x61, run));
        data.push(0x62);
    }
    // Equal prefixes and competing longer matches exercise match_start.
    for len in 3..=258 {
        let pattern = random(300, 0xfeed_face);
        data.extend_from_slice(&pattern);
        data.extend_from_slice(&pattern[..len]);
        data.push(pattern[len] ^ 0xff);
    }
    check(&data);
    assert_deflate_equal(&data, 1, 1);
}

#[test]
fn multi_megabyte_streams_slide_the_window_repeatedly() {
    check(&random(3 * 1024 * 1024 + 263, 0xcafe_f00d));
    check(&vec![0; 3 * 1024 * 1024 + 1]);
    let pattern = random(32749, 0xdead_beef);
    let data: Vec<_> = pattern.into_iter().cycle().take(3 * 1024 * 1024).collect();
    check(&data);
}

#[test]
fn png_filtered_synthetic_rows() {
    // All five PNG predictors, each on flat, gradient and noisy RGBA rows.
    // The predictors are built independently of the production filter.
    for width in [1, 17, 257, 1024] {
        for kind in 0..3 {
            let mut previous = vec![0_u8; width * 4];
            let mut data = Vec::new();
            for y in 0_usize..160 {
                let row = match kind {
                    0 => vec![0x7f; width * 4],
                    1 => (0..width * 4).map(|x| (x + y).to_le_bytes()[0]).collect(),
                    _ => random(width * 4, 1 + y as u64),
                };
                let filter = y % 5;
                data.push(u8::try_from(filter).unwrap());
                for x in 0..row.len() {
                    let left = if x >= 4 { row[x - 4] } else { 0 };
                    let up = previous[x];
                    let corner = if x >= 4 { previous[x - 4] } else { 0 };
                    let predictor = match filter {
                        0 => 0,
                        1 => left,
                        2 => up,
                        3 => left.midpoint(up),
                        _ => {
                            let p = i32::from(left) + i32::from(up) - i32::from(corner);
                            let [a, b, c] = [left, up, corner].map(|v| (p - i32::from(v)).abs());
                            if a <= b && a <= c {
                                left
                            } else if b <= c {
                                up
                            } else {
                                corner
                            }
                        }
                    };
                    data.push(row[x].wrapping_sub(predictor));
                }
                previous = row;
            }
            check(&data);
        }
    }
}
