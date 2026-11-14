"""Version 2 pins: `outputs.formats`, `outputs.image` and `unitycn_key`.

Each derives from `textures-five` by docs/BATCH_SPEC.md's hand rules (1.5,
section 3, 7.3, 9.3): a texture in a listed format is exported with the
pinned decoder pixels of `decode.json` or `raw.json`, PNG-encoded here as
texture_case.py does; one in a format the list leaves out is the expected
skip `unsupported_format(<name>)`; and the head records the effective
options. The UnityCN case's bundle is encrypted by the Rust harness with
the core's synthetic test encryptor; its key, a made-up one, is the
harness's environment, never the spec's.
"""

import copy
import json
from pathlib import Path

from model import load_case, write_case
from texture_case import fixture, texture_output

# TextureFormat values, for the head's order.
ORDER = {"RGB24": 3, "RGBA32": 4, "RGB565": 7, "BGR24": 8, "DXT1": 10, "DXT5": 12, "BC7": 25, "BC4": 26}
V1 = ["RGB24", "RGBA32", "DXT1", "DXT5", "BC7"]
# b"synthetic-key-01", the core's synthetic test key, in hex.
KEY_HEX = "73796e7468657469632d6b65792d3031"


def recordings():
    root = Path("crates/unityrs-texture/tests/fixtures")
    decode = json.loads((root / "decode.json").read_text())
    raw = json.loads((root / "raw.json").read_text())
    return {3: decode, 4: decode, 10: decode, 12: decode, 25: decode, 7: raw, 8: raw, 26: raw}


def v2(case, name, formats=None, key=False):
    """`case` at version 2: its name, its spec's options and its head's."""
    case["name"] = name
    case["spec"]["spec_version"] = 2
    if formats is not None:
        case["spec"]["outputs"]["formats"] = formats
    if key:
        case["spec"]["unitycn_key"] = {"env": "UNITYRS_GOLDEN_KEY"}
    head = case["normalized"]["head"]
    head["formats"] = sorted(formats or V1, key=ORDER.__getitem__)
    head["image"] = "png"
    head["unitycn_key"] = key
    return case


def textures(case, listed, skipped):
    """`case`'s textures: `listed` exported, then `skipped` left out of the
    list, each a format's name, at path ids in that order."""
    recorded = recordings()
    rows = case["normalized"]["rows"]
    case["texture_inputs"], case["outputs"] = [], {}
    rows["outputs"], rows["skips"] = [], []
    for path_id, name in enumerate(listed + skipped, 1):
        value = ORDER[name]
        source, row, pinned = texture_output(fixture(recorded[value], value), path_id)
        case["texture_inputs"].append(source)
        if name in listed:
            rows["outputs"].append(row)
            case["outputs"][row["key"]] = pinned
        else:
            # Section 3's answer 2, an expected skip (9.3).
            rows["skips"].append(
                {
                    "input": "x.bundle",
                    "serialized_file": "CAB-five",
                    "path_id": path_id,
                    "class": "Texture2D",
                    "output": "texture",
                    "sprite": None,
                    "reason": f"unsupported_format({name})",
                    "detail": None,
                    "cut": False,
                }
            )
    rows["outputs"].sort(key=lambda r: r["key"])
    counts = case["normalized"]["head"]["counts"]
    counts.update(
        outputs_written=len(listed),
        skips=len(skipped),
        skips_expected=len(skipped),
        inputs_complete=0 if skipped else 1,
        inputs_with_skips=1 if skipped else 0,
    )
    rows["inputs"][0].update(
        status="complete_with_skips" if skipped else "complete", skips=len(skipped)
    )


def generate():
    five = load_case("textures-five")

    # The defaults are version 1's: the same tree and rows, the head's
    # options added.
    write_case(v2(copy.deepcopy(five), "v2-textures-defaults"))

    # A list of its own, out of order, with two of the T1 formats: RGB565
    # (RGB) and BC4 (L). DXT1, of version 1's five, and BGR24, which this
    # binary decodes, are left out of the list and skipped.
    case = v2(copy.deepcopy(five), "v2-textures-formats", ["RGB565", "BC4", "RGBA32", "DXT5"])
    case["fixture"] = "textures:formats"
    textures(case, ["RGB565", "BC4", "RGBA32", "DXT5"], ["DXT1", "BGR24"])
    write_case(case)

    # The same through a UnityCN-encrypted bundle, the key read from the
    # variable the spec names.
    case = v2(copy.deepcopy(five), "v2-unitycn", ["RGB565", "RGBA32"], key=True)
    case["fixture"] = "textures:unitycn"
    case["encrypted"] = True
    case["env"] = {"UNITYRS_GOLDEN_KEY": KEY_HEX}
    textures(case, ["RGB565", "RGBA32"], ["DXT1"])
    write_case(case)
