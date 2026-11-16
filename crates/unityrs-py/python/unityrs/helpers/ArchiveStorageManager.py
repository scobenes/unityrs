# Ported from UnityPy 1.25.2 (MIT): helpers/ArchiveStorageManager.py:12-143. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""UnityCN configuration and bounded standalone decryption helpers."""
import re
from .. import _native

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


def decrypt_key(key, data, keybytes):
    return _native.cn_key(key, data, keybytes)


def brute_force_key(fp, key_sig, data_sig, pattern=re.compile(rb"(?=(\w{16}))"), verbose=False):
    guard = _native.TreeTraversal()
    cap = _native.Limits().max_input_bytes
    data = bytearray()
    with open(fp, "rb") as stream:
        while True:
            size = min(65536, cap + 1 - len(data))
            guard.step(0, size * 2 + 256, size)
            block = stream.read(size)
            if not block:
                break
            data.extend(block)
            if len(data) > cap:
                raise _native.LimitError("UnityCN key search exceeds max_input_bytes")
    matches = []
    # Finditer preserves findall's group shape while charging each retained match.
    for match in pattern.finditer(data):
        size = sum(max(0, end - start) for start, end in match.regs)
        guard.step(0, 256 + size * 2, 1)
        matches.append(match.group(1) if len(match.groups()) == 1 else
                       match.groups() if match.groups() else match.group())
    for i, key in enumerate(matches):
        if verbose:
            print(f"Trying {i + 1}/{len(matches)} - {key}")
        if decrypt_key(key_sig, data_sig, key) == UNITY3D_SIGNATURE:
            if verbose:
                print(f"Found key: {key}")
            return key
    return None


class ArchiveStorageDecryptor:
    unknown_1: int
    index: bytes
    substitute: bytes = bytes(0x10)

    def __init__(self, reader):
        self.unknown_1 = reader.read_u_int()
        self.data, self.key = read_vector(reader)
        self.data_sig, self.key_sig = read_vector(reader)
        if DECRYPT_KEY is None:
            raise LookupError("\n".join([
                "The BundleFile is encrypted, but no key was provided!",
                "You can set the key via UnityPy.set_assetbundle_decrypt_key(key).",
                "To try brute-forcing the key, use UnityPy.helpers.ArchiveStorageManager.brute_force_key(fp, key_sig, data_sig)",
                f"with  key_sig = {self.key_sig}, data_sig = {self.data_sig},and fp being the path to global-metadata.dat or a memory dump.",
            ]))
        signature = decrypt_key(self.key_sig, self.data_sig, DECRYPT_KEY)
        if signature != UNITY3D_SIGNATURE:
            raise Exception(f"Invalid signature {signature} != {UNITY3D_SIGNATURE}")
        data = decrypt_key(self.key, self.data, DECRYPT_KEY)
        data = bytes(nibble for byte in data for nibble in (byte >> 4, byte & 0xF))
        self.index = data[:0x10]
        self.substitute = bytes(data[0x10 + i * 4 + j] for j in range(4) for i in range(4))

    def decrypt_block(self, data, index):
        return _native.cn_block(self.index, self.substitute, data, index)

    def decrypt_byte(self, view, offset, index):
        b = (self.substitute[((index >> 2) & 3) + 4] + self.substitute[index & 3]
             + self.substitute[((index >> 4) & 3) + 8] + self.substitute[(index % 256 >> 6) + 12])
        view[offset] = ((self.index[view[offset] & 0xF] - b) & 0xF
                        | 0x10 * (self.index[view[offset] >> 4] - b)) % 256
        return view[offset], offset + 1, index + 1

    def decrypt(self, data, index, remaining):
        token, offset, index = self.decrypt_byte(data, 0, index)
        literals = token >> 4
        if literals == 15:
            value = 255
            while value == 255:
                value, offset, index = self.decrypt_byte(data, offset, index)
                literals += value
        offset += literals
        if offset < remaining:
            _, offset, index = self.decrypt_byte(data, offset, index)
            _, offset, index = self.decrypt_byte(data, offset, index)
            if token & 15 == 15:
                value = 255
                while value == 255:
                    value, offset, index = self.decrypt_byte(data, offset, index)
        return offset
