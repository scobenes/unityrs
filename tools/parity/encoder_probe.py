"""Reproduce B3b's encoder ISA probe against frozen raw amd64 wheel output.

Run Python only in the existing development containers, from the worktree:
    python tools/parity/encoder_probe.py --check
    python tools/parity/encoder_probe.py --oracle

The native C++ companion also accepts framed input/output files. To replay on
macOS without Python, export inputs in the container with --export-input PATH,
compile encoder_probe.cpp with the selected flags below and vendor core sources,
then execute: encoder-probe inputs.bin outputs.bin. --import-output can compare
that native output in the container. All recorded variants use quality 100,
LDR, U8 RGBA, USE_DECODE_UNORM8 and one thread, exactly as UnityPy does even
for its ASTC HDR texture format aliases. The source closure includes astcenc's
symbolic decompressor because its encoder uses that in the search loop.
"""
from __future__ import annotations

import argparse
import ctypes
import json
import os
import platform
import shlex
import struct
import sys
from pathlib import Path

import child
import scratch

ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / 'crates/unityrs-texture'
FIXTURE = CRATE / 'tests/fixtures/encoder_isa.json'
ETC = ['ProcessDxtc.cpp', 'ProcessRGB.cpp', 'bc7enc.cpp', 'Dither.cpp', 'Tables.cpp']
BASE = ['-std=c++20', '-O2', '-DNDEBUG', '-ffp-contract=off', '-DASTCENC_SVE=0']
MODES = {
    'scalar': ['-U__ARM_NEON', '-U__ARM_NEON__', '-U__AVX__', '-U__SSE4_1__', '-U__AVX2__',
               '-U__AVX512F__', '-U__AVX512BW__', '-U__AVX512VL__',
               '-DASTCENC_NEON=0', '-DASTCENC_SSE=0', '-DASTCENC_AVX=0',
               '-DASTCENC_POPCNT=0', '-DASTCENC_F16C=0'],
    'sse41': ['-msse4.1', '-mpopcnt', '-DASTCENC_NEON=0', '-DASTCENC_SSE=41',
              '-DASTCENC_AVX=0', '-DASTCENC_POPCNT=1', '-DASTCENC_F16C=0'],
    'avx2': ['-msse4.1', '-mfma', '-mbmi', '-mavx2', '-mpopcnt', '-mf16c',
             '-DASTCENC_NEON=0', '-DASTCENC_SSE=41', '-DASTCENC_AVX=2',
             '-DASTCENC_POPCNT=1', '-DASTCENC_F16C=1'],
    'neon': ['-D__ARM_NEON', '-DASTCENC_NEON=1', '-DASTCENC_SSE=0',
             '-DASTCENC_AVX=0', '-DASTCENC_POPCNT=0', '-DASTCENC_F16C=0'],
}


def framed(cases):
    result = bytearray()
    for c in cases:
        raw = bytes.fromhex(c['rgba'])
        result += struct.pack('<7I', c['f'], c['w'], c['h'], c['bx'], c['by'],
                              len(raw), len(bytes.fromhex(c['oracle'])))
        result += raw
    return result


def compile_command(mode, destination, shared=False, relative=False):
    root = Path('.') if relative else ROOT
    crate = root / 'crates/unityrs-texture'
    command = [os.environ.get('CXX', 'c++'), *BASE, *MODES[mode]]
    if shared:
        command += ['-shared', '-fPIC']
    return [*command, '-I' + str(crate / 'vendor/etcpak'),
            '-I' + str(crate / 'vendor/astcenc'), str(root / 'tools/parity/encoder_probe.cpp'),
            *(str(crate / 'vendor/etcpak' / s) for s in ETC),
            *(str(root / s.relative_to(ROOT)) for s in sorted((CRATE / 'vendor/astcenc').glob('*.cpp'))),
            '-pthread', '-o', str(destination)]


