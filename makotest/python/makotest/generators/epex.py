"""Synthetic EPEX Spot day-ahead price curves.

Shaped to the ingest contract a MaKo platform expects: one point per market time
unit (MTU), 15- or 60-minute resolution, ct/kWh, **negative prices permitted**.

Negative hours are not a curiosity — they are load-bearing for §51 EEG
(Vergütungsausfall during sustained negative prices) and for §41a EnWG dynamic
tariffs. A generator that only produced positive prices would leave both paths
untested.

## The delivery day is a local day

A day-ahead delivery day is a **Europe/Berlin calendar day**, and two of them
each year are not 24 hours long: the last Sunday in March has 23 and the last in
October has 25. At quarter-hourly resolution that is 92 and 100 MTUs, not 96.

Assuming 96 invents four MTUs in March and drops four in October — and both
errors land in the middle of the day, where a curve still looks plausible and a
settlement quietly comes out wrong. The day boundary therefore comes from the
platform's own timezone resolution (`berlin_day_bounds`), so a curve and the
Fristen it is asserted against cannot disagree about when a day starts.

Curves are synthetic and deterministic. This is a test fixture, not market data;
never present its output as a price forecast.
"""

from __future__ import annotations

import datetime as _dt
import math
import random
from collections.abc import Iterator
from dataclasses import dataclass
from typing import Literal

from .._native import berlin_day_bounds

__all__ = ["EpexGenerator", "MtuPrice", "Profile"]

Profile = Literal["flat", "winter_peak", "solar_glut", "volatile"]


@dataclass(frozen=True, slots=True)
class MtuPrice:
    """A single MTU price point."""

    mtu_start: _dt.datetime
    """UTC start of the market time unit."""
    price_date: _dt.date
    """Europe/Berlin delivery date."""
    mtu_minutes: int
    avg_ct_kwh: float

    def as_ingest_row(self) -> dict[str, object]:
        """The payload shape a platform's EPEX ingest endpoint accepts."""
        return {
            "mtu_start": self.mtu_start.isoformat(),
            "price_date": self.price_date.isoformat(),
            "mtu_minutes": self.mtu_minutes,
            "avg_ct_kwh": round(self.avg_ct_kwh, 4),
            "source": "makotest",
        }


class EpexGenerator:
    """Deterministic day-ahead curve generator.

    Two runs with the same seed produce identical curves, which is what makes
    golden-file assertions viable and turns a flaky failure into a reproducible
    one::

        >>> a = list(EpexGenerator(seed=42).day("2026-11-01"))
        >>> b = list(EpexGenerator(seed=42).day("2026-11-01"))
        >>> a == b
        True
    """

    def __init__(self, *, seed: int = 0, mtu_minutes: int = 15) -> None:
        if mtu_minutes not in (15, 60):
            raise ValueError("mtu_minutes must be 15 or 60 (BDEW market time units)")
        self._seed = seed
        self.mtu_minutes = mtu_minutes

    def mtu_count(self, date: str | _dt.date) -> int:
        """How many MTUs the Europe/Berlin delivery day `date` has.

        96 on an ordinary day at quarter-hourly resolution, 92 on the short
        March day and 100 on the long October one.
        """
        start, end = self._bounds(_as_date(date))
        return int((end - start).total_seconds() // 60) // self.mtu_minutes

    def day(
        self,
        date: str | _dt.date,
        *,
        profile: Profile = "flat",
        negative_hours: int = 0,
        base_ct_kwh: float = 8.0,
    ) -> Iterator[MtuPrice]:
        """Yield one `MtuPrice` per MTU of a Europe/Berlin delivery day.

        `negative_hours` forces that many consecutive midday MTU blocks below
        zero — the §51 EEG / dynamic-tariff case. It is measured against the
        day's own length, so it stays inside a 23-hour day too.
        """
        day = _as_date(date)
        if negative_hours < 0:
            raise ValueError("negative_hours must not be negative")

        start, _ = self._bounds(day)
        per_day = self.mtu_count(day)
        negative_mtus = negative_hours * 60 // self.mtu_minutes
        if negative_mtus > per_day:
            hours = per_day * self.mtu_minutes // 60
            raise ValueError(
                f"negative_hours={negative_hours} exceeds the {hours}-hour "
                f"Europe/Berlin day on {day.isoformat()}"
            )

        # Seed per day so `day()` is order-independent: asking for 2026-11-02
        # first must not change what 2026-11-01 produces. A string seed keeps
        # the two components unambiguous and is stable across CPython versions;
        # tuple seeds were removed in Python 3.14.
        rng = random.Random(f"{self._seed}:{day.toordinal()}")

        neg_start = per_day // 2 - negative_mtus // 2
        neg_end = neg_start + negative_mtus

        for i in range(per_day):
            minute_of_day = i * self.mtu_minutes
            price = _shape(profile, minute_of_day, base_ct_kwh)
            price += rng.uniform(-0.4, 0.4)
            if neg_start <= i < neg_end:
                price = -abs(price) - 0.5
            yield MtuPrice(
                start + _dt.timedelta(minutes=minute_of_day),
                day,
                self.mtu_minutes,
                price,
            )

    def month(self, year: int, month: int, **kw: object) -> Iterator[MtuPrice]:
        """Yield every MTU of a calendar month."""
        day = _dt.date(year, month, 1)
        while day.month == month:
            yield from self.day(day, **kw)  # type: ignore[arg-type]
            day += _dt.timedelta(days=1)

    def _bounds(self, day: _dt.date) -> tuple[_dt.datetime, _dt.datetime]:
        start, end = berlin_day_bounds(day.isoformat())
        return _dt.datetime.fromisoformat(start), _dt.datetime.fromisoformat(end)


def _shape(profile: Profile, minute_of_day: int, base: float) -> float:
    hour = minute_of_day / 60.0
    if profile == "flat":
        return base
    if profile == "winter_peak":
        # Morning and evening peaks, deep night trough.
        return (
            base
            + 4.0
            * (
                math.exp(-(((hour - 8.0) / 1.6) ** 2))
                + math.exp(-(((hour - 18.5) / 1.8) ** 2))
            )
            - 2.0 * math.exp(-(((hour - 3.0) / 2.5) ** 2))
        )
    if profile == "solar_glut":
        # Midday collapse from PV infeed.
        return base - 6.0 * math.exp(-(((hour - 13.0) / 2.6) ** 2))
    if profile == "volatile":
        return base + 5.0 * math.sin(hour / 24.0 * 4 * math.pi)
    raise ValueError(f"unknown profile {profile!r}")


def _as_date(value: str | _dt.date) -> _dt.date:
    if isinstance(value, _dt.date):
        return value
    return _dt.date.fromisoformat(value)
