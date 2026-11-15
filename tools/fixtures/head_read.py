"""Record HEAD 417d595 reads of synthetic SF 23-26 and format-9 bundles.

Run in the pinned oracle container: python3 tools/fixtures/head_read.py [--check].
All bytes are constructed here; no Editor or external asset files are used.
The envelopes are
HEAD helpers/TypeTreeNode.py:348-357 and HEAD helpers/TypeTreeNode.py:375-383; resolution is HEAD files/SerializedFile.py:356-394.
"""
from __future__ import annotations

import argparse
import json
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
from head_oracle import HEAD_COMMIT, readers  # noqa: E402

DEST = ROOT / "crates/unityrs-core/tests/fixtures/head_read.json"


def pack(fmt, *values, endian="<"):
    return struct.pack(endian + fmt, *values)


def node(level, ty, name, flags=0, ref=0, meta=0):
    return level, ty, name, flags, ref, meta


BASE = [node(0, "Synthetic", "Base"), node(1, "int", "value")]


def handler(nodes=BASE, refs=(), key=b"T" * 16, size=None, version=19, endian="<", magic=b"mhtt"):
    payload = b""
    if nodes is not None:
        strings, offsets = bytearray(), {}
        def offset(s):
            if s not in offsets:
                offsets[s] = len(strings)
                strings.extend(s.encode() + b"\0")
            return offsets[s]
        rows = bytearray()
        for i, (level, ty, name, flags, ref, meta) in enumerate(nodes):
            rows.extend(pack("hBBIIiii", 1, level, flags, offset(ty), offset(name), -1, i, meta, endian=endian))
            if version >= 19:
                rows.extend(pack("Q", ref, endian=endian))
        payload = magic + pack("iii", version, len(nodes), len(strings), endian=endian) + rows + strings
        payload += pack("I", len(refs), endian=endian) + b"".join(refs)
    return key + pack("I", len(payload) if size is None else size, endian=endian) + payload


def serialized(version, types, shared=(), ref_types=(), endian="<", data=None, trees=True):
    """Absolute alignment, large header, one object per type, arbitrary envelopes."""
    buf = bytearray(48)
    buf.extend(b"6000.4.0f1\0" + pack("i?i", 0, trees, len(types), endian=endian))
    def record(info, ref=False):
        buf.extend(pack("i?h", 1, False, -1, endian=endian) + bytes(16))
        if trees:
            buf.extend(info)
            buf.extend(b"Shared\0Synthetic\0Assembly\0" if ref else pack("i", 0, endian=endian))
    for info in types:
        record(info)
    if data is None:
        data = [pack("i", 37 + i, endian=endian) for i in range(len(types))]
    buf.extend(pack("i", len(data), endian=endian))
    body = bytearray()
    for i, obj in enumerate(data):
        buf.extend(bytes(-len(buf) % 4))
        buf.extend(pack("qqIi", i + 1, len(body), len(obj), i, endian=endian))
        body.extend(obj)
    buf.extend(pack("iii", 0, 0, len(ref_types), endian=endian))
    for info in ref_types:
        record(info, True)
    if version >= 26:
        buf.extend(pack("i", len(shared), endian=endian) + b"".join(shared))
    buf.extend(b"synthetic\0")
    metadata_size = len(buf) - 48
    buf.extend(bytes(-len(buf) % 16))
    offset = len(buf)
    buf.extend(body)
    buf[:48] = pack("IIII?3xIqqq", 0, 0, version, 0, endian == ">", metadata_size, len(buf), offset, 0, endian=">")
    return bytes(buf)


