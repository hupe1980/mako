"""GeLi Gas end to end — the track, the Frist shape, and the wire differences.

The toolkit publishes five families, and Gas differs from Strom in three ways a
Strom-only suite never touches: UTILMD runs a parallel **release track** (`G…`
against `S…`) on the same date, the answer Frist runs to the **end of the n-th
Werktag** rather than to a clock time on it, and a Gas answer names **no
Codeliste** in `SG4 STS+E01` DE 1131 where a GPKE answer names its EBD.

Every one of those is a silent difference: a Gas message built on the Strom
track validates against the wrong profile, an end-of-day Frist asserted as a
clock time is wrong by hours, and a DE 1131 the Gas MIG does not require is a
segment component no conformant receiver expects.
"""

from __future__ import annotations

import pytest

from conftest import MELO, ON
from makotest import (
    MarktpartnerSim,
    UtilmdTransaction,
    antwort_codes,
    antwort_obligation,
    assert_answer_pid,
    assert_deadline_is,
    assert_edifact_valid,
    assert_frist_met,
    build_interchange,
    build_utilmd,
    release_for,
    validate_edifact,
)
from makotest.plugin import LF_ID, NB_ID

RECEIVED = "2026-03-02T09:00:00Z"


def gas_anmeldung(pid: int = 44001, *, dar: str = "G1") -> bytes:
    """A GeLi Gas Anmeldung NN (Lieferbeginn), LF → NB."""
    message = build_utilmd(
        pid,
        LF_ID,
        NB_ID,
        on=ON,
        message_ref="G1",
        transactions=[
            UtilmdTransaction(
                "VORGANG-G1",
                locations=[("melo", MELO)],
                dates=[("92", "20260501")],
            )
        ],
    )
    return build_interchange(
        sender=LF_ID, receiver=NB_ID, dar=dar, messages=[message], on=ON
    )


def nb() -> MarktpartnerSim:
    return MarktpartnerSim(mp_id=NB_ID, rolle="NB", reference_date=ON)


class TestTrack:
    def test_the_sparte_is_derived_from_the_pid_band(self):
        """44xxx is Gas and 55xxx is Strom, so a caller need not repeat it."""
        report = validate_edifact(gas_anmeldung(), ON)
        assert report.messages[0].release.startswith("G")
        assert report.messages[0].rules_applied

    def test_the_two_tracks_carry_different_releases_on_one_date(self):
        """The reason `sparte` exists at all: one date, two active profiles."""
        strom = release_for("UTILMD", ON, "STROM")
        gas = release_for("UTILMD", ON, "GAS")
        assert strom.startswith("S") and gas.startswith("G")
        assert strom != gas

    def test_the_answer_stays_on_the_gas_track(self):
        """A Gas answer built against the Strom profile would report a mismatch."""
        sim = nb()
        sim.on(44001).bestaetigung(antwort_code="E15", ebd="E_3007")
        reply = sim.receive(gas_anmeldung())
        answer = assert_edifact_valid(reply.business, on=ON)
        assert answer.messages[0].release.startswith("G")


class TestFrist:
    def test_the_window_runs_to_the_end_of_the_nth_werktag(self):
        """Not a clock time on it — that is the GPKE shape, and it is hours off."""
        obligation = antwort_obligation(44001)
        assert obligation.family == "geli-gas"
        assert obligation.shape == "end_of_werktag"
        assert obligation.clock_time is None
        assert obligation.window == "4 WT"

        due = obligation.due_at(RECEIVED)
        assert due.startswith("2026-03-06T23:59:59"), due

    def test_the_two_families_do_not_share_an_instant(self):
        strom = antwort_obligation(55001).due_at(RECEIVED)
        gas = antwort_obligation(44001).due_at(RECEIVED)
        assert strom != gas

    def test_the_simulator_reports_the_gas_window(self):
        sim = nb()
        sim.on(44001).bestaetigung(antwort_code="E15", ebd="E_3007")
        reply = sim.receive(gas_anmeldung(), received_at=RECEIVED)

        assert_deadline_is(reply.due_at, received=RECEIVED, pid=44001)
        assert_frist_met(44001, received=RECEIVED, answered_at=reply.answered_at)


