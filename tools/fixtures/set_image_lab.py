"""Extract Pillow 9.5.0's fixed LAB transform, Little CMS 2.15 (MIT).

No runtime dependency is added. The pinned wheel builds a single 33-cubed
16-bit CLUT; Rust interpolates that immutable table as cmsintrp.c does. Read
its optimized pipeline rather than quantizing the eight-bit output samples.
"""
import ctypes as C
import glob
import struct
from pathlib import Path


def table():
    import PIL
    from PIL import ImageCms
    assert PIL.__version__ == "9.5.0" and ImageCms.versions()[1] == "2.15"
    paths = glob.glob(str(Path(PIL.__file__).parent.parent / "Pillow.libs/*lcms*"))
    assert len(paths) == 1
    lib = C.CDLL(paths[0])
    ptr, u32 = C.c_void_p, C.c_uint32
    signatures = [
        ("cmsCreateLab2Profile", ptr, [ptr]), ("cmsCreate_sRGBProfile", ptr, []),
        ("cmsCreateTransform", ptr, [ptr, u32, ptr, u32, u32, u32]),
        ("cmsPipelineGetPtrToFirstStage", ptr, [ptr]), ("cmsStageNext", ptr, [ptr]),
        ("cmsStageType", u32, [ptr]), ("cmsStageData", ptr, [ptr]),
        ("cmsDeleteTransform", None, [ptr]), ("cmsCloseProfile", u32, [ptr]),
    ]
    for name, result, args in signatures:
        function = getattr(lib, name)
        function.restype, function.argtypes = result, args

    class Transform(C.Structure):
        _fields_ = [("formats", u32 * 2), ("functions", ptr * 5),
                    ("cache", C.c_uint16 * 32), ("lut", ptr)]

    class Clut(C.Structure):
        _fields_ = [("table", C.POINTER(C.c_uint16)), ("params", ptr),
                    ("entries", u32), ("float_values", u32)]

    class Params(C.Structure):
        _fields_ = [("context", ptr), ("flags", u32), ("inputs", u32),
                    ("outputs", u32), ("samples", u32 * 15)]

    lab, rgb = lib.cmsCreateLab2Profile(None), lib.cmsCreate_sRGBProfile()
    # _imagingcms.c: LAB uses LabV2 plus an unused extra channel, RGB[A/X]
    # uses RGBA. Default intent PERCEPTUAL, flags zero, no alpha copying.
    transform = lib.cmsCreateTransform(lab, (30 << 16) | (1 << 7) | (3 << 3) | 1,
                                       rgb, (4 << 16) | (1 << 7) | (3 << 3) | 1, 0, 0)
    assert transform
    try:
        pipeline = C.cast(transform, C.POINTER(Transform)).contents.lut
        stage = lib.cmsPipelineGetPtrToFirstStage(pipeline)
        assert lib.cmsStageType(stage) == 0x636C7574 and not lib.cmsStageNext(stage)
        data = C.cast(lib.cmsStageData(stage), C.POINTER(Clut)).contents
        params = C.cast(data.params, C.POINTER(Params)).contents
        assert list(params.samples)[:3] == [33, 33, 33]
        assert params.inputs == params.outputs == 3
        assert data.entries == 33 ** 3 * 3 and not data.float_values
        return struct.pack("<" + "H" * data.entries, *data.table[:data.entries])
    finally:
        lib.cmsDeleteTransform(transform)
        lib.cmsCloseProfile(lab)
        lib.cmsCloseProfile(rgb)
