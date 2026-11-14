"""Record CPython's .9G and .4f over deterministic binary64 bits (P2a L6)."""
import argparse
import json
import math
import random
import struct
from pathlib import Path


def fixture():
    bits = {0, 1, (1 << 63), (1 << 52) - 1, 1 << 52,
            0x7fefffffffffffff, 0xffefffffffffffff,
            0x7ff0000000000000, 0xfff0000000000000,
            0x7ff8000000000000, 0xfff8000000000000}
    def add(x):
        bits.add(struct.unpack('>Q', struct.pack('>d', x))[0])
    for exponent in (-324, -308, -5, -4, 8, 9, 308):
        x = 10.0 ** exponent
        for v in (x, -x, math.nextafter(x, 0), math.nextafter(x, math.inf)):
            add(v)
    for i in range(-1024, 1025):
        add(i / 32)
        add(i / 20000)
    rng = random.Random(1252)
    bits.update(rng.getrandbits(64) for _ in range(4096))
    cases = []
    for n in sorted(bits):
        x = struct.unpack('>d', n.to_bytes(8, 'big'))[0]
        cases.append({'bits': f'{n:016x}', 'g9': format(x, '.9G'), 'f4': format(x, '.4f')})
    return {'reference': 'CPython format, binary64', 'cases': cases}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--check', action='store_true')
    args = p.parse_args()
    path = Path(__file__).resolve().parents[2] / 'crates/unityrs-mesh/tests/fixtures/pyfmt.json'
    body = json.dumps(fixture(), separators=(',', ':'), sort_keys=True) + '\n'
    if args.check:
        if not path.exists() or path.read_text() != body:
            p.exit(1, 'stale fixture: pyfmt.json\n')
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)


if __name__ == '__main__':
    main()
