"""The `sprites` kind's reference (`sprites_kind.py`) on its own branches, beside the fixture its scenes record
(`tools/fixtures/sprites.py`): a top-level WebFile row (`Divergence`); a `MemoryError` that is Pillow's
(`pillow_memory_error`) and one that is not (propagated); its own name read (`atlas_name`, B3's review round 2, T4);
the render data's selection (E17, B3's review rounds 1 and 2, R2, T1 and S3), on objects made here: a tag's atlas
found past another's and a nameless one, never reading a later one; a tag none has, whose walk reads every name; a
tag that is no string; tags that do not index; a key the atlas lacks; keys compared as Python compares them, and keys
of another shape; an atlas of any class; an item that does not unpack; an entry that is no `SpriteAtlasData`; no
tag; and the Sprite's fields of another shape (R8); `resolve`'s order (S5); pointers and render data as UnityPy
constructs and follows them, the names a `Sprite` cannot set, and the render of the data the plan resolved (round 3,
S3a, S3b and S6). Needs UnityPy 1.25.2 and Pillow 9.5.0 (the oracle image), skipped elsewhere.
"""

import re
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402

if ORACLE:
    import ngui_read as rd
    import sprites_kind as sk
    from PIL import Image
    from UnityPy.classes import GUID, PPtr, Sprite, SpriteAtlas, SpriteAtlasData, Texture2D
    from UnityPy.classes.UnknownObject import UnknownObject
    from UnityPy.enums import ClassIDType
    from UnityPy.export import SpriteHelper
    from UnityPy.files import WebFile


def raise_here():
    """A `MemoryError` raised outside Pillow."""
    raise MemoryError


def too_wide():
    """Pillow's own `MemoryError`: an RGBA image of 2^29 pixels' width, a line past a C `int`."""
    return Image.new("RGBA", (1 << 29, 1))


class Fake:
    """An object of a file: its class, its name as `peek_name` gives it (an exception raised), its read."""

    def __init__(self, path_id, type_, name=None, value=None):
        self.path_id = path_id
        self.type = type_
        self.name = name
        self.value = value
        self.serialized_type = SimpleNamespace(node=object())
        self.assets_file = None
        self.names_read = 0

    def read(self, check_read=True):
        return self.value


def rect(x, y, w, h):
    return SimpleNamespace(x=float(x), y=float(y), width=float(w), height=float(h))


def data(path_id, w=2, alpha=None):
    """A `SpriteAtlasData` on the texture at `path_id` (and the alpha texture at `alpha`, where set)."""
    return SpriteAtlasData(
        alphaTexture=PPtr(m_FileID=0, m_PathID=alpha or 0),
        downscaleMultiplier=1.0,
        settingsRaw=2,
        texture=PPtr(m_FileID=0, m_PathID=path_id),
        textureRect=rect(0, 0, w, 2),
        textureRectOffset=None,
        uvTransform=None,
    )


def guid(*words):
    return GUID(data_0_=words[0], data_1_=words[1], data_2_=words[2], data_3_=words[3])


def atlas(name, entries):
    return SpriteAtlas(
        m_IsVariant=False,
        m_Name=name,
        m_PackedSpriteNamesToIndex=[],
        m_PackedSprites=[],
        m_RenderDataMap=entries,
        m_Tag=name or "",
    )


class Node:
    """A typetree node: its type, name and children, the fields `atlas_name` reads."""

    def __init__(self, name, children=()):
        self.m_Level, self.m_Type, self.m_Name, self.m_ByteSize, self.m_Version = 0, "SpriteAtlas", name, -1, 1
        self.m_Children = list(children)


class Peeked:
    """An object whose `parse_as_dict` reads `values` for the children it is given, each read recorded."""

    def __init__(self, names, values):
        self.serialized_type = SimpleNamespace(node=Node("Base", [Node(n) for n in names]))
        self.values = values
        self.reads = []

    def parse_as_dict(self, node, check_read=True):
        self.reads.append(([c.m_Name for c in node.m_Children], check_read))
        return {c.m_Name: self.values[c.m_Name] for c in node.m_Children}


