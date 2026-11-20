"""Object mode's direct read gives the full path's objects, work and memory."""
import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
from binding_cases import S
import attrs
import unityrs
import unityrs.classes

SKIP = {'object_reader', 'assetsfile', 'assets_file', '__node__'}


def shape(v):
    """A value's types and contents, an object's attributes in order."""
    t = type(v)
    if t is dict:
        return ('dict', [(k, shape(x)) for k, x in v.items()])
    if t in (list, tuple):
        return (t.__name__, [shape(x) for x in v])
    if t in (int, float, str, bytes, bool, type(None)):
        return (t.__name__, v)
    if hasattr(v, '__dict__'):
        items = list(vars(v).items())
    else:
        items = [(a.name, getattr(v, a.name)) for a in attrs.fields(t)]
    return (t.__module__ + '.' + t.__name__,
            [(k, type(x).__name__ if k in SKIP else shape(x)) for k, x in items])


def node(ty, name, kids=()):
    return S.Node(ty, name, list(kids))


def floats(ty, name, names):
    return node(ty, name, [node('float', n) for n in names])


def pptr(target, name):
    return node(f'PPtr<{target}>', name, [node('int', 'm_FileID'), node('SInt64', 'm_PathID')])


def vector(name, elem):
    return node('vector', name, [node('Array', 'Array', [node('int', 'size'), elem])])


def string(name):
    return node('string', name, [node('Array', 'Array', [node('int', 'size'),
                                                         node('char', 'data')])])


def text(value):
    raw = value.encode()
    return struct.pack('<i', len(raw)) + raw + bytes(-len(raw) % 4)


def ptr(file_id, path_id):
    return struct.pack('<iq', file_id, path_id)


def fixture():
    transform = node('Transform', 'Base', [
        pptr('GameObject', 'm_GameObject'),
        floats('Quaternionf', 'm_LocalRotation', 'xyzw'),
        floats('Vector3f', 'm_LocalPosition', 'xyz'),
        floats('Vector3f', 'm_LocalScale', 'xyz'),
        vector('m_Children', pptr('Transform', 'data')),
        pptr('Transform', 'm_Father')])
    asset = node('TextAsset', 'Base', [string('m_Name'), string('m_Script')])
    extra = node('TextAsset', 'Base', [string('m_Name'), string('m_Script'),
                                       node('int', '1st extra')])
    unknown = node('Y7DirectThing', 'Base', [floats('Vector3f', 'm_Point', 'xyz'),
                                             vector('items', node('int', 'data'))])
    types = [S.Type(4, transform), S.Type(49, asset), S.Type(50, extra), S.Type(51, unknown)]
    transform_data = (ptr(0, 7) + struct.pack('<4f', 0, 0, 0, 1) + struct.pack('<3f', 1.5, -2, 3)
                      + struct.pack('<3f', 1, 1, 1) + struct.pack('<i', 2) + ptr(0, 8) + ptr(1, 9)
                      + ptr(0, 0))
    objs = [S.Obj(1, 0, transform_data),
            S.Obj(2, 1, text('name') + text('caf\u00e9 \U0001f600')),
            S.Obj(3, 2, text('n') + text('') + struct.pack('<i', 9)),
            S.Obj(4, 3, struct.pack('<3f', 1.5, -2.0, 3.25) + struct.pack('<3i', 2, 5, 6))]
    return S.serialized_file(types, objs)


def read(data, index, full, limits=None, budget=None):
    env = unityrs.Environment(limits=limits, budget=budget)
    env.load_file(data, name='f')
    obj = sorted(env.objects, key=lambda o: o.path_id)[index]
    if full:
        obj._native = FullPath(obj._native)
    return obj.read()


class FullPath:
    """The object's native half, its direct read declined."""

    def __init__(self, native):
        self._inner = native

    def __getattr__(self, name):
        return getattr(self._inner, name)

    def parse_direct(self, *args):
        return None


def least(ok):
    lo, hi = 0, 1 << 32
    while lo < hi:
        mid = (lo + hi) // 2
        if ok(mid):
            hi = mid
        else:
            lo = mid + 1
    return lo


def succeeds(fn):
    try:
        fn()
    except Exception:
        return False
    return True


