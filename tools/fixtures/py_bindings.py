"""Record Python API observations; Rust tests replay them without UnityPy.

    python tools/fixtures/py_bindings.py --check
"""

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))
import binding_cases as B
import UnityPy
import object_cases as O
import scratch

HELPERS = [
    ("binding_y3_values", "observe_enums"),
    ("binding_y3_values", "observe_versions"),
    ("binding_y3_tree", "observe"),
    ("binding_y3_tree", "observe_mutation"),
    ("binding_y3_config", "observe_config"),
    ("binding_y3_config", "observe_container"),
    ("binding_y3_config", "observe_config_loads"),
    ("binding_y3_config", "observe_peek"),
    ("binding_y3_objects", "observe_objects"),
    ("binding_y3_objects", "observe_head"),
]

EXPORTS = [
    ("binding_y4_images", name) for name in (
        "observe_textures", "observe_set_image", "observe_arrays", "observe_sprites", "observe_image_formats")
] + [("binding_y4_images_matrix", "observe_decode_matrix")] + [
    ("binding_y4_exports", name) for name in (
        "observe_audio", "observe_shader", "observe_mesh", "observe_surface",
        "observe_renderer_errors", "observe_audio_resources", "observe_renderer",
        "observe_shader_cases", "observe_audio_pcm")
] + [
    ("binding_y4_save", name) for name in (
        "observe_objects", "observe_files", "observe_environment", "observe_cabs", "observe_writers", "observe_legacy")
] + [
    ("binding_y4_review_save", name) for name in (
        "observe_serialized_mapping", "observe_serialized_payload", "observe_serialized_metadata",
        "observe_bundle_headers", "observe_legacy_headers", "observe_buffer_payloads")
] + [
    ("binding_y4_review_images", name) for name in ("observe_mipmap_arguments", "observe_buffer_budgets")
] + [
    ("binding_y4_review_object", name) for name in (
        "observe_reentrant_writer", "observe_numeric_writer", "observe_sequence_writer", "observe_nonnumeric_writer")
] + [
    ("binding_y4_review_exports", name) for name in (
        "observe_materials", "observe_shader_cap", "observe_output_security")
] + [
    ("binding_y4_round3", name) for name in (
        "observe_cab_budget", "observe_cab_length", "observe_cab_padding", "observe_typed_sequences",
        "observe_packer_threads", "observe_mesh_thread", "observe_shader_thread",
        "observe_sprite_thread", "observe_entry_threads", "observe_switch_invalid_counts")
] + [
    ("binding_y4_round4", name) for name in (
        "observe_pairs", "observe_managed", "observe_cab_mapping", "observe_export_buffers",
        "observe_strided_writes", "observe_output_names", "observe_bundle_views", "observe_path_budgets")
] + [
    ("binding_y4_round5", name) for name in (
        "observe_buffer_lengths", "observe_vector_lengths", "observe_cab_validation",
        "observe_array_storage", "observe_exact_output_names", "observe_snapshot_threads", "observe_array_inputs")
] + [
    ("binding_y4_round6", name) for name in (
        "observe_blob_lifetime", "observe_resource_inheritance", "observe_pointer_owner",
        "observe_payload_lengths", "observe_clear_replacement", "observe_detached_material")
] + [
    ("binding_y4_round7", name) for name in (
        "observe_streamed_lifetime", "observe_cab_protocol", "observe_deferred_replacements")
]

TOOLS = [("binding_y5_loader", "observe_" + name) for name in
         ("paths", "filesystem", "streams", "splits", "lookup")] + [
    ("binding_y5_objects", "observe_" + name) for name in
    ("patch", "forwarding", "gameobject", "containers", "iteration", "generators")
] + [("binding_y5_tools", "observe_tools_" + name) for name in
     ("dispatch", "files", "objects", "crawl", "extract")] + [
    ("binding_y5_modules", "observe_" + name) for name in
    ("reexports", "streams", "module_tables", "primitive_maps", "compression",
     "texture_converter", "converter_helpers", "export_modules", "texture_extension", "decrypt_key",
     "encrypted_load", "generator", "generator_cache")
]

RESIDUE = [("binding_y6_loader", "observe_" + name) for name in
           ("sensitive_path", "case_lookup", "hashes", "readers", "buffers",
            "pointer_case", "suffix", "urls", "symlinks")] + [
    ("binding_y6_objects", "observe_" + name) for name in
    ("references", "nonclass", "transform", "preload")
] + [("binding_y6_exports", "observe_" + name) for name in
     ("material_textures", "audio_float", "audio_options")] + [
    ("binding_y6_parse", "observe_" + name) for name in
    ("forced_raw", "forced_parsers", "forced_failures", "forced_positions")
] + [
    ("binding_y6_review", "observe_" + name) for name in
    ("detached_save", "detached_mark", "detached_raw", "detached_patch",
     "constructor_states", "compressed_failures")
]