def failure(test, f, want=("decode_error", "render")):
    with test.assertRaises(rd.Failure) as got:
        f()
    test.assertEqual((got.exception.reason, got.exception.detail), want)


@unittest.skipUnless(ORACLE, "needs the oracle image (UnityPy 1.25.2)")
class Branches(unittest.TestCase):
    def test_a_top_level_webfile_row_is_a_divergence(self):
        web = WebFile.__new__(WebFile)
        bundle = SimpleNamespace(files={})
        with (
            mock.patch.object(rd, "load", return_value=bundle),
            mock.patch.object(rd, "rows", return_value=[("x.web", web)]),
            self.assertRaises(rd.Divergence),
        ):
            sk.cut("x.bundle", {"x.bundle": b""}, "{name}.png")

    def test_a_memory_error_is_pillows_only_from_pillow(self):
        own = data(1)
        p = SimpleNamespace(sprite=SimpleNamespace(assets_file=SimpleNamespace(_cache={}), m_RD=own), data=own)
        render = "get_image_from_sprite"
        with mock.patch.object(sk.SpriteHelper, render, lambda _: too_wide()), self.assertRaises(rd.Failure) as got:
            sk.rendered(p)
        self.assertEqual((got.exception.reason, got.exception.detail), ("decode_error", "pillow_memory_error"))
        with mock.patch.object(sk.SpriteHelper, render, lambda _: raise_here()), self.assertRaises(MemoryError):
            sk.rendered(p)


@unittest.skipUnless(ORACLE, "needs the oracle image (UnityPy 1.25.2)")
class Names(unittest.TestCase):
    """The reference's own name read (`atlas_name`, as `peek_name` reads one): the first `m_Name` or `name` child,
    read unchecked with the children up to it, its value whatever its type; none with neither (B3's review round 2,
    T4)."""

    def test_the_first_name_child_is_read_with_the_children_before_it(self):
        for names, want, read in (
            (["a", "m_Name", "b"], "x", ["a", "m_Name"]),
            (["name", "m_Name"], "y", ["name"]),
            (["a", "name"], "y", ["a", "name"]),
        ):
            obj = Peeked(names, {"a": 1, "b": 2, "m_Name": "x", "name": "y"})
            self.assertEqual(sk.atlas_name(obj), want, names)
            self.assertEqual(obj.reads, [(read, False)], names)

    def test_a_name_that_is_no_string_is_its_value(self):
        obj = Peeked(["m_Name"], {"m_Name": 7})
        self.assertEqual(sk.atlas_name(obj), 7)

    def test_no_name_child_is_none_unread(self):
        obj = Peeked(["a", "m_Names"], {})
        self.assertIsNone(sk.atlas_name(obj))
        self.assertEqual(obj.reads, [])


