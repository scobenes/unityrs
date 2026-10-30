"""PARITY.md rows on exports, run against the oracle: what FMOD and fmod_toolkit make of each audio codec, and a
Shader's text, the tables the converter reads, and older shaders.

Sections 14.2, 15.5, 16 and 19.1. Like `test_parity_rows.py`, every input is built here, so nothing needs the
corpus, and each test names the rows it pins; a row's test column names the test back. Objects are written from the
TPK's node for their engine version (`test_rows_objects.tpk_object`); FSB5 banks are laid out from the format's own
rules (`export_support.py`), not by UnityPy. Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import ctypes
import json
import shutil
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from export_support import element, fsb5, wav_header  # noqa: E402
from rows_support import ORACLE, load  # noqa: E402
from test_rows_objects import default_value, tpk_file, tpk_object  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import fmod_toolkit  # loads the bundled libfmod, which pyfmodex needs first
    import pyfmodex
    from pyfmodex.exceptions import FmodError
    from UnityPy.export import AudioClipConverter
    from UnityPy.helpers import Tpk
    from UnityPy.helpers.UnityVersion import UnityVersion


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class ExportRows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-exports-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    # Section 16: what FMOD and fmod_toolkit make of each codec, on banks built here.
    def test_audio_codecs(self):
        pcm = struct.pack("<8h", 1, -1, 2, -2, 3, -3, 4, -4)
        wavs = fmod_toolkit.raw_to_wav(fsb5(2, (pcm, 4), (pcm[:8], 2), channels=2), "c", 2, 44100)
        # PCM16: the samples as they are behind the 44-byte header; a second subsound is `{name}-1.wav`.
        self.assertEqual(
            wavs, {"c.wav": wav_header(2, 44100, 16, 16) + pcm, "c-1.wav": wav_header(2, 44100, 16, 8) + pcm[:8]}
        )
        # IMA ADPCM (36 bytes per 64 samples), FADPCM (0x8C per 256) and MPEG (silent 128 kbps frames, 1152
        # samples each) decode to PCM16.
        frame = bytes.fromhex("fffb9004") + bytes(413)
        for mode, data, count in ((7, bytes(72), 128), (16, bytes(0x118), 512), (11, frame * 5, 5760)):
            with self.subTest(mode=mode):
                wav = fmod_toolkit.raw_to_wav(fsb5(mode, (data, count)), "c", 1, 44100)["c.wav"]
                self.assertEqual(wav, wav_header(1, 44100, 16, 2 * count) + bytes(2 * count))
        # The bundled libfmod refuses the other codecs in a bank built here: PCM8, PCM24, PCM32, PCM float, GC
        # ADPCM, VAG, HEVAG, CELT, XWMA and Vorbis with VERSION; XMA, ATRAC9 and Opus with UNSUPPORTED.
        refused = {1: "VERSION", 3: "VERSION", 4: "VERSION", 5: "VERSION", 6: "VERSION", 8: "VERSION", 9: "VERSION"}
        refused |= {
            12: "VERSION",
            14: "VERSION",
            15: "VERSION",
            10: "UNSUPPORTED",
            13: "UNSUPPORTED",
            17: "UNSUPPORTED",
        }
        for mode, error in refused.items():
            with self.subTest(mode=mode), self.assertRaisesRegex(FmodError, f"^{error}$"):
                fmod_toolkit.raw_to_wav(fsb5(mode, (bytes(64), 32)), "c", 1, 44100)
        # Not a bank: FMOD opens an AIFF, a MOD or an MP3 as one sound with no subsounds, and fmod_toolkit exports
        # subsounds only, so nothing comes back.
        samples = struct.pack(">4h", 0, 1000, -1000, 32767)
        rate = struct.pack(">HQ", 16383 + 15, 44100 << 48)  # 44100 as an 80-bit extended float
        comm = b"COMM" + struct.pack(">IhIh", 18, 1, 4, 16) + rate
        ssnd = b"SSND" + struct.pack(">III", 8 + len(samples), 0, 0) + samples
        aiff = b"FORM" + struct.pack(">I", 4 + len(comm) + len(ssnd)) + b"AIFF" + comm + ssnd
        mod = bytes(20) + bytes(30) * 31 + bytes([1, 0]) + bytes(128) + b"M.K." + bytes(1024)
        for name, data in (("aiff", aiff), ("mod", mod), ("mp3", frame * 5)):
            with self.subTest(name):
                self.assertEqual(fmod_toolkit.raw_to_wav(data, "c", 1, 44100), {})
        # So their row is Tier U, not a gap, which section 0 keeps for a format UnityPy produces output for.
        parity = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8").splitlines()
        rows = [row.split(" | ") for row in parity if row.startswith("| tracker and legacy audio ")]
        self.assertEqual([row[3] for row in rows], ["U"])

        # fmod_toolkit's own rules, on a stand-in subsound: PCM float becomes int16 as `(v * 32768)` clipped and
        # truncated toward zero, or stays float (format 3); any other format raises TypeError, its message joining
        # a str and the enum.
        floats = struct.pack("<7f", 0.5, -0.5, 1.0, -1.0, 2.0, 0.99999, -0.0000305)
        self.assertEqual(
            struct.unpack("<7h", fmod_toolkit.fmod.convert_pcm_float_to_pcm_int16(floats)),
            (16384, -16384, 32767, -32768, 32767, 32767, 0),
        )
        buffer = ctypes.create_string_buffer(floats, len(floats))

        def subsound(fmt, bits=32):
            return SimpleNamespace(
                format=SimpleNamespace(format=fmt, channels=1, bits=bits),
                default_frequency=8000,
                get_length=lambda unit: len(floats),
                lock=lambda offset, length: [(ctypes.addressof(buffer), ctypes.c_uint(len(floats)))],
                unlock=lambda *parts: None,
            )

        float_format = pyfmodex.enums.SOUND_FORMAT.PCMFLOAT
        as_int = fmod_toolkit.fmod.subsound_to_wav(subsound(float_format))
        self.assertEqual(as_int[:44], wav_header(1, 8000, 16, 14))
        self.assertEqual(struct.unpack("<7h", as_int[44:]), (16384, -16384, 32767, -32768, 32767, 32767, 0))
        as_float = fmod_toolkit.fmod.subsound_to_wav(subsound(float_format), convert_pcm_float=False)
        self.assertEqual(as_float, wav_header(1, 8000, 32, 28, audio_format=3) + floats)
        with self.assertRaisesRegex(TypeError, "^can only concatenate str"):
            fmod_toolkit.fmod.subsound_to_wav(subsound(pyfmodex.enums.SOUND_FORMAT.BITSTREAM, 0))
        # PCM8, PCM24 and PCM32: the data as it is, behind a header of that width.
        for name, bits in (("PCM8", 8), ("PCM24", 24), ("PCM32", 32)):
            with self.subTest(name):
                got = fmod_toolkit.fmod.subsound_to_wav(subsound(getattr(pyfmodex.enums.SOUND_FORMAT, name), bits))
                self.assertEqual(got, wav_header(1, 8000, bits, 28) + floats)

        # AudioClip.samples is extract_audioclip_samples(self).
        from UnityPy.classes import AudioClip

        clip = object.__new__(AudioClip)
        with mock.patch.object(AudioClipConverter, "extract_audioclip_samples", return_value={"x": b""}) as call:
            self.assertEqual(clip.samples, {"x": b""})
        call.assert_called_once_with(clip)
        # The enums.
        from UnityPy.enums import AudioCompressionFormat, AudioType

        self.assertEqual(
            {e.name: e.value for e in AudioType},
            {
                "UNKNOWN": 0, "ACC": 1, "AIFF": 2, "IT": 10, "MOD": 12, "MPEG": 13, "OGGVORBIS": 14, "S3M": 17,
                "WAV": 20, "XM": 21, "XMA": 22, "VAG": 23, "AUDIOQUEUE": 24,
            },
        )  # fmt: skip
        self.assertEqual(
            {e.name: e.value for e in AudioCompressionFormat},
            {
                "PCM": 0, "Vorbis": 1, "ADPCM": 2, "MP3": 3, "VAG": 4, "HEVAG": 5, "XMA": 6, "AAC": 7, "GCADPCM": 8,
                "ATRAC9": 9,
            },
        )  # fmt: skip

    # Sections 14.2, 15.5, 19.1: a 2021.3 Shader's text, the tables the converter reads, and older shaders.
    def test_shader_text(self):
        import lz4.block
        from UnityPy.export import ShaderConverter as SC

        def subprogram(version, kind, code, keywords=(), local=None, endian="<"):
            out = struct.pack(f"{endian}ii", version, kind) + bytes(12) + (bytes(4) if version >= 201608170 else b"")
            for words in (keywords, *(() if local is None else (local,))):
                out += struct.pack(f"{endian}i", len(words)) + b"".join(S.s32(w, endian) for w in words)
            return out + struct.pack(f"{endian}i", len(code)) + code + bytes(-len(code) % 4)

        def program(subprograms, entry=12, endian="<"):
            head = 4 + entry * len(subprograms)
            out, table = b"", b""
            for sub in subprograms:
                table += struct.pack(f"{endian}ii", head + len(out), len(sub)) + bytes(entry - 8)
                out += sub
            return struct.pack(f"{endian}i", len(subprograms)) + table + out

        def value(v):
            return {"val": v, "name": ""}

        def prop(name, kind, defaults=(0, 0, 0, 0), attributes=(), texture=("", 2)):
            p = element(48, "m_ParsedForm", "m_PropInfo", "m_Props")
            p.update(m_Name=name, m_Description=name.strip("_"), m_Type=kind, m_Attributes=list(attributes))
            p.update({f"m_DefValue[{i}]": float(v) for i, v in enumerate(defaults)})
            return {**p, "m_DefTexture": {**p["m_DefTexture"], "m_DefaultName": texture[0], "m_TexDim": texture[1]}}

        def sub(blob, kind, tier=0):
            return {
                **element(48, "m_ParsedForm", "m_SubShaders", "m_Passes", "progVertex", "m_SubPrograms"),
                "m_BlobIndex": blob,
                "m_GpuProgramType": kind,
                "m_ShaderHardwareTier": tier,
            }

        passes = element(48, "m_ParsedForm", "m_SubShaders", "m_Passes")
        state = {
            **passes["m_State"],
            "m_Name": "P",
            "m_Tags": {"tags": [("LightMode", "Always")]},
            "gpuProgramID": 7,
            "lighting": True,
        }
        state |= {
            k: value(v)
            for k, v in dict(
                zTest=2, zWrite=0, culling=0, offsetFactor=1, offsetUnits=2, alphaToMask=1, zClip=0
            ).items()
        }
        normal = {**passes, "m_Type": 0, "m_State": state}
        normal["progVertex"] = {**normal["progVertex"], "m_SubPrograms": [sub(0, 4, 1), sub(0, 4, 2), sub(1, 15)]}
        subshader = element(48, "m_ParsedForm", "m_SubShaders")
        subshader |= {"m_LOD": 100, "m_Tags": {"tags": [("RenderType", "Opaque")]}}
        subshader["m_Passes"] = [
            normal,
            {**passes, "m_Type": 1, "m_UseName": "Other/PASS"},
            {**passes, "m_Type": 2, "m_TextureName": "_Grab"},
        ]
        form = element(48, "m_ParsedForm")
        form |= {
            "m_Name": "Test/S",
            "m_FallbackName": "Diffuse",
            "m_CustomEditorName": "Ed",
            "m_SubShaders": [subshader],
        }
        form["m_PropInfo"] = {"m_Props": [
            prop("_Color", 0, (1, 0.5, 0, 1), ["HDR"]), prop("_V", 1), prop("_F", 2, (0.25,)),
            prop("_R", 3, (0.5, 0, 2)), prop("_MainTex", 4, texture=("white", 2)), prop("_Any", 4, texture=("", 1)),
            prop("_None", 4, texture=("", 0)),
        ]}  # fmt: skip
        # Platform D3D11 then GLES3, each its own LZ4 slice; a third platform has no lengths and is never read.
        glsl = subprogram(201806140, 4, b"void main(){}", ["A", "B"], ["L"])
        programs = [
            program([subprogram(202012090, 0, b"x"), subprogram(202012090, 15, b"DXBC")]),
            program([glsl, glsl]),
        ]
        packed = [lz4.block.compress(p, store_size=False) for p in programs]
        shader = tpk_object(
            48,
            m_Name="Test/S",
            m_ParsedForm=form,
            platforms=[4, 9, 18],
            offsets=[[0], [len(packed[0])]],
            compressedLengths=[[len(p)] for p in packed],
            decompressedLengths=[[len(p)] for p in programs],
            compressedBlob=list(b"".join(packed)),
        )
        parsed = load(tpk_file([(1, 48, shader)]))[0].objects[1].parse_as_object()
        props = [
            '[HDR] _Color ("Color", Color) = (1,0.5,0,1)\n', '_V ("V", Vector) = (0,0,0,0)\n',
            '_F ("F", Float) = 0.25\n', '_R ("R", Range(0, 2)) = 0.5\n', '_MainTex ("MainTex", 2D) = "white" { }\n',
            '_Any ("Any", any) = "" { }\n', '_None ("None", ) = "" { }\n',
        ]  # fmt: skip
        glsl_text = 'Keywords { "A" "B" }\nLocal Keywords { "L" }\n"void main(){}"'
        expected = (
            SC.HEADER + 'Shader "Test/S" {\n' + "\n".join(["Properties {\n", *props, "}\n"])
            + 'SubShader {\n LOD 100\n Tags { "RenderType" = "Opaque" }\n'
            + ' Pass {\n Name "P"\n  Tags { "LightMode" = "Always" }\n AlphaToMask On\n ZClip Off\n ZTest Less\n'
            + " ZWrite Off\n Cull Off\n Offset 1.0, 2.0\n  Lighting On\n  GpuProgramID 7\n"
            + 'Program "vp" {\n'
            + f'SubProgram "gles3 hw_tier01 " {{\n{glsl_text}\n}}\nSubProgram "gles3 hw_tier02 " {{\n{glsl_text}\n}}\n'
            + 'SubProgram "d3d11 " {\n"// shader disassembly not supported on DXBC"\n}\n}\n}\n'
            + ' UsePass "Other/PASS"\n GrabPass {\n "_Grab"\n}\n}\n'
            + 'Fall back "Diffuse"\nCustomEditor "Ed"\n}'
        )  # fmt: skip
        self.assertEqual(parsed.export(), expected)
        with mock.patch.object(SC, "export_shader", return_value="t") as export:
            self.assertEqual(parsed.export(), "t")
        export.assert_called_once_with(parsed)

        # From 2021.3.10, 2022.1.13 and 2022.2 (the TPK's first versions with the field) a pass's programs are in
        # m_PlayerSubPrograms, a list per keyword set; ShaderConverter reads only m_SubPrograms, so a shader whose
        # programs are all there exports its state and no Program block.
        def progs(version):
            return set(element(48, "m_ParsedForm", "m_SubShaders", "m_Passes", "progVertex", version=version))

        for version, has in (("2021.3.9f1", False), ("2021.3.10f1", True), ("2022.1.12f1", False)):
            self.assertEqual("m_PlayerSubPrograms" in progs(version), has, version)
        for version in ("2022.1.13f1", "2022.2.0f1", "2022.3.0f1", "6000.0.0f1"):
            self.assertIn("m_PlayerSubPrograms", progs(version), version)
        late = "2022.3.0f1"
        late_node = Tpk.get_typetree_node(48, UnityVersion.from_str(late))
        player_sub = default_value(next(n for n in late_node.traverse() if n.m_Type == "SerializedPlayerSubProgram"))
        player_sub |= {"m_BlobIndex": 0, "m_GpuProgramType": 4}
        late_program = program([subprogram(202012090, 4, b"late")])
        late_packed = lz4.block.compress(late_program, store_size=False)

        def late_file(field):
            pass_ = element(48, "m_ParsedForm", "m_SubShaders", "m_Passes", version=late)
            vertex = dict(pass_["progVertex"])
            if field == "m_SubPrograms":
                sub_ = element(48, "m_ParsedForm", "m_SubShaders", "m_Passes", "progVertex", field, version=late)
                vertex[field] = [{**sub_, "m_BlobIndex": 0, "m_GpuProgramType": 4}]
            else:
                vertex[field] = [[player_sub]]
            pass_ |= {"m_Type": 0, "progVertex": vertex}
            subshader_ = {**element(48, "m_ParsedForm", "m_SubShaders", version=late), "m_Passes": [pass_]}
            form_ = {**element(48, "m_ParsedForm", version=late), "m_Name": "Late/S", "m_SubShaders": [subshader_]}
            data = tpk_object(
                48,
                late,
                m_Name="Late/S",
                m_ParsedForm=form_,
                platforms=[9],
                offsets=[[0]],
                compressedLengths=[[len(late_packed)]],
                decompressedLengths=[[len(late_program)]],
                compressedBlob=list(late_packed),
            )
            return tpk_file([(1, 48, data)], unity_version=late.encode())

        def late_text(field):
            return load(late_file(field))[0].objects[1].parse_as_object().export()

        self.assertIn('Program "vp" {\nSubProgram "gles3 " {\n"late"\n}\n}\n', late_text("m_SubPrograms"))
        player_text = late_text("m_PlayerSubPrograms")
        self.assertIn(" Pass {\n", player_text)
        self.assertNotIn('Program "', player_text)
        self.assertNotIn("SubProgram", player_text)
        # shader_survey counts both kinds of entry, and the texts with and without a Program block (section 21).
        import contextlib
        import io

        import shader_survey

        for field in ("m_SubPrograms", "m_PlayerSubPrograms"):
            (self.tmp / field).mkdir()
            (self.tmp / field / "late.assets").write_bytes(late_file(field))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                shader_survey.main([str(self.tmp / field)])
            survey = json.loads(out.getvalue())
            with self.subTest(field):
                self.assertEqual(
                    survey["pass_sub_program_entries"], {"m_SubPrograms": 0, "m_PlayerSubPrograms": 0} | {field: 1}
                )
                kind = "text with a Program block" if field == "m_SubPrograms" else "text with no Program block"
                self.assertEqual((survey["shaders"], survey["sub_programs"], survey["export"]), (1, 1, {kind: 1}))

        # The state's lines: each only when its value is not the default; an unknown ZTest or Cull value prints
        # the keyword alone; Offset prints each value with str() of the float32 widened exactly.
        def state_text(**fields):
            plain = {k: v for k, v in fields.items() if k in ("m_Name", "m_LOD", "lighting", "gpuProgramID")}
            wrapped = {k: value(v) for k, v in fields.items() if k not in plain}
            pass_ = {**passes, "m_Type": 0, "m_State": {**passes["m_State"], **plain, **wrapped}}
            form_ = {**element(48, "m_ParsedForm"), "m_SubShaders": [{**element(48, "m_ParsedForm", "m_SubShaders")}]}
            form_["m_SubShaders"][0]["m_Passes"] = [pass_]
            data = tpk_object(48, m_Name="S", m_ParsedForm=form_, compressedBlob=[0])  # a blob, and no platform
            text = load(tpk_file([(1, 48, data)]))[0].objects[1].parse_as_object().export()
            return text[text.index(" Pass {\n") + len(" Pass {\n") : text.index("  GpuProgramID")]

        defaults = dict(zTest=4, zWrite=1, culling=2, offsetFactor=0, offsetUnits=0, alphaToMask=0, zClip=1)
        self.assertEqual(state_text(**defaults), "")
        self.assertEqual(state_text(**defaults, m_Name="N", m_LOD=200), ' Name "N"\n  LOD 200\n')
        self.assertEqual(state_text(**defaults | {"zTest": 9, "culling": 3}), " ZTest \n Cull \n")
        names = {0: "Off", 1: "Never", 2: "Less", 3: "Equal", 5: "Greater", 6: "NotEqual", 7: "GEqual", 8: "Always"}
        for z, name in names.items():
            self.assertEqual(state_text(**defaults | {"zTest": z}), f" ZTest {name}\n")
        self.assertEqual([state_text(**defaults | {"culling": c}) for c in (0, 1)], [" Cull Off\n", " Cull Front\n"])
        self.assertEqual(state_text(**defaults | {"offsetUnits": 0.1}), " Offset 0.0, 0.10000000149011612\n")
        self.assertEqual(state_text(**defaults | {"offsetFactor": -1}), " Offset -1.0, 0.0\n")
        self.assertEqual(
            state_text(**defaults | {"alphaToMask": 2, "zClip": 0, "zWrite": 2}, lighting=True),
            " AlphaToMask On\n ZClip Off\n ZWrite Off\n  Lighting On\n",
        )
        # A property of type 5 (Int, from 2021.1) raises once its name is written.
        int_prop = SimpleNamespace(m_Attributes=[], m_Name="_I", m_Description="I", m_Type=5)
        with self.assertRaisesRegex(ValueError, "^5$"):
            SC.ConvertSerializedProperty(int_prop)

        # ShaderSubProgram: each program type's text; Metal skips its entry name, behind the 0xF00DCAFE header or,
        # without it, after dropping four bytes; an unknown type raises.
        def exported(kind, code, version=202012090):
            return SC.ShaderSubProgram(EndianBinaryReader(subprogram(version, kind, code), endian="<")).Export()

        from UnityPy.streams import EndianBinaryReader

        texts = {kind: exported(kind, b"src") for kind in range(32) if kind not in (23, 24)}
        self.assertEqual({k for k, t in texts.items() if t == '"src"'}, {1, 2, 3, 4, 5, 6, 7, 8, 26, 27, 28, 29, 30})
        self.assertEqual(
            {k for k, t in texts.items() if t == '"// shader disassembly not supported on DXBC"'}, set(range(9, 23))
        )
        self.assertEqual(
            (texts[25], texts[0], texts[31]),
            ('""', '"//shader disassembly not supported on 0"', '"//shader disassembly not supported on 31"'),
        )
        self.assertEqual(exported(23, struct.pack("<II", 0xF00DCAFE, 8) + b"main\0metal"), '"metal"')
        self.assertEqual(exported(24, b"1234main\0metal"), '"metal"')
        with self.assertRaises(ValueError):
            exported(32, b"")
        # The text kinds decode strictly: code that is not UTF-8 raises UnicodeDecodeError, Metal's after its entry
        # name (section 19.2). Code of length 0 prints "" for every type.
        for kind in (1, 4, 8, 26, 30):
            with self.subTest(kind=kind), self.assertRaises(UnicodeDecodeError):
                exported(kind, b"\x80\xff\x00\x01")
        for kind, code in ((23, struct.pack("<II", 0xF00DCAFE, 8) + b"main\0\x80"), (24, b"1234main\0\x80\xff")):
            with self.subTest(kind=kind), self.assertRaises(UnicodeDecodeError):
                exported(kind, code)
        self.assertEqual({exported(kind, b"") for kind in range(32)}, {'""'})
        # Metal code raises before that: struct.error when shorter than its tag (or than 8 bytes with the header),
        # and the memory reader's plain Exception when no NUL follows the tag or the header's offset.
        cafe = struct.pack("<I", 0xF00DCAFE)
        for kind in (23, 24):
            for code, error, message in (
                (b"\x01\x02\x03", struct.error, "at least 4 bytes"),
                (cafe + b"\x01", struct.error, "at least 8 bytes"),
                (b"ABCD", Exception, "^String not terminated$"),
                (b"ABCDentry-without-nul code", Exception, "^String not terminated$"),
                (cafe + struct.pack("<i", 100) + b"main\0x", Exception, "^String not terminated$"),
            ):
                with self.subTest(kind=kind, code=code), self.assertRaisesRegex(error, message) as caught:
                    exported(kind, code)
                self.assertIs(type(caught.exception), error)
            self.assertEqual(exported(kind, b"ABCDmain\0hello"), '"hello"')
            # The reader looks for the NUL only within 32767 bytes of where the name starts (section 13): with none
            # there it raises only when the code ends within that window; past it, the 32767 bytes are the name
            # and the rest is the text.
            for start in (b"ABCD", cafe + struct.pack("<i", 8)):
                with self.subTest(kind=kind, start=start[:4]):
                    with self.assertRaisesRegex(Exception, "^String not terminated$"):
                        exported(kind, start + b"n" * 32767)
                    self.assertEqual(exported(kind, start + b"n" * 32767 + b"x"), '"x"')
                    self.assertEqual(exported(kind, start + b"n" * 32767 + b"x" * 7233), '"' + "x" * 7233 + '"')

            # The header's offset P is never checked: the NUL is looked for in code[max(P, 0):max(P + 32767, 0)],
            # and with none there the name is code[P:P + 32767] and the text code[P + 32767:], Python slices. With
            # the NUL at 12: -8 and -32754 search from the code's start and find it; -32755 stops short of it, so
            # the text starts at it; -32767 searches nothing and leaves the whole code, header included, which is
            # not UTF-8, as -40000 does; -32768 leaves the last byte.
            def at(offset, kind=kind):
                return exported(kind, cafe + struct.pack("<i", offset) + b"main\0body")

            self.assertEqual((at(-1), at(-8), at(-32754)), ('"body"',) * 3)
            self.assertEqual((at(-32755), at(-32768)), ('"\0body"', '"y"'))
            for offset in (-32767, -40000):
                with self.assertRaisesRegex(UnicodeDecodeError, "byte 0xfe in position 0"):
                    at(offset)
        # Local keywords are read only for versions in [201806140, 202012090); the reserved word from 201608170.
        for version, local in ((201806140, ["L"]), (202012089, ["L"]), (202012090, None), (201802150, None)):
            reader = EndianBinaryReader(subprogram(version, 4, b"c", ["K"], local) + b"\xff" * 4, endian="<")
            got = SC.ShaderSubProgram(reader)
            self.assertEqual((got.m_Keywords, got.m_LocalKeywords, bytes(got.m_ProgramCode)), (["K"], local, b"c"))
        got = SC.ShaderSubProgram(EndianBinaryReader(subprogram(201510240, 4, b"c", ["K"]), endian="<"))
        self.assertEqual((got.m_Keywords, bytes(got.m_ProgramCode)), (["K"], b"c"))
        # ShaderProgram: 12-byte table entries from 2019.3, 8 before.
        for version, entry in (((2019, 3, 0, 1), 12), ((2019, 2, 9, 1), 8)):
            got = SC.ShaderProgram(EndianBinaryReader(program([glsl, glsl], entry), endian="<"), version)
            self.assertEqual([s.m_Keywords for s in got.m_SubPrograms], [["A", "B"]] * 2)

        # The tables: the program types each platform can use; NaCl, Flash, PSM and every platform the table lacks
        # raise; the platform names.
        usable = {}
        for platform in range(-1, 22):
            try:
                usable[platform] = [t for t in range(32) if SC.CheckGpuProgramUsable(platform, t)]
            except NotImplementedError:
                usable[platform] = None
        consoles = [26, 27, 28, 29, 30]
        self.assertEqual(usable, {
            -1: None, 0: [1], 1: [9, 10, 11, 12], 2: consoles, 3: None, 4: list(range(15, 23)), 5: [5], 6: None,
            7: None, 8: [13, 14], 9: [2, 3, 4], 10: consoles, 11: consoles, 12: consoles, 13: None, 14: [23],
            15: [6, 7, 8], 16: consoles, 17: consoles, 18: [25], 19: consoles, 20: consoles, 21: None,
        })  # fmt: skip
        names = "openGL d3d9 xbox360 ps3 d3d11 gles glesdesktop flash d3d11_9x gles3 psp2 ps4 xboxone psm metal glcore"
        names += " n3ds wiiu vulkan switch xboxone+_d3d12"
        self.assertEqual([SC.GetPlatformString(p) for p in range(-1, 22)], ["unknown", *names.split(), "unknown"])
        from UnityPy.enums import (
            PassType,
            SerializedPropertyType,
            ShaderCompilerPlatform,
            ShaderGpuProgramType,
            TextureDimension,
        )

        self.assertEqual([e.value for e in ShaderCompilerPlatform], list(range(-1, 21)))
        self.assertEqual([e.value for e in ShaderGpuProgramType], list(range(32)))
        self.assertEqual([e.name for e in PassType], ["kPassTypeNormal", "kPassTypeUse", "kPassTypeGrab"])
        self.assertEqual(
            [e.name for e in SerializedPropertyType], ["kColor", "kVector", "kFloat", "kRange", "kTexture"]
        )
        self.assertEqual([e.value for e in TextureDimension], [-1, 0, 1, 2, 3, 4, 5, 6, 2147483647])

        # Before 5.5: m_Script is a string, and `bytes(m_Script)` raises TypeError on the blob path (5.3-5.4) and
        # on the plain path alike.
        blob = program([subprogram(201510240, 4, b"code", endian=">")], 8, endian=">")
        for version, fields in (
            (
                "5.4.6f3",
                {"m_SubProgramBlob": list(lz4.block.compress(blob, store_size=False)), "decompressedSize": len(blob)},
            ),
            ("5.4.6f3", {}),
            ("4.7.2f1", {}),
        ):
            data = tpk_object(48, version, m_Name="old", m_Script="Shader { GpuProgramIndex 0 }", **fields)
            old = load(tpk_file([(1, 48, data)], unity_version=version.encode()))[0].objects[1].parse_as_object()
            with (
                self.subTest(version=version, blob=bool(fields)),
                self.assertRaisesRegex(TypeError, "without an encoding"),
            ):
                old.export()
        # The blob is read before m_Script is touched: one laid out little-endian is read with a byte-swapped count
        # (256 reads as 65536) and offsets, and raises struct.error instead.
        little = program([subprogram(201510240, 4, b"code")] * 256, 8)
        self.assertEqual(struct.unpack(">i", little[:4]), (65536,))
        packed_little = list(lz4.block.compress(little, store_size=False))
        data = tpk_object(
            48, "5.4.6f3", m_Name="old", m_Script="x", m_SubProgramBlob=packed_little, decompressedSize=len(little)
        )
        old = load(tpk_file([(1, 48, data)], unity_version=b"5.4.6f3"))[0].objects[1].parse_as_object()
        with self.assertRaises(struct.error):
            old.export()
        # The table of that count is allocated first, a list of that many slots: a low byte of 1 asks for 16777216
        # and reaches the struct.error, one of 0x7F for 2130706432, which raises MemoryError where the process
        # cannot allocate them (here, a subprocess allowed 1 GiB more than it holds after importing UnityPy).
        import subprocess

        limited = (
            "import resource, sys\n"
            "from UnityPy.export import ShaderConverter as SC\n"
            "from UnityPy.streams import EndianBinaryReader\n"
            "held = int(open('/proc/self/statm').read().split()[0]) * resource.getpagesize()\n"
            "resource.setrlimit(resource.RLIMIT_AS, (held + (1 << 30), resource.getrlimit(resource.RLIMIT_AS)[1]))\n"
            "try:\n"
            "    SC.ShaderProgram(EndianBinaryReader(sys.stdin.buffer.read()), (5, 4, 6, 1))\n"
            "except Exception as error:\n"
            "    print(type(error).__name__)\n"
        )
        for count, slots, error in ((1, 16777216, "error"), (127, 2130706432, "MemoryError")):
            little = program([subprogram(201510240, 4, b"code")] * count, 8)
            self.assertEqual(struct.unpack(">i", little[:4]), (slots,))
            run = subprocess.run([sys.executable, "-c", limited], input=little, capture_output=True, check=True)
            self.assertEqual(run.stdout.decode().strip(), error, f"count {count}")
        # A count whose low byte is 0x80 or more reads big-endian as negative (128 as -2147483648): the table is
        # empty, no entry is read, and export raises the TypeError of a blob that parses. Export has no sub-program
        # for any `GpuProgramIndex N` line, and raises IndexError.
        for count in (128, 200, 255):
            little = program([subprogram(201510240, 4, b"code")] * count, 8)
            self.assertLess(struct.unpack(">i", little[:4])[0], 0)
            packed_little = list(lz4.block.compress(little, store_size=False))
            data = tpk_object(
                48, "5.4.6f3", m_Name="old", m_Script="x", m_SubProgramBlob=packed_little, decompressedSize=len(little)
            )
            old = load(tpk_file([(1, 48, data)], unity_version=b"5.4.6f3"))[0].objects[1].parse_as_object()
            with self.subTest(count=count), self.assertRaisesRegex(TypeError, "without an encoding"):
                old.export()
            got = SC.ShaderProgram(EndianBinaryReader(little), (5, 4, 6, 1))
            self.assertEqual(got.m_SubPrograms, [])
            with self.assertRaisesRegex(IndexError, "^list index out of range$"):
                got.Export("GpuProgramIndex 0")
        # From 5.5 a Shader has no m_Script, so one with an empty compressedBlob takes the plain path and raises on
        # bytes(None): the plain path returns text for no version.
        for version in ("5.5.6f1", "2021.3.2f1"):
            self.assertNotIn("m_Script", element(48, version=version))
            data = tpk_object(48, version, m_Name="empty")
            empty = load(tpk_file([(1, 48, data)], unity_version=version.encode()))[0].objects[1].parse_as_object()
            with (
                self.subTest(version),
                self.assertRaisesRegex(TypeError, "^cannot convert 'NoneType' object to bytes$"),
            ):
                empty.export()
        # The 5.4 blob itself reads big-endian and fills `GpuProgramIndex N`, N being int() of the rest of the line:
        # Python's int grammar, whitespace around the number allowed (a trailing \r, so a CRLF line fills), a sign,
        # `_` between digits and any script's digits; anything else after the number raises ValueError.
        got = SC.ShaderProgram(EndianBinaryReader(blob), (5, 4, 6, 1))
        self.assertEqual(got.Export("Shader {\nGpuProgramIndex 0\n}"), 'Shader {\n"code"\n}')
        self.assertEqual(got.Export("Shader {\r\nGpuProgramIndex 0\r\n}"), 'Shader {\r\n"code"\n}')
        # int() in base 10, not Python's integer-literal grammar: a sign and digits of any script fill, which no
        # literal is, and a base prefix or a float is refused, which literals are.
        for rest in ("0 ", "0\t", " 0", "\xa00\xa0", "+0", "0_0", "٠", "-1", "00", " 0 "):
            with self.subTest(rest=rest):
                self.assertEqual(got.Export(f"GpuProgramIndex {rest}\nX"), '"code"\nX')
        for rest in ("0 }", "0\rX", "0x0", "0b0", "0o0", "0.0", "0e0", "0j", "1_", "_1", "zero"):
            with self.subTest(rest=rest), self.assertRaisesRegex(ValueError, "^invalid literal for int"):
                got.Export(f"GpuProgramIndex {rest}")
        # The whitespace int() takes: ASCII space, \t, \v, \f and \r, and every non-ASCII character str.isspace()
        # takes, but not \x1c to \x1f, which str.isspace() takes too. At most 4300 digits, the `_` not counted
        # (sys.get_int_max_str_digits()'s default).
        spaces = [chr(c) for c in range(0x110000) if chr(c).isspace() and chr(c) != "\n"]
        for space in spaces:
            with self.subTest(space=hex(ord(space))):
                if "\x1c" <= space <= "\x1f":
                    with self.assertRaisesRegex(ValueError, "^invalid literal for int"):
                        got.Export(f"GpuProgramIndex {space}0{space}\nX")
                else:
                    self.assertEqual(got.Export(f"GpuProgramIndex {space}0{space}\nX"), '"code"\nX')
        self.assertEqual(sys.get_int_max_str_digits(), 4300)
        for rest in ("0" * 4300, "_".join("0" * 4300), "-" + "0" * 4300, " " + "0" * 4300 + " "):
            with self.subTest(digits=len(rest)):
                self.assertEqual(got.Export(f"GpuProgramIndex {rest}"), '"code"')
        limit = (
            "^Exceeds the limit \\(4300 digits\\) for integer string conversion: value has 4301 digits; "
            "use sys.set_int_max_str_digits\\(\\) to increase the limit$"
        )
        for rest in ("0" * 4301, "_".join("0" * 4301)):
            with self.subTest(digits=len(rest)), self.assertRaisesRegex(ValueError, limit):
                got.Export(f"GpuProgramIndex {rest}")
        self.assertEqual(got.Export("GpuProgramIndex\t0"), "GpuProgramIndex\t0", "the regex wants one space")
        # N indexes the table as a list: outside [-len, len) raises IndexError, a negative N inside counts from the
        # end.
        for n in (1, -2):
            with self.subTest(n=n), self.assertRaisesRegex(IndexError, "^list index out of range$"):
                got.Export(f"GpuProgramIndex {n}")
        two = SC.ShaderProgram(
            EndianBinaryReader(
                program(
                    [subprogram(201510240, 4, b"one", endian=">"), subprogram(201510240, 4, b"two", endian=">")],
                    8,
                    endian=">",
                )
            ),
            (5, 4, 6, 1),
        )
        self.assertEqual(
            [two.Export(f"GpuProgramIndex {n}") for n in (-2, -1, 0, 1)], ['"one"', '"two"', '"one"', '"two"']
        )
        # Each entry's offset sets the memory reader's Position unchecked: a negative one counts from the blob's
        # end, and a field that straddles index 0 reads b"" (a fixed-width one raises struct.error). A negative
        # keyword count reads no keyword; a code length of -1 reads the rest of the blob, another negative one the
        # slice section 13 gives (b"" here). The export of each still raises the TypeError of a blob that parses.
        sub = subprogram(201510240, 4, b"abcd", ["K"], endian=">")

        def entry(offset, rest=sub):
            return SC.ShaderProgram(EndianBinaryReader(struct.pack(">iii", 1, offset, len(rest)) + rest), (5, 4, 6, 1))

        self.assertEqual(entry(12).Export("GpuProgramIndex 0"), 'Keywords { "K" }\n"abcd"')
        trailing = sub + b"TAIL"
        self.assertEqual(entry(-len(trailing), trailing).Export("GpuProgramIndex 0"), 'Keywords { "K" }\n"abcd"')
        at_end = entry(-len(sub)).m_SubPrograms[0]  # the code's 4 bytes would run from -4 to index 0
        self.assertEqual((at_end.m_Keywords, bytes(at_end.m_ProgramCode)), (["K"], b""))
        for offset, message in ((-2, "^not enough data to unpack 4 bytes at offset -2$"), (-1000, "out of range")):
            with self.subTest(offset=offset), self.assertRaisesRegex(struct.error, message):
                entry(offset)
        # A sub-program whose fixed-width fields run past the blob's end raises struct.error, its offset inside the
        # blob or not: the version at len - 2, a blob cut after the program type or inside the keyword count, and a
        # keyword's or local keyword's length (a keyword's bytes past the end read "", `read_aligned_string`). One
        # whose program type ShaderGpuProgramType lacks raises ValueError.
        local = subprogram(201806140, 4, b"abcd", ["K"], local=["L"], endian=">")
        self.assertEqual(len(local), 56)  # 24 of header, count 1 and "K" to 36, count 1 and "L" to 48, code
        for offset, rest in (
            (12 + len(sub) - 2, sub),
            (12, sub[:8]),
            (12, sub[:22]),
        ):
            with self.subTest(offset=offset, rest=len(rest)), self.assertRaisesRegex(struct.error, "^unpack_from"):
                entry(offset, rest)
        # The length read itself fails, at the length's own offset (the blob starts 12 bytes into the program).
        for rest, at in (
            (sub[:24], 24),  # a keyword count of 1, and no length after it
            (sub[:26], 24),  # half of the keyword's length
            (local[:40], 40),  # a local keyword count of 1, and no length after it
            (local[:42], 40),
        ):
            size, at = 12 + len(rest), 12 + at
            message = (
                f"^unpack_from requires a buffer of at least {at + 4} bytes for unpacking 4 bytes at offset {at} "
                f"\\(actual buffer size is {size}\\)$"
            )
            with self.subTest(rest=len(rest)), self.assertRaisesRegex(struct.error, message):
                entry(12, rest)
        for kind in (32, 9999, -1):
            with (
                self.subTest(kind=kind),
                self.assertRaisesRegex(ValueError, f"^{kind} is not a valid ShaderGpuProgramType$"),
            ):
                entry(12, subprogram(201510240, kind, b"abcd", ["K"], endian=">"))

        def raw(keywords, code_length):
            return (
                struct.pack(">ii", 201510240, 4) + bytes(12) + struct.pack(">ii", keywords, code_length) + b"abcdTAIL"
            )

        for keywords, code_length, code in ((-1, 4, b"abcd"), (-5, 4, b"abcd"), (0, -1, b"abcdTAIL"), (0, -2, b"")):
            with self.subTest(keywords=keywords, code_length=code_length):
                read = entry(12, raw(keywords, code_length)).m_SubPrograms[0]
                self.assertEqual((read.m_Keywords, bytes(read.m_ProgramCode)), ([], code))
        negative = program([raw(-1, -1)], 8, endian=">")
        data = tpk_object(
            48,
            "5.4.6f3",
            m_Name="old",
            m_Script="GpuProgramIndex 0",
            m_SubProgramBlob=list(lz4.block.compress(negative, store_size=False)),
            decompressedSize=len(negative),
        )
        old = load(tpk_file([(1, 48, data)], unity_version=b"5.4.6f3"))[0].objects[1].parse_as_object()
        with self.assertRaisesRegex(TypeError, "without an encoding"):
            old.export()
        # Export raises the sub-program's own error before that TypeError: struct.error for fields past the blob's end,
        # ValueError for a program type the enum lacks.
        for subs, error in (([sub[:8]], struct.error), ([subprogram(201510240, 40, b"x", endian=">")], ValueError)):
            blob_ = program(subs, 8, endian=">")
            data = tpk_object(
                48,
                "5.4.6f3",
                m_Name="old",
                m_Script="GpuProgramIndex 0",
                m_SubProgramBlob=list(lz4.block.compress(blob_, store_size=False)),
                decompressedSize=len(blob_),
            )
            old = load(tpk_file([(1, 48, data)], unity_version=b"5.4.6f3"))[0].objects[1].parse_as_object()
            with self.subTest(error=error.__name__), self.assertRaises(error):
                old.export()
