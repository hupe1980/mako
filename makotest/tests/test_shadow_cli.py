"""The installed shadow command reports coverage, uncertainty and disagreement."""

import hashlib
import json
import os
import shutil
import subprocess
import sysconfig
from pathlib import Path

import pytest

from makotest import shadow, shadow_io
from makotest.cli import main

FIXTURES = Path(__file__).resolve().parent / "fixtures/shadow"


@pytest.fixture
def capture(tmp_path):
    if not shadow_io._SUPPORTED:
        pytest.skip("requires descriptor-relative POSIX filesystem operations")
    return Path(shutil.copytree(FIXTURES, tmp_path.resolve() / "capture"))


def arguments(root, *, empty=False):
    source = root / "empty" if empty else root
    return [
        "shadow",
        "reconcile",
        "--journal",
        str(source / "journal.json"),
        "--inventory",
        str(source / "inventory.json"),
        "--mirror-root",
        str(root / ("empty-mirror" if empty else "mirror")),
    ]


def installed(*args, env=None):
    command = shutil.which("makotest", path=sysconfig.get_path("scripts"))
    assert command, (
        "install the makotest console entry point before running its CLI tests"
    )
    return subprocess.run(
        [command, *args],
        capture_output=True,
        text=True,
        check=False,
        env=env,
        timeout=20,
    )


def report(result, status, code):
    assert result.returncode == code, result.stderr
    assert result.stderr == ""
    assert len(result.stdout.splitlines()) == 1
    result_json = json.loads(result.stdout)
    assert result_json["status"] == status
    return result_json


def test_unsupported_acquisition_is_an_unknown_result(monkeypatch, capsys):
    monkeypatch.setattr(shadow_io, "_SUPPORTED", False)
    assert main(arguments(FIXTURES)) == 2
    output = capsys.readouterr()
    assert output.err == ""
    assert json.loads(output.out)["reason_codes"] == ["unsupported_platform"]


def test_help_names_the_inputs_and_every_exit_status():
    result = installed("shadow", "reconcile", "--help")
    assert result.returncode == 0
    for text in (
        "--journal",
        "--inventory",
        "--mirror-root",
        "Example",
        "0 complete or confirmed_empty",
        "1 mismatch",
        "2 unknown",
        "Reacquire the frozen mirror and its inventory together",
        "time window and required directions on both sides",
        "adds zero inbound comparison",
        "eligible_for_comparison",
    ):
        assert text in result.stdout


def test_offline_rehearsal_detects_a_substituted_occurrence(capture, tmp_path):
    # A site hook executes inside the installed process before package imports.
    # Record an attempted socket even if the caller catches its exception.
    hooks = tmp_path / "hooks"
    hooks.mkdir()
    (hooks / "sitecustomize.py").write_text(
        "import os, sys\nfrom pathlib import Path\n"
        "marker = Path(os.environ['SHADOW_AUDIT'])\n"
        "marker.write_text('active')\n"
        "def audit(event, args):\n"
        "    if event.startswith('socket.'):\n"
        "        marker.write_text(event)\n"
        "        raise RuntimeError('network access')\n"
        "sys.addaudithook(audit)\n"
    )
    audit = tmp_path / "audit"
    env = dict(os.environ, PYTHONPATH=str(hooks), SHADOW_AUDIT=str(audit))
    complete = installed(*arguments(capture), env=env)
    data = report(complete, "complete", 0)
    assert data["eligible_inbound"] == 2
    assert data["counts"]["confirmed_empty_batches"] == 1
    for name in ("journal", "inventory"):
        path = capture / f"{name}.json"
        value = json.loads(path.read_bytes())
        for batch in value["batches"]:
            batch["entries"].reverse()
        value["batches"].reverse()
        path.write_text(json.dumps(value))
    assert installed(*arguments(capture), env=env).stdout == complete.stdout
    path = capture / "inventory.json"
    value = json.loads(path.read_bytes())
    batch = next(b for b in value["batches"] if b["direction"] == "inbound")
    second, first = batch["entries"]
    second["id"] = first["id"]
    mirror = capture / "mirror"
    shutil.copyfile(mirror / first["path"], mirror / second["path"])
    path.write_text(json.dumps(value))
    mismatch = installed(*arguments(capture), env=env)
    data = report(mismatch, "mismatch", 1)
    assert data["counts"]["expected"] == data["counts"]["observed"] == 2
    assert data["counts"]["missing"] == data["counts"]["duplicate"] == 1
    assert data["eligible_for_comparison"] is False
    # Both the wire identifiers and local storage names stay inside the operator's estate.
    for private in ("SHADOW-1", "4012345000023", "51238696012", "occ-a", "first.capture"):
        assert private not in complete.stdout + mismatch.stdout
    assert audit.read_text() == "active"


