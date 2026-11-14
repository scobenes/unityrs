# Construction from the repository's pinned synthetic oracle recordings.
# This never runs the batch engine or reads any engine output.
import hashlib
import json
from pathlib import Path

from model import write_case


def generate():
    cases = []
    selections = {
        "ngui": [
            "own_sheet_by_material_name",
            "struct_maintex_in_dependency",
            "material_in_dependency",
            "json_bytes_with_textures",
            "non_ascii",
            "raw_names",
        ],
        "sprites": [
            "own_sheets",
            "dependency_sheets",
            "alpha_textures",
            "atlas_lookup",
        ],
    }
    fields = [
        "bundles",
        "bundles_failed",
        "inputs",
        "inputs_complete",
        "inputs_with_skips",
        "inputs_failed",
        "inputs_not_run",
        "outputs_written",
        "outputs_not_written",
        "outputs_withheld",
        "outputs_failed",
        "outputs_unwritable",
        "skips",
        "skips_expected",
        "collisions",
        "flips",
        "archives_written",
    ]
    for family, names in selections.items():
        d = json.loads(
            Path(f"crates/unityrs-testkit/tests/fixtures/{family}.json").read_text()
        )
        for name in names:
            meta = next(s for s in d["scenes"] if s["name"] == name)
            results = [r for r in d["results"] if r["scene"] == name]
            assert len(results) == 1
            r = results[0]
            if family == "ngui":
                opts = meta["options"]
                outputs = {
                    "atlases": {
                        "sprite_key": opts["sprite_key"],
                        "json_key": opts["json_key"],
                        "script_classes": opts["script_classes"],
                        "skip_name_suffixes": opts["skip_name_suffixes"],
                    }
                }
                if opts["texture_key"]:
                    outputs["textures"] = {"key": opts["texture_key"]}
            else:
                outputs = {"sprites": {"key": meta["key"]}}
            template = {
                "spec_version": 1,
                "run_id": "golden",
                "attempt": 1,
                "threads": 1,
                "download_concurrency": 1,
                "max_inflight_uncompressed_bytes": 268435456,
                "max_decoded_bytes": 16777216,
                "dep_cache_bytes": 33554432,
                "spool": {
                    "dir": "$SPOOL",
                    "size_bytes": 1073741824,
                    "reserve_bytes": 0,
                },
                "inputs": [],
                "dependencies": [],
                "outputs": outputs,
                "sink": {"kind": "directory", "root": "$SINK"},
            }
            bundles = []
            for b in meta["bundles"]:
                dest = "inputs" if b["id"] in meta["inputs"] else "dependencies"
                template[dest].append(
                    {"id": b["id"], "path": "$SOURCE:" + b["id"], "deps": b["deps"]}
                )
                bundles.append(
                    {
                        "id": b["id"],
                        "status": "staged",
                        "reason": None,
                        "detail": None,
                        "size": "$BUNDLE_SIZE:" + b["id"],
                        "sha256": b["sha256"],
                        "peek": None,
                    }
                )
            rows = {
                "bundles": sorted(bundles, key=lambda b: b["id"]),
                "inputs": [],
                "skips": [],
                "diagnostics": [],
                "outputs": [],
                "collisions": [],
                "archives": [],
                "archive_members": [],
            }
            expected_outputs = {}
            for out in r["outputs"]:
                key = out["key"]
                md5 = out.get("png_md5", out.get("md5"))
                kind = out.get("kind", "sprite")
                visible = None
                if kind != "atlas_json":
                    pixels = bytes.fromhex(out["pixels"])
                    channels = {"L": 1, "LA": 2, "RGB": 3, "RGBA": 4}[out["mode"]]
                    rgba = bytearray()
                    for i in range(0, len(pixels), channels):
                        px = pixels[i : i + channels]
                        if channels == 1:
                            px = px * 3 + b"\xff"
                        elif channels == 2:
                            px = px[:1] * 3 + px[1:]
                        elif channels == 3:
                            px += b"\xff"
                        rgba.extend(px if px[3] else bytes(4))
                    w, h = out["size"]
                    visible = hashlib.sha256(f"{w}x{h}:".encode() + rgba).hexdigest()
                rows["outputs"].append(
                    {
                        "key": key,
                        "kind": kind,
                        "unit": out.get("unit"),
                        "input": r["input"],
                        "serialized_file": out["serialized_file"],
                        "path_id": out["path_id"],
                        "md5": md5,
                        "visible_hash": visible,
                        "mode": out.get("mode"),
                        "size": "$OUTPUT_SIZE:" + key,
                        "status": "published",
                        "reason": None,
                        "cut": False,
                    }
                )
                expected_outputs[key] = {"md5": md5}
                if "text" in out:
                    expected_outputs[key]["text"] = out["text"]
            rows["outputs"].sort(key=lambda o: o["key"])
            expected = 0
            for skip in r["skips"]:
                reason = skip["reason"]
                expected += int(
                    reason
                    in [
                        "empty_rect",
                        "empty_image",
                        "duplicate_name",
                        "reference_atlas",
                        "ngui_layout_unsupported",
                    ]
                    or reason.startswith("unsupported_platform(")
                    or (
                        reason.startswith("unsupported_format(")
                        and not reason[19:-1].isdecimal()
                    )
                )
                rows["skips"].append(
                    {
                        "input": r["input"],
                        "serialized_file": skip["serialized_file"],
                        "path_id": skip["path_id"],
                        "class": skip["cls"],
                        "output": skip["output"],
                        "sprite": skip.get("sprite"),
                        "reason": reason,
                        "detail": skip["detail"],
                        "cut": False,
                    }
                )
            rows["inputs"] = [
                {
                    "evictions": 0,
                    "id": r["input"],
                    "status": "complete_with_skips" if r["skips"] else "complete",
                    "reason": None,
                    "detail": None,
                    "skips": len(r["skips"]),
                    "unresolved_scripts": r.get("unresolved_scripts", 0),
                    "cut": False,
                }
            ]
            counts = dict.fromkeys(fields, 0)
            counts.update(
                bundles=len(bundles),
                inputs=1,
                inputs_with_skips=int(bool(r["skips"])),
                inputs_complete=int(not r["skips"]),
                outputs_written=len(r["outputs"]),
                skips=len(r["skips"]),
                skips_expected=expected,
            )
            code = 0 if expected == len(r["skips"]) else 3
            head = {
                "status": "complete" if code == 0 else "complete_with_skips",
                "exit_code": code,
                "stop_reason": None,
                "counts": counts,
                "warnings": dict.fromkeys(
                    [
                        "resume_list_failed",
                        "verify_list_failed",
                        "etag_not_md5",
                        "abort_failed",
                    ],
                    0,
                ),
                "run": dict.fromkeys(
                    ["same_named_cabs", "undeclared_dependencies", "nested_containers"],
                    0,
                ),
            }
            cases.append(
                {
                    "name": f"{family}-{name}",
                    "fixture": family + ":" + name,
                    "spec": template,
                    "exit_code": code,
                    "outputs": expected_outputs,
                    "normalized": {"head": head, "rows": rows},
                }
            )
    for case in cases:
        write_case(case)
