"""Y4 save API observations, built only from MIT synthetic fixtures."""

import contextlib
import gzip
import hashlib
import io
import struct
from types import SimpleNamespace

from binding_cases import S, typed
import scratch


def _load(module, data, name="save.assets"):
    with contextlib.redirect_stdout(io.StringIO()):
        return module.Environment().load_file(data, name=name)


def _asset():
    tree = S.Node("Y4Save", "Base", [S.Node("int", "value"), S.Node("string", "name")])
    return S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, struct.pack("<i", 7) + S.s32("before"))])


def _attempt(call):
    try:
        return {"value": typed(call())}
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}


def observe_objects(module):
    file = _load(module, _asset())
    obj = file.objects[1]
    original = obj.get_raw_data()
    result = {"initial": [file.is_changed, obj.data]}
    result["write"] = _attempt(lambda: obj.save_typetree({"value": 19, "name": "after"}))
    result["changed"] = file.is_changed
    result["data"] = typed(obj.data)
    result["original_read"] = [obj.get_raw_data() == original, obj.parse_as_dict()]
    data = file.save()
    result["saved"] = _load(module, data).objects[1].parse_as_dict()
    result["repeat"] = file.save() == data
    result["still_changed"] = file.is_changed
    obj.set_raw_data(struct.pack("<i", 31) + S.s32("raw"))
    result["raw_replacement"] = _load(module, file.save()).objects[1].parse_as_dict()
    # Object.save delegates to the same mutable reader.
    parsed = obj.read()
    parsed.value = 23
    parsed.name = "class"
    result["class_return"] = parsed.save()
    result["class_saved"] = _load(module, file.save()).objects[1].parse_as_dict()
    file.is_changed = False
    result["cleared_flag"] = file.is_changed
    obj.set_raw_data(original)
    result["remarked_flag"] = file.is_changed
    return result


def observe_files(module):
    payload = _asset()
    packed = S.fs_bundle([(b"save.assets", payload, 4), (b"data.resS", b"resource", 0)])
    bundle = _load(module, packed, "save.bundle")
    result = {"initial": [bundle.is_changed, bundle.files["save.assets"].is_changed]}
    bundle.files["save.assets"].objects[1].set_raw_data(struct.pack("<i", 29) + S.s32("nested"))
    result["propagated"] = [bundle.is_changed, bundle.files["save.assets"].is_changed]
    for packer in (None, "none", "original", "lz4", "lzma", (195, 3)):
        data = bundle.save(packer=packer)
        loaded = _load(module, data, "reload.bundle")
        result[repr(packer)] = {"hash": hashlib.sha256(data).hexdigest(),
                               "saved": loaded.files["save.assets"].objects[1].parse_as_dict(),
                               "resource": loaded.files["data.resS"].bytes}
        result[repr(packer)]["resource"] = typed(result[repr(packer)]["resource"])
    result["bad_packer"] = _attempt(lambda: bundle.save(packer="unknown"))
    asset = bundle.files.pop("save.assets")
    bundle.files["replacement.assets"] = asset
    result["bundle_mutated_mapping"] = list(_load(module, bundle.save()).files)
    result["bundle_replacement"] = _load(module, bundle.save()).files["replacement.assets"].objects[1].parse_as_dict()
    web = _load(module, S.web_file([(b"save.assets", payload), (b"raw", b"hello")]), "save.web")
    plain = web.save()
    result["web"] = hashlib.sha256(plain).hexdigest()
    result["web_gzip"] = gzip.decompress(web.save(packer="gzip")) == plain
    import brotli
    result["web_brotli"] = brotli.decompress(web.save(packer="brotli")) == plain
    result["web_unknown"] = web.save(packer="unknown") == plain
    result["web_false_packer"] = web.save(packer=None) == plain
    subset = web.save(files={"copied": web.files["raw"]})
    result["web_mapping"] = [[name, typed(value.bytes)] for name, value in _load(module, subset).files.items()]
    del web.files["save.assets"]
    result["web_mutated_mapping"] = list(_load(module, web.save()).files)
    return result


def observe_environment(module):
    with scratch.folder(prefix="y4-save-") as root:
        env = module.Environment()
        a = env.load_file(_asset(), name="x/s.assets")
        b = env.load_file(_asset(), name="y/s.assets")
        env.load_file(_asset(), name="unchanged.assets")
        a.objects[1].set_raw_data(struct.pack("<i", 10) + S.s32("first"))
        b.objects[1].set_raw_data(struct.pack("<i", 20) + S.s32("last"))
        env.fs = SimpleNamespace(sep="/")
        returned = env.save(out_path=str(root))
        result = {"return": returned, "files": sorted(p.name for p in root.iterdir()),
                  "saved": _load(module, (root / "s.assets").read_bytes()).objects[1].parse_as_dict(),
                  "changed": [a.is_changed, b.is_changed]}
        try:
            env.save(out_path=str(root / "missing"))
        except Exception as error:
            result["missing"] = type(error).__name__
        return result


