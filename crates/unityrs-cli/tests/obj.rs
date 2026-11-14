use std::process::Command;
use unityrs_testkit::{
    child,
    scene3d::{
        self, Layout,
        mesh::{MeshVersion, mesh},
    },
    scratch::Scratch,
};
#[path = "../../unityrs-mesh/tests/support/export_cases.rs"]
#[allow(
    dead_code,
    reason = "shared recorder cases include oracle-only metadata"
)]
mod cases;

fn run_case(case: &cases::Case, output: &std::path::Path) -> std::process::Output {
    let tmp = Scratch::new("obj-cli-input");
    let input = tmp.join("input.assets");
    std::fs::write(
        &input,
        scene3d::file(Layout::Unity6000, &[], &case.objects).unwrap(),
    )
    .unwrap();
    child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("obj")
            .arg(&input)
            .args(["--path-id", &case.target.to_string(), "-o"])
            .arg(output),
    )
    .unwrap()
}

#[test]
fn cut_triangle_leaves_no_partial_obj() {
    let tmp = Scratch::new("obj-cli-cut");
    let case = cases::cases()
        .into_iter()
        .find(|c| c.name == "cut_triangle")
        .unwrap();
    let result = run_case(&case, &tmp.join("mesh.obj"));
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("triangle tuple"));
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn unsafe_texture_name_discards_all_renderer_output() {
    let tmp = Scratch::new("obj-cli-unsafe");
    let mut case = cases::cases()
        .into_iter()
        .find(|c| c.name == "textures")
        .unwrap();
    // The first texture saves tex.png; the next one has an unsafe name.
    scene3d::set(
        &mut case.objects[6].value,
        "m_Name",
        unityrs_testkit::ngui::s("tex"),
    );
    scene3d::set(
        &mut case.objects[5].value,
        "m_Name",
        unityrs_testkit::ngui::s("a/"),
    );
    let result = run_case(&case, tmp.path());
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unsafe output name"));
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn bare_assertion_has_a_cli_reason() {
    let tmp = Scratch::new("obj-cli-assertion");
    let mut case = cases::cases()
        .into_iter()
        .find(|c| c.name == "missing_uv_normals")
        .unwrap();
    let unityrs_testkit::ngui::V::Struct(_, fields) = &mut case.objects[0].value else {
        panic!("mesh")
    };
    fields.retain(|(name, _)| name != "m_VertexData");
    let result = run_case(&case, &tmp.join("mesh.obj"));
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unityrs obj: assertion failed"));
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn renderer_directory_destination_refuses_before_replacing_existing_texture() {
    let tmp = Scratch::new("obj-cli-publish");
    let case = cases::cases()
        .into_iter()
        .find(|c| c.name == "textures")
        .unwrap();
    std::fs::write(tmp.join("tex.png"), b"existing texture").unwrap();
    std::fs::create_dir(tmp.join("m.mtl")).unwrap();
    let result = run_case(&case, tmp.path());
    assert!(!result.status.success());
    let reason = String::from_utf8_lossy(&result.stderr);
    assert!(
        reason.contains("m.mtl") && reason.contains("directory"),
        "{reason}"
    );
    assert_eq!(
        std::fs::read(tmp.join("tex.png")).unwrap(),
        b"existing texture"
    );
    let mut entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    entries.sort();
    assert_eq!(entries, ["m.mtl", "tex.png"]);
}

#[test]
fn renderer_publishes_textures_and_text_after_success() {
    let tmp = Scratch::new("obj-cli-renderer");
    let case = cases::cases()
        .into_iter()
        .find(|c| c.name == "textures")
        .unwrap();
    std::fs::write(tmp.join("tex.png"), b"existing texture").unwrap();
    let result = run_case(&case, tmp.path());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        ["_BumpMap.png", "_MainTex.png", "m.mtl", "m.obj", "tex.png"]
    );
    assert!(
        std::fs::read(tmp.join("tex.png"))
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
    assert!(
        std::fs::read_to_string(tmp.join("m.mtl"))
            .unwrap()
            .ends_with("map_Kd tex.png\nmap_bump _BumpMap.png\nbump _BumpMap.png\nmap_Ks tex.png")
    );
}
#[test]
fn obj_command_writes_raw_text() {
    let tmp = Scratch::new("obj-cli");
    let input = tmp.join("input.assets");
    let out = tmp.join("mesh.obj");
    std::fs::write(
        &input,
        scene3d::file(
            Layout::Unity2022,
            &[],
            &[mesh(
                4,
                b"m",
                &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                &[0, 1, 2],
                MeshVersion::V2022,
            )],
        )
        .unwrap(),
    )
    .unwrap();
    let result = child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("obj")
            .arg(&input)
            .args(["--path-id", "4", "-o"])
            .arg(&out),
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(out).unwrap(),
        b"g m\nv -0 0 0\nv -1 0 0\nv -0 1 0\ng m_0\nf 3/3/3 2/2/2 1/1/1\n"
    );
}
