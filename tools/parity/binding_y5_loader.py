"""Synthetic remaining loader observations; never record runtime schemas."""
import contextlib
import io
import os
from pathlib import Path

import binding_cases as B
import scratch


def _data():
    return next(data for name, data in B.cases() if name == "text")


def _call(function):
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            return function()
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}


def _state(env):
    return {"keys": list(env.files), "objects": [obj.path_id for obj in env.objects],
            "cabs": list(env.cabs), "path": "<cwd>" if env.path == os.getcwd() else env.path}


def _clean(value, root):
    return B.replace_string(value, str(root), "<root>")


def observe_paths(module):
    with scratch.folder("binding-y5-paths-") as directory:
        root = Path(directory)
        (root / "one.assets").write_bytes(_data())
        (root / "raw.bin").write_bytes(b"raw")
        folder = root / "folder"
        folder.mkdir()
        (folder / "sub").mkdir()
        (folder / "a.assets").write_bytes(_data())
        (folder / "sub" / "b.assets").write_bytes(_data())
        result = {}
        for name, args in (("file", (str(root / "one.assets"),)),
                           ("directory", (str(folder),)),
                           ("multiple", (str(folder), str(root / "one.assets"))),
                           ("missing", (str(root / "missing"),)),
                           ("pathlike", (root / "one.assets",))):
            result[name] = _call(lambda: _state(module.Environment(*args)))
        for name, method in (("load", "load"), ("load_files", "load_files"),
                             ("load_folder", "load_folder")):
            env = module.Environment(path=str(root))
            argument = str(folder) if method == "load_folder" else [str(root / "one.assets")]
            returned = getattr(env, method)(argument)
            result[name] = [returned, _state(env)]
        env = module.Environment(path=str(root))
        result["load_assets"] = env.load_assets(["first", "second"], lambda _: io.BytesIO(_data()))
        result["load_assets_state"] = _state(env)
        env = module.Environment(path=str(root))
        result["missing_load"] = env.load_file("absent")
        result["default_fields"] = [env.local_files, env.local_files_simple,
                                    env.typetree_generator, env.get("unknown", 13)]
        return _clean(result, root)


def observe_filesystem(module):
    from fsspec.implementations.memory import MemoryFileSystem
    with scratch.folder("binding-y5-fs-") as directory:
        root = Path(directory)
        fs = MemoryFileSystem(skip_instance_cache=True)
        prefix = str(root / "remote")
        fs.pipe(prefix + "/one.assets", _data())
        fs.pipe(prefix + "/joined.assets.split0", _data()[:40])
        fs.pipe(prefix + "/joined.assets.split1", _data()[40:])
        result = {}
        for name in ("one.assets", "joined.assets.split0"):
            result[name] = _call(lambda: _state(module.Environment(prefix + "/" + name, fs=fs)))
        env = module.Environment(fs=fs, path=str(root))
        result["identity"] = env.fs is fs
        result["missing_local"] = _call(lambda: env.load_file(prefix + "/one.assets"))
        env = module.Environment(fs=fs)
        result["default"] = env.path
        result["load_missing_path"] = _call(lambda: env.load_file(prefix + "/one.assets"))
        result["folder"] = _call(lambda: _state(module.Environment(prefix, fs=fs)))
        result["split_direct"] = _call(lambda: _state(module.Environment(
            fs=fs, path=str(root))))
        env = module.Environment(fs=fs, path=str(root))
        env.load_file(prefix + "/joined.assets.split0")
        result["split_state"] = _state(env)
        result["split_bytes"] = env._load_split_file(prefix + "/joined.assets").hex()
        return _clean(result, root)


def observe_streams(module):
    result = []
    for payload in (_data(), b"raw", B.S.web_file([(b"raw", b"raw")])):
        for name in (7, None, b"raw.bin", "a\\b.assets"):
            for explicit in (None, "override"):
                stream = io.BytesIO(payload)
                stream.name = name
                env = module.Environment()
                def load():
                    item = env.load_file(stream, name=explicit)
                    return [B.typed(list(env.files)), item.name if hasattr(item, "name") else None,
                            B.typed(list(env.cabs))]
                result.append(_call(load))
    class ReadOnly:
        def read(self, size=-1):
            return b"raw"
    result.append(_call(lambda: module.Environment().load_file(ReadOnly(), name="input")))
    return result


def observe_splits(module):
    with scratch.folder("binding-y5-splits-") as directory:
        root = Path(directory)
        data = _data()
        base = str(root / "joined.assets")
        Path(base + ".split0").write_bytes(data[:40])
        Path(base + ".split1").write_bytes(data[40:])
        result = {}
        for suffix in (".split0", ".split1", ".split0.tail"):
            env = module.Environment(path=str(root))
            env.load_file(base + suffix)
            result[suffix] = _state(env)
        env = module.Environment(path=str(root))
        env.load_files([base + ".split1", base + ".split0"])
        result["dedup"] = _state(env)
        env = module.Environment(path=str(root))
        env.load_file(str(root / "missing.split0"))
        result["missing"] = _state(env)
        return _clean(result, root)


def observe_lookup(module):
    with scratch.folder("binding-y5-lookup-") as directory:
        root = Path(directory)
        folder = root / "sub"
        folder.mkdir()
        (folder / "Input.assets").write_bytes(_data())
        env = module.Environment(path=str(root))
        result = {}
        result["found"] = _call(lambda: env.find_file("Input.assets").name)
        result["state"] = _state(env)
        result["lists"] = [env.local_files, env.local_files_simple]
        result["cached"] = env.find_file("Input.assets") is env.get_cab("input.assets")
        result["missing"] = _call(lambda: env.find_file("absent"))
        return _clean(result, root)
