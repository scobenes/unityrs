"""Round-seven streamed lifetimes, CAB protocol and deferred replacements."""
import struct
import tracemalloc

from binding_cases import S
from binding_y4_images import image
from binding_y4_save import _asset, _load, _attempt


def _streamed_texture():
    integers = dict(m_Width=1, m_Height=1, m_TextureFormat=4,
                    m_CompleteImageSize=100_000, m_ImageCount=1, m_IsReadable=1,
                    m_LightmapFormat=0, m_TextureDimension=2, m_TextureSettings=0)
    fields = [S.Node("string", "m_Name")] + [S.Node("int", key) for key in integers]
    fields += [S.vec(key, S.Node("UInt8", "data")) for key in ("image data", "m_PlatformBlob")]
    fields.append(S.Node("StreamingInfo", "m_StreamData", [
        S.Node("string", "path"), S.Node("UInt64", "offset"), S.Node("unsigned int", "size")]))
    payload = S.s32("streamed") + struct.pack("<" + "i" * len(integers), *integers.values())
    payload += struct.pack("<ii", 0, 100_000) + bytes(100_000)
    payload += S.s32("pixels.resS") + struct.pack("<QI", 0, 100_000)
    source = S.serialized_file([S.Type(28, S.Node("Texture2D", "Base", fields))], [S.Obj(1, 0, payload)])
    return S.fs_bundle([(b"pixels.assets", source, 4), (b"pixels.resS", bytes(100_000), 0)])


def observe_streamed_lifetime(module, corrected=False):
    if module.__name__ == "unityrs":
        budget = module.Budget(200_000_000)
        bundle = module.Environment(budget=budget).load_file(_streamed_texture(), name="pixels.bundle")
    else:
        bundle = _load(module, _streamed_texture())
    obj = bundle.files["pixels.assets"].objects[1].read()
    result = {"decoded": image(obj.image), "public_bytes": type(obj.get_image_data()).__name__}
    if module.__name__ == "UnityPy":
        result["refused"] = ["MemoryError", False, True] if corrected else None
        return result
    held = obj.object_reader._native.tree_traversal()
    remaining = 150_000
    held.step(0, budget.limit - budget.used - remaining, 0)
    before = budget.used
    tracemalloc.start()
    try:
        obj.image
        error = None
    except Exception as caught:
        error = type(caught).__name__
    peak = tracemalloc.get_traced_memory()[1]
    tracemalloc.stop()
    result["refused"] = [error, peak > remaining, budget.used == before]
    del held
    assert image(obj.image) == result["decoded"]
    return result


def observe_cab_protocol(module):
    result = {}
    scalars = (("byte", -7), ("u_byte", 250), ("short", -1234), ("u_short", 60000),
               ("int", -123456), ("u_int", 4000000000), ("long", -1234567890123),
               ("u_long", 12345678901234567890), ("float", 1.25), ("double", -2.5), ("boolean", True))
    arrays = (("boolean", [True, False]), ("u_short", [3, 60000]), ("int", [-7, 8]),
              ("u_int", [7, 4000000000]), ("float", [1.25, -2.5]), ("string", ["a", "é"]))
    for endian in ("<", ">"):
        bundle = _load(module, S.fs_bundle([(b"raw.resS", b"hello", 0)]))
        cab = bundle.get_writeable_cab()
        cab.endian = endian
        returns = [getattr(cab, "write_" + kind)(value) for kind, value in scalars]
        returns += [cab.write_string_to_null("a\udcff"), cab.write_aligned_string("é")]
        returns += [getattr(cab, "write_" + kind + "_array")(values) for kind, values in arrays]
        returns += [cab.write_int_array([9], True), cab.write_u_int_array([10], True),
                    cab.write_float_array([3.5], True), cab.write_byte_array(b"xy"),
                    cab.write_array(cab.write_short, [11, -12], False)]
        cab.Position = 0
        cab.write_long(42)
        errors = [_attempt(lambda: cab.write_u_int(-1)), _attempt(lambda: cab.write_float("bad"))]
        result[endian] = [returns, cab.bytes.hex(), cab.Position, cab.Length, errors]
    bundle = _load(module, S.fs_bundle([(b"raw.resS", b"hello", 0)]))
    cab = bundle.get_writeable_cab()
    cab.write(b"prefix")
    returned = bundle.save_fs(cab, 64, 64)
    result["save_fs"] = [returned, cab.bytes.hex(), cab.Position, cab.Length]
    return result


def observe_deferred_replacements(module):
    result = {}
    for label, replacement in (("list", []), ("tuple", (1,)), ("str", "bad"), ("int", 3)):
        for container in ("asset", "bundle", "web"):
            source = _asset()
            if container != "asset":
                source = (S.fs_bundle([(b"child.assets", source, 4)]) if container == "bundle"
                          else S.web_file([(b"child.assets", source)]))
            parent = _load(module, source)
            file = parent if container == "asset" else parent.files["child.assets"]
            obj = file.objects[1]
            original = obj.get_raw_data()
            obj.set_raw_data(b"previous")
            file.is_changed = parent.is_changed = False
            assigned = _attempt(lambda: obj.set_raw_data(replacement))
            result[label + "/" + container] = [assigned, obj.data is replacement,
                file.is_changed, parent.is_changed, obj.get_raw_data() == original,
                _attempt(parent.save), obj.data is replacement, file.is_changed, parent.is_changed]
            obj.set_raw_data(None)
            reloaded = _load(module, parent.save())
            restored = reloaded if container == "asset" else reloaded.files["child.assets"]
            assert restored.objects[1].get_raw_data() == original
    if module.__name__ == "unityrs":
        budget = module.Budget(200_000_000)
        parent = module.Environment(budget=budget).load_file(
            S.fs_bundle([(b"child.assets", _asset(), 4)]), name="full.bundle")
    else:
        parent = _load(module, S.fs_bundle([(b"child.assets", _asset(), 4)]))
    file = parent.files["child.assets"]
    obj = file.objects[1]
    replacement = []
    if module.__name__ == "unityrs":
        held = file._native.tree_traversal()
        held.step(0, budget.limit - budget.used, 0)
        before = budget.used
    assigned = _attempt(lambda: obj.set_raw_data(replacement))
    result["exhausted"] = [assigned, obj.data is replacement, file.is_changed, parent.is_changed]
    if module.__name__ == "unityrs":
        assert budget.used == before
        del held
    result["exhausted"].append(_attempt(parent.save))
    return result
