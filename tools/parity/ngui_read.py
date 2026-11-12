"""The reads of the NGUI cutter's reference (`ngui_cutter.py`): docs/BATCH_SPEC.md section 5's recognition and sheet
reads, each failure classified as docs/plans/batch.md 2.7 classifies it.

**The Environment** (4.3) registers the input's view, from its bytes: the other bundles with `is_dependency=True` in
bytewise order of id, then the input. While it loads, each container's directory rows are kept, in directory order,
each with the object UnityPy's reader made of it (`File.read_files`, files/File.py:76-90, wrapped to record what it
assigns): UnityPy's `files` dict keeps one object per path, a repeated path's last in its first row's place, and 4.3
walks the rows. From the rows it builds 4.3's registry, as the core keeps it: each top-level bundle in load order,
each row in directory order under its registry key (`simplify_name`), a top-level row's own name a key at its place,
a bundle's entries after it (Lazy difference (b)). Inside a reached bundle every row but a bundle is a key, whatever
its kind, a WebFile among them; a bundle there is no key and its entries are registered; no WebFile's entries are
(the core's reach, crates/unityrs-core/src/env/registry/reach.rs). A key's value is its last registration. Then it
is sealed: `find_file` answers from that registry, a bundle's key not found (the core's `NotFound`) and a WebFile's
`decode_error`, `unsupported` (version 1's core refuses to open a WebFile), and UnityPy's lookups past its registry
(`find_file`'s check of `<path>/<name>`, which with an empty `path` is the host's root, and the files `load_file`
finds relative to the working directory, which `load_dependencies` asks for) are replaced by a miss, and its
filesystem by one that refuses every call. So a name the view does not register is not found, whatever is on the
host; a CAB whose key a later bundle row takes is not found either, where UnityPy's `get_cab`, which gives a bundle
no key, would answer with it; and one a later WebFile row takes fails `unsupported`.

**Pointers** are dereferenced by UnityPy's `PPtr.deref` (classes/PPtr.py:43-90), which looks in the reading file's
own bundle, then `find_file`, the registry above. Its failures are batch.md 2.7's `DerefError`s. A null pointer,
an `m_FileID` whose index `m_FileID - 1` is past the externals or below their start (a negative index counts from
the end, as Python's list and the core's `external_index` do), a missing object, a raw or bundle row of the
reading file's own bundle and a raw row the registry answers are `bad_pointer`; a bundle the registry answers, and a
CAB the view does not register, are `outside_view(<cab>)`; a WebFile row, of the reading file's own bundle or the
registry's, is `decode_error`, `unsupported` (version 1's core refuses to open one).

**Object reads** go through UnityPy (UnityPyBoost's reader, or UnityPy's own for an object of no bytes): a read
that runs out or meets a value of the wrong shape is a `ParseError`'s `Read`, `decode_error` with the code `read`.
An object whose type has no typetree is not read: version 1 configures no typetree provider (batch.md A8), so the
core's read is `NoTypeTree`, code `asset_bundle`, where UnityPy would read it by its TPK. The atlas, texture, Material
and MonoScript reads use UnityPy's `check_read`, its default: a read that does not end where the object's bytes do
fails the check (`helpers/TypeTreeHelper.py:155-156`), which is the core's `ParseError::CheckRead`, code
`asset_bundle` (BATCH_SPEC 7.3). That failure is told apart from every other `ValueError` by the check itself:
the same read without it succeeds. Names and the MonoBehaviour's head are read without the check (`name_of`, as
`peek_name` reads a name, and `parse_monobehaviour_head`). A peeked name of an object whose type has no typetree
fails as its object read does (`asset_bundle`): the core reads such a name through the object's typetree, and has
none without a provider. A MonoBehaviour's head name needs no typetree. UnityPy's `TypeTreeError` is never one: its
one raise passes one argument to a class that takes two (files/ObjectReader.py:306), so it surfaces as a
`TypeError`, a `read`, and no read here is `tree`.
Only UnityPy's calls are wrapped; the values they return are checked for the shape the reads need, a wrong one the
read's failure (4.4 step 2's rule, in its order). `MemoryError` and anything else are not classified: they
propagate, as the engine stops on a host's refusal or an unknown error.

UnityPy's `peek_name` caches, for the process, the nodes up to `m_Name` (or `name`) by the root's name, type and
version, and caches a type with no name field as `None` too (helpers/TypeTreeNode.py:190-211). So the name read here
(`name_of`) takes no node from that cache: it reads the name as `peek_name` does, over the object's own nodes, and
no answer depends on the order objects are read in.

**A sheet's data** (section 5) is its `image_data`, else the `m_StreamData` resource that 4.3's own-chain procedure
finds: the spec's lookup, written here over the rows kept at the load and UnityPy's candidate names
(helpers/ResourceReader.py:10-33), reading the row with UnityPy's own `_get_resource_data`; the view's registry, built
from the rows, only when no own row answers, each candidate in order answered by its key's value, a bundle passing
to the next candidate and a WebFile failing the read, `decode_error`, `unsupported` (the core's `resource_try`,
crates/unityrs-core/src/env/resource_data.rs), where UnityPy's `get_cab`, which gives a bundle no key, would answer
with an earlier row. The own chain meets no WebFile row: the input's own rows are the only ones it walks, and an
input with one is out of scope (`Divergence`). The bytes go to UnityPy's
`parse_image_data`, as `Texture2D.image` gives them to it, so the image is UnityPy's and Pillow's, after section 3's
answers 5 to 10 in their order. Answer 11, the PNG encoder's width cap, is the texture output's own (`encoder_refusal`,
asked where a texture's PNG would be made), not the image's: a sheet too wide to write is still cut.
"""

