//! Formats 23-25 node-info envelopes without a reference-hash list, the
//! layout Unity 6000.6 writes (PARITY.md section 19.1). All bytes are built
//! here; no Editor output is used.
use std::sync::Arc;
use unityrs_core::files::{SaveLayout, SaveOptions, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::stream::ReaderKind;
use unityrs_core::{Bytes, FileContext, RawStr, Value};
use unityrs_testkit::blob::{NodeSpec, Strings, blob_tree};
use unityrs_testkit::writer::ByteWriter;

fn nodes() -> [NodeSpec; 2] {
    [
        NodeSpec::new(0, "Synthetic", "Base"),
        NodeSpec::new(1, "int", "m_A").size(4),
    ]
}

/// The hash, the serialized size and the envelope. `refs` appends HEAD's
/// reference-hash list inside the size; `None` writes the bare envelope.
fn envelope(format: u32, refs: Option<&[[u8; 16]]>) -> Vec<u8> {
    let mut payload = ByteWriter::le();
    payload.bytes(b"mhtt").i32(i32::try_from(format).unwrap());
    payload.bytes(&blob_tree(&nodes(), format, false, Strings::Local, &[]));
    if let Some(refs) = refs {
        payload.u32(u32::try_from(refs.len()).unwrap());
        for hash in refs {
            payload.bytes(hash);
        }
    }
    let mut out = ByteWriter::le();
    out.bytes(&[b'H'; 16])
        .u32(u32::try_from(payload.len()).unwrap())
        .bytes(payload.as_slice());
    out.into_inner()
}

/// A type's count of one, then class 1, unstripped, no script, and `info`.
fn head(out: &mut ByteWriter, info: &[u8]) {
    out.i32(1).i32(1).bool(false).i16(-1).bytes(&[0; 16]);
    out.bytes(info);
}

/// One plain type with dependencies `[5, 7]`, one ref type and one object.
fn file(format: u32, info: &[u8]) -> Bytes {
    let mut out = ByteWriter::le();
    out.bytes(&[0; 48]).cstr(b"6000.6.0f1").i32(19).bool(true);
    head(&mut out, info);
    out.i32(2).i32(5).i32(7); // dependencies
    out.i32(1).align(4).i64(1).i64(0).u32(4).i32(0); // one object
    out.i32(0).i32(0); // scripts, externals
    head(&mut out, info);
    out.cstr(b"Part").cstr(b"Synthetic").cstr(b"Assembly");
    if format >= 26 {
        out.i32(0); // shared subtrees
    }
    out.cstr(b"");
    let metadata = u32::try_from(out.len() - 48).unwrap();
    out.align(16);
    let offset = u64::try_from(out.len()).unwrap();
    out.i32(37);
    let mut data = out.into_inner();
    let size = u64::try_from(data.len()).unwrap();
    data[8..12].copy_from_slice(&format.to_be_bytes());
    data[20..24].copy_from_slice(&metadata.to_be_bytes());
    data[24..32].copy_from_slice(&size.to_be_bytes());
    data[32..40].copy_from_slice(&offset.to_be_bytes());
    Bytes::from(data)
}

fn parse(data: Bytes, kind: ReaderKind) -> Result<Arc<SerializedFile>, unityrs_core::Error> {
    SerializedFile::parse_with_reader(
        RawStr::from_static(b"CAB-synthetic"),
        data,
        None,
        &Meter::unlimited(),
        &Arc::new(FileContext::unitypy()),
        kind,
    )
}

/// Each envelope in `saved`: `None` when its size ends at the blob's end,
/// else the reference count its list holds.
fn envelopes(saved: &[u8]) -> Vec<Option<u32>> {
    let word = |at: usize| u32::from_le_bytes(saved[at..at + 4].try_into().unwrap());
    let mut out = Vec::new();
    for at in 0..saved.len() - 4 {
        if &saved[at..at + 4] == b"mhtt" {
            let size = word(at - 4);
            let blob = 16 + 32 * word(at + 8) + word(at + 12);
            out.push((size != blob).then(|| word(at + usize::try_from(blob).unwrap())));
        }
    }
    out
}

fn assert_readable(file: &Arc<SerializedFile>) {
    let ty = &file.types()[0];
    assert_eq!(ty.nodes.as_ref().unwrap().len(), 2);
    assert_eq!(ty.type_dependencies.as_deref(), Some(&[5, 7][..]));
    let reference = &file.ref_types().unwrap()[0];
    assert_eq!(reference.nodes, ty.nodes);
    assert_eq!(
        reference.class_name.as_ref().map(RawStr::as_bytes),
        Some(&b"Part"[..])
    );
    let parsed = file.object(1).unwrap().parse_as_dict(true).unwrap();
    assert_eq!(parsed.get(&[b"m_A"]).and_then(Value::as_i64), Some(37));
}

#[test]
fn formats_23_to_25_read_bare_envelopes_and_save_them_bare() {
    for format in 23..=25 {
        let bare = envelope(format, None);
        let original = file(format, &bare);
        for kind in [ReaderKind::Memory, ReaderKind::Stream] {
            let file = parse(original.clone(), kind).unwrap();
            assert_readable(&file);
            assert_eq!(envelopes(&original), [None, None]);
            for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
                let saved = file.save(SaveOptions { layout }).unwrap();
                // Both envelopes keep their layout: no reference count added.
                assert_eq!(envelopes(&saved), [None, None], "{format} {layout:?}");
                let back = parse(saved.clone(), kind).unwrap();
                assert_readable(&back);
                assert_eq!(back.save(SaveOptions { layout }).unwrap(), saved);
            }
        }
    }
}

