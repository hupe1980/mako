"""Read a trusted local export and a read-only POSIX mirror snapshot.

Every path component is opened relative to its pinned parent with O_NOFOLLOW.
A second tree walk checks names and metadata, including the configured root.
A writable tree cannot provide an atomic view across multiple filesystem calls.
Use a local immutable snapshot or stop all writers for the run. The operator
controls the mount namespace and excludes nested bind mounts. Regular files
have one link, and the mirror stays on the root file system.
"""

from __future__ import annotations

import errno
import hashlib
import json
import math
import os
import stat
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path, PureWindowsPath
from typing import NamedTuple

_MAX_MANIFEST = 8 << 20
_MAX_MESSAGE = 16 << 20
_MAX_BYTES = 1 << 30
_MAX_FILES = 10_000
_MAX_DIRS = 10_000
_MAX_DEPTH = 32
_MAX_JSON_DEPTH = 64
_MAX_PATH_BYTES = 4096
_CHUNK = 64 << 10

_SUPPORTED = (
    os.name == "posix"
    and all(hasattr(os, name) for name in ("O_NOFOLLOW", "O_DIRECTORY", "O_NONBLOCK"))
    and os.open in os.supports_dir_fd
    and os.stat in os.supports_dir_fd
    and os.stat in os.supports_follow_symlinks
    and os.scandir in os.supports_fd
)


