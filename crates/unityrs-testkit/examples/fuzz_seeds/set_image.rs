//! Every mode and `TextureFormat`, including invalid values, sizes and gates.
pub(super) fn write(seeds: &mut super::Seeds) -> std::io::Result<()> {
    for format in 0..=83i32 {
        for mode in 0..23u8 {
            for (width, height) in [(0, 0u32), (0, 5), (1, 1), (7, 9), (35, 19), (u32::MAX, 1)] {
                let mut input = Vec::new();
                input.extend_from_slice(&width.to_le_bytes());
                input.extend_from_slice(&height.to_le_bytes());
                input.extend_from_slice(&format.to_le_bytes());
                let mips: i32 = if mode % 3 == 2 { 1 } else { 4 };
                input.extend_from_slice(&mips.to_le_bytes());
                input.extend_from_slice(&[
                    mode,
                    mode % 3,
                    0,
                    mode % 4,
                    mode % 4,
                    2,
                    73,
                    0,
                    255,
                    128,
                ]);
                seeds.add(
                    "set_image",
                    &format!("f{format}-m{mode}-{width}x{height}"),
                    &input,
                )?;
            }
        }
    }
    for exponent in [1, 3, 31, 32, 255u8] {
        for mode in [6, 7, 10u8] {
            let mut input = Vec::new();
            input.extend_from_slice(&17u32.to_le_bytes());
            input.extend_from_slice(&19u32.to_le_bytes());
            input.extend_from_slice(&4i32.to_le_bytes());
            input.extend_from_slice(&1i32.to_le_bytes());
            input.extend_from_slice(&[mode, 2, 0, exponent, 3, 2, 73, 0, 255, 128]);
            seeds.add(
                "set_image",
                &format!("switch-m{mode}-exp{exponent}"),
                &input,
            )?;
        }
    }
    for n in 0..22 {
        seeds.add("set_image", &format!("header{n}"), &vec![0; n])?;
    }
    Ok(())
}
