"""Compare one offline Strom NB answer with independently supplied evidence."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import subprocess
import sys
from datetime import datetime, time, timedelta

from . import _native as edi
from .shadow import _alias, _fingerprint, _instant, _manifest, _record, reconcile
from .shadow_io import ShadowInputError, Snapshot


def _require(condition: bool, reason: str) -> None:
    if not condition:
        raise ShadowInputError(reason)


def _transaction(raw: bytes, day: str):
    parsed = edi.validate_edifact(raw, day)
    _require(
        parsed.is_valid
        and parsed.rules_applied
        and parsed.envelope is not None
        and len(parsed.messages) == 1
        and len(parsed.messages[0].vorgaenge) == 1,
        "unsupported_interchange",
    )
    return parsed.envelope, parsed.messages[0], parsed.messages[0].vorgaenge[0]


def _answer(raw: bytes, case: dict, token: str):
    received = datetime.fromisoformat(edi.in_berlin(case["received_at"]))
    day = received.date().isoformat()
    env, message, tx = _transaction(raw, day)
    delivery = tx.iso_date("92")
    _require(
        message.pruefidentifikator == 55001
        and message.release == edi.release_for("UTILMD", day, "STROM") == "S2.2"
        and bool(tx.transaktionsgrund)
        and tx.transaktionsgrund_ergaenzung == "ZW4"
        and bool(tx.vorgangsnummer)
        and len(tx.locations) == 1
        and tx.location("Z16") == case["malo"]
        and env.receiver_id == case["nb"]
        and case["role"] == "NB"
        and bool(delivery)
        and case["grid_member"] is True
        and case["supply_status"] == "Stillgelegt",
        "unsupported_case",
    )
    tree, code, step = edi.decommissioned_nb_refusal(
        case["malo"], delivery, case["received_at"]
    )
    reply_id = f"R{token}"
    if reply_id == tx.vorgangsnummer:
        reply_id = f"S{token}"
    reply_tx = edi.UtilmdTransaction(
        reply_id,
        transaktionsgrund=tx.transaktionsgrund,
        transaktionsgrund_ergaenzung=tx.transaktionsgrund_ergaenzung,
        antwort_code=code,
        antwort_ebd=tree,
        references=[("TN", tx.vorgangsnummer)],
    )
    msg = edi.build_utilmd(
        55003,
        env.receiver_id,
        env.sender_id,
        on=day,
        sparte="STROM",
        message_ref=token,
        transactions=[reply_tx],
    )
    wire = edi.build_interchange(
        env.receiver_id,
        env.sender_id,
        token,
        [msg],
        on=day,
        time=received.strftime("%H%M"),
    )
    envelope, answer, reply = _transaction(wire, day)
    _require(
        envelope.sender_id == env.receiver_id
        and envelope.receiver_id == env.sender_id
        and answer.release == message.release
        and answer.pruefidentifikator == 55003
        and reply.vorgangsnummer == reply_id
        and reply.transaktionsgrund == tx.transaktionsgrund
        and reply.transaktionsgrund_ergaenzung == tx.transaktionsgrund_ergaenzung
        and reply.antwort_code == code
        and reply.antwort_codeliste == tree
        and reply.reference("TN") == tx.vorgangsnummer,
        "invalid_rendered_answer",
    )
    return (
        wire,
        tx.vorgangsnummer,
        {
            "pid": answer.pruefidentifikator,
            "code": reply.antwort_code,
            "tree": reply.antwort_codeliste,
        },
        step,
    )


def _daemon_answer(workspace, nb: str, scope: str, packet: bytes):
    try:
        workspace.mkdir(mode=0o700, exist_ok=True)
    except OSError as error:
        raise ShadowInputError("workspace_unavailable") from error
    cmd = ["makod", "shadow", "--workspace", str(workspace), "--nb", nb]

    def call(action, *args, data=None):
        try:
            result = subprocess.run(
                [*cmd, action, *args],
                input=data if data is not None else b"",
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                timeout=45,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise ShadowInputError("daemon_unavailable") from error
        _require(result.returncode == 0, f"daemon_{action}_refused")
        return result.stdout

    call("prepare", "--scope", scope, data=packet)
    capture = json.loads(call("run"))
    _require(
        isinstance(capture, dict) and isinstance(capture.get("captured"), dict),
        "invalid_daemon_report",
    )
    counts = capture["captured"]
    _require(
        capture.get("mode") == "shadow"
        and capture.get("pending") == 0
        and capture.get("dead_letters") is False
        and not counts.get("unrenderable")
        and not counts.get("refused"),
        "daemon_capture_incomplete",
    )
    return call("answer"), counts


def _review_rows(source, mirror, snapshot, nb):
    """Bind every inbound envelope to the same NB before projecting its identity."""
    rows = []
    for key, batch in sorted(source.batches.items()):
        if batch.direction != "inbound":
            continue
        paths = dict(mirror.batches[key].entries)
        day = edi.in_berlin(batch.start + "Z")[:10]
        for occurrence, digest in batch.entries:
            raw = snapshot.verified_bytes(paths[occurrence], digest)
            envelope = edi.validate_edifact(raw, day).envelope
            _require(
                envelope is not None and envelope.receiver_id == nb, "scope_mismatch"
            )
            identity = _fingerprint(
                [
                    "mako.shadow.review.v1",
                    nb,
                    source.scope.scope,
                    source.scope.capture_layer,
                    *key,
                    occurrence,
                ]
            )
            rows.append(
                {
                    "id": identity,
                    "batch_revision": _fingerprint(batch.revision),
                    "request_sha256": digest,
                    "assessment": None,
                }
            )
    return rows


def _complete_werktage(start, end):
    first = datetime.fromisoformat(edi.in_berlin(start + "Z"))
    last = datetime.fromisoformat(edi.in_berlin(end + "Z"))
    day = first.date()
    if first.time() != time.min or ("." in start and start.split(".")[1].strip("0")):
        day += timedelta(days=1)
    _require((last.date() - day).days <= 3660, "input_limit_exceeded")
    count = 0
    while day < last.date():
        count += int(edi.is_werktag(day.isoformat()))
        day += timedelta(days=1)
    return count


def _paired_reply(snapshot, source, mirror, request, received_at):
    """Select a reply only when the parties and reference identify one request."""
    env, message, tx = _transaction(request, edi.in_berlin(received_at)[:10])
    _require(
        message.pruefidentifikator == 55001 and bool(tx.vorgangsnummer),
        "unsupported_case",
    )
    inbound_matches = 0
    paired = None
    for key, batch in source.batches.items():
        paths = dict(mirror.batches[key].entries)
        for alias, sha in batch.entries:
            raw = snapshot.verified_bytes(paths[alias], sha)
            packet = edi.validate_edifact(raw, edi.in_berlin(batch.start + "Z")[:10])
            peer = packet.envelope
            _require(
                peer is not None and packet.is_valid and packet.rules_applied,
                "unassessed_interchange",
            )
            parties = peer.sender_id, peer.receiver_id
            if batch.direction == "inbound":
                if parties == (env.sender_id, env.receiver_id):
                    inbound_matches += sum(
                        transaction.vorgangsnummer == tx.vorgangsnummer
                        for msg in packet.messages
                        if msg.pruefidentifikator == 55001
                        for transaction in msg.vorgaenge
                    )
                continue
            if parties != (env.receiver_id, env.sender_id):
                continue
            for answer in packet.messages:
                if answer.pruefidentifikator not in edi.answer_pids(55001):
                    continue
                for reply in answer.vorgaenge:
                    if reply.reference("TN") != tx.vorgangsnummer:
                        continue
                    _require(paired is None, "ambiguous_observation")
                    _require(
                        message.release == answer.release
                        and reply.transaktionsgrund == tx.transaktionsgrund
                        and reply.transaktionsgrund_ergaenzung
                        == tx.transaktionsgrund_ergaenzung
                        and reply.antwort_codeliste
                        and reply.antwort_code,
                        "unpaired_observation",
                    )
                    paired = (
                        sha,
                        {
                            "pid": answer.pruefidentifikator,
                            "tree": reply.antwort_codeliste,
                            "code": reply.antwort_code,
                        },
                    )
    _require(inbound_matches == 1, "ambiguous_observation")
    return paired


def _observe(path, snapshot, source, mirror, case, request, digest, expected, report):
    """Correlate the custodian's positive receipt export with the mirrored reply."""
    obs = _record(snapshot.read_external(path), "version scope inbound outbound")
    _require(
        type(obs["version"]) is int
        and obs["version"] == 1
        and obs["scope"] == source.scope.scope,
        "unpaired_observation",
    )

    def receipt(value):
        r = _record(value, "sha256 receipt_sha256 received_at positive")
        _require(
            r["positive"] is True
            and isinstance(r["receipt_sha256"], str)
            and re.fullmatch("[a-f0-9]{64}", r["receipt_sha256"]) is not None,
            "invalid_receipt",
        )
        return r, _instant(r["received_at"])

    inbound, arrived = receipt(obs["inbound"])
    outbound, delivered = receipt(obs["outbound"])
    _require(
        inbound["sha256"] == digest
        and arrived == _instant(case["received_at"])
        and arrived <= delivered
        and inbound["receipt_sha256"] != outbound["receipt_sha256"],
        "unpaired_observation",
    )
    paired = _paired_reply(snapshot, source, mirror, request, case["received_at"])
    _require(paired is not None, "unpaired_observation")
    sha, answer = paired
    _require(
        sha == outbound["sha256"] and all(expected[k] == v for k, v in answer.items()),
        "unpaired_observation",
    )
    report["observation_fingerprint"] = _fingerprint(obs)
    report["incumbent_received_at"] = outbound["received_at"]
    report["timing"] = (
        "late" if delivered > _instant(report["deadline"]["due_at"]) else "on_time"
    )


