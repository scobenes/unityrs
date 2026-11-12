//! Replays the sprite oracle's fixture (`tests/fixtures/sprite.json`,
//! written by `tools/fixtures/sprite.py` from UnityPy 1.25.2, Pillow 9.5.0
//! and numpy on `OpenBLAS`'s Haswell kernels) without Python: each case's
//! Sprite (and atlas) read through its hand-built tree, its render data
//! chosen by `sprite::select_render_data`, its textures decoded, and the
//! sprite rendered, then compared with UnityPy's image, bit for bit (a large
//! one by its sha256), or its exception, by class and message; a
//! `LinAlgError` case with its Tier C image (`PARITY.md` section 15.2). The
//! meter sweeps hold every case to its unbounded answer or a refusal of the
//! meter's: its bytes, then its decode work (`docs/plans/extract.md` E59).

use std::sync::Arc;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::LimitKind;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed};
use unityrs_meshdecode::{MeshError, MeshHandler, Source, Version};
use unityrs_testkit::Rng;
use unityrs_texture::sprite::{
    Pptr, Sheet, SpriteData, SpriteError, SpriteFields, atlas_render_data, render,
    select_render_data,
};
use unityrs_texture::{Format, Image, Mode, decode};

/// The cases `sprite.py` records, how many are images, and how many are
/// Tier C.
const CASES: usize = 152;
const IMAGES: usize = 95;
const TIER_C: usize = 3;

/// An image past this many bytes is recorded by its sha256 (`sprite.py`).
const HEX_LIMIT: usize = 4096;

fn load() -> J {
    serde_json::from_str(include_str!("fixtures/sprite.json")).unwrap()
}

/// `sprite.py`'s sheet bytes: `SplitMix64` from the seed, little-endian.
fn splitmix(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    out.truncate(len);
    out
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

fn parse(rows: &J, data: Vec<u8>) -> Parsed {
    let rows = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let level = i32::try_from(r[0].as_i64().unwrap()).unwrap();
            NodeRow::new(level, r[1].as_str().unwrap(), r[2].as_str().unwrap())
                .with_meta_flag(r[3].as_i64().unwrap())
        })
        .collect();
    let list = NodeList::trusted(rows).unwrap();
    let tree = Tree::compile(Arc::new(list), &Limits::default()).unwrap();
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter: &meter,
    };
    walk::boost::read(&Bytes::from(data), &tree, &ctx).unwrap()
}

fn texture(t: &J, meter: &Arc<Meter>) -> Image {
    let (w, h) = (t["width"].as_u64().unwrap(), t["height"].as_u64().unwrap());
    let (format, bpp) = match t["format"].as_str().unwrap() {
        "RGBA32" => (Format::Rgba32, 4),
        "RGB24" => (Format::Rgb24, 3),
        other => panic!("{other}"),
    };
    let data = splitmix(
        t["seed"].as_u64().unwrap(),
        usize::try_from(w * h * bpp).unwrap(),
    );
    decode(
        format,
        &data,
        i32::try_from(w).unwrap(),
        i32::try_from(h).unwrap(),
        meter,
    )
    .unwrap()
}

fn image(im: &Image) -> J {
    let (mode, bpp) = match im.mode() {
        Mode::Rgb => ("RGB", 3),
        Mode::Rgba => ("RGBA", 4),
    };
    let size = [im.width(), im.height()];
    let pixels = im.pixels();
    if pixels.len() <= HEX_LIMIT {
        return serde_json::json!({ "mode": mode, "size": size, "hex": hex(pixels) });
    }
    let nonzero = pixels
        .chunks(bpp)
        .filter(|p| p.iter().any(|&b| b != 0))
        .count();
    let sha256 = hex(&Sha256::digest(pixels));
    serde_json::json!({ "mode": mode, "size": size, "sha256": sha256, "nonzero": nonzero })
}

fn error(e: &SpriteError) -> J {
    match e.python() {
        Some((class, message)) => serde_json::json!([class, message]),
        None => serde_json::json!(format!("rust: {e:?}")),
    }
}

/// The case's sprite rendered: its textures decoded on `textures`, the
/// sheet, the mesh and the render charged to `meter`.
fn rendered(
    body: &J,
    case: &J,
    textures: &Arc<Meter>,
    meter: &Arc<Meter>,
) -> Result<Image, SpriteError> {
    rendered_as(body, case, textures, meter, &|_| {}, &|_| {})
}

