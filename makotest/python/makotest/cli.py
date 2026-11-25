"""`makotest` on the command line.

A library you cannot point at a file from a shell is one only Python teams
adopt. The questions worth reaching for without writing a test include what is
wrong with this interchange, when is this answer due, is this identifier one a
counterparty would accept, and what is a counterparty entitled to answer with.

The domain commands use the same bindings as the Python assertions. The shadow
command compares independent local evidence for the selected traffic window.

    makotest validate inbound.edi --on 2026-04-01
    makotest frist 55001 --received 2026-03-02T09:00:00Z
    makotest id 9900357000004
    makotest pids UTILMD --on 2026-04-01 --sparte STROM
    makotest codes --pid 55001
    makotest shadow reconcile --help

Exit status is 0 for agreement, 1 for a disagreement or negative answer, and 2
when a determination cannot be made.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Any, Never

__all__ = ["main"]


class _InvalidShadowArguments(Exception):
    """A shadow usage error may contain a private path or identifier."""


class _ShadowParser(argparse.ArgumentParser):
    def error(self, message: str) -> Never:
        raise _InvalidShadowArguments


def main(argv: Sequence[str] | None = None) -> int:
    words = sys.argv[1:] if argv is None else list(argv)
    positional = (word for word in words if not word.startswith("-"))
    command = next(positional, None)
    parser = _parser(_ShadowParser if command == "shadow" else argparse.ArgumentParser)
    try:
        args = parser.parse_args(words)
    except _InvalidShadowArguments:
        action = next(positional, None)
        if action in ("invoice", "pair"):
            kind = "status" if action == "invoice" else "classification"
            print(
                json.dumps(
                    {"version": 1, kind: "unassessed", "reason": "invalid_arguments"}
                )
            )
            return 2
        if action == "rehearse":
            from .rehearsal import unassessed_report

            print(
                json.dumps(unassessed_report() | {"reason": "invalid_arguments"}),
                file=sys.stderr,
            )
            return 2
        from .shadow import unknown_report

        return _emit_shadow(unknown_report("invalid_arguments"))
    if args.command == "shadow":
        if args.action == "pair":
            from .shadow_pair import run

            return run(args)
        if args.action == "invoice":
            from .invoice_review import run

            return run(args)
        if args.action == "rehearse":
            from .rehearsal import run

            return run(args)
        return _shadow_reconcile(args)
    handler = {
        "validate": _validate,
        "frist": _frist,
        "id": _identify,
        "pids": _pids,
        "codes": _codes,
        "versions": _versions,
    }[args.command]
    # `--json` is accepted on either side of the subcommand; argparse cannot
    # share one flag across both levels, so the two dests are merged here.
    args.json = bool(getattr(args, "json", False) or getattr(args, "json_global", False))
    try:
        return handler(args)
    except (ValueError, OSError) as exc:
        print(f"makotest: {exc}", file=sys.stderr)
        return 2


def _parser(
    parser_type: type[argparse.ArgumentParser] = argparse.ArgumentParser,
) -> argparse.ArgumentParser:
    parser = parser_type(
        prog="makotest",
        description=(
            "Validate EDIFACT, resolve answer Fristen and inspect BDEW "
            "identifiers, or reconcile local shadow traffic against a source journal."
        ),
    )
    parser.add_argument(
        "--json",
        dest="json_global",
        action="store_true",
        help="emit machine-readable JSON",
    )
    #: `--json` again on every subcommand, so it works on either side of it.
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument(
        "--json",
        action="store_true",
        help="emit machine-readable JSON",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    validate = sub.add_parser(
        "validate",
        parents=[common],
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

    frist = sub.add_parser(
        "frist", parents=[common], help="the published answer Frist for a PID"
    )
    frist.add_argument("pid", type=int, help="the inbound Prüfidentifikator")
    frist.add_argument(
        "--received",
        metavar="RFC3339",
        help="when the request arrived; prints the instant the answer is due",
    )

    identify = sub.add_parser("id", parents=[common], help="classify a BDEW identifier")
    identify.add_argument("value", help="a MaLo, MeLo, Marktpartner-ID or EIC")

    pids = sub.add_parser(
        "pids",
        parents=[common],
        help="Prüfidentifikatoren the compiled profiles validate",
    )
    pids.add_argument("message_type", help="e.g. UTILMD")
    pids.add_argument("--on", metavar="ISO_DATE", help="restrict to that date's profile")
    pids.add_argument("--sparte", choices=["STROM", "GAS"], help="UTILMD track")

    codes = sub.add_parser(
        "codes",
        parents=[common],
        help="the Antwortcodes an Entscheidungsbaum publishes",
    )
    target = codes.add_mutually_exclusive_group(required=True)
    target.add_argument("--ebd", metavar="E_XXXX", help="the Entscheidungsbaum")
    target.add_argument(
        "--pid",
        type=int,
        metavar="N",
        help="an inbound Prüfidentifikator, resolved to the tree its Frist names",
    )

    sub.add_parser(
        "versions", parents=[common], help="every compiled BDEW format version"
    )
    shadow = sub.add_parser(
        "shadow", parents=[common], help="account for the input to a shadow comparison"
    )
    actions = shadow.add_subparsers(dest="action", required=True)
    pair = actions.add_parser(
        "pair", help="compare observed answers in two closed wire captures"
    )
    pair.add_argument(
        "--incumbent", required=True, type=Path, help="incumbent capture directory"
    )
    pair.add_argument(
        "--shadow", required=True, type=Path, help="shadow capture directory"
    )
    pair.add_argument("--case", required=True, type=Path, help="private NB case file")
    invoice = actions.add_parser(
        "invoice", help="compare an ESA invoice and its REMADV locally"
    )
    invoice.add_argument(
        "--invoice", required=True, type=Path, help="captured BO4E Rechnung"
    )
    invoice.add_argument(
        "--reference",
        required=True,
        type=Path,
        help="independent price, quantity, facts and expected REMADV",
    )
    reconcile = actions.add_parser(
        "reconcile",
        parents=[common],
        help="compare a closed source journal and a local mirror inventory",
        description=(
            "Compare per-message occurrences and bytes for a closed capture window. "
            "Write one versioned JSON report to stdout. All inputs stay local."
        ),
        epilog=(
            "Example (from the capture directory)\n"
            "  makotest shadow reconcile --journal journal.json "
            "--inventory inventory.json --mirror-root mirror\n\n"
            "Exit status\n"
            "  0 complete or confirmed_empty, 1 mismatch, 2 unknown or invalid input.\n\n"
            "Recovery\n"
            "  Missing or unlisted file? Reacquire the frozen mirror and its inventory"
            " together.\n"
            "  Scope mismatch or coverage gap? Obtain closed batches for the same"
            " cohort,\n"
            "  capture layer, time window and required directions on both sides.\n"
            "  A confirmed_empty interval accounts for time and adds zero inbound"
            " comparison\n"
            "  candidates. Check eligible_for_comparison before comparing outcomes.\n"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    rehearse = actions.add_parser(
        "rehearse",
        help="compare one offline Strom NB refusal with an independent expectation",
        description="Recheck the capture and compare one validated NB reply.",
        epilog=(
            "From the private capture directory:\n"
            "makotest shadow rehearse --journal journal.json --inventory inventory.json "
            "--mirror-root mirror --coverage reports/complete.json --case case.json "
            "--expected expected.json --emit summary\n\n"
            "Exit 0: matched or no candidates. Exit 1: paired difference.\n"
            "Exit 2: unavailable evidence. The answer stream contains private EDIFACT."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    rehearse.add_argument(
        "--coverage", required=True, type=Path, help="saved reconciliation report"
    )
    rehearse.add_argument(
        "--case", required=True, type=Path, help="local occurrence and NB facts"
    )
    rehearse.add_argument(
        "--expected", required=True, type=Path, help="independent expected answer"
    )
    rehearse.add_argument(
        "--makod-workspace",
        type=Path,
        help="compare a makod capture in a private occurrence directory",
    )
    rehearse.add_argument(
        "--also",
        nargs="+",
        type=Path,
        action="append",
        default=[],
        metavar="FILE",
        help="add a case, expectation and optional receipt to the review",
    )
    rehearse.add_argument(
        "--observation",
        type=Path,
        help="positive AS4 receipts for the request and incumbent answer",
    )
    rehearse.add_argument(
        "--review-nb", help="expected recipient MP-ID of every inbound review row"
    )
    rehearse.add_argument(
        "--emit",
        choices=("report", "summary", "answer", "daemon-input", "review"),
        default="report",
        help="write the report, summary, private answer, makod input or NB review file",
    )
    for action in (reconcile, rehearse):
        action.add_argument(
            "--journal",
            required=True,
            type=Path,
            metavar="FILE",
            help="version 1 closed journal export from the source system",
        )
        action.add_argument(
            "--inventory",
            required=True,
            type=Path,
            metavar="FILE",
            help="version 1 inventory of independently captured occurrences",
        )
        action.add_argument(
            "--mirror-root",
            required=True,
            type=Path,
            metavar="DIR",
            help="stable read-only directory containing only the captured payload files",
        )
    return parser


def _shadow_reconcile(args: argparse.Namespace) -> int:
    from .shadow import reconcile, unknown_report
    from .shadow_io import ShadowInputError, Snapshot

    try:
        with Snapshot(args.mirror_root) as snapshot:
            journal, inventory = snapshot.read_manifests(args.journal, args.inventory)
            report = reconcile(journal, inventory, snapshot)
    except ShadowInputError as exc:
        report = unknown_report(exc.code)
    except OSError:
        report = unknown_report("input_unavailable")
    except (ValueError, RecursionError):
        report = unknown_report("invalid_manifest")
    return _emit_shadow(report)


def _emit_shadow(report: dict[str, Any]) -> int:
    status = {"complete": 0, "confirmed_empty": 0, "mismatch": 1, "unknown": 2}
    print(json.dumps(report, sort_keys=True, separators=(",", ":"), allow_nan=False))
    return status[report["status"]]


def _emit(args: argparse.Namespace, payload: dict[str, Any], lines: list[str]) -> None:
    if args.json:
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        print("\n".join(lines))


def _validate(args: argparse.Namespace) -> int:
    from ._native import validate_edifact

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
    from ._native import antwort_obligation

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
    # Rendered by the binding, from the shape and the Werktag count together —
    # „15:00 Uhr am ÜT" and „15:00 Uhr des 1. WT nach dem ÜT" share a clock time
    # and are a day apart.
    window = obligation.window
    payload: dict[str, Any] = {
        "pruefidentifikator": obligation.trigger_pid,
        "name": obligation.name,
        "family": obligation.family,
        "answered_by": obligation.answered_by,
        "shape": obligation.shape,
        "werktage": obligation.werktage,
        "clock_time": obligation.clock_time,
        "window": window,
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
    from ._native import (
        eic_is_valid,
        eic_type_char,
        malo_is_valid,
        melo_is_valid,
        message_types_of,
        mp_id_authority,
        mp_id_check_digit_schemes,
        mp_id_is_valid,
    )

    value = args.value
    payload: dict[str, Any] = {"value": value, "kinds": []}
    lines = []
    if malo_is_valid(value):
        payload["kinds"].append("marktlokation")
        lines.append("Marktlokations-ID — 11 digits, Kap. 8.1 check digit valid")
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
    from ._native import pruefidentifikatoren, release_for

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


def _codes(args: argparse.Namespace) -> int:
    from ._native import antwort_codes, antwort_codes_for_pid, antwort_obligation

    if args.ebd:
        tree, codes = args.ebd, antwort_codes(args.ebd)
    else:
        codes = antwort_codes_for_pid(args.pid)
        obligation = antwort_obligation(args.pid)
        tree = obligation.ebd if obligation else None
        if not codes:
            _emit(
                args,
                {"pruefidentifikator": args.pid, "ebd": tree, "codes": []},
                [
                    f"no Entscheidungsbaum with a published Codeliste for "
                    f"Prüfidentifikator {args.pid}."
                ],
            )
            return 1
    _emit(
        args,
        {
            "ebd": tree,
            "codes": [
                {
                    "code": c.code,
                    "cluster": c.cluster,
                    "ist_zustimmung": c.ist_zustimmung,
                    "wire_codeliste": c.wire_codeliste,
                    "bedeutung": c.bedeutung,
                    "braucht_bemerkung": c.braucht_bemerkung,
                }
                for c in codes
            ],
        },
        [f"{tree} — {len(codes)} Antwortcode(s)"]
        + [
            f"  {c.code:5} {c.cluster:30} {c.bedeutung}"
            + ("  [braucht FTX+ACB]" if c.braucht_bemerkung else "")
            for c in codes
        ]
        + [
            "",
            # DE 1131 carries the Codeliste, which is the EBD number only for
            # the GPKE and GeLi Gas trees.
            "  SG4 STS+E01 DE 1131 carries "
            + ", ".join(sorted({c.wire_codeliste or "(nothing)" for c in codes})),
        ],
    )
    return 0


def _versions(args: argparse.Namespace) -> int:
    from ._native import format_versions

    versions = format_versions()
    _emit(args, {"format_versions": versions}, versions)
    return 0


def _read(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


if __name__ == "__main__":  # pragma: no cover - module entry point
    raise SystemExit(main())
