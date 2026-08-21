"""`makotest` on the command line.

A library you cannot point at a file from a shell is one only Python teams
adopt. The three things worth reaching for without writing a test are: what is
wrong with this interchange, when is this answer due, and is this identifier one
a counterparty would accept.

Everything here is a thin front end over the same bindings the assertions use,
so the CLI and a test cannot disagree about an answer.

    makotest validate inbound.edi --on 2026-04-01
    makotest frist 55001 --received 2026-03-02T09:00:00Z
    makotest id 9900357000004
    makotest pids UTILMD --on 2026-04-01 --sparte STROM

Exit status is 0 when the answer is yes and 1 when it is no, so a shell can
branch on it.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from typing import Any

from ._native import (
    antwort_obligation,
    eic_is_valid,
    eic_type_char,
    format_versions,
    malo_is_valid,
    melo_is_valid,
    message_types_of,
    mp_id_authority,
    mp_id_check_digit_schemes,
    mp_id_is_valid,
    pruefidentifikatoren,
    release_for,
    validate_edifact,
)

__all__ = ["main"]


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    handler = {
        "validate": _validate,
        "frist": _frist,
        "id": _identify,
        "pids": _pids,
        "versions": _versions,
    }[args.command]
    try:
        return handler(args)
    except (ValueError, OSError) as exc:
        print(f"makotest: {exc}", file=sys.stderr)
        return 2


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="makotest",
        description=(
            "Validate EDIFACT, resolve answer Fristen and inspect BDEW "
            "identifiers, through the same engine a MaKo platform runs."
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit machine-readable JSON instead of a human report",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    validate = sub.add_parser(
        "validate",
        help="validate an EDIFACT interchange (MIG + AHB + semantic rules)",
    )
    validate.add_argument("file", help="path to the interchange, or - for stdin")
    validate.add_argument(
        "--on",
        required=True,
        metavar="ISO_DATE",
        help=(
            "the date the message would be sent, which selects the BDEW format "
            "version. Required: the same bytes are valid on one and invalid on "
            "another."
        ),
    )

    frist = sub.add_parser("frist", help="the published answer Frist for a PID")
    frist.add_argument("pid", type=int, help="the inbound Prüfidentifikator")
    frist.add_argument(
        "--received",
        metavar="RFC3339",
        help="when the request arrived; prints the instant the answer is due",
    )

    identify = sub.add_parser("id", help="classify a BDEW identifier")
    identify.add_argument("value", help="a MaLo, MeLo, Marktpartner-ID or EIC")

    pids = sub.add_parser(
        "pids", help="Prüfidentifikatoren the compiled profiles validate"
    )
    pids.add_argument("message_type", help="e.g. UTILMD")
    pids.add_argument("--on", metavar="ISO_DATE", help="restrict to that date's profile")
    pids.add_argument("--sparte", choices=["STROM", "GAS"], help="UTILMD track")

    sub.add_parser("versions", help="every compiled BDEW format version")
    return parser


def _emit(args: argparse.Namespace, payload: dict[str, Any], lines: list[str]) -> None:
    if args.json:
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        print("\n".join(lines))


def _validate(args: argparse.Namespace) -> int:
    raw = sys.stdin.buffer.read() if args.file == "-" else _read(args.file)
    report = validate_edifact(raw, args.on)
    payload: dict[str, Any] = {
        "valid": report.is_valid,
        "on": args.on,
        "messages": [
            {
                "index": m.index,
                "message_ref": m.message_ref,
                "message_type": m.message_type,
                "pruefidentifikator": m.pruefidentifikator,
                "release": m.release,
                "valid": m.is_valid,
                "rules_applied": m.rules_applied,
                "findings": [
                    {
                        "severity": f.severity,
                        "rule_id": f.rule_id,
                        "rule_origin": f.rule_origin,
                        "position": f.position,
                        "message": f.message,
                        "suggestion": f.suggestion,
                    }
                    for f in m.findings
                ],
            }
            for m in report.messages
        ],
    }
    if report.envelope is not None:
        payload["envelope"] = {
            "sender": report.envelope.sender_id,
            "sender_qualifier": report.envelope.sender_qualifier,
            "receiver": report.envelope.receiver_id,
            "receiver_qualifier": report.envelope.receiver_qualifier,
            "control_ref": report.envelope.control_ref,
            "transmission_date": report.envelope.transmission_date,
            "test_indicator": report.envelope.test_indicator,
            "structurally_valid": report.envelope.is_structurally_valid,
        }

    lines = []
    if report.envelope is not None:
        e = report.envelope
        lines.append(
            f"{e.sender_id}:{e.sender_qualifier} → {e.receiver_id}:"
            f"{e.receiver_qualifier}  ref={e.control_ref}"
            + ("  [TEST]" if e.test_indicator else "")
        )
    for m in report.messages:
        lines.append(
            f"#{m.index} {m.message_type or '?'} {m.release or '?'} "
            f"pid={m.pruefidentifikator or '-'} "
            f"{'valid' if m.is_valid else 'INVALID'}"
            + ("" if m.rules_applied else "  ⚠ no AHB rules — checked nothing")
        )
        for f in m.findings:
            lines.append(
                f"    {f.severity:8} [{f.rule_id or '-'}] {f.position or '-'}: "
                f"{f.message}"
            )
            if f.suggestion:
                lines.append(f"             hint: {f.suggestion}")
    vacuous = [m for m in report.messages if not m.rules_applied]
    lines.append("")
    lines.append(f"{'valid' if report.is_valid else 'INVALID'} on {args.on}")
    if vacuous:
        # A pass that checked nothing is not a pass, and the exit status has to
        # say so or a CI gate built on this command is decoration.
        lines.append(
            "no AHB rules were applied to Prüfidentifikator(s) "
            + ", ".join(str(m.pruefidentifikator) for m in vacuous)
            + " — this interchange 'validated' unchecked"
        )
    _emit(args, payload, lines)
    return 0 if report.is_valid and not vacuous else 1


def _frist(args: argparse.Namespace) -> int:
    obligation = antwort_obligation(args.pid)
    if obligation is None:
        _emit(
            args,
            {"pruefidentifikator": args.pid, "obligation": None},
            [
                f"no published answer Frist for Prüfidentifikator {args.pid}. "
                f"That is unknown, not unbounded."
            ],
        )
        return 1
    window = (
        f"{obligation.clock_time} on the 1. Werktag"
        if obligation.clock_time
        else f"{obligation.werktage} Werktage"
    )
    payload: dict[str, Any] = {
        "pruefidentifikator": obligation.trigger_pid,
        "name": obligation.name,
        "family": obligation.family,
        "answered_by": obligation.answered_by,
        "shape": obligation.shape,
        "werktage": obligation.werktage,
        "clock_time": obligation.clock_time,
        "bestaetigung_pid": obligation.bestaetigung_pid,
        "ablehnung_pid": obligation.ablehnung_pid,
        "ebd": obligation.ebd,
        "source": obligation.source,
    }
    lines = [
        f"{obligation.trigger_pid}  {obligation.name}  [{obligation.family}]",
        f"  answered by  {obligation.answered_by} within {window} ({obligation.shape})",
        f"  answer PIDs  {obligation.bestaetigung_pid} bestätigt / "
        f"{obligation.ablehnung_pid} abgelehnt"
        + (f"  EBD {obligation.ebd}" if obligation.ebd else ""),
        f"  Fundstelle   {obligation.source}",
    ]
    if args.received:
        due = obligation.due_at(args.received)
        payload["received"] = args.received
        payload["due_at"] = due
        lines.append(f"  received {args.received} → due {due}")
    _emit(args, payload, lines)
    return 0


def _identify(args: argparse.Namespace) -> int:
    value = args.value
    payload: dict[str, Any] = {"value": value, "kinds": []}
    lines = []
    if malo_is_valid(value):
        payload["kinds"].append("marktlokation")
        lines.append("Marktlokations-ID — 11 digits, §8.1 check digit valid")
    if melo_is_valid(value):
        payload["kinds"].append("messlokation")
        lines.append("Messlokations-ID — 33 characters (no check digit is defined)")
    if mp_id_is_valid(value):
        schemes = mp_id_check_digit_schemes(value)
        payload["kinds"].append("marktpartner")
        payload["authority"] = mp_id_authority(value)
        payload["check_digit_schemes"] = schemes
        lines.append(
            f"Marktpartner-ID — 13 digits, issued by {mp_id_authority(value)}; "
            + (
                f"check digit valid under {', '.join(schemes)}"
                if schemes
                else "check digit valid under NEITHER procedure — every "
                "conformant counterparty would refuse it"
            )
        )
    if eic_is_valid(value):
        payload["kinds"].append("eic")
        payload["eic_type"] = eic_type_char(value)
        kind = {
            "X": "Party (e.g. a Bilanzkreis)",
            "Y": "Area (e.g. a Bilanzierungsgebiet)",
        }
        lines.append(
            f"EIC — object type {eic_type_char(value)}: "
            f"{kind.get(eic_type_char(value), 'see ENTSO-E')}"
        )
    if value.isdigit() and len(value) == 5:
        types = message_types_of(int(value))
        if types:
            payload["kinds"].append("pruefidentifikator")
            payload["message_types"] = types
            lines.append(f"Prüfidentifikator — carried by {', '.join(types)}")
    if not payload["kinds"]:
        lines.append(
            f"{value!r} is not a valid identifier of any family this build knows"
        )
    _emit(args, payload, lines)
    return 0 if payload["kinds"] else 1


def _pids(args: argparse.Namespace) -> int:
    codes = pruefidentifikatoren(args.message_type, args.on, args.sparte)
    release = release_for(args.message_type, args.on, args.sparte) if args.on else None
    _emit(
        args,
        {
            "message_type": args.message_type.upper(),
            "on": args.on,
            "sparte": args.sparte,
            "release": release,
            "pruefidentifikatoren": codes,
        },
        [
            f"{len(codes)} Prüfidentifikator(en) with AHB rules"
            + (f" on {args.on} (release {release})" if args.on else ""),
            " ".join(str(c) for c in codes) or "(none)",
        ],
    )
    return 0 if codes else 1


def _versions(args: argparse.Namespace) -> int:
    versions = format_versions()
    _emit(args, {"format_versions": versions}, versions)
    return 0


def _read(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


if __name__ == "__main__":  # pragma: no cover - module entry point
    raise SystemExit(main())
