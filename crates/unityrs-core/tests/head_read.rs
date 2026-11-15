//! Replay HEAD 417d595's reader on in-code synthetic files. Editor-built
//! Unity 6000.4/6000.5 fixtures remain the target for the full HEAD rows.
use std::sync::Arc;

use serde_json::{Value as Json, json};
use unityrs_core::budget::Reservation;
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::once::CellError;
use unityrs_core::stream::ReaderKind;
use unityrs_core::{BundleFile, Bytes, Error, FileContext, Parsed, PrimArray, RawStr, Value};

fn oracle() -> Json {
    serde_json::from_str(include_str!("fixtures/head_read.json")).unwrap()
}

fn unhex(s: &str) -> Bytes {
    Bytes::from(
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>(),
    )
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| {
            [
                char::from(DIGITS[usize::from(b >> 4)]),
                char::from(DIGITS[usize::from(b & 15)]),
            ]
        })
        .collect()
}

fn encode(p: &Parsed, v: &Value) -> Json {
    match v {
        Value::Int(n) => json!(n),
        Value::UInt(n) => json!(n),
        Value::Bool(b) => json!(b),
        Value::Str(s) => json!(std::str::from_utf8(s.as_bytes()).unwrap()),
        Value::F32(bits) => json!(f64::from(f32::from_bits(*bits))),
        Value::Bytes(b) => {
            json!({"bytes": hex(b)})
        }
        Value::Prims(PrimArray::I32(values)) => json!(values.as_ref()),
        Value::List(items) => json!(items.iter().map(|v| encode(p, v)).collect::<Vec<_>>()),
        Value::Dict(m) => {
            let nodes = &p.lists()[m.list() as usize];
            let mut out = serde_json::Map::new();
            for (key, value) in m.entries() {
                out.insert(
                    String::from_utf8(nodes.name(*key).to_vec()).unwrap(),
                    encode(p, value),
                );
            }
            Json::Object(out)
        }
        other => panic!("unexpected fixture value: {other:?}"),
    }
}

fn objects(f: &Arc<SerializedFile>) -> Result<Json, Error> {
    let mut values = serde_json::Map::new();
    for object in f.objects() {
        let parsed = object
            .parse_as_dict(true)
            .map_err(|e| Error::AssetBundle(Arc::new(e)))?;
        values.insert(
            object.path_id().to_string(),
            encode(&parsed, parsed.value()),
        );
    }
    Ok(Json::Object(values))
}

fn trees(types: &[unityrs_core::files::SerializedType]) -> Json {
    json!(
        types
            .iter()
            .map(|t| t.nodes.as_ref().map(|nodes| nodes
                .rows()
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    json!([
                        std::str::from_utf8(row.name.as_bytes()).unwrap(),
                        std::str::from_utf8(row.ty.as_bytes()).unwrap(),
                        nodes
                            .children(unityrs_core::typetree::NodeId(u32::try_from(i).unwrap()))
                            .len()
                    ])
                })
                .collect::<Vec<_>>()))
            .collect::<Vec<_>>()
    )
}

fn serialized(f: &Arc<SerializedFile>) -> Result<Json, Error> {
    Ok(json!({"objects": objects(f)?,
        "user": std::str::from_utf8(f.user_information().unwrap().as_bytes()).unwrap(),
        "types": trees(f.types()),
        "ref_types": trees(f.ref_types().unwrap_or_default()) }))
}

fn bundle(b: &Arc<BundleFile>) -> Result<Json, Error> {
    let mut files = serde_json::Map::new();
    for (i, slot) in b.files().iter().enumerate() {
        let file = b.entry(i)?;
        let value = match file {
            File::Serialized(f) => json!({"objects": objects(&f)?}),
            File::Raw(f) => {
                json!({"bytes": hex(&f.data)})
            }
            other => panic!("unexpected entry: {other:?}"),
        };
        files.insert(
            std::str::from_utf8(slot.name.as_bytes())
                .unwrap()
                .to_owned(),
            value,
        );
    }
    Ok(json!({"files": files}))
}

fn read(case: &Json, ctx: &Arc<FileContext>, meter: &Arc<Meter>) -> Result<Json, Error> {
    let data = unhex(case["hex"].as_str().unwrap());
    let reader = if case["stream"] == true {
        ReaderKind::Stream
    } else {
        ReaderKind::Memory
    };
    if case["kind"] == "serialized" {
        serialized(&SerializedFile::parse_with_reader(
            RawStr::from("CAB-synthetic"),
            data,
            None,
            meter,
            ctx,
            reader,
        )?)
    } else {
        let open = || {
            BundleFile::open_with_reader(
                RawStr::from("synthetic.bundle"),
                data.clone(),
                None,
                meter,
                ctx,
                reader,
            )
        };
        let lazy = bundle(&open()?)?;
        let eager = open()?;
        eager.load_all()?;
        assert_eq!(bundle(&eager)?, lazy, "lazy and eager reads agree");
        Ok(lazy)
    }
}

