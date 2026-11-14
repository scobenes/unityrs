//! Synthetic archive seeds covering nested loading, splits and duplicate reads.
use super::Seeds;

pub(super) fn seeds(s: &mut Seeds) -> std::io::Result<()> {
    for (name, entries) in [
        ("stored", vec![("CAB-A", b"abc".as_slice(), false)]),
        ("deflate", vec![("sub/CAB-A", b"abc".as_slice(), true)]),
        (
            "split",
            vec![
                ("a.split0", b"abc".as_slice(), false),
                ("a.split1", b"def".as_slice(), true),
            ],
        ),
        ("empty", vec![]),
    ] {
        s.add("zip", name, &unityrs_testkit::zip::archive(&entries))?;
    }
    let nested = unityrs_testkit::zip::archive(&[
        ("a.split0", b"abc", false),
        ("a.split1", b"def", true),
        ("duplicate", b"first", false),
        ("duplicate", b"last", true),
    ]);
    s.add(
        "zip",
        "nested-duplicates",
        &unityrs_testkit::zip::archive(&[("nested", &nested, false), ("nested", &nested, true)]),
    )?;
    Ok(())
}
