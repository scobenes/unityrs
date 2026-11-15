//! Hash-verified libwebp 1.3.0, patched in `OUT_DIR`, generic C on every architecture.
//! `HAVE_CONFIG_H` suppresses cpu.h's automatic SIMD selection. All external
//! symbols are prefixed by webp-symbols.h and hidden (including DSP globals).
//! Every malloc/calloc/free calls Rust; no C allocation bypasses the meter.
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::{env, fs, path::Path};

#[path = "build_webp_patch.rs"]
mod patch;

const PATCHES: [(&str, &str); 2] = [
    (
        "0001-alpha-initialize-bitwriter.patch",
        "src/enc/alpha_enc.c",
    ),
    ("0002-palette-free-cooccurrence.patch", "src/enc/vp8l_enc.c"),
];

pub fn build(out: &Path) {
    let root = Path::new("vendor");
    let manifest = fs::read_to_string(root.join("libwebp-1.3.0.sha256")).expect("WebP manifest");
    let mut build = cc::Build::new();
    let derived = out.join("libwebp-1.3.0");
    let config = out.join("src/webp");
    fs::create_dir_all(&config).expect("WebP config directory");
    fs::write(config.join("config.h"), "/* Generic C, no SIMD. */\n").expect("WebP config");
    build
        .include(out)
        .include(&derived)
        .define("HAVE_CONFIG_H", None)
        // types.h otherwise overrides hidden visibility on public functions.
        .define("WEBP_EXTERN", Some("extern"))
        .define("WEBP_USE_THREAD", None)
        .warnings(false)
        .opt_level(3)
        .flag_if_supported("-std=c99")
        .flag_if_supported("-fvisibility=hidden")
        .flag("-include")
        .flag("webp-alloc.h")
        .flag("-include")
        .flag("webp-symbols.h")
        .file("webp-shim.c");
    for line in manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (sha, name) = line.split_once("  ").expect("WebP manifest line");
        if name.ends_with(".tar.gz") {
            continue;
        }
        let path = root.join(name);
        let bytes = fs::read(&path).expect("WebP source");
        let digest = Sha256::digest(&bytes)
            .iter()
            .fold(String::new(), |mut s, b| {
                write!(s, "{b:02x}").expect("format hash");
                s
            });
        assert_eq!(digest, sha, "vendored WebP changed: {name}");
        let relative = name.strip_prefix("libwebp-1.3.0/").unwrap();
        let copied = derived.join(relative);
        fs::create_dir_all(copied.parent().unwrap()).expect("WebP source directory");
        fs::write(&copied, &bytes).expect("copy pristine WebP source into OUT_DIR");
        if Path::new(name).extension().is_some_and(|e| e == "c") {
            build.file(&copied);
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for (name, target) in PATCHES {
        let path = root.join("patches").join(name);
        let diff = fs::read_to_string(&path).expect("WebP patch");
        let source = derived.join(target);
        let original = fs::read_to_string(&source).expect("WebP patch source");
        fs::write(&source, patch::apply(&original, &diff, target)).expect("patched WebP source");
        println!("cargo:rerun-if-changed={}", path.display());
    }
    build.compile("unityrs_webp");
    for file in [
        "build_webp.rs",
        "build_webp_patch.rs",
        "webp-shim.c",
        "webp-alloc.h",
        "webp-symbols.h",
        "vendor/libwebp-1.3.0.sha256",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    println!(
        "cargo:rustc-env=UNITYRS_IMAGEIO_WEBP_ARCHIVE={}",
        out.join("libunityrs_webp.a").display()
    );
    if !env::var("TARGET").unwrap_or_default().contains("msvc") {
        println!("cargo:rustc-link-lib=m");
    }
}
