"""Werktag arithmetic — the same calendar the platform uses."""

from makotest import add_werktage, is_werktag, next_werktag


def test_weekend_is_not_a_werktag():
    assert not is_werktag("2026-11-07")  # Saturday
    assert not is_werktag("2026-11-08")  # Sunday
    assert is_werktag("2026-11-06")      # Friday


def test_bundesweiter_feiertag_is_not_a_werktag():
    assert not is_werktag("2026-01-01")   # Neujahr
    assert not is_werktag("2026-10-03")   # Tag der Deutschen Einheit
    assert not is_werktag("2026-12-25")   # 1. Weihnachtstag


def test_landesfeiertag_counts_as_non_werktag():
    """BDEW is conservative-inclusive: a holiday in *any* Land is a non-Werktag.

    This keeps a Frist from ever being computed shorter than the AHB requires
    for some participant, so it must not be treated as a bug.
    """
    assert not is_werktag("2026-01-06")   # Heilige Drei Könige (BY, BW, ST)
    assert not is_werktag("2026-11-01")   # Allerheiligen (BW, BY, NW, RP, SL)


def test_advancing_skips_weekend_and_holiday():
    # Thu 24 Dec -> 25/26 are holidays, 27/28 weekend -> 2 WT lands on Tue 29.
    assert add_werktage("2026-12-24", 2) == "2026-12-29"


def test_next_werktag_is_idempotent_on_a_werktag():
    assert next_werktag("2026-11-06") == "2026-11-06"
    assert next_werktag("2026-11-07") == "2026-11-09"  # Sat -> Mon