class ShadowInputError(ValueError):
    """A portable refusal containing only an allowlisted reason code."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


def validate_relative(path: str) -> None:
    """Admit an exact, bounded relative POSIX path shared by inventory and mirror."""
    if not isinstance(path, str) or "\x00" in path or "\\" in path:
        raise ShadowInputError("unsafe_path")
    if len(path) > _MAX_PATH_BYTES:
        raise ShadowInputError("input_limit_exceeded")
    parts = path.split("/")
    if any(part in ("", ".", "..") for part in parts) or PureWindowsPath(path).drive:
        raise ShadowInputError("unsafe_path")
    try:
        size = len(path.encode("utf-8"))
    except UnicodeEncodeError:
        raise ShadowInputError("unsafe_path") from None
    if len(parts) > _MAX_DEPTH or size > _MAX_PATH_BYTES:
        raise ShadowInputError("input_limit_exceeded")


class _Stamp(NamedTuple):
    device: int
    inode: int
    mode: int
    links: int
    size: int
    modified: int
    changed: int


def _stamp(info: os.stat_result) -> _Stamp:
    return _Stamp(
        info.st_dev,
        info.st_ino,
        info.st_mode,
        info.st_nlink,
        info.st_size,
        info.st_mtime_ns,
        info.st_ctime_ns,
    )


def _unchanged(expected: _Stamp, observed: _Stamp) -> None:
    if expected != observed:
        raise ShadowInputError("snapshot_changed")


@contextmanager
def _filesystem() -> Iterator[None]:
    if not _SUPPORTED:
        raise ShadowInputError("unsupported_platform")
    try:
        yield
    except OSError as error:
        code = (
            "unsafe_path"
            if error.errno in {errno.ELOOP, errno.ENOTDIR}
            else "input_unavailable"
        )
        raise ShadowInputError(code) from None


def _absolute(path: Path) -> Path:
    if ".." in path.parts or "\x00" in str(path) or path.anchor == "//":
        raise ShadowInputError("unsafe_path")
    absolute = path.absolute()
    try:
        size = len(str(absolute).encode("utf-8"))
    except UnicodeEncodeError:
        raise ShadowInputError("unsafe_path") from None
    if size > _MAX_PATH_BYTES:
        raise ShadowInputError("input_limit_exceeded")
    return absolute


@contextmanager
def _directory(parts: tuple[str, ...], parent: int | None = None) -> Iterator[int]:
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY
    fd = os.open("/", flags) if parent is None else os.dup(parent)
    try:
        for name in parts:
            child = os.open(name, flags, dir_fd=fd)
            os.close(fd)
            fd = child
        yield fd
    finally:
        os.close(fd)


def _named_stamp(parent: int, name: str) -> _Stamp:
    return _stamp(os.stat(name, dir_fd=parent, follow_symlinks=False))


def _manifest_stamp(path: Path) -> _Stamp:
    with _directory(path.parent.parts[1:]) as parent:
        info = _named_stamp(parent, path.name)
        if not stat.S_ISREG(info.mode) or info.links != 1:
            raise ShadowInputError("unsafe_path")
        return info


def _read_regular(
    parent: int, name: str, expected: _Stamp, limit: int
) -> Iterator[bytes]:
    if not stat.S_ISREG(expected.mode) or expected.links != 1:
        raise ShadowInputError("unsafe_path")
    if expected.size > limit:
        raise ShadowInputError("input_limit_exceeded")
    _unchanged(expected, _named_stamp(parent, name))
    # Nonblocking avoids waiting on a FIFO substituted between stat and open.
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | getattr(os, "O_NOCTTY", 0)
    fd = os.open(name, flags, dir_fd=parent)
    try:
        _unchanged(expected, _stamp(os.fstat(fd)))
        count = 0
        while chunk := os.read(fd, min(_CHUNK, limit + 1 - count)):
            count += len(chunk)
            if count > limit:
                raise ShadowInputError("input_limit_exceeded")
            if count > expected.size:
                raise ShadowInputError("snapshot_changed")
            yield chunk
        if count != expected.size:
            raise ShadowInputError("snapshot_changed")
        _unchanged(expected, _stamp(os.fstat(fd)))
        _unchanged(expected, _named_stamp(parent, name))
    finally:
        os.close(fd)


def _unique_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ShadowInputError("invalid_json")
        result[key] = value
    return result


def _finite_number(token: str) -> float:
    number = float(token)
    if not math.isfinite(number):
        raise ShadowInputError("invalid_json")
    return number


def _bound_json_depth(data: bytes) -> None:
    # Bound nesting before allocating containers in the platform JSON decoder.
    depth = 0
    quoted = escaped = False
    for byte in data:
        if quoted:
            if escaped:
                escaped = False
            elif byte == 92:  # Backslash
                escaped = True
            elif byte == 34:  # Double quote
                quoted = False
        elif byte == 34:
            quoted = True
        elif byte in b"{[":
            depth += 1
            if depth > _MAX_JSON_DEPTH:
                raise ShadowInputError("input_limit_exceeded")
        elif byte in b"}]":
            depth -= 1


def read_manifest(path: Path, expected: _Stamp | None = None) -> object:
    """Read at most 8 MiB of UTF-8 JSON from one stable regular file."""
    with _filesystem():
        absolute = _absolute(path)
        if not absolute.name:
            raise ShadowInputError("unsafe_path")
        with _directory(absolute.parent.parts[1:]) as parent:
            if expected is None:
                expected = _named_stamp(parent, absolute.name)
            data = b"".join(_read_regular(parent, absolute.name, expected, _MAX_MANIFEST))
        _bound_json_depth(data)
        try:
            result = json.loads(
                data.decode("utf-8"),
                object_pairs_hook=_unique_keys,
                parse_float=_finite_number,
                parse_constant=_finite_number,
            )
        except (ValueError, RecursionError):
            raise ShadowInputError("invalid_json") from None
        with _directory(absolute.parent.parts[1:]) as parent:
            _unchanged(expected, _named_stamp(parent, absolute.name))
        return result


def _inventory(root: int) -> dict[str, _Stamp]:
    entries = {}
    files = directories = total = 0
    device = os.fstat(root).st_dev

    def walk(parent: int, prefix: str) -> None:
        nonlocal files, directories, total
        expected = _stamp(os.fstat(parent))
        entries[prefix] = expected
        with os.scandir(parent) as children:
            for child in children:
                path = prefix + "/" + child.name if prefix else child.name
                validate_relative(path)
                item = _named_stamp(parent, child.name)
                if item.device != device:
                    raise ShadowInputError("unsafe_path")
                if stat.S_ISDIR(item.mode):
                    directories += 1
                    if directories > _MAX_DIRS:
                        raise ShadowInputError("input_limit_exceeded")
                    with _directory((child.name,), parent) as fd:
                        _unchanged(item, _stamp(os.fstat(fd)))
                        walk(fd, path)
                elif stat.S_ISREG(item.mode):
                    if item.links != 1:
                        raise ShadowInputError("unsafe_path")
                    files += 1
                    total += item.size
                    if (
                        files > _MAX_FILES
                        or item.size > _MAX_MESSAGE
                        or total > _MAX_BYTES
                    ):
                        raise ShadowInputError("input_limit_exceeded")
                    entries[path] = item
                else:
                    raise ShadowInputError("unsafe_path")
        _unchanged(expected, _stamp(os.fstat(parent)))

    walk(root, "")
    return entries


class Snapshot:
    """Verify the full mirror tree before and after reading its indexed content."""

    def __init__(self, path: Path):
        self._path = path
        self._root: int | None = None
        self._entries: dict[str, _Stamp] = {}
        self._digests: dict[str, str] = {}
        self._manifests: dict[Path, _Stamp] = {}

    def __enter__(self) -> Snapshot:
        if self._root is not None:
            raise ShadowInputError("input_unavailable")
        with _filesystem():
            self._path = _absolute(self._path)
            with _directory(self._path.parts[1:]) as fd:
                self._entries = _inventory(fd)
                self._digests.clear()
                self._manifests.clear()
                self._root = os.dup(fd)
        return self

    def read_manifests(self, journal: Path, inventory: Path) -> tuple[object, object]:
        """Read the same two external inputs whose identities are verified at exit."""
        if self._root is None or self._manifests:
            raise ShadowInputError("input_unavailable")
        source, mirror = self._read_inputs(journal, inventory)
        return source, mirror

    def read_external(self, path: Path) -> object:
        """Read an additional evidence file pinned until snapshot exit."""
        return self._read_inputs(path)[0]

    def _read_inputs(self, *paths: Path) -> list[object]:
        if self._root is None:
            raise ShadowInputError("input_unavailable")
        with _filesystem():
            identities = {
                (stamp.device, stamp.inode)
                for stamp in (*self._entries.values(), *self._manifests.values())
            }
            admitted = {}
            for path in paths:
                path = _absolute(path)
                info = _manifest_stamp(path)
                identity = info.device, info.inode
                if identity in identities:
                    raise ShadowInputError("invalid_manifest")
                identities.add(identity)
                admitted[path] = info
            self._manifests.update(admitted)
            return [read_manifest(path, info) for path, info in admitted.items()]

    def files(self) -> set[str]:
        """Return every regular relative filename, including unindexed captures."""
        if self._root is None:
            raise ShadowInputError("input_unavailable")
        return {path for path, entry in self._entries.items() if stat.S_ISREG(entry.mode)}

    def digest(self, relative: str) -> str:
        """Hash an admitted regular file with SHA-256, bounded to 16 MiB."""
        validate_relative(relative)
        if self._root is None:
            raise ShadowInputError("input_unavailable")
        if relative not in self._digests:
            digest = hashlib.sha256()
            for chunk in self._chunks(relative):
                digest.update(chunk)
            self._digests[relative] = digest.hexdigest()
        return self._digests[relative]

    def verified_bytes(self, relative: str, expected_digest: str) -> bytes:
        """Bind the bytes passed to the decision to the journal digest."""
        data = b"".join(self._chunks(relative))
        if hashlib.sha256(data).hexdigest() != expected_digest:
            raise ShadowInputError("snapshot_changed")
        return data

    def _chunks(self, relative: str) -> Iterator[bytes]:
        validate_relative(relative)
        if self._root is None or relative not in self._entries:
            raise ShadowInputError("input_unavailable")
        *parts, name = relative.split("/")
        with _filesystem(), _directory(tuple(parts), self._root) as parent:
            yield from _read_regular(parent, name, self._entries[relative], _MAX_MESSAGE)

    def __exit__(self, exc_type, exc_value, traceback) -> None:
        if self._root is None:
            raise ShadowInputError("input_unavailable")
        try:
            if exc_type is None:
                with _filesystem(), _directory(self._path.parts[1:]) as fd:
                    _unchanged(self._entries[""], _stamp(os.fstat(fd)))
                    if self._entries != _inventory(fd):
                        raise ShadowInputError("snapshot_changed")
                    for path, expected in self._manifests.items():
                        _unchanged(expected, _manifest_stamp(path))
                    # Reopen the configured name after traversing the pinned directory.
                    with _directory(self._path.parts[1:]) as current:
                        _unchanged(self._entries[""], _stamp(os.fstat(current)))
        finally:
            os.close(self._root)
            self._root = None
