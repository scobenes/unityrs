//! A completed owner Editor fixture run (`tools/fixtures/unity/README.md`),
//! exported through the library's two phases, and the values the fixture
//! scripts author (`tools/fixtures/unity/Editor/UnityrsStaticFixtures.cs`,
//! `UnityrsUrpFixtures.cs`) in glTF's space.
#![allow(dead_code, clippy::too_many_lines)]
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use unityrs_core::{
    Bytes, RawStr,
    env::{CabResolver, EnvOptions, Environment, Source},
};
use unityrs_gltf::{MaterialMapper, Profile, collect_with_mapper, write_glb};
use unityrs_scene::{GraphOptions, Unit, unit_from_container};
use unityrs_testkit::{
    ngui::{Object, V, st, vector},
    scene3d::{self as s, mesh as m},
};

/// Quaternion.Euler composes three single-axis rotations in f32: each saved
/// component sums four products of three factors, each within an ulp, so it
/// lies within 2^-20 of the exact rotation.
pub const ROTATION: f64 = 1. / 1_048_576.;
/// A static batch's vertices are Unity's f32 localToWorld products, from
/// that rotation: below magnitude 8 they keep 8 ulps of 2^-21.
pub const PLACEMENT: f64 = 1. / 262_144.;

/// One run folder: `<version>/StandaloneWindows64` or `URP-StandaloneWindows64`.
pub struct Run {
    pub dir: PathBuf,
    pub version: String,
    pub stamp: String,
}

