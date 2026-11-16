#!/usr/bin/env python3
"""Validate aggregate calibration evidence and select constants deterministically.

Explicit review tool only: never run during a build or silently repair checks.
No measurements, raw reports, external packages or runtime imports are needed.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys

U64_MAX = (1 << 64) - 1
TARGETS = ('x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu')
PROFILES = ('debug', 'release')
SINKS = ('directory', 'store')
COEFFICIENTS = ('c_base', 'c_env', 'm_thread', 'm_pool', 'm_dispatch', 'm_end')
SCENES = (
    'worker-count', 'pool-controller', 'pool-finisher', 'environment',
    'full-dispatch-channel', 'full-end-channel', 'base-startup',
    'base-failed-input', 'base-archive', 'base-old-journal', 'base-resume',
    'base-lookup', 'normal-run', 'loose-publication', 'zip-publication',
    'http-tls', 'view-paths', 'header-ownership', 'scale-residual',
    'native-components', 'retention', 'store-multipart', 'store-list',
    'store-verification', 'store-journal', 'store-final-head',
    'stop-phase-1', 'stop-phase-2', 'stop-phase-3', 'stop-phase-4',
    'stop-phase-5', 'stop-phase-6', 'stop-phase-7',
)
ENVELOPES = {
    'connection': 4 << 20, 'finisher': 24 << 20, 'view_member': 1024,
    'header_table_entry': 48, 'mask_entry': 32, 'header_copies': 4,
    'credential_holders': 6, 'credential_id': 128, 'credential_secret': 128,
    'credential_token': 16 << 10, 'multipart_list': 2 << 20,
    'response_body': 64 << 10, 'worker_state': 115344, 'panic_slot': 2048,
    'abort_pair': 2080, 'journal_line': 16 << 10, 'archive_resume_entry': 160,
}
PENDING = ('native-matrix', 'store-calibration', 'stop-calibration',
           'scene-coverage', 'whole-run-matrix', 'final-acceptance')
SCHEMA_DIR = Path(__file__).resolve().parents[2] / 'docs/memory'


class Invalid(ValueError):
    """An invalid aggregate; diagnostics never echo input values or labels."""


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Invalid('duplicate JSON member')
        result[key] = value
    return result


def parse(raw):
    """Reject duplicate members and JSON's nonstandard nonfinite extension."""
    def nonfinite(_):
        raise Invalid('nonfinite JSON number')
    try:
        return json.loads(raw, object_pairs_hook=_object, parse_constant=nonfinite)
    except (ValueError, RecursionError) as error:
        raise Invalid('invalid JSON aggregate') from error


def _schema(name):
    return json.loads((SCHEMA_DIR / name).read_text(encoding='utf-8'))


def _validate(value, schema, root):
    """Small validator for the explicit subset used by the public schemas."""
    if '$ref' in schema:
        node = root
        for part in schema['$ref'].removeprefix('#/').split('/'):
            node = node[part]
        return _validate(value, node, root)
    kind = schema.get('type')
    types = {'object': dict, 'array': list, 'string': str, 'integer': int,
             'boolean': bool}
    if kind and type(value) is not types[kind]:
        raise Invalid('aggregate member has wrong type')
    if 'const' in schema and (type(value) is not type(schema['const'])
                              or value != schema['const']):
        raise Invalid('aggregate member has unsupported value')
    if 'enum' in schema and value not in schema['enum']:
        raise Invalid('aggregate member has unsupported value')
    if kind == 'integer':
        if not schema.get('minimum', 0) <= value <= schema.get('maximum', U64_MAX):
            raise Invalid('aggregate integer outside allowed range')
    elif kind == 'string':
        if 'pattern' in schema and re.fullmatch(schema['pattern'], value) is None:
            raise Invalid('aggregate string has unsupported form')
    elif kind == 'object':
        properties = schema['properties']
        if set(value) - set(properties) or set(schema['required']) - set(value):
            raise Invalid('unknown or missing aggregate member')
        for key, child in value.items():
            _validate(child, properties[key], root)
    elif kind == 'array':
        if not schema.get('minItems', 0) <= len(value) <= schema.get('maxItems', U64_MAX):
            raise Invalid('aggregate array has invalid length')
        if schema.get('uniqueItems') and len({json.dumps(x, sort_keys=True) for x in value}) != len(value):
            raise Invalid('aggregate array has duplicate entries')
        for child in value:
            _validate(child, schema['items'], root)


