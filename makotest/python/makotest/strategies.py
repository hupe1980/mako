"""Hypothesis strategies over the BDEW domain types.

Import requires `hypothesis` (`pip install makotest[hypothesis]`); the rest of
`makotest` does not, so property-based testing stays opt-in.

Why these live in the library rather than in each consumer: **a random
identifier is almost never a valid one.** A random 11-digit string is a valid
Marktlokations-ID one time in ten, a random 16-character string is essentially
never a valid EIC, and a hand-rolled strategy therefore generates values the
platform rejects on the check digit — so the test proves only that rejection
works. Same for Prüfidentifikatoren: invent one and you exercise the unknown-PID
path, where validation passes vacuously.

Every strategy here constructs its values through the same Rust routines the
platform validates with, so a drawn value is one the system under test genuinely
accepts::

    >>> from hypothesis import given
    >>> from makotest.strategies import malo_ids, pruefidentifikatoren
    >>> @given(malo=malo_ids(), pid=pruefidentifikatoren(message_type="UTILMD"))
    ... def test_roundtrip(malo, pid): ...
"""

from __future__ import annotations

from typing import TYPE_CHECKING

try:
    from hypothesis import reject
    from hypothesis import strategies as st
except ImportError as exc:  # pragma: no cover - dependency guard
    raise ImportError(
        "makotest.strategies needs hypothesis. Install it with "
        "`pip install makotest[hypothesis]`."
    ) from exc

from ._native import (
    antwort_obligations,
    berlin_mtu_count,
    bilanzierungsgebiet_from_prefix,
    bilanzkreis_from_prefix,
    malo_from_base,
    message_types_of,
    mp_id_from_base,
    resource_id_from_base,
)
from ._native import (
    pruefidentifikatoren as _pruefidentifikatoren,
)

if TYPE_CHECKING:
    from hypothesis.strategies import SearchStrategy

