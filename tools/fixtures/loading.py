"""Record core ZIP/split L0-L2 loading answers with UnityPy 1.25.2.

Run in the oracle container: python tools/fixtures/loading.py [--check].
Inputs come from the Rust testkit. For archive splits the reference loads
explicit member concatenations (PARITY section 19.2); the actual UnityPy
ZIP result is also recorded, pinning its cwd-relative split bug. Filesystem
splits are exercised with load_files; no external input files are needed.
"""
from __future__ import annotations

import argparse
import io
import json
import os
import struct
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))
import child  # noqa: E402
import scratch  # noqa: E402
import UnityPy  # noqa: E402

DEST = ROOT / "crates/unityrs-core/tests/fixtures/loading/oracle.json"


def summary(env, prefix=""):
    def key(name):
        return name.removeprefix(prefix)
    return {
        "keys": [key(k) for k, v in env.files.items() if v is not None],
        "cabs": list(env.cabs),
        "object_count": len(env.objects),
        "objects": [{"path_id": obj.path_id, "tree": obj.read_typetree()} for obj in env.objects],
    }


def joined_reference(entries):
    env = UnityPy.Environment()
    seen = set()
    by_name = {e["name"]: bytes.fromhex(e["hex"]) for e in entries}
    for entry in entries:
        name = entry["name"]
        if ".split" in name:
            base = name.rsplit(".split", 1)[0]
            if base in seen:
                continue
            seen.add(base)
            parts = []
            for i in range(999):
                part = by_name.get(f"{base}.split{i}")
                if part is not None:
                    parts.append(part)
                elif i:
                    break
            env.load_file(b"".join(parts), name=base)
        else:
            env.load_file(io.BytesIO(by_name[name]), name=name)
    return summary(env)


def attempt(fn):
    try:
        return fn()
    except Exception as exc:
        return {"error": type(exc).__name__}


def filesystem_cases(asset):
    Path("CAB-P.split0").write_bytes(asset)
    Path("CAB-P.split0.bak").write_bytes(asset)
    Path("dir.split1").mkdir()
    Path("dir.split1/inner").write_bytes(asset)
    Path("CAB-M.split1").write_bytes(asset)
    cases = []
    for name in ("CAB-P.split0.bak", "dir.split1/inner", "CAB-M.split9", "missing.split0", "dir.split1"):
        def load(name=name):
            env = UnityPy.Environment()
            env.load_file(name)
            return summary(env)
        actual = attempt(load)
        if name in ("CAB-P.split0.bak", "dir.split1/inner"):
            env = UnityPy.Environment()
            env.load_file(io.BytesIO(asset), name=name)
            reference = summary(env)
        else:
            reference = actual
        cases.append({"path": name, "oracle": actual, "reference": reference})
    return cases