def rehearse(args: argparse.Namespace, snapshot: Snapshot, report: dict) -> bytes | None:
    journal, inventory = snapshot.read_manifests(args.journal, args.inventory)
    coverage = reconcile(journal, inventory, snapshot)
    _require(
        _fingerprint(snapshot.read_external(args.coverage)) == _fingerprint(coverage),
        "coverage_changed",
    )
    _require(coverage["status"] in ("complete", "confirmed_empty"), "coverage_incomplete")
    eligible = coverage["eligible_inbound"]
    report.update(
        coverage=coverage["fingerprints"],
        scope=coverage["scope_fingerprint"],
        eligible=eligible,
        assessed=0,
        unassessed=eligible,
    )
    source, mirror = _manifest(journal, "sha256"), _manifest(inventory, "path")
    if args.emit == "review":
        nb = args.review_nb
        _require(
            isinstance(nb, str) and nb.isascii() and nb.isdigit() and len(nb) == 13,
            "missing_review_nb",
        )
        report["_review"] = {
            "version": 1,
            "scope": report["scope"],
            "nb": nb,
            "role": "NB",
            "coverage": report["coverage"],
            "directions": list(source.scope.directions),
            "start": source.scope.start + "Z",
            "end": source.scope.end + "Z",
            "complete_werktage": _complete_werktage(source.scope.start, source.scope.end),
            "rows": _review_rows(source, mirror, snapshot, nb),
        }
    if not eligible:
        report["status"] = "no_candidates"
        return None
    case = _record(
        snapshot.read_external(args.case),
        "version scope capture_layer batch occurrence role nb malo grid_member "
        "supply_status received_at",
    )
    expected = _record(
        snapshot.read_external(args.expected),
        "version scope batch revision occurrence request_sha256 vorgang pid tree code",
    )
    _require(
        all(
            type(obj["version"]) is int and obj["version"] == 1
            for obj in (case, expected)
        ),
        "unsupported_version",
    )
    key = ("inbound", _alias(case["batch"]))
    occurrence = _alias(case["occurrence"])
    batch = source.batches.get(key)
    _require(
        batch is not None
        and source.scope.scope == case["scope"]
        and source.scope.capture_layer == case["capture_layer"] == "wire",
        "scope_mismatch",
    )
    received = _instant(case["received_at"])
    _require(batch.start <= received < batch.end, "receipt_outside_batch")
    digest = dict(batch.entries).get(occurrence)
    _require(
        digest is not None
        and expected["request_sha256"] == digest
        and expected["batch"] == batch.id
        and expected["revision"] == batch.revision
        and expected["occurrence"] == occurrence
        and expected["scope"] == source.scope.scope,
        "unpaired_expectation",
    )
    if args.emit == "review":
        _require(args.review_nb == case["nb"], "scope_mismatch")
        report["_selected"] = _fingerprint(
            [
                "mako.shadow.review.v1",
                case["nb"],
                source.scope.scope,
                source.scope.capture_layer,
                *key,
                occurrence,
            ]
        )
    raw = snapshot.verified_bytes(dict(mirror.batches[key].entries)[occurrence], digest)
    token = _fingerprint([coverage["scope_fingerprint"], key, occurrence, digest])[:14]
    wire, vorgang, actual, step = _answer(raw, case, token)
    _require(expected["vorgang"] == vorgang, "unpaired_expectation")
    _require(
        type(expected["pid"]) is int
        and expected["tree"] in ("E_0622", "E_0623")
        and isinstance(expected["code"], str),
        "invalid_expectation",
    )
    entry = edi.antwort_code(expected["tree"], expected["code"])
    approved, refused = edi.answer_pids(55001)
    _require(
        entry is not None
        and entry.ist_zustimmung is not None
        and expected["pid"] == (approved if entry.ist_zustimmung else refused),
        "invalid_expectation",
    )
    private_packet = None
    if args.emit == "daemon-input" or args.makod_workspace is not None:
        packet = {
            "version": 1,
            "coverage": coverage["fingerprints"],
            "scope": coverage["scope_fingerprint"],
            "direction": key[0],
            "role": case["role"],
            "batch": batch.id,
            "revision": batch.revision,
            "occurrence": occurrence,
            "request_sha256": digest,
            "wire": base64.b64encode(raw).decode("ascii"),
            "received_at": case["received_at"],
            "nb": case["nb"],
            "malo": case["malo"],
            "decision": actual,
            "grid_member": case["grid_member"],
            "supply_status": case["supply_status"],
            "pruefschritt": step,
            "facts": _fingerprint(case),
            "ebd_revision": "4.3",
        }
        private_packet = json.dumps(packet, sort_keys=True).encode("utf-8")
    obligation = edi.antwort_obligation(55001)
    _require(obligation is not None, "missing_answer_obligation")
    report.update(
        deadline={
            "due_at": obligation.due_at(case["received_at"]),
            "source": obligation.source,
        },
        pruefschritt=step,
        facts_fingerprint=_fingerprint(case),
        expected_fingerprint=_fingerprint(expected),
        release="S2.2",
        ebd_revision="4.3",
    )
    if args.observation is not None:
        _observe(
            args.observation,
            snapshot,
            source,
            mirror,
            case,
            raw,
            digest,
            expected,
            report,
        )
    _compare(report, wire, actual, {key: expected[key] for key in actual})
    return private_packet if private_packet is not None else wire


