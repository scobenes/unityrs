"""Synthetic texture extension and standalone sprite observations; no TPK data."""
import importlib
import json
from pathlib import Path
from types import SimpleNamespace
from binding_y4_images import capture, image


def _module(package, name):
    return importlib.import_module(package.__name__ + '.export.' + name)


def observe_switch_predicate(package):
    helper = importlib.import_module(package.__name__ + '.helpers.TextureSwizzler')
    result = []
    for exponent in (0, 1, 2, 30, 31, 32, 63):
        blob = bytes(8) + exponent.to_bytes(4, 'little')
        result.append([exponent, helper.get_switch_gobs_per_block(blob),
                       helper.is_switch_swizzled(38, blob), helper.is_switch_swizzled(0, blob),
                       helper.is_switch_swizzled(38, blob[:11])])
    result.append([helper.is_switch_swizzled(38, None), helper.is_switch_swizzled(38, [])])
    return result


def observe_texture_dispatch(package):
    from PIL import Image
    converter = _module(package, 'Texture2DConverter')
    TF = package.enums.TextureFormat
    calls = []

    def extension(data, width, height, marker):
        calls.append([type(data).__name__, data.hex(), width, height, marker])
        return Image.new('RGBA', (width, height), (17, 31, 47, 63))

    original = dict(converter.CONV_TABLE)
    result = {}
    try:
        for fmt in (TF.RGBA32, TF.BGRA32, TF.RGB565, TF.DXT1Crunched, TF.DXT5Crunched,
                    TF.ETC_RGB4Crunched, TF.ETC2_RGBA8Crunched):
            converter.CONV_TABLE[fmt] = (extension, (fmt.name,))
        blob = bytes(8) + b'\x01\x00\x00\x00'
        # Both RGB promotions and first-mip cropping pass through the patched slot.
        raw = bytes(range(256)) * 4
        cases = [('raw-no-version', b'\x01\x02\x03\x04', 1, 1, TF.RGBA32, None, 0, None),
                 ('switch-rgb', raw, 3, 2, TF.RGB24, (2020, 1), 38, blob),
                 ('switch-bgr', raw, 3, 2, TF.BGR24, (2020, 1), 38, blob),
                 ('xbox', bytes(range(12)), 2, 3, TF.RGB565, (5, 0), 11, None)]
        root = Path(__file__).resolve().parents[2] / 'crates/unityrs-texture/tests/fixtures'
        for filename, version in [('crunch_native.json', (2017, 2)),
                                  ('unitycrunch_native.json', (2017, 3))]:
            fixtures = json.loads((root / filename).read_text())['cases']
            for fmt in (28, 29, 64, 65):
                case = next((c for c in fixtures if c['format'] == fmt and c['width'] == 3 and c['height'] == 5), None)
                if case:
                    cases.append((f'{filename}-{fmt}', bytes.fromhex(case['data']['hex']),
                                  3, 5, TF(fmt), version, 0, None))
        cases += [('bad-crunch', b'invalid', 4, 4, TF.DXT1Crunched, (2017, 3), 0, None),
                  ('bad-legacy', b'invalid', 4, 4, TF.DXT1Crunched, (2017, 2), 0, None)]
        for label, data, width, height, fmt, version, platform, platform_blob in cases:
            calls.clear()
            output = capture(lambda: image(converter.parse_image_data(data, width, height, fmt,
                version, platform, platform_blob, False)))
            result[label] = [output, list(calls)]
        del converter.CONV_TABLE[TF.DXT1Crunched]
        result['removed-crunch-slot'] = capture(lambda: image(converter.parse_image_data(
            b'invalid', 4, 4, TF.DXT1Crunched, (2017, 3), 0)))
    finally:
        converter.CONV_TABLE.clear()
        converter.CONV_TABLE.update(original)
    return result


