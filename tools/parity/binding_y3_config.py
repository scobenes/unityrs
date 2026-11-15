"""Y3 config, ContainerHelper and object peek observations on MIT synthetic inputs.

Independent of the binding and oracle imports; recorder and binding tests select
one module. No generated class or TPK content is retained.
"""

import importlib
import contextlib
import io
import struct
import warnings
from types import SimpleNamespace

from binding_cases import S, typed


def _call(function):
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            return {"value": typed(function())}
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}


def observe_config(module):
    config = importlib.import_module(module.__name__ + ".config")
    old = config.FALLBACK_UNITY_VERSION, config.SERIALIZED_FILE_PARSE_TYPETREE
    result = {}
    try:
        for value in (None, 12, b"2020.1.0f1", "", "not-a-version", "2020.1.0f1"):
            config.FALLBACK_UNITY_VERSION = value
            with warnings.catch_warnings(record=True) as seen:
                warnings.simplefilter("always")
                answer = _call(config.get_fallback_version)
                answer["warnings"] = [[type(w.message).__name__, str(w.message)] for w in seen]
                result[repr(value)] = answer
        config.FALLBACK_UNITY_VERSION = "2020.1.0f1"
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            result["warnings_error"] = _call(config.get_fallback_version)
        with warnings.catch_warnings(record=True) as seen:
            warnings.simplefilter("once")
            config.get_fallback_version()
            config.get_fallback_version()
            result["once"] = len(seen)
    finally:
        config.FALLBACK_UNITY_VERSION, config.SERIALIZED_FILE_PARSE_TYPETREE = old
    return result


class _Pointer:
    def __init__(self, path_id, file_id=0, target=None, error=None):
        self.path_id, self.file_id = path_id, file_id
        self.target, self.error, self.calls = target, error, 0

    def __bool__(self):
        return self.path_id != 0

    def __str__(self):
        return f"ptr({self.file_id},{self.path_id})"

    def deref(self):
        self.calls += 1
        if self.error:
            raise self.error("synthetic failure")
        return self.target


def _helper(module):
    return importlib.import_module(module.__name__ + ".helpers.ContainerHelper").ContainerHelper


def observe_container(module):
    helper = _helper(module)
    a, b, c = _Pointer(1), _Pointer(2), _Pointer(1, 3)
    rows = [("z", SimpleNamespace(asset=a)), ("a", SimpleNamespace(asset=b)),
            ("z", SimpleNamespace(asset=c))]
    container = helper(rows)
    result = {
        "length": len(container),
        "items": [[k, str(v)] for k, v in container.items()],
        # keys/iteration and values follow Python set iteration, which is
        # process-specific; compare their members and their internal agreement.
        "keys": sorted(container.keys()),
        "iteration": list(container) == container.keys(),
        "values": sorted(str(v) for v in container.values()),
        "lookup": str(container["z"]), "attribute": str(container.z),
        "mapping": [[k, str(v)] for k, v in container.container_dict.items()],
        "dict_access": _call(lambda: container.__dict__),
        "path_dict": list(container.path_dict.items()),
        "contains": ["z" in container, "missing" in container],
        "str": str(container), "missing": _call(lambda: container.missing),
        "assignment": _call(lambda: container.__setitem__("x", a)),
        "deletion": _call(lambda: container.__delitem__("x")),
        "list_identity": container.container is rows,
    }
    target = SimpleNamespace(assets_file=SimpleNamespace(_container=helper([])))
    p = _Pointer(7, target=target)
    missing = _Pointer(8, error=FileNotFoundError)
    keyed = _Pointer(9, error=KeyError)
    null = _Pointer(0)
    preload = [p, missing, keyed, null]
    info = lambda index, size: SimpleNamespace(asset=a, preloadIndex=index, preloadSize=size)
    source = SimpleNamespace(m_PreloadTable=preload, m_Container=[
        ("first", info(0, 4)), ("second", info(0, 1)),
        ("negative", info(-1, 2)), ("past", info(2, 3)), ("empty", info(0, 0))])
    container = helper(source)
    container.parse_preload_table()
    container.parse_preload_table()
    result["preload"] = {"paths": list(target.assets_file._container.path_dict.items()),
                         "calls": [p.calls, missing.calls, keyed.calls, null.calls],
                         "cleared": container._preload_table is None}
    bad = _Pointer(10, error=IndexError)
    source.m_PreloadTable = [bad]
    source.m_Container = [("bad", info(0, 1))]
    container = helper(source)
    result["uncaught"] = _call(container.parse_preload_table)
    result["uncaught_retains_table"] = container._preload_table is source.m_PreloadTable
    return result


