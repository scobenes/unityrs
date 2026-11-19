//! P2a's semantic rows (`PARITY.md` section 24) against an owner-built Editor
//! fixture run. The expected values are those the fixture scripts author
//! (`tools/fixtures/unity/Editor/UnityrsStaticFixtures.cs` and
//! `UnityrsUrpFixtures.cs`), mirrored into glTF's space; the bundles are read
//! from the run folder at test time, never committed.
//!
//! Each test runs on both Editor lines and fails unless both runs are named:
//! `UNITYRS_STATIC_RUN_2022` a 2022.3 main run's `<version>/StandaloneWindows64`
//! folder (its `run.json` beside `bundles/`), `UNITYRS_STATIC_RUN_6000` a
//! 6000.6 one's. `UNITYRS_STATIC_URP_RUN_6000` names a 6000.6 URP run's
//! `<version>/URP-StandaloneWindows64`; no 2022.3 URP run is checked yet.
//! The 6000.6 bundles need the core's Unity 6 type-tree and Mesh readers.
mod collect_support;
mod editor_support;
mod material_support;
use editor_support::{self as e, Glb, Node, Run, Select, node};
use serde_json::{Value, json};
use unityrs_gltf::{CorePbr, MaterialMapper, UnlitExtras};

const DEFAULT: &[u8] = br#"{"profile_version":1}"#;
const RESOLVE: &[u8] = br#"{"profile_version":1,"builtin":"resolve"}"#;
const STATIC: &str = "assets/unityrsfixtures/static/";
const PRIMITIVES: [(&str, i64); 6] = [
    ("Cube", 10202),
    ("Cylinder", 10206),
    ("Sphere", 10207),
    ("Capsule", 10208),
    ("Plane", 10209),
    ("Quad", 10210),
];

fn prefab(run: &Run, name: &str, profile: &[u8], mapper: &dyn MaterialMapper) -> Glb {
    let path = format!("{STATIC}{name}.prefab");
    e::export(
        run,
        "static-objects",
        &Select::Prefab(&path),
        profile,
        mapper,
        None,
    )
}
fn batch(run: &Run) -> Glb {
    e::export(
        run,
        "static-batch-scene",
        &Select::Scene("BuildPlayer-batch"),
        DEFAULT,
        &UnlitExtras,
        None,
    )
}

/// Unity names a saved prefab's root after its asset file. Children keep the
/// order the script adds them in, with their authored local TRS.
#[test]
#[ignore = "set UNITYRS_STATIC_RUN_2022 and UNITYRS_STATIC_RUN_6000 to owner-built Editor fixture runs"]
fn editor_hierarchy_and_trs() {
    for run in e::runs(&[
        "static-primitives",
        "static-materials",
        "static-lod",
        "static-skin",
    ]) {
        hierarchy_and_trs(&run);
    }
}
fn hierarchy_and_trs(run: &Run) {
    let bones = vec![
        node("Upper", [0., 1., 0.], vec![]),
        node("Right", [1., 0., 0.], vec![]),
        node("Left", [-1., 0., 0.], vec![]),
    ];
    let mut skin = node(
        "skin",
        [2., 1., -3.],
        vec![node(
            "Skin",
            [0.; 3],
            vec![
                node("RootBone", [0.; 3], bones),
                node("RendererA", [0.; 3], vec![]),
                node("RendererB", [0.; 3], vec![]),
            ],
        )],
    );
    skin.r = e::mirror_rotation(e::euler(20., 30., 40.));
    skin.s = [1., 2., 0.5];
    let primitives = PRIMITIVES
        .iter()
        .zip(0u8..)
        .map(|((name, _), i)| node(name, [f64::from(i) * 3., 0., 0.], vec![]))
        .collect();
    let trees: [(&str, Node); 4] = [
        ("skin", skin),
        ("primitives", node("primitives", [0.; 3], primitives)),
        (
            "materials",
            node(
                "materials",
                [0.; 3],
                vec![
                    node("Metallic", [-1., 0., 0.], vec![]),
                    node("Unlit", [1., 0., 0.], vec![]),
                ],
            ),
        ),
        (
            "lod",
            node(
                "lod",
                [0.; 3],
                vec![node("Near", [0.; 3], vec![]), node("Far", [0.; 3], vec![])],
            ),
        ),
    ];
    for (name, tree) in &trees {
        let glb = prefab(run, name, DEFAULT, &UnlitExtras);
        let roots = glb.roots();
        assert_eq!(roots[0], 0, "{name}: the prefab root comes first");
        e::assert_tree(&glb, roots[0], tree, &run.version);
        // Everything after the prefab root is an identity-TRS attachment.
        for &root in &roots[1..] {
            let n = &glb.nodes()[root];
            assert!(
                n.get("mesh").is_some() && n.get("children").is_none(),
                "{n}"
            );
            assert_eq!(n["translation"], json!([0, 0, 0]));
            assert_eq!(n["rotation"], json!([0, 0, 0, 1]));
            assert_eq!(n["scale"], json!([1, 1, 1]));
        }
    }
}