def review_cases(asset):
    """CPython zipfile acceptance: synthetic inputs, no game data."""
    def make(name="CAB-R", payload=asset, method=zipfile.ZIP_STORED):
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w", compression=method) as z:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = method
            z.writestr(info, payload)
        data = bytearray(buf.getvalue())
        # Match UnityPy's byte detector for nested ZIPs.
        data[4] = 0
        data[data.index(b"PK\x01\x02") + 6] = 0
        return data

    cases = []
    def add(name, data):
        data = bytes(data)
        def load():
            env = UnityPy.Environment()
            env.load_zip_file(data)
            return summary(env)
        cases.append({"name": name, "zip": data.hex(), "oracle": attempt(load)})

    original = make()
    data = make(name="empty", payload=b"")
    center = data.index(b"PK\x01\x02")
    struct.pack_into("<L", data, center+16, 123)
    add("empty-stored-bad-crc", data)
    data = make(name="empty", payload=b"", method=zipfile.ZIP_DEFLATED)
    center = data.index(b"PK\x01\x02")
    struct.pack_into("<L", data, center+16, 123)
    data[35] = 7  # Invalid block: loading never reads a zero-size payload.
    add("empty-deflate-corrupt-payload-bad-crc", data)
    for padding in (1, 2, 3):
        data = original.copy()
        center = data.index(b"PK\x01\x02")
        n = struct.unpack_from("<H", data, 26)[0]
        data[30+n:30+n] = bytes(padding)
        struct.pack_into("<H", data, 28, padding)
        struct.pack_into("<L", data, len(data)-6, center+padding)
        add(f"zipalign-{padding}", data)
    for markers in range(8):
        data = original.copy()
        center = data.index(b"PK\x01\x02")
        fields = bytearray()
        for bit, offset, value in ((1, 24, len(asset)), (2, 20, len(asset)), (4, 42, 0)):
            if markers & bit:
                struct.pack_into("<L", data, center+offset, 0xFFFFFFFF)
                fields.extend(struct.pack("<Q", value))
        extra = struct.pack("<HH", 1, len(fields)) + fields + b"xyz"
        data[len(data)-22:len(data)-22] = extra
        struct.pack_into("<H", data, center+30, len(extra))
        end = len(data)-22
        size = end-center
        struct.pack_into("<L", data, end+12, size)
        record = struct.pack("<4sQ2H2L4Q", b"PK\x06\x06", 44, 45, 45, 0, 0, 1, 1, size, center)
        locator = struct.pack("<4sLQL", b"PK\x06\x07", 0, end, 1)
        data[end:end] = record + locator
        end = len(data)-22
        struct.pack_into("<HLL", data, end+10, 0xFFFF, 0xFFFFFFFF, 0xFFFFFFFF)
        add(f"zip64-{markers}", b"prefix" + data)
    add("selfname", make("CAB-Z", make("CAB-Z")))
    for method in (zipfile.ZIP_BZIP2, zipfile.ZIP_LZMA):
        add(f"method-{method}", make(method=method))
    for name, offset, value in (("local-flags", 6, 0x61), ("local-method", 8, 99),
                                ("local-crc", 14, 42), ("local-size", 22, 0)):
        data = original.copy()
        struct.pack_into("<H", data, offset, value)
        add(name, data)
    data = original.copy()
    end = len(data)-22
    struct.pack_into("<4H", data, end+4, 9, 8, 7, 0)
    struct.pack_into("<H", data, end+20, 999)
    add("eocd-prefix-tail-comment-disks", b"prefix" + data + b"tail")
    data = make(method=zipfile.ZIP_DEFLATED)
    center = data.index(b"PK\x01\x02")
    struct.pack_into("<L", data, center+24, len(asset)+1)
    add("deflate-larger-declared-size", data)
    data = make(method=zipfile.ZIP_DEFLATED)
    center = data.index(b"PK\x01\x02")
    extra = struct.pack("<HHQ", 1, 8, 1 << 40)
    data[center+46+len("CAB-R"):center+46+len("CAB-R")] = extra
    struct.pack_into("<L", data, center+24, 0xFFFFFFFF)
    struct.pack_into("<H", data, center+30, len(extra))
    struct.pack_into("<L", data, len(data)-10, len(data)-22-center)
    add("deflate-terabyte-declared-size", data)
    data = make(payload=b"ab", method=zipfile.ZIP_DEFLATED)
    center = data.index(b"PK\x01\x02")
    start = 30 + struct.unpack_from("<H", data, 26)[0]
    payload = b"\x00\x02\x00\xfd\xffab\x07"
    data[start:center] = payload
    center = start + len(payload)
    struct.pack_into("<LLL", data, center+16, zipfile.crc32(b"a"), len(payload), 1)
    struct.pack_into("<L", data, len(data)-6, center)
    add("deflate-small-declaration-corrupt-read-window", data)
    for name, extras, reloff in (("zip64-extensible-data", b"12345678", None),
                                 ("zip64-reloff-large", b"", 1 << 40),
                                 ("zip64-reloff-three", b"", 3)):
        data = original.copy()
        center = data.index(b"PK\x01\x02")
        end = len(data)-22
        record = struct.pack("<4sQ2H2L4Q", b"PK\x06\x06", 44+len(extras),
                             45, 45, 0, 0, 1, 1, end-center, center)
        locator = struct.pack("<4sLQL", b"PK\x06\x07", 0, end if reloff is None else reloff, 1)
        data[end:end] = record + extras + locator
        add(name, data)
    data = make(method=zipfile.ZIP_DEFLATED)
    center = data.index(b"PK\x01\x02")
    compressed = struct.unpack_from("<L", data, center+20)[0]
    data[center:center] = b"tail"
    struct.pack_into("<L", data, center+4+20, compressed+4)
    struct.pack_into("<L", data, len(data)-6, center+4)
    add("deflate-trailing-data", data)
    data = make(payload=b"a" * 20000, method=zipfile.ZIP_DEFLATED)
    center = data.index(b"PK\x01\x02")
    start = 30 + struct.unpack_from("<H", data, 26)[0]
    payload = b"\x00" + struct.pack("<HH", 20000, 0xFFFF ^ 20000) + b"a" * 20000 + b"\x07"
    data[start:center] = payload
    center = start + len(payload)
    struct.pack_into("<LLL", data, center+16, zipfile.crc32(b"a"), len(payload), 1)
    struct.pack_into("<L", data, len(data)-6, center)
    add("deflate-corrupt-after-retained-prefix", data)
    data = original.copy()
    center = data.index(b"PK\x01\x02")
    row = data[center:len(data)-22]
    data[len(data)-22:len(data)-22] = row
    struct.pack_into("<L", data, len(data)-10, len(row)*2)
    add("duplicate-local-offset", data)
    return cases


