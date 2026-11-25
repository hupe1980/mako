"""Coverage is a decision over two sealed inventories and the actual mirror bytes."""

import copy
import hashlib
import json

import pytest

from makotest.shadow import issue_tokens, reconcile, unknown_report
from makotest.shadow_io import ShadowInputError

START = "2026-04-01T00:00:00Z"
MID = "2026-04-01T00:10:00Z"
END = "2026-04-01T00:20:00Z"


def digest(content):
    return hashlib.sha256(content).hexdigest()


def batch(entries, *, direction="inbound", name="set1", start=START, end=END):
    return {
        "id": name,
        "revision": "r1",
        "direction": direction,
        "start": start,
        "end": end,
        "closed": True,
        "count": len(entries),
        "entries": entries,
    }


def manifest(*batches, directions=("inbound",), start=START, end=END):
    return {
        "version": 1,
        "scope": "cohort",
        "capture_layer": "wire",
        "directions": list(directions),
        "start": start,
        "end": end,
        "batches": list(batches),
    }


def source(*entries):
    return batch([{"id": label, "sha256": digest(value)} for label, value in entries])


def inventory(*entries):
    return batch([{"id": label, "path": path} for label, path in entries])


def assess(root, journal, mirror):
    # These tests cover inventory semantics on every platform. The descriptor
    # and tree-stability checks have their own real-filesystem test suite.
    class Mirror:
        def files(self):
            return {path.name for path in root.iterdir()}

        def digest(self, name):
            return digest((root / name).read_bytes())

    return reconcile(journal, mirror, Mirror())


def report_bytes(report):
    return json.dumps(report, separators=(",", ":")).encode()


def test_duplicate_substitution_with_equal_totals_retains_both_issues(tmp_path):
    j = manifest(source(("customerMarker", b"x"), ("missing", b"y")))
    m = manifest(
        inventory(
            ("customerMarker", "payload-customerMarker"),
            ("customerMarker", "second-copy"),
        )
    )
    for name in ["payload-customerMarker", "second-copy"]:
        (tmp_path / name).write_bytes(b"x")
    result = assess(tmp_path, j, m)
    assert result["status"] == "mismatch"
    assert result["counts"]["expected"] == result["counts"]["observed"] == 2
    assert result["counts"]["expected_identities"] == 2
    assert result["counts"]["observed_identities"] == 1
    assert result["reason_codes"] == ["duplicate", "missing"]
    assert result["eligible_inbound"] == 0
    assert not result["eligible_for_comparison"]
    aliases = issue_tokens("cohort", "wire", "inbound", "set1", "r1", "customerMarker")
    assert {
        "reason": "duplicate",
        "batch": aliases[0],
        "occurrence": aliases[1],
        "count": 1,
    } in result["issues"]
    assert b"customerMarker" not in report_bytes(result)
    assert b"second-copy" not in report_bytes(result)
    assert digest(b"x").encode() not in report_bytes(result)
    m["batches"][0]["entries"].reverse()
    j["batches"][0]["entries"].reverse()
    assert report_bytes(assess(tmp_path, j, m)) == report_bytes(result)


def test_retransmissions_have_distinct_occurrences_even_with_identical_bytes(tmp_path):
    j = manifest(source(("a", b"same"), ("b", b"same")))
    m = manifest(inventory(("a", "one"), ("b", "two")))
    for name in ["one", "two"]:
        (tmp_path / name).write_bytes(b"same")
    clean = assess(tmp_path, j, m)
    assert clean["status"] == "complete"
    assert clean["eligible_for_comparison"] and clean["eligible_inbound"] == 2
    m["batches"][0]["entries"].reverse()
    j["batches"][0]["entries"].reverse()
    assert report_bytes(assess(tmp_path, j, m)) == report_bytes(clean)
    (tmp_path / "one").write_bytes(b"changed")
    changed = assess(tmp_path, j, m)
    assert changed["reason_codes"] == ["changed"]
    assert changed["counts"]["changed"] == 1
    assert clean["fingerprints"]["observed"] != changed["fingerprints"]["observed"]
    (tmp_path / "three").write_bytes(b"unexpected")
    (tmp_path / "two").unlink()
    m["batches"][0]["entries"] = [
        {"id": "unexpected", "path": "three"},
        {"id": "a", "path": "one"},
    ]
    extra = assess(tmp_path, j, m)
    assert extra["reason_codes"] == ["changed", "extra", "missing"]


