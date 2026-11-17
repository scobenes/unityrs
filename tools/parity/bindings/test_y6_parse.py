"""Forced helper parsing compared with pinned UnityPy and bounded inputs."""
import contextlib
import io
import unittest

import UnityPy
import unityrs
import binding_y6_parse as observations


class ParseFile(unittest.TestCase):
    def test_forced_raw_types_and_asset_suffixes_preserve_reader(self):
        self.assertEqual(observations.observe_forced_raw(unityrs),
                         observations.observe_forced_raw(UnityPy))

    def test_explicit_parser_ignores_detection(self):
        self.assertEqual(observations.observe_forced_parsers(unityrs),
                         observations.observe_forced_parsers(UnityPy))

    def test_forced_failure_prints_and_restores_reader(self):
        self.assertEqual(observations.observe_forced_failures(unityrs),
                         observations.observe_forced_failures(UnityPy))

    def test_forced_parsers_refuse_oversized_input(self):
        env = unityrs.Environment(limits=unityrs.Limits(max_input_bytes=8))
        reader = unityrs.streams.EndianBinaryReader(io.BytesIO(b"x" * 64))
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(unityrs.LimitError):
            unityrs.helpers.ImportHelper.parse_file(
                reader, env, "large", typ=unityrs.enums.FileType.BundleFile)
        self.assertEqual(reader.Position, 0)
        self.assertEqual(env.files, {})

    def test_forced_parser_budget_refusal_keeps_environment_empty(self):
        env = unityrs.Environment(budget=unityrs.Budget(0))
        reader = unityrs.streams.EndianBinaryReader(b"raw")
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(MemoryError):
            unityrs.helpers.ImportHelper.parse_file(
                reader, env, "forced", typ=unityrs.enums.FileType.AssetsFile)
        self.assertEqual(env.files, {})

    def test_forced_raw_preserves_position_and_skips_detection(self):
        reader = unityrs.streams.EndianBinaryReader(b"A" * 20)
        reader.Position = 7
        env = unityrs.Environment(budget=unityrs.Budget(0))
        result = unityrs.helpers.ImportHelper.parse_file(
            reader, env, "forced", typ=unityrs.enums.FileType.ResourceFile)
        self.assertIs(result, reader)
        self.assertEqual(reader.Position, 7)

    def test_nonzero_parser_position_has_a_named_refusal(self):
        original = observations.observe_forced_positions(UnityPy)
        self.assertEqual([entry.get("error") for entry in original],
                         ["error", "NotImplementedError", None, "error", "Exception", None])
        self.assertEqual(observations.observe_forced_positions(unityrs),
                         observations.observe_forced_positions(UnityPy, corrected=True))
