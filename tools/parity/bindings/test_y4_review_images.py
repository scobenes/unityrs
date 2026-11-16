"""Unbounded mip counts and Python validation before texture mutation."""

import unittest

import UnityPy
import unityrs

from binding_y4_review_images import observe_mipmap_arguments


class MipmapArguments(unittest.TestCase):
    def test_unbounded_and_float_mipmap_counts_match_oracle(self):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        self.assertEqual(observe_mipmap_arguments(unityrs), observe_mipmap_arguments(UnityPy))

    def test_unbounded_switch_mip_refusal_retains_fields(self):
        from PIL import Image
        from types import SimpleNamespace
        from binding_y4_images import texture

        obj = texture(unityrs, m_PlatformBlob=bytes(8) + b"\x01\0\0\0",
                      object_reader=SimpleNamespace(platform=38))
        before = vars(obj).copy()
        with self.assertRaisesRegex(NotImplementedError, "Switch mip chain"):
            obj.set_image(Image.new("RGBA", (8, 8)), 4, 2**128)
        self.assertEqual(vars(obj), before)
