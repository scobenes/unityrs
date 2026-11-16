"""Round-four writer parity and allocation regressions, replayed by Rust."""
import array
import struct
import tracemalloc
from types import SimpleNamespace

from binding_cases import S
from binding_y4_save import _asset, _load, _legacy
from binding_y4_round3 import _container
from binding_y4_exports import mesh
from binding_y4_review_save import _ReviewWriter, _prefix


def _attempt(call):
    try:
        return {"value": call()}
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}


def observe_pairs(module):
    pair = S.Node("pair", "x", [S.Node("int", "first"), S.Node("int", "second")])
    source = S.serialized_file([S.Type(49, S.Node("Root", "Base", [pair]))], [S.Obj(1, 0, bytes(8))])
    result = {}

    class Indexed:
        def __getitem__(self, index):
            if index > 1:
                raise AssertionError("pair read a third element")
            return 10 + index * 10
        def __iter__(self):
            raise AssertionError("pair iterated its input")

    for label, value in (("mapping", {0: 10, 1: 20}), ("indexed", Indexed()),
                         ("extra", [10, 20, 30]), ("missing", {0: 10}), ("short", [10])):
        obj = _load(module, source).objects[1]
        result[label] = _attempt(lambda: obj.save_typetree({"x": value}).hex())
    return result


def observe_managed(module):
    reference = S.ref_object("ref")
    root = S.Node("Root", "Base", [reference])
    data = S.Node("Payload", "Base", [S.Node("int", "value"),
        S.Node("pair", "pair", [S.Node("int", "first"), S.Node("int", "second")])])
    refs = [S.Type(-1, data, ref_names=["FixtureType", "", "FixtureAssembly"])]
    payload = struct.pack("<q", 1) + S.s32("FixtureType") + S.s32("") + S.s32("FixtureAssembly") + bytes(12)
    source = S.serialized_file([S.Type(49, root)], [S.Obj(1, 0, payload)], ref_types=refs)
    obj = _load(module, source).objects[1]
    value = {"ref": {"rid": 1, "type": {"class": "FixtureType", "ns": "", "asm": "FixtureAssembly"},
                     "data": {"value": 11, "pair": {0: 10, 1: 20}, "unused": object()}}}
    return _attempt(lambda: obj.save_typetree(value).hex())


def observe_cab_mapping(module):
    result = {}
    for web in (False, True):
        for serialized in (False, True):
            file = _container(module, web=web)
            owner = file.files["child.assets"] if serialized else file
            if serialized:
                # Avoid UnityPy's unrelated CPython FileIdentifier __class__
                # assignment failure by starting with a registered path.
                owner.externals.append(SimpleNamespace(path="archive:/child.assets/new.resS"))
            first = owner.get_writeable_cab("new.resS")
            first.write(b"old")
            del file.files["new.resS"]
            second = owner.get_writeable_cab("new.resS")
            result[f"{web}/{serialized}"] = {"bytes": second.bytes.hex(), "distinct": second is not first,
                                             "old": first.bytes.hex()}
        source = S.web_file([(b"old.resS", b"old")]) if web else S.fs_bundle([(b"old.resS", b"old", 0)])
        file = _load(module, source)
        del file.files["old.resS"]
        result[f"resource/{web}"] = _attempt(lambda: file.get_writeable_cab("old.resS").bytes.hex())
    return result


def observe_export_buffers(module):
    value = mesh(module)
    value.m_IndexBuffer = array.array("B", value.m_IndexBuffer)
    value.m_VertexData.m_DataSize = array.array("B", value.m_VertexData.m_DataSize)
    return _attempt(value.export)


def observe_strided_writes(module):
    result = {}
    for readonly in (False, True):
        source = bytearray(b"abcdef")
        data = memoryview(bytes(source) if readonly else source)[::2]
        cab = _container(module).get_writeable_cab("new.resS")
        cab.write(b"old")
        cab.Position = 1
        result[f"cab/{readonly}"] = _attempt(lambda: cab.write(data))
        result[f"cab_state/{readonly}"] = [cab.Position, cab.bytes.hex()]
        file = _load(module, _asset())
        file.objects[1].data = data
        result[f"serialized/{readonly}"] = _attempt(lambda: file.save().hex())
    return result