def observe_astc_context(package):
    import astc_encoder as A
    converter = _module(package, 'Texture2DConverter')
    cached = converter._get_astc_context
    cached.cache_clear()
    first = converter.get_astc_context((4, 4))
    same = converter.get_astc_context((4, 4))
    distinct = cached(-123, (4, 4))
    config = first.config
    result = {'identity': [first is same, first is distinct],
              'config': [int(config.profile), config.block_x, config.block_y, config.block_z,
                         int(config.flags), first.threads],
              'cache': list(cached.cache_info()), 'parameters': cached.cache_parameters(),
              'unhashable': capture(lambda: converter.get_astc_context([4, 4]))}
    swizzle = A.ASTCSwizzle.from_str('RGBA')
    pixels = bytes((51, 102, 153, 204)) * 16
    src = A.ASTCImage(A.ASTCType.U8, 4, 4, 1, pixels)
    encoded = first.compress(src, swizzle)
    result['encoded'] = encoded.hex()
    rectangle = converter.get_astc_context((5, 4))
    rectangular_data = rectangle.compress(src, swizzle)
    result['rectangle'] = [rectangular_data.hex(),
        rectangle.decompress(rectangular_data, A.ASTCImage(A.ASTCType.U8, 4, 4, 1), swizzle).data.hex()]
    result['footprints'] = {}
    for block in ((4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6),
                  (8, 8), (10, 5), (10, 6), (10, 8), (10, 10), (12, 10), (12, 12)):
        context = converter.get_astc_context(block)
        compressed = context.compress(src, swizzle)
        decoded = context.decompress(compressed, A.ASTCImage(A.ASTCType.U8, 4, 4, 1), swizzle)
        result['footprints'][f'{block[0]}x{block[1]}'] = [compressed.hex(), decoded.data.hex()]
    result['invalid-block'] = capture(lambda: converter.get_astc_context((3, 3)))
    dest = A.ASTCImage(A.ASTCType.U8, 4, 4, 1)
    returned = first.decompress(encoded, dest, swizzle)
    result['decoded'] = [returned is dest, dest.data.hex()]
    result['astc_helper'] = image(converter.astc(encoded + b'extra', 4, 4, (4, 4)))
    result['compress_helper'] = converter.compress_astc(pixels, 4, 4, package.enums.TextureFormat.ASTC_RGBA_4x4).hex()
    result['short'] = capture(lambda: converter.astc(b'x', 4, 4, (4, 4)))
    cached.cache_clear()
    return result


def _mesh(positions=None, uv=None, groups=None):
    if positions is None:
        positions = [(0., 0., 0.), (3., 0., 0.), (0., 3., 0.), (3., 3., 0.)]
    if uv is None:
        uv = [(0., 0.), (1., 0.), (0., 1.), (1., 1.)]
    if groups is None:
        groups = [[(0, 1, 2), (1, 3, 2)]]
    return SimpleNamespace(m_Vertices=positions, m_UV0=uv, get_triangles=lambda: iter(groups))


def observe_sprite_geometry(package):
    from PIL import Image
    helper = _module(package, 'SpriteHelper')
    sprite = SimpleNamespace(m_PixelsToUnits=1., object_reader=None)
    result = {}
    for mode, channels in [('RGB', 3), ('RGBA', 4)]:
        source = Image.frombytes(mode, (4, 4), bytes(range(16 * channels)))
        for name, mesh in [('square', _mesh()), ('negative', _mesh(groups=[[(-4, -3, -2)]])),
                           ('triangle', _mesh(groups=[[(0, 1, 2)]])),
                           ('unequal-uv-negative', _mesh(uv=[(0., 0.), (1., 0.), (0., 1.)], groups=[[(0, 1, -1)]])),
                           ('empty', _mesh(positions=[])),
                           ('no-uv', _mesh(uv=[])), ('short-uv', _mesh(uv=[(0.,)] * 4)),
                           ('no-submesh', _mesh(groups=[])),
                           ('out-of-range', _mesh(groups=[[(0, 1, 9)]]))]:
            for mask in (False, True):
                target = source.copy()
                func = helper.mask_sprite if mask else helper.render_sprite_mesh
                result[f'{mode}-{name}-{mask}'] = capture(lambda: image(func(sprite, mesh, target)))
    result['settings'] = []
    for raw in (0, 3, 7, 11, 15, 19, 64, 255):
        def settings():
            value = helper.SpriteSettings(raw)
            return [value.settingsRaw, value.packed, int(value.packingMode),
                    int(value.packingRotation), int(value.meshType)]
        result['settings'].append(capture(settings))
    from binding_y4_images import texture
    cache = {}
    owner = SimpleNamespace(assets_file=SimpleNamespace(_cache=cache), object_reader=None)
    tex = texture(package)
    ptr = SimpleNamespace(path_id=7, deref_parse_as_object=lambda: tex)
    first = helper.get_image(owner, ptr, None)
    same = helper.get_image(owner, ptr, None)
    alpha = SimpleNamespace(path_id=8, deref_parse_as_object=lambda: texture(package,
        data=b'\x11\xf0', m_TextureFormat=63))
    merged = helper.get_image(owner, ptr, alpha)
    result['get-image'] = [first is same, image(first), image(merged),
                           [str(key) for key in cache]]
    source = Image.frombytes('RGBA', (4, 4), bytes(range(64)))
    for name, src_tri, dst_tri in [
        ('translate', ((0, 0), (2, 0), (0, 2)), ((1, 1), (3, 1), (1, 3))),
        ('affine', ((0, 0), (4, 0), (0, 4)), ((0, 0), (3, 0), (1, 4))),
        ('clipped', ((-2, -2), (3, -2), (-2, 3)), ((0, 0), (5, 0), (0, 5))),
    ]:
        destination = Image.new('RGBA', (5, 5), (9, 8, 7, 6))
        returned = helper.copy_triangle(source, src_tri, destination, dst_tri)
        result[name] = [returned is None, image(destination)]
    for src_mode, dst_mode in [('RGB', 'RGBA'), ('RGBA', 'RGB')]:
        for transform in (False, True):
            source = Image.new(src_mode, (3, 3), (11, 22, 33))
            destination = Image.new(dst_mode, (5, 5), (9, 8, 7))
            src_tri = ((-1, -1), (3, -1), (-1, 3))
            dst_tri = ((0, 0), (5 if transform else 4, 0), (0, 4))
            helper.copy_triangle(source, src_tri, destination, dst_tri)
            result[f'mixed-{src_mode}-{dst_mode}-{transform}'] = image(destination)
    return result


