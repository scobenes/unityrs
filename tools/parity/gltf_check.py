#!/usr/bin/env python3
"""Standard-library L6g accessor and winding checks (static3d sections 2.4/3.2).

Expected JSON: a list of {mesh, primitive, attributes, indices, targets} records.
Attributes and targets contain dense rows in Unity coordinates, as recorded by
MeshHandler. Missing optional keys are not compared. Values are rounded to f32;
normalise follows the writer's documented rules. No exporter is used as oracle.
"""
import argparse
import json
import math
import struct
from pathlib import Path


def read_glb(path):
    data = Path(path).read_bytes()
    if len(data) < 20 or data[:4] != b"glTF":
        raise ValueError("GLB header")
    version, length = struct.unpack_from("<II", data, 4)
    if version != 2 or length != len(data):
        raise ValueError("GLB version/length")
    chunks, at = [], 12
    while at < len(data):
        size, kind = struct.unpack_from("<I4s", data, at)
        at += 8
        if size % 4 or at + size > len(data):
            raise ValueError("GLB chunk range/alignment")
        chunks.append((kind, data[at:at + size]))
        at += size
    if not chunks or chunks[0][0] != b"JSON" or len(chunks) > 2:
        raise ValueError("GLB chunk order")
    if len(chunks) == 2 and chunks[1][0] != b"BIN\0":
        raise ValueError("GLB BIN chunk")
    return json.loads(chunks[0][1]), chunks[1][1] if len(chunks) == 2 else b""


WIDTH = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}
FORMAT = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}


def accessor(doc, binary, index):
    a = doc["accessors"][index]
    width, count = WIDTH[a["type"]], a["count"]
    code = FORMAT[a["componentType"]]
    size = struct.calcsize("<" + code)

    def rows(view_id, offset, n, row_width, row_code):
        view = doc["bufferViews"][view_id]
        if view.get("buffer", 0) != 0:
            raise ValueError("external buffer")
        fmt = "<" + row_code * row_width
        row_size = struct.calcsize(fmt)
        stride = view.get("byteStride", row_size)
        start = view.get("byteOffset", 0)
        if offset < 0 or stride < row_size or (n and offset + (n-1)*stride + row_size > view["byteLength"]):
            raise ValueError("accessor view range")
        if start + view["byteLength"] > len(binary):
            raise ValueError("buffer view range")
        return [list(struct.unpack_from(fmt, binary, start + offset + i*stride)) for i in range(n)]

    result = rows(a["bufferView"], a.get("byteOffset", 0), count, width, code) if "bufferView" in a else [[0]*width for _ in range(count)]
    if "sparse" in a:
        s = a["sparse"]
        inds, vals = s["indices"], s["values"]
        sparse_ids = rows(inds["bufferView"], inds.get("byteOffset", 0), s["count"], 1, FORMAT[inds["componentType"]])
        values = rows(vals["bufferView"], vals.get("byteOffset", 0), s["count"], width, code)
        previous = -1
        for (i,), value in zip(sparse_ids, values):
            if i <= previous or i >= count:
                raise ValueError("sparse index order/range")
            result[i], previous = value, i
    if a.get("normalized") and code != "f":
        bits = size * 8
        signed = code.islower()
        divisor = (1 << (bits - 1 if signed else bits)) - 1
        result = [[max(-1, v/divisor) if signed else v/divisor for v in row] for row in result]
    return result


def f32(v):
    try:
        return struct.unpack("<f", struct.pack("<f", v))[0]
    except OverflowError:
        return math.copysign(math.inf, v)


def converted(semantic, values, normalise=True, morph=False):
    out = []
    for source in values:
        row = list(source)
        if semantic in ("POSITION", "NORMAL", "TANGENT"):
            row[0] = -row[0]
            if semantic == "TANGENT" and not morph:
                row[3] = -row[3]
        if semantic.startswith("TEXCOORD_"):
            row[1] = 1 - row[1]
        row = list(map(f32, row))
        if normalise and not morph:
            if semantic in ("NORMAL", "TANGENT"):
                length = math.sqrt(sum(v*v for v in row[:3]))
                row[:3] = [f32(v/length) for v in row[:3]]
                if semantic == "TANGENT":
                    row[3] = math.copysign(1.0, row[3])
            if semantic == "WEIGHTS_0":
                row = [v if math.isfinite(v) and v > 0 else 0.0 for v in row]
                total = sum(row)
                row = [f32(v/total) for v in row] if total else [1., 0., 0., 0.]
        out.append(row)
    return out


def equal(actual, expected, label):
    if len(actual) != len(expected) or any(len(a) != len(b) for a, b in zip(actual, expected)):
        raise AssertionError(f"{label}: array shape")
    for i, (a, b) in enumerate(zip(actual, expected)):
        if any(not (x == y or (math.isnan(x) and math.isnan(y))) for x, y in zip(a, b)):
            raise AssertionError(f"{label} row {i}: {a} != {b}")


