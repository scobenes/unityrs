"""Review regressions for live save snapshots, compared with UnityPy 1.25.2."""

import hashlib
import struct
from types import SimpleNamespace

from binding_cases import S
from binding_y4_save import _asset, _legacy, _load, _Writer


class _ReviewWriter(_Writer):
    def write_byte(self, value):
        self.write(struct.pack("b", value))

    write_bytes = _Writer.write


def _digest(data):
    return hashlib.sha256(data).hexdigest()


def observe_serialized_mapping(module):
    file = _load(module, _asset())
    original = file.objects[1]
    file.objects.clear()
    empty = file.save()
    result = {"empty": [_digest(empty), list(_load(module, empty).objects)]}
    original.path_id = -7
    file.objects[99] = original
    saved = file.save()
    result["reinserted"] = [_digest(saved), list(_load(module, saved).objects)]
    # Dictionary keys do not determine the saved object's path id.
    file.objects = {"unrelated": original, "duplicate": original}
    result["values"] = _digest(file.save())
    return result


def observe_serialized_payload(module):
    file = _load(module, _asset())
    obj = file.objects[1]
    obj.data = struct.pack("<i", 42) + S.s32("assigned")
    saved = file.save()
    result = {"assigned": [_digest(saved), _load(module, saved).objects[1].parse_as_dict()]}
    obj.set_raw_data(struct.pack("<i", 81) + S.s32("set"))
    obj.data = None
    result["reset"] = _digest(file.save())
    obj.data = b""
    result["empty"] = [_digest(file.save()), _load(module, file.save()).objects[1].get_raw_data().hex()]
    return result


def observe_buffer_payloads(module):
    result = {}
    for method in ("assigned", "set_raw_data"):
        for kind in ("mutable", "readonly"):
            file = _load(module, _asset())
            payload = struct.pack("<i", 42) + S.s32("buffer")
            source = bytearray(payload)
            view = memoryview(source if kind == "mutable" else bytes(source))
            try:
                if method == "assigned":
                    file.objects[1].data = view
                else:
                    file.objects[1].set_raw_data(view)
                saved = file.save()
                # Mutating live storage after save must not change its bytes.
                source[0] = 99
                result[method + ":" + kind] = _load(module, saved).objects[1].get_raw_data().hex()
            except Exception as error:
                result[method + ":" + kind] = {"error": type(error).__name__, "message": str(error)}
    return result


def observe_serialized_metadata(module):
    file = _load(module, _asset())
    file.unity_version = "2020.3.5f1"
    file._m_target_platform = 13
    file.userInformation = "review metadata"
    saved = file.save()
    result = {"file": _digest(saved)}
    obj = file.objects[1]
    obj.path_id = -123
    result["object"] = _digest(file.save())
    file.header.reserved = b"xyz"
    result["reserved"] = _digest(file.save())
    file.header.endian = ">"
    file.unknown = 17
    obj.type_id = -1
    result["endian_type_and_unknown"] = _digest(file.save())
    file._enable_type_tree = False
    result["tree_disabled"] = _digest(file.save())
    file.externals = [SimpleNamespace(temp_empty="", guid=b"0123456789abcdef", type=0, path="other.assets")]
    # UnityPy serializes External records through their write method.
    if module.__name__ == "UnityPy":
        from UnityPy.files.SerializedFile import FileIdentifier
        external = object.__new__(FileIdentifier)
        for key, value in vars(file.externals[0]).items():
            setattr(external, key, value)
        file.externals = [external]
    result["external"] = _digest(file.save())
    return result


def _prefix(bundle):
    return (bundle.signature.encode() + b"\0" + struct.pack(">I", bundle.version)
            + bundle.version_player.encode() + b"\0" + bundle.version_engine.encode() + b"\0")


def observe_bundle_headers(module):
    bundle = _load(module, S.fs_bundle([(b"raw.resS", b"resource", 0)]))
    bundle.version_player = "review-version"
    bundle.version_engine = "2018.4.1f1"
    result = {"save": _digest(bundle.save())}
    bundle.signature = "UnityRaw"
    bundle.version = 6
    result["signature"] = _digest(bundle.save())
    bundle.signature = "UnityFS"
    bundle.version = 7
    bundle._uses_block_alignment = True
    result["format"] = _digest(bundle.save())
    bundle.dataflags = type(bundle.dataflags)(192)
    bundle._block_info_flags = 64
    result["original_flags"] = _digest(bundle.save(packer="original"))
    writer = _ReviewWriter(data=_prefix(bundle))
    result["fs_return"] = bundle.save_fs(writer, 64, 64)
    result["fs"] = _digest(writer.bytes)
    bundle._uses_block_alignment = False
    writer = _ReviewWriter(data=b"arbitrary-prefix")
    bundle.save_fs(writer, 64, 64)
    result["fs_offset"] = _digest(writer.bytes)
    return result


def observe_legacy_headers(module):
    bundle = _load(module, _legacy(1))
    bundle.version_player = "review-version"
    bundle.version_engine = "2.6.2f1"
    result = {"save": _digest(bundle.save())}
    bundle.version = 3
    result["version"] = _digest(bundle.save())
    bundle.signature = "UnityWeb"
    result["signature"] = _digest(bundle.save())
    writer = _ReviewWriter(data=_prefix(bundle))
    result["web_raw_return"] = bundle.save_web_raw(writer)
    result["web_raw"] = _digest(writer.bytes)
    return result
