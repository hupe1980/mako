"""Hypothesis strategies over the BDEW domain types.

Import requires `hypothesis` (`pip install makotest[hypothesis]`); the rest of
`makotest` does not, so property-based testing stays opt-in.

Why these live in the library rather than in each consumer: a random 11-digit
string is almost never a valid Marktlokations-ID, so a hand-rolled strategy
generates values the platform rejects on the check digit and the test proves
only that rejection works. Same for Prüfidentifikatoren — invent one and you
exercise the unknown-PID path, where validation passes vacuously. Every
strategy here draws from the same Rust core the platform validates with, so
generated values are ones the system under test genuinely accepts.

    >>> from hypothesis import given
    >>> from makotest.strategies import malo_ids, pruefidentifikatoren
    >>> @given(malo=malo_ids(), pid=pruefidentifikatoren(message_type="UTILMD"))
    ... def test_roundtrip(malo, pid): ...
"""

from __future__ import annotations

from typing import TYPE_CHECKING

try:
    from hypothesis import strategies as st
except ImportError as exc:  # pragma: no cover - dependency guard
    raise ImportError(
        "makotest.strategies needs hypothesis. Install it with "
        "`pip install makotest[hypothesis]`."
    ) from exc

from ._native import (
    malo_from_base,
    message_types_of,
    pruefidentifikatoren as _pruefidentifikatoren,
)

if TYPE_CHECKING:
    from hypothesis.strategies import SearchStrategy

__all__ = [
    "bilanzierungsgebiete",
    "malo_ids",
    "marktpartner_ids",
    "melo_ids",
    "pruefidentifikatoren",
    "werktage",
    "zeitreihen",
]

# ── Identifiers ───────────────────────────────────────────────────────────────


def malo_ids() -> SearchStrategy[str]:
    """Check-digit-valid 11-digit Marktlokations-IDs.

    The check digit is computed by the same Rust routine the platform validates
    with, so every drawn value is one it accepts.
    """
    return st.integers(min_value=1_000_000_000, max_value=9_999_999_999).map(
        lambda base: malo_from_base(str(base))
    )


def melo_ids(*, country: str = "DE") -> SearchStrategy[str]:
    """Well-formed 33-character Messlokations-IDs.

    Structure is `<2-char country><31 alphanumerics>`. Unlike the MaLo there is
    no check digit — the constraint is length and character set.
    """
    if len(country) != 2 or not country.isalpha():
        raise ValueError(f"country must be a 2-letter ISO code, got {country!r}")
    body = st.text(
        alphabet="0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        min_size=31,
        max_size=31,
    )
    return body.map(lambda rest: f"{country.upper()}{rest}")


def marktpartner_ids(*, kind: str = "bdew") -> SearchStrategy[str]:
    """Marktpartner identifiers — BDEW-Codenummer or GLN.

    `kind="bdew"` draws 13-digit codes starting `99` (the DE BDEW range, UNB
    qualifier 500); `kind="gln"` draws GS1 GLNs (qualifier 14); `kind="dvgw"`
    draws 13-digit DVGW codes starting `98` (qualifier 502). The qualifier a
    platform derives from the ID is exactly what these distinguish.
    """
    prefixes = {"bdew": "99", "dvgw": "98", "gln": "40"}
    if kind not in prefixes:
        raise ValueError(f"kind must be one of {sorted(prefixes)}, got {kind!r}")
    prefix = prefixes[kind]
    return st.text(alphabet="0123456789", min_size=11, max_size=11).map(
        lambda rest: f"{prefix}{rest}"
    )


