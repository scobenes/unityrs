"""Filesystem containment regressions using synthetic split resources."""
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import scratch


class FindFilePaths(unittest.TestCase):
    def test_filesystem_inventory_is_charged_before_caching(self):
        import unityrs
        from unityrs import _search
        root = scratch.for_case(self)
        (root / 'raw.resource').write_bytes(b'inside')
        for bound, maximum in (('MAX_ENTRIES', 1), ('MAX_BYTES', 1024)):
            env = unityrs.Environment(path=root)
            with self.subTest(bound=bound), patch.object(_search, bound, maximum):
                with self.assertRaises(unityrs.LimitError):
                    env.find_file('missing.resource')
                self.assertEqual(env.local_files, [])

    def test_deep_filesystem_inventory_uses_iterative_traversal(self):
        import unityrs
        from types import SimpleNamespace
        root = scratch.for_case(self)
        def entries(path):
            depth = 0 if str(path) == str(root) else int(Path(path).name[1:])
            child = root / (f'd{depth + 1}' if depth < 1500 else 'raw.resource')
            return iter([SimpleNamespace(path=str(child),
                                         is_dir=lambda **kw: depth < 1500)])
        env = unityrs.Environment(path=root)
        with patch.object(os, 'scandir', side_effect=entries):
            with self.assertRaises(FileNotFoundError):
                env.find_file('missing.resource')
        self.assertEqual(env.local_files, [str(root / 'raw.resource')])

    def test_split_companions_cannot_escape_root(self):
        import unityrs
        root = scratch.for_case(self)
        inside = root / 'inside'
        inside.mkdir()
        outside = root / 'outside.resource'
        outside.write_bytes(b'outside')
        for selected, escaped in ((0, 1), (1, 0), (0, 2)):
            with self.subTest(selected=selected, escaped=escaped):
                base = inside / f'raw{selected}{escaped}.resource'
                for index in range(max(selected, escaped) + 1):
                    part = Path(f'{base}.split{index}')
                    if index == escaped:
                        part.symlink_to(outside)
                    else:
                        part.write_bytes(b'inside')
                for dependency in (False, True):
                    env = unityrs.Environment(path=inside)
                    with self.assertRaises(FileNotFoundError):
                        env.find_file(f'{base.name}.split{selected}', dependency)
                    self.assertEqual(env.files, {})
                    self.assertEqual(env.cabs, {})

    def test_internal_split_aliases_keep_keys_and_join_rules(self):
        import UnityPy
        import unityrs
        scratch_root = scratch.for_case(self)
        root = scratch_root / 'inside'
        root.mkdir()
        outside = scratch_root / 'outside.resource'
        outside.write_bytes(b'outside')
        (root / 'first.resource').write_bytes(b'first')
        (root / 'joined.resource.split0').symlink_to(root / 'first.resource')
        (root / 'joined.resource.split1').write_bytes(b'second')
        # A gap terminates the join; later parts are never opened.
        (root / 'joined.resource.split3').symlink_to(outside)
        for name in ('joined.resource.split0', 'joined.resource.split1'):
            actual = unityrs.Environment(path=root)
            oracle = UnityPy.Environment(path=str(root))
            result = actual.find_file(name, is_dependency=False)
            expected = oracle.find_file(name, is_dependency=False)
            self.assertEqual(result.bytes, b'firstsecond')
            self.assertEqual(result.bytes, expected.bytes)
            self.assertEqual(list(actual.files), list(oracle.files))

    def test_missing_split_zero_is_skipped(self):
        import unityrs
        root = scratch.for_case(self)
        (root / 'raw.resource.split1').write_bytes(b'first')
        (root / 'raw.resource.split2').write_bytes(b'second')
        result = unityrs.Environment(path=root).find_file('raw.resource.split1')
        self.assertEqual(result.bytes, b'firstsecond')

    def test_different_case_sibling_remains_outside_on_sensitive_filesystem(self):
        import unityrs
        root = scratch.for_case(self)
        inside = root / 'inside'
        inside.mkdir()
        other = root / 'INSIDE'
        if other.exists():
            self.assertTrue(os.path.samefile(inside, other))
            return
        other.mkdir()
        resource = other / 'raw.resource'
        resource.write_bytes(b'outside')
        with self.assertRaises(FileNotFoundError):
            unityrs.Environment(path=inside).find_file(str(resource))

    def test_absolute_root_case_alias_uses_filesystem_identity(self):
        import UnityPy
        import unityrs
        root = Path(__file__).resolve().parents[3]
        alias = root.with_name(root.name.swapcase())
        candidate = alias / 'Cargo.toml'
        env = unityrs.Environment(path=root)
        if alias.exists() and os.path.samefile(root, alias):
            result = env.find_file(str(candidate))
            self.assertEqual(result.bytes, (root / 'Cargo.toml').read_bytes())
            expected = UnityPy.Environment(path=str(root)).find_file(str(candidate))
            self.assertEqual(result.bytes, expected.bytes)
        else:
            self.assert_find_file_matches_unitypy(root, str(candidate))

    def test_case_sensitive_alias_follows_the_suffix_fallback(self):
        # The branch above that a case-sensitive file system takes, on a
        # tree it controls (scratch lives under the temporary directory): the
        # alias is not the root, so the lookup falls back to UnityPy's suffix
        # match, or raises when nothing matches.
        root = scratch.for_case(self) / 'proj'
        (root / 'sub').mkdir(parents=True)
        (root / 'Cargo.toml').write_bytes(b'toml')
        (root / 'Cargo.lock').write_bytes(b'lock')
        (root / 'sub' / 'x.bin').write_bytes(b'x')
        alias = root.with_name('PROJ')
        if alias.exists():
            self.skipTest('case-insensitive file system')
        for name in ('Cargo.toml', 'zzz.toml'):
            self.assert_find_file_matches_unitypy(root, str(alias / name))

    def assert_find_file_matches_unitypy(self, root, candidate):
        import UnityPy
        import unityrs
        try:
            expected = UnityPy.Environment(path=str(root)).find_file(candidate)
        except FileNotFoundError:
            with self.assertRaises(FileNotFoundError):
                unityrs.Environment(path=str(root)).find_file(candidate)
            return
        result = unityrs.Environment(path=str(root)).find_file(candidate)
        self.assertEqual(result is None, expected is None)
        self.assertEqual(getattr(result, 'bytes', None), getattr(expected, 'bytes', None))
