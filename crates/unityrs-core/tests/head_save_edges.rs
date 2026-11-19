//! Malformed stream-read envelopes and large shared dictionaries.
use std::sync::Arc;
use unityrs_core::files::{SaveError, SaveLayout, SaveOptions, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::stream::ReaderKind;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::blob::{NodeSpec, Strings, blob_tree};
use unityrs_testkit::writer::ByteWriter;

fn envelope(nodes: &[NodeSpec]) -> Vec<u8> {
    let mut payload = ByteWriter::le();
    payload.bytes(b"mhtt").i32(19);
    payload.bytes(&blob_tree(nodes, 19, false, Strings::Local, &[]));
    payload.u32(0); // reference hashes
    let mut out = ByteWriter::le();
    out.bytes(&[b'T'; 16])
        .u32(u32::try_from(payload.len()).unwrap())
        .bytes(payload.as_slice());
    out.into_inner()
}

fn records(format: u32, types: &[Vec<u8>], refs: &[Vec<u8>], object: bool) -> Bytes {
    let mut out = ByteWriter::le();
    out.bytes(&[0; 48]).cstr(b"6000.4.0f1").i32(0).bool(true);
    for (entries, is_ref) in [(types, false), (refs, true)] {
        out.i32(i32::try_from(entries.len()).unwrap());
        for info in entries {
            out.i32(1).bool(false).i16(-1).bytes(&[0; 16]);
            out.bytes(info);
            if is_ref {
                out.cstr(b"Part").cstr(b"Synthetic").cstr(b"Assembly");
            } else {
                out.i32(0); // dependencies
            }
        }
        if !is_ref {
            out.i32(i32::from(object));
            if object {
                out.align(4).i64(1).i64(0).u32(4).i32(0);
            }
            out.i32(0).i32(0); // scripts, externals
        }
    }
    out.cstr(b"synthetic");
    let metadata = u32::try_from(out.len() - 48).unwrap();
    out.align(16);
    let offset = u64::try_from(out.len()).unwrap();
    if object {
        out.i32(37);
    }
    let mut data = out.into_inner();
    let size = u64::try_from(data.len()).unwrap();
    data[8..12].copy_from_slice(&format.to_be_bytes());
    data[20..24].copy_from_slice(&metadata.to_be_bytes());
    data[24..32].copy_from_slice(&size.to_be_bytes());
    data[32..40].copy_from_slice(&offset.to_be_bytes());
    Bytes::from(data)
}

#[test]
fn semantic_formats_23_to_25_preserve_flagged_descendants_and_readable_objects() {
    let mut part = NodeSpec::new(1, "Part", "part");
    part.type_flags = 32;
    let nodes = [
        NodeSpec::new(0, "Synthetic", "Base"),
        part,
        NodeSpec::new(2, "int", "value"),
    ];
    for format in 23..=25 {
        for kind in [ReaderKind::Memory, ReaderKind::Stream] {
            let meter = Meter::unlimited();
            let info = envelope(&nodes);
            let file = parse(
                records(
                    format,
                    std::slice::from_ref(&info),
                    std::slice::from_ref(&info),
                    true,
                ),
                &meter,
                kind,
            );
            assert!(file.object(1).unwrap().parse_as_dict(true).is_ok());
            for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
                let saved = file.save(SaveOptions { layout }).unwrap();
                let back = parse(saved.clone(), &meter, kind);
                if layout == SaveLayout::Semantic {
                    assert_eq!(back.types()[0].nodes, file.types()[0].nodes);
                    assert_eq!(
                        back.ref_types().unwrap()[0].nodes,
                        file.ref_types().unwrap()[0].nodes
                    );
                    assert!(back.object(1).unwrap().parse_as_dict(true).is_ok());
                } else {
                    assert_eq!(back.types()[0].nodes.as_ref().unwrap().len(), 2);
                    assert_eq!(
                        back.ref_types().unwrap()[0].nodes.as_ref().unwrap().len(),
                        2
                    );
                }
                assert_eq!(back.save(SaveOptions { layout }).unwrap(), saved);
            }
        }
    }
}

#[test]
fn head_preflight_charges_each_plain_and_reference_type_before_validation() {
    use unityrs_core::LimitKind;
    use unityrs_core::budget::{Reservation, Unbounded};
    use unityrs_core::limits::ChargeError;

    for missing_ref in [false, true] {
        for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
            let info = envelope(&[NodeSpec::new(0, "Synthetic", "Base")]);
            let mut entries = vec![info; 5000];
            entries.push(vec![0; 20]); // no-info handler, only the last type is missing
            let (types, refs) = if missing_ref {
                (&[][..], entries.as_slice())
            } else {
                (entries.as_slice(), &[][..])
            };
            for remaining in [0, 5000, 5001] {
                let meter = Meter::with_decode_work(
                    1 << 26,
                    1 << 26,
                    Arc::new(Unbounded),
                    Reservation::Input,
                );
                let file = parse(records(23, types, refs, false), &meter, ReaderKind::Memory);
                meter
                    .count_decode((1 << 26) - meter.decoded() - remaining)
                    .unwrap();
                let before = (meter.used(), meter.granted(), meter.decoded());
                let result = file.save(SaveOptions { layout });
                if remaining == 5001 {
                    assert_eq!(result, Err(SaveError::Missing("HEAD type node")));
                    assert_eq!(meter.decoded() - before.2, 5001);
                } else {
                    let Err(SaveError::Charge(ChargeError::Limit(error))) = result else {
                        panic!("preflight must refuse exhausted work before validating a type");
                    };
                    assert_eq!(error.kind(), LimitKind::DecodeWork);
                }
                assert_eq!((meter.used(), meter.granted()), (before.0, before.1));
            }
        }
    }
}

