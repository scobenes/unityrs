"""Aggregate calibration validation, selection, privacy and read-only checks."""
import copy
import importlib.util
import io
import json
from contextlib import redirect_stderr
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/parity'))
import scratch

SPEC = importlib.util.spec_from_file_location('regenerate', ROOT / 'tools/memory/regenerate.py')
gen = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gen)


def record(target='x86_64-unknown-linux-gnu', profile='debug', sink='directory', kind='synthetic'):
    return {
        'target': target, 'profile': profile, 'sink': sink, 'kind': kind,
        'provenance': {
            'revision': '1' * 40, 'lockfile_sha256': '2' * 64,
            'rustc': [1, 90, 0], 'glibc': [2, 39], 'kernel': [6, 8, 0],
            'native_libraries_sha256': '3' * 64, 'features': 'test-hooks',
            'native_flags_sha256': '5' * 64,
            'native_compilers': {'c': [13, 3, 0], 'cxx': [13, 3, 0]},
            'allocator': 'system-glibc', 'native': True, 'page_size': 4096,
            'clean': True,
            'mmap_threshold': 131072, 'arena_extra': 2,
        },
        'coverage': {scene: kind == 'synthetic' for scene in gen.SCENES},
        **({'whole_run_coverage': {
            name: (name != 'store-roundtrip' or sink == 'store')
            for name in (*gen.WHOLE_SCENES, *gen.WHOLE_STORE_SCENES)
        }} if kind == 'whole-run' else {}),
        'coefficients': {name: 4096 for name in gen.COEFFICIENTS},
        'structural_floors': {'m_dispatch': 128, 'm_end': 4096},
        'fixed_envelopes': {name: limit for name, limit in gen.ENVELOPES.items()},
        'attribution': {
            'base_workers': 1, 'base_download_concurrency': 1,
            'held_charges_only': True, 'live_deductions_only': True,
            'raw_incremental_coefficients': True, 'native_only_charges_excluded': True,
            'observer_allocation_free': True, 'coherent_observations': True,
            'peer_excluded': True, 'parent_startup_included': True,
        },
    }


def document(final=False):
    records = [record()]
    if final:
        records = [record(t, p, s) for t in gen.TARGETS
                   for p in gen.PROFILES for s in gen.SINKS]
        records.extend(record(gen.TARGETS[1], p, s, 'whole-run')
                       for p in gen.PROFILES for s in gen.SINKS)
    return {'schema_version': 1, 'status': 'final' if final else 'provisional-b12b',
            'records': records}


class SelectionTests(unittest.TestCase):
    def test_hand_calculated_margin_and_rounding(self):
        cases = [(0, 65536, 4096, (65536, 65536)),
                 (1, 65536, 4096, (65536, 69632)),
                 (262145, 65536, 4096, (65537, 331776)),
                 (0, 64, 64, (64, 64)), (257, 64, 64, (65, 384))]
        for raw, minimum, quantum, expected in cases:
            with self.subTest(raw=raw, minimum=minimum):
                self.assertEqual(gen.select(raw, minimum, quantum, 0), expected)

    def test_floor_rounds_up_without_hiding_raw_or_margin(self):
        self.assertEqual(gen.select(1, 64, 64, 1025), (64, 1088))

    def test_overflow_is_refused(self):
        for raw in (gen.U64_MAX, gen.U64_MAX - 64):
            with self.assertRaises(gen.Invalid):
                gen.select(raw, 64, 64, 0)

    def test_maxima_and_structural_floors_are_across_records(self):
        data = document(True)
        data['records'][0]['coefficients']['c_base'] = 262145
        data['records'][1]['structural_floors']['m_end'] = 20000
        result = gen.generate(data)
        self.assertEqual(result['coefficients']['c_base'],
                         {'raw': 262145, 'margin': 65537, 'structural_floor': 0,
                          'selected': 331776})
        self.assertEqual(result['coefficients']['m_end']['selected'], 20032)

    def test_canonical_bytes_do_not_depend_on_record_order(self):
        data = document(True)
        first = gen.render(gen.generate(data))
        data['records'].reverse()
        self.assertEqual(first, gen.render(gen.generate(data)))


