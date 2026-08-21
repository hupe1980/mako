"""The pytest plugin's fixtures — loaded via the pytest11 entry point."""

import pytest

from makotest import mp_id_check_digit_schemes, validate_edifact
from makotest.generators import EpexGenerator
from makotest.plugin import BIKO_ID, DEFAULT_REFERENCE_DATE, LF_ID, NB_ID


def test_frozen_clock_is_anchored_to_the_session_date(frozen_clock, makotest_on):
    """One day, not two: the clock and the format version describe the same date."""
    from makotest import next_werktag

    assert frozen_clock.date == next_werktag(makotest_on)


def test_frozen_clock_advances_by_werktage():
    from makotest.plugin import FrozenClock

    # Tue 2026-11-03 + 4 WT -> Mon 2026-11-09 (skips Sat/Sun).
    clock = FrozenClock("2026-11-03T09:00:00")
    assert clock.date == "2026-11-03"
    clock.advance_werktage(4)
    assert clock.date == "2026-11-09"


def test_the_clock_resolves_the_berlin_offset_of_the_date_it_lands_on():
    """A German wall clock is +01:00 for part of the year and +02:00 for the rest.

    A clock that kept the offset it was constructed with would report 09:00+01:00
    after crossing into summer time — an hour off the wall clock it claims, in
    the same direction for every deadline the test then asserts.
    """
    from makotest.plugin import FrozenClock

    clock = FrozenClock("2026-03-27T09:00:00")  # Friday, CET
    assert clock.instant == "2026-03-27T09:00:00+01:00"
    clock.advance_werktage(2)  # -> Tuesday 31 March, CEST
    assert clock.instant == "2026-03-31T09:00:00+02:00"
    clock.advance(days=-4)
    assert clock.instant == "2026-03-27T09:00:00+01:00"


def test_epex_fixture_is_seeded(epex, makotest_seed):
    assert isinstance(epex, EpexGenerator)
    a = [p.avg_ct_kwh for p in epex.day("2026-11-01")]
    b = [p.avg_ct_kwh for p in EpexGenerator(seed=makotest_seed).day("2026-11-01")]
    assert a == b, "same seed must reproduce the curve exactly"


def test_the_reference_date_is_a_session_input(makotest_on):
    """`--makotest-on` pins the format version for the whole suite."""
    assert makotest_on == DEFAULT_REFERENCE_DATE


def test_the_simulators_share_the_session_reference_date(nb_sim, biko_sim, makotest_on):
    assert nb_sim.reference_date == makotest_on
    assert biko_sim.reference_date == makotest_on


@pytest.mark.parametrize("mp_id", [NB_ID, LF_ID, BIKO_ID])
def test_every_fixture_identifier_carries_a_valid_check_digit(mp_id):
    """A fixture is the first place a reader copies from.

    An invented Marktpartner-ID is exactly the defect this toolkit exists to
    catch, so its own fixtures must not carry one.
    """
    assert mp_id_check_digit_schemes(mp_id), f"{mp_id} satisfies no procedure"


def test_the_default_reference_date_covers_every_message_type_built(makotest_on):
    """CONTRL only enters the compiled set on FV2026-01-01.

    A simulator dated earlier cannot acknowledge anything, which is a confusing
    failure to hit from a default.
    """
    from makotest import release_for

    for message_type in ("UTILMD", "MSCONS", "APERAK", "CONTRL"):
        sparte = "STROM" if message_type == "UTILMD" else None
        assert release_for(message_type, makotest_on, sparte) is not None, message_type


def test_a_simulator_reply_validates_on_the_session_date(nb_sim, anmeldung, makotest_on):
    nb_sim.on(55001).bestaetigung()
    reply = nb_sim.receive(anmeldung)
    assert validate_edifact(reply.business, makotest_on).is_valid


def test_markers_are_registered(pytestconfig):
    markers = pytestconfig.getini("markers")
    assert any(m.startswith("regulatory(") for m in markers)
    assert any(m.startswith("requires_docker") for m in markers)
