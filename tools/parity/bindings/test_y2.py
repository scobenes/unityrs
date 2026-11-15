"""Y2 compares runtime generation and object construction with the pinned oracle.

No generated names, fields or class-ID maps are recorded by this suite.
"""
import importlib
import unittest
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


def shape(module):
    import attrs
    import types
    def code_shape(code, hash_method=False):
        def constant(value):
            if isinstance(value,types.CodeType):
                return code_shape(value,hash_method)
            if isinstance(value,tuple):
                return tuple(constant(v) for v in value)
            if hash_method and type(value) is int:
                return '<attrs hash salt>'
            return value
        return (code.co_code, code.co_names, code.co_varnames,
                tuple(constant(v) for v in code.co_consts))
    def method_shape(cls, name):
        method = getattr(cls,name,None)
        code = getattr(method,'__code__',None)
        return (name in cls.__dict__, None if method is None else
                code_shape(code,name == '__hash__') if code is not None else type(method).__name__)
    def behaviour(cls):
        values = {a.alias:1 for a in attrs.fields(cls) if a.init}
        first, second = cls(**values), cls(**values)
        def hashed(value):
            try:
                return hash(value)
            except Exception as error:
                return type(error).__name__
        before = hashed(first)
        result = [first == second, first != second, first == object(),
                  type(before).__name__, before == hashed(second)]
        fields = attrs.fields(cls)
        if fields:
            setattr(second,fields[0].name,2)
            result += [first == second,first != second,before == hashed(second)]
        return result
    result = {}
    for name, cls in vars(module).items():
        if isinstance(cls, type) and cls.__module__ == module.__name__:
            result[name] = (
                tuple(base.__name__ for base in cls.__bases__),
                list(cls.__annotations__.items()),
                [(a.name, a.type, 'required' if a.default is attrs.NOTHING else a.default,
                  a.init, a.kw_only, a.eq, a.eq_key, a.hash, a.order, a.order_key,
                  a.repr, a.inherited, a.alias, dict(a.metadata), a.converter,
                  a.validator, a.on_setattr) for a in attrs.fields(cls)],
                getattr(cls, '__slots__', None),
                tuple((name,method_shape(cls,name)) for name in
                      ('__init__','__eq__','__ne__','__hash__','__lt__','__le__','__gt__','__ge__',
                       '__repr__','__setattr__','__getstate__','__setstate__')),
                getattr(cls,'__match_args__',None), getattr(cls,'__attrs_own_setattr__',None),
                tuple(name in vars(cls) for name in ('__slots__','__dict__','__weakref__')),
                behaviour(cls),
            )
    return result


