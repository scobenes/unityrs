"""Codec callbacks run on the caller and reader replacements redirect parsing."""
import unittest
import UnityPy
import unityrs
import binding_y6b_codecs as O


class Internals(unittest.TestCase):
    def test_malformed_standalone_codec_exceptions(self):
        self.assertEqual(O.observe_codec_failures(unityrs), O.observe_codec_failures(UnityPy))

    def test_reentrant_bundle_decompression_callbacks(self):
        self.assertEqual(O.observe_codec_callbacks(unityrs), O.observe_codec_callbacks(UnityPy))

    def test_reentrant_bundle_compression_and_chunk_maps(self):
        self.assertEqual(O.observe_codec_compression(unityrs), O.observe_codec_compression(UnityPy))

    def test_replaced_reader_and_endian(self):
        self.assertEqual(O.observe_reader_redirect(unityrs), O.observe_reader_redirect(UnityPy))

    def test_partial_failure_cursor(self):
        self.assertEqual(O.observe_reader_failures(unityrs), O.observe_reader_failures(UnityPy))
