"""Reader and damaged-container residue against UnityPy 1.25.2."""
import unittest

import UnityPy
import unityrs

import binding_y6_objects as cases


class ObjectResidue(unittest.TestCase):
    def setUp(self):
        self.assertEqual(UnityPy.__version__, '1.25.2')

    def test_reference_arrays_preserve_typed_and_empty_entries(self):
        self.assertEqual(cases.observe_references(unityrs),
                         cases.observe_references(UnityPy))

    def test_reference_arrays_record_original_python_none_failure(self):
        result = cases.observe_references(UnityPy)
        for first in (False, True):
            for mode in ('parse_as_dict', 'parse_as_object'):
                self.assertEqual(result[f'python:True:{first}'][mode], {
                    'error': 'AttributeError',
                    'message': "'NoneType' object has no attribute 'm_MetaFlag'"})

    def test_nonclass_names_use_missing_class_reference(self):
        self.assertEqual(cases.observe_nonclass(unityrs),
                         cases.observe_nonclass(UnityPy, corrected=True))

    def test_original_nonclass_lookups_are_isolated_and_characterized(self):
        result = cases.observe_nonclass(UnityPy)
        self.assertEqual(result['__class__:1'], {'signal': 'SIGSEGV'})
        self.assertEqual(result['List:0']['error'], 'ValueError')
        self.assertEqual(result['attrs_define:0']['value'], ['function'])
        self.assertEqual(result['__repr__:0']['value'], ['str'])
        self.assertEqual(result['__str__:0']['value'], ['str'])
        self.assertEqual(result['__sizeof__:0']['value'], ['int'])
        self.assertEqual(result['__init_subclass__:0']['value'], ['NoneType'])
        self.assertEqual(result['__subclasshook__:0']['value'], ['NotImplementedType'])
        self.assertEqual(result['Optional:1']['value'],
                         ['UnknownObject', 'Optional', {'f0': 1}])

    def test_synthetic_transform_reader_and_pointer_identity(self):
        self.assertEqual(cases.observe_transform(unityrs), cases.observe_transform(UnityPy))

    def test_damaged_preload_spans_and_cross_file_paths(self):
        self.assertEqual(cases.observe_preload(unityrs), cases.observe_preload(UnityPy))


if __name__ == '__main__':
    unittest.main()