def bilanzierungsgebiete() -> SearchStrategy[str]:
    """Bilanzierungsgebiet EIC codes — 16 characters, `<2-digit issuer>X<13>`.

    The final character of a real EIC is an ISO check character, which is
    **not** computed here: mako carries Marktpartner and Bilanzierungsgebiet
    identifiers as unvalidated wire-boundary types, so nothing on the path
    under test verifies it. Values are structurally shaped like the codes in
    the wild (`10XDE-EON-NETZ--`) and will not survive a checksum validator.
    """
    issuer = st.text(alphabet="0123456789", min_size=2, max_size=2)
    rest = st.text(
        alphabet="0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-",
        min_size=13,
        max_size=13,
    )
    return st.tuples(issuer, rest).map(lambda t: f"{t[0]}X{t[1]}")


# ── Prüfidentifikatoren ───────────────────────────────────────────────────────


def pruefidentifikatoren(
    *,
    message_type: str | None = None,
    sparte: str | None = None,
) -> SearchStrategy[int]:
    """Prüfidentifikatoren the compiled profile set actually validates.

    Draws only from PIDs with real AHB rules. A PID without rules validates
    vacuously — `is_valid` is `True` having checked nothing — so generating one
    would produce a test that cannot fail.

    `message_type` restricts to one EDIFACT type (e.g. `"UTILMD"`). `sparte`
    restricts UTILMD to `"STROM"` (55xxx) or `"GAS"` (44xxx); it has no effect
    on other types, whose bands are Sparte-neutral.
    """
    if message_type is None:
        pool: list[int] = []
        for mt in ("UTILMD", "MSCONS", "ORDERS", "ORDRSP", "INVOIC", "REMADV", "APERAK"):
            pool.extend(_pruefidentifikatoren(mt))
    else:
        pool = list(_pruefidentifikatoren(message_type))

    if sparte is not None:
        band = {"STROM": (55000, 55999), "GAS": (44000, 44999)}
        key = sparte.upper()
        if key not in band:
            raise ValueError(f"sparte must be STROM or GAS, got {sparte!r}")
        lo, hi = band[key]
        # Only UTILMD splits its band by Sparte; leave other types untouched.
        pool = [p for p in pool if "UTILMD" not in message_types_of(p) or lo <= p <= hi]

    if not pool:
        raise ValueError(
            f"no Prüfidentifikatoren with AHB rules for message_type="
            f"{message_type!r}, sparte={sparte!r} — nothing to draw from"
        )
    return st.sampled_from(sorted(set(pool)))


# ── Time ──────────────────────────────────────────────────────────────────────


def werktage(
    *,
    min_date: str = "2026-01-01",
    max_date: str = "2027-12-31",
) -> SearchStrategy[str]:
    """ISO 8601 dates that are Werktage under the BDEW MaKo calendar.

    Deadline arithmetic behaves differently on a Werktag than on a Samstag or a
    Feiertag, so a strategy over arbitrary dates conflates two questions. Draw
    from here when the test is about the Frist, not about the calendar.
    """
    import datetime as _dt

    from ._native import is_werktag

    lo = _dt.date.fromisoformat(min_date)
    hi = _dt.date.fromisoformat(max_date)
    if lo > hi:
        raise ValueError(f"min_date {min_date} is after max_date {max_date}")
    return (
        st.integers(min_value=0, max_value=(hi - lo).days)
        .map(lambda n: (lo + _dt.timedelta(days=n)).isoformat())
        .filter(is_werktag)
    )


# ── Measurements ──────────────────────────────────────────────────────────────


def zeitreihen(
    *,
    periods: int = 96,
    max_kwh: float = 50.0,
) -> SearchStrategy[list[float]]:
    """Consumption time series in kWh, one value per MTU.

    Defaults to 96 periods — a full day at the 15-minute resolution MSCONS uses
    for RLM. Values are non-negative (consumption); negate the series for a
    feed-in profile.
    """
    if periods < 1:
        raise ValueError(f"periods must be >= 1, got {periods}")
    return st.lists(
        st.floats(
            min_value=0.0,
            max_value=max_kwh,
            allow_nan=False,
            allow_infinity=False,
            width=32,
        ),
        min_size=periods,
        max_size=periods,
    )
