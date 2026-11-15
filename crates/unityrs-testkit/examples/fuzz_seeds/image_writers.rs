//! Synthetic inputs for `fuzz/src/image_writers.rs`'s fixed 13-byte header.
//! Large dimensions use a short repeating source; the harness caps generated
//! pixels at 65,536 and refuses every ICNS allocation before its costly resize.

const FORMATS: [&str; 19] = [
    "bmp", "dib", "tga", "ppm", "pcx", "sgi", "im", "tiff", "dds", "ico", "icns", "eps", "png",
    "gif", "jpeg", "mpo", "pdf", "jpeg2000", "webp",
];
const MODES: [(&str, usize); 5] = [("l", 1), ("la", 2), ("rgb", 3), ("rgba", 4), ("p", 1)];
const SIZES: [(u32, u32); 19] = [
    (0, 0),
    (0, 1),
    (1, 0),
    (1, 1),
    (2, 1),
    (3, 2),
    (15, 1),
    (16, 1),
    (17, 1),
    (255, 1),
    (256, 1),
    (65534, 1),
    (65535, 1),
    (65536, 1),
    (i32::MAX as u32, 1),
    (u32::MAX, 1),
    (u32::MAX, u32::MAX),
    (256, 256),
    (1, 65536),
];

fn input(width: u32, height: u32, controls: [u8; 5], len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(13 + len);
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&controls);
    bytes.extend((0..len).map(|n| n.to_le_bytes()[0].wrapping_mul(73).wrapping_add(0xc0)));
    bytes
}

pub(super) fn write(s: &mut super::Seeds) -> std::io::Result<()> {
    for (format, name) in (0_u8..).zip(FORMATS) {
        for (mode, (mode_name, channels)) in (0_u8..).zip(MODES) {
            // Policy 12 selects raw dimensions like 0, with an explicit palette.
            // Policy 4 selects reduced dimensions like 1, with a palette.
            // Policy 5 selects the side table like 2, with a palette.
            for palette in 0..=usize::from(mode == 4) {
                let policies = if palette == 0 { [0, 1, 2] } else { [12, 4, 5] };
                for (n, (width, height)) in (0_u8..).zip(SIZES) {
                    let pixels = u64::from(width) * u64::from(height);
                    let len = if pixels <= 16 {
                        usize::try_from(pixels).expect("at most 16 pixels") * channels
                    } else {
                        32
                    };
                    let controls = [mode, format, policies[0], n % 4, n % 4];
                    s.add(
                        "image_writers",
                        &format!("{name}-{mode_name}-p{palette}-{width}x{height}"),
                        &input(width, height, controls, len),
                    )?;
                }
                // Exact, short and long raw pixel buffers exercise Image's
                // validation independently of the generated writer image.
                for len in [2 * channels - 1, 2 * channels, 2 * channels + 1] {
                    s.add(
                        "image_writers",
                        &format!("{name}-{mode_name}-p{palette}-pixels{len}"),
                        &input(2, 1, [mode, format, policies[0], 0, 0], len),
                    )?;
                }
                for policy in &policies[1..] {
                    s.add(
                        "image_writers",
                        &format!("{name}-{mode_name}-p{palette}-policy{policy}"),
                        &input(3, 2, [mode, format, *policy, 3, 3], 768),
                    )?;
                }
            }
        }
    }
    // Every truncated header length, including the empty input.
    for len in 0..13 {
        s.add("image_writers", &format!("header{len}"), &vec![0; len])?;
    }
    Ok(())
}
