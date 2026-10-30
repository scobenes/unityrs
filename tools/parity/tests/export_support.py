"""What the export row tests share, not a test module: inputs laid out from the formats' own rules, not by UnityPy.

`element` is the default value of a field of a class's TPK node (`test_rows_objects.default_value`); `vertex_data`,
`submeshes` and `index_buffer` lay out a mesh's or a sprite's data; `fsb5` builds an FSB5 bank, and `wav_header` is
fmod_toolkit's RIFF header.
"""

import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402
from test_rows_objects import VERSION, default_value  # noqa: E402

if ORACLE:
    from UnityPy.helpers import Tpk
    from UnityPy.helpers.UnityVersion import UnityVersion


def element(class_id, *path, version=VERSION):
    """The default value of the field at `path` in `class_id`'s TPK node, an array's element for an array:
    `element(43, "m_SubMeshes")` is one SubMesh."""
    node = Tpk.get_typetree_node(class_id, UnityVersion.from_str(version))
    for name in path:
        node = next(child for child in node.m_Children if child.m_Name == name)
        if node.m_Children and node.m_Children[0].m_Type == "Array":
            node = node.m_Children[0].m_Children[1]
    return default_value(node)


def vertex_data(count, columns, endian="<"):
    """A 2019-and-later `m_VertexData` of `count` vertices. `columns` maps a channel to (stream, format, struct code,
    one tuple per vertex); a stream holds its channels interleaved in channel order and starts at a multiple of 16,
    the layout Unity writes and `MeshHandler.get_streams` derives."""
    channels = [{"stream": 0, "offset": 0, "format": 0, "dimension": 0} for _ in range(14)]
    data = b""
    for stream in sorted({column[0] for column in columns.values()}):
        own = sorted((chn, column) for chn, column in columns.items() if column[0] == stream)
        offset = 0
        for chn, (_, fmt, code, rows) in own:
            channels[chn] = {"stream": stream, "offset": offset, "format": fmt, "dimension": len(rows[0])}
            offset += struct.calcsize(code) * len(rows[0])
        for v in range(count):
            data += b"".join(struct.pack(f"{endian}{len(rows[v])}{code}", *rows[v]) for _, (_, _, code, rows) in own)
        data += bytes(-len(data) % 16)
    return {"m_VertexCount": count, "m_Channels": channels, "m_DataSize": data}


def submeshes(count, *spans, class_id=43, version=VERSION):
    """SubMesh values: one (first byte, index count, topology) per span."""
    out = []
    for first, n, topology in spans:
        sub = element(class_id, *(("m_RD",) if class_id == 213 else ()), "m_SubMeshes", version=version)
        sub.update(
            firstByte=first, indexCount=n, vertexCount=count, **({"topology": topology} if "topology" in sub else {})
        )
        out.append(sub)
    return out


def index_buffer(indices, code="H"):
    return list(struct.pack(f"<{len(indices)}{code}", *indices))


def fsb5(mode, *samples, channels=1):
    """An FSB5 (version 1) bank of `mode`'s codec: one (data, sample count) per subsound, each at 44100 Hz (rate
    index 8), the sample headers' data offsets in 32-byte units."""
    headers, data = b"", b""
    for payload, count in samples:
        data += bytes(-len(data) % 32)
        headers += struct.pack("<Q", 8 << 1 | (channels - 1) << 5 | (len(data) // 32) << 7 | count << 34)
        data += payload
    head = struct.pack("<6I", 1, len(samples), len(headers), 0, len(data), mode)
    return b"FSB5" + head + bytes(32) + headers + data


def wav_header(channels, rate, bits, length, audio_format=1):
    """fmod_toolkit's 44-byte RIFF header."""
    block = channels * bits // 8
    return struct.pack(
        "<4sI4s4sIHHIIHH4sI",
        b"RIFF", length + 36, b"WAVE", b"fmt ", 16, audio_format, channels, rate, rate * block, block, bits,
        b"data", length,
    )  # fmt: skip