def sf_cases():
    for v in range(23, 27):
        for e in ("<", ">"):
            suffix = "be" if e == ">" else "le"
            yield f"v{v}-{suffix}", serialized(v, [handler(endian=e)], endian=e)
            yield f"v{v}-stripped-{suffix}", serialized(v, [b""], endian=e, trees=False, data=[])
    complex_nodes = [node(0, "Synthetic", "Base"), node(1, "string", "label"),
                     node(1, "vector", "numbers"), node(2, "Array", "Array"),
                     node(3, "int", "size"), node(3, "int", "data"),
                     node(1, "float", "fraction"), node(1, "TypelessData", "raw"),
                     node(1, "bool", "enabled")]
    for v in range(23, 27):
        for e in ("<", ">"):
            data = pack("i", 3, endian=e) + b"abc\0" + pack("iiif", 2, -17, 999, 0.125, endian=e)
            data += pack("i", 3, endian=e) + b"xyz\1"
            yield f"complex-{v}-{e}", serialized(v, [handler(complex_nodes, endian=e)], endian=e, data=[data])
    for size in (0, 7):
        yield f"no-info-{size}", serialized(23, [handler(None, size=size)], data=[])
    for embedded in (18, 19, 23, -1):
        yield f"embedded-{embedded}", serialized(23, [handler(version=embedded, size=8)])
    yield "bad-magic", serialized(23, [handler(magic=b"nope")])
    yield "empty-blob", serialized(23, [handler([])])
    yield "truncated-envelope", serialized(23, [handler()])[:90]
    a, b = b"A" * 16, b"B" * 16
    root = [node(0, "Synthetic", "Base"), node(1, "Part", "left", 32), node(1, "Part", "right", 32)]
    part = [node(0, "Part", "Ignored"), node(1, "int", "answer")]
    nested = [node(0, "Part", "Ignored"), node(1, "Part", "nested", 32)]
    for e in ("<", ">"):
        h = lambda *args, **kw: handler(*args, endian=e, **kw)
        for kind, subtree in (("shared", h(part, key=a)), ("empty-shared", h(None, key=a))):
            data = [pack("ii", 37, 38, endian=e)] if kind == "shared" else [b""]
            yield f"{kind}-{e}", serialized(26, [h(root, [a])], [subtree], [h(root, [a])], endian=e, data=data)
        yield f"nested-{e}", serialized(26, [h(root, [a])], [h(nested, [b], key=a), h(part, key=b)], endian=e,
                                        data=[pack("ii", 37, 38, endian=e)])
    yield "shared-root", serialized(26, [handler([node(0, "Synthetic", "Base", 32)], [a])], [handler(part, key=a)])
    yield "duplicate-hash", serialized(26, [handler(BASE[:1] + [node(1, "Part", "part", 32)], [a])],
                                       [handler(None, key=a), handler(part, key=a)])
    yield "replace-children", serialized(26, [handler(root[:2] + [node(2, "int", "discarded")], [a])], [handler(part, key=a)])
    yield "missing-hash", serialized(26, [handler(root, [a])])
    yield "hash-index", serialized(26, [handler(root, [])], [handler(part, key=a)])
    yield "cycle", serialized(26, [handler(root, [a])], [handler(nested, [a], key=a)])
    yield "unused-cycle", serialized(26, [handler()], [handler(nested, [a], key=a)])
    blobs = [node(0, "Synthetic", "Base"), node(1, "vector", "blobs"),
             node(2, "Array", "Array"), node(3, "int", "size"),
             node(3, "TypelessData", "data", meta=0x4000)]
    yield "aligned-typeless-negative-zero-object", serialized(23, [handler(blobs)], data=[b""]) + pack("ii", 1, -8)
    c = b"C" * 16
    discarded = [node(0, "Synthetic", "Base"), node(1, "Part", "parent", 32),
                 node(2, "Part", "discarded", 32, ref=1)]
    expanded = [node(0, "Part", "Ignored"), node(1, "Part", "revisited", 32, ref=2)]
    yield "discarded-descendant-snapshot", serialized(26, [handler(discarded, [a, b])],
            [handler(part, key=a), handler(expanded, [c, c, c], key=b), handler(part, key=c)], data=[])
    placeholders = [node(0, "Synthetic", "Base")] + [node(level, "Part", "child", 32) for level in range(1, 5)]
    ordinary = [node(0, "Part", "Ignored"), node(1, "Part", "ordinary"), node(2, "int", "value")]
    wide = [node(0, "Part", "Ignored")] + [node(1, "int", f"value{i}") for i in range(4)]
    for name, imported in (("nested-ordinary-import", ordinary), ("nested-wide-import", wide)):
        yield name, serialized(26, [handler(placeholders, [a])], [handler(imported, key=a)], data=[])



