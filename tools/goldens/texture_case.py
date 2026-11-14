import binascii
import hashlib
import json
import struct
import zlib
from pathlib import Path

from model import load_case, write_case


def generate():
    case = load_case("sprites-own_sheets")
    case["name"] = "textures-five"
    case["fixture"] = "textures:five"
    case["spec"]["outputs"] = {"textures": {"key": "{name}.png"}}
    case["spec"]["inputs"] = [
        {"id": "x.bundle", "path": "$SOURCE:x.bundle", "deps": []}
    ]
    rows = case["normalized"]["rows"]
    rows["outputs"] = []
    rows["bundles"] = [
        {
            "id": "x.bundle",
            "status": "staged",
            "reason": None,
            "detail": None,
            "size": "$BUNDLE_SIZE:x.bundle",
            "sha256": "$BUNDLE_SHA256:x.bundle",
            "peek": None,
        }
    ]
    case["texture_inputs"] = []
    case["outputs"] = {}
    decode = json.loads(
        Path("crates/unityrs-texture/tests/fixtures/decode.json").read_text()
    )
    for i, fmt in enumerate([3, 4, 10, 12, 25], 1):
        c = next(
            c
            for c in decode["cases"]
            if c["format"] == fmt
            and c.get("mode")
            and c["width"] == 1
            and c["height"] == 1
        )
        name = c["id"].split("-")[0]
        key = name + ".png"
        mode = c["mode"]
        pixels = bytes.fromhex(c["pixels"])
        case["texture_inputs"].append(
            {
                "format": fmt,
                "name": name,
                "data": c["data"],
                "source_sha256": c["data_sha256"],
            }
        )

        def chunk(kind, data):
            return (
                struct.pack(">I", len(data))
                + kind
                + data
                + struct.pack(">I", binascii.crc32(kind + data))
            )

        png = (
            b"\x89PNG\r\n\x1a\n"
            + chunk(
                b"IHDR",
                struct.pack(">IIBBBBB", 1, 1, 8, 2 if mode == "RGB" else 6, 0, 0, 0),
            )
            + chunk(b"IDAT", zlib.compress(b"\0" + pixels, 6))
            + chunk(b"IEND", b"")
        )
        md5 = hashlib.md5(png).hexdigest()
        rgba = pixels if mode == "RGBA" else pixels + b"\xff"
        visible = hashlib.sha256(b"1x1:" + (rgba if rgba[3] else bytes(4))).hexdigest()
        rows["outputs"].append(
            {
                "key": key,
                "kind": "texture",
                "unit": None,
                "input": "x.bundle",
                "serialized_file": "CAB-five",
                "path_id": i,
                "md5": md5,
                "visible_hash": visible,
                "mode": mode,
                "size": len(png),
                "status": "published",
                "reason": None,
                "cut": False,
            }
        )
        case["outputs"][key] = {"md5": md5, "hex": png.hex()}
    rows["outputs"].sort(key=lambda r: r["key"])
    # The original texture pin predates the oracle projection's leading
    # evictions field; preserve its reviewable serialized field order.
    row = rows["inputs"][0]
    row["evictions"] = row.pop("evictions")
    case["normalized"]["head"]["counts"]["outputs_written"] = 5
    write_case(case)
