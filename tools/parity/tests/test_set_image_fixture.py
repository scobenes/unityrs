"""The write fixture must observe the oracle's object, including saved fields."""
import copy
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))
import child


class SetImageFixture(unittest.TestCase):
    def test_object_expectations_follow_each_mutated_oracle_field(self):
        # LoadedModules forbids fixture modules in the parity suite's process.
        proc = child.run([sys.executable, str(HERE), "--run"], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)

    def check_fields(self):
        sys.path.insert(0, str(HERE.parents[2] / "fixtures"))
        import set_image as fixture

        baseline = fixture.record(fixture.TF.RGBA32, (1, 1))
        setter = fixture.set_image
        mutations = {
            "m_Width": 2, "m_Height": 3, "m_TextureFormat": 3,
            "m_CompleteImageSize": 73, "m_MipMap": True, "m_MipCount": 5,
            "m_PlatformBlob": [4, 5, 6], "image_data": b"changed",
            "m_StreamData.path": "changed.resS", "m_StreamData.offset": 789,
            "m_StreamData.size": 123,
        }
        for name, value in mutations.items():
            def mutate(texture, *args):
                setter(texture, *args)
                if name.startswith("m_StreamData."):
                    setattr(texture.m_StreamData, name.split(".")[1], value)
                else:
                    setattr(texture, name, value)

            with self.subTest(field=name), patch.object(fixture, "set_image", mutate):
                changed = fixture.record(fixture.TF.RGBA32, (1, 1))
                self.assertEqual(changed["initial"], baseline["initial"])
                expected = copy.deepcopy(baseline["object"]["fields"])
                if name.startswith("m_StreamData."):
                    expected["m_StreamData"][name.split(".")[1]] = value
                elif name == "image_data":
                    expected["image data"] = fixture.digest(value)
                elif name == "m_PlatformBlob":
                    expected[name] = fixture.digest(bytes(value))
                else:
                    expected[name] = value
                self.assertEqual(changed["object"]["fields"], expected)
                for endian in ("<", ">"):
                    self.assertNotEqual(changed["object"][endian], baseline["object"][endian])


if __name__ == "__main__":
    if sys.argv[1:] == ["--run"]:
        SetImageFixture().check_fields()
    else:
        unittest.main()
