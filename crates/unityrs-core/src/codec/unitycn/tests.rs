//! Vector layout and an AES-128 known-answer test independent of the encryptor.
use super::*;
use crate::stream::{Endian, ReaderKind};

#[test]
fn aes_ecb_encrypts_the_vector_before_xor() {
    let key = std::array::from_fn(|i| u8::try_from(i).unwrap());
    let vector: [u8; 16] = std::array::from_fn(|i| u8::try_from(i * 17).unwrap());
    let encrypted = [
        0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5,
        0x5a,
    ];
    assert_eq!(
        decrypt_key(&vector, &[0; 16], &key).unwrap(),
        (encrypted, 16)
    );
    assert_eq!(
        decrypt_key(&vector, &encrypted, &key).unwrap(),
        ([0; 16], 16)
    );
}

#[test]
fn each_vector_reads_data_key_and_skips_one_byte_for_both_readers() {
    let bytes: Vec<u8> = (0..66).collect();
    for kind in [ReaderKind::Memory, ReaderKind::Stream] {
        let mut r = Reader::with_kind(&bytes, Endian::Big, kind);
        let (data, key) = read_vector(&mut r).unwrap();
        assert_eq!(data, &bytes[..16]);
        assert_eq!(key, &bytes[16..32]);
        assert_eq!(r.position(), 33);
        let (data, key) = read_vector(&mut r).unwrap();
        assert_eq!(data, &bytes[33..49]);
        assert_eq!(key, &bytes[49..65]);
        assert_eq!(r.position(), 66);
    }
}

#[test]
fn empty_ecb_input_reaches_signature_validation_instead_of_length_validation() {
    // Empty AES input is aligned (zero blocks), so a short header with no
    // signature key reaches UnityPy's invalid-signature Exception.
    let header = [0; 53];
    for len in 4..=53 {
        let mut r = Reader::new(&header[..len], Endian::Big);
        assert!(
            matches!(
                UnityCnDecryptor::read(&mut r, Some(&[0; 16])),
                Err(Error::UnityCnSignature)
            ),
            "header length {len}"
        );
    }
}