class Classes(unittest.TestCase):
    def test_generated_class_set(self):
        oracle = importlib.import_module('UnityPy.classes.generated')
        actual = importlib.import_module('unityrs.classes.generated')
        self.assertEqual(shape(actual), shape(oracle))

    def test_class_id_map(self):
        def mapping(package):
            module = importlib.import_module(package + '.classes.ClassIDTypeToClassMap')
            return {int(k): v.__name__ for k, v in module.ClassIDTypeToClassMap.items()}
        self.assertEqual(mapping('unityrs'), mapping('UnityPy'))

    def test_base_math_behaviour(self):
        import copy
        def observe(package):
            c = importlib.import_module(package + '.classes')
            p = c.PPtr(m_FileID=2, m_PathID=3)
            color = c.ColorRGBA(rgba=0x12345678)
            color.rgba = 0xFEDCBA98
            unknown = c.UnknownObject(value='x'*130)
            obj = c.Object(x=1)
            out = [repr(c.Vector2f()), repr(c.Vector3f(1,2,3)), repr(c.Quaternionf()),
                   color.rgba, repr(unknown), repr(copy.copy(obj)), copy.copy(obj).x,
                   bool(p), p.file_id, p.path_id, hash(p), p == c.PPtr(m_FileID=2,m_PathID=3)]
            for method in (obj.save, p.deref, c.PPtr(m_FileID=0,m_PathID=0).deref):
                try: method()
                except Exception as e: out.append((type(e).__name__, str(e)))
            return out
        self.assertEqual(observe('unityrs'), observe('UnityPy'))

    def test_existing_synthetic_object_reads(self):
        import UnityPy
        import unityrs
        from object_cases import cases, observe
        for name, data in cases():
            with self.subTest(name=name):
                self.assertEqual(observe(unityrs, data), observe(UnityPy, data))
        data = next(data for name,data in cases() if name == 'text')
        actual = unityrs.load(data).objects[0].read_typetree(as_dict=False)
        expected = UnityPy.load(data).objects[0].read_typetree(wrap=True)
        self.assertEqual((type(actual).__name__,actual.m_Name,actual.m_Script),
                         (type(expected).__name__,expected.m_Name,expected.m_Script))

    def test_every_class_construction_rule(self):
        import attrs
        import struct
        import UnityPy
        import unityrs
        from binding_cases import S
        from object_cases import observe
        generated = importlib.import_module('UnityPy.classes.generated')
        for name, cls in vars(generated).items():
            if not isinstance(cls,type) or cls.__module__ != generated.__name__:
                continue
            fields = list(attrs.fields(cls))
            for mode in ('all','missing','extra','optional_extra'):
                names = [a.name for a in fields if mode != 'optional_extra' or a.default is attrs.NOTHING]
                if mode == 'missing' and names:
                    names.pop(0)
                if mode in ('extra','optional_extra'):
                    names.append('m_Extra')
                inner = S.Node(name,'inner',[S.Node('int',n) for n in names])
                root = S.Node('Object','Base',[S.Node('int','pad'),inner])
                data = S.serialized_file([S.Type(49,root)],[S.Obj(1,0,struct.pack('<'+'i'*(len(names)+1),*range(len(names)+1)))])
                with self.subTest(name=name,mode=mode):
                    self.assertEqual(observe(unityrs,data),observe(UnityPy,data))

    def test_pointer_resolution(self):
        import UnityPy
        import unityrs
        import struct
        from binding_cases import S
        def observe(module):
            c = importlib.import_module(module.__name__ + '.classes')
            tree = S.Node('Object','Base',[S.Node('int','value')])
            first = S.serialized_file([S.Type(49,tree)],[S.Obj(1,0,struct.pack('<i',123))],externals=[S.External('archive:/other/other.assets')])
            second = S.serialized_file([S.Type(49,tree)],[S.Obj(2,0,struct.pack('<i',456))])
            env = module.Environment()
            a = env.load_file(first,name='first.assets')
            b = env.load_file(second,name='other.assets')
            result=[]
            for file_id,path_id in ((0,1),(1,2),(1,999),(2,1),(-1,1),(0,0)):
                p = c.PPtr(m_FileID=file_id,m_PathID=path_id,assetsfile=a)
                try:
                    reader=p.deref()
                    result.append([reader.path_id,int(p.type),p.deref_parse_as_dict(),p.read().value,reader is b.objects.get(2)])
                except Exception as e:
                    result.append([type(e).__name__,str(e)])
            return result
        self.assertEqual(observe(unityrs),observe(UnityPy))

    def test_no_tpk_error_and_explicit_sources(self):
        import sys
        import child
        code = r"""
import importlib.util, os, types
from pathlib import Path
import unityrs
spec = importlib.util.find_spec('UnityPy')
path = Path(next(iter(spec.submodule_search_locations))) / 'resources' / 'lzma.tpk'
mode = os.environ['Y2_SOURCE_MODE']
original = importlib.util.find_spec
importlib.util.find_spec = lambda name,*a,**kw: None if name=='UnityPy' else original(name,*a,**kw)
if mode == 'bytes':
    unityrs.config = types.SimpleNamespace(TPK=path.read_bytes())
elif mode == 'path':
    os.environ['UNITYRS_TPK'] = str(path)
else:
    os.environ.pop('UNITYRS_TPK',None)
try:
    import unityrs.classes
except ImportError as error:
    assert mode == 'missing', str(error)
    assert 'UNITYRS_TPK' in str(error) and 'config.TPK' in str(error), str(error)
else:
    assert mode != 'missing'
    assert unityrs.classes.Object
"""
        import os
        for mode in ('missing','bytes','path'):
            with self.subTest(mode=mode):
                child.run([sys.executable,'-c',code],env=dict(os.environ,Y2_SOURCE_MODE=mode),check=True,capture_output=True,text=True,timeout=120)

    def test_tpk_codecs_and_rejections(self):
        import struct
        import lz4.block
        from unityrs._class_tpk import load, source
        from unityrs.classes.generated import _checked
        data = source()
        blob = load(data)
        # Re-encode at test time; no payload or schema enters the repository.
        from UnityPy.helpers.CompressionHelper import decompress_lzma
        decoded = decompress_lzma(data[20:])
        for compression,payload in ((0,decoded),(1,lz4.block.compress(decoded,store_size=False))):
            header = struct.pack('<IbbbbIII',0x2A4B5054,1,compression,0,0,0,len(payload),len(decoded))
            actual = load(header+payload)
            self.assertEqual(actual.StringBuffer,blob.StringBuffer)
            self.assertEqual(actual.ClassInformation,blob.ClassInformation)
            self.assertEqual(actual.NodeBuffer,blob.NodeBuffer)
        for prefix in range(20):
            with self.subTest(prefix=prefix), self.assertRaises(ImportError):
                load(data[:prefix])
        from unityrs.classes._generator import GENERATED_HEADER
        for text in ('import os', '@unitypy_define\nclass X:\n    x: int = print(1)',
                     '@unitypy_define\nclass X:\n    x: print(1)',
                     '@unitypy_define\nclass X:\n    def __init__(self): print(1)'):
            with self.subTest(text=text), self.assertRaises(ImportError):
                _checked(GENERATED_HEADER+'\n'+text)

    def test_object_conversion_keeps_combined_staging_charged(self):
        import unityrs
        from binding_cases import S
        root = S.Node('Object','Base',[S.Node('string','a'),S.Node('string','b')])
        data = S.serialized_file([S.Type(49,root)],[S.Obj(1,0,S.s32('a'*8192)+S.s32('b'*8192))])
        budget = unityrs.Budget(80_000)
        env = unityrs.Environment(data,budget=budget)
        baseline = budget.used
        for _ in range(3):
            with self.assertRaises(MemoryError):
                env.objects[0].read()
        # Failed conversion releases its staging, while the compiled tree stays.
        self.assertLessEqual(budget.used, baseline + 512)
        self.assertEqual(len(env.objects[0].parse_as_dict()['a']),8192)
        enough = unityrs.Environment(data,budget=unityrs.Budget(90_000))
        self.assertEqual(enough.objects[0].read().b,'b'*8192)
        from unityrs.enums import ClassIDType
        self.assertEqual(int(ClassIDType.TextAsset),49)