def _file(module, tree, payload, version=b"2021.3.2f1"):
    data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, payload)], unity_version=version)
    env = module.Environment()
    with contextlib.redirect_stdout(io.StringIO()):
        file = env.load_file(data, name="config.assets")
    return file, file.objects[1]


def observe_config_loads(module):
    config = importlib.import_module(module.__name__ + ".config")
    old = config.FALLBACK_UNITY_VERSION, config.SERIALIZED_FILE_PARSE_TYPETREE
    root = S.Node("Y3ConfigText", "Y3Config", [S.Node("string", "m_Name")])
    result = {}
    try:
        config.FALLBACK_UNITY_VERSION = "bad\ud800"
        result["unused_surrogate_fallback"] = _call(
            lambda: _file(module, root, S.s32("valid"))[1].parse_as_dict())
        for value in (None, 12, "not-a-version", "2020.1.0f1"):
            config.FALLBACK_UNITY_VERSION = value
            with warnings.catch_warnings(record=True) as seen:
                warnings.simplefilter("always")
                def load():
                    file, obj = _file(module, root, S.s32("fallback"), b"")
                    return str(file.version), obj.peek_name()
                answer = _call(load)
                answer["warnings"] = [[type(w.message).__name__, str(w.message)] for w in seen]
                result[repr(value)] = answer
        config.FALLBACK_UNITY_VERSION = None
        for flag in (False, True, 0, None):
            config.SERIALIZED_FILE_PARSE_TYPETREE = flag
            file, obj = _file(module, root, S.s32("still-readable"))
            result["tree_" + repr(flag)] = [file._enable_type_tree, obj.parse_as_dict(), obj.peek_name()]
        # A single environment observes a setting changed after construction.
        env = module.Environment()
        data = S.serialized_file([S.Type(49, root)], [S.Obj(1, 0, S.s32("dynamic"))], unity_version=b"")
        with warnings.catch_warnings(record=True) as seen:
            warnings.simplefilter("always")
            versions = []
            for fallback in ("2019.1.0f1", "2020.1.0f1"):
                config.FALLBACK_UNITY_VERSION = fallback
                file = env.load_file(data, name=fallback)
                versions.append(str(file.version))
            result["changed_after_constructor"] = versions
            result["changed_warnings"] = [str(w.message) for w in seen]
        config.FALLBACK_UNITY_VERSION = "2020.1.0f1"
        env = module.Environment()
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            with contextlib.redirect_stdout(io.StringIO()):
                result["load_warning_error"] = _call(lambda: env.load_file(data, name="refused.assets"))
        result["warning_error_publication"] = [list(env.files), list(env.cabs)]

        # A custom handler re-enters the same environment; version resolution
        # must hold neither a core registry lock nor a warning-state lock.
        env = module.Environment()
        nested_data = S.serialized_file([S.Type(49, root)], [S.Obj(1, 0, S.s32("nested"))])
        delivered = []
        def showwarning(message, category, filename, lineno, file=None, line=None):
            delivered.append([category.__name__, str(message)])
            config.FALLBACK_UNITY_VERSION = "2019.1.0f1"
            env.load_file(nested_data, name="nested.assets")
        with warnings.catch_warnings():
            warnings.simplefilter("always")
            warnings.showwarning = showwarning
            file = env.load_file(data, name="outer.assets")
        result["reentrant_warning"] = [delivered, list(env.files), str(file.version)]
        for label, replacement in [("none", None), ("bytes", b"2019.1.0f1")]:
            config.FALLBACK_UNITY_VERSION = "2020.1.0f1"
            env = module.Environment()
            with warnings.catch_warnings():
                warnings.simplefilter("always")
                warnings.showwarning = lambda *args, **kwargs: setattr(config, "FALLBACK_UNITY_VERSION", replacement)
                result["warning_mutates_" + label] = _call(lambda: env.load_file(data, name="mutation.assets"))
            result["warning_mutation_publication_" + label] = [list(env.files), list(env.cabs)]
        # Registries belong to the two resolution modules, including across
        # repeated loads and new environments.
        bundle = S.fs_bundle([(b"inner.assets", data, 4)], engine=b"0.0.0")
        config.FALLBACK_UNITY_VERSION = "2020.1.0f1"
        with warnings.catch_warnings(record=True) as seen:
            warnings.simplefilter("once")
            for _ in range(2):
                module.Environment().load_file(bundle, name="fallback.bundle")
                module.Environment().load_file(data, name="fallback.assets")
            result["file_kind_once"] = [str(w.message) for w in seen]
        for flag in (False, True):
            config.FALLBACK_UNITY_VERSION = "2020.1.0f1"
            config.SERIALIZED_FILE_PARSE_TYPETREE = not flag
            delivered = []
            def mutate(message, *args, **kwargs):
                delivered.append(str(message))
                config.FALLBACK_UNITY_VERSION = "2019.1.0f1"
                config.SERIALIZED_FILE_PARSE_TYPETREE = flag
            with warnings.catch_warnings():
                warnings.simplefilter("always")
                warnings.showwarning = mutate
                file = module.Environment().load_file(bundle, name="mutated.bundle")
            inner = file.files["inner.assets"]
            result["callback_settings_" + str(flag)] = [
                delivered, str(inner.version), inner._enable_type_tree]
        for raw in (b"invalid\xff", b"invalid\xc3\xa9", b"\xff\xfe"):
            invalid = S.serialized_file([S.Type(49, root)], [], unity_version=raw)
            answer = _call(lambda: module.Environment().load_file(invalid, name="invalid.assets"))
            # Preserve lone surrogates losslessly in JSON consumed by Rust.
            if "message" in answer:
                answer["message"] = typed(answer["message"])
            result["invalid_version_" + raw.hex()] = answer
    finally:
        config.FALLBACK_UNITY_VERSION, config.SERIALIZED_FILE_PARSE_TYPETREE = old
    return result


