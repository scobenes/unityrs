# Ported from UnityPy 1.25.2 (MIT): export/MeshRendererExporter.py:25-189. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
def get_mesh(meshR):
    from ..classes._exports import _get_mesh
    return _get_mesh(meshR)


def export_mesh_renderer(renderer, export_dir):
    from ..classes._exports import _renderer_export
    return _renderer_export(renderer, export_dir)


def export_material(mat):
    from .. import _native
    from ..classes._exports import _reader, _environment, _guard
    guard = _guard(mat)
    for _, texture_env in mat.m_SavedProperties.m_TexEnvs:
        guard.step(0, 0, 1)
        if texture_env is not None and texture_env.m_Texture:
            raise NotImplementedError("unsupported export_material: textured material")
    return _native.export_material(mat, _reader(mat), _environment(mat), [])
