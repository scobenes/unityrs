# Ported from UnityPy git 417d595 (MIT): UnityPy/tools/TpkClassGenerator.py. See LICENSE-UNITYPY-HEAD and THIRD_PARTY_LICENSES.
"""Generate in memory from the caller's TPK, never from a committed schema.

HEAD tools/TpkClassGenerator.py:132-306 defines field merging and typing;
HEAD tools/TpkClassGenerator.py:309-367 defines release-root traversal and bases.
The installed 1.25.2 generated module is compared in full at test time.
"""
import ast as _ast
from abc import ABC as _ABC
from . import math as _math
from .Object import Object as _Object
from .PPtr import PPtr as _PPtr
from ._generation_limits import Budget as _Budget, check_source as _check_source, ExpansionError as _ExpansionError
from ._generator import generate_classes as _generate, GENERATED_HEADER as _header


def _checked(source):
    # TPK names are untrusted. The generator emits imports and a fixed decorator;
    # validate only its class/field definitions before executing those names.
    _check_source(source)
    tree = _ast.parse(source)
    allowed = (_ast.Name, _ast.Subscript, _ast.Tuple, _ast.Load)
    header = _ast.parse(_header)
    count = len(header.body)
    if [_ast.dump(n) for n in tree.body[:count]] != [_ast.dump(n) for n in header.body]:
        raise ImportError("invalid generated header")
    budget = _Budget()
    # Only trusted classes imported by the fixed header can start a hierarchy.
    roots = {'object': object, 'Object': _Object, 'ABC': _ABC, 'PPtr': _PPtr}
    roots.update((name, value) for name, value in vars(_math).items()
                 if isinstance(value, type) and not name.startswith('_'))
    hierarchies = {name: (len(cls.__mro__), len(cls.__mro__),
                         sum(len(vars(base).get('__annotations__', {})) for base in cls.__mro__))
                   for name, cls in roots.items()}
    for node in tree.body[count:]:
        if (not isinstance(node, _ast.ClassDef) or node.keywords or
                len(node.decorator_list) != 1 or
                not isinstance(node.decorator_list[0], _ast.Name) or
                node.decorator_list[0].id != "unitypy_define"):
            raise ImportError("invalid class definition in TPK")
        budget.class_record()
        bases = []
        for base in node.bases:
            if not isinstance(base, _ast.Name):
                raise ImportError('invalid class base in TPK')
            if base.id not in hierarchies:
                raise ImportError('unknown or forward class base in TPK')
            bases.append(hierarchies[base.id])
        fields = 0
        for field in node.body:
            if isinstance(field, _ast.Pass):
                continue
            if not isinstance(field, _ast.AnnAssign) or not isinstance(field.target, _ast.Name):
                raise ImportError('invalid field definition in TPK')
            budget.field()
            fields += 1
            if any(not isinstance(part, allowed) for part in _ast.walk(field.annotation)):
                raise ImportError('invalid field type in TPK')
            if field.value is not None and not (isinstance(field.value, _ast.Constant) and field.value.value is None):
                raise ImportError('invalid field default in TPK')
        hierarchies[node.name] = budget.inheritance(bases, fields)
    return tree


try:
    exec(compile(_checked(_generate()), '<unityrs TPK classes>', 'exec'))
except (MemoryError, _ExpansionError):
    raise
except Exception as _error:
    raise ImportError('cannot generate unityrs.classes from the supplied TPK') from _error
