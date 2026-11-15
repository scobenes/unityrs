"""Shared, extraction-free verifier for the encoder source distributions.

Manifests use SHA-256, two spaces, then the source distribution member name.
The archive itself has one entry too. All files under the vendored source
directory must appear exactly once; separate upstream notices are pinned in
their own manifests because they are not source distribution members.
"""
from __future__ import annotations

import hashlib
import io
import re
import sys
import tarfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
VENDOR = Path("crates/unityrs-texture/vendor")


def manifest(text: str) -> dict[str, str]:
    entries = {}
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        sha, name = line.split("  ", 1)
        path = PurePosixPath(name)
        if not re.fullmatch(r"[a-f0-9]{64}", sha) or path.is_absolute() or ".." in path.parts:
            raise ValueError(f"invalid manifest entry: {line}")
        if name in entries:
            raise ValueError(f"duplicate manifest entry: {name}")
        entries[name] = sha
    return entries


def problems(data: bytes, archive: str, prefix: str, directory: str, root: Path = ROOT) -> list[str]:
    vendor = root / VENDOR
    entries = manifest((vendor / (directory + ".sha256")).read_text(encoding="utf-8"))
    wanted = entries.pop(archive, None)
    out = []
    if wanted is None or hashlib.sha256(data).hexdigest() != wanted:
        out.append(f"{archive}: archive SHA-256 differs from the manifest")
    listed = {name for name in entries if name.startswith(prefix)}
    if not listed:
        out.append(f"{directory}: manifest has no source files")
    actual = {prefix + p.relative_to(vendor / directory).as_posix()
              for p in (vendor / directory).rglob("*") if p.is_file() or p.is_symlink()}
    for name in sorted(actual - listed):
        out.append(f"{name}: vendored file is not in the manifest")
    notices = vendor / (directory + ".notices.sha256")
    if notices.exists():
        for name, sha in manifest(notices.read_text(encoding="utf-8")).items():
            path = vendor.parent / name
            if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != sha:
                out.append(f"{name}: upstream notice SHA-256 differs from its separate manifest")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        for name, sha in sorted(entries.items()):
            if not name.startswith(prefix):
                out.append(f"{name}: outside {prefix}")
                continue
            matches = [member for member in tar.getmembers() if member.name == name]
            if len(matches) != 1 or not matches[0].isfile():
                out.append(f"{name}: archive must contain exactly one regular member")
                continue
            source = tar.extractfile(matches[0]).read()
            if hashlib.sha256(source).hexdigest() != sha:
                out.append(f"{name}: archive member SHA-256 differs from the manifest")
            path = vendor / directory / name.removeprefix(prefix)
            if path.is_symlink() or not path.is_file() or path.read_bytes() != source:
                out.append(f"{name}: vendored bytes differ from the archive member")
    return out


def main(archive: str, prefix: str, directory: str, argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 2 or args[0] != "verify":
        print(f"usage: {Path(sys.argv[0]).name} verify {archive}", file=sys.stderr)
        return 2
    try:
        found = problems(Path(args[1]).read_bytes(), archive, prefix, directory)
    except (OSError, ValueError, tarfile.TarError) as error:
        found = [str(error)]
    for problem in found:
        print(f"{Path(sys.argv[0]).stem}: {problem}", file=sys.stderr)
    return int(bool(found))
