"""Original synthetic writer for the Unity Crunch fork, never an encoder port.

Reference: texture2ddecoder 1.0.5 src/Texture2DDecoder/unitycrunch/
crn_decomp.h:3102-3334 (palettes), :3352-3725 (block references).
Imported by texture_crunch.py; native blocks are calculated independently.
"""

from texture_crunch import Bits, Model, crc16, palettes


def fork_palettes(count, etc, alpha):
    streams, entries = palettes(count)
    if etc:
        bits = Bits()
        model = Model.uniform(32)
        model.write(bits)
        current = 0
        colors = []
        for i in range(count):
            for shift in range(0, 32, 8):
                delta = (i * 3 + shift + 1) & (7 if shift == 24 else 31)
                model.emit(bits, delta)
                current = (current + (delta << shift)) & 0xffffffff
            current &= 0x1f1f1f1f
            colors.append(current.to_bytes(4, 'little'))
        streams[0], entries[0] = bits.finish(), colors
    for kind in (1, 3) if alpha else (1,):
        bits = Bits()
        model = Model.uniform(16 if kind == 1 else 64)
        model.write(bits)
        previous = [0] * 8
        selectors = []
        for i in range(count):
            values = []
            for j in range(8):
                delta = (i * 7 + j * 3 + 1) & (15 if kind == 1 else 63)
                model.emit(bits, delta)
                previous[j] ^= delta
                values += [previous[j] & (3 if kind == 1 else 7),
                           previous[j] >> (2 if kind == 1 else 3)]
            if kind == 1 and not etc:
                selectors.append(sum((0, 2, 3, 1)[s] << (2*p) for p, s in enumerate(values)).to_bytes(4, 'little'))
            elif kind == 3 and not etc:
                selectors.append(sum((0, 2, 3, 4, 5, 6, 7, 1)[s] << (3*p) for p, s in enumerate(values)).to_bytes(6, 'little'))
            elif kind == 1:
                orientations = []
                for flip in range(2):
                    word = 0
                    for h in range(4):
                        for w in range(4):
                            s = values[h*4+w if flip else w*4+h]
                            # Linear -> ETC sign/magnitude, transposed byte order.
                            low, high = (~(s ^ (s >> 1))) & 1, (~s >> 1) & 1
                            t = (8 + h + 4*w) & 15
                            word |= (high | low << 16) << t
                    orientations.append(word.to_bytes(4, 'little'))
                selectors.append(orientations)
            else:
                orientations = []
                for flip in range(2):
                    word = 0
                    for p in range(16):
                        s = values[p]
                        s = 3-s if s <= 3 else s
                        position = 4*(p % 4) + p//4 if flip else p
                        word |= s << (45 - 3*position)
                    orientations.append(word.to_bytes(6, 'big'))
                selectors.append(orientations)
        streams[kind], entries[kind] = bits.finish(), selectors
    return streams, entries


def color_block(e0, e1, selector, flip):
    diff = all(-4 <= b-a <= 3 for a, b in zip(e0[:3], e1[:3]))
    colors = bytes(((a << 3 | ((b-a) & 7)) if diff else
                    ((a << 3 & 240) | b >> 1)) & 255 for a, b in zip(e0[:3], e1[:3]))
    return colors + bytes(((e0[3] << 5 | e1[3] << 2 | int(diff) << 1 | flip) & 255,)) + selector[flip]