fn input(handlers: &[Vec<u8>], terminate: bool) -> Bytes {
    let mut data = vec![0; 48];
    data.extend_from_slice(b"6000.4.0f1\0");
    data.extend_from_slice(&0i32.to_le_bytes()); // platform
    data.push(1); // trees
    for _ in 0..5 {
        // types, objects, scripts, externals, ref types
        data.extend_from_slice(&0i32.to_le_bytes());
    }
    data.extend_from_slice(&i32::try_from(handlers.len()).unwrap().to_le_bytes());
    for handler in handlers {
        data.extend_from_slice(handler);
    }
    if terminate {
        data.push(0); // user information
    }
    let metadata_size = u32::try_from(data.len() - 48).unwrap();
    let size = u64::try_from(data.len()).unwrap();
    data[8..12].copy_from_slice(&26u32.to_be_bytes());
    data[20..24].copy_from_slice(&metadata_size.to_be_bytes());
    data[24..32].copy_from_slice(&size.to_be_bytes());
    data[32..40].copy_from_slice(&size.to_be_bytes());
    Bytes::from(data)
}

fn parse(data: Bytes, meter: &Arc<Meter>, kind: ReaderKind) -> Arc<SerializedFile> {
    SerializedFile::parse_with_reader(
        RawStr::from_static(b"CAB-synthetic"),
        data,
        None,
        meter,
        &Arc::new(FileContext::unitypy()),
        kind,
    )
    .unwrap()
}

#[test]
fn a_truncated_stream_reference_hash_is_refused_on_read() {
    let mut handler = vec![b'T'; 16];
    handler.extend_from_slice(&60u32.to_le_bytes());
    handler.extend_from_slice(b"mhtt");
    handler.extend_from_slice(&19i32.to_le_bytes());
    handler.extend_from_slice(&1u32.to_le_bytes()); // one root node
    handler.extend_from_slice(&7u32.to_le_bytes());
    handler.extend_from_slice(&[1, 0, 0, 0]); // version, level, flags
    handler.extend_from_slice(&0u32.to_le_bytes()); // type offset
    handler.extend_from_slice(&2u32.to_le_bytes()); // name offset
    handler.extend_from_slice(&(-1i32).to_le_bytes());
    handler.extend_from_slice(&[0; 16]); // index, meta, ref type hash
    handler.extend_from_slice(b"S\0Base\0");
    handler.extend_from_slice(&1u32.to_le_bytes());
    handler.push(b'x'); // truncated reference hash, accepted by HEAD's stream reads
    // The list is bounded by its envelope (Tier C, PARITY.md section 19.1),
    // so the hash no reader could complete is refused before any save.
    for kind in [ReaderKind::Memory, ReaderKind::Stream] {
        let error = SerializedFile::parse_with_reader(
            RawStr::from_static(b"CAB-synthetic"),
            input(std::slice::from_ref(&handler), false),
            None,
            &Meter::unlimited(),
            &Arc::new(FileContext::unitypy()),
            kind,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("reference hashes exceed the envelope")
        );
    }
}

#[test]
fn shared_dictionary_save_work_is_subquadratic_and_releases_index_storage() {
    for count in [1024u32, 4096] {
        let handlers: Vec<_> = (0..count)
            .map(|i| {
                let mut handler = i.to_le_bytes().to_vec();
                handler.extend_from_slice(&[0; 16]); // rest of hash and no-info size
                handler
            })
            .collect();
        let meter = Meter::unlimited();
        let original = input(&handlers, true);
        let file = parse(original.clone(), &meter, ReaderKind::Memory);
        let before = (meter.decoded(), meter.used());
        let saved = file.save(SaveOptions::default()).unwrap();
        let work = meter.decoded() - before.0;
        assert!(
            work < u64::from(count) * 1024,
            "{count} handlers used {work} work units"
        );
        // The shared count starts at 48 + 11 + 4 + 1 + 5*4 = 84.
        // Little-endian keys after 255 differ from sorted hash order.
        let end = 88 + usize::try_from(count).unwrap() * 20;
        assert_eq!(&saved[84..end], &original[84..end]);
        drop(saved);
        assert_eq!(meter.used(), before.1);
    }
}
