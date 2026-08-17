"""Build → validate round-trips.

The load-bearing property: a message `makotest` builds must survive the same
AHB validation the platform applies on ingest. If build and validate ever
disagree, every test written on top of the builder is meaningless.
"""

import pytest

from makotest import (
    UtilmdTransaction,
    build_interchange,
    build_mscons,
    build_utilmd,
    validate_edifact,
)


def test_utilmd_build_roundtrips_through_validation():
    raw = build_utilmd(
        pruefidentifikator=55001,
        sender="4012345000023",
        receiver="9900357000004",
        release="S2.1",
        message_ref="MSG-001",
        document_date="20251101",
        transactions=[
            UtilmdTransaction(
                object_type="melo",
                object_id="DE00014559929E00856996N5139699L01",
                process_dates=[("163", "20251101")],
                references=[("Z13", "55001")],
            )
        ],
    )
    report = validate_edifact(raw, "2025-10-01")
    assert report.pruefidentifikator == 55001
    assert report.message_type == "UTILMD"


def test_mscons_build_roundtrips_through_validation():
    raw = build_mscons(
        pruefidentifikator=13003,
        sender="4012345000023",
        receiver="9900357000004",
        metering_point="51238696012",
        quantities=[("220", "1234.567", "KWH")],
        release="2.4c",
        document_date="20251101",
    )
    report = validate_edifact(raw, "2025-10-01")
    assert report.pruefidentifikator == 13003
    assert report.message_type == "MSCONS"


def test_builders_emit_a_message_not_an_interchange():
    """`build_*` returns UNH..UNT. Sending it needs the UNB/UNZ envelope."""
    raw = build_mscons(
        pruefidentifikator=13003,
        sender="4012345000023",
        receiver="9900357000004",
        metering_point="51238696012",
        quantities=[("220", "100", "KWH")],
    )
    text = raw.decode("latin-1")
    assert text.startswith("UNH+")
    assert "UNB+" not in text


def test_interchange_envelope_makes_it_sendable():
    msg = build_mscons(
        pruefidentifikator=13003,
        sender="4012345000023",
        receiver="9900357000004",
        metering_point="51238696012",
        quantities=[("220", "100", "KWH")],
    )
    wire = build_interchange(
        sender="4012345000023",
        receiver="9900357000004",
        dar="REF001",
        messages=[msg],
        date="260802",
        time="0915",
    )
    text = wire.decode("latin-1")
    # DE0007 per AF 6.1d: 4012345000023 is a GLN -> 14;
    # 9900357000004 starts with 99 -> BDEW -> 500.
    assert text.startswith("UNB+UNOC:3+4012345000023:14+9900357000004:500+260802:0915+REF001'")
    assert text.endswith("UNZ+1+REF001'")


def test_interchange_roundtrips_through_validation():
    """The envelope must not break parsing — this is the full wire unit."""
    msg = build_mscons(
        pruefidentifikator=13003,
        sender="4012345000023",
        receiver="9900357000004",
        metering_point="51238696012",
        quantities=[("220", "1234.567", "KWH")],
        release="2.4c",
        document_date="20251101",
    )
    wire = build_interchange("4012345000023", "9900357000004", "R1", [msg])
    report = validate_edifact(wire, "2025-10-01")
    assert report.pruefidentifikator == 13003


def test_unz_count_reflects_multiple_messages():
    msg = build_mscons(
        pruefidentifikator=13003,
        sender="4012345000023",
        receiver="9900357000004",
        metering_point="51238696012",
        quantities=[("220", "100", "KWH")],
    )
    wire = build_interchange("4012345000023", "9900357000004", "R1", [msg, msg])
    assert wire.decode("latin-1").endswith("UNZ+2+R1'")


def test_obis_code_is_validated_not_passed_through():
    with pytest.raises(ValueError, match="invalid OBIS"):
        build_mscons(
            pruefidentifikator=13003,
            sender="4012345000023",
            receiver="9900357000004",
            metering_point="51238696012",
            quantities=[("220", "100", "KWH")],
            obis="not-an-obis-code",
        )


def test_unknown_object_type_is_rejected_with_the_valid_set():
    with pytest.raises(ValueError, match="unknown object_type"):
        build_utilmd(
            pruefidentifikator=55001,
            sender="4012345000023",
            receiver="9900357000004",
            transactions=[UtilmdTransaction(object_type="widget", object_id="X")],
        )


def test_invalid_pruefidentifikator_is_rejected():
    with pytest.raises(ValueError):
        build_utilmd(pruefidentifikator=1, sender="A", receiver="B")
