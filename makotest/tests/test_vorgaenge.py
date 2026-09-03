"""Vorgang state — what a counterparty remembers between requests.

A Netzbetreiber does not answer two Anmeldungen for one Marktlokation the same
way: the second meets a Vorgang already in Bearbeitung, and `E_0622` publishes
`A06` for exactly that. Without that memory a platform that re-sends a request
it has already had confirmed is never contradicted, and the test still passes.
"""

from __future__ import annotations

import pytest
from hypothesis import HealthCheck, settings
from hypothesis.stateful import RuleBasedStateMachine, invariant, precondition, rule

from conftest import MALO, ON, utilmd_interchange
from makotest import (
    MarktpartnerSim,
    assert_edifact_valid,
    validate_edifact,
)
from makotest.plugin import NB_ID


def nb() -> MarktpartnerSim:
    sim = MarktpartnerSim(mp_id=NB_ID, rolle="NB", reference_date=ON)
    sim.on(55001).bestaetigung(antwort_code="A51", ebd="E_0623")
    sim.on(55001).bei_offenem_vorgang().ablehnung(
        antwort_code="A06", process_dates=[("Z07", "20260501")]
    )
    return sim


class TestVorgangOnTheWire:
    """Reading an answer back structurally, rather than matching bytes."""

    def test_a_parsed_message_exposes_its_sg4_vorgaenge(self):
        report = validate_edifact(utilmd_interchange(55001), ON)
        vorgang = report.messages[0].vorgaenge[0]
        assert vorgang.vorgangsnummer == "VORGANG-1"
        assert vorgang.location("malo") == MALO
        # Format 303 on the wire, so the raw value carries time and zone.
        assert vorgang.date("92") == "202605010000+00"
        assert vorgang.iso_date("92") == "2026-05-01"
        assert vorgang.reference("Z13") == "55001"

    def test_a_lokation_is_asked_for_by_type_not_by_position(self):
        """`LOC+Z16` and `LOC+Z17` differ by one character.

        A substring check over the raw bytes passes on the wrong one, which is
        why the Vorgang resolves the qualifier rather than the caller.
        """
        vorgang = validate_edifact(utilmd_interchange(55001), ON).messages[0].vorgaenge[0]
        assert vorgang.location("malo") == MALO
        assert vorgang.location("melo") is None

    def test_a_message_type_with_no_vorgaenge_reports_none(self):
        sim = nb()
        reply = sim.receive(utilmd_interchange(55001))
        ack = validate_edifact(reply.ack, ON)
        assert ack.messages[0].vorgaenge == []


class TestDuplicateAnmeldung:
    def test_a_repeat_request_meets_the_open_vorgang(self):
        sim = nb()
        first = sim.receive(utilmd_interchange(55001, dar="R1"))
        second = sim.receive(utilmd_interchange(55001, dar="R2"))

        assert (first.pid, first.antwort_code) == (55002, "A51")
        assert (second.pid, second.antwort_code) == (55003, "A06")
        assert_edifact_valid(second.business, on=ON)

    def test_the_confirmation_is_what_opens_the_vorgang(self):
        sim = nb()
        assert len(sim.vorgaenge) == 0
        sim.receive(utilmd_interchange(55001))
        assert [v.lokation for v in sim.vorgaenge.offene] == [MALO]
        assert sim.vorgaenge.offen(MALO).antwort_pid == 55002

    def test_a_refusal_leaves_the_lokation_free(self):
        """A counterparty that held it would refuse the corrected resubmission.

        The refusal is the reason to resend, so holding the Lokation after one
        would make every correction unanswerable.
        """
        sim = MarktpartnerSim(mp_id=NB_ID, rolle="NB", reference_date=ON)
        sim.on(55001).ablehnung(antwort_code="A06", process_dates=[("Z07", "20260501")])
        sim.receive(utilmd_interchange(55001))
        assert len(sim.vorgaenge) == 0

    def test_closing_the_vorgang_restores_the_first_request_answer(self):
        sim = nb()
        sim.receive(utilmd_interchange(55001, dar="R1"))
        assert sim.vorgaenge.schliessen(MALO) is not None
        assert sim.receive(utilmd_interchange(55001, dar="R2")).pid == 55002

    def test_a_different_lokation_is_a_first_request(self):
        """The register is keyed per Lokation, not per counterparty."""
        other = "51238696781"
        sim = nb()
        sim.receive(utilmd_interchange(55001, dar="R1"))
        reply = sim.receive(utilmd_interchange(55001, lokation=other, dar="R2"))
        assert reply.pid == 55002

    def test_binding_only_the_repeat_case_still_answers_the_first(self):
        """Falling back is what keeps a partial binding from answering nothing."""
        sim = MarktpartnerSim(mp_id=NB_ID, rolle="NB", reference_date=ON)
        sim.on(55001).bei_offenem_vorgang().ablehnung(
            antwort_code="A06", process_dates=[("Z07", "20260501")]
        )
        reply = sim.receive(utilmd_interchange(55001))
        assert reply.business is None, "no unconditional binding to answer with"
        assert reply.ack_kind == "CONTRL"

    def test_the_register_is_keyed_on_the_lokation_not_the_vorgangsnummer(self):
        """A duplicate carries a *new* Vorgangsnummer — it is the sender's ref.

        Keying on it would make every duplicate look like a first request, which
        is precisely the case this exists to catch.
        """
        sim = nb()
        sim.receive(utilmd_interchange(55001, dar="R1"))
        # Same Lokation, and `utilmd_interchange` always writes VORGANG-1, so
        # the discriminating field has to be the Lokation either way.
        assert sim.receive(utilmd_interchange(55001, dar="R2")).pid == 55003