@unittest.skipUnless(ORACLE, "needs the oracle image (UnityPy 1.25.2)")
class Selection(unittest.TestCase):
    """`render_data` over a file whose objects are, in order: `other` (another tag), a nameless atlas, `packs` (keys
    K1 and K2), and `broken`, whose name's read fails; each name's peeks counted (`names_read`)."""

    def setUp(self):
        self.k1, self.k2, self.k3 = (guid(1, 2, 3, 4), 1), (guid(1, 2, 3, 4), 2), (guid(1, 2, 3, 5), 1)
        sa = ClassIDType.SpriteAtlas
        self.objects = {
            3: Fake(3, sa, "other", atlas("other", [(self.k1, data(1))])),
            4: Fake(4, sa, None, atlas(None, [(self.k1, data(1))])),
            5: Fake(5, sa, "packs", atlas("packs", [(self.k1, data(2, 3)), (self.k2, data(2, 4))])),
            6: Fake(6, sa, rd.Failure(*rd.READ), None),
            7: Fake(7, ClassIDType.Texture2D),
        }
        self.file = SimpleNamespace(objects=self.objects)
        for o in self.objects.values():
            o.assets_file = self.file
        self.obj = SimpleNamespace(assets_file=self.file)
        patch = mock.patch.object(sk, "atlas_name", side_effect=self.peek)
        patch.start()
        self.addCleanup(patch.stop)

    def peek(self, o):
        o.names_read += 1
        if isinstance(o.name, Exception):
            raise o.name
        return o.name

    def names_read(self):
        return [self.objects[i].names_read for i in (3, 4, 5, 6)]

    def sprite(self, tags, key, atlas_ptr=None):
        return SimpleNamespace(
            m_SpriteAtlas=atlas_ptr, m_AtlasTags=tags, m_RenderDataKey=key, m_RD=data(9), m_PixelsToUnits=1.0
        )

    def fails(self, sprite, want=("decode_error", "render")):
        failure(self, lambda: sk.render_data(sprite, self.obj), want)

    def test_a_tag_finds_the_first_atlas_of_its_name(self):
        got, holder = sk.render_data(self.sprite(["packs", "other"], self.k2), self.obj)
        self.assertEqual((got.texture.m_PathID, got.textureRect.width), (2, 4))
        self.assertIs(holder, self.file)
        self.assertEqual(self.names_read(), [1, 1, 1, 0])

    def test_a_tag_none_has_reads_every_name(self):
        self.fails(self.sprite(["nothing"], self.k1), rd.READ)
        self.assertEqual(self.names_read(), [1, 1, 1, 1])

    def test_a_tag_that_is_no_string_equals_no_name(self):
        del self.objects[6]
        for tags in ([5], b"packs", [b"packs"]):
            sprite = self.sprite(tags, self.k1)
            got, _ = sk.render_data(sprite, self.obj)
            self.assertIs(got, sprite.m_RD, tags)

    def test_tags_that_do_not_index_raise_after_the_first_name(self):
        self.fails(self.sprite(7, self.k1))
        self.assertEqual(self.names_read(), [1, 0, 0, 0])

    def test_a_key_the_atlas_lacks_is_render(self):
        self.fails(self.sprite(["packs"], self.k3))

    def test_keys_compare_as_python_compares_them(self):
        packs = self.objects[5].value
        big = (guid(1, 2, 3, 4), 1 << 63)
        packs.m_RenderDataMap = [(big, data(2, 5)), ((guid(1, 2, 3, 4), 1.0), data(2, 6))]
        got, _ = sk.render_data(self.sprite(["packs"], (guid(1, 2, 3, 4), float(1 << 63))), self.obj)
        self.assertEqual(got.textureRect.width, 5)
        got, _ = sk.render_data(self.sprite(["packs"], self.k1), self.obj)
        self.assertEqual(got.textureRect.width, 6)

    def test_a_key_of_another_shape_equals_none(self):
        packs = self.objects[5].value
        unknown = UnknownObject(data_0_=1, data_1_=2, data_2_=3, data_3_=4)
        packs.m_RenderDataMap = [(("g", 1), data(2)), ((unknown, 1), data(2))]
        for key in (("g", 1), (unknown, 1), self.k1):
            self.fails(self.sprite(["packs"], key))

    def test_an_atlas_of_any_class_is_walked(self):
        packs = self.objects[5]
        packs.value = UnknownObject(m_RenderDataMap=[(self.k1, data(2, 7))])
        got, _ = sk.render_data(self.sprite(["packs"], self.k1), self.obj)
        self.assertEqual(got.textureRect.width, 7)
        for value in (UnknownObject(m_Name="packs"), SimpleNamespace(m_RenderDataMap=5)):
            packs.value = value
            self.fails(self.sprite(["packs"], self.k1))

    def test_an_item_that_does_not_unpack_is_render(self):
        packs = self.objects[5].value
        for first in (5, (self.k1,), (self.k1, data(2), 1)):
            packs.m_RenderDataMap = [first, (self.k1, data(2))]
            self.fails(self.sprite(["packs"], self.k1))

    def test_an_entry_that_is_no_sprite_atlas_data_is_render(self):
        packs = self.objects[5].value
        entry = data(2)
        packs.m_RenderDataMap = [(self.k1, UnknownObject(texture=entry.texture, textureRect=entry.textureRect))]
        self.fails(self.sprite(["packs"], self.k1))

    def test_no_tag_is_the_sprites_own_render_data(self):
        sprite = self.sprite([], self.k1)
        got, holder = sk.render_data(sprite, self.obj)
        self.assertIs(got, sprite.m_RD)
        self.assertIs(holder, self.file)
        self.assertEqual(sum(o.names_read for o in self.objects.values()), 0)

    def test_fields_of_another_shape_are_read(self):
        good = self.sprite(["packs"], None)
        sk.fields(good)
        float_settings = data(1)
        float_settings.settingsRaw = 2.0
        sk.fields(SimpleNamespace(**{**vars(good), "m_AtlasTags": [1], "m_RenderDataKey": ("g", 1)}))
        # A pointer that is no `PPtr` is followed later, as UnityPy follows it (round 3, S3a).
        sk.fields(SimpleNamespace(**{**vars(good), "m_SpriteAtlas": 5}))
        no_texture = data(1)
        no_texture.texture = sk.MISSING
        unknown_rd = UnknownObject(textureRect=rect(0, 0, 2, 2), settingsRaw=2)
        for bad in (
            SimpleNamespace(**{**vars(good), "m_PixelsToUnits": 1}),
            SimpleNamespace(**{**vars(good), "m_RD": float_settings}),
            SimpleNamespace(**{**vars(good), "m_SpriteAtlas": PPtr(m_FileID=0, m_PathID=1.5)}),
            SimpleNamespace(**{**vars(good), "m_RD": unknown_rd}),
        ):
            failure(self, lambda: sk.fields(bad), rd.READ)

    def test_a_pointer_is_true_and_followed_as_unitypy_takes_it(self):
        # Round 3, S3a: a `PPtr` of path id 0 is false, so its tag is looked for; any other value's truth is
        # Python's, and following one that is no `PPtr` raises.
        got, _ = sk.render_data(self.sprite(["packs"], self.k1, PPtr(m_FileID=7, m_PathID=0)), self.obj)
        self.assertEqual(got.textureRect.width, 3)
        got, _ = sk.render_data(self.sprite([], self.k1, 0), self.obj)
        self.assertEqual(got.texture.m_PathID, 9)
        for ptr in (UnknownObject(m_FileID=0, m_PathID=5), 5):
            self.fails(self.sprite(["packs"], self.k1, ptr))
        self.assertEqual(self.names_read(), [1, 1, 1, 0])
        # `m_AtlasTags` a `PPtr`: false of path id 0, else not indexable.
        got, _ = sk.render_data(self.sprite(PPtr(m_FileID=0, m_PathID=0), self.k1), self.obj)
        self.assertEqual(got.texture.m_PathID, 9)
        self.fails(self.sprite(PPtr(m_FileID=0, m_PathID=3), self.k1))