def observe_cabs(module):
    bundle = _load(module, S.fs_bundle([(b"save.assets", _asset(), 4)]), "save.bundle")
    result = {"top_level": _load(module, _asset()).get_writeable_cab() is None}
    cab = bundle.get_writeable_cab()
    result["created"] = [list(bundle.files), bundle.is_changed, cab.name, cab.flags, cab.endian]
    result["reuse"] = bundle.get_writeable_cab() is cab
    result["write_return"] = cab.write_bytes(b"abc")
    result["bytes"] = typed(cab.bytes)
    loaded = _load(module, bundle.save(), "reload.bundle")
    result["saved"] = typed(loaded.files[cab.name].bytes)
    cab.Position = 0
    result["overwrite_return"] = cab.write(b"Z")
    cab.Position = 5
    result["extend_return"] = cab.write(b"Q")
    result["seek_bytes"] = typed(cab.bytes)
    result["seek_position"] = cab.Position
    result["seek_saved"] = typed(_load(module, bundle.save()).files[cab.name].bytes)
    result["existing"] = _attempt(lambda: bundle.get_writeable_cab("save.assets"))
    return result


class _Writer:
    """Small duck writer to test supported writer arguments without UnityPy."""
    def __init__(self, endian=">", data=b""):
        self.endian, self.data, self.Position = endian, bytearray(data), len(data)

    @property
    def bytes(self):
        return bytes(self.data)

    def write(self, data):
        self.data[self.Position:self.Position + len(data)] = data
        self.Position += len(data)
        return len(data)

    def align_stream(self, alignment=4):
        self.write(b"\0" * ((alignment - self.Position % alignment) % alignment))

    def write_int(self, value):
        self.write(struct.pack(self.endian + "i", value))

    def write_u_int(self, value):
        self.write(struct.pack(self.endian + "I", value))

    def write_long(self, value):
        self.write(struct.pack(self.endian + "q", value))


def observe_writers(module):
    file = _load(module, _asset())
    obj = file.objects[1]
    writer = _Writer("<", b"prefix")
    result = {"tree": typed(obj.save_typetree({"value": 41, "name": "writer"}, writer=writer)),
              "data": typed(obj.data), "position": writer.Position}
    metadata, data = _Writer("<"), _Writer("<", b"start")
    result["object_return"] = obj.write(file.header, metadata, data)
    result["metadata"] = typed(metadata.bytes)
    result["payload"] = typed(data.bytes)
    bundle = _load(module, S.fs_bundle([(b"raw.resS", b"hello", 0)]))
    writer = _Writer()
    result["fs_return"] = bundle.save_fs(writer, 64, 64)
    saved = bundle.save()
    position = saved.index(b"\0") + 5
    position = saved.index(b"\0", position) + 1
    position = saved.index(b"\0", position) + 1
    result["fs_body"] = writer.bytes == saved[position:]
    return result


def _legacy(version):
    name, payload = b"data.resS", b"legacy-resource"
    directory_size = (4 + len(name) + 1 + 8 + 3) & ~3
    directory = struct.pack(">i", 1) + name + b"\0" + struct.pack(">II", directory_size, len(payload))
    content = directory.ljust(directory_size, b"\0") + payload
    prefix = b"UnityRaw\0" + struct.pack(">I", version) + b"5.x.x\0" + b"2.6.1f1\0"
    header_size = (len(prefix) + 24 + 4 * (version >= 2) + 4 * (version >= 3) + 3) & ~3
    total = header_size + len(content)
    header = struct.pack(">IIIIII", total, header_size, 1, 1, len(content), len(content))
    if version >= 2:
        header += struct.pack(">I", total)
    if version >= 3:
        header += struct.pack(">I", directory_size)
    return (prefix + header).ljust(header_size, b"\0") + content


def observe_legacy(module):
    result = {}
    for version in (1, 2, 3):
        bundle = _load(module, _legacy(version))
        writer = _Writer()
        returned = bundle.save_web_raw(writer)
        saved = bundle.save()
        result[str(version)] = {"return": returned, "body": hashlib.sha256(writer.bytes).hexdigest(),
                                "save": hashlib.sha256(saved).hexdigest(),
                                "resource": typed(_load(module, saved).files["data.resS"].bytes)}
        bundle.files["renamed.resS"] = bundle.files.pop("data.resS")
        result[str(version)]["renamed"] = list(_load(module, bundle.save()).files)
    return result
