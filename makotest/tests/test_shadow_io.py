import hashlib
import json
import os

import pytest

from makotest import shadow_io
from makotest.shadow_io import (
    ShadowInputError,
    Snapshot,
    read_manifest,
    validate_relative,
)


@pytest.fixture
def root(tmp_path):
    if not shadow_io._SUPPORTED:
        pytest.skip("requires descriptor-relative POSIX filesystem operations")
    # macOS's system temp directory can have symlink ancestors.
    return tmp_path.resolve()


def refuses(code):
    return pytest.raises(ShadowInputError, match=f"^{code}$")


@pytest.mark.parametrize(
    "bad",
    [
        b'{"x":{"a":1,"a":2}}',
        b"[NaN]",
        b"Infinity",
        b"-Infinity",
        b"1e999",
        b'{"bad":"\xff"}',
        b"\xff\xfe{\x00}\x00",
        b"\xef\xbb\xbf{}",
        b"{}{}",
    ],
)
def test_manifest_accepts_only_strict_utf8_json(root, bad):
    path = root / "private-token"
    path.write_bytes(bad)
    with refuses("invalid_json") as error:
        read_manifest(path)
    assert error.value.code == str(error.value)


def test_manifest_size_counts_utf8_bytes_before_decoding(root):
    path = root / "journal"
    manifest = '{"currency":"€","fraction":1.25}'.encode()
    path.write_bytes(manifest + b" " * (8 * 1024**2 - len(manifest)))
    assert read_manifest(path) == {"currency": "€", "fraction": 1.25}
    with path.open("ab") as handle:
        handle.write(b" ")
    with refuses("input_limit_exceeded"):
        read_manifest(path)


@pytest.mark.parametrize(
    "bad",
    [
        "",
        ".",
        "..",
        "/outside",
        "../file",
        "a/../b",
        "a/./b",
        "a//b",
        "a/",
        "c:foo",
        "c:/foo",
        "a\\b",
        "a\0b",
        "\udcff",
    ],
)
def test_index_requires_exact_relative_names(bad):
    with refuses("unsafe_path"):
        validate_relative(bad)


def test_parent_traversal_in_configured_paths_is_refused(root):
    with refuses("unsafe_path"):
        read_manifest(root / ".." / "journal")
    with refuses("unsafe_path"), Snapshot(root / ".."):
        pytest.fail("parent path admitted")


def test_readonly_snapshot_lists_unindexed_files_and_hashes_exact_bytes(root):
    (root / "nested").mkdir()
    payload = b"\x00\r\n\xff"
    (root / "nested" / "captured").write_bytes(payload)
    (root / ".late").write_bytes(b"")
    with Snapshot(root) as snapshot:
        assert snapshot.files() == {".late", "nested/captured"}
        assert snapshot.digest("nested/captured") == hashlib.sha256(payload).hexdigest()
        assert snapshot.digest(".late") == hashlib.sha256(b"").hexdigest()
        with refuses("unsafe_path"):
            snapshot.digest("nested")
        with refuses("input_unavailable"):
            snapshot.digest("not-indexed")


def test_symlinks_refused_in_root_ancestry_and_every_member(root):
    mirror = root / "real" / "mirror"
    mirror.mkdir(parents=True)
    file = mirror / "member"
    file.write_text("{}")
    alias = root / "alias"
    alias.symlink_to(mirror.parent, target_is_directory=True)
    for link in [alias, alias / "mirror"]:
        with refuses("unsafe_path"), Snapshot(link):
            pytest.fail("symlinked root admitted")
    with refuses("unsafe_path"):
        read_manifest(alias / "mirror" / "member")
    (mirror / "other").symlink_to(file)
    with refuses("unsafe_path"):
        read_manifest(mirror / "other")
    with refuses("unsafe_path"), Snapshot(mirror):
        pytest.fail("symlinked member admitted")
    (mirror / "other").unlink()
    (mirror / "other").symlink_to(root, target_is_directory=True)
    with refuses("unsafe_path"), Snapshot(mirror):
        pytest.fail("symlinked subtree admitted")


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="requires POSIX FIFO")
def test_special_files_refused_without_reading_them(root):
    fifo = root / "capture"
    os.mkfifo(fifo)
    with refuses("unsafe_path"):
        read_manifest(fifo)
    with refuses("unsafe_path"), Snapshot(root):
        pytest.fail("FIFO admitted")


def test_snapshot_admits_exact_message_limit_and_refuses_one_more_byte(root):
    file = root / "capture"
    size = 16 * 1024**2
    with file.open("wb") as handle:
        handle.truncate(size)
    with Snapshot(root) as snapshot:
        assert snapshot.digest("capture") == hashlib.sha256(bytes(size)).hexdigest()
    with file.open("ab") as handle:
        handle.write(b"x")
    with refuses("input_limit_exceeded"), Snapshot(root):
        pytest.fail("oversize file admitted")


@pytest.mark.parametrize("limit", ["_MAX_FILES", "_MAX_DIRS", "_MAX_DEPTH", "_MAX_BYTES"])
def test_tree_limits_include_unindexed_files_and_empty_directories(
    root, monkeypatch, limit
):
    subdir = root / "subdir"
    subdir.mkdir()
    (subdir / "empty-dir").mkdir()
    (subdir / "one").write_bytes(b"1")
    (root / "two").write_bytes(b"2")
    monkeypatch.setattr(shadow_io, limit, 1)
    with refuses("input_limit_exceeded"), Snapshot(root):
        pytest.fail("unbounded capture admitted")


