"""Synthetic module/re-export observations; no generated TPK data is retained."""
import importlib
import io
import struct
import json
from pathlib import Path
from types import SimpleNamespace


def _module(package, path):
    return importlib.import_module(package.__name__ + "." + path)


def runtime_class_map(package):
    """Record only in memory: this map may derive from the runtime TPK."""
    mapping = _module(package, "classes.ClassIDTypeToClassMap").ClassIDTypeToClassMap
    return [[int(key), cls.__name__] for key, cls in mapping.items()]


def observe_reexports(package):
    result = {}
    for path, names in {
        "streams": ["EndianBinaryReader", "EndianBinaryWriter"],
        "helpers": ["ArchiveStorageManager", "CompressionHelper", "ImportHelper", "TypeTreeHelper", "ContainerHelper", "UnityVersion"],
        "export": ["AudioClipConverter", "MeshExporter", "MeshRendererExporter", "SpriteHelper", "Texture2DConverter"],
    }.items():
        module = _module(package, path)
        result[path] = {name: getattr(module, name) is getattr(_module(package, path + "." + name), name, _module(package, path + "." + name)) for name in names}
        if hasattr(module, "__all__"):
            result[path + "_all"] = list(module.__all__)
    result["root"] = [package.load is package.Environment, package.AssetsManager is package.Environment,
                      package.set_assetbundle_decrypt_key is _module(package, "helpers.ArchiveStorageManager").set_assetbundle_decrypt_key]
    legacy = _module(package, "classes.legacy_patch")
    classes = _module(package, "classes")
    result["legacy"] = {name: getattr(legacy, name) is getattr(classes, name) for name in (
        "AudioClip", "GameObject", "Mesh", "Renderer", "Shader", "Sprite", "Texture2D", "Texture2DArray")}
    result["CIT"] = _module(package, "classes.ClassIDTypeToClassMap").CIT is package.enums.ClassIDType
    environment = _module(package, "environment")
    node_module = _module(package, "helpers.TypeTreeNode")
    result["environment"] = [environment.Environment is package.Environment,
                             environment.reSplit.pattern]
    result["node_base"] = node_module.TypeTreeNode.__bases__[0] is node_module.TypeTreeNodeC
    return result


def observe_streams(package):
    streams = _module(package, "streams")
    reader_module = _module(package, "streams.EndianBinaryReader")
    rows = []
    for endian in ("<", ">"):
        writer = streams.EndianBinaryWriter(endian=endian)
        for name, value in (("int", -19), ("u_short", 65534), ("double", 2.5), ("boolean", True)):
            getattr(writer, "write_" + name)(value)
        writer.write_aligned_string("a\udcff")
        data = writer.bytes
        for source in (data, io.BytesIO(data)):
            reader = streams.EndianBinaryReader(source, endian)
            values = [getattr(reader, "read_" + name)() for name in ("int", "u_short", "double", "boolean")]
            values.append(reader.read_aligned_string().encode("utf-8", "surrogateescape").hex())
            rows.append([values, reader.Position, reader.Length, reader.bytes.hex()])
    return {"rows": rows, "loop": [reader_module.endian_s, reader_module.reader_type_name,
            reader_module.struct_type_char, reader_module.func_name, reader_module.struct.format,
            reader_module.memory_read_func is reader_module.MEMORY_FUNCTIONS[">"]["read_double"],
            reader_module.stream_read_func is reader_module.STREAM_FUNCTIONS[">"]["read_double"]]}


def observe_module_tables(package):
    helper = _module(package, "helpers.TypeTreeHelper")
    converter = _module(package, "export.Texture2DConverter")
    compression = _module(package, "helpers.CompressionHelper")
    return {"primitives": [[key, sorted(getattr(helper, key))] for key in ("FUNCTION_READ_MAP", "FUNCTION_READ_MAP_ARRAY", "FUNCTION_WRITE_MAP")],
            "alignment": helper.kAlignBytes,
            "texture_loop": [converter.tf.name, converter.split, converter.block_size],
            "texture_alias": converter.TF is package.enums.TextureFormat,
            "blocks": [[key.name, list(value) if value else None] for key, value in converter.TEXTURE_FORMAT_BLOCK_SIZE_TABLE.items()],
            "padding": converter.RG_PADDING_MAP,
            "compression": [sorted(int(k) for k in getattr(compression, name)) for name in ("COMPRESSION_MAP", "DECOMPRESSION_MAP")],
            "chunks": sorted([int(k), v] for k, v in compression.COMPRESSION_CHUNK_SIZE_MAP.items())}