def validate_input(data):
    schema = _schema('aggregate-input-v1.schema.json')
    _validate(data, schema, schema)
    seen = set()
    provenance = None
    for record in data['records']:
        axis = tuple(record[name] for name in ('kind', 'target', 'profile', 'sink'))
        if axis in seen:
            raise Invalid('duplicate calibration axis')
        seen.add(axis)
        p = record['provenance']
        if not p['native'] or not p['clean']:
            raise Invalid('emulated or modified-source evidence is ineligible')
        # Kernel and glibc can differ between native architecture hosts. The
        # measured code, dependencies, toolchain, features and native libraries
        # must describe one build configuration across the complete matrix.
        build = tuple(json.dumps(p[name], sort_keys=True) for name in (
            'revision', 'lockfile_sha256', 'rustc', 'features',
            'native_libraries_sha256',
            'native_flags_sha256',
        ))
        if provenance is not None and build != provenance:
            raise Invalid('mismatched build provenance')
        provenance = build
        if record['structural_floors']['m_end'] < 3072 + 256:
            raise Invalid('end representation floor omits bounded detail or slot overhead')
        for name, ceiling in ENVELOPES.items():
            if record['fixed_envelopes'][name] > ceiling:
                raise Invalid('fixed envelope breached; bound revision required')
    if data['status'] == 'final':
        required = {('synthetic', t, p, s) for t in TARGETS for p in PROFILES for s in SINKS}
        if not required <= seen:
            raise Invalid('incomplete native architecture/profile/sink matrix')
        if not whole_complete(seen):
            raise Invalid('complete native whole-run profile/sink coverage required')
        for record in data['records']:
            if any(not record['coverage'][scene] for scene in SCENES
                   if record['sink'] == 'store' or not scene.startswith('store-')):
                raise Invalid('incomplete final scene/store/stop coverage')


def whole_complete(axes):
    return any({('whole-run', target, profile, sink)
                for profile in PROFILES for sink in SINKS} <= axes
               for target in TARGETS)


