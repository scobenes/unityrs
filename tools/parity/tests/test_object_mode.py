"""PARITY.md section 9.2.1 holds exactly what `object_mode.py` prints in the oracle image, and the rule section 9.2
states holds for every class.

Needs UnityPy 1.25.2 with UnityPyBoost (the oracle image); skipped elsewhere.
"""

import inspect
import sys
import unittest
from abc import ABC
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import inventory  # noqa: E402
import object_mode  # noqa: E402

try:
    import attrs
    import UnityPy
    from UnityPy import classes
    from UnityPy.helpers import TypeTreeHelper

    ORACLE = UnityPy.__version__ == inventory.PINNED_VERSION and TypeTreeHelper.read_typetree_boost is not None
except ImportError:  # pragma: no cover - outside the oracle image
    ORACLE = False

UNKNOWN = "`UnknownObject`"


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 with UnityPyBoost (the oracle image)")
class ObjectMode(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = inventory.DEFAULT_PARITY.read_text(encoding="utf-8")

    def test_table_is_what_parse_class_does(self):
        """Section 9.2.1: every name `UnityPy.classes` holds, and what each path builds for it."""
        printed = object_mode.table().splitlines()
        lines = self.text.splitlines()
        self.assertTrue(printed[0] in lines, "PARITY.md has no section 9.2.1 table")
        start = lines.index(printed[0])
        held = lines[start : start + len(printed)]
        for number, (want, have) in enumerate(zip(printed, held, strict=True)):
            self.assertEqual(have[:200], want[:200], f"line {number} of the table")
            self.assertEqual(have, want, f"line {number} of the table")
        headings = [line for line in lines[:start] if line.startswith("### ")]
        self.assertEqual(headings[-1], "### 9.2.1 By class")

    def test_an_extra_field_follows_slots_and_dict_for_every_class(self):
        """Section 9.2: UnityPyBoost gives an `UnknownObject` for an extra field exactly when `__slots__`, read
        through the MRO, is a non-empty tuple, or the instances have no `__dict__`; never by the first base."""
        rows = object_mode.class_rows()
        self.assertEqual(sum(len(members) for members in rows.values()), len(object_mode.names()[0]))
        for (slots, has_dict, boost, _), members in rows.items():
            with self.subTest(slots=slots, has_dict=has_dict, first=members[0]):
                self.assertEqual(boost == UNKNOWN, slots == "a non-empty tuple" or has_dict == "no")
        by_name = {name: key for key, members in rows.items() for name in members}
        # Field-less classes attrs builds with slots=True on Object or ABC have `()`, and keep the extra.
        for name in ("GameManager", "EmptyObject", "Derived", "MemorySettings"):
            self.assertEqual(getattr(classes, name).__slots__, ())
            self.assertEqual(attrs.fields(getattr(classes, name)), ())
            self.assertEqual(by_name[name][2], "the class, `m_Extra` an attribute", name)
        self.assertEqual(by_name["AssetDatabaseMetrics"][2:], (UNKNOWN, "raises `AttributeError`"))
        self.assertEqual(by_name["MarshallingTestObject"][2:], (UNKNOWN, "the class, `m_Extra` an attribute"))

    def test_why_a_generated_class_has_its_slots(self):
        """Section 9.2's account of the generated classes: attrs gives a class built with `slots=True` its own
        fields as `__slots__`, and `__weakref__` too when its first base is `object` or `ABC`, which have no
        weak-reference slot (`Object` has one); one built with `slots=False` has no `__slots__` of its own and
        finds ABC's `()`."""
        generated = [
            clz
            for clz in vars(classes).values()
            if inspect.isclass(clz) and clz.__module__ == "UnityPy.classes.generated"
        ]
        self.assertEqual(len(generated), 869)
        for clz in generated:
            with self.subTest(clz.__name__):
                own = clz.__dict__.get("__slots__")
                fields = tuple(field.name for field in attrs.fields(clz))
                if own is None:
                    self.assertEqual(clz.__slots__, ())
                    continue
                first = clz.__bases__[0]
                self.assertIn(first, (object, ABC, classes.Object))
                weakref = ("__weakref__",) if first in (object, ABC) else ()
                self.assertEqual(first.__weakrefoffset__ == 0, first in (object, ABC))
                self.assertEqual(sorted(own), sorted(fields + weakref))


if __name__ == "__main__":
    unittest.main()
