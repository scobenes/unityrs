# Ported from UnityPy 1.25.2 (MIT): export/MeshRendererExporter.py:25-189. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
def get_mesh(meshR):
    from ..classes._exports import _get_mesh
    return _get_mesh(meshR)


def export_mesh_renderer(renderer, export_dir):
    from ..classes._exports import _renderer_export
    return _renderer_export(renderer, export_dir)


def export_material(mat):
    from .. import _native
    from ..classes._exports import _reader, _environment, _guard, _material_texture_names
    guard = _guard(mat)
    texture_names, textures = _material_texture_names(mat, guard)
    return _native.export_material(mat, _reader(mat), _environment(mat), texture_names)
