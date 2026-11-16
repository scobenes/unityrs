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
        "observe_bundle_headers", "observe_legacy_headers")
] + [
    ("binding_y4_review_images", "observe_mipmap_arguments")
] + [
    ("binding_y4_review_object", name) for name in (
        "observe_reentrant_writer", "observe_numeric_writer")
] + [
    ("binding_y4_review_exports", name) for name in (
        "observe_materials", "observe_shader_cap", "observe_output_security")
]


def helper_records():
    import importlib
    references = {
        ("binding_y4_images", "observe_set_image"): "PARITY.md 18.1: convert unsupported packer modes to RGBA",
        ("binding_y4_images", "observe_image_formats"): "PARITY.md 18.1: convert unsupported packer modes to RGBA",
        ("binding_y4_images", "observe_textures"): "PARITY.md 19.2: zero-size textures yield empty RGBA before fetching data",
        ("binding_y4_exports", "observe_audio"): "PARITY.md 19.2: empty audio resources yield no samples",
        ("binding_y4_exports", "observe_audio_resources"): "PARITY.md 19.2: empty audio resources yield no samples",
        ("binding_y4_review_exports", "observe_shader_cap"): "PARITY.md 19.3: parsed shader export honors max_block_output",
        ("binding_y4_review_exports", "observe_output_security"): "PARITY.md 19.3: binding asset output confinement",
    }
    records = []
    for observer, function in HELPERS + EXPORTS:
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
    records = [{"name": name, "hex": data.hex(), "expected": B.observe(UnityPy, data)} for name, data in B.cases()]
    with scratch.folder("unityrs-py-api-") as root:
        data = next(data for name, data in B.cases() if name == "text")
        records.append({"name": "paths", "hex": data.hex(), "expected": B.observe_paths(UnityPy, data, root)})
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
