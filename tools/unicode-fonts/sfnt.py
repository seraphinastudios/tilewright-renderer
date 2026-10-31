# Copyright 2026 Seraphina Studios
# SPDX-License-Identifier: MIT
"""Bounded independent SFNT cmap/name inventory; no rasterizer or host fonts."""
import struct


def require(condition, message):
    if not condition:
        raise ValueError(message)


def part(data, offset, length):
    require(0 <= offset <= len(data) and 0 <= length <= len(data) - offset,
            'SFNT bounds')
    return data[offset:offset + length]


def unpack(fmt, data, offset=0):
    return struct.unpack(fmt, part(data, offset, struct.calcsize(fmt)))


def inventory(data):
    require(len(data) <= 24 * 1024 * 1024, 'font byte limit')
    require(part(data, 0, 4) in (b'\x00\x01\x00\x00', b'OTTO'), 'SFNT header')
    count, = unpack('>H', data, 4)
    require(0 < count <= 128, 'SFNT table count')
    tables = {}
    for i in range(count):
        tag, _, offset, length = unpack('>4sIII', data, 12 + i * 16)
        require(tag not in tables, 'duplicate SFNT table')
        tables[tag] = part(data, offset, length)
    require(b'cmap' in tables and b'name' in tables, 'required SFNT tables')
    cmap = tables[b'cmap']
    version, count = unpack('>HH', cmap)
    require(version == 0 and 0 < count <= 128, 'cmap header')
    scalars = set()
    visited = set()
    for i in range(count):
        platform, encoding, offset = unpack('>HHI', cmap, 4 + i * 8)
        if platform != 0 and (platform, encoding) not in ((3, 1), (3, 10)):
            continue
        if offset in visited:
            continue
        visited.add(offset)
        fmt, = unpack('>H', cmap, offset)
        if fmt == 4:
            length, _, twice = unpack('>HHH', cmap, offset + 2)
            table = part(cmap, offset, length)
            require(twice > 0 and twice % 2 == 0, 'cmap segments')
            segments = twice // 2
            previous = -1
            for seg in range(segments):
                end, = unpack('>H', table, 14 + 2 * seg)
                start, = unpack('>H', table, 16 + 2 * segments + 2 * seg)
                delta, = unpack('>h', table, 16 + 4 * segments + 2 * seg)
                position = 16 + 6 * segments + 2 * seg
                relative, = unpack('>H', table, position)
                require(previous < start <= end, 'cmap sorted segments')
                previous = end
                for scalar in range(start, min(end, 0xFFFE) + 1):
                    if relative:
                        glyph, = unpack('>H', table, position + relative + 2 * (scalar - start))
                        glyph = (glyph + delta) & 0xFFFF if glyph else 0
                    else:
                        glyph = (scalar + delta) & 0xFFFF
                    if glyph:
                        scalars.add(scalar)
        elif fmt == 12:
            length, _, groups = unpack('>III', cmap, offset + 4)
            table = part(cmap, offset, length)
            require(groups <= 100000, 'cmap groups limit')
            previous = -1
            for group in range(groups):
                start, end, glyph = unpack('>III', table, 16 + group * 12)
                require(previous < start <= end <= 0x10FFFF, 'cmap group bounds/order')
                require(glyph + end - start <= 0xFFFF, 'cmap glyph range')
                previous = end
                scalars.update(range(start + (glyph == 0), end + 1))
        else:
            # Format 14 is variation sequences, not scalar cmap coverage.
            require(fmt == 14, 'unsupported Unicode cmap format')
    require(scalars and not any(0xD800 <= x <= 0xDFFF for x in scalars), 'Unicode scalars')
    table = tables[b'name']
    fmt, count, storage = unpack('>HHH', table)
    require(fmt in (0, 1) and count <= 16384, 'name header')
    names = {}
    for i in range(count):
        platform, _, _, key, length, offset = unpack('>HHHHHH', table, 6 + i * 12)
        if key not in (0, 1, 2, 5, 6, 13, 14):
            continue
        require(platform in (0, 1, 3), 'name encoding')
        value = part(table, storage + offset, length).decode('utf-16-be' if platform in (0, 3) else 'mac_roman')
        names.setdefault(str(key), set()).add(value)
    ranges = []
    for scalar in sorted(scalars):
        if ranges and ranges[-1][1] + 1 == scalar:
            ranges[-1][1] = scalar
        else:
            ranges.append([scalar, scalar])
    return {'scalar_count': len(scalars), 'ranges': ranges,
            'name_records': {key: sorted(value) for key, value in sorted(names.items())},
            'tables': sorted(tag.decode('ascii') for tag in tables)}