def select(raw, minimum, quantum, floor):
    """Return (margin, selected), checking every u64 arithmetic boundary."""
    if any(type(n) is not int or not 0 <= n <= U64_MAX
           for n in (raw, minimum, quantum, floor)) or quantum == 0:
        raise Invalid('invalid selection operand')
    margin = max((raw // 4) + bool(raw % 4), minimum)
    padded = raw + margin
    if padded > U64_MAX:
        raise Invalid('coefficient margin overflow')
    base = max(padded, floor)
    selected = ((base + quantum - 1) // quantum) * quantum
    if selected > U64_MAX:
        raise Invalid('coefficient rounding overflow')
    return margin, selected


def _pending(data):
    if data['status'] == 'final':
        return []
    records = data['records']
    axes = {(r['target'], r['profile'], r['sink']) for r in records
            if r['kind'] == 'synthetic'}
    result = {'store-calibration', 'stop-calibration', 'final-acceptance'}
    if len(axes) != len(TARGETS) * len(PROFILES) * len(SINKS):
        result.add('native-matrix')
    if not whole_complete({tuple(r[name] for name in ('kind', 'target', 'profile', 'sink'))
                           for r in records}):
        result.add('whole-run-matrix')
    stores = [r for r in records if r['sink'] == 'store']
    if not stores or any(not r['coverage'][s] for r in stores for s in SCENES
                         if s.startswith('store-')):
        result.add('store-calibration')
    if any(not r['coverage'][s] for r in records for s in SCENES
           if s.startswith('stop-')):
        result.add('stop-calibration')
    if any(not r['coverage'][s] for r in records for s in SCENES
           if not s.startswith(('stop-', 'store-'))):
        result.add('scene-coverage')
    return sorted(result)


def generate(data):
    validate_input(data)
    result = {'schema_version': 1, 'status': data['status'],
              'shipping_replacement': data['status'] == 'final',
              'pending': _pending(data), 'coefficients': {},
              'records': sorted(data['records'], key=lambda r: (
                  r['kind'], r['target'], r['profile'], r['sink']))}
    for name in COEFFICIENTS:
        raw = max(r['coefficients'][name] for r in data['records'])
        floor = max(r['structural_floors'].get(name, 0) for r in data['records'])
        minimum, quantum = (64, 64) if name.startswith('m_') and name in (
            'm_dispatch', 'm_end') else (65536, 4096)
        margin, selected = select(raw, minimum, quantum, floor)
        result['coefficients'][name] = {'raw': raw, 'margin': margin,
                                        'structural_floor': floor, 'selected': selected}
    validate_output(result)
    return result


def validate_output(result):
    schema = _schema('aggregate-output-v1.schema.json')
    _validate(result, schema, schema)
    validate_input({key: result[key] for key in ('schema_version', 'status', 'records')})
    if result['shipping_replacement'] != (result['status'] == 'final'):
        raise Invalid('output shipping status mismatch')
    if result['pending'] != _pending(result):
        raise Invalid('output pending coverage mismatch')
    for name in COEFFICIENTS:
        raw = max(r['coefficients'][name] for r in result['records'])
        floor = max(r['structural_floors'].get(name, 0) for r in result['records'])
        minimum, quantum = (64, 64) if name in ('m_dispatch', 'm_end') else (65536, 4096)
        margin, selected = select(raw, minimum, quantum, floor)
        if result['coefficients'][name] != {
            'raw': raw, 'margin': margin, 'structural_floor': floor, 'selected': selected}:
            raise Invalid('output coefficient selection mismatch')


def render(result):
    validate_output(result)
    return (json.dumps(result, sort_keys=True, indent=2) + '\n').encode('utf-8')


def rust_source(result):
    validate_output(result)
    if not result['shipping_replacement']:
        raise Invalid('provisional evidence cannot generate shipping constants')
    lines = ['// Generated by tools/memory/regenerate.py; aggregate schema v1.',
             '// Review with the corresponding aggregate evidence.',
             'use super::Constants;', '', 'pub const MEASURED: Constants = Constants {']
    for name in ('c_base', 'm_thread', 'm_pool', 'm_dispatch', 'm_end', 'c_env'):
        lines.append(f"    {name}: {result['coefficients'][name]['selected']},")
    lines.extend([
        f"    c_net: {ENVELOPES['connection']},", '};', '',
        'pub const SELECTED: Constants = MEASURED;',
        'pub const CALIBRATED: bool = true;', '',
        'const _: () =',
        '    assert!(MEASURED.m_end >= (size_of::<crate::job::End>() + super::END_SLOT_OVERHEAD) as u64);',
        'const _: () = assert!(MEASURED.m_dispatch >= size_of::<crate::worker::Dispatch>() as u64);',
        '',
    ])
    return '\n'.join(lines).encode('utf-8')


def bootstrap_source():
    """Compiled fallback, explicitly containing no measurement claims."""
    return ('// Generated by tools/memory/regenerate.py --bootstrap; no calibration evidence.\n'
            'use super::Constants;\n\n'
            'pub const SELECTED: Constants = super::PROVISIONAL;\n'
            'pub const CALIBRATED: bool = false;\n').encode('utf-8')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', nargs='*', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--rust-output', type=Path)
    parser.add_argument('--final', action='store_true', help='require complete eligible evidence')
    parser.add_argument('--bootstrap', action='store_true', help='emit the provisional compiled fallback')
    parser.add_argument('--compiled-evidence', type=Path,
                        help='verify compiled Rust against a selected aggregate document')
    parser.add_argument('--check', action='store_true', help='compare only; never write')
    args = parser.parse_args(argv)
    try:
        if args.compiled_evidence is not None:
            if args.input or args.output or args.final or args.bootstrap or not args.check or args.rust_output is None:
                raise Invalid('compiled evidence requires only --check and a Rust output')
            result = parse(args.compiled_evidence.read_text(encoding='utf-8'))
            outputs = [(args.rust_output, rust_source(result))]
        elif args.bootstrap:
            if args.input or args.output or args.final or args.rust_output is None:
                raise Invalid('bootstrap requires only a Rust output')
            outputs = [(args.rust_output, bootstrap_source())]
        elif not args.input or args.output is None:
            raise Invalid('aggregate inputs and output required')
        paths = [p.resolve() for p in args.input]
        if args.compiled_evidence is not None:
            paths.append(args.compiled_evidence.resolve())
        if args.output is not None:
            paths.append(args.output.resolve())
        if args.rust_output is not None:
            paths.append(args.rust_output.resolve())
        if len(paths) != len(set(paths)):
            raise Invalid('input and output paths must be distinct')
        existing = [p for p in paths if p.exists()]
        if any(a.samefile(b) for i, a in enumerate(existing) for b in existing[i + 1:]):
            raise Invalid('input and output paths must be distinct')
        if not args.bootstrap and args.compiled_evidence is None:
            documents = [parse(path.read_text(encoding='utf-8')) for path in args.input]
            for doc in documents:
                validate_input(doc)
            data = {'schema_version': 1,
                    'status': 'final' if args.final or any(d['status'] == 'final' for d in documents)
                    else 'provisional-b12b',
                    'records': [r for d in documents for r in d['records']]}
            result = generate(data)
            outputs = [(args.output, render(result))]
            if args.rust_output is not None:
                outputs.append((args.rust_output, rust_source(result)))
        if args.check:
            stale = any(not path.is_file() or path.read_bytes() != content
                        for path, content in outputs)
            if stale:
                print('aggregate output is missing or stale', file=sys.stderr)
                return 1
        else:
            for path, content in outputs:
                path.write_bytes(content)
        return 0
    except (Invalid, OSError, UnicodeError, RecursionError) as error:
        # OS errors can contain input paths. Keep diagnostics aggregate-only too.
        message = str(error) if isinstance(error, Invalid) else 'aggregate file operation failed'
        print(message, file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
