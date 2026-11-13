//! Independent contract encoding, not a phase-6 implementation. This module
//! also runs with `rustc --test` when the workspace dependencies are unavailable.

#[must_use]
pub fn key(index: u32, long: bool) -> String {
    if long {
        format!(
            "a{index:03}/{}/{}/{}/{}",
            "x".repeat(255),
            "x".repeat(255),
            "x".repeat(255),
            "x".repeat(251)
        )
    } else {
        format!("a{index:03}")
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Keys {
    Short,
    Long,
    Quotes,
    Backslashes,
    C1,
    Unicode,
}
pub const FAMILIES: [Keys; 6] = [
    Keys::Short,
    Keys::Long,
    Keys::Quotes,
    Keys::Backslashes,
    Keys::C1,
    Keys::Unicode,
];

#[must_use]
pub fn family_key(index: u32, keys: Keys) -> String {
    let fill = match keys {
        Keys::Short => return key(index, false),
        Keys::Long => return key(index, true),
        Keys::Quotes => '\"',
        Keys::Backslashes => '\\',
        Keys::C1 => '\u{85}',
        Keys::Unicode => '界',
    };
    let segment = |bytes| {
        let mut text = String::new();
        for n in 0..bytes / fill.len_utf8() {
            text.push(if matches!(keys, Keys::C1) {
                char::from(0x80 + u8::try_from(n % 32).unwrap())
            } else {
                fill
            });
        }
        text + &"x".repeat(bytes % fill.len_utf8())
    };
    format!(
        "a{index:03}/{}/{}/{}/{}",
        segment(255),
        segment(255),
        segment(255),
        segment(251)
    )
}

/// All fields required by `BATCH_SPEC` 7.3, compact JSON plus newline.
/// No candidates means no included or left-out members; digests/size are null.
#[must_use]
pub fn row(key: &str) -> String {
    let mut escaped = String::new();
    for c in key.chars() {
        match c {
            '\"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            c => escaped.push(c),
        }
    }
    format!(
        "{{\"key\":\"{escaped}\",\"status\":\"empty\",\"reason\":null,\"members\":0,\"left_out\":0,\"md5\":null,\"etag\":null,\"size\":null}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{FAMILIES, Keys, family_key, key, row};

    #[test]
    fn escaped_families_have_bounded_segments_and_exact_row_widths() {
        for keys in FAMILIES {
            for index in 0..256 {
                let k = family_key(index, keys);
                assert!(k.split('/').all(|s| !s.is_empty() && s.len() <= 255));
                let (raw, encoded) = match keys {
                    Keys::Short => (4, 4),
                    Keys::Quotes | Keys::Backslashes => (1_024, 2_040),
                    _ => (1_024, 1_024),
                };
                assert_eq!(k.len(), raw);
                assert_eq!(row(&k).len(), encoded + 102);
            }
        }
    }

    #[test]
    fn maximum_keys_are_valid_lengths_and_pairwise_unrelated() {
        let keys: Vec<_> = (0..256).map(|i| key(i, true)).collect();
        for (i, k) in keys.iter().enumerate() {
            assert_eq!(k.len(), 1024);
            assert!(k.split('/').all(|s| !s.is_empty() && s.len() <= 255));
            for other in keys.iter().skip(i + 1) {
                assert!(k < other);
                assert!(!other.starts_with(&(k.clone() + "/")));
                assert!(!k.starts_with(&(other.clone() + "/")));
            }
        }
    }

    #[test]
    fn mandatory_empty_rows_alone_exceed_six_summary_work_units() {
        for (long, width, bytes, claim) in [
            (false, 106, 27_136, 28_672),
            (true, 1_126, 288_256, 290_816),
        ] {
            let encoded: usize = (0..256)
                .map(|i| {
                    let r = row(&key(i, long));
                    assert_eq!(r.len(), width);
                    r.len()
                })
                .sum();
            assert_eq!(encoded, bytes);
            let rounded = encoded.div_ceil(4096) * 4096;
            assert_eq!(rounded, claim);
            assert!(rounded > 6 * 4096);
            println!("EMPTY_ARCHIVES bytes={encoded} claim={rounded} S=4096");
        }
    }
}

/// The original B6c escaped-key gate, retained under encoded-length `A`.
#[must_use]
pub fn escaped_key(index: u32) -> String {
    format!(
        "{index:03}/{}/{}/{}/{}",
        "\"".repeat(254),
        "\"".repeat(254),
        "\"".repeat(254),
        "\"".repeat(255)
    )
}

#[cfg(test)]
mod encoded_gate {
    use super::{escaped_key, row};
    #[test]
    fn escaped_archive_streams_fit_the_encoded_allowance() {
        let rounded = |n: u64| n.div_ceil(512) * 512;
        let mut rows = 0;
        let mut metadata = 16;
        let mut a = 0;
        for i in 0..256 {
            let key = escaped_key(i);
            let encoded = u64::try_from(row(&key).len() - 102).unwrap();
            rows += encoded + 102;
            metadata += 35 + u64::try_from(key.len()).unwrap();
            a += rounded(encoded + 512);
        }
        let retained = rounded(rows) + rounded(metadata);
        assert_eq!((rows, metadata, a), (548_608, 271_120, 655_360));
        assert_eq!(retained, 820_224);
        assert!(retained <= 6 * 4096 + 2 * a);
    }
}
