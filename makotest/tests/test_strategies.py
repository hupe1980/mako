"""Hypothesis strategies over the domain types."""

from __future__ import annotations

import pytest

hypothesis = pytest.importorskip("hypothesis", reason="makotest[hypothesis] not installed")

from hypothesis import HealthCheck, given, settings  # noqa: E402

from makotest import malo_is_valid, melo_is_valid, message_types_of, pid_has_ahb_rules  # noqa: E402
from makotest.strategies import (  # noqa: E402
    bilanzierungsgebiete,
    malo_ids,
    marktpartner_ids,
    melo_ids,
    pruefidentifikatoren,
    werktage,
    zeitreihen,
)

# The strategies call into Rust; the default deadline is tight for the first
# call while the profile registry initialises.
_settings = settings(max_examples=50, deadline=None)


class TestIdentifiers:
    @given(malo=malo_ids())
    @_settings
    def test_every_generated_malo_is_check_digit_valid(self, malo):
        """The whole reason this lives in the library.

        A random 11-digit string is almost never a valid MaLo, so a hand-rolled
        strategy would exercise the rejection path and prove nothing.
        """
        assert malo_is_valid(malo)
        assert len(malo) == 11

    @given(melo=melo_ids())
    @_settings
    def test_every_generated_melo_is_well_formed(self, melo):
        assert melo_is_valid(melo)
        assert len(melo) == 33
        assert melo.startswith("DE")

    @given(melo=melo_ids(country="AT"))
    @_settings
    def test_the_country_prefix_is_honoured(self, melo):
        assert melo.startswith("AT")

    def test_an_invalid_country_code_is_rejected(self):
        with pytest.raises(ValueError, match="2-letter ISO code"):
            melo_ids(country="DEU")

    @given(mp=marktpartner_ids(kind="bdew"))
    @_settings
    def test_bdew_codes_are_13_digits_starting_99(self, mp):
        """The prefix decides the UNB DE0007 qualifier, so it must be exact."""
        assert len(mp) == 13
        assert mp.startswith("99")
        assert mp.isdigit()

    @given(mp=marktpartner_ids(kind="dvgw"))
    @_settings
    def test_dvgw_codes_start_98(self, mp):
        assert mp.startswith("98")

    def test_an_unknown_marktpartner_kind_is_rejected(self):
        with pytest.raises(ValueError, match="kind must be one of"):
            marktpartner_ids(kind="nosuch")

    @given(eic=bilanzierungsgebiete())
    @_settings
    def test_bilanzierungsgebiet_eics_are_16_chars_with_an_x(self, eic):
        assert len(eic) == 16
        assert eic[2] == "X"


class TestPruefidentifikatoren:
    @given(pid=pruefidentifikatoren(message_type="UTILMD"))
    @_settings
    def test_only_pids_with_real_ahb_rules_are_drawn(self, pid):
        """A PID without rules validates vacuously — never generate one."""
        assert pid_has_ahb_rules("UTILMD", pid)

    @given(pid=pruefidentifikatoren(message_type="UTILMD", sparte="STROM"))
    @_settings
    def test_sparte_strom_restricts_to_the_55xxx_band(self, pid):
        assert 55000 <= pid <= 55999

    @given(pid=pruefidentifikatoren(message_type="UTILMD", sparte="GAS"))
    @_settings
    def test_sparte_gas_restricts_to_the_44xxx_band(self, pid):
        assert 44000 <= pid <= 44999

    @given(pid=pruefidentifikatoren())
    @_settings
    def test_the_unrestricted_pool_spans_message_types(self, pid):
        assert message_types_of(pid) != []

    def test_an_unknown_sparte_is_rejected(self):
        with pytest.raises(ValueError, match="sparte must be STROM or GAS"):
            pruefidentifikatoren(message_type="UTILMD", sparte="WASSER")

    def test_a_message_type_with_no_pids_is_an_error_not_an_empty_draw(self):
        """CONTRL has no PIDs; drawing from nothing must fail loudly."""
        with pytest.raises(ValueError, match="nothing to draw from"):
            pruefidentifikatoren(message_type="CONTRL")


class TestTimeAndMeasurements:
    @given(tag=werktage(min_date="2026-01-01", max_date="2026-12-31"))
    @settings(max_examples=25, deadline=None, suppress_health_check=[HealthCheck.filter_too_much])
    def test_every_generated_date_is_a_werktag(self, tag):
        from makotest import is_werktag

        assert is_werktag(tag)

    def test_an_inverted_date_range_is_rejected(self):
        with pytest.raises(ValueError, match="is after"):
            werktage(min_date="2027-01-01", max_date="2026-01-01")

    @given(series=zeitreihen(periods=96))
    @_settings
    def test_a_series_has_one_value_per_mtu(self, series):
        assert len(series) == 96
        assert all(v >= 0.0 for v in series)

    def test_a_non_positive_period_count_is_rejected(self):
        with pytest.raises(ValueError, match="periods must be >= 1"):
            zeitreihen(periods=0)
