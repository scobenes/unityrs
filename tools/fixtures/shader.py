"""Record Shader.export on synthetic embedded-tree SerializedFiles.

Run with the pinned UnityPy 1.25.2 oracle; --check regenerates without writing.
Every tree, shader program and compressed slice is built here, without a TPK
or external asset. Both UnityPyBoost and the pure Python object reader must
produce the same export text or exception class and message.
"""
from __future__ import annotations

import argparse
import copy
import json
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
from synth import Node, Obj, Type, serialized_file, vec  # noqa: E402

OUT = ROOT / "crates/unityrs-export/tests/fixtures/shader.json"
STAGES = ("progVertex", "progFragment", "progGeometry", "progHull", "progDomain")


def leaf(name, kind="int"):
    return Node(kind, name, meta=0x4000 if kind in ("string", "bool") else 0)


def klass(kind, name, *children):
    return Node(kind, name, children)


def tags(name):
    return klass("SerializedTagMap", name, vec("tags", klass(
        "pair", "data", leaf("first", "string"), leaf("second", "string"))))


def tree(nested=False, *, omit=(), types=None, player=False):
    prop = klass("SerializedProperty", "data", vec("m_Attributes", leaf("data", "string")),
                 leaf("m_Name", "string"), leaf("m_Description", "string"), leaf("m_Type"),
                 leaf("m_Flags"), *(leaf(f"m_DefValue[{i}]", "float") for i in range(4)),
                 klass("SerializedTextureProperty", "m_DefTexture", leaf("m_DefaultName", "string"),
                       leaf("m_TexDim")))
    state = klass("SerializedShaderState", "m_State", leaf("m_Name", "string"), leaf("m_LOD"),
                  tags("m_Tags"), *(klass("SerializedShaderFloatValue", name, leaf("val", "float"),
                                         leaf("name", "string")) for name in (
                      "alphaToMask", "zClip", "zTest", "zWrite", "culling", "offsetFactor", "offsetUnits")),
                  leaf("lighting", "bool"), leaf("gpuProgramID"))
    sub = klass("SerializedSubProgram", "data", leaf("m_BlobIndex"), leaf("m_GpuProgramType"),
                leaf("m_ShaderHardwareTier"))
    passe = klass("SerializedPass", "data", leaf("m_Type"), leaf("m_UseName", "string"),
                  leaf("m_TextureName", "string"), state,
                  *(klass("SerializedProgram", stage, vec("m_SubPrograms", copy.deepcopy(sub)))
                    for stage in STAGES))
    parsed = klass("SerializedShader", "m_ParsedForm", leaf("m_Name", "string"),
                   klass("SerializedProperties", "m_PropInfo", vec("m_Props", prop)),
                   vec("m_SubShaders", klass("SerializedSubShader", "data", leaf("m_LOD"),
                                            tags("m_Tags"), vec("m_Passes", passe))),
                   leaf("m_FallbackName", "string"), leaf("m_CustomEditorName", "string"))
    def numbers(name):
        return vec(name, vec("data", leaf("data")) if nested else leaf("data"))
    root = klass("Shader", "Base", leaf("m_Name", "string"), parsed,
                 vec("platforms", leaf("data")), numbers("offsets"), numbers("compressedLengths"),
                 numbers("decompressedLengths"), vec("compressedBlob", leaf("data", "UInt8"), meta=0x4000),
                 vec("m_SubProgramBlob", leaf("data", "UInt8"), meta=0x4000), leaf("decompressedSize"),
                 vec("m_Script", leaf("data", "UInt8"), meta=0x4000))

    def vary(node):
        node.children = [child for child in node.children if child.name not in omit]
        for i, child in enumerate(node.children):
            if child.name in (types or {}):
                kind = types[child.name]
                data = (klass("pair", "data", leaf("first"), leaf("second"))
                        if kind == "pair" else leaf("data", kind))
                node.children[i] = (leaf(child.name, kind) if child.name == "m_Script"
                                    else vec(child.name, vec("data", data) if nested else data))
            else:
                vary(child)
        if player and node.type == "SerializedProgram":
            player_sub = klass("SerializedPlayerSubProgram", "data", leaf("m_BlobIndex"),
                               leaf("m_GpuProgramType"), vec("m_KeywordIndices", leaf("data")),
                               leaf("m_ShaderRequirements"))
            node.children.append(vec("m_PlayerSubPrograms", vec("data", player_sub)))
    vary(root)
    return root


