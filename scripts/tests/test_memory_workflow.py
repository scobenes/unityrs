"""Native calibration publishes only a successful aggregate document."""
from pathlib import Path
import unittest
try:
    import yaml
except ImportError:
    yaml = None


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

    def test_only_aggregate_json_is_uploaded_after_success(self):
        uploads = [s for s in self.job['steps'] if 'actions/upload-artifact@' in s.get('uses', '')]
        self.assertEqual(len(uploads), 1)
        self.assertEqual(uploads[0]['with']['path'], '${{ runner.temp }}/aggregate.json')
        self.assertNotIn('if', uploads[0])
        self.assertEqual(uploads[0]['with']['if-no-files-found'], 'error')

    def test_collection_is_native_and_asserts_architecture_and_page_size(self):
        commands = '\n'.join(s.get('run', '') for s in self.job['steps'])
        self.assertIn('uname -m', commands)
        self.assertIn('getconf PAGESIZE', commands)
        self.assertIn('tools/memory/collect.py --native', commands)
        self.assertIn('--bootstrap', commands)
        self.assertIn('--compiled-evidence docs/memory/constants-v1.json', commands)
        self.assertNotIn('--emulated', commands)
        self.assertNotIn('--allow-dirty', commands)

    def test_bound_changes_and_manual_dispatch_trigger_calibration(self):
        triggers = self.workflow.get('on', self.workflow.get(True))
        self.assertIn('workflow_dispatch', triggers)
        paths = triggers['pull_request']['paths']
        for required in ('crates/**', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/**',
                         'tools/memory/**', 'docs/memory/**', 'docs/BATCH_SPEC.md', 'docs/plans/batch.md'):
            self.assertIn(required, paths)


if __name__ == '__main__':
    unittest.main()
