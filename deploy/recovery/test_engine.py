"""The fixture opens a restored engine only after the whole private checkpoint matches."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import engine


class CheckpointTests(unittest.TestCase):
    def test_modified_or_missing_checkpoint_is_refused_before_open(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.object(engine, "_read") as reader,
        ):
            root = Path(tmp)
            checkpoint = root / "makod-checkpoint"
            checkpoint.mkdir()
            data = checkpoint / "object"
            data.write_bytes(b"saved")
            manifest = {"tree_sha256": engine._digest(checkpoint), "process": {}}
            data.write_bytes(b"other")
            with self.assertRaises(ValueError):
                engine.restore(Path("unused"), root, manifest)
            data.unlink()
            checkpoint.rmdir()
            with self.assertRaises(ValueError):
                engine.restore(Path("unused"), root, manifest)
            reader.assert_not_called()

    def test_checkpoint_refuses_a_link_inside_or_in_place_of_its_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            target = root / "target"
            target.mkdir()
            (root / "alias").symlink_to(target, target_is_directory=True)
            with self.assertRaises(ValueError):
                engine._digest(root / "alias")
            (target / "file").symlink_to(root / "outside")
            with self.assertRaises(ValueError):
                engine._digest(target)