def write(node, value, output):
    """A small writer for this hand-built tree; alignment is absolute in the object."""
    kind = node.type
    if kind in ("int", "float", "UInt8", "bool"):
        output.extend(struct.pack({"int": "<i", "float": "<f", "UInt8": "<B", "bool": "<?"}[kind], value))
    elif kind == "string":
        raw = value.encode("utf8")
        output.extend(struct.pack("<i", len(raw)) + raw)
        output.extend(b"\0" * (-len(output) % 4))
    elif node.children and node.children[0].type == "Array":
        output.extend(struct.pack("<i", len(value)))
        for item in value:
            write(node.children[0].children[1], item, output)
    elif kind == "pair":
        for child, item in zip(node.children, value, strict=True):
            write(child, item, output)
    else:
        for child in node.children:
            write(child, value[child.name], output)
    if node.meta & 0x4000 or (node.children and node.children[0].type == "Array" and node.children[0].meta & 0x4000):
        output.extend(b"\0" * (-len(output) % 4))


def aligned_string(text, output):
    raw = text.encode()
    output.extend(struct.pack("<i", len(raw)) + raw)
    output.extend(b"\0" * (-len(output) % 4))


def program(version, entries):
    """Program table and subprogram records, with real little-endian offsets."""
    stride = 12 if version >= (2019, 3) else 8
    output = bytearray(struct.pack("<i", len(entries)) + bytes(stride * len(entries)))
    for i, entry in enumerate(entries):
        struct.pack_into("<i", output, 4 + i * stride, len(output))
        rev = entry.get("revision", 201806140)
        output.extend(struct.pack("<ii", rev, entry["kind"]) + bytes(12))
        if rev >= 201608170:
            output.extend(bytes(4))
        keywords = entry.get("keywords", [])
        output.extend(struct.pack("<i", len(keywords)))
        for keyword in keywords:
            aligned_string(keyword, output)
        if 201806140 <= rev < 202012090:
            keywords = entry.get("local", [])
            output.extend(struct.pack("<i", len(keywords)))
            for keyword in keywords:
                aligned_string(keyword, output)
        code = entry.get("code", b"code\n")
        output.extend(struct.pack("<i", len(code)) + code)
        output.extend(b"\0" * (-len(output) % 4))
    return bytes(output)


def literal_lz4(raw):
    # One literal-only sequence avoids depending on an encoder's version.
    size = len(raw)
    output = bytearray([min(size, 15) << 4])
    if size >= 15:
        size -= 15
        while size >= 255:
            output.append(255)
            size -= 255
        output.append(size)
    output.extend(raw)
    return bytes(output)


def state(**changes):
    value = {"m_Name": "", "m_LOD": 0, "m_Tags": {"tags": []}, "lighting": False,
             "gpuProgramID": 0}
    value.update({key: {"val": number, "name": ""} for key, number in {
        "alphaToMask": 0.0, "zClip": 1.0, "zTest": 4.0, "zWrite": 1.0,
        "culling": 2.0, "offsetFactor": 0.0, "offsetUnits": 0.0}.items()})
    for key, item in changes.items():
        value[key] = {"val": item, "name": ""} if key in value and isinstance(value[key], dict) and "val" in value[key] else item
    return value


def passe(subs=(), stage="progVertex", **changes):
    value = {"m_Type": 0, "m_UseName": "", "m_TextureName": "", "m_State": state()}
    value.update({name: {"m_SubPrograms": list(subs) if name == stage else []} for name in STAGES})
    value.update(changes)
    return value


