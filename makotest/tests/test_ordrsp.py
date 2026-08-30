"""The ORDERS answer — the third wire an Antwortcode travels on.

WiM and ESA answer an ORDERS with an ORDRSP, and the reason rides `SG2 AJT`:
DE 4465 the Prüfschritt code, DE 1082 the Entscheidungsbaum it belongs to. That
is the same pair as REMADV's `AJT` and UTILMD's `SG4 STS+E01` — three wires, one
catalogue — and the EBD is not decoration: the same numeric code lives in
several trees, so a code without its tree resolves to nothing.
"""

from __future__ import annotations

import pytest

from conftest import ON
from makotest import (
    antwort_code,
    antwort_codes,
    assert_antwort_code,
    assert_edifact_valid,
    build_interchange,
    build_ordrsp,
    validate_edifact,
)
from makotest.plugin import LF_ID, NB_ID

#: ESA Wertebestellung answer — the PID whose AHB needs the LIN/IMD segments.
WERTEBESTELLUNG_PID = 19011


def antwort(code: str, ebd: str, *, pid: int = WERTEBESTELLUNG_PID, **kw) -> bytes:
    message = build_ordrsp(
        pid,
        NB_ID,
        LF_ID,
        antwort_code=code,
        antwort_ebd=ebd,
        on=ON,
        abonnement="Z01",
        line_item=True,
        item_description=True,
        **kw,
    )
    return build_interchange(
        sender=NB_ID, receiver=LF_ID, dar="O1", messages=[message], on=ON
    )


class TestAdjustment:
    def test_the_answer_states_its_code_and_its_tree(self):
        wire = antwort("A01", "E_0254")
        assert "AJT+A01+E_0254" in wire.decode()
        assert_edifact_valid(wire, on=ON)

    def test_the_message_is_an_ordrsp_the_ahb_really_checks(self):
        report = validate_edifact(antwort("A01", "E_0254"), ON)
        message = report.messages[0]
        assert message.message_type == "ORDRSP"
        assert message.pruefidentifikator == WERTEBESTELLUNG_PID
        assert message.rules_applied, "a vacuous pass would prove nothing"

    def test_a_code_without_its_tree_is_refused(self):
        """DE 1082 is what makes DE 4465 resolvable, so neither travels alone."""
        with pytest.raises(ValueError, match="both antwort_code and antwort_ebd"):
            build_ordrsp(WERTEBESTELLUNG_PID, NB_ID, LF_ID, antwort_code="A01", on=ON)
        with pytest.raises(ValueError, match="both antwort_code and antwort_ebd"):
            build_ordrsp(WERTEBESTELLUNG_PID, NB_ID, LF_ID, antwort_ebd="E_0254", on=ON)

    def test_an_answer_may_carry_no_adjustment_at_all(self):
        """Not every ORDRSP refuses something."""
        message = build_ordrsp(
            WERTEBESTELLUNG_PID,
            NB_ID,
            LF_ID,
            on=ON,
            abonnement="Z01",
            line_item=True,
            item_description=True,
        )
        assert b"AJT+" not in message


class TestOneCatalogueThreeWires:
    def test_the_ordrsp_code_resolves_against_the_same_catalogue(self):
        """`AJT` here, `AJT` on a REMADV, `STS+E01` on a UTILMD — one lookup."""
        resolved = antwort_code("E_0254", "A01")
        assert resolved is not None and resolved.ist_zustimmung is False
        assert_antwort_code("A01", ebd="E_0254", accepted=False)

    def test_the_tree_decides_what_the_code_means(self):
        """`A01` is published by several trees with unrelated meanings."""
        esa = antwort_code("E_0254", "A01")
        abmeldung = antwort_code("E_0607", "A01")
        assert esa is not None and abmeldung is not None
        assert esa.bedeutung != abmeldung.bedeutung

    def test_the_tree_publishes_both_sides(self):
        codes = antwort_codes("E_0254")
        assert any(c.ist_zustimmung for c in codes)
        assert any(c.ist_zustimmung is False for c in codes)


class TestKnownLimitation:
    def test_a_pid_needing_an_ansprechpartner_cannot_be_built_conformantly(self):
        """19002 makes `CTA` and `COM` Muss, and the builder exposes neither.

        Pinned so the limitation is visible rather than surprising: the message
        builds and the AHB names exactly what is missing, so a test reaching for
        this PID gets a precise answer instead of a puzzling one. The gap is in
        the builder's surface, not in the profile.
        """
        message = build_ordrsp(
            19002, NB_ID, LF_ID, antwort_code="A01", antwort_ebd="E_0254", on=ON
        )
        wire = build_interchange(
            sender=NB_ID, receiver=LF_ID, dar="O2", messages=[message], on=ON
        )
        report = validate_edifact(wire, ON)
        assert not report.is_valid
        assert {f.rule_id for f in report.errors} == {
            "AHB-19002-CTA-M",
            "AHB-19002-COM-M",
        }
