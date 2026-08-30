"""Domain assertions.

Registered for pytest assertion rewriting in `makotest/__init__.py`, so a failure
prints the actual mismatch rather than `assert False`.

Every assertion here reads an **observable contract** — a REST response, a
CloudEvent, a rendered EDIFACT interchange — never a platform's internal
database. That is what keeps the toolkit portable across implementations.

The expectations are computed with the platform's own arithmetic and its own
tables, so an assertion measures the same thing the platform registered rather
than a re-derivation that has to be kept in step by hand.

Two failures, two exceptions. `AssertionError` means the **system under test**
is wrong. `ValueError` means the **test** is — a Prüfidentifikator with no
published Frist, an event pattern the catalog cannot satisfy, two mutually
exclusive arguments. The distinction matters here more than usual: an assertion
that cannot fail is this toolkit's central failure mode, and it should not look
like a system defect.
"""

from __future__ import annotations

import re
from collections.abc import Iterable, Mapping
from datetime import datetime
from decimal import Decimal, InvalidOperation
from typing import Any

from ._native import (
    ValidationReport,
    ablehnung_pid,
    antwort_code,
    antwort_codes,
    antwort_obligation,
    bestaetigung_pid,
    bo4e_schema_version,
    cloudevent_json_members,
    deadline_at_werktage,
    event_matches,
    event_type_exists,
    event_types_matching,
    is_valid_extension_key,
    parse_cloudevent_time,
    validate_edifact,
)

__all__ = [
    "assert_answer_pid",
    "assert_antwort_code",
    "assert_bo4e_generation_matches",
    "assert_cloudevent",
    "assert_deadline_is",
    "assert_edifact_valid",
    "assert_event_emitted",
    "assert_frist_met",
    "assert_invoice_reconciles",
    "assert_no_event_emitted",
    "assert_rule_fires",
    "assert_rules_applied",
    "find_events",
]


# ── EDIFACT ───────────────────────────────────────────────────────────────────


def assert_edifact_valid(raw: bytes, *, on: str) -> ValidationReport:
    """Assert an interchange passes MIG + AHB + semantic validation.

    `on` is the date the message would really be sent, and selects the BDEW
    format version in force. It is required: a message valid under FV2025-10-01
    can be invalid under FV2026-10-01, and a default of "today" would make the
    same test mean different things in different months.

    Also asserts the AHB rules were **applied**. A Prüfidentifikator the profile
    set has no rules for validates vacuously — `is_valid` comes back true having
    checked nothing — so a bare validity assertion over one proves nothing.
    """
    __tracebackhide__ = True
    report = validate_edifact(raw, on)
    if not report.is_valid:
        detail = "\n".join(
            f"  [{f.rule_id or '-'}] {f.position or '-'}: {f.message}"
            + (f"\n      hint: {f.suggestion}" if f.suggestion else "")
            for f in report.errors
        )
        raise AssertionError(
            f"EDIFACT interchange is invalid on {on}:\n{detail or '  (no error detail)'}"
        )
    _assert_rules_applied(report, on)
    return report


def assert_rules_applied(raw: bytes, *, on: str) -> ValidationReport:
    """Assert every message in `raw` was checked against real AHB rules.

    Use this on a message you expect to be *invalid*, where
    `assert_edifact_valid` is not the assertion you want but a vacuous pass is
    still the failure mode you care about.
    """
    __tracebackhide__ = True
    report = validate_edifact(raw, on)
    _assert_rules_applied(report, on)
    return report


def _assert_rules_applied(report: ValidationReport, on: str) -> None:
    __tracebackhide__ = True
    unchecked = [m for m in report.messages if not m.rules_applied]
    if unchecked:
        pids = ", ".join(str(m.pruefidentifikator) for m in unchecked)
        raise AssertionError(
            f"no AHB rules were applied on {on} to Prüfidentifikator(s) {pids}. "
            f"The message 'validated' because the profile set has no rules for "
            f"that code, not because it was correct — this test cannot fail as "
            f"written. Check the PID, or the format version you validated on."
        )