def _compare(report, wire, actual, expected):
    differences = [field for field, value in actual.items() if expected[field] != value]
    classifications = ["antwort"] if differences else []
    if report["timing"] == "late":
        differences.append("timing")
        classifications.append("frist")
    report.update(
        status="different" if differences else "matched",
        assessed=1,
        unassessed=report["eligible"] - 1,
        actual=actual,
        expected=expected,
        matched=int(not differences),
        different=int(bool(differences)),
        differences=differences,
        classifications=classifications or ["gleich"],
        answer_sha256=hashlib.sha256(wire).hexdigest(),
    )


def _compare_daemon(workspace, data, report):
    packet = json.loads(data)
    wire, captures = _daemon_answer(workspace, packet["nb"], packet["scope"], data)
    day = datetime.fromisoformat(edi.in_berlin(packet["received_at"])).date().isoformat()
    env, answer, reply = _transaction(wire, day)
    source, _, request = _transaction(base64.b64decode(packet["wire"]), day)
    _require(
        env.sender_id == source.receiver_id
        and env.receiver_id == source.sender_id
        and answer.release == report["release"]
        and reply.reference("TN") == request.vorgangsnummer
        and reply.transaktionsgrund == request.transaktionsgrund
        and reply.transaktionsgrund_ergaenzung == request.transaktionsgrund_ergaenzung,
        "unpaired_daemon_answer",
    )
    actual = {
        "pid": answer.pruefidentifikator,
        "code": reply.antwort_code,
        "tree": reply.antwort_codeliste,
    }
    _compare(report, wire, actual, report["expected"])
    report["capture"] = captures
    return wire


