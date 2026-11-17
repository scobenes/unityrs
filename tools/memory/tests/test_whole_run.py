"""Whole-run facts and matching synthetic certificates stay independent."""
import copy
import unittest
from test_regenerate import document, gen, record


class WholeRunTests(unittest.TestCase):
    def test_one_worker_baseline_must_be_possible_on_input(self):
        for baseline in (1, (1 << 63) - 1):
            data = document(True)
            for item in data['records'][8:]:
                item['coefficients']['c_base'] = baseline
            with self.subTest(valid_baseline=baseline):
                self.assertTrue(gen.generate(data)['shipping_replacement'])
        data = document(True)
        data['status'] = 'provisional-b12b'
        for item in data['records'][8:]:
            item['whole_run_coverage']['one-worker'] = False
            item['coefficients']['c_base'] = 0
        self.assertIn('scene-coverage', gen.generate(data)['pending'])
        for final in (False, True):
            for baseline in (0, 1 << 63, (1 << 63) + 1):
                for index in (8, 9, 10, 11, None):
                    data = document(True)
                    if not final:
                        data['status'] = 'provisional-b12b'
                    for i, item in enumerate(data['records']):
                        if item['kind'] == 'whole-run' and (index is None or i == index):
                            item['coefficients']['c_base'] = baseline
                    with self.subTest(final=final, baseline=baseline, index=index):
                        with self.assertRaisesRegex(gen.Invalid, 'impossible whole-run one-worker baseline'):
                            gen.generate(data)

    def test_one_worker_baseline_must_be_possible_on_output(self):
        for final in (False, True):
            data = document(True)
            if not final:
                data['status'] = 'provisional-b12b'
            # The synthetic maximum keeps every derived selection unchanged
            # when the whole-run bases are tampered with, including at 2**63.
            data['records'][0]['coefficients']['c_base'] = (1 << 63) + 1
            output = gen.generate(data)
            for baseline in (0, 1 << 63, (1 << 63) + 1):
                for index in (8, 9, 10, 11, None):
                    bad = copy.deepcopy(output)
                    for i, item in enumerate(bad['records']):
                        if item['kind'] == 'whole-run' and (index is None or i == index):
                            item['coefficients']['c_base'] = baseline
                    with self.subTest(final=final, baseline=baseline, index=index):
                        with self.assertRaisesRegex(gen.Invalid, 'impossible whole-run one-worker baseline'):
                            gen.validate_output(bad)

    def test_whole_run_never_claims_synthetic_scenes(self):
        for scene in gen.SCENES:
            data = document(True)
            data['records'][8]['coverage'][scene] = True
            with self.subTest(scene=scene), self.assertRaises(gen.Invalid):
                gen.generate(data)

    def test_whole_run_requires_closed_facts_in_both_modes(self):
        for final in (False, True):
            for key in (None, *gen.WHOLE_SCENES, *gen.WHOLE_STORE_SCENES):
                data = document(True)
                if not final:
                    data['status'] = 'provisional-b12b'
                whole = data['records'][8]
                if key is None:
                    del whole['whole_run_coverage']
                else:
                    del whole['whole_run_coverage'][key]
                with self.subTest(final=final, missing=key), self.assertRaises(gen.Invalid):
                    gen.generate(data)
        for value in (1, 'true', None):
            data = document(True)
            data['records'][8]['whole_run_coverage']['resume'] = value
            with self.subTest(value=value), self.assertRaises(gen.Invalid):
                gen.generate(data)
        data = document(True)
        data['records'][8]['whole_run_coverage']['unrecognized'] = True
        with self.assertRaises(gen.Invalid):
            gen.generate(data)

    def test_partial_whole_run_facts_remain_pending(self):
        for scene in gen.WHOLE_SCENES:
            data = document(True)
            data['status'] = 'provisional-b12b'
            data['records'][8]['whole_run_coverage'][scene] = False
            with self.subTest(scene=scene):
                self.assertIn('scene-coverage', gen.generate(data)['pending'])
        data = document(True)
        data['status'] = 'provisional-b12b'
        self.assertNotIn('scene-coverage', gen.generate(data)['pending'])

    def test_companions_are_checked_before_final_selection(self):
        for group, keys in (
            ('coefficients', gen.COEFFICIENTS[1:]),
            ('structural_floors', ('m_dispatch', 'm_end')),
            ('fixed_envelopes', gen.ENVELOPES),
        ):
            for key in keys:
                data = document(True)
                data['status'] = 'provisional-b12b'
                data['records'][8][group][key] -= 1
                with self.subTest(group=group, key=key), self.assertRaises(gen.Invalid):
                    gen.generate(data)
        # A standalone whole-run aggregate is merged with its companion later.
        data = document()
        data['records'] = [record(kind='whole-run')]
        gen.generate(data)

    def test_whole_run_companion_provenance_must_match(self):
        changes = {'revision': 'a' * 40, 'lockfile_sha256': 'a' * 64,
                   'rustc': [1, 99, 1],
                   'native_compilers': {'c': [12, 2, 0], 'cxx': [12, 2, 0]},
                   'native_libraries_sha256': 'a' * 64,
                   'native_flags_sha256': 'a' * 64}
        for key, value in changes.items():
            data = document(True)
            data['records'][8]['provenance'][key] = value
            with self.subTest(key=key), self.assertRaisesRegex(gen.Invalid, 'mismatched build provenance'):
                gen.generate(data)

    def test_whole_run_features_require_the_supported_build(self):
        # A standalone provisional record has no companion to mismatch.
        data = document()
        data['records'] = [record(kind='whole-run')]
        data['records'][0]['provenance']['features'] = 'other'
        with self.assertRaisesRegex(gen.Invalid, 'unsupported value'):
            gen.generate(data)

    def test_whole_run_cannot_hide_an_envelope_breach_in_its_base(self):
        for key, ceiling in gen.ENVELOPES.items():
            data = document(True)
            data['records'][8]['coefficients']['c_base'] = 1 << 30
            data['records'][8]['fixed_envelopes'][key] = ceiling + 1
            with self.subTest(envelope=key), self.assertRaises(gen.Invalid):
                gen.generate(data)

    def test_both_schemas_require_kind_specific_whole_run_facts(self):
        for schema_name in ('aggregate-input-v1.schema.json', 'aggregate-output-v1.schema.json'):
            schema = gen._schema(schema_name)
            for misplaced in (False, True):
                data = document(True)
                if misplaced:
                    data['records'][0]['whole_run_coverage'] = data['records'][8]['whole_run_coverage']
                else:
                    del data['records'][8]['whole_run_coverage']
                with self.subTest(schema=schema_name, misplaced=misplaced), self.assertRaises(gen.Invalid):
                    gen._validate(data['records'][0 if misplaced else 8], schema['$defs']['record'], schema)

    def test_output_revalidates_whole_run_facts_and_certificates(self):
        output = gen.generate(document(True))
        for group, key, value in (
            ('whole_run_coverage', 'resume', False),
            ('coverage', 'stop-phase-1', True),
            ('coefficients', 'm_pool', 1),
        ):
            bad = copy.deepcopy(output)
            whole = next(r for r in bad['records'] if r['kind'] == 'whole-run')
            whole[group][key] = value
            with self.subTest(group=group), self.assertRaises(gen.Invalid):
                gen.validate_output(bad)