def bundle(payloads, offsets, codec=0, at_end=False, pad=False, names=None):
    """Independent block order, explicit offsets, compressed info, raw directory."""
    import lz4.block
    from UnityPy.helpers.CompressionHelper import compress_lzma
    encode = {0: lambda x: x, 1: compress_lzma, 2: lambda x: lz4.block.compress(x, store_size=False)}[codec]
    stored = [encode(p) for p in payloads]
    info = bytes(16) + pack("i", len(payloads), endian=">")
    for raw, compressed, offset in zip(payloads, stored, offsets):
        info += pack("IIHQ", len(raw), len(compressed), codec, offset, endian=">")
    names = names or ["payload.resS"]
    info += pack("i", len(names), endian=">")
    for name in names:
        info += pack("qqI", 0, sum(map(len, payloads)), 0, endian=">") + name.encode() + b"\0"
    compressed_info = encode(info)
    flags = 0x40 | codec | (0x80 if at_end else 0) | (0x200 if pad else 0)
    header = b"UnityFS\0" + pack("I", 9, endian=">") + b"6000.4.0f1\0" * 2
    header += pack("qIII", 0, len(compressed_info), len(info), flags, endian=">")
    header += bytes(-len(header) % 16)
    prefix = header if at_end else header + compressed_info
    if pad:
        prefix += bytes(-len(prefix) % 16)
    body = bytearray()
    for compressed, offset in zip(stored, offsets):
        if offset > 1 << 32:
            continue  # deliberately short read at a 64-bit offset
        if len(body) < offset + len(compressed):
            body.extend(bytes(offset + len(compressed) - len(body)))
        body[offset:offset + len(compressed)] = compressed
    result = bytearray(prefix + body + (compressed_info if at_end else b""))
    size_at = 8 + 4 + len(b"6000.4.0f1\0") * 2
    result[size_at:size_at + 8] = pack("q", len(result), endian=">")
    return bytes(result)


def bundle_cases():
    for codec in (0, 1, 2):
        for at_end in (False, True):
            for pad in (False, True):
                yield f"blocks-{codec}-{at_end}-{pad}", bundle([b"alpha", b"beta"], [64, 0], codec, at_end, pad)
    yield "offset-64bit", bundle([b"missing"], [(1 << 32) + 16])
    yield "overlap", bundle([b"abc", b"abc"], [0, 0])
    yield "duplicate-path", bundle([b"abc"], [0], names=["payload.resS", "payload.resS"])
    yield "no-blocks", bundle([], [])
    for v in range(23, 27):
        sf = serialized(v, [handler()])
        yield f"serialized-{v}", bundle([sf[:100], sf[100:]], [512, 0], names=["CAB-synthetic"])
    yield "stored-empty-after-gap", bundle([b"alpha", b"", b"beta"], [0, 64, 64])
    yield "stored-empty-before-first", bundle([b"", b"alpha", b"beta"], [64, 0, 5])
    yield "stored-empty-between-reordered", bundle([b"alpha", b"", b"beta"], [64, 128, 0])


def tag(value):
    if isinstance(value, bytes):
        return {"bytes": value.hex()}
    if isinstance(value, dict):
        return {k: tag(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [tag(v) for v in value]
    return value


def record():
    import UnityPy
    from UnityPy.streams import EndianBinaryReader
    assert UnityPy.__version__ == "1.25.2"
    out = {"head": HEAD_COMMIT, "generator": "tools/fixtures/head_read.py", "cases": []}
    with readers() as (sf, bf, _):
        for kind, cases in (("serialized", sf_cases()), ("bundle", bundle_cases())):
            for name, data in cases:
                for stream in (False, True):
                    from io import BytesIO
                    reader = EndianBinaryReader(BytesIO(data) if stream else data)
                    try:
                        f = (sf.SerializedFile(reader, name="CAB-synthetic") if kind == "serialized"
                             else bf.BundleFile(reader, None, name="synthetic.bundle"))
                        def objects(f):
                            return {str(k): tag(o.parse_as_dict()) for k, o in f.objects.items()}
                        def trees(types):
                            return [None if t.node is None else
                                    [[n.m_Name, n.m_Type, len(n.m_Children)] for n in t.node.traverse()] for t in types]
                        if kind == "serialized":
                            result = {"objects": objects(f), "user": f.userInformation,
                                      "types": trees(f.types), "ref_types": trees(f.ref_types or [])}
                        else:
                            result = {"files": {k: ({"objects": objects(v)} if hasattr(v, "objects")
                                                    else {"bytes": v.bytes.hex()}) for k, v in f.files.items()}}
                    except Exception as exc:
                        result = {"error": type(exc).__name__}
                    out["cases"].append({"kind": kind, "name": name, "stream": stream, "hex": data.hex(), "result": result})
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    result = record()
    cases = result.pop("cases")
    output = json.dumps(result, sort_keys=True)[:-1] + ', "cases": [\n'
    output += ",\n".join(json.dumps(c, sort_keys=True, separators=(",", ":")) for c in cases) + "\n]}\n"
    if args.check:
        if not DEST.is_file() or DEST.read_text() != output:
            raise SystemExit("HEAD read oracle is stale; run tools/fixtures/head_read.py")
    else:
        DEST.write_text(output)
    print(f"HEAD read oracle: {len(json.loads(output)['cases'])} cases checked")


if __name__ == "__main__":
    main()
