"""The UnityPy side of the LZMA spike (docs/spikes/lzma.md).

    sets    write the bundle lists all.txt, largest20.txt and sample200.txt
    synth   write synthetic bundles, built here from no game data, and their lists
    oracle  UnityPy's own decompression of every LZMA block: path, block, then
            length and sha256, or "error" and the exception UnityPy raised
    bench   time UnityPy's CompressionHelper.decompress_lzma in 1 process or N
    report  median MB/s per implementation, from the TSVs both benches write
    crates  recent releases and top reverse dependencies of both crates, from crates.io

Run it in the oracle container (scripts/dev), where UnityPy 1.25.2 is pinned.
Every block is captured through UnityPy's own BundleFile reader, so the bytes
timed and hashed here are the bytes UnityPy itself hands to liblzma.
"""

import argparse
import ctypes
import hashlib
import importlib
import json
import lzma
import multiprocessing
import random
import statistics
import struct
import sys
import time
import urllib.request
from collections import defaultdict
from pathlib import Path

import lz4.block
import UnityPy
from UnityPy.helpers import CompressionHelper

# The module, not the class that UnityPy.files re-exports under the same name.
bundle_module = importlib.import_module("UnityPy.files.BundleFile")

SEED = 20261028
# Lower bounds, largest first, for the per-block report.
SIZE_BINS = ((1_000_000, ">= 1 MB"), (64_000, "64 kB-1 MB"), (0, "< 64 kB"))
LARGEST = "largest block"
# The implementation the report compares the others with.
BASELINE = "liblzma"
LZMA = 1
COMPRESSION_MASK = 0x3F
DICT = 1 << 19


def capture(path):
    """UnityPy-loads one bundle and returns its LZMA data blocks as
    (block index, compressed bytes, uncompressed size, UnityPy's output).
    UnityPy stops at the first block it cannot decompress; that block's
    output is the exception it raised."""
    calls = []
    original = bundle_module.BundleFile.decompress_data

    def recording(self, data, size, flags, index=0):
        try:
            out = original(self, data, size, flags, index)
        except Exception as e:
            calls.append((int(flags), index, bytes(data), size, e))
            raise
        calls.append((int(flags), index, bytes(data), size, out))
        return out

    bundle_module.BundleFile.decompress_data = recording
    try:
        UnityPy.load(path)
    except Exception:
        # A data block's LZMA failure is a result. Any other failure (the
        # blocks-info is the first call, and parsing follows the blocks) is not.
        last = calls[-1] if len(calls) > 1 else None
        if last is None or not isinstance(last[4], Exception) or last[0] & COMPRESSION_MASK != LZMA:
            raise
    finally:
        bundle_module.BundleFile.decompress_data = original
    return [(i, d, s, o) for f, i, d, s, o in calls[1:] if f & COMPRESSION_MASK == LZMA]


def read_list(path):
    return [line.strip() for line in Path(path).read_text().splitlines() if line.strip()]


def cmd_sets(args):
    corpus = Path(args.corpus)
    files = json.loads((corpus / "index.json").read_text())["files"]
    samples = [f for f in files if f["kind"] == "unitypy-sample"]
    samples = [f for f in samples if (corpus / f["path"]).read_bytes()[:7] == b"UnityFS"]
    bundles = [f for f in files if f["kind"] == "bundle"]
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)

    def write(name, chosen):
        (out / name).write_text("".join(f"{corpus / f['path']}\n" for f in chosen))
        mb = sum(f["bytes"] for f in chosen) / 1e6
        print(f"{name}: {len(chosen)} bundles, {mb:.1f} MB compressed")

    write("all.txt", bundles + samples)
    print(f"all.txt: {len(bundles)} corpus bundles and {len(samples)} UnityPy sample bundles")
    # A bundle byte-identical across builds is timed once.
    unique = {}
    for f in sorted(bundles, key=lambda f: f["path"]):
        unique.setdefault(f["sha256"], f)
    print(f"{len(bundles) - len(unique)} corpus bundles repeat another byte for byte; each is timed once")
    by_size = sorted(unique.values(), key=lambda f: (-f["bytes"], f["path"]))
    write("largest20.txt", by_size[:20])
    rest = sorted(by_size[20:], key=lambda f: f["path"])
    write("sample200.txt", sorted(random.Random(SEED).sample(rest, 200), key=lambda f: f["path"]))


