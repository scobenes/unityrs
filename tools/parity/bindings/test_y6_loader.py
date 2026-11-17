"""The remaining loader observations compared with the pinned oracle."""
import unittest
import UnityPy
import unityrs
import binding_y6_loader as O


class LoaderResidue(unittest.TestCase):
    def compare(self, name, corrected=False):
        function = getattr(O, 'observe_' + name)
        self.assertEqual(function(unityrs), function(UnityPy, corrected=True) if corrected else function(UnityPy))

    def test_sensitive_path(self):
        self.compare('sensitive_path')

    def test_case_lookup(self):
        self.compare('case_lookup')

    def test_hashes(self):
        self.compare('hashes')

    def test_explicit_readers(self):
        self.compare('readers')

    def test_mutable_buffers_and_readonly_streams(self):
        self.compare('buffers', True)

    def test_exact_case_pointer_retry(self):
        self.compare('pointer_case', True)

    def test_suffix_boundaries(self):
        self.compare('suffix', True)

    def test_prefixed_environment_path(self):
        self.compare('urls', True)

    def test_symlink_walk_forms(self):
        self.compare('symlinks')
