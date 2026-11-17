"""Synthetic ImportHelper.parse_file observations; no runtime schema data.

UnityPy 1.25.2 helpers/ImportHelper.py:127-156 selects an explicitly supplied
parser without detection and leaves raw readers unchanged.
"""
import contextlib
import gzip
import io

import binding_cases as B


def _record(module, payload, typ, name="forced", dependency=False, stream=False, position=0, endian=">", detached=False):
    helper = module.helpers.ImportHelper
    reader = module.streams.EndianBinaryReader(io.BytesIO(payload) if stream else payload, endian=endian)
    reader.Position = position
    env = module.Environment()
    output = io.StringIO()
    try:
        with contextlib.redirect_stdout(output):
            result = helper.parse_file(reader, None if detached else env, name, typ=typ,
                                       is_dependency=dependency)
        value = {"class": type(result).__name__, "identity": result is reader,
                 "name": getattr(result, "name", None),
                 "parent": getattr(result, "parent", None) is env,
                 "dependency": getattr(result, "is_dependency", None)}
        if isinstance(getattr(result, "objects", None), dict):
            value["objects"] = list(result.objects)
        if isinstance(getattr(result, "files", None), dict):
            value["files"] = list(result.files)
        if detached:
            pending = [result]
            environments = []
            while pending:
                file = pending.pop()
                if isinstance(file, module.files.File):
                    environments.append(file.environment is None)
                    if isinstance(file, (module.files.BundleFile, module.files.WebFile)):
                        pending.extend(file.files.values())
            value["detached_environments"] = environments
    except Exception as error:
        value = {"error": type(error).__name__, "message": str(error),
                 "position": reader.Position}
    value.update(environment_files=list(env.files), cabs=list(env.cabs),
                 stdout=output.getvalue())
    return value


def observe_forced_raw(module):
    result = []
    payloads = [data for name, data in B.cases() if name in ("text", "bundle", "resource")]
    payloads += [B.S.web_file([]), b"A" * 20]
    for payload in payloads:
        for typ in (module.enums.FileType.ResourceFile, module.enums.FileType.ZIP, 123):
            result.append(_record(module, payload, typ))
    for suffix in (".resS", ".resource", ".config", ".xml", ".dat"):
        result.append(_record(module, b"A" * 20, module.enums.FileType.AssetsFile,
                              name="forced" + suffix))
    return result


def observe_forced_parsers(module):
    # Both small inputs are ResourceFile according to detection, while their
    # explicitly chosen parsers accept the complete synthetic metadata.
    minimal = B.S.serialized_file([], [])[:85]
    web = B.S.web_file([], signature=b"UnityWebData")
    result = {}
    for stream in (False, True):
        for dependency in (False, True):
            key = str((stream, dependency))
            result["serialized" + key] = _record(
                module, minimal, module.enums.FileType.AssetsFile,
                name="dir/forced.assets", dependency=dependency, stream=stream)
            result["web" + key] = _record(
                module, web, module.enums.FileType.WebFile,
                name="dir/forced.web", dependency=dependency, stream=stream)
    payload = B.S.fs_bundle([(b"child", b"raw", 0)])
    result["bundle"] = _record(module, payload, module.enums.FileType.BundleFile)
    result["normal"] = _record(module, payload, None)
    assets = B.S.serialized_file([], [])
    nested = B.S.fs_bundle([(b"child", assets, 4),
                           (b"web", B.S.web_file([(b"nested", assets)]), 0)])
    result["detached"] = _record(module, nested, module.enums.FileType.BundleFile, detached=True)
    return result


def observe_forced_failures(module):
    result = []
    assets = B.S.serialized_file([], [])
    for stream in (False, True):
        result += [_record(module, b"raw", typ, stream=stream) for typ in
                   (module.enums.FileType.AssetsFile, module.enums.FileType.BundleFile,
                    module.enums.FileType.WebFile)]
        result += [_record(module, assets, module.enums.FileType.BundleFile, stream=stream),
                   _record(module, assets, module.enums.FileType.WebFile, stream=stream),
                   _record(module, B.S.fs_bundle([]), module.enums.FileType.WebFile, stream=stream)]
        result.append(_record(module, gzip.compress(b"invalid\0" + b"\0" * 64, mtime=0),
                              module.enums.FileType.WebFile, stream=stream))
    return result


def observe_forced_positions(module, *, corrected=False):
    """Keep the oracle answers beside the bounded constructor-state refusal.

    Assets EOF is safe: no count or malformed metadata reaches UnityPyBoost.
    The core constructors start at zero, so accepting another position would
    silently parse different bytes. PARITY section 19.3 records the named refusal.
    """
    assets = B.S.serialized_file([], [])
    cases = [(assets, module.enums.FileType.AssetsFile, len(assets), ">"),
             (B.S.fs_bundle([]), module.enums.FileType.BundleFile, 1, ">"),
             (B.S.web_file([]), module.enums.FileType.WebFile, 1, ">"),
             (b"raw", module.enums.FileType.AssetsFile, 0, "<"),
             (b"raw", module.enums.FileType.BundleFile, 0, "<"),
             (B.S.web_file([]), module.enums.FileType.WebFile, 0, "<")]
    result = []
    for payload, typ, position, endian in cases:
        message = ("parse_file nonzero parser position is not exposed" if position else
                   "parse_file non-default parser endian is not exposed" if
                   typ in (module.enums.FileType.AssetsFile, module.enums.FileType.BundleFile)
                   and endian != ">" else None)
        if corrected and module.__name__ == "UnityPy" and message:
            result.append({"error": "NotImplementedError", "message": message, "position": 0,
                           "environment_files": [], "cabs": [],
                           "stdout": f"Error parsing file 'forced' as {typ}: {message}\n"})
        else:
            result.append(_record(module, payload, typ, position=position, endian=endian))
    return result