def assert_rule_fires(raw: bytes, rule_prefix: str, *, on: str) -> None:
    """Assert a specific validation rule rejects the interchange.

    The counterpart to `assert_edifact_valid` — proves a *negative* case really
    is caught by the rule you think catches it, rather than by some unrelated
    error further up the stack.
    """
    __tracebackhide__ = True
    report = validate_edifact(raw, on)
    if not report.by_rule(rule_prefix):
        seen = sorted({f.rule_id or "-" for f in report.findings})
        raise AssertionError(
            f"expected rule {rule_prefix!r} to fire on {on}, but it did not. "
            f"Rules that did fire: {seen or '(none — the message validated)'}"
        )


def assert_answer_pid(actual: int, *, anfrage: int, accepted: bool) -> None:
    """Assert an answer carries the Prüfidentifikator the AHB assigns.

    The mapping is not `Anfrage + 1`: GPKE 55077 rejects with 55080 because
    55079 is unassigned, and GeLi Gas 44020 can be confirmed but never rejected.
    Resolving it from the shared table is the only way a test and the platform
    agree on what a conformant answer looks like.
    """
    __tracebackhide__ = True
    expected = bestaetigung_pid(anfrage) if accepted else ablehnung_pid(anfrage)
    kind = "Bestätigung" if accepted else "Ablehnung"
    if expected is None:
        raise ValueError(
            f"the AHB assigns no {kind} to Prüfidentifikator {anfrage}. Either it "
            f"is not a request PID, or this process has no such answer — "
            f"44020 is confirmable but never rejectable."
        )
    if actual != expected:
        raise AssertionError(
            f"wrong answer Prüfidentifikator for the {kind} of {anfrage}: "
            f"expected {expected}, got {actual}."
        )


def assert_antwort_code(actual: str, *, ebd: str, accepted: bool | None = None) -> None:
    """Assert an answer states an Antwortcode its Entscheidungsbaum publishes.

    The answer PID says a counterparty refused; the Antwortcode in `SG4 STS+E01`
    says *why*, and it is only meaningful inside the tree that publishes it —
    `A02` is „Vorlauffrist nicht eingehalten" in `E_0607` and „Marktlokation
    nimmt nicht an der Marktkommunikation teil" in `E_0622`. `ebd` names the
    tree, and a code that tree has no leaf for is a defect however plausible it
    looks.

    `accepted` additionally pins the code's **Cluster** — whether the BDEW
    classes it as a Zustimmung or an Ablehnung. That is a property of the code,
    not of the message carrying it, so this catches an answer that refuses on a
    Bestätigungs-PID. Leave it `None` to check membership only.

    Raises `ValueError` when `accepted` is asked of a tree off the agreement
    axis: `E_0595` states whether a Stammdatenänderung follows, and neither of
    its clusters is a Zustimmung, so reading one as a refusal would invert the
    answer.
    """
    __tracebackhide__ = True
    resolved = antwort_code(ebd, actual)
    if resolved is None:
        published = ", ".join(c.code for c in antwort_codes(ebd))
        raise AssertionError(
            f"{actual!r} is not an Antwortcode {ebd} publishes. A code means "
            f"nothing outside its tree, and this one has no leaf here. "
            f"{ebd} publishes: {published}."
        )
    if accepted is None:
        return
    if resolved.ist_zustimmung is None:
        raise ValueError(
            f"{ebd} is off the agreement axis — its clusters state whether data "
            f"follows, not whether the request was granted, so {actual!r} is "
            f"neither an acceptance nor a refusal. Drop accepted=."
        )
    if resolved.ist_zustimmung != accepted:
        wanted = "Zustimmung" if accepted else "Ablehnung"
        raise AssertionError(
            f"{ebd} classes {actual!r} as {resolved.cluster}, not a {wanted}: "
            f"„{resolved.bedeutung}"
        )


# ── Fristen ───────────────────────────────────────────────────────────────────


