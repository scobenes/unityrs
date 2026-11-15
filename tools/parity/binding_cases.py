"""Y1 synthetic inputs and lossless Python type observations (UnityPy 1.25.2).

PARITY sections 3, 5.2, 7.2, 9.1 and 19.2. No installed binding is needed
by the recorder; the binding suite uses the same observations on both modules.
"""

import contextlib
import io
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "tests"))
import synth as S


def typed(value):
    """Keep tuple/list, bool/int, raw strings, signed zero and NaN payloads."""
    if isinstance(value, dict):
        return ["dict", [[typed(k), typed(v)] for k, v in value.items()]]
    if isinstance(value, (list, tuple)):
        return [type(value).__name__, [typed(v) for v in value]]
    if isinstance(value, str):
        return ["str", value.encode("utf-8", "surrogatepass").hex()]
    if isinstance(value, bytes):
        return ["bytes", value.hex()]
    if isinstance(value, float):
        return ["float", struct.pack(">d", value).hex()]
    return [type(value).__name__, value]


def cases():
    text = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
    data = S.serialized_file([S.Type(49, text)], [S.Obj(7, 0, S.s32(b"name\xff") + S.s32("body"))])
    yield "text", data
    yield "bundle", S.fs_bundle([(b"CAB-y1", data, 4), (b"CAB-y1.resS", b"resource", 0)])
    yield "nested", S.fs_bundle([(b"nested", S.fs_bundle([(b"CAB-y1", data, 4)]), 0)])
    yield "resource", b"raw bytes"
    fields = [S.Node(t, n, meta=S.ALIGN if t == "bool" else 0) for t, n in [("bool", "b"), ("SInt64", "i"), ("UInt64", "u"),
                                       ("float", "f"), ("double", "d"), ("string", "s"),
                                       ("TypelessData", "raw")]]
    fields += [S.Node("vector", "v", [S.Node("Array", "Array", [S.Node("int", "size"), S.Node("UInt8", "data")])]),
               S.Node("pair", "pair", [S.Node("int", "first"), S.Node("float", "second")])]
    payload = b"\x01\x00\x00\x00" + struct.pack("<qQfd", -(2**63), 2**64-1, -0.0, float("inf"))
    payload += S.s32(b"bad\x80\xff\x00utf8") + struct.pack("<i", 3) + b"abc"
    payload += struct.pack("<i", 3) + b"\x00\x80\xff" + struct.pack("<if", -17, 1.25)
    yield "types", S.serialized_file([S.Type(49, S.Node("TextAsset", "Base", fields))], [S.Obj(1, 0, payload)])
    yield "check_read", S.serialized_file([S.Type(49, text)], [S.Obj(1, 0, S.s32("a") + S.s32("b") + b"tail")])
    yield "short_read", S.serialized_file([S.Type(49, text)], [S.Obj(1, 0, b"\x01")])
    for kind in ("int", "bool", "string", "TypelessData"):
        payload = b"\x01" if kind == "int" else (struct.pack("<i", 8) if kind != "bool" else b"")
        # A nonempty object selects Boost even when its first field cannot fit.
        tree = S.Node("TextAsset", "Base", [S.Node("int", "prefix"), S.Node(kind, "value")])
        yield "short_" + kind, S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, b"\0"*4 + payload)])
    for version in (19, 20):
        for size in (None, 0):
            tree = S.Node("TextAsset", "Base", [S.ref_object("ref")])
            payload = struct.pack("<q", 1) + S.s32("Class") + S.s32("Namespace") + S.s32("Assembly")
            yield f"refs_{version}_{size}", S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, payload, byte_size=size)], version=version)
        tree = S.Node("AssetBundle", "Base", [S.ref_object("ref")])
        yield f"assetbundle_refs_{version}", S.serialized_file([S.Type(142, tree)], [S.Obj(1, 0, payload, byte_size=0)], version=version)
    yield "unterminated", b"A" * 20
    for kind, fmt, values in [("bool", "?", [False, True]), ("UInt8", "B", [0, 255]),
                              ("SInt8", "b", [-128, 127]), ("UInt16", "H", [0, 65535]),
                              ("SInt16", "h", [-32768, 32767]), ("UInt32", "I", [0, 2**32-1]),
                              ("SInt32", "i", [-2**31, 2**31-1]), ("UInt64", "Q", [0, 2**64-1]),
                              ("SInt64", "q", [-2**63, 2**63-1]), ("float", "f", [-0.0, float("inf")]),
                              ("double", "d", [-0.0, float("nan")])]:
        tree = S.Node("TextAsset", "Base", [S.vec("items", S.Node(kind, "data"))])
        payload = struct.pack("<i" + fmt*2, 2, *values)
        yield "array_" + kind, S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, payload)])
    yield "unknown_class", S.serialized_file([S.Type(1234567, text)], [S.Obj(1, 0, S.s32("a") + S.s32("b"))])
    import gzip
    good = gzip.compress(b"UnityWebData1.0\0" + bytes(range(256)), mtime=0)
    yield "gzip_truncated", good[:-4]
    yield "gzip_crc", good[:-8] + b"\x01" + good[-7:] + b"\0"*8
    yield "gzip_corrupt", b"\x1f\x8b\x08\0" + b"\0"*6 + b"\xff"*128