/// `AddSkins` and `NewSkin`: two renderers over one four-influence panel,
/// bones `[RootBone, Upper, Right, Left]` rooted at `RootBone`, bind poses
/// identity and translations down, left and right, a two-frame `Lift`
/// channel at 25 and 80, and renderer weights 12.5 and 50.
#[test]
#[ignore = "set UNITYRS_STATIC_RUN_2022 and UNITYRS_STATIC_RUN_6000 to owner-built Editor fixture runs"]
fn editor_skins_and_morph_targets() {
    for run in e::runs(&["static-skin"]) {
        skins_and_morph_targets(&run);
    }
}
fn skins_and_morph_targets(run: &Run) {
    let glb = prefab(run, "skin", DEFAULT, &UnlitExtras);
    let bind = [[0., 0., 0.], [0., -1., 0.], [-1., 0., 0.], [1., 0., 0.]];
    let indices = [
        [0., 1., 2., 3.],
        [1., 0., 3., 2.],
        [2., 3., 0., 1.],
        [3., 2., 1., 0.],
    ];
    let mut meshes = Vec::new();
    for (renderer, weight) in [("RendererA", 12.5), ("RendererB", 50.)] {
        let attached = glb.named(renderer, true);
        assert_eq!(attached["weights"], json!([weight / 80.]), "{renderer}");
        meshes.push(attached["mesh"].clone());
        let skin = &glb.json["skins"][e::usize(&attached["skin"])];
        let names: Vec<_> = skin["joints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|j| glb.nodes()[e::usize(j)]["name"].clone())
            .collect();
        assert_eq!(names, ["RootBone", "Upper", "Right", "Left"], "{renderer}");
        assert_eq!(glb.nodes()[e::usize(&skin["skeleton"])]["name"], "RootBone");
        // diag(-1, 1, 1, 1) B diag(-1, 1, 1, 1): a translation's x negates.
        let matrices = glb.accessor(&skin["inverseBindMatrices"]);
        assert_eq!(
            matrices.len(),
            names.len(),
            "{renderer}: one bind per joint"
        );
        assert_eq!(matrices.len(), bind.len(), "{renderer}");
        for (matrix, t) in matrices.iter().zip(bind) {
            let mut want = vec![0.; 16];
            for i in 0..4 {
                want[i * 5] = 1.;
            }
            want[12..15].copy_from_slice(&e::mirror(t));
            assert_eq!(*matrix, want, "{renderer} inverse bind");
        }
    }
    assert_eq!(meshes[0], meshes[1], "the renderers share one glTF mesh");
    let mesh = &glb.json["meshes"][e::usize(&meshes[0])];
    assert_eq!(mesh["extras"]["targetNames"], json!(["Lift"]));
    assert_eq!(mesh["extras"]["blend_shape_full_weights"], "25;80;");
    let primitive = &mesh["primitives"][0];
    let attributes = &primitive["attributes"];
    let positions: Vec<_> = e::PANEL.iter().map(|p| e::mirror(*p).to_vec()).collect();
    assert_eq!(glb.accessor(&attributes["POSITION"]), positions);
    let normal = e::mirror(e::PANEL_NORMAL).to_vec();
    assert_eq!(glb.accessor(&attributes["NORMAL"]), vec![normal; 4]);
    let uvs: Vec<_> = e::PANEL_UV.iter().map(|[u, v]| vec![*u, 1. - v]).collect();
    assert_eq!(glb.accessor(&attributes["TEXCOORD_0"]), uvs);
    let joints: Vec<_> = indices.iter().map(|row| row.to_vec()).collect();
    assert_eq!(glb.accessor(&attributes["JOINTS_0"]), joints);
    // The authored weights are dyadic, so f32 keeps them exactly.
    assert_eq!(
        glb.accessor(&attributes["WEIGHTS_0"]),
        vec![vec![0.5, 0.25, 0.1875, 0.0625]; 4]
    );
    assert_eq!(
        glb.accessor(&primitive["indices"]),
        reversed_panel_indices()
    );
    // The last frame's deltas, vertices 2 and 3 forward by 0.5 and 1.
    let deltas = [[0., 0., 0.], [0., 0., 0.], [0., 0., 0.5], [0., 0., 1.]];
    let target: Vec<_> = deltas.iter().map(|d| e::mirror(*d).to_vec()).collect();
    assert_eq!(glb.accessor(&primitive["targets"][0]["POSITION"]), target);
}

fn reversed_panel_indices() -> Vec<Vec<f64>> {
    e::PANEL_TRIANGLES
        .iter()
        .flat_map(|t| [t[2], t[1], t[0]])
        .map(|i| vec![f64::from(u8::try_from(i).unwrap())])
        .collect()
}

/// `BuildBatchScene`: `Zulu` at (3, 1, -2) and `Alpha` at (-1, 2, 4),
/// rotated 45 degrees about y and scaled (2, 1, 0.5), share one combined
/// mesh. Each renderer's range must land where Unity placed the panel.
#[test]
#[ignore = "set UNITYRS_STATIC_RUN_2022 and UNITYRS_STATIC_RUN_6000 to owner-built Editor fixture runs"]
fn editor_static_batch_placement() {
    for run in e::runs(&["static-batch-scene"]) {
        static_batch_placement(&run);
    }
}
fn static_batch_placement(run: &Run) {
    let glb = batch(run);
    let objects = [
        ("Zulu", [3., 1., -2.], [0., 0., 0., 1.], [1.; 3]),
        ("Alpha", [-1., 2., 4.], e::euler(0., 45., 0.), [2., 1., 0.5]),
    ];
    for (name, t, q, s) in objects {
        let original = glb.named(name, false);
        assert_eq!(original["extras"]["static_batched"], "true", "{name}");
        assert_eq!(e::floats(&original["translation"]), e::mirror(t));
        assert_eq!(e::floats(&original["scale"]), s);
        e::close(
            &e::floats(&original["rotation"]),
            &e::mirror_rotation(q),
            e::ROTATION,
            name,
        );
        let attached = glb.named(name, true);
        assert_eq!(
            attached["extras"]["source_path_id"],
            original["extras"]["path_id"]
        );
        // The range's vertices are already in world space: an identity
        // attachment places them once.
        assert_eq!(attached["translation"], json!([0, 0, 0]), "{name}");
        assert_eq!(attached["rotation"], json!([0, 0, 0, 1]), "{name}");
        assert_eq!(attached["scale"], json!([1, 1, 1]), "{name}");
        assert!(attached.get("matrix").is_none() && attached.get("children").is_none());
        assert!(
            glb.roots()
                .contains(&glb.nodes().iter().position(|n| n == attached).unwrap())
        );
        let primitive = &glb.json["meshes"][e::usize(&attached["mesh"])]["primitives"][0];
        let attributes = &primitive["attributes"];
        let (positions, normals, uvs) = (
            glb.accessor(&attributes["POSITION"]),
            glb.accessor(&attributes["NORMAL"]),
            glb.accessor(&attributes["TEXCOORD_0"]),
        );
        let corners: Vec<usize> = glb
            .accessor(&primitive["indices"])
            .iter()
            .map(|i| e::index(i[0]))
            .collect();
        assert_eq!(corners.len(), 6, "{name}: one panel's two triangles");
        let place = |v: [f64; 3]| {
            let r = e::rotate(q, [v[0] * s[0], v[1] * s[1], v[2] * s[2]]);
            e::mirror([r[0] + t[0], r[1] + t[1], r[2] + t[2]])
        };
        // Normals take the inverse transpose of the scale, then normalise.
        let n = e::PANEL_NORMAL;
        let n = e::rotate(q, [n[0] / s[0], n[1] / s[1], n[2] / s[2]]);
        let length = n.iter().map(|x| x * x).sum::<f64>().sqrt();
        let normal = e::mirror(n.map(|x| x / length));
        let authored = e::PANEL_TRIANGLES.iter().flat_map(|t| [t[2], t[1], t[0]]);
        for (corner, vertex) in corners.into_iter().zip(authored) {
            let label = format!("{} {name} corner {vertex}", run.version);
            e::close(
                &positions[corner],
                &place(e::PANEL[vertex]),
                e::PLACEMENT,
                &label,
            );
            e::close(&normals[corner], &normal, e::PLACEMENT, &label);
            let [u, v] = e::PANEL_UV[vertex];
            assert_eq!(uvs[corner], [u, 1. - v], "{label}");
        }
    }
    // The scene's roots are its three parentless Transforms, then the two
    // attachments. Their order is not compared: the Editor keeps no authored
    // root order in the build, and the same authored Zulu, Alpha and marker
    // take a different object-table order from run to run.
    let roots: Vec<_> = glb
        .roots()
        .iter()
        .map(|&r| glb.nodes()[r]["name"].clone())
        .collect();
    assert_eq!(roots.len(), 5);
    let mut originals: Vec<_> = roots[..3].iter().map(|n| n.as_str().unwrap()).collect();
    originals.sort_unstable();
    let mut authored = vec!["Zulu", "Alpha", run.stamp.as_str()];
    authored.sort_unstable();
    assert_eq!(originals, authored);
    let mut attachments: Vec<_> = roots[3..].iter().map(|n| n.as_str().unwrap()).collect();
    attachments.sort_unstable();
    assert_eq!(attachments, ["Alpha", "Zulu"]);
}

/// Each primitive is `GameObject.CreatePrimitive(type)`, named after its
/// type, so its `MeshFilter` names the built-in mesh of that type. The
/// default material is copied into an asset bundle, never referenced.
#[test]
#[ignore = "set UNITYRS_STATIC_RUN_2022 and UNITYRS_STATIC_RUN_6000 to owner-built Editor fixture runs"]
fn editor_builtin_mesh_file_ids() {
    for run in e::runs(&[
        "static-primitives",
        "static-materials",
        "static-lod",
        "static-skin",
    ]) {
        builtin_mesh_file_ids(&run);
    }
}
fn builtin_mesh_file_ids(run: &Run) {
    let builtin = |glb: &Glb, name: &str, kind: &str| {
        let id = PRIMITIVES.iter().find(|(n, _)| *n == kind).unwrap().1;
        assert_eq!(
            glb.named(name, false)["extras"]["unity_builtin"],
            json!([{"file": "unity default resources", "id": id, "name": kind}]),
            "{} {name}",
            run.version
        );
    };
    let glb = prefab(run, "primitives", DEFAULT, &UnlitExtras);
    for (name, _) in PRIMITIVES {
        builtin(&glb, name, name);
    }
    assert_eq!(glb.issues("builtin_placeholder"), 6);
    let glb = prefab(run, "materials", DEFAULT, &UnlitExtras);
    builtin(&glb, "Metallic", "Cube");
    builtin(&glb, "Unlit", "Quad");
    // LOD 0 holds Near; the default profile exports only that level.
    let glb = prefab(run, "lod", DEFAULT, &UnlitExtras);
    builtin(&glb, "Near", "Sphere");
    assert!(
        glb.named("Far", false)["extras"]
            .get("unity_builtin")
            .is_none()
    );
    // `AddSkins` assigns the Cube's default material; the bundle holds its
    // own copy, so the skin maps a local material and names no built-in.
    let glb = prefab(run, "skin", DEFAULT, &UnlitExtras);
    let material = glb.material("Default-Material");
    assert_eq!(material["extras"]["shader"], "Standard");
    assert!(!glb.json.to_string().contains("unity_builtin"));
}

fn linear(c: [f32; 4]) -> Vec<f64> {
    material_support::linear_color(c.map(f64::from)).to_vec()
}
/// A material's base colour texture binding, which must exist, on set 0.
fn bound<'a>(factors: &'a Value, material: &str) -> &'a Value {
    let base = &factors["baseColorTexture"];
    assert!(base["index"].is_u64(), "{material}: no base colour texture");
    assert!(
        base.get("texCoord").is_none_or(|set| set == 0),
        "{material}"
    );
    base
}
fn transform(info: &Value) -> &Value {
    &info["extensions"]["KHR_texture_transform"]
}

