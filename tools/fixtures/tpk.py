"""G7's UnityPy side (docs/plans/core.md sections 6 and 7.2): what UnityPy 1.25.2's
helpers/Tpk.py builds for every class of a TPK at every version it lists.

Runs in the oracle image. Two parts:

- The synthetic set. TPKs built here in code, each a case UnityPy's own TPK cannot show
  (a class for each root-flag combination 0, 64, 128 and 192, each followed by a class
  with both roots; a class for every flag byte; versions whose type byte is 0-6 and 255;
  a node's fields at their extremes; a 300-deep chain; entries out of order, removed
  classes, a class listed twice; ids past their tables; counts read as UnityPy reads
  them; string lengths, and strings that are not UTF-8; header sizes and checks (magic,
  version, compression and data type, a file or a properties header cut short, reads
  that run out at a byte); blobs of data type 1-3 that UnityPy's own readers cannot
  read; common strings numbered by code points; LZ4 and LZMA
  payloads, with the blocks and streams on which liblz4 and liblzma decide differently
  from other decoders, and LZMA payloads at `TpkProvider`'s decoded bound whose one far
  match makes unityrs-core's codec decode again with a larger dictionary), run through
  `Tpk.py` (`TpkFile(...).GetDataBlob()`, then
  `get_typetree_node` per class and version, and `get_common_strings()`, with the blob
  in place of the bundled one). The TPKs (hex) and UnityPy's outcomes (JSON)
  are committed under crates/unityrs-tpk/tests/fixtures/tpk/, and
  `cargo test -p unityrs-tpk --test g7` replays them without Python. A case marked
  `refused` is one UnityPy loads and unityrs refuses by design (`PARITY.md` section
  19.3): a TPK past one of `TpkProvider`'s bounds, a brotli one, one with a string
  UnityPy reads on (past the payload's end, or with a length prefix of 6 bytes), or one
  whose common strings name a string past the buffer. Only UnityPy's load, class count
  and common strings are recorded for it.
- The oracle image's own TPK (`UnityPy/resources/lzma.tpk`). Nothing from it is
  committed (decision 10): its outcomes are written outside the repository and
  compared at test time.

Each TPK's header record holds `get_common_strings()` (`[offset, string]` pairs, or
the exception's name), the table every blob typetree reads.

An outcome is UnityPy's node list (every node in pre-order, each with all ten
`TypeTreeNode` fields the dump reads), or "none" where UnityPy raises `KeyError`,
`ValueError` or `AssertionError` (`no_typetree`), or "error" with the exception's name
for anything else (an id past its table). A TPK UnityPy cannot load records the
exception's name as its "load". G7 maps each exception name to the error the port gives
for it (`crates/unityrs-tpk/tests/common/mod.rs`), and an unmapped pair fails G7.

    scripts/dev python tools/fixtures/tpk.py            # write the synthetic fixtures
    scripts/dev python tools/fixtures/tpk.py --check    # regenerate and compare them, then
                                                        # run G7 on the image's TPK
    scripts/dev python tools/fixtures/tpk.py --outcomes OUT [--tpk PATH]
                                                        # UnityPy's outcomes for a TPK
                                                        # (default: the image's), as JSON lines
    scripts/dev python tools/fixtures/tpk.py --survey   # counts about the image's TPK
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import struct
import sys
from collections.abc import Iterator
from io import BytesIO
from pathlib import Path

import brotli
import lz4.block
import UnityPy
from UnityPy.helpers import Tpk, TypeTreeNode
from UnityPy.helpers.CompressionHelper import decompress_lzma
from UnityPy.helpers.UnityVersion import UnityVersion

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-tpk/tests/fixtures/tpk"
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import child  # noqa: E402
import scratch  # noqa: E402
from tpk_build import (  # noqa: E402
    BROTLI,
    I32_MAX,
    LZ4,
    LZMA,
    MAX_DECODED_BYTES,
    MAX_RECORDS,
    NONE,
    blob,
    both,
    chain,
    cls,
    compress,
    decoded_before_failing,
    far_match_stream,
    header,
    lz4_sequence,
    node,
    root_flags_blob,
    strings,
    tpk_file,
    varint,
    ver,
)

UNITYPY = "1.25.2"

def image_tpk() -> Path:
    return Path(UnityPy.__file__).parent / "resources" / "lzma.tpk"


# --- building TPKs --------------------------------------------------------------------
def synthetic_cases() -> Iterator[tuple[str, bytes, int]]:
    """`(name, TPK bytes, the tree depth to open it with)`."""
    yield "root_flags", tpk_file(root_flags_blob()), 256

    # Entries: removed and re-added, out of order, empty, a class listed twice.
    nodes = [node(0, 1, [1]), node(2, 3, []), node(4, 5, [])]
    entries = [
        (50, [(ver(2017), both(0)), (ver(2018), None), (ver(2019), both(2))]),
        (51, [(ver(2019), both(0)), (ver(2017), both(2)), (ver(2021), both(1))]),
        (52, []),
        (53, [(ver(2017), both(0))]),
        (54, [(ver(2018), None)]),
        (53, [(ver(2017), both(2))]),
    ]
    versions = [ver(2016), ver(2017), ver(2017, 3), ver(2018), ver(2019), ver(2020), ver(2021), ver(2022)]
    yield "entries", tpk_file(blob(versions, entries, nodes, strings(6))), 256

    # An entry's presence byte: UnityPy reads a class after any nonzero byte (`if
    # stream.read(1)[0]`), so 2, 0x80 and 0xFF are classes as 1 is, and 0 is none.
    nodes = [node(0, 1, [1]), node(2, 3, []), node(4, 5, [])]
    entries = [
        (55, [(ver(2017), both(0), 2), (ver(2018), None), (ver(2019), both(2), 0x80), (ver(2020), both(1), 0xFF)]),
        (56, [(ver(2017), both(2), 0xFF)]),
    ]
    versions = [ver(2016), ver(2017), ver(2018), ver(2019), ver(2020)]
    yield "presence_bytes", tpk_file(blob(versions, entries, nodes, strings(6))), 256

    # Type bytes 0-5, 6 and 255, in the version list, the entries and the common strings.
    types = [0, 1, 2, 3, 4, 5, 6, 255]
    at = [ver(2, typ=t) for t in types]
    nodes = [node(i % 3, 3, []) for i in range(8)]
    entries = [(60, [(v, both(i)) for i, v in enumerate(at)])]
    versions = sorted({*at, *(v - 1 for v in at), *(v + 1 for v in at), ver(1), ver(3)})
    common = [(v, 1) for v in at]
    yield "version_types", tpk_file(blob(versions, entries, nodes, strings(4), common, [0])), 256

    # Common strings: every index, in order, the version list unused; offsets count code
    # points (é is two bytes, one code point), an index listed twice is numbered twice.
    names = ["Base", "é", "日本", "", "m_X", "unused"]
    common = [(ver(2019), 2), (ver(2021), 1)]
    nodes = [node(0, 0, [1]), node(4, 1, [])]
    entries = [(90, [(ver(2020), both(0))])]
    indices = [1, 0, 2, 1, 3, 4]
    yield "common_strings", tpk_file(blob([ver(2020)], entries, nodes, names, common, indices)), 256

    # Every flag byte (`<HHb`: signed, which TpkUnityClassFlags takes as its eight bits),
    # each class's roots written as its flags name them: the editor root under 64, the
    # release root under 128.
    nodes = [node(0, 1, [1]), node(2, 3, []), node(4, 5, [])]
    entries = []
    for flags in range(256):
        editor = 2 if flags & 64 else None
        release = 0 if flags & 128 else None
        entries.append((1000 + flags, [(ver(2020), cls(flags, editor, release))]))
    yield "flag_bytes", tpk_file(blob([ver(2020)], entries, nodes, strings(6))), 256

    # A node's other fields as UnityPy keeps them: a negative version, byte sizes of
    # every sign, type flags (`b`, not copied into the node).
    nodes = [
        node(0, 1, [1, 2], byte_size=0, version=-2, type_flags=-1),
        node(2, 3, [], byte_size=-(2**31), version=-(2**15), type_flags=0x55),
        node(4, 5, [], byte_size=2**31 - 1, version=2**15 - 1, type_flags=1),
    ]
    yield "node_fields", tpk_file(blob([ver(2020)], [(81, [(ver(2020), both(0))])], nodes, strings(6))), 256

    # Meta flags of 2^31 and more: a TPK node stores a u32 (Tpk.py:361), which UnityPy
    # keeps as a positive int, so NodeRow's i64 must not read it as negative.
    nodes = [node(0, 1, [1, 2], meta=0x8000_4000), node(2, 3, [], meta=0xFFFF_FFFF), node(4, 5, [], meta=1 << 31)]
    yield "meta_flag_u32", tpk_file(blob([ver(2020)], [(80, [(ver(2020), both(0))])], nodes, strings(6))), 256

    # A 300-deep chain: UnityPy builds it, levels 0 to 299.
    yield "deep_chain_300", tpk_file(blob([ver(2020)], [(1, [(ver(2020), both(0))])], chain(300), strings(2))), 300

    # Ids past their tables: UnityPy raises IndexError.
    nodes = [node(0, 1, [1]), node(2, 3, []), node(0, 1, [9]), node(0, 9, [])]
    entries = [(70, [(ver(2020), both(9))]), (71, [(ver(2020), both(2))]), (72, [(ver(2020), both(3))])]
    entries.append((73, [(ver(2020), both(0))]))
    yield "ids_past_their_tables", tpk_file(blob([ver(2020)], entries, nodes, strings(4))), 256

    # Counts: a negative count UnityPy reads with range() is no items; one it reads as a
    # struct format raises; a huge one runs out of bytes.
    one = ([ver(2020)], [(1, [(ver(2020), both(0))])], [node(0, 1, [1]), node(2, 3, [])], strings(4))
    keys = ("classes", "entries", "common", "nodes", "strings", "versions", "indices")
    for key in keys:
        for value in (-1, -(2**31), 2**30, I32_MAX):
            yield f"count_{key}_{value}", tpk_file(blob(*one, counts={key: value})), 256
    # A negative count with its items written after it anyway: they are read as what
    # follows. Only the indices' own case names a common string, so a TPK UnityPy loads
    # is not one unityrs refuses for its common strings.
    for key in keys:
        indices = [0] if key == "indices" else []
        data = blob(*one, [(ver(2020), 1)], indices, counts={key: -1}, items_anyway=True)
        yield f"count_{key}_-1_items_follow", tpk_file(data), 256

    # A string length prefix of 5 bytes, the most unityrs reads (refused_cases holds one
    # of 6), not minimal; the string ends the payload.
    data = blob([ver(2020)], [(1, [(ver(2020), both(0))])], [node(0, 1, [1]), node(2, 3, [])], strings(4))
    yield "string_length_5_bytes", tpk_file(data[:-3] + b"\x82\x80\x80\x80\x00" + data[-2:]), 256

    # No classes at all, and a truncated file.
    yield "no_classes", tpk_file(blob([], [], [], [])), 256
    full = tpk_file(root_flags_blob())
    yield "truncated", full[: len(full) - 5], 256
    yield "negative_compressed_size", tpk_file(root_flags_blob(), sizes=(-1, 0)), 256
    # Compressed sizes past the file, whatever the codec, and a stored payload's
    # uncompressed size, which nothing reads.
    payload = root_flags_blob()
    for codec, codec_name in ((NONE, "stored"), (LZ4, "lz4"), (LZMA, "lzma")):
        data = compress(payload, codec)
        for size in (I32_MAX, 2**31):
            sizes = (size, len(payload))
            yield f"compressed_size_{codec_name}_{size}", tpk_file(payload, codec, sizes, data), 256
    yield "stored_declared_u32_max", tpk_file(payload, sizes=(len(payload), 2**32 - 1)), 256

    # Compressed payloads.
    yield "lz4", tpk_file(root_flags_blob(), LZ4), 256
    yield "lzma", tpk_file(root_flags_blob(), LZMA), 256
    yield from codec_cases()
    yield from header_cases()


def codec_cases() -> Iterator[tuple[str, bytes, int]]:
    """Blocks and streams on which python-lz4 (liblz4 1.9.4) and liblzma decide
    differently from lz4_flex and lzma-rs, and the header sizes each codec reads."""
    payload = root_flags_blob()
    n = len(payload)

    # A match at offset 0, four of the payload's zeros: liblz4 writes zeros for it
    # (the whole payload decodes), lz4_flex refuses the offset.
    z = payload.index(b"\0\0\0\0")
    block = lz4_sequence(payload[:z], 0, 4) + lz4_sequence(payload[z + 4 :])
    assert lz4.block.decompress(block, n) == payload
    yield "lz4_offset_0_match", tpk_file(payload, LZ4, data=block), 256

    # The last match ends 2 bytes before the end: liblz4 needs the last 5 to be
    # literals and refuses the block; lz4_flex decodes it.
    tail = payload + b"\0" * 8 + b"zz"
    block = lz4_sequence(payload + b"\0", 1, 7) + lz4_sequence(b"zz")
    try:
        lz4.block.decompress(block, len(tail))
        raise AssertionError("liblz4 decoded a match into the last literals")
    except lz4.block.LZ4BlockError:
        pass
    yield "lz4_match_in_the_last_literals", tpk_file(tail, LZ4, data=block), 256

    # The header's size is a maximum to liblz4 (a larger one decodes; at 2^31 and
    # above python-lz4 refuses it), and LZMA ignores it.
    block = compress(payload, LZ4)
    for name, size in [("larger", n + 100), ("smaller", n - 1), ("2_31", 2**31), ("u32_max", 2**32 - 1)]:
        yield f"lz4_declared_{name}", tpk_file(payload, LZ4, sizes=(len(block), size), data=block), 256
    stream = compress(payload, LZMA)
    for name, size in [("zero", 0), ("smaller", n - 1), ("larger", n + 100), ("u32_max", 2**32 - 1)]:
        yield f"lzma_declared_{name}", tpk_file(payload, LZMA, sizes=(len(stream), size), data=stream), 256

    # LZMA to the end of the stream: a corrupt end marker is liblzma's error (lzma-rs,
    # stopping at the header's size, never read it); a stream cut short decodes what
    # it holds; properties liblzma refuses (pb 5).
    corrupt = bytearray(stream)
    corrupt[-3] ^= 0xFF
    yield "lzma_end_marker_corrupt", tpk_file(payload, LZMA, data=bytes(corrupt)), 256
    yield "lzma_cut_short", tpk_file(payload, LZMA, data=stream[:-8]), 256
    bad_props = bytes([225]) + stream[1:]
    yield "lzma_bad_properties", tpk_file(payload, LZMA, data=bad_props), 256
    yield from far_match_cases()


# Where a stream's one far match begins in each `far_match_cases` payload, by the
# header's declared size. 1 MiB - 1 is review round 13's: 4 KiB before the end. 4095:
# unityrs-core's LZMA windows then end 16,772,632 bytes in, just below the output's
# room of 4095 * 2^12 bytes, so an attempt that fails there and a retry that grows that
# room past it would hold both blocks and a 32 MiB dictionary, more than the decoder's
# 64 MiB (`TpkProvider::MAX_DECODER_MEMORY`), had the output not been allocated whole.
FAR_MATCHES = {1048575: MAX_DECODED_BYTES - 4096, 4095: 16_772_568}


def far_match_cases() -> Iterator[tuple[str, bytes, int]]:
    """Payloads of exactly `MAX_DECODED_BYTES` whose LZMA stream has one match about
    16 MiB back, near its end: a decoder whose dictionary is smaller fails there, and
    unityrs-core's codec decodes again with a dictionary of twice what it made, about
    32 MiB, beside the output (review round 13: with the output's room grown as it
    decoded, a valid TPK was refused for decoder memory, depending on the header's
    declared size, which UnityPy ignores)."""
    blob = root_flags_blob()
    marked = hashlib.sha256(b"unityrs far match").digest()
    for declared, at in FAR_MATCHES.items():
        # A marker byte before the first copy, so no match can start before the second.
        payload = bytearray(MAX_DECODED_BYTES)
        payload[: len(blob)] = blob
        payload[len(blob)] = 1
        payload[len(blob) + 1 : len(blob) + 1 + len(marked)] = marked
        payload[at : at + len(marked)] = marked
        stream = far_match_stream(bytes(payload))
        failed = decoded_before_failing(stream, 4096)
        assert failed in (at - 1, at), (declared, at, failed)
        sizes = (len(stream), declared)
        name = f"lzma_far_match_at_the_decoded_bound_declared_{declared}"
        yield name, tpk_file(bytes(payload), LZMA, sizes=sizes, data=stream), 256


def header_cases() -> Iterator[tuple[str, bytes, int]]:
    """The header's checks (`Tpk.py:155-171`) and the reads that run out where a
    struct read does not raise `error`: each case one UnityPy refuses."""
    payload = root_flags_blob()
    stored = tpk_file(payload)

    def with_byte(at: int, value: int, data: bytes = stored) -> bytes:
        return data[:at] + bytes([value]) + data[at + 1 :]

    # The data type (byte 6): 1-5 are members whose blob is not a type-tree blob
    # (`ToBlob`, then `init`'s assertion), the rest are not members (signed: 128 is
    # -128, 255 is -1). The payload is decoded before the blob is chosen.
    for data_type in (1, 2, 3, 4, 5, 6, 127, 128, 255):
        yield f"data_type_{data_type}", with_byte(6, data_type), 256
    # A block liblz4 refuses (codec_cases' last-literals match) under data type 4:
    # the decode's error, not the data type's.
    tail = payload + b"\0" * 8 + b"zz"
    block = lz4_sequence(payload + b"\0", 1, 7) + lz4_sequence(b"zz")
    yield "data_type_4_lz4_refused", with_byte(6, 4, tpk_file(tail, LZ4, data=block)), 256
    # A blob of data type 1-3 that UnityPy's own reader cannot read (`ToBlob`,
    # `Tpk.py:113-121`): it raises that reader's exception before `init`'s assertion,
    # and unityrs, which reads no such blob, refuses it as it refuses one that reads
    # (`PARITY.md`, `TpkFile`'s row; review round 16). A collection is a count, then
    # each blob's string, data type and blob (`:243-256`), so it nests, and a nested
    # blob raises its own exception: `ToBlob`'s for a nested 4, the type-tree reader's
    # for a nested 0 (one that ends at a common-string version's count byte:
    # `IndexError`); a string is a varint length and its UTF-8 (`read_string`,
    # `:445-456`).
    one = struct.pack("<i", 1)
    not_utf8 = varint(1) + b"\xff"
    nested = (one + varint(0) + bytes([1])) * 1000
    cut = struct.pack("<qiiiQ", 0, 0, 0, 1, 0)
    unread = {
        "data_type_1_collection_not_utf8": (1, one + not_utf8),
        "data_type_1_collection_nested_data_type_9": (1, one + varint(0) + bytes([9])),
        "data_type_1_collection_nested_data_type_4": (1, one + varint(0) + bytes([4])),
        "data_type_1_collection_nested_type_tree_cut": (1, one + varint(0) + bytes([0]) + cut),
        "data_type_1_collection_nested_1000_deep": (1, nested),
        "data_type_2_file_system_not_utf8": (2, one + not_utf8),
        "data_type_2_file_system_empty": (2, b""),
        "data_type_3_json_not_utf8": (3, not_utf8),
        "data_type_3_json_empty": (3, b""),
        "data_type_3_json_length_past_an_index": (3, varint((1 << 64) - 1)),
    }
    for name, (data_type, data) in unread.items():
        yield name, with_byte(6, data_type, tpk_file(data)), 256
    # The compression type (byte 5), the version (byte 4) and the magic.
    for compression in (4, 127, 128, 255):
        yield f"compression_{compression}", with_byte(5, compression), 256
    for version in (0, 2, 255):
        yield f"version_{version}", with_byte(4, version), 256
    yield "bad_magic", with_byte(0, ord("U")), 256
    yield "header_19_bytes", stored[:19], 256
    yield "empty_file", b"", 256
    # An LZMA payload of 4 bytes: `decompress_lzma` unpacks 5 bytes of properties.
    yield "lzma_properties_4_bytes", tpk_file(payload, LZMA, data=compress(payload, LZMA)[:4]), 256

    # A payload that ends where `stream.read(1)[0]` reads: an entry's presence byte,
    # a common-string version's count byte (`IndexError`).
    at = ver(2020)
    one = blob([at], [(1, [(at, both(0))])], [node(0, 1, [1]), node(2, 3, [])], strings(4))
    presence = 8 + 4 + 8 + 4 + 4 + 4 + 8
    assert one[presence] == 1
    yield "cut_at_presence_byte", tpk_file(one[:presence]), 256
    common = blob([at], [], [node(0, 1, [])], strings(2), [(at, 1)], [0])
    count = 8 + 4 + 8 + 4 + 4 + 8
    assert common[count] == 1
    yield "cut_at_common_count_byte", tpk_file(common[:count]), 256
    # Past a table's bound, where UnityPy runs out on another exception than `error`
    # or `ValueError`: entries that end at a presence byte (`IndexError`), strings
    # whose first is not UTF-8 (`UnicodeDecodeError`).
    head = blob([at], [], [], [])[:20]
    entries = struct.pack("<iiI", 1, 1, MAX_RECORDS + 1) + struct.pack("<QB", at, 0)
    data = head + entries + struct.pack("<Q", at)
    yield "class_entries_past_the_bound_cut_at_presence_byte", tpk_file(data), 256
    data = blob([at], [], [node(0, 1, [])], ["x"], counts={"strings": MAX_RECORDS + 1})
    yield "strings_past_the_bound_not_utf8", tpk_file(data[:-1] + b"\xff"), 256
    # Within every bound, a string that is not UTF-8 (`.decode("utf-8")`, strict:
    # `UnicodeDecodeError`): a byte no UTF-8 holds, an encoded surrogate, overlong
    # forms, a code point past U+10FFFF, a lone continuation byte and a sequence cut
    # short, each the last string or one with strings after it.
    nodes = [node(0, 1, [1]), node(2, 3, [])]
    bad = {"ff": b"\xff", "surrogate": b"\xed\xa0\x80", "overlong_nul": b"\xc0\x80"}
    bad |= {"overlong_slash": b"\xe0\x80\xaf", "past_10ffff": b"\xf4\x90\x80\x80"}
    bad |= {"continuation": b"a\x80", "cut_short": b"\xe6\x97z"}
    for name, raw in bad.items():
        for where, names in (("last", [*strings(4), raw]), ("first", [raw, *strings(4)])):
            data = blob([at], [(1, [(at, both(0))])], nodes, names)
            yield f"string_not_utf8_{name}_{where}", tpk_file(data), 256


def refused_cases() -> Iterator[tuple[str, bytes, str]]:
    """`(name, TPK bytes, what unityrs refuses it for)`: TPKs UnityPy loads and
    unityrs refuses by design, each one record past a `TpkProvider` bound (the
    `TpkError::TooLarge` it names), a brotli one (`Unsupported`), two strings UnityPy
    reads on (`Malformed`, its message), and one whose common strings name a string
    past the buffer (`CommonStringPastBuffer`)."""
    over = MAX_RECORDS + 1
    at = ver(2020)
    nodes, names = [node(0, 1, [])], strings(2)
    big = blob([at], [], nodes, names) + b"\0" * (MAX_DECODED_BYTES + 1)
    yield "refused_decoded_payload", tpk_file(big[: MAX_DECODED_BYTES + 1], LZMA), "decoded payload"
    tables = [
        ("versions", blob([at] * over, [], nodes, names)),
        ("classes", blob([at], [(i, []) for i in range(over)], nodes, names)),
        ("class entries", blob([at], [(1, [(at, None)] * over)], nodes, names)),
        ("common-string versions", blob([at], [], nodes, names, [(at, 0)] * over)),
        ("common-string indices", blob([at], [], nodes, names, [], [0] * over)),
        ("nodes", blob([at], [], [node(0, 1, [])] * over, names)),
        ("strings", blob([at], [], nodes, [""] * over)),
    ]
    for table, data in tables:
        yield f"refused_{table.replace(' ', '_').replace('-', '_')}", tpk_file(data, LZMA), table
    yield "refused_brotli", tpk_file(root_flags_blob(), BROTLI), "brotli"
    # The last string runs past the payload's end, which UnityPy decodes short; and a
    # length prefix of 6 bytes, which UnityPy reads on.
    data = blob([at], [(1, [(at, both(0))])], [node(0, 1, [1]), node(2, 3, [])], [*strings(4), "abcde"])
    yield "refused_string_past_the_payload", tpk_file(data[:-2]), "a string longer than the rest of the payload"
    data = blob([at], [(1, [(at, both(0))])], [node(0, 1, [1]), node(2, 3, [])], [*strings(4), ""])
    data = data[:-1] + b"\x80\x80\x80\x80\x80\x00"
    yield "refused_string_length_6_bytes", tpk_file(data), "a string length of more than 5 bytes"
    # A common string past the string buffer: UnityPy raises IndexError in every blob
    # parse (`get_common_strings()`); unityrs refuses the TPK.
    data = blob([at], [], nodes, strings(2), [], [0, 2])
    yield "refused_common_string_index", tpk_file(data), "common-string index"


# --- running Tpk.py -------------------------------------------------------------------


def node_rows(tree) -> list[list]:
    return [
        [
            n.m_Level,
            n.m_Type,
            n.m_Name,
            n.m_ByteSize,
            n.m_Version,
            n.m_TypeFlags,
            n.m_VariableCount,
            n.m_Index,
            n.m_MetaFlag,
            n.m_RefTypeHash,
        ]
        for n in tree.traverse()
    ]


def class_outcomes(class_id: int, versions: list[int], trees: dict[int, int]) -> dict:
    """UnityPy's outcome for `class_id` at each version, deduplicated: `picks[i]` is the
    index in `outcomes` of the outcome at `versions[i]`. A node list's `tree` numbers
    the tree object UnityPy returned, the same number for the same object across every
    class of the TPK (`trees`: its `id()` to its number), so a test can check that
    unityrs shares a tree wherever UnityPy's caches do."""
    outcomes: list[dict] = []
    seen: dict[object, int] = {}
    picks = []
    for v in versions:
        try:
            tree = Tpk.get_typetree_node(class_id, UnityVersion(v))
            key: object = ("tree", id(tree))
            if key not in seen:
                outcome = {"rows": node_rows(tree), "tree": trees.setdefault(id(tree), len(trees))}
        except (KeyError, ValueError, AssertionError) as e:
            key = ("none", type(e).__name__)
            outcome = {"none": type(e).__name__}
        except Exception as e:  # noqa: BLE001 - an IndexError from a walk past a table
            key = ("error", type(e).__name__)
            outcome = {"error": type(e).__name__}
        if key not in seen:
            seen[key] = len(outcomes)
            outcomes.append(outcome)
        picks.append(seen[key])
    return {"class": class_id, "picks": picks, "outcomes": outcomes}


