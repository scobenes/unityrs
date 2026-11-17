"""Small-budget boundaries for the remaining reader-bearing helper entries."""
import importlib
import gc
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace as N
from unittest.mock import patch

from PIL import Image
sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
import binding_y6b_meter as O
import binding_y6b_review as R
from binding_y6b_crypto_shader import crypto_inputs
from binding_y6b_images import _mesh
import unityrs


class EntryMeters(unittest.TestCase):
    def boundary(self, obj, budget, call, check, high=65536):
        baseline = budget.used
        start = obj.reader.Position
        def attempt(available):
            obj.reader.Position = start
            held = obj._native.tree_traversal()
            held.step(0, budget.limit - budget.used - available, 0)
            try:
                check(call())
                return True
            except MemoryError:
                return False
            finally:
                del held
                gc.collect()
                self.assertEqual(budget.used, baseline)
        self.assertFalse(attempt(0))
        self.assertTrue(attempt(high))
        low = 0
        while high - low > 1:
            middle = (low + high) // 2
            if attempt(middle):
                high = middle
            else:
                low = middle
        self.assertFalse(attempt(high - 1))
        self.assertTrue(attempt(high))

    def owner(self, payload=None):
        budget = unityrs.Budget(4_000_000)
        if payload is None:
            obj, _ = O.tree_reader(unityrs, budget=budget)
        else:
            data = O.B.S.serialized_file([O.B.S.Type(49)], [O.B.S.Obj(1, 0, payload)], enable_type_tree=False)
            obj = unityrs.Environment(data, budget=budget).objects[0]
            obj.reader.Position = obj.byte_start
        return obj, budget

    def test_direct_reference_lookup_uses_asset_file_meter(self):
        helper = importlib.import_module('unityrs.helpers.TypeTreeHelper')
        obj, budget = self.owner()
        node = O.detached_node(unityrs)
        obj.assets_file.ref_types = [N(m_ClassName='Synthetic', m_NameSpace='', m_AssemblyName='', node=node)]
        self.boundary(obj, budget,
            lambda: helper.get_ref_type_node({'type': {'class': 'Synthetic', 'ns': '', 'asm': ''}}, obj.assets_file),
            lambda value: self.assertIs(value, node))

    def test_reference_read_write_and_accelerated_resolver_boundaries(self):
        helper = importlib.import_module('unityrs.helpers.TypeTreeHelper')
        from binding_y6b_tree import _node
        descriptor = _node(unityrs, 'SyntheticType', 'type', [
            _node(unityrs, 'string', name) for name in ('class', 'ns', 'asm')])
        root = _node(unityrs, 'ReferencedObject', 'Base', [
            descriptor, _node(unityrs, 'ReferencedObjectData', 'data')])
        value = {'type': {'class': 'Synthetic', 'ns': '', 'asm': ''}, 'data': 23}
        writer = unityrs.streams.EndianBinaryWriter(endian='<')
        helper.write_typetree(value['type'], descriptor, writer)
        writer.write_int(23)
        payload = writer.bytes
        for kind in ('pure', 'accelerated', 'value', 'write', 'write_value'):
            obj, budget = self.owner(payload)
            file = obj.assets_file
            file.ref_types = [N(m_ClassName='Synthetic', m_NameSpace='', m_AssemblyName='', node=_node(unityrs))]
            def call():
                if kind.startswith('write'):
                    out = unityrs.streams.EndianBinaryWriter(endian='<')
                    if kind == 'write':
                        helper.write_typetree(value, root, out, file)
                    else:
                        helper.write_value(value, root, out, helper.TypeTreeConfig(True, file))
                    return out.bytes
                if kind == 'value':
                    return helper.read_value(root, obj.reader, helper.TypeTreeConfig(True, file))
                return helper.read_typetree(root, obj.reader, byte_size=len(payload) if kind == 'accelerated' else None,
                                            check_read=False, assetsfile=file)
            with self.subTest(kind=kind):
                self.boundary(obj, budget, call,
                              lambda result: self.assertEqual(result, payload if kind.startswith('write') else value),
                              high=262144)

    def test_read_vector_uses_reader_meter_for_fixed_spans(self):
        helper = importlib.import_module('unityrs.helpers.ArchiveStorageManager')
        obj, budget = self.owner(bytes(range(33)))
        held = obj._native.tree_traversal()
        held.step(0, budget.limit - budget.used - 31, 0)
        try:
            with self.assertRaises(MemoryError):
                helper.read_vector(obj.reader)
        finally:
            del held
        obj.reader.Position = obj.byte_start
        self.boundary(obj, budget, lambda: helper.read_vector(obj.reader),
                      lambda value: self.assertEqual(value, (bytes(range(16)), bytes(range(16, 32)))))

    def test_decryptor_constructor_uses_reader_meter_through_key_handoff(self):
        helper = importlib.import_module('unityrs.helpers.ArchiveStorageManager')
        inputs = crypto_inputs()
        obj, budget = self.owner(bytes.fromhex(inputs['header']))
        obj.reader.endian = '>'
        def check(value):
            self.assertEqual(len(value.index), 16)
            self.assertEqual(len(value.substitute), 16)
            self.assertEqual(obj.reader.Position - obj.byte_start, 70)
        with patch.object(helper, 'DECRYPT_KEY', bytes.fromhex(inputs['key'])):
            # Header reads fit; the three live AES snapshots do not. A fresh
            # meter in cn_key would wrongly let construction finish here.
            baseline = budget.used
            held = obj._native.tree_traversal()
            held.step(0, budget.limit - budget.used - 288, 0)
            try:
                with self.assertRaises(MemoryError):
                    helper.ArchiveStorageDecryptor(obj.reader)
                self.assertEqual(obj.reader.Position - obj.byte_start, 70)
            finally:
                del held
            self.assertEqual(budget.used, baseline)
            obj.reader.Position = obj.byte_start
            self.boundary(obj, budget, lambda: helper.ArchiveStorageDecryptor(obj.reader), check)

    def test_shader_constructor_entry_boundaries(self):
        helper = importlib.import_module('unityrs.export.ShaderConverter')
        for program in (False, True):
            budget = unityrs.Budget(4_000_000)
            obj, reader = R.shader_reader(unityrs, 2, program, budget=budget)
            def construct():
                return helper.ShaderProgram(reader, (2019, 3)) if program else helper.ShaderSubProgram(reader)
            def check(value):
                self.assertEqual((value.m_SubPrograms[0] if program else value).m_Keywords, ['', ''])
            with self.subTest(program=program):
                self.boundary(obj, budget, construct, check)

    def test_sprite_geometry_entry_boundaries(self):
        helper = importlib.import_module('unityrs.export.SpriteHelper')
        for name in ('mask_sprite', 'render_sprite_mesh'):
            obj, budget = self.owner()
            sprite = N(object_reader=obj, m_PixelsToUnits=1.)
            with self.subTest(name=name):
                self.boundary(obj, budget, lambda: getattr(helper, name)(sprite, _mesh(), Image.new('RGBA', (4, 4))),
                              lambda value: self.assertEqual(value.mode, 'RGBA'))

    def test_texture_preprocessing_default_and_replaced_converter_boundaries(self):
        helper = importlib.import_module('unityrs.export.Texture2DConverter')
        for replaced in (False, True):
            obj, budget = self.owner()
            texture = N(object_reader=obj, m_Width=1, m_Height=1, m_TextureFormat=4,
                        m_PlatformBlob=None, get_image_data=lambda: b'\x01\x02\x03\xff')
            table = dict(helper.CONV_TABLE)
            if replaced:
                table[unityrs.enums.TextureFormat.RGBA32] = (lambda data, w, h: Image.frombytes('RGBA', (w, h), data), ())
            with patch.dict(helper.CONV_TABLE, table), self.subTest(replaced=replaced):
                self.boundary(obj, budget, lambda: helper.get_image_from_texture2d(texture),
                              lambda value: self.assertEqual(value.getpixel((0, 0)), (1, 2, 3, 255)))

    def test_sprite_sheet_and_image_entry_boundaries(self):
        helper = importlib.import_module('unityrs.export.SpriteHelper')
        for entry in ('get_image', 'get_image_from_sprite'):
            obj, budget = self.owner()
            texture = N(object_reader=obj, m_Width=1, m_Height=1, m_TextureFormat=4,
                        m_PlatformBlob=None, m_ImageData=b'\x01\x02\x03\xff', m_StreamData=None)
            pointer = N(path_id=1, deref_parse_as_object=lambda: texture)
            sprite = N(object_reader=obj, assets_file=obj.assets_file, m_PixelsToUnits=1.,
                       m_SpriteAtlas=None, m_AtlasTags=[], m_RD=N(texture=pointer, alphaTexture=None,
                       textureRect=N(x=0., y=0., width=1., height=1.), settingsRaw=2))
            texture.get_image_data = lambda **options: texture.m_ImageData
            def call():
                obj.assets_file._cache.clear()
                if entry == 'get_image':
                    return helper.get_image(sprite, pointer, None)
                return helper.get_image_from_sprite(sprite)
            with self.subTest(entry=entry):
                self.boundary(obj, budget, call,
                              lambda value: self.assertEqual(value.getpixel((0, 0)), (1, 2, 3, 255)))

    def test_shader_export_entry_boundaries(self):
        helper = importlib.import_module('unityrs.export.ShaderConverter')
        for entry in ('export_shader', 'ConvertSerializedShader'):
            obj, budget = self.owner()
            form = N(m_Name='Synthetic', m_PropInfo=N(m_Props=[]), m_SubShaders=[],
                     m_FallbackName='', m_CustomEditorName='')
            shader = N(object_reader=obj, platforms=[], compressedLengths=[], decompressedLengths=[],
                       offsets=[], compressedBlob=b'', m_ParsedForm=form, m_SubProgramBlob=None,
                       m_Script=b'Shader "Synthetic" {}')
            with self.subTest(entry=entry):
                self.boundary(obj, budget, lambda: getattr(helper, entry)(shader),
                              lambda value: self.assertIn('Shader "Synthetic"', value))

    def test_empty_shader_conversion_retains_owner_guard_for_nested_stages(self):
        helper = importlib.import_module('unityrs.export.ShaderConverter')
        from binding_y6b_crypto_shader import property_value
        for large_name in (False, True):
            obj, budget = self.owner()
            form = N(m_Name='x' * 20_000 if large_name else 'Synthetic',
                     m_PropInfo=N(m_Props=[] if large_name else [property_value(0)] * 1000),
                     m_SubShaders=[], m_FallbackName='', m_CustomEditorName='')
            shader = N(object_reader=obj, platforms=[], compressedLengths=[], decompressedLengths=[],
                       offsets=[], compressedBlob=b'', m_ParsedForm=form)
            held = obj._native.tree_traversal()
            held.step(0, budget.limit - budget.used - 32768, 0)
            try:
                with self.subTest(large_name=large_name), self.assertRaises(MemoryError):
                    helper.ConvertSerializedShader(shader)
            finally:
                del held

    def test_codec_loader_and_save_entry_boundaries(self):
        helper = importlib.import_module('unityrs.helpers.CompressionHelper')
        # Builtins decode in the core; replacements stage on the same input
        # meter before/after invoking caller code.
        for replaced in (False, True):
            obj, budget = self.owner()
            bundle = O.B.S.fs_bundle([(b'raw', b'synthetic', 0)], codec=0)
            table = dict(helper.DECOMPRESSION_MAP)
            if replaced:
                table[0] = lambda data, size: data
            with patch.dict(helper.DECOMPRESSION_MAP, table), self.subTest(replaced=replaced):
                self.boundary(obj, budget, lambda: unityrs.Environment(bundle, budget=budget),
                              lambda env: self.assertEqual(len(env.files), 1), high=262144)
            packed = next(iter(unityrs.Environment(bundle, budget=budget).files.values()))
            for mode in (0, 2, 3):
                table = dict(helper.COMPRESSION_MAP)
                if replaced and mode:
                    original = table[mode]
                    table[mode] = lambda data, original=original: original(data)
                with patch.dict(helper.COMPRESSION_MAP, table), self.subTest(replaced=replaced, mode=mode):
                    self.boundary(obj, budget, lambda: packed.save(packer=(0x40, mode)),
                                  lambda data: self.assertTrue(data.startswith(b'UnityFS')), high=524288)
