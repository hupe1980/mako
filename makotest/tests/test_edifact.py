"""EDIFACT validation through the platform's own AHB engine."""

import pytest

from makotest import validate_edifact
from makotest.assertions import assert_edifact_valid, assert_rule_fires

# A minimal MSCONS carrying a valid 11-digit Marktlokations-ID in LOC+172.
MSCONS_VALID = (
    b"UNB+UNOC:3+4012345000023:14+9900357000004:14+190101:0000+1'"
    b"UNH+1+MSCONS:D:04B:UN:2.4c'"
    b"BGM+7:::+00013003::+9'"
    b"DTM+137:20230101:102'"
    b"RFF+ACE:REF001'"
    b"NAD+MS+4012345000023::293'"
    b"UNS+D'"
    b"LOC+172+51238696781'"
    b"QTY+220:100:KWH'"
    b"UNT+9+1'"
    b"UNZ+1+1'"
)

# The same message with a Meldepunkt matching neither location-ID scheme.
MSCONS_BAD_LOCATION = MSCONS_VALID.replace(b"LOC+172+51238696781'", b"LOC+172+BADID'")

# LOC+172 is the Meldepunkt and may carry a 33-character Messlokations-ID —
# this value is the MSCONS MIG 2.5's own worked example for the segment.
MSCONS_MELO = MSCONS_VALID.replace(
    b"LOC+172+51238696781'", b"LOC+172+DE00014559929E00856996N5139699L01'"
)


def test_valid_mscons_passes():
    assert_edifact_valid(MSCONS_VALID, on="2025-10-01")


def test_report_exposes_pid_and_type():
    report = validate_edifact(MSCONS_VALID, "2025-10-01")
    assert report.pruefidentifikator == 13003
    assert report.message_type == "MSCONS"


def test_bad_location_id_fires_the_location_rule():
    assert_rule_fires(MSCONS_BAD_LOCATION, "SEM-MSCONS-LOCATION-FORMAT", on="2025-10-01")


def test_33_char_melo_in_loc172_is_accepted():
    """A MeLo in the Meldepunkt is legal — the qualifier fixes the role, not the scheme."""
    assert_edifact_valid(MSCONS_MELO, on="2025-10-01")


def test_unparseable_input_raises_rather_than_reporting():
    with pytest.raises(ValueError, match="parse failed"):
        validate_edifact(b"this is not EDIFACT at all")


def test_assert_rule_fires_reports_what_did_fire():
    """A wrong rule name must fail loudly, listing the rules that did fire."""
    with pytest.raises(AssertionError, match="expected rule"):
        assert_rule_fires(MSCONS_BAD_LOCATION, "SEM-NOT-A-REAL-RULE", on="2025-10-01")
