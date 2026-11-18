use super::{
    LOWER_EXCEPTIONS, ND_ZEROS, RUST_UNICODE_VERSION, SIGMA_CASE_IGNORABLE, SIGMA_CASED,
    is_case_ignorable, is_sigma_cased, lower_exception, nd_digit,
};

#[test]
fn ascii_digits_are_seeded() {
    assert_eq!(nd_digit('0'), Some(0));
    assert_eq!(nd_digit('9'), Some(9));
    assert_eq!(nd_digit('/'), None);
    assert_eq!(nd_digit(':'), None);
    assert_eq!(nd_digit('a'), None);
    assert_eq!(lower_exception('A'), None);
}

/// Python 3.11's `Nd` set is 66 runs of ten (the generator asserts that
/// every `Nd` character is its run's zero plus its digit). Here: the runs
/// are sorted and disjoint, every one of their 660 code points is numeric
/// to Rust too and reads as its digit, and every other code point reads
/// as no digit.
#[test]
fn the_full_nd_set_reads_as_its_digits_and_nothing_else_does() {
    assert_eq!(ND_ZEROS.len(), 66);
    assert!(ND_ZEROS.windows(2).all(|w| w[0] + 10 <= w[1]));
    let mut digits = 0;
    for cp in 0..=0x10_FFFF_u32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let run = ND_ZEROS.iter().find(|&&z| (z..z + 10).contains(&cp));
        match run {
            Some(&zero) => {
                digits += 1;
                assert!(c.is_numeric(), "U+{cp:04X}");
                assert_eq!(nd_digit(c), u8::try_from(cp - zero).ok(), "U+{cp:04X}");
            }
            None => assert_eq!(nd_digit(c), None, "U+{cp:04X}"),
        }
    }
    assert_eq!(digits, 660);
}

#[test]
fn non_ascii_digits_python_knows_read_and_newer_ones_do_not() {
    // ARABIC-INDIC DIGIT SEVEN, FULLWIDTH DIGIT THREE, MATHEMATICAL
    // MONOSPACE DIGIT NINE (the last of the five adjacent runs at U+1D7CE).
    assert_eq!(nd_digit('\u{0667}'), Some(7));
    assert_eq!(nd_digit('\u{FF13}'), Some(3));
    assert_eq!(nd_digit('\u{1D7FF}'), Some(9));
    // Garay digit zero (Unicode 16): a digit to Rust, not to Python 3.11.
    assert!('\u{10D40}'.is_numeric());
    assert_eq!(nd_digit('\u{10D40}'), None);
}

/// Each exception is a code point where Rust's `char::to_lowercase`
/// gives something other than Python's answer (the generator lists every
/// such code point; this checks Rust's side of each).
#[test]
fn every_lower_exception_differs_from_rust() {
    assert_eq!(LOWER_EXCEPTIONS.len(), 55);
    assert!(LOWER_EXCEPTIONS.windows(2).all(|w| w[0].0 < w[1].0));
    for &(cp, python) in LOWER_EXCEPTIONS {
        let c = char::from_u32(cp).unwrap();
        let rust: String = c.to_lowercase().collect();
        assert_ne!(rust, python, "U+{cp:04X}");
        assert_eq!(lower_exception(c), Some(python), "U+{cp:04X}");
    }
    // LATIN CAPITAL LETTER RAMS HORN (Unicode 16) lowers to U+0264 in
    // Rust; Python 3.11 does not know it and leaves it.
    assert_eq!(lower_exception('\u{A7CB}'), Some("\u{A7CB}"));
    assert_eq!('\u{A7CB}'.to_lowercase().next(), Some('\u{0264}'));
}

/// The two final-sigma classes are sorted, disjoint runs; ASCII letters
/// and the capital sigma are cased, the full stop and a combining acute
/// are case-ignorable, and characters Unicode assigned after 14.0 are
/// neither, though Rust's newer tables give them a class.
#[test]
fn the_sigma_classes_are_unicode_14s() {
    for t in [SIGMA_CASED, SIGMA_CASE_IGNORABLE] {
        assert!(t.iter().all(|&(a, b)| a <= b));
        assert!(t.windows(2).all(|w| w[0].1 + 1 < w[1].0));
    }
    for c in ['A', 'z', '\u{3A3}', '\u{1F88}'] {
        assert!(is_sigma_cased(c) && !is_case_ignorable(c), "{c:?}");
    }
    for c in ['.', '\'', '\u{301}', '\u{2B0}'] {
        assert!(is_case_ignorable(c) && !is_sigma_cased(c), "{c:?}");
    }
    for c in [' ', '0', '\u{5D0}'] {
        assert!(!is_case_ignorable(c) && !is_sigma_cased(c), "{c:?}");
    }
    // LATIN CAPITAL LETTER RAMS HORN (Unicode 16) and MODIFIER LETTER
    // CYRILLIC SMALL A (Unicode 15): Rust lowers a sigma after the first
    // as final and skips the second, Python 3.11 does neither.
    for c in ['\u{A7CB}', '\u{1E030}'] {
        assert!(!is_case_ignorable(c) && !is_sigma_cased(c), "{c:?}");
    }
    assert!("\u{A7CB}\u{3A3}".to_lowercase().ends_with('\u{3C2}'));
    assert!("A\u{1E030}\u{3A3}".to_lowercase().ends_with('\u{3C2}'));
}

#[test]
fn the_exceptions_were_generated_for_this_toolchain() {
    assert_eq!(
        char::UNICODE_VERSION,
        RUST_UNICODE_VERSION,
        "Rust's Unicode moved: run tools/gen/gen_tables.py"
    );
}