@unittest.skipUnless(ORACLE, "needs the oracle image (UnityPy 1.25.2)")
class Order(unittest.TestCase):
    """`resolve` in `get_image`'s order (B3's review round 2, S5): the texture's dereference and answers, then the
    alpha texture's, so a texture whose read fails beside an alpha texture that names nothing is the texture's skip."""

    def test_the_texture_answers_before_the_alpha_is_dereferenced(self):
        calls = []

        def deref(file_id, path_id, holder):
            calls.append(("deref", path_id))
            if path_id == 8:
                raise rd.Failure("bad_pointer", None)
            return path_id

        def answered(texture):
            calls.append(("answered", texture))
            raise rd.Failure(*rd.READ)

        sprite = SimpleNamespace(m_RD=data(1, alpha=8))
        with (
            mock.patch.object(rd, "checked", return_value=sprite),
            mock.patch.object(rd, "typed", return_value=SimpleNamespace(read=None)),
            mock.patch.object(rd, "shaped", side_effect=lambda v, *_: v),
            mock.patch.object(sk, "fields"),
            mock.patch.object(sk, "render_data", return_value=(sprite.m_RD, None)),
            mock.patch.object(rd, "deref", side_effect=deref),
            mock.patch.object(sk, "answered", side_effect=answered),
        ):
            failure(self, lambda: sk.resolve(SimpleNamespace(assets_file=None)), rd.READ)
        self.assertEqual(calls, [("deref", 1), ("answered", 1)])

    def resolved(self, rd_value):
        """`resolve` of a Sprite whose render data is `rd_value`: its result or failure, and the dereferences."""
        calls = []

        def deref(file_id, path_id, holder):
            calls.append(path_id)
            return path_id

        with (
            mock.patch.object(rd, "checked", return_value=SimpleNamespace(m_RD=rd_value)),
            mock.patch.object(rd, "typed", return_value=SimpleNamespace(read=None)),
            mock.patch.object(rd, "shaped", side_effect=lambda v, *_: v),
            mock.patch.object(sk, "fields"),
            mock.patch.object(sk, "render_data", return_value=(rd_value, None)),
            mock.patch.object(rd, "deref", side_effect=deref),
            mock.patch.object(sk, "answered", side_effect=lambda t: t),
        ):
            try:
                return sk.resolve(SimpleNamespace(assets_file=None))[2:], calls
            except rd.Failure as f:
                return (f.reason, f.detail), calls

    def test_an_unknown_render_data_with_no_alpha_is_render_before_any_dereference(self):
        # Round 3, S3a: export/SpriteHelper.py:84 reads `alphaTexture`; a constructed class gives its default.
        texture = PPtr(m_FileID=0, m_PathID=1)
        unknown = UnknownObject(texture=texture, textureRect=rect(0, 0, 2, 2), settingsRaw=2)
        self.assertEqual(self.resolved(unknown), (sk.RENDER, []))
        unknown.alphaTexture = PPtr(m_FileID=0, m_PathID=0)
        self.assertEqual(self.resolved(unknown), ((1, None), [1]))

    def test_a_pointer_that_is_no_pptr_is_render_before_any_dereference(self):
        # `get_image`'s `.path_id`s (export/SpriteHelper.py:48, export/SpriteHelper.py:55).
        other = UnknownObject(m_FileID=0, m_PathID=1)
        self.assertEqual(self.resolved(data(1, alpha=None)), ((1, None), [1]))
        for texture, alpha in ((other, None), (other, 2), (PPtr(m_FileID=0, m_PathID=1), other)):
            d = data(1)
            d.texture, d.alphaTexture = texture, alpha
            self.assertEqual(self.resolved(d), (sk.RENDER, []), (texture, alpha))
        # A false alpha texture is none, whatever it is.
        d = data(1)
        d.alphaTexture = 0
        self.assertEqual(self.resolved(d), ((1, None), [1]))


