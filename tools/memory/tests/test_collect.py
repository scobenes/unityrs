"""The collector accepts only closed aggregate observations and real coverage."""
import importlib.util
import io
from contextlib import redirect_stderr
from pathlib import Path
import unittest
from unittest.mock import patch
import sys
from test_regenerate import gen, scratch

sys.modules['regenerate'] = gen
SPEC = importlib.util.spec_from_file_location('memory_collect', Path(gen.__file__).with_name('collect.py'))
collect = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(collect)


class ProtocolTests(unittest.TestCase):
    def test_repeated_system_kills_never_echo_child_output(self):
        output = scratch.for_case(self, 'memory-collector-failure-') / 'aggregate.json'
        stderr = io.StringIO()
        with patch.object(collect, 'collect', side_effect=collect.child.SystemKilled('raw child output')):
            with redirect_stderr(stderr):
                status = collect.main(['--native', '--profile', 'debug', '--output', str(output)])
        self.assertEqual(status, 2)
        self.assertEqual(stderr.getvalue(),
                         'calibration failed; no aggregate published\nreason: SystemKilled\n')
        self.assertFalse(output.exists())

    def test_a_refusal_names_its_reason_and_a_failed_command_its_step_only(self):
        output = scratch.for_case(self, 'memory-collector-refusal-') / 'aggregate.json'
        stderr = io.StringIO()
        with patch.object(collect, 'collect', side_effect=collect.gen.Invalid('calibration requires clean source')):
            with redirect_stderr(stderr):
                status = collect.main(['--native', '--profile', 'debug', '--output', str(output)])
        self.assertEqual(status, 2)
        self.assertEqual(stderr.getvalue(), 'calibration failed; no aggregate published\n'
                         'reason: calibration requires clean source\n')
        failed = collect.child.CompletedProcess(['cargo'], 101, b'raw child output', b'raw child error')
        with patch.object(collect.child, 'run', return_value=failed):
            with self.assertRaises(collect.gen.Invalid) as raised:
                collect.run(['cargo', 'test', '--locked', '-p', 'unityrs-batch', '--manifest-path', '/x/Cargo.toml'])
        message = str(raised.exception)
        self.assertEqual(message, 'calibration command failed (cargo test --locked -p unityrs-batch --manifest-path, status 101)')
        self.assertNotIn('raw', message)
        self.assertFalse(output.exists())

    def test_numeric_maxima_do_not_include_unrelated_test_output(self):
        text = 'unrelated sensitive text\nB12_COEFFICIENT c_base 1\nB12_COEFFICIENT c_base 3\nB12_FLOOR m_end 4000\n'
        self.assertEqual(collect.numeric_protocol(text), {
            'COEFFICIENT': {'c_base': 3}, 'FLOOR': {'m_end': 4000}, 'ENVELOPE': {}})

    def test_unknown_or_noninteger_observations_are_refused(self):
        for text in ('B12_COEFFICIENT arbitrary 3', 'B12_COEFFICIENT c_base -1',
                     'B12_COEFFICIENT c_base 1.0', 'B12_FLOOR c_base 1',
                     'B12_ENVELOPE finisher ' + str(gen.U64_MAX + 1)):
            with self.subTest(text=text), self.assertRaises(gen.Invalid):
                collect.numeric_protocol(text)

    def test_every_coverage_flag_has_an_explicit_test(self):
        self.assertEqual(set(collect.COVERAGE), set(gen.SCENES))
        self.assertTrue(all(collect.COVERAGE.values()))
        tests = {test for names in collect.COVERAGE.values() for test in names}
        text = ''.join('test ' + name + ' ... ok\n' for name in tests)
        self.assertTrue(all(collect.coverage(text).values()))
        # The final test in each binary is followed by libtest's summary;
        # --nocapture can put numeric observations before its outcome.
        text = ''.join('test ' + name + ' ... \nB12_COEFFICIENT c_base 1\nok\n'
                       '\ntest result: ok. 1 passed; 0 failed; 0 ignored\n'
                       for name in tests)
        self.assertTrue(all(collect.coverage(text).values()))

    def test_missing_coverage_never_becomes_a_successful_record(self):
        tests = {test for names in collect.COVERAGE.values() for test in names}
        for omitted in tests:
            text = ''.join('test ' + n + ' ... ok\n' for n in tests if n != omitted)
            with self.subTest(test=omitted), self.assertRaises(gen.Invalid):
                collect.coverage(text)

    def test_ignored_tests_do_not_supply_coverage_to_diagnostic_records(self):
        tests = {test for names in collect.COVERAGE.values() for test in names}
        omitted = 'store_multipart_normal_and_stopped_transfers_remain_counted'
        text = ''.join('test ' + n + ' ... ' + ('ignored, scale' if n == omitted else 'ok') + '\n'
                       for n in tests)
        with self.assertRaises(gen.Invalid):
            collect.coverage(text)
        self.assertFalse(collect.coverage(text, complete=False)['store-multipart'])

    def test_both_profiles_build_locked_test_hooks_without_cross_compilation(self):
        for profile in gen.PROFILES:
            library, view = collect.support_commands(profile)
            self.assertIn('stage::env::tests::', library)
            self.assertIn('uncharged_dependency_owners_fit_the_bounded_view_share', view)
            for command in (*collect.commands(profile), library, view):
                self.assertIn('--locked', command)
                self.assertEqual('--release' in command, profile == 'release')
                self.assertNotIn('--target', command)

    def test_native_fingerprint_is_deterministic(self):
        first = collect.native_fingerprint()
        self.assertRegex(first, r'^[0-9a-f]{64}$')
        self.assertEqual(first, collect.native_fingerprint())

    def test_host_versions_have_closed_numeric_fields(self):
        self.assertEqual(collect.version('rustc 1.99.1 (revision)', 3), [1, 99, 1])
        self.assertEqual(collect.version('2.39', 2), [2, 39])
        with self.assertRaises(gen.Invalid):
            collect.version('arbitrary', 3)

    def test_build_and_allocator_overrides_are_ineligible(self):
        for name in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CFLAGS', 'CXXFLAGS',
                     'CARGO_PROFILE_RELEASE_LTO', 'CFLAGS_aarch64_unknown_linux_gnu',
                     'CC', 'RUSTC_WRAPPER', 'LD_PRELOAD', 'GLIBC_TUNABLES',
                     'MALLOC_ARENA_MAX', 'UNITYRS_TEST_TRACE', 'HOST_CC', 'TARGET_CXX',
                     'HOST_CFLAGS', 'TARGET_AR', 'CRATE_CC_NO_DEFAULTS',
                     'B12_RETENTION', 'X12_JOB'):
            with self.subTest(name=name), self.assertRaises(gen.Invalid):
                collect.check_build_environment({name: 'override'})
        collect.check_build_environment({'CARGO_TARGET_DIR': '/tmp/build', 'HOME': '/tmp'})


if __name__ == '__main__':
    unittest.main()
