"""Synthetic consumption and feed-in curves, laid out over a delivery day.

Shaped to the contract a MaKo platform ingests meter data on: one value per
market time unit, kWh per interval, 15- or 60-minute resolution.

## These are not Standardlastprofile

The BDEW Standardlastprofile (H0, G0–G6, L0–L2) are published coefficient
tables, and this build does not carry them. The profiles here are **synthetic
shapes** chosen to exercise the paths a platform has — a daytime commercial
peak, an evening household peak, a midday feed-in inversion — and they are
deliberately named for what they look like rather than for an SLP class. A
generator that called itself `H0` while inventing the coefficients would be
worse than none: every settlement asserted against it would look authoritative
and be wrong.

Use these to drive ingest, Ersatzwertbildung and settlement *plumbing*. For a
figure that has to match a published profile, take the profile.

## The delivery day is a local day

A day is a Europe/Berlin calendar day, so two a year are not 24 hours long: 92
and 100 quarter-hourly MTUs rather than 96. The length comes from
`berlin_mtu_count`, so a curve and the Fristen it is asserted against cannot
disagree about when a day starts — and `Zaehlerstandsgang` refuses a series of
the wrong length rather than laying it out past midnight.

Curves are synthetic and deterministic. Never present their output as measured
consumption.
"""

from __future__ import annotations

import datetime as _dt
import math
import random
from dataclasses import dataclass
from typing import Literal

from .._native import berlin_mtu_count

__all__ = ["LastgangGenerator", "Lastprofil"]

Lastprofil = Literal["haushalt", "gewerbe", "waermepumpe", "pv_einspeisung"]

#: Fraction of the annual quantity a single day carries, before the seasonal
#: factor. `1/365` — a leap day is a longer year, not a heavier day.
_TAGESANTEIL = 1.0 / 365.0


@dataclass(frozen=True, slots=True)
class _Shape:
    """A daily shape as a sum of Gaussian bumps over a flat base."""

    base: float
    #: `(centre hour, width in hours, height)` per bump.
    bumps: tuple[tuple[float, float, float], ...]
    #: How much heavier the darkest day of the year is than the lightest.
    #: `1.0` is flat across the year.
    saisonhub: float
    #: `True` when the curve peaks in summer rather than in winter.
    sommerlastig: bool = False


_SHAPES: dict[str, _Shape] = {
    # Morning and a dominant evening peak, deep night trough; heavier in winter.
    "haushalt": _Shape(
        base=0.35,
        bumps=((7.5, 1.3, 0.7), (12.5, 1.2, 0.5), (19.0, 2.0, 1.4)),
        saisonhub=0.35,
    ),
    # Flat through business hours, near-nothing overnight and at the weekend.
    "gewerbe": _Shape(
        base=0.15,
        bumps=((11.0, 3.6, 1.6),),
        saisonhub=0.20,
    ),
    # Runs mostly at night on the cheap tariff window, and hard in winter.
    "waermepumpe": _Shape(
        base=0.5,
        bumps=((3.0, 3.0, 1.2), (16.0, 2.5, 0.6)),
        saisonhub=1.30,
    ),
    # Generation, not consumption: one midday bell, nothing after dark, and
    # strongly summer-lastig.
    "pv_einspeisung": _Shape(
        base=0.0,
        bumps=((13.0, 3.1, 1.0),),
        saisonhub=1.60,
        sommerlastig=True,
    ),
}


