"""Reconcile a sealed source journal against an independently acquired local mirror."""

from __future__ import annotations

import hashlib
import json
import re
from collections import Counter, defaultdict
from dataclasses import asdict, dataclass
from datetime import UTC, datetime

from .shadow_io import ShadowInputError, Snapshot, validate_relative

_ALIAS = re.compile(r"[A-Za-z0-9_-]{1,64}")
_SHA256 = re.compile(r"[A-Fa-f0-9]{64}")
_INSTANT = re.compile(
    r"([0-9]{4}-[0-9]{2}-[0-9]{2})[Tt]([0-9]{2}:[0-9]{2}:[0-9]{2})"
    r"(?:\.([0-9]{1,9}))?([Zz]|[+-](?:[01][0-9]|2[0-3]):[0-5][0-9])"
)
_DIRECTIONS = {"inbound", "outbound"}
_MAX_ENTRIES = 10_000
_UNKNOWN_CODES = {
    "invalid_arguments",
    "input_unavailable",
    "unsafe_path",
    "snapshot_changed",
    "input_limit_exceeded",
    "invalid_json",
    "unsupported_platform",
    "invalid_manifest",
    "unsupported_version",
    "invalid_direction",
    "invalid_interval",
    "open_batch",
    "inconsistent_count",
    "conflicting_batch",
    "scope_mismatch",
    "missing_batch",
    "revision_mismatch",
    "interval_mismatch",
    "coverage_gap",
    "coverage_overlap",
    "reused_path",
    "unlisted_file",
    "missing_file",
}


def _fingerprint(value: object) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _batch_token(
    scope: str, layer: str, direction: str, batch: str, revision: str
) -> str:
    return _fingerprint(
        ["mako.shadow.batch.v1", scope, layer, direction, batch, revision]
    )


def issue_tokens(
    scope: str,
    capture_layer: str,
    direction: str,
    batch_id: str,
    revision: str,
    occurrence_id: str,
) -> tuple[str, str]:
    """Derive the portable batch and occurrence aliases from a private journal row."""
    batch = _batch_token(scope, capture_layer, direction, batch_id, revision)
    return batch, _fingerprint(["mako.shadow.occurrence.v1", batch, occurrence_id])


def unknown_report(code: str) -> dict:
    """Build the portable failure result, without reflecting input or OS error text."""
    return {
        "version": 1,
        "status": "unknown",
        "eligible_for_comparison": False,
        "eligible_inbound": 0,
        "scope_fingerprint": None,
        "fingerprints": {"journal": None, "inventory": None, "observed": None},
        "start": None,
        "end": None,
        "directions": [],
        "counts": dict.fromkeys(
            (
                "expected",
                "observed",
                "expected_identities",
                "observed_identities",
                "missing",
                "extra",
                "duplicate",
                "changed",
                "confirmed_empty_batches",
            ),
            None,
        ),
        "reason_codes": [code if code in _UNKNOWN_CODES else "invalid_manifest"],
        "issues": [],
        "batches": [],
    }


def _record(value: object, keys: str) -> dict:
    if not isinstance(value, dict) or value.keys() != set(keys.split()):
        raise ShadowInputError("invalid_manifest")
    return value


def _list(value: object) -> list:
    if not isinstance(value, list):
        raise ShadowInputError("invalid_manifest")
    return value


def _alias(value: object) -> str:
    if not isinstance(value, str) or not _ALIAS.fullmatch(value):
        raise ShadowInputError("invalid_manifest")
    return value


def _instant(value: object) -> str:
    """A UTC sort key with a whole-second prefix and an exact fractional suffix."""
    match = _INSTANT.fullmatch(value) if isinstance(value, str) else None
    if match is None:
        raise ShadowInputError("invalid_interval")
    day, clock, fraction, offset = match.groups()
    try:
        local = datetime.fromisoformat(f"{day}T{clock}{offset.upper()}")
        whole = local.astimezone(UTC).isoformat(timespec="seconds").removesuffix("+00:00")
    except (ValueError, OverflowError):
        raise ShadowInputError("invalid_interval") from None
    # datetime truncates sub-microsecond values. Even a one-nanosecond hole in
    # claimed coverage must stay visible. The empty suffix orders before ".1".
    fraction = (fraction or "").rstrip("0")
    return f"{whole}.{fraction}" if fraction else whole


