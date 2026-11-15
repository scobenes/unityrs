"""Record spec BC6H raw halves and RGB images, independently checked by vendored bcdec.

All fourteen modes, all thirty-two partitions, endpoint bit scatter, transformed
wraparound, signed extrema, signed negative zero, anchor index shortening, and
3/4-bit interpolation are exercised. bcdec is an independent check, never the
reference; its loss of signed negative zero is counted explicitly. Regeneration
and --check use only the pinned container and vendored source, with no network.
"""
from __future__ import annotations

import argparse
import hashlib
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "parity"))
import child  # noqa: E402
import references as ref  # noqa: E402
import scratch  # noqa: E402
import texture  # noqa: E402

OUT = texture.OUT / "bc6h.json"
SPEC = "https://registry.khronos.org/DataFormat/specs/1.3/dataformat.1.3.html#BPTC"


def hash_bytes(data):
    return hashlib.sha256(data).hexdigest()


def scatter(mode):
    position = 0
    for element in ref.BC6H_LAYOUTS[mode].split(","):
        name, span = element[:-1].split("[")
        high, _, low = span.partition(":")
        a, b = int(high), int(low or high)
        step = 1 if a >= b else -1
        yield name, list(range(b, a + step, step)), position
        position += abs(a - b) + 1


def pack(mode, endpoints, partition=0, index=0):
    values = {f"{ch}{e}": v for e, channels in enumerate(endpoints) for ch, v in zip("rgb", channels)}
    values["m"] = mode
    bits = 0
    for name, targets, position in scatter(mode):
        for at, target in enumerate(targets):
            bits |= (values.get(name, 0) >> target & 1) << (position + at)
    two = ref.BC6H_MODES[mode][1] != 0
    position = 82 if two else 65
    if two:
        bits |= partition << 77
    for texel in range(16):
        width = (3 if two else 4) - int(texel == 0 or two and texel == ref.BC6H_ANCHORS[partition])
        value = index[texel] if isinstance(index, list) else index
        bits |= (value & ((1 << width) - 1)) << position
        position += width
    assert position == 128
    return bits.to_bytes(16, "little")


def blocks():
    for mode, (_, pb, ep, delta) in sorted(ref.BC6H_MODES.items()):
        for partition in range(32 if pb else 1):
            for trial in range(3):
                data = int.from_bytes(texture.splitmix(texture.seed_of(f"bc6-{mode}-{partition}-{trial}"), 16), "little")
                width = 2 if mode < 2 else 5
                data = data >> width << width | mode
                if pb:
                    data = data & ~(31 << 77) | partition << 77
                yield f"mode-{mode}-partition-{partition}-trial-{trial}", mode, partition if pb else None, data.to_bytes(16, "little")
        # Exercise every endpoint bit individually, including reversed high bits
        # of the mode 11/15 bases, and signed/magnitude boundaries.
        for bit in range(ep):
            for endpoint in (0, 1):
                e = [[0] * 3 for _ in range(4)]
                e[endpoint] = [1 << bit] * 3
                yield f"mode-{mode}-endpoint-{endpoint}-bit-{bit}", mode, 0 if pb else None, pack(mode, e, index=5)
        for value in (0, (1 << ep) - 1, 1 << (ep - 1), (1 << (ep - 1)) - 1):
            e = [[value] * 3] + [[(1 << d) - 1 for d in delta]] * 3
            yield f"mode-{mode}-boundary-{value}", mode, 0 if pb else None, pack(mode, e, index=list(range(16)))
    for label, e0, e1, index in (
        ("worked", (400, 488, 500), (0, 0, 0), 0),
        ("midpoint-not-maximum", (512, 100, 100), (0, 0, 0), 8),
        ("rounding", (0, 0, 0), (1023, 1023, 1023), 1),
        ("signed-negative-zero", (0, 0, 0), (1023, 1023, 1023), 1),
    ):
        yield label, 3, None, pack(3, [e0, e1], index=index)
    # In full 16-bit mode, -1 remains -1 before final scaling and is -0.0.
    yield "signed-negative-zero-full", 15, None, pack(15, [(65535,) * 3, (0,) * 3])


def half_bytes(data, signed):
    return b"".join(struct.pack("<3H", *pixel) for pixel in ref.bc6h_block(data, signed=signed))


def independent(entries):
    """Compare every raw half against bcdec, allowing only its documented -0 loss."""
    with scratch.folder("bc6h-oracle-") as folder:
        exe = folder / "bc6h"
        child.run(["cc", "-std=c99", "-O2", "-Wall", "-Wextra", str(HERE / "bc6h_oracle.c"), "-o", str(exe)],
                  check=True, capture_output=True)
        data = b"".join(bytes([signed]) + entry[3] for entry in entries for signed in (False, True))
        result = child.run([str(exe)], input=data, check=True, capture_output=True).stdout
    assert len(result) == len(entries) * 192
    negative_zeros = 0
    for i, entry in enumerate(entries):
        for signed in (False, True):
            got = result[(2 * i + signed) * 96:(2 * i + signed + 1) * 96]
            expected = half_bytes(entry[3], signed)
            for actual, wanted in zip(struct.unpack("<48H", got), struct.unpack("<48H", expected), strict=True):
                if actual != wanted:
                    assert signed and actual == 0 and wanted == 0x8000, (entry[0], signed, actual, wanted)
                    negative_zeros += 1
    return negative_zeros


