# Ported from UnityPy 1.25.2 (MIT): export/MeshExporter.py:11-56. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
def export_mesh(m_Mesh, format="obj"):
    from ..classes._exports import _mesh_export
    return _mesh_export(m_Mesh, format)


def export_mesh_obj(mesh, material_names=None):
    from ..classes._exports import _mesh_obj
    return _mesh_obj(mesh, material_names)
