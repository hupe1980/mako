"""Hypothesis strategies over the domain types.

The property every one of these asserts: a drawn value is one the platform
**accepts**. A strategy that generated structurally-shaped but check-digit-invalid
identifiers would make every property test a test of the rejection path.
"""

from __future__ import annotations

import pytest

hypothesis = pytest.importorskip(
    "hypothesis", reason="makotest[hypothesis] not installed"
)

from hypothesis import HealthCheck, given, settings

from conftest import ON
from makotest import (
    bilanzierungsgebiet_is_valid,
    bilanzkreis_is_valid,
    eic_is_valid,
    eic_type_char,
    is_werktag,
    malo_is_valid,
    melo_is_valid,
    message_types_of,
    mp_id_check_digit_schemes,
    mp_id_unb_qualifier,
    pid_has_ahb_rules,
    resource_id_is_valid,
)
from makotest.strategies import (
    antwort_pids,
    bilanzierungsgebiete,
    bilanzkreise,
    malo_ids,
    marktpartner_ids,
    melo_ids,
    pruefidentifikatoren,
    resource_ids,
    werktage,
    zeitreihen,
)

_settings = settings(max_examples=50, deadline=None)


class TestIdentifiers:
    @given(malo=malo_ids())
    @_settings
    def test_every_generated_malo_is_check_digit_valid(self, malo):
        """The whole reason this lives in the library.

        A random 11-digit string is a valid MaLo one time in ten, so a
        hand-rolled strategy would spend nine tenths of its budget on the
        rejection path and prove nothing.
        """
        assert malo_is_valid(malo)
        assert len(malo) == 11

    @given(melo=melo_ids())
    @_settings
    def test_every_generated_melo_is_well_formed(self, melo):
        assert melo_is_valid(melo)
        assert len(melo) == 33 and melo.startswith("DE")

    @given(melo=melo_ids(country="AT"))
    @_settings
    def test_the_country_prefix_is_honoured(self, melo):
        assert melo.startswith("AT")

    def test_an_invalid_country_code_is_rejected(self):
        with pytest.raises(ValueError, match="2-letter ISO code"):
            melo_ids(country="DEU")

    @pytest.mark.parametrize(
        ("kind", "scheme", "qualifier"),
        [("bdew", "bdew", "500"), ("dvgw", "bdew", "502"), ("gln", "gln", "14")],
    )
    def test_every_generated_mp_id_carries_a_valid_check_digit(
        self, kind, scheme, qualifier
    ):
        """§2.3 defines two procedures, and an invented ID satisfies neither."""

        @given(mp=marktpartner_ids(kind=kind))
        @_settings
        def check(mp):
            assert len(mp) == 13 and mp.isdigit()
            # A code can satisfy both procedures; what matters is that the one
            # this kind was drawn under accepts it.
            assert scheme in mp_id_check_digit_schemes(mp)
            assert mp_id_unb_qualifier(mp) == qualifier

        check()

    def test_an_unknown_marktpartner_kind_is_rejected(self):
        with pytest.raises(ValueError, match="kind must be one of"):
            marktpartner_ids(kind="nosuch")

    @given(eic=bilanzkreise())
    @_settings
    def test_a_bilanzkreis_is_a_party_with_a_real_check_character(self, eic):
        assert len(eic) == 16
        assert eic_type_char(eic) == "X"
        assert eic_is_valid(eic) and bilanzkreis_is_valid(eic)

    @given(eic=bilanzierungsgebiete())
    @_settings
    def test_a_bilanzierungsgebiet_is_an_area_not_a_party(self, eic):
        """The object-type character is the only thing separating the two.

        A series filed against the wrong one is a misfiling the BIKO cannot tell
        from a correct submission, so the two never share a strategy.
        """
        assert eic_type_char(eic) == "Y"
        assert bilanzierungsgebiet_is_valid(eic)
        assert not bilanzkreis_is_valid(eic)

    @pytest.mark.parametrize("kind", ["nelo", "nebe", "tr", "sr", "sg", "cr", "paket"])
    def test_every_resource_id_family_draws_valid_values(self, kind):
        @given(value=resource_ids(kind=kind))
        @settings(max_examples=20, deadline=None)
        def check(value):
            assert len(value) == 11
            assert resource_id_is_valid(kind, value)

        check()