#[test]
fn listed_reference_hashes_still_read_as_head_reads_them() {
    let refs = [[0xA5; 16], [0x5A; 16]];
    for format in 23..=26 {
        let listed = envelope(format, Some(&refs));
        let original = file(format, &listed);
        let file = parse(original, ReaderKind::Memory).unwrap();
        assert_readable(&file);
        for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
            let saved = file.save(SaveOptions { layout }).unwrap();
            assert_eq!(envelopes(&saved), [Some(2), Some(2)], "{format} {layout:?}");
            assert_readable(&parse(saved, ReaderKind::Memory).unwrap());
        }
    }
}

#[test]
fn format_26_keeps_heads_reference_count_after_a_bare_envelope() {
    // Format 26 has shared subtrees and keeps HEAD's envelope exactly, so the
    // dependency count is taken for a reference count and the tables shift.
    let original = file(26, &envelope(26, None));
    let misread = parse(original, ReaderKind::Memory)
        .map(|file| file.types()[0].type_dependencies.as_deref() != Some(&[5, 7][..]));
    assert!(misread.unwrap_or(true));
}

#[test]
fn a_size_other_than_the_blob_end_reads_a_reference_count() {
    // A size that is only a presence gate (HEAD's reading) is not bare.
    let mut info = envelope(23, Some(&[]));
    let size = u32::from_le_bytes(info[16..20].try_into().unwrap());
    info[16..20].copy_from_slice(&(size + 1).to_le_bytes());
    let file = parse(file(23, &info), ReaderKind::Memory).unwrap();
    assert_readable(&file);
}

#[test]
fn a_listed_envelope_whose_size_excludes_its_list_reads_as_bare() {
    // Tier C: HEAD takes this size for a presence gate and reads the list
    // after it, as format 26 still does here. At 23 the bare reading takes
    // the list's count (0) for the dependencies, the real dependency count
    // (2) for the object count, and the tables after it shift.
    for format in [23, 26] {
        let mut info = envelope(format, None);
        info.extend_from_slice(&0u32.to_le_bytes()); // the list, outside the size
        let got = parse(file(format, &info), ReaderKind::Memory);
        if format == 26 {
            assert_readable(&got.unwrap());
        } else {
            assert!(
                matches!(got, Err(unityrs_core::Error::TypeIndex { .. })),
                "{got:?}"
            );
        }
    }
}