def observe_primitive_maps(package):
    maps = _module(package, "helpers.TypeTreeHelper")
    streams = _module(package, "streams")
    rows = []
    for name, write in maps.FUNCTION_WRITE_MAP.items():
        value = "abc" if name == "string" else b"abc" if name == "TypelessData" else 2
        writer = streams.EndianBinaryWriter()
        write(writer, value)
        reader = streams.EndianBinaryReader(writer.bytes)
        result = maps.FUNCTION_READ_MAP[name](reader)
        if isinstance(result, bytes):
            result = result.hex()
        rows.append([name, writer.bytes.hex(), result, reader.Position])
    return rows


def observe_compression(package):
    module = _module(package, "helpers.CompressionHelper")
    rows = []
    for data in (b"", b"x", b"abc" * 200, bytes(range(256)) * 600):
        for kind in ("lzma", "lz4"):
            packed = getattr(module, "compress_" + kind)(data)
            decoded = getattr(module, "decompress_" + kind)(packed, len(data)) if kind == "lz4" else module.decompress_lzma(packed)
            rows.append([kind, packed.hex(), decoded == data])
        packed = module.compress_lzma(data, True)
        rows.append(["lzma-sized", packed.hex(), module.decompress_lzma(packed, True) == data])
        packed = module.compress_brotli(data)
        # The core's existing stored-metablock Brotli writer is Tier B for
        # compressed bytes. The public helper preserves decoded bytes.
        rows.append(["brotli-roundtrip", len(data), module.decompress_brotli(packed) == data])
    # UnityPy's quality-11 encoder, on this tool's own synthetic payload.
    packed = bytes.fromhex("1b6300f88d94aba35b390727a437bcdeddef4251a2b44760182880f85186cad0adba30bf970e")
    rows.append(["brotli-oracle-input", module.decompress_brotli(packed).hex()])
    return rows


def observe_texture_converter(package):
    from PIL import Image
    module = _module(package, "export.Texture2DConverter")
    data = bytes([1, 2, 3, 255, 4, 5, 6, 255, 10, 20, 30, 255, 40, 50, 60, 255])
    rows = []
    for flip in (True, False):
        image = module.parse_image_data(data, 2, 2, package.enums.TextureFormat.RGBA32, (2020, 3, 0, 0), 0, flip=flip)
        packed, target = module.image_to_texture2d(image, package.enums.TextureFormat.RGBA32, flip=flip)
        rows.append([image.mode, list(image.size), image.tobytes().hex(), packed.hex(), int(target)])
    return rows


def observe_converter_helpers(package):
    module = _module(package, "export.Texture2DConverter")
    cases = [
        ("half", struct.pack("<3e", 0.25, 0.5, 0.75), (1, 1, "RGB", "raw", "RGB")),
        ("rg", bytes(range(16)), (1, 1, "RGB", "RG;8s", "RGB")),
        ("rg", bytes(range(16)), (1, 1, "RGB", "RG", "RGB")),
        ("rgb9e5float", bytes(4), (1, 1)),
        ("atc", bytes(8), (4, 4, False)),
        ("pvrtc", bytes(32), (8, 8, True)),
        ("etc", bytes(8), (4, 4, "ETC1")),
        ("eac", bytes(8), (4, 4, "EAC_R")),
    ]
    rows = []
    for name, data, args in cases:
        try:
            image = getattr(module, name)(data, *args)
            result = [image.mode, list(image.size), image.tobytes().hex()]
        except Exception as error:
            result = [type(error).__name__, str(error)]
        rows.append([name, result])
    return rows


def observe_export_modules(package):
    from binding_y4_exports import mesh
    from binding_y4_images import texture
    classes = _module(package, "classes")
    audio = classes.AudioClip(m_Name="synthetic", m_AudioData=b"OggS synthetic")
    shader = classes.Shader(m_Name="synthetic", m_Script=b"Shader { synthetic }")
    geometry = mesh(package)
    tex = texture(package)
    image = _module(package, "export.Texture2DConverter").get_image_from_texture2d(tex)
    return {
        "audio": {name: data.hex() for name, data in _module(package, "export.AudioClipConverter").extract_audioclip_samples(audio).items()},
        "mesh": _module(package, "export.MeshExporter").export_mesh(geometry),
        "obj_materials": _module(package, "export.MeshExporter").export_mesh_obj(geometry, ["synthetic"]),
        "shader": _module(package, "export.ShaderConverter").export_shader(shader),
        "shader_header": _module(package, "export.ShaderConverter").HEADER,
        "texture": [image.mode, list(image.size), image.tobytes().hex()],
    }


