"""Tier C and U Shader fixtures, called by shader.py in the pinned oracle.

The expected Tier C text is assembled explicitly, independently of the Rust
converter. Parameter records deliberately contain no valid code header.
"""
import copy
import json
import struct

import shader as base

OUT = base.OUT.with_name("shader_modern.json")
LABELS = dict(zip(base.STAGES, ("vp", "fp", "gp", "hp", "dp"), strict=True))
HEADER = "//////////////////////////////////////////\n//\n// NOTE: This is *not* a valid shader file\n//\n///////////////////////////////////////////\n"


def expected(blocks):
    return HEADER + ('Shader "Synthetic/Écho" {\nProperties {\n\n}\n'
                     'SubShader {\n Pass {\n  GpuProgramID 0\n') + blocks + '}\n}\n}'


def block(stage, text, platform="openGL"):
    return f'Program "{LABELS[stage]}" {{\nSubProgram "{platform} " {{\n{text}\n}}\n}}\n'


def player_pass(groups, stage="progVertex", params=()):
    value = base.passe()
    for name in base.STAGES:
        value[name]["m_PlayerSubPrograms"] = copy.deepcopy(groups) if name == stage else []
        value[name]["m_ParameterBlobIndices"] = list(params) if name == stage else []
    return value


def sub(index=0, kind=1):
    return {"m_BlobIndex": index, "m_GpuProgramType": kind,
            "m_KeywordIndices": [0], "m_ShaderRequirements": 0}


