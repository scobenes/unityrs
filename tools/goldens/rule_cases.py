import copy

from model import load_case, write_case


def generate():
    original = load_case("textures-five")
    source = next(x for x in original["texture_inputs"] if x["format"] == 4)
    output = next(
        x for x in original["normalized"]["rows"]["outputs"] if x["key"] == "RGBA32.png"
    )
    for name in [
        "zero",
        "failed-input",
        "failed-unused",
        "expected-only",
        "unexpected-skip",
        "budget",
        "withheld",
        "publish-incomplete",
    ]:
        c = copy.deepcopy(original)
        c["name"] = "rules-" + name
        c["fixture"] = "synthetic:" + name
        c["texture_inputs"] = []
        c["outputs"] = {}
        c["missing_sources"] = []
        c["spec"]["outputs"] = {"textures": {"key": "{name}.png"}}
        c["spec"]["dependencies"] = []
        rows = c["normalized"]["rows"]
        rows["outputs"] = []
        rows["skips"] = []
        rows["bundles"][0]["sha256"] = "$BUNDLE_SHA256:x.bundle"
        rows["inputs"][0].update(status="complete", skips=0, evictions=0)
        h = c["normalized"]["head"]
        h["status"] = "complete"
        h["exit_code"] = 0
        counts = h["counts"]
        counts.update(outputs_written=0)
        if name in ["failed-input", "failed-unused", "withheld", "publish-incomplete"]:
            missing = "x.bundle" if name == "failed-input" else "z"
            c["missing_sources"] = [missing]
            bundle = {
                "id": missing,
                "status": "failed",
                "reason": "source_missing",
                "detail": None,
                "size": None,
                "sha256": None,
                "peek": None,
            }
            summary = {
                "id": missing,
                "status": "failed",
                "reason": "source_missing",
                "detail": None,
                "skips": 0,
                "unresolved_scripts": 0,
                "cut": False,
                "evictions": None,
            }
            if name == "failed-input":
                rows["bundles"] = [bundle]
                rows["inputs"] = [summary]
            else:
                rows["bundles"].append(bundle)
                field = "dependencies" if name == "failed-unused" else "inputs"
                c["spec"][field].append({"id": "z", "path": "$SOURCE:z", "deps": []})
                if field == "inputs":
                    rows["inputs"].append(summary)
            counts.update(
                bundles=len(rows["bundles"]),
                bundles_failed=1,
                inputs=len(rows["inputs"]),
                inputs_failed=int(name != "failed-unused"),
                inputs_complete=int(name != "failed-input"),
            )
            h.update(status="incomplete", exit_code=4)
        if name in ["expected-only", "unexpected-skip"]:
            fmt = 1 if name == "expected-only" else 9999
            c["texture_inputs"] = [
                {"name": "skip", "format": fmt, "data": {"len": 1, "seed": 1}}
            ]
            reason = (
                "unsupported_format(Alpha8)" if fmt == 1 else "unsupported_format(9999)"
            )
            rows["skips"] = [
                {
                    "input": "x.bundle",
                    "serialized_file": "CAB-five",
                    "path_id": 1,
                    "class": "Texture2D",
                    "output": "texture",
                    "sprite": None,
                    "reason": reason,
                    "detail": None,
                    "cut": False,
                }
            ]
            rows["inputs"][0].update(status="complete_with_skips", skips=1)
            counts.update(
                inputs_complete=0,
                inputs_with_skips=1,
                skips=1,
                skips_expected=int(fmt == 1),
            )
            if fmt != 1:
                h.update(status="complete_with_skips", exit_code=3)
        if name == "budget":
            c["texture_inputs"] = [
                {
                    "name": "large",
                    "format": 4,
                    "size": [512, 512],
                    "data": {"zeros": 1048576},
                }
            ]
            c["spec"]["max_export_work"] = 1048576
            rows["inputs"][0].update(
                status="failed", reason="limit(export_work, 1048576)"
            )
            counts.update(inputs_complete=0, inputs_failed=1)
            h.update(status="incomplete", exit_code=5)
        if name in ["withheld", "publish-incomplete"]:
            c["texture_inputs"] = [source]
            c["spec"]["publish_incomplete"] = name == "publish-incomplete"
            out = copy.deepcopy(output)
            out["path_id"] = 1
            if name == "withheld":
                out.update(status="withheld", reason="publish_incomplete")
                counts["outputs_withheld"] = 1
            else:
                c["outputs"] = {"RGBA32.png": original["outputs"]["RGBA32.png"]}
                counts["outputs_written"] = 1
            rows["outputs"] = [out]
        c["exit_code"] = h["exit_code"]
        write_case(c)
