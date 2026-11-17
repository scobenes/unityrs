"""Cache identity and exporter roots/default arguments against the oracle."""
import unittest

import UnityPy
import unityrs
import binding_y5_round4 as cases


class ReviewRound4(unittest.TestCase):
    def test_concurrent_container_access_shares_one_mutable_helper(self):
        expected = cases.observe_container_identity(UnityPy)
        self.assertEqual(expected, [1, ['shared'] * 4, True])
        self.assertEqual(cases.observe_container_identity(unityrs), expected)

    def test_container_extraction_accepts_equivalent_root_spellings(self):
        expected = cases.observe_root_spellings(UnityPy)
        self.assertTrue(all('value' in row[0] and len(row[1]) == 2 for row in expected.values()))
        self.assertEqual(cases.observe_root_spellings(unityrs), expected)

    def test_object_reader_cache_publication_holds_environment_lock(self):
        cases.check_object_reader_publication(unityrs)
        cases.check_object_reader_publication(unityrs, detached=True)

    def test_sprite_cache_decodes_once_under_concurrent_first_access(self):
        cases.check_sprite_cache_publication(unityrs)
        cases.check_sprite_cache_publication(unityrs, detached=True)


def extension_test(name):
    def test(self):
        expected = cases.observe_none_extensions(UnityPy)[name]
        if name == 'Sprite':
            self.assertTrue(all(row[0]['error'] == 'ValueError' for row in expected))
        else:
            self.assertTrue(all('value' in row[0] for row in expected))
        self.assertEqual(cases.observe_none_extensions(unityrs)[name], expected)
    return test


for name in ('TextAsset', 'Font', 'Mesh', 'Shader', 'MonoBehaviour', 'AudioClip',
             'Sprite', 'Texture2D', 'GameObject'):
    setattr(ReviewRound4, 'test_none_extension_' + name, extension_test(name))