def helper_records():
    import importlib
    references = {
        ("binding_y5_loader", "observe_filesystem"): "PARITY.md 19.2: custom filesystem reads use the same file opened as a stream",
        ("binding_y5_loader", "observe_streams"): "PARITY.md 19.2: non-string names retain keys and register textual CAB names; read-only streams load their bytes",
        ("binding_y6_loader", "observe_buffers"): "PARITY.md 19.2: mutable nameless buffers and read-only streams load their bytes reference",
        ("binding_y6_loader", "observe_pointer_case"): "PARITY.md 19.2: exact-case PPtr basename retry skips non-string keys",
        ("binding_y6_loader", "observe_suffix"): "PARITY.md 19.2: suffix lookup requires a separator boundary",
        ("binding_y6_loader", "observe_urls"): "PARITY.md 19.2: URL-prefixed environment paths retain the literal existence gate",
        ("binding_y6_objects", "observe_nonclass"): "PARITY.md 19.2: non-class attributes use the missing-type UnknownObject reference",
        ("binding_y6_exports", "observe_material_textures"): "PARITY.md 19.1: iterate material texture dictionary items and snapshot current resolved texture names",
        ("binding_y6_exports", "observe_audio_float"): "PARITY.md 16: PCM float bank VERSION refusal retains the fmod_toolkit subsound WAV reference",
        ("binding_y6_parse", "observe_forced_positions"): "PARITY.md 19.3: forced parsers explicitly refuse a nonzero cursor or non-default Assets/Bundle endian",
        ("binding_y5_objects", "observe_iteration"): "PARITY.md 19.2: file iteration follows nested containers and yields requested objects",
        ("binding_y4_images", "observe_set_image"): "PARITY.md 18.1: convert unsupported packer modes to RGBA",
        ("binding_y4_images", "observe_image_formats"): "PARITY.md 18.1: convert unsupported packer modes to RGBA",
        ("binding_y4_images", "observe_textures"): "PARITY.md 19.2: zero-size textures yield empty RGBA before fetching data",
        ("binding_y4_exports", "observe_audio"): "PARITY.md 19.2: empty audio resources yield no samples",
        ("binding_y4_exports", "observe_audio_resources"): "PARITY.md 19.2: empty audio resources yield no samples",
        ("binding_y4_review_exports", "observe_shader_cap"): "PARITY.md 19.3: parsed shader export honors max_block_output",
        ("binding_y4_review_exports", "observe_output_security"): "PARITY.md 19.3: binding asset output confinement",
        ("binding_y4_review_images", "observe_buffer_budgets"): "PARITY.md 19.3: charge buffer bytes before allocation",
        ("binding_y4_round3", "observe_cab_budget"): "PARITY.md 19.3: CAB snapshots and padding charge before allocation",
        ("binding_y4_round4", "observe_output_names"): "PARITY.md 19.3: validate output names and charge copies before allocation",
        ("binding_y4_round4", "observe_path_budgets"): "PARITY.md 19.3: fallible charged path staging",
        ("binding_y4_round6", "observe_blob_lifetime"): "PARITY.md 19.3: keep blob staging charged through image decode and encode",
        ("binding_y4_round7", "observe_streamed_lifetime"): "PARITY.md 19.3: retain streamed resource staging through blob staging and image decode",
        ("binding_y4_round6", "observe_pointer_owner"): "PARITY.md 19.1: correct texture dictionary iteration; preserve current pointer ownership",
    }
    for name, reason in (
        ("observe_cab_validation", "validate encoded CAB external paths before mutation"),
        ("observe_array_storage", "charge actual image array storage before slicing"),
        ("observe_exact_output_names", "normalize validated names before suffix construction"),
        ("observe_snapshot_threads", "release the GIL during native value staging"),
    ):
        references["binding_y4_round5", name] = "PARITY.md 19.3: " + reason
    records = []
    for observer, function in HELPERS + EXPORTS + TOOLS + RESIDUE:
        call = getattr(importlib.import_module(observer), function)
        observed = call(UnityPy)
        record = {"name": f"{observer}.{function}", "observer": observer,
                  "function": function, "expected": observed}
        if (observer, function) in references:
            # Keep the original oracle result beside the stated B/C reference.
            record["oracle"] = observed
            record["reference"] = references[observer, function]
            record["expected"] = call(UnityPy, corrected=True)
        records.append(record)
    return records


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if UnityPy.__version__ != "1.25.2":
        raise RuntimeError("requires UnityPy 1.25.2")
    records = []
    for name, data in B.cases():
        original = B.observe(UnityPy, data)
        expected = B.observe(UnityPy, data, corrected=True)
        record = {"name": name, "hex": data.hex(), "expected": expected}
        if original != expected:
            record.update(oracle=original, reference="PARITY.md 19.2: file iteration follows nested containers and yields requested objects")
        records.append(record)
    with scratch.folder("unityrs-py-api-") as root:
        data = next(data for name, data in B.cases() if name == "text")
        original = B.observe_paths(UnityPy, data, root)
        expected = B.observe_paths(UnityPy, data, root, corrected=True)
        record = {"name": "paths", "hex": data.hex(), "expected": expected}
        if original != expected:
            record.update(oracle=original, reference="PARITY.md 19.2: file iteration follows nested containers and yields requested objects")
        records.append(record)
    records.extend(helper_records())
    encoded = "[\n" + ",\n".join(json.dumps(r, separators=(",", ":")) for r in records) + "\n]\n"
    out = ROOT / "crates/unityrs-py/tests/fixtures/api.json"
    if args.check:
        if out.read_text() != encoded:
            raise SystemExit("Python API fixtures differ")
    else:
        out.write_text(encoded)
    print(f"Python API: {len(records)} synthetic API fixtures")
    objects = [{"name": name, "hex": data.hex(), "expected": O.observe(UnityPy, data)} for name, data in O.cases()]
    encoded = "[\n" + ",\n".join(json.dumps(r, separators=(",", ":")) for r in objects) + "\n]\n"
    out = ROOT / "crates/unityrs-py/tests/fixtures/object_api.json"
    if args.check:
        if out.read_text() != encoded:
            raise SystemExit("Y2 object API fixtures differ")
    else:
        out.write_text(encoded)
    print(f"Y2: {len(objects)} synthetic object API fixtures")


if __name__ == "__main__":
    main()
