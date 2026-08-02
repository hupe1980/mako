"""The EPEX generator — determinism and the negative-price path."""

from makotest.generators import EpexSim


def test_day_yields_one_point_per_mtu():
    assert len(list(EpexSim(mtu_minutes=15).day("2026-11-01"))) == 96
    assert len(list(EpexSim(mtu_minutes=60).day("2026-11-01"))) == 24


def test_same_seed_reproduces_the_curve():
    a = list(EpexSim(seed=42).day("2026-11-01", profile="winter_peak"))
    b = list(EpexSim(seed=42).day("2026-11-01", profile="winter_peak"))
    assert [p.avg_ct_kwh for p in a] == [p.avg_ct_kwh for p in b]


def test_different_seeds_differ():
    a = [p.avg_ct_kwh for p in EpexSim(seed=1).day("2026-11-01")]
    b = [p.avg_ct_kwh for p in EpexSim(seed=2).day("2026-11-01")]
    assert a != b


def test_day_order_does_not_affect_output():
    """Asking for a later day first must not change an earlier day's curve."""
    sim = EpexSim(seed=7)
    first = [p.avg_ct_kwh for p in sim.day("2026-11-01")]
    _ = list(sim.day("2026-11-02"))
    again = [p.avg_ct_kwh for p in sim.day("2026-11-01")]
    assert first == again


def test_negative_hours_produce_negative_prices():
    """§51 EEG and §41a dynamic tariffs both need this path exercised."""
    pts = list(EpexSim(seed=3).day("2026-06-21", profile="solar_glut", negative_hours=6))
    negatives = [p for p in pts if p.avg_ct_kwh < 0]
    assert len(negatives) == 6 * 4  # 6 hours at 15-minute resolution


def test_ingest_row_shape():
    p = next(iter(EpexSim(seed=1).day("2026-11-01")))
    row = p.as_ingest_row()
    assert set(row) == {"mtu_start", "price_date", "mtu_minutes", "avg_ct_kwh", "source"}
    assert row["mtu_minutes"] in (15, 60)


def test_invalid_mtu_is_rejected():
    import pytest

    with pytest.raises(ValueError, match="15 or 60"):
        EpexSim(mtu_minutes=30)