/// [`rendered`], with the Sprite's bytes changed by `patch` before they are
/// read and its render data by `data` before it renders: a case of
/// unityrs's alone, which no oracle run recorded.
fn rendered_as(
    body: &J,
    case: &J,
    textures: &Arc<Meter>,
    meter: &Arc<Meter>,
    patch: &dyn Fn(&mut Vec<u8>),
    data: &dyn Fn(&mut SpriteData),
) -> Result<Image, SpriteError> {
    let mut bytes = unhex(case["sprite"].as_str().unwrap());
    patch(&mut bytes);
    let sprite = parse(&body["trees"]["sprite"], bytes);
    let tree = case["atlas_tree"].as_str().unwrap_or("atlas");
    let atlas = case["atlas"]
        .as_str()
        .map(|h| parse(&body["trees"][tree], unhex(h)));
    let fields = SpriteFields::read(&sprite, meter)?;
    // The case's file: its atlas at path id 5, and nothing else.
    let atlas = atlas.as_ref();
    let deref = |p: Pptr| atlas.filter(|_| p.path_id == 5);
    let file_atlases = atlas.map(|a| {
        let name = a.get(&[b"m_Name"]).and_then(|v| v.as_bytes()).unwrap();
        (name, move || Some(a))
    });
    let rd = select_render_data(&fields, deref, file_atlases, meter)?;
    let tex = |id: i64| texture(&case["textures"][id.to_string()], textures);
    let main = tex(rd.texture.path_id);
    let alpha = rd
        .alpha_texture
        .filter(Pptr::is_set)
        .map(|p| tex(p.path_id));
    let sheet = Sheet::new(&main, alpha.as_ref(), meter)?;
    let v = &case["version"];
    let n = |i: usize| u16::try_from(v[i].as_u64().unwrap()).unwrap();
    let b = |i: usize| u8::try_from(v[i].as_u64().unwrap()).unwrap();
    let version = Version::new(n(0), n(1), n(2), b(3), b(4));
    let mesh = || MeshHandler::new(Source::sprite(&sprite), version).process(None, meter);
    let mut sprite_data = fields.data(&rd);
    data(&mut sprite_data);
    render(&sheet, &sprite_data, mesh, meter)
}