def _interval(row: dict) -> tuple[str, str]:
    start, end = _instant(row["start"]), _instant(row["end"])
    if start >= end:
        raise ShadowInputError("invalid_interval")
    return start, end


@dataclass(frozen=True)
class _Scope:
    scope: str
    capture_layer: str
    directions: tuple[str, ...]
    start: str
    end: str


@dataclass(frozen=True)
class _Batch:
    id: str
    revision: str
    direction: str
    start: str
    end: str
    entries: tuple[tuple[str, str], ...]


@dataclass
class _Manifest:
    scope: _Scope
    batches: dict[tuple[str, str], _Batch]

    def fingerprint(self, role: str) -> str:
        return _fingerprint(
            [
                "mako.shadow.manifest.v1",
                role,
                asdict(self.scope),
                [asdict(self.batches[key]) for key in sorted(self.batches)],
            ]
        )


def _batch(raw: object, scope: _Scope, field: str) -> _Batch:
    row = _record(raw, "id revision direction start end closed count entries")
    batch_id, revision = _alias(row["id"]), _alias(row["revision"])
    direction = row["direction"]
    if direction not in scope.directions:
        raise ShadowInputError("invalid_direction")
    start, end = _interval(row)
    if start < scope.start or end > scope.end:
        raise ShadowInputError("invalid_interval")
    if row["closed"] is not True:
        raise ShadowInputError("open_batch")
    rows = _list(row["entries"])
    if len(rows) > _MAX_ENTRIES:
        raise ShadowInputError("input_limit_exceeded")
    if type(row["count"]) is not int or row["count"] != len(rows):
        raise ShadowInputError("inconsistent_count")
    entries = []
    for item in rows:
        entry = _record(item, f"id {field}")
        identifier = _alias(entry["id"])
        value = entry[field]
        if not isinstance(value, str):
            raise ShadowInputError("invalid_manifest")
        if field == "path":
            validate_relative(value)
        else:
            if not _SHA256.fullmatch(value):
                raise ShadowInputError("invalid_manifest")
            value = value.lower()
        entries.append((identifier, value))
    return _Batch(batch_id, revision, direction, start, end, tuple(sorted(entries)))


def _manifest(raw: object, field: str) -> _Manifest:
    if raw is None:
        raise ShadowInputError("input_unavailable")
    row = _record(raw, "version scope capture_layer directions start end batches")
    if type(row["version"]) is not int or row["version"] != 1:
        raise ShadowInputError("unsupported_version")
    directions = _list(row["directions"])
    if (
        not 1 <= len(directions) <= 2
        or any(not isinstance(d, str) or d not in _DIRECTIONS for d in directions)
        or len(set(directions)) != len(directions)
    ):
        raise ShadowInputError("invalid_direction")
    scope = _Scope(
        _alias(row["scope"]),
        _alias(row["capture_layer"]),
        tuple(sorted(directions)),
        *_interval(row),
    )
    batches = {}
    for item in _list(row["batches"]):
        batch = _batch(item, scope, field)
        key = batch.direction, batch.id
        if batches.setdefault(key, batch) != batch:
            raise ShadowInputError("conflicting_batch")
    return _Manifest(scope, batches)


def _coverage(source: _Manifest, mirror: _Manifest) -> set[str]:
    if source.scope != mirror.scope:
        return {"scope_mismatch"}
    reasons = set()
    if source.batches.keys() != mirror.batches.keys():
        reasons.add("missing_batch")
    for key in source.batches.keys() & mirror.batches.keys():
        expected, observed = source.batches[key], mirror.batches[key]
        if expected.revision != observed.revision:
            reasons.add("revision_mismatch")
        if (expected.start, expected.end) != (observed.start, observed.end):
            reasons.add("interval_mismatch")
    for direction in source.scope.directions:
        cursor = source.scope.start
        windows = sorted(
            (b.start, b.end) for b in source.batches.values() if b.direction == direction
        )
        for start, end in windows:
            if start < cursor:
                reasons.add("coverage_overlap")
            if start > cursor:
                reasons.add("coverage_gap")
            cursor = max(cursor, end)
        if cursor != source.scope.end:
            reasons.add("coverage_gap")
    return reasons


