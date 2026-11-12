"""Malformed PNGs must fail before any pixel reconstruction or hashing."""
import struct
import unittest
import zlib

from check import pixels


def chunk(kind, body):
    return (struct.pack('>I', len(body)) + kind + body
            + struct.pack('>I', zlib.crc32(kind + body)))


def png(raw, *, tail=b'', iend=True):
    compressed = zlib.compress(raw) + tail
    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', 4, 4, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', compressed[:3])
            + chunk(b'IDAT', compressed[3:])
            + (chunk(b'IEND', b'') if iend else b''))


class PngChecks(unittest.TestCase):
    def test_complete_scanlines_across_idat_chunks(self):
        self.assertEqual(pixels(png(bytes(68))), (bytes(64), [4, 4], 'RGBA'))

    def test_missing_scanlines(self):
        with self.assertRaisesRegex(AssertionError, 'scanline length'):
            pixels(png(bytes(17)))

    def test_extra_decompressed_byte(self):
        with self.assertRaisesRegex(AssertionError, 'scanline length'):
            pixels(png(bytes(69)))

    def test_trailing_compressed_byte(self):
        with self.assertRaisesRegex(AssertionError, 'zlib stream'):
            pixels(png(bytes(68), tail=b'X'))

    def test_second_zlib_stream(self):
        with self.assertRaisesRegex(AssertionError, 'zlib stream'):
            pixels(png(bytes(68), tail=zlib.compress(b'')))

    def test_incomplete_zlib_stream(self):
        data = (b'\x89PNG\r\n\x1a\n'
                + chunk(b'IHDR', struct.pack('>IIBBBBB', 4, 4, 8, 6, 0, 0, 0))
                + chunk(b'IDAT', zlib.compress(bytes(68))[:-1])
                + chunk(b'IEND', b''))
        with self.assertRaisesRegex(AssertionError, 'zlib stream'):
            pixels(data)

    def test_missing_iend(self):
        with self.assertRaisesRegex(AssertionError, 'IEND'):
            pixels(png(bytes(68), iend=False))


if __name__ == '__main__':
    unittest.main()
