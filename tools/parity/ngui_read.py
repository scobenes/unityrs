"""The reads of the NGUI cutter's reference (`ngui_cutter.py`): docs/BATCH_SPEC.md section 5's recognition and sheet
reads, each failure classified as docs/plans/batch.md 2.7 classifies it.

**The Environment** (4.3) registers the input's view, from its bytes: the other bundles with `is_dependency=True` in
bytewise order of id, then the input. Then it is sealed: UnityPy's lookups past its registry (`find_file`'s check
of `<path>/<name>`, which with an empty `path` is the host's root, and the files `load_file` finds relative to the
working directory, which `load_dependencies` asks for) are replaced by a miss, and its filesystem by one that
refuses every call. So a name the view does not register is not found, whatever is on the host.

**Pointers** are dereferenced by UnityPy's `PPtr.deref` (classes/PPtr.py:43-90), which looks in the reading file's
own bundle, then the registry. Its failures are batch.md 2.7's `DerefError`s: a null pointer, an `m_FileID` below 0
or past the externals, a missing object and a target that is no SerializedFile (a raw or container row) are
`bad_pointer`; a CAB the view does not register is `outside_view(<cab>)`.

**Object reads** go through UnityPy (UnityPyBoost's reader): a read that runs out or meets a value of the wrong
shape is a `ParseError`'s `Read`, `decode_error` with the code `read`, and UnityPy's `TypeTreeError` its `Tree`,
`tree` (BATCH_SPEC 7.3's codes). `MemoryError` and anything else are not classified: they propagate, as the
engine stops on a host's refusal or an unknown error.

**A sheet's data** (section 5) is its `image_data`, else the `m_StreamData` resource that 4.3's own-chain procedure
finds: the spec's lookup, written here, over UnityPy's candidate names (helpers/ResourceReader.py:10-33), reading
the row with UnityPy's own `_get_resource_data`; the registry (UnityPy's `get_cab`) only when no own row answers.
The bytes go to UnityPy's `parse_image_data`, as `Texture2D.image` gives them to it, so the image is UnityPy's and
Pillow's, after section 3's answers 5 to 10 in their order.
"""

from __future__ import annotations

import ntpath

import UnityPy
from PIL import Image
from UnityPy.classes import PPtr
from UnityPy.enums import BuildTarget, ClassIDType, TextureFormat
from UnityPy.environment import simplify_name
from UnityPy.exceptions import TypeTreeError
from UnityPy.export import Texture2DConverter
from UnityPy.files import BundleFile, SerializedFile, WebFile
from UnityPy.helpers import TextureSwizzler
from UnityPy.helpers.ResourceReader import _get_resource_data
from UnityPy.helpers.TypeTreeHelper import FUNCTION_READ_MAP

