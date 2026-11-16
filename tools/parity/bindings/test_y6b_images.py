"""Installed-wheel parity for texture hooks and standalone sprite helpers."""
import importlib
import unittest
import UnityPy
import unityrs
import binding_y6b_images as observations


class ImageExtensions(unittest.TestCase):
    def test_dispatch_preprocess_matches_oracle(self):
        actual = observations.observe_texture_dispatch(unityrs)
        self.assertEqual(actual, observations.observe_texture_dispatch(UnityPy))
        self.assertGreaterEqual(len(actual), 9)
        self.assertEqual(actual['bad-crunch'][1], [])
        self.assertIn('error', actual['bad-crunch'][0])

    def test_astc_context_identity_cache_and_native_codecs(self):
        actual = observations.observe_astc_context(unityrs)
        self.assertEqual(actual, observations.observe_astc_context(UnityPy))
        self.assertTrue(actual['identity'][0])
        self.assertFalse(actual['identity'][1])
        self.assertEqual(actual['decoded'][0], True)

    def test_standalone_geometry_matches_oracle(self):
        actual = observations.observe_sprite_geometry(unityrs)
        self.assertEqual(actual, observations.observe_sprite_geometry(UnityPy))
        self.assertIn('value', actual['RGBA-square-False'])
        self.assertIn('value', actual['RGB-triangle-True'])
        self.assertEqual(actual['RGBA-no-uv-False']['message'], 'No UV coordinates found in sprite mesh!')

    def test_matrix_arithmetic_and_errors_match_oracle(self):
        actual = observations.observe_sprite_matrices(unityrs)
        self.assertEqual(actual, observations.observe_sprite_matrices(UnityPy))
        self.assertEqual(actual['singular']['inverse']['error'], 'ZeroDivisionError')
        self.assertEqual(actual['one']['determinant'], {'value': 0})

    def test_astc_context_options_have_explicit_refusals(self):
        import astc_encoder as A
        converter = importlib.import_module('unityrs.export.Texture2DConverter')
        context = converter.get_astc_context((4, 4))
        source = A.ASTCImage(A.ASTCType.F32, 4, 4, 1, bytes(256))
        with self.assertRaisesRegex(NotImplementedError, '2D U8'):
            context.compress(source, A.ASTCSwizzle.from_str('RGBA'))
        source = A.ASTCImage(A.ASTCType.U8, 4, 4, 1, bytes(64))
        with self.assertRaisesRegex(NotImplementedError, 'RGBA swizzle'):
            context.compress(source, A.ASTCSwizzle.from_str('BGRA'))

    def test_astc_context_bounds_keep_oracle_support_recorded(self):
        actual = observations.observe_astc_options(unityrs)
        self.assertEqual(actual, observations.observe_astc_options(UnityPy, corrected=True))
        oracle = observations.observe_astc_options(UnityPy)
        self.assertEqual(set(oracle), {'float', 'swizzle', 'native-type'})
        for value in oracle.values():
            self.assertIn('value', value)

    def test_sprite_bounds_preserve_singular_and_luminance_oracle(self):
        actual = observations.observe_sprite_bounds(unityrs)
        self.assertEqual(actual, observations.observe_sprite_bounds(UnityPy, corrected=True))
        oracle = observations.observe_sprite_bounds(UnityPy)
        self.assertEqual(oracle['singular']['error'], 'LinAlgError')
        self.assertIn('value', oracle['luminance'])


if __name__ == '__main__':
    unittest.main()
