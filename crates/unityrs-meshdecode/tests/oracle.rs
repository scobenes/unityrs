//! Replays the oracle's fixtures (`tests/fixtures/`, written by
//! `tools/fixtures/meshdecode.py` from UnityPy 1.25.2) without Python:
//! every format's struct code and size by version, the enums' members,
//! UnityPyBoost's gather on its own, and every handler case through the
//! core's typetree read, `MeshHandler::process` and `get_triangles`, each
//! array bit for bit, each exception by type and message, and each part a
//! later phase ports where UnityPy begins it; each call made twice, the
//! second answering as the first, each array read through the public
//! accessors (`len`, `tuple_len`, `component`). Where `PARITY.md` section
//! 19.1's Tier C rows on the compressed mesh and on the normalised formats
//! show, the case's answer is the corrected one the script records beside
//! UnityPy's ([`DIVERGES`] and [`NORMALISED`], the expected-divergence
//! mask), and a packed item past 32 bits is refused ([`REFUSED`]);
//! `tests/compressed.rs` and `tests/normalised.rs` pin what each divergence
//! changes. The same cases on UnityPy's TPK trees when `meshdecode.py
//! --tpk` hands them over on standard input, and a streamed Mesh read
//! through a real Environment.

use std::sync::{Arc, Mutex};

use serde_json::{Value as J, json};
use unityrs_core::env::{EnvOptions, Environment, Source as EnvSource};
use unityrs_core::files::File;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed, RawStr, SerializedFile};
use unityrs_meshdecode::{
    Component, FileResources, Gather, GfxPrimitiveType, MeshError, MeshHandler, MeshTopology,
    Processed, Resources, Source, SubMeshTriangles, Triangles, Unsupported, Version, VertexArray,
    VertexChannelFormat, VertexFormat, VertexFormat2017, get_channel_component_size,
    get_channel_dtype, unpack_vertexdata,
};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::env;
use unityrs_testkit::serialized::{self, SerializedSpec, one_object};

/// The cases `meshdecode.py` records.
const FORMAT_VERSIONS: usize = 10;
const UNPACK_CASES: usize = 251;
const PROCESS_CASES: usize = 526;

/// The expected-divergence mask (`PARITY.md` section 19.1): each case whose
/// answer UnityPy's three Tier C rows on `decompress_compressed_mesh`
/// change (the UV offset, the fourth bone weight, `normalize`'s square),
/// and what they change, the arrays (`outcome` where the corrected answer
/// is another outcome); `tests/compressed.rs` pins each value changed. The
/// script records the corrected answer beside UnityPy's
/// (`tools/fixtures/meshdecode.py`'s `FIXES` and `NORMALIZE_FIX`).
const DIVERGES: &[(&str, &[&str])] = &[
    ("compressed_values", &["m_BoneWeights", "m_UV2"]),
    ("compressed_uv_dims", &["m_UV5", "m_UV7"]),
    ("compressed_uv_short", &["outcome"]),
    ("compressed_uv_unaligned", &["m_UV2"]),
    ("compressed_normals_pow", &["m_Normals"]),
    ("compressed_skin_weights", &["m_BoneWeights"]),
];

