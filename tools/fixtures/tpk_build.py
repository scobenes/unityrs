"""Building TPKs: the helpers `tpk.py` builds its synthetic cases with (a blob as
`TpkTypeTreeBlob` reads it, a TPK file around a payload, LZ4 sequences, and LZMA
streams whose one far match a decoder with a small dictionary fails on).

`tpk.py` imports this module; it records nothing when run on its own.
"""

from __future__ import annotations

import lzma
import struct

import brotli
import lz4.block

MAGIC = 0x2A4B5054  # b"TPK*"
NONE, LZ4, LZMA, BROTLI = 0, 1, 2, 3
I32_MAX = 2**31 - 1

# TpkProvider's bounds (crates/unityrs-tpk/src/provider.rs), for the `refused` cases.
MAX_DECODED_BYTES = 16 << 20
MAX_RECORDS = 1 << 16

# The dictionary the far-match streams' property bytes name: twice the payload, so a
# retry's dictionary is never cut to it.
FAR_DICTIONARY = 32 << 20


def ver(major: int, minor: int = 0, build: int = 0, typ: int = 3, number: int = 1) -> int:
    """A version packed as a TPK stores it (`UnityVersion.from_list`)."""
    return (major << 48) | (minor << 32) | (build << 16) | (typ << 8) | number


def node(
    type_name: int,
    name: int,
    subs: list[int],
    byte_size: int = -1,
    version: int = 1,
    meta: int = 0x4000,
    type_flags: int = 0,
):
    return (type_name, name, byte_size, version, type_flags, meta, subs)


def cls(flags: int, editor: int | None = None, release: int | None = None):
    return (flags, editor, release)


def both(root: int):
    return cls(192, root, root)


def varint(n: int) -> bytes:
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        if not n:
            out.append(byte)
            return bytes(out)
        out.append(byte | 0x80)


def blob(
    versions: list[int],
    classes: list[tuple[int, list]],
    nodes: list[tuple],
    strings: list[str | bytes],
    common: list[tuple[int, int]] = (),
    indices: list[int] = (),
    counts: dict[str, int] | None = None,
    items_anyway: bool = False,
) -> bytes:
    """A type-tree blob as `TpkTypeTreeBlob` reads it (`Tpk.py:232-240`).

    `counts` replaces a count (`versions`, `classes`, `entries` (each class's),
    `common`, `indices`, `nodes`, `strings`) with another value; for a count of zero or
    less the items are not written, unless `items_anyway`.
    """
    counts = counts or {}

    def n(key: str, items) -> tuple[bytes, bool]:
        value = counts.get(key, len(items))
        return struct.pack("<i", value), value > 0 or items_anyway

    out = bytearray(struct.pack("<q", 1))
    head, write = n("versions", versions)
    out += head
    if write:
        out += b"".join(struct.pack("<Q", v) for v in versions)
    head, write = n("classes", classes)
    out += head
    for class_id, entries in classes if write else ():
        out += struct.pack("<i", class_id)
        head, write_entries = n("entries", entries)
        out += head
        for version, c, *presence in entries if write_entries else ():
            out += struct.pack("<Q", version)
            if c is None:
                out += b"\0"
                continue
            flags, editor, release = c
            # The class's presence byte (`stream.read(1)[0]`, `Tpk.py:342`): 1, or
            # another nonzero byte given after the class.
            out += bytes(presence or [1]) + struct.pack("<HHB", 7, 0, flags)
            out += b"".join(struct.pack("<H", r) for r in (editor, release) if r is not None)
    head, write = n("common", common)
    out += head
    for v, c in common if write else ():
        out += struct.pack("<QB", v, c)
    head, write = n("indices", indices)
    out += head
    if write:
        out += b"".join(struct.pack("<H", i) for i in indices)
    head, write = n("nodes", nodes)
    out += head
    for type_name, name, byte_size, version, type_flags, meta, subs in nodes if write else ():
        out += struct.pack("<HHihbIH", type_name, name, byte_size, version, type_flags, meta, len(subs))
        out += b"".join(struct.pack("<H", s) for s in subs)
    head, write = n("strings", strings)
    out += head
    for s in strings if write else ():
        data = s if isinstance(s, bytes) else s.encode()
        out += varint(len(data)) + data
    return bytes(out)


def compress(payload: bytes, compression: int) -> bytes:
    if compression == LZ4:
        return lz4.block.compress(payload, store_size=False)
    if compression == LZMA:
        # `decompress_lzma` (helpers/CompressionHelper.py): 5 bytes of properties, then
        # a raw LZMA1 stream.
        lc, lp, pb, dict_size = 3, 0, 2, 1 << 16
        filters = [{"id": lzma.FILTER_LZMA1, "dict_size": dict_size, "lc": lc, "lp": lp, "pb": pb}]
        data = struct.pack("<BI", (pb * 5 + lp) * 9 + lc, dict_size)
        return data + lzma.compress(payload, format=lzma.FORMAT_RAW, filters=filters)
    if compression == BROTLI:
        return brotli.compress(payload)
    return payload