def assert_deadline_is(
    actual: str,
    *,
    received: str,
    pid: int | None = None,
    werktage: int | None = None,
) -> None:
    """Assert a deadline is exactly the instant the Festlegung buys.

    Pass `pid` — the **inbound** Prüfidentifikator that started the clock — and
    the expectation comes from the platform's own answer-Frist table, in the
    shape that process really uses. Pass `werktage` only for a window with no
    table entry, where you are asserting the WiM cut-off shape explicitly.

    Both `actual` and `received` are RFC 3339. Write it this way rather than
    comparing dates: a day-granular comparison passes on a deadline that is
    hours wrong, and no single shape fits every family — GPKE answers are due at
    a **clock time on the n-th Werktag after the Übertragungstag** or on the
    **ÜT itself**, GeLi Gas at the **end of the n-th Werktag**, WiM at **17:00 on
    the n-th**.

    The comparison is over **instants**, not over the strings. A Frist carries
    the Europe/Berlin offset and a platform commonly reports it in UTC:
    `"…T10:00:00Z"` and `"…T11:00:00+01:00"` are the same moment, and failing
    one of them would report a correct deadline as a defect.
    """
    __tracebackhide__ = True
    if (pid is None) == (werktage is None):
        raise ValueError(
            "pass exactly one of pid= (the published Frist for that process) or "
            "werktage= (an explicit WiM-shaped cut-off)"
        )

    if pid is not None:
        obligation = antwort_obligation(pid)
        if obligation is None:
            raise ValueError(
                f"no published answer Frist for Prüfidentifikator {pid}. That is "
                f"unknown, not unbounded — if the platform registered a deadline "
                f"anyway it is an operating convention, and asserting it against "
                f"a Festlegung would misattribute it."
            )
        expected = obligation.due_at(received)
        basis = f"{obligation.name} ({obligation.family}) — {obligation.source}"
    else:
        assert werktage is not None  # narrowed by the guard above
        expected = deadline_at_werktage(received, werktage)
        basis = f"{werktage} Werktage to the 17:00 Europe/Berlin cut-off"

    if _instant(actual) != _instant(expected):
        raise AssertionError(
            f"deadline mismatch\n"
            f"  received: {received}\n"
            f"  basis:    {basis}\n"
            f"  expected: {expected}\n"
            f"  actual:   {actual}\n"
            f"Weekends, any holiday observed in a German state, and 24./31.12. "
            f"do not count as Werktage."
        )


def assert_frist_met(pid: int, *, received: str, answered_at: str) -> None:
    """Assert an answer was sent inside the published window for `pid`.

    `received` is when the request arrived, `answered_at` when the answer went
    out; both RFC 3339. The window comes from the platform's table, so this
    measures the obligation rather than a re-derivation of it.

    The comparison is over **instants**, not over the strings: a deadline
    carries the Europe/Berlin offset and an answer is often timestamped in UTC,
    and `"…T10:30:00Z"` sorts before `"…T11:00:00+01:00"` while being half an
    hour later.
    """
    __tracebackhide__ = True
    obligation = antwort_obligation(pid)
    if obligation is None:
        raise ValueError(
            f"no published answer Frist for Prüfidentifikator {pid} — there is "
            f"no window to have met."
        )
    due = obligation.due_at(received)
    if _instant(answered_at) > _instant(due):
        raise AssertionError(
            f"answer to {pid} ({obligation.name}) was late\n"
            f"  received:  {received}\n"
            f"  due:       {due}\n"
            f"  answered:  {answered_at}\n"
            f"  Fundstelle: {obligation.source}"
        )


# ── CloudEvents ───────────────────────────────────────────────────────────────