class DirectObject(unittest.TestCase):
    def test_each_object_is_the_full_paths(self):
        data = fixture()
        for index in range(4):
            with self.subTest(index=index):
                direct = read(data, index, False)
                full = read(data, index, True)
                self.assertEqual(shape(direct), shape(full))
                self.assertIsNotNone(direct.object_reader)

    def test_the_kinds_built(self):
        data = fixture()
        names = [type(read(data, i, False)).__name__ for i in range(4)]
        self.assertEqual(names, ['Transform', 'TextAsset', 'TextAsset', 'UnknownObject'])
        transform = read(data, 0, False)
        child = transform.m_Children[1]
        self.assertEqual((type(child).__name__, child.m_FileID, child.m_PathID), ('PPtr', 1, 9))
        self.assertIsNotNone(child.assetsfile)
        self.assertEqual(transform.m_LocalPosition.y, -2.0)
        self.assertEqual(read(data, 2, False).x1st_extra, 9)

    def test_only_the_first_constructions_read_directly(self):
        data = fixture()
        env = unityrs.Environment()
        env.load_file(data, name='f')
        objs = sorted(env.objects, key=lambda o: o.path_id)
        taken = [o._native.parse_direct(o, True, True) is not None for o in objs]
        self.assertEqual(taken, [True, True, False, False])

    def test_work_and_memory_bounds_are_the_full_paths(self):
        data = fixture()
        for index in range(3):
            with self.subTest(index=index):
                work = [least(lambda m: succeeds(lambda: read(
                    data, index, full, limits=unityrs.Limits(max_decode_work=m))))
                    for full in (False, True)]
                self.assertEqual(work[0], work[1])
                memory = [least(lambda m: succeeds(lambda: read(
                    data, index, full, budget=unityrs.Budget(m))))
                    for full in (False, True)]
                self.assertEqual(memory[0], memory[1])

    def test_a_tree_refused_raises_the_full_paths_error(self):
        data = fixture()
        for limits in (unityrs.Limits(max_tree_depth=2), unityrs.Limits(max_decode_work=1)):
            errors = []
            for full in (False, True):
                env = unityrs.Environment(limits=limits)
                try:
                    env.load_file(data, name='f')
                    obj = sorted(env.objects, key=lambda o: o.path_id)[0]
                    if full:
                        obj._native = FullPath(obj._native)
                    obj.read()
                except Exception as error:
                    errors.append((type(error), str(error), obj.reader.Position))
            self.assertEqual(len(errors), 2)
            self.assertEqual(errors[0], errors[1])

    def test_a_raising_constructor_gives_the_full_paths_object(self):
        data = fixture()
        classes = unityrs.classes
        original = classes.Vector3f

        class Refuses(original):
            def __init__(self, *args, **kwargs):
                raise ValueError('refused')
        classes.Vector3f = Refuses
        try:
            direct = read(data, 0, False)
            full = read(data, 0, True)
        finally:
            classes.Vector3f = original
        self.assertEqual(type(direct.m_LocalPosition).__name__, 'UnknownObject')
        self.assertEqual(shape(direct), shape(full))

    def test_each_constructor_runs_once_after_the_read(self):
        data = fixture()
        classes = unityrs.classes
        vector, transform = classes.Vector3f, classes.Transform
        code = vector.__init__.__code__
        calls, made = [], []

        def profile(frame, event, arg):
            if event == 'call' and frame.f_code is code:
                calls.append(frame)

        class Stateful(transform):
            def __init__(self, **kwargs):
                made.append(len(calls))
                super().__init__(**kwargs)
        # Read directly, then declined at the root once its fields' dicts
        # are made: the full path calls each class, once.
        for root, want in ((transform, []), (Stateful, [2])):
            with self.subTest(root=root.__name__):
                calls.clear()
                made.clear()
                classes.Transform = root
                sys.setprofile(profile)
                try:
                    obj = read(data, 0, False)
                finally:
                    sys.setprofile(None)
                    classes.Transform = transform
                self.assertEqual(len(calls), 2)
                self.assertEqual(made, want)
                self.assertIs(type(obj), root)
                self.assertEqual(obj.m_LocalPosition.y, -2.0)


if __name__ == '__main__':
    unittest.main()
