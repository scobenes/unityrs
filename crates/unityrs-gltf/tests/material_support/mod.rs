#![allow(dead_code)]
use unityrs_core::limits::Meter;
use unityrs_gltf::{CorePbr, MaterialMapper, Profile, Scene, UnlitExtras};
use unityrs_testkit::{
    ngui::{self, Object, V},
    scene3d as s,
};

pub const DEFAULT_PROFILE: &[u8] = br#"{"profile_version":1}"#;
pub const PIXELS: [u8; 16] = [
    255, 0, 0, 255, 0, 255, 0, 128, // Unity's bottom row.
    0, 0, 255, 255, 255, 255, 0, 64, // Unity's top row.
];
pub const TOP_FIRST: [u8; 16] = [
    0, 0, 255, 255, 255, 255, 0, 64, 255, 0, 0, 255, 0, 255, 0, 128,
];

pub fn objects(
    shader: &[u8],
    floats: &[(&str, f32)],
    colors: &[(&str, [f32; 4])],
    envs: &[(&str, V)],
) -> Vec<Object> {
    let mut out = super::collect_support::objects(false, false);
    out[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10), (0, 10)], (0, 0)));
    out.extend([
        s::material(10, b"paint", (0, 11), floats, colors, envs),
        s::shader(11, shader),
        ngui::texture(
            12,
            b"asymmetric",
            ngui::format::RGBA32,
            (2, 2),
            PIXELS.to_vec(),
        ),
    ]);
    out
}
pub fn simple() -> Vec<Object> {
    objects(
        b"Standard",
        &[],
        &[("_Color", [0.25, 0.5, 0.75, 1.])],
        &[("_MainTex", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
    )
}
pub fn rich(shader: &[u8]) -> Vec<Object> {
    let mut objects = objects(
        shader,
        &[
            ("_Metallic", 0.25),
            ("_Glossiness", 0.75),
            ("_BumpScale", 0.75),
            ("_OcclusionStrength", 0.5),
            ("_Cutoff", 0.375),
            ("_Mode", 1.),
            ("_AlphaClip", 1.),
            ("_Cull", 0.),
        ],
        &[
            ("_Color", [0.25, 0.5, 0.75, 1.]),
            ("_EmissionColor", [0.125, 0.25, 0.5, 1.]),
        ],
        &[
            ("_MainTex", s::tex_env((0, 12), [0.5, 0.25], [0.125, 0.125])),
            ("_BumpMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
            ("_OcclusionMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
            ("_EmissionMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
        ],
    );
    s::set(
        &mut objects[7].value,
        "m_ShaderKeywords",
        V::Str(b"_EMISSION".to_vec()),
    );
    objects
}
pub fn collect(objects: &[Object], mapper: &dyn MaterialMapper, profile: &[u8]) -> Scene {
    let (env, graph) = super::collect_support::graph(objects);
    let meter = Meter::unlimited();
    let scene = unityrs_gltf::collect_with_mapper(
        &env,
        &graph,
        &Profile::parse(profile, &meter).unwrap(),
        mapper,
        &meter,
    )
    .unwrap();
    validate(&scene, "generated");
    scene
}
pub fn json(scene: &Scene) -> serde_json::Value {
    let bytes = super::collect_support::write(scene);
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + len]).unwrap()
}

pub fn embedded_image(scene: &Scene) -> Vec<u8> {
    let bytes = super::collect_support::write(scene);
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
    let view = usize::try_from(json["images"][0]["bufferView"].as_u64().unwrap()).unwrap();
    let offset = usize::try_from(
        json["bufferViews"][view]["byteOffset"]
            .as_u64()
            .unwrap_or(0),
    )
    .unwrap();
    let length =
        usize::try_from(json["bufferViews"][view]["byteLength"].as_u64().unwrap()).unwrap();
    let bin_start = 28 + len;
    bytes[bin_start + offset..bin_start + offset + length].to_vec()
}
pub fn reason(scene: &Scene, expected: &str) {
    assert!(
        scene
            .collection_issues
            .as_slice()
            .iter()
            .any(|issue| issue.reason == expected && issue.count > 0),
        "missing {expected}: {:?}",
        scene.collection_issues
    );
    let json = json(scene);
    assert!(
        json["asset"]["extras"].to_string().contains(expected),
        "missing asset reason {expected}: {json}"
    );
}
pub fn missing_texture_objects() -> Vec<Object> {
    objects(
        b"Standard",
        &[],
        &[],
        &[("_MainTex", s::tex_env((0, 999), [1.; 2], [0.; 2]))],
    )
}
pub fn texture_case(format: i32, data: Vec<u8>) -> Vec<Object> {
    let mut out = simple();
    out[9] = ngui::texture(12, b"asymmetric", format, (2, 2), data);
    out
}

pub fn validate(scene: &Scene, name: &str) {
    // Always exercise the production writer, even when the external tool is absent.
    let bytes = super::collect_support::write(scene);
    let Some(exe) = std::env::var_os("UNITYRS_GLTF_VALIDATOR") else {
        assert!(std::env::var_os("UNITYRS_GLTF_VALIDATOR_REQUIRED").is_none());
        eprintln!("material validator skipped: set UNITYRS_GLTF_VALIDATOR");
        return;
    };
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-material-validator");
    let path = scratch.path().join(format!("{name}.glb"));
    std::fs::write(&path, bytes).unwrap();
    let out = unityrs_testkit::child::output(
        std::process::Command::new(exe)
            .args(["--stdout", "--all"])
            .arg(path),
    )
    .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["validatorVersion"], "2.0.0-dev.3.10");
    assert_eq!(report["issues"]["numErrors"], 0, "{name}: {report}");
    assert_eq!(report["issues"]["numWarnings"], 0, "{name}: {report}");
    assert_eq!(report["issues"]["truncated"], false);
    for message in report["issues"]["messages"].as_array().unwrap() {
        assert!(
            message["severity"].as_u64().unwrap() > 1,
            "{name}: {message}"
        );
    }
    assert!(out.status.success(), "{name}: {report}");
}

pub fn validator_cases() -> Vec<(&'static str, Scene)> {
    vec![
        ("unlit", collect(&simple(), &UnlitExtras, DEFAULT_PROFILE)),
        (
            "pbr",
            collect(&rich(b"Standard"), &CorePbr, DEFAULT_PROFILE),
        ),
        (
            "specular",
            collect(
                &rich(b"Standard (Specular setup)"),
                &CorePbr,
                DEFAULT_PROFILE,
            ),
        ),
        (
            "urp",
            collect(
                &rich(b"Universal Render Pipeline/Lit"),
                &CorePbr,
                DEFAULT_PROFILE,
            ),
        ),
        (
            "missing-texture",
            collect(&missing_texture_objects(), &CorePbr, DEFAULT_PROFILE),
        ),
        (
            "empty-texture",
            collect(
                &texture_case(ngui::format::RGBA32, vec![]),
                &CorePbr,
                DEFAULT_PROFILE,
            ),
        ),
        (
            "short-texture",
            collect(
                &texture_case(ngui::format::RGBA32, vec![0; 3]),
                &CorePbr,
                DEFAULT_PROFILE,
            ),
        ),
        (
            "unsupported-texture",
            collect(&texture_case(999, vec![0; 16]), &CorePbr, DEFAULT_PROFILE),
        ),
        (
            "unknown-shader",
            collect(
                &objects(b"Synthetic/Unknown", &[], &[], &[]),
                &CorePbr,
                DEFAULT_PROFILE,
            ),
        ),
    ]
}

/// Independent PNG scanline reconstruction using only Python's standard library.
pub fn assert_pixels(bytes: &[u8], expected: &[u8]) {
    assert_pixels_sized(bytes, (2, 2), expected);
}
pub fn assert_pixels_sized(bytes: &[u8], size: (u32, u32), expected: &[u8]) {
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-material-pixels");
    let path = scratch.path().join("texture.png");
    std::fs::write(&path, bytes).unwrap();
    let script = r"
import json, struct, sys, zlib
b = open(sys.argv[1], 'rb').read()
assert b[:8] == b'\x89PNG\r\n\x1a\n'
p, compressed, header = 8, bytearray(), None
while p < len(b):
    n = struct.unpack('>I', b[p:p+4])[0]
    tag, data = b[p+4:p+8], b[p+8:p+8+n]
    assert zlib.crc32(tag+data) & 0xffffffff == struct.unpack('>I', b[p+8+n:p+12+n])[0]
    if tag == b'IHDR': header = struct.unpack('>IIBBBBB', data)
    if tag == b'IDAT': compressed.extend(data)
    p += n + 12
w,h,depth,color,comp,filt,interlace = header
assert (w,h,depth,color,comp,filt,interlace) == (int(sys.argv[2]),int(sys.argv[3]),8,6,0,0,0), header
raw, stride, previous, rgba, p = zlib.decompress(compressed), w*4, bytearray(w*4), bytearray(), 0
for y in range(h):
    f, row = raw[p], bytearray(raw[p+1:p+1+stride]); p += stride+1
    for x in range(stride):
        a = row[x-4] if x >= 4 else 0
        c = previous[x-4] if x >= 4 else 0
        u = previous[x]
        if f == 0: predictor = 0
        elif f == 1: predictor = a
        elif f == 2: predictor = u
        elif f == 3: predictor = (a+u)//2
        elif f == 4:
            q = a+u-c
            ds = [abs(q-a), abs(q-u), abs(q-c)]
            predictor = [a,u,c][ds.index(min(ds))]
        else: raise AssertionError(f)
        row[x] = (row[x] + predictor) & 255
    rgba.extend(row); previous = row
assert p == len(raw)
print(json.dumps(list(rgba)))
";
    let out = unityrs_testkit::child::output(
        std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(path)
            .arg(size.0.to_string())
            .arg(size.1.to_string()),
    )
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let actual: Vec<u8> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(actual, expected);
}

/// Independent exact EOTF reference for saved material factors.
pub fn linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
pub fn linear_color(mut c: [f64; 4]) -> [f64; 4] {
    for v in &mut c[..3] {
        *v = linear(*v);
    }
    c
}

pub fn assert_factor(value: &serde_json::Value, expected: &[f64]) {
    let actual = value.as_array().unwrap();
    assert_eq!(actual.len(), expected.len());
    for (a, b) in actual.iter().zip(expected) {
        assert!(
            (a.as_f64().unwrap() - b).abs() < 1e-14,
            "{value}: {expected:?}"
        );
    }
}
