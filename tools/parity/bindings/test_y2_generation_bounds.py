"""Synthetic inputs exercise expansion limits before generated code executes."""
from pathlib import Path
import struct
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import child


def empty_chain_tpk(count):
    strings = ['Object', ''] + ['Record' + str(i) for i in range(count)]
    records = [struct.pack('<iiQBHHB', 0, 1, 0, 1, 0, 1, 0)]
    for i in range(count):
        records.append(struct.pack('<iiQBHHB', i + 1, 1, 0, 1,
                                   i + 2, 0 if i == 0 else i + 1, 0))
    payload = struct.pack('<qii', 0, 0, len(records)) + b''.join(records)
    payload += struct.pack('<iiii', 0, 0, 0, len(strings))
    payload += b''.join(bytes([len(s)]) + s.encode() for s in strings)
    return struct.pack('<IbbbbIII', 0x2A4B5054, 1, 0, 0, 0, 0,
                       len(payload), len(payload)) + payload


class GenerationBounds(unittest.TestCase):
    def test_empty_inheritance_chain_refused_before_first_class(self):
        code = """
import sys, types, attrs, unityrs
unityrs.config = types.SimpleNamespace(TPK=bytes.fromhex(sys.stdin.read()))
original = attrs.define
def define(cls=None, **kwargs):
    if cls is not None and cls.__module__ == 'unityrs.classes.generated':
        raise AssertionError('generated a class before inheritance preflight')
    return original(cls, **kwargs)
attrs.define = define
try:
    import unityrs.classes
except ImportError as error:
    assert type(error).__name__ == 'ExpansionError', repr(error)
    assert 'inheritance' in str(error), str(error)
else:
    raise AssertionError('unbounded inheritance accepted')
"""
        data = empty_chain_tpk(2000)
        self.assertLess(len(data), 80_000)
        result = child.run([sys.executable, '-c', code], input=data.hex(),
                           capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_class_execution_cost_is_preflighted(self):
        from unityrs.classes.generated import _checked
        from unityrs.classes._generator import GENERATED_HEADER
        from unityrs.classes import _generation_limits as limits
        cases = (
            ('MAX_INHERITANCE_DEPTH', 4,
             ''.join('@unitypy_define\nclass R%d(%s):\n    pass\n' %
                     (i, 'Object' if i == 0 else 'R%d' % (i - 1)) for i in range(5))),
            ('MAX_MRO_TOTAL', 12,
             ''.join('@unitypy_define\nclass R%d(Object):\n    pass\n' % i
                     for i in range(4))),
            ('MAX_INHERITED_FIELDS', 10,
             '@unitypy_define\nclass R(Object):\n    a: int\n    b: int\n    c: int\n'
             '@unitypy_define\nclass S(R):\n    d: int\n    e: int\n'
             '@unitypy_define\nclass T(S):\n    pass\n'),
            ('MAX_CLASSES', 2,
             ''.join('@unitypy_define\nclass R%d:\n    pass\n' % i for i in range(3))),
            ('MAX_FIELDS', 2,
             '@unitypy_define\nclass R:\n    a: int\n    b: int\n    c: int\n'),
            ('MAX_FIELDS_PER_CLASS', 2,
             '@unitypy_define\nclass R:\n    a: int\n    b: int\n    c: int\n'),
        )
        for bound, maximum, definitions in cases:
            with self.subTest(bound=bound), patch.object(limits, bound, maximum, create=True):
                with self.assertRaisesRegex(limits.ExpansionError, 'expansion'):
                    _checked(GENERATED_HEADER + '\n' + definitions)

    def test_small_inheritance_and_multiple_bases_are_accepted(self):
        from unityrs.classes.generated import _checked
        from unityrs.classes._generator import GENERATED_HEADER
        tree = _checked(GENERATED_HEADER + '''
@unitypy_define
class R(Object):
    a: int
@unitypy_define
class S(R, ABC):
    b: int
''')
        self.assertEqual([n.name for n in tree.body[-2:]], ['R', 'S'])

    def test_cached_versions_cannot_bypass_generator_work_limit(self):
        from types import SimpleNamespace as Record
        from unityrs.classes import _generator as generator, _generation_limits as limits
        data = empty_chain_tpk(1)
        from unityrs._class_tpk import load
        tpk = load(data)
        tpk.NodeBuffer = [Record(TypeName=0, Name=0, SubNodes=()),
                          Record(TypeName=2, Name=0, SubNodes=())]
        tpk.ClassInformation[0] = [(0, Record(Name=0, Base=1, ReleaseRootNode=0))]
        tpk.ClassInformation[1] = [(0, Record(Name=2, Base=0, ReleaseRootNode=1))] * 100
        with patch.multiple(generator, TPK=tpk, NODES=tpk.NodeBuffer,
                            STRINGS=tpk.StringBuffer, CLASS_CACHE_ID={},
                            CLASS_CACHE_NAME={}, TYPE_CACHE={}, BUDGET=limits.Budget()), \
                patch.object(limits, 'MAX_WORK', 20, create=True):
            with self.assertRaisesRegex(limits.ExpansionError, 'work expansion'):
                generator.generate_classes()

    def test_distinct_cached_types_are_charged(self):
        from types import SimpleNamespace as Record
        from unityrs.classes import _generator as generator, _generation_limits as limits
        nodes = [Record(TypeName=0, Name=0, SubNodes=()) for _ in range(10)]
        with patch.multiple(generator, NODES=nodes, STRINGS=['int'], TYPE_CACHE={},
                            BUDGET=limits.Budget()), \
                patch.object(limits, 'MAX_CACHE_RECORDS', 3, create=True):
            with self.assertRaisesRegex(limits.ExpansionError, 'cache expansion'):
                for node_id in range(len(nodes)):
                    generator.generate_field_type(node_id)

    def test_annotation_depth_is_bounded_before_generation(self):
        from types import SimpleNamespace as Record
        from unityrs.classes import _generator as generator, _generation_limits as limits
        nodes = [Record(TypeName=0, Name=0, SubNodes=(i + 1, i + 1)) for i in range(65)]
        nodes.append(Record(TypeName=1, Name=0, SubNodes=()))
        with patch.multiple(generator, NODES=nodes, STRINGS=['pair', 'int'],
                            TYPE_CACHE={}, BUDGET=limits.Budget()):
            with self.assertRaisesRegex(limits.ExpansionError, 'annotation depth expansion'):
                generator.generate_field_type(0)

    def test_annotation_total_is_charged_across_distinct_nodes(self):
        from types import SimpleNamespace as Record
        from unityrs.classes import _generator as generator, _generation_limits as limits
        nodes = [Record(TypeName=0, Name=0, SubNodes=(20, 20)) for _ in range(20)]
        nodes.append(Record(TypeName=1, Name=0, SubNodes=()))
        with patch.multiple(generator, NODES=nodes, STRINGS=['pair', 'int'],
                            TYPE_CACHE={}, BUDGET=limits.Budget()), \
                patch.object(limits, 'MAX_ANNOTATION_TOTAL', 1000):
            with self.assertRaisesRegex(limits.ExpansionError, 'annotation expansion'):
                for node_id in range(20):
                    generator.generate_field_type(node_id)

    def test_oversized_field_names_are_refused_during_traversal(self):
        from types import SimpleNamespace as Record
        from unityrs.classes import _generator as generator, _generation_limits as limits
        nodes = [Record(TypeName=0, Name=0, SubNodes=(1,)),
                 Record(TypeName=1, Name=2, SubNodes=())]
        with patch.multiple(generator, NODES=nodes, STRINGS=['Record', 'int', 'v' * 17000],
                            CLASS_CACHE_ID={}, CLASS_CACHE_NAME={}, TYPE_CACHE={},
                            BUDGET=limits.Budget()):
            with self.assertRaisesRegex(limits.ExpansionError, 'field name expansion'):
                generator.implement_node_class(0)

    def test_equal_field_names_cannot_amplify_cached_edge_work(self):
        from types import SimpleNamespace as Record
        from unityrs.classes import _generator as generator, _generation_limits as limits
        first = ('value' * 2000).encode().decode()
        second = first.encode().decode()
        self.assertIsNot(first, second)
        nodes = [Record(TypeName=0, Name=0, SubNodes=(20 + i % 2,)) for i in range(20)]
        nodes += [Record(TypeName=1, Name=2, SubNodes=()),
                  Record(TypeName=1, Name=3, SubNodes=())]
        with patch.multiple(generator, NODES=nodes, STRINGS=['Record', 'int', first, second],
                            CLASS_CACHE_ID={}, CLASS_CACHE_NAME={}, TYPE_CACHE={},
                            BUDGET=limits.Budget()), \
                patch.object(limits, 'MAX_STRING_WORK', 200000, create=True):
            with self.assertRaisesRegex(limits.ExpansionError, 'string work expansion'):
                for node_id in range(20):
                    generator.implement_node_class(node_id)

    def test_tpk_cumulative_records_and_allocations_are_bounded(self):
        from unityrs import _class_tpk as tpk
        data = empty_chain_tpk(8)
        # Each table fits alone; their class, version and string records do not.
        with patch.object(tpk, 'MAX_RECORDS', 15):
            with self.assertRaisesRegex(ImportError, 'record limit'):
                tpk.load(data)
        with patch.object(tpk, 'MAX_TABLE_BYTES', 256, create=True):
            with self.assertRaisesRegex(ImportError, 'allocation limit'):
                tpk.load(data)

    def test_real_tpk_has_headroom_for_all_expansion_limits(self):
        from unityrs.classes import _generator as generator, _generation_limits as limits
        from unityrs.classes.generated import _checked
        self.assertLess(generator.BUDGET.work, limits.MAX_WORK // 4)
        self.assertLess(generator.BUDGET.cache_records, limits.MAX_CACHE_RECORDS // 4)
        self.assertLess(generator.BUDGET.string_work, limits.MAX_STRING_WORK // 4)
        # Source validation also charges inheritance, including all bases.
        with patch.multiple(generator, CLASS_CACHE_ID={}, CLASS_CACHE_NAME={},
                            TYPE_CACHE={}, BUDGET=limits.Budget()):
            _checked(generator.generate_classes())

    def test_tpk_node_edges_share_the_record_budget(self):
        from unityrs import _class_tpk as tpk
        # One node with ten children fits each individual table's limit.
        payload = struct.pack('<qiiii', 0, 0, 0, 0, 0)
        payload += struct.pack('<iHHihbIH', 1, 0, 0, -1, 1, 0, 0, 10)
        payload += struct.pack('<10Hi', *([0] * 10), 1) + b'\x01x'
        data = struct.pack('<IbbbbIII', 0x2A4B5054, 1, 0, 0, 0, 0,
                           len(payload), len(payload)) + payload
        with patch.object(tpk, 'MAX_RECORDS', 10):
            with self.assertRaisesRegex(ImportError, 'record limit'):
                tpk.load(data)

    def test_lzma_output_cannot_exceed_the_declared_size(self):
        import lzma
        from unityrs._class_tpk import load
        data = empty_chain_tpk(2)
        raw = data[20:]
        settings = {'id': lzma.FILTER_LZMA1, 'dict_size': 1 << 20,
                    'lc': 3, 'lp': 0, 'pb': 2}
        compressed = struct.pack('<BI', 93, 1 << 20)
        compressed += lzma.compress(raw, format=lzma.FORMAT_RAW, filters=[settings])
        header = struct.pack('<IbbbbIII', 0x2A4B5054, 1, 2, 0, 0, 0,
                             len(compressed), len(raw) - 1)
        with self.assertRaisesRegex(ImportError, 'size differs'):
            load(header + compressed)


if __name__ == '__main__':
    unittest.main()