class Installed:
    """Puts `blob` in place of the bundled TPK, with empty caches, for the `with` block."""

    def __init__(self, blob: Tpk.TpkTypeTreeBlob) -> None:
        self.blob = blob

    def __enter__(self) -> None:
        self.saved = Tpk.TPKTYPETREE
        Tpk.TPKTYPETREE = self.blob
        self.clear()

    def __exit__(self, *exc: object) -> None:
        Tpk.TPKTYPETREE = self.saved
        self.clear()

    @staticmethod
    def clear() -> None:
        Tpk.CLASSES_CACHE.clear()
        Tpk.NODES_CACHE.clear()
        TypeTreeNode.COMMONSTRING_CACHE.clear()


def load(data: bytes) -> Tpk.TpkTypeTreeBlob:
    """`init()`'s load (`Tpk.py:36-39`)."""
    found = Tpk.TpkFile(BytesIO(data)).GetDataBlob()
    assert isinstance(found, Tpk.TpkTypeTreeBlob)
    return found


def outcomes(data: bytes) -> Iterator[dict]:
    """A header, then one record per class (and one for a class the TPK does not hold),
    then a trailer: how many records were written and how many (class, version) pairs
    UnityPy was asked for, counted as it ran, so a test can tell a comparison that
    missed any of them, or an outcome file cut short."""
    try:
        found = load(data)
    except Exception as e:  # noqa: BLE001 - whatever the reader raises
        yield {"load": type(e).__name__}
        return
    versions = [int(v) for v in found.Versions]
    with Installed(found):
        yield {
            "load": None,
            "class_count": len(found.ClassInformation),
            "versions": [str(v) for v in versions],
            "common_strings": common_strings(),
        }
        absent = max(found.ClassInformation, default=0) + 1
        trees: dict[int, int] = {}
        records = pairs = 0
        for class_id in [*found.ClassInformation, absent]:
            record = class_outcomes(class_id, versions, trees)
            records += 1
            pairs += len(record["picks"])
            yield record
        yield {"records": records, "pairs": pairs}


