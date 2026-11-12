"""Independent stdlib checks over actual disk artifacts; no oracle dependency."""
import hashlib
import json
from pathlib import Path
import struct
import sys
import zlib


def pixels(data):
    width, height, depth, color = struct.unpack('>IIBB', data[16:26])
    assert depth == 8 and color in (2, 6)
    channels = 3 if color == 2 else 4
    parts = []
    at = 8
    ended = False
    while at < len(data):
        size = int.from_bytes(data[at:at + 4], 'big')
        kind = data[at + 4:at + 8]
        body = data[at + 8:at + 8 + size]
        assert zlib.crc32(kind + body) == int.from_bytes(data[at + 8 + size:at + 12 + size], 'big')
        if kind == b'IDAT':
            parts.append(body)
        at += size + 12
        if kind == b'IEND':
            assert size == 0 and at == len(data), 'invalid PNG IEND or trailing data'
            ended = True
            break
    assert ended, 'PNG is missing IEND'
    stream = zlib.decompressobj()
    try:
        raw = stream.decompress(b''.join(parts))
        raw += stream.flush()
    except zlib.error as error:
        raise AssertionError(f'invalid PNG zlib stream: {error}') from error
    assert stream.eof and not stream.unused_data and not stream.unconsumed_tail, (
        'PNG IDAT must contain exactly one complete zlib stream with no trailing bytes')
    stride = width * channels
    expected = height * (1 + stride)
    assert len(raw) == expected, f'PNG scanline length: expected {expected}, got {len(raw)}'
    out = bytearray()
    for y in range(height):
        row = raw[y * (stride + 1):(y + 1) * (stride + 1)]
        out.extend(bytes(stride))
        for x, value in enumerate(row[1:]):
            a = out[y * stride + x - channels] if x >= channels else 0
            b = out[(y - 1) * stride + x] if y else 0
            c = out[(y - 1) * stride + x - channels] if y and x >= channels else 0
            if row[0] == 4:
                p = a + b - c
                distances = [abs(p - a), abs(p - b), abs(p - c)]
                pred = [a, b, c][distances.index(min(distances))]
            else:
                pred = [0, a, b, (a + b) // 2][row[0]]
            out[y * stride + x] = (value + pred) % 256
        if y == 1 and out[:stride] == out[stride:]:
            # Huge boundary fixtures have identical rows. Prove the encoded
            # suffix repeats in C, then hash the repeated decoded row.
            if raw[2 * (stride + 1):] == row * (height - 2):
                return bytes(out[:stride]) * height, [width, height], 'RGB' if channels == 3 else 'RGBA'
    return bytes(out), [width, height], 'RGB' if channels == 3 else 'RGBA'


if __name__ == '__main__':
    for job in json.loads(Path(sys.argv[1]).read_text()):
        data = Path(job['file']).read_bytes()
        want = job['want']
        for alg in ('md5', 'sha256'):
            if alg in want:
                assert hashlib.new(alg, data).hexdigest() == want[alg], (job, alg)
        if 'crc32' in want:
            assert zlib.crc32(data) == want['crc32'], job
        if 'text' in want:
            assert data.decode('utf-8') == want['text'], job
        if 'mode' in want:
            image, size, mode = pixels(data)
            assert size == want['size'] and mode == want['mode'], job
            if want.get('visible_hash'):
                visible = bytearray()
                step = 3 if mode == 'RGB' else 4
                for at in range(0, len(image), step):
                    pixel = image[at:at + step]
                    if step == 3:
                        pixel += b'\xff'
                    visible.extend(pixel if pixel[3] else bytes(4))
                prefix = f'{size[0]}x{size[1]}:'.encode()
                assert hashlib.sha256(prefix + visible).hexdigest() == want['visible_hash'], job
            if 'pixels' in want:
                assert image.hex() == want['pixels'], job
            if 'pixels_sha256' in want:
                assert hashlib.sha256(image).hexdigest() == want['pixels_sha256'], job
