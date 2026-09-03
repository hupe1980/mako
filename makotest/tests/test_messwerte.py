"""The meter-data chain: curve → gateway → MSCONS → counterparty.

The link this pins is the one that is easy to get wrong and impossible to see:
interval data has to carry its **measurement period**. A bare `QTY` states a
magnitude with no time reference, so the receiver cannot place it on the
settlement grid — and the AHB does **not** reject it, so a Lastgang assembled
from flat quantities validates while being unusable. That is the toolkit's
central failure mode wearing meter-data clothes, and the only defence is making
the correct form the one a test naturally reaches for.
"""

from __future__ import annotations

import pytest

from conftest import ON
from makotest import (
    ImsysSim,
    assert_edifact_valid,
    build_interchange,
    build_mscons,
)
from makotest.generators import LastgangGenerator
from makotest.plugin import IMSYS_MELO, LF_ID, NB_ID

#: „Lastgang Marktlokation, Tranche" (MSB → LF, GPKE Teil 4).
LASTGANG_PID = 13025
MALO = "51238696012"


def gateway(tag: str = "2026-11-01") -> ImsysSim:
    return ImsysSim(melo_id=IMSYS_MELO, taf="TAF-7", today=tag)


def delivered(tag: str = "2026-11-01", **kw):
    smgw = gateway(tag)
    werte = LastgangGenerator(seed=3).day(tag)
    return smgw.deliver(tag, werte=werte, **kw)


def wrap(message: bytes, *, dar: str = "M1") -> bytes:
    return build_interchange(
        sender=NB_ID, receiver=LF_ID, dar=dar, messages=[message], on=ON
    )


class TestIntervalQuantities:
    def test_a_lastgang_carries_one_measurement_period_per_interval(self):
        gang = delivered()
        wire = wrap(
            gang.as_mscons(
                pruefidentifikator=LASTGANG_PID,
                sender_mp_id=NB_ID,
                receiver_mp_id=LF_ID,
                on=ON,
                malo_id=MALO,
            )
        )
        assert_edifact_valid(wire, on=ON)

        text = wire.decode()
        assert text.count("QTY+") == gang.mtu_count
        # Each QTY states its start and end; the 13025 column adds the SG6
        # Übertragungszeitraum (`DTM+163`/`164` once, before the positions).
        assert text.count("DTM+163") == gang.mtu_count + 1, "each QTY states its start"
        assert text.count("DTM+164") == gang.mtu_count + 1, "and its end"

    def test_a_bare_quantity_on_a_lastgang_is_refused(self):
        """A Lastgang value without its period cannot be placed on the grid.

        The 13025 column makes the SG10 Messperiode Muss on every QTY; filling
        one in would be inventing settlement data, so the builder refuses and
        names `intervals=` instead.
        """
        with pytest.raises(ValueError, match="intervals="):
            build_mscons(
                LASTGANG_PID,
                NB_ID,
                LF_ID,
                MALO,
                [("220", "1234.567", "KWH")],
                on=ON,
                obis="1-0:1.29.0",
            )

    def test_an_mscons_with_no_quantity_at_all_is_refused(self):
        """A delivery that delivers nothing is a test defect, not a message."""
        with pytest.raises(ValueError, match="at least one quantity"):
            build_mscons(LASTGANG_PID, NB_ID, LF_ID, MALO, on=ON)


