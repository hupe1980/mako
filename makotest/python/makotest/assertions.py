"""Domain assertions.

Registered for pytest assertion rewriting in `makotest/__init__.py`, so a failure
prints the actual mismatch rather than `assert False`.

Every assertion here reads an **observable contract** — a REST response, a
CloudEvent, a rendered EDIFACT interchange — never a platform's internal
database. That is what keeps the toolkit portable across implementations.
"""

from __future__ import annotations

from ._native import ValidationReport, validate_edifact

__all__ = [
    "assert_bo4e_generation_matches",
    "assert_edifact_valid",
    "assert_positions_sum_to_total",
    "assert_rule_fires",
]


def assert_edifact_valid(raw: bytes, *, on: str | None = None) -> ValidationReport:
    """Assert an interchange passes MIG + AHB + semantic validation.

    `on` selects the BDEW format version in force (ISO 8601). Pass the date the
    message would really be sent — a message valid under FV2025-10-01 can be
    invalid under FV2026-10-01, and defaulting to "today" silently hides that.
    """
    report = validate_edifact(raw, on)
    if not report.is_valid:
        errors = [f for f in report.findings if f.severity in ("error", "critical")]
        detail = "\n".join(f"  [{f.rule_id or '-'}] {f.segment or '-'}: {f.message}" for f in errors)
        raise AssertionError(
            f"EDIFACT interchange is invalid "
            f"(pid={report.pruefidentifikator}, type={report.message_type}):\n{detail}"
        )
    return report


def assert_rule_fires(raw: bytes, rule_prefix: str, *, on: str | None = None) -> None:
    """Assert a specific validation rule rejects the interchange.

    The counterpart to `assert_edifact_valid` — proves a *negative* case really
    is caught by the rule you think catches it, rather than by some unrelated
    error further up the stack.
    """
    report = validate_edifact(raw, on)
    hits = report.by_rule(rule_prefix)
    if not hits:
        seen = sorted({f.rule_id or "-" for f in report.findings})
        raise AssertionError(
            f"expected rule {rule_prefix!r} to fire, but it did not. "
            f"Rules that did fire: {seen or '(none — the message validated)'}"
        )


def assert_positions_sum_to_total(invoice: dict, *, tolerance_eur: float = 0.005) -> None:
    """Assert a BO4E `Rechnung`'s positions reconcile with its stated net total.

    Tolerance defaults to half a cent: invoice positions are rounded per line,
    so an exact equality assertion produces false failures on legitimate output.
    """
    positions = invoice.get("rechnungspositionen") or []
    total = _money(invoice.get("gesamtnetto"))
    summed = sum(_money(p.get("teilsummeNetto")) for p in positions)
    if abs(summed - total) > tolerance_eur:
        raise AssertionError(
            f"invoice positions sum to {summed:.4f} EUR but gesamtnetto is "
            f"{total:.4f} EUR (delta {summed - total:+.4f}, tolerance "
            f"{tolerance_eur})"
        )


def assert_bo4e_generation_matches(platform_generation: str) -> None:
    """Assert the platform's BO4E generation matches the one makotest builds.

    Testing v202607 objects against a platform on a different generation
    produces passes that mean nothing, so this is worth asserting once per
    session rather than debugging later.
    """
    from . import BO4E_GENERATION

    if not str(platform_generation).startswith(BO4E_GENERATION):
        raise AssertionError(
            f"BO4E generation mismatch: makotest builds {BO4E_GENERATION}, "
            f"platform advertises {platform_generation}. Assertions over "
            f"business objects would be meaningless."
        )


def _money(value: object) -> float:
    """Coerce a BO4E money field (scalar or `{wert, waehrung}` COM) to float."""
    if value is None:
        return 0.0
    if isinstance(value, dict):
        return float(value.get("wert") or 0.0)
    return float(value)  # type: ignore[arg-type]