def sample(n):
    """The bytes synth.rs's `sample` makes: in every 4096, a 2048-byte run of
    one word, which LZMA writes as long matches, then pseudo-random bytes,
    which it writes as literals."""
    seed = 0x2545F491
    out = bytearray(n)
    for i in range(n):
        seed = (seed * 1_664_525 + 1_013_904_223) & 0xFFFFFFFF
        out[i] = b"unityrs"[i % 7] if i % 4096 < 2048 else seed >> 24
    return bytes(out)


def lzma_block(data):
    """A Unity LZMA block: properties 0x5D (lc 3, lp 0, pb 2), the dictionary
    size, then raw LZMA1 from Python's liblzma."""
    lzma1 = {"id": lzma.FILTER_LZMA1, "dict_size": DICT, "lc": 3, "lp": 0, "pb": 2}
    return bytes([0x5D]) + struct.pack("<I", DICT) + lzma.compress(data, format=lzma.FORMAT_RAW, filters=[lzma1])


def synth_bundle(fmt, engine, flags, blocks, align, pad):
    """A UnityFS bundle holding `blocks` (stored bytes, uncompressed size,
    block flags) and no directory nodes, laid out as synth.rs lays it out.
    `align` and `pad` are where UnityPy expects the header alignment and the
    padding before the first block."""
    info = bytes(16) + struct.pack(">I", len(blocks))
    for stored, size, block_flags in blocks:
        info += struct.pack(">IIH", size, len(stored), block_flags)
    info += struct.pack(">I", 0)
    compression = flags & COMPRESSION_MASK
    if compression == 0:
        stored_info = info
    else:
        mode = "high_compression" if compression == 3 else "default"
        stored_info = lz4.block.compress(info, mode=mode, store_size=False)
    header = b"UnityFS\0" + struct.pack(">I", fmt) + b"5.x.x\0" + engine.encode() + b"\0"
    header += struct.pack(">qIII", 0, len(stored_info), len(info), flags)

    def pad16(b):
        return b + bytes(-len(b) % 16)

    file = pad16(header) if align else header
    if not flags & 0x80:
        file += stored_info
    if pad:
        file = pad16(file)
    file += b"".join(stored for stored, _, _ in blocks)
    if flags & 0x80:
        file += stored_info
    return file


# Header shapes UnityPy reads, with its alignment and padding for each:
# (name, format, engine, flags, align, pad). Format 6 from 2019.4.15 aligns.
SHAPES = (
    ("format6-2019.4.20-front-raw", 6, "2019.4.20f1", 0x40, True, False),
    ("format6-2019.4.20-front-lz4hc", 6, "2019.4.20f1", 0x43, True, False),
    ("format6-2019.4.20-end-raw", 6, "2019.4.20f1", 0x80, True, False),
    ("format6-2019.4.14-end-lz4", 6, "2019.4.14f1", 0xC2, False, False),
    ("format7-2020.3.33-front-lz4hc", 7, "2020.3.33f1", 0x43, True, False),
    ("format8-6000.3-front-lz4-padded", 8, "6000.3.17f1", 0x242, True, True),
    ("format8-6000.3-end-lz4hc-padded", 8, "6000.3.17f1", 0x2C3, True, True),
)
# Blocks on which UnityPy and the bench decoders, or the lzma-rust2 crate
# alone, part ways (codec.rs). The over-long sizes fall in a run (100,000 %
# 4096 = 1,696) and in a random span (101,304 % 4096 = 3,000). 0xFFFFFFF1 is
# the smallest dictionary size lzma-rust2 refuses.
CONTRACT = ("truncated", "over-long-in-a-match", "over-long-between-symbols", "props-0x5f", "dict-0xfffffff1")