FIVE = ("RGB24", "RGBA32", "DXT1", "DXT5", "BC7")
TEXTURE_PIXELS = 268_435_456
INT_MIN, INT_MAX = -(2**31), 2**31 - 1
# Pillow's raw decoder refuses a row of more than `INT_MAX / bits - 7` pixels (BATCH_SPEC section 3's answer 8).
RAW_WIDTH_CAP = {"RGBA32": INT_MAX // 32 - 7, "RGB24": INT_MAX // 24 - 7}
# `Image._crop`'s decompression-bomb check refuses a crop of more pixels than this (section 5's crops).
CROP_PIXELS = 2 * Image.MAX_IMAGE_PIXELS
# The typetree types `FUNCTION_READ_MAP` reads as an integer (helpers/TypeTreeHelper.py:29-52): all but `float`,
# `double`, `bool`, `string` and `TypelessData`.
INTEGER = frozenset(FUNCTION_READ_MAP) - {"float", "double", "bool", "string", "TypelessData"}
RECT = ("x", "y", "width", "height")
# What an object read raises when its bytes run out or a value has the wrong shape.
READ_FAILURES = (EOFError, ValueError, IndexError, KeyError, TypeError, AttributeError)


class Failure(Exception):
    """A read's failure as batch.md 2.7 classifies it: an object's skip, its reason and detail."""

    def __init__(self, reason: str, detail: str | None = None):
        super().__init__(reason if detail is None else f"{reason}, {detail}")
        self.reason, self.detail = reason, detail


class _Sealed:
    """The sealed Environment's filesystem: any call is a bug of the seal, never an answer."""

    def __getattr__(self, name):
        raise RuntimeError(f"the reference's Environment is sealed to its view (fs.{name})")


def bytewise(text: str) -> bytes:
    return text.encode("utf-8")


def load(view: dict[str, bytes], input_id: str):
    """The input's bundle, in an Environment of its view, sealed."""
    env = UnityPy.Environment(path="")
    for bid in sorted((b for b in view if b != input_id), key=bytewise):
        env.load_file(view[bid], name=bid, is_dependency=True)
    bundle = env.load_file(view[input_id], name=input_id)
    env.find_file = lambda name, is_dependency=True: env.get_cab(name)
    env.load_file = lambda *args, **kwargs: None
    env.fs = _Sealed()
    return bundle


def read(f, *args):
    """An object read through UnityPy: its value, or its `Failure`."""
    try:
        return f(*args)
    except TypeTreeError as e:
        raise Failure("decode_error", "tree") from e
    except READ_FAILURES as e:
        raise Failure("decode_error", "read") from e


def deref(file_id: int, path_id: int, assets_file):
    """The object a pointer of `assets_file` names, or its `DerefError` as a `Failure`."""
    if path_id == 0 or file_id < 0 or file_id > len(assets_file.externals):
        raise Failure("bad_pointer")  # Null, FileIdBelowStart, FileIdPastEnd
    try:
        return PPtr(m_FileID=file_id, m_PathID=path_id, assetsfile=assets_file).deref()
    except FileNotFoundError as e:  # the sealed registry: NotFound
        raise Failure(f"outside_view({assets_file.externals[file_id - 1].path.rsplit('/')[-1]})") from e
    except (KeyError, AttributeError) as e:  # MissingObject; RawTarget, a row with no objects
        raise Failure("bad_pointer") from e


def deref_value(pointer, assets_file):
    """A typetree pointer value dereferenced from the file that holds it."""
    if not isinstance(pointer, dict):
        raise Failure("decode_error", "read")
    return deref(read(pointer.__getitem__, "m_FileID"), read(pointer.__getitem__, "m_PathID"), assets_file)


# ------------------------------------------------------------------ recognition (BATCH_SPEC section 5)


def _plain(node) -> bool:
    """A node `read_value` reads neither by `FUNCTION_READ_MAP` nor as a pair or a ReferencedObject."""
    return node.m_Type not in FUNCTION_READ_MAP and node.m_Type not in ("pair", "ReferencedObject")


def _vector(node) -> bool:
    return _plain(node) and bool(node.m_Children) and node.m_Children[0].m_Type == "Array"


def sprites_field(root) -> bool:
    """Recognition 2, from the typetree alone: the root's last field `mSprites` is a vector
    (helpers/TypeTreeHelper.py:182-263) whose element is a struct with integer `x`, `y`, `width` and `height`
    and a `string` `name`, the last child of each name counting, as the dict `read_value` builds keeps it."""
    field = next((c for c in reversed(root.m_Children) if c.m_Name == "mSprites"), None)
    if field is None or not _vector(field) or len(field.m_Children[0].m_Children) < 2:
        return False
    element = field.m_Children[0].m_Children[1]
    if not _plain(element) or _vector(element):
        return False
    named = {c.m_Name: c.m_Type for c in element.m_Children}
    return all(named.get(k) in INTEGER for k in RECT) and named.get("name") == "string"


def recognise(obj, script_classes) -> tuple[str, str | None]:
    """Section 5's recognition: `none` (no atlas, not counted), `unresolved` (`m_Script`'s read failed, counted),
    `no_typetree`, `layout` (`ngui_layout_unsupported`) or `atlas`, with the MonoBehaviour's `m_Name`. `m_Script`
    is read with UnityPy's MonoBehaviour head (files/ObjectReader.py:325-330), which needs no typetree."""
    try:
        head = read(obj.parse_monobehaviour_head)
        script = deref(head.m_Script.m_FileID, head.m_Script.m_PathID, obj.assets_file)
        if script.type != ClassIDType.MonoScript:
            return "none", None
        class_name = read(lambda: script.parse_as_dict()["m_ClassName"])
    except Failure:
        return "unresolved", None
    if class_name not in script_classes:
        return "none", None
    node = obj.serialized_type.node if obj.serialized_type is not None else None
    if node is None:
        return "no_typetree", head.m_Name
    return ("atlas" if sprites_field(node) else "layout"), head.m_Name


def maintex_name(tree: dict, obj) -> str | None:
    """Step 1: the material's `_MainTex` texture's `m_Name`; None for no name, a missing `material` field and
    every object-level failure among them."""
    if "material" not in tree:
        return None
    try:
        material = deref_value(tree["material"], obj.assets_file)
        envs = read(lambda: [tuple(e) for e in material.parse_as_dict()["m_SavedProperties"]["m_TexEnvs"]])
        for env in envs:
            if len(env) != 2:
                raise Failure("decode_error", "read")
            first, second = env
            # The first a string, or a struct whose `name` is (classes/generated.py:11477).
            if first == "_MainTex" or (isinstance(first, dict) and first.get("name") == "_MainTex"):
                texture = deref_value(read(lambda second=second: second["m_Texture"]), material.assets_file)
                name = read(texture.peek_name)
                return name if isinstance(name, str) else None
    except Failure:
        return None
    return None


# ------------------------------------------------------------------ a texture's answers (BATCH_SPEC section 3)


def format_name(value: int) -> str:
    try:
        return TextureFormat(value).name
    except ValueError:
        return str(value)


def selection(texture, platform: int) -> str | None:
    """Section 3's answers 1 to 3, the skips that need no pixels, with UnityPy's predicates: a side of 0
    (export/Texture2DConverter.py:265-266), the format, then the platform (helpers/TextureSwizzler.py:121-127,
    export/Texture2DConverter.py:569)."""
    if not texture.m_Width or not texture.m_Height:
        return "empty_image"
    name = format_name(texture.m_TextureFormat)
    if name not in FIVE:
        return f"unsupported_format({name})"
    if TextureSwizzler.is_switch_swizzled(platform, texture.m_PlatformBlob):
        return "unsupported_platform(switch)"
    swapped = TextureFormat(texture.m_TextureFormat) in Texture2DConverter.XBOX_SWAP_FORMATS
    if platform == BuildTarget.XBOX360 and swapped:
        return "unsupported_platform(xbox360)"
    return None


def candidates(path: str) -> list[str]:
    """UnityPy's names for a resource path, in order (helpers/ResourceReader.py:10-33)."""
    basename = ntpath.basename(path)
    name, _ = ntpath.splitext(basename)
    return [basename, f"{name}.resource", f"{name}.assets.resS", f"{name}.resS"]


def _row(f, offset: int, size: int) -> bytes | None:
    """A matched row: a raw one's bytes, a SerializedFile's failure (`not_a_reader`), a container's None."""
    if isinstance(f, SerializedFile):
        raise Failure("decode_error", "malformed")
    if isinstance(f, (BundleFile, WebFile)):
        return None
    return bytes(_get_resource_data(f, offset, size))


def resource(sf: SerializedFile, path: str, offset: int, size: int) -> bytes:
    """4.3's own-chain procedure: for each candidate in order, the containers that hold `sf`, innermost first,
    each one's matching top-level rows (by registry key) last to first, a container row passed over; then the
    registry (`batch.md` 2.6 step 8); else `outside_view(<the path as read>)`."""
    for name in candidates(path):
        key = simplify_name(name)
        container = sf.parent
        while isinstance(container, (BundleFile, WebFile)):
            for row, f in reversed(list(container.files.items())):
                if simplify_name(row) == key and (data := _row(f, offset, size)) is not None:
                    return data
            container = container.parent
    for name in candidates(path):
        f = sf.environment.get_cab(name)
        if f is not None and (data := _row(f, offset, size)) is not None:
            return data
    raise Failure(f"outside_view({path})")


def data_of(texture, sf: SerializedFile) -> bytes:
    """Section 5's sheet data: `image_data`, else the stream's resource; empty with no stream or an empty path."""
    if texture.image_data:
        return bytes(texture.image_data)
    stream = getattr(texture, "m_StreamData", None)
    if stream is None or not stream.path:
        return b""
    return resource(sf, stream.path, stream.offset, stream.size)


def decode(texture, obj):
    """Section 3's answers 4 to 10, then the image `Texture2D.image` gives (classes/legacy_patch/Texture2D.py:8-11),
    from the data `data_of` reads; a skip is a `Failure`."""
    data = data_of(texture, obj.assets_file)
    width, height = texture.m_Width, texture.m_Height
    if not data:
        raise Failure("decode_error", "no_image_data")
    if width < 0 or height < 0:
        raise Failure("decode_error", "negative_size")
    fmt = TextureFormat(texture.m_TextureFormat)
    padded = Texture2DConverter.get_compressed_image_size(width, height, fmt)
    if padded[0] * padded[1] > TEXTURE_PIXELS:
        raise Failure(f"limit(texture_pixels, {TEXTURE_PIXELS})")
    # As `get_image_from_texture2d` passes them (export/Texture2DConverter.py:237-251).
    reader = texture.object_reader
    version = getattr(reader, "version", (0, 0, 0, 0))
    platform = getattr(reader, "platform", BuildTarget.UnknownPlatform)
    blob = getattr(texture, "m_PlatformBlob", None)
    try:
        return Texture2DConverter.parse_image_data(data, width, height, fmt, version, platform, blob)
    except MemoryError:
        if width > RAW_WIDTH_CAP.get(fmt.name, width):
            raise Failure("decode_error", "pillow_memory_error") from None
        raise
    except Image.DecompressionBombError:
        raise Failure("decode_error", "decompression_bomb") from None
    except ValueError as e:
        if str(e) != "not enough image data":
            raise
        raise Failure("decode_error", "not_enough_data") from None


def crop_refusal(x: int, y: int, width: int, height: int) -> str | None:
    """Section 5: a crop Pillow 9.5.0 refuses, decided from the rectangle alone. `Image._crop` checks the area
    first, then its C crop parses each edge as an `int`."""
    if width * height > CROP_PIXELS:
        return "decompression_bomb"
    if x < INT_MIN or y < INT_MIN or x + width > INT_MAX or y + height > INT_MAX:
        return "crop_overflow"
    return None
