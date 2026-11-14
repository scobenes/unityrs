//! Exercise arbitrary nibble tables and token walks, including the virtual zero.
#![no_main]
use libfuzzer_sys::fuzz_target;
use unityrs_core::LimitKind;
use unityrs_core::budget::DecodeBuf;
use unityrs_core::codec::{BlockDecryptor, unitycn::UnityCnDecryptor};
use unityrs_core::stream::{Endian, Reader};

fuzz_target!(|data: &[u8]| {
    if data.len() > unityrs_fuzz::MEMORY as usize {
        return;
    }
    // Synthetic header with a valid signature. Mutating only its table vector
    // reaches every table while preserving signature validation.
    let mut header = HEADER;
    for (to, from) in header.iter_mut().skip(4).take(16).zip(data) {
        *to ^= from;
    }
    let mut reader = Reader::new(&header, Endian::Big);
    if let Ok(dec) = UnityCnDecryptor::read(&mut reader, Some(b"synthetic-key-01")) {
        let ctx = unityrs_fuzz::context();
        let meter = unityrs_fuzz::meter(&ctx);
        let mut out = match DecodeBuf::new(&meter, 0, unityrs_fuzz::MEMORY, LimitKind::InputMemory)
        {
            Ok(out) => out,
            Err(_) => return,
        };
        if let Ok(block) = out.grow_zeroed(data.len()) {
            block.copy_from_slice(data);
            let mut index = [0; 8];
            for (to, from) in index.iter_mut().zip(data) {
                *to = *from;
            }
            let _ = dec.decrypt(block, u64::from_le_bytes(index) as usize);
        }
    }
});

const HEADER: [u8; 70] = [
    16, 32, 48, 64, 112, 221, 191, 237, 173, 201, 144, 121, 16, 74, 24, 161, 107, 185, 159, 218,
    50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 205, 13, 42, 85, 110, 150, 140,
    195, 147, 40, 66, 139, 104, 95, 93, 106, 114, 87, 87, 87, 87, 87, 87, 87, 87, 87, 87, 87, 87,
    87, 87, 87, 87, 168,
];