def unassessed_report() -> dict:
    return {
        "version": 1,
        "status": "unassessed",
        "eligible": None,
        "assessed": 0,
        "unassessed": None,
        "matched": 0,
        "different": 0,
        "timing": "unassessed",
    }


def _summary(report: dict) -> str:
    counts = [
        f"{report[key] if report[key] is not None else 'unknown'} {key}"
        for key in ("eligible", "assessed", "unassessed")
    ]
    lines = [f"{report['status']} | " + ", ".join(counts)]
    if "capture" in report:
        lines.append(
            "makod captured "
            + ", ".join(
                f"{count} {kind}" for kind, count in sorted(report["capture"].items())
            )
        )
    if "actual" in report:
        for label in ("expected", "actual"):
            reply = report[label]
            lines.append(f"{label}: PID {reply['pid']}  {reply['tree']}  {reply['code']}")
        timing = (
            "dispatch timing unassessed"
            if report["timing"] == "unassessed"
            else f"incumbent delivery {report['timing']}"
        )
        lines.append(f"Prüfschritt {report['pruefschritt']} | {timing}")
        lines.append(
            f"Reply due {report['deadline']['due_at']} | {report['deadline']['source']}"
        )
    elif "reason" in report:
        reason = report["reason"]
        hints = {
            "workspace_inside_mirror": "Choose a private directory outside the mirror.",
            "workspace_unavailable": "Check write access to the parent directory.",
            "daemon_unavailable": "Check that the matching makod binary is on PATH.",
            "daemon_prepare_refused": (
                "Check permissions (0700). Use a fresh directory for changed inputs."
            ),
            "daemon_capture_incomplete": "Inspect the workspace with the report command.",
        }
        lines.append(f"{reason} | {hints[reason]}" if reason in hints else reason)
    return "\n".join(lines)


