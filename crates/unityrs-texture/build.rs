// Ported from Arm astcenc commit 631fb04 (Apache-2.0): the exact original
// aligned allocation templates are pinned below for the local build adapter.
// Build copies also make configuration selection local and angular table
// initialization run once, so independent caller contexts never race.
// See LICENSE-ASTCENC. All vendored source files remain unmodified.
#[cfg(feature = "encode")]
use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn main() {
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=encoder_bridge.cpp");
    println!("cargo:rerun-if-changed=encoder_allocator.hpp");
    #[cfg(feature = "encode")]
    build();
}
#[cfg(feature = "encode")]
fn replace(source: &mut String, old: &str, new: &str, count: usize) {
    assert_eq!(
        source.matches(old).count(),
        count,
        "upstream allocation patch drift: {old}"
    );
    *source = source.replace(old, new);
}
#[cfg(feature = "encode")]
const ORIGINAL_ALLOCATOR: &str = r"template<typename T>
T* aligned_malloc(size_t size, size_t align)
{
	void* ptr;
	int error = 0;

	// Don't allow this to under-align a type
	size_t min_align = astc::max(alignof(T), sizeof(void*));
	size_t real_align = astc::max(min_align, align);

#if defined(_WIN32)
	ptr = _aligned_malloc(size, real_align);
#else
	error = posix_memalign(&ptr, real_align, size);
#endif

	if (error || (!ptr))
	{
		return nullptr;
	}

	return static_cast<T*>(ptr);
}

/**
 * @brief Free an aligned memory buffer.
 *
 * @param ptr   The buffer to free.
 */
template<typename T>
void aligned_free(T* ptr)
{
#if defined(_WIN32)
	_aligned_free(ptr);
#else
	free(ptr);
#endif
}
";
#[cfg(feature = "encode")]
fn patch(path: &Path, dest: &Path) {
    let name = path.file_name().unwrap().to_str().unwrap();
    let mut s = fs::read_to_string(path).unwrap();

    match name {
        "astcenc_internal.h" => {
            replace(
                &mut s,
                ORIGINAL_ALLOCATOR,
                "template<typename T>\nT* aligned_malloc(size_t size, size_t align) { return static_cast<T*>(unityrs_astc_allocate(size, astc::max(align, alignof(T)))); }\ntemplate<typename T>\nvoid aligned_free(T* ptr) { unityrs_astc_release(ptr); }",
                1,
            );
        }
        "astcenc_entry.cpp" => {
            replace(
                &mut s,
                "static const std::array<astcenc_preset_config, 6>* preset_configs;",
                "const std::array<astcenc_preset_config, 6>* preset_configs;",
                1,
            );
            replace(
                &mut s,
                "new astcenc_context",
                "unityrs_astc_new<astcenc_context>()",
                1,
            );
            replace(&mut s, "delete ctxo;", "unityrs_astc_delete(ctxo);", 4);
            replace(
                &mut s,
                "new vfloat4[ag.work_memory_size]",
                "unityrs_astc_array<vfloat4>(ag.work_memory_size)",
                1,
            );
            replace(
                &mut s,
                "new float[texel_count]",
                "unityrs_astc_array<float>(texel_count)",
                1,
            );
            replace(
                &mut s,
                "delete[] arg.work_memory;",
                "unityrs_astc_release(arg.work_memory);",
                1,
            );
            replace(
                &mut s,
                "delete[] ctx->input_alpha_averages;",
                "unityrs_astc_release(ctx->input_alpha_averages);",
                1,
            );
            replace(
                &mut s,
                "prepare_angular_tables();",
                "static const bool angular_ready = []() { prepare_angular_tables(); return true; }();\n\t(void)angular_ready;",
                1,
            );
        }
        "astcenc_block_sizes.cpp" => {
            replace(
                &mut s,
                "new dt_init_working_buffers",
                "unityrs_astc_new<dt_init_working_buffers>()",
                2,
            );
            replace(&mut s, "delete wb;", "unityrs_astc_delete(wb);", 2);
            replace(
                &mut s,
                "delete[] percentiles;",
                "unityrs_astc_release(percentiles);",
                1,
            );
        }
        "astcenc_partition_tables.cpp" => {
            replace(
                &mut s,
                "new uint64_t[BLOCK_MAX_PARTITIONINGS * BIT_PATTERN_WORDS]",
                "unityrs_astc_array<uint64_t>(BLOCK_MAX_PARTITIONINGS * BIT_PATTERN_WORDS)",
                1,
            );
            replace(
                &mut s,
                "delete[] canonical_patterns;",
                "unityrs_astc_release(canonical_patterns);",
                1,
            );
        }
        "astcenc_percentile_tables.cpp" => replace(
            &mut s,
            "new float[WEIGHTS_MAX_BLOCK_MODES]",
            "unityrs_astc_array<float>(WEIGHTS_MAX_BLOCK_MODES)",
            1,
        ),
        _ => {}
    }
    if path.extension().is_some_and(|ext| ext == "cpp") || name == "astcenc_internal.h" {
        s.insert_str(0, "#include \"encoder_allocator.hpp\"\n");
    }
    fs::write(dest, s).unwrap();
}
#[cfg(feature = "encode")]
fn build() {
    let dir = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("astcenc");
    fs::create_dir_all(&dir).unwrap();
    let mut astc_files = Vec::new();
    for entry in fs::read_dir("vendor/astcenc").unwrap() {
        let path = entry.unwrap().path();
        let dst = dir.join(path.file_name().unwrap());
        patch(&path, &dst);
        if path.extension().is_some_and(|e| e == "cpp") {
            astc_files.push(dst);
        }
    }
    astc_files.sort();
    let mut b = cc::Build::new();
    b.cpp(true)
        .std("c++20")
        .opt_level(2)
        .warnings(false)
        .define("NDEBUG", None)
        .include(".")
        .include("vendor/etcpak")
        .include(&dir)
        .flag_if_supported("-ffp-contract=off")
        .flag_if_supported("-fno-fast-math")
        .define("ASTCENC_NEON", "0")
        .define("ASTCENC_SVE", "0")
        .define("ASTCENC_SSE", "0")
        .define("ASTCENC_AVX", "0")
        .define("ASTCENC_POPCNT", "0")
        .define("ASTCENC_F16C", "0")
        .flag_if_supported("-U__ARM_NEON")
        .flag_if_supported("-U__ARM_NEON__")
        .flag_if_supported("-U__AVX__")
        .flag_if_supported("-U__SSE4_1__")
        .flag_if_supported("-U__AVX2__")
        .flag_if_supported("-U__AVX512F__")
        .flag_if_supported("-U__AVX512BW__")
        .flag_if_supported("-U__AVX512VL__");
    for file in [
        "ProcessDxtc.cpp",
        "ProcessRGB.cpp",
        "bc7enc.cpp",
        "Dither.cpp",
        "Tables.cpp",
    ] {
        b.file(Path::new("vendor/etcpak").join(file));
    }
    b.files(astc_files)
        .file("encoder_bridge.cpp")
        .compile("unityrs_texture_encoders");
}
