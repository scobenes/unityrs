"""Complete synthetic URP material and Android texture fixture runs through saved inspect and show documents."""

import hashlib
import json
import struct
import sys
import unittest
import zlib
from pathlib import Path

from anim_fixture_support import (
    ASSETS,
    STATIC,
    archive,
    game_object,
    mesh_parts,
    pointer,
    stamp,
    static_bundles,
    write,
)

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import child  # noqa: E402
import scratch  # noqa: E402

URP_RUN = dict(run_id="2" * 32, editor_version="6000.6.9f1", build_target="StandaloneWindows64",
               build_target_id=19, variant="urp")
ANDROID_RUN = dict(run_id="3" * 32, editor_version="2022.3.62f3", build_target="Android",
                   build_target_id=13, variant="android")
LIT, UNLIT = "Universal Render Pipeline/Lit", "Universal Render Pipeline/Unlit"
RESOURCE = "CAB-synthetic.resS"
# name: width, height, TextureFormat, bytes
ANDROID = {
    "astc-4x4.png": (64, 64, 48, 4096), "astc-6x6.png": (40, 24, 50, 448), "astc-8x8.png": (64, 64, 51, 1024),
    "etc-rgb4.png": (64, 64, 34, 2048), "etc2-rgb4.png": (64, 64, 45, 2048),
    "etc2-rgb4-punchthrough.png": (64, 64, 46, 2048), "etc2-rgba8.png": (64, 64, 47, 4096),
}


def tex_env(path_id, scale=(1., 1.), offset=(0., 0.)):
    return {"t": "tuple", "v": ["_BaseMap" if path_id == 5 else "_BumpMap", {
        "m_Texture": pointer(path_id), "m_Scale": dict(zip("xy", scale, strict=True)),
        "m_Offset": dict(zip("xy", offset, strict=True))}]}


def urp_rows():
    lit = {"m_Shader": pointer(7), "m_ValidKeywords": ["_EMISSION", "_NORMALMAP"], "m_InvalidKeywords": [],
           "m_SavedProperties": {
               "m_TexEnvs": [tex_env(5, (2., 3.), (.25, .5)), tex_env(6)],
               "m_Floats": {"_Metallic": .625, "_Smoothness": .375, "_BumpScale": 1.5, "_Cull": 2.},
               "m_Colors": {"_BaseColor": dict(zip("rgba", (.25, .5, .75, 1.), strict=True)),
                            "_EmissionColor": dict(zip("rgba", (.125, .25, .5, 1.), strict=True))}}}
    unlit = {"m_Shader": pointer(8), "m_ValidKeywords": ["_ALPHATEST_ON"], "m_InvalidKeywords": [],
             "m_SavedProperties": {
                 "m_TexEnvs": [tex_env(5)], "m_Floats": {"_AlphaClip": 1., "_Cutoff": .375},
                 "m_Colors": {"_BaseColor": dict(zip("rgba", (1., .5, .25, 1.), strict=True))}}}
    return {
        "lit": lit, "unlit": unlit,
        "shaders": {7: LIT, 8: UNLIT},
        # The Editor's base texels: alpha 64 on every third texel, below the Unlit cutoff.
        "textures": {5: ("UrpBase", 4, rgba(16, lambda i: 64 if i % 3 == 0 else 255)),
                     6: ("UrpNormal", 2, rgba(4, lambda i: 255))},
    }


def rgba(count, alpha):
    return b"".join(bytes(((40 * i) & 255, (255 - 30 * i) & 255, 90 + 10 * i, alpha(i))) for i in range(count))


