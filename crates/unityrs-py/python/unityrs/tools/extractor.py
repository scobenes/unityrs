# Ported from UnityPy 1.25.2 (MIT): tools/extractor.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""UnityPy-compatible extractor calls with confined local output writes.

Sources: tools/extractor.py:25-78, tools/extractor.py:81-157,
tools/extractor.py:165-174, tools/extractor.py:177-187,
tools/extractor.py:190-197, tools/extractor.py:200-207,
tools/extractor.py:210-224, tools/extractor.py:227-247,
tools/extractor.py:250-268, tools/extractor.py:271-281,
tools/extractor.py:284-304, tools/extractor.py:307-324,
tools/extractor.py:327-358 and tools/extractor.py:361-368.
"""
import json
import os
from io import BytesIO
from pathlib import Path
from typing import Callable, Dict, Iterable, List, Optional, Sequence, Tuple, Union

import unityrs as UnityPy
from ..classes import (AudioClip, Font, GameObject, Mesh, MonoBehaviour, Object,
                       PPtr, Shader, Sprite, TextAsset, Texture2D)
from ..enums.ClassIDType import ClassIDType
from ..files import ObjectReader, SerializedFile
from .._native import LimitError, TreeTraversal
from .._buffers import snapshot_bytes
from . import _paths
from ._bounded import guard_for, json_bound, text


def export_obj(
    obj: Union[ObjectReader, PPtr], fp: str, append_name: bool = False,
    append_path_id: bool = False, export_unknown_as_typetree: bool = False,
    asset_filter: Optional[Callable[[Object], bool]] = None,
) -> List[Tuple[SerializedFile, int]]:
    export_func = EXPORT_TYPES.get(obj.type)
    if not export_func:
        if export_unknown_as_typetree:
            export_func = exportMonoBehaviour
        else:
            return []
    if isinstance(obj, PPtr):
        obj = obj.deref()
    guard = guard_for(obj)
    instance = obj.parse_as_object()
    if asset_filter and not asset_filter(instance):
        return []
    text(guard, fp)
    if append_name:
        name = getattr(instance, "m_Name", obj.type.name)
        name = _paths.relative(name, guard)
        guard.step(0, (len(fp) + len(name)) * 8 + 256, 1)
        fp = os.path.join(fp, name)
    fp, extension = os.path.splitext(fp)
    if append_path_id:
        guard.step(0, len(fp) * 8 + 512, 1)
        identity = _paths.relative(f"{obj.m_PathID}", guard)
        fp = f"{fp}_{identity}"
    return export_func(instance, fp, extension)


def extract_assets(
    src: Union[Path, BytesIO, bytes, bytearray], dst: Path,
    use_container: bool = True, ignore_first_container_dirs: int = 0,
    append_path_id: bool = False, export_unknown_as_typetree: bool = False,
    asset_filter: Optional[Callable[[Object], bool]] = None,
) -> List[Tuple[SerializedFile, int]]:
    env = UnityPy.load(src)
    guard = TreeTraversal(getattr(env, "_limits", None), getattr(env, "_budget", None))
    guard.step(0, 512 + len(EXPORT_TYPES) * 128, len(EXPORT_TYPES))
    exported = []
    export_types_keys = list(EXPORT_TYPES.keys())

    def defaulted_export_index(type):
        try:
            return export_types_keys.index(type)
        except (IndexError, ValueError):
            return 999

    dst = os.fspath(dst)
    values = env.container if use_container else env.objects
    guard.step(0, len(values) * 512 + 256, len(values))
    if use_container:
        values = sorted(values.items(), key=lambda x: defaulted_export_index(x[1].type))
    else:
        values = sorted(values, key=lambda x: defaulted_export_index(x.type))
    # Empty sources do not open the destination in the original extractor.
    if not values:
        return exported
    with _paths.root(dst, guard, create=use_container):
        for entry in values:
            obj_path, obj = entry if use_container else (None, entry)
            if asset_filter is not None and not asset_filter(obj):
                continue
            if use_container:
                obj_path = _paths.relative(obj_path, guard)
                guard.step(0, len(obj_path) * 512 + len(dst) * 8 + 256, len(obj_path))
                parts = [x for x in obj_path.split("/")[ignore_first_container_dirs:] if x]
                if not parts:
                    raise ValueError("unsafe asset output name")
                obj_dest = os.path.join(dst, *parts)
                _paths.mkdir(os.path.dirname(obj_dest), guard)
            else:
                guard.step(0, 0, len(exported))
                if (obj.assets_file, obj.path_id) in exported:
                    continue
                obj_dest = dst
            result = export_obj(obj, obj_dest, append_name=not use_container,
                                append_path_id=append_path_id,
                                export_unknown_as_typetree=export_unknown_as_typetree,
                                asset_filter=asset_filter)
            guard.step(0, len(result) * 256, len(result))
            exported.extend(result)
    return exported


def exportTextAsset(obj: Union[TextAsset, ObjectReader], fp: str,
                    extension: str = ".txt") -> List[Tuple[SerializedFile, int]]:
    if isinstance(obj, ObjectReader):
        obj = obj.parse_as_object()
    if not extension:
        extension = ".txt"
    guard = guard_for(obj)
    text(guard, obj.m_Script)
    with _paths.file(f"{fp}{extension}", "wb", guard) as output:
        output.write(obj.m_Script.encode("utf-8", "surrogateescape"))
    return [(obj.assets_file, obj.object_reader.path_id)]


def exportFont(obj: Union[Font, ObjectReader], fp: str,
               extension: str = "") -> List[Tuple[SerializedFile, int]]:
    if isinstance(obj, ObjectReader):
        obj = obj.parse_as_object()
    if obj.m_FontData:
        guard = guard_for(obj)
        guard.step(0, 256, 1)
        extension = ".otf" if obj.m_FontData[0:4] == b"OTTO" else ".ttf"
        with _paths.file(f"{fp}{extension}", "wb", guard) as output:
            output.write(snapshot_bytes(obj.m_FontData, guard))
    return [(obj.assets_file, obj.object_reader.path_id)]


def exportMesh(obj: Union[Mesh, ObjectReader], fp: str, extension=".obj") -> List[Tuple[SerializedFile, int]]:
    if isinstance(obj, ObjectReader):
        obj = obj.parse_as_object()
    if not extension:
        extension = ".obj"
    guard = guard_for(obj)
    with _paths.file(f"{fp}{extension}", "wt", guard, encoding="utf8", newline="") as output:
        value = obj.export()
        text(guard, value)
        output.write(value)
    return [(obj.assets_file, obj.object_reader.path_id)]


def exportShader(obj: Union[Shader, ObjectReader], fp: str, extension=".txt") -> List[Tuple[SerializedFile, int]]:
    if isinstance(obj, ObjectReader):
        obj = obj.parse_as_object()
    if not extension:
        extension = ".txt"
    guard = guard_for(obj)
    with _paths.file(f"{fp}{extension}", "wt", guard, encoding="utf8", newline="") as output:
        value = obj.export()
        text(guard, value)
        output.write(value)
    return [(obj.assets_file, obj.object_reader.path_id)]


def exportMonoBehaviour(obj: Union[MonoBehaviour, ObjectReader], fp: str,
                        extension: str = "") -> List[Tuple[SerializedFile, int]]:
    reader = obj.object_reader if isinstance(obj, MonoBehaviour) else obj
    guard = guard_for(reader)
    try:
        export = reader.parse_as_dict()
        extension = ".json"
        json_bound(guard, export)
        export = json.dumps(export, indent=4, ensure_ascii=False).encode("utf8", errors="surrogateescape")
    except (LimitError, MemoryError):
        raise
    except Exception:
        extension = ".bin"
        export = reader.get_raw_data()
    with _paths.file(f"{fp}{extension}", "wb", guard) as output:
        output.write(export)
    return [(obj.assets_file, obj.path_id)]


def exportAudioClip(obj: Union[AudioClip, ObjectReader], fp: str,
                    extension: str = "") -> List[Tuple[SerializedFile, int]]:
    if isinstance(obj, ObjectReader):
        clip = obj.parse_as_object()
    else:
        clip = obj
        obj = clip.object_reader
    guard = guard_for(obj)
    samples = clip.samples
    if len(samples) == 1:
        with _paths.file(f"{fp}.wav", "wb", guard) as output:
            output.write(next(iter(samples.values())))
    elif len(samples) > 1:
        pending = []
        for name, clip_data in samples.items():
            guard.step(0, 128, 1)
            pending.append((_paths.relative(name, guard), clip_data))
        _paths.mkdir(fp, guard)
        for name, clip_data in pending:
            text(guard, name)
            with _paths.file(os.path.join(fp, f"{name}.wav"), "wb", guard) as output:
                output.write(clip_data)
    return [(obj.assets_file, obj.path_id)]


def exportSprite(obj: Union[Sprite, ObjectReader], fp: str,
                 extension: str = ".png") -> List[Tuple[SerializedFile, int]]:
    if isinstance(obj, ObjectReader):
        sprite = obj.parse_as_object()
    else:
        sprite = obj
        obj = sprite.object_reader
    _paths.image(sprite.image, f"{fp}{extension}", guard_for(obj))
    exported = [(obj.assets_file, obj.path_id),
                (sprite.m_RD.texture.assetsfile, sprite.m_RD.texture.path_id)]
    alpha_assets_file = getattr(sprite.m_RD.alphaTexture, "assets_file", None)
    alpha_path_id = getattr(sprite.m_RD.alphaTexture, "path_id", None)
    if alpha_path_id and alpha_assets_file:
        exported.append((alpha_assets_file, alpha_path_id))
    return exported


def exportTexture2D(obj: Union[Texture2D, ObjectReader], fp: str,
                    extension: str = ".png") -> List[Tuple[SerializedFile, int]]:
    if isinstance(obj, ObjectReader):
        obj = obj.parse_as_object()
    if not extension:
        extension = ".png"
    if obj.m_Width:
        _paths.image(obj.image, f"{fp}{extension}", guard_for(obj))
    return [(obj.assets_file, obj.object_reader.path_id)]


def exportGameObject(obj: Union[GameObject, ObjectReader], fp: str,
                     extension: str = "") -> List[Tuple[SerializedFile, int]]:
    if isinstance(obj, ObjectReader):
        obj = obj.parse_as_object()
    guard = guard_for(obj)
    exported = [(obj.assets_file, obj.object_reader.path_id)]
    refs = crawl_obj(obj)
    if refs:
        _paths.mkdir(fp, guard)
    for ref_id, ref in refs.items():
        guard.step(0, 256, len(exported) + 1)
        if (ref.assets_file, ref_id) in exported or ref.type == ClassIDType.GameObject:
            continue
        try:
            result = export_obj(ref, fp, True, True)
            guard.step(0, len(result) * 256, len(result))
            exported.extend(result)
        except (LimitError, MemoryError):
            raise
        except Exception as error:
            print(f"Failed to export {ref_id}")
            print(error)
    return exported


EXPORT_TYPES = {
    ClassIDType.GameObject: exportGameObject,
    ClassIDType.Sprite: exportSprite,
    ClassIDType.AudioClip: exportAudioClip,
    ClassIDType.Font: exportFont,
    ClassIDType.Mesh: exportMesh,
    ClassIDType.MonoBehaviour: exportMonoBehaviour,
    ClassIDType.Shader: exportShader,
    ClassIDType.TextAsset: exportTextAsset,
    ClassIDType.Texture2D: exportTexture2D,
}
ASSEMBLY_NAME_DLL = str
CLASS_NAME = str
MONOBEHAVIOUR_TYPETREES: Dict[ASSEMBLY_NAME_DLL, Dict[CLASS_NAME, List[Dict]]] = {}


def crawl_obj(obj: Union[Object, ObjectReader, PPtr], ret: Optional[dict] = None) -> Dict[int, Union[Object, PPtr]]:
    if not ret:
        ret = {}
    guard = guard_for(obj)
    guard.step(0, 512, 1)
    pending = [(obj, 0)]
    while pending:
        obj, depth = pending.pop()
        guard.step(depth, 512, 1)
        if depth and obj.m_PathID in ret:
            continue
        if isinstance(obj, PPtr):
            if obj.m_PathID == 0 and obj.m_FileID == 0 and obj.m_Index == -2:
                continue
            try:
                values = obj.deref_parse_as_dict().values()
            except AttributeError:
                continue
        elif isinstance(obj, Object):
            values = obj.__dict__.values()
        elif isinstance(obj, ObjectReader):
            values = obj.parse_as_dict().values()
        else:
            continue
        guard.step(depth, 256, 1)
        ret[obj.m_PathID] = obj
        children = []
        for value in _flatten(values, guard):
            if isinstance(value, (Object, PPtr)) and value.m_PathID not in ret:
                guard.step(depth, 256, 1)
                children.append(value)
        for value in reversed(children):
            pending.append((value, depth + 1))
    return ret


def flatten(seq: Sequence) -> Iterable:
    yield from _flatten(seq, TreeTraversal())


def _flatten(seq, guard):
    def snapshot(values, depth):
        if hasattr(values, "__len__"):
            guard.step(depth, len(values) * 16 + 256, len(values))
            return list(values)
        result = []
        for value in values:
            guard.step(depth, 32, 1)
            result.append(value)
        return result

    guard.step(0, 512, 1)
    pending = [(iter(snapshot(seq, 0)), id(seq))]
    active = {id(seq)}
    while pending:
        iterator, marker = pending[-1]
        try:
            elem = next(iterator)
        except StopIteration:
            pending.pop()
            active.discard(marker)
            continue
        guard.step(len(pending) - 1, 128, 1)
        if isinstance(elem, (list, tuple, dict)):
            if id(elem) in active:
                raise ValueError("Extractor sequence contains a cycle")
            active.add(id(elem))
            values = elem.values() if isinstance(elem, dict) else elem
            # Snapshot each level as UnityPy does, charging unsized protocols
            # one element at a time before retaining it.
            pending.append((iter(snapshot(values, len(pending))), id(elem)))
        else:
            yield elem
