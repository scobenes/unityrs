"""The systems tools/fixtures/sprite.py records numpy's solutions of, for unityrs-texture's bit test of
`sprite::affine::solve` (crates/unityrs-texture/tests/sprite_solve.rs; docs/plans/extract.md section 3, item 6), not
a fixture script.

`systems()` gives SYSTEMS systems of `copy_triangle`'s shape (`export/SpriteHelper.py:273-287`: the 6 x 6 matrix of
three integer destination corners, the vector of three integer source corners), from a seeded `random.Random`, in the
shapes the Python model of item 6's port was measured on: a destination on a canvas of a side from 3 to 4,096 and a
source within a sheet of a side from 3 to 4,096 either way of 0; in one system of seven, one destination coordinate
shared with another corner's (cancellation, and pivots that meet exact zeros), and in one of ten, three collinear
destination corners. `solution()` solves one as `copy_triangle` does (`np.linalg.solve(M, y)`, the lists of ints as
it passes them) and gives its six float64s as one string of their IEEE-754 bits in hex, big-endian, each 16 digits,
or None where numpy raises `LinAlgError`. Recorded under the pinned Haswell kernel, they are the reference's bits.

No system here can tell a fused multiply-add from an unfused one: the one on numpy's path (`dgemv_kernel_4x2`'s
`vfmadd231pd`, getf2's update) multiplies by alpha = -1, which is exact, so the fused and the unfused step round the
same sum once (`crates/unityrs-texture/src/sprite/affine.rs`).

Run on its own it only imports (tools/parity/tests/test_fixture_modules.py).
"""

from __future__ import annotations

import random
import struct

SYSTEMS = 2000
SEED = 17


def system(src: list, dst: list) -> tuple[list, list]:
    """`copy_triangle`'s M and y (`export/SpriteHelper.py:273-287`) for source and destination corners."""
    (x11, x12), (x21, x22), (x31, x32) = src
    (y11, y12), (y21, y22), (y31, y32) = dst
    m = [[y11, y12, 1, 0, 0, 0], [y21, y22, 1, 0, 0, 0], [y31, y32, 1, 0, 0, 0],
         [0, 0, 0, y11, y12, 1], [0, 0, 0, y21, y22, 1], [0, 0, 0, y31, y32, 1]]  # fmt: skip
    return m, [x11, x21, x31, x12, x22, x32]


def systems() -> list[dict]:
    """The seeded batch, each as its corners: {"src": [[x, y] x3], "dst": [[x, y] x3]}."""
    rng = random.Random(SEED)
    out = []
    for k in range(SYSTEMS):
        side, sheet = rng.randint(3, 4096), rng.randint(3, 4096)
        dst = [[rng.randint(0, side), rng.randint(0, side)] for _ in range(3)]
        src = [[rng.randint(-sheet, sheet), rng.randint(-sheet, sheet)] for _ in range(3)]
        if k % 7 == 3:
            a, b = rng.sample(range(3), 2)
            axis = rng.randrange(2)
            dst[a][axis] = dst[b][axis]
        if k % 10 == 5:
            start = [rng.randint(0, side // 2), rng.randint(0, side // 2)]
            step = [rng.randint(0, side // 4), rng.randint(0, side // 4)]
            dst = [[start[0] + i * step[0], start[1] + i * step[1]] for i in range(3)]
        out.append({"src": src, "dst": dst})
    return out


def solution(np, src: list, dst: list) -> str | None:
    """numpy's solution's bits, as `systems()` says; None for `LinAlgError`."""
    m, y = system(src, dst)
    try:
        a = np.linalg.solve(m, y)
    except np.linalg.LinAlgError:
        return None
    return "".join(struct.pack(">d", float(v)).hex() for v in a)
