"""Compare replies to one inbound request across two closed captures."""

import json

from . import _native as edi
from .rehearsal import _paired_reply, _require, _transaction
from .shadow import _fingerprint, _instant, _manifest, _record, reconcile
from .shadow_io import ShadowInputError, Snapshot


def _side(manifests, snapshot, case):
    journal, inventory = manifests
    coverage = reconcile(journal, inventory, snapshot)
    _require(
        coverage["status"] == "complete"
        and coverage["directions"] == ["inbound", "outbound"],
        "coverage_incomplete",
    )
    source, mirror = _manifest(journal, "sha256"), _manifest(inventory, "path")
    _require(
        source.scope.scope == case["scope"]
        and source.scope.capture_layer == case["capture_layer"] == "wire"
        and case["role"] == "NB",
        "scope_mismatch",
    )
    key = ("inbound", case["batch"])
    batch = source.batches.get(key)
    received = _instant(case["received_at"])
    _require(
        batch is not None and batch.start <= received < batch.end, "unpaired_request"
    )
    digest = dict(batch.entries).get(case["occurrence"])
    _require(digest is not None, "unpaired_request")
    raw = snapshot.verified_bytes(
        dict(mirror.batches[key].entries)[case["occurrence"]], digest
    )
    env, _, tx = _transaction(raw, edi.in_berlin(case["received_at"])[:10])
    _require(
        env.receiver_id == case["nb"] and tx.location("Z16") == case["malo"],
        "scope_mismatch",
    )
    reply = _paired_reply(snapshot, source, mirror, raw, case["received_at"])
    return coverage, digest, reply[1] if reply else None


def run(args):
    try:
        _require(args.incumbent.resolve() != args.shadow.resolve(), "same_capture")
        with (
            Snapshot(args.incumbent / "mirror") as incumbent,
            Snapshot(args.shadow / "mirror") as shadow,
        ):
            old_inputs = incumbent.read_manifests(
                args.incumbent / "journal.json", args.incumbent / "inventory.json"
            )
            new_inputs = shadow.read_manifests(
                args.shadow / "journal.json", args.shadow / "inventory.json"
            )
            case = _record(
                incumbent.read_external(args.case),
                "version scope capture_layer batch occurrence role nb malo grid_member "
                "supply_status received_at",
            )
            _require(
                type(case["version"]) is int and case["version"] == 1,
                "unsupported_version",
            )
            old, old_digest, expected = _side(old_inputs, incumbent, case)
            new, new_digest, actual = _side(new_inputs, shadow, case)
            _require(
                old["scope_fingerprint"] == new["scope_fingerprint"]
                and old_digest == new_digest,
                "scope_mismatch",
            )
            if actual and expected:
                kind = "gleich" if actual == expected else "antwort"
            elif actual or expected:
                kind = "nur_mako" if actual else "nur_alt"
            else:
                kind = "unassessed"
            report = {
                "version": 1,
                "classification": kind,
                "expected": expected,
                "actual": actual,
                "scope": old["scope_fingerprint"],
                "incumbent_coverage": old["fingerprints"],
                "shadow_coverage": new["fingerprints"],
                "case_fingerprint": _fingerprint(case),
                "timing": "unassessed",
            }
    except (
        ShadowInputError,
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        OverflowError,
    ) as error:
        report = {
            "version": 1,
            "classification": "unassessed",
            "reason": error.code
            if isinstance(error, ShadowInputError)
            else "invalid_input",
        }
    print(json.dumps(report, sort_keys=True))
    return {"gleich": 0, "antwort": 1, "nur_mako": 1, "nur_alt": 1, "unassessed": 2}[
        report["classification"]
    ]