def expected_omissions(attrs):
    """Decide omissions from rounded source directions before unit normalization."""
    omitted = set()
    for semantic in ("NORMAL", "TANGENT"):
        for row in attrs.get(semantic, []):
            xyz = list(map(f32, row[:3]))
            if not all(math.isfinite(v) for v in xyz) or not any(xyz):
                omitted.add(semantic)
                if semantic == "NORMAL":
                    omitted.add("TANGENT")
                break
    return omitted


def check_accessors(doc, binary, expected, normalise=True):
    for case in expected:
        if "skin" in case:
            matrices = case["matrices"]
            values = [[f32(matrix[row*4+col] * (-1 if (row == 0) != (col == 0) else 1)) for col in range(4) for row in range(4)] for matrix in matrices]
            equal(accessor(doc, binary, doc["skins"][case["skin"]]["inverseBindMatrices"]), values, "inverse binds")
            continue
        p = doc["meshes"][case["mesh"]]["primitives"][case["primitive"]]
        attrs = case.get("attributes", {})
        omitted = expected_omissions(attrs) if normalise else set()
        for semantic in sorted(omitted):
            if semantic in p["attributes"]:
                raise AssertionError(f"{semantic}: expected absent")
        for semantic, values in attrs.items():
            if semantic in omitted:
                continue
            converted_values = converted(semantic, values, normalise)
            if semantic == "JOINTS_0" and normalise and "WEIGHTS_0" in attrs:
                weights = converted("WEIGHTS_0", attrs["WEIGHTS_0"], True)
                converted_values = [[j if w > 0 else 0 for j, w in zip(js, ws)] if any(math.isfinite(f32(v)) and f32(v) > 0 for v in original) else [0,0,0,0] for js, ws, original in zip(converted_values, weights, attrs["WEIGHTS_0"])]
            equal(accessor(doc, binary, p["attributes"][semantic]), converted_values, semantic)
        if "indices" in case:
            values = case["indices"]
            equal(accessor(doc, binary, p["indices"]), [[i] for t in values for i in reversed(t)], "indices")
        if "targets" in case and len(p.get("targets", [])) != len(case["targets"]):
            raise AssertionError("morph target count")
        for actual, target in zip(p.get("targets", []), case.get("targets", [])):
            for semantic in sorted(omitted):
                if semantic in actual:
                    raise AssertionError(f"morph {semantic}: expected absent")
            for semantic, values in target.items():
                if semantic in omitted:
                    continue
                equal(accessor(doc, binary, actual[semantic]), converted(semantic, values, normalise, morph=True), "morph " + semantic)
    return len(expected)


def sub(a, b):
    return [x-y for x, y in zip(a, b)]


def cross(a, b):
    return [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]]


def orientation(doc, binary):
    results = []
    for mi, mesh in enumerate(doc.get("meshes", [])):
        for pi, p in enumerate(mesh["primitives"]):
            attrs = p["attributes"]
            eligible = agree = 0
            if "NORMAL" in attrs:
                positions = accessor(doc, binary, attrs["POSITION"])
                normals = accessor(doc, binary, attrs["NORMAL"])
                diagonal2 = sum((max(v[c] for v in positions)-min(v[c] for v in positions))**2 for c in range(3))
                indices = [v[0] for v in accessor(doc, binary, p["indices"])] if "indices" in p else list(range(len(positions)))
                for i in range(0, len(indices), 3):
                    a, b, c = indices[i:i+3]
                    winding = cross(sub(positions[b], positions[a]), sub(positions[c], positions[a]))
                    area = math.sqrt(sum(x*x for x in winding)) / 2
                    if area <= 1e-12 * diagonal2:
                        continue
                    eligible += 1
                    mean = [sum(normals[j][k] for j in (a,b,c))/3 for k in range(3)]
                    agree += sum(x*y for x,y in zip(winding, mean)) > 0
            status = "N/A" if eligible == 0 else "pass" if agree/eligible >= 0.99 else "fail"
            results.append(dict(mesh=mi, primitive=pi, eligible=eligible, agree=agree, status=status))
            if status == "fail":
                raise AssertionError(f"orientation mesh {mi} primitive {pi}: {agree}/{eligible}")
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("glb", type=Path)
    parser.add_argument("--expected", type=Path)
    parser.add_argument("--raw", action="store_true")
    args = parser.parse_args()
    doc, binary = read_glb(args.glb)
    compared = check_accessors(doc, binary, json.loads(args.expected.read_text(encoding="utf-8")), not args.raw) if args.expected else 0
    print(json.dumps(dict(compared=compared, orientation=orientation(doc,binary)), ensure_ascii=False))


if __name__ == "__main__":
    main()