def sub(kind=1, index=0, tier=0):
    return {"m_BlobIndex": index, "m_GpuProgramType": kind, "m_ShaderHardwareTier": tier}


def prop(kind, values=(0.0, 0.0, 1.0, 0.0), dim=2, name="_Value", attrs=()):
    return {"m_Attributes": list(attrs), "m_Name": name, "m_Description": 'Label "é"',
            "m_Type": kind, "m_Flags": 0, **{f"m_DefValue[{i}]": v for i, v in enumerate(values)},
            "m_DefTexture": {"m_DefaultName": "white", "m_TexDim": dim}}


def make_case(name, *, version=(2021, 3, 9), platforms=(0,), entries=None, passes=None,
              props=(), nested=False, short=None, fallback="", editor="", sub_lod=0, sub_tags=(), script=None, overrides=None, schema=None):
    entries = [[{"kind": 1}]] if entries is None else entries
    blob, offsets, compressed, decompressed = bytearray(b"prefix"), [], [], []
    for items in entries:
        raw = program(version, items)
        packed = literal_lz4(raw)
        offsets.append(len(blob))
        compressed.append(len(packed))
        decompressed.append(len(raw))
        blob.extend(packed)
        blob.extend(b"gap")
    value = {"m_Name": "Synthetic Shader", "m_ParsedForm": {
        "m_Name": "Synthetic/Écho", "m_PropInfo": {"m_Props": list(props)},
        "m_SubShaders": [{"m_LOD": sub_lod, "m_Tags": {"tags": list(sub_tags)},
                          "m_Passes": [passe([sub()])] if passes is None else passes}],
        "m_FallbackName": fallback, "m_CustomEditorName": editor}, "platforms": list(platforms),
        "offsets": offsets, "compressedLengths": compressed, "decompressedLengths": decompressed,
        "compressedBlob": list(blob), "m_SubProgramBlob": [], "decompressedSize": 0, "m_Script": []}
    if script is not None:
        value["compressedBlob"] = []
        value["m_Script"] = list(script)
    if short:
        field, length = short if isinstance(short, tuple) else (short, 1)
        value[field] = value[field][:length]
    if nested:
        for field in ("offsets", "compressedLengths", "decompressedLengths"):
            # The second item must be ignored by get_entry.
            value[field] = [[n, 2147483647] for n in value[field]]
    value.update(overrides or {})
    root = tree(nested, **(schema or {}))
    output = bytearray()
    write(root, value, output)
    version_text = ".".join(map(str, version)) + "f1"
    data = serialized_file([Type(48, root)], [Obj(1, 0, bytes(output))], unity_version=version_text)
    return {"case": name, "version": version_text, "input": data.hex()}


