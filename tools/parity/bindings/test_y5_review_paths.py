"""Regressions for destination anchoring and atomic image failures."""
import unittest
from types import SimpleNamespace as NS
from unittest import mock

import UnityPy
import unityrs
import binding_y5_review as R
import binding_y5_tools as B
from binding_cases import S
from binding_y4_save import _load
import scratch


def exporters(module):
    from PIL import Image
    reader = NS(assets_file="file", path_id=1)
    common = dict(assets_file="file", object_reader=reader)
    E = B.extractor(module)
    return [
        (E.exportTextAsset, NS(m_Script="data", **common)),
        (E.exportFont, NS(m_FontData=b"OTTOdata", **common)),
        (E.exportMesh, NS(export=lambda: "mesh", **common)),
        (E.exportShader, NS(export=lambda: "shader", **common)),
        (E.exportMonoBehaviour, NS(parse_as_dict=lambda: {}, assets_file="file", path_id=1)),
        (E.exportAudioClip, NS(samples={"one": b"audio"}, object_reader=reader)),
        (E.exportSprite, NS(image=Image.new("RGBA", (1, 1)), object_reader=reader,
                           m_RD=NS(texture=NS(assetsfile="file", path_id=2), alphaTexture=NS(path_id=0)))),
        (E.exportTexture2D, NS(m_Width=1, image=Image.new("RGBA", (1, 1)), **common)),
        (E.exportGameObject, NS(**common)),
    ]


class ReviewPaths(unittest.TestCase):
    def test_destination_is_opened_after_dispatch_and_both_filters(self):
        self.assertEqual(R.observe_destination_order(unityrs), R.observe_destination_order(UnityPy))

    def test_failed_png_replays_oracle_partial_and_preserves_our_existing_file(self):
        oracle = R.observe_failed_image(UnityPy)
        self.assertEqual(oracle["size"], 33)
        self.assertFalse(oracle["preserved"])
        self.assertEqual(R.observe_failed_image(unityrs), R.observe_failed_image(UnityPy, corrected=True))

    def test_every_direct_exporter_refuses_hostile_extensions_before_creating(self):
        E = B.extractor(unityrs)
        with scratch.folder("y5-direct-extensions-") as folder:
            dst = folder / "out"
            dst.mkdir()
            (dst / "name").mkdir()
            for function, obj in exporters(unityrs):
                for suffix in ("/../../outside", "/absolute", "../outside", "a/b", "a\\b", "c:x", "nul\0"):
                    with self.subTest(exporter=function.__name__, suffix=suffix):
                        with mock.patch.object(E, "crawl_obj", return_value={}):
                            with self.assertRaises(ValueError):
                                function(obj, str(dst / "name"), suffix)
            self.assertEqual(sorted(path.relative_to(folder).as_posix() for path in folder.rglob("*")),
                             ["out", "out/name"])

    def test_every_direct_exporter_uses_exact_destination_string(self):
        calls = []
        class Destination(str):
            def __format__(self, spec):
                calls.append(spec)
                return str(outside / "redirect")
        E = B.extractor(unityrs)
        with scratch.folder("y5-direct-format-") as folder:
            outside = folder / "outside"
            outside.mkdir()
            for index, (function, obj) in enumerate(exporters(unityrs)):
                dst = folder / str(index)
                dst.mkdir()
                with self.subTest(exporter=function.__name__), mock.patch.object(E, "crawl_obj", return_value={}):
                    function(obj, Destination(str(dst / "name")))
            self.assertEqual(calls, [])
            self.assertEqual(list(outside.iterdir()), [])

    def test_every_direct_exporter_refuses_symlinked_destination_components(self):
        E = B.extractor(unityrs)
        with scratch.folder("y5-direct-symlinks-") as folder:
            outside = folder / "outside"
            outside.mkdir()
            (folder / "link").symlink_to(outside, target_is_directory=True)
            for function, obj in exporters(unityrs):
                # GameObject needs a reference to trigger directory creation.
                with self.subTest(exporter=function.__name__), mock.patch.object(E, "crawl_obj", return_value={1: obj}):
                    with self.assertRaises(OSError):
                        function(obj, str(folder / "link" / "name"))
            self.assertEqual(list(outside.iterdir()), [])

    def test_object_and_audio_leaf_names_refuse_separators_before_creating(self):
        E = B.extractor(unityrs)
        with scratch.folder("y5-leaf-names-") as folder:
            for name in ("/absolute", "..", "a/b", "a\\b", "c:x", "nul\0"):
                tree = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
                obj = _load(unityrs, S.serialized_file([S.Type(49, tree)],
                    [S.Obj(1, 0, S.s32(name) + S.s32("data"))])).objects[1]
                with self.subTest(name=name, surface="object"), self.assertRaises(ValueError):
                    E.export_obj(obj, str(folder / "absent"), append_name=True)
                audio = NS(samples={"safe": b"ok", name: b"bad"}, object_reader=NS(assets_file="file", path_id=1))
                with self.subTest(name=name, surface="audio"), self.assertRaises(ValueError):
                    E.exportAudioClip(audio, str(folder / "absent"))
            self.assertEqual(list(folder.iterdir()), [])

    def test_gameobject_mesh_and_material_names_are_preflighted_before_mkdir(self):
        E = B.extractor(unityrs)
        obj = NS(assets_file="file", object_reader=NS(path_id=1))
        with scratch.folder("y5-graph-names-") as folder:
            for kind in (unityrs.enums.ClassIDType.Mesh, unityrs.enums.ClassIDType.Material):
                for name in ("/absolute", "..", "a/b", "a\\b", "c:x", "nul\0"):
                    refs = {2: NS(assets_file="file", m_Name="safe", type=kind),
                            3: NS(assets_file="file", m_Name=name, type=kind)}
                    with self.subTest(kind=kind, name=name), mock.patch.object(E, "crawl_obj", return_value=refs):
                        with self.assertRaises(ValueError):
                            E.exportGameObject(obj, str(folder / "absent"))
            self.assertEqual(list(folder.iterdir()), [])

    def test_extension_subclasses_cannot_redirect_output(self):
        calls = []
        class Extension(str):
            def __format__(self, spec):
                calls.append(spec)
                return "/../../outside"
        E = B.extractor(unityrs)
        obj = NS(m_Script="data", assets_file="file", object_reader=NS(path_id=1))
        with scratch.folder("y5-extension-format-") as folder:
            E.exportTextAsset(obj, str(folder / "name"), Extension(".txt"))
            self.assertEqual((folder / "name.txt").read_bytes(), b"data")
            self.assertEqual(calls, [])
