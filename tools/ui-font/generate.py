#!/usr/bin/env python3
# Copyright 2026 Seraphina Studios
# SPDX-License-Identifier: MIT
"""Deterministic development-only rasterization of one explicit pinned font."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import struct

UPSTREAM = "895ec691990d041dd727c7b5afa3ce56525d98e6"
SOURCE_SHA256 = "af0bff7599c3df3831755c16e39b3c496df74b8c8d8a1161b14dc8461be17cb4"
NOTICE_SHA256 = "50ab8dd54680d3473f649c9db86fece88434d097c7834475c1c72d2f8c429215"
TOOL_IMAGE = "sha256:7abe7338937bc6609ebae586eb66e7a79ff8667d19f62cb1ed212cc1fbf7e13f"
SIZE = 20
WIDTH = 512
PADDING = 2


def digest(data):
    return hashlib.sha256(data).hexdigest()


def json_bytes(value):
    return (json.dumps(value, indent=2, ensure_ascii=True) + "\n").encode()


def read_source(path):
    """Read bounded cmap format 4/12 and name tables without ambient fonts."""
    data = path.read_bytes()
    if digest(data) != SOURCE_SHA256:
        raise ValueError("pinned font SHA-256 mismatch")
    count = struct.unpack_from(">H", data, 4)[0]
    tables = {}
    for i in range(count):
        tag, _, offset, length = struct.unpack_from(">4sIII", data, 12 + 16 * i)
        if offset + length > len(data):
            raise ValueError("invalid sfnt table bounds")
        tables[tag] = data[offset:offset + length]
    cmap = tables[b"cmap"]
    scalars = set()
    for i in range(struct.unpack_from(">H", cmap, 2)[0]):
        platform, encoding, offset = struct.unpack_from(">HHI", cmap, 4 + 8 * i)
        if platform != 0 and (platform, encoding) not in ((3, 1), (3, 10)):
            continue
        fmt = struct.unpack_from(">H", cmap, offset)[0]
        if fmt == 4:
            length, _, twice_count = struct.unpack_from(">HHH", cmap, offset + 2)
            table = cmap[offset:offset + length]
            segments = twice_count // 2
            for seg in range(segments):
                end = struct.unpack_from(">H", table, 14 + 2 * seg)[0]
                start = struct.unpack_from(">H", table, 16 + 2 * segments + 2 * seg)[0]
                delta = struct.unpack_from(">h", table, 16 + 4 * segments + 2 * seg)[0]
                range_pos = 16 + 6 * segments + 2 * seg
                range_offset = struct.unpack_from(">H", table, range_pos)[0]
                for scalar in range(start, min(end, 0xFFFE) + 1):
                    if range_offset:
                        glyph = struct.unpack_from(">H", table, range_pos + range_offset + 2 * (scalar - start))[0]
                        glyph = (glyph + delta) & 0xFFFF if glyph else 0
                    else:
                        glyph = (scalar + delta) & 0xFFFF
                    if glyph:
                        scalars.add(scalar)
        elif fmt == 12:
            groups = struct.unpack_from(">I", cmap, offset + 12)[0]
            for group in range(groups):
                start, end, glyph = struct.unpack_from(">III", cmap, offset + 16 + group * 12)
                if end > 0x10FFFF or end < start:
                    raise ValueError("invalid cmap group")
                scalars.update(range(start + (glyph == 0), end + 1))
    name_table = tables[b"name"]
    _, count, storage = struct.unpack_from(">HHH", name_table)
    names = {}
    for i in range(count):
        platform, _, _, name_id, length, offset = struct.unpack_from(">HHHHHH", name_table, 6 + i * 12)
        if name_id in (0, 1, 2, 5, 6, 13, 14):
            raw = name_table[storage + offset:storage + offset + length]
            value = raw.decode("utf-16-be" if platform in (0, 3) else "mac_roman")
            names.setdefault(str(name_id), set()).add(value)
    return sorted(scalars), {key: sorted(values) for key, values in names.items()}


def requested_scalars():
    # Deliberately excludes controls, combining marks, bidi and soft hyphen.
    extras = [0x2022, 0x2026, 0x2030, 0x2039, 0x203A, 0x20AC, 0x2122,
              0x2212, 0x25A1, 0xFFFD]
    return sorted(set(range(0x20, 0x7F)) | set(range(0xA0, 0xAD)) |
                  set(range(0xAE, 0x180)) | set(range(0x2010, 0x2016)) |
                  set(range(0x2018, 0x2020)) | set(range(0x2190, 0x2194)) | set(extras))


def generate(source_dir, output_dir):
    from PIL import Image, ImageFont, features, __version__
    if __version__ != "11.1.0" or features.version("freetype2") != "2.13.3":
        raise ValueError("use the pinned Pillow 11.1.0 / FreeType 2.13.3 image")
    source = source_dir / "RobotoMono-Regular.ttf"
    notice = source_dir / "OFL.txt"
    if digest(notice.read_bytes()) != NOTICE_SHA256:
        raise ValueError("pinned license SHA-256 mismatch")
    cmap, names = read_source(source)
    selected = [scalar for scalar in requested_scalars() if scalar in cmap]
    if 0xFFFD not in selected:
        raise ValueError("font lacks the required visible replacement character")
    font = ImageFont.truetype(io.BytesIO(source.read_bytes()), size=SIZE,
                              layout_engine=ImageFont.Layout.BASIC)
    glyphs, masks = [], []
    x, y, row_height = PADDING, PADDING, 0
    for scalar in selected:
        character = chr(scalar)
        mask, bearing = font.getmask2(character, mode="L", anchor="ls")
        width, height = mask.size
        ink = any(bytes(mask))
        frame = None
        if ink:
            if x + width + PADDING > WIDTH:
                x, y, row_height = PADDING, y + row_height + PADDING, 0
            frame = [x, y, width, height]
            masks.append((frame, bytes(mask)))
            x += width + PADDING
            row_height = max(row_height, height)
        glyphs.append({"scalar": scalar, "frame": frame,
                       "bearing": list(bearing), "advance": font.getlength(character)})
    height = 1 << (y + row_height + PADDING - 1).bit_length()
    image = Image.new("RGBA", (WIDTH, height), (255, 255, 255, 0))
    for (x, y, width, glyph_height), mask in masks:
        glyph_image = Image.new("RGBA", (width, glyph_height), (255, 255, 255, 0))
        glyph_image.putalpha(Image.frombytes("L", (width, glyph_height), mask))
        image.paste(glyph_image, (x, y))
    atlas = b"ATX1" + struct.pack("<II", WIDTH, height) + image.tobytes()
    solid = b"ATX1" + struct.pack("<II", 1, 1) + bytes([255] * 4)
    ascent, descent = font.getmetrics()
    metrics = {"schema": 1, "font": "Roboto Mono Regular", "license": "OFL-1.1",
               "native_size": SIZE, "ascent": ascent, "descent": descent,
               "line_gap": 0, "fallback": 0xFFFD, "atlas_size": [WIDTH, height],
               "supported_scalars": selected, "glyphs": glyphs}
    rust = ["// Generated by tools/ui-font/generate.py; do not edit.",
            "// Font data copyright 2015 The Roboto Mono Project Authors.",
            "// SPDX-License-Identifier: OFL-1.1", "",
            f"pub const ATLAS_SIZE: [u32; 2] = [{WIDTH}, {height}];",
            "pub const FALLBACK: char = '\\u{fffd}';",
            f"pub const METRICS: crate::FontMetrics = crate::FontMetrics {{ native_size: {SIZE}.0, ascent: {ascent}.0, descent: {descent}.0, line_gap: 0.0 }};"]
    for key, blob in (("ATLAS_SHA256", atlas), ("SOLID_SHA256", solid)):
        rust.append(f"pub const {key}: [u8; 32] = {list(hashlib.sha256(blob).digest())};")
    rust.append("pub const GLYPHS: &[crate::BitmapGlyph] = &[")
    for g in glyphs:
        frame = "None" if g["frame"] is None else "Some(tilewright_scene::TextureRect { x: %d, y: %d, width: %d, height: %d })" % tuple(g["frame"])
        rust.append("    crate::BitmapGlyph { character: '\\u{%x}', frame: %s, bearing: [%s, %s], advance: %s }," %
                    (g["scalar"], frame, repr(float(g["bearing"][0])), repr(float(g["bearing"][1])), repr(float(g["advance"]))))
    rust.append("];")
    files = {"atlas.atx": atlas, "solid.atx": solid, "metrics.json": json_bytes(metrics),
             "glyphs.rs": ("\n".join(rust) + "\n").encode(),
             "source-cmap.json": json_bytes({"scalars": cmap, "name_records": names})}
    manifest = {"schema": 1, "license": "OFL-1.1", "copyright": "Copyright 2015 The Roboto Mono Project Authors (https://github.com/googlefonts/robotomono)",
                "reserved_font_names": [], "reserved_font_names_evidence": "Pinned upstream OFL.txt declares none after the copyright statement.",
                "source": {"repository": "https://github.com/googlefonts/RobotoMono", "revision": UPSTREAM,
                           "path": "fonts/ttf/RobotoMono-Regular.ttf", "sha256": SOURCE_SHA256,
                           "url": f"https://github.com/googlefonts/RobotoMono/blob/{UPSTREAM}/fonts/ttf/RobotoMono-Regular.ttf"},
                "notice": {"path": "OFL.txt", "sha256": NOTICE_SHA256, "url": f"https://github.com/googlefonts/RobotoMono/blob/{UPSTREAM}/OFL.txt"},
                "generator": {"path": "tools/ui-font/generate.py", "sha256": digest(Path(__file__).read_bytes()), "image": TOOL_IMAGE,
                              "python": "3.13.5", "pillow": "11.1.0", "freetype": "2.13.3",
                              "layout": "BASIC; independent scalar masks, no shaping or kerning", "anchor": "ls (left baseline)",
                              "mode": "L mask to white RGBA8 with mask alpha", "native_size": SIZE,
                              "packing": "scalar-sorted shelves, width 512, two transparent pixels between frames, power-of-two height",
                              "runtime": "immutable ATX1/metrics only; no font lookup, parsing, rasterization or network"},
                "coverage": {"description": "Printable ASCII, available Latin-1 and Latin Extended-A, selected punctuation/arrows/symbols; see exact supported_scalars in metrics.json.",
                             "source_cmap_count": len(cmap), "selected_count": len(selected),
                             "unsupported": "Other scalars including combining marks, bidi controls and shaped scripts require explicit diagnostics; original UTF-8 is preserved by consumers.",
                             "omitted_requested_scalars": sorted(set(requested_scalars()) - set(selected))},
                "files": {name: {"sha256": digest(blob), "bytes": len(blob),
                                 "license": "MIT" if name == "solid.atx" else "OFL-1.1",
                                 "author": "Seraphina Studios" if name == "solid.atx" else "The Roboto Mono Project Authors",
                                 "source": "tools/ui-font/generate.py synthetic white RGBA constant" if name == "solid.atx" else f"https://github.com/googlefonts/RobotoMono/blob/{UPSTREAM}/fonts/ttf/RobotoMono-Regular.ttf",
                                 "transformation": {"atlas.atx": "Selected independent 20 px grayscale glyph masks packed as white RGBA8 ATX1",
                                                    "solid.atx": "Original synthetic 1x1 opaque white RGBA8 ATX1",
                                                    "metrics.json": "Selected scalar frames, baseline bearings and advances",
                                                    "glyphs.rs": "Rust constants generated from the same glyph metrics and texture hashes",
                                                    "source-cmap.json": "Exact mapped scalar and selected embedded name-table inventory"}[name],
                                 "notice": "Renderer MIT LICENSE" if name == "solid.atx" else "OFL.txt"}
                          for name, blob in files.items()}}
    files["manifest.json"] = json_bytes(manifest)
    output_dir.mkdir(parents=True, exist_ok=True)
    for name, blob in files.items():
        (output_dir / name).write_bytes(blob)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    generate(args.source_dir, args.output_dir)
