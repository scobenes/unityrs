"""The global name-peek cache preserves identity within bounded retention."""
import gc
import importlib
import unittest
import weakref
from unittest.mock import patch

import unityrs
from test_helpers import S
from unityrs.helpers.TypeTreeNode import TypeTreeNode

helper = importlib.import_module("unityrs.helpers.TypeTreeNode")


def named_root(name, prefix=()):
    return TypeTreeNode(0, "Root", name, -1, 1,
                        [*prefix, TypeTreeNode(1, "string", "m_Name", -1, 1)])


class NamePeekCache(unittest.TestCase):
    def setUp(self):
        self.enterContext(patch.object(helper, "NAME_PEEK_NODE_CACHE", {}))
        self.enterContext(patch.object(helper, "_NAME_PEEK_RETAINED_NODES", 0, create=True))

    def test_cached_peek_does_not_retain_native_file(self):
        tree = S.Node("TextAsset", "cache-lifetime", [S.Node("string", "m_Name")])
        data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, S.s32("value"))])
        budget = unityrs.Budget(1000000)
        environment = unityrs.Environment(data, budget=budget)
        obj = environment.objects[0]
        file_ref = weakref.ref(obj.assets_file._native)
        root = obj._get_typetree_node()
        peek = root.get_name_peek_node()
        self.assertGreater(budget.used, 0)
        del root, obj, environment
        gc.collect()
        self.assertIsNone(file_ref())
        self.assertEqual(budget.used, 0)
        self.assertIs(next(iter(helper.NAME_PEEK_NODE_CACHE.values())), peek)
        self.assertIsNone(peek[0]._tree_source())
        self.assertIsNone(peek[0].m_Children[0]._tree_source())
        self.assertEqual([node.m_Name for node in peek[0].traverse()],
                         ["cache-lifetime", "m_Name"])

    def test_dead_source_falls_back_to_default_limits(self):
        tree = S.Node("TextAsset", "cache-limits", [S.Node("string", "m_Name")])
        data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, S.s32("value"))])
        environment = unityrs.Environment(data, limits=unityrs.Limits(max_tree_depth=3))
        obj = environment.objects[0]
        root = obj._get_typetree_node()
        child = root.m_Children[0]
        for level in range(2, 6):
            descendant = TypeTreeNode(level, "int", "value", 4, 1)
            child.m_Children.append(descendant)
            child = descendant
        peek = root.get_name_peek_node()
        with self.assertRaises(MemoryError):
            list(peek[0].traverse())
        del root, obj, environment
        gc.collect()
        self.assertEqual(len(list(peek[0].traverse())), 6)

    def test_cached_tuple_identity_and_first_seen_win(self):
        first = named_root("same-key")
        second = named_root("same-key", [TypeTreeNode(1, "int", "value", 4, 1)])
        peek = first.get_name_peek_node()
        self.assertIs(first.get_name_peek_node(), peek)
        self.assertIs(second.get_name_peek_node(), peek)
        self.assertEqual(len(peek[0].m_Children), 1)

    def test_entry_cap_returns_equal_distinct_results(self):
        for index in range(4096):
            named_root(str(index)).get_name_peek_node()
        self.assertEqual(len(helper.NAME_PEEK_NODE_CACHE), 4096)
        root = named_root("overflow")
        first, second = root.get_name_peek_node(), root.get_name_peek_node()
        self.assertEqual(first, second)
        self.assertIsNot(first, second)
        self.assertIs(named_root("0").get_name_peek_node(),
                      helper.NAME_PEEK_NODE_CACHE[("0", "Root", 1)])
        self.assertEqual(len(helper.NAME_PEEK_NODE_CACHE), 4096)

    def test_total_node_cap_counts_prefix_subtrees(self):
        # Each prefix retains its new root, the preceding subtree, and m_Name.
        leaf = TypeTreeNode(2, "int", "leaf", 4, 1)
        branch = TypeTreeNode(1, "Branch", "branch", -1, 1, [leaf, leaf])
        with patch.object(helper, "_NAME_PEEK_MAX_NODES", 8, create=True):
            first = named_root("five", [branch]).get_name_peek_node()
            second_root = named_root("three")
            second_root.m_Children[0].m_Children.append(leaf)
            second = second_root.get_name_peek_node()
            self.assertIs(named_root("five").get_name_peek_node(), first)
            self.assertIs(second_root.get_name_peek_node(), second)
            overflow = named_root("overflow")
            a, b = overflow.get_name_peek_node(), overflow.get_name_peek_node()
            self.assertEqual(a, b)
            self.assertIsNot(a, b)
            self.assertEqual(len(helper.NAME_PEEK_NODE_CACHE), 2)

    def test_oversized_subtree_count_stops_at_node_cap(self):
        visits = 0
        leaf = TypeTreeNode(2, "int", "leaf", 4, 1)

        class Children:
            def __iter__(self):
                nonlocal visits
                while True:
                    visits += 1
                    if visits > 1048576:
                        raise AssertionError("count continued past the node cap")
                    yield leaf

        branch = TypeTreeNode(1, "Branch", "branch", -1, 1, Children())
        root = named_root("oversized", [branch])
        result = root.get_name_peek_node()
        self.assertEqual(result[1], "m_Name")
        self.assertGreater(visits, 0)
        self.assertLessEqual(visits, 1048576)
        self.assertEqual(helper.NAME_PEEK_NODE_CACHE, {})

    def test_deep_and_cyclic_subtree_counts_are_iterative_and_bounded(self):
        branch = TypeTreeNode(1, "Branch", "branch", -1, 1)
        for _ in range(2000):
            branch = TypeTreeNode(1, "Branch", "branch", -1, 1, [branch])
        root = named_root("deep", [branch])
        result = root.get_name_peek_node()
        self.assertIs(root.get_name_peek_node(), result)
        cycle = TypeTreeNode(1, "Branch", "cycle", -1, 1)
        cycle.m_Children.append(cycle)
        with patch.object(helper, "_NAME_PEEK_MAX_NODES", 4096, create=True):
            root = named_root("cyclic", [cycle])
            self.assertIsNot(root.get_name_peek_node(), root.get_name_peek_node())
        self.assertEqual(len(helper.NAME_PEEK_NODE_CACHE), 1)
