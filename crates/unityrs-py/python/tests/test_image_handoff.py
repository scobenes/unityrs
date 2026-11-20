"""The decoded pixels Pillow is handed, and the charges around them."""
import gc
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from PIL import Image

import unityrs
from unityrs.classes import _images
from binding_y4_images import texture
from binding_y4_save import _asset

SIDE = 64
PIXELS = SIDE * SIDE * 4


def _owner(limit=4_000_000):
    budget = unityrs.Budget(limit)
    env = unityrs.Environment(budget=budget)
    return budget, env.load_file(_asset(), name="owner.assets").objects[1]


class PixelHandoff(unittest.TestCase):
    def test_pixels_outlive_views_pillow_keeps(self):
        """Pillow gets a `bytes` it may keep views of, a slice or a cast, and
        read after the call: the pixels are the object's own, never memory
        the decoder frees once the call returns."""
        data = bytes(range(256)) * (PIXELS // 256)
        kept = []
        original = Image.frombytes

        def frombytes(mode, size, pixels, *args):
            self.assertIs(type(pixels), bytes)
            view = memoryview(pixels)
            kept.append((view[1:], view.cast("c"), bytes(pixels)))
            return original(mode, size, pixels, *args)

        with patch.object(Image, "frombytes", frombytes):
            image = texture(unityrs, data=data, m_Width=SIDE, m_Height=SIDE).image
        gc.collect()
        # Churn the allocator, so freed pixels would be overwritten.
        churn = [bytearray(PIXELS) for _ in range(8)]
        (tail, cast, copy), = kept
        self.assertEqual(bytes(tail), copy[1:])
        self.assertEqual(cast.tobytes(), copy)
        self.assertEqual(image.tobytes(), copy)
        del churn


class DecodeCharges(unittest.TestCase):
    def free_needed(self, call):
        """The least free budget under which `call(reader)` is not refused."""
        def fits(free):
            budget, reader = _owner()
            held = reader._native.tree_traversal()
            held.step(0, budget.limit - budget.used - free, 0)
            try:
                call(reader)
            except MemoryError:
                return False
            return True
        lo, hi = 0, 1_000_000
        self.assertTrue(fits(hi))
        while lo < hi:
            mid = (lo + hi) // 2
            if fits(mid):
                hi = mid
            else:
                lo = mid + 1
        return lo

    def test_input_and_image_stay_charged_through_the_copy(self):
        """The input's snapshot and the decoded image are both still charged
        when the copy for Python is: a tight budget the three do not fit
        refuses the decode."""
        def decode(reader):
            unityrs._native._image_decode(
                bytearray(PIXELS), SIDE, SIDE, 4, (0, 0), 0, None, reader._native)
        self.assertGreaterEqual(self.free_needed(decode), PIXELS + 2 * PIXELS + 1024)

    def test_nothing_of_the_decode_is_charged_at_the_pixels_step(self):
        """By the step for the pixels, the decode's charges are released."""
        budget, reader = _owner()
        steps = []
        made = _images._guard

        class Guard:
            def __init__(self, inner):
                self.inner = inner

            def step(self, depth, memory, work):
                before = budget.used
                out = self.inner.step(depth, memory, work)
                steps.append((memory, before, budget.used))
                return out

            def __getattr__(self, name):
                return getattr(self.inner, name)

        obj = SimpleNamespace(object_reader=reader)
        with patch.object(_images, "_guard", lambda o: Guard(made(o))):
            _images._decode(obj, bytes(PIXELS), SIDE, SIDE, 4, 0, None, (0, 0))
        at = [i for i, (memory, _, _) in enumerate(steps) if memory == PIXELS + 1024]
        self.assertEqual(len(at), 1)
        self.assertGreater(at[0], 0)
        # Between the input's snapshot step and the pixels' step, the decode
        # charged its input, image and copy, and released all three.
        self.assertEqual(steps[at[0]][1], steps[at[0] - 1][2])

if __name__ == "__main__":
    unittest.main()
