# Ported from UnityPy 1.25.2 (MIT): export/SpriteHelper.py:29-346. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Sprite image and standalone geometry helpers over the native renderer."""
from .. import _native
from ..enums import SpritePackingMode, SpritePackingRotation, SpriteMeshType


class SpriteSettings:
    def __init__(self, settings_raw):
        self.settingsRaw = settings_raw
        self.packed = bool(settings_raw & 1)
        self.packingMode = SpritePackingMode((settings_raw >> 1) & 1)
        self.packingRotation = SpritePackingRotation((settings_raw >> 2) & 15)
        self.meshType = SpriteMeshType((settings_raw >> 6) & 1)


def get_image_from_sprite(m_Sprite):
    from ..classes._images_sprite import get_sprite_image
    return get_sprite_image(m_Sprite)


def get_image(sprite, texture, alpha_texture):
    from PIL import Image
    from .Texture2DConverter import get_image_from_texture2d
    from ..classes._images import _guard
    assert sprite.assets_file, "Sprite assets file is not set!"
    guard = _guard(sprite)
    cache = sprite.assets_file._cache
    key = (texture.path_id, alpha_texture.path_id) if alpha_texture else texture.path_id
    if key not in cache:
        image = get_image_from_texture2d(texture.deref_parse_as_object(), False)
        guard.step(0, image.width * image.height * 8 + 1024, image.width * image.height)
        if alpha_texture:
            alpha = get_image_from_texture2d(alpha_texture.deref_parse_as_object(), False)
            image = Image.merge("RGBA", (*image.split()[:3], alpha.split()[0]))
        cache[key] = image
    return cache[key]


def _geometry(sprite, mesh, image, mask):
    from PIL import Image
    from ..classes._images import _guard, _reader
    guard = _guard(sprite)
    positions = mesh.m_Vertices
    if mask:
        assert positions, "No vertices found in sprite mesh!"
    uv = [] if mask else mesh.m_UV0
    groups = []
    staged_positions, staged_uv = positions, uv
    remap = not mask and positions and uv and len(positions) != len(uv)
    if remap:
        count = max(len(positions), len(uv))
        guard.step(0, count * 64 + 1024, count)
        staged_positions = list(positions) + [positions[0]] * (count - len(positions))
        staged_uv = list(uv) + [(0., 0.)] * (count - len(uv))
    for group in mesh.get_triangles():
        guard.step(0, 64, 1)
        if not mask:
            if not positions:
                continue
            if not uv:
                raise ValueError("No UV coordinates found in sprite mesh!")
        triangles = []
        for tri in group:
            guard.step(0, 192, 1)
            if mask:
                a, b, c = tri
                tri = (a, b, c)
            elif len(tri) < 3:
                raise IndexError("tuple index out of range")
            normalized = []
            for index in tri:
                # Python's list indexing accepts negative indices.
                if not -len(positions) <= index < len(positions):
                    raise IndexError("list index out of range")
                position_index = index % len(positions)
                if not mask:
                    if not -len(uv) <= index < len(uv):
                        raise IndexError("list index out of range")
                    uv_index = index % len(uv)
                    if remap and uv_index != position_index:
                        position_index = len(staged_positions)
                        staged_positions.append(positions[index])
                        staged_uv.append(uv[index])
                normalized.append(position_index)
            triangles.append(tuple(normalized))
        groups.append(triangles)
        if not mask:
            break
    guard.step(0, image.width * image.height * 16 + 1024, image.width * image.height)
    flipped = image.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    mode, width, height, pixels = _native._sprite_geometry(
        flipped.tobytes(), flipped.mode, flipped.width, flipped.height,
        staged_positions, staged_uv, groups, sprite.m_PixelsToUnits, mask, _reader(sprite))
    guard.step(0, len(pixels) + 1024, len(pixels))
    result = Image.frombytes(mode, (width, height), pixels)
    if mask and image.mode != "RGBA":
        image.putalpha(result.getchannel("A"))
        return image
    return result


def mask_sprite(m_Sprite, mesh, sprite_image):
    return _geometry(m_Sprite, mesh, sprite_image, True)


def render_sprite_mesh(m_Sprite, mesh, texture):
    return _geometry(m_Sprite, mesh, texture, False)


def copy_triangle(src_img, src_tri, dst_img, dst_tri):
    from PIL import Image
    guard = _native.TreeTraversal()
    guard.step(0, (src_img.width * src_img.height + dst_img.width * dst_img.height) * 16 + 1024,
               src_img.width * src_img.height + dst_img.width * dst_img.height)
    source = src_img.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    destination = dst_img.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    mode, width, height, pixels = _native._sprite_copy_triangle(
        source.tobytes(), source.mode, source.width, source.height, src_tri,
        destination.tobytes(), destination.mode, destination.width, destination.height, dst_tri)
    guard.step(0, len(pixels) + 1024, len(pixels))
    dst_img.paste(Image.frombytes(mode, (width, height), pixels))


def transpose_matrix(m):
    return map(list, zip(*m))


def get_matrix_minor(m, i, j):
    return [row[:j] + row[j + 1:] for row in (m[:i] + m[i + 1:])]


def _matrix_guard(m):
    guard = _native.TreeTraversal()
    size = len(m)
    work = 1
    # Reserve the recursive cofactor expansion's work before starting it.
    for count in range(1, size + 1):
        work = min((1 << 64) - 1, work * count)
        guard.step(0, count * 64, min((1 << 64) - 1, work * size * size * 40))
    return guard


def _determinant(m):
    if len(m) == 2:
        return m[0][0] * m[1][1] - m[0][1] * m[1][0]
    return sum((-1) ** c * m[0][c] * _determinant(get_matrix_minor(m, 0, c)) for c in range(len(m)))


def get_matrix_determinant(m):
    guard = _matrix_guard(m)
    return _determinant(m)


def get_matrix_inverse(m):
    guard = _matrix_guard(m)
    determinant = _determinant(m)
    if len(m) == 2:
        return [[m[1][1] / determinant, -m[0][1] / determinant],
                [-m[1][0] / determinant, m[0][0] / determinant]]
    cofactors = [[(-1) ** (r + c) * _determinant(get_matrix_minor(m, r, c))
                  for c in range(len(m))] for r in range(len(m))]
    return [[c / determinant for c in row] for row in transpose_matrix(cofactors)]


def linalg_solve(M, y):
    inverse = get_matrix_inverse(M)
    return [sum(inverse[i][j] * y[j] for j in range(len(y))) for i in range(len(inverse))]


__all__ = ["get_image_from_sprite"]