def common_strings() -> list | dict:
    """`get_common_strings()` with the installed blob (the one call, `version=None`):
    `[offset, string]` pairs, or the exception's name."""
    try:
        table = TypeTreeNode.get_common_strings()
    except Exception as e:  # noqa: BLE001 - whatever it raises
        return {"error": type(e).__name__}
    return [[o, s] for o, s in sorted(table.items())]


def synthetic() -> list[dict]:
    cases = []
    for name, data, depth in synthetic_cases():
        records = list(outcomes(data))
        head, classes, tail = records[0], records[1:-1], records[-1] if len(records) > 1 else {}
        cases.append({"name": name, "tpk": data.hex(), "depth": depth, **head, "classes": classes, **tail})
    for name, data, what in refused_cases():
        found = load(data)
        with Installed(found):
            common = common_strings()
        cases.append(
            {
                "name": name,
                "tpk": data.hex(),
                "depth": 256,
                "load": None,
                "class_count": len(found.ClassInformation),
                "common_strings": common,
                "refused": what,
            }
        )
    return cases


# --- output ---------------------------------------------------------------------------


def dump(cases: list[dict]) -> str:
    """One case per line, so a regenerated fixture diffs by case."""
    lines = [json.dumps(c, sort_keys=True, separators=(",", ":")) for c in cases]
    return "{\n \"cases\": [\n  " + ",\n  ".join(lines) + "\n ],\n \"unitypy\": " + json.dumps(UNITYPY) + "\n}\n"


