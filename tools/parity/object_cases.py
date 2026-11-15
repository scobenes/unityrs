"""Object observations restricted to synthetic node names and input values.

Never enumerate attrs fields or record defaults from a generated class here.
"""
import contextlib
import io
import struct
import binding_cases as B
from binding_cases import S


def cases():
    yield from B.cases()
    for kind in ('Vector3f', 'Quaternionf', 'Object', 'GameManager', 'TextAsset', 'MissingFixtureClass'):
        for fields in ([], [S.Node('int', 'm_Extra')]):
            tree = S.Node('Object', 'Base', [S.Node('int','pad'), S.Node(kind,'inner',fields)])
            payload = struct.pack('<'+'i'*(1+len(fields)), *range(1+len(fields)))
            yield kind + ('_extra' if fields else '_empty'), S.serialized_file([S.Type(49,tree)],[S.Obj(1,0,payload)])
    tree = S.Node('Object','Base',[S.Node('int',n) for n in ('²ab','2cd','(int&)3cd?','pass','from','a.b','a-b','a b')])
    yield 'clean_names', S.serialized_file([S.Type(49,tree)],[S.Obj(1,0,struct.pack('<8i',*range(8)))])
    for aligned in (False,True):
        element = S.Node('Vector3f','data',[S.Node('float','x'), S.Node('int','extra')],meta=S.ALIGN if aligned else 0)
        tree = S.Node('Object','Base',[S.vec('items',element)])
        yield 'class_array_'+str(aligned), S.serialized_file([S.Type(49,tree)],[S.Obj(1,0,struct.pack('<ififi',2,1.5,4,2.5,8))])
    pointer = S.Node('PPtr<Object>','ptr',[S.Node('int','m_FileID'),S.Node('SInt64','m_PathID')])
    tree = S.Node('Object','Base',[pointer])
    yield 'pointer', S.serialized_file([S.Type(49,tree)],[S.Obj(1,0,struct.pack('<iq',0,1))])

    # A declared byte_size of zero selects the pure-Python reader. Its payload
    # remains in the file, allowing check_read=False to exercise construction.
    for aligned in (False,True):
        for kind in ('Vector3f','TextAsset','MissingFixtureClass'):
            element = S.Node(kind,'data',[S.Node('float','x'),S.Node('int','extra')],meta=S.ALIGN if aligned else 0)
            tree = S.Node('Object','Base',[S.vec('items',element)])
            payload = struct.pack('<ifi',1,1.5,4)
            yield 'pure_array_'+kind+'_'+str(aligned), S.serialized_file([S.Type(49,tree)],[S.Obj(1,0,payload,byte_size=0)])
    for fields in ([S.Node('int','m_FileID'),S.Node('SInt64','m_PathID'),S.Node('int','extra')],
                   [S.Node('int','m_FileID')]):
        tree = S.Node('Object','Base',[S.Node('PPtr<Object>','ptr',fields)])
        payload = struct.pack('<iqi',0,1,5) if len(fields)==3 else struct.pack('<i',0)
        yield 'pure_pointer_'+str(len(fields)), S.serialized_file([S.Type(49,tree)],[S.Obj(1,0,payload,byte_size=0)])

    tree = S.Node('Object','Base',[S.vec('items',S.Node('int','data',meta=S.ALIGN))])
    yield 'pure_primitive_array', S.serialized_file([S.Type(49,tree)],[S.Obj(1,0,struct.pack('<3i',2,5,8),byte_size=0)])

    for kind in ('Vector3f','int'):
        ref_root = (S.Node(kind,'Base',[S.Node('float',n) for n in ('x','y','z')])
                    if kind == 'Vector3f' else S.Node('int','Base'))
        root = S.Node('Object','Base',[S.ref_object('ref')])
        payload = struct.pack('<q',7)+S.s32('FixtureType')+S.s32('')+S.s32('FixtureAssembly')
        payload += struct.pack('<3f',1,2,3) if kind=='Vector3f' else struct.pack('<i',123)
        for zero in (False,True):
            refs = [S.Type(-1,ref_root,ref_names=['FixtureType','','FixtureAssembly'])]
            yield 'reference_'+kind+'_'+str(zero), S.serialized_file([S.Type(49,root)],
                [S.Obj(1,0,payload,byte_size=0 if zero else None)],ref_types=refs)

    for first in ('m_FileID','(int&)m_FileID'):
        fields = [S.Node('int',first),S.Node('SInt64','m_PathID'),S.Node('int','extra')]
        root = S.Node('Object','Base',[S.vec('items',S.Node('PPtr<Object>','data',fields,meta=S.ALIGN))])
        payload = struct.pack('<iiqi',1,0,1,2)
        yield 'pure_pointer_array_'+first, S.serialized_file([S.Type(49,root)],[S.Obj(1,0,payload,byte_size=0)])
    fields = [S.Node('float',n) for n in ('x','y','z')]+[S.Node('int','extra')]
    root = S.Node('Object','Base',[S.vec('items',S.Node('Vector3f','data',fields,meta=S.ALIGN))])
    yield 'pure_array_constructor_before_size', S.serialized_file([S.Type(49,root)],[S.Obj(1,0,struct.pack('<i3fi',1,1,2,3,4),byte_size=0)])

    root = S.Node('Object','Base',[S.Node('TextAsset','inner',[S.Node('int','__node__')])])
    yield 'fallback_overwrites_node', S.serialized_file([S.Type(49,root)],[S.Obj(1,0,struct.pack('<i',123))])

    for zero in (False, True):
        root = S.Node('Object','Base',[S.Node('int','dup'),S.vec('dup',S.Node('int','data'))])
        yield 'duplicate_shapes_'+str(zero), S.serialized_file([S.Type(49,root)],
            [S.Obj(1,0,struct.pack('<4i',7,2,8,9),byte_size=0 if zero else None)])
    vector = S.Node('Vector3f','first',[S.Node('float',n) for n in ('x','y','z')]+[S.Node('int','extra')])
    for later in (S.Node('PPtr<Object>','later',[S.Node('int','m_FileID')]),
                  S.Node('int','later'), S.vec('later',S.Node('int','data'))):
        root = S.Node('Object','Base',[vector,later])
        payload = struct.pack('<3fii',1,2,3,4,0) if later.type.startswith('PPtr') else struct.pack('<3fi',1,2,3,4)
        yield 'constructor_before_'+later.type, S.serialized_file([S.Type(49,root)],
            [S.Obj(1,0,payload,byte_size=0)])
    for children in ([vector], [vector,S.Node('int','second')]):
        root = S.Node('Object','Base',[S.Node('pair','items',children)])
        yield 'pair_constructor_before_failure_'+str(len(children)), S.serialized_file(
            [S.Type(49,root)],[S.Obj(1,0,struct.pack('<3fi',1,2,3,4),byte_size=0)])
    element = S.Node('Vector3f','data',vector.children,meta=S.ALIGN)
    root = S.Node('Object','Base',[S.vec('items',element)])
    yield 'array_constructor_before_failure', S.serialized_file([S.Type(49,root)],
        [S.Obj(1,0,struct.pack('<i3fi',2,1,2,3,4),byte_size=0)])
    descriptor = {'class':'FixtureType','ns':'FixtureNamespace','asm':'FixtureAssembly'}
    for zero in (False,True):
        for index,field in enumerate(descriptor):
            for last_wins in (False,True):
                reference = S.ref_object('ref')
                reference.children[1].children.insert(index,S.Node('string',field))
                root = S.Node('Object','Base',[reference])
                ref_root = S.Node('Object','Base',[S.Node('int','value')])
                strings = []
                for name,value in descriptor.items():
                    strings.extend((('',value) if last_wins else (value,'')) if name == field else (value,))
                payload = struct.pack('<q',7)+b''.join(S.s32(s) for s in strings)+struct.pack('<i',123)
                refs = [S.Type(-1,ref_root,ref_names=list(descriptor.values()))]
                yield 'duplicate_reference_'+field+'_'+str(zero)+'_'+str(last_wins), S.serialized_file(
                    [S.Type(49,root)],[S.Obj(1,0,payload,byte_size=0 if zero else None)],ref_types=refs)
    for zero in (False,True):
        reference = S.ref_object('ref')
        reference.children += S.ref_object().children[1:]
        first = S.Node('Vector3f','Base',vector.children)
        second = S.Node('Object','Base',[S.Node('int','value')])
        refs = [S.Type(-1,node,ref_names=[name,'','FixtureAssembly'])
                for node,name in ((first,'FirstType'),(second,'SecondType'))]
        payload = struct.pack('<q',7)+S.s32('FirstType')+S.s32('')+S.s32('FixtureAssembly')
        payload += struct.pack('<3fi',1,2,3,4)+S.s32('SecondType')+S.s32('')+S.s32('FixtureAssembly')
        payload += struct.pack('<i',123)
        root = S.Node('Object','Base',[reference])
        yield 'duplicate_reference_data_shapes_'+str(zero), S.serialized_file(
            [S.Type(49,root)],[S.Obj(1,0,payload,byte_size=0 if zero else None)],ref_types=refs)
    for array in (False,True):
        reference = S.ref_object('ref')
        field = S.Node('Vector3f','data' if array else 'class',vector.children)
        reference.children[1].children[0] = S.vec('class',field) if array else field
        root = S.Node('Object','Base',[reference])
        ref_root = S.Node('Object','Base',[S.Node('int','value')])
        refs = [S.Type(-1,ref_root,ref_names=['FixtureType','','FixtureAssembly'])]
        payload = struct.pack('<q',7)+(struct.pack('<i',1) if array else b'')
        payload += struct.pack('<3fi',1,2,3,4)+S.s32('')+S.s32('FixtureAssembly')
        yield 'reference_descriptor_constructor_before_lookup_'+str(array), S.serialized_file(
            [S.Type(49,root)],[S.Obj(1,0,payload,byte_size=0)],ref_types=refs)


