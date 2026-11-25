"""The rehearsal only assesses the private occurrence the expectation identifies."""

import hashlib
import json
import shutil
import socket
from pathlib import Path

import pytest

from makotest import _native, shadow_io
from makotest.cli import main

FIXTURES = Path(__file__).parent / "fixtures" / "shadow"
pytestmark = pytest.mark.skipif(
    not shadow_io._SUPPORTED, reason="requires POSIX snapshots"
)


@pytest.fixture
def inputs(tmp_path, monkeypatch):
    def refuse_network(*args, **kwargs):
        pytest.fail("offline rehearsal attempted network access")

    monkeypatch.setattr(socket, "socket", refuse_network)
    return shutil.copytree(FIXTURES, tmp_path.resolve() / "private")


def invoke(root, *args, empty=False):
    manifests = root / "empty" if empty else root
    mirror = root / "empty-mirror" if empty else root / "mirror"
    status = "confirmed_empty" if empty else "complete"
    return main(
        [
            "shadow",
            "rehearse",
            "--journal",
            str(manifests / "journal.json"),
            "--inventory",
            str(manifests / "inventory.json"),
            "--mirror-root",
            str(mirror),
            "--case",
            str(root / "case.json"),
            "--expected",
            str(root / "expected.json"),
            "--coverage",
            str(root / "reports" / f"{status}.json"),
            *args,
        ]
    )


def change(path, **fields):
    doc = json.loads(path.read_bytes())
    path.write_text(json.dumps(doc | fields))


def test_rehearsal_compares_rendered_answer_and_keeps_denominators(inputs, capsysbinary):
    assert invoke(inputs) == 0
    report = json.loads(capsysbinary.readouterr().out)
    assert report["deadline"]["due_at"] == "2026-11-02T11:00:00+01:00"
    assert (
        report["eligible"],
        report["matched"],
        report["assessed"],
        report["unassessed"],
    ) == (2, 1, 1, 1)
    assert report["timing"] == "unassessed"
    assert report["actual"] == {"tree": "E_0622", "code": "A02", "pid": 55003}
    assert report["pruefschritt"] == 30
    assert invoke(inputs, "--emit", "answer") == 0
    raw = capsysbinary.readouterr().out
    assert hashlib.sha256(raw).hexdigest() == report["answer_sha256"]
    reply = _native.validate_edifact(raw, "2026-11-01")
    assert reply.is_valid and reply.rules_applied
    assert reply.messages[0].vorgaenge[0].vorgangsnummer != "VORGANG-1"
    assert reply.messages[0].vorgaenge[0].reference("TN") == "VORGANG-1"
    change(inputs / "expected.json", code="A07")
    assert invoke(inputs, "--emit", "answer") == 1
    assert capsysbinary.readouterr().out == raw
    assert invoke(inputs) == 1
    report = json.loads(capsysbinary.readouterr().out)
    assert report["differences"] == ["code"] and report["different"] == 1
    assert "VORGANG-1" not in str(report) and "occ-a" not in str(report)


@pytest.mark.skipif(shutil.which("makod") is None, reason="requires a local makod binary")
def test_daemon_preserves_captured_reply_across_expectations(inputs, capsysbinary):
    workspace = ("--makod-workspace", str(inputs / "workspace"))
    assert invoke(inputs, *workspace) == 0
    report = json.loads(capsysbinary.readouterr().out)
    assert report["capture"] == {"edifact": 2, "erp": 2}
    assert (report["eligible"], report["assessed"], report["unassessed"]) == (2, 1, 1)
    assert report["timing"] == "unassessed"

    assert invoke(inputs, *workspace, "--emit", "answer") == 0
    wire = capsysbinary.readouterr().out
    assert hashlib.sha256(wire).hexdigest() == report["answer_sha256"]

    change(inputs / "expected.json", code="A07")
    assert invoke(inputs, *workspace) == 1
    changed = json.loads(capsysbinary.readouterr().out)
    assert changed["differences"] == ["code"]
    assert changed["answer_sha256"] == report["answer_sha256"]
    assert changed["expected_fingerprint"] != report["expected_fingerprint"]


