"""Cold-copy a closed local market-process store and compare its re-folded business state."""

import hashlib
import json
import shutil
import subprocess
import tempfile
from pathlib import Path


class ProcessMismatch(ValueError):
    """A valid checkpoint re-folds to a different business process or message."""


def _read(binary: Path, command: str, directory: Path) -> dict:
    result = subprocess.run(
        [binary, command, directory], stdout=subprocess.PIPE, text=True, check=True
    )
    return json.loads(result.stdout)


def _digest(directory: Path) -> str:
    """Bind object names and contents, including the relative directory structure."""
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("engine checkpoint must be a local directory")
    files = []
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError("engine checkpoint contains a symbolic link")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError("engine checkpoint contains a non-regular file")
        with path.open("rb") as stream:
            digest = hashlib.sha256()
            for chunk in iter(lambda: stream.read(65536), b""):
                digest.update(chunk)
        files.append((path.relative_to(directory).as_posix(), digest.hexdigest()))
    return hashlib.sha256(json.dumps(files, separators=(",", ":")).encode()).hexdigest()


def capture(binary: Path, work_dir: Path) -> dict:
    archive = work_dir / "makod-checkpoint"
    with tempfile.TemporaryDirectory(dir=work_dir) as scratch:
        source = Path(scratch) / "source"
        evidence = _read(binary, "seed", source)
        checkpoint = _digest(source)
        shutil.copytree(source, archive)
    return {"tree_sha256": checkpoint, "process": evidence}


def restore(binary: Path, work_dir: Path, snapshot: dict) -> None:
    archive = work_dir / "makod-checkpoint"
    if not archive.is_dir() or _digest(archive) != snapshot["tree_sha256"]:
        raise ValueError("engine checkpoint missing or changed")
    with tempfile.TemporaryDirectory(dir=work_dir) as scratch:
        target = Path(scratch) / "restored"
        shutil.copytree(archive, target)
        if _digest(target) != snapshot["tree_sha256"]:
            raise ValueError("restored engine tree differs from checkpoint")
        if _read(binary, "inspect", target) != snapshot["process"]:
            raise ProcessMismatch(
                "restored engine process or pending protocol message differs"
            )