/// `BuildMaterials`: Standard `Metallic` (colour (.4, .6, .8), metallic
/// .625, glossiness .375, emission (.125, .25, .5), the checker scaled
/// (2, 3) and offset (.25, .5)) and `Unlit/Texture` `Unlit` (the checker).
/// A user-supplied resource file gives their Cube and Quad UVs.
#[test]
#[ignore = "set UNITYRS_STATIC_RUN_2022 and UNITYRS_STATIC_RUN_6000 to owner-built Editor fixture runs"]
fn editor_builtin_materials() {
    for run in e::runs(&["static-materials"]) {
        builtin_materials(&run);
    }
}
fn builtin_materials(run: &Run) {
    // `SetPixels32` fills rows bottom first; PNG output writes the top first.
    let checker: Vec<u8> = [
        [80, 20, 120, 128],
        [200, 240, 160, 0],
        [20, 100, 180, 255],
        [230, 180, 40, 255],
    ]
    .concat();
    let path = format!("{STATIC}materials.prefab");
    for (mapper, pbr) in [
        (&CorePbr as &dyn MaterialMapper, true),
        (&UnlitExtras, false),
    ] {
        let resolver = Some(e::Builtins::new());
        let glb = e::export(
            run,
            "static-objects",
            &Select::Prefab(&path),
            RESOLVE,
            mapper,
            resolver,
        );
        let metallic = glb.material("metallic");
        let factors = &metallic["pbrMetallicRoughness"];
        material_support::assert_factor(&factors["baseColorFactor"], &linear([0.4, 0.6, 0.8, 1.]));
        let base = bound(factors, "metallic");
        assert_eq!(transform(base)["scale"], json!([2, 3]));
        assert_eq!(transform(base)["offset"], json!([0.25, -2.5]));
        let image = &glb.json["textures"][e::usize(&base["index"])]["source"];
        material_support::assert_pixels_sized(&glb.image(image), (2, 2), &checker);
        if pbr {
            assert_eq!(factors["metallicFactor"], 0.625);
            assert_eq!(factors["roughnessFactor"], 0.625);
            material_support::assert_factor(
                &metallic["emissiveFactor"],
                &linear([0.125, 0.25, 0.5, 1.])[..3],
            );
            assert!(metallic.get("extensions").is_none(), "{metallic}");
        } else {
            assert_eq!(metallic["extensions"], json!({"KHR_materials_unlit": {}}));
        }
        assert!(metallic.get("alphaMode").is_none() && metallic["doubleSided"] == false);
        let unlit = glb.material("unlit");
        assert_eq!(unlit["extensions"], json!({"KHR_materials_unlit": {}}));
        assert_eq!(unlit["extras"]["shader"], "Unlit/Texture");
        let base = bound(&unlit["pbrMetallicRoughness"], "unlit");
        assert!(transform(base).is_null(), "{unlit}");
        let image = &glb.json["textures"][e::usize(&base["index"])]["source"];
        material_support::assert_pixels_sized(&glb.image(image), (2, 2), &checker);
    }
}

