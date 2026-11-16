"""Shared entry-point matrices: inputs exist before allocation measurement."""
import array
from types import SimpleNamespace

from binding_cases import S
from binding_y4_round3 import _container
from binding_y4_save import _asset
from binding_y4_exports import mesh
from binding_y4_images import texture, make


def native_cases(module):
    file = _container(module, bounded=True)
    web = _container(module, web=True, bounded=True)
    owner = file.files["child.assets"].objects[1]
    serialized = owner.assets_file
    cab = file.get_writeable_cab("new.resS")
    cab.write(b"abc")
    data = bytearray(8_000_000)
    text = "x" * 8_000_000
    array_data = array.array("B", data)
    modes = ["é" * 8_000_000 for _ in range(8)]
    native = module._native
    header = file._save_header(file._native.tree_traversal())
    rows = [(b"raw", 0, data)]
    metadata = ((22, "<", data, 0), None, None, True, 0, None)
    tree_rows = list(owner._get_typetree_node()._native_rows(use_boost=False, _guard=owner._native.tree_traversal()))
    def exhausted(call):
        guard = file._native.tree_traversal()
        budget = file.environment._budget
        guard.step(0, budget.limit - budget.used, 0)
        return call()
    return [
        ("audio", lambda: native.export_audio(data, b"name", owner._native)),
        ("audio_name", lambda: native.export_audio(b"RIFF", data, owner._native)),
        ("vorbis_bytes", lambda: native.export_audio(b"RIFF", b"name", owner._native, None, bytes_view)),
        ("vorbis_str", lambda: native.export_audio(b"RIFF", b"name", owner._native, None, text)),
        ("shader", lambda: native.export_shader({"m_Script": data}, (0, 0), owner._native)),
        ("mesh", lambda: native.export_mesh({"m_IndexBuffer": array_data}, 0, owner._native)),
        ("material", lambda: native.export_material({"m_Name": text}, owner._native, file.environment._native)),
        ("decode", lambda: native._image_decode(data, 1, 1, 4, (0, 0), 0, None, owner._native)),
        ("encode", lambda: native._image_encode(data, 1, 1, "RGBA", 4, 0, None, owner._native)),
        ("sprite", lambda: native._sprite_render(data, "RGBA", 1, 1, (0,0,1,1), 2, 1, None, owner._native)),
        ("image_resource", lambda: native._image_resource(file.environment._native, serialized._native, data, 0, 1)),
        ("set_raw_data", lambda: owner._native.set_raw_data(data)),
        ("save_typetree", lambda: owner._native.save_typetree({"value": 7, "name": text}, tree_rows)),
        ("cab_write", lambda: cab._native.write_bytes(data, 0)),
        ("cab_padding", lambda: cab._native.write_zeroes(0, len(data))),
        ("cab_data", lambda: exhausted(cab._native.data)),
        ("file_save", lambda: exhausted(lambda: serialized._native.save("save", None, 64, 64, "UnityWebData1.0"))),
        ("cab_name", lambda: file._native.get_writeable_cab(data, None)),
        ("cab_create", lambda: file._native.create_cab(data, 0, ">")),
        ("typetree_row_text", lambda: owner._native.save_typetree({}, [(0, b"Root", data, -1, 1, 0, 0, 0, 0, None)])),
        ("serialized", lambda: serialized._native.save_serialized_snapshot(metadata, [], [])),
        ("bundle", lambda: file._native.save_bundle_entries("save", rows, None, (64,64), header)),
        ("bundle_fs", lambda: file._native.save_bundle_entries("fs", rows, None, (64,64), header)),
        ("bundle_web_raw", lambda: file._native.save_bundle_entries("web_raw", rows, None, (64,64), header)),
        ("web", lambda: web._native.save_web_entries([(b"raw", data)], "none", b"UnityWebData1.0")),
        ("load_path", lambda: file.environment._native.load_path(text, False, False)),
        ("load_files", lambda: file.environment._native.load_files(text)),
        ("load_folder", lambda: file.environment._native.load_folder(text)),
        ("load_zip_path", lambda: file.environment._native.load_zip_path(text)),
        ("bundle_mode", lambda: file._native.save_bundle_entries(modes[0], rows, None, (64,64), header)),
        ("file_mode", lambda: file._native.save(modes[1], None, 64, 64, "UnityWebData1.0")),
        ("file_signature", lambda: web._native.save("save", None, 64, 64, modes[2])),
        ("encode_mode", lambda: native._image_encode(data, 1, 1, modes[3], 4, 0, None, owner._native)),
        ("sprite_mode", lambda: native._sprite_render(data, modes[4], 1, 1, (0,0,1,1), 2, 1, None, owner._native)),
        ("typetree_endian", lambda: owner._native.save_typetree({"value": 7, "name": text}, tree_rows, 0, modes[5])),
        ("web_packer", lambda: web._native.save_web_entries([(b"raw", data)], modes[6], b"UnityWebData1.0")),
        ("bundle_packer", lambda: file._native.save_bundle_entries("save", rows, modes[7], (64,64), header)),
    ]


# Keep an immutable oversized path without making it inside the measured call.
bytes_view = b"x" * 8_000_000


