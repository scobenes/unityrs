//! Bounded still-image WebP encoding, hostile dimensions and options.
#![no_main]
use libfuzzer_sys::fuzz_target;
use unityrs_imageio::{Buffer, Charger, Image, Mode, WebpOptions, encode_webp, webp_plan};
struct Limited;
impl Charger for Limited {
    type Charge = ();
    type Refusal = ();
    fn charge(&mut self, _: Buffer, requests: &[usize]) -> Result<(), ()> {
        if requests.iter().sum::<usize>() > 128 << 20 {
            Err(())
        } else {
            Ok(())
        }
    }
}
fuzz_target!(|data: &[u8]| {
    if data.len() < 12 {
        return;
    }
    let width = u32::from_le_bytes(data[..4].try_into().unwrap());
    let height = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let _ = webp_plan(width, height);
    let mode = Mode::ALL[data[8] as usize % 4];
    let pixels = &data[12..data.len().min((1 << 20) + 12)];
    if let Ok(image) = Image::new(width, height, mode, pixels) {
        let options = WebpOptions {
            quality: f32::from(data[9]),
            method: data[10],
            exact: data[11] & 1 != 0,
        };
        let _ = encode_webp(&image, options, &mut Limited);
    }
});
