# Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.
"""Fixture observation names (UnityPyBoost/TypeTreeHelper.cpp:28-65)."""
import re


def clean_name(name, *, ascii_digit=True):
    if name.startswith('(int&)'):
        name = name[6:]
    if name.endswith('?'):
        name = name[:-1]
    name = re.sub(r'[ .:\-\[\]]', '_', name)
    if name in ('pass','from'):
        name += '_'
    if name and ('0' <= name[0] <= '9'):
        name = 'x' + name
    return name
