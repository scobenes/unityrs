# Ported from UnityPy 1.25.2 (MIT): helpers/ArchiveStorageManager.py:12-30. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""UnityCN key configuration used by new environments."""
UNITY3D_SIGNATURE = b"#$unity3dchina!@"
DECRYPT_KEY = None


def set_assetbundle_decrypt_key(key):
    if isinstance(key, str):
        key = key.encode("utf-8", "surrogateescape")
    if len(key) != 16:
        raise ValueError(f"AssetBundle Key length is wrong. It should be 16 bytes and now is {len(key)} bytes.")
    global DECRYPT_KEY
    DECRYPT_KEY = key


def read_vector(reader):
    data = reader.read_bytes(0x10)
    key = reader.read_bytes(0x10)
    reader.Position += 1
    return data, key
