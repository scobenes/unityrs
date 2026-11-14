//! `CPython` ZIP compatibility and hostile input regressions from C1 review.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{io::Write, sync::Arc};
use unityrs_core::{
    Bytes, Error, FileContext, LimitKind, RawStr,
    budget::{Counting, Reservation, Unbounded},
    env::{EnvOptions, Environment, Source},
    limits::{Limits, Meter},
    zip::ZipArchive,
};
use unityrs_testkit::{scratch::Scratch, zip::archive};

fn central(data: &[u8]) -> usize {
    data.windows(4).position(|w| w == b"PK\x01\x02").unwrap()
}
fn set16(data: &mut [u8], at: usize, value: u16) {
    data[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn set32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn meter(limits: &Limits) -> Arc<Meter> {
    Meter::for_input(limits, Arc::new(Unbounded), Reservation::Input)
}
fn read(data: Vec<u8>) -> Result<Bytes, Error> {
    let limits = Limits::default();
    ZipArchive::open(data.into(), &meter(&limits))?.read(0, &limits)
}
fn load(env: &Environment, data: Vec<u8>) -> Result<(), Error> {
    env.load_zip_file(Source::Bytes {
        name: RawStr::from("review.zip"),
        data: data.into(),
    })
}
fn limited(work: u64) -> Environment {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_decode_work = work;
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(ctx);
    Environment::new(options)
}
fn work_error(result: &Result<(), Error>) {
    assert!(matches!(result, Err(Error::Limit(e)) if e.kind() == LimitKind::DecodeWork));
}

#[test]
fn zipalign_local_padding_and_short_central_tails_are_ignored() {
    for n in 1..=3 {
        let mut data = archive(&[("assets/bin/Data/a", b"abc", false)]);
        let c = central(&data);
        data.splice(
            30 + "assets/bin/Data/a".len()..30 + "assets/bin/Data/a".len(),
            vec![0; n],
        );
        set16(&mut data, 28, u16::try_from(n).unwrap());
        let end = data.len() - 22;
        set32(&mut data, end + 16, u32::try_from(c + n).unwrap());
        assert_eq!(&read(data).unwrap()[..], b"abc");
    }
    for extra in [
        vec![9, 8, 7],
        vec![1, 0, 0, 0],
        vec![2, 0, 1, 0, 42, 9, 8, 7],
    ] {
        let mut data = archive(&[("a", b"abc", false)]);
        let c = central(&data);
        data.splice(c + 47..c + 47, extra.iter().copied());
        set16(&mut data, c + 30, u16::try_from(extra.len()).unwrap());
        let end = data.len() - 22;
        set32(&mut data, end + 12, u32::try_from(end - c).unwrap());
        assert_eq!(&read(data).unwrap()[..], b"abc");
    }
}

#[test]
fn central_metadata_wins_and_eocd_is_permissive() {
    let original = archive(&[("a", b"abc", true)]);
    for (at, value) in [(6, 0x2849), (8, 99), (14, 55), (18, 999), (22, 888)] {
        let mut data = original.clone();
        set16(&mut data, at, value);
        assert_eq!(&read(data).unwrap()[..], b"abc", "local {at}");
    }
    for flags in [0x808, 0x2800] {
        let mut data = original.clone();
        let c = central(&data);
        set16(&mut data, c + 8, flags);
        assert_eq!(&read(data).unwrap()[..], b"abc"); // no descriptor, bit 13 ignored
    }
    for n in [0, 1, 999] {
        let mut data = original.clone();
        let end = data.len() - 22;
        set16(&mut data, end + 20, n);
        set16(&mut data, end + 4, 5);
        set16(&mut data, end + 6, 6);
        set16(&mut data, end + 8, 123);
        set16(&mut data, end + 10, 0);
        let c = central(&data);
        set16(&mut data, c + 34, 9);
        data.extend(b"trailing bytes");
        data.splice(..0, b"executable prefix".iter().copied());
        assert_eq!(&read(data).unwrap()[..], b"abc");
    }
    for flags in [0x820, 0x840, 0x801] {
        let mut data = original.clone();
        let c = central(&data);
        set16(&mut data, c + 8, flags);
        assert!(read(data).is_err());
    }
}

#[test]
fn local_names_are_compared_after_decoding_without_nul_truncation() {
    let mut data = archive(&[("é", b"abc", false)]); // Different flags are fine when the decoded ASCII name agrees.
    let mut ascii = archive(&[("a", b"abc", false)]);
    set16(&mut ascii, 6, 0);
    assert_eq!(&read(ascii).unwrap()[..], b"abc");
    set16(&mut data, 6, 0);
    assert!(read(data).is_err());
    let mut nul = archive(&[("a\0b", b"abc", false)]);
    nul[32] = b'c';
    assert!(read(nul).is_err());
    let mut data = archive(&[("a", b"abc", false)]);
    data[30] = b'b';
    assert!(read(data).is_err());
}

#[test]
fn eof_uses_crc_and_truncates_to_declared_output() {
    for deflate in [false, true] {
        let mut data = archive(&[("a", b"abc", deflate)]);
        let c = central(&data);
        set32(&mut data, c + 24, 4);
        assert_eq!(&read(data).unwrap()[..], b"abc");
        let mut data = archive(&[("a", b"abc", deflate)]);
        let c = central(&data);
        set32(&mut data, c + 24, 2);
        set32(&mut data, c + 16, unityrs_testkit::zip::crc32(b"ab"));
        assert_eq!(&read(data).unwrap()[..], b"ab");
        let mut data = archive(&[("a", b"abc", deflate)]);
        let c = central(&data);
        set32(&mut data, c + 24, 2);
        assert!(read(data).unwrap_err().to_string().contains("Bad CRC-32"));
    }
    let mut data = archive(&[("a", b"abc", true)]);
    let c = central(&data);
    data.splice(c..c, [22, 33, 44]);
    let end = data.len() - 22;
    set32(&mut data, c + 3 + 20, 11);
    set32(&mut data, end + 16, u32::try_from(c + 3).unwrap());
    assert_eq!(&read(data).unwrap()[..], b"abc");
    // A missing final deflate marker is accepted if the decoded CRC agrees.
    let mut data = archive(&[("a", b"abc", true)]);
    data[31] = 0;
    assert_eq!(&read(data).unwrap()[..], b"abc");
}

#[test]
fn zip64_selective_extras_and_eocd_records() {
    for markers in 0..8 {
        let mut data = archive(&[("a", b"abc", false)]);
        let c = central(&data);
        let mut extra = vec![1, 0, 0, 0];
        for (i, (at, value)) in [(24, 3_u64), (20, 3), (42, 0)].into_iter().enumerate() {
            if markers & (1 << i) != 0 {
                set32(&mut data, c + at, u32::MAX);
                extra.extend(value.to_le_bytes());
            }
        }
        let len = extra.len();
        set16(&mut extra, 2, u16::try_from(len - 4).unwrap());
        data.splice(c + 47..c + 47, extra);
        set16(&mut data, c + 30, u16::try_from(len).unwrap());
        let end = data.len() - 22;
        set32(&mut data, end + 12, u32::try_from(end - c).unwrap());
        let size = end - c;
        let mut records = b"PK\x06\x06".to_vec();
        records.extend(44_u64.to_le_bytes());
        records.extend([0; 12]);
        records.extend(1_u64.to_le_bytes());
        records.extend(1_u64.to_le_bytes());
        records.extend(u64::try_from(size).unwrap().to_le_bytes());
        records.extend(u64::try_from(c).unwrap().to_le_bytes());
        records.extend(b"PK\x06\x07");
        records.extend([0; 4]);
        records.extend(u64::try_from(end).unwrap().to_le_bytes());
        records.extend(1_u32.to_le_bytes());
        data.splice(end..end, records);
        let end = data.len() - 22;
        set16(&mut data, end + 10, u16::MAX);
        set32(&mut data, end + 12, u32::MAX);
        set32(&mut data, end + 16, u32::MAX);
        data.splice(..0, [0, 1, 2]);
        assert_eq!(&read(data).unwrap()[..], b"abc", "markers {markers}");
    }
    let mut data = archive(&[("a", b"abc", false)]);
    let c = central(&data);
    set32(&mut data, c + 24, u32::MAX);
    data.splice(c + 47..c + 47, [1, 0, 0, 0]);
    set16(&mut data, c + 30, 4);
    let end = data.len() - 22;
    set32(&mut data, end + 12, u32::try_from(end - c).unwrap());
    assert!(read(data).is_err());
}

#[test]
fn declared_four_gib_empty_member_never_preallocates_it() {
    let mut data = archive(&[("a", b"", false)]);
    let c = central(&data);
    set32(&mut data, c + 24, 0xffff_fff0);
    let mut limits = Limits::default();
    limits.max_inflate = u64::MAX;
    let budget = Arc::new(Counting::new(128 * 1024));
    let m = Meter::for_input(&limits, budget.clone(), Reservation::Input);
    let zip = ZipArchive::open(data.into(), &m).unwrap();
    assert!(zip.read(0, &limits).unwrap().is_empty());
    drop(zip);
    assert_eq!(budget.used(), 0);
}

#[test]
fn metadata_and_find_searches_refuse_at_the_work_limit() {
    let data = archive(&vec![("long/common/prefix/a", &b""[..], false); 1000]);
    let mut limits = Limits::default();
    limits.max_decode_work = 1000;
    assert!(
        matches!(ZipArchive::open(data.clone().into(), &meter(&limits)), Err(Error::Limit(e)) if e.kind() == LimitKind::DecodeWork)
    );
    let mut limits = Limits::default();
    limits.max_decode_work = 1_000_000;
    let m = meter(&limits);
    let zip = ZipArchive::open(data.into(), &m).unwrap();
    m.count_decode(limits.max_decode_work - m.decoded())
        .unwrap();
    assert!(
        matches!(zip.find(b"long/common/prefix/a"), Err(Error::Limit(e)) if e.kind() == LimitKind::DecodeWork)
    );
}

// Compress large synthetic data efficiently; the testkit's tiny deflate
// builder deliberately emits only a single uncompressed 64 KiB block.
fn compressed_member(bytes: &[u8]) -> Vec<u8> {
    let mut deflate = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    deflate.write_all(bytes).unwrap();
    let payload = deflate.finish().unwrap();
    let mut data = archive(&[("a", b"", true)]);
    let c = central(&data);
    data.splice(31..c, payload.iter().copied());
    let c = 31 + payload.len();
    let end = data.len() - 22;
    let mut crc = flate2::Crc::new();
    crc.update(bytes);
    set32(&mut data, c + 16, crc.sum());
    set32(&mut data, c + 20, u32::try_from(payload.len()).unwrap());
    set32(&mut data, c + 24, u32::try_from(bytes.len()).unwrap());
    set32(&mut data, end + 16, u32::try_from(c).unwrap());
    data
}
fn duplicate_rows(data: &[u8], n: usize) -> Vec<u8> {
    let c = central(data);
    let local = &data[..c];
    let row = &data[c..data.len() - 22];
    let mut out = Vec::with_capacity(c * n + row.len() * n + 22);
    for _ in 0..n {
        out.extend(local);
    }
    for i in 0..n {
        let start = out.len();
        out.extend(row);
        set32(&mut out, start + 42, u32::try_from(i * c).unwrap());
    }
    out.extend(&data[data.len() - 22..]);
    let end = out.len() - 22;
    set32(&mut out, end + 12, u32::try_from(row.len() * n).unwrap());
    set32(&mut out, end + 16, u32::try_from(c * n).unwrap());
    set16(&mut out, end + 8, u16::try_from(n).unwrap());
    set16(&mut out, end + 10, u16::try_from(n).unwrap());
    out
}

#[test]
fn duplicate_bomb_and_nested_bomb_stop_on_inflated_work() {
    let data = compressed_member(&vec![0; 64 * 1024 * 1024]);
    work_error(&load(
        &limited(2 * 1024 * 1024),
        duplicate_rows(&data, 4000),
    ));
    let mut data = archive(&[("a", &vec![0; 60_000], false)]);
    for _ in 0..8 {
        data = duplicate_rows(&compressed_member(&data), 8);
    }
    work_error(&load(&limited(512 * 1024), data));
}

#[test]
fn duplicate_rows_reuse_one_inflation() {
    let data = duplicate_rows(&compressed_member(&vec![0; 64 * 1024]), 4000);
    let env = limited(8 * 1024 * 1024);
    load(&env, data).unwrap();
    assert_eq!(env.files().count(), 1);
}

#[test]
fn missing_paths_follow_the_caller_and_split_rules() {
    let scratch = Scratch::new("review-missing");
    let env = Environment::new(EnvOptions::unitypy());
    std::fs::write(scratch.path().join("base.split0"), b"abc").unwrap();
    for name in ["missing", "missing.split9", "base", "base.split9"] {
        env.load_path(&scratch.path().join(name)).unwrap();
        assert_eq!(env.files().count(), 0);
    }
    for name in ["missing", "base"] {
        assert!(env.load_files([scratch.path().join(name)]).is_err());
    }
    assert!(
        env.load_file(Source::Path(scratch.path().join("missing")))
            .unwrap()
            .is_none()
    );
    assert!(
        env.load_file(Source::Path(scratch.path().join("base")))
            .unwrap()
            .is_some()
    );
    env.load_files([scratch.path().join("base.split9")])
        .unwrap();
    assert_eq!(env.files().count(), 1);
}

#[test]
fn dot_only_filenames_have_no_zip_extension() {
    let scratch = Scratch::new("review-dot-names");
    let mut data = archive(&[("inner", b"abc", false)]);
    data[4] = 20;
    for name in [".zip", "..zip", "a\\.zip", ".apk", "dir\\..zip"] {
        let path = scratch.path().join(name);
        std::fs::write(&path, &data).unwrap();
        let env = Environment::new(EnvOptions::unitypy());
        env.load_path(&path).unwrap();
        assert_eq!(
            env.files().next().unwrap().name.as_bytes(),
            path.as_os_str().as_encoded_bytes()
        );
    }
}

#[test]
fn nested_zip_none_overwrites_inner_and_earlier_files_in_load_order() {
    let asset = unityrs_testkit::env::serialized(
        &[],
        &[unityrs_testkit::env::Obj::named(7, "synthetic", &[])],
    );
    let inner = archive(&[("CAB-Z", &asset, false)]);
    let env = Environment::new(EnvOptions::unitypy());
    env.load_file(Source::Bytes {
        name: RawStr::from("CAB-Z"),
        data: asset.clone().into(),
    })
    .unwrap();
    load(&env, archive(&[("CAB-Z", &inner, false)])).unwrap();
    assert_eq!(env.files().count(), 0);
    assert_eq!(env.objects().count(), 0);
    // Replacing None restores the original dict position.
    env.load_file(Source::Bytes {
        name: RawStr::from("other"),
        data: Bytes::from_static(b"abc"),
    })
    .unwrap();
    env.load_file(Source::Bytes {
        name: RawStr::from("CAB-Z"),
        data: asset.into(),
    })
    .unwrap();
    assert_eq!(
        env.files().map(|e| e.name.to_string()).collect::<Vec<_>>(),
        ["CAB-Z", "other"]
    );
}

#[test]
fn unsupported_bzip2_and_lzma_methods_are_explicitly_refused() {
    for method in [12, 14] {
        let mut data = archive(&[("a", b"abc", false)]);
        let c = central(&data);
        set16(&mut data, c + 10, method);
        assert!(
            read(data)
                .unwrap_err()
                .to_string()
                .contains("unsupported compression method")
        );
    }
}

#[test]
fn duplicate_central_offsets_follow_cpythons_stable_overlap_order() {
    let mut data = archive(&[("a", b"abc", false)]);
    let c = central(&data);
    let row = data[c..data.len() - 22].to_vec();
    let end = data.len() - 22;
    data.splice(end..end, row.iter().copied());
    let end = data.len() - 22;
    set32(&mut data, end + 12, u32::try_from(row.len() * 2).unwrap());
    let limits = Limits::default();
    let zip = ZipArchive::open(data.into(), &meter(&limits)).unwrap();
    assert_eq!(&zip.read(0, &limits).unwrap()[..], b"abc");
    assert!(
        zip.read(1, &limits)
            .unwrap_err()
            .to_string()
            .contains("overlapping")
    );
    assert_eq!(zip.find(b"a").unwrap(), Some(1));
}

#[test]
fn deflate_stops_after_the_read_window_before_later_corruption() {
    let mut data = archive(&[("a", &vec![b'a'; 20_000], true)]);
    data[31] = 0; // nonfinal stored deflate block
    let c = central(&data);
    data.insert(c, 7); // invalid next block type
    let c = c + 1;
    let end = data.len() - 22;
    set32(&mut data, c + 20, 20_006);
    set32(&mut data, c + 24, 1);
    set32(&mut data, c + 16, unityrs_testkit::zip::crc32(b"a"));
    set32(&mut data, end + 16, u32::try_from(c).unwrap());
    assert_eq!(&read(data.clone()).unwrap()[..], b"a");
    let mut limits = Limits::default();
    limits.max_decode_work = 25_000;
    let m = meter(&limits);
    let zip = ZipArchive::open(data.clone().into(), &m).unwrap();
    assert_eq!(&zip.read(0, &limits).unwrap()[..], b"a");
    // The same stream is corrupt when its declaration requires the bad block.
    set32(&mut data, c + 24, 20_001);
    assert!(
        read(data)
            .unwrap_err()
            .to_string()
            .contains("invalid deflate")
    );
}

#[test]
fn deflate_prefix_reads_charge_every_inflated_byte() {
    let mut data = archive(&[("a", &vec![b'a'; 20_000], true)]);
    let c = central(&data);
    for (size, work) in [(0, 8218), (1, 8218), (8192, 24602), (8193, 24604)] {
        set32(&mut data, c + 24, size);
        let expected = vec![b'a'; usize::try_from(size).unwrap()];
        set32(&mut data, c + 16, unityrs_testkit::zip::crc32(&expected));
        let mut limits = Limits::default();
        limits.max_inflate = 20_000;
        let m = meter(&limits);
        let zip = ZipArchive::open(data.clone().into(), &m).unwrap();
        let before = m.decoded();
        assert_eq!(&zip.read(0, &limits).unwrap()[..], expected);
        // Local header/name + compressed windows + all output, discarded too.
        assert_eq!(m.decoded() - before, work);
        limits.max_decode_work = before + work - 1;
        let m = meter(&limits);
        let zip = ZipArchive::open(data.clone().into(), &m).unwrap();
        assert!(
            matches!(zip.read(0, &limits), Err(Error::Limit(e)) if e.kind() == LimitKind::DecodeWork)
        );
    }
}

#[test]
fn many_split_bases_use_linear_hash_work() {
    let entries: Vec<_> = (0..4000).map(|i| format!("name-{i:04}.split0")).collect();
    let rows: Vec<_> = entries
        .iter()
        .map(|name| (name.as_str(), &b"abc"[..], false))
        .collect();
    let env = limited(12 * 1024 * 1024);
    load(&env, archive(&rows)).unwrap();
    assert_eq!(env.files().count(), 4000);
}

#[test]
fn unused_duplicate_local_metadata_is_not_validated() {
    let mut data = archive(&[("a", b"first", false), ("a", b"last", false)]);
    let c = central(&data);
    let second = c + 47;
    let end = data.len() - 22;
    // A negative concat-adjusted offset on an unused row remains harmless.
    set32(&mut data, end + 16, u32::try_from(c + 100).unwrap());
    let old = u32::from_le_bytes(data[second + 42..second + 46].try_into().unwrap());
    set32(&mut data, second + 42, old + 100);
    set16(&mut data, c + 8, 0x861); // unused encrypted/patched row
    let env = Environment::new(EnvOptions::unitypy());
    load(&env, data).unwrap();
    assert_eq!(env.files().count(), 1);
}

#[test]
fn a_zip64_locator_without_room_for_its_record_is_refused() {
    let mut data = b"PK\x06\x07".to_vec();
    data.extend([0; 12]);
    data.extend(1_u32.to_le_bytes());
    data.extend(archive(&[]));
    assert!(ZipArchive::open(data.into(), &meter(&Limits::default())).is_err());
}

#[test]
fn a_short_physical_read_can_finish_on_stream_end_or_declared_size() {
    for deflate in [false, true] {
        let mut data = archive(&[("a", b"abc", deflate), ("unused", b"", false)]);
        let c = central(&data);
        let second = c + 47;
        // A later offset sets a distant read boundary, as in CPython.
        set32(&mut data, second + 42, 100_000);
        set32(&mut data, c + 20, 1000);
        let limits = Limits::default();
        let zip = ZipArchive::open(data.clone().into(), &meter(&limits)).unwrap();
        assert_eq!(&zip.read(0, &limits).unwrap()[..], b"abc");
        // Stored data that still needs more bytes raises EOF on its next read.
        if !deflate {
            set32(&mut data, c + 24, 1000);
            assert!(read(data).is_err());
        }
    }
}

#[test]
fn cp437_decodes_every_high_byte_in_order() {
    // bytes(range(128, 256)).decode("cp437"), CPython 3.11.18.
    const EXPECTED: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ ";
    let mut data = archive(&[(&"a".repeat(128), b"abc", false)]);
    let c = central(&data);
    set16(&mut data, 6, 0);
    set16(&mut data, c + 8, 0);
    for (i, b) in (128..=255).enumerate() {
        data[30 + i] = b;
        data[c + 46 + i] = b;
    }
    let limits = Limits::default();
    let zip = ZipArchive::open(data.into(), &meter(&limits)).unwrap();
    assert_eq!(zip.members()[0].name.as_bytes(), EXPECTED.as_bytes());
    assert_eq!(&zip.read(0, &limits).unwrap()[..], b"abc");
}

fn zip64_record(extras: &[u8], reloff: Option<u64>) -> Vec<u8> {
    let mut data = archive(&[("a", b"abc", false)]);
    let c = central(&data);
    let end = data.len() - 22;
    let mut record = b"PK\x06\x06".to_vec();
    record.extend((44 + extras.len() as u64).to_le_bytes());
    record.extend([0; 12]);
    record.extend(1_u64.to_le_bytes());
    record.extend(1_u64.to_le_bytes());
    record.extend(((end - c) as u64).to_le_bytes());
    record.extend((c as u64).to_le_bytes());
    record.extend(extras);
    record.extend(b"PK\x06\x07");
    record.extend([0; 4]);
    record.extend(reloff.unwrap_or(end as u64).to_le_bytes());
    record.extend(1_u32.to_le_bytes());
    data.splice(end..end, record);
    data
}

#[test]
fn zip64_locator_and_extensible_data_follow_cpython_31118() {
    assert_eq!(&read(zip64_record(b"12345678", None)).unwrap()[..], b"abc");
    for offset in [1 << 40, 3] {
        assert!(read(zip64_record(b"", Some(offset))).is_err(), "{offset}");
    }
    let mut data = zip64_record(b"", None);
    let record = data.windows(4).position(|w| w == b"PK\x06\x06").unwrap();
    data[record + 4..record + 12].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(read(data).is_err());
}

#[test]
fn declared_size_above_inflate_limit_loads_small_actual_output() {
    let mut data = archive(&[("a", b"abc", true)]);
    let c = central(&data);
    set32(&mut data, c + 24, 1 << 30);
    let mut limits = Limits::default();
    limits.max_inflate = 3;
    let zip = ZipArchive::open(data.into(), &meter(&limits)).unwrap();
    assert_eq!(&zip.read(0, &limits).unwrap()[..], b"abc");
}

#[test]
fn short_declared_output_still_checks_the_entire_read_window() {
    let mut data = archive(&[("a", b"ab", true)]);
    data[31] = 0;
    let c = central(&data);
    data.insert(c, 7);
    let c = c + 1;
    set32(&mut data, c + 20, 8);
    set32(&mut data, c + 24, 1);
    set32(&mut data, c + 16, unityrs_testkit::zip::crc32(b"a"));
    let end = data.len() - 22;
    set32(&mut data, end + 16, u32::try_from(c).unwrap());
    assert!(read(data).is_err());
}

#[test]
fn explicit_zip_path_opens_only_the_named_split_part() {
    let s = Scratch::new("explicit-zip-split");
    std::fs::write(
        s.path().join("x.split0"),
        archive(&[("first", b"abc", false)]),
    )
    .unwrap();
    std::fs::write(
        s.path().join("x.split1"),
        archive(&[("second", b"def", false)]),
    )
    .unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    env.load_zip_file(Source::Path(s.path().join("x.split0")))
        .unwrap();
    assert_eq!(
        env.files().map(|e| e.name.to_string()).collect::<Vec<_>>(),
        ["first"]
    );
}

#[test]
fn discarded_deflate_output_still_obeys_the_inflate_limit() {
    let mut data = archive(&[("a", b"ab", true)]);
    let c = central(&data);
    set32(&mut data, c + 24, 1);
    set32(&mut data, c + 16, unityrs_testkit::zip::crc32(b"a"));
    let mut limits = Limits::default();
    limits.max_inflate = 1;
    let zip = ZipArchive::open(data.into(), &meter(&limits)).unwrap();
    assert!(matches!(zip.read(0, &limits), Err(Error::Limit(e)) if e.kind() == LimitKind::Inflate));
}
