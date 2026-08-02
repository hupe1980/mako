"""The pytest plugin's fixtures — loaded via the pytest11 entry point."""

from makotest.generators import EpexSim


def test_frozen_clock_advances_by_werktage(frozen_clock):
    # Tue 2026-11-03 + 4 WT -> Mon 2026-11-09 (skips Sat/Sun).
    assert frozen_clock.date == "2026-11-03"
    frozen_clock.advance_werktage(4)
    assert frozen_clock.date == "2026-11-09"


def test_epex_sim_fixture_is_seeded(epex_sim, makotest_seed):
    assert isinstance(epex_sim, EpexSim)
    a = [p.avg_ct_kwh for p in epex_sim.day("2026-11-01")]
    b = [p.avg_ct_kwh for p in EpexSim(seed=makotest_seed).day("2026-11-01")]
    assert a == b, "same seed must reproduce the curve exactly"


def test_regulatory_marker_is_registered(pytestconfig):
    markers = pytestconfig.getini("markers")
    assert any(m.startswith("regulatory(") for m in markers)
    assert any(m.startswith("requires_docker") for m in markers)
