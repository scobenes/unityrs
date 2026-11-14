import hashlib
import json
import zlib
from pathlib import Path

from model import load_case, write_case


def generate():
    d = json.loads(Path("crates/unityrs-testkit/tests/fixtures/ngui.json").read_text())
    choice = next(c for c in d["winners"] if c["case"] == "lost_only")
    meta = next(c for c in d["scenes"] if c["name"] == "winners")
    case = load_case("ngui-own_sheet_by_material_name")
    case["name"] = "ngui-losing-units"
    case["fixture"] = "ngui:winners"
    inputs = choice["inputs"]
    case["spec"]["inputs"] = []
    case["spec"]["dependencies"] = []
    case["outputs"] = {}
    rows = case["normalized"]["rows"]
    rows.update(bundles=[], inputs=[], skips=[], outputs=[], collisions=[])
    options = meta["options"]
    case["spec"]["outputs"] = {
        "atlases": {
            "sprite_key": options["sprite_key"],
            "json_key": options["json_key"],
            "script_classes": options["script_classes"],
            "skip_name_suffixes": options["skip_name_suffixes"],
        }
    }
    counts = dict.fromkeys(case["normalized"]["head"]["counts"], 0)
    lookup = {}

    def visible(out):
        if out["kind"] == "atlas_json":
            return None
        px = bytes.fromhex(out["pixels"])
        channels = {"RGB": 3, "RGBA": 4}[out["mode"]]
        rgba = bytearray()
        for i in range(0, len(px), channels):
            p = px[i : i + channels]
            p = p + b"\xff" if channels == 3 else p
            rgba.extend(p if p[3] else bytes(4))
        w, h = out["size"]
        return hashlib.sha256(f"{w}x{h}:".encode() + rgba).hexdigest()

    def size(out):
        if out["kind"] == "atlas_json":
            return len(out["text"].encode())
        assert out["size"] == [1, 1]
        return 57 + len(zlib.compress(b"\0" + bytes.fromhex(out["pixels"]), 6))

    for identity in sorted(inputs):
        b = next(b for b in meta["bundles"] if b["id"] == identity)
        case["spec"]["inputs"].append(
            {"id": identity, "path": "$SOURCE:" + identity, "deps": b["deps"]}
        )
        rows["bundles"].append(
            {
                "id": identity,
                "status": "staged",
                "reason": None,
                "detail": None,
                "size": "$BUNDLE_SIZE:" + identity,
                "sha256": b["sha256"],
                "peek": None,
            }
        )
        r = next(
            r
            for r in d["results"]
            if r["scene"] == "winners" and r["input"] == identity
        )
        rows["inputs"].append(
            {
                "id": identity,
                "status": "complete_with_skips" if r["skips"] else "complete",
                "reason": None,
                "detail": None,
                "evictions": 0,
                "skips": len(r["skips"]),
                "unresolved_scripts": r["unresolved_scripts"],
                "cut": False,
            }
        )
        counts["inputs_with_skips" if r["skips"] else "inputs_complete"] += 1
        for out in r["outputs"]:
            lookup[identity, out["key"]] = out
        for skip in r["skips"]:
            rows["skips"].append(
                {
                    "input": identity,
                    "serialized_file": skip["serialized_file"],
                    "path_id": skip["path_id"],
                    "class": skip["cls"],
                    "output": skip["output"],
                    "sprite": skip.get("sprite"),
                    "reason": skip["reason"],
                    "detail": skip["detail"],
                    "cut": False,
                }
            )
    for decision in choice["decisions"]:
        key = decision["key"]
        winner = decision["winner"]
        out = lookup[winner, key]
        published = decision["status"] == "publish"
        md5 = out.get("png_md5", out.get("md5"))
        unit = out.get("unit")
        whash = visible(out)
        rows["outputs"].append(
            {
                "key": key,
                "kind": out["kind"],
                "unit": unit,
                "input": winner,
                "serialized_file": out["serialized_file"],
                "path_id": out["path_id"],
                "md5": md5,
                "visible_hash": whash,
                "mode": out.get("mode"),
                "size": size(out),
                "status": "published" if published else "withheld",
                "reason": decision["reason"],
                "cut": False,
            }
        )
        counts["outputs_written" if published else "outputs_withheld"] += 1
        if published:
            case["outputs"][key] = {"md5": md5}
            if out["kind"] == "atlas_json":
                case["outputs"][key]["text"] = out["text"]
        if len(decision["candidates"]) > 1:
            counts["collisions"] += 1
            counts["flips"] += int(decision["flip"])
            for identity, unit, rule, representative in decision["candidates"]:
                candidate = lookup[identity, key]
                chash = visible(candidate)
                rows["collisions"].append(
                    {
                        "key": key,
                        "input": identity,
                        "serialized_file": candidate["serialized_file"],
                        "md5": candidate.get("png_md5", candidate.get("md5")),
                        "visible_hash": chash,
                        "winner": representative,
                        "rule": rule,
                        "flip": decision["flip"],
                        "pixels_differ": bool(
                            not representative and whash and chash and whash != chash
                        ),
                        "cut": False,
                    }
                )
    counts.update(
        bundles=len(inputs),
        inputs=len(inputs),
        skips=len(rows["skips"]),
        skips_expected=0,
    )
    head = case["normalized"]["head"]
    head.update(status="complete_with_skips", exit_code=3, counts=counts)
    case["exit_code"] = 3
    write_case(case)