def observe(module, data=None, name="input.assets", env=None):
    """Stable public observations, with addresses and process hashes excluded."""
    env = module.Environment() if env is None else env
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            if data is not None:
                env.load_file(data, name=name)
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}

    def file_record(f):
        result = {"class": type(f).__name__}
        if isinstance(getattr(f, "files", None), dict):
            result["dependency"] = f.is_dependency
            result["changed"] = f.is_changed
            result["get_files"] = f.get("files") is f.files
            result["mapping"] = list(f.keys()) == list(f.files) and list(f.items()) == list(f.files.items()) and list(f.values()) == list(f.files.values()) and all(f[k] is v for k, v in f.items())
            for method, args in [("get_assets", ()), ("get_objects", ()),
                                 ("get_filtered_objects", ()), ("get_filtered_objects", ([49],))]:
                key = method + str(args)
                try:
                    result[key] = [getattr(x, "path_id", getattr(x, "name", None)) for x in getattr(f, method)(*args)]
                except Exception as error:
                    result[key] = {"error": type(error).__name__, "message": str(error)}
        if isinstance(getattr(f, "name", None), str):
            result["name"] = f.name
        if isinstance(getattr(f, "files", None), dict):
            result["files"] = [[k, file_record(v)] for k, v in f.files.items()]
        if isinstance(getattr(f, "objects", None), dict):
            result["objects"] = []
            for key, obj in f.objects.items():
                row = {"key": key, "path_id": obj.path_id, "class_id": obj.class_id,
                       "type_id": obj.type_id, "byte_start": obj.byte_start, "byte_size": obj.byte_size,
                       "version2": obj.version2, "platform": int(obj.platform),
                       "raw": obj.get_raw_data().hex(), "identity": obj.assets_file is f, "type_name": obj.type.name,
                       "get_missing": obj.get("missing", 5)}
                for check in (True, False):
                    try:
                        row[str(check)] = typed(obj.parse_as_dict(check_read=check))
                    except Exception as error:
                        row[str(check)] = {"error": type(error).__name__, "message": str(error)}
                result["objects"].append(row)
        return result

    return {"aliases": module.load is module.Environment and module.AssetsManager is module.Environment, "files": [[k, file_record(v)] for k, v in env.files.items()],
            "objects": [o.path_id for o in env.objects],
            "assets": [f.name for f in env.assets], "file": hasattr(env, "file"), "cabs": list(env.cabs)}