@pytest.mark.parametrize("change", ["add", "replace", "rewrite", "subtree", "root"])
def test_exit_rechecks_configured_root_and_every_member(root, change):
    directory = root / "capture"
    file = directory / "nested" / "file"
    file.parent.mkdir(parents=True)
    file.write_bytes(b"abc")
    with refuses("snapshot_changed"), Snapshot(directory) as capture:
        assert capture.digest("nested/file") == hashlib.sha256(b"abc").hexdigest()
        if change == "add":
            (file.parent / "late").touch()
        elif change == "replace":
            replacement = root / "new"
            replacement.write_bytes(b"abc")
            replacement.replace(file)
        elif change == "rewrite":
            file.write_bytes(b"xyz")
        elif change == "subtree":
            file.parent.rename(root / "old-nested")
            file.parent.mkdir()
            file.write_bytes(b"abc")
        else:
            directory.rename(root / "old")
            file.parent.mkdir(parents=True)
            file.write_bytes(b"abc")


def test_member_changing_during_read_refuses_digest(root, monkeypatch):
    file = root / "capture"
    file.write_bytes(b"abc")
    original_read = os.read

    def read_then_change(fd, length):
        chunk = original_read(fd, length)
        if chunk:
            file.write_bytes(b"xyz")
        return chunk

    with (
        refuses("snapshot_changed"),
        Snapshot(root) as capture,
        monkeypatch.context() as patch,
    ):
        patch.setattr(os, "read", read_then_change)
        with refuses("snapshot_changed"):
            capture.digest("capture")
        # A failed digest cannot subsequently yield a positive snapshot decision.
        with refuses("snapshot_changed"):
            capture.digest("capture")


def test_manifest_revalidates_configured_name_after_parsing(root, monkeypatch):
    journal = root / "journal"
    journal.write_bytes(b'{"a":1}')
    replacement = root / "replacement"
    replacement.write_bytes(b'{"a":1}')
    loads = shadow_io.json.loads

    def replace_after_read(text, **kwargs):
        replacement.replace(journal)
        return loads(text, **kwargs)

    monkeypatch.setattr(shadow_io.json, "loads", replace_after_read)
    with refuses("snapshot_changed"):
        read_manifest(journal)


def test_unsupported_os_fails_closed_before_open(tmp_path, monkeypatch):
    monkeypatch.setattr(shadow_io, "_SUPPORTED", False)
    with refuses("unsupported_platform"):
        read_manifest(tmp_path / "missing")
    with refuses("unsupported_platform"), Snapshot(tmp_path):
        pytest.fail("unsupported backend admitted")


def test_json_depth_is_bounded_before_allocating_nested_containers(root):
    manifest = root / "manifest"
    manifest.write_bytes(b"[" * 65 + b"]" * 65)
    with refuses("input_limit_exceeded"):
        read_manifest(manifest)
    # Brackets inside JSON strings do not create new containers.
    text = '["' * 100
    manifest.write_text(json.dumps([text]))
    assert read_manifest(manifest) == [text]


def test_external_hard_link_cannot_alias_a_manifest_or_mirror_capture(root):
    mirror = root / "mirror"
    mirror.mkdir()
    external = root / "outside"
    external.write_bytes(b"{}")
    capture = mirror / "capture"
    os.link(external, capture)
    with refuses("unsafe_path"):
        read_manifest(capture)
    with refuses("unsafe_path"), Snapshot(mirror):
        pytest.fail("multiply linked capture admitted")


def test_journal_and_inventory_are_distinct_files_outside_the_mirror(root):
    mirror = root / "mirror"
    mirror.mkdir()
    journal = root / "journal"
    journal.touch()
    internal = mirror / "inventory"
    internal.touch()
    with Snapshot(mirror) as snapshot:
        with refuses("invalid_manifest"):
            snapshot.read_manifests(journal, journal)
        with refuses("invalid_manifest"):
            snapshot.read_manifests(journal, internal)
        with refuses("invalid_manifest"):
            snapshot.read_manifests(internal, journal)


@pytest.mark.parametrize("replacement", [b"{}", b'{"new": true}'])
def test_both_manifests_remain_pinned_until_the_final_decision(root, replacement):
    mirror = root / "mirror"
    mirror.mkdir()
    journal, inventory = root / "journal", root / "inventory"
    journal.write_bytes(b"{}")
    inventory.write_bytes(b"{}")
    with refuses("snapshot_changed"), Snapshot(mirror) as snapshot:
        snapshot.read_manifests(journal, inventory)
        other = root / "replacement"
        other.write_bytes(replacement)
        other.replace(inventory)


def test_file_growth_cannot_exceed_a_byte_limit_during_streaming(root, monkeypatch):
    file = root / "capture"
    file.write_bytes(b"ab")
    original_read = os.read

    def grow_after_read(fd, length):
        chunk = original_read(fd, length)
        if chunk == b"ab":
            with file.open("ab") as output:
                output.write(b"x")
        return chunk

    monkeypatch.setattr(shadow_io, "_MAX_MESSAGE", 2)
    with refuses("input_limit_exceeded"), Snapshot(root) as capture:
        monkeypatch.setattr(os, "read", grow_after_read)
        capture.digest("capture")


def test_manifest_read_uses_the_identity_pinned_for_the_pair(root, monkeypatch):
    mirror = root / "mirror"
    mirror.mkdir()
    journal, inventory = root / "journal", root / "inventory"
    journal.write_bytes(b"{}")
    inventory.write_bytes(b"{}")
    original_read = shadow_io.read_manifest

    def replace_before_read(path, expected):
        if path == inventory:
            replacement = root / "replacement"
            replacement.write_bytes(b"{}")
            replacement.replace(path)
        return original_read(path, expected)

    monkeypatch.setattr(shadow_io, "read_manifest", replace_before_read)
    with refuses("snapshot_changed"), Snapshot(mirror) as snapshot:
        snapshot.read_manifests(journal, inventory)
        pytest.fail("the substituted file was parsed before the final snapshot check")
