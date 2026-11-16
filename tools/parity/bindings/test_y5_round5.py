"""Review round five against the pinned oracle and exact-string references."""
import unittest
import UnityPy
import unityrs
import binding_y5_round5 as cases


class ReviewRound5(unittest.TestCase):
    def test_repeated_root_separators(self):
        expected = cases.observe_repeated_roots(UnityPy)
        self.assertTrue(all('value' in row[0] and len(row[1]) == 2 for row in expected))
        self.assertEqual(cases.observe_repeated_roots(unityrs), expected)

    def test_hostile_direct_arguments(self):
        cases.check_strings(unityrs, 'direct')

    def test_hostile_extractor_arguments_and_names(self):
        cases.check_strings(unityrs, 'extract')

    def test_hostile_class_and_environment_arguments_and_names(self):
        cases.check_strings(unityrs, 'classes')

    def test_audio_scalar_names_retain_formatting(self):
        self.assertEqual(cases.observe_audio_scalar_names(unityrs),
                         cases.observe_audio_scalar_names(UnityPy))


def extension_test(name):
    def test(self):
        expected = cases.observe_falsy_extensions(UnityPy)[name]
        self.assertTrue(all(('error' if name == 'Sprite' else 'value') in row[0] for row in expected))
        self.assertEqual(cases.observe_falsy_extensions(unityrs)[name], expected)
    return test


for name in ('TextAsset', 'Font', 'Mesh', 'Shader', 'MonoBehaviour', 'AudioClip',
             'Sprite', 'Texture2D', 'GameObject'):
    setattr(ReviewRound5, 'test_falsy_extension_' + name, extension_test(name))


def runtime_key_test(self):
    expected = cases.observe_runtime_keys(UnityPy)
    self.assertEqual(len(expected), 2)
    self.assertEqual(cases.observe_runtime_keys(unityrs), expected)


ReviewRound5.test_runtime_configuration_retains_bundle_key = runtime_key_test
