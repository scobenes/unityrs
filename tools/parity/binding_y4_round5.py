"""Round-five protocol, allocation and output preflight regressions."""
import array
import struct

from binding_cases import S
from binding_y4_save import _load
from binding_y4_round3 import _container
from binding_y4_round4 import _attempt, _measure
from binding_y4_images import make
from binding_y4_exports import mesh, _MemoryFS
from binding_y4_review_exports import renderer


def _object(module, child):
    return _load(module, S.serialized_file([S.Type(49, S.Node("Root", "Base", [child]))],
                                         [S.Obj(1, 0, bytes(4))])).objects[1]


def observe_buffer_lengths(module):
    result = {}
    for label, value in (("array", array.array("I", [1, 2, 3])),
                         ("view", memoryview(array.array("I", [1, 2, 3]))),
                         ("matrix", memoryview(bytes(range(12))).cast("B", (3, 4)))):
        obj = _object(module, S.Node("TypelessData", "x"))
        raw = obj.save_typetree({"x": value})
        assert raw == struct.pack("<i", 3) + memoryview(value).tobytes()
        # Check the containing object's advertised payload size, too.
        saved = _load(module, obj.assets_file.save()).objects[1]
        result[label] = [raw.hex(), saved.byte_size, saved.get_raw_data().hex()]
    inner = S.Node("TypelessData", "data")
    vector = S.Node("vector", "x", [S.Node("Array", "Array", [S.Node("int", "size"), inner])])
    obj = _object(module, vector)
    raw = obj.save_typetree({"x": [array.array("I", [9]), memoryview(bytes(range(8))).cast("B", (2, 4))]})
    assert raw == struct.pack("<iii", 2, 1, 9) + struct.pack("<i", 2) + bytes(range(8))
    result["nested"] = raw.hex()
    class Encoded:
        def encode(self, *args):
            return memoryview(array.array("I", [1, 2, 3]))
    obj = _object(module, S.Node("string", "x"))
    result["encoded"] = obj.save_typetree({"x": Encoded()}).hex()
    assert result["encoded"] == "03000000010000000200000003000000"
    return result


def observe_vector_lengths(module):
    vector = S.Node("vector", "x", [S.Node("Array", "Array", [
        S.Node("int", "size"), S.Node("int", "data")])])
    result = {}
    class Sized:
        def __len__(self):
            return 1
        def __iter__(self):
            return iter([10, 20])
    for label, value in (("generator", (x for x in [10, 20])), ("sized", Sized())):
        obj = _object(module, vector)
        result[label] = _attempt(lambda: obj.save_typetree({"x": value}).hex())
    outer = S.Node("vector", "x", [S.Node("Array", "Array", [
        S.Node("int", "size"), S.Node("vector", "data", vector.children)])])
    obj = _object(module, outer)
    result["nested"] = _attempt(lambda: obj.save_typetree({"x": [Sized(), [30]]}).hex())
    return result


def observe_cab_validation(module, corrected=False):
    result = {}
    for web in (False, True):
        for label, name in (("nul", "bad\0.resS"), ("window", "x" * 32768), ("utf8", "é" * 16384)):
            if corrected and module.__name__ == "UnityPy":
                result[f"{web}/{label}"] = ["ValueError", True, True]
                continue
            file = _container(module, web=web)
            owner = file.files["child.assets"]
            files, externals = list(file.files), list(owner.externals)
            try:
                owner.get_writeable_cab(name)
                error = None
            except Exception as caught:
                error = type(caught).__name__
            result[f"{web}/{label}"] = [error, list(file.files) == files, owner.externals == externals]
    return result


def observe_array_storage(module, corrected=False):
    if corrected and module.__name__ == "UnityPy":
        return {label: {"error": "MemoryError", "copied": False} for label in ("typed", "negative")}
    file = _container(module)
    if module.__name__ == "unityrs":
        file = module.Environment(budget=module.Budget(4_000_000)).load_file(file.save(), name="owner")
    owner = file.files["child.assets"].objects[1]
    data = array.array("I", [0]) * 2_000_000
    obj = make(module, "Texture2DArray", image_data=data, m_Width=1, m_Height=1,
               m_DataSize=len(data), m_Depth=1, m_Format=8, m_StreamData=None, object_reader=owner)
    result = {"typed": _measure(lambda: obj.images)}
    obj.image_data = [0] * 2_000_000
    obj.m_DataSize = -1
    result["negative"] = _measure(lambda: obj.images)
    return result


def observe_exact_output_names(module, corrected=False):
    class Name(str):
        def __add__(self, suffix):
            return "../escape" + suffix
        def __getitem__(self, index):
            return "../escape"
        def __str__(self):
            return "../escape"
    geometry = mesh(module)
    geometry.m_Name = Name("safe")
    fs = _MemoryFS()
    value = renderer(module, geometry, [], fs)
    try:
        value.export("export")
        error = None
    except Exception as caught:
        error = type(caught).__name__
    if corrected and module.__name__ == "UnityPy":
        return {"error": None, "files": ["export/safe.mtl", "export/safe.obj"]}
    return {"error": error, "files": sorted(fs.files)}


def observe_snapshot_threads(module, corrected=False):
    import sys
    import threading
    if corrected and module.__name__ == "UnityPy":
        return {"progress": True}
    vector = S.Node("vector", "x", [S.Node("Array", "Array", [
        S.Node("int", "size"), S.Node("int", "data")])])
    obj = _object(module, vector)
    progress, errors = [], []
    ready = threading.Event()
    native = getattr(module, "_native", None)
    barrier = native._RowCopyBarrier() if hasattr(native, "_RowCopyBarrier") else None
    caller = threading.get_ident()
    class Last:
        def __index__(self):
            assert threading.get_ident() == caller
            assert progress, "native staging held the GIL throughout the vector"
            assert obj.parse_as_dict() == {"x": []}, "numeric callback could not reenter"
            return 13
    class Values(list):
        def __iter__(self):
            if barrier is not None:
                barrier.arm_value()
            ready.set()
            return super().__iter__()
    values = Values([7] * 199_999 + [Last()])
    def other():
        try:
            ready.wait()
            if barrier is not None:
                barrier.wait_for_copy()
            progress.append(True)
            if barrier is not None:
                barrier.acknowledge()
        except BaseException as error:
            errors.append(str(error))
    thread = threading.Thread(target=other)
    thread.start()
    interval = sys.getswitchinterval()
    sys.setswitchinterval(30)
    succeeded = False
    try:
        raw = obj.save_typetree({"x": values})
        assert len(raw) == 800_004 and raw[-4:] == struct.pack("<i", 13)
        succeeded = True
    except AssertionError:
        if module.__name__ != "UnityPy":
            raise
    finally:
        sys.setswitchinterval(interval)
        thread.join()
    assert not errors, errors
    return {"progress": succeeded}


def observe_array_inputs(module):
    owner = _container(module).files["child.assets"].objects[1]
    result = {}
    for label, data, size in (("list", [1, 2, 3, 4], 4), ("tuple", (1, 2, 3, 4), 4),
                              ("typed", array.array("I", [0x04030201]), 1)):
        for width in (0, 1):
            obj = make(module, "Texture2DArray", image_data=data, m_Width=width, m_Height=1,
                       m_DataSize=size, m_Depth=1, m_Format=8, m_StreamData=None, object_reader=owner)
            result[f"{label}/{width}"] = [image.tobytes().hex() for image in obj.images]
    return result
