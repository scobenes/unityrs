"""Forced constructors compared with the pinned oracle."""
import sys
import unittest

import UnityPy
import unityrs
import binding_y6_review as review
import child


class ForcedReview(unittest.TestCase):
    def compare(self, name):
        observe = getattr(review, name)
        self.assertEqual(observe(unityrs), observe(UnityPy))

    def test_detached_save(self):
        self.compare("observe_detached_save")

    def test_detached_mark_changed(self):
        self.compare("observe_detached_mark")

    def test_detached_set_raw_data(self):
        self.compare("observe_detached_raw")

    def test_detached_patch(self):
        self.compare("observe_detached_patch")

    def test_successful_constructor_reader_states(self):
        self.compare("observe_constructor_states")

    def test_compressed_failures_use_inner_reader(self):
        self.compare("observe_compressed_failures")

    def test_exception_lookup_reenters_on_caller(self):
        for module in ("UnityPy", "unityrs"):
            child.run([sys.executable, "-c",
                       f"import {module} as m; from binding_y6_review import exception_reentry; exception_reentry(m)"],
                      check=True, capture_output=True, text=True, timeout=45)
