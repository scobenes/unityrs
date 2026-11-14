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
        c = fixture(decode, fmt)
        source, row, pinned = texture_output(c, i)
        case["texture_inputs"].append(source)
        rows["outputs"].append(row)
        case["outputs"][row["key"]] = pinned
    rows["outputs"].sort(key=lambda r: r["key"])
    # The original texture pin predates the oracle projection's leading
    # evictions field; preserve its reviewable serialized field order.
    row = rows["inputs"][0]
    row["evictions"] = row.pop("evictions")
    case["normalized"]["head"]["counts"]["outputs_written"] = 5
    write_case(case)


def fixture(recording, fmt):
    """The 1x1 exact case of `fmt` in a pinned decoder recording."""
    return next(
        c
        for c in recording["cases"]
        if c["format"] == fmt
        and c.get("mode")
        and c["width"] == 1
        and c["height"] == 1
    )


def chunk(kind, data):
    return (
        struct.pack(">I", len(data))
        + kind
        + data
        + struct.pack(">I", binascii.crc32(kind + data))
    )


COLOUR_TYPES = {"L": 0, "RGB": 2, "RGBA": 6}


def texture_output(c, path_id, serialized_file="CAB-five"):
    """A 1x1 texture of recording case `c` at `path_id`: its texture input,
    its `outputs` row and its pinned PNG, from the recorded pixels alone."""
    name = c["id"].split("-")[0]
    key = name + ".png"
    mode = c["mode"]
    pixels = bytes.fromhex(c["pixels"])
    source = {
        "format": c["format"],
        "name": name,
        "data": c["data"],
        "source_sha256": c["data_sha256"],
    }
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(
            b"IHDR",
            struct.pack(">IIBBBBB", 1, 1, 8, COLOUR_TYPES[mode], 0, 0, 0),
        )
        + chunk(b"IDAT", zlib.compress(b"\0" + pixels, 6))
        + chunk(b"IEND", b"")
    )
    md5 = hashlib.md5(png).hexdigest()
    # E20: convert("RGBA"), each pixel of alpha 0 as four zeros.
    rgba = {"L": pixels * 3 + b"\xff", "RGB": pixels + b"\xff", "RGBA": pixels}[mode]
    visible = hashlib.sha256(b"1x1:" + (rgba if rgba[3] else bytes(4))).hexdigest()
    row = {
        "key": key,
        "kind": "texture",
        "unit": None,
        "input": "x.bundle",
        "serialized_file": serialized_file,
        "path_id": path_id,
        "md5": md5,
        "visible_hash": visible,
        "mode": mode,
        "size": len(png),
        "status": "published",
        "reason": None,
        "cut": False,
    }
    return source, row, {"md5": md5, "hex": png.hex()}
