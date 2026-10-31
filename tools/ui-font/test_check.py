# Copyright 2026 Seraphina Studios
# SPDX-License-Identifier: MIT
"""Adversarial asset checks; no Pillow, Docker, network or host fonts needed."""
import json
from pathlib import Path
import shutil
import tempfile
import unittest

from check import check
from generate import digest

ASSETS = Path(__file__).resolve().parents[2] / "crates/tilewright-render-ui/assets"


class AssetChecks(unittest.TestCase):
    def test_bundled_asset(self):
        self.assertEqual(check(ASSETS)["glyphs"], 337)

    def test_corrupt_texture_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            assets = Path(tmp)
            shutil.copytree(ASSETS, assets, dirs_exist_ok=True)
            atlas = bytearray((assets / "atlas.atx").read_bytes())
            atlas[-1] ^= 1
            (assets / "atlas.atx").write_bytes(atlas)
            with self.assertRaisesRegex(ValueError, "generated checksum"):
                check(assets)

    def test_forged_manifest_cannot_authorize_out_of_bounds_frame(self):
        with tempfile.TemporaryDirectory() as tmp:
            assets = Path(tmp)
            shutil.copytree(ASSETS, assets, dirs_exist_ok=True)
            metrics = json.loads((assets / "metrics.json").read_text())
            metrics["glyphs"][1]["frame"] = [511, 0, 10, 10]
            blob = json.dumps(metrics).encode()
            (assets / "metrics.json").write_bytes(blob)
            manifest = json.loads((assets / "manifest.json").read_text())
            manifest["files"]["metrics.json"].update({"bytes": len(blob), "sha256": digest(blob)})
            (assets / "manifest.json").write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "frame bounds"):
                check(assets)


if __name__ == "__main__":
    unittest.main()
