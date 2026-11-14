"""L6: every Mesh in the pinned sample files, arrays/type tags/triangles/OBJ.

All sample-derived values stay in memory or child pipes. Public output is
aggregate counts only. Tier C decoding uses meshdecode.py's corrected source.
"""
import argparse
import json
import ntpath
import struct
import sys
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/parity'))
sys.path.insert(0, str(ROOT / 'tools/fixtures'))


def number(x):
    return {'f': struct.pack('>d', x).hex()} if isinstance(x, float) else x


def arrays(h):
    names = ['m_Vertices', 'm_Normals', 'm_Tangents', 'm_Colors', 'm_BoneWeights', 'm_BoneIndices']
    names += [f'm_UV{i}' for i in range(8)]
    attrs = {name: [[number(x) for x in row] for row in getattr(h, name)]
             for name in names if getattr(h, name) is not None}
    # MeshHandler sets m_BindPose only when it unpacks a compressed mesh's bind poses (below 5), and never
    # initialises it otherwise (helpers/MeshHelper.py:515-525).
    bind_pose = getattr(h, 'm_BindPose', None)
    if bind_pose is not None:
        attrs['m_BindPose'] = [[[number(x) for x in row] for row in matrix] for matrix in bind_pose]
    return {'vertex_count': h.m_VertexCount, 'use16': h.m_Use16BitIndices,
            'index_buffer': None if h.m_IndexBuffer is None else list(h.m_IndexBuffer), 'attrs': attrs}


def reference(path):
    import UnityPy
    from UnityPy.helpers.MeshHelper import MeshHandler
    from UnityPy.export.MeshExporter import export_mesh_obj
    env = UnityPy.load(str(path))
    records = []
    for obj in env.objects:
        if obj.type.value != 43:
            continue
        try:
            mesh = obj.parse_as_object()
            h = MeshHandler(mesh)
            h.process()
            values = arrays(h)
            subs = h.get_triangles() if h.m_VertexCount > 0 and h.m_Vertices else []
            triangles = {'ok': {'submeshes': [{'tri': [list(t) for t in sub if len(t) == 3],
                                                'cut': next((list(t) for t in sub if 0 < len(t) < 3), []),
                                                'empty': sum(not t for t in sub)} for sub in subs],
                                'index_counts': [s.indexCount for s in mesh.m_SubMeshes] if subs else []}}
            # get_triangles mutates strips, so OBJ must start with a fresh parse.
            text = export_mesh_obj(obj.parse_as_object())
            result = {'arrays': values, 'triangles': triangles,
                      'obj': text.encode('utf8', 'surrogateescape').hex() if text is not False else None}
        except (ValueError, AttributeError, TypeError, IndexError, AssertionError, NotImplementedError) as error:
            result = {'error': type(error).__name__}
        name = ntpath.basename(obj.assets_file.name).encode('utf8', 'surrogateescape').hex()
        records.append({'file': name, 'path_id': obj.path_id, 'result': result})
    return sorted(records, key=lambda r: (r['file'], r['path_id']))


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument('samples', type=Path)
    p.add_argument('--replay', type=Path, required=True)
    p.add_argument('--tpk', type=Path, required=True)
    args = p.parse_args()
    import UnityPy
    if UnityPy.__version__ != '1.25.2':
        p.exit(2, 'requires UnityPy 1.25.2\n')
    import child
    # meshdecode turns on its own --tpk guard (no writes, no spawn but its cargo replay) when it sees --tpk in
    # argv; this script spawns its own replay, so it imports meshdecode for CORRECTED alone, without the guard.
    with mock.patch.object(sys, 'argv', sys.argv[:1]):
        import meshdecode
    from UnityPy.helpers.MeshHelper import MeshHandler
    manifest = json.loads((ROOT / 'tools/parity/unitypy_samples.json').read_text())
    total = divergences = 0
    for item in manifest['files']:
        path = args.samples / item['name']
        original = reference(path)
        with mock.patch.multiple(MeshHandler, **meshdecode.CORRECTED):
            expected = reference(path)
        divergences += sum(a != b for a, b in zip(original, expected))
        proc = child.run([str(args.replay.resolve()), str(path.resolve()), str(args.tpk.resolve())],
                         capture_output=True, text=True)
        if proc.returncode:
            p.exit(1, 'sample L6 replay failed (sample-derived details suppressed)\n')
        got = json.loads(proc.stdout)
        if got != expected:
            p.exit(1, 'sample L6 mismatch (sample-derived details suppressed)\n')
        total += len(expected)
    if total == 0:
        p.exit(1, 'sample L6 checked no Mesh objects\n')
    print(f'L6: {total} meshes, {divergences} corrected-oracle divergences, zero mismatches')


if __name__ == '__main__':
    main()