def assert_cloudevent(
    event: Mapping[str, Any],
    *,
    type: str | None = None,
    subject: str | None = None,
) -> None:
    """Assert one emitted event is a conformant CloudEvent of the expected kind.

    Three checks a hand-written comparison usually skips:

    * the `type` is in the platform's **catalog** — a typo or a retired name
      would otherwise pass forever as "the platform never emitted this";
    * the envelope satisfies CloudEvents 1.0 (the four required context
      attributes, `specversion == "1.0"`, an RFC 3339 `time`);
    * extension keys satisfy §3.3 — a key colliding with a context attribute or
      with `data` serialises twice and every receiver rejects the event.
      `data_base64` is the one JSON-format member that is neither a context
      attribute nor a legal extension name, and it is accepted; carrying it
      *and* `data` is not.

    `time` is **optional** in CloudEvents 1.0 but demanded here: an event stream
    a platform asks a market partner to reconcile against a Frist has to say
    when the event happened, and an absent `time` is a defect this toolkit
    exists to name rather than a conformance question.
    """
    __tracebackhide__ = True
    missing = [a for a in ("specversion", "id", "source", "type") if a not in event]
    if missing:
        raise AssertionError(
            f"not a CloudEvent: required attribute(s) {missing} absent. Got keys "
            f"{sorted(event)}."
        )
    if "time" not in event:
        raise AssertionError(
            "the event carries no `time`. CloudEvents 1.0 makes it optional, but "
            "an event that cannot be placed on the clock cannot be reconciled "
            "against a Frist, which is what this stream is asserted against. "
            f"Got keys {sorted(event)}."
        )
    if event["specversion"] != "1.0":
        raise AssertionError(
            f"CloudEvents specversion is {event['specversion']!r}, expected '1.0'"
        )
    parse_cloudevent_time(str(event["time"]))

    if "data" in event and "data_base64" in event:
        raise AssertionError(
            "the event carries both `data` and `data_base64`. §3.1 of the JSON "
            "format makes them mutually exclusive — an event with two payloads "
            "has no rule for which one a receiver reads."
        )

    actual_type = str(event["type"])
    if not event_type_exists(actual_type):
        raise AssertionError(
            f"{actual_type!r} is not a type the platform declares. Either the "
            f"emitter invented it, or it names a retired one — a renamed type "
            f"still 'matches' nothing forever."
        )

    members = set(cloudevent_json_members())
    bad_keys = [k for k in event if k not in members and not is_valid_extension_key(k)]
    if bad_keys:
        raise AssertionError(
            f"illegal CloudEvents extension attribute(s) {sorted(bad_keys)}: §3.3 "
            f"allows lowercase letters and digits only, and a key colliding with "
            f"a core attribute is emitted twice."
        )

    if type is not None:
        if not event_type_exists(type):
            raise ValueError(
                f"the expected type {type!r} is not in the platform's catalog — "
                f"this assertion could never have passed."
            )
        if actual_type != type:
            raise AssertionError(f"expected event type {type!r}, got {actual_type!r}")

    if subject is not None and event.get("subject") != subject:
        raise AssertionError(
            f"expected subject {subject!r}, got {event.get('subject')!r}"
        )


def find_events(
    events: Iterable[Mapping[str, Any]],
    pattern: str,
    *,
    subject: str | None = None,
) -> list[Mapping[str, Any]]:
    """Every event whose type a subscription `pattern` would deliver.

    The matcher is the platform's own — `*` matches any sequence, `?` exactly
    one character — so a test filters the way the platform's routing does rather
    than by `startswith`.

    A pattern the catalog can never satisfy raises: it would silently select
    nothing, and "no event matched" is what the assertion is trying to
    distinguish a real absence from.
    """
    __tracebackhide__ = True
    if not event_types_matching(pattern):
        raise ValueError(
            f"no declared event type matches {pattern!r}. A subscription on it "
            f"would be dead, and this filter can only ever return nothing."
        )
    return [
        e
        for e in events
        if event_matches(pattern, str(e.get("type", "")))
        and (subject is None or e.get("subject") == subject)
    ]


def assert_event_emitted(
    events: Iterable[Mapping[str, Any]],
    pattern: str,
    *,
    subject: str | None = None,
) -> Mapping[str, Any]:
    """Assert at least one event matching `pattern` was emitted, and return it.

    Returns the first match so the caller can go on to assert over its `data`.
    """
    __tracebackhide__ = True
    collected = list(events)
    hits = find_events(collected, pattern, subject=subject)
    if not hits:
        seen = sorted({str(e.get("type", "?")) for e in collected})
        raise AssertionError(
            f"no event matching {pattern!r}"
            + (f" with subject {subject!r}" if subject else "")
            + f" was emitted. Types seen: {seen or '(none)'}."
        )
    assert_cloudevent(hits[0], subject=subject)
    return hits[0]


