"""The EPEX generator — determinism, the local day, and negative prices."""

import pytest

from makotest.generators import EpexGenerator


class TestDayLength:
    """A delivery day is a Europe/Berlin day, and two a year are not 24 hours."""

    @pytest.mark.parametrize(
        ("date", "quarter_hours"),
        [
            ("2026-06-21", 96),
            ("2026-03-29", 92),  # CET -> CEST: 23 hours
            ("2026-10-25", 100),  # CEST -> CET: 25 hours
        ],
    )
    def test_the_dst_days_carry_92_and_100_mtus(self, date, quarter_hours):
        """Assuming 96 invents four MTUs in March and drops four in October.

        Both errors land mid-day, where a curve still looks plausible and a
        settlement quietly comes out wrong.
        """
        sim = EpexGenerator(mtu_minutes=15)
        assert sim.mtu_count(date) == quarter_hours
        assert len(list(sim.day(date))) == quarter_hours

    def test_hourly_resolution_follows_the_same_day(self):
        sim = EpexGenerator(mtu_minutes=60)
        assert [sim.mtu_count(d) for d in ("2026-03-29", "2026-06-21", "2026-10-25")] == [
            23,
            24,
            25,
        ]

    def test_the_day_starts_at_local_midnight_not_utc(self):
        """Summer-time midnight in Berlin is 22:00 UTC the day before."""
        first = next(iter(EpexGenerator().day("2026-06-21")))
        assert first.mtu_start.isoformat() == "2026-06-20T22:00:00+00:00"
        assert first.price_date.isoformat() == "2026-06-21"


class TestDeterminism:
    def test_same_seed_reproduces_the_curve(self):
        def curve() -> list[float]:
            sim = EpexGenerator(seed=42)
            return [p.avg_ct_kwh for p in sim.day("2026-11-01", profile="winter_peak")]

        assert curve() == curve()

    def test_different_seeds_differ(self):
        a = [p.avg_ct_kwh for p in EpexGenerator(seed=1).day("2026-11-01")]
        b = [p.avg_ct_kwh for p in EpexGenerator(seed=2).day("2026-11-01")]
        assert a != b

    def test_day_order_does_not_affect_output(self):
        """Asking for a later day first must not change an earlier day's curve."""
        sim = EpexGenerator(seed=7)
        first = [p.avg_ct_kwh for p in sim.day("2026-11-01")]
        _ = list(sim.day("2026-11-02"))
        assert [p.avg_ct_kwh for p in sim.day("2026-11-01")] == first


class TestNegativePrices:
    def test_negative_hours_produce_negative_prices(self):
        """§51 EEG and §41a dynamic tariffs both need this path exercised."""
        points = list(
            EpexGenerator(seed=3).day(
                "2026-06-21", profile="solar_glut", negative_hours=6
            )
        )
        assert len([p for p in points if p.avg_ct_kwh < 0]) == 6 * 4

    def test_negative_hours_are_measured_against_the_days_own_length(self):
        """A 23-hour day cannot hold 24 negative hours, and says so."""
        with pytest.raises(ValueError, match="exceeds the 23-hour"):
            list(EpexGenerator().day("2026-03-29", negative_hours=24))

    def test_a_negative_count_is_rejected(self):
        with pytest.raises(ValueError, match="must not be negative"):
            list(EpexGenerator().day("2026-06-21", negative_hours=-1))


class TestIngestShape:
    def test_ingest_row_shape(self):
        row = next(iter(EpexGenerator(seed=1).day("2026-11-01"))).as_ingest_row()
        assert set(row) == {
            "mtu_start",
            "price_date",
            "mtu_minutes",
            "avg_ct_kwh",
            "source",
        }
        assert row["mtu_minutes"] in (15, 60)

    def test_invalid_mtu_is_rejected(self):
        with pytest.raises(ValueError, match="15 or 60"):
            EpexGenerator(mtu_minutes=30)

    def test_a_month_covers_every_day(self):
        points = list(EpexGenerator(seed=1).month(2026, 3))
        assert len(points) == 31 * 96 - 4, "March has one 23-hour day"
