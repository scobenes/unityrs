"""Final selection requires native synthetic and whole-run evidence together."""
import copy
import unittest
import io
import json
from contextlib import redirect_stderr

from test_regenerate import gen, document, record, scratch


class FinalEvidenceTests(unittest.TestCase):
    def test_synthetic_matrix_alone_cannot_freeze_constants(self):
        data = document(True)
        data['records'] = [r for r in data['records'] if r['kind'] == 'synthetic']
        with self.assertRaises(gen.Invalid):
            gen.generate(data)

    def test_partial_evidence_reports_missing_whole_run_matrix(self):
        data = document(True)
        data['status'] = 'provisional-b12b'
        data['records'] = data['records'][:8]
        self.assertIn('whole-run-matrix', gen.generate(data)['pending'])

    def test_one_native_whole_run_architecture_is_sufficient(self):
        data = document(True)
        data['records'][-1] = record(gen.TARGETS[1], 'release', 'store', 'whole-run')
        self.assertTrue(gen.generate(data)['shipping_replacement'])

    def test_whole_run_cannot_substitute_for_a_synthetic_axis(self):
        data = document(True)
        data['records'][0]['kind'] = 'whole-run'
        data['records'].pop()
        with self.assertRaises(gen.Invalid):
            gen.generate(data)

    def test_emulated_whole_run_is_ineligible(self):
        data = document(True)
        data['records'][-1]['provenance']['native'] = False
        with self.assertRaises(gen.Invalid):
            gen.generate(data)

    def test_whole_run_requires_complete_applicable_coverage(self):
        for scene in gen.SCENES:
            data = document(True)
            data['records'][-1]['coverage'][scene] = False
            with self.subTest(scene=scene), self.assertRaises(gen.Invalid):
                gen.generate(data)

    def test_each_whole_run_profile_and_sink_axis_is_required(self):
        data = document(True)
        for index in range(8, 12):
            changed = copy.deepcopy(data)
            del changed['records'][index]
            with self.subTest(index=index), self.assertRaises(gen.Invalid):
                gen.generate(changed)

    def test_whole_run_maximum_participates_in_selection(self):
        data = document(True)
        data['records'][-1]['coefficients']['c_base'] = 262145
        self.assertEqual(gen.generate(data)['coefficients']['c_base']['selected'], 331776)

    def test_whole_run_provenance_must_match_synthetic_build(self):
        for field in ('revision', 'lockfile_sha256', 'native_libraries_sha256', 'native_flags_sha256'):
            data = document(True)
            data['records'][-1]['provenance'][field] = '4' * len(data['records'][-1]['provenance'][field])
            with self.subTest(field=field), self.assertRaises(gen.Invalid):
                gen.generate(data)

    def test_duplicate_whole_run_axis_is_rejected(self):
        data = document(True)
        data['records'].append(copy.deepcopy(data['records'][-1]))
        with self.assertRaises(gen.Invalid):
            gen.generate(data)

    def test_generated_selection_is_the_only_shipping_selection(self):
        source = gen.rust_source(gen.generate(document(True))).decode()
        self.assertIn('pub const SELECTED: Constants = MEASURED;', source)
        self.assertIn('pub const CALIBRATED: bool = true;', source)

    def test_bootstrap_selects_estimates_without_fabricating_measurements(self):
        source = gen.bootstrap_source().decode()
        self.assertIn('pub const SELECTED: Constants = super::PROVISIONAL;', source)
        self.assertIn('pub const CALIBRATED: bool = false;', source)
        self.assertNotIn('MEASURED:', source)

    def test_dirty_source_record_cannot_freeze_constants(self):
        data = document(True)
        data['records'][-1]['provenance']['clean'] = False
        with self.assertRaises(gen.Invalid):
            gen.generate(data)


class FinalCommandTests(unittest.TestCase):
    def setUp(self):
        self.folder = scratch.for_case(self, 'memory-final-')

    def invoke(self, arguments):
        with redirect_stderr(io.StringIO()):
            return gen.main(arguments)

    def test_multiple_downloaded_documents_and_whole_run_generate_then_check(self):
        records = document(True)['records']
        inputs = []
        for index, subset in enumerate((records[:4], records[4:8], records[8:])):
            path = self.folder / f'input{index}.json'
            path.write_text(json.dumps({'schema_version': 1, 'status': 'provisional-b12b',
                                        'records': subset}))
            inputs.append(str(path))
        rust = self.folder / 'measured.rs'
        output = self.folder / 'selected.json'
        args = [*inputs, '--final', '--output', str(output), '--rust-output', str(rust)]
        self.assertEqual(self.invoke(args), 0)
        self.assertEqual(self.invoke([*args, '--check']), 0)
        verify = ['--compiled-evidence', str(output), '--rust-output', str(rust), '--check']
        self.assertEqual(self.invoke(verify), 0)
        rust.write_text('stale')
        self.assertEqual(self.invoke(verify), 1)
        self.assertEqual(rust.read_text(), 'stale')

    def test_compiled_evidence_mode_cannot_mutate_or_accept_provisional_evidence(self):
        source = self.folder / 'selected.json'
        rust = self.folder / 'measured.rs'
        source.write_bytes(gen.render(gen.generate(document())))
        args = ['--compiled-evidence', str(source), '--rust-output', str(rust)]
        self.assertEqual(self.invoke(args), 2)
        self.assertEqual(self.invoke([*args, '--check']), 2)
        self.assertFalse(rust.exists())

    def test_bootstrap_command_is_read_only_when_checking(self):
        rust = self.folder / 'measured.rs'
        args = ['--bootstrap', '--rust-output', str(rust)]
        self.assertEqual(self.invoke([*args, '--check']), 1)
        self.assertFalse(rust.exists())
        self.assertEqual(self.invoke(args), 0)
        before = rust.stat().st_mtime_ns
        self.assertEqual(self.invoke([*args, '--check']), 0)
        self.assertEqual(rust.stat().st_mtime_ns, before)


if __name__ == '__main__':
    unittest.main()