/// A format-26 file whose last bytes are `handler`, its one shared handler.
fn shared_tail(handler: &[u8]) -> Bytes {
    let mut out = ByteWriter::le();
    out.bytes(&[0; 48]).cstr(b"6000.6.0f1").i32(19).bool(true);
    out.i32(0).i32(0).i32(0).i32(0).i32(0); // types, objects, scripts, externals, refs
    out.i32(1).bytes(handler);
    let mut data = out.into_inner();
    let size = u64::try_from(data.len()).unwrap();
    data[8..12].copy_from_slice(&26u32.to_be_bytes());
    let metadata = u32::try_from(data.len() - 48).unwrap();
    data[20..24].copy_from_slice(&metadata.to_be_bytes());
    data[24..32].copy_from_slice(&size.to_be_bytes());
    data[32..40].copy_from_slice(&size.to_be_bytes());
    Bytes::from(data)
}

/// A listed envelope with `count` as its count, `hashes` bytes of hashes and
/// a declared size of `size(blob_end)`, where `blob_end` is the bytes from
/// the magic to the end of the blob.
fn listed(count: u32, hashes: usize, size: impl Fn(u32) -> u32) -> Vec<u8> {
    let mut info = envelope(26, None);
    let blob_end = u32::from_le_bytes(info[16..20].try_into().unwrap());
    info[16..20].copy_from_slice(&size(blob_end).to_le_bytes());
    info.extend_from_slice(&count.to_le_bytes());
    info.extend(std::iter::repeat_n(0xA5, hashes));
    info
}

fn refusal(data: Bytes, kind: ReaderKind) -> String {
    let meter = Meter::unlimited();
    let error = SerializedFile::parse_with_reader(
        RawStr::from_static(b"CAB-synthetic"),
        data,
        None,
        &meter,
        &Arc::new(FileContext::unitypy()),
        kind,
    )
    .unwrap_err();
    assert!(meter.decoded() < 4096, "refused before reading the list");
    assert_eq!(meter.used(), 0);
    error.to_string()
}

#[test]
fn a_reference_list_is_bounded_by_its_envelope_before_any_hash_is_read() {
    for kind in [ReaderKind::Memory, ReaderKind::Stream] {
        // A huge count in an envelope declaring room for the count alone.
        let huge = shared_tail(&listed(u32::MAX, 64, |end| end + 4));
        assert!(refusal(huge, kind).contains("reference hashes exceed the envelope"));
        // A size reaching past the blob but not past the list it holds.
        let short = shared_tail(&listed(2, 32, |end| end + 20));
        assert!(refusal(short, kind).contains("reference hashes exceed the envelope"));
        // A declared hash the input ends inside.
        let truncated = shared_tail(&listed(1, 3, |end| end + 20));
        assert!(refusal(truncated, kind).contains("short reference hash"));
        // A size below the blob's end (HEAD's presence-only size) is bounded
        // by the input instead: a count past its end is refused.
        let presence = shared_tail(&listed(1 << 20, 64, |_| 8));
        assert!(refusal(presence, kind).contains("reference hashes exceed the envelope"));
    }
}

#[test]
fn a_canonical_bare_file_saves_to_its_own_bytes() {
    // The writer rebuilds each blob, taking common strings where it can, and
    // pads object data to 8: built that way, with no unused strings, the
    // saved file equals the original byte for byte.
    let common: &[(u32, &[u8])] = &[(55, b"Base"), (222, b"int")];
    let mut payload = ByteWriter::le();
    payload.bytes(b"mhtt").i32(23);
    payload.bytes(&blob_tree(&nodes(), 23, false, Strings::Common, common));
    let mut info = ByteWriter::le();
    info.bytes(&[b'H'; 16])
        .u32(u32::try_from(payload.len()).unwrap())
        .bytes(payload.as_slice());
    let mut original = file(23, info.as_slice()).to_vec();
    original.resize(original.len().next_multiple_of(8), 0);
    let size = u64::try_from(original.len()).unwrap();
    original[24..32].copy_from_slice(&size.to_be_bytes());
    let original = Bytes::from(original);
    let file = parse(original.clone(), ReaderKind::Memory).unwrap();
    assert_readable(&file);
    for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
        let saved = file.save(SaveOptions { layout }).unwrap();
        assert_eq!(envelopes(&saved), [None, None], "{layout:?}");
        assert_eq!(saved, original, "{layout:?}");
    }
}
