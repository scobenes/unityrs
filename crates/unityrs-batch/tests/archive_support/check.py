"""Independent section-8 encoder and seekable virtual-prefix zipfile reader.

Only synthetic members enter this test-only checker. Python's ZIP writer is
never the byte oracle. The virtual prefix occupies logical positions, not RAM.
"""
import hashlib
import io
import json
from pathlib import Path
import struct
import sys
import zipfile
import zlib

ZIP32 = 0xFFFFFFFF
ZIP16 = 0xFFFF
PART = 16 * 1024 * 1024


def encode(members, origin=0):
    local = bytearray()
    central = bytearray()
    for name, payload in members:
        name = name.encode('utf-8')
        size = len(payload)
        crc = zlib.crc32(payload)
        offset = origin + len(local)
        sizes = size >= ZIP32
        local_extra = struct.pack('<HHQQ', 1, 16, size, size) if sizes else b''
        fields = (struct.pack('<QQ', size, size) if sizes else b'')
        if offset >= ZIP32:
            fields += struct.pack('<Q', offset)
        extra = struct.pack('<HH', 1, len(fields)) + fields if fields else b''
        classic = min(size, ZIP32)
        local += struct.pack('<I5H3I2H', 0x04034B50,
                             45 if sizes else 20, 0x0800, 0, 0, 0x21,
                             crc, classic, classic, len(name), len(local_extra))
        local += name + local_extra + payload
        central += struct.pack('<I6H3I5H2I', 0x02014B50,
                               20, 45 if extra else 20, 0x0800, 0, 0, 0x21,
                               crc, classic, classic, len(name), len(extra),
                               0, 0, 0, 0, min(offset, ZIP32))
        central += name + extra
    start = origin + len(local)
    count = len(members)
    tail = bytearray()
    if count >= ZIP16 or len(central) >= ZIP32 or start >= ZIP32:
        tail += struct.pack('<IQ2H2I4Q', 0x06064B50, 44, 45, 45, 0, 0,
                            count, count, len(central), start)
        tail += struct.pack('<IIQI', 0x07064B50, 0, start + len(central), 1)
    tail += struct.pack('<I4H2IH', 0x06054B50, 0, 0,
                        min(count, ZIP16), min(count, ZIP16),
                        min(len(central), ZIP32), min(start, ZIP32), 0)
    return bytes(local + central + tail)


class VirtualPrefix(io.RawIOBase):
    def __init__(self, tail, origin):
        self.tail = tail
        self.origin = origin
        self.position = 0

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=0):
        base = (0, self.position, self.origin + len(self.tail))[whence]
        position = base + offset
        if position < 0:
            raise ValueError('negative seek')
        self.position = position
        return position

    def read(self, size=-1):
        end = self.origin + len(self.tail)
        if size < 0:
            size = end - self.position
        size = max(0, min(size, end - self.position))
        # zipfile seeks near the end; a mistaken unbounded prefix read fails
        # rather than allocating several GiB in the oracle.
        assert size <= 64 * 1024 * 1024, 'unbounded virtual read'
        prefix = min(size, max(0, self.origin - self.position))
        at = max(0, self.position - self.origin)
        result = bytes(prefix) + self.tail[at:at + size - prefix]
        self.position += len(result)
        return result


def etag(data):
    if len(data) <= PART:
        return hashlib.md5(data).hexdigest()
    combined = hashlib.md5()
    count = 0
    for at in range(0, len(data), PART):
        combined.update(hashlib.md5(data[at:at + PART]).digest())
        count += 1
    return f'{combined.hexdigest()}-{count}'


def check(path, description, origin, summary):
    data = Path(path).read_bytes()
    description = json.loads(Path(description).read_text())
    if isinstance(description, dict):
        members = [(f'm{i:06}', b'') for i in range(description['count'])]
    else:
        members = [(name, bytes.fromhex(payload)) for name, payload in description]
    expected = encode(members, origin)
    assert data == expected, 'canonical ZIP byte mismatch'
    with zipfile.ZipFile(VirtualPrefix(data, origin)) as archive:
        assert archive.namelist() == [name for name, _ in members]
        for info, (name, payload) in zip(archive.infolist(), members):
            assert info.filename == name
            assert info.compress_type == zipfile.ZIP_STORED
            assert info.flag_bits == 0x0800
            assert info.date_time == (1980, 1, 1, 0, 0, 0)
            assert info.comment == b''
            assert info.internal_attr == info.external_attr == 0
            assert archive.read(info) == payload
    result = json.loads(Path(summary).read_text())
    assert result['size'] == origin + len(data)
    assert result['emitted_size'] == len(data)
    assert result['md5'] == hashlib.md5(data).hexdigest()
    assert result['etag'] == etag(data)


def check_digests(path):
    for actual in json.loads(Path(path).read_text()):
        size = actual['size']
        whole = hashlib.md5()
        combined = hashlib.md5()
        current = hashlib.md5()
        part_size = count = 0
        left = size
        while left:
            n = min(65536, left, PART - part_size)
            chunk = b'x' * n
            whole.update(chunk)
            current.update(chunk)
            part_size += n
            left -= n
            if part_size == PART:
                combined.update(current.digest())
                count += 1
                current = hashlib.md5()
                part_size = 0
        if part_size:
            combined.update(current.digest())
            count += 1
        expected_etag = whole.hexdigest() if size <= PART else f'{combined.hexdigest()}-{count}'
        assert actual['md5'] == whole.hexdigest()
        assert actual['etag'] == expected_etag


if __name__ == '__main__':
    if sys.argv[1] == '--digests':
        check_digests(sys.argv[2])
    else:
        check(sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4])