def assert_no_event_emitted(
    events: Iterable[Mapping[str, Any]],
    pattern: str,
    *,
    subject: str | None = None,
) -> None:
    """Assert nothing matching `pattern` was emitted.

    Worth having as its own helper because the naive form — filtering by a
    literal type string — passes for a type that no longer exists. `find_events`
    refuses a pattern the catalog cannot satisfy, so this cannot pass vacuously.
    """
    __tracebackhide__ = True
    hits = find_events(list(events), pattern, subject=subject)
    if hits:
        raise AssertionError(
            f"expected no event matching {pattern!r}"
            + (f" with subject {subject!r}" if subject else "")
            + f", but {len(hits)} was/were emitted: "
            + ", ".join(str(e.get("type")) for e in hits[:5])
        )


# ── Business objects ──────────────────────────────────────────────────────────


def assert_invoice_reconciles(
    invoice: Mapping[str, Any], *, tolerance_eur: str = "0.01"
) -> None:
    """Assert every stated total of a BO4E `Rechnung` agrees with the others.

    Four identities, each checked only when both of its sides are present, so a
    partial invoice is asserted for what it does state rather than failing on
    what it omits:

    | Identity | Reads |
    |---|---|
    | `Σ teilsummeNetto` = `gesamtnetto` | the positions add up |
    | `Σ teilsummeSteuer.steuerwert` = `gesamtsteuer` | the VAT lines add up |
    | `gesamtnetto + gesamtsteuer` = `gesamtbrutto` | net and gross agree |
    | `gesamtbrutto − vorausgezahlt − rabattBrutto` = `zuZahlen` | the amount demanded |

    Checking only the first is the trap worth naming: an invoice whose positions
    add up correctly and whose `zuZahlen` is wrong is exactly the defect that
    reaches a customer, because the positions are what a reviewer reads and
    `zuZahlen` is what gets collected.

    Money is compared as `Decimal`, never `float`: a cent is not representable in
    binary floating point, and an invoice assertion that drifts in the last place
    is worse than no assertion. `tolerance_eur` defaults to one cent — each
    total is independently rounded, so two of them can legitimately differ by the
    rounding of the last place. Pass `"0"` to demand exact agreement.

    An invoice that states none of the four identities raises `ValueError`: with
    nothing to reconcile there is nothing this assertion can fail on, and a
    silent pass over an empty or misspelled mapping is the vacuous assertion
    this toolkit exists to prevent.
    """
    __tracebackhide__ = True
    tolerance = Decimal(tolerance_eur)
    positions = invoice.get("rechnungspositionen") or []

    checks: list[tuple[str, Decimal, Decimal, str]] = []
    if positions and "gesamtnetto" in invoice:
        checks.append(
            (
                "Σ teilsummeNetto = gesamtnetto",
                sum((_money(p.get("teilsummeNetto")) for p in positions), Decimal(0)),
                _money(invoice.get("gesamtnetto")),
                f"across {len(positions)} position(s)",
            )
        )
    if positions and "gesamtsteuer" in invoice:
        checks.append(
            (
                "Σ teilsummeSteuer.steuerwert = gesamtsteuer",
                sum(
                    (
                        _money((p.get("teilsummeSteuer") or {}).get("steuerwert"))
                        for p in positions
                    ),
                    Decimal(0),
                ),
                _money(invoice.get("gesamtsteuer")),
                f"across {len(positions)} position(s)",
            )
        )
    if "gesamtbrutto" in invoice and "gesamtnetto" in invoice:
        checks.append(
            (
                "gesamtnetto + gesamtsteuer = gesamtbrutto",
                _money(invoice.get("gesamtnetto")) + _money(invoice.get("gesamtsteuer")),
                _money(invoice.get("gesamtbrutto")),
                "",
            )
        )
    if "zuZahlen" in invoice and "gesamtbrutto" in invoice:
        checks.append(
            (
                "gesamtbrutto − vorausgezahlt − rabattBrutto = zuZahlen",
                _money(invoice.get("gesamtbrutto"))
                - _money(invoice.get("vorausgezahlt"))
                - _money(invoice.get("rabattBrutto")),
                _money(invoice.get("zuZahlen")),
                "",
            )
        )

    if not checks:
        raise ValueError(
            "this Rechnung states none of the four identities, so there is "
            "nothing to reconcile and this assertion could not have failed. "
            "A position sum needs `rechnungspositionen` alongside `gesamtnetto` "
            "or `gesamtsteuer`; the totals need `gesamtbrutto` alongside "
            f"`gesamtnetto` or `zuZahlen`. Got keys {sorted(invoice)}."
        )

    failures = [
        f"  {identity}: {computed} vs {stated} (delta {computed - stated:+})"
        + (f" — {note}" if note else "")
        for identity, computed, stated, note in checks
        if abs(computed - stated) > tolerance
    ]
    if failures:
        raise AssertionError(
            f"the Rechnung does not reconcile (tolerance {tolerance_eur} EUR):\n"
            + "\n".join(failures)
        )