def inputs():
    cases = []
    for version, rev in [((5, 5, 0), 201608170), ((5, 6, 0), 201609010),
                         ((2017, 3, 0), 201708220), ((2018, 2, 0), 201802150),
                         ((2019, 2, 0), 201806140), ((2019, 3, 0), 201806140),
                         ((2020, 1, 0), 201806140), ((2021, 2, 0), 202012090),
                         ((2021, 3, 9), 202012090), ((2021, 3, 10), 202012090)]:
        cases.append(make_case("version_" + "_".join(map(str, version)), version=version,
                               entries=[[{"kind": 1, "revision": rev, "keywords": ["GLOBAL", "É"],
                                          "local": ["LOCAL"], "code": b"void main() {}\n"}]]))
    numbers = [0.0, -0.0, 1.0, -1.0, 1.23456789, 999999.5, 1000000.0, 1e20, 1e-4, 1e-5,
               3.4028234663852886e38, 1.401298464324817e-45, float("inf"), float("-inf"), float("nan")]
    properties = []
    for i, number in enumerate(numbers):
        values = (number, number, -number, 0.25)
        for kind in range(4):
            properties.append(prop(kind, values, name=f"_P{i}_{kind}", attrs=("HDR", "Toggle(X)")))
    properties.extend(prop(4, dim=dim, name=f"_Tex{dim}") for dim in (-1, 0, 1, 2, 3, 4, 5, 6, 7, 2147483647))
    cases.append(make_case("properties_float_str_g_and_texture_dimensions", props=properties, passes=[]))
    cases.append(make_case("unknown_property_type", props=[prop(99)]))
    passes = []
    for ztest in range(10):
        passes.append(passe(m_State=state(m_Name=f"Pass {ztest}", m_LOD=ztest,
                      m_Tags={"tags": [("LightMode", "Forward"), ("dup", "a"), ("dup", "b")]},
                      alphaToMask=1.0, zClip=0.0, zTest=float(ztest), zWrite=0.0,
                      culling=float(ztest % 4), offsetFactor=-0.0 if ztest == 0 else 1e-5,
                      offsetUnits=-2.5, lighting=True, gpuProgramID=-7)))
    passes.extend([passe(m_Type=1, m_UseName='Other/PASS'), passe(m_Type=2),
                   passe(m_Type=2, m_TextureName="_Grab"), passe(m_Type=99)])
    cases.append(make_case("states_tags_lod_use_grab_unknown_pass", passes=passes, fallback="Diffuse",
                           editor="SyntheticEditor", sub_lod=200, sub_tags=[("Queue", "Geometry")]))
    mapping = {1: 0, 2: 9, 3: 9, 4: 9, 5: 5, 6: 15, 7: 15, 8: 15,
               **{n: 1 for n in range(9, 13)}, 13: 8, 14: 8,
               **{n: 4 for n in range(15, 23)}, 23: 14, 24: 14, 25: 18,
               **{n: 2 for n in range(26, 31)}}
    for kind in range(32):
        platform = mapping.get(kind, 0)
        # Metadata and blob kind are intentionally independent: this also
        # reaches MetalFS, Unknown and RayTracing in ShaderSubProgram.Export.
        metadata_kind = 23 if kind == 24 else 1 if kind in (0, 31) else kind
        code = b"entry\0metal source\n" if kind in (23, 24) else b"synthetic code\n"
        cases.append(make_case(f"program_kind_{kind}", platforms=[platform],
                               entries=[[{"kind": kind, "code": code, "keywords": ["K"], "local": ["L"]}]],
                               passes=[passe([sub(metadata_kind)])]))
    for platform in (-1, *range(21), 99):
        kind = {0: 1, 1: 9, 4: 15, 5: 5, 8: 13, 9: 2, 14: 23, 15: 6, 18: 25}.get(platform, 26)
        code = b"entry\0source" if kind == 23 else b"source"
        cases.append(make_case(f"platform_{platform}", platforms=[platform], entries=[[{"kind": kind, "code": code}]],
                               passes=[passe([sub(kind)])]))
    for stage in STAGES:
        cases.append(make_case(f"stage_{stage}", passes=[passe([sub()], stage=stage)]))
    cases.append(make_case("metal_header_offset", platforms=[14], entries=[[{
        "kind": 23, "code": struct.pack("<Ii", 0xF00DCAFE, 12) + b"skip" + b"main\0metal\n"}]],
        passes=[passe([sub(23)])]))
    cases.append(make_case("empty_code", entries=[[{"kind": 1, "code": b""}]]))
    cases.append(make_case("invalid_program_enum", entries=[[{"kind": 99}]]))
    cases.append(make_case("legacy_program_revision", entries=[[{"kind": 1, "revision": 201509030}]]))
    cases.append(make_case("unusable_program_omitted", platforms=[0], passes=[passe([sub(9)])]))
    cases.append(make_case("metal_fragment_metadata_omitted", platforms=[14],
                           entries=[[{"kind": 24, "code": b"main\0source"}]], passes=[passe([sub(24)])]))
    entries = [[{"kind": 9}, {"kind": 9}], [{"kind": 1, "code": b"index zero"},
                                             {"kind": 1, "code": b"index one"}]]
    passes = [passe([sub(1, 1, 2), sub(9, 0), sub(1, 0), sub(1, 1, 0), sub(1, 1, 1)])]
    for nested in (False, True):
        cases.append(make_case(f"multiple_platforms_tiers_sorted_nested_{nested}", platforms=[1, 0],
                               entries=entries, passes=passes, nested=nested))
        for field in ("compressedLengths", "decompressedLengths"):
            cases.append(make_case(f"short_{field}_nested_{nested}", platforms=[1, 0],
                                   entries=entries, passes=passes, nested=nested, short=field))
    cases.append(make_case("empty_platforms", platforms=[], entries=[], passes=[passe([sub()])]))
    for field in ("compressedLengths", "decompressedLengths"):
        cases.append(make_case(f"empty_{field}_stops_before_platform", short=(field, 0)))
    cases.append(make_case("first_usable_platform_wins", platforms=[0, 0], entries=[
        [{"kind": 1, "code": b"first", "keywords": ['QUOTE"', "BACK\\SLASH", "\n"]}],
        [{"kind": 1, "code": b"must not appear"}]]))
    cases.append(make_case("missing_blob_index", passes=[passe([sub(index=1)])]))
    for name, script in [("empty", b""), ("utf8", "// Shader É\n".encode()), ("invalid_utf8", b"\xff")]:
        cases.append(make_case(f"script_{name}", script=script))
    cases.append(make_case("unknown_blob_program_precedes_unknown_property",
                           entries=[[{"kind": 99}]], props=[prop(99)]))
    cases.append(make_case("unknown_second_program_precedes_first_output",
                           entries=[[{"kind": 1, "code": b"\xff"}, {"kind": 99}]]))
    cases.append(make_case("negative_compressed_length_uses_slice_end", passes=[], overrides={
        "compressedBlob": list(bytes.fromhex("4000000000")), "offsets": [0],
        "compressedLengths": [-1], "decompressedLengths": [4]}))
    raw = program((2021, 3, 9), [{"kind": 1, "code": b"size prefixed program"}])
    packed = struct.pack("<I", len(raw)) + literal_lz4(raw)
    cases.append(make_case("negative_decompressed_size_reads_prefix", overrides={
        "compressedBlob": list(packed), "offsets": [0], "compressedLengths": [len(packed)],
        "decompressedLengths": [-1]}))
    cases.append(make_case("negative_decompressed_size_minus2_reads_prefix", overrides={
        "compressedBlob": list(packed), "offsets": [0], "compressedLengths": [len(packed)],
        "decompressedLengths": [-2]}))
    for name, packed in [
        ("truncated", bytes(3)),
        ("above_signed_max", struct.pack("<I", 2147483648) + literal_lz4(raw)),
        ("length_mismatch", struct.pack("<I", len(raw) + 1) + literal_lz4(raw)),
    ]:
        cases.append(make_case(f"negative_decompressed_size_prefix_{name}", overrides={
            "compressedBlob": list(packed), "offsets": [0], "compressedLengths": [len(packed)],
            "decompressedLengths": [-1]}))
    first_raw = program((2021, 3, 9), [{"kind": 99}])
    first = literal_lz4(first_raw)
    cases.append(make_case("first_program_error_precedes_second_slice_decompression", platforms=[0, 0], overrides={
        "compressedBlob": list(first + b"\xff"), "offsets": [0, len(first)],
        "compressedLengths": [len(first), 1], "decompressedLengths": [len(first_raw), 4]}))
    cases.append(make_case("state_without_zClip", schema={"omit": ("zClip",)}))
    for name, value, schema in [
        ("script_missing", [], {"omit": ("m_Script",)}),
        ("script_string", "synthetic", {"types": {"m_Script": "string"}}),
    ]:
        cases.append(make_case(name, overrides={"compressedBlob": [], "m_Script": value}, schema=schema))
    for platform, stopped in [(False, False), (True, True)]:
        cases.append(make_case(f"missing_lengths_offsets_stopped_{stopped}",
                               platforms=[0] if platform else [], passes=[],
                               overrides={"compressedLengths": []},
                               schema={"omit": ("decompressedLengths", "offsets")}))
    cases.append(make_case("empty_platforms_missing_all_lengths_offsets", platforms=[], passes=[],
                           schema={"omit": ("compressedLengths", "decompressedLengths", "offsets")}))
    cases.append(make_case("empty_decompressed_missing_offsets", passes=[],
                           overrides={"decompressedLengths": []}, schema={"omit": ("offsets",)}))
    for nested in (False, True):
        cases.append(make_case(f"short_offsets_nested_{nested}", nested=nested, short=("offsets", 0)))
        for field in ("offsets", "compressedLengths", "decompressedLengths"):
            if nested:
                cases.append(make_case(f"empty_entry_{field}", nested=True, overrides={field: [[]]}))
            for kind, value in [("string", "bad"), ("float", 1.5)]:
                suffix = "_nested" if nested else ""
                cases.append(make_case(f"invalid_{field}_{kind}{suffix}", nested=nested,
                                       overrides={field: [[value]] if nested else [value]},
                                       schema={"types": {field: kind}}))
    cases.append(make_case("short_offsets_after_first_block", platforms=[0, 0],
                           entries=[[{"kind": 1}], [{"kind": 1}]], short=("offsets", 1)))
    for offset_type, offset in [("string", "bad"), ("float", 1.5)]:
        for length_type, length in [("string", "bad"), ("float", 1.5)]:
            cases.append(make_case(f"mixed_slice_types_{offset_type}_{length_type}",
                                   overrides={"offsets": [offset], "compressedLengths": [length]},
                                   schema={"types": {"offsets": offset_type, "compressedLengths": length_type}}))
    cases.append(make_case("converter_invalid_source_utf8", entries=[[{"kind": 1, "code": b"\xff"}]]))
    cases.append(make_case("negative_multidigit_hardware_tiers", passes=[passe([
        sub(tier=-12), sub(tier=-1), sub(tier=0), sub(tier=10), sub(tier=123)])]))
    for stage in STAGES:
        player_pass = passe()
        for name in STAGES:
            player_pass[name]["m_PlayerSubPrograms"] = [[{
                "m_BlobIndex": 0, "m_GpuProgramType": 1, "m_KeywordIndices": [0],
                "m_ShaderRequirements": 0}]] if name == stage else []
        cases.append(make_case(f"player_only_{stage}_2021_3_10", version=(2021, 3, 10),
                               passes=[player_pass], schema={"player": True}))
    # get_entry unwraps lists, but pair-typed elements are Python tuples.
    for field in ("offsets", "compressedLengths", "decompressedLengths"):
        cases.append(make_case(f"pair_entry_{field}", overrides={field: [(6, 999)]},
                               schema={"types": {field: "pair"}}))
    # Exercise bool identity and integer use in every entry position. Nested
    # bool arrays differ upstream: Boost returns lists, the pure reader tuples.
    for value in (False, True):
        for field in ("offsets", "compressedLengths", "decompressedLengths"):
            overrides = {"compressedBlob": list(b"x" + bytes.fromhex("4000000000")),
                         "offsets": [1], "compressedLengths": [5],
                         "decompressedLengths": [4], field: [value]}
            if field == "offsets":
                overrides["compressedBlob"] = list(b"x" * int(value) + bytes.fromhex("4000000000"))
            cases.append(make_case(f"bool_{field}_{value}", passes=[], overrides=overrides,
                                   schema={"types": {field: "bool"}}))
        for other_kind, other in (("string", "bad"), ("float", 1.5), ("bool", value)):
            for bool_field, other_field in (("offsets", "compressedLengths"),
                                            ("compressedLengths", "offsets")):
                # One both-bool case covers both operand positions.
                if other_kind == "bool" and bool_field == "compressedLengths":
                    continue
                cases.append(make_case(
                    f"bool_slice_{bool_field}_{value}_with_{other_kind}",
                    overrides={bool_field: [value], other_field: [other]},
                    schema={"types": {bool_field: "bool", other_field: other_kind}}))
    for value in (False, True):
        cases.append(make_case(f"bool_platform_{value}", platforms=[value],
                               entries=[[{"kind": 9 if value else 1}]],
                               passes=[passe([sub(9 if value else 1)])],
                               schema={"types": {"platforms": "bool"}}))
    return cases


