"""Standalone helper edge behavior through the installed wheel."""
import importlib
import struct
import unittest
from types import SimpleNamespace as N

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

    def test_stage_exports_current_type_without_reparsing_program(self):
        sub = S.ShaderSubProgram.__new__(S.ShaderSubProgram)
        sub.m_Version = 202012090
        sub.m_ProgramType = 999
        sub.m_Keywords = ['EDIT']
        sub.m_LocalKeywords = None
        sub.m_ProgramCode = b'\xff'
        program = S.ShaderProgram.__new__(S.ShaderProgram)
        program.m_SubPrograms = [sub]
        variants = [N(m_BlobIndex=0, m_GpuProgramType=1)]
        self.assertEqual(S.ConvertSerializedSubPrograms(variants, [0], [program]),
                         'SubProgram "openGL " {\nKeywords { "EDIT" }\n'
                         '"//shader disassembly not supported on 999"\n}\n')

    def test_stage_ignores_unused_programs(self):
        self.assertEqual(S.ConvertSerializedPass(N(m_Type=1, m_UseName='Synthetic/PASS'),
                                                [0], [None]), ' UsePass "Synthetic/PASS"\n')
        variants = [N(m_BlobIndex=0, m_GpuProgramType=9)]
        self.assertEqual(S.ConvertSerializedSubPrograms(variants, [0], [None]), '')


if __name__ == '__main__':
    unittest.main()