def round5_cases(asset):
    """Caller spelling, plain-open boundaries, empty roots and None siblings."""
    from UnityPy.classes.PPtr import PPtr
    from UnityPy.files.SerializedFile import FileIdentifier
    from UnityPy.streams import EndianBinaryReader

    def raw_answers(env):
        prefix = os.getcwd() + "/"
        return {
            "keys": [key.removeprefix(prefix) for key in env.files],
            "data": [file.bytes.decode("utf-8") for file in env.files.values()],
        }

    def load(caller, name):
        if caller == "constructor":
            return UnityPy.Environment(name)
        env = UnityPy.Environment()
        env.load_file(name)
        return env

    out = {}
    old_cwd, old_home = os.getcwd(), os.environ.get("HOME")
    with scratch.folder() as folder:
        try:
            os.chdir(folder)
            os.environ["HOME"] = str(Path.cwd() / "home")
            Path("a.split0").write_bytes(b"abc")
            names = ["file:a.split0", "a.split0"]
            env = UnityPy.Environment()
            env.load_files(names)
            out["split_keys"] = list(env.files)
            out["constructor_split_keys"] = list(UnityPy.Environment(*names).files)

            Path("~").mkdir()
            Path("home").mkdir()
            for name, data in (("plain", b"physical"), ("file:plain", b"literal protocol"),
                               ("~/plain", b"literal tilde"), ("home/plain", b"expanded tilde")):
                Path(name).write_bytes(data)
            env = UnityPy.Environment()
            env.load_files(["file:plain", "~/plain"])
            out["literal_load_files"] = raw_answers(env)
            for caller in ("constructor", "load_file"):
                for name in ("file:plain", "~/plain"):
                    out[f"{caller}:{name}"] = raw_answers(load(caller, name))
            Path("file:plain").unlink()
            for caller in ("constructor", "load_file"):
                out[f"{caller}_missing_literal"] = raw_answers(load(caller, "file:plain"))

            # A fresh cwd makes every root spelling's absolute walk identical.
            Path("root").mkdir()
            os.chdir("root")
            Path("plain").write_bytes(b"current directory")
            for root in ("", "file:", "file://", "local:", "local://"):
                for caller in ("folder_root", "constructor_root"):
                    if caller == "constructor_root":
                        env = UnityPy.Environment(root)
                    else:
                        env = UnityPy.Environment()
                        env.load_folder(root)
                    out[f"{caller}:{root}"] = raw_answers(env)

            buf = io.BytesIO()
            with zipfile.ZipFile(buf, "w") as archive:
                archive.writestr("member", b"abc")
            nested = bytearray(buf.getvalue())
            nested[4] = 0  # Match the byte detector, as the Rust archive does.
            nested[nested.index(b"PK\x01\x02") + 6] = 0
            for old_key, name in (("CAB-X", "CAB-X"), ("dir/CAB-X", "CAB-X"), ("CAB-X", "cab-x")):
                env = UnityPy.Environment()
                caller = env.load_file(io.BytesIO(asset), name="caller")
                external = b"\0" * 21 + b"CAB-X\0"
                caller.externals.append(FileIdentifier(caller.header, EndianBinaryReader(external, endian="<")))
                env.load_file(io.BytesIO(asset), name=old_key)
                pointer = PPtr(m_FileID=1, m_PathID=7, assetsfile=caller)
                before = pointer.deref().path_id
                env.load_file(bytes(nested), name=name)
                out[f"none:{old_key}:{name}"] = {
                    "before": before, "after": attempt(lambda: pointer.deref().path_id),
                }
        finally:
            os.chdir(old_cwd)
            if old_home is None:
                os.environ.pop("HOME", None)
            else:
                os.environ["HOME"] = old_home
    return out