/// Read `run.json`, requiring the variant and each named case recorded.
pub fn run(var: &str, variant: &str, cases: &[&str]) -> Run {
    let dir = PathBuf::from(std::env::var_os(var).unwrap_or_else(|| panic!("set {var}")));
    let doc: Value = serde_json::from_slice(&std::fs::read(dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(doc["format"], "unityrs.fixture_run");
    assert_eq!(doc["version"], 1);
    let run = &doc["run"];
    assert_eq!(run["variant"], variant);
    for case in cases {
        let row = doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == *case)
            .unwrap_or_else(|| panic!("{case} missing from run.json"));
        assert_eq!(row["status"], "recorded", "{case}: {row}");
    }
    let field = |key: &str| run[key].as_str().unwrap().to_owned();
    let stamp = format!(
        "unityrs fixture run {} {} {}",
        field("run_id"),
        field("editor_version"),
        field("build_target")
    );
    Run {
        dir,
        version: field("editor_version"),
        stamp,
    }
}

pub enum Select<'a> {
    /// A container path whose entry targets a `GameObject`.
    Prefab(&'a str),
    /// A scene bundle's `BuildPlayer-<name>` file.
    Scene(&'a str),
}

/// A written GLB's JSON and BIN.
pub struct Glb {
    pub json: Value,
    bin: Vec<u8>,
}

/// Export one unit of `bundle` as the CLI does, then check the GLB with the
/// pinned validator when `UNITYRS_GLTF_VALIDATOR` names it.
pub fn export(
    run: &Run,
    bundle: &str,
    select: &Select<'_>,
    profile: &[u8],
    mapper: &dyn MaterialMapper,
    resolver: Option<Arc<Builtins>>,
) -> Glb {
    let mut options = EnvOptions::unitypy();
    options.resolver = resolver.clone().map(|r| r as Arc<dyn CabResolver>);
    let env = Environment::new(options);
    env.load(Source::Path(run.dir.join("bundles").join(bundle)))
        .unwrap();
    let mut units = Vec::new();
    match select {
        Select::Prefab(path) => {
            for entry in env.container().unwrap() {
                let entry = entry.unwrap();
                if entry.row.path.as_ref().map(RawStr::as_bytes) == Some(path.as_bytes()) {
                    units.extend(unit_from_container(&env, &entry.file, &entry.row).unwrap());
                }
            }
        }
        Select::Scene(name) => {
            for object in env.objects() {
                let object = object.unwrap();
                let file = object.file();
                if file.name().as_bytes() == name.as_bytes()
                    && !units
                        .iter()
                        .any(|u| matches!(u, Unit::Scene(f) if Arc::ptr_eq(f, file)))
                {
                    units.push(Unit::Scene(file.clone()));
                }
            }
        }
    }
    assert_eq!(units.len(), 1, "{bundle}: one unit");
    let unit = units.pop().unwrap();
    // As the CLI does, offer the resources only once the container walk
    // that selected the unit is over.
    if let Some(resolver) = resolver {
        resolver.armed.store(true, Ordering::Release);
    }
    let meter = unit.meter().clone();
    let profile = Profile::parse(profile, &meter).unwrap();
    let graph = unit.graph(&env, &GraphOptions::default(), &meter).unwrap();
    let scene = collect_with_mapper(&env, &graph, &profile, mapper, &meter).unwrap();
    let mut bytes = Vec::new();
    write_glb(&scene, &profile.options(), &meter, &mut bytes).unwrap();
    validate(&bytes, bundle);
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let json = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
    let bin = bytes.get(28 + len..).unwrap_or_default().to_vec();
    Glb { json, bin }
}

fn validate(bytes: &[u8], name: &str) {
    let Some(exe) = std::env::var_os("UNITYRS_GLTF_VALIDATOR") else {
        assert!(std::env::var_os("UNITYRS_GLTF_VALIDATOR_REQUIRED").is_none());
        eprintln!("Editor fixture validator skipped: set UNITYRS_GLTF_VALIDATOR");
        return;
    };
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-editor-validator");
    let path = scratch.path().join("unit.glb");
    std::fs::write(&path, bytes).unwrap();
    let out = unityrs_testkit::child::output(
        std::process::Command::new(exe)
            .args(["--stdout", "--all"])
            .arg(path),
    )
    .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["validatorVersion"], "2.0.0-dev.3.10");
    assert_eq!(report["issues"]["numErrors"], 0, "{name}: {report}");
    assert_eq!(report["issues"]["numWarnings"], 0, "{name}: {report}");
    assert!(out.status.success(), "{name}: {report}");
}

impl Glb {
    /// Rows of one accessor, sparse substitutions applied.
    pub fn accessor(&self, index: &Value) -> Vec<Vec<f64>> {
        let a = &self.json["accessors"][usize(index)];
        let width = match a["type"].as_str().unwrap() {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            other => panic!("accessor type {other}"),
        };
        let count = usize(&a["count"]);
        let mut values = if a["bufferView"].is_null() {
            vec![0.; count * width]
        } else {
            self.view(&a["bufferView"], &a["componentType"], count * width)
        };
        if let Some(sparse) = a.get("sparse") {
            let n = usize(&sparse["count"]);
            let rows = self.view(
                &sparse["indices"]["bufferView"],
                &sparse["indices"]["componentType"],
                n,
            );
            let replaced = self.view(
                &sparse["values"]["bufferView"],
                &a["componentType"],
                n * width,
            );
            for (k, row) in rows.iter().enumerate() {
                let at = self::index(*row) * width;
                values[at..at + width].copy_from_slice(&replaced[k * width..(k + 1) * width]);
            }
        }
        values.chunks(width).map(<[f64]>::to_vec).collect()
    }
    fn view(&self, view: &Value, component: &Value, n: usize) -> Vec<f64> {
        let view = &self.json["bufferViews"][usize(view)];
        let at = usize(&view["byteOffset"]);
        let size = match component.as_u64().unwrap() {
            5121 => 1,
            5123 => 2,
            5125 | 5126 => 4,
            other => panic!("component type {other}"),
        };
        self.bin[at..at + n * size]
            .chunks_exact(size)
            .map(|w| match (component.as_u64().unwrap(), w) {
                (5121, [b]) => f64::from(*b),
                (5123, [a, b]) => f64::from(u16::from_le_bytes([*a, *b])),
                (5125, w) => f64::from(u32::from_le_bytes(w.try_into().unwrap())),
                (_, w) => f64::from(f32::from_le_bytes(w.try_into().unwrap())),
            })
            .collect()
    }
    /// An embedded image's PNG bytes.
    pub fn image(&self, index: &Value) -> Vec<u8> {
        let view =
            &self.json["bufferViews"][usize(&self.json["images"][usize(index)]["bufferView"])];
        let at = usize(&view["byteOffset"]);
        self.bin[at..at + usize(&view["byteLength"])].to_vec()
    }
    pub fn nodes(&self) -> &[Value] {
        self.json["nodes"].as_array().unwrap()
    }
    pub fn roots(&self) -> Vec<usize> {
        self.json["scenes"][0]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(usize)
            .collect()
    }
    /// The one node of `name` that carries a mesh (an attachment), or none.
    pub fn named(&self, name: &str, with_mesh: bool) -> &Value {
        let found: Vec<_> = self
            .nodes()
            .iter()
            .filter(|n| n["name"] == name && n.get("mesh").is_some() == with_mesh)
            .collect();
        assert_eq!(found.len(), 1, "{name} (mesh {with_mesh})");
        found[0]
    }
    pub fn material(&self, name: &str) -> &Value {
        let found: Vec<_> = self.json["materials"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["name"] == name)
            .collect();
        assert_eq!(found.len(), 1, "material {name}");
        found[0]
    }
    pub fn issues(&self, reason: &str) -> u64 {
        self.json["asset"]["extras"]["collection_issues"]
            .as_array()
            .map_or(0, |rows| {
                rows.iter()
                    .filter(|r| r["reason"] == reason)
                    .map(|r| r["count"].as_u64().unwrap())
                    .sum()
            })
    }
}

pub fn usize(v: &Value) -> usize {
    usize::try_from(v.as_u64().unwrap_or(0)).unwrap()
}
/// An index accessor's value, an integer below 2^32.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn index(v: f64) -> usize {
    assert!(v.fract() == 0. && (0. ..4_294_967_296.).contains(&v), "{v}");
    v as usize
}

