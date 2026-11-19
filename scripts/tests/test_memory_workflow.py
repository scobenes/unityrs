"""Native calibration publishes only a successful aggregate document."""
import os
from pathlib import Path
import sys
import unittest
try:
    import yaml
except ImportError:
    yaml = None

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'tools/parity'))
import child  # noqa: E402
import scratch  # noqa: E402


@unittest.skipIf(yaml is None, 'needs PyYAML (scripts/requirements-tests.txt)')
class MemoryWorkflowTests(unittest.TestCase):
    def setUp(self):
        path = Path(__file__).resolve().parents[2] / '.github/workflows/memory-calibration.yml'
        self.workflow = yaml.safe_load(path.read_text())
        self.job = self.workflow['jobs']['synthetic']

    def test_matrix_uses_both_native_architectures_and_supported_profiles(self):
        matrix = self.job['strategy']['matrix']
        self.assertEqual(set(matrix['profile']), {'debug', 'release'})
        self.assertEqual(set(matrix['arch']), {'x86_64', 'aarch64'})
        self.assertEqual({row['arch']: row['runner'] for row in matrix['include']}, {
            'x86_64': 'ubuntu-24.04', 'aarch64': 'ubuntu-24.04-arm'})

    def test_jobs_use_the_pinned_bookworm_image_without_extra_privileges(self):
        self.assertEqual(self.job['container'], {
            'image': 'debian:bookworm@sha256:268fb24c39bca92d326dd83ea3abebe4a3f7751c804ffef23bb6d857dd343ff8'})

    def test_debian_dependencies_are_installed_before_checkout(self):
        steps = self.job['steps']
        install = next(i for i, s in enumerate(steps) if 'apt-get install' in s.get('run', ''))
        checkout = next(i for i, s in enumerate(steps) if 'actions/checkout@' in s.get('uses', ''))
        self.assertLess(install, checkout)
        command = steps[install]['run'].replace('\\\n', ' ')
        self.assertIn('--no-install-recommends', command)
        for package in ('gcc', 'g++', 'make', 'git', 'curl', 'ca-certificates',
                        'python3', 'libc6-dev', 'procps'):
            self.assertIn(package, command.split())

    def test_rustup_is_pinned_and_checksum_verified_for_each_native_architecture(self):
        script = next(s['run'] for s in self.job['steps'] if 'rustup-init' in s.get('run', ''))
        self.assertIn('https://static.rust-lang.org/rustup/archive/1.28.2/', script)
        for arch, digest in (
            ('x86_64', '20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c'),
            ('aarch64', 'e3853c5a252fca15252d07cb23a1bdd9377a8c6f3efa01531109281ae47f841c'),
        ):
            self.assertRegex(script, rf'{arch}\)\s+sha256={digest}\s*;;')
        self.assertIn('sha256sum --check --strict', script)
        self.assertLess(script.index('sha256sum --check --strict'), script.index('chmod +x'))
        self.assertIn('--default-toolchain none', script)
        self.assertIn('rustup toolchain install --no-self-update', script)

    def test_compilers_are_asserted_before_measurement(self):
        steps = self.job['steps']
        assertion = next(i for i, s in enumerate(steps) if '-dumpfullversion' in s.get('run', ''))
        collection = next(i for i, s in enumerate(steps) if 'tools/memory/collect.py' in s.get('run', ''))
        self.assertLess(assertion, collection)
        script = steps[assertion]['run']
        folder = scratch.for_case(self, 'memory-workflow-compilers-')
        for compiler, variable in (('cc', 'TEST_CC_VERSION'), ('c++', 'TEST_CXX_VERSION')):
            path = folder / compiler
            path.write_text('#!/bin/sh\n[ "$1" = -dumpfullversion ] || exit 2\n'
                            f'printf \'%s\\n\' "${variable}"\n')
            path.chmod(0o755)
        for c, cxx, mismatch in (('12.2.0', '12.2.0', None),
                                 ('13.3.0', '12.2.0', 'cc'),
                                 ('12.2.0', '13.3.0', 'c++'),
                                 ('12.2.0', '', 'c++')):
            with self.subTest(c=c, cxx=cxx):
                env = dict(os.environ, PATH=str(folder) + os.pathsep + os.defpath,
                           TEST_CC_VERSION=c, TEST_CXX_VERSION=cxx)
                result = child.run(['sh', '-e', '-c', script], env=env,
                                   capture_output=True, text=True)
                self.assertEqual(result.returncode, 0 if mismatch is None else 1, result.stderr)
                if mismatch:
                    self.assertIn(f'{mismatch} version mismatch: expected 12.2.0, got ', result.stderr)

    def test_checkout_uses_pr_head_and_marks_workspace_safe(self):
        checkout = next(s for s in self.job['steps'] if 'actions/checkout@' in s.get('uses', ''))
        self.assertEqual(checkout['with']['ref'], '${{ github.event.pull_request.head.sha || github.sha }}')
        commands = '\n'.join(s.get('run', '') for s in self.job['steps'])
        self.assertIn('git config --global --add safe.directory "$GITHUB_WORKSPACE"', commands)

    def test_only_aggregate_json_is_uploaded_after_success(self):
        uploads = [s for s in self.job['steps'] if 'actions/upload-artifact@' in s.get('uses', '')]
        self.assertEqual(len(uploads), 1)
        self.assertEqual(uploads[0]['with']['path'], 'calibration-aggregate.json')
        steps = self.job['steps']
        collect = next(i for i, s in enumerate(steps) if 'tools/memory/collect.py' in s.get('run', ''))
        stage = next(i for i, s in enumerate(steps) if s.get('run', '').startswith('cp "$RUNNER_TEMP/aggregate.json"'))
        self.assertIn('--output "$RUNNER_TEMP/aggregate.json"', steps[collect]['run'])
        self.assertLess(collect, stage)
        self.assertLess(stage, steps.index(uploads[0]))
        # Container jobs: expressions name host paths, so no step may use them.
        for step in steps:
            for text in (step.get('run', ''), *map(str, step.get('with', {}).values())):
                self.assertNotIn('runner.temp', text)
                self.assertNotIn('github.workspace', text)
        self.assertNotIn('if', uploads[0])
        self.assertEqual(uploads[0]['with']['if-no-files-found'], 'error')

    def test_collection_is_native_and_asserts_architecture_and_page_size(self):
        commands = '\n'.join(s.get('run', '') for s in self.job['steps'])
        self.assertIn('uname -m', commands)
        self.assertIn('getconf PAGESIZE', commands)
        self.assertIn('tools/memory/collect.py --native', commands)
        self.assertNotIn('--emulated', commands)
        self.assertNotIn('--allow-dirty', commands)

    def test_compiled_evidence_is_checked_unconditionally_before_collection(self):
        steps = self.job['steps']
        checks = [i for i, step in enumerate(steps)
                  if 'tools/memory/regenerate.py' in step.get('run', '')]
        self.assertEqual(len(checks), 1)
        check, = checks
        self.assertNotIn('if', steps[check])
        self.assertEqual(steps[check]['run'].split(), [
            'python3', 'tools/memory/regenerate.py',
            '--compiled-evidence', 'docs/memory/constants-v1.json',
            '--rust-output', 'crates/unityrs-batch/src/bound/measured.rs', '--check'])
        collect = next(i for i, s in enumerate(steps) if 'tools/memory/collect.py' in s.get('run', ''))
        self.assertLess(check, collect)

    def test_bound_changes_and_manual_dispatch_trigger_calibration(self):
        triggers = self.workflow.get('on', self.workflow.get(True))
        self.assertIn('workflow_dispatch', triggers)
        paths = triggers['pull_request']['paths']
        for required in ('crates/**', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/**',
                         'tools/memory/**', 'docs/memory/**', 'docs/BATCH_SPEC.md', 'docs/plans/batch.md'):
            self.assertIn(required, paths)


if __name__ == '__main__':
    unittest.main()