def observe_sprite_matrices(package):
    helper = _module(package, 'SpriteHelper')
    result = {}
    for name, matrix in [('two', [[4, 7], [2, 6]]),
                         ('three', [[2., 1., 0.], [0., 3., 1.], [1., 0., 4.]]),
                         ('singular', [[1, 2], [2, 4]]), ('one', [[7]]),
                         ('empty', []), ('ragged', [[1, 2, 3], [4, 5]])]:
        result[name] = {
            'transpose': list(helper.transpose_matrix(matrix)),
            'minor': helper.get_matrix_minor(matrix, 0, 0),
            'determinant': capture(lambda: helper.get_matrix_determinant(matrix)),
            'inverse': capture(lambda: helper.get_matrix_inverse(matrix)),
            'solve': capture(lambda: helper.linalg_solve(matrix, list(range(1, len(matrix) + 1)))),
        }
    return result


def observe_astc_options(package, corrected=False):
    """Retain the actual oracle's support beside the native context's bounds."""
    import astc_encoder as A
    converter = _module(package, 'Texture2DConverter')
    rgba = A.ASTCSwizzle.from_str('RGBA')
    bgra = A.ASTCSwizzle.from_str('BGRA')
    source = A.ASTCImage(A.ASTCType.U8, 4, 4, 1, bytes(64))
    floats = A.ASTCImage(A.ASTCType.F32, 4, 4, 1, bytes(256))
    cases = [
        ('float', (4, 4), floats, rgba, 'ASTC context supports 2D U8 images'),
        ('swizzle', (4, 4), source, bgra, 'ASTC context supports RGBA swizzle'),
    ]
    result = {}
    for name, block, image_value, swizzle, refusal in cases:
        if corrected and package.__name__ == 'UnityPy':
            result[name] = {'error': 'NotImplementedError', 'message': refusal}
        else:
            result[name] = capture(lambda: converter.get_astc_context(block).compress(image_value, swizzle).hex())
    result['native-type'] = {'value': isinstance(converter.get_astc_context((4, 4)), A.ASTCContext)}
    if corrected and package.__name__ == 'UnityPy':
        result['native-type'] = {'value': False}
    converter._get_astc_context.cache_clear()
    return result


def observe_sprite_bounds(package, corrected=False):
    """The standalone adapters retain the existing renderer's explicit bounds."""
    from PIL import Image
    helper = _module(package, 'SpriteHelper')
    source = Image.new('RGBA', (4, 4), (1, 2, 3, 4))
    destination = Image.new('RGBA', (4, 4), (9, 8, 7, 6))

    def singular():
        helper.copy_triangle(source, ((0, 0), (4, 0), (0, 4)), destination,
                             ((0, 0), (1, 1), (2, 2)))
        return image(destination)

    def luminance():
        canvas = Image.new('L', (4, 4), 77)
        helper.copy_triangle(canvas, ((0, 0), (3, 0), (0, 3)), canvas,
                             ((0, 0), (3, 0), (0, 3)))
        return image(canvas)

    result = {'singular': capture(singular), 'luminance': capture(luminance)}
    if corrected and package.__name__ == 'UnityPy':
        result['singular'] = {'value': image(Image.new('RGBA', (4, 4), (9, 8, 7, 6)))}
        result['luminance'] = {'error': 'NotImplementedError',
            'message': 'the sprite renderer is not verified on a sheet of mode L'}
    return result
