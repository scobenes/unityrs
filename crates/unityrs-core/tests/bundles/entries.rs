//! The entries' weak parent handle, and entry slicing as Python slices.

use super::open;
use unityrs_core::files::{File, ParentKind};
use unityrs_core::stream::py_read;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::serialized::build as file;
use unityrs_testkit::{blob::NodeSpec, serialized::one_object};

#[test]
fn an_entry_holds_its_bundle_weakly() {
    let data = build(&BundleSpec {
        entries: vec![EntrySpec {
            name: b"CAB-x".to_vec(),
            data: file(&one_object(
                1,
                vec![NodeSpec::new(0, "GameObject", "Base")],
                vec![],
            )),
            flags: 4,
        }],
        ..BundleSpec::default()
    });
    let b = open(&data);
    let File::Serialized(sf) = b.entry(0).unwrap() else {
        panic!("a SerializedFile entry")
    };
    let parent = sf.parent().unwrap().clone();
    let ParentKind::Bundle {
        bundle,
        version_engine,
    } = &parent.kind
    else {
        panic!("a bundle parent")
    };
    assert_eq!(version_engine.as_bytes(), b"2022.3.0f1");
    assert_eq!(parent.depth, 1);
    assert!(bundle.upgrade().is_some());
    drop(b);
    assert!(
        bundle.upgrade().is_none(),
        "the entry does not keep its bundle alive"
    );
}

/// The directory row of `name` patched to `offset` and `size` (blocks-info
/// stored, so the row is in the file as written).
fn with_row(name: &[u8], offset: i64, size: i64) -> (Vec<u8>, Vec<u8>) {
    let entries = vec![
        EntrySpec {
            name: b"a".to_vec(),
            data: (0..50u8).collect(),
            flags: 0,
        },
        EntrySpec {
            name: b"zz-target".to_vec(),
            data: (100..130u8).collect(),
            flags: 0,
        },
    ];
    let joined: Vec<u8> = entries.iter().flat_map(|e| e.data.clone()).collect();
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: 16,
        entries,
        ..BundleSpec::default()
    });
    let mut key = name.to_vec();
    key.push(0);
    let at = data.windows(key.len()).position(|w| w == key).unwrap();
    data[at - 20..at - 12].copy_from_slice(&offset.to_be_bytes());
    data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    (data, joined)
}

#[test]
fn entry_slices_are_python_slices_of_the_joined_data() {
    for (offset, size) in [
        (0, 30),
        (-5, 3),
        (-5, 10),
        (-1, -1),
        (10, -1),
        (10, -3),
        (-100, 5),
        (70, 0),
        (75, 100),
        (80, 1),
        (200, 5),
        (i64::MIN, 4),
        (3, i64::MAX),
        (i64::MAX, i64::MAX),
        (-3, i64::MIN),
    ] {
        let (data, joined) = with_row(b"zz-target", offset, size);
        let b = open(&data);
        let (range, _) = py_read(joined.len(), i128::from(offset), size);
        let want = &joined[range];
        let got = b.entry_bytes(1).unwrap();
        assert_eq!(&got[..], want, "offset {offset} size {size}");
        assert!(b.load_all().is_ok());
        assert_eq!(&b.entry_bytes(1).unwrap()[..], want);
    }
}