def gradient(width, height):
    return b"".join(bytes((x * 255 // max(1, width - 1), y * 255 // max(1, height - 1), (x ^ y) * 4 & 255, 255))
                    for y in range(height) for x in range(width))


def png(width, height, pixels):
    """An RGBA PNG whose rows cycle through the five filter types, as an encoder may choose."""
    stride, raw, prior = 4 * width, b"", bytes(4 * width)
    for y in range(height):
        line, kind = pixels[y * stride:(y + 1) * stride], y % 5
        out = bytearray([kind])
        for i in range(stride):
            a, b = line[i - 4] if i >= 4 else 0, prior[i]
            c = prior[i - 4] if i >= 4 else 0
            p = a + b - c
            paeth = a if abs(p - a) <= abs(p - b) and abs(p - a) <= abs(p - c) else b if abs(p - b) <= abs(p - c) else c
            out.append((line[i] - (0, a, b, (a + b) // 2, paeth)[kind]) & 255)
        raw, prior = raw + bytes(out), line

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def stream(size):
    return {"offset": 0, "size": size, "path": f"archive:/CAB-synthetic/{RESOURCE}"}


def saved_extraction(inspect, bundle, digest, rows):
    """The saved `unityrs extract` report and PNGs: rows of (path ID, container path, PNG bytes or failure)."""
    index_path = inspect / f"{bundle}.inspect.json"
    index = json.loads(index_path.read_text())
    index["inputs"][0]["files"].append(dict(file_index=len(index["inputs"][0]["files"]), file=RESOURCE,
                                            kind="ResourceFile", unity_version=None, target_platform=None))
    write(index_path, index)
    report, root = dict(written=[], failures=[]), inspect / f"{bundle}.extract"
    for path_id, path, result in rows:
        row = dict(path=path, serialized_file="CAB-synthetic", path_id=path_id, class_id=28)
        if isinstance(result, bytes):
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_bytes(result)
            report["written"].append(row | dict(size=len(result)))
        else:
            report["failures"].append(row | dict(reason=result))
    write(inspect / f"{bundle}.extract.json", dict(bundle_sha256=digest, report=report))


def write_shader_show(inspect, bundle, digest, path_id, name):
    """The depth-limited show of a built shader: m_ParsedForm.m_Name survives, its large members are cut."""
    index = json.loads((inspect / f"{bundle}.inspect.json").read_text())
    header = next(row for row in index["objects"] if row["path_id"] == path_id)
    cut = {"$unityrs": "cut", "reason": "depth", "omitted": 1}
    value = {"t": "dict", "v": [["m_Name", ""], ["m_ParsedForm", {"t": "dict", "v": [
        ["m_PropInfo", cut], ["m_SubShaders", cut], ["m_Name", name]]}]]}
    write(inspect / f"{bundle}.1.{path_id}.shader.json", dict(
        format="unityrs.show", version=1, bundle_sha256=digest, object=header, value=value,
        truncated=dict(cuts=2, items=0, depth=2, bytes=0, binary=0)))


def urp_bundle(bundles, inspect, spec, run=URP_RUN):
    assets = {ASSETS + "urp/" + name: i for name, i in (
        ("materials.prefab", 1), ("lit.mat", 3), ("unlit.mat", 4), ("base.asset", 5), ("normal.asset", 6))}
    rows = [(i, 28, name, {"m_Width": size, "m_Height": size, "m_CompleteImageSize": len(data),
                           "m_TextureFormat": 4, "m_MipCount": 1, "image data": {"t": "bytes", "hex": ""},
                           "m_StreamData": stream(len(data))})
            for i, (name, size, data) in spec["textures"].items()]
    rows += [(3, 21, "UrpLit", spec["lit"]), (4, 21, "UrpUnlit", spec["unlit"])]
    rows += [(i, 48, "", {"m_ParsedForm": {"m_Name": name}}) for i, name in spec["shaders"].items()]
    transforms = [1002, 1006]
    game_object(rows, 1, 1000, "materials", children=transforms)
    for i, (label, transform) in enumerate(zip(("Lit", "Unlit"), transforms, strict=True)):
        obj, mesh, renderer = (transform + offset for offset in (-1, 1, 2))
        game_object(rows, obj, transform, label, parent=1000, components=(mesh, renderer))
        mesh_parts(rows, obj, mesh, renderer, pointer(10500 + i), pointer(3 + i))
    rows += [(10500, 43, "Cube", {}), (10501, 43, "Quad", {})]
    digest = archive(bundles, inspect, "urp-materials", assets, rows, run)
    for path_id, name in spec["shaders"].items():
        write_shader_show(inspect, "urp-materials", digest, path_id, name)
    saved_extraction(inspect, "urp-materials", digest, [
        (i, f"{ASSETS}urp/{('base', 'normal')[i - 5]}.png", png(size, size, data))
        for i, (_, size, data) in spec["textures"].items()])


def texture(width, height, texture_format, size, *, inline=False):
    """Unity streams bundle texels into the .resS node."""
    data = {"t": "bytes", "hex": payload(size).hex() if inline else ""}
    return {"m_Width": width, "m_Height": height, "m_CompleteImageSize": size, "m_TextureFormat": texture_format,
            "m_MipCount": 1, "image data": data,
            "m_StreamData": {"offset": 0, "size": 0, "path": ""} if inline else stream(size)}


def payload(size):
    return bytes(i * 7 % 251 + 1 for i in range(size))


def android_bundle(bundles, inspect, textures, run=ANDROID_RUN, decoded=None):
    """`decoded` maps a texture name to its saved extract result, by default a gradient at its size."""
    assets = {ASSETS + "android/" + name: i for i, name in enumerate(textures, 1)}
    rows = [(i, 28, name.removesuffix(".png"), value) for i, (name, value) in enumerate(textures.items(), 1)]
    digest = archive(bundles, inspect, "android-textures", assets, rows, run)
    results = {name: png(value["m_Width"], value["m_Height"], gradient(value["m_Width"], value["m_Height"]))
               for name, value in textures.items()} | (decoded or {})
    saved_extraction(inspect, "android-textures", digest, [
        (i, ASSETS + "android/" + name, results[name]) for i, name in enumerate(textures, 1)])


class VariantRunTests(unittest.TestCase):
    def setUp(self):
        self.folder = scratch.for_case(self)
        self.inspect, self.bundles = self.folder / "inspection", self.folder / "bundles"
        for folder in (self.inspect, self.bundles):
            folder.mkdir()

    def handoff(self, run, cases):
        manifest = dict(format="unityrs.fixture_run", version=1, run=run, cases=[
            dict(name=name, status="recorded", reason="") for name in cases])
        write(self.folder / "run.json", manifest)
        (self.folder / "build.log").write_text(stamp(run) + "\nFinished.\n", encoding="utf-8")
        (self.folder / "unityrs-generated.txt").write_text(
            "Generated by the unityrs fixture builder.\n" + stamp(run) + "\n", encoding="utf-8")
        return manifest

    def complete(self):
        return child.run([sys.executable, "-I", str(HERE / "anim_replay.py"), "complete", str(self.folder),
                          "--inspect-dir", str(self.inspect)], capture_output=True, text=True)

    def assert_refused(self, message):
        result = self.complete()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(message, result.stderr)

    def urp(self, edit=None, cases=("urp-materials",)):
        spec = urp_rows()
        if edit:
            edit(spec)
        urp_bundle(self.bundles, self.inspect, spec)
        return self.handoff(URP_RUN, cases)

    def android(self, edit=None, run=ANDROID_RUN, decoded=None):
        textures = {name: texture(*row) for name, row in ANDROID.items()}
        if edit:
            edit(textures)
        static_bundles(self.bundles, self.inspect, run)
        android_bundle(self.bundles, self.inspect, textures, run, decoded)
        return self.handoff(run, (*STATIC, "android-textures"))

    def test_urp_run_validates_shaders_keywords_and_properties(self):
        self.urp()
        result = self.complete()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "Validated 1 urp fixture case(s).")

    def test_urp_refuses_a_built_in_shader(self):
        self.urp(lambda spec: spec["shaders"].update({8: "Unlit/Texture"}))
        self.assert_refused("built URP shader mismatch in unlit.mat: 'Unlit/Texture'")

    def test_urp_refuses_a_dropped_keyword(self):
        self.urp(lambda spec: spec["lit"].update(m_ValidKeywords=["_NORMALMAP"], m_InvalidKeywords=["_EMISSION"]))
        self.assert_refused("built URP keywords missing from lit.mat: _EMISSION")

    def test_urp_refuses_a_changed_property(self):
        self.urp(lambda spec: spec["lit"]["m_SavedProperties"]["m_Floats"].update(_Smoothness=.5))
        self.assert_refused("built URP float mismatch: lit.mat _Smoothness")

    def test_urp_refuses_a_changed_texture_slot(self):
        self.urp(lambda spec: spec["lit"]["m_SavedProperties"].update(m_TexEnvs=[tex_env(5), tex_env(6)]))
        self.assert_refused("built URP texture slot mismatch: lit.mat _BaseMap")

    def test_urp_refuses_a_base_texture_above_the_alpha_cutoff(self):
        self.urp(lambda spec: spec["textures"].update({5: ("UrpBase", 4, rgba(16, lambda i: 255))}))
        self.assert_refused("built URP base texture does not straddle the Unlit alpha cutoff")

    def test_urp_refuses_an_unrecorded_case(self):
        manifest = self.urp()
        manifest["cases"][0].update(status="failed", reason="Prepare urp-materials: no URP shader")
        write(self.folder / "run.json", manifest)
        self.assert_refused("urp-materials: failed (Prepare urp-materials: no URP shader)")

    def test_urp_refuses_a_built_in_case(self):
        self.urp(cases=("urp-materials", "static-materials"))
        self.assert_refused("unknown or duplicate urp run.json case: static-materials")

    def test_android_run_validates_static_bundles_and_texture_formats(self):
        self.android()
        result = self.complete()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "Validated 6 android fixture case(s).")

    def test_android_accepts_inline_texture_data(self):
        self.android(lambda textures: textures.update({"astc-6x6.png": texture(40, 24, 50, 448, inline=True)}))
        # Canonical show summarises the 448 bytes as their length and digest.
        path = self.inspect / "android-textures.1.2.show.json"
        doc = json.loads(path.read_text())
        rows = doc["value"]["v"]
        index = next(i for i, (key, _) in enumerate(rows) if key == "image data")
        rows[index][1] = {"t": "bytes", "len": 448, "sha256": hashlib.sha256(payload(448)).hexdigest()}
        write(path, doc)
        result = self.complete()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_android_refuses_short_inline_hex(self):
        self.android(lambda textures: textures["astc-6x6.png"].update(
            {"image data": {"t": "bytes", "hex": payload(48).hex()}, "m_StreamData": {"offset": 0, "size": 0,
                                                                                     "path": ""}}))
        self.assert_refused("built texture data size mismatch: astc-6x6.png")

    def test_android_refuses_hex_beside_a_summarised_length(self):
        self.android(lambda textures: textures["astc-6x6.png"].update(
            {"image data": {"t": "bytes", "len": 448, "hex": payload(48).hex()}}))
        self.assert_refused("invalid inline image data: astc-6x6.png")

    def test_android_refuses_a_stream_in_the_serialized_file(self):
        self.android(lambda textures: textures["astc-4x4.png"]["m_StreamData"].update(
            path="archive:/CAB-synthetic/CAB-synthetic"))
        self.assert_refused("built texture stream must name the bundle's .resS node: astc-4x4.png")

    def test_android_refuses_a_stream_of_the_wrong_length(self):
        self.android(lambda textures: textures["etc2-rgb4.png"]["m_StreamData"].update(size=1024))
        self.assert_refused("built texture data size mismatch: etc2-rgb4.png")

    def test_android_refuses_a_texture_unityrs_cannot_decode(self):
        # A range past the end of the resource node fails the decode.
        self.android(decoded={"astc-6x6.png": "decode"})
        self.assert_refused("unityrs extract could not decode the built texture astc-6x6.png: decode")

    def test_android_refuses_a_decode_at_another_size(self):
        self.android(decoded={"astc-8x8.png": png(32, 32, gradient(32, 32))})
        self.assert_refused("unityrs extract decoded astc-8x8.png at 32x32, not its declared size")

    def test_android_refuses_zeroed_texels(self):
        self.android(decoded={"etc2-rgba8.png": png(64, 64, bytes(4 * 64 * 64))})
        self.assert_refused("the built texture etc2-rgba8.png decodes to a single colour")

    def test_android_refuses_an_uncompressed_fallback(self):
        self.android(lambda textures: textures.update({"etc2-rgba8.png": texture(64, 64, 4, 16384)}))
        self.assert_refused("etc2-rgba8.png is 4, expected 47 from its Android override")

    def test_android_refuses_the_wrong_block_size(self):
        self.android(lambda textures: textures.update({"astc-6x6.png": texture(40, 24, 50, 640)}))
        self.assert_refused("built texture data size mismatch: astc-6x6.png")

    def test_android_run_must_target_android(self):
        self.android(run=ANDROID_RUN | {"build_target": "StandaloneWindows64", "build_target_id": 19})
        self.assert_refused("the Android fixture run must target Android (13)")

    def test_unknown_variant_is_refused(self):
        self.urp()
        manifest = json.loads((self.folder / "run.json").read_text())
        manifest["run"]["variant"] = "hdrp"
        write(self.folder / "run.json", manifest)
        self.assert_refused("unknown fixture run variant in run.json: 'hdrp'")


if __name__ == "__main__":
    unittest.main()
