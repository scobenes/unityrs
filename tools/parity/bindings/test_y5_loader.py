"""Remaining loader API compared with the pinned oracle."""
import unittest

import UnityPy
import unityrs
import binding_y5_loader as observations


class Loader(unittest.TestCase):
    def compare(self, function):
        self.assertEqual(function(unityrs), function(UnityPy))

    def test_constructor_and_loading_methods(self):
        self.compare(observations.observe_paths)

    def test_supplied_filesystem_and_local_lookup_quirks(self):
        self.assertEqual(observations.observe_filesystem(unityrs), observations.observe_filesystem(UnityPy, corrected=True))

    def test_stream_name_types(self):
        self.assertEqual(observations.observe_streams(unityrs), observations.observe_streams(UnityPy, corrected=True))

    def test_split_loading_and_deduplication(self):
        self.compare(observations.observe_splits)

    def test_dependency_inventory_and_cached_lookup(self):
        self.compare(observations.observe_lookup)