def image_cases(entries):
    for w, h in ((1, 1), (3, 2), (4, 4), (5, 7), (9, 3), (2, 9)):
        n = ref.needed_bytes("BC6H", w, h) // 16
        data = b"".join(entries[(i * 37 + w) % len(entries)][3] for i in range(n))
        for suffix, payload in (("exact", data), ("mips", data + bytes(48))):
            c = texture.case(f"BC6H-{w}x{h}-{suffix}", texture.TF.BC6H, w, h, {"hex": payload.hex()})
            _, pixels = ref.decode("BC6H", payload, w, h)
            c["reference_sha256"] = hash_bytes(bytes(v for p in pixels for v in p))
            c["signed_reference_sha256"] = signed_image_hash(payload, w, h)
            c["data"] = payload.hex()
            yield c
    # The exact Section 17 matrix case records UnityPy's real exception beside our answer.
    payload = bytes([0x30]) * 64
    c = texture.case("BC6H-matrix", texture.TF.BC6H, 8, 8, {"hex": payload.hex()})
    _, pixels = ref.decode("BC6H", payload, 8, 8)
    c["reference_sha256"] = hash_bytes(bytes(v for p in pixels for v in p))
    c["signed_reference_sha256"] = signed_image_hash(payload, 8, 8)
    c["data"] = payload.hex()
    yield c


def signed_image_hash(data, w, h):
    across = -(-w // 4)
    blocks = [ref.bc6h_block(data[i:i + 16], signed=True) for i in range(0, ref.needed_bytes("BC6H", w, h), 16)]
    pixels = [blocks[(y // 4) * across + x // 4][(y % 4) * 4 + x % 4]
              for y in range(h - 1, -1, -1) for x in range(w)]
    return hash_bytes(bytes(ref.float_to_byte(ref.half_bits_to_float(v)) for pixel in pixels for v in pixel))


def rust_tables():
    """Compact scatter descriptors (destination scalar, target low bit, signed width)."""
    out = ["//! BC6H tables from Khronos Data Format Specification 1.3 section 20.2.",
           "//! Reproduce with `tools/fixtures/texture_bc6h.py --rust-tables`.",
           "use super::Mode;", "#[rustfmt::skip]", "pub(super) const MODES: &[Mode] = &["]
    for mode, (transformed, pb, ep, delta) in sorted(ref.BC6H_MODES.items()):
        fields = []
        for name, targets, _ in scatter(mode):
            if name == "m":
                continue
            dest = int(name[1]) * 3 + "rgb".index(name[0])
            width = len(targets) * (-1 if targets[0] > targets[-1] else 1)
            fields.append(f"({dest}, {min(targets)}, {width})")
        out.append(f"    Mode {{ number: {mode}, transformed: {str(transformed).lower()}, two: {str(bool(pb)).lower()}, bits: {ep}, delta: {list(delta)}, fields: &[")
        for at in range(0, len(fields), 5):
            out.append("        " + ", ".join(fields[at:at + 5]) + ",")
        out.append("    ] },")
    out.extend(["];", "pub(super) const PARTITIONS: [u16; 32] = ["])
    for at in range(0, 32, 8):
        masks = [sum(int(v) << i for i, v in enumerate(p)) for p in ref.BC6H_PARTITIONS[at:at + 8]]
        out.append("    " + ", ".join(f"0x{mask:04x}" for mask in masks) + ",")
    out.extend(["];", f"pub(super) const ANCHORS: [usize; 32] = {list(ref.BC6H_ANCHORS)};", ""])
    return "\n".join(out)


def fixture():
    entries = list(blocks())
    negative_zeros = independent(entries)
    records = [{"id": label, "mode": mode, "partition": partition, "data": data.hex(),
                "unsigned": hash_bytes(half_bytes(data, False)), "signed": hash_bytes(half_bytes(data, True))}
               for label, mode, partition, data in entries]
    return {"generator": "tools/fixtures/texture_bc6h.py", "reference": SPEC,
            "independent": "bcdec v0.985 (vendored MIT BC6H subset)",
            "bcdec_negative_zero_differences": negative_zeros,
            "unitypy": texture.UnityPy.__version__, "pillow": texture.PIL.__version__,
            "blocks": records, "cases": list(image_cases(entries))}


def dump(document):
    """Two compact records per line keeps the complete fixture below 800 lines."""
    lines = texture.dump(document).splitlines()
    out, i = [], 0
    while i < len(lines):
        line = lines[i]
        if line.startswith("  {") and i + 1 < len(lines) and lines[i + 1].startswith("  {"):
            line += " " + lines[i + 1].lstrip()
            i += 1
        out.append(line)
        i += 1
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--rust-tables", action="store_true")
    args = ap.parse_args()
    if args.rust_tables:
        print(rust_tables(), end="")
        return
    if (texture.UnityPy.__version__, texture.PIL.__version__) != ("1.25.2", "9.5.0"):
        raise SystemExit("needs UnityPy 1.25.2 and Pillow 9.5.0")
    text = dump(fixture())
    if args.check:
        if OUT.read_text() != text:
            raise SystemExit("stale fixture: bc6h.json")
    else:
        OUT.write_text(text)


if __name__ == "__main__":
    main()