class VorgangLifecycle(RuleBasedStateMachine):
    """The register must never contradict the answers it produced.

    A model over the (Anmeldung → Bestätigung/Ablehnung → Storno) cycle: the
    invariant is that a Lokation is held **iff** the last answer for it was a
    Bestätigung, and that the answer PID always matches what the register said
    before the request.
    """

    def __init__(self) -> None:
        super().__init__()
        self.sim = nb()
        self.dar = 0

    def _send(self, melo: str) -> tuple[int | None, bool]:
        self.dar += 1
        occupied = self.sim.vorgaenge.offen(melo) is not None
        request = utilmd_interchange(55001, lokation=melo, dar=f"R{self.dar}")
        reply = self.sim.receive(request)
        return reply.pid, occupied

    @rule()
    def anmeldung(self) -> None:
        pid, was_occupied = self._send(MALO)
        # Occupied → the repeat binding refuses; free → the first one confirms.
        assert pid == (55003 if was_occupied else 55002)

    @rule()
    @precondition(lambda self: len(self.sim.vorgaenge) > 0)
    def storno(self) -> None:
        self.sim.vorgaenge.schliessen(MALO)

    @invariant()
    def a_held_lokation_was_confirmed(self) -> None:
        for vorgang in self.sim.vorgaenge.offene:
            assert vorgang.antwort_pid == 55002, "only a Bestätigung holds a Lokation"
            assert vorgang.pid == 55001

    @invariant()
    def the_register_never_double_books(self) -> None:
        lokationen = [v.lokation for v in self.sim.vorgaenge.offene]
        assert len(lokationen) == len(set(lokationen))


TestVorgangLifecycle = VorgangLifecycle.TestCase
TestVorgangLifecycle.settings = settings(
    max_examples=25,
    deadline=None,
    # Building and validating EDIFACT is orders of magnitude slower than drawing
    # an integer, which is what the data-generation health check is written for.
    suppress_health_check=[HealthCheck.data_too_large, HealthCheck.too_slow],
)


@pytest.mark.regulatory("BK6-24-174 GPKE Teil 2 — EBD E_0622 Prüfschritt 70")
def test_the_duplicate_refusal_cites_the_published_code():
    """`A06` is „Andere Anmeldung in Bearbeitung", and the AHB obliges its DTM."""
    sim = nb()
    sim.receive(utilmd_interchange(55001, dar="R1"))
    reply = sim.receive(utilmd_interchange(55001, dar="R2"))
    vorgang = validate_edifact(reply.business, ON).messages[0].vorgaenge[0]
    assert vorgang.antwort_code == "A06"
    assert vorgang.antwort_codeliste == "E_0622"
    assert vorgang.iso_date("Z07") == "2026-05-01"
