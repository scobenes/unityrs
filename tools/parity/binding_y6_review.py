"""Synthetic forced-constructor review observations; no runtime schemas."""
import contextlib
import gzip
import io
import struct
import sys
import threading
import types

import binding_cases as B
from binding_y6_parse import _record


def _detached(module, operation):
    assets = next(data for name, data in B.cases() if name == "text")
    payloads = [(assets, module.enums.FileType.AssetsFile),
                (B.S.web_file([(b"child", assets)]), module.enums.FileType.WebFile),
                (B.S.fs_bundle([(b"web", B.S.web_file([(b"child", assets)]), 0)]),
                 module.enums.FileType.BundleFile)]
    result = []
    for payload, typ in payloads:
        root = module.helpers.ImportHelper.parse_file(
            module.streams.EndianBinaryReader(payload), None, "detached", typ)
        file = root
        chain = [root]
        while not isinstance(file, module.files.SerializedFile):
            file = next(iter(file.files.values()))
            chain.append(file)
        obj = file.objects[7]
        try:
            if operation == "save":
                value = [entry.save().hex() for entry in chain]
            elif operation == "mark":
                file.mark_changed()
                value = [entry.is_changed for entry in chain]
            elif operation == "raw":
                obj.set_raw_data(obj.get_raw_data())
                value = [obj.data.hex(), [entry.is_changed for entry in chain], root.save().hex()]
            else:
                obj.patch({"m_Name": "changed", "m_Script": "body"})
                value = [obj.data.hex(), [entry.is_changed for entry in chain], root.save().hex()]
            result.append({"value": value, "detached": [entry.environment is None for entry in chain]})
        except Exception as error:
            result.append({"error": type(error).__name__, "message": str(error)})
    return result


def observe_detached_save(module):
    return _detached(module, "save")


def observe_detached_mark(module):
    return _detached(module, "mark")


def observe_detached_raw(module):
    return _detached(module, "raw")


def observe_detached_patch(module):
    return _detached(module, "patch")


def observe_constructor_states(module):
    from binding_y4_save import _legacy
    from binding_y5_objects import _container_asset
    assets = B.S.serialized_file([], [])[:85]
    child = next(data for name, data in B.cases() if name == "text")
    web = B.S.web_file([], signature=b"UnityWebData")
    cases = [(assets, module.enums.FileType.AssetsFile),
             (_container_asset("path", 7), module.enums.FileType.AssetsFile),
             (web, module.enums.FileType.WebFile),
             (B.S.web_file([(b"child", child)]), module.enums.FileType.WebFile),
             (gzip.compress(web, mtime=0), module.enums.FileType.WebFile),
             (B.S.fs_bundle([(b"child", child, 4)]), module.enums.FileType.BundleFile),
             (_legacy(3), module.enums.FileType.BundleFile)]
    result = []
    for stream in (False, True):
        for payload, typ in cases:
            reader = module.streams.EndianBinaryReader(io.BytesIO(payload) if stream else payload)
            module.helpers.ImportHelper.parse_file(reader, module.Environment(), "forced", typ)
            result.append([reader.Position, reader.endian])
    return result


def observe_compressed_failures(module):
    result = []
    signature = b"UnityWebData\0"
    payloads = [b"raw", signature, signature + b"\x01\x02",
                signature + (29).to_bytes(4, "little") + b"\x01\x02"]
    for stream in (False, True):
        for payload in payloads:
            result.append(_record(module, gzip.compress(payload, mtime=0),
                                  module.enums.FileType.WebFile, stream=stream))
    return result


def observe_oversized_member_states(module):
    payload = bytearray(B.S.web_file([(b"child", b"raw")]))
    struct.pack_into("<i", payload, 24, 100)
    result = []
    for stream in (False, True):
        reader = module.streams.EndianBinaryReader(io.BytesIO(payload) if stream else bytes(payload))
        file = module.helpers.ImportHelper.parse_file(
            reader, module.Environment(), "forced", module.enums.FileType.WebFile)
        result.append([reader.Position, reader.endian, file.files["child"].bytes.hex()])
    return result


def observe_failed_constructor_endian(module):
    # Before/after the endian assignment, including a short extended header.
    payloads = [(b"raw", module.enums.FileType.WebFile),
                (gzip.compress(b"raw", mtime=0), module.enums.FileType.WebFile)]
    for big in (False, True):
        for version in (8, 17, 22):
            data = B.S.serialized_file([], [], version=version, endian=">" if big else "<")
            sizes = ((16, 17, 20, 25, 48, 60) if version >= 22 else
                     (16, 17, 20, 25) if version >= 9 else (16, 17))
            payloads.extend((data[:size], module.enums.FileType.AssetsFile) for size in sizes)
    result = []
    for stream in (False, True):
        for payload, typ in payloads:
            reader = module.streams.EndianBinaryReader(io.BytesIO(payload) if stream else payload)
            with contextlib.redirect_stdout(io.StringIO()):
                try:
                    module.helpers.ImportHelper.parse_file(reader, module.Environment(), "forced", typ)
                except Exception as error:
                    result.append([reader.Position, reader.endian, type(error).__name__])
                else:
                    raise AssertionError("truncated constructor succeeded")
    return result


def _nested_failure(module, *, registrations):
    broken = b"UnityWebData1.0\0" + struct.pack("<i", 33) + b"x"
    assets = next(data for name, data in B.cases() if name == "text")
    result = []
    for stream in (False, True):
        for bundle in (False, True):
            rows = [(b"first", assets), (b"broken", broken)]
            payload = (B.S.fs_bundle([(name, data, 0) for name, data in rows]) if bundle else
                       B.S.web_file(rows))
            reader = module.streams.EndianBinaryReader(io.BytesIO(payload) if stream else payload)
            env = module.Environment()
            typ = module.enums.FileType.BundleFile if bundle else module.enums.FileType.WebFile
            with contextlib.redirect_stdout(io.StringIO()):
                try:
                    module.helpers.ImportHelper.parse_file(reader, env, "forced", typ)
                except Exception as error:
                    if registrations:
                        first = env.get_cab("first")
                        fields = None if first is None else first.objects[7].parse_as_dict()
                        result.append([list(env.files), list(env.cabs),
                                       None if fields is None else
                                       [fields["m_Name"].encode("utf-8", "surrogateescape").hex(),
                                        fields["m_Script"]]])
                    else:
                        result.append([type(error).__name__, str(error)])
                else:
                    raise AssertionError("malformed second member succeeded")
    return result


def observe_partial_child_registration(module):
    return _nested_failure(module, registrations=True)


def observe_nested_bounds_failures(module):
    return _nested_failure(module, registrations=False)


def exception_reentry(module):
    """A real module override re-enters the lock held by the parser caller."""
    import struct
    env = module.Environment()
    caller = threading.get_ident()
    seen = []

    class Reentrant(types.ModuleType):
        def __getattribute__(self, name):
            if name == "error":
                seen.append(threading.get_ident())
                env.load_file(b"raw", name="reentered")
            return super().__getattribute__(name)

    original = sys.modules["struct"]
    original_class = original.__class__
    original.__class__ = Reentrant
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                module.helpers.ImportHelper.parse_file(
                    module.streams.EndianBinaryReader(b"raw"), env, "forced",
                    module.enums.FileType.AssetsFile)
            except struct.error:
                pass
            else:
                raise AssertionError("forced Assets parser accepted raw bytes")
    finally:
        original.__class__ = original_class
    assert seen and all(thread == caller for thread in seen), seen
    assert "reentered" in env.files