@pytest.mark.parametrize(
    ("file", "fields", "reason"),
    [
        ("case", {"role": "LF"}, "unsupported_case"),
        ("case", {"grid_member": False}, "unsupported_case"),
        ("case", {"received_at": "2026-11-01T01:00:00Z"}, "receipt_outside_batch"),
        ("case", {"occurrence": "occ-b"}, "unpaired_expectation"),
        ("expected", {"vorgang": "PRIVATE"}, "unpaired_expectation"),
        ("expected", {"revision": "changed"}, "unpaired_expectation"),
        ("expected", {"code": "invalid"}, "invalid_expectation"),
        ("expected", {"pid": 55002}, "invalid_expectation"),
        ("reports/complete", {"status": "unknown"}, "coverage_changed"),
    ],
)
def test_insufficient_evidence_refuses_private_output(
    inputs, capsysbinary, file, fields, reason
):
    change(inputs / f"{file}.json", **fields)
    assert invoke(inputs, "--emit", "answer") == 2
    streams = capsysbinary.readouterr()
    assert not streams.out and b"PRIVATE" not in streams.err
    report = json.loads(streams.err)
    assert report["reason"] == reason and report["assessed"] == 0
    assert report["eligible"] == report["unassessed"]


def test_confirmed_empty_has_no_candidates(inputs, capsys):
    (inputs / "empty-mirror").mkdir()
    assert invoke(inputs, empty=True) == 0
    report = json.loads(capsys.readouterr().out)
    assert report["status"] == "no_candidates"
    assert report["eligible"] == report["assessed"] == report["unassessed"] == 0


def test_rehearsal_rechecks_exact_bytes(inputs, monkeypatch, capsysbinary):
    original = shadow_io.Snapshot.verified_bytes

    def replaced(self, relative, expected):
        path = inputs / "mirror" / relative
        path.write_bytes(path.read_bytes() + b" ")
        return original(self, relative, expected)

    monkeypatch.setattr(shadow_io.Snapshot, "verified_bytes", replaced)
    assert invoke(inputs, "--emit", "answer") == 2
    streams = capsysbinary.readouterr()
    assert not streams.out
    result = json.loads(streams.err)
    assert result["reason"] == "snapshot_changed" and result["eligible"] is None


@pytest.mark.parametrize("hazard", ["outside", "oversize"])
def test_rehearsal_confines_and_bounds_every_capture(
    inputs, capsysbinary, monkeypatch, hazard
):
    """083-shadow-run/SR-026 and 083-shadow-run/SR-027 apply before comparison."""
    if hazard == "outside":
        path = inputs / "mirror" / "inbound" / "first.capture"
        path.unlink()
        path.symlink_to(inputs / "case.json")
    else:
        monkeypatch.setattr(shadow_io, "_MAX_MESSAGE", 8)
    assert invoke(inputs, "--emit", "answer") == 2
    streams = capsysbinary.readouterr()
    assert streams.out == b""
    report = json.loads(streams.err)
    assert report["reason"] in {"unsafe_path", "input_limit_exceeded"}
    assert report["assessed"] == 0


def test_late_anmeldung_reaches_the_vorlauf_check_before_stilllegung():
    assert _native.decommissioned_nb_refusal(
        "51238696012", "2026-11-15", "2026-11-14T23:10:00Z"
    ) == ("E_0622", "A07", 15)


def test_summary_exposes_scope_and_pair_for_review(inputs, capsys):
    change(inputs / "expected.json", code="A07")
    assert invoke(inputs, "--emit", "summary") == 1
    text = capsys.readouterr().out
    assert "2 eligible, 1 assessed, 1 unassessed" in text
    assert "expected: PID 55003  E_0622  A07" in text
    assert "actual: PID 55003  E_0622  A02" in text
    assert "dispatch timing unassessed" in text


@pytest.mark.parametrize(
    "words", [["shadow", "rehearse"], ["shadow", "--json", "rehearse"]]
)
def test_usage_error_cannot_enter_the_answer_stream(words, capsys):
    assert main([*words, "--emit", "answer", "--private-path"]) == 2
    streams = capsys.readouterr()
    assert streams.out == ""
    assert json.loads(streams.err)["reason"] == "invalid_arguments"
    assert "private-path" not in streams.err


def test_private_packet_pins_the_selected_input_and_reference(inputs, capsysbinary):
    assert invoke(inputs, "--emit", "daemon-input") == 0
    packet = json.loads(capsysbinary.readouterr().out)
    expected = json.loads((inputs / "expected.json").read_bytes())
    coverage = json.loads((inputs / "reports/complete.json").read_bytes())
    assert packet["request_sha256"] == expected["request_sha256"]
    assert packet["direction"] == "inbound" and packet["role"] == "NB"
    assert (packet["batch"], packet["revision"], packet["occurrence"]) == (
        "batch-a",
        "r1",
        "occ-a",
    )
    assert packet["coverage"] == coverage["fingerprints"]
    assert packet["scope"] == coverage["scope_fingerprint"]
    assert packet["decision"] == {"pid": 55003, "tree": "E_0622", "code": "A02"}
    change(inputs / "expected.json", occurrence="occ-b")
    assert invoke(inputs, "--emit", "daemon-input") == 2
    assert capsysbinary.readouterr().out == b""