def typed(value, template=None):
    if isinstance(value, dict):
        return ['dict', [[k,typed(v, template.get(k) if isinstance(template,dict) else None)] for k,v in value.items()]]
    if isinstance(getattr(value,'m_Type',None),str):
        return ['node',value.m_Type,value.m_Name]
    if isinstance(value,(tuple,list)):
        return [type(value).__name__, [typed(v, template[i] if template is not None else None) for i,v in enumerate(value)]]
    if type(value).__module__.startswith(('UnityPy.classes','unityrs.classes')):
        from binding_names import clean_name
        fields = []
        if isinstance(template,dict):
            fields = [[clean_name(k,ascii_digit=True),typed(getattr(value,clean_name(k,ascii_digit=True),None),v)] for k,v in template.items()]
        return ['object',type(value).__name__,getattr(value,'get_type',lambda:None)(),fields]
    return B.typed(value)


def observe(module,data):
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            env = module.Environment()
            env.load_file(data,name='input.assets')
        result=[]
        for reader in env.objects:
            for check in (True,False):
                for method in ('read','parse_as_object','wrap'):
                    try:
                        obj=(reader.read_typetree(wrap=True,check_read=check) if method=='wrap'
                             else getattr(reader,method)(check_read=check))
                        fields=reader.parse_as_dict(check_read=check)
                        result.append([typed(obj,fields),obj.object_reader is reader,obj.assets_file is reader.assets_file])
                    except Exception as error:
                        result.append({'error':type(error).__name__,'message':str(error)})
        return result
    except Exception as error:
        return {'error':type(error).__name__,'message':str(error)}