#[test]
fn head_reads_replay_the_recorded_oracle() {
    let oracle = oracle();
    assert_eq!(oracle["head"], "417d59522a998fdee9a9c2b29fe35f3063222457");
    assert_eq!(oracle["cases"].as_array().unwrap().len(), 146);
    for case in oracle["cases"].as_array().unwrap() {
        let ctx = Arc::new(FileContext::unitypy());
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let got = read(case, &ctx, &meter);
        if case["result"].get("error").is_some() {
            let error = got.expect_err("oracle recorded a refusal");
            let class = match error {
                Error::OutOfBounds { .. } => "error",
                Error::Tree(unityrs_core::typetree::TreeError::Empty) => "IndexError",
                Error::AssetBundle(ref e)
                    if matches!(
                        e.as_ref(),
                        unityrs_core::walk::ParseError::Read(unityrs_core::walk::ReadError {
                            detail: unityrs_core::walk::ReadDetail::Stream(
                                unityrs_core::stream::StreamRefusal::NegativeRead
                            ),
                            ..
                        })
                    ) =>
                {
                    "ValueError"
                }

                Error::Malformed {
                    what: "HEAD typetree",
                    detail,
                } => match detail.as_ref() {
                    "shared hash index out of range" => "IndexError",
                    "shared subtree hash not found" => "KeyError",
                    "cyclic shared subtree" => "RecursionError",
                    "Invalid type tree blob magic" => "ValueError",
                    other => panic!("unexpected HEAD refusal: {other}"),
                },
                other => panic!("unexpected oracle refusal: {other:?}"),
            };
            assert_eq!(
                class,
                case["result"]["error"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(
                got.unwrap_or_else(|e| panic!("{}: {e}", case["name"])),
                case["result"],
                "{}",
                case["name"]
            );
        }
        assert_eq!(meter.used(), 0, "{}: grants returned", case["name"]);
    }
}

#[test]
fn shared_expansion_and_offsets_obey_memory_and_work_limits() {
    for case in oracle()["cases"].as_array().unwrap() {
        for small_work in [false, true] {
            let mut ctx = FileContext::unitypy();
            if small_work {
                ctx.limits.max_decode_work = 16;
            } else {
                ctx.limits.max_input_memory = 64;
            }
            let ctx = Arc::new(ctx);
            let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
            assert!(read(case, &ctx, &meter).is_err(), "{}", case["name"]);
            assert_eq!(meter.used(), 0, "{}: refused grants returned", case["name"]);
        }
    }
}

thread_local! {
    static OWNER_FAILURE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
struct Owners;
impl unityrs_testkit::counted::Tally for Owners {
    fn refuses(&self, size: usize) -> bool {
        let node = std::mem::size_of::<unityrs_core::typetree::NodeList>();
        if size != node + 8 && size != node + 16 {
            return false;
        }
        OWNER_FAILURE
            .try_with(|left| {
                let n = left.get();
                left.set(n.saturating_sub(1));
                n == 1
            })
            .unwrap_or(false)
    }
    fn grew(&self, _: usize, _: bool) {}
    fn shrank(&self, _: usize) {}
}
#[global_allocator]
static ALLOC: unityrs_testkit::counted::Allocator<Owners> =
    unityrs_testkit::counted::Allocator(Owners);

#[test]
fn shared_owner_allocation_failures_return_errors() {
    const VAR: &str = "UNITYRS_HEAD_OWNER_FAILURE";
    if let Ok(n) = std::env::var(VAR) {
        let cases = oracle();
        let case = cases["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "shared-root" && c["stream"] == false)
            .unwrap();
        let data = unhex(case["hex"].as_str().unwrap());
        let ctx = Arc::new(FileContext::unitypy());
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        OWNER_FAILURE.with(|left| left.set(n.parse().unwrap()));
        let result =
            SerializedFile::parse(RawStr::from_static(b"synthetic"), data, None, &meter, &ctx);
        OWNER_FAILURE.with(|left| left.set(0));
        let error = result.unwrap_err();
        assert!(error.is_transient(), "{error:?}");
        assert!(!error.is_meter_refusal(), "{error:?}");
        assert_eq!(meter.used(), 0);
        return;
    }
    for n in 1..=3 {
        let output = unityrs_testkit::child::output(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "shared_owner_allocation_failures_return_errors",
                    "--nocapture",
                ])
                .env(VAR, n.to_string()),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "owner {n}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn discarded_descendant_snapshot_matches_head() {
    for case in oracle()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"] == "discarded-descendant-snapshot")
    {
        let ctx = Arc::new(FileContext::unitypy());
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let error = read(case, &ctx, &meter).unwrap_err();
        assert!(
            matches!(error, Error::Malformed { what: "HEAD typetree", ref detail } if detail.as_ref() == "shared hash index out of range")
        );
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn head_value_mode_preserves_the_legacy_array_error() {
    use unityrs_core::typetree::{NodeList, NodeRow, Tree};
    use unityrs_core::walk::{ReadCtx, ReadDetail, ReadOptions, RefTypes, python};
    let case = oracle()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "aligned-typeless-negative-zero-object" && c["stream"] == false)
        .unwrap()
        .clone();
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let got = read(&case, &ctx, &meter).unwrap();
    assert_eq!(got["objects"]["1"]["blobs"], json!([{"bytes":""}]));
    let nodes = NodeList::trusted(vec![
        NodeRow::new(0, "Synthetic", "Base"),
        NodeRow::new(1, "vector", "blobs"),
        NodeRow::new(2, "Array", "Array"),
        NodeRow::new(3, "int", "size"),
        NodeRow::new(3, "TypelessData", "data").with_meta_flag(0x4000),
    ])
    .unwrap()
    .into_shared()
    .unwrap();
    let tree = Tree::compile(nodes, &ctx.limits).unwrap();
    let options = ReadOptions::default();
    let read_ctx = ReadCtx {
        endian: unityrs_core::stream::Endian::Little,
        ref_types: RefTypes::NoFile,
        options: &options,
        meter: &meter,
    };
    let bytes = Bytes::from([1i32.to_le_bytes(), (-8i32).to_le_bytes()].concat());
    let error = python::read(&bytes, 0, &tree, &read_ctx).unwrap_err();
    assert!(matches!(
        error.detail,
        ReadDetail::Python("TypeError: read_bytes() missing 1 required positional argument")
    ));
    assert_eq!(meter.used(), 0);
}