def cmd_synth(args):
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)

    def write(name, file):
        path = out / f"{name}.bundle"
        path.write_bytes(file)
        return f"{path}\n"

    # Three blocks: LZMA, LZ4HC, LZMA.
    first, last = sample(60_000), sample(30_000)[::-1]
    lz4hc = lz4.block.compress(first[:5_000], mode="high_compression", store_size=False)
    blocks = [(lzma_block(first), len(first), 0x41), (lz4hc, 5_000, 0x43), (lzma_block(last), len(last), 0x41)]
    shapes = "".join(write(name, synth_bundle(*shape, blocks, align, pad)) for name, *shape, align, pad in SHAPES)
    (out / "shapes.txt").write_text(shapes)

    data = sample(200_000)
    block = lzma_block(data)
    small = bytes(b & 7 for b in data)
    # Bytes under 8 put every literal in context 0 for lc 3 and lc 5 alike,
    # so this lc 3 stream is also a valid lc 5 one, which liblzma rejects.
    props = b"\x5f" + lzma_block(small)[1:]
    corpus_layout = (8, "6000.3.17f1", 0x243)
    contract = {
        "truncated": (block[: len(block) // 2], len(data)),
        "over-long-in-a-match": (block, 100_000),
        "over-long-between-symbols": (block, 101_304),
        "props-0x5f": (props, len(small)),
        # The dictionary size only bounds how far back a match may reach, so
        # the same stream under a larger declared size decodes the same.
        "dict-0xfffffff1": (block[:1] + struct.pack("<I", 0xFFFFFFF1) + block[5:], len(data)),
    }
    lines = []
    for name in CONTRACT:
        stored, size = contract[name]
        lines.append(write(name, synth_bundle(*corpus_layout, [(stored, size, 0x41)], True, True)))
    (out / "contract.txt").write_text("".join(lines))
    print(f"{len(SHAPES)} header shapes in shapes.txt, {len(CONTRACT)} decode-contract cases in contract.txt")


def cmd_oracle(args):
    rows = 0
    with open(args.out, "w") as out:
        for path in read_list(args.list):
            for index, _, size, result in capture(path):
                if isinstance(result, Exception):
                    name = type(result).__name__
                    print(f"{path}#{index}: UnityPy raised {name}: {result}", file=sys.stderr)
                    out.write(f"{path}\t{index}\terror\t{name}\n")
                else:
                    if len(result) != size:
                        print(f"{path}#{index}: UnityPy made {len(result)} of {size} bytes", file=sys.stderr)
                    out.write(f"{path}\t{index}\t{len(result)}\t{hashlib.sha256(result).hexdigest()}\n")
                rows += 1
    print(f"{rows} LZMA blocks from {args.list}")


# Set before the pool forks, so that workers share the blocks instead of
# receiving them over a pipe.
BLOCKS = []


def decode(i):
    path, index, data, size = BLOCKS[i]
    start = time.perf_counter()
    out = CompressionHelper.decompress_lzma(data)
    seconds = time.perf_counter() - start
    if len(out) != size:
        raise ValueError(f"{path}#{index}: {len(out)} of {size} bytes")
    return seconds


def noop(_):
    return None


def cmd_bench(args):
    for path in read_list(args.list):
        for i, d, s, out in capture(path):
            if isinstance(out, Exception):
                raise SystemExit(f"{path}#{i}: UnityPy raised {out!r}, and only blocks it decodes are timed")
            BLOCKS.append((path, i, d, s))
    # Largest first, as the Rust bench does.
    BLOCKS.sort(key=lambda b: -b[3])
    total = sum(b[3] for b in BLOCKS)
    # Python's lzma module binds whichever liblzma.so.5 the loader finds, so
    # LD_LIBRARY_PATH can swap it; the name records which one ran.
    lib = ctypes.CDLL("liblzma.so.5")
    lib.lzma_version_string.restype = ctypes.c_char_p
    name = f"unitypy-liblzma-{lib.lzma_version_string().decode()}"
    block_rows = []
    pool = None
    if args.threads > 1:
        pool = multiprocessing.get_context("fork").Pool(args.threads)
        pool.map(noop, range(args.threads * 4))
    decode(len(BLOCKS) - 1)  # warm up
    for rep in range(args.reps):
        if pool is None:
            times = [decode(i) for i in range(len(BLOCKS))]
            seconds = sum(times)
            for (path, index, _, size), t in zip(BLOCKS, times):
                block_rows.append(f"{name}\t{args.set}\t{rep}\t{path}\t{index}\t{size}\t{t:.6f}\n")
        else:
            start = time.perf_counter()
            for _ in pool.imap_unordered(decode, range(len(BLOCKS)), chunksize=1):
                pass
            seconds = time.perf_counter() - start
        print(f"{name}\t{args.set}\t{args.threads}\t{rep}\t{len(BLOCKS)}\t{total}\t{seconds:.6f}", flush=True)
    if pool is not None:
        pool.close()
        pool.join()
    if args.per_block:
        # Appended to, so that several runs can collect into one file.
        with open(args.per_block, "a") as out:
            out.writelines(block_rows)
    print(f"# UnityPy {UnityPy.__version__}, {name}", file=sys.stderr)


def cmd_report(args):
    runs = defaultdict(list)
    for tsv in args.runs:
        for line in Path(tsv).read_text().splitlines():
            impl, set_name, threads, _, blocks, total, seconds = line.split("\t")
            runs[(set_name, int(threads), impl)].append((int(blocks), int(total), float(seconds)))
    rates = {}
    print("| set | threads | implementation | blocks | MB out | reps | median s | MB/s (median) | MB/s (range) |")
    print("|---|---:|---|---:|---:|---:|---:|---:|---:|")
    for (set_name, threads, impl), rows in sorted(runs.items()):
        blocks, total = rows[0][0], rows[0][1]
        secs = [r[2] for r in rows]
        median = statistics.median(secs)
        rates[(set_name, threads, impl)] = total / median / 1e6
        lo, hi = total / max(secs) / 1e6, total / min(secs) / 1e6
        print(
            f"| {set_name} | {threads} | {impl} | {blocks} | {total / 1e6:.1f} | {len(rows)} "
            f"| {median:.3f} | {total / median / 1e6:.1f} | {lo:.1f}-{hi:.1f} |"
        )
    report_ratios(rates)
    if args.per_block:
        report_blocks(args.per_block)


def report_ratios(rates):
    """How many times as fast the baseline is as each implementation, how
    each scales, and what a GB of output costs it, all from the medians above."""
    many = max(threads for _, threads, _ in rates)
    if many == 1:
        return
    print()
    print(
        f"| set | implementation | {BASELINE} / this, 1 core | {BASELINE} / this, {many}-way "
        f"| {many}-way / 1 core | core-s per GB | s per GB, {many}-way |"
    )
    print("|---|---|---:|---:|---:|---:|---:|")
    for set_name, impl in sorted({(s, i) for s, _, i in rates}):
        one, par = rates.get((set_name, 1, impl)), rates.get((set_name, many, impl))
        base_one, base_par = rates.get((set_name, 1, BASELINE)), rates.get((set_name, many, BASELINE))
        if None in (one, par, base_one, base_par):
            continue
        print(
            f"| {set_name} | {impl} | {base_one / one:.2f} | {base_par / par:.2f} | {par / one:.1f} "
            f"| {1000 / one:.2f} | {1000 / par:.2f} |"
        )


def report_blocks(per_block):
    """Each block's median seconds per implementation, summed by block size,
    so that the cost of starting a stream shows apart from steady decoding.
    The largest block of each set is named by that title alone."""
    times = defaultdict(list)
    for tsv in per_block:
        for line in Path(tsv).read_text().splitlines():
            impl, set_name, _, path, index, size, seconds = line.split("\t")
            times[(set_name, impl, path, int(index), int(size))].append(float(seconds))
    sizes = defaultdict(dict)
    for set_name, _, path, index, size in times:
        sizes[set_name][(path, index)] = size
    largest = {s: max((size, key) for key, size in blocks.items()) for s, blocks in sizes.items()}

    print()
    print("| set | blocks | MB out | smallest, MB | median, MB | largest, MB |")
    print("|---|---:|---:|---:|---:|---:|")
    for set_name in sorted(sizes):
        s = sorted(sizes[set_name].values())
        print(
            f"| {set_name} | {len(s)} | {sum(s) / 1e6:.1f} | {s[0] / 1e6:.3f} "
            f"| {statistics.median(s) / 1e6:.2f} | {s[-1] / 1e6:.1f} |"
        )

    sums = defaultdict(lambda: [0, 0.0, 0])
    for (set_name, impl, path, index, size), secs in times.items():
        labels = ["all", next(label for lo, label in SIZE_BINS if size >= lo)]
        if (size, (path, index)) == largest[set_name]:
            labels.append(LARGEST)
        for label in labels:
            total = sums[(set_name, label, impl)]
            total[0] += size
            total[1] += statistics.median(secs)
            total[2] += 1
    impls = sorted({impl for _, _, impl in sums})
    print()
    print(f"MB/s, except the last two rows of each set: seconds, and how many times as fast {BASELINE} is.")
    print()
    print("| set | blocks | n | MB out | " + " | ".join(impls) + " |")
    print("|---|---|---:|---:|" + "---:|" * len(impls))
    for set_name in sorted(sizes):
        labels = [label for _, label in SIZE_BINS[::-1]] + ["all", LARGEST]
        for label in labels:
            cells = [sums.get((set_name, label, impl)) for impl in impls]
            first = next((c for c in cells if c), None)
            if first is None:
                continue
            row = " | ".join(f"{c[0] / c[1] / 1e6:.1f}" if c else "" for c in cells)
            print(f"| {set_name} | {label} | {first[2]} | {first[0] / 1e6:.1f} | {row} |")
        cells = [sums.get((set_name, LARGEST, impl)) for impl in impls]
        row = " | ".join(f"{c[1]:.3f} s" if c else "" for c in cells)
        print(f"| {set_name} | {LARGEST}, seconds | 1 | | {row} |")
        base = sums.get((set_name, LARGEST, BASELINE))
        if base:
            row = " | ".join(f"{c[1] / base[1]:.2f}" if c else "" for c in cells)
            print(f"| {set_name} | {LARGEST}, {BASELINE} / this | 1 | | {row} |")


def cmd_crates(args):
    def get(url):
        request = urllib.request.Request(url, headers={"User-Agent": "unityrs-bench-lzma (spike)"})
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)

    api = "https://crates.io/api/v1/crates"
    for name in ("liblzma", "lzma-rust2"):
        versions = get(f"{api}/{name}/versions")["versions"][: args.releases]
        print(f"{name} releases: " + ", ".join(f"{v['num']} ({v['created_at'][:10]})" for v in versions))
        users = get(f"{api}/{name}/reverse_dependencies?per_page={args.users}")
        print(f"{name} reverse dependencies by downloads: " + ", ".join(v["crate"] for v in users["versions"]))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("sets")
    p.add_argument("--corpus", default="/corpus")
    p.add_argument("--out-dir", required=True)
    p = sub.add_parser("synth")
    p.add_argument("--out-dir", required=True)
    p = sub.add_parser("oracle")
    p.add_argument("--list", required=True)
    p.add_argument("--out", required=True)
    p = sub.add_parser("bench")
    p.add_argument("--list", required=True)
    p.add_argument("--set", required=True)
    p.add_argument("--threads", type=int, default=1)
    p.add_argument("--reps", type=int, default=5)
    p.add_argument("--per-block")
    p = sub.add_parser("report")
    p.add_argument("runs", nargs="+")
    p.add_argument("--per-block", nargs="*")
    p = sub.add_parser("crates")
    p.add_argument("--releases", type=int, default=5)
    p.add_argument("--users", type=int, default=8)
    args = parser.parse_args()
    commands = {
        "sets": cmd_sets,
        "synth": cmd_synth,
        "oracle": cmd_oracle,
        "bench": cmd_bench,
        "report": cmd_report,
        "crates": cmd_crates,
    }
    commands[args.cmd](args)


if __name__ == "__main__":
    main()
