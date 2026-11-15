"""Record writable CAB semantics and explicit Tier C corrections (UnityPy 1.25.2).

Run in the pinned oracle container: python tools/fixtures/writeable_cab.py [--check].
File.get_writeable_cab and SerializedFile.get_writeable_cab are exercised
directly. The latter's slotted __class__ assignment raises TypeError; its
corrected oracle constructs FileIdentifier without that assignment. WebFile's
save calls an absent writer.save; its corrected oracle uses writer.bytes.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
import uuid
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))
import synth as S  # noqa: E402
from serialized_save import patched_function  # noqa: E402

DEST = ROOT / "crates/unityrs-core/tests/fixtures/writeable_cab.json"
FIXED_UUID = uuid.UUID("00000000-0000-1000-8000-001122334455")


def load(data):
    import UnityPy
    env = UnityPy.load(data)
    return env, next(iter(env.files.values()))


def record():
    import UnityPy
    from UnityPy.files.SerializedFile import SerializedFile
    from UnityPy.files.WebFile import WebFile
    assert UnityPy.__version__ == "1.25.2"
    sf_correction = patched_function(SerializedFile.get_writeable_cab,
        "file_identifier = FileIdentifierFake()", "file_identifier = object.__new__(FileIdentifier)",
        [("file_identifier.__class__ = FileIdentifier", "pass")])
    web_correction = patched_function(WebFile.save, "isinstance(f, EndianBinaryReader)",
        "isinstance(f, (EndianBinaryReader, EndianBinaryWriter))")
    # Keep the saved empty-object asset above the detector's minimum size.
    asset = S.serialized_file([], [], enable_type_tree=False, user_information=b"synthetic-writable-cab" * 8)
    result = {"reference": "UnityPy 1.25.2", "guid": FIXED_UUID.urn[-16:].encode().hex(), "cases": []}
    for kind in ("bundle", "web"):
        for resource, asset_name in ((False, "CAB-a"), (True, "CAB-a"), (False, "dir/CAB-a")):
            entries = [(asset_name.encode(), asset, 4)] + ([(b"old.resS", b"old", 7)] if resource else [])
            data = S.fs_bundle(entries, codec=0) if kind == "bundle" else S.web_file([(n, d) for n,d,_ in entries])
            for caller in ("container", "serialized"):
                for name in (None, "new.resS", ""):
                    env, parent = load(data)
                    file = parent.files[asset_name]
                    target = parent if caller == "container" else file
                    before = sorted(env.cabs)
                    actual_error = None
                    if caller == "serialized":
                        try:
                            target.get_writeable_cab() if name is None else target.get_writeable_cab(name)
                        except TypeError:
                            actual_error = "TypeError"
                        assert actual_error == "TypeError"
                        assert file.externals == []
                        env, parent = load(data)
                        file = parent.files[asset_name]
                        target = file
                    with mock.patch.object(SerializedFile, "get_writeable_cab", sf_correction), mock.patch("uuid.uuid1", return_value=FIXED_UUID):
                        writer = target.get_writeable_cab() if name is None else target.get_writeable_cab(name)
                        again = target.get_writeable_cab() if name is None else target.get_writeable_cab(name)
                        assert again is writer
                    assert sorted(env.cabs) == before
                    assert not parent.is_changed and not file.is_changed
                    writer.write_bytes(b"abc")
                    writer.write_int(0x11223344)
                    if caller == "serialized":
                        file.mark_changed()
                        assert file.is_changed and parent.is_changed
                        assert len(file.externals) == 1
                    web_error = None
                    try:
                        saved = parent.save(packer="none")
                    except AttributeError:
                        assert kind == "web"
                        web_error = "AttributeError"
                        with mock.patch.object(WebFile, "save", web_correction):
                            saved = parent.save(packer="none")
                    reloaded_env, reloaded = load(saved)
                    key = writer.name
                    assert key in reloaded.files and key.lower() in reloaded_env.cabs
                    case = {"kind": kind, "resource": resource, "caller": caller, "name": name,
                        "input": data.hex(), "output": saved.hex(), "key": key, "flags": writer.flags,
                        "endian": writer.endian, "payload": writer.bytes.hex(), "actual_error": actual_error,
                        "web_error": web_error, "keys": list(reloaded.files), "cabs_before": before,
                        "cabs_after": sorted(reloaded_env.cabs)}
                    if caller == "serialized":
                        case["path"] = writer.path
                    result["cases"].append(case)
            env, parent = load(data)
            try:
                parent.get_writeable_cab(asset_name)
            except ValueError:
                result["cases"].append({"kind": kind, "input": data.hex(), "conflict": True})
            else:
                raise AssertionError("reader key must refuse")
    _, parent = load(S.web_file([(b"CAB-a", asset)]))
    file = parent.files["CAB-a"]
    try:
        file.get_writeable_cab("valid\0name.resS")
    except TypeError:
        pass
    else:
        raise AssertionError("slotted external registration must raise")
    assert file.externals == []
    assert "valid\0name.resS" in parent.files
    with mock.patch.object(SerializedFile, "get_writeable_cab", sf_correction), mock.patch("uuid.uuid1", return_value=FIXED_UUID):
        writer = file.get_writeable_cab("valid\0name.resS")
    assert writer.path == "archive:/CAB-a/valid\0name.resS"
    result["nul_path"] = {"actual_error": "TypeError", "corrected_path": writer.path}
    env, top = load(asset)
    assert top.get_writeable_cab() is None
    result["top"] = asset.hex()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    result = record()
    output = json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n"
    assert len(output) < 1_500_000
    if args.check:
        if not DEST.is_file() or DEST.read_text() != output:
            raise SystemExit("writable CAB fixture is stale")
    else:
        DEST.write_text(output)
    print(f"writable CAB oracle: {len(result['cases'])} cases checked")


if __name__ == "__main__":
    main()