class TestPruefidentifikatoren:
    @given(pid=pruefidentifikatoren(message_type="UTILMD"))
    @_settings
    def test_only_pids_with_real_ahb_rules_are_drawn(self, pid):
        """A PID without rules validates vacuously — never generate one."""
        assert pid_has_ahb_rules("UTILMD", pid)

    @given(pid=pruefidentifikatoren(message_type="UTILMD", sparte="STROM", on=ON))
    @_settings
    def test_dating_the_pool_keeps_it_inside_the_active_profile(self, pid):
        assert 55000 <= pid <= 55999
        assert pid_has_ahb_rules("UTILMD", pid, on=ON, sparte="STROM")

    @given(pid=pruefidentifikatoren(message_type="UTILMD", sparte="GAS"))
    @_settings
    def test_sparte_gas_restricts_to_the_44xxx_band(self, pid):
        assert 44000 <= pid <= 44999

    @given(pid=pruefidentifikatoren())
    @_settings
    def test_the_unrestricted_pool_spans_message_types(self, pid):
        assert message_types_of(pid) != []

    def test_the_unrestricted_pool_covers_every_pid_carrying_type(self):
        """A hand-kept subset would silently never draw an IFTSTA or QUOTES PID.

        The docstring promises "PIDs the compiled profile set actually
        validates", so the default has to be asked of the build rather than
        listed — a property drawing from two thirds of the catalogue while
        claiming all of it is the vacuous-coverage failure in another shape.
        """
        from makotest import pid_carrying_message_types

        covered = {
            mt
            for pid in pruefidentifikatoren().__dict__["elements"]
            for mt in message_types_of(pid)
        }
        assert covered == set(pid_carrying_message_types())

    @given(pid=antwort_pids())
    @_settings
    def test_every_antwort_pid_has_a_published_window(self, pid):
        from makotest import antwort_obligation

        assert antwort_obligation(pid) is not None

    def test_an_unknown_sparte_is_rejected(self):
        with pytest.raises(ValueError, match='"STROM" or "GAS"'):
            pruefidentifikatoren(message_type="UTILMD", sparte="WASSER")

    def test_a_message_type_with_no_pids_is_an_error_not_an_empty_draw(self):
        """CONTRL has no PIDs; drawing from nothing must fail loudly."""
        with pytest.raises(ValueError, match="nothing to draw from"):
            pruefidentifikatoren(message_type="CONTRL")


class TestTimeAndMeasurements:
    @given(tag=werktage(min_date="2026-01-01", max_date="2026-12-31"))
    @settings(
        max_examples=25,
        deadline=None,
        suppress_health_check=[HealthCheck.filter_too_much],
    )
    def test_every_generated_date_is_a_werktag(self, tag):
        assert is_werktag(tag)

    def test_an_inverted_date_range_is_rejected(self):
        with pytest.raises(ValueError, match="is after"):
            werktage(min_date="2027-01-01", max_date="2026-01-01")

    @given(series=zeitreihen(periods=96))
    @_settings
    def test_a_series_has_one_value_per_mtu(self, series):
        assert len(series) == 96
        assert all(v >= 0.0 for v in series)

    @pytest.mark.parametrize(
        ("tag", "mtus"), [("2026-03-29", 92), ("2026-06-21", 96), ("2026-10-25", 100)]
    )
    def test_a_dated_series_is_as_long_as_its_own_delivery_day(self, tag, mtus):
        """Two Europe/Berlin days a year are not 24 hours long.

        A series fixed at 96 invents four periods in March and drops four in
        October, mid-day, where it still looks plausible.
        """

        @given(series=zeitreihen(on=tag))
        @settings(max_examples=5, deadline=None)
        def check(series):
            assert len(series) == mtus

        check()

    def test_a_non_positive_period_count_is_rejected(self):
        with pytest.raises(ValueError, match="periods must be >= 1"):
            zeitreihen(periods=0)

    def test_a_length_has_to_come_from_exactly_one_place(self):
        with pytest.raises(ValueError, match="exactly one of"):
            zeitreihen()
        with pytest.raises(ValueError, match="exactly one of"):
            zeitreihen(periods=96, on="2026-06-21")