def blob_inputs():
    version = (2021, 3)
    base = program(version, [{"kind": 1, "code": b"source"}])
    def case(name, data, index=0, ver=version):
        return {"case": name, "version": list(ver), "input": bytes(data).hex(), "index": index}
    def patched(offset, value):
        data = bytearray(base)
        struct.pack_into("<i", data, offset, value)
        return data
    cases = [case("valid", base), case("negative_index_last", base, -1),
             case("negative_index_past_start", base, -2), case("index_past_end", base, 1),
             case("negative_capacity", struct.pack("<i", -1)), case("zero_capacity", bytes(4)),
             case("short_capacity", b"\0\0"), case("short_table", struct.pack("<ii", 2, 16))]
    cases.append(case("negative_table_offset_minus4_wraps_fixed_field",
                      struct.pack("<iiiiii", 1, -4, 0, 0, 0, 4) + b"code" + bytes(4)))
    cases.append(case("negative_table_offset_minus8_wraps_two_fixed_fields",
                      struct.pack("<iiiiiii", 1, -8, 0, 0, 0, 0, 4) + b"code"
                      + struct.pack("<ii", 201806140, 1)))
    cases.append(case("negative_table_offset_minus8_unknown_kind",
                      struct.pack("<iiiiiii", 1, -8, 0, 0, 0, 0, 4) + b"code"
                      + struct.pack("<ii", 201806140, 99)))
    for offset in (-100, -4, -2, -1, 0, 1, len(base) - 4, len(base), len(base) + 1):
        cases.append(case(f"table_offset_{offset}", patched(4, offset)))
    for name, field in [("keyword_count", 40), ("local_keyword_count", 44), ("code_length", 48)]:
        for value in (-2, -1, 0, 1, 2147483647):
            cases.append(case(f"{name}_{value}", patched(field, value)))
    # Invalid aligned lengths return an empty keyword without consuming data.
    header = struct.pack("<iiii", 1, 16, 0, 0)
    record_header = struct.pack("<ii", 201806140, 1) + bytes(16)
    for length in (-2, -1, 0, 2147483647):
        raw = header + record_header + struct.pack("<iiii", 1, length, 0, 4) + b"code"
        cases.append(case(f"invalid_keyword_length_{length}", raw))
    for revision in (201509030, 201608169, 201608170, 201806139, 201806140, 202012089, 202012090):
        cases.append(case(f"revision_boundary_{revision}", program(version, [{
            "kind": 1, "revision": revision, "keywords": ["GLOBAL"], "local": ["LOCAL"]}])))
    for ver in ((2019, 2), (2019, 3)):
        cases.append(case(f"table_stride_{ver[0]}_{ver[1]}", program(ver, [
            {"kind": 1, "code": b"zero"}, {"kind": 1, "code": b"one"}]), index=1, ver=ver))
    for offset in (-100, -4, -2, -1, 0, 4, 8, 9, 100):
        code = struct.pack("<Ii", 0xF00DCAFE, offset) + b"main\0source"
        cases.append(case(f"metal_offset_{offset}", program(version, [{"kind": 23, "code": code}])))
    for name, code in [("short_header", b"abc"), ("empty", b""),
                       ("unterminated", b"ABCDentry"),
                       ("name_at_limit", b"ABCD" + b"n" * 32766 + b"\0source"),
                       ("name_past_limit", b"ABCD" + b"n" * 32768 + b"\0source"),
                       ("invalid_source_utf8", b"ABCDentry\0\xff"),
                       ("invalid_name_utf8", b"ABCD\xff\0source")]:
        cases.append(case(f"metal_{name}", program(version, [{"kind": 23, "code": code}])))
    for kind in (1, 8, 26, 30):
        cases.append(case(f"invalid_source_utf8_kind_{kind}", program(version, [{"kind": kind, "code": b"\xff"}])))
    cases.append(case("unknown_second_kind_precedes_first_decode", program(version, [
        {"kind": 1, "code": b"\xff"}, {"kind": 99}])))
    return cases