def observe_untextured_material(package):
    """Empty texture properties exercise the oracle's supported material path."""
    from binding_y4_exports import _new
    classes = _module(package, "classes")
    exporter = _module(package, "export.MeshRendererExporter")
    rows = []
    for name, floats in (
        ("plain", []),
        ("polished", [("_Shininess", 7.5), ("_Transparency", 0.25)]),
    ):
        material = _new(classes.Material, m_Name=name,
                        m_Shader=classes.PPtr(m_FileID=0, m_PathID=0),
                        m_SavedProperties=_new(classes.UnityPropertySheet, m_TexEnvs=[],
                            m_Colors=[], m_Floats=floats))
        rows.append(exporter.export_material(material))
    return rows


def observe_texture_extension(package, platform=0, platform_blob=None):
    from PIL import Image
    module = _module(package, "export.Texture2DConverter")
    key = package.enums.TextureFormat.RGBA32
    previous = module.CONV_TABLE[key]
    calls = []
    def conversion(data, width, height, marker):
        calls.append([data.hex(), width, height, marker])
        return Image.new("RGBA", (width, height), (11, 22, 33, 44))
    try:
        module.CONV_TABLE[key] = (conversion, ("custom",))
        image = module.parse_image_data(bytes(16), 2, 2, key, (2020, 3, 0, 0), platform, platform_blob)
        return [calls, image.tobytes().hex()]
    finally:
        module.CONV_TABLE[key] = previous


def observe_decrypt_key(package):
    module = _module(package, "helpers.ArchiveStorageManager")
    old = module.DECRYPT_KEY
    result = []
    try:
        for key in (b"0123456789abcdef", "0123456789abcdef", b"short", "\udcff" * 16):
            try:
                package.set_assetbundle_decrypt_key(key)
                result.append(["ok", module.DECRYPT_KEY.hex()])
            except Exception as error:
                result.append([type(error).__name__, str(error)])
    finally:
        module.DECRYPT_KEY = old
    return result


def observe_generator(package):
    module = _module(package, "helpers.TypeTreeGenerator")
    try:
        module.TypeTreeGenerator("2020.3.0f1")
    except ImportError as error:
        return [type(error).__name__, str(error)]
    return ["installed"]


def observe_generator_cache(package):
    module = _module(package, "helpers.TypeTreeGenerator")
    class Generator(module.TypeTreeGenerator):
        def __init__(self):
            self.cache = {}
            self.calls = []

        def get_nodes(self, assembly, fullname):
            self.calls.append([assembly, fullname])
            return [SimpleNamespace(m_Level=0, m_Type="Synthetic", m_Name="Base", m_MetaFlag=0),
                    SimpleNamespace(m_Level=1, m_Type="int", m_Name="value", m_MetaFlag=0)]
    generator = Generator()
    first = generator.get_nodes_up("Assembly", "Synthetic")
    second = generator.get_nodes_up("Assembly", "Synthetic")
    third = generator.get_nodes_up("Assembly.dll", "Synthetic")
    return [generator.calls, first is second, first is third,
            [[node.m_Level, node.m_Type, node.m_Name, node.m_MetaFlag]
             for node in first.traverse()]]


def observe_encrypted_load(package):
    """The existing published synthetic UnityCN input contains no game data."""
    module = _module(package, "helpers.ArchiveStorageManager")
    fixture = Path(__file__).resolve().parents[2] / "crates/unityrs-core/tests/fixtures/unitycn/oracle.json"
    inputs = json.loads(fixture.read_text())["inputs"]
    old = module.DECRYPT_KEY
    rows = []
    try:
        for case in inputs["bundles"]:
            env = package.Environment()
            package.set_assetbundle_decrypt_key(bytes.fromhex(inputs["key"]))
            env.load_file(bytes.fromhex(case["hex"]), name="synthetic.bundle")
            leaf = env.files["synthetic.bundle"]
            for name in case.get("path", ["synthetic.resS"]):
                leaf = leaf.files[name]
            rows.append([case["name"], leaf.bytes.hex()])
    finally:
        module.DECRYPT_KEY = old
    return rows
