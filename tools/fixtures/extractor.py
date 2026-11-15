"""Record L9 extractor trees from embedded-tree synthetic files.

Pinned UnityPy 1.25.2: original exception and partial tree beside the
PARITY 19.2 corrected tree and complete ordered reports. Paths are hex
filesystem bytes; trees contain lengths and SHA-256, never outputs.
--check regenerates byte for byte. --inputs reads extract_inputs JSON from
stdin, useful when the Rust example was built on a different architecture.
"""
import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))


def tree(root):
    return {os.fsencode(p.relative_to(root)).hex(): {"size": p.stat().st_size,
            "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
            for p in sorted(root.rglob("*")) if p.is_file()}


def corrected(env, dst, options):
    """Independent corrected-oracle interpretation of the 19.2 rows.

    Keep original per-type exporters wherever they write; fix reader versus
    instance dispatch, None ids inside crawls, zero images and catch/continue.
    """
    from UnityPy.tools import extractor as E
    from UnityPy.enums import ClassIDType
    from unittest.mock import patch
    import builtins
    exported, failures, written, failed = [], [], [], []
    dst.mkdir(parents=True)
    priorities = list(E.EXPORT_TYPES)
    accept = lambda obj: not options.get("asset_filter") or obj.type.value in options["asset_filter"]
    priority = lambda obj: priorities.index(obj.type) if obj.type in priorities else 999

    def identity(reader):
        return {"serialized_file": os.path.basename(reader.assets_file.name),
                "path_id": reader.path_id, "class_id": reader.type.value}

    def failure(reader, fp):
        failures.append(reader.path_id)
        failed.append({**identity(reader), "path": os.fsencode(fp.relative_to(dst)).hex(),
                       "reason": "mesh"})

    def write(reader, fp, *args, **kwargs):
        if reader.type == ClassIDType.GameObject:
            return write_impl(reader, fp, *args, **kwargs)
        opened = []
        original_open = builtins.open

        def observe(file, mode="r", *args, **kwargs):
            result = original_open(file, mode, *args, **kwargs)
            if "w" in mode and isinstance(file, (str, bytes, os.PathLike)):
                opened.append(Path(file))
            return result

        with patch("builtins.open", observe):
            result = write_impl(reader, fp, *args, **kwargs)
        for path in opened:
            if path.is_file():
                written.append({**identity(reader),
                                "path": os.fsencode(path.relative_to(dst)).hex(),
                                "size": path.stat().st_size})
        return result

    def write_impl(reader, fp, append_name=False, append_id=False, crawl_id=False, unknown=False):
        kind = reader.type
        if kind not in E.EXPORT_TYPES and not unknown:
            return []
        try:
            instance = reader.parse_as_object()
        except Exception:
            if kind != ClassIDType.MonoBehaviour and not unknown:
                raise
            instance = None
        if append_name:
            fp = os.path.join(str(fp), getattr(instance, "m_Name", kind.name))
        stem, extension = os.path.splitext(str(fp))
        if append_id:
            stem += f"_{reader.path_id}" if crawl_id else "_None"
        fp = Path(stem)
        if kind == ClassIDType.GameObject:
            return crawl(reader, Path(fp))
        if kind in (ClassIDType.Sprite, ClassIDType.Texture2D):
            if kind == ClassIDType.Texture2D and not instance.m_Width:
                return [(reader.assets_file, reader.path_id)]
            image = instance.image
            extension = extension or ".png"
            filename = f"{fp}{extension}"
            try:
                image.save(filename)
                if extension.lower() == ".ico" and min(image.size) < 16:
                    Path(filename).unlink()
                    image.save(filename + ".png")
            except (ValueError, OSError, KeyError, SystemError):
                # Pillow may have opened a leaf before raising.
                Path(filename).unlink(missing_ok=True)
                if image.width and image.height:
                    image.save(filename + ".png")
            result = [(reader.assets_file, reader.path_id)]
            if kind == ClassIDType.Sprite:
                result.append((instance.m_RD.texture.assetsfile, instance.m_RD.texture.path_id))
            return result
        if kind == ClassIDType.Mesh and instance.export() is False:
            return [(reader.assets_file, reader.path_id)]
        exporter = E.EXPORT_TYPES.get(kind, E.exportMonoBehaviour)
        exporter(reader, str(fp), extension)
        return [(reader.assets_file, reader.path_id)]

    def refs(value):
        if isinstance(value, dict):
            if "m_FileID" in value and "m_PathID" in value:
                if value["m_PathID"]:
                    yield value
            else:
                for child in value.values():
                    yield from refs(child)
        elif isinstance(value, (tuple, list)):
            for child in value:
                yield from refs(child)

    def crawl(root, fp):
        from UnityPy.classes import PPtr
        seen, result, pending = set(), [], [root]
        fp.mkdir(parents=True, exist_ok=True)
        while pending:
            reader = pending.pop()
            key = (reader.assets_file, reader.path_id)
            if key in seen:
                continue
            seen.add(key)
            if reader.type == ClassIDType.GameObject:
                result.append(key)
            elif key not in result:
                try:
                    result.extend(write(reader, fp, True, True, True))
                except Exception:
                    failure(reader, fp)
            try:
                pointers = list(refs(reader.parse_as_dict()))
            except Exception:
                continue
            for pointer in reversed(pointers):
                pending.append(PPtr(assetsfile=reader.assets_file, **pointer).deref())
        return result

    if options.get("use_container", True):
        entries = sorted(env.container.items(), key=lambda entry: priority(entry[1]))
        for name, pointer in entries:
            reader = pointer.deref()
            if not accept(reader):
                continue
            dest = dst.joinpath(*(s for s in name.split("/")[options.get("ignore_first_container_dirs", 0):] if s))
            dest.parent.mkdir(parents=True, exist_ok=True)
            try:
                exported.extend(write(reader, dest, append_id=options.get("append_path_id", False),
                                      unknown=options.get("export_unknown_as_typetree", False)))
            except Exception:
                failure(reader, dest)
    else:
        for reader in sorted(env.objects, key=priority):
            if not accept(reader) or (reader.assets_file, reader.path_id) in exported:
                continue
            try:
                exported.extend(write(reader, dst, True, options.get("append_path_id", False),
                                      unknown=options.get("export_unknown_as_typetree", False)))
            except Exception:
                failure(reader, dst)
    identities = [{"serialized_file": os.path.basename(file.name), "path_id": path_id}
                  for file, path_id in exported]
    return failures, [path_id for _, path_id in exported], {
        "written": written, "failures": failed, "exported": identities}


def record(case):
    import UnityPy
    from UnityPy.tools import extractor
    import scratch
    source = bytes.fromhex(case["input"])
    options = dict(case["options"])
    classes = options.pop("asset_filter", None)
    if classes:
        options["asset_filter"] = lambda obj: getattr(obj, "object_reader", obj).type.value in classes
    with scratch.folder("extractor-oracle-") as root:
        dst = root / "original"
        if not options.get("use_container", True) and case["case"] != "missing_dst":
            dst.mkdir()
        try:
            result = extractor.extract_assets(source, dst, **options)
            original = {"exported": [path_id for _, path_id in result]}
        except Exception as error:
            original = {"exception": type(error).__name__}
        original["tree"] = tree(dst)
        fixed = root / "corrected"
        env = UnityPy.Environment()
        env.load_file(source, name="synthetic.assets")
        failures, exported, report = corrected(env, fixed, case["options"])
        expected = {"tree": tree(fixed), "failures": failures, "exported": exported,
                    "report": report}
    return {**case, "unitypy": original, "expected": expected}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--inputs", action="store_true")
    args = parser.parse_args()
    import UnityPy
    assert UnityPy.__version__ == "1.25.2"
    if args.inputs:
        cases = json.load(sys.stdin)
    else:
        import child
        proc = child.run(["cargo", "run", "--quiet", "--locked", "-p", "unityrs-export",
                          "--example", "extract_inputs"], cwd=ROOT, check=True,
                         capture_output=True, text=True)
        cases = json.loads(proc.stdout)
    shader = json.loads((ROOT / "crates/unityrs-export/tests/fixtures/shader.json").read_text())
    shader = next(case for case in shader["cases"] if case["case"] == "version_5_5_0")
    cases.append({"case": "shader", "input": shader["input"], "options": {"use_container": False}})
    result = {"reference": "UnityPy 1.25.2", "level": "L9", "cases": [record(case) for case in cases]}
    body = ('{\n  "reference": "UnityPy 1.25.2", "level": "L9",\n  "cases": [\n'
            + ",\n".join("    " + json.dumps(case, ensure_ascii=False) for case in result["cases"])
            + "\n  ]\n}\n")
    out = ROOT / "crates/unityrs-export/tests/fixtures/extractor.json"
    if args.check:
        if not out.exists() or out.read_bytes() != body.encode():
            parser.exit(1, "stale extractor.json; regenerate in the oracle\n")
    else:
        out.write_bytes(body.encode())
    return 0


if __name__ == "__main__":
    sys.exit(main())
