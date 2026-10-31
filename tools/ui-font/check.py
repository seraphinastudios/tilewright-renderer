#!/usr/bin/env python3
# Copyright 2026 Seraphina Studios
# SPDX-License-Identifier: MIT
"""Dependency-free validation of the bundled OFL font asset and its bounds."""
import argparse
import json
import math
from pathlib import Path
import re
import struct

from generate import NOTICE_SHA256, SOURCE_SHA256, TOOL_IMAGE, UPSTREAM, digest, read_source, requested_scalars


def require(condition, message):
    if not condition:
        raise ValueError(message)


def check(assets):
    manifest = json.loads((assets / "manifest.json").read_text())
    require(manifest["schema"] == 1, "manifest schema")
    require(manifest["license"] == "OFL-1.1", "font data must retain OFL-1.1")
    require(manifest["source"]["revision"] == UPSTREAM, "source revision")
    require(manifest["source"]["sha256"] == SOURCE_SHA256, "source hash record")
    require(manifest["notice"]["sha256"] == NOTICE_SHA256, "notice hash record")
    require(manifest["generator"]["image"] == TOOL_IMAGE, "tool image")
    require(manifest["generator"]["sha256"] == digest(Path(__file__).with_name("generate.py").read_bytes()), "generator source checksum")
    require(digest((assets / "OFL.txt").read_bytes()) == NOTICE_SHA256, "notice checksum")
    require(manifest["reserved_font_names"] == [], "unexpected reserved names")
    expected_files = {"atlas.atx", "solid.atx", "metrics.json", "glyphs.rs", "source-cmap.json"}
    require(set(manifest["files"]) == expected_files, "exact generated file set")
    for name, entry in manifest["files"].items():
        blob = (assets / name).read_bytes()
        require(len(blob) == entry["bytes"] and digest(blob) == entry["sha256"], f"generated checksum: {name}")
        require(entry["license"] == ("MIT" if name == "solid.atx" else "OFL-1.1"), f"asset license: {name}")
        require(entry["author"] == ("Seraphina Studios" if name == "solid.atx" else "The Roboto Mono Project Authors"), f"asset author: {name}")
        require(entry["source"] and entry["transformation"] and entry["notice"], f"asset provenance: {name}")
    cmap, names = read_source(assets / "RobotoMono-Regular.ttf")
    require(json.loads((assets / "source-cmap.json").read_text()) == {"scalars": cmap, "name_records": names}, "source cmap/name records")
    require(names["0"] == [manifest["copyright"]], "embedded copyright")
    require(names["1"] == ["Roboto Mono"] and names["2"] == ["Regular"], "font face identity")
    metrics = json.loads((assets / "metrics.json").read_text())
    expected_scalars = [scalar for scalar in requested_scalars() if scalar in cmap]
    require(metrics["supported_scalars"] == expected_scalars, "exact supported scalar set")
    require(manifest["coverage"]["source_cmap_count"] == len(cmap), "source cmap count")
    require(manifest["coverage"]["selected_count"] == len(expected_scalars), "selected scalar count")
    require(manifest["coverage"]["omitted_requested_scalars"] == sorted(set(requested_scalars()) - set(cmap)), "missing scalar record")
    require(metrics["license"] == "OFL-1.1" and metrics["schema"] == 1, "metrics license/schema")
    require(metrics["native_size"] == 20 and metrics["ascent"] > 0 and metrics["descent"] >= 0 and metrics["line_gap"] >= 0, "font vertical metrics")
    atlas = (assets / "atlas.atx").read_bytes()
    require(atlas[:4] == b"ATX1", "atlas header")
    width, height = struct.unpack_from("<II", atlas, 4)
    require([width, height] == metrics["atlas_size"] and width == 512 and 0 < height <= 1024, "atlas dimensions")
    require(len(atlas) == 12 + width * height * 4, "atlas byte count")
    require((assets / "solid.atx").read_bytes() == b"ATX1" + struct.pack("<II", 1, 1) + b"\xff" * 4, "solid texture")
    glyphs = metrics["glyphs"]
    require([g["scalar"] for g in glyphs] == expected_scalars, "sorted unique glyphs")
    occupied = set()
    for glyph in glyphs:
        require(len(glyph["bearing"]) == 2 and all(math.isfinite(v) and abs(v) <= 64 for v in glyph["bearing"]), "bounded finite bearing")
        require(math.isfinite(glyph["advance"]) and 0 < glyph["advance"] <= 64, "bounded positive advance")
        frame = glyph["frame"]
        if frame is None:
            require(glyph["scalar"] in (0x20, 0xA0), "only whitespace omits ink")
            continue
        require(len(frame) == 4 and all(isinstance(v, int) for v in frame), "integer frame")
        x, y, w, h = frame
        require(x >= 2 and y >= 2 and w > 0 and h > 0 and x + w <= width - 2 and y + h <= height - 2, "frame bounds/padding")
        pixels = {row * width + col for row in range(y, y + h) for col in range(x, x + w)}
        require(not occupied.intersection(pixels), "overlapping frames")
        occupied.update(pixels)
        require(any(atlas[12 + p * 4 + 3] for p in pixels), "empty visible glyph")
    require(metrics["fallback"] == 0xFFFD and next(g for g in glyphs if g["scalar"] == 0xFFFD)["frame"] is not None, "visible fallback")
    require(next(g for g in glyphs if g["scalar"] == 0x20)["frame"] is None, "space advances without quad")
    for p in range(width * height):
        require(atlas[12 + 4 * p:15 + 4 * p] == b"\xff" * 3, "white atlas RGB")
        if p not in occupied:
            require(atlas[15 + 4 * p] == 0, "transparent atlas padding")
    rust = (assets / "glyphs.rs").read_text()
    require("SPDX-License-Identifier: OFL-1.1" in rust, "generated Rust font-data license")
    entries = re.findall(r"character: '\\u\{([0-9a-f]+)\}', frame: (.*?), bearing: \[([^]]+)\], advance: ([0-9.]+) \},", rust)
    require(len(entries) == len(glyphs), "Rust glyph count")
    for (scalar, frame, bearing, advance), glyph in zip(entries, glyphs):
        require(int(scalar, 16) == glyph["scalar"], "Rust scalar")
        actual_frame = None if frame == "None" else [int(v) for v in re.findall(r"(?:x|y|width|height): (\d+)", frame)]
        require(actual_frame == glyph["frame"], "Rust frame")
        require([float(v.strip()) for v in bearing.split(",")] == glyph["bearing"] and float(advance) == glyph["advance"], "Rust glyph metrics")
    for key, file in (("ATLAS_SHA256", "atlas.atx"), ("SOLID_SHA256", "solid.atx")):
        values = re.search(key + r": \[u8; 32\] = \[([^]]+)\]", rust)
        require(values is not None and bytes(int(v.strip()) for v in values[1].split(",")).hex() == manifest["files"][file]["sha256"], "Rust texture hash")
    return {"glyphs": len(glyphs), "source_scalars": len(cmap), "atlas_size": [width, height], "atlas_sha256": digest(atlas)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, default=Path(__file__).resolve().parents[2] / "crates/tilewright-render-ui/assets")
    args = parser.parse_args()
    print(json.dumps(check(args.assets), sort_keys=True))