def outcome(call):
    try:
        return {"text": call()}
    except Exception as error:
        return {"exception": type(error).__name__, "message": str(error)}


def record_blob(case):
    from UnityPy.export.ShaderConverter import ShaderProgram
    from UnityPy.streams import EndianBinaryReader
    return outcome(lambda: ShaderProgram(EndianBinaryReader(bytes.fromhex(case["input"]), endian="<"),
                                         tuple(case["version"])).m_SubPrograms[case["index"]].Export())


def record(case, pure=False):
    import UnityPy
    from UnityPy.helpers import TypeTreeHelper
    old = TypeTreeHelper.read_typetree_boost
    if pure:
        TypeTreeHelper.read_typetree_boost = None
    try:
        env = UnityPy.Environment(bytes.fromhex(case["input"]))
        shader = next(o for o in env.objects if o.path_id == 1).parse_as_object()
        assert type(shader) is UnityPy.classes.Shader, type(shader)
        if case["case"].startswith("player_only_"):
            stage = case["case"].removeprefix("player_only_").removesuffix("_2021_3_10")
            program = getattr(shader.m_ParsedForm.m_SubShaders[0].m_Passes[0], stage)
            assert not program.m_SubPrograms
            assert type(program.m_PlayerSubPrograms[0][0]) is UnityPy.classes.SerializedPlayerSubProgram
            assert program.m_PlayerSubPrograms[0][0].m_BlobIndex == 0
        try:
            return {"text": shader.export()}
        except Exception as error:
            return {"exception": type(error).__name__, "message": str(error)}
    finally:
        TypeTreeHelper.read_typetree_boost = old