/// The rest of the mask: the cases whose answer `PARITY.md` section 19.1's
/// UNorm/SNorm row changes, a Mesh's channel in a `UNorm`, `SNorm` or byte
/// colour format, by name: a name, or one with a `*` standing for any run
/// of characters, and the arrays the normalised channel fills. A Sprite's
/// case is never here (`tests/normalised.rs`).
const NORMALISED: &[(&str, &[&str])] = &[
    ("format_5.6.7f1_2_d*", &["m_Vertices"]),
    ("format_2016.4.0f1_2_d*", &["m_Vertices"]),
    ("format_2017.1.0f3_2_d*", &["m_Vertices"]),
    ("format_2017.1.0f3_3_d*", &["m_Vertices"]),
    ("format_2017.1.0f3_4_d*", &["m_Vertices"]),
    ("format_2017.1.0f3_5_d*", &["m_Vertices"]),
    ("format_2017.1.0f3_6_d*", &["m_Vertices"]),
    ("format_2018.4.0f1_2_d*", &["m_Vertices"]),
    ("format_2018.4.0f1_3_d*", &["m_Vertices"]),
    ("format_2018.4.0f1_4_d*", &["m_Vertices"]),
    ("format_2018.4.0f1_5_d*", &["m_Vertices"]),
    ("format_2018.4.0f1_6_d*", &["m_Vertices"]),
    ("format_2019.4.0f1_2_d*", &["m_Vertices"]),
    ("format_2019.4.0f1_3_d*", &["m_Vertices"]),
    ("format_2019.4.0f1_4_d*", &["m_Vertices"]),
    ("format_2019.4.0f1_5_d*", &["m_Vertices"]),
    ("format_6000.0.0f1_2_d*", &["m_Vertices"]),
    ("format_6000.0.0f1_3_d*", &["m_Vertices"]),
    ("format_6000.0.0f1_4_d*", &["m_Vertices"]),
    ("format_6000.0.0f1_5_d*", &["m_Vertices"]),
    ("bits_endian_*", &["m_Colors"]),
    ("streams_three", &["m_Colors"]),
    ("colour_5.6.7f1_d*", &["m_Colors"]),
    ("colour_2017.4.0f1_d*", &["m_Colors"]),
    ("colour_2018.1.0f2_d*", &["m_Tangents"]),
    ("colour_past_end", &["m_Colors"]),
    ("own_all", &["m_Colors"]),
    ("own_mask_skips", &["m_Colors"]),
    ("normalised_*_positions", &["m_Vertices"]),
    ("normalised_*_positions_big", &["m_Vertices"]),
    ("normalised_*_colour", &["m_Colors"]),
    ("normalised_*_uv0", &["m_UV0"]),
    ("normalised_2019.4.0f1_skin", &["m_BoneWeights"]),
];

/// Whether `name` is `pattern`, a `*` in it standing for any run of
/// characters.
fn matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        Some((head, tail)) => {
            name.len() >= head.len() + tail.len() && name.starts_with(head) && name.ends_with(tail)
        }
        None => pattern == name,
    }
}

/// The cases the crate refuses where UnityPy answers, by the refusal's
/// detail (`PARITY.md` section 15.3's `unpack_ints` row): an item past 32
/// bits.
const REFUSED: &[(&str, &str)] = &[("compressed_ints_33", "a packed item wider than 32 bits")];