def round6_cases():
    """fsspec trailing-slash opens and lexical walk-root key spellings."""
    def raw_answers(env):
        prefix = os.getcwd() + "/"
        return {
            "keys": [key.removeprefix(prefix) for key in env.files],
            "data": [file.bytes.decode("utf-8") for file in env.files.values()],
        }

    out = {}
    old_cwd, old_home = os.getcwd(), os.environ.get("HOME")
    with scratch.folder() as folder:
        try:
            os.chdir(folder)
            Path("home").mkdir()
            os.environ["HOME"] = str(Path.cwd() / "home")
            buf = io.BytesIO()
            with zipfile.ZipFile(buf, "w") as archive:
                archive.writestr("member", b"zip data")
            Path("x.zip").write_bytes(buf.getvalue())
            Path("home/x.zip").write_bytes(buf.getvalue())
            for name in ("x.zip/", "x.zip///", "file:x.zip/", "local://x.zip///",
                         "~/x.zip/", "file:~/x.zip///"):
                env = UnityPy.Environment()
                env.load_zip_file(name)
                out[f"zip:{name}"] = raw_answers(env)

            Path("a/b/child").mkdir(parents=True)
            Path("a/b/x").write_bytes(b"root file")
            Path("a/b/child/y").write_bytes(b"nested file")
            for root in ("a//b", "a/./b", "a/b/", "./a//b", "././a/b",
                         "a//b///", "a/b/../b"):
                for absolute in (False, True):
                    # pathlib would discard the spelling this case pins.
                    supplied = f"{os.getcwd()}/{root}" if absolute else root
                    label = f"absolute:{root}" if absolute else root
                    for caller in ("folder", "constructor"):
                        if caller == "constructor":
                            env = UnityPy.Environment(supplied)
                        else:
                            env = UnityPy.Environment()
                            env.load_folder(supplied)
                        out[f"{caller}:{label}"] = raw_answers(env)
        finally:
            os.chdir(old_cwd)
            if old_home is None:
                os.environ.pop("HOME", None)
            else:
                os.environ["HOME"] = old_home
    return out


def record(inputs):
    assert UnityPy.__version__ == "1.25.2", UnityPy.__version__
    out = {"generator": "tools/fixtures/loading.py", "unitypy": UnityPy.__version__, "inputs": inputs, "scenes": []}
    # Keep cwd empty so UnityPy's ZIP split bug is deterministic.
    with scratch.folder() as folder:
        old = os.getcwd()
        try:
            os.chdir(folder)
            for scene in inputs["scenes"]:
                env = UnityPy.Environment()
                env.load_zip_file(bytes.fromhex(scene["zip"]))
                actual = summary(env)
                reference = joined_reference(scene["entries"])
                out["scenes"].append({"name": scene["name"], "zip": actual, "reference": reference})
            asset = bytes.fromhex(inputs["asset"])
            half = len(asset) // 2
            Path("CAB-S.split0").write_bytes(asset[:half])
            Path("CAB-S.split1").write_bytes(asset[half:])
            env = UnityPy.Environment()
            env.load_files(["CAB-S.split1", "CAB-S.split0"])
            out["filesystem_split"] = summary(env)
            out["filesystem_cases"] = filesystem_cases(asset)
            out["review_cases"] = review_cases(asset)
            out["round5"] = round5_cases(asset)
            out["round6"] = round6_cases()
            # Explicit load_zip_file opens a path directly, including a
            # split-looking filename holding an independent archive.
            archives = []
            for i, name in enumerate(("CAB-First", "CAB-Second")):
                buf = io.BytesIO()
                with zipfile.ZipFile(buf, "w") as z:
                    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                    z.writestr(info, asset)
                archives.append(buf.getvalue())
                Path(f"x.split{i}").write_bytes(archives[-1])
            env = UnityPy.Environment()
            env.load_zip_file("x.split0")
            out["explicit_zip_split"] = {"parts": [part.hex() for part in archives], "oracle": summary(env)}
            out["missing_paths"] = []
            for caller in ("constructor", "load_file", "load_files"):
                for name in ("missing", "missing.split9", "CAB-P", "CAB-P.split9"):
                    def load(caller=caller, name=name):
                        if caller == "constructor":
                            env = UnityPy.Environment(name)
                        else:
                            env = UnityPy.Environment()
                            if caller == "load_files":
                                env.load_files([name])
                            else:
                                env.load_file(name)
                        return summary(env)
                    out["missing_paths"].append({"caller": caller, "path": name, "oracle": attempt(load)})
        finally:
            os.chdir(old)
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    inputs = json.loads(child.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "unityrs-core", "--example", "loading_fixtures"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout)
    output = json.dumps(record(inputs), ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    if args.check:
        if not DEST.is_file() or DEST.read_text(encoding="utf-8") != output:
            raise SystemExit("loading oracle JSON is missing or stale; run tools/fixtures/loading.py")
    else:
        DEST.parent.mkdir(parents=True, exist_ok=True)
        DEST.write_text(output, encoding="utf-8")
        print(DEST.relative_to(ROOT))


if __name__ == "__main__":
    main()
