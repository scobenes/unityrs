"""Read the named files (nodes) of a UnityFS bundle, standard library only.

The fixture checks read a texture's stream range from the bundle's `.resS`
node, which `unityrs inspect` and `show` do not expose. Blocks may be stored,
LZMA or LZ4/LZ4HC compressed, as Unity writes them.
"""

import lzma
import struct

MAX_BYTES = 256 << 20


def lz4_block(src, size, require):
    out, i = bytearray(), 0

    def length(n):
        nonlocal i
        if n == 15:
            while True:
                require(i < len(src), "truncated LZ4 block")
                byte = src[i]
                i += 1
                n += byte
                if byte != 255:
                    break
        return n

    while i < len(src):
        token = src[i]
        i += 1
        n = length(token >> 4)
        require(i + n <= len(src) and len(out) + n <= size, "invalid LZ4 literal run")
        out += src[i:i + n]
        i += n
        if i == len(src):
            break
        require(i + 2 <= len(src), "truncated LZ4 match offset")
        offset = src[i] | src[i + 1] << 8
        i += 2
        n = length(token & 15) + 4
        require(0 < offset <= len(out) and len(out) + n <= size, "invalid LZ4 match")
        start = len(out) - offset
        while n:
            chunk = out[start:start + min(n, offset)]
            out += chunk
            start += len(chunk)
            n -= len(chunk)
    require(len(out) == size, "LZ4 block size mismatch")
    return bytes(out)


def decompress(data, size, flags, require):
    kind = flags & 0x3F
    if kind == 0:
        require(len(data) == size, "stored block size mismatch")
        return bytes(data)
    if kind == 1:
        require(len(data) >= 5, "truncated LZMA block")
        props, dict_size = data[0], int.from_bytes(data[1:5], "little")
        require(props < 9 * 5 * 5, "invalid LZMA properties")
        lc, rest = props % 9, props // 9
        filters = [dict(id=lzma.FILTER_LZMA1, lc=lc, lp=rest % 5, pb=rest // 5, dict_size=dict_size)]
        out = lzma.LZMADecompressor(lzma.FORMAT_RAW, filters=filters).decompress(data[5:], max_length=size)
        require(len(out) == size, "LZMA block size mismatch")
        return out
    require(kind in (2, 3), f"unsupported bundle compression {kind}")
    return lz4_block(data, size, require)


def bundle_nodes(path, require):
    """Map each node path to its bytes."""
    try:
        return read_nodes(path, require)
    except (struct.error, IndexError, lzma.LZMAError, UnicodeDecodeError) as exc:
        raise ValueError(f"unreadable bundle {path.name}: {exc}") from exc


def read_nodes(path, require):
    data = path.read_bytes()
    require(data.startswith(b"UnityFS\0"), f"invalid UnityFS signature: {path.name}")
    cursor = 8
    version, = struct.unpack_from(">I", data, cursor)
    cursor += 4
    for _ in range(2):
        end = data.index(b"\0", cursor)
        cursor = end + 1
    _, info_size, info_full, flags = struct.unpack_from(">qIII", data, cursor)
    cursor += 20
    if version >= 7:
        cursor += -cursor % 16
    if flags & 0x80:
        info = data[len(data) - info_size:]
    else:
        info = data[cursor:cursor + info_size]
        cursor += info_size
    require(len(info) == info_size and info_full <= MAX_BYTES, f"invalid bundle directory: {path.name}")
    info = decompress(info, info_full, flags, require)
    if flags & 0x200:
        cursor += -cursor % 16
    blocks, = struct.unpack_from(">i", info, 16)
    at, payload, total = 20, [], 0
    for _ in range(blocks):
        full, stored, block_flags = struct.unpack_from(">IIH", info, at)
        at += 10
        total += full
        require(total <= MAX_BYTES and cursor + stored <= len(data), f"invalid bundle block: {path.name}")
        payload.append(decompress(data[cursor:cursor + stored], full, block_flags, require))
        cursor += stored
    payload = b"".join(payload)
    count, = struct.unpack_from(">i", info, at)
    at += 4
    nodes = {}
    for _ in range(count):
        offset, size, _ = struct.unpack_from(">qqI", info, at)
        end = info.index(b"\0", at + 20)
        name = info[at + 20:end].decode("utf-8")
        at = end + 1
        require(offset >= 0 and size >= 0 and offset + size <= len(payload) and name not in nodes,
                f"invalid bundle node {name!r}: {path.name}")
        nodes[name] = payload[offset:offset + size]
    return nodes
