//! Compile pristine liblz4 1.9.4, as bundled by python-lz4 4.4.4.
//! Public function names are prefixed at compilation, leaving the vendored
//! originals byte for byte intact. No C heap allocation is enabled: HC uses
//! the caller's fallibly allocated, metered state.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let dir = Path::new("vendor/lz4-1.9.4");
    let mut names = BTreeSet::new();
    for header in ["lz4.h", "lz4hc.h"] {
        let text = fs::read_to_string(dir.join(header)).expect("read pristine LZ4 header");
        // All exported LZ4 functions contain a lowercase letter. Constants
        // and function-like annotation macros use uppercase names only.
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at].is_ascii_alphabetic() || bytes[at] == b'_' {
                let start = at;
                while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                    at += 1;
                }
                let word = &text[start..at];
                let mut after = at;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if word.starts_with("LZ4")
                    && word.bytes().any(|b| b.is_ascii_lowercase())
                    && bytes.get(after) == Some(&b'(')
                {
                    names.insert(word.to_owned());
                }
            } else {
                at += 1;
            }
        }
    }
    let mut build = cc::Build::new();
    build
        .include(dir)
        .define("LZ4_STATIC_LINKING_ONLY_DISABLE_MEMORY_ALLOCATION", "1")
        .define("LZ4_HEAPMODE", "0")
        .define("LZ4HC_HEAPMODE", "0")
        .warnings(false)
        .opt_level(3)
        .flag_if_supported("-std=c99")
        .flag_if_supported("-fvisibility=hidden");
    for name in names {
        build.define(&name, format!("unityrs_{name}").as_str());
    }
    // These non-static internals have no declarations in the public headers.
    // Keep them private by name as well as ELF/Mach-O symbol visibility.
    for name in [
        "LZ4_compress_forceExtDict",
        "LZ4_decompress_safe_forceExtDict",
        "LZ4_decompress_safe_partial_forceExtDict",
        "read_long_length_no_check",
    ] {
        build.define(name, format!("unityrs_{name}").as_str());
    }
    for file in ["lz4.c", "lz4hc.c"] {
        build.file(dir.join(file));
    }
    build.compile("unityrs_lz4hc");
    for file in ["lz4.c", "lz4hc.c", "lz4.h", "lz4hc.h"] {
        println!("cargo:rerun-if-changed={}", dir.join(file).display());
    }
    println!("cargo:rerun-if-changed=build.rs");
    let out = env::var("OUT_DIR").expect("OUT_DIR");
    println!("cargo:rustc-env=UNITYRS_CORE_LZ4_ARCHIVE={out}/libunityrs_lz4hc.a");
}
