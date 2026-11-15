# Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Generator name normalization; object mode uses Boost's ASCII digit rule."""


def clean_name(name, *, ascii_digit=False):
    import re
    if not name:
        return name
    if name.startswith('(int&)'):
        name = name[6:]
    if name.endswith('?'):
        name = name[:-1]
    name = re.sub(r'[ .:\-\[\]]', '_', name)
    if name in ('pass', 'from'):
        name += '_'
    if name and (('0' <= name[0] <= '9') if ascii_digit else name[0].isdigit()):
        name = 'x' + name
    return name