def test_daemon_handoff_waits_for_final_snapshot_check(inputs, monkeypatch, capsysbinary):
    close = shadow_io.Snapshot.__exit__

    def refuse_changed_snapshot(self, *args):
        close(self, *args)
        raise shadow_io.ShadowInputError("snapshot_changed")

    monkeypatch.setattr(shadow_io.Snapshot, "__exit__", refuse_changed_snapshot)
    workspace = inputs.parent / "shadow-store"
    assert invoke(inputs, "--makod-workspace", str(workspace), "--emit", "answer") == 2
    assert not workspace.exists()
    assert capsysbinary.readouterr().out == b""


def test_daemon_store_cannot_modify_the_mirror(inputs, capsysbinary):
    workspace = inputs / "mirror" / "shadow-state"
    assert invoke(inputs, "--makod-workspace", str(workspace), "--emit", "answer") == 2
    streams = capsysbinary.readouterr()
    assert not streams.out and b"workspace_inside_mirror" in streams.err
    assert not workspace.exists()


def test_review_lists_every_occurrence_without_private_identifiers(inputs, capsysbinary):
    args = ("--emit", "review", "--review-nb", "9900357000003")
    assert invoke(inputs, *args) == 0
    first = json.loads(capsysbinary.readouterr().out)
    assert first["complete_werktage"] == 0
    assert len(first["rows"]) == 2
    assert first["rows"][1]["assessment"] is None
    assert first["rows"][0]["assessment"]["producer"] == "reference"
    assert "occ-a" not in str(first) and "51238696012" not in str(first)
    change(inputs / "expected.json", code="A07")
    assert invoke(inputs, *args) == 1
    changed = json.loads(capsysbinary.readouterr().out)
    assert first["rows"][0]["id"] == changed["rows"][0]["id"]
    assert first["rows"][0]["assessment"] != changed["rows"][0]["assessment"]
    assert invoke(inputs, "--emit", "review", "--review-nb", "9900357000004") == 2
    streams = capsysbinary.readouterr()
    assert streams.out == b"" and json.loads(streams.err)["reason"] == "scope_mismatch"


def test_review_confirms_zero_and_counts_full_berlin_workdays(inputs, capsysbinary):
    from makotest.rehearsal import _complete_werktage

    (inputs / "empty-mirror").mkdir()
    assert (
        invoke(inputs, "--emit", "review", "--review-nb", "9900357000003", empty=True)
        == 0
    )
    assert json.loads(capsysbinary.readouterr().out)["rows"] == []
    assert _complete_werktage("2026-11-01T23:00:00", "2026-11-02T23:00:00") == 1
    assert _complete_werktage("2026-11-01T23:00:00.000000001", "2026-11-02T23:00:00") == 0


@pytest.mark.parametrize(
    ("delivered", "result", "timing"),
    [
        ("2026-11-02T10:00:00Z", 0, "on_time"),
        ("2026-11-02T10:00:00.000000001Z", 1, "late"),
    ],
)
def test_incumbent_timing_requires_a_paired_receiver_receipt(
    inputs, capsysbinary, delivered, result, timing
):
    assert invoke(inputs, "--emit", "answer") == 0
    answer = capsysbinary.readouterr().out
    wire = inputs / "mirror" / "reply.capture"
    wire.write_bytes(answer)
    sha = hashlib.sha256(answer).hexdigest()

    def write_outbound(filename, entries):
        path = inputs / filename
        data = json.loads(path.read_bytes())
        batch = next(b for b in data["batches"] if b["direction"] == "outbound")
        batch.update(count=len(entries), entries=entries)
        path.write_text(json.dumps(data))

    def coverage():
        assert (
            main(
                [
                    "shadow",
                    "reconcile",
                    "--journal",
                    str(inputs / "journal.json"),
                    "--inventory",
                    str(inputs / "inventory.json"),
                    "--mirror-root",
                    str(inputs / "mirror"),
                ]
            )
            == 0
        )
        (inputs / "reports/complete.json").write_bytes(capsysbinary.readouterr().out)

    write_outbound("journal.json", [{"id": "reply", "sha256": sha}])
    write_outbound("inventory.json", [{"id": "reply", "path": "reply.capture"}])
    coverage()
    request = json.loads((inputs / "expected.json").read_bytes())["request_sha256"]
    obs = {
        "version": 1,
        "scope": "cohort-a",
        "inbound": {
            "positive": True,
            "sha256": request,
            "receipt_sha256": "a" * 64,
            "received_at": "2026-11-01T00:10:00Z",
        },
        "outbound": {
            "positive": True,
            "sha256": sha,
            "receipt_sha256": "b" * 64,
            "received_at": delivered,
        },
    }
    path = inputs / "receipt.json"
    path.write_text(json.dumps(obs))
    assert invoke(inputs, "--observation", str(path)) == result
    report = json.loads(capsysbinary.readouterr().out)
    assert report["timing"] == timing
    assert ("timing" in report["differences"]) == (timing == "late")
    obs["outbound"]["positive"] = False
    path.write_text(json.dumps(obs))
    assert (
        invoke(
            inputs,
            "--observation",
            str(path),
            "--emit",
            "review",
            "--review-nb",
            "9900357000003",
        )
        == 2
    )
    assert not capsysbinary.readouterr().out
    obs["outbound"]["positive"] = True
    path.write_text(json.dumps(obs))
    (inputs / "mirror/another.capture").write_bytes(answer)
    write_outbound(
        "journal.json", [{"id": s, "sha256": sha} for s in ("reply", "another")]
    )
    write_outbound(
        "inventory.json",
        [
            {"id": s, "path": p}
            for s, p in [("reply", "reply.capture"), ("another", "another.capture")]
        ],
    )
    coverage()
    assert invoke(inputs, "--observation", str(path)) == 2
    assert json.loads(capsysbinary.readouterr().out)["reason"] == "ambiguous_observation"