class ValidationTests(unittest.TestCase):
    def rejected(self, data):
        with self.assertRaises(gen.Invalid):
            gen.generate(data)

    def test_final_matrix_is_accepted(self):
        result = gen.generate(document(True))
        self.assertEqual(result['pending'], [])
        self.assertTrue(result['shipping_replacement'])

    def test_partial_retains_pending_and_cannot_generate_rust(self):
        result = gen.generate(document())
        self.assertFalse(result['shipping_replacement'])
        self.assertIn('store-calibration', result['pending'])
        self.assertIn('native-matrix', result['pending'])
        with self.assertRaises(gen.Invalid):
            gen.rust_source(result)

    def test_unknown_fields_are_rejected_at_every_object(self):
        for place in ('root', 'record', 'provenance', 'coverage', 'coefficients',
                      'structural_floors', 'fixed_envelopes', 'attribution'):
            data = document()
            node = data if place == 'root' else data['records'][0]
            if place not in ('root', 'record'):
                node = node[place]
            node['private_label'] = 'unacceptable'
            with self.subTest(place=place):
                self.rejected(data)

    def test_missing_fields_are_rejected_at_every_object(self):
        for place in ('root', 'record', 'provenance', 'coverage', 'coefficients',
                      'structural_floors', 'fixed_envelopes', 'attribution'):
            data = document()
            node = data if place == 'root' else data['records'][0]
            if place not in ('root', 'record'):
                node = node[place]
            del node[next(iter(node))]
            with self.subTest(place=place):
                self.rejected(data)

    def test_numbers_are_unsigned_integers_not_boolean_float_or_text(self):
        for value in (True, -1, 1.0, '123', gen.U64_MAX + 1):
            data = document()
            data['records'][0]['coefficients']['c_env'] = value
            with self.subTest(value=value):
                self.rejected(data)

    def test_no_arbitrary_labels_in_provenance_or_axes(self):
        for field in ('target', 'profile', 'sink'):
            data = document()
            data['records'][0][field] = 'unacceptable'
            self.rejected(data)
        for field in ('revision', 'lockfile_sha256', 'native_libraries_sha256',
                      'rustc', 'glibc', 'kernel', 'features', 'allocator'):
            data = document()
            data['records'][0]['provenance'][field] = 'unacceptable'
            self.rejected(data)

    def test_emulation_and_wrong_allocator_configuration_are_refused(self):
        for field, value in (('native', False), ('native', 1), ('page_size', 65536),
                             ('mmap_threshold', 0), ('arena_extra', 1),
                             ('allocator', 'other'), ('features', 'default')):
            data = document()
            data['records'][0]['provenance'][field] = value
            with self.subTest(field=field, value=value):
                self.rejected(data)

    def test_both_schemas_require_every_provenance_field_and_native_compiler(self):
        paths = [(field,) for field in record()['provenance']]
        paths.extend(('native_compilers', compiler) for compiler in ('c', 'cxx'))
        for kind in ('input', 'output'):
            schema = gen._schema('aggregate-' + kind + '-v1.schema.json')
            for path in paths:
                data = document()
                if kind == 'output':
                    data = gen.generate(data)
                node = data['records'][-1]['provenance']
                for field in path[:-1]:
                    node = node[field]
                del node[path[-1]]
                with self.subTest(schema=kind, path=path):
                    with self.assertRaises(gen.Invalid):
                        gen._validate(data, schema, schema)

    def test_native_compilers_require_three_unsigned_version_integers(self):
        for compiler in ('c', 'cxx'):
            for value in ([], [13, 3], [13, 3, 0, 1], '13.3.0',
                          [True, 3, 0], [-1, 3, 0], [13.0, 3, 0],
                          [gen.U64_MAX + 1, 3, 0]):
                data = document()
                data['records'][0]['provenance']['native_compilers'][compiler] = value
                with self.subTest(compiler=compiler, value=value):
                    self.rejected(data)

    def test_unrecognized_native_toolchain_fields_are_refused(self):
        data = document()
        data['records'][0]['provenance']['native_compilers']['other'] = [1, 0, 0]
        self.rejected(data)

    def test_mismatched_build_provenance_is_refused(self):
        for field, value in (('revision', '4' * 40), ('lockfile_sha256', '4' * 64),
                             ('rustc', [1, 91, 0]),
                             ('native_libraries_sha256', '4' * 64)):
            data = document(True)
            data['records'][1]['provenance'][field] = value
            self.rejected(data)

    def test_duplicate_axis_and_empty_records_are_refused(self):
        data = document()
        data['records'].append(copy.deepcopy(data['records'][0]))
        self.rejected(data)
        data['records'] = []
        self.rejected(data)

    def test_each_missing_final_axis_is_refused(self):
        data = document(True)
        for index in range(8):
            changed = copy.deepcopy(data)
            del changed['records'][index]
            self.rejected(changed)

    def test_each_missing_final_scene_is_refused(self):
        for scene in gen.SCENES:
            data = document(True)
            data['records'][1 if scene.startswith('store-') else 0]['coverage'][scene] = False
            with self.subTest(scene=scene):
                self.rejected(data)

        # Whole-run flags are independent; synthetic false flags are honest.
        gen.generate(document(True))
        for scene in (*gen.WHOLE_SCENES, *gen.WHOLE_STORE_SCENES):
            data = document(True)
            whole = next(r for r in data['records']
                         if r['kind'] == 'whole-run' and r['sink'] == 'store')
            whole['whole_run_coverage'][scene] = False
            with self.subTest(whole_scene=scene):
                self.rejected(data)
        data = document(True)
        whole = next(r for r in data['records']
                     if r['kind'] == 'whole-run' and r['sink'] == 'directory')
        whole['whole_run_coverage']['store-roundtrip'] = True
        self.rejected(data)
        data = document()
        data['records'][0]['whole_run_coverage'] = record(kind='whole-run')['whole_run_coverage']
        self.rejected(data)
        for group, keys in (
            ('coefficients', [k for k in gen.COEFFICIENTS if k != 'c_base']),
            ('structural_floors', ['m_dispatch', 'm_end']),
            ('fixed_envelopes', list(gen.ENVELOPES)),
        ):
            for key in keys:
                data = document(True)
                data['records'][8][group][key] -= 1
                with self.subTest(companion_group=group, key=key):
                    self.rejected(data)

    def test_pending_stop_and_store_labels_cannot_disappear_in_partial(self):
        data = document()
        for scene in gen.SCENES:
            if scene.startswith('stop-') or scene.startswith('store-'):
                data['records'][0]['coverage'][scene] = False
        pending = gen.generate(data)['pending']
        self.assertIn('store-calibration', pending)
        self.assertIn('stop-calibration', pending)
        self.assertIn('final-acceptance', pending)

    def test_fixed_envelope_breaches_are_refused_including_partial(self):
        for name, limit in gen.ENVELOPES.items():
            data = document()
            data['records'][0]['fixed_envelopes'][name] = limit + 1
            with self.subTest(name=name):
                self.rejected(data)

    def test_end_floor_must_include_inline_detail_and_slot_overhead(self):
        data = document()
        data['records'][0]['structural_floors']['m_end'] = 3072
        self.rejected(data)

    def test_duplicate_json_keys_and_nonfinite_numbers_are_refused(self):
        for raw in ('{"schema_version":1,"schema_version":1}', '{"x":NaN}',
                    '{"x":Infinity}', '{"x":-Infinity}'):
            with self.assertRaises(gen.Invalid):
                gen.parse(raw)

    def test_validation_error_does_not_echo_rejected_text(self):
        data = document()
        data['records'][0]['target'] = 'sensitive sentinel'
        with self.assertRaises(gen.Invalid) as caught:
            gen.generate(data)
        self.assertNotIn('sensitive sentinel', str(caught.exception))

    def test_output_is_validated_before_serialization(self):
        result = gen.generate(document())
        result['private_label'] = 'unacceptable'
        with self.assertRaises(gen.Invalid):
            gen.render(result)

    def test_output_derived_fields_cannot_be_forged(self):
        for field in ('raw', 'margin', 'selected', 'structural_floor'):
            result = gen.generate(document())
            result['coefficients']['c_base'][field] += 1
            with self.assertRaises(gen.Invalid):
                gen.render(result)
        for name, value in (('shipping_replacement', True), ('pending', [])):
            result = gen.generate(document())
            result[name] = value
            with self.assertRaises(gen.Invalid):
                gen.render(result)

    def test_unsafe_attribution_is_refused(self):
        for field, value in record()['attribution'].items():
            data = document()
            data['records'][0]['attribution'][field] = False if type(value) is bool else 2
            with self.subTest(field=field):
                self.rejected(data)

    def test_public_schemas_match_runtime_vocabulary_and_each_other(self):
        schemas = [gen._schema('aggregate-' + kind + '-v1.schema.json')
                   for kind in ('input', 'output')]
        self.assertEqual(schemas[0]['$defs'], schemas[1]['$defs'])
        properties = schemas[0]['$defs']['record']['properties']
        self.assertEqual(set(properties['coverage']['properties']), set(gen.SCENES))
        self.assertEqual(set(properties['fixed_envelopes']['properties']), set(gen.ENVELOPES))
        self.assertEqual(set(properties['coefficients']['properties']), set(gen.COEFFICIENTS))
        for name, values in (('target', gen.TARGETS), ('profile', gen.PROFILES),
                             ('sink', gen.SINKS)):
            self.assertEqual(properties[name]['enum'], list(values))

    def test_digest_patterns_reject_embedded_or_surrounding_text(self):
        for text in ('x' + '1' * 40, '1' * 40 + 'x', '/' + '1' * 40 + '/'):
            data = document()
            data['records'][0]['provenance']['revision'] = text
            self.rejected(data)

    def test_provisional_mode_keeps_final_store_and_stop_pending(self):
        data = document(True)
        data['status'] = 'provisional-b12b'
        result = gen.generate(data)
        self.assertEqual(result['pending'],
                         ['final-acceptance', 'stop-calibration', 'store-calibration'])
        self.assertFalse(result['shipping_replacement'])


