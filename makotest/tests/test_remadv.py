"""The invoice answer — REMADV, and the code that says why.

An INVOIC is answered with a REMADV: a Zahlungsavis that confirms it and states
what will be transferred, or a Rückmeldung that refuses it. The refusal rides
`AJT` — DE 4465 the Antwortcode, DE 1082 the EBD it is drawn from — which is
the REMADV twin of UTILMD's `STS+E01` and required for the same reason: a
rejection without its code gives the sender nothing to correct.

The tree is `E_0406`, and it is the same catalogue the UTILMD answers resolve
against, so a code is checked the same way on both wires.
"""

from __future__ import annotations

import pytest

from conftest import ON
from makotest import (
    Positionsfehler,
    antwort_code,
    antwort_codes,
    assert_antwort_code,
    assert_edifact_valid,
    build_interchange,
    build_remadv,
    validate_edifact,
)
from makotest.plugin import LF_ID, NB_ID

#: Strom rejection at Kopf und Summe (33003); the position-level one is 33004.
RUECKMELDUNG_PID = 33003
POSITIONS_PID = 33004
RECHNUNG = dict(
    rechnungsnummer="RE-2026-001",
    faelliger_betrag="1234.56",
    rechnungsdatum="202604010000",
)


def rueckmeldung(pid: int = RUECKMELDUNG_PID, **kw) -> bytes:
    """A REMADV refusing an invoice — nothing is transferred."""
    message = build_remadv(
        pid,
        LF_ID,
        NB_ID,
        ueberweisungsbetrag="0",
        on=ON,
        **RECHNUNG,
        **kw,
    )
    return build_interchange(
        sender=LF_ID, receiver=NB_ID, dar="R1", messages=[message], on=ON
    )


class TestRueckmeldung:
    def test_the_refusal_states_its_code_and_its_tree(self):
        wire = rueckmeldung(kopf_gruende=[("A70", "E_0406")])
        assert "AJT+A70+E_0406" in wire.decode()
        assert_edifact_valid(wire, on=ON)

    def test_the_answer_is_a_remadv_the_ahb_really_checks(self):
        report = validate_edifact(rueckmeldung(kopf_gruende=[("A70", "E_0406")]), ON)
        message = report.messages[0]
        assert message.message_type == "REMADV"
        assert message.pruefidentifikator == RUECKMELDUNG_PID
        assert message.rules_applied, "a vacuous pass would prove nothing"

    def test_a_refusal_transfers_nothing(self):
        """`MOA+12` is `0` on an Abweisung — refusing an invoice moves no money."""
        text = rueckmeldung(kopf_gruende=[("A70", "E_0406")]).decode()
        assert "MOA+12:0" in text

    def test_the_answered_invoice_is_named(self):
        """The issuer correlates on the invoice's own document number."""
        assert "RE-2026-001" in rueckmeldung(kopf_gruende=[("A70", "E_0406")]).decode()


class TestPositionsfehler:
    def test_a_refused_position_carries_its_own_reason(self):
        wire = rueckmeldung(
            POSITIONS_PID,
            positionsfehler=[Positionsfehler(1, [("A70", "E_0406")], "Summenpruefung")],
        )
        text = wire.decode()
        assert text.count("AJT+") == 1, "on the position; 33004 carries no Kopf-AJT"
        assert "DLI+1" in text
        assert_edifact_valid(wire, on=ON)

    def test_every_refused_position_gets_its_own_group(self):
        """`SG10` repeats up to 9999 times — one per refused Rechnungsposition.

        REMADV MIG 2.9e segment 0410 makes „Rückmeldungen auf Positionsebene"
        `C … 9999`, which is the whole point of the itemized rejection 33004.
        The groups sit *before* `UNS`, so an order check that treated the
        header as a flat sequence rejected every second `DLI`.
        """
        for count in (1, 2, 5):
            wire = rueckmeldung(
                POSITIONS_PID,
                positionsfehler=[
                    Positionsfehler(n + 1, [("A70", "E_0406")]) for n in range(count)
                ],
            )
            report = validate_edifact(wire, ON)
            assert report.is_valid, [f.rule_id for f in report.errors]
            assert wire.decode().count("DLI+") == count


class TestTheCodeCatalogue:
    def test_the_ajt_code_resolves_against_the_same_catalogue(self):
        """One catalogue for both wires: `AJT` here, `STS+E01` on a UTILMD."""
        resolved = antwort_code("E_0406", "A70")
        assert resolved is not None
        assert resolved.ist_zustimmung is False
        assert_antwort_code("A70", ebd="E_0406", accepted=False)

    def test_a_code_from_another_tree_is_caught(self):
        """`A70` means the Netznutzungs-Summenprüfung *here* and nothing there."""
        assert antwort_code("E_0622", "A70") is None
        with pytest.raises(AssertionError, match="not an Antwortcode"):
            assert_antwort_code("A70", ebd="E_0622")

    def test_the_catch_all_codes_want_an_erlaeuterung(self):
        """`A96` and `A99` are „Sonstiges" — bare, they say nothing correctable."""
        wants = {c.code for c in antwort_codes("E_0406") if c.braucht_bemerkung}
        assert wants == {"A96", "A99"}


class TestZahlungsavis:
    def test_a_confirmation_transfers_the_amount_due(self):
        """`380` Handelsrechnung, and the Überweisungsbetrag is the fällige."""
        message = build_remadv(
            33001,
            LF_ID,
            NB_ID,
            ueberweisungsbetrag="1234.56",
            dokumentenart="380",
            on=ON,
            **RECHNUNG,
        )
        wire = build_interchange(
            sender=LF_ID, receiver=NB_ID, dar="Z1", messages=[message], on=ON
        )
        text = wire.decode()
        assert "MOA+12:1234.56" in text
        assert "AJT+" not in text, "a confirmation refuses nothing"
        assert validate_edifact(wire, ON).messages[0].message_type == "REMADV"