def observe_peek(module):
    result = {}
    # Distinct roots isolate this suite from all other process-wide cache keys.
    for label, children, payload in [
        ("named", [S.Node("int", "prefix"), S.Node("string", "m_Name"), S.Node("string", "tail")], struct.pack("<i", 7) + S.s32("name") + b"\xff"),
        ("name", [S.Node("string", "name")], S.s32("lower")),
        ("absent", [S.Node("int", "value")], struct.pack("<i", 4)),
        ("first_name", [S.Node("string", "name"), S.Node("string", "m_Name")], S.s32("first") + S.s32("second")),
    ]:
        tree = S.Node("Y3Peek" + label, "Y3Peek" + label, children)
        _, obj = _file(module, tree, payload)
        result[label] = _call(obj.peek_name)
    # Same root key but different layout preserves UnityPy's cached-node bug.
    tree = S.Node("Y3PeekCollision", "Y3PeekCollision", [S.Node("string", "m_Name")])
    _, first = _file(module, tree, S.s32("cached"))
    result["cache_first"] = _call(first.peek_name)
    tree = S.Node("Y3PeekCollision", "Y3PeekCollision", [S.Node("string", "prefix"), S.Node("string", "m_Name")])
    _, second = _file(module, tree, S.s32("prefix-as-name") + S.s32("actual"))
    result["cache_collision"] = _call(second.peek_name)
    return result