/// An expected glTF node: Unity's authored TRS mirrored by negating x.
pub struct Node {
    pub name: String,
    pub t: [f64; 3],
    pub r: [f64; 4],
    pub s: [f64; 3],
    pub children: Vec<Node>,
}
pub fn node(name: &str, t: [f64; 3], children: Vec<Node>) -> Node {
    Node {
        name: name.into(),
        t: mirror(t),
        r: [0., 0., 0., 1.],
        s: [1.; 3],
        children,
    }
}

/// Compare one subtree: names, child order and scale exactly, translation
/// exactly (every authored value is an f32), rotation within [`ROTATION`].
pub fn assert_tree(glb: &Glb, index: usize, want: &Node, label: &str) {
    let got = &glb.nodes()[index];
    let path = format!("{label}/{}", want.name);
    assert_eq!(got["name"], want.name.as_str(), "{path}");
    assert_eq!(floats(&got["translation"]), want.t, "{path} translation");
    assert_eq!(floats(&got["scale"]), want.s, "{path} scale");
    close(
        &floats(&got["rotation"]),
        &want.r,
        ROTATION,
        &format!("{path} rotation"),
    );
    let children: Vec<usize> = got["children"]
        .as_array()
        .map_or(Vec::new(), |c| c.iter().map(usize).collect());
    assert_eq!(children.len(), want.children.len(), "{path} children");
    for (child, expected) in children.into_iter().zip(&want.children) {
        assert_tree(glb, child, expected, &path);
    }
}

pub fn floats(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}
pub fn close(got: &[f64], want: &[f64], tolerance: f64, label: &str) {
    assert_eq!(got.len(), want.len(), "{label}: {got:?} {want:?}");
    for (a, b) in got.iter().zip(want) {
        assert!(
            (a - b).abs() <= tolerance,
            "{label}: {got:?} against {want:?}"
        );
    }
}

