"""Review round six standalone parsing and scalar exporter parity."""
import unittest

import UnityPy
import unityrs
import binding_y5_round6 as cases


class ReviewRound6(unittest.TestCase):
    def test_formatted_scalar_suffixes_remain_confined(self):
        cases.check_scalar_suffix_confinement(unityrs)

    def test_parse_file_preserves_environment_files_and_cabs(self):
        expected = cases.observe_parse_registration(UnityPy)
        for row in expected['serialized']:
            self.assertEqual(row['before'], row['after'])
            self.assertEqual(row['identities'], [True, True])
            self.assertEqual(row['later_identities'], [True, True])
            self.assertTrue(row['parent'])
            self.assertTrue(row['dependency'])
            self.assertEqual(row['value'], {'m_Name': 'name', 'm_Script': 'body'})
        self.assertEqual(expected['containers'], {
            'bundle': {'files': [], 'cabs': ['cab-y1', 'cab-y1.ress'], 'parent': True,
                       'resource': b'resource'.hex()},
            'nested': {'files': [], 'cabs': ['cab-y1'], 'parent': True},
            'duplicate_containers': {'files': [], 'cabs': ['cab-y1', 'cab-y1.ress', 'first.ress'],
                                     'parent': True, 'resource': b'first'.hex()},
            'raw_tree_limit': {'files': [], 'cabs': ['raw.resource'], 'parent': True},
        })
        self.assertEqual(cases.observe_parse_registration(unityrs), expected)

    def test_export_obj_converts_destination_after_dispatch_and_filter(self):
        expected = cases.observe_skipped_destination_conversion(UnityPy)
        for label in ('unsupported', 'filtered'):
            for outcome, calls in expected[label]:
                self.assertEqual(outcome, {'value': ['list', []]})
                self.assertEqual(calls, [] if label == 'unsupported' else ['named.old'])
        self.assertTrue(all(row[0]['error'] == 'TypeError' for row in expected['accepted']))
        self.assertEqual(cases.observe_skipped_destination_conversion(unityrs), expected)


def extension_test(name):
    def test(self):
        expected = cases.observe_scalar_extensions(UnityPy)[name]
        self.assertEqual(len(expected), 12)
        if name in ('Font', 'MonoBehaviour', 'AudioClip', 'GameObject'):
            self.assertTrue(all('value' in row[0] for row in expected))
        self.assertEqual(cases.observe_scalar_extensions(unityrs)[name], expected)
    return test


for name in ('TextAsset', 'Font', 'Mesh', 'Shader', 'MonoBehaviour', 'AudioClip',
             'Sprite', 'Texture2D', 'GameObject'):
    setattr(ReviewRound6, 'test_scalar_extension_' + name, extension_test(name))