def oracle(cases):
    import etcpak
    import astc_encoder as astc
    import importlib.metadata
    assert importlib.metadata.version('etcpak') == '0.9.14'
    assert importlib.metadata.version('astc-encoder-py') == '0.1.12'
    contexts = {}
    for c in cases:
        raw = bytes.fromhex(c['rgba'])
        if c['f']:
            # Each call owns fresh bytes: the ETC Python binding swaps R/B in place.
            answer = getattr(etcpak, 'compress_' + c['name'])(raw, c['w'], c['h'])
        else:
            key = c['bx'], c['by']
            if key not in contexts:
                config = astc.ASTCConfig(astc.ASTCProfile.LDR, *key, block_z=1,
                                        quality=100, flags=astc.ASTCConfigFlags.USE_DECODE_UNORM8)
                contexts[key] = astc.ASTCContext(config)
            image = astc.ASTCImage(astc.ASTCType.U8, c['w'], c['h'], 1, raw)
            answer = contexts[key].compress(image, astc.ASTCSwizzle.from_str('RGBA'))
        assert answer.hex() == c['oracle'], (c['name'], c['seed'])
    return {'etcpak': [m for m in sys.modules if m.startswith('etcpak._etcpak_')],
            'astcenc': [m for m in sys.modules if m.startswith('astc_encoder._encoder_')]}


def check(cases, modes, output):
    machine = platform.machine()
    system = platform.system()
    key_prefix = 'mac-' if system == 'Darwin' else ('linux_arm_' if machine == 'aarch64' else '')
    report = {}
    for mode in modes:
        with scratch.folder('encoder-probe-') as folder:
            library = folder / (mode + '.so')
            command = compile_command(mode, library, shared=True)
            compiled = child.run(command, capture_output=True, text=True)
            if compiled.returncode:
                print(compiled.stderr, file=sys.stderr)
                compiled.check_returncode()
            lib = ctypes.CDLL(str(library))
            differences = {}
            for c in cases:
                raw = bytes.fromhex(c['rgba'])
                buf = ctypes.create_string_buffer(len(bytes.fromhex(c['oracle'])))
                if c['f']:
                    code = lib.etc(c['f'], raw, c['w'], c['h'], buf)
                else:
                    code = lib.astc(raw, c['w'], c['h'], c['bx'], c['by'], buf)
                assert code == 0, (mode, c['name'], c['seed'], code)
                actual = buf.raw.hex()
                expected = c[key_prefix + mode]
                assert actual == expected, (mode, c['name'], c['seed'])
                differences[c['name']] = differences.get(c['name'], 0) + (actual != c['oracle'])
                if output is not None:
                    output.append(dict(name=c['name'], seed=c['seed'], variant=key_prefix + mode,
                                       bytes=actual))
            report[mode] = differences
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--oracle', action='store_true')
    parser.add_argument('--modes', help='comma-separated subset of scalar,sse41,avx2,neon')
    parser.add_argument('--output', type=Path, help='preserve new raw outputs')
    parser.add_argument('--export-input', type=Path)
    parser.add_argument('--import-output', type=Path)
    parser.add_argument('--variant', help='recorded variant for --import-output')
    parser.add_argument('--native-command', choices=MODES,
                        help='print a portable native compilation command; no build is run')
    args = parser.parse_args()
    if args.native_command:
        print(shlex.join(compile_command(args.native_command, '/tmp/encoder-probe', relative=True)))
        return
    fixture = json.loads(FIXTURE.read_text())
    cases = fixture['cases']
    report = {}
    if args.export_input:
        args.export_input.write_bytes(framed(cases))
    if args.import_output:
        data, pos = args.import_output.read_bytes(), 0
        assert args.variant
        for c in cases:
            n = len(bytes.fromhex(c['oracle']))
            assert data[pos:pos+n].hex() == c[args.variant], (c['name'], c['seed'])
            pos += n
        assert pos == len(data)
        report['native_verified'] = len(cases)
    if args.oracle:
        report['oracle_dispatch'] = oracle(cases)
    if args.check or not any([args.oracle, args.export_input, args.import_output]):
        modes = (args.modes.split(',') if args.modes else
                 (['scalar', 'sse41', 'avx2'] if platform.machine() == 'x86_64' else ['scalar', 'neon']))
        outputs = [] if args.output else None
        report['different_bytes'] = check(cases, modes, outputs)
        if args.output:
            args.output.write_text(json.dumps(outputs, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