def write_outcomes(tpk: Path, out: Path) -> None:
    with out.open("w") as f:
        for record in outcomes(tpk.read_bytes()):
            f.write(json.dumps(record, separators=(",", ":")) + "\n")


def survey() -> None:
    data = image_tpk().read_bytes()
    file = Tpk.TpkFile(BytesIO(data))
    found = load(data)
    decoded = {
        NONE: lambda b: b,
        LZ4: lambda b: lz4.block.decompress(b, file.UncompressedSize),
        LZMA: decompress_lzma,
        BROTLI: brotli.decompress,
    }[int(file.CompressionType)](file.CompressedBytes)
    tables = {
        "versions": len(found.Versions),
        "classes": len(found.ClassInformation),
        "class entries": sum(len(i) for i in found.ClassInformation.values()),
        "common-string versions": len(found.CommonString.VersionInformation),
        "common-string indices": len(found.CommonString.StringBufferIndices),
        "nodes": len(found.NodeBuffer),
        "strings": len(found.StringBuffer),
    }
    print(
        f"file {len(data)} bytes, compression {int(file.CompressionType)}, compressed {file.CompressedSize}, "
        f"header's uncompressed size {file.UncompressedSize}, decoded {len(decoded)}; tables: "
        + ", ".join(f"{k} {v}" for k, v in tables.items())
    )
    trees = rows = deepest = largest = 0
    records: set = set()
    roots: dict[int, int] = {}
    with Installed(found):
        for info in found.ClassInformation.values():
            for _, c in info:
                if c is None or c.ReleaseRootNode is None:
                    continue
                tree = node_rows(Tpk.generate_node(c))
                trees += 1
                rows += len(tree)
                largest = max(largest, len(tree))
                deepest = max(deepest, max(r[0] for r in tree))
                records.add(c)
                roots[c.ReleaseRootNode] = len(tree)
    print(
        f"classes {len(found.ClassInformation)}, versions {len(found.Versions)}, "
        f"entries {sum(len(i) for i in found.ClassInformation.values())}, trees {trees}, "
        f"rows {rows}, largest tree {largest} rows, deepest level {deepest}, "
        f"largest meta flag {max(n.MetaFlag for n in found.NodeBuffer)}"
    )
    # NODES_CACHE keeps one tree per distinct class record; unityrs-tpk keeps one per
    # release root, the one field generate_node reads.
    print(
        f"distinct class records {len(records)}, distinct release roots {len(roots)}, "
        f"rows over the distinct roots {sum(roots.values())}"
    )