class TestAntwort:
    def test_the_answer_pids_come_from_the_gas_table(self):
        sim = nb()
        sim.on(44001).bestaetigung(antwort_code="E15", ebd="E_3007")
        reply = sim.receive(gas_anmeldung())
        assert_answer_pid(reply.pid, anfrage=44001, accepted=True)
        assert reply.pid == 44002

    def test_a_gas_answer_names_no_codeliste_on_the_wire(self):
        """DE 1131 carries the Codeliste, and the Gas MIG does not require one.

        The Strom counterpart writes its EBD there, so a simulator that treated
        the two alike would emit a component the Gas receiver never expects.
        """
        sim = nb()
        sim.on(44001).ablehnung(antwort_code="A03")
        reply = sim.receive(gas_anmeldung())
        vorgang = validate_edifact(reply.business, ON).messages[0].vorgaenge[0]

        assert vorgang.antwort_code == "A03"
        assert vorgang.antwort_codeliste is None
        assert_edifact_valid(reply.business, on=ON)

    def test_gas_splits_vorpruefung_from_lieferbeginn_like_strom(self):
        """`E_3005` refuses; `E_3007` decides the Lieferbeginn.

        The same two-tree shape as `E_0622` / `E_0623`, so confirming a 44001
        means naming the second — and the answer-Frist table names the first.
        """
        assert antwort_obligation(44001).ebd == "E_3005"
        assert all(c.ist_zustimmung is False for c in antwort_codes("E_3005"))
        assert [c.code for c in antwort_codes("E_3007") if c.ist_zustimmung] == [
            "E15",
            "Z01",
            "Z43",
            "Z44",
        ]

    def test_a_strom_code_is_refused_on_a_gas_process(self):
        """`A01` is published by neither Gas tree, however ordinary it looks."""
        with pytest.raises(ValueError, match="E_3005 does not publish"):
            nb().on(44001).ablehnung(antwort_code="A01")


class TestVorgangState:
    def test_a_repeat_gas_anmeldung_meets_the_open_vorgang(self):
        """The register is Sparte-neutral: it keys on the Lokation."""
        sim = nb()
        sim.on(44001).bestaetigung(antwort_code="E15", ebd="E_3007")
        sim.on(44001).bei_offenem_vorgang().ablehnung(antwort_code="Z35")

        assert sim.receive(gas_anmeldung(dar="G1")).pid == 44002
        second = sim.receive(gas_anmeldung(dar="G2"))
        assert second.pid == 44003
        assert second.antwort_code == "Z35"
        assert_edifact_valid(second.business, on=ON)

    def test_z35_obliges_a_third_party_status_not_a_bemerkung(self):
        """Gas Bedingung `[84]` names `Z35`, but it governs the second `STS`.

        „Wenn SG4 STS+E01++Z35 vorhanden" makes the `SG4 STS` „Status der
        Antwort des dritten Marktbeteiligten" segment Muss — the Gas twin of
        Strom's `[356]` on `A50`. The `SG4 FTX` „Bemerkung" is governed by `[48]`
        and names only `E14`, so a bare `Z35` is a complete Ablehnung as far as
        the Bemerkung goes.
        """
        from makotest import antwort_code

        assert antwort_code("E_3005", "Z35").braucht_bemerkung is False
        assert antwort_code("E_3005", "E14").braucht_bemerkung is True

        wire = build_interchange(
            sender=NB_ID,
            receiver=LF_ID,
            dar="Z1",
            messages=[
                build_utilmd(
                    44003,
                    NB_ID,
                    LF_ID,
                    on=ON,
                    transactions=[
                        UtilmdTransaction("V-G1", antwort_code="Z35", antwort_ebd=None)
                    ],
                )
            ],
            on=ON,
        )
        assert_edifact_valid(wire, on=ON)