def _compare(scope: _Scope, batch: _Batch, actual: list[tuple[str, str]]) -> list[dict]:
    expected = defaultdict(Counter)
    observed = defaultdict(Counter)
    for identifier, digest in batch.entries:
        expected[identifier][digest] += 1
    for identifier, digest in actual:
        observed[identifier][digest] += 1
    issues = []
    for identifier in expected.keys() | observed.keys():
        left, right = expected[identifier], observed[identifier]
        n, m = left.total(), right.total()
        matched = (left & right).total()
        # An extra copy of an expected identity is a duplicate, not evidence
        # for another occurrence. Changed counts pair unmatched copies by ID.
        counts = {
            "missing": max(n - m, 0),
            "extra": int(not n and m > 0),
            "duplicate": max(n, m) - 1,
            "changed": min(n, m) - matched,
        }
        if any(counts.values()):
            batch_token, occurrence_token = issue_tokens(
                scope.scope,
                scope.capture_layer,
                batch.direction,
                batch.id,
                batch.revision,
                identifier,
            )
            issues.extend(
                {
                    "reason": reason,
                    "batch": batch_token,
                    "occurrence": occurrence_token,
                    "count": count,
                }
                for reason, count in counts.items()
                if count
            )
    return issues


def reconcile(journal: object, inventory: object, snapshot: Snapshot) -> dict:
    """Return current coverage; reject invalid input with a safe ShadowInputError."""
    source, mirror = _manifest(journal, "sha256"), _manifest(inventory, "path")
    scope = source.scope
    report = unknown_report("invalid_manifest")
    report.update(
        scope_fingerprint=_fingerprint(["mako.shadow.scope.v1", asdict(scope)]),
        start=scope.start + "Z",
        end=scope.end + "Z",
        directions=list(scope.directions),
    )
    fingerprints = report["fingerprints"]
    fingerprints["journal"] = source.fingerprint("journal")
    fingerprints["inventory"] = mirror.fingerprint("inventory")
    counts = report["counts"]
    counts["expected"] = sum(len(b.entries) for b in source.batches.values())
    counts["expected_identities"] = sum(
        len({identifier for identifier, _ in b.entries}) for b in source.batches.values()
    )
    indexed = sum(len(b.entries) for b in mirror.batches.values())

    reasons = _coverage(source, mirror)
    paths = {path for batch in mirror.batches.values() for _, path in batch.entries}
    if len(paths) != indexed:
        reasons.add("reused_path")
    files = snapshot.files()
    if files - paths:
        reasons.add("unlisted_file")
    if paths - files:
        reasons.add("missing_file")
    if reasons:
        report["reason_codes"] = sorted(reasons)
        return report

    counts["observed"] = indexed
    counts["observed_identities"] = sum(
        len({identifier for identifier, _ in b.entries}) for b in mirror.batches.values()
    )
    counts.update(missing=0, extra=0, duplicate=0, changed=0, confirmed_empty_batches=0)
    observed_batches = []
    eligible_inbound = 0
    issues = []
    for key, expected in sorted(source.batches.items()):
        observed = sorted(
            (identifier, snapshot.digest(path))
            for identifier, path in mirror.batches[key].entries
        )
        observed_batches.append([key, expected.revision, observed])
        defects = _compare(scope, expected, observed)
        issues.extend(defects)
        if defects:
            status = "mismatch"
        elif not expected.entries:
            status = "confirmed_empty"
            counts["confirmed_empty_batches"] += 1
        else:
            status = "complete"
            if expected.direction == "inbound":
                eligible_inbound += len(expected.entries)
        report["batches"].append(
            {
                "batch": _batch_token(
                    scope.scope,
                    scope.capture_layer,
                    expected.direction,
                    expected.id,
                    expected.revision,
                ),
                "direction": expected.direction,
                "start": expected.start + "Z",
                "end": expected.end + "Z",
                "status": status,
                "expected": len(expected.entries),
                "observed": len(observed),
            }
        )
    report["batches"].sort(key=lambda b: (b["direction"], b["start"][:-1], b["batch"]))
    fingerprints["observed"] = _fingerprint(
        ["mako.shadow.observed.v1", fingerprints["inventory"], observed_batches]
    )
    for issue in issues:
        counts[issue["reason"]] += issue["count"]
    report["issues"] = sorted(
        issues, key=lambda i: (i["batch"], i["occurrence"], i["reason"])
    )
    report["reason_codes"] = sorted({i["reason"] for i in issues})
    if issues:
        report["status"] = "mismatch"
    else:
        report["status"] = "complete" if counts["expected"] else "confirmed_empty"
        report["eligible_inbound"] = eligible_inbound
        report["eligible_for_comparison"] = eligible_inbound > 0
    return report