class WholeProtocolTests(unittest.TestCase):
    def parse(self, text, workers=1, dc=1):
        from test_collect import collect
        return collect.whole_memory_protocol(text, workers=workers, download_concurrency=dc)

    def test_exact_baseline_and_nonbaseline_protocol(self):
        line = 'WHOLE_MEMORY_V1 4096 100 255 1 1 0 0 128 0 0 0 4096\n'
        self.assertEqual(self.parse('B12_COEFFICIENT c_base 4096\n' + line)['peak'], 4096)
        line = line.replace('255 1 1', '255 8 4')
        self.assertEqual(self.parse(line, 8, 4)['workers'], 8)
        with self.assertRaises(gen.Invalid):
            self.parse('B12_COEFFICIENT c_base 4096\n' + line, 8, 4)

    def test_missing_duplicate_truncated_and_malformed_telemetry_is_rejected(self):
        base = 'B12_COEFFICIENT c_base 4096\n'
        line = 'WHOLE_MEMORY_V1 4096 100 255 1 1 0 0 128 0 0 0 4096\n'
        for text in ('', base, line, base + line + line, base + base + line,
                     base + line.rstrip(), base + line.replace(' 100 ', ' 0 '),
                     base + line.replace(' 255 ', ' 127 '),
                     base + line.replace(' 1 1 ', ' 8 1 '),
                     base + line.replace(' 128 ', ' -128 '),
                     base + line.replace(' 128 ', ' 1.5 '),
                     base + line.replace(' 128 ', ' ' + str(gen.U64_MAX + 1) + ' '),
                     base + line.replace(' 128 ', ' ' + '9' * 5000 + ' '),
                     base + line.replace(' 0 128 ', ' 1 128 '),
                     base + line.replace(' 0 0 0 4096', ' 1 0 0 4096'),
                     base + line.replace(' 4096\n', ' 2 4096\n'),
                     base.replace('4096', '4097') + line,
                     base + line + 'B12_FLOOR m_end 4096\n',
                     base + line.replace('WHOLE_MEMORY_V1', 'WHOLE_MEMORY_V2'),
                     base.replace('4096', '0') + line.replace(' 4096 ', ' 0 '),
                     base + line.replace(' 0 4096\n', ' 1 4096\n'),
                     base + line.replace(' 128 ', ' ' + str(1 << 63) + ' '),
                     base + line.replace(' 0 0 0 4096', ' 0 ' + str(1 << 63) + ' 0 4096'),
                     base + line.replace(' 0 4096\n', ' 0 ' + str(1 << 63) + '\n')):
            with self.subTest(text=text[:100]), self.assertRaises(gen.Invalid):
                self.parse(text)

    def test_missing_component_telemetry_is_refused(self):
        from test_collect import collect
        from unittest.mock import patch
        fixture = record()
        for missing, reason in ((None, None),
                                ('COEFFICIENT', 'missing coefficient measurements'),
                                ('FLOOR', 'missing structural measurements')):
            observations = {'COEFFICIENT': fixture['coefficients'].copy(),
                            'FLOOR': fixture['structural_floors'].copy(), 'ENVELOPE': {}}
            if missing:
                observations[missing].pop(next(iter(observations[missing])))
            with patch.object(collect, 'provenance', return_value=(fixture['target'], fixture['provenance'])), \
                 patch.object(collect, 'run', return_value=''), \
                 patch.object(collect.child, 'run', return_value=collect.child.CompletedProcess([], 1)), \
                 patch.object(collect, 'coverage', return_value=fixture['coverage']), \
                 patch.object(collect, 'numeric_protocol', return_value=observations):
                with self.subTest(missing=missing):
                    if missing:
                        with self.assertRaisesRegex(gen.Invalid, reason):
                            collect.collect('debug', True)
                    else:
                        data = collect.collect('debug', True)
                        self.assertEqual(len(data['records']), 2)
                        gen.validate_input(data)

    def test_dirty_checkout_is_refused(self):
        from test_collect import collect
        from unittest.mock import patch
        # Environment and host validation precede Git; the refusal is before any build.
        def command(args):
            if args[0] == 'rustc':
                return 'rustc 1.99.1\nhost: aarch64-unknown-linux-gnu\n'
            return ' M tracked-source\n'
        with patch.dict(collect.os.environ, {'CARGO_HOME': '/tmp/empty-cargo-home'}, clear=True), \
             patch.object(collect.platform, 'system', return_value='Linux'), \
             patch.object(collect.platform, 'machine', return_value='aarch64'), \
             patch.object(collect, 'run', side_effect=command):
            with self.assertRaisesRegex(gen.Invalid, 'clean source'):
                collect.provenance(True, False)
