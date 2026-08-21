"""Werktag arithmetic — the same calendar the platform uses."""

import pytest

from makotest import (
    add_werktage,
    berlin_day_bounds,
    berlin_instant,
    berlin_mtu_count,
    is_werktag,
    next_werktag,
)


def test_weekend_is_not_a_werktag():
    assert not is_werktag("2026-11-07")  # Saturday
    assert not is_werktag("2026-11-08")  # Sunday
    assert is_werktag("2026-11-06")  # Friday


def test_bundesweiter_feiertag_is_not_a_werktag():
    assert not is_werktag("2026-01-01")  # Neujahr
    assert not is_werktag("2026-10-03")  # Tag der Deutschen Einheit
    assert not is_werktag("2026-12-25")  # 1. Weihnachtstag


def test_landesfeiertag_counts_as_non_werktag():
    """BDEW is conservative-inclusive: a holiday in *any* Land is a non-Werktag.

    This keeps a Frist from ever being computed shorter than the Festlegung
    requires for some participant, so it must not be treated as a bug.
    """
    assert not is_werktag("2026-01-06")  # Heilige Drei Könige (BY, BW, ST)
    assert not is_werktag("2026-11-01")  # Allerheiligen (BW, BY, NW, RP, SL)


def test_heiligabend_and_silvester_are_feiertage():
    """GPKE Teil 1: „Der 24.12. und der 31.12. gelten als Feiertage."

    Neither is a statutory holiday anywhere in Germany, so a calendar built from
    the Feiertagsgesetze alone counts them as Werktage and computes every
    year-end Frist one or two days short.
    """
    assert not is_werktag("2026-12-24")
    assert not is_werktag("2026-12-31")


def test_advancing_skips_weekend_and_holiday():
    # Thu 24 Dec -> 25/26 are holidays, 27/28 weekend -> 2 WT lands on Tue 29.
    assert add_werktage("2026-12-24", 2) == "2026-12-29"


def test_next_werktag_is_idempotent_on_a_werktag():
    assert next_werktag("2026-11-06") == "2026-11-06"
    assert next_werktag("2026-11-07") == "2026-11-09"  # Sat -> Mon


class TestBerlinDay:
    """A German delivery day is a local day, and two a year are not 24 hours."""

    def test_an_ordinary_day_is_24_hours(self):
        start, end = berlin_day_bounds("2026-06-21")
        assert start == "2026-06-20T22:00:00Z"  # 00:00 CEST
        assert end == "2026-06-21T22:00:00Z"

    def test_the_dst_days_are_23_and_25_hours(self):
        """Assuming 24 invents an hour in March and drops one in October.

        At quarter-hourly resolution that is four phantom MTUs and four missing
        ones, landing mid-day where a curve still looks plausible.
        """
        import datetime as dt

        def hours(date: str) -> float:
            start, end = berlin_day_bounds(date)
            span = dt.datetime.fromisoformat(end) - dt.datetime.fromisoformat(start)
            return span.total_seconds() / 3600

        assert hours("2026-03-29") == 23  # CET -> CEST
        assert hours("2026-10-25") == 25  # CEST -> CET


class TestBerlinInstant:
    """A German wall clock is +01:00 for part of the year and +02:00 for the rest.

    The offset is a property of the date, so it is resolved rather than carried:
    anything that keeps the offset it was constructed with reports an instant an
    hour off the wall clock it claims, in the same direction for every deadline
    computed from it.
    """

    def test_the_offset_follows_the_date(self):
        assert berlin_instant("2026-03-27", "09:00") == "2026-03-27T09:00:00+01:00"
        assert berlin_instant("2026-03-31", "09:00") == "2026-03-31T09:00:00+02:00"

    def test_seconds_are_optional(self):
        assert berlin_instant("2026-11-03", "09:00:30") == "2026-11-03T09:00:30+01:00"

    @pytest.mark.parametrize("bad", ["0900", "25:00", "09:00:00:00", "nine"])
    def test_a_malformed_clock_time_is_refused(self, bad):
        with pytest.raises(ValueError, match="HH:MM"):
            berlin_instant("2026-11-03", bad)


class TestMarketTimeUnits:
    @pytest.mark.parametrize(
        ("date", "quarters", "hours"),
        [("2026-03-29", 92, 23), ("2026-06-21", 96, 24), ("2026-10-25", 100, 25)],
    )
    def test_a_day_is_measured_in_its_own_units(self, date, quarters, hours):
        assert berlin_mtu_count(date, 15) == quarters
        assert berlin_mtu_count(date, 60) == hours

    def test_a_resolution_that_does_not_tile_the_day_is_refused(self):
        """Truncating would reintroduce the phantom-MTU error this exists to stop."""
        with pytest.raises(ValueError, match="do not tile"):
            berlin_mtu_count("2026-06-21", 7)


#: Start dates that sit next to every awkward feature of the calendar.
STARTS = [
    "2026-12-22",  # before the year-end cluster
    "2026-03-27",  # before the CET/CEST transition
    "2026-01-04",  # a Sunday
    "2026-01-01",  # a Feiertag
    "2026-05-13",
]


class TestCalendarInvariants:
    """Relations the Werktag calendar has to satisfy for every input.

    Cheaper than enumerating dates and stronger: each one is a statement about
    the arithmetic rather than about one week of it.
    """

    @pytest.mark.parametrize("start", STARTS)
    @pytest.mark.parametrize(("m", "n"), [(1, 1), (2, 3), (0, 4), (5, 0), (3, 7)])
    def test_advancing_composes(self, start, m, n):
        """Advancing in two steps must land where advancing in one does.

        A `FrozenClock` moved repeatedly through a scenario would otherwise drift
        away from the same Frist computed in one go.
        """
        assert add_werktage(start, m + n) == add_werktage(add_werktage(start, m), n)

    @pytest.mark.parametrize("start", STARTS)
    def test_advancing_is_monotone_in_the_count(self, start):
        dates = [add_werktage(start, n) for n in range(6)]
        assert dates == sorted(dates)
        assert len(set(dates[1:])) == 5, "each further Werktag is a new day"

    def test_advancing_is_monotone_in_the_start_date(self):
        """An earlier request can never get a later deadline than a later one."""
        import datetime as dt

        day = dt.date.fromisoformat("2026-12-18")
        results = []
        for _ in range(20):
            results.append(add_werktage(day.isoformat(), 3))
            day += dt.timedelta(days=1)
        assert results == sorted(results)

    @pytest.mark.parametrize("start", STARTS)
    def test_next_werktag_is_idempotent(self, start):
        once = next_werktag(start)
        assert next_werktag(once) == once
        assert is_werktag(once)