def observe_paths(module, data, root):
    """Local loading and nameless stream cases, with scratch paths normalized."""
    root = Path(root)
    path = root / "path.assets"
    path.write_bytes(data)
    folder = root / "folder"
    folder.mkdir(exist_ok=True)
    (folder / "a.assets").write_bytes(data)
    results = {}
    for key, source in [("path", str(path)), ("folder", str(folder)), ("bytes", data),
                        ("stream", io.BytesIO(data)), ("missing", str(root / "missing")), ("stream20", io.BytesIO(b"A"*20))]:
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                env = module.Environment(source)
                results[key] = observe(module, env=env)
                if key in ("bytes", "stream", "stream20"):
                    original = str(next(iter(env.files)))
                    token = "<hash>" if key == "bytes" else "<stream>"
                    results[key] = replace_string(results[key], original, token)
            except Exception as error:
                results[key] = {"error": type(error).__name__}
    results["multiple"] = observe(module, env=module.Environment(str(path), str(folder)))
    env = module.Environment(path=str(root))
    env.load_file("path.assets")
    results["relative"] = observe(module, env=env)
    env.load_file(data, name="dependency", is_dependency=True)
    results["dependency"] = observe(module, env=env)
    env = module.Environment()
    first = env.load_file(data, name="CAB-y1")
    nested = env.load_file(S.web_file([(b"CAB-y1", data)]), name="nested")
    before = env.get_cab("CAB-y1")
    env.load_file(data, name="unrelated")
    results["cab_order"] = [before is nested.files["CAB-y1"], env.get_cab("CAB-y1") is before,
                            env.get_cab("CAB-y1") is first]
    # Replacing an earlier dict key is a later registration.
    replaced = env.load_file(data, name="CAB-y1")
    env.load_file(data, name="other")
    results["cab_replaced"] = env.get_cab("CAB-y1") is replaced
    env.register_cab("CAB-y1", first)
    env.load_file(data, name="manual-unrelated")
    kept = env.get_cab("CAB-y1") is first
    latest = env.load_file(data, name="CAB-y1")
    results["cab_manual"] = [kept, env.get_cab("CAB-y1") is latest]
    web = S.web_file([(b"leaf", b"raw")])
    results["stream_names"] = []
    for name in (None, 7, b"bytes"):
        stream = io.BytesIO(web)
        stream.name = name
        env = module.Environment(stream)
        results["stream_names"].append([typed(next(iter(env.files))), env.file.name])
    base = root / "split.assets"
    Path(str(base) + ".split0").write_bytes(data[:len(data)//2])
    Path(str(base) + ".split1").write_bytes(data[len(data)//2:])
    env = module.Environment()
    env.load_file(str(base) + ".split0")
    results["split_direct"] = observe(module, env=env)
    import zipfile
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("nested", S.web_file([(b"CAB-y1", data)]))
        archive.writestr("CAB-y1", data)
    env = module.Environment()
    env.load_file(data, name="CAB-y1")
    env.load_zip_file(buffer.getvalue())
    results["cab_zip_replaced"] = [env.get_cab("CAB-y1") is env.files["CAB-y1"],
                                   env.get_cab("CAB-y1") is env.files["nested"].files["CAB-y1"]]
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("z.assets", data)
        archive.writestr("raw.resS", b"raw")
    for key, source in (("zip_bytes", buffer.getvalue()), ("zip_stream", io.BytesIO(buffer.getvalue()))):
        env = module.Environment()
        env.load_zip_file(source)
        results[key] = observe(module, env=env)

    def clean(value):
        if isinstance(value, str):
            return value.replace(str(root), "<root>")
        if isinstance(value, list):
            return [clean(x) for x in value]
        if isinstance(value, dict):
            return {k: clean(v) for k, v in value.items()}
        return value
    return clean(results)


def replace_string(value, original, replacement):
    if isinstance(value, str):
        return value.replace(original, replacement)
    if isinstance(value, list):
        return [replace_string(x, original, replacement) for x in value]
    if isinstance(value, dict):
        return {k: replace_string(v, original, replacement) for k, v in value.items()}
    return value