def dump_fixture(cases, blobs):
    """One complete case per line keeps the recording compact and diffs local."""
    compact = lambda value: json.dumps(value, ensure_ascii=False)
    lines = ['{', '  "reference": "UnityPy 1.25.2",', '  "tier": "A",']
    for field, values, comma in [("cases", cases, ","), ("blobs", blobs, "")]:
        lines.append(f'  "{field}": [')
        lines.extend("    " + compact(value) + ("," if i + 1 < len(values) else "")
                     for i, value in enumerate(values))
        lines.append("  ]" + comma)
    return "\n".join([*lines, "}", ""])


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    import UnityPy
    if UnityPy.__version__ != "1.25.2":
        parser.exit(2, "requires UnityPy 1.25.2\n")
    cases = []
    for case in inputs():
        try:
            oracle = record(case)
            assert oracle == record(case, pure=True), "Boost/pure reader difference"
        except Exception as error:
            raise RuntimeError(f"shader case {case['case']}: {error}") from error
        cases.append({**case, "oracle": oracle})
    blobs = [{**case, "oracle": record_blob(case)} for case in blob_inputs()]
    body = dump_fixture(cases, blobs)
    if args.check:
        if not OUT.exists() or OUT.read_bytes() != body.encode("utf8"):
            parser.exit(1, "stale or missing shader.json; regenerate in the pinned oracle\n")
    else:
        OUT.parent.mkdir(parents=True, exist_ok=True)
        OUT.write_bytes(body.encode("utf8"))
    print(f"shader: {len(cases)} synthetic cases, Boost and pure Python exports agree; {len(blobs)} blob cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