def test_confirmed_empty_accounts_for_time(capture):
    (capture / "empty-mirror").mkdir()
    data = report(installed(*arguments(capture, empty=True)), "confirmed_empty", 0)
    assert data["eligible_inbound"] == 0
    assert data["eligible_for_comparison"] is False


@pytest.mark.parametrize("payload", [b'{"v":1,"v":true}', b"[" * 2000 + b"]" * 2000])
def test_malformed_journal_keeps_the_path_private(capture, payload):
    args = arguments(capture)
    private = capture / "PRIVATE-SOURCE"
    private.write_bytes(payload)
    args[3] = str(private)
    result = installed(*args)
    data = report(result, "unknown", 2)
    assert data["reason_codes"]
    assert "PRIVATE-SOURCE" not in result.stdout


def test_usage_error_keeps_unexpected_arguments_private(capture):
    result = installed("--j", *arguments(capture), "PRIVATE-ARGUMENT")
    assert report(result, "unknown", 2)["reason_codes"] == ["invalid_arguments"]
    assert "PRIVATE-ARGUMENT" not in result.stdout


def test_escaping_paths_stop_before_reading_private_content(capture):
    path = capture / "inventory.json"
    value = json.loads(path.read_bytes())
    value["batches"][0]["entries"][0]["path"] = "../PRIVATE-FILE"
    path.write_text(json.dumps(value))
    (capture / "PRIVATE-FILE").write_text("PRIVATE-CONTENT")
    result = installed(*arguments(capture))
    assert report(result, "unknown", 2)["eligible_for_comparison"] is False
    assert "PRIVATE-" not in result.stdout


def test_a_hardlinked_empty_inventory_cannot_attest_its_own_journal(capture):
    (capture / "empty-mirror").mkdir()
    manifest = capture / "empty/inventory.json"
    manifest.unlink()
    os.link(capture / "empty/journal.json", manifest)
    result = installed(*arguments(capture, empty=True))
    assert report(result, "unknown", 2)["eligible_inbound"] == 0


def test_a_self_indexed_manifest_cannot_count_as_mirrored_traffic(capture):
    args = arguments(capture)
    value = json.loads((capture / "inventory.json").read_bytes())
    mirror = capture / "mirror"
    shutil.rmtree(mirror / "inbound")
    inventory = mirror / "PRIVATE-MANIFEST"
    value["batches"][0].update(count=1, entries=[{"id": "x", "path": inventory.name}])
    inventory.write_text(json.dumps(value))
    args[5] = str(inventory)
    value["batches"][0]["entries"] = [
        {"id": "x", "sha256": hashlib.sha256(inventory.read_bytes()).hexdigest()}
    ]
    (capture / "journal.json").write_text(json.dumps(value))
    result = installed(*args)
    assert report(result, "unknown", 2)["eligible_inbound"] == 0
    assert "PRIVATE-MANIFEST" not in result.stdout


@pytest.mark.parametrize("name", ["journal", "inventory"])
def test_manifest_replaced_during_hashing_invalidates_the_result(
    capture, name, monkeypatch, capsys
):
    original = shadow.reconcile

    def replace_after_read(journal, inventory, snapshot):
        result = original(journal, inventory, snapshot)
        assert result["status"] == "complete"
        path = capture / f"{name}.json"
        value = json.loads(path.read_bytes())
        value["version"] = True  # compares equal to Python's integer 1
        replacement = path.with_suffix(".new")
        replacement.write_text(json.dumps(value))
        replacement.replace(path)
        return result

    monkeypatch.setattr(shadow, "reconcile", replace_after_read)
    assert main(arguments(capture)) == 2
    assert json.loads(capsys.readouterr().out)["reason_codes"] == ["snapshot_changed"]


def test_snapshot_close_failure_emits_one_redacted_unknown(capture, monkeypatch, capsys):
    original = shadow_io.Snapshot.__exit__

    def fail_on_close(snapshot, *args):
        original(snapshot, *args)
        raise OSError("PRIVATE-CLOSE-FAILURE")

    monkeypatch.setattr(shadow_io.Snapshot, "__exit__", fail_on_close)
    assert main(arguments(capture)) == 2
    output = capsys.readouterr()
    assert output.err == ""
    assert len(output.out.splitlines()) == 1
    assert "PRIVATE-" not in output.out
    assert json.loads(output.out)["reason_codes"] == ["input_unavailable"]