from __future__ import annotations

import ntpath
import struct
from contextlib import contextmanager

import UnityPy
from PIL import Image
from UnityPy.classes import PPtr, Texture2D
from UnityPy.enums import BuildTarget, ClassIDType, TextureFormat
from UnityPy.environment import simplify_name
from UnityPy.export import Texture2DConverter
from UnityPy.files import BundleFile, File, SerializedFile, WebFile
from UnityPy.helpers import TextureSwizzler
from UnityPy.helpers.ResourceReader import _get_resource_data
from UnityPy.helpers.TypeTreeHelper import FUNCTION_READ_MAP
from UnityPy.helpers.TypeTreeNode import TypeTreeNode

FIVE = ("RGB24", "RGBA32", "DXT1", "DXT5", "BC7")
TEXTURE_PIXELS = 268_435_456
INT_MIN, INT_MAX = -(2**31), 2**31 - 1
# Pillow's raw decoder refuses a row of more than `INT_MAX / bits - 7` pixels (BATCH_SPEC section 3's answer 8).
RAW_WIDTH_CAP = {"RGBA32": INT_MAX // 32 - 7, "RGB24": INT_MAX // 24 - 7}
# Pillow's PNG encoder refuses a row of more than `INT_MAX / bits - 7` pixels too, raising `MemoryError` at the save
# (`src/encode.c`), by the image's mode (section 3's answer 11 and section 5's crops).
ENCODER_WIDTH_CAP = {"RGBA": INT_MAX // 32 - 7, "RGB": INT_MAX // 24 - 7}
# `Image._crop`'s decompression-bomb check refuses a crop of more pixels than this (section 5's crops).
CROP_PIXELS = 2 * Image.MAX_IMAGE_PIXELS
# The typetree types `FUNCTION_READ_MAP` reads as an integer (helpers/TypeTreeHelper.py:29-52): all but `float`,
# `double`, `bool`, `string` and `TypelessData`.
INTEGER = frozenset(FUNCTION_READ_MAP) - {"float", "double", "bool", "string", "TypelessData"}
RECT = ("x", "y", "width", "height")
# What UnityPy's object reads raise when the bytes run out or a value has the wrong shape: `struct.error` from
# UnityPy's own reader (an object of no bytes skips UnityPyBoost, helpers/TypeTreeHelper.py:146-153), the others from
# either reader.
READ_FAILURES = (EOFError, ValueError, IndexError, KeyError, TypeError, AttributeError, struct.error)
# Where each container's directory rows are kept at the load, and the Environment's registry built from them.
ROWS = "unityrs_rows"
REGISTRY = "unityrs_registry"


class Failure(Exception):
    """A read's failure as batch.md 2.7 classifies it: an object's skip, its reason and detail."""

    def __init__(self, reason: str, detail: str | None = None):
        super().__init__(reason if detail is None else f"{reason}, {detail}")
        self.reason, self.detail = reason, detail


READ = ("decode_error", "read")


class Divergence(Exception):
    """A case the reference knowingly does not model, where the engine and UnityPy part: not a `Failure`, so it
    propagates from `cut` unclassified, and any harness running the reference sees an exception, not a record. Two
    cases raise it: a view member or input that UnityPy does not parse as a `BundleFile` (`load`), which the engine
    fails at its peek or at selection (`not_a_bundle`, or the open's code: a WebFile's `unsupported`), where UnityPy
    goes on; and an input with a top-level WebFile row (`ngui_cutter.cut`)."""


class _Sealed:
    """The sealed Environment's filesystem: any call is a bug of the seal, never an answer."""

    def __getattr__(self, name):
        raise RuntimeError(f"the reference's Environment is sealed to its view (fs.{name})")


def bytewise(text: str) -> bytes:
    return text.encode("utf-8")


def _recording(read_files):
    """`File.read_files`, keeping each directory row's name and object in directory order on the container."""

    def wrapper(self, reader, nodes):
        rows = []

        class Recording(dict):
            def __setitem__(inner, name, f):
                rows.append((name, f))
                super().__setitem__(name, f)

        self.files = Recording(self.files)
        try:
            read_files(self, reader, nodes)
        finally:
            self.files = dict(self.files)
        setattr(self, ROWS, rows)

    return wrapper


def load(view: dict[str, bytes], input_id: str):
    """The input's bundle, in an Environment of its view, its containers' rows kept, sealed; `Divergence` for a
    view member or the input that UnityPy does not parse as a bundle."""
    env = UnityPy.Environment(path="")
    original = File.read_files
    File.read_files = _recording(original)
    loaded = []
    try:
        for bid in sorted((b for b in view if b != input_id), key=bytewise):
            loaded.append((bid, env.load_file(view[bid], name=bid, is_dependency=True)))
        bundle = env.load_file(view[input_id], name=input_id)
        loaded.append((input_id, bundle))
    finally:
        File.read_files = original
    for bid, f in loaded:
        if not isinstance(f, BundleFile):
            raise Divergence(f"{bid}: UnityPy gives a {type(f).__name__}, which the engine fails (not a bundle)")
    setattr(env, REGISTRY, registry(env.files.values()))
    env.find_file = lambda name, is_dependency=True: found(env, name)
    env.load_file = lambda *args, **kwargs: None
    env.fs = _Sealed()
    return bundle


def rows(container) -> list:
    """A container's directory rows, `(path, object)`, in directory order, as the load kept them."""
    return getattr(container, ROWS)


def registry(loaded) -> dict:
    """4.3's registry of the loaded top-level bundles, in load order: every row by its registry key, a key's value
    its last registration (the core's greatest rank). A top-level row's own name is a key, a bundle's entries
    registered after it; inside a reached bundle every row but a bundle is a key, a WebFile among them, and a bundle
    there is no key, its entries registered. No WebFile's entries are registered."""
    keys = {}

    def walk(bundle, top: bool):
        for path, f in rows(bundle):
            if top or not isinstance(f, BundleFile):
                keys[simplify_name(path)] = f
            if isinstance(f, BundleFile):
                walk(f, False)

    for f in loaded:
        if isinstance(f, BundleFile):
            walk(f, True)
    return keys


def found(env, name: str):
    """`find_file`, sealed to 4.3's registry: its key's value; None for a bundle (the core's `NotFound`); a WebFile's
    failure (`unsupported`: version 1's core refuses to open one)."""
    f = getattr(env, REGISTRY).get(simplify_name(name))
    if isinstance(f, WebFile):
        raise Failure("decode_error", "unsupported")
    return None if isinstance(f, BundleFile) else f


def typed(obj):
    """`obj`, when its type has a typetree; else its read's failure, the core's `NoTypeTree` (`asset_bundle`)."""
    if obj.serialized_type is None or obj.serialized_type.node is None:
        raise Failure("decode_error", "asset_bundle")
    return obj


def name_of(obj) -> str:
    """4.4 step 1's name read, in its order: no typetree is its object read's failure (`typed`); a root with no
    `m_Name` or `name` child, and a value not a string, the read's. The name is read as `peek_name` reads it
    (files/ObjectReader.py:189-197), by UnityPy's own `parse_as_dict` without the check, over the root's children up
    to the name's, but from the object's own nodes: `peek_name` takes them from its process-wide cache."""
    node = typed(obj).serialized_type.node
    at = next((i for i, child in enumerate(node.m_Children) if child.m_Name in ("m_Name", "name")), None)
    if at is None:
        raise Failure(*READ)
    peek = TypeTreeNode(
        node.m_Level, node.m_Type, node.m_Name, node.m_ByteSize, node.m_Version, node.m_Children[: at + 1]
    )
    return field(read(lambda: obj.parse_as_dict(peek, check_read=False)), node.m_Children[at].m_Name, str)


@contextmanager
def sealed_images():
    """Refuse every Texture2D image assignment during a reference's reads and rendering; restore on exit."""
    # UnityPy 1.25.2's setter (classes/legacy_patch/Texture2D.py:78) delegates to set_image, whose non-image
    # branch opens the value (classes/legacy_patch/Texture2D.py:25-26). A sealed attempt never opens a host path
    # a file's data names. Raise instead, as an invalid non-image value does, at every construction, including
    # nested fields and direct Sprite/atlas reads. UnityPy can fall back to UnknownObject; unused fields stay
    # unused. Preserve the getter for SpriteHelper's image reads, and restore even on failure or nested use.
    original = Texture2D.image

    def refuse(self, value):
        raise TypeError("Texture2D.image assignment is refused in a sealed reference")

    Texture2D.image = original.setter(refuse)
    try:
        yield
    finally:
        Texture2D.image = original


@sealed_images()
def read(f, *args):
    """A UnityPy read: its value, or its `Failure`."""
    try:
        return f(*args)
    except READ_FAILURES as e:
        raise Failure(*READ) from e


@sealed_images()
def checked(method):
    """A UnityPy object read with `check_read`, as UnityPy's default: `method(check_read=True)`'s value, or its
    `Failure`. A `ValueError` is the check's when the same read without it succeeds: `asset_bundle`."""
    try:
        return method(check_read=True)
    except ValueError as e:
        try:
            method(check_read=False)
        except READ_FAILURES:
            raise Failure(*READ) from e
        raise Failure("decode_error", "asset_bundle") from e
    except READ_FAILURES as e:
        raise Failure(*READ) from e


def shaped(value, *types):
    """A read value, or the read's failure when it is not of `types`."""
    if not isinstance(value, types) or isinstance(value, bool) and bool not in types:
        raise Failure(*READ)
    return value


def field(tree, name: str, *types):
    """A field of a read dict, checked for `types`; a missing one, or a value not a dict, is the read's failure."""
    return shaped(shaped(tree, dict).get(name), *types)


def sibling(file_id: int, assets_file):
    """The row of the reading file's own bundle that `PPtr.deref` takes for an external (classes/PPtr.py:68-78:
    the external's path less `archive:/` and `assets/`, its last part, against each `files` key, lower-cased), or
    None."""
    container = assets_file.parent
    if file_id == 0 or container is None:
        return None
    path = assets_file.externals[file_id - 1].path
    path = path[9:] if path.startswith("archive:/") else path
    path = path[7:] if path.startswith("assets/") else path
    name = path.rsplit("/")[-1].lower()
    return next((f for key, f in container.files.items() if key.lower() == name), None)


def deref(file_id: int, path_id: int, assets_file):
    """The object a pointer of `assets_file` names, or its `DerefError` as a `Failure`: a WebFile sibling, as a
    WebFile the registry answers (`found`), `decode_error`, `unsupported`."""
    n = len(assets_file.externals)
    if path_id == 0 or (file_id != 0 and not -n <= file_id - 1 < n):
        raise Failure("bad_pointer")  # Null, FileIdBelowStart, FileIdPastEnd
    if isinstance(sibling(file_id, assets_file), WebFile):
        raise Failure("decode_error", "unsupported")
    try:
        return PPtr(m_FileID=file_id, m_PathID=path_id, assetsfile=assets_file).deref()
    except FileNotFoundError as e:  # the sealed registry: NotFound
        raise Failure(f"outside_view({assets_file.externals[file_id - 1].path.rsplit('/')[-1]})") from e
    except (KeyError, AttributeError) as e:  # MissingObject; RawTarget, a row with no objects
        raise Failure("bad_pointer") from e


def deref_value(pointer, assets_file):
    """A typetree pointer value dereferenced from the file that holds it."""
    return deref(field(pointer, "m_FileID", int), field(pointer, "m_PathID", int), assets_file)


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
    field_node = next((c for c in reversed(root.m_Children) if c.m_Name == "mSprites"), None)
    if field_node is None or not _vector(field_node) or len(field_node.m_Children[0].m_Children) < 2:
        return False
    element = field_node.m_Children[0].m_Children[1]
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
        pointer = shaped(getattr(head, "m_Script", None), PPtr)
        name = shaped(getattr(head, "m_Name", None), str)
        script = deref(shaped(pointer.m_FileID, int), shaped(pointer.m_PathID, int), obj.assets_file)
        if script.type != ClassIDType.MonoScript:
            return "none", None
        class_name = field(checked(typed(script).parse_as_dict), "m_ClassName", str)
    except Failure:
        return "unresolved", None
    if class_name not in script_classes:
        return "none", None
    node = obj.serialized_type.node if obj.serialized_type is not None else None
    if node is None:
        return "no_typetree", name
    return ("atlas" if sprites_field(node) else "layout"), name


def atlas_tree(obj) -> dict:
    """The atlas's own read, whole (section 5's "Skipped atlases"): its typetree's value, checked."""
    tree = checked(obj.parse_as_dict)
    field(tree, "mSprites", list)
    return tree


def maintex_name(tree: dict, obj) -> str | None:
    """Step 1: the material's `_MainTex` texture's `m_Name`, read as 4.4 step 1 reads it (`name_of`); None for no
    name, a missing `material` field and every object-level failure among them. The pairs are read up to the first
    `_MainTex` match; a pair whose first is neither a string nor a struct whose `name` is one is passed over."""
    if "material" not in tree:
        return None
    try:
        material = deref_value(tree["material"], obj.assets_file)
        properties = field(checked(typed(material).parse_as_dict), "m_SavedProperties", dict)
        for env in field(properties, "m_TexEnvs", list):
            if len(shaped(env, tuple, list)) != 2:
                raise Failure(*READ)
            first, second = env
            # The first a string, or a struct whose `name` is (classes/generated.py:11477).
            if first == "_MainTex" or (isinstance(first, dict) and first.get("name") == "_MainTex"):
                texture = deref_value(field(second, "m_Texture", dict), material.assets_file)
                return name_of(texture)
    except Failure:
        return None
    return None


# ------------------------------------------------------------------ a texture's answers (BATCH_SPEC section 3)


# A Texture2D's extra child `image` is set through `Texture2D.image`'s setter (classes/legacy_patch/Texture2D.py:78),
# which opens any value that is no image as a path or a file (classes/legacy_patch/Texture2D.py:25-26, `Image.open`):
# a string or bytes naming an image on the host builds the Texture2D from that file; a path Pillow cannot open fails
# the set, leaving an `UnknownObject`. A sealed attempt never opens a host path a file's data names, so the engine
# refuses every value (`ngui/read.rs`'s `SETTERLESS`), `read`. Both references replace the setter in
# `sealed_images`; this root guard additionally preserves checked-read precedence. A stated divergence
# (PARITY.md, Tier C), which `test_sprites_kind.py` pins against the real setter.
IMAGE = "image"


def texture_of(obj):
    """A Texture2D's object read, in 4.4 step 2's order: its typetree, the checked read, UnityPy's `Texture2D` (a
    typetree that lacks a field the class requires, classes/generated.py:5153-5164, is read as an `UnknownObject`,
    helpers/TypeTreeHelper.py:241-249), integer sides and format, an `m_StreamData`, when it has one, of a string path
    and two integers, `image_data` bytes, and `m_PlatformBlob` a list or absent; each failure but the first two the
    read's (`read`). A root with a child whose cleaned name is `image` is `read` whatever its value (`IMAGE`),
    after its checked dict read, so this policy never invokes the setter or masks a checked read's failure."""
    node = typed(obj).serialized_type.node
    if any(child._clean_name == IMAGE for child in node.m_Children):
        checked(obj.parse_as_dict)
        raise Failure(*READ)
    texture = shaped(checked(obj.read), Texture2D)
    for name in ("m_Width", "m_Height", "m_TextureFormat"):
        shaped(getattr(texture, name, None), int)
    stream = getattr(texture, "m_StreamData", None)
    if stream is not None:
        shaped(getattr(stream, "path", None), str)
        shaped(getattr(stream, "offset", None), int)
        shaped(getattr(stream, "size", None), int)
    shaped(getattr(texture, "image_data", None), bytes)
    if getattr(texture, "m_PlatformBlob", None) is not None:
        shaped(texture.m_PlatformBlob, list)
    return texture


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
    """A matched row: a raw one's bytes, a SerializedFile's failure (`decode_error`, `malformed`: the core's
    `not_a_reader`), a WebFile's (`decode_error`, `unsupported`: version 1's core refuses to open one), a bundle's
    None."""
    if isinstance(f, SerializedFile):
        raise Failure("decode_error", "malformed")
    if isinstance(f, WebFile):
        raise Failure("decode_error", "unsupported")
    if isinstance(f, BundleFile):
        return None
    return bytes(_get_resource_data(f, offset, size))


def resource(sf: SerializedFile, path: str, offset: int, size: int) -> bytes:
    """4.3's own-chain procedure: for each candidate in order, the containers that hold `sf`, innermost first,
    each one's matching top-level directory rows (by registry key) last to first, a bundle row passed over; then
    the view's registry (`batch.md` 2.6 step 8), each candidate in order by its key's value, a bundle passing to the
    next candidate and a WebFile failing the read (`decode_error`, `unsupported`); else `outside_view(<the path as
    read>)`."""
    for name in candidates(path):
        key = simplify_name(name)
        container = sf.parent
        while isinstance(container, (BundleFile, WebFile)):
            for row, f in reversed(rows(container)):
                if simplify_name(row) == key and (data := _row(f, offset, size)) is not None:
                    return data
            container = container.parent
    keys = getattr(sf.environment, REGISTRY)
    for name in candidates(path):
        f = keys.get(simplify_name(name))
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


def mode_of(texture) -> str:
    """The mode of the image of a texture of the five formats: `RGB` for RGB24, else `RGBA`."""
    return "RGB" if TextureFormat(texture.m_TextureFormat).name == "RGB24" else "RGBA"


def encoder_refusal(image) -> str | None:
    """Section 3's answer 11, a texture output's own skip: `pillow_memory_error` when `image` is wider than Pillow's
    PNG encoder takes in its mode, whose save would raise `MemoryError`; asked before the texture's PNG is made, so
    only where one is (a sheet's atlases cut the image whatever its width)."""
    if image.width > ENCODER_WIDTH_CAP.get(image.mode, image.width):
        return "pillow_memory_error"
    return None


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


def crop_refusal(x: int, y: int, width: int, height: int, mode: str = "RGBA") -> str | None:
    """Section 5: a crop Pillow 9.5.0 refuses, decided from the rectangle and the sheet's mode alone, after
    `empty_rect` (both sides positive). `Image._crop` checks the area first (computed exactly: an unsigned 128-bit
    product), then its C crop parses each edge as an `int`; then the PNG encoder refuses a crop wider than it takes in
    `mode` (`pillow_memory_error`), at the save."""
    if width * height > CROP_PIXELS:
        return "decompression_bomb"
    if x < INT_MIN or y < INT_MIN or x + width > INT_MAX or y + height > INT_MAX:
        return "crop_overflow"
    if width > ENCODER_WIDTH_CAP[mode]:
        return "pillow_memory_error"
    return None