@unittest.skipUnless(ORACLE, "needs the oracle image (UnityPy 1.25.2)")
class Constructed(unittest.TestCase):
    """What the reference takes from UnityPy's classes (B3's review round 3, S3b and S6)."""

    def test_the_setterless_names_are_what_a_sprite_and_a_texture_refuse_and_the_engines(self):
        # Every name on `Sprite` (and, round 4's R4, `Texture2D`) and its bases, and the three of every instance,
        # set on an instance as `parse_class` sets an extra: the refused ones are the list, which the engine's
        # `SETTERLESS` is. `Texture2D.image`'s setter raises for a value that is neither an image nor a path to one
        # (the next test).
        for cls in (Sprite, Texture2D):
            names = {"__class__", "__dict__", "__weakref__"}.union(*(vars(k) for k in cls.__mro__))
            refused = set()
            for name in names:
                try:
                    object.__setattr__(cls.__new__(cls), name, 0)
                except (AttributeError, TypeError):
                    refused.add(name)
            self.assertEqual(refused, set(sk.SETTERLESS), cls)
        source = (HERE.parents[1] / "crates/unityrs-export/src/ngui/read.rs").read_text()
        block = re.search(r"pub\(crate\) const SETTERLESS: \[&\[u8\]; \d+\] = \[(.*?)\];", source, re.S)
        self.assertEqual(tuple(re.findall(r'b"([^"]*)"', block.group(1))), sk.SETTERLESS)

    def test_a_texture_image_naming_a_host_image_is_built_by_unitypy_and_refused_here(self):
        # Round 5's Q2: `Texture2D.image`'s setter opens a value that is no image as a path
        # (classes/legacy_patch/Texture2D.py:25-26). Set it on an initialized Texture2D, with a synthetic PNG in
        # scratch: UnityPy accepts both str and bytes paths and replaces the texture's sides and pixels.
        # `sprite_attributes`' sheet 7 has a string `image`, the testkit's `IMAGE_PATH`. Both references refuse
        # it, `read`, before UnityPy reads it, as the engine does (the fixture's `texture_image_path`): a stated
        # divergence (PARITY.md, Tier C).
        import scratch
        import sprites_scenes

        source = (HERE.parents[1] / "crates/unityrs-testkit/src/ngui/sprite_objects.rs").read_text()
        path = re.search(r'pub const IMAGE_PATH: &str = "([^"]*)";', source).group(1)
        scene = sprites_scenes.scenes()["sprite_attributes"]
        objects = rd.load(scene.view("x.bundle"), "x.bundle").files["CAB-o4"].objects
        built = rd.texture_of(objects[1])
        self.assertIsInstance(built, Texture2D)
        here = scratch.for_case(self)
        Image.new("RGBA", (3, 5), (1, 2, 3, 255)).save(here / path)
        for value in (str(here / path), bytes(here / path)):
            built.image = value
            self.assertEqual((built.m_Width, built.m_Height), (3, 5))
            self.assertEqual(built.image.getpixel((0, 0)), (1, 2, 3, 255))
        with mock.patch.object(Image, "open", side_effect=AssertionError("opened")) as opened:
            failure(self, lambda: rd.texture_of(objects[7]), rd.READ)
            failure(self, lambda: sk.answered(objects[7]), rd.READ)
            # parse_class can swallow a setter's exception and return UnknownObject. Check the forbidden I/O
            # itself, so that fallback cannot make an attempted open look like the policy's read refusal.
            opened.assert_not_called()
        (here / path).unlink()
        failure(self, lambda: rd.texture_of(objects[7]), rd.READ)

    def test_an_image_child_keeps_the_checked_reads_failure_before_the_policy_refusal(self):
        # The policy must not turn a checked read's `asset_bundle` into `read`, and must not call the setter.
        import sprites_scenes

        scene = sprites_scenes.scenes()["sprite_attributes"]
        obj = rd.load(scene.view("x.bundle"), "x.bundle").files["CAB-o4"].objects[7]
        obj.byte_size += 1
        with mock.patch.object(Image, "open", side_effect=AssertionError("opened")) as opened:
            failure(self, lambda: rd.texture_of(obj), ("decode_error", "asset_bundle"))
            opened.assert_not_called()

    def test_the_render_is_of_the_data_the_plan_resolved(self):
        # Round 3, S6: whatever the Sprite's own lookup would find, `get_image` is handed the planned data's
        # pointers, the Sprite's own or the atlas entry's.
        own, entry = data(1), data(2, alpha=3)
        sprite = SimpleNamespace(
            m_SpriteAtlas=PPtr(m_FileID=0, m_PathID=0),
            m_AtlasTags=[1],
            m_RenderDataKey=None,
            m_RD=own,
        )
        for planned in (own, entry):
            handed = []

            def get_image(s, texture, alpha, handed=handed):
                handed.append((texture, alpha))
                return Image.new("RGBA", (4, 4))

            with mock.patch.object(SpriteHelper, "get_image", side_effect=get_image):
                image = SpriteHelper.get_image_from_sprite(sk.Resolved(sprite, planned))
            self.assertEqual(handed, [(planned.texture, planned.alphaTexture)])
            self.assertEqual(image.size, (2, 2))


if __name__ == "__main__":
    unittest.main()
