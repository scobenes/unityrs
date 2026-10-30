//! Two safety limits `PARITY.md` section 19.3 lists that no other test pins:
//! nested containers past `Limits::max_container_depth`, and a gzip stream
//! that inflates past `Limits::max_inflate` while it is detected. UnityPy
//! has neither bound: it recurses until Python's recursion limit, and
//! inflates the whole stream (`helpers/ImportHelper.py:75-83`).
//!
//!     scripts/dev cargo test -p unityrs-core --test limits

use std::io::Write as _;
use std::sync::Arc;

use flate2::Compression;
use flate2::write::GzEncoder;
use unityrs_core::budget::Reservation;
use unityrs_core::detect::{DetectError, FileType, check_file_type};
use unityrs_core::files::File;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::{BundleFile, Bytes, Error, FileContext, LimitKind, RawStr};
use unityrs_testkit::env::{Obj, bundle, serialized};

/// Three bundles nested in a top-level one: entries at container depths 1,
/// 2 and 3 (a top-level file's entry is at depth 1).
fn nested() -> Vec<u8> {
    let mut data = bundle(&[("CAB-x", serialized(&[], &[Obj::named(1, "n", &[])]))]);
    for name in ["b0.bundle", "b1.bundle", "b2.bundle"] {
        data = bundle(&[(name, data)]);
    }
    data
}

/// Opens `data` and follows entry 0 down; returns the depth of each bundle
/// reached and the error that stopped the walk, if one did.
fn walk(data: Vec<u8>, max_container_depth: u32) -> (usize, Option<Error>) {
    let mut c = FileContext::unitypy();
    c.limits.max_container_depth = max_container_depth;
    let ctx = Arc::new(c);
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let mut b =
        BundleFile::open(RawStr::from("top"), Bytes::from(data), None, &meter, &ctx).unwrap();
    let mut bundles = 1;
    loop {
        match b.entry(0) {
            Ok(File::Bundle(inner)) => {
                bundles += 1;
                b = inner;
            }
            Ok(_) => return (bundles, None),
            Err(e) => return (bundles, Some(e)),
        }
    }
}

#[test]
fn nested_containers_past_the_depth_bound_are_refused() {
    assert!(matches!(
        walk(nested(), Limits::default().max_container_depth),
        (4, None)
    ));
    let (bundles, e) = walk(nested(), 2);
    assert_eq!(bundles, 3);
    // A limit is the bundle's error as it is, not wrapped in the entry's.
    assert!(
        matches!(e, Some(Error::Limit(l)) if l.kind == LimitKind::ContainerDepth && l.value == 3 && l.max == 2),
        "{e:?}"
    );
}

#[test]
fn a_gzip_stream_past_the_inflate_bound_is_refused_at_detection() {
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(&vec![0u8; 1 << 20]).unwrap();
    let data = gz.finish().unwrap();
    assert!(data.len() >= 128, "{}", data.len());
    let mut small = Limits::default();
    small.max_inflate = 1 << 16;
    let refused = check_file_type(&data, &small);
    assert!(
        matches!(&refused, Err(DetectError::InflateLimit(l))
            if l.kind == LimitKind::Inflate && l.value > l.max && l.max == 1 << 16),
        "{refused:?}"
    );
    // Its text is every limit refusal's (`PARITY.md` section 19.3).
    let text = refused.unwrap_err().to_string();
    assert!(
        text.starts_with("limit Inflate: ") && text.ends_with(" > 65536"),
        "{text}"
    );
    // Under the default bound the stream inflates whole, and zeros are no
    // WebFile signature.
    assert_eq!(
        check_file_type(&data, &Limits::default()).unwrap(),
        FileType::ResourceFile
    );
}
