//! Synthetic encoder inputs: modes, alpha, valid and refused dimensions.
use super::Seeds;
#[allow(clippy::cast_possible_truncation)]
pub(super) fn add(s: &mut Seeds) -> std::io::Result<()> {
    for (width, height) in [(0u32, 0u32), (1, 1), (3, 5), (16384, 0), (u32::MAX, 0)] {
        for (mode, channels) in [1, 2, 3, 4].into_iter().enumerate() {
            for quality in [0, 80, 95, 100, 255] {
                let mut bytes = Vec::new();
                bytes.extend_from_slice(&width.to_le_bytes());
                bytes.extend_from_slice(&height.to_le_bytes());
                bytes.extend_from_slice(&[mode as u8, quality, 4, 0]);
                let len = width as usize * height as usize * channels;
                bytes.extend((0..len).map(|i| (i * 73) as u8));
                s.add(
                    "webp",
                    &format!("{width}-{height}-{mode}-{quality}"),
                    &bytes,
                )?;
            }
        }
    }
    Ok(())
}