def test_review_combines_two_independent_cases_or_withholds_the_whole_export(
    inputs, capsysbinary
):
    case = json.loads((inputs / "case.json").read_bytes())
    expected = json.loads((inputs / "expected.json").read_bytes())
    second_case, second_expected = (
        inputs / "second-case.json",
        inputs / "second-expected.json",
    )
    second_case.write_text(
        json.dumps(case | {"malo": "51238696020", "occurrence": "occ-b"})
    )
    second_expected.write_text(
        json.dumps(
            expected
            | {
                "vorgang": "VORGANG-2",
                "occurrence": "occ-b",
                "request_sha256": hashlib.sha256(
                    (inputs / "mirror/inbound/second.capture").read_bytes()
                ).hexdigest(),
            }
        )
    )
    args = (
        "--emit",
        "review",
        "--review-nb",
        "9900357000003",
        "--also",
        str(second_case),
        str(second_expected),
    )
    assert invoke(inputs, *args) == 0
    assert all(r["assessment"] for r in json.loads(capsysbinary.readouterr().out)["rows"])
    assert invoke(inputs, *args, "--also", str(second_case), str(second_expected)) == 2
    streams = capsysbinary.readouterr()
    assert streams.out == b"" and json.loads(streams.err)["reason"] == "duplicate_review"


def test_closed_captures_distinguish_one_sided_answers_from_an_unknown_window(
    inputs, tmp_path, capsysbinary
):
    assert invoke(inputs, "--emit", "answer") == 0
    answer = capsysbinary.readouterr().out
    other = shutil.copytree(inputs, tmp_path / "shadow")

    def observed(root):
        (root / "mirror/reply.capture").write_bytes(answer)
        for file, column, value in [
            ("inventory.json", "path", "reply.capture"),
            ("journal.json", "sha256", hashlib.sha256(answer).hexdigest()),
        ]:
            path = root / file
            manifest = json.loads(path.read_text())
            batch = next(b for b in manifest["batches"] if b["direction"] == "outbound")
            batch.update(count=1, entries=[{"id": "reply", column: value}])
            path.write_text(json.dumps(manifest))

    def compare(a, b):
        code = main(
            [
                "shadow",
                "pair",
                "--incumbent",
                str(a),
                "--shadow",
                str(b),
                "--case",
                str(inputs / "case.json"),
            ]
        )
        return code, json.loads(capsysbinary.readouterr().out)["classification"]

    assert compare(inputs, other) == (2, "unassessed")
    observed(other)
    assert compare(inputs, other) == (1, "nur_mako")
    assert compare(other, inputs) == (1, "nur_alt")
    observed(inputs)
    assert compare(inputs, other) == (0, "gleich")
    second = other / "mirror/inbound/second.capture"
    original = second.read_bytes().replace(b"VORGANG-2", b"VORGANG-1")
    for payload, outcome in [
        (original.replace(b"4012345000023", b"4012345000016"), (0, "gleich")),
        (original, (2, "unassessed")),
    ]:
        second.write_bytes(payload)
        journal = other / "journal.json"
        manifest = json.loads(journal.read_bytes())
        entry = next(
            e
            for batch in manifest["batches"]
            for e in batch["entries"]
            if e["id"] == "occ-b"
        )
        entry["sha256"] = hashlib.sha256(payload).hexdigest()
        journal.write_text(json.dumps(manifest))
        assert compare(inputs, other) == outcome
    (other / "mirror/reply.capture").unlink()
    assert compare(inputs, other) == (2, "unassessed")