def fork_level(width, height, etc, alpha, count, models, entries, phase=0, edge=False):
    bx, by = (width+3)//4, (height+3)//4
    nx, ny = (bx+1)&~1, (by+1)&~1
    out = Bits()
    # Previous-row endpoints, and the saved bottom-row reference.
    row = [[0, 0, 0, 0] for _ in range(nx)]
    ce = ae = diagonal_ce = diagonal_ae = 0
    blocks = [None] * (bx*by)
    group = 0
    for y in range(ny):
        for x in range(nx):
            if y % 2 == 0 and (etc or x % 2 == 0):
                group = (0xE4, 0x1B, 0x72, 0x8D)[(y//2+x+phase) % 4]
                # The reference's row struct has no initializer. First-row
                # references must not read previous-row or diagonal storage.
                if y == 0:
                    group &= ~2 if etc else 0x55
                    if etc and x == 0:
                        group &= ~8
                if edge:
                    group = 0
                models[0].emit(out, group)
            if y % 2:
                reference = row[x][0]
            elif etc:
                reference = (group & 3) | ((group >> 2) & 12)
                row[x][0] = ((group >> 2) & 3) | ((group >> 4) & 12)
            else:
                reference = group & 3
                group >>= 2
                row[x][0] = group & 3
                group >>= 2
            if reference & 3 == 0:
                delta = (x+y+1) % count
                models[1].emit(out, delta)
                ce = (ce + delta) % count
                if alpha:
                    models[3].emit(out, delta)
                    ae = (ae + delta) % count
                row[x][1:3] = [ce, ae]
            elif reference & 3 == 1:
                row[x][1:3] = [ce, ae]
            elif etc and reference & 3 == 3:
                ce, ae = diagonal_ce, diagonal_ae
                row[x][1:3] = [ce, ae]
            else:
                ce, ae = row[x][1:3]
            e0 = entries[0][ce]
            cs = (x+2*y) % count
            ass = (2*x+y) % count
            if edge and (x >= bx or y >= by):
                cs = ass = count + 1
            models[2].emit(out, cs)
            if alpha:
                models[4].emit(out, ass)
            if etc:
                if reference >> 2:
                    delta = (x+1) % count
                    models[1].emit(out, delta)
                    ce = (ce+delta) % count
                diagonal_ce, diagonal_ae = row[x][3] & 0xffff, row[x][3] >> 16
                row[x][3] = ce | ae << 16
            if x < bx and y < by:
                flip = (reference >> 3) ^ 1
                color = color_block(e0, entries[0][ce], entries[1][cs], flip) if etc else e0 + entries[1][cs]
                a = entries[2][ae] + (entries[3][ass][flip] if etc else entries[3][ass]) if alpha else b''
                blocks[y*bx+x] = a+color
    return out.finish(), {'width': width, 'height': height, 'blocks': b''.join(blocks).hex()}


def build_fork(width, height, kind, count=3, mips=1, edge=False):
    etc, alpha = kind >= 10, kind in (2, 12)
    streams, entries = fork_palettes(count, etc, alpha)
    delta = Model.uniform(max(count, 4) if edge else count)
    selector = Model.uniform(count+2 if edge else count)
    models = [Model.uniform(256), delta, selector, delta, selector]
    tables = Bits()
    for model in models[:5 if alpha else 3]:
        model.write(tables)
    pieces = streams[:4 if alpha else 2] + [tables.finish()]
    levels = []
    for mip in range(mips):
        stream, expected = fork_level(max(1, width >> mip), max(1, height >> mip), etc, alpha,
                                      count, models, entries, mip, edge)
        pieces.append(stream)
        levels.append(expected)
    size = 70 + 4*mips
    offsets = []
    for piece in pieces:
        offsets.append(size)
        size += len(piece)
    header = bytearray(b'Hx' + (70+4*mips).to_bytes(2, 'big') + bytes(2))
    payload = b''.join(pieces)
    header += size.to_bytes(4, 'big') + crc16(payload).to_bytes(2, 'big')
    header += width.to_bytes(2, 'big') + height.to_bytes(2, 'big') + bytes((mips, 1, kind)) + bytes(14)
    for i in range(4):
        header += (offsets[i].to_bytes(3, 'big') + len(pieces[i]).to_bytes(3, 'big') + count.to_bytes(2, 'big')) if i < (4 if alpha else 2) else bytes(8)
    table = 4 if alpha else 2
    header += len(pieces[table]).to_bytes(2, 'big') + offsets[table].to_bytes(3, 'big')
    header += b''.join(offset.to_bytes(4, 'big') for offset in offsets[table+1:])
    header[4:6] = crc16(header[6:]).to_bytes(2, 'big')
    return bytes(header)+payload, levels


def cases():
    import hashlib
    rows = []
    for kind, fmt in ((0, 28), (2, 29), (10, 64), (12, 65)):
        for width, height, count, mips, edge in ((1, 1, 1, 1, False), (3, 5, 3, 1, False),
            (7, 9, 3, 1, False), (33, 29, 17, 6, False), (17, 33, 3, 5, False),
            (1, 1, 1, 1, True), (1, 9, 1, 1, True)):
            data, levels = build_fork(width, height, kind, count, mips, edge)
            rows.append({'id': f'Unity-{kind}-{width}x{height}-m{mips}-edge{int(edge)}',
                         'format': fmt, 'unity': True, 'width': width, 'height': height,
                         'data': {'hex': data.hex()}, 'data_sha256': hashlib.sha256(data).hexdigest(), 'levels': levels})
        # Zero-extension with valid palette/table ranges; one-byte last mip.
        data, levels = build_fork(17, 17, kind, count=1)
        start = int.from_bytes(data[70:74], 'big')
        data = bytearray(data[:start+1])
        data[-1] = 0
        data[6:10] = len(data).to_bytes(4, 'big')
        # Every reference/delta/selector is zero in the infinite zero tail.
        zero_data, zero_levels = build_fork(1, 1, kind, count=1, edge=True)
        block = bytes.fromhex(zero_levels[0]['blocks'])
        levels[0]['blocks'] = (block*25).hex()
        rows.append({'id': f'Unity-{kind}-truncated-level', 'format': fmt, 'unity': True,
                     'width': 17, 'height': 17, 'data': {'hex': data.hex()},
                     'data_sha256': hashlib.sha256(data).hexdigest(), 'levels': levels})
        data, levels = build_fork(7, 9, kind)
        data = bytearray(data)
        data[6:10] = (len(data)-1).to_bytes(4, 'big')
        rows.append({'id': f'Unity-{kind}-underdeclared-data-size', 'format': fmt, 'unity': True,
                     'width': 7, 'height': 9, 'data': {'hex': data.hex()},
                     'data_sha256': hashlib.sha256(data).hexdigest(), 'levels': levels})
    for kind, fmt in ((10, 64), (12, 65)):
        data, levels = build_fork(1, 1, kind, count=1, edge=True)
        start = int.from_bytes(data[70:74], 'big')
        data = bytearray(data[:start+1])
        data[-1] = 3  # Initial diagonal predictor is explicitly palette 0.
        data[6:10] = len(data).to_bytes(4, 'big')
        rows.append({'id': f'Unity-{kind}-initial-diagonal', 'format': fmt, 'unity': True,
                     'width': 1, 'height': 1, 'data': {'hex': data.hex()},
                     'data_sha256': hashlib.sha256(data).hexdigest(), 'levels': levels})
    data, levels = build_fork(7, 9, 11)
    rows.append({'id': 'Unity-11-ETC2-colour', 'format': 64, 'unity': True,
                 'width': 7, 'height': 9, 'data': {'hex': data.hex()},
                 'data_sha256': hashlib.sha256(data).hexdigest(), 'levels': levels})
    # decode_palettes ignores both alpha descriptors without alpha endpoints
    # (texture2ddecoder 1.0.5 unitycrunch/crn_decomp.h:3131-3147).
    data, levels = build_fork(1, 1, 0, count=1, edge=True)
    data = bytearray(data)
    data[57:65] = bytes.fromhex('00004a0000010001')
    data[4:6] = crc16(data[6:74]).to_bytes(2, 'big')
    rows.append({'id': 'Unity-0-unused-alpha-selector', 'format': 28, 'unity': True,
                 'width': 1, 'height': 1, 'data': {'hex': data.hex()},
                 'data_sha256': hashlib.sha256(data).hexdigest(), 'levels': levels})
    # The same endpoint gate ignores even out-of-buffer/zero-size ranges.
    # Cover DXT and ETC, preserving the existing unused-alpha-selector bytes.
    for kind, fmt in ((0, 28), (10, 64)):
        for label, descriptors in (("endpoint", (2,)), ("selector", (3,)), ("both", (2, 3))):
            data, levels = build_fork(1, 1, kind, count=1, edge=True)
            data = bytearray(data)
            for i in descriptors:
                data[33+8*i:41+8*i] = bytes.fromhex(
                    'ffffff0000000000' if i == 2 else 'ffffff0000000001')
            data[4:6] = crc16(data[6:74]).to_bytes(2, 'big')
            rows.append({'id': f'Unity-{kind}-ignored-alpha-{label}-range', 'format': fmt,
                         'unity': True, 'width': 1, 'height': 1, 'data': {'hex': data.hex()},
                         'data_sha256': hashlib.sha256(data).hexdigest(), 'levels': levels})
    return rows