class CommandTests(unittest.TestCase):
    def setUp(self):
        self.folder = scratch.for_case(self, 'unityrs-memory-')
        self.source = self.folder / 'input.json'
        self.output = self.folder / 'output.json'
        self.source.write_text(json.dumps(document()), encoding='utf-8')

    def run_main(self, *extra):
        with redirect_stderr(io.StringIO()):
            return gen.main([str(self.source), '--output', str(self.output), *extra])

    def test_write_then_check_and_stale_check_preserves_file(self):
        self.assertEqual(self.run_main(), 0)
        before = self.output.stat().st_mtime_ns
        self.assertEqual(self.run_main('--check'), 0)
        self.assertEqual(self.output.stat().st_mtime_ns, before)
        self.output.write_text('stale', encoding='utf-8')
        self.assertEqual(self.run_main('--check'), 1)
        self.assertEqual(self.output.read_text(), 'stale')

    def test_missing_check_does_not_create_output_or_directories(self):
        self.output = self.folder / 'missing' / 'output.json'
        self.assertEqual(self.run_main('--check'), 1)
        self.assertFalse(self.output.parent.exists())

    def test_provisional_rust_request_does_not_write_any_output(self):
        rust = self.folder / 'measured.rs'
        self.assertEqual(self.run_main('--rust-output', str(rust)), 2)
        self.assertFalse(rust.exists())
        self.assertFalse(self.output.exists())

    def test_final_rust_and_json_are_deterministic_and_checked(self):
        self.source.write_text(json.dumps(document(True)), encoding='utf-8')
        rust = self.folder / 'measured.rs'
        self.assertEqual(self.run_main('--rust-output', str(rust)), 0)
        self.assertIn('pub const MEASURED: Constants', rust.read_text())
        self.assertIn('size_of::<crate::job::End>()', rust.read_text())
        self.assertIn('size_of::<crate::worker::Dispatch>()', rust.read_text())
        self.assertEqual(self.run_main('--rust-output', str(rust), '--check'), 0)
        rust.write_text('stale', encoding='utf-8')
        self.assertEqual(self.run_main('--rust-output', str(rust), '--check'), 1)
        self.assertEqual(rust.read_text(), 'stale')

    def test_input_output_aliases_are_refused(self):
        self.output = self.source
        before = self.source.read_bytes()
        self.assertEqual(self.run_main(), 2)
        self.assertEqual(self.source.read_bytes(), before)

    def test_invalid_input_does_not_replace_existing_output(self):
        self.output.write_bytes(b'keep')
        self.source.write_text('{"unexpected":"sensitive sentinel"}', encoding='utf-8')
        self.assertEqual(self.run_main(), 2)
        self.assertEqual(self.output.read_bytes(), b'keep')


if __name__ == '__main__':
    unittest.main()
