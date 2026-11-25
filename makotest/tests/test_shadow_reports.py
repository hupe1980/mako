"""Portable reports pin the v1 input coverage contract for downstream comparisons."""

import json
import os
from pathlib import Path

import pytest

from makotest.cli import main

FIXTURES = Path(__file__).parent / "fixtures" / "shadow"


@pytest.mark.skipif(os.name != "posix", reason="requires descriptor-relative snapshots")
@pytest.mark.parametrize("status", ["complete", "confirmed_empty"])
def test_portable_reference_report(status, capsys, tmp_path):
    inputs = FIXTURES if status == "complete" else FIXTURES / "empty"
    mirror = FIXTURES / "mirror" if status == "complete" else tmp_path.resolve()
    code = main(
        [
            "shadow",
            "reconcile",
            "--journal",
            str(inputs / "journal.json"),
            "--inventory",
            str(inputs / "inventory.json"),
            "--mirror-root",
            str(mirror),
        ]
    )
    streams = capsys.readouterr()
    assert code == 0, streams.out
    assert streams.err == ""
    report = json.loads(streams.out)
    assert report == json.loads((FIXTURES / "reports" / f"{status}.json").read_bytes())
    assert report["status"] == status
    assert report["eligible_for_comparison"] is (status == "complete")
    assert report["eligible_inbound"] == (2 if status == "complete" else 0)