/// A fixture, compiled in: this file opens no file at all, so the TPK
/// replay reads only its standard input, and a source scan
/// (`tools/parity/tests/test_meshdecode_tpk.py`) holds it to that.
fn load(name: &str) -> J {
    let text = match name {
        "formats.json" => include_str!("fixtures/formats.json"),
        "unpack.json" => include_str!("fixtures/unpack.json"),
        "process.json" => include_str!("fixtures/process.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn uint<T: TryFrom<u64>>(v: &J) -> T
where
    T::Error: std::fmt::Debug,
{
    T::try_from(v.as_u64().unwrap()).unwrap()
}

fn version(v: &J) -> Version {
    Version::new(
        uint(&v[0]),
        uint(&v[1]),
        uint(&v[2]),
        uint(&v[3]),
        uint(&v[4]),
    )
}

/// An exception as the fixtures write it, or the crate's own refusal.
fn error(e: &MeshError) -> J {
    if let MeshError::Unsupported(u) = e {
        let kind = match u {
            Unsupported::FixedChannels => "fixed_channels".to_owned(),
            Unsupported::MeshArray(f) => format!("mesh_array:{f}"),
            Unsupported::SpriteVertices => "sprite_vertices".to_owned(),
            other => panic!("{other:?}"),
        };
        return json!({ "p2a": kind });
    }
    match e.python() {
        Some((class, message)) => json!({ "err": [class, message] }),
        None => json!({ "rust": format!("{e:?}") }),
    }
}

#[test]
fn every_format_reads_as_the_version_s_table_says() {
    let body = load("formats.json");
    let cases = body["cases"].as_array().unwrap();
    assert_eq!(cases.len(), FORMAT_VERSIONS);
    let mut bad = Vec::new();
    for c in cases {
        let v = version(&c["version"]);
        for (fmt, want) in c["formats"].as_array().unwrap().iter().enumerate() {
            let fmt = u8::try_from(fmt).unwrap();
            let have = match (
                get_channel_dtype(v, fmt),
                get_channel_component_size(v, fmt),
            ) {
                (Ok(code), Ok(size)) => json!({ "code": code.code().to_string(), "size": size }),
                (Err(e), Err(_)) => error(&e),
                other => panic!("{other:?}"),
            };
            if &have != want {
                bad.push(format!("{} {fmt}: want {want}, have {have}", c["version"]));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn the_enums_are_unitypy_s() {
    fn members<T: std::fmt::Debug + Copy>(all: &[T], value: impl Fn(T) -> i64) -> J {
        J::Array(
            all.iter()
                .map(|&m| json!([format!("{m:?}"), value(m)]))
                .collect(),
        )
    }
    let body = load("formats.json");
    let enums = &body["enums"];
    assert_eq!(
        enums["VertexChannelFormat"],
        members(&VertexChannelFormat::ALL, |m| m as i64)
    );
    assert_eq!(
        enums["VertexFormat2017"],
        members(&VertexFormat2017::ALL, |m| m as i64)
    );
    assert_eq!(
        enums["VertexFormat"],
        members(&VertexFormat::ALL, |m| m as i64)
    );
    assert_eq!(
        enums["MeshTopology"],
        members(&MeshTopology::ALL, MeshTopology::value)
    );
    assert_eq!(
        enums["GfxPrimitiveType"],
        members(&GfxPrimitiveType::ALL, GfxPrimitiveType::value)
    );
}

#[test]
fn the_gather_is_unitypyboost_s() {
    let body = load("unpack.json");
    let cases = body["cases"].as_array().unwrap();
    assert_eq!(cases.len(), UNPACK_CASES);
    let meter = Meter::unlimited();
    let mut bad = Vec::new();
    for c in cases {
        let data = unhex(c["data"].as_str().unwrap());
        let args = Gather {
            component_byte_size: uint(&c["cbs"]),
            vertex_count: uint(&c["vc"]),
            stream_offset: uint(&c["soff"]),
            stream_stride: uint(&c["stride"]),
            channel_offset: uint(&c["choff"]),
            channel_dimension: uint(&c["dim"]),
            swap: c["swap"].as_bool().unwrap(),
        };
        let have = match unpack_vertexdata(&data, &args, &meter) {
            Ok(b) => json!({ "ok": hex(b.as_slice()) }),
            Err(MeshError::WrappedAccess) => json!({ "wrapped": true }),
            Err(e) => error(&e),
        };
        let want: J = ["ok", "err", "wrapped"]
            .iter()
            .filter_map(|k| c.get(*k).map(|v| ((*k).to_owned(), v.clone())))
            .collect::<serde_json::Map<_, _>>()
            .into();
        if have != want {
            bad.push(format!("{args:?}: want {want}, have {have}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// `get_resource_data` as the script mocks it: the case's bytes, the call
/// recorded.
struct Resource {
    bytes: Bytes,
    calls: Mutex<Vec<J>>,
}

impl Resources for Resource {
    fn resource(
        &self,
        path: &RawStr,
        offset: u64,
        size: u64,
    ) -> Result<Bytes, unityrs_core::Error> {
        let path = String::from_utf8(path.as_bytes().to_vec()).unwrap();
        self.calls.lock().unwrap().push(json!([path, offset, size]));
        Ok(self.bytes.clone())
    }
}

/// Whether a failure may say what the tree held: only with
/// `UNITYRS_MESHDECODE_TPK_VERBOSE` set, as a run on the TPK's trees must
/// print nothing of them into CI's public log.
fn verbose() -> bool {
    std::env::var_os("UNITYRS_MESHDECODE_TPK_VERBOSE").is_some()
}

/// `stage` failed, with the core's error only when [`verbose`].
fn failed(stage: &str, e: &dyn std::fmt::Debug) -> String {
    if verbose() {
        format!("{stage} failed: {e:?}")
    } else {
        format!("{stage} failed")
    }
}

/// The case's value read through its tree; a failure names the stage.
fn parse(rows: &J, data: Vec<u8>) -> Result<Parsed, String> {
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
    let list = NodeList::trusted(rows).map_err(|e| failed("the node list", &e))?;
    let tree =
        Tree::compile(Arc::new(list), &Limits::default()).map_err(|e| failed("the tree", &e))?;
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter: &meter,
    };
    walk::boost::read(&Bytes::from(data), &tree, &ctx).map_err(|e| failed("the read", &e))
}

fn component(c: Component) -> J {
    match c {
        Component::Float(x) => json!({ "f": format!("{:016x}", x.to_bits()) }),
        Component::Int(i) => json!(i),
        other => panic!("{other:?}"),
    }
}

/// Tuple `t` of `a`, through the public accessors: the components it
/// holds ([`VertexArray::tuple_len`]), each by [`VertexArray::component`].
fn tuple(a: &VertexArray, t: usize) -> Vec<J> {
    let n = a.tuple_len(t).expect("a tuple within len()");
    (0..n)
        .map(|c| component(a.component(t, c).expect("a component within tuple_len()")))
        .collect()
}

/// `m_BindPose` as UnityPy's `reshape(shape=(4, 4))` gives it: four rows
/// a matrix, the last matrix's cut short, then empty.
fn matrices(a: &VertexArray) -> J {
    assert_eq!(a.dimension(), 16);
    (0..a.len())
        .map(|t| {
            let m = tuple(a, t);
            (0..4)
                .map(|r| m.get(4 * r..m.len().min(4 * r + 4)).unwrap_or(&[]).to_vec())
                .collect::<J>()
        })
        .collect()
}

/// An array as UnityPy's list of tuples, through the public accessors.
fn array(a: &VertexArray) -> J {
    (0..a.len()).map(|t| J::Array(tuple(a, t))).collect()
}

fn processed(p: &Processed<'_>, calls: Option<&Mutex<Vec<J>>>) -> J {
    let mut attrs = serde_json::Map::new();
    let named = [
        ("m_Vertices", p.vertices()),
        ("m_Normals", p.normals()),
        ("m_Tangents", p.tangents()),
        ("m_Colors", p.colors()),
        ("m_BoneWeights", p.bone_weights()),
        ("m_BoneIndices", p.bone_indices()),
    ];
    for (name, a) in named {
        if let Some(a) = a {
            attrs.insert(name.to_owned(), array(a));
        }
    }
    for i in 0..8 {
        if let Some(a) = p.uv(i) {
            attrs.insert(format!("m_UV{i}"), array(a));
        }
    }
    if let Some(a) = p.bind_pose() {
        attrs.insert("m_BindPose".to_owned(), matrices(a));
    }
    let call = calls
        .and_then(|c| c.lock().unwrap().first().cloned())
        .unwrap_or(J::Null);
    json!({ "ok": {
        "vertex_count": p.vertex_count(),
        "use16": p.use_16bit_indices(),
        "index_buffer": p.index_buffer().map(|b| b.as_slice().to_vec()),
        "attrs": attrs,
        "resource_call": call,
    }})
}

fn triangles(t: &Triangles) -> J {
    let subs: Vec<J> = t
        .submeshes()
        .iter()
        .map(|s| json!({ "tri": s.triangles(), "cut": s.cut(), "empty": s.empty() }))
        .collect();
    let counts: Vec<u64> = t
        .submeshes()
        .iter()
        .map(SubMeshTriangles::index_count)
        .collect();
    assert!(t.submeshes().iter().all(
        |s| s.len() == s.triangles().len() as u64 + u64::from(!s.cut().is_empty()) + s.empty()
    ));
    json!({ "ok": { "submeshes": subs, "index_counts": counts } })
}

/// Every case of a `process.json` body through the handler: its
/// mismatches. `process`, then `get_triangles`, each against UnityPy's;
/// then each again, which must give the first answer: the handler changes
/// nothing it reads, where UnityPy's second `process` reads what its first
/// left (`again`, recorded) and its `get_triangles` the counts it wrote.
fn replay(body: &J) -> Vec<String> {
    let trees = &body["trees"];
    let cases = body["cases"].as_array().unwrap();
    let meter = Meter::unlimited();
    let mut bad = Vec::new();
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let parsed = match parse(
            &trees[c["tree"].as_str().unwrap()],
            unhex(c["hex"].as_str().unwrap()),
        ) {
            Ok(parsed) => parsed,
            Err(why) => {
                bad.push(format!("{name} [parse]: {why}"));
                continue;
            }
        };
        let res = c["resource"].as_str().map(|h| Resource {
            bytes: Bytes::from(unhex(h)),
            calls: Mutex::new(Vec::new()),
        });
        let reader = c["reader"].as_bool().unwrap_or(false);
        let resources: Option<&dyn Resources> = match &res {
            Some(r) if reader => Some(r),
            _ => None,
        };
        let src = if c["source"] == "mesh" {
            Source::mesh(&parsed)
        } else {
            Source::sprite(&parsed)
        };
        let handler =
            MeshHandler::new(src, version(&c["version"])).little_endian(c["endian"] == "<");
        let calls = res.as_ref().map(|r| &r.calls);
        let run = || match handler.process(resources, &meter) {
            Ok(p) => (processed(&p, calls), Some(p)),
            Err(e) => (error(&e), None),
        };
        let (have, done) = run();
        // The corrected answer where UnityPy's differs (Tier C), and the
        // crate's refusal where it refuses what UnityPy unpacks.
        let want = match REFUSED.iter().find(|(n, _)| *n == name) {
            Some((_, why)) => json!({ "rust": format!("Malformed({why:?})") }),
            None => c.get("corrected").unwrap_or(&c["process"]).clone(),
        };
        if have != want {
            bad.push(format!("{name} [process]\n  want {want}\n  have {have}"));
            continue;
        }
        if run().0 != have {
            bad.push(format!("{name} [process twice]"));
        }
        let Some(p) = &done else { continue };
        if c["again"].is_null() {
            bad.push(format!("{name}: UnityPy's second process unrecorded"));
        }
        let have = match p.get_triangles(&meter) {
            Ok(t) => triangles(&t),
            Err(e) => error(&e),
        };
        if have != c["triangles"] {
            bad.push(format!(
                "{name} [triangles]\n  want {}\n  have {have}",
                c["triangles"]
            ));
        }
        let again = p
            .get_triangles(&meter)
            .map(|t| triangles(&t))
            .map_err(|e| error(&e));
        if again.unwrap_or_else(|e| e) != have {
            bad.push(format!("{name} [triangles twice]"));
        }
    }
    bad
}

#[test]
fn every_handler_case_is_unitypy_s() {
    let body = load("process.json");
    let cases = body["cases"].as_array().unwrap();
    assert_eq!(cases.len(), PROCESS_CASES);
    let bad = replay(&body);
    assert!(
        bad.is_empty(),
        "{} of {}:\n{}",
        bad.len(),
        cases.len(),
        bad.join("\n")
    );
    // UnityPy's second `process` gives the first's answer in few of them
    // (PARITY.md section 19.3): it reads the index tuple its first left, so
    // it raises, or leaves another index buffer.
    let mut split = std::collections::BTreeMap::<String, usize>::new();
    for c in cases.iter().filter(|c| c["process"].get("ok").is_some()) {
        let again = &c["again"];
        let kind = again
            .as_str()
            .or_else(|| again["err"][0].as_str())
            .unwrap_or("other");
        *split.entry(kind.to_owned()).or_default() += 1;
    }
    let want = [
        ("ValueError", 38),
        ("differs", 66),
        ("error", 328),
        ("same", 26),
    ];
    let want: std::collections::BTreeMap<String, usize> =
        want.iter().map(|&(k, n)| (k.to_owned(), n)).collect();
    assert_eq!(split, want, "UnityPy's second process, by outcome");
}

/// The cases laid out on UnityPy's own TPK trees in place of the hand-built
/// ones, read from standard input: `tools/fixtures/meshdecode.py --tpk`
/// holds them in memory and hands them over in a pipe, as nothing derived
/// from a TPK is written to any path (docs/plans/core.md section 9, Q2), and
/// runs this with `UNITYRS_MESHDECODE_TPK_STDIN` set. Skipped when it is
/// unset; otherwise it prints `read <n> cases from standard input`, which
/// the script requires (with `--nocapture`), so a skipped run is no pass.
/// A failure names the cases and the part that differs; what each gave,
/// from the TPK's trees, only with `UNITYRS_MESHDECODE_TPK_VERBOSE` set
/// (`--verbose`), as CI's log is public.
#[test]
fn the_cases_on_the_tpk_s_trees_answer_as_unitypy_does() {
    if std::env::var_os("UNITYRS_MESHDECODE_TPK_STDIN").is_none() {
        eprintln!("skipped: UNITYRS_MESHDECODE_TPK_STDIN is unset (meshdecode.py --tpk sets it)");
        return;
    }
    let verbose = verbose();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
        .unwrap_or_else(|e| panic!("{}", failed("reading standard input", &e)));
    let body: J =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}", failed("the cases' JSON", &e)));
    let cases = body["cases"].as_array().unwrap();
    println!("read {} cases from standard input", cases.len());
    assert_eq!(cases.len(), PROCESS_CASES);
    let bad: Vec<String> = replay(&body)
        .into_iter()
        .map(|b| match b.lines().next() {
            Some(first) if !verbose => first.to_owned(),
            _ => b,
        })
        .collect();
    assert!(
        bad.is_empty(),
        "{} of {} on the TPK's trees:\n{}",
        bad.len(),
        cases.len(),
        bad.join("\n")
    );
}

/// A fixture case by name.
fn case<'a>(body: &'a J, name: &str) -> &'a J {
    body["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no case {name}"))
}

/// A bundle holding `case`'s Mesh in a SerializedFile `CAB-x` of version
/// 2019.4.0f1, its tree stored, and, with `resource`, the `.resS` beside
/// it: `pad` bytes, then the case's resource bytes, then `pad` more.
fn streamed_bundle(body: &J, case: &J, resource: bool, pad: usize) -> Vec<u8> {
    let nodes = body["trees"][case["tree"].as_str().unwrap()]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let mut n = NodeSpec::new(
                u8::try_from(r[0].as_u64().unwrap()).unwrap(),
                r[1].as_str().unwrap(),
                r[2].as_str().unwrap(),
            )
            .size(-1);
            n.meta_flag = i32::try_from(r[3].as_i64().unwrap()).unwrap();
            n
        })
        .collect();
    let file = serialized::build(&SerializedSpec {
        unity_version: b"2019.4.0f1".to_vec(),
        ..one_object(43, nodes, unhex(case["hex"].as_str().unwrap()))
    });
    let mut entries = vec![("CAB-x", file)];
    if resource {
        let mut res = vec![0xA5; pad];
        res.extend(unhex(case["resource"].as_str().unwrap()));
        res.extend(vec![0x5A; pad]);
        entries.push(("CAB-x.resS", res));
    }
    env::bundle(&entries)
}

/// The bundle's SerializedFile, loaded through an Environment.
fn load_cab(env: &Environment, bytes: Vec<u8>) -> Arc<SerializedFile> {
    let src = EnvSource::Bytes {
        name: RawStr::from("mesh.bundle"),
        data: Bytes::from(bytes),
    };
    let File::Bundle(bundle) = env.load(src).unwrap() else {
        panic!("not a bundle")
    };
    let File::Serialized(cab) = bundle.entry(0).unwrap() else {
        panic!("not a SerializedFile")
    };
    cab
}

/// `get_resource_data` recorded, then the real read.
struct Recorded<'a>(FileResources<'a>, Mutex<Vec<J>>);

impl Resources for Recorded<'_> {
    fn resource(
        &self,
        path: &RawStr,
        offset: u64,
        size: u64,
    ) -> Result<Bytes, unityrs_core::Error> {
        let name = String::from_utf8(path.as_bytes().to_vec()).unwrap();
        self.1.lock().unwrap().push(json!([name, offset, size]));
        self.0.resource(path, offset, size)
    }
}

/// `PARITY.md` section 19.3's resource row: a Mesh whose `m_StreamData`
/// names a `.resS` in its bundle reads its vertex data through a real
/// Environment's `resource` (`FileResources`), as UnityPy's run of the case
/// read it from `get_resource_data`; with the `.resS` gone, the core's
/// error, `MeshError::Resource`.
#[test]
fn a_streamed_mesh_reads_its_resource_through_an_environment() {
    let body = load("process.json");
    let c = case(&body, "stream_data_2019.4.0f1");
    // The case's offset is 7: the `.resS` holds 7 bytes before its data.
    assert_eq!(c["process"]["ok"]["resource_call"][1], 7);
    let meter = Meter::unlimited();
    let env = Environment::new(EnvOptions::unitypy());
    let cab = load_cab(&env, streamed_bundle(&body, c, true, 7));
    let parsed = cab.object(1).unwrap().parse_as_dict(true).unwrap();
    let version = Version::of(&cab.version().unwrap().version);
    assert_eq!(version, Version::new(2019, 4, 0, 3, 1));
    let resources = Recorded(
        FileResources {
            env: &env,
            file: &cab,
        },
        Mutex::new(Vec::new()),
    );
    let p = MeshHandler::new(Source::mesh(&parsed), version)
        .process(Some(&resources), &meter)
        .unwrap();
    // The fixture's version is 2019.4.0f1 too; its answer is UnityPy's.
    assert_eq!(c["version"], json!([2019, 4, 0, 3, 1]));
    assert_eq!(processed(&p, Some(&resources.1)), c["process"]);

    let env = Environment::new(EnvOptions::unitypy());
    let cab = load_cab(&env, streamed_bundle(&body, c, false, 0));
    let parsed = cab.object(1).unwrap().parse_as_dict(true).unwrap();
    let resources = FileResources {
        env: &env,
        file: &cab,
    };
    let e = MeshHandler::new(Source::mesh(&parsed), version)
        .process(Some(&resources), &meter)
        .unwrap_err();
    assert!(
        matches!(&e, MeshError::Resource(unityrs_core::Error::NotFound(_))),
        "{e:?}"
    );
    assert_eq!(e.python(), None);
}

/// Each case whose corrected answer differs from UnityPy's is in the mask
/// ([`DIVERGES`], then [`NORMALISED`]), with what differs, each masked case
/// differs so, and each of the mask's names names a case.
#[test]
fn the_tier_c_divergences_are_the_mask() {
    let body = load("process.json");
    let mut found = Vec::new();
    let mut want: Vec<(String, Vec<String>)> = Vec::new();
    let mut unused: Vec<&str> = NORMALISED.iter().map(|(n, _)| *n).collect();
    for c in body["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let masked = DIVERGES
            .iter()
            .find(|(n, _)| *n == name)
            .or_else(|| NORMALISED.iter().find(|(n, _)| matches(n, name)));
        if let Some((n, what)) = masked {
            unused.retain(|u| u != n);
            want.push((
                name.to_owned(),
                what.iter().map(|s| (*s).to_owned()).collect(),
            ));
        }
        let Some(fixed) = c.get("corrected") else {
            continue;
        };
        let theirs = &c["process"];
        let mut what: Vec<String> = match (&theirs["ok"]["attrs"], &fixed["ok"]["attrs"]) {
            (J::Object(a), J::Object(b)) => {
                let mut keys: Vec<String> = a.keys().chain(b.keys()).cloned().collect();
                keys.sort();
                keys.dedup();
                keys.into_iter().filter(|k| a.get(k) != b.get(k)).collect()
            }
            _ => vec!["outcome".to_owned()],
        };
        what.sort();
        found.push((name.to_owned(), what));
    }
    assert_eq!(found, want);
    assert!(unused.is_empty(), "the mask names no case: {unused:?}");
}