def _measure(call):
    tracemalloc.start()
    tracemalloc.reset_peak()
    try:
        call()
        error = None
    except Exception as caught:
        error = type(caught).__name__
    peak = tracemalloc.get_traced_memory()[1]
    tracemalloc.stop()
    return {"error": error, "copied": peak > 1_000_000}


def observe_output_names(module, corrected=False):
    import scratch
    result = {}
    for label, name in (("slashes", "/" * 2_000_000), ("leaf", "prefix/" + "x" * 2_000_000),
                        ("plain", "x" * 2_000_000)):
        file = _container(module, bounded=True)
        env = file.environment
        env.files = {name: file}
        file.mark_changed()
        with scratch.folder("round4-names-") as folder:
            result[label] = _measure(lambda: env.save(out_path=str(folder)))
    if corrected and module.__name__ == "UnityPy":
        return {"slashes": {"error": "ValueError", "copied": False},
                "leaf": {"error": "MemoryError", "copied": False},
                "plain": {"error": "MemoryError", "copied": False}}
    return result


def observe_path_budgets(module, corrected=False):
    if module.__name__ == "UnityPy":
        # The oracle has no owning-budget API or native path staging API.
        return {"bytes": "MemoryError", "str": "MemoryError"} if corrected else None
    owner = _container(module, bounded=True).files["child.assets"].objects[1]
    result = {}
    for label, path in (("bytes", b"x" * 8_000_000), ("str", "x" * 8_000_000)):
        try:
            module._native.export_audio(b"RIFF", b"clip", owner._native, None, path)
        except Exception as error:
            result[label] = type(error).__name__
    return result


def observe_bundle_views(module):
    result = {}
    for legacy in (False, True):
        file = _load(module, _legacy(3) if legacy else S.fs_bundle([(b"raw.resS", b"x" * 2_000_000, 0)]))
        if legacy:
            # Use a large live replacement in the legacy bundle as well.
            file.files = {"raw": SimpleNamespace(flags=0, save=lambda: bytes(2_000_000))}
        writer = _ReviewWriter(data=_prefix(file))
        received = []
        original_write = writer.write
        def write(data):
            if len(data) > 1_000_000:
                received.append(isinstance(data, memoryview))
            original_write(data)
        writer.write = write
        if legacy:
            file.save_web_raw(writer)
        else:
            file.save_fs(writer, 64, 64)
        # Views are an allocation policy; the oracle's byte result is identical.
        result[str(legacy)] = len(writer.bytes) > 2_000_000
        if module.__name__ == "unityrs":
            assert received and all(received), "bundle adapter copied a payload slice"
            _adapter_budget(module, legacy)
    return result


def _adapter_budget(module, legacy):
    budget = module.Budget(32_000_000)
    source = _legacy(3) if legacy else S.fs_bundle([(b"raw.resS", b"small", 0)])
    file = module.Environment(budget=budget).load_file(source, name="owner")
    payload = b"x" * 2_000_000
    file.files = {"raw.resS": SimpleNamespace(flags=0, save=lambda: payload)}
    native = file._native
    held = native.tree_traversal()

    class Native:
        def __getattr__(self, name):
            return getattr(native, name)
        def save_bundle_entries(self, *args):
            data = native.save_bundle_entries(*args)
            held.step(0, budget.limit - budget.used - 1024, 0)
            tracemalloc.start()
            tracemalloc.reset_peak()
            return data

    class Sink(_ReviewWriter):
        def write(self, data):
            self.Position += len(data)

    file._native = Native()
    writer = Sink(data=_prefix(file))
    try:
        file.save_web_raw(writer) if legacy else file.save_fs(writer, 64, 64)
        peak = tracemalloc.get_traced_memory()[1]
    finally:
        tracemalloc.stop()
    assert peak < 4096, f"adapter copied payload with 1 KiB remaining: {peak}"