pub fn mirror([x, y, z]: [f64; 3]) -> [f64; 3] {
    [-x, y, z]
}
/// Unity's rotation (x, y, z, w) in glTF's space.
pub fn mirror_rotation([x, y, z, w]: [f64; 4]) -> [f64; 4] {
    [x, -y, -z, w]
}
fn axis(i: usize, degrees: f64) -> [f64; 4] {
    let (sin, cos) = (degrees.to_radians() / 2.).sin_cos();
    let mut q = [0., 0., 0., cos];
    q[i] = sin;
    q
}
fn product(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}
/// `Quaternion.Euler(x, y, z)`: z first, then x, then y.
pub fn euler(x: f64, y: f64, z: f64) -> [f64; 4] {
    product(product(axis(1, y), axis(0, x)), axis(2, z))
}
pub fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let p = product(
        product(q, [v[0], v[1], v[2], 0.]),
        [-q[0], -q[1], -q[2], q[3]],
    );
    [p[0], p[1], p[2]]
}

/// The batch panel and skin mesh both start from `NewPanel`.
pub const PANEL: [[f64; 3]; 4] = [[-1., 0., 0.], [1., 0., 0.], [-1., 2., 0.], [1., 2., 0.]];
pub const PANEL_UV: [[f64; 2]; 4] = [[0., 0.], [1., 0.], [0., 1.], [1., 1.]];
pub const PANEL_TRIANGLES: [[usize; 3]; 2] = [[0, 2, 1], [1, 2, 3]];
/// `RecalculateNormals` of that winding, in Unity's space.
pub const PANEL_NORMAL: [f64; 3] = [0., 0., -1.];

/// A user-supplied resource file (the `resolve` mode's `CabResolver`) whose
/// Cube and Quad carry normals, tangents and UVs, so textured materials on
/// built-in meshes map.
#[derive(Debug)]
pub struct Builtins {
    bytes: Bytes,
    armed: AtomicBool,
}
impl Builtins {
    pub fn new() -> Arc<Self> {
        let meshes = [uv_quad(10202, b"Cube"), uv_quad(10210, b"Quad")];
        Arc::new(Self {
            bytes: Bytes::from(s::file(s::Layout::Unity2022, &[], &meshes).unwrap()),
            armed: AtomicBool::new(false),
        })
    }
}
impl CabResolver for Builtins {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        if !self.armed.load(Ordering::Acquire) {
            return None;
        }
        let base = name.rsplit(|b| matches!(b, b'/' | b'\\')).next()?;
        base.eq_ignore_ascii_case(b"unity default resources")
            .then(|| Source::Bytes {
                name: RawStr::from_static(b"supplied-builtins"),
                data: self.bytes.clone(),
            })
    }
}
fn uv_quad(path_id: i64, name: &[u8]) -> Object {
    let positions = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]];
    let mut mesh = m::mesh(
        path_id,
        name,
        &positions,
        &[0, 2, 1, 1, 2, 3],
        m::MeshVersion::V2022,
    );
    let mut channels = Vec::new();
    let mut offset = 0u8;
    for i in 0..14 {
        let dimension = match i {
            0 | 1 => 3,
            2 => 4,
            4 => 2,
            _ => 0,
        };
        channels.push(st(
            "ChannelInfo",
            vec![
                ("stream", V::UInt8(0)),
                ("offset", V::UInt8(offset)),
                ("format", V::UInt8(0)),
                ("dimension", V::UInt8(dimension)),
            ],
        ));
        offset += dimension * 4;
    }
    let mut data = Vec::new();
    // Facing -z, as `RecalculateNormals` gives this winding, with a tangent
    // along x, so a normal map has the tangent space the validator asks for.
    for p in positions {
        let attributes = [0., 0., -1., 1., 0., 0., -1.];
        for x in p.iter().chain(&attributes).chain(&p[..2]) {
            data.extend(x.to_le_bytes());
        }
    }
    data.resize(data.len().next_multiple_of(16), 0);
    s::set(
        &mut mesh.value,
        "m_VertexData",
        st(
            "VertexData",
            vec![
                ("m_VertexCount", V::UInt(4)),
                ("m_Channels", vector(channels[0].clone(), channels)),
                ("m_DataSize", V::Bytes(data)),
            ],
        ),
    );
    mesh
}
