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
    # The column pairs the EBD with the Abonnement: `E_0254` answers an
    # Abbestellung (`IMD Z02`), `E_0256` a Bestellung (`Z01`).
    kw.setdefault("abonnement", "Z02" if ebd == "E_0254" else "Z01")
    message = build_ordrsp(
        pid,
        NB_ID,
        LF_ID,
        antwort_code=code,
        antwort_ebd=ebd,
        on=ON,
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

    def test_an_answer_without_a_code_still_carries_its_muss_ajt(self):
        """`SG2 AJT` is Muss on 19011: the answer states a Prüfschritt either way.

        A caller who names none gets the column's placeholder, and the message
        still validates — the builder completes what the column requires.
        """
        message = build_ordrsp(
            WERTEBESTELLUNG_PID,
            NB_ID,
            LF_ID,
            on=ON,
            abonnement="Z01",
            line_item=True,
            item_description=True,
        )
        assert b"AJT+" in message
        wire = build_interchange(
            sender=NB_ID, receiver=LF_ID, dar="O3", messages=[message], on=ON
        )
        assert_edifact_valid(wire, on=ON)


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


class TestCompletion:
    def test_a_pid_needing_an_ansprechpartner_gets_one(self):
        """19002 makes `CTA` and `COM` Muss; the builder exposes neither.

        The message is completed to its column, so the Ansprechpartner the
        builder cannot name is filled in and the answer validates.
        """
        # 19002 answers out of `S_0068` (Strom) or `G_0074` (Gas).
        message = build_ordrsp(
            19002, NB_ID, LF_ID, antwort_code="A01", antwort_ebd="S_0068", on=ON
        )
        assert b"CTA+" in message and b"COM+" in message
        wire = build_interchange(
            sender=NB_ID, receiver=LF_ID, dar="O2", messages=[message], on=ON
        )
        assert_edifact_valid(wire, on=ON)
