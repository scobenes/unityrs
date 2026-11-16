# Ported from UnityPy 1.25.2 (MIT): export/ShaderConverter.py:30-36, export/ShaderConverter.py:41-55. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
HEADER = """//////////////////////////////////////////
//
// NOTE: This is *not* a valid shader file
//
///////////////////////////////////////////
"""


def export_shader(m_Shader):
    from ..classes._exports import _shader_export
    return _shader_export(m_Shader)