__all__ = [
    "antwort_pids",
    "bilanzierungsgebiete",
    "bilanzkreise",
    "malo_ids",
    "marktpartner_ids",
    "melo_ids",
    "pruefidentifikatoren",
    "resource_ids",
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
    return st.text(
        alphabet="0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ", min_size=31, max_size=31
    ).map(lambda rest: f"{country.upper()}{rest}")


def marktpartner_ids(*, kind: str = "bdew") -> SearchStrategy[str]:
    """Marktpartner-IDs with a **valid check digit**.

    `kind="bdew"` draws BDEW-Codenummern (`99…`, UNB qualifier 500), `"dvgw"`
    DVGW-Codenummern (`98…`, qualifier 502) and `"gln"` GS1 Global Location
    Numbers (qualifier 14).

    The check digit is not decoration and the two families do not share one:
    §2.3 of the BDEW Anwendungshilfe defines the Lok- und
    Waggon-Kennzeichnungsverfahren for BDEW/DVGW codes and the GS1/EAN-13
    procedure for a GLN, and they disagree on almost every base. Each digit here
    is computed by `rubo4e`, so a drawn ID survives a counterparty that checks.
    """
    prefixes = {"bdew": "99", "dvgw": "98", "gln": "40"}
    if kind not in prefixes:
        raise ValueError(f"kind must be one of {sorted(prefixes)}, got {kind!r}")
    scheme = "gln" if kind == "gln" else "bdew"
    prefix = prefixes[kind]
    return st.text(alphabet="0123456789", min_size=10, max_size=10).map(
        lambda rest: mp_id_from_base(f"{prefix}{rest}", scheme)
    )


def bilanzkreise(*, issuer: str = "11") -> SearchStrategy[str]:
    """Bilanzkreis-IDs — 16-character EICs with object type **`X`** (Party).

    A Bilanzkreis is held by a Bilanzkreisverantwortlicher, which ENTSO-E
    classifies as a party. The trailing character is the real ENTSO-E check
    character, computed by `rubo4e`.
    """
    return _eic(issuer, "X", bilanzkreis_from_prefix)


def bilanzierungsgebiete(*, issuer: str = "11") -> SearchStrategy[str]:
    """Bilanzierungsgebiet-IDs — 16-character EICs with object type **`Y`** (Area).

    A Bilanzierungsgebiet is a grid area, not a party, and the object-type
    character at position 3 is the *only* thing separating it from a
    Bilanzkreis: both are 16 characters and both carry a valid check character.
    MSCONS SG6 carries both as free text under different `LOC` qualifiers, so a
    series filed against the wrong one is a misfiling the BIKO cannot tell from
    a correct submission — which is why these are two strategies rather than one
    with a flag.
    """
    return _eic(issuer, "Y", bilanzierungsgebiet_from_prefix)


def _eic(issuer: str, object_type: str, complete) -> SearchStrategy[str]:
    if len(issuer) != 2 or not issuer.isalnum():
        raise ValueError(f"issuer must be 2 alphanumerics, got {issuer!r}")

    @st.composite
    def build(draw) -> str:
        rest = draw(
            st.text(
                alphabet="0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-",
                min_size=12,
                max_size=12,
            )
        )
        try:
            return complete(f"{issuer}{object_type}{rest}")
        except ValueError:
            # ENTSO-E prohibits '-' as a check character, so roughly one prefix
            # in 37 has no valid completion. `assume` redraws inside the
            # generator, which keeps shrinking working on the draw itself;
            # `.filter()` on the outside would leave the reducer unable to tell
            # a rejected draw from a failing one. Patching the character instead
            # would bias the alphabet away from the real code space.
            reject()
            raise  # unreachable; `reject()` does not return

    return build()


def resource_ids(*, kind: str = "nelo") -> SearchStrategy[str]:
    """BDEW §8.2 ASCII identifiers with a valid check digit.

    `kind` is one of `nelo` (Netzlokation), `nebe` (Netzbereich), `tr`
    (Technische Ressource), `sr` (Steuerbare Ressource), `sg` (Steuergruppe),
    `cr` (Cluster Ressource) or `paket` (Netzbetreiberwechsel).

    These are the objects a UTILMD transaction names alongside a MaLo or MeLo,
    so a test that builds one needs to be able to draw one. The Codetyp is fixed
    per family by the BDEW document and is supplied here, not chosen.
    """
    prefixes = dict(_resource_kinds())
    if kind not in prefixes:
        raise ValueError(f"kind must be one of {sorted(prefixes)}, got {kind!r}")
    prefix = prefixes[kind]
    return st.text(
        alphabet="0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        min_size=10 - len(prefix),
        max_size=10 - len(prefix),
    ).map(lambda rest: resource_id_from_base(kind, f"{prefix}{rest}"))


def _resource_kinds() -> list[tuple[str, str]]:
    from ._native import resource_id_kinds

    return resource_id_kinds()


# ── Prüfidentifikatoren ───────────────────────────────────────────────────────


def pruefidentifikatoren(
    *,
    message_type: str | None = None,
    sparte: str | None = None,
    on: str | None = None,
) -> SearchStrategy[int]:
    """Prüfidentifikatoren the compiled profile set actually validates.

    Draws only from PIDs with real AHB rules. A PID without rules validates
    vacuously — `is_valid` is `True` having checked nothing — so generating one
    would produce a test that cannot fail.

    `message_type` restricts to one EDIFACT type (e.g. `"UTILMD"`). `sparte`
    restricts UTILMD to `"STROM"` (55xxx) or `"GAS"` (44xxx). `on` (ISO 8601)
    restricts to the profile **active on that date**, which is narrower and
    usually what you want: a PID retired at the last Formatumstellung is still
    known to the registry through its old profile, but a message carrying it
    today validates vacuously anyway.
    """
    types = (
        [message_type]
        if message_type is not None
        else ["UTILMD", "MSCONS", "ORDERS", "ORDRSP", "INVOIC", "REMADV", "APERAK"]
    )
    pool: list[int] = []
    for mt in types:
        pool.extend(_pruefidentifikatoren(mt, on, sparte))

    if sparte is not None and message_type is None:
        # `sparte` is only meaningful for UTILMD; drop the other types' PIDs
        # rather than silently returning a mixed pool the caller did not ask for.
        band = {"STROM": (55000, 55999), "GAS": (44000, 44999)}
        key = sparte.upper()
        if key not in band:
            raise ValueError(f"sparte must be STROM or GAS, got {sparte!r}")
        lo, hi = band[key]
        pool = [p for p in pool if "UTILMD" not in message_types_of(p) or lo <= p <= hi]

    if not pool:
        raise ValueError(
            f"no Prüfidentifikatoren with AHB rules for message_type="
            f"{message_type!r}, sparte={sparte!r}, on={on!r} — nothing to draw from"
        )
    return st.sampled_from(sorted(set(pool)))


def antwort_pids() -> SearchStrategy[int]:
    """Inbound PIDs whose answer Frist a Festlegung quantifies.

    Every draw has a published window, so a property over these can assert the
    platform registered *a* deadline without the test having to know which
    shape applies — the table does.
    """
    return st.sampled_from(sorted(o.trigger_pid for o in antwort_obligations()))


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
    periods: int | None = None,
    on: str | None = None,
    mtu_minutes: int = 15,
    max_kwh: float = 50.0,
) -> SearchStrategy[list[float]]:
    """Consumption time series in kWh, one value per market time unit.

    Pass `on` (an ISO 8601 delivery date) and the length is the number of MTUs
    that **Europe/Berlin** day really has — 96 normally, 92 on the short March
    day and 100 on the long October one. Pass `periods` to fix a length outright
    when the test is not about a calendar day.

    Values are non-negative (consumption); negate the series for a feed-in
    profile.
    """
    if (periods is None) == (on is None):
        raise ValueError(
            "pass exactly one of on= (a delivery date, whose Europe/Berlin day "
            "decides the length) or periods= (an explicit count)"
        )
    if on is not None:
        periods = berlin_mtu_count(on, mtu_minutes)
    assert periods is not None  # narrowed by the guard above
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