def test_identical_batch_repeat_is_idempotent_and_revision_rebinds(tmp_path):
    j, m = manifest(source(("a", b"a"))), manifest(inventory(("a", "file")))
    (tmp_path / "file").write_bytes(b"a")
    before = assess(tmp_path, j, m)
    j["batches"] *= 2
    m["batches"] *= 2
    assert report_bytes(assess(tmp_path, j, m)) == report_bytes(before)
    j["batches"][0]["revision"] = "r2"
    stale = assess(tmp_path, j, m)
    assert stale["status"] == "unknown" and stale["reason_codes"] == ["revision_mismatch"]
    assert stale["counts"]["observed"] is None
    m["batches"][0]["revision"] = "r2"
    after = assess(tmp_path, j, m)
    assert after["status"] == "complete"
    assert all(after["fingerprints"][k] != v for k, v in before["fingerprints"].items())
    j["batches"].append(dict(j["batches"][0], revision="r3"))
    with pytest.raises(ShadowInputError, match="conflicting_batch"):
        assess(tmp_path, j, m)


def test_unlisted_late_files_and_indexed_missing_bytes_are_unknown(tmp_path):
    j, m = manifest(source()), manifest(inventory())
    empty = assess(tmp_path, j, m)
    assert empty["status"] == "confirmed_empty"
    assert empty["counts"]["confirmed_empty_batches"] == 1
    assert empty["counts"]["observed"] == empty["counts"]["expected"] == 0
    assert not empty["eligible_for_comparison"]
    (tmp_path / "late-file").write_bytes(b"late")
    late = assess(tmp_path, j, m)
    assert late["status"] == "unknown"
    assert late["reason_codes"] == ["unlisted_file"]
    assert late["fingerprints"]["observed"] is None
    (tmp_path / "late-file").unlink()
    result = assess(tmp_path, j, manifest(inventory(("a", "missing-file"))))
    assert result["reason_codes"] == ["missing_file"]
    assert result["counts"]["observed"] is None
    with pytest.raises(ShadowInputError, match="input_unavailable"):
        assess(tmp_path, None, m)
    invalid = unknown_report("secret-token")
    assert invalid["reason_codes"] == ["invalid_manifest"]
    assert b"secret-token" not in report_bytes(invalid)
    assert all(v is None for v in invalid["counts"].values())


def test_each_physical_path_attests_only_one_logical_occurrence(tmp_path):
    (tmp_path / "one").write_bytes(b"a")
    j = manifest(source(("x", b"a"), ("y", b"a")))
    m = manifest(inventory(("x", "one"), ("y", "one")))
    result = assess(tmp_path, j, m)
    assert result["status"] == "unknown"
    assert result["reason_codes"] == ["reused_path"]


def test_directions_remain_independent_and_only_the_source_defines_scope(tmp_path):
    j, m = manifest(source(("a", b"a"))), manifest(inventory(("a", "a")))
    (tmp_path / "a").write_bytes(b"a")
    for h in (j, m):
        h["directions"] = ["outbound"]
        h["batches"][0]["direction"] = "outbound"
    outbound = assess(tmp_path, j, m)
    assert outbound["status"] == "complete"
    assert not outbound["eligible_for_comparison"]
    j["directions"].append("inbound")
    assert assess(tmp_path, j, m)["reason_codes"] == ["scope_mismatch"]
    m["directions"].append("inbound")
    assert assess(tmp_path, j, m)["reason_codes"] == ["coverage_gap"]
    j["batches"].append(source(("a", b"b")))
    m["batches"].append(inventory(("a", "b")))
    (tmp_path / "b").write_bytes(b"b")
    both = assess(tmp_path, j, m)
    assert both["eligible_inbound"] == 1
    assert both["counts"]["observed"] == 2
    assert len({b["batch"] for b in both["batches"]}) == 2


def test_contiguous_coverage_keeps_the_empty_subinterval(tmp_path):
    first, second = batch([], end=MID), batch([], name="two", start=MID)
    j = manifest(first, second)
    m = copy.deepcopy(j)
    j["batches"][0] = dict(source(("a", b"x")), end=MID)
    m["batches"][0] = dict(inventory(("a", "x")), end=MID)
    (tmp_path / "x").write_bytes(b"x")
    result = assess(tmp_path, j, m)
    assert result["status"] == "complete" and result["eligible_inbound"] == 1
    assert [b["status"] for b in result["batches"]] == ["complete", "confirmed_empty"]
    for start, reason in [
        ("2026-04-01T00:10:00.000000001Z", "coverage_gap"),
        ("2026-04-01T00:09:59.999999999Z", "coverage_overlap"),
    ]:
        j["batches"][1]["start"] = m["batches"][1]["start"] = start
        result = assess(tmp_path, j, m)
        assert result["status"] == "unknown" and result["reason_codes"] == [reason]