/// `BuildUrpMaterials`: `UrpLit` (base colour (.25, .5, .75), the 4x4 base
/// map scaled (2, 3) and offset (.25, .5), a normal map at scale 1.5,
/// metallic .625, smoothness .375, emission (.125, .25, .5)) and `UrpUnlit`
/// (base colour (1, .5, .25), alpha clip at .375).
#[test]
#[ignore = "set UNITYRS_STATIC_URP_RUN_6000 to an owner-built URP Editor fixture run"]
fn editor_urp_materials() {
    let run = e::run(
        "UNITYRS_STATIC_URP_RUN_6000",
        "6000.6.",
        "urp",
        &["urp-materials"],
    );
    // C#'s byte casts wrap, as `as u8` does.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pixels: Vec<u8> = (0..16i32)
        .map(|i| {
            [
                (40 * i) as u8,
                (255 - 30 * i) as u8,
                (90 + 10 * i) as u8,
                if i % 3 == 0 { 64 } else { 255 },
            ]
        })
        .collect::<Vec<_>>()
        .chunks(4)
        .rev()
        .flatten()
        .flatten()
        .copied()
        .collect();
    let path = "assets/unityrsfixtures/urp/materials.prefab";
    for (mapper, pbr) in [
        (&CorePbr as &dyn MaterialMapper, true),
        (&UnlitExtras, false),
    ] {
        let resolver = Some(e::Builtins::new());
        let glb = e::export(
            &run,
            "urp-materials",
            &Select::Prefab(path),
            RESOLVE,
            mapper,
            resolver,
        );
        let lit = glb.material("lit");
        let factors = &lit["pbrMetallicRoughness"];
        material_support::assert_factor(
            &factors["baseColorFactor"],
            &linear([0.25, 0.5, 0.75, 1.]),
        );
        let base = bound(factors, "lit");
        assert_eq!(transform(base)["scale"], json!([2, 3]));
        assert_eq!(transform(base)["offset"], json!([0.25, -2.5]));
        let image = &glb.json["textures"][e::usize(&base["index"])]["source"];
        material_support::assert_pixels_sized(&glb.image(image), (4, 4), &pixels);
        if pbr {
            assert_eq!(factors["metallicFactor"], 0.625);
            assert_eq!(factors["roughnessFactor"], 0.625);
            material_support::assert_factor(
                &lit["emissiveFactor"],
                &linear([0.125, 0.25, 0.5, 1.])[..3],
            );
            assert_eq!(lit["normalTexture"]["scale"], 1.5, "{lit}");
        }
        let unlit = glb.material("unlit");
        assert_eq!(unlit["extensions"], json!({"KHR_materials_unlit": {}}));
        let factors = &unlit["pbrMetallicRoughness"];
        material_support::assert_factor(&factors["baseColorFactor"], &linear([1., 0.5, 0.25, 1.]));
        assert_eq!(
            (&unlit["alphaMode"], &unlit["alphaCutoff"]),
            (&json!("MASK"), &json!(0.375))
        );
        let image = &glb.json["textures"][e::usize(&bound(factors, "unlit")["index"])]["source"];
        material_support::assert_pixels_sized(&glb.image(image), (4, 4), &pixels);
    }
}