def inputs():
    cases = []
    schema = {"player": True, "parameters": True}
    for version in ((2021, 3, 10), (2022, 1, 13), (2022, 2, 0), (6000, 3, 0)):
        for stage in base.STAGES:
            case = base.make_case(f"player_{stage}_{version[0]}_{version[1]}_{version[2]}",
                                  version=version, passes=[player_pass([[sub()]], stage)], schema=schema,
                                  entries=[[{"kind": 1, "revision": 202012090, "code": b"player code"}]])
            cases.append({**case, "rust": {"text": expected(block(stage, '"player code"'))}})
    for stage in base.STAGES:
        old = next(c for c in base.inputs() if c["case"] == f"player_only_{stage}_2021_3_10")
        cases.append({**old, "rust": {"text": expected(block(stage, '"code\n"'))}})
    # Parameter indices have the same group/variant coordinates as the player
    # lists. Each platform's table may use an index for a different record type.
    dxbc = '"// shader disassembly not supported on DXBC"'
    for name, groups, params in (
        ("player_variant_parameters_per_platform", [[sub(), sub(1, 15)]], [[1, 0]]),
        ("player_group_parameters_per_platform", [[], [sub()], [sub(1, 15)]], [[], [1], [0]]),
    ):
        case = base.make_case(name, version=(6000, 3, 0), schema=schema,
                              platforms=(0, 4), passes=[player_pass(groups, params=params)],
                              entries=[[{"kind": 1, "code": b"GL code"}, {"raw": b"PGlobals"}],
                                       [{"raw": b"PGlobals"}, {"kind": 15, "code": b"DX code"}]])
        variants = ('Program "vp" {\nSubProgram "openGL " {\n"GL code"\n}\n'
                    f'SubProgram "d3d11 " {{\n{dxbc}\n}}\n}}\n')
        cases.append({**case, "rust": {"text": expected(variants)}})
    case = base.make_case("player_first_usable_platform", version=(6000, 3, 0), schema=schema,
                          platforms=(4, 21), passes=[player_pass([[sub(kind=15)]])],
                          entries=[[{"kind": 15}], [{"kind": 15}]])
    cases.append({**case, "rust": {"text": expected(block("progVertex", dxbc, "d3d11"))}})
    for active in (False, True):
        value = player_pass([[sub()]] if active else [[], [], []])
        for stage in base.STAGES[1:]:
            value[stage]["m_PlayerSubPrograms"] = [[], [], []]
        case = base.make_case(f"player_empty_groups_active_{active}", schema=schema, passes=[value])
        cases.append({**case, "rust": {"text": expected(block("progVertex", '"code\n"') if active else "")}})
    # A parameter record can precede or follow code, and may resemble a
    # revision word. Neither is a ShaderSubProgram; never decode it as one.
    for parameter_first in (False, True):
        code_index, param_index = (1, 0) if parameter_first else (0, 1)
        entries = [{"kind": 1, "revision": 202012090, "code": b"6000.3 code"},
                   {"raw": struct.pack("<i", 202012090) + b"PGlobals\0"}]
        if parameter_first:
            entries.reverse()
        case = base.make_case(f"parameters_first_{parameter_first}_6000_3", version=(6000, 3, 0),
                              entries=[entries], passes=[player_pass([[sub(code_index)]], params=[[param_index]])],
                              schema=schema, nested=True)
        cases.append({**case, "rust": {"text": expected(block("progVertex", '"6000.3 code"'))}})
    groups = [[sub(1)], [], [sub(0), sub(1)]]
    case = base.make_case("player_groups_duplicate_sorted", version=(6000, 3, 0), schema=schema,
                          passes=[player_pass(groups)], entries=[[{"kind": 1, "revision": 202012090,
                          "code": b"zero"}, {"kind": 1, "revision": 202012090, "code": b"one"}]])
    variants = ''.join(f'SubProgram "openGL " {{\n"{code}"\n}}\n' for code in ("zero", "one", "one"))
    cases.append({**case, "rust": {"text": expected('Program "vp" {\n' + variants + '}\n')}})
    for name, groups, params, entries in [
        ("negative_code_index", [[sub(-1)]], [], [{"kind": 1}]),
        ("missing_code_index", [[sub(2)]], [], [{"kind": 1}]),
        ("negative_parameter_index", [[sub()]], [[-1]], [{"kind": 1}]),
        ("missing_parameter_index", [[sub()]], [[2]], [{"kind": 1}]),
        ("parameter_code_overlap", [[sub()]], [[0]], [{"kind": 1}]),
        ("referenced_bad_code", [[sub()]], [[1]], [{"kind": 99}, {"raw": b"PGlobals"}]),
    ]:
        case = base.make_case(name, version=(6000, 3, 0), schema=schema,
                              entries=[entries], passes=[player_pass(groups, params=params)])
        cases.append({**case, "rust": {"error": "ProgramType" if name == "referenced_bad_code" else "LayoutIndex"}})
    mixed = player_pass([[sub()]])
    mixed["progVertex"]["m_SubPrograms"] = [base.sub(index=-1)]
    for bad in (False, True):
        case = base.make_case(f"legacy_list_preferred_{bad}", schema=schema, passes=[mixed],
                              entries=[[{"kind": 99 if bad else 1}, {"kind": 1, "code": b"last"}]])
        cases.append({**case, "rust": {"error": "ProgramType"} if bad else {
            "text": expected(block("progVertex", '\"last\"'))}})
    mixed = player_pass([], params=[[0]])
    mixed["progVertex"]["m_SubPrograms"] = [base.sub(index=-1)]
    mixed["progFragment"]["m_PlayerSubPrograms"] = [[sub(1)]]
    case = base.make_case("mixed_stages_signed_legacy_index", schema=schema, passes=[mixed],
                          entries=[[{"raw": b"parameter"}, {"kind": 1, "code": b"last"}]])
    cases.append({**case, "rust": {"text": expected(block("progVertex", '\"last\"') +
                                                       block("progFragment", '\"last\"'))}})
    # The 12-byte table is (offset, length, segment). Parameters stay in
    # segment zero; the referenced code is in the next compressed segment.
    code = base.program((6000, 3), [{"kind": 1, "revision": 202012090, "code": b"segmented"}])[16:]
    param = struct.pack("<i", 202012090) + b"PGlobals"
    for name, segment, length in (("valid", 1, len(code)), ("missing", 2, len(code)),
                                   ("short_record", 1, 8), ("outside_record", 1, len(code) + 4)):
        table = struct.pack("<iiiiiii", 2, 0, length, segment, 28, len(param), 0) + param
        packed = [base.literal_lz4(data) for data in (table, code)]
        case = base.make_case(f"segments_6000_3_{name}", version=(6000, 3, 0), nested=True,
                              schema=schema, passes=[player_pass([[sub()]], params=[[1]])],
                              overrides={"compressedBlob": list(b"".join(packed)),
                              "offsets": [[0, len(packed[0])]],
                              "compressedLengths": [[len(p) for p in packed]],
                              "decompressedLengths": [[len(table), len(code)]]})
        cases.append({**case, "rust": {"text": expected(block("progVertex", '\"segmented\"'))}
                      if name == "valid" else {"error": "LayoutIndex"}})
    # Native legacy scripts are strings. UnityPy reads the default-BE blob,
    # then bytes(str) raises; also record its direct replacement operation.
    for minor, revision in ((3, 201509030), (4, 201510240)):
        raw = struct.pack(">iiiiiiiiii", 1, 12, 24, revision, 1, 0, 0, 0, 0, 0)
        variants = [("valid", raw, "GpuProgramIndex 0\n", {"text": HEADER + '\"\"\n'})]
        for name, data in [("negative_count", struct.pack(">i", -1)),
                           ("short_table", struct.pack(">i", 2)),
                           ("negative_offset", struct.pack(">iii", 1, -1, 0)),
                           ("outside_offset", struct.pack(">iii", 1, 999, 0)),
                           ("short_header", raw[:20]),
                           ("unknown_kind", raw[:16] + struct.pack(">i", 99) + raw[20:]),
                           ("negative_keywords", raw[:32] + struct.pack(">i", -1) + raw[36:]),
                           ("negative_code_length", raw[:36] + struct.pack(">i", -1))]:
            variants.append((name, data, "GpuProgramIndex 0\n", {"refusal": f"BigEndian5{minor}Blob"}))
        for number in ("1", "-2", "bad", "0x0", "0.0", "_0", "0_", "0__0", "\x1c0", "0 }", "0" * 4301):
            variants.append((f"index_bad_{len(variants)}", raw, f"GpuProgramIndex {number}\n", {"refusal": "GpuProgramIndex"}))
        for i, number in enumerate(("-1", "+0", "٠", "0_0", "\xa00\x85", "　0\r", "0" * 4300)):
            variants.append((f"index_good_{i}", raw, f"GpuProgramIndex {number}\n", {"text": HEADER + '\"\"\n'}))
        for name, data, script, rust in variants:
            case = base.make_case(f"big_endian_5_{minor}_{name}", version=(5, minor, 0),
                                  overrides={"m_SubProgramBlob": list(base.literal_lz4(data)),
                                  "decompressedSize": len(data), "m_Script": script},
                                  schema={"types": {"m_Script": "string"}})
            cases.append({**case, "rust": rust, "replacement_blob": data.hex(), "replacement_script": script})
    return cases