def run_g7(tpk: Path) -> int:
    """G7 on `tpk`: UnityPy's outcomes outside the repository, then unityrs-tpk's test."""
    with scratch.folder() as tmp:
        out = Path(tmp) / "outcomes.jsonl"
        write_outcomes(tpk, out)
        env = dict(os.environ, UNITYRS_ORACLE_TPK=str(tpk), UNITYRS_ORACLE_TPK_OUTCOMES=str(out))
        cmd = ["cargo", "test", "--locked", "-p", "unityrs-tpk", "--test", "g7", "--", "--exact", "image_tpk", "--nocapture"]
        run = child.run(cmd, cwd=ROOT, env=env, check=False, stdout=child.PIPE, text=True)
        sys.stdout.write(run.stdout)
        if run.returncode != 0:
            return run.returncode
        # A filter that matches no test passes with nothing run: G7 cannot skip.
        if "test result: ok. 1 passed;" not in run.stdout:
            print("G7: cargo ran no image_tpk test", file=sys.stderr)
            return 1
        return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="compare the fixtures, then run G7 on the image's TPK")
    ap.add_argument("--outcomes", type=Path, help="write UnityPy's outcomes for --tpk here, as JSON lines")
    ap.add_argument("--tpk", type=Path, help="the TPK for --outcomes (default: the image's)")
    ap.add_argument("--survey", action="store_true", help="print counts about the image's TPK")
    args = ap.parse_args()
    if UnityPy.__version__ != UNITYPY:
        print(f"needs UnityPy {UNITYPY}, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    if args.survey:
        survey()
        return 0
    if args.outcomes:
        write_outcomes(args.tpk or image_tpk(), args.outcomes)
        return 0
    text = dump(synthetic())
    path = OUT / "synthetic.json"
    if args.check:
        if not path.exists() or path.read_text() != text:
            print("stale fixture (run tools/fixtures/tpk.py): " + str(path.relative_to(ROOT)), file=sys.stderr)
            return 1
        return run_g7(image_tpk())
    OUT.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
