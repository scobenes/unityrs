"""Installed-wheel class export behavior against the pinned UnityPy oracle."""
import unittest
import json
import struct
from pathlib import Path

import UnityPy
import unityrs

import binding_y4_exports as observations


class Exports(unittest.TestCase):
    def compare(self, observer):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        self.assertEqual(observer(unityrs), observer(UnityPy))

    def test_audio_current_attributes(self):
        self.assertEqual(observations.observe_audio(unityrs), observations.observe_audio(UnityPy, corrected=True))

    def test_shader_current_attributes_and_exceptions(self):
        self.compare(observations.observe_shader)

    def test_mesh_current_attributes_and_exceptions(self):
        self.compare(observations.observe_mesh)

    def test_only_shipped_class_export_methods_exist(self):
        self.compare(observations.observe_surface)

    def test_renderer_missing_reader_exceptions(self):
        self.compare(observations.observe_renderer_errors)

    def test_external_audio_resource_and_inline_precedence(self):
        self.assertEqual(observations.observe_audio_resources(unityrs),
                         observations.observe_audio_resources(UnityPy, corrected=True))

    def test_renderer_current_mesh_and_empty_directory(self):
        self.compare(observations.observe_renderer)

    def test_existing_synthetic_shader_cases_through_class_api(self):
        self.compare(observations.observe_shader_cases)

    def test_audio_pcm16_ima_fadpcm_and_multiple_subsounds(self):
        self.compare(observations.observe_audio_pcm)

    def _vorbis_case(self):
        fixture = Path(__file__).resolve().parents[3] / "crates/unityrs-audio/tests/fixtures/vorbis.json"
        case = json.loads(fixture.read_text())["cases"][0]
        from unityrs.classes import AudioClip
        return case, AudioClip(m_Name="tone", m_AudioData=bytes.fromhex(case["bank"]))

    def _set_headers(self, value):
        import unityrs.config as config
        sentinel = object()
        old = getattr(config, "AUDIO_VORBIS_HEADERS", sentinel)
        def restore():
            if old is sentinel:
                del config.AUDIO_VORBIS_HEADERS
            else:
                config.AUDIO_VORBIS_HEADERS = old
        self.addCleanup(restore)
        config.AUDIO_VORBIS_HEADERS = value

    def test_vorbis_headers_are_explicit_and_malformed_tables_are_rejected(self):
        _, clip = self._vorbis_case()
        self._set_headers(None)
        with self.assertRaisesRegex(NotImplementedError, "setup headers"):
            _ = clip.samples
        self._set_headers(b"bad")
        with self.assertRaisesRegex(ValueError, "Vorbis table"):
            _ = clip.samples

    def test_vorbis_uses_independently_encoded_runtime_setup_bytes_or_path(self):
        from scratch import for_case
        import unityrs.config as config
        case, clip = self._vorbis_case()
        setup = bytes.fromhex(case["setup"])
        headers = b"VST1" + struct.pack("<III", 1, case["crc"], len(setup)) + setup
        self._set_headers(headers)
        output = clip.samples["tone.wav"]
        reference = bytes.fromhex(case["independent_fmod_order_pcm16"])
        actual = output[44:]
        self.assertEqual(len(actual), len(reference))
        expected = [sample[0] for sample in struct.iter_unpack("<h", reference)]
        decoded = [sample[0] for sample in struct.iter_unpack("<h", actual)]
        self.assertLessEqual(max(abs(a-b) for a,b in zip(expected, decoded)), 2)
        path = for_case(self) / "own-setup.vst"
        path.write_bytes(headers)
        config.AUDIO_VORBIS_HEADERS = path
        self.assertEqual(clip.samples["tone.wav"], output)

    def test_python_byte_coercion_refuses_large_integer_before_allocation(self):
        from binding_cases import cases
        env = unityrs.Environment(budget=unityrs.Budget(1_000_000))
        env.load_file(next(data for name, data in cases() if name == "text"), name="owner.assets")
        from unityrs.classes import AudioClip, Shader
        audio = AudioClip(m_Name="huge", m_AudioData=2_000_000)
        shader = Shader(m_Name="huge", m_Script=2_000_000)
        audio.object_reader = shader.object_reader = env.objects[0]
        with self.assertRaises(MemoryError):
            _ = audio.samples
        with self.assertRaises(MemoryError):
            shader.export()