def assert_bo4e_generation_matches(platform_generation: str) -> None:
    """Assert the platform's BO4E generation matches the bundled object model.

    The expected value is asked of the linked `rubo4e` rather than written down
    here, so it cannot drift from the crates the wheel bundles. Testing one
    generation's objects against a platform on another produces passes that mean
    nothing, so this is worth asserting once per session rather than debugging
    later.
    """
    __tracebackhide__ = True
    expected = bo4e_schema_version()
    if not _same_generation(str(platform_generation), expected):
        raise AssertionError(
            f"BO4E generation mismatch: makotest bundles {expected}, platform "
            f"advertises {platform_generation}. Assertions over business objects "
            f"would be meaningless."
        )


def _same_generation(actual: str, expected: str) -> bool:
    """Compare the generation only — `v202607.0.0` and `202607` are the same."""
    resolved = _generation(actual)
    return resolved is not None and resolved == _generation(expected)


def _generation(value: str) -> str | None:
    """The `YYYYMM` generation inside a BO4E version string, or `None`.

    A generation is a six-digit year-month, and it is the only part that has to
    match: `202607.1.0`, `v202607.0.0` and `BO4E-202607` all name the same one.
    Collapsing every digit in the string and taking the first six gets that
    wrong the moment anything numeric precedes it — `BO4E-202607` would read as
    `420260` — and the failure is a *mismatch* report on a version that matched.

    Returns `None` when nothing in the string looks like a generation, which is
    a different answer from a generation that disagrees.
    """
    for match in re.finditer(r"(?<!\d)(\d{6})(?!\d)", value):
        year, month = int(match.group(1)[:4]), int(match.group(1)[4:])
        if 2000 <= year <= 2099 and 1 <= month <= 12:
            return match.group(1)
    return None


def _instant(value: str) -> datetime:
    """Parse an RFC 3339 timestamp into an aware `datetime`."""
    parsed = datetime.fromisoformat(value)
    if parsed.tzinfo is None:
        raise AssertionError(
            f"{value!r} carries no UTC offset. A Frist is an instant, and a "
            f"naive timestamp cannot be compared with one."
        )
    return parsed


def _money(value: object) -> Decimal:
    """Coerce a BO4E money field (scalar or `{wert, waehrung}` COM) to Decimal."""
    if value is None:
        return Decimal(0)
    if isinstance(value, dict):
        value = value.get("wert")
        if value is None:
            return Decimal(0)
    try:
        # `str()` first: Decimal(float) would inherit the float's binary error.
        return Decimal(str(value))
    except (InvalidOperation, TypeError) as exc:
        raise AssertionError(f"not a money value: {value!r}") from exc
