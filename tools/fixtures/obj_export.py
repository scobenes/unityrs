"""Record OBJ/MTL and corrected renderer fixtures from testkit-built inputs.

Requires the pinned UnityPy 1.25.2 / Pillow 9.5.0 oracle. No TPK is used:
export_inputs emits synthetic files with embedded, hand-built trees.
The corrected oracle compiles the original exporter with only PARITY 19.1's
color, texture dictionary, optional batch and material-offset fixes.
"""
import argparse
import inspect
import json
import sys
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/parity'))


def corrected(module):
    source = inspect.getsource(module)
    fixes = [
        ('(color.R, color.G, color.B, color.A)', '(color.r, color.g, color.b, color.a)'),
        ('for key, texEnv in texEnvs:', 'for key, texEnv in texEnvs.items():'),
        ('hasattr(renderer, "m_StaticBatchInfo") and renderer.m_StaticBatchInfo.subMeshCount > 0',
         'getattr(renderer, "m_StaticBatchInfo", None) is not None and renderer.m_StaticBatchInfo.subMeshCount > 0'),
        ('elif hasattr(renderer, "m_SubsetIndices"):', 'elif getattr(renderer, "m_SubsetIndices", None):'),
        ('        if mat_index < 0 or mat_index >= len(renderer.m_Materials):\n            continue',
         '        if mat_index < 0 or mat_index >= len(renderer.m_Materials):\n            material_names.append(None)\n            continue'),
    ]
    for old, new in fixes:
        if source.count(old) != 1:
            raise RuntimeError('corrected-oracle replacement no longer unique')
        source = source.replace(old, new)
    patched = types.ModuleType('UnityPy.export._corrected_renderer')
    patched.__package__ = 'UnityPy.export'
    exec(compile(source, '<corrected renderer>', 'exec'), patched.__dict__)
    return patched


def outcome(call):
    try:
        value = call()
        return {'value': value.encode('utf8', 'surrogateescape').hex() if isinstance(value, str) else value}
    except (AttributeError, TypeError, ValueError, IndexError, KeyError, NotImplementedError) as error:
        return {'error': type(error).__name__}


def renderer(case, module):
    import UnityPy
    from UnityPy.classes.generated import (
        GameObject, Material, Mesh, MeshFilter, MeshRenderer, SkinnedMeshRenderer, Texture2D,
    )
    import scratch
    env = UnityPy.Environment(bytes.fromhex(case['input']))
    expected = {1: GameObject, 21: Material, 23: MeshRenderer, 28: Texture2D,
                33: MeshFilter, 43: Mesh, 137: SkinnedMeshRenderer}
    objects = {}
    for reader in env.objects:
        obj = reader.parse_as_object()
        cls = expected[reader.type.value]
        assert type(obj) is cls, f"{case['case']} path {reader.path_id}: expected {cls.__name__}, got {type(obj).__name__}"
        objects[reader.path_id] = obj
    with scratch.folder('obj-export-') as tmpdir:
        result = outcome(lambda: module.export_mesh_renderer(objects[case['target']], str(tmpdir)))
        result['files'] = {p.relative_to(tmpdir).as_posix(): p.read_bytes().hex()
                           for p in sorted(tmpdir.rglob('*')) if p.is_file()}
    result['materials'] = [outcome(lambda o=o: module.export_material(objects[o.path_id]))
                           for o in env.objects if o.type.value == 21]
    return result


def record(inputs):
    import UnityPy
    from UnityPy.export import MeshExporter, MeshRendererExporter
    patch = corrected(MeshRendererExporter)
    obj_cases, renderer_cases = [], []
    for case in inputs:
        if case['kind'] == 'obj':
            env = UnityPy.Environment(bytes.fromhex(case['input']))
            mesh = next(o for o in env.objects if o.path_id == case['target']).parse_as_object()
            names = case['materials']
            names = None if names is None else [None if n is None else bytes.fromhex(n).decode('utf8', 'surrogateescape') for n in names]
            obj_cases.append({**case, 'oracle': outcome(lambda: MeshExporter.export_mesh_obj(mesh, names))})
        else:
            original, fixed = renderer(case, MeshRendererExporter), renderer(case, patch)
            renderer_cases.append({**case, 'oracle': original, 'corrected': fixed,
                                   'diverges': original != fixed})
    return {'obj.json': {'reference': 'UnityPy 1.25.2', 'cases': obj_cases},
            'renderer.json': {'reference': 'UnityPy 1.25.2, PARITY 19.1 corrected oracle', 'cases': renderer_cases}}


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument('--check', action='store_true')
    args = p.parse_args()
    try:
        import UnityPy
        from PIL import __version__ as pillow
    except ImportError:
        p.exit(2, 'requires UnityPy 1.25.2 and Pillow 9.5.0 in the oracle environment\n')
    if UnityPy.__version__ != '1.25.2' or pillow != '9.5.0':
        p.exit(2, 'requires UnityPy 1.25.2 and Pillow 9.5.0\n')
    import child
    proc = child.run(['cargo', 'run', '--quiet', '--locked', '-p', 'unityrs-mesh', '--example', 'export_inputs'],
                     cwd=ROOT, check=True, capture_output=True, text=True)
    out = ROOT / 'crates/unityrs-mesh/tests/fixtures'
    for name, data in record(json.loads(proc.stdout)).items():
        body = json.dumps(data, indent=2, ensure_ascii=False) + '\n'
        path = out / name
        if args.check:
            if not path.exists() or path.read_text() != body:
                p.exit(1, f'stale fixture: {name}\n')
        else:
            out.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
    return 0


if __name__ == '__main__':
    sys.exit(main())
