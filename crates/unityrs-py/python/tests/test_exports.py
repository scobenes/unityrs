"""Y4 wheel ownership, optional Pillow, and current object save smoke tests."""
import importlib
import struct
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
import child
import synth as S
import unityrs


class ExportAPI(unittest.TestCase):
    def test_pillow_is_optional_until_image_access(self):
        script = """
import sys
class NoPillow:
    def find_spec(self, name, *args):
        if name == 'PIL' or name.startswith('PIL.'):
            raise ModuleNotFoundError("No module named 'PIL'", name='PIL')
sys.meta_path.insert(0, NoPillow())
import unityrs
from unityrs.classes import Texture2D
value = object.__new__(Texture2D)
try:
    value.image
except ModuleNotFoundError as error:
    assert error.name == 'PIL', error
    assert str(error) == "No module named 'PIL'", error
else:
    raise AssertionError('image access needs Pillow')
"""
        result = child.run([sys.executable, "-c", script], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_texture_edit_is_saved_only_when_requested(self):
        from PIL import Image
        from binding_y4_images import texture
        tree = S.Node("Texture2D", "Base", [S.Node("int", "m_Width"),
            S.Node("int", "m_Height"), S.Node("int", "m_TextureFormat"),
            S.Node("int", "m_CompleteImageSize"), S.Node("TypelessData", "image data")])
        original = struct.pack("<5i", 1, 2, 4, 8, 8) + bytes(range(1, 9))
        file = unityrs.Environment().load_file(S.serialized_file(
            [S.Type(28, tree)], [S.Obj(1, 0, original)]), name="texture.assets")
        reader = file.objects[1]
        value = texture(unityrs, object_reader=reader)
        value.set_image(Image.new("RGBA", (2, 2), (11, 22, 33, 44)))
        self.assertFalse(file.is_changed)
        self.assertIsNone(value.save())
        self.assertTrue(file.is_changed)
        self.assertEqual(reader.get_raw_data(), original)
        reloaded = unityrs.Environment(file.save()).objects[0].parse_as_dict()
        self.assertEqual((reloaded["m_Width"], reloaded["m_Height"]), (2, 2))
        self.assertEqual(reloaded["image data"], bytes((11, 22, 33, 44)) * 4)

    def test_detached_class_save_requires_reader(self):
        Object = importlib.import_module("unityrs.classes.Object").Object
        with self.assertRaisesRegex(ValueError, "ObjectReader not set"):
            Object().save()

    def test_audio_passthrough_owns_current_bytes(self):
        from unityrs.classes import AudioClip
        data = b"OggSsynthetic"
        clip = AudioClip(m_Name="first", m_AudioData=data)
        first = clip.samples
        clip.m_Name, clip.m_AudioData = "second", b"RIFFnext"
        self.assertEqual(first, {"first.ogg": data})
        self.assertEqual(clip.samples, {"second.wav": b"RIFFnext"})

    def test_shader_exports_current_script(self):
        from unityrs.classes import Shader
        shader = Shader(m_Name="synthetic", m_Script=b"first")
        original = shader.export()
        shader.m_Script = list(b"second")
        self.assertTrue(original.endswith("first"))
        self.assertTrue(shader.export().endswith("second"))


if __name__ == "__main__":
    unittest.main()