#[test]
fn every_sprite_renders_as_unitypy_s() {
    let body = load();
    assert_eq!(body["openblas_core"], "Haswell");
    let cases = body["cases"].as_array().unwrap();
    assert_eq!(cases.len(), CASES);
    let meter = Meter::unlimited();
    let (mut images, mut tier_c, mut bad) = (0, 0, Vec::new());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let have = rendered(&body, case, &meter, &meter);
        // A singular system's case answers as UnityPy does when the
        // triangle copies nothing.
        let want = if case["err"][0] == "LinAlgError" {
            tier_c += 1;
            &case["tier_c"]
        } else {
            case
        };
        let ok = if want["image"].is_null() {
            have.as_ref().err().is_some_and(|e| error(e) == want["err"])
        } else {
            images += 1;
            have.as_ref().is_ok_and(|im| image(im) == want["image"])
        };
        if !ok {
            let have = match &have {
                Ok(im) => image(im),
                Err(e) => error(e),
            };
            let want = if want["image"].is_null() {
                &want["err"]
            } else {
                &want["image"]
            };
            bad.push(format!("{name}\n  want {want}\n  have {have}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {CASES} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert_eq!((images, tier_c), (IMAGES, TIER_C));
}

/// Every buffer is charged before it is allocated: under a meter of any
/// size, a case renders as it does unbounded or is refused by the meter,
/// whose charge is returned when the image is dropped; the image holds its
/// own charge alone.
#[test]
fn every_buffer_is_charged_and_returned() {
    let body = load();
    let textures = Meter::unlimited();
    let mut refused = 0;
    for case in body["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let unbounded = rendered(&body, case, &textures, &Meter::unlimited())
            .map(|im| image(&im))
            .map_err(|e| error(&e));
        for limit in [0, 1, 16, 64, 100, 200, 400, 1000, 4000] {
            let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
            match rendered(&body, case, &textures, &meter) {
                Ok(im) => {
                    assert_eq!(meter.used(), im.charge().bytes(), "{name} at {limit}");
                    assert_eq!(Ok(image(&im)), unbounded, "{name} at {limit}");
                }
                Err(
                    SpriteError::Charge(ChargeError::Limit(_))
                    | SpriteError::Mesh(MeshError::Charge(ChargeError::Limit(_))),
                ) => refused += 1,
                Err(e) => assert_eq!(Err(error(&e)), unbounded, "{name} at {limit}"),
            }
            assert_eq!(meter.used(), 0, "{name} at {limit}");
        }
    }
    assert!(refused > 100, "{refused} refusals");
}

/// A refusal of the meter's decode work, the renderer's or the mesh
/// handler's.
fn decode_work_refused(e: &SpriteError) -> bool {
    let work =
        |c: &ChargeError| matches!(c, ChargeError::Limit(l) if l.kind == LimitKind::DecodeWork);
    match e {
        SpriteError::Charge(c) | SpriteError::Mesh(MeshError::Charge(c)) => work(c),
        _ => false,
    }
}

/// A decode-work allowance changes no answer: under any allowance, a case
/// renders as it does unbounded or is refused for its work, and one
/// allowance, `ENOUGH`, renders every case. That a loop counts its work at
/// all, it cannot show (a loop that counts nothing passes it): the rows of
/// an empty, tall canvas are pinned by
/// `an_empty_tall_canvas_s_rows_are_work`, and the lookups' scans by
/// `the_atlas_lookup_s_scans_are_work` and
/// `a_name_s_bytes_and_a_map_s_fields_are_work`.
#[test]
fn every_loop_s_work_is_counted() {
    const ENOUGH: u64 = 1 << 24;
    let body = load();
    let textures = Meter::unlimited();
    let mut refused = 0;
    for case in body["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let unbounded = rendered(&body, case, &textures, &Meter::unlimited())
            .map(|im| image(&im))
            .map_err(|e| error(&e));
        for work in [
            0,
            1,
            4,
            16,
            64,
            256,
            1024,
            4096,
            1 << 14,
            1 << 16,
            1 << 20,
            ENOUGH,
        ] {
            let meter =
                Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input);
            match rendered(&body, case, &textures, &meter) {
                Ok(im) => assert_eq!(Ok(image(&im)), unbounded, "{name} at {work}"),
                Err(e) if decode_work_refused(&e) => {
                    assert!(work < ENOUGH, "{name} needs more than {ENOUGH}: {e}");
                    refused += 1;
                }
                Err(e) => assert_eq!(Err(error(&e)), unbounded, "{name} at {work}"),
            }
        }
    }
    assert!(refused > 300, "{refused} refusals");
}

/// A Sprite: `m_PixelsToUnits`, an `m_RD` whose `settingsRaw` is of type
/// `settings`, an `m_RenderDataKey` whose id is of type `id`, and `tags`
/// atlas tags of type `tag` (each empty, or 0), each read from the bytes
/// those types take.
fn sprite_of(settings: &str, id: &str, tag: &str, tags: u32) -> Parsed {
    let rows = serde_json::json!([
        [0, "Sprite", "Base", 0],
        [1, "float", "m_PixelsToUnits", 0],
        [1, "SpriteRenderData", "m_RD", 0],
        [2, "PPtr<Texture2D>", "texture", 0],
        [3, "int", "m_FileID", 0],
        [3, "SInt64", "m_PathID", 0],
        [2, "Rectf", "textureRect", 0],
        [3, "float", "x", 0],
        [3, "float", "y", 0],
        [3, "float", "width", 0],
        [3, "float", "height", 0],
        [2, settings, "settingsRaw", 0],
        [1, "pair", "m_RenderDataKey", 0],
        [2, "GUID", "first", 0],
        [3, "unsigned int", "data[0]", 0],
        [3, "unsigned int", "data[1]", 0],
        [3, "unsigned int", "data[2]", 0],
        [3, "unsigned int", "data[3]", 0],
        [2, id, "second", 0],
        [1, "vector", "m_AtlasTags", 0],
        [2, "Array", "Array", 0],
        [3, "int", "size", 0],
        [3, tag, "data", 0],
    ]);
    let mut data = 100.0_f32.to_le_bytes().to_vec();
    data.extend([0; 12]); // the texture pointer
    for v in [0.0_f32, 0.0, 1.0, 1.0] {
        data.extend(v.to_le_bytes());
    }
    data.extend([0; 4]); // settingsRaw, a u32 or an f32
    data.extend([0; 16]); // the GUID
    data.extend(if id == "SInt64" { &[0; 8][..] } else { &[0; 4] });
    data.extend(tags.to_le_bytes());
    for _ in 0..tags {
        data.extend([0; 4]); // an empty string, or an int
    }
    parse(&rows, data)
}

/// A field of a type no typetree gives it is `SpriteError::Malformed`,
/// naming it, before anything renders (`PARITY.md` section 19.3, Tier U);
/// an atlas the caller cannot resolve is `AtlasUnresolved`.
#[test]
fn a_field_of_another_type_is_malformed() {
    let malformed = |s: &Parsed| match SpriteFields::read(s, &Meter::unlimited()) {
        Err(SpriteError::Malformed(what)) => what,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        malformed(&sprite_of("float", "SInt64", "string", 1)),
        "m_RD"
    );
    assert_eq!(
        malformed(&sprite_of("unsigned int", "float", "string", 1)),
        "m_RenderDataKey"
    );
    assert_eq!(
        malformed(&sprite_of("unsigned int", "SInt64", "int", 1)),
        "m_AtlasTags"
    );

    let sprite = sprite_of("unsigned int", "SInt64", "string", 1);
    let fields = SpriteFields::read(&sprite, &Meter::unlimited()).unwrap();
    assert_eq!(fields.atlas_tag, Some(&b""[..]));
    let atlas = parse(
        &serde_json::json!([
            [0, "SpriteAtlas", "Base", 0],
            [1, "int", "m_RenderDataMap", 0]
        ]),
        vec![0; 4],
    );
    let none = |_: Pptr| None;
    let meter = Meter::unlimited();
    assert!(matches!(
        select_render_data(&fields, none, [(&b""[..], || Some(&atlas))], &meter),
        Err(SpriteError::Malformed("m_RenderDataMap"))
    ));
    assert!(matches!(
        select_render_data(&fields, none, [(&b""[..], || None)], &meter),
        Err(SpriteError::AtlasUnresolved)
    ));
    // Another tag's atlas is skipped, and with none found the Sprite's own
    // render data is used.
    assert_eq!(
        select_render_data(&fields, none, [(&b"other"[..], || Some(&atlas))], &meter).unwrap(),
        fields.rd
    );
    // A set `m_SpriteAtlas` the caller cannot resolve is `AtlasUnresolved`,
    // whatever the file's atlases (the pointer path).
    let pointed = SpriteFields {
        sprite_atlas: Some(Pptr {
            file_id: 0,
            path_id: 5,
        }),
        ..fields.clone()
    };
    let mut asked = None;
    let deref = |p: Pptr| {
        asked = Some(p);
        None
    };
    assert!(matches!(
        select_render_data(&pointed, deref, [(&b""[..], || Some(&atlas))], &meter),
        Err(SpriteError::AtlasUnresolved)
    ));
    assert_eq!(asked.map(|p| p.path_id), Some(5));
}

/// An atlas whose `m_RenderDataMap` holds `entries` entries, none of them
/// the key `sprite_of` gives (its id is 1, the Sprite's 0).
fn atlas_of(entries: u32) -> Parsed {
    let rows = serde_json::json!([
        [0, "SpriteAtlas", "Base", 0],
        [1, "map", "m_RenderDataMap", 0],
        [2, "Array", "Array", 0],
        [3, "int", "size", 0],
        [3, "pair", "data", 0],
        [4, "pair", "first", 0],
        [5, "GUID", "first", 0],
        [6, "unsigned int", "data[0]", 0],
        [6, "unsigned int", "data[1]", 0],
        [6, "unsigned int", "data[2]", 0],
        [6, "unsigned int", "data[3]", 0],
        [5, "SInt64", "second", 0],
        [4, "int", "second", 0],
    ]);
    let mut data = entries.to_le_bytes().to_vec();
    for _ in 0..entries {
        data.extend([0; 16]);
        data.extend(1_i64.to_le_bytes());
        data.extend([0; 4]);
    }
    parse(&rows, data)
}

/// A meter of `work` units of decode work, its bytes unbounded.
fn work_meter(work: u64) -> Arc<Meter> {
    Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input)
}

/// The decode work `f` counts, exactly: its answer under that allowance is
/// its answer unbounded, and one unit less refuses it.
fn work_of<T: PartialEq + std::fmt::Debug>(
    f: impl Fn(&Arc<Meter>) -> Result<T, SpriteError>,
) -> u64 {
    let unbounded = work_meter(u64::MAX);
    let answer = f(&unbounded).map_err(|e| error(&e));
    let work = unbounded.decoded();
    assert_eq!(f(&work_meter(work)).map_err(|e| error(&e)), answer);
    let short = f(&work_meter(work - 1));
    assert!(short.as_ref().is_err_and(decode_work_refused), "{short:?}");
    work
}

/// Each atlas compared, each map entry read and each field a lookup walks
/// is decode work, counted before it is read, and a name's comparison by
/// the bytes it may read: a scan of a file's atlases, or of one atlas's
/// map, counts exactly that, and is refused one unit short of it.
#[test]
fn the_atlas_lookup_s_scans_are_work() {
    const LONG: u64 = 10_000;
    let sprite = sprite_of("unsigned int", "SInt64", "string", 1);
    let fields = SpriteFields::read(&sprite, &Meter::unlimited()).unwrap();
    let atlas = atlas_of(u32::try_from(LONG).unwrap());
    let none = |_: Pptr| None;

    // One atlas of LONG entries, none the key: its name against the tag
    // (both empty, 1), the atlas's fields walked to its map (the map's own
    // name, 1 + 15), then each entry (1) and its key's GUID words, each
    // looked up by name past those before it (`data[0]` to `data[3]`,
    // 1 + 7 a name: 8 + 16 + 24 + 32).
    let one = |m: &Arc<Meter>| select_render_data(&fields, none, [(&b""[..], || Some(&atlas))], m);
    assert!(matches!(
        one(&Meter::unlimited()),
        Err(SpriteError::NoAtlasData)
    ));
    assert_eq!(work_of(one), 1 + 16 + LONG * (1 + 80));

    // LONG atlases of another name, of another length than the tag's: 1
    // each, and then the Sprite's own render data.
    let others = |m: &Arc<Meter>| {
        select_render_data(
            &fields,
            none,
            (0..LONG).map(|_| (&b"other"[..], || Some(&atlas))),
            m,
        )
    };
    assert_eq!(others(&Meter::unlimited()).unwrap(), fields.rd);
    assert_eq!(work_of(others), LONG);
}

/// An atlas whose root holds `fields` int fields with names as long as
/// `m_RenderDataMap`, and all but its last 3 bytes the same
/// (`m_RenderData000` on), then an empty `m_RenderDataMap`.
fn atlas_after(fields: u32) -> Parsed {
    assert!(fields <= 1000);
    let mut rows = vec![serde_json::json!([0, "SpriteAtlas", "Base", 0])];
    for i in 0..fields {
        rows.push(serde_json::json!([
            1,
            "int",
            format!("m_RenderData{i:03}"),
            0
        ]));
    }
    for row in serde_json::json!([
        [1, "map", "m_RenderDataMap", 0],
        [2, "Array", "Array", 0],
        [3, "int", "size", 0],
        [3, "pair", "data", 0],
        [4, "pair", "first", 0],
        [5, "GUID", "first", 0],
        [6, "unsigned int", "data[0]", 0],
        [6, "unsigned int", "data[1]", 0],
        [6, "unsigned int", "data[2]", 0],
        [6, "unsigned int", "data[3]", 0],
        [5, "SInt64", "second", 0],
        [4, "int", "second", 0],
    ])
    .as_array()
    .unwrap()
    {
        rows.push(row.clone());
    }
    let mut data = vec![0; 4 * fields as usize];
    data.extend(0_u32.to_le_bytes());
    parse(&J::Array(rows), data)
}

/// A name compared is as much work as the bytes the comparison may read,
/// so a long tag against atlas names of its length, all but their last
/// byte its own, counts each byte; many fields before an empty map count
/// each field walked and its name's bytes; and each tag a Sprite holds
/// counts its check.
#[test]
fn a_name_s_bytes_and_a_map_s_fields_are_work() {
    const N: usize = 100_000;
    const ATLASES: u64 = 10;
    const LONG: u32 = 10_000;
    let sprite = sprite_of("unsigned int", "SInt64", "string", 1);
    let base = SpriteFields::read(&sprite, &Meter::unlimited()).unwrap();
    let empty = atlas_of(0);

    // A tag of N bytes against ATLASES names of N bytes that differ from it
    // at the last: 1 + N each, the Sprite's own render data once all are
    // compared. Against names a byte shorter, no byte is read: 1 each.
    let tag = vec![b'a'; N];
    let mut name = tag.clone();
    *name.last_mut().unwrap() = b'b';
    let fields = SpriteFields {
        atlas_tag: Some(&tag[..]),
        ..base.clone()
    };
    let scan = |name: &[u8], m: &Arc<Meter>| {
        select_render_data(
            &fields,
            |_: Pptr| None,
            (0..ATLASES).map(|_| (name, || Some(&empty))),
            m,
        )
    };
    assert_eq!(scan(&name, &Meter::unlimited()).unwrap(), fields.rd);
    assert_eq!(work_of(|m| scan(&name, m)), ATLASES * (1 + N as u64));
    assert_eq!(work_of(|m| scan(&name[1..], m)), ATLASES);

    // 1000 fields before an empty map, each name as long as the map's (1 +
    // 15 each), then the map's own: no entry, `NoAtlasData`.
    let atlas = atlas_after(1000);
    let lookup = |m: &Arc<Meter>| atlas_render_data(&atlas, base.render_data_key.as_ref(), m);
    assert!(matches!(
        lookup(&Meter::unlimited()),
        Err(SpriteError::NoAtlasData)
    ));
    assert_eq!(work_of(lookup), 1000 * 16 + 16);

    // Each of a Sprite's tags is checked, and counted: LONG tags count
    // LONG - 1 more than one does.
    let many = sprite_of("unsigned int", "SInt64", "string", LONG);
    let read = |s: &Parsed| work_of(|m: &Arc<Meter>| SpriteFields::read(s, m).map(|f| f.rd));
    assert_eq!(read(&many) - read(&sprite), u64::from(LONG) - 1);
}

/// Bytes `from` replaced by `to`, at their one place in `bytes`.
fn replace_once(bytes: &mut [u8], from: &[u8], to: &[u8]) {
    let at: Vec<usize> = (0..=bytes.len() - from.len())
        .filter(|&i| bytes[i..i + from.len()] == *from)
        .collect();
    assert_eq!(at.len(), 1, "{from:02x?}");
    bytes[at[0]..at[0] + to.len()].copy_from_slice(to);
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// One of `an_empty_tall_canvas_s_rows_are_work`'s renders: its label, its
/// canvas's rows, the passes over them it counts, the size of its image,
/// its case, and its changes to the Sprite's bytes and to its render data.
struct Run {
    label: String,
    rows: u64,
    passes: u64,
    size: (u32, u32),
    case: J,
    patch: Patch,
    data: Box<dyn Fn(&mut SpriteData)>,
}

/// A change to a Sprite's bytes.
type Patch = Box<dyn Fn(&mut Vec<u8>)>;

/// An empty canvas's rows are work, counted before they are stepped
/// through, each pass of the render over them: a crop 0 wide and 2e9 rows
/// high, unpacked (made, copied, flipped at the end: 3 passes), flipped
/// either way (4), turned half way (both flips, 5), and masked (the crop's
/// 2, the mask's canvas made and stepped, the flip: 5); one 5e8 high
/// turned a quarter (made, copied, and the turn's reading of it: 3; the
/// turned canvas is 5e8 wide and 0 high); a mesh canvas 0 wide and 6e8
/// high (made, flipped: 2; its triangle's system is singular, a column of
/// zeros, so `copy_triangle` stops before its own count), and the same
/// canvas with a translated triangle, whose crop `copy_unchanged` counts
/// (3). Each renders unbounded to an image of its size, counting at least
/// its passes' rows and less than a pass more (so a pass's count removed,
/// or one doubled, fails), is refused under an allowance of 0 and under
/// one of half its rows (which its bytes, a few hundred, would fit), and
/// gives the same image under one its work fits. No oracle run records
/// them: Pillow's own row-pointer array for such an image is 8 bytes a row
/// (`PARITY.md` section 19.3).
#[test]
fn an_empty_tall_canvas_s_rows_are_work() {
    let body = load();
    let textures = Meter::unlimited();
    for run in &tall_runs(&body) {
        let Run {
            label,
            rows,
            passes,
            ..
        } = run;
        let at = |m: &Arc<Meter>| {
            rendered_as(&body, &run.case, &textures, m, &*run.patch, &*run.data)
                .map(|im| (im.width(), im.height()))
        };
        let unbounded = work_meter(u64::MAX);
        let answer = at(&unbounded);
        assert_eq!(answer.as_ref().ok(), Some(&run.size), "{label}: {answer:?}");
        let work = unbounded.decoded();
        assert!(
            (passes * rows..(passes + 1) * rows).contains(&work),
            "{label}: {work}, not {passes} passes of {rows} rows"
        );
        for allowance in [0, rows / 2, passes * rows - 1] {
            let r = at(&work_meter(allowance));
            assert!(
                r.as_ref().is_err_and(decode_work_refused),
                "{label} at {allowance}: {r:?}"
            );
        }
        assert_eq!(at(&work_meter(work)).ok(), Some(run.size), "{label}");
    }
}

/// `an_empty_tall_canvas_s_rows_are_work`'s renders (its docs).
fn tall_runs(body: &J) -> Vec<Run> {
    let case = |name: &str| {
        body["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()
            .clone()
    };
    // settingsRaw: packed (bit 0), rectangle packing (bit 1), the rotation
    // in bits 2 to 5.
    let packed = |rotation: u64| 0b11 | rotation << 2;
    let mut runs: Vec<Run> = Vec::new();
    // A quarter turn of a crop 0 x 2e9 would be 2e9 wide, past Pillow's
    // width cap, which UnityPy meets before the crop's rows are stepped
    // through; that one is 0 x 5e8, which turns to 5e8 x 0.
    for (label, rows, passes, settings) in [
        ("unpacked", 2_000_000_000, 3, 0b10),
        ("flipped left-right", 2_000_000_000, 4, packed(1)),
        ("flipped top-bottom", 2_000_000_000, 4, packed(2)),
        ("turned half way", 2_000_000_000, 5, packed(3)),
        ("turned a quarter", 500_000_000, 3, packed(4)),
    ] {
        let h = u32::try_from(rows).unwrap();
        runs.push(Run {
            label: format!("a 0 x {rows} crop, {label}"),
            rows,
            passes,
            size: if settings == packed(4) { (h, 0) } else { (0, h) },
            case: case("crop_inside"),
            patch: Box::new(|_| {}),
            #[allow(clippy::cast_precision_loss)] // exact: below 2^53
            data: Box::new(move |d| {
                d.texture_rect = [0.0, 0.0, 0.0, rows as f64];
                d.settings_raw = settings;
            }),
        });
    }
    runs.push(Run {
        label: "a 0 x 2e9 crop, masked".into(),
        rows: 2_000_000_000,
        passes: 5,
        size: (0, 2_000_000_000),
        case: case("mask_seeded_0"),
        patch: Box::new(|_| {}),
        data: Box::new(move |d| d.texture_rect = [0.0, 0.0, 0.0, 2.0e9]),
    });
    // mesh_canvas_width_cap's vertices, (0, 0), (6e8, 0) and (3e8, 0), as
    // (0, 0), (0, 6e8) and (0, 3e8): a canvas 0 wide, 6e8 high.
    let upright = |b: &mut Vec<u8>| {
        for y in [6.0e8, 3.0e8] {
            replace_once(b, &f32s(&[y, 0.0, 0.0]), &f32s(&[0.0, y, 0.0]));
        }
    };
    runs.push(Run {
        label: "a mesh canvas 0 x 6e8".into(),
        rows: 600_000_000,
        passes: 2,
        size: (0, 600_000_000),
        case: case("mesh_canvas_width_cap"),
        patch: Box::new(upright),
        data: Box::new(|_| {}),
    });
    // And its uv corners, (0.1, 0.1), (0.2, 0.1) and (0.1, 0.2) of the
    // 16 x 16 sheet, as (0, 0), (0, 6e8 / 16) and (0, 3e8 / 16), each exact
    // in a float32: source corners (0, 0), (0, 6e8) and (0, 3e8), the
    // destination's, a translation.
    runs.push(Run {
        label: "a mesh canvas 0 x 6e8, its triangle translated".into(),
        rows: 600_000_000,
        passes: 3,
        size: (0, 600_000_000),
        case: case("mesh_canvas_width_cap"),
        patch: Box::new(move |b| {
            upright(b);
            for (from, to) in [
                ([0.1, 0.1], [0.0, 0.0]),
                ([0.2, 0.1], [0.0, 3.75e7]),
                ([0.1, 0.2], [0.0, 1.875e7]),
            ] {
                replace_once(b, &f32s(&from), &f32s(&to));
            }
        }),
        data: Box::new(|_| {}),
    });
    runs
}
