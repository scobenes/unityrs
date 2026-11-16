"""Y4 decode matrix from the core's existing synthetic encoded inputs."""

import hashlib
import json
from collections import Counter
from pathlib import Path
from types import SimpleNamespace

from binding_y4_images import capture, texture

FIXTURES = Path(__file__).resolve().parents[2] / "crates/unityrs-texture/tests/fixtures"


def _data(spec):
    if isinstance(spec, str):
        return bytes.fromhex(spec)
    if "hex" in spec:
        return bytes.fromhex(spec["hex"])
    if "zeros" in spec:
        return bytes(spec["zeros"])
    state = spec["seed"]
    mask = 2**64 - 1
    result = bytearray()
    while len(result) < spec["len"]:
        state = (state + 0x9E3779B97F4A7C15) & mask
        value = state
        value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & mask
        value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & mask
        value ^= value >> 31
        result.extend(value.to_bytes(8, "little"))
    del result[spec["len"]:]
    if "bc7_mode" in spec:
        mode = spec["bc7_mode"]
        for at in range(0, len(result) - len(result) % 16, 16):
            result[at] = 0 if mode == 8 else (result[at] & ~((2 << mode) - 1)) | (1 << mode)
    return bytes(result)


def observe_decode_matrix(module):
    result = {}
    for name, key in (("decode", "cases"), ("raw", "cases"), ("etc", "cases"),
                      ("astc", "format_cases"), ("pvrtc_atc", "cases"),
                      ("platform", "textures"), ("crunch", "cases"), ("crunch", "unity_cases")):
        saved = json.loads((FIXTURES / (name + ".json")).read_text())
        counts = Counter()
        for case in saved[key]:
            # Keep only Tier A successes with small outputs. Negative dimensions,
            # allocator limits and documented Tier B/C corrections have their own
            # core fixtures and explicit binding observations.
            if "sha256" not in case or case.get("diverging", 0):
                continue
            # Invalid ASTC blocks have architecture-dependent oracle error
            # colours; the valid encoded samples exercise the Tier A port.
            if name == "astc" and "encoded" not in case["id"]:
                continue
            if not (0 < case["width"] <= 32 and 0 < case["height"] <= 32):
                continue
            bucket = (case["format"], case.get("platform", 19))
            if counts[bucket] >= 3:
                continue
            counts[bucket] += 1
            data = _data(case["data"])
            if "data_sha256" in case:
                assert hashlib.sha256(data).hexdigest() == case["data_sha256"], case["id"]
            blob = case.get("blob")
            if isinstance(blob, str):
                blob = bytes.fromhex(blob)
            legacy = name == "platform" or (name == "crunch" and key == "cases")
            version = (2017, 2, 0, 3, 1) if legacy else (2020, 3, 0, 3, 1)
            obj = texture(module, data, m_Width=case["width"], m_Height=case["height"],
                          m_TextureFormat=case["format"], m_PlatformBlob=blob,
                          object_reader=SimpleNamespace(platform=case.get("platform", 19), version=version))
            def observe():
                image = obj.image
                return {"mode": image.mode, "size": list(image.size),
                        "sha256": hashlib.sha256(image.tobytes()).hexdigest()}
            row = capture(observe)
            if module.__name__ == "UnityPy":
                assert row == {"value": {k: case[k] for k in ("mode", "size", "sha256")}}, case["id"]
            result[name + "/" + key + "/" + case["id"]] = row
    return result