class LastgangGenerator:
    """Deterministic consumption / feed-in curves.

    Two runs with the same seed produce identical series, which is what makes a
    golden-file assertion viable and turns a flaky failure into a reproducible
    one::

        >>> a = LastgangGenerator(seed=42).day("2026-11-01")
        >>> b = LastgangGenerator(seed=42).day("2026-11-01")
        >>> a == b
        True

    The output feeds the gateway and the meter-data builder directly::

        gang = smgw.deliver("2026-11-01", werte=lastgang.day("2026-11-01"))
        gang.as_mscons(pruefidentifikator=13025, ...)   # one QTY per interval
    """

    def __init__(self, *, seed: int = 0, mtu_minutes: int = 15) -> None:
        if mtu_minutes not in (15, 60):
            raise ValueError("mtu_minutes must be 15 or 60 (BDEW market time units)")
        self._seed = seed
        self.mtu_minutes = mtu_minutes

    def mtu_count(self, date: str | _dt.date) -> int:
        """How many MTUs the Europe/Berlin day `date` has — 92, 96 or 100."""
        return berlin_mtu_count(_as_date(date).isoformat(), self.mtu_minutes)

    def day(
        self,
        date: str | _dt.date,
        *,
        profile: Lastprofil = "haushalt",
        jahresmenge_kwh: float = 3_500.0,
        streuung: float = 0.12,
    ) -> list[float]:
        """kWh per market time unit for one Europe/Berlin delivery day.

        `jahresmenge_kwh` is the annual quantity the curve is scaled to — the
        consumption for a load profile, the generation for `pv_einspeisung`.
        The day's total is that figure spread over 365 days and then weighted by
        a seasonal factor, so a January household day is heavier than a July one
        and a July PV day is heavier than a January one.

        `streuung` is the relative noise on each interval. `0` gives the bare
        shape, which is what a golden-file comparison usually wants.

        Values are **non-negative** and `pv_einspeisung` is no exception: it is
        generation measured at its own register, and which sign a platform
        settles it under is the platform's convention, not this generator's.
        """
        day = _as_date(date)
        if jahresmenge_kwh < 0:
            raise ValueError(
                f"jahresmenge_kwh must not be negative, got {jahresmenge_kwh}"
            )
        if streuung < 0:
            raise ValueError(f"streuung must not be negative, got {streuung}")
        try:
            shape = _SHAPES[profile]
        except KeyError:
            raise ValueError(
                f"unknown profile {profile!r}; known: {sorted(_SHAPES)}"
            ) from None

        periods = self.mtu_count(day)
        # Seeded per day so `day()` is order-independent: asking for 2026-11-02
        # first must not change what 2026-11-01 produces. A string seed keeps the
        # components unambiguous and is stable across CPython versions; tuple
        # seeds were removed in Python 3.14.
        rng = random.Random(f"{self._seed}:{profile}:{day.toordinal()}")

        raw = [
            max(0.0, _bumped(shape, i * self.mtu_minutes / 60.0))
            * (1.0 + rng.uniform(-streuung, streuung))
            for i in range(periods)
        ]
        total = sum(raw)
        if total <= 0.0:
            return [0.0] * periods

        # Scale to the day's share of the year. Done after the noise so the
        # stated daily quantity holds exactly whatever the noise did — a series
        # whose total drifts with `streuung` could not be asserted against.
        tagesmenge = jahresmenge_kwh * _TAGESANTEIL * _saisonfaktor(shape, day)
        return [value * tagesmenge / total for value in raw]

    def month(self, year: int, month: int, **kw: object) -> dict[str, list[float]]:
        """`{ISO date: series}` for every day of a calendar month."""
        out: dict[str, list[float]] = {}
        day = _dt.date(year, month, 1)
        while day.month == month:
            out[day.isoformat()] = self.day(day, **kw)  # type: ignore[arg-type]
            day += _dt.timedelta(days=1)
        return out


def _bumped(shape: _Shape, hour: float) -> float:
    return shape.base + sum(
        height * math.exp(-(((hour - centre) / width) ** 2))
        for centre, width, height in shape.bumps
    )


def _saisonfaktor(shape: _Shape, day: _dt.date) -> float:
    """Seasonal weight for `day`, averaging 1.0 across the year.

    A cosine over the day of the year, peaking at the winter solstice — or at
    the summer one for a generation profile. Crude on purpose: it is a shape,
    not a Standardlastprofil, and a more elaborate curve here would suggest a
    fidelity this generator does not have.
    """
    phase = 2.0 * math.pi * (day.timetuple().tm_yday - 1) / 365.0
    swing = math.cos(phase) if not shape.sommerlastig else -math.cos(phase)
    return 1.0 + shape.saisonhub * swing / 2.0


def _as_date(value: str | _dt.date) -> _dt.date:
    if isinstance(value, _dt.date):
        return value
    return _dt.date.fromisoformat(value)