def header(compression: int, compressed: int, uncompressed: int) -> bytes:
    """`TpkFile.Struct` (`<IbbbbIII`); a negative size is written as its u32."""
    sizes = (compressed & 0xFFFF_FFFF, uncompressed & 0xFFFF_FFFF)
    return struct.pack("<IBBBBIII", MAGIC, 1, compression, 0, 0, 0, *sizes)


def tpk_file(
    payload: bytes,
    compression: int = NONE,
    sizes: tuple[int, int] | None = None,
    data: bytes | None = None,
) -> bytes:
    """A TPK file around `payload` (`Tpk.py:146-174`), compressed as asked (or `data`
    as the stored bytes); `sizes` replaces the header's compressed and uncompressed
    sizes."""
    if data is None:
        data = compress(payload, compression)
    compressed, uncompressed = sizes or (len(data), len(payload))
    return header(compression, compressed, uncompressed) + data


def lz4_sequence(literals: bytes, offset: int | None = None, length: int = 0) -> bytes:
    """One LZ4 sequence: a token, the literals, and a match of `length` bytes at
    `offset` (none for the block's last sequence)."""

    def extra(n: int) -> bytes:
        out = bytearray()
        while n >= 255:
            out.append(255)
            n -= 255
        out.append(n)
        return bytes(out)

    match = 0 if offset is None else length - 4
    token = bytes([min(len(literals), 15) << 4 | min(match, 15)])
    out = token + (extra(len(literals) - 15) if len(literals) >= 15 else b"") + literals
    if offset is not None:
        out += struct.pack("<H", offset) + (extra(match - 15) if match >= 15 else b"")
    return out


def chain(n: int) -> list[tuple]:
    return [node(0, 1, [i + 1] if i + 1 < n else []) for i in range(n)]


def strings(n: int) -> list[str]:
    return [f"s{i}" for i in range(n)]


def root_flags_blob() -> bytes:
    """Classes 10, 20, 30, 40 with root flags 0, 64, 128, 192, each followed by a class
    with both roots; class 45 carries other flag bits with its release root."""
    nodes = [node(0, 1, [1]), node(2, 3, []), node(4, 5, [1, 1], meta=0)]
    at = ver(2018, 4, 2)
    classes = []
    for class_id, c in [(10, cls(0)), (20, cls(64, editor=2)), (30, cls(128, release=0)), (40, cls(192, 2, 0))]:
        classes.append((class_id, [(at, c)]))
        classes.append((class_id + 1, [(at, both(2))]))
    classes.append((45, [(at, cls(1 | 4 | 16 | 128, release=2))]))
    return blob([ver(5), at, ver(2022, 1)], classes, nodes, strings(6))


def far_match_stream(payload: bytes) -> bytes:
    """`payload` as `decompress_lzma` reads it, coded with a dictionary that reaches
    over the whole payload (a hash-chain match finder: a binary tree that far needs
    hundreds of MiB), its properties naming `FAR_DICTIONARY`."""
    lc, lp, pb = 3, 0, 2
    reach = MAX_DECODED_BYTES + (1 << 20)
    filters = [{"id": lzma.FILTER_LZMA1, "dict_size": reach, "lc": lc, "lp": lp, "pb": pb, "mf": lzma.MF_HC4}]
    head = struct.pack("<BI", (pb * 5 + lp) * 9 + lc, FAR_DICTIONARY)
    return head + lzma.compress(payload, format=lzma.FORMAT_RAW, filters=filters)


def decoded_before_failing(stream: bytes, dict_size: int) -> int:
    """How many bytes Python's liblzma returns from `stream` (`far_match_stream`'s) with
    a dictionary of `dict_size` before its data error, decoded in steps of 64 KiB and
    then of one byte: where the first match farther back than that begins, or one byte
    before it (asked for one byte, liblzma 5.4 goes on to the next symbol when the byte
    ends one, and fails in that call)."""
    lc, lp, pb = 3, 0, 2
    filters = [{"id": lzma.FILTER_LZMA1, "dict_size": dict_size, "lc": lc, "lp": lp, "pb": pb}]

    def run(coarse_to: int) -> int:
        d = lzma.LZMADecompressor(format=lzma.FORMAT_RAW, filters=filters)
        made, data = 0, stream[5:]
        while True:
            step = 1 << 16 if made + (1 << 16) <= coarse_to else 1
            try:
                got = d.decompress(data, max_length=step)
            except lzma.LZMAError:
                return made
            data = b""
            made += len(got)
            if d.eof or not got:
                raise AssertionError("the stream decoded without a far match")

    coarse = run(MAX_DECODED_BYTES + 1)
    return run(coarse - coarse % (1 << 16))
