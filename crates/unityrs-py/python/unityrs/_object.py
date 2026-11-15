# Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPyBoost/TypeTreeHelper.cpp. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Object construction, after detached core parsing and metered conversion.

UnityPyBoost/TypeTreeHelper.cpp:458-583 is the Boost construction rule.
helpers/TypeTreeHelper.py:233-258 and helpers/TypeTreeHelper.py:314-357
supply the zero-byte reader's two construction rules.
"""
from types import SimpleNamespace


def get_annotation_keys(cls):
    return set(getattr(cls, '__annotations__', {}))


def _reserve(charge, memory, work):
    if charge is not None:
        charge.reserve(memory, work)


def _node_lists(lists, charge):
    # Y3's public node, when available, is also the UnknownObject node type.
    try:
        from .helpers.TypeTreeNode import TypeTreeNode
    except ModuleNotFoundError as error:
        if error.name not in ('unityrs.helpers', 'unityrs.helpers.TypeTreeNode'):
            raise
        TypeTreeNode = SimpleNamespace
    result = []
    for rows in lists:
        # Native staging already reserves each node and edge's Python memory.
        _reserve(charge, 0, 4096 * len(rows) + sum(
            64 * len(row['m_Children']) + 4 * (len(row['m_Name']) + len(row['m_Type']))
            for row in rows))
        nodes = [TypeTreeNode(**dict(row, m_Children=[])) for row in rows]
        for node, row in zip(nodes, rows):
            node.m_Children = [nodes[i] for i in row['m_Children']]
        result.append(nodes)
    return result


def _construct(node, values, assetsfile, boost, array=False, charge=None, node_work=0):
    from . import classes
    from .classes import UnknownObject
    if node.m_Type.startswith('PPtr<'):
        cls = classes.PPtr
        _reserve(charge, 2048, 256)
        values['assetsfile'] = assetsfile
        if not boost:
            if array:
                return cls(**values)
            return cls(assetsfile=assetsfile, m_FileID=values['m_FileID'], m_PathID=values['m_PathID'])
    else:
        try:
            cls = getattr(classes, node.m_Type)
            missing = False
        except AttributeError:
            cls = UnknownObject
            missing = True
        # PARITY 19.2 deliberately treats module nonclasses as missing.
        if not isinstance(cls, type) or cls.__module__ == 'builtins':
            cls = UnknownObject
            missing = True
        if missing and boost:
            values['__node__'] = node
    # attrs defaults can materialize many fields from a zero-field value.
    # Include retry/fallback dictionaries and annotation/slot filtering, and
    # charge before invoking a constructor or copying its keys.
    fields = (len(getattr(cls, '__attrs_attrs__', ())) +
              len(getattr(cls, '__annotations__', {})) +
              len(getattr(cls, '__slots__', ())))
    _reserve(charge, 512 + 256 * (fields + len(values)),
             256 * (fields + len(values) + 1) + node_work)
    if not boost and array:
        keys = get_annotation_keys(cls)
        if cls is UnknownObject or keys - values.keys():
            return UnknownObject(node, **values)
        result = cls(**{k:values[k] for k in keys})
        for k in values.keys() - keys:
            setattr(result, k, values[k])
        return result
    try:
        return cls(**values)
    except Exception as error:
        if not boost and not isinstance(error, TypeError):
            raise
    keys = get_annotation_keys(cls)
    if boost:
        slots = getattr(cls, '__slots__', ())
        if isinstance(slots, tuple) and slots:
            return UnknownObject(**dict(values, __node__=node))
        # Boost removes extras from kwargs in child order, then appends them
        # after the constructor fields when falling back to UnknownObject.
        from .classes._names import clean_name
        extras = {}
        for child in node.m_Children:
            name = clean_name(child.m_Name, ascii_digit=True)
            if name not in keys and name in values:
                extras[name] = values.pop(name)
        if extras:
            try:
                result = cls(**values)
                for k,v in extras.items():
                    object.__setattr__(result,k,v)
                return result
            except Exception:
                pass
        # Boost adds the node, then restores extras. An extra named __node__
        # therefore wins here, while the nonempty-slots shortcut overwrites it.
        values['__node__'] = node
        values.update(extras)
        return UnknownObject(**values)
    if cls is UnknownObject or keys - values.keys():
        return UnknownObject(node, **values)
    extras = values.keys() - keys
    if extras:
        result = cls(**{k:values[k] for k in keys})
        for k in extras:
            setattr(result,k,values[k])
        return result
    return UnknownObject(**values)


def build(data, lists, assetsfile, boost, references=(), completed=0, error=None, charge=None):
    """Postorder work stack keeps large arrays and deep trees off Python's stack."""
    from .classes._names import clean_name
    nodes = _node_lists(lists, charge)
    indices = {id(node): i for rows in nodes for i,node in enumerate(rows)}
    # Cache schema costs once; a class-array element may scan this same set
    # of child names repeatedly while cleaning names and filtering extras.
    costs = {id(node): 4 * len(node.m_Type) + sum(
        256 + 4 * len(child.m_Name) for child in node.m_Children)
             for rows in nodes for node in rows}
    def fields(node, value):
        return [(child,value[indices[id(child)]]) for child in node.m_Children
                if indices[id(child)] in value]
    ref_roots = {}
    _reserve(charge, 256 * len(references), 256 * len(references))
    for cls, ns, asm, index in references:
        _reserve(charge, 0, 4 * (len(cls) + len(ns) + len(asm)))
        ref_roots.setdefault((cls,ns,asm), nodes[index][0])
    root = [None]
    tasks = [('value', data, nodes[0][0], root, 0, False)]
    while tasks:
        if error is not None and completed == 0:
            raise error
        action, value, node, parent, key, array = tasks.pop()
        _reserve(charge, 256, 256)
        if action == 'reference':
            from .classes import UnknownObject
            parent[key] = UnknownObject(node, **value) if boost else value
            completed -= 1
        elif action == 'class':
            parent[key] = _construct(node, value, assetsfile, boost, array, charge, costs[id(node)])
            completed -= 1
        elif action == 'tuple':
            parent[key] = tuple(value)
        elif isinstance(value, dict):
            _reserve(charge, 256 * len(value), costs[id(node)])
            out = {}
            reference = node.m_Type == 'ReferencedObject'
            tasks.append(('reference' if reference else 'class', out, node, parent, key, array))
            ref_root = None

            # Reverse scheduling, forward key insertion (duplicates last win).
            children = []
            for child, raw_value in fields(node,value):
                if reference and child.m_Name == 'type':
                    names = {(part.m_Name if child.m_Type == 'ReferencedObject'
                              else clean_name(part.m_Name, ascii_digit=True)):v
                             for part,v in fields(child,raw_value)} if isinstance(raw_value,dict) else {}
                    descriptor = tuple(names.get(k) for k in ('class','ns','asm'))
                    ref_root = ref_roots.get(descriptor) if all(isinstance(v,str) for v in descriptor) else None
                raw_names = reference or (not boost and array and node.m_Type.startswith('PPtr<'))
                name = child.m_Name if raw_names else clean_name(child.m_Name, ascii_digit=True)
                out[name] = None
                target_node = ref_root if ref_root is not None and child.m_Type == 'ReferencedObjectData' else child
                children.append((target_node,name,raw_value))
            for child, name, raw_value in reversed(children):
                tasks.append(('value', raw_value, child, out, name, False))
        elif isinstance(value, (tuple,list)):
            _reserve(charge, 256 * len(value), 256 * len(value))
            out = [None] * len(value)
            if isinstance(value, tuple):
                tasks.append(('tuple',out,node,parent,key,array))
            else:
                parent[key] = out
            if node.m_Type == 'pair':
                children = [(node.m_Children[i],False) for i in range(len(value))]
            else:
                child = node.m_Children[0].m_Children[1]
                children = [(child, not boost and bool((child.m_MetaFlag or 0) & 0x4000))] * len(value)
            for i in reversed(range(len(value))):
                child, aligned_array = children[i]
                tasks.append(('value',value[i],child,out,i,aligned_array))
        else:
            parent[key] = value
    if error is not None:
        raise error
    return root[0]
