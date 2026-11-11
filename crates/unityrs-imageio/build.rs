//! Compiles the deflate side of the vendored zlib 1.2.13
//! (`vendor/zlib-1.2.13/`, pristine: `vendor/zlib-1.2.13.sha256`) into a
//! static library that only this crate links.
//!
//! - `Z_PREFIX`: zconf.h renames the symbols the library defines, internal
//!   ones included (`_tr_init`, `_dist_code`, `zcalloc`), to `z_*`, so none
//!   is a symbol an unprefixed zlib defines (a system `libz`, a C library
//!   bundling zlib, `libz-sys`'s static build), whose names it would
//!   otherwise take, interpose or clash with at a static link. All but
//!   one: zlib's own name for its error strings is already `z_errmsg`,
//!   which zconf.h leaves alone, so `z_errmsg` is defined to
//!   `unityrs_z_errmsg` as well. A second `Z_PREFIX` zlib in one static
//!   link would still clash on every `z_*` name. (`Z_SOLO` would also drop
//!   `zcalloc`, but makes zlib define its own `zmemcpy`, `zmemcmp` and
//!   `zmemzero`, names `Z_PREFIX` leaves alone; so it is not defined, and
//!   `zutil.c` includes `gzguts.h`, vendored for it. zlib's `zcalloc` is
//!   never called: the crate always passes `zalloc` and `zfree` of its
//!   own, `src/zlib.rs`.) `tests/zlib.rs` reads the archive's symbols.
//! - `-fvisibility=hidden`, where the compiler takes it: no symbol leaves a
//!   shared object this crate is linked into (a Python extension), so the
//!   library neither interposes on nor is interposed by a zlib loaded beside
//!   it.
//! - `-std=gnu99`: zlib 1.2.13 defines its functions in the K&R style, which
//!   C23 (GCC 15's default) no longer accepts.
//! - Optimised whatever the profile: deflate's bytes do not depend on it,
//!   and the debug build's tests deflate megabytes.
//!
//! Only these files are compiled: deflate (`deflate.c`, `trees.c`), the
//! checksums (`adler32.c` for the zlib stream, `crc32.c` for PNG chunks) and
//! `zutil.c`.

use std::env;
use std::path::Path;

const ZLIB: &str = "vendor/zlib-1.2.13";
const SOURCES: [&str; 5] = ["adler32.c", "crc32.c", "deflate.c", "trees.c", "zutil.c"];
const HEADERS: [&str; 7] = [
    "crc32.h",
    "deflate.h",
    "gzguts.h",
    "trees.h",
    "zconf.h",
    "zlib.h",
    "zutil.h",
];

fn main() {
    let dir = Path::new(ZLIB);
    let mut build = cc::Build::new();
    build
        .include(dir)
        .define("Z_PREFIX", None)
        .define("z_errmsg", Some("unityrs_z_errmsg"))
        .warnings(false)
        .opt_level(2)
        .flag_if_supported("-std=gnu99")
        .flag_if_supported("-fvisibility=hidden");
    for file in SOURCES {
        build.file(dir.join(file));
    }
    build.compile("unityrs_zlib");

    for file in SOURCES.iter().chain(HEADERS.iter()) {
        println!("cargo:rerun-if-changed={}", dir.join(file).display());
    }
    println!("cargo:rerun-if-changed=build.rs");
    // The target triple, for tests/flate2_backends.rs's `cargo metadata
    // --filter-platform`.
    let target = env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=UNITYRS_IMAGEIO_TARGET={target}");
    // The archive `compile` wrote, for tests/zlib.rs's symbol check.
    let out = env::var("OUT_DIR").unwrap_or_default();
    let archive = Path::new(&out).join("libunityrs_zlib.a");
    println!(
        "cargo:rustc-env=UNITYRS_IMAGEIO_ZLIB_ARCHIVE={}",
        archive.display()
    );
}
