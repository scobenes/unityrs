"""Y1 binding parity. The wheel and UnityPy 1.25.2 are required, never skipped.

Run explicitly in the wheel job. Missing dependencies fail the suite.
"""

import unittest


class BindingRows(unittest.TestCase):
    def test_y1_synthetic_rows(self):
        import UnityPy
        import unityrs
        import binding_cases as B

        self.assertEqual(UnityPy.__version__, "1.25.2")
        for name, data in B.cases():
            with self.subTest(name=name):
                self.assertEqual(B.observe(unityrs, data), B.observe(UnityPy, data, corrected=True))

    def test_y1_paths_and_streams(self):
        import UnityPy
        import unityrs
        import binding_cases as B
        import scratch
        data = next(data for name, data in B.cases() if name == "text")
        with scratch.folder("unityrs-bindings-") as root:
            self.assertEqual(B.observe_paths(unityrs, data, root), B.observe_paths(UnityPy, data, root, corrected=True))
