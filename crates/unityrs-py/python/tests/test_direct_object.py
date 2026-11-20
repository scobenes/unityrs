"""Object mode's direct read gives the full path's objects, work and memory."""
import contextlib
import functools
import gc
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


@contextlib.contextmanager
def pairs_class():
    """A file of one `Y7Pairs`, whose `m_Pair` holds two `Vector3f` (and
    `m_Items` a list, so its dict is one a collection tracks), with a plain
    class for it in `unityrs.classes` while in use."""
    import attrs
    classes = unityrs.classes

    @attrs.define(slots=False)
    class Y7Pairs(classes.Object):
        m_Pair: object = None
        m_Items: object = None

    tree = node('Y7Pairs', 'Base', [node('pair', 'm_Pair', [
        floats('Vector3f', 'first', 'xyz'), floats('Vector3f', 'second', 'xyz')]),
        vector('m_Items', node('int', 'data'))])
    classes.Y7Pairs = Y7Pairs
    try:
        yield S.serialized_file([S.Type(114, tree)],
                                [S.Obj(1, 0, struct.pack('<6f2i', 1, 2, 3, 4, 5, 6, 1, 7))])
    finally:
        del classes.Y7Pairs


def large_fixture(count=10000):
    """A small object, then one of more values than a direct read takes."""
    small = node('TextAsset', 'Base', [string('m_Name'), string('m_Script')])
    large = node('Y7Large', 'Base', [vector('items', node('int', 'data'))])
    types = [S.Type(49, small), S.Type(52, large)]
    objs = [S.Obj(1, 0, text('a') + text('b')),
            S.Obj(2, 1, struct.pack(f'<i{count}i', count, *range(count)))]
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

    def test_a_replaced_reader_property_runs_as_the_full_paths(self):
        """A replaced `reader` property (on either class, after an object's
        first read, or dressed as the original) runs exactly as often as on
        the full path, with the full path's value: a replaced
        `ObjectReader.reader` is never called by the direct read itself."""
        from unityrs.files import ObjectReader, SerializedFile

        def fresh(full, read_before):
            env = unityrs.Environment()
            env.load_file(fixture(), name='f')
            obj = sorted(env.objects, key=lambda o: o.path_id)[1]
            obj.assets_file.reader
            if read_before:
                obj.read_typetree()
            if full:
                obj._native = FullPath(obj._native)
            return obj

        expected = fresh(True, False).read_typetree()
        for cls in (ObjectReader, SerializedFile):
            original = cls.__dict__['reader']
            calls = []

            # Its name, module and code's wrapper as the original's.
            @functools.wraps(original.fget)
            def get(self, original=original):
                calls.append(type(self).__name__)
                return original.fget(self)

            for read_before in (False, True):
                objs = [fresh(full, read_before) for full in (False, True)]
                cls.reader = property(get, original.fset)
                try:
                    seen = []
                    for obj in objs:
                        del calls[:]
                        seen.append((obj.read_typetree(), list(calls)))
                finally:
                    cls.reader = original
                with self.subTest(cls=cls.__name__, read_before=read_before):
                    self.assertEqual(seen[0], seen[1])
                    self.assertEqual(seen[0][0], expected)
                    if cls is ObjectReader:
                        self.assertIn('ObjectReader', seen[0][1])

    def test_a_pair_of_two_classes(self):
        """A `pair` whose items are both classes' objects: each made, the
        pair then their tuple, as on the full path."""
        with pairs_class() as data:
            env = unityrs.Environment()
            env.load_file(data, name='f')
            obj = env.objects[0]
            self.assertIsNotNone(obj._native.parse_direct(obj, True, True))
            direct = read(data, 0, False)
            full = read(data, 0, True)
        self.assertIs(type(direct.m_Pair), tuple)
        self.assertEqual([type(v).__name__ for v in direct.m_Pair], ['Vector3f', 'Vector3f'])
        self.assertEqual(shape(direct), shape(full))

    def test_a_half_made_pairs_parent_changed_while_its_items_are_made(self):
        """Code run while a pair's items are made (here a profile hook, as
        the first item's class is called) finds only an opaque stand-in
        for the pair in its parent: trying to grow it changes nothing (the
        full path's object), and replacing it raises."""
        classes = unityrs.classes
        for change in ('grow', 'replace'):
            seen = []

            def profile(frame, event, arg):
                if event != 'call' or frame.f_code is not classes.Vector3f.__init__.__code__ or seen:
                    return
                for o in gc.get_objects():
                    # The root's dict.
                    if (type(o) is dict and o.keys() == {'m_Pair', 'm_Items'}
                            and o['m_Items'] == [7]):
                        stand = o['m_Pair']
                        seen.append(type(stand))
                        if change == 'replace':
                            o['m_Pair'] = [1, 2, 3]
                        else:
                            try:
                                stand.append(3)
                            except AttributeError:
                                pass

            with pairs_class() as data, self.subTest(change=change):
                full = read(data, 0, True)
                env = unityrs.Environment()
                env.load_file(data, name='f')
                obj = env.objects[0]
                sys.setprofile(profile)
                try:
                    if change == 'replace':
                        with self.assertRaises(RuntimeError):
                            obj._native.parse_direct(obj, True, True)
                    else:
                        direct = obj._native.parse_direct(obj, True, True)
                finally:
                    sys.setprofile(None)
                self.assertEqual(seen, [object])
                if change == 'grow':
                    self.assertEqual(shape(direct), shape(full))

    def test_a_container_freed_while_its_items_are_made(self):
        """A container freed while a child's object is still to go in it
        (the root's list of children, taken from the root's dict, emptied,
        and a collection forced, as the first class is called) leaves the
        read to raise, never to write into freed memory."""
        pptr = unityrs.classes.PPtr.__init__.__code__
        for _ in range(3):
            pending = []

            def profile(frame, event, arg):
                if event != 'call' or frame.f_code is not pptr or pending:
                    return
                for o in gc.get_objects():
                    children = o.get('m_Children') if type(o) is dict else None
                    # The root's list, its children still the classes'
                    # dicts (their objects not yet in it).
                    if (type(children) is list and len(children) == 2
                            and all(type(c) is dict for c in children)
                            and [c.get('m_PathID') for c in children] == [8, 9]):
                        pending.append(len(children))
                        o.pop('m_Children').clear()
                gc.collect()

            env = unityrs.Environment()
            env.load_file(fixture(), name='f')
            obj = sorted(env.objects, key=lambda o: o.path_id)[0]
            sys.setprofile(profile)
            try:
                with self.assertRaises(IndexError):
                    obj._native.parse_direct(obj, True, True)
            finally:
                sys.setprofile(None)
            self.assertEqual(pending, [2])

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

    def test_a_large_object_last_fits_the_work_left(self):
        data = large_fixture()

        def whole(full, work):
            env = unityrs.Environment(limits=unityrs.Limits(max_decode_work=work))
            env.load_file(data, name='f')
            objs = sorted(env.objects, key=lambda o: o.path_id)
            # The large one last, and again: its declined direct reads
            # count nothing the full path counts again.
            for obj in objs + objs[-1:]:
                if full:
                    obj._native = FullPath(obj._native)
                obj.parse_as_dict()
        work = [least(lambda m: succeeds(lambda: whole(full, m))) for full in (False, True)]
        self.assertEqual(work[0], work[1])
        env = unityrs.Environment()
        env.load_file(data, name='f')
        large = sorted(env.objects, key=lambda o: o.path_id)[1]
        self.assertEqual(len(large.parse_as_dict()['items']), 10000)


if __name__ == '__main__':
    unittest.main()
