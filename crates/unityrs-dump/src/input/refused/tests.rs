use std::sync::Arc;

use unityrs_core::budget::{BudgetExceeded, Reservation};
use unityrs_core::provider::ProviderError;
use unityrs_core::walk::{ReadError, ReadErrorKind};
use unityrs_core::{LimitKind, RawStr};

use super::*;
use crate::record::NOTE_MAX;

fn limit() -> LimitError {
    LimitError {
        kind: LimitKind::ZeroSizeElements,
        value: 18_446_744_073_709_551_615,
        max: 18_446_744_073_709_551_614,
    }
}

fn budget() -> BudgetExceeded {
    BudgetExceeded {
        requested: 64,
        what: Reservation::Input,
    }
}

/// `inner` failed in an entry named `n` x `len` inside `levels` bundles.
fn in_entries(inner: Error, len: usize, levels: usize) -> Error {
    (0..levels).fold(inner, |e, i| Error::Entry {
        name: RawStr::from(("n".repeat(len) + &i.to_string()).as_str()),
        source: Arc::new(e),
    })
}

fn read_limit(detail: ReadDetail) -> ParseError {
    ParseError::Read(ReadError {
        kind: ReadErrorKind::Other,
        at: 0,
        detail,
    })
}

fn read_tree_limit() -> ParseError {
    read_limit(ReadDetail::Tree(TreeError::Limit(limit())))
}

/// The refusal at the root of each error that wraps one, however it is
/// wrapped, through every name the entries add.
fn wrapped(inner: &Error) -> Vec<Error> {
    let mut out = vec![inner.clone()];
    for (len, levels) in [(1, 1), (300, 1), (3, 40), (5000, 3)] {
        out.push(in_entries(inner.clone(), len, levels));
        let load = Error::Deref(Arc::new(DerefError::Load(in_entries(
            inner.clone(),
            len,
            levels,
        ))));
        out.push(in_entries(load, len, 1));
    }
    out
}

/// Every limit an error carries is in its note whole, after the text's
/// first characters when the names before it are longer than the note, and
/// the note is never longer than [`NOTE_MAX`]; a note that fits is the
/// whole text.
#[test]
fn a_limit_refusal_is_kept_whole_in_its_note_behind_any_names() {
    let tail = limit().to_string();
    let roots = [
        Error::Limit(limit()),
        Error::Detect(DetectError::InflateLimit(limit())),
        Error::Detect(DetectError::Work(limit())),
        Error::Tree(TreeError::Limit(limit())),
        Error::Tree(TreeError::Charge(ChargeError::Limit(limit()))),
        Error::AssetBundle(Arc::new(read_limit(ReadDetail::Limit(limit())))),
        Error::AssetBundle(Arc::new(read_limit(ReadDetail::Charge(
            ChargeError::Limit(limit()),
        )))),
        Error::AssetBundle(Arc::new(read_tree_limit())),
    ];
    let mut cut = 0;
    for root in roots {
        for e in wrapped(&root) {
            let n = note(&e);
            let text = e.to_string();
            assert!(n.chars().count() <= NOTE_MAX, "{n}");
            assert!(n.ends_with(&tail), "{text:.80}: {n}");
            if text.chars().count() <= NOTE_MAX {
                assert_eq!(n, text);
            } else {
                cut += 1;
                let head: String = text.chars().take(NOTE_MAX - 3 - tail.len()).collect();
                assert_eq!(n, format!("{head}...{tail}"));
            }
            assert!(memory_refused(&e).is_none());
        }
    }
    assert!(cut >= 8 * 6, "{cut} notes cut");
    // An object's read and its resave, the dereference `m_Script` names
    // failing in a long-named entry among them.
    let deref = |len| {
        ParseError::Deref(Arc::new(DerefError::Load(in_entries(
            Error::Limit(limit()),
            len,
            2,
        ))))
    };
    let provider = ProviderError {
        provider: "tpk",
        detail: format!("{}{tail}", "d".repeat(400)).into(),
        limit: Some(limit()),
        memory: None,
    };
    let reads = [
        read_limit(ReadDetail::Limit(limit())),
        read_tree_limit(),
        ParseError::Tree(TreeError::Limit(limit())),
        ParseError::Provider(provider),
        deref(1),
        deref(400),
    ];
    for e in &reads {
        let n = read_note(e);
        assert!(n.chars().count() <= NOTE_MAX && n.ends_with(&tail), "{n}");
        assert!(out_of_memory(e).is_ok());
        let w = WriteError::Tree(Box::new(e.clone()));
        let n = write_note(&w);
        assert!(n.chars().count() <= NOTE_MAX && n.ends_with(&tail), "{n}");
        assert!(write_out_of_memory(&w).is_ok());
    }
    for w in [
        WriteError::Limit(limit()),
        WriteError::Charge(ChargeError::Limit(limit())),
    ] {
        assert_eq!(write_note(&w), tail);
    }
}

/// A refusal of memory is found through the same wrappers, and is never a
/// limit a note keeps; an error with neither keeps `note_of`'s note.
#[test]
fn a_memory_refusal_is_found_through_every_wrapper_and_is_no_limit() {
    let b = budget();
    let roots = [
        Error::Budget(b.clone()),
        Error::Tree(TreeError::Charge(ChargeError::Budget(b.clone()))),
        Error::AssetBundle(Arc::new(read_limit(ReadDetail::Charge(
            ChargeError::Budget(b.clone()),
        )))),
    ];
    for root in roots {
        for e in wrapped(&root) {
            assert_eq!(memory_refused(&e), Some(&b));
            let text = e.to_string();
            assert_eq!(note(&e), crate::record::note(&text));
        }
    }
    let provider = ParseError::Provider(ProviderError {
        provider: "tpk",
        detail: b.to_string().into(),
        limit: None,
        memory: Some(b.clone()),
    });
    assert_eq!(out_of_memory(&provider), Err(Error::Budget(b.clone())));
    let w = WriteError::Tree(Box::new(provider));
    assert_eq!(write_out_of_memory(&w), Err(Error::Budget(b.clone())));
    let w = WriteError::Charge(ChargeError::Budget(b.clone()));
    assert_eq!(write_out_of_memory(&w), Err(Error::Budget(b)));
    let plain = in_entries(
        Error::Malformed {
            what: "x",
            detail: "y".into(),
        },
        400,
        2,
    );
    assert!(memory_refused(&plain).is_none());
    assert_eq!(note(&plain), crate::record::note_of(&plain));
}