class TestTheBerlinDay:
    @pytest.mark.parametrize(
        ("tag", "expected"),
        [("2026-03-29", 92), ("2026-06-21", 96), ("2026-10-25", 100)],
    )
    def test_the_series_length_follows_the_local_day(self, tag, expected):
        gang = delivered(tag)
        wire = wrap(
            gang.as_mscons(
                pruefidentifikator=LASTGANG_PID,
                sender_mp_id=NB_ID,
                receiver_mp_id=LF_ID,
                on=ON,
                malo_id=MALO,
            )
        )
        assert wire.decode().count("QTY+") == expected
        assert_edifact_valid(wire, on=ON)

    def test_the_periods_are_format_303_with_an_offset(self):
        """A zone-less `303` is malformed, so the offset is part of the value."""
        gang = delivered("2026-10-25")
        text = gang.as_mscons(
            pruefidentifikator=LASTGANG_PID,
            sender_mp_id=NB_ID,
            receiver_mp_id=LF_ID,
            on=ON,
            malo_id=MALO,
        ).decode()
        # The 25-hour day starts at 22:00 UTC the previous day (CEST, +02:00).
        assert "DTM+163:202610242200?+00:303" in text

    def test_the_bound_formatter_and_the_builder_agree(self):
        """One renderer for a regulated wire format, not two.

        `format_303` is bound rather than written in Python precisely so it
        cannot drift from what the builders emit — this compares the two on the
        same instant, through a real message.
        """
        from makotest import berlin_day_bounds, format_303

        start, _ = berlin_day_bounds("2026-10-25")
        gang = delivered("2026-10-25")
        text = gang.as_mscons(
            pruefidentifikator=LASTGANG_PID,
            sender_mp_id=NB_ID,
            receiver_mp_id=LF_ID,
            on=ON,
            malo_id=MALO,
        ).decode()
        # The builder escapes `+` with the EDIFACT release character.
        assert f"DTM+163:{format_303(start).replace('+', '?+')}:303" in text

    def test_the_first_period_starts_where_the_berlin_day_does(self):
        gang = delivered("2026-06-21")
        text = gang.as_mscons(
            pruefidentifikator=LASTGANG_PID,
            sender_mp_id=NB_ID,
            receiver_mp_id=LF_ID,
            on=ON,
            malo_id=MALO,
        ).decode()
        assert "DTM+163:202606202200?+00:303" in text


class TestGaps:
    def test_a_luecke_is_an_absent_interval_not_a_zero(self):
        """Substitution is the path being tested; a zero would be settled."""
        gang = delivered(luecken=[41, 42])
        text = gang.as_mscons(
            pruefidentifikator=LASTGANG_PID,
            sender_mp_id=NB_ID,
            receiver_mp_id=LF_ID,
            on=ON,
            malo_id=MALO,
        ).decode()
        assert text.count("QTY+") == gang.mtu_count - 2
        assert not gang.vollstaendig

    def test_an_all_gap_series_is_refused_rather_than_sent_empty(self):
        tag = "2026-11-01"
        smgw = gateway(tag)
        gang = smgw.deliver(
            tag, werte=LastgangGenerator(seed=3).day(tag), luecken=list(range(96))
        )
        with pytest.raises(ValueError, match="no value to report"):
            gang.as_mscons(
                pruefidentifikator=LASTGANG_PID,
                sender_mp_id=NB_ID,
                receiver_mp_id=LF_ID,
                on=ON,
            )


class TestLokation:
    def test_the_series_defaults_to_the_gateways_messlokation(self):
        """13018 reports a Lastgang against the Messlokation."""
        gang = delivered()
        text = gang.as_mscons(
            pruefidentifikator=13018,
            sender_mp_id=NB_ID,
            receiver_mp_id=LF_ID,
            on=ON,
        ).decode()
        assert IMSYS_MELO in text

    def test_a_marktlokation_series_names_it_instead(self):
        """13025 is „Lastgang Marktlokation, Tranche"."""
        gang = delivered()
        text = gang.as_mscons(
            pruefidentifikator=LASTGANG_PID,
            sender_mp_id=NB_ID,
            receiver_mp_id=LF_ID,
            on=ON,
            malo_id=MALO,
        ).decode()
        assert MALO in text and IMSYS_MELO not in text


class TestChain:
    def test_the_whole_meter_data_chain_holds_together(self):
        """Curve → gateway → MSCONS → validated interchange, in one pass."""
        tag = "2026-10-25"
        smgw = gateway(tag)
        gang = smgw.deliver(tag, werte=LastgangGenerator(seed=42).day(tag))

        wire = wrap(
            gang.as_mscons(
                pruefidentifikator=LASTGANG_PID,
                sender_mp_id=NB_ID,
                receiver_mp_id=LF_ID,
                on=ON,
                malo_id=MALO,
            )
        )
        report = assert_edifact_valid(wire, on=ON)
        assert report.messages[0].pruefidentifikator == LASTGANG_PID
        assert report.messages[0].message_type == "MSCONS"
        assert gang.mtu_count == 100
