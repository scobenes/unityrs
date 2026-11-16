"""Round-one helper regressions compared with UnityPy 1.25.2."""
import unittest
import UnityPy
import unityrs
import binding_y6b_review as O


class HelperReview(unittest.TestCase):
    def test_schema_failure_keeps_original_cursor(self):
        self.assertEqual(O.observe_schema_failure_cursor(unityrs), O.observe_schema_failure_cursor(UnityPy))

    def test_truncated_objects_keep_actual_stream_advance(self):
        self.assertEqual(O.observe_truncated_object_cursor(unityrs), O.observe_truncated_object_cursor(UnityPy))

    def test_live_string_and_byte_callbacks_run_on_caller(self):
        self.assertEqual(O.observe_tree_live_callbacks(unityrs), O.observe_tree_live_callbacks(UnityPy))

    def test_empty_shader_blob_still_decompresses_platforms(self):
        self.assertEqual(O.observe_shader_empty_blob(unityrs), O.observe_shader_empty_blob(UnityPy))

    def test_lz4_signed_arguments_and_header_rules(self):
        self.assertEqual(O.observe_lz4_signed_sizes(unityrs), O.observe_lz4_signed_sizes(UnityPy))
