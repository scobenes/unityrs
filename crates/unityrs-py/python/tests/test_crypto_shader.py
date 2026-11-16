"""Standalone helper edge behavior through the installed wheel."""
import importlib
import struct
import unittest

import unityrs
from unityrs.streams import EndianBinaryReader

S = importlib.import_module('unityrs.export.ShaderConverter')
C = importlib.import_module('unityrs.helpers.ArchiveStorageManager')


class CryptoShader(unittest.TestCase):
    def test_invalid_key_and_vector_lengths(self):
        with self.assertRaisesRegex(ValueError, 'Incorrect AES key length'):
            C.decrypt_key(bytes(16), b'x', b'bad')
        with self.assertRaisesRegex(ValueError, 'aligned to block boundary'):
            C.decrypt_key(b'x', b'x', bytes(16))

    def test_empty_aes_vector(self):
        self.assertEqual(C.decrypt_key(b'', b'ignored', bytes(16)), b'')

    def test_mutated_decryptor_tables_do_not_overflow(self):
        value = C.ArchiveStorageDecryptor.__new__(C.ArchiveStorageDecryptor)
        value.index = bytes([255] * 16)
        value.substitute = bytes([255] * 16)
        data = bytearray(b'\x00')
        value.decrypt_byte(data, 0, 0)
        self.assertEqual(value.decrypt_block(b'\x00', 0), bytes(data))

    def test_malformed_program_count_is_bounded(self):
        with self.assertRaises(unityrs.LimitError):
            S.ShaderProgram(EndianBinaryReader(struct.pack('<i', 2_000_000_000), endian='<'), (2018, 1))

    def test_program_negative_count_and_plain_export(self):
        value = S.ShaderProgram(EndianBinaryReader(struct.pack('<i', -1), endian='<'), (2018, 1))
        self.assertEqual(value.m_SubPrograms, [])
        self.assertEqual(value.Export('plain text'), 'plain text')
        with self.assertRaises(IndexError):
            value.Export('GpuProgramIndex 0')

    def test_shader_blend_placeholder_and_unknown_platform(self):
        self.assertEqual(S.ConvertSerializedShaderRTBlendState(object()), '')
        self.assertEqual(S.GetPlatformString(12345), 'unknown')
        with self.assertRaises(NotImplementedError):
            S.CheckGpuProgramUsable(12345, 1)


if __name__ == '__main__':
    unittest.main()