def _evaluate(args: argparse.Namespace):
    report = unassessed_report()
    wire = None
    try:
        with Snapshot(args.mirror_root) as snapshot:
            try:
                wire = rehearse(args, snapshot, report)
            except ShadowInputError as error:
                if error.code == "snapshot_changed":
                    report = unassessed_report()
                report["reason"] = error.code
            except (ValueError, RuntimeError, OverflowError):
                report["reason"] = "invalid_input"
    except (ShadowInputError, OSError) as error:
        report = unassessed_report()
        report["reason"] = (
            error.code if isinstance(error, ShadowInputError) else "input_unavailable"
        )
        wire = None
    # Publish only after Snapshot verifies the independently inventoried inputs on exit.
    if wire is not None and args.makod_workspace is not None:
        try:
            _require(args.emit != "daemon-input", "conflicting_output")
            _require(
                not args.makod_workspace.resolve().is_relative_to(
                    args.mirror_root.resolve()
                ),
                "workspace_inside_mirror",
            )
            workspace = args.makod_workspace
            if args.also:
                workspace.mkdir(mode=0o700, exist_ok=True)
                workspace = workspace / report["_selected"]
            wire = _compare_daemon(workspace, wire, report)
        except (OSError, ShadowInputError, ValueError, RuntimeError) as error:
            report = unassessed_report() | {
                "eligible": report["eligible"],
                "coverage": report["coverage"],
                "reason": error.code
                if isinstance(error, ShadowInputError)
                else "daemon_unavailable",
            }
    if "reason" in report:
        report.update(status="unassessed", assessed=0, unassessed=report["eligible"])
        wire = None
    return report, wire


def _review(args):
    _require(len(args.also) <= 9999, "input_limit_exceeded")
    comparisons = [(args.case, args.expected, args.observation)]
    for pair in args.also:
        _require(len(pair) in (2, 3), "invalid_arguments")
        comparisons.append((pair[0], pair[1], pair[2] if len(pair) == 3 else None))
    packet = None
    rows = {}
    differs = False
    for case, expected, observation in comparisons:
        selected_args = argparse.Namespace(
            **(
                vars(args)
                | {
                    "case": case,
                    "expected": expected,
                    "observation": observation,
                }
            )
        )
        report, _ = _evaluate(selected_args)
        if "reason" in report:
            raise ShadowInputError(report["reason"])
        exported = report["_review"]
        if packet is None:
            packet = exported
            rows = {r["id"]: r for r in packet["rows"]}
        _require(
            exported["scope"] == packet["scope"]
            and exported["coverage"] == packet["coverage"],
            "coverage_changed",
        )
        selected = report.get("_selected")
        if selected is None:
            _require(len(comparisons) == 1, "no_candidates")
            continue
        row = rows[selected]
        _require(row["assessment"] is None, "duplicate_review")
        assessment = {
            key: report[key]
            for key in (
                "actual",
                "expected",
                "facts_fingerprint",
                "expected_fingerprint",
                "pruefschritt",
                "release",
                "ebd_revision",
                "answer_sha256",
                "deadline",
                "timing",
            )
        }
        assessment["observation_fingerprint"] = report.get("observation_fingerprint")
        assessment["incumbent_received_at"] = report.get("incumbent_received_at")
        assessment["producer"] = "makod" if "capture" in report else "reference"
        row["assessment"] = assessment
        differs |= report["status"] == "different"
    print(json.dumps(packet, sort_keys=True))
    return int(differs)


def run(args: argparse.Namespace) -> int:
    if args.emit == "review" or args.also:
        try:
            _require(args.emit == "review", "invalid_arguments")
            return _review(args)
        except ShadowInputError as error:
            print(
                json.dumps(unassessed_report() | {"reason": error.code}), file=sys.stderr
            )
            return 2
    report, wire = _evaluate(args)
    private = args.emit in ("answer", "daemon-input", "review")
    if private and wire is not None:
        sys.stdout.buffer.write(wire)
    elif args.emit == "summary":
        print(_summary(report))
    else:
        print(
            json.dumps(report, sort_keys=True),
            file=sys.stderr if private else sys.stdout,
        )
    return {"matched": 0, "different": 1, "unassessed": 2, "no_candidates": 0}[
        report["status"]
    ]
