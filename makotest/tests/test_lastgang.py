"""Synthetic Lastgang curves — shape, scale, and the Europe/Berlin day.

These are deliberately **not** Standardlastprofile: this build carries no BDEW
coefficient tables, so the profiles are named for what they look like. The tests
below pin the properties a consumer relies on — length, determinism, the annual
scale and the direction of the seasonal swing — and nothing that would claim SLP
fidelity.
"""

from __future__ import annotations

import pytest

from conftest import ON
from makotest import berlin_mtu_count
from makotest.generators import LastgangGenerator

PROFILES = ("haushalt", "gewerbe", "waermepumpe", "pv_einspeisung")


def peak_hour(series: list[float], mtu_minutes: int = 15) -> int:
    return max(range(len(series)), key=series.__getitem__) * mtu_minutes // 60


class TestDeliveryDay:
    @pytest.mark.parametrize(
        ("tag", "expected"),
        [("2026-03-29", 92), ("2026-06-21", 96), ("2026-10-25", 100)],
    )
    def test_a_series_has_one_value_per_mtu_of_that_berlin_day(self, tag, expected):
        """The two DST days are the whole reason the length is asked, not assumed.

        A 96-value series on the 23-hour March day runs four intervals past
        midnight, and the error lands mid-day where the curve still looks
        plausible.
        """
        gen = LastgangGenerator(seed=1)
        assert len(gen.day(tag)) == expected == berlin_mtu_count(tag, 15)
        assert gen.mtu_count(tag) == expected

    def test_hourly_resolution_tiles_the_same_day(self):
        assert len(LastgangGenerator(seed=1, mtu_minutes=60).day("2026-10-25")) == 25

    def test_an_unsupported_resolution_is_refused(self):
        with pytest.raises(ValueError, match="15 or 60"):
            LastgangGenerator(mtu_minutes=7)


class TestDeterminism:
    def test_the_same_seed_reproduces_the_curve(self):
        assert LastgangGenerator(seed=42).day(ON) == LastgangGenerator(seed=42).day(ON)

    def test_a_different_seed_does_not(self):
        assert LastgangGenerator(seed=1).day(ON) != LastgangGenerator(seed=2).day(ON)

    def test_day_order_does_not_affect_output(self):
        """Seeded per day, so asking for one day first cannot change another."""
        forwards = LastgangGenerator(seed=7)
        first = forwards.day("2026-11-01")
        forwards.day("2026-11-02")

        backwards = LastgangGenerator(seed=7)
        backwards.day("2026-11-02")
        assert backwards.day("2026-11-01") == first

    def test_the_profiles_do_not_share_a_stream(self):
        gen = LastgangGenerator(seed=3)
        assert gen.day(ON, profile="haushalt") != gen.day(ON, profile="gewerbe")


class TestScale:
    @pytest.mark.parametrize("profile", PROFILES)
    def test_a_year_of_days_sums_to_the_annual_quantity(self, profile):
        """The seasonal factor weights days against each other, not in total.

        A factor that did not average to 1.0 would make every annual figure
        asserted against this generator quietly wrong.
        """
        gen = LastgangGenerator(seed=5)
        total = sum(
            sum(gen.day(f"2026-{month:02d}-15", profile=profile))
            for month in range(1, 13)
        )
        assert total * 365 / 12 == pytest.approx(3_500.0, rel=0.08)

    @pytest.mark.parametrize("profile", PROFILES)
    def test_the_stated_daily_quantity_survives_the_noise(self, profile):
        """Scaling happens after the noise, so `streuung` cannot move the total."""
        gen = LastgangGenerator(seed=5)
        quiet = sum(gen.day(ON, profile=profile, streuung=0.0))
        noisy = sum(gen.day(ON, profile=profile, streuung=0.4))
        assert quiet == pytest.approx(noisy, rel=1e-9)

    @pytest.mark.parametrize("profile", PROFILES)
    def test_every_value_is_non_negative(self, profile):
        """Feed-in is generation at its own register; the sign is the platform's."""
        assert all(v >= 0.0 for v in LastgangGenerator(seed=9).day(ON, profile=profile))

    def test_a_zero_annual_quantity_gives_a_zero_series(self):
        series = LastgangGenerator(seed=1).day(ON, jahresmenge_kwh=0.0)
        assert series and not any(series)

    @pytest.mark.parametrize(
        ("kwargs", "match"),
        [
            ({"jahresmenge_kwh": -1.0}, "must not be negative"),
            ({"streuung": -0.1}, "must not be negative"),
            ({"profile": "h0"}, "unknown profile"),
        ],
    )
    def test_a_nonsense_argument_is_refused(self, kwargs, match):
        with pytest.raises(ValueError, match=match):
            LastgangGenerator(seed=1).day(ON, **kwargs)


class TestShape:
    @pytest.mark.parametrize(
        ("profile", "lo", "hi"),
        [
            ("haushalt", 17, 21),  # evening peak
            ("gewerbe", 9, 13),  # business hours
            ("waermepumpe", 1, 5),  # the night window
            ("pv_einspeisung", 11, 15),  # solar noon
        ],
    )
    def test_each_profile_peaks_where_its_name_says(self, profile, lo, hi):
        series = LastgangGenerator(seed=4).day(ON, profile=profile, streuung=0.0)
        assert lo <= peak_hour(series) <= hi

    def test_a_generation_profile_is_dark_at_night(self):
        series = LastgangGenerator(seed=4).day(
            "2026-06-21", profile="pv_einspeisung", streuung=0.0
        )
        midnight = series[:8] + series[-8:]  # first and last two hours
        assert max(midnight) < max(series) / 100

    @pytest.mark.parametrize(
        ("profile", "heavier"),
        [
            ("haushalt", "winter"),
            ("gewerbe", "winter"),
            ("waermepumpe", "winter"),
            ("pv_einspeisung", "summer"),
        ],
    )
    def test_the_seasonal_swing_points_the_right_way(self, profile, heavier):
        gen = LastgangGenerator(seed=6)
        january = sum(gen.day("2026-01-15", profile=profile))
        july = sum(gen.day("2026-07-15", profile=profile))
        assert (january > july) is (heavier == "winter")


class TestIntegration:
    def test_a_curve_feeds_the_gateway_delivery_unchanged(self):
        """`Zaehlerstandsgang` refuses a wrong-length series, so this is a real check."""
        from makotest import ImsysSim
        from makotest.plugin import IMSYS_MELO

        smgw = ImsysSim(melo_id=IMSYS_MELO, taf="TAF-7", today="2026-10-25")
        werte = LastgangGenerator(seed=11).day("2026-10-25", profile="haushalt")
        gang = smgw.deliver("2026-10-25", werte=werte)

        assert gang.mtu_count == 100 and gang.vollstaendig
        assert len(gang.as_direct_push(sender_mp_id="9900357000003")["intervals"]) == 100

    def test_the_plugin_fixture_is_seeded_from_the_session(self, lastgang, makotest_seed):
        assert lastgang.day(ON) == LastgangGenerator(seed=makotest_seed).day(ON)