def python_case(module, operation):
    from PIL import Image
    file = _container(module, bounded=True)
    web = _container(module, web=True, bounded=True)
    owner = file.files["child.assets"].objects[1]
    data = bytearray(64 << 20)
    data[::4096] = b"x" * (len(data) // 4096)
    text = "x" * (64 << 20)
    if operation in ("samples_sequence", "image_sequence"):
        class Sequence:
            def __len__(self):
                return 1
            def __iter__(self):
                return iter(data)
        sequence = Sequence()
        if operation == "samples_sequence":
            obj = make(module, "AudioClip", m_AudioData=sequence, m_Resource=None, m_Name="clip", object_reader=owner)
            return lambda: obj.samples
        obj = texture(module, sequence, object_reader=owner)
        return lambda: obj.image
    obj = texture(module, data, object_reader=owner)
    cab = file.get_writeable_cab("new.resS")
    if operation == "object_write":
        from binding_y4_review_save import _ReviewWriter
        owner.data = data
        return lambda: owner.write(owner.assets_file.header, _ReviewWriter(), _ReviewWriter())
    if operation == "sprite_external_path":
        import importlib
        classes = importlib.import_module(module.__name__ + ".classes")
        owner.assets_file.externals = [SimpleNamespace(path="archive:/" + text + "/missing")]
        pointer = classes.PPtr(m_FileID=1, m_PathID=1, assetsfile=owner.assets_file)
        obj = make(module, "Sprite", object_reader=owner, m_SpriteAtlas=pointer)
        return lambda: obj.image
    if operation in ("serialized", "object_raw"):
        owner.data = data
        return owner.assets_file.save if operation == "serialized" else lambda: owner.set_raw_data(data)
    if operation in ("typetree", "object_save"):
        value = {"value": 7, "name": text}
        if operation == "typetree":
            return lambda: owner.save_typetree(value)
        value = owner.read()
        value.name = text
        return value.save
    if operation in ("bundle", "bundle_fs", "bundle_web_raw", "web"):
        target = web if operation == "web" else file
        target.files = {text: SimpleNamespace(flags=0, save=lambda: data)}
        if operation == "bundle_fs":
            from binding_y4_review_save import _ReviewWriter
            return lambda: target.save_fs(_ReviewWriter(), 64, 64)
        if operation == "bundle_web_raw":
            from binding_y4_review_save import _ReviewWriter
            return lambda: target.save_web_raw(_ReviewWriter())
        return target.save
    if operation == "environment":
        file.environment.files = {"prefix/" + text: file}
        file.mark_changed()
        return lambda: file.environment.save(out_path="/tmp")
    if operation in ("cab_write", "cab_save"):
        if operation == "cab_write":
            return lambda: cab.write(data)
        # A large writer can exist under a larger budget; reduce the remaining
        # allowance with the same owning meter before measuring its handoff.
        large = module.Environment(budget=module.Budget(256 << 20)).load_file(
            S.fs_bundle([(b"child.assets", _asset(), 4)]), name="large")
        cab = large.get_writeable_cab("new.resS")
        cab.write(data)
        guard = large._native.tree_traversal()
        guard.step(0, large.environment._budget.limit - large.environment._budget.used - 1024, 0)
        return lambda: (guard, cab.save())[1]
    if operation in ("image", "image_blob"):
        if operation == "image_blob":
            obj.image_data = b"1234"
            obj.m_PlatformBlob = data
        return lambda: obj.image
    if operation in ("set_image", "image_setter"):
        source = Image.new("RGBA", (4096, 4096))
        return (lambda: obj.set_image(source)) if operation == "set_image" else lambda: setattr(obj, "image", source)
    if operation == "array_images":
        obj = make(module, "Texture2DArray", image_data=data, m_Width=4096, m_Height=4096,
                   m_DataSize=len(data), m_Depth=1, m_Format=8, m_StreamData=None, object_reader=owner)
        return lambda: obj.images
    if operation == "sprite_image":
        obj = make(module, "Sprite", object_reader=owner, m_SpriteAtlas=None, m_AtlasTags=None,
                   m_RD=SimpleNamespace(texture=SimpleNamespace(path_id=1), alphaTexture=None))
        owner.assets_file._cache[1] = Image.new("RGBA", (4096, 4096))
        return lambda: obj.image
    if operation == "samples":
        obj = make(module, "AudioClip", m_AudioData=data, m_Resource=None, m_Name="clip", object_reader=owner)
        return lambda: obj.samples
    if operation == "shader_export":
        obj = make(module, "Shader", m_Script=data, m_SubProgramBlob=None, compressedBlob=None, object_reader=owner)
        return obj.export
    if operation == "mesh_export":
        obj = mesh(module)
        obj.object_reader = owner
        obj.m_IndexBuffer = array.array("B", data)
        return obj.export
    if operation == "renderer_export":
        obj = mesh(module)
        obj.m_Name = text
        renderer = make(module, "SkinnedMeshRenderer", object_reader=owner, m_Materials=[],
                        m_Mesh=SimpleNamespace(deref_parse_as_object=lambda: obj))
        return lambda: renderer.export("/tmp")
    raise AssertionError(operation)


PYTHON_CASES = ("serialized", "object_raw", "typetree", "object_save", "bundle", "bundle_fs",
    "bundle_web_raw", "web", "environment", "cab_write", "cab_save", "image", "image_blob",
    "set_image", "image_setter", "array_images", "sprite_image", "samples", "shader_export",
    "mesh_export", "renderer_export", "samples_sequence", "image_sequence", "object_write", "sprite_external_path")