def generate(check):
    cases = []
    for case in inputs():
        oracle = base.record(case)
        assert oracle == base.record(case, pure=True), case["case"]
        recorded = {**case, "oracle": oracle}
        if "replacement_blob" in case:
            from UnityPy.export.ShaderConverter import ShaderProgram
            from UnityPy.streams import EndianBinaryReader
            recorded["replacement_oracle"] = base.outcome(lambda: ShaderProgram(
                EndianBinaryReader(bytes.fromhex(case["replacement_blob"])), (5, 4)).Export(case["replacement_script"]))
        cases.append(recorded)
    for suffix, selected in (("modern", [c for c in cases if "replacement_blob" not in c]),
                             ("legacy", [c for c in cases if "replacement_blob" in c])):
        out = OUT.with_name(f"shader_{suffix}.json")
        body = '{\n  "reference": "UnityPy 1.25.2",\n  "tier": "C/U",\n  "cases": [\n'
        body += ',\n'.join('    ' + json.dumps(c, ensure_ascii=False) for c in selected)
        body += '\n  ]\n}\n'
        if check:
            if not out.exists() or out.read_bytes() != body.encode():
                raise SystemExit(f"stale or missing {out.name}; regenerate with shader.py")
        else:
            out.write_bytes(body.encode())
    print(f"shader modern: {len(cases)} synthetic L7 cases, Boost/pure agree")