@pytest.mark.parametrize(
    ("start", "end", "utc_start", "utc_end"),
    [
        (
            "2026-10-25T02:00:00+02:00",
            "2026-10-25T02:00:00+01:00",
            "2026-10-25T00:00:00Z",
            "2026-10-25T01:00:00Z",
        ),
        (
            "2026-03-29T01:30:00+01:00",
            "2026-03-29T03:30:00+02:00",
            "2026-03-29T00:30:00Z",
            "2026-03-29T01:30:00Z",
        ),
    ],
)
def test_dst_transitions_compare_actual_instants(
    tmp_path, start, end, utc_start, utc_end
):
    j = manifest(batch([], start=start, end=end), start=start, end=end)
    m = manifest(batch([], start=utc_start, end=utc_end), start=utc_start, end=utc_end)
    result = assess(tmp_path, j, m)
    assert result["status"] == "confirmed_empty"
    assert (result["start"], result["end"]) == (utc_start, utc_end)
    assert report_bytes(assess(tmp_path, m, j)) == report_bytes(result)


@pytest.mark.parametrize(
    ("updates", "code"),
    [
        ({"closed": False}, "open_batch"),
        ({"closed": 1}, "open_batch"),
        ({"count": True}, "inconsistent_count"),
        ({"count": -1}, "inconsistent_count"),
        ({"count": 1}, "inconsistent_count"),
        ({"end": START}, "invalid_interval"),
        ({"start": "2026-04-01T00:00:00"}, "invalid_interval"),
        ({"start": "2026-04-01T00:00:00.1234567890Z"}, "invalid_interval"),
        ({"start": "2026-04-01T00:00:00+12:61"}, "invalid_interval"),
        ({"id": "x" * 65}, "invalid_manifest"),
        ({"revision": "bad/file"}, "invalid_manifest"),
        ({"direction": "any"}, "invalid_direction"),
        ({"extra": "marker"}, "invalid_manifest"),
    ],
)
def test_invalid_batch_boundaries_are_refused(tmp_path, updates, code):
    source_row = dict(source(), **updates)
    with pytest.raises(ShadowInputError, match=code):
        assess(tmp_path, manifest(source_row), manifest(inventory()))


def test_oversized_and_malformed_entries_fail_closed(tmp_path):
    m = manifest(inventory())
    for entry in [
        {"id": "x", "sha256": ""},
        {"id": "x", "path": "wrong-side"},
        {"id": 3, "sha256": digest(b"")},
        [],
    ]:
        with pytest.raises(ShadowInputError, match="invalid_manifest"):
            assess(tmp_path, manifest(batch([entry])), m)
    with pytest.raises(ShadowInputError, match="input_limit_exceeded"):
        assess(
            tmp_path, manifest(batch([{"id": "x", "sha256": digest(b"")}] * 10_001)), m
        )
    with pytest.raises(ShadowInputError, match="unsafe_path"):
        assess(tmp_path, manifest(source()), manifest(inventory(("a", "../escape"))))


def test_expected_duplicate_identity_never_qualifies_even_when_bytes_agree(tmp_path):
    (tmp_path / "a").write_bytes(b"x")
    (tmp_path / "b").write_bytes(b"x")
    j = manifest(source(("A", b"x"), ("A", b"x")))
    m = manifest(inventory(("A", "a"), ("A", "b")))
    result = assess(tmp_path, j, m)
    assert result["reason_codes"] == ["duplicate"]
    assert result["status"] == "mismatch"


@pytest.mark.parametrize("field", ["scope", "capture_layer"])
def test_capture_layer_and_cohort_binding_must_match(tmp_path, field):
    j, m = manifest(source()), manifest(inventory())
    m[field] = "different"
    assert assess(tmp_path, j, m)["reason_codes"] == ["scope_mismatch"]


def test_partial_batch_set_and_changed_interval_are_unknown(tmp_path):
    first = batch([], end=MID)
    second = batch([], name="two", start=MID)
    journal = manifest(first, second)
    for mirror, reason in [
        (manifest(first), "missing_batch"),
        (
            manifest(first, dict(second, start="2026-04-01T00:09:00Z")),
            "interval_mismatch",
        ),
    ]:
        result = assess(tmp_path, journal, mirror)
        assert result["status"] == "unknown"
        assert reason in result["reason_codes"]
        assert result["counts"]["confirmed_empty_batches"] is None


def test_aliases_cannot_be_correlated_across_directions_and_revisions():
    original = issue_tokens("c", "wire", "inbound", "b", "r1", "o")
    assert all(
        a != b
        for a, b in zip(
            original, issue_tokens("c", "wire", "outbound", "b", "r1", "o"), strict=True
        )
    )
    assert all(
        a != b
        for a, b in zip(
            original, issue_tokens("c", "wire", "inbound", "b", "r2", "o"), strict=True
        )
    )
