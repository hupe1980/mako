"""Counterparty simulators.

The property that matters: a reply is a **rendered interchange the platform can
be fed**. Everything else here is about the unhappy paths — the wrong
acknowledgement, the misrouted submission, the dead control path, and silence.
"""

import pytest

from conftest import MELO, ON, utilmd_interchange
from makotest import (
    BikoSim,
    ImsysSim,
    Klaerfall,
    MarktpartnerSim,
    UtilmdTransaction,
    assert_edifact_valid,
    assert_frist_met,
    build_interchange,
    build_mscons,
    build_utilmd,
    validate_edifact,
)
from makotest.plugin import BIKO_ID, LF_ID, NB_ID
from makotest.simulators import STEUERUNG_TAF


def mscons_interchange(pid: int = 13003, *, sender: str = NB_ID) -> bytes:
    message = build_mscons(
        pid,
        sender,
        BIKO_ID,
        metering_point="51238696012",
        quantities=[("220", "1234.567", "KWH")],
        on=ON,
    )
    return build_interchange(
        sender=sender, receiver=BIKO_ID, dar="MS1", messages=[message], on=ON
    )


@pytest.fixture
def nb() -> MarktpartnerSim:
    return MarktpartnerSim(mp_id=NB_ID, rolle="NB", reference_date=ON)


class TestMarktpartnerSim:
    def test_the_reply_is_edifact_the_platform_can_be_fed(self, nb, anmeldung):
        """The whole point. A dict answer cannot close the loop."""
        nb.on(55001).bestaetigung(process_dates=[("163", "20260501")])
        reply = nb.receive(anmeldung)

        assert reply.pid == 55002, "Bestätigung Anmeldung is 55002"
        assert_edifact_valid(reply.ack, on=ON)
        answer = assert_edifact_valid(reply.business, on=ON)
        assert answer.pruefidentifikator == 55002
        assert answer.envelope.sender_id == NB_ID, "the parties swap"
        assert answer.envelope.receiver_id == LF_ID

    def test_a_conformant_partner_acknowledges_before_it_answers(self, nb, anmeldung):
        nb.on(55001).bestaetigung()
        reply = nb.receive(anmeldung)
        assert reply.ack_kind == "CONTRL"
        assert reply.ack_positive

    def test_the_ablehnung_uses_the_ahb_answer_pid(self, nb, anmeldung):
        nb.on(55001).ablehnung(erc="A06")
        reply = nb.receive(anmeldung)
        assert reply.pid == 55003
        assert reply.erc == "A06"
        assert_edifact_valid(reply.business, on=ON)

    def test_timeout_answers_nothing_at_all(self, nb, anmeldung):
        """The Frist path: no acknowledgement either, or the platform sees liveness."""
        nb.on(55001).timeout()
        reply = nb.receive(anmeldung)
        assert not reply
        assert reply.ack is None and reply.business is None
        assert nb.exchanges[-1].request_pid == 55001

    def test_an_unconfigured_partner_sends_no_business_answer(self, nb, anmeldung):
        """Forgetting to bind an answer must not fabricate one."""
        reply = nb.receive(anmeldung)
        assert reply.business is None
        assert reply.ack is not None, "it still acknowledges what it received"

    def test_the_reply_carries_the_published_frist(self, nb, anmeldung):
        """GPKE 55001: 11:00 on the 1. Werktag after the Übertragungstag."""
        nb.on(55001).bestaetigung()
        received = "2026-03-02T09:00:00Z"
        reply = nb.receive(anmeldung, received_at=received)
        assert reply.due_at == "2026-03-03T11:00:00+01:00"
        assert_frist_met(55001, received=received, answered_at=reply.due_at)

    @pytest.mark.parametrize("binder", ["bestaetigung", "timeout"])
    def test_binding_an_answer_pid_is_rejected_however_it_is_bound(self, nb, binder):
        """55002 answers 55001; it is not something a partner can be asked.

        `.timeout()` and `.antwort()` do not consult the answer table, so the
        refusal has to sit on `.on()` — otherwise those two bind a rule that can
        never fire and the test silently exercises nothing.
        """
        with pytest.raises(ValueError, match="is an answer to 55001"):
            getattr(nb.on(55002), binder)()

    def test_an_explicit_answer_on_an_answer_pid_is_rejected_too(self, nb):
        with pytest.raises(ValueError, match="is an answer to 55001"):
            nb.on(55003).antwort(pid=55002)

    def test_44020_cannot_be_rejected(self, nb):
        """The asymmetric family must not offer an Ablehnung."""
        nb.on(44020).bestaetigung()
        with pytest.raises(ValueError, match="never rejectable"):
            nb.on(44020).ablehnung()

    def test_an_explicit_pid_produces_an_adversarial_answer(self, nb, anmeldung):
        """A counterparty replying with the wrong PID is a thing that happens."""
        nb.on(55001).antwort(pid=55006)
        assert nb.receive(anmeldung).pid == 55006

    def test_an_explicit_pid_cannot_also_be_a_timeout(self, nb):
        """The binding would say both "answer with this" and "answer nothing"."""
        with pytest.raises(ValueError, match="carries no PID"):
            nb.on(55001).antwort(pid=55002, modus="timeout")

    def test_a_bare_message_is_refused_with_a_reason(self, nb):
        from makotest import build_utilmd

        with pytest.raises(ValueError, match="build_interchange"):
            nb.receive(build_utilmd(55001, LF_ID, NB_ID, on=ON))


class TestInterchangeIdentity:
    """UNB DE0020 identifies the interchange to the receiver.

    A partner that reused one would be sending a duplicate every conformant
    receiver is entitled to discard, so a scenario whose second exchange looks
    like a replay of the first cannot be run against a platform that checks.
    """

    def test_every_reply_carries_its_own_datenaustauschreferenz(self, nb):
        nb.on(55001).bestaetigung()
        first = nb.receive(utilmd_interchange(dar="REQ1"))
        second = nb.receive(utilmd_interchange(dar="REQ2"))
        refs = {
            validate_edifact(wire, ON).envelope.control_ref
            for wire in (first.ack, first.business, second.ack, second.business)
        }
        assert len(refs) == 4, f"four interchanges, four references: {refs}"

    def test_the_references_are_reproducible_across_runs(self):
        """A counter, not a clock — so a scenario stays byte-reproducible."""

        def run() -> bytes:
            sim = MarktpartnerSim(mp_id=NB_ID, rolle="NB", reference_date=ON)
            sim.on(55001).bestaetigung()
            return sim.receive(utilmd_interchange()).business

        assert run() == run()


class TestMultiMessageInterchange:
    """An interchange carries several messages, each its own Vorgang.

    Answering only the first and calling that the answer to the interchange is
    how a broken second one ships.
    """

    #: `(PID, BGM DE 1001)` — the AHB fixes the document code per process, and
    #: 55004 Abmeldung is `E0F` where 55001 Anmeldung is `E01`.
    VORGAENGE = ((55001, "E01"), (55004, "E0F"))

    def two_vorgaenge(self) -> bytes:
        return build_interchange(
            sender=LF_ID,
            receiver=NB_ID,
            dar="MULTI1",
            on=ON,
            messages=[
                build_utilmd(
                    pid,
                    LF_ID,
                    NB_ID,
                    on=ON,
                    message_ref=str(index + 1),
                    document_code=document_code,
                    transactions=[
                        UtilmdTransaction(
                            "melo",
                            MELO,
                            process_dates=[("163", "20260501")],
                            references=[("Z13", str(pid))],
                        )
                    ],
                )
                for index, (pid, document_code) in enumerate(self.VORGAENGE)
            ],
        )

    def test_every_bound_message_is_answered(self, nb):
        nb.on(55001).bestaetigung()
        nb.on(55004).bestaetigung()
        reply = nb.receive(self.two_vorgaenge())

        assert reply.pids == (55002, 55005)
        answer = assert_edifact_valid(reply.business, on=ON)
        assert [m.pruefidentifikator for m in answer.messages] == [55002, 55005]

    def test_the_single_message_accessors_refuse_to_speak_for_two(self, nb):
        nb.on(55001).bestaetigung()
        nb.on(55004).ablehnung(erc="A06")
        reply = nb.receive(self.two_vorgaenge())
        with pytest.raises(ValueError, match=r"read `pids`"):
            _ = reply.pid
        assert reply.ercs == (None, "A06")

    def test_an_unbound_second_message_leaves_the_first_answered(self, nb):
        nb.on(55001).bestaetigung()
        reply = nb.receive(self.two_vorgaenge())
        assert reply.pids == (55002,)
        assert nb.exchanges[-1].request_pids == (55001, 55004)


class TestLateAnswer:
    """A conformant message sent after the Frist is a thing that happens.

    The platform has to notice, and it cannot if the simulator can only answer
    instantly.
    """

    def test_a_delayed_answer_lands_after_the_published_deadline(self, nb):
        nb.on(55001).bestaetigung(delay_werktage=3)
        received = "2026-03-02T09:00:00Z"
        reply = nb.receive(utilmd_interchange(), received_at=received)

        assert reply.due_at == "2026-03-03T11:00:00+01:00"
        assert reply.answered_at > reply.due_at
        with pytest.raises(AssertionError, match="was late"):
            assert_frist_met(55001, received=received, answered_at=reply.answered_at)

    def test_an_undelayed_answer_is_sent_when_the_request_arrives(self, nb):
        nb.on(55001).bestaetigung()
        received = "2026-03-02T09:00:00Z"
        reply = nb.receive(utilmd_interchange(), received_at=received)
        assert reply.answered_at == received
        assert_frist_met(55001, received=received, answered_at=reply.answered_at)


class TestAcknowledgementChoice:
    """A CONTRL reports a syntax failure; an APERAK an application one.

    They tell the counterparty to retry different things, so answering either
    with the other is a defect a platform should not have to tolerate.
    """

    def test_an_ahb_violation_earns_an_aperak(self, nb):
        wire = utilmd_interchange(melo="NOTAMELO")
        reply = nb.receive(wire)
        assert reply.ack_kind == "APERAK"
        assert not reply.ack_positive
        assert b"BGM+313" in reply.ack
        assert reply.business is None, "a strict partner does not answer what it refused"

    def test_a_lenient_partner_can_be_made_to_answer_anyway(self):
        """Some counterparties do. A platform has to survive it."""
        lenient = MarktpartnerSim(
            mp_id=NB_ID, rolle="NB", reference_date=ON, strict=False
        )
        assert not lenient.receive(utilmd_interchange(melo="NOTAMELO"))


class TestBikoSim:
    def test_a_conformant_summenzeitreihe_is_accepted(self):
        biko = BikoSim(mp_id=BIKO_ID, reference_date=ON)
        reply = biko.receive(mscons_interchange(), bilanzkreis="11XBK-------1")
        assert_edifact_valid(reply.ack, on=ON)
        assert reply.ack_kind == "APERAK" and reply.ack_positive
        assert biko.accepted and not biko.klaerfaelle
        assert b"BGM+312" in reply.ack

    def test_both_simulators_answer_with_the_same_envelope_type(self, nb, anmeldung):
        """One `Reply` shape, so a test never has to remember which is which."""
        nb.on(55001).bestaetigung()
        assert type(nb.receive(anmeldung)) is type(
            BikoSim(mp_id=BIKO_ID, reference_date=ON).receive(mscons_interchange())
        )

    def test_a_klaerfall_comes_back_as_an_error_aperak(self):
        biko = BikoSim(mp_id=BIKO_ID, reference_date=ON)
        biko.reject_next(Klaerfall("summe_weicht_ab", detail="delta 12.4 kWh"))
        first = biko.receive(mscons_interchange(), bilanzkreis="11XBK-------1")
        assert b"BGM+313" in first.ack
        assert not first.ack_positive and first.erc
        assert len(biko.klaerfaelle) == 1

    def test_reject_next_is_queued_not_sticky(self):
        """So the re-submission after Clearing can be asserted — the whole point."""
        biko = BikoSim(mp_id=BIKO_ID, reference_date=ON)
        biko.reject_next(Klaerfall("summe_weicht_ab"))
        biko.receive(mscons_interchange())
        biko.receive(mscons_interchange())
        assert [s.accepted for s in biko.submissions] == [False, True]

    def test_reject_bilanzkreis_is_sticky_for_that_bilanzkreis_only(self):
        biko = BikoSim(mp_id=BIKO_ID, reference_date=ON)
        biko.reject_bilanzkreis("11XBK-------1", Klaerfall("bilanzkreis_unbekannt"))
        biko.receive(mscons_interchange(), bilanzkreis="11XBK-------1")
        biko.receive(mscons_interchange(), bilanzkreis="11XBK-------1")
        biko.receive(mscons_interchange(), bilanzkreis="11XBK-------2")
        assert [s.accepted for s in biko.submissions] == [False, False, True]

    def test_a_utilmd_is_not_a_summenzeitreihe(self, anmeldung):
        """A BIKO that accepted this would agree to something no real one does.

        Every assertion downstream of that acceptance would be meaningless — so
        it is refused as misrouted rather than accepted.
        """
        biko = BikoSim(mp_id=BIKO_ID, reference_date=ON)
        reply = biko.receive(anmeldung)
        assert b"BGM+313" in reply.ack
        assert not reply.ack_positive
        assert biko.submissions[-1].refused
        assert "UTILMD" in biko.submissions[-1].refused

    def test_a_messwesen_mscons_is_not_addressed_to_a_biko(self):
        """13002 is a Gas Zählerstand for a Netzbetreiber, not a MaBiS series."""
        biko = BikoSim(mp_id=BIKO_ID, reference_date=ON)
        biko.receive(mscons_interchange(13002))
        assert "13002" in biko.submissions[-1].refused

    def test_accept_by_default_false_rejects_everything(self):
        biko = BikoSim(mp_id=BIKO_ID, reference_date=ON, accept_by_default=False)
        biko.receive(mscons_interchange())
        assert not biko.submissions[-1].accepted


class TestImsysSim:
    def test_a_fresh_gateway_has_a_valid_certificate(self):
        assert ImsysSim(melo_id=MELO).certificate_state == "valid"

    def test_expiry_inside_the_warn_window_reports_expiring(self):
        smgw = ImsysSim(melo_id=MELO)
        assert smgw.expire_certificate_in(days=5).certificate_state == "expiring"

    def test_a_past_expiry_reports_expired(self):
        smgw = ImsysSim(melo_id=MELO)
        assert smgw.expire_certificate_in(days=-1).certificate_state == "expired"

    def test_revocation_is_distinct_from_expiry(self):
        assert ImsysSim(melo_id=MELO).revoke_certificate().certificate_state == "revoked"

    def test_advancing_the_clock_can_cross_expiry_mid_test(self):
        smgw = ImsysSim(melo_id=MELO)
        smgw.expire_certificate_in(days=10)
        assert smgw.certificate_state == "expiring"
        assert smgw.advance(days=11).certificate_state == "expired"

    def test_an_expired_certificate_yields_no_usable_cls_channel(self):
        """A §14a dispatch must not steer through a dead control path."""
        smgw = ImsysSim(melo_id=MELO, taf=STEUERUNG_TAF)
        smgw.expire_certificate_in(days=-1)
        channel = smgw.open_channel("CLS-1")
        assert not channel.open
        assert channel.last_error == "certificate expired"

    def test_steering_needs_taf_11_not_taf_14(self):
        """TAF-11 is „Steuerung von unterbrechbaren Verbrauchseinrichtungen".

        TAF-14 is „Hochfrequente Messwertbereitstellung für Mehrwertdienste" — a
        fast read-out, not a control path. Ordering it and expecting to steer is
        a configuration error whose symptom is a dispatch that does nothing.
        """
        assert ImsysSim(melo_id=MELO, taf=STEUERUNG_TAF).open_channel("CLS-1").open
        wrong = ImsysSim(melo_id=MELO, taf="TAF-14").open_channel("CLS-1")
        assert not wrong.open
        assert "TAF-11" in wrong.last_error

    def test_delivery_records_gaps_so_ersatzwertbildung_can_be_tested(self):
        smgw = ImsysSim(melo_id=MELO)
        gang = smgw.deliver("2026-11-01", luecken=[42, 41])
        assert gang.luecken == [41, 42], "sorted"
        assert not gang.vollstaendig
        assert smgw.letzte_lieferung is gang

    def test_a_complete_series_reports_vollstaendig(self):
        smgw = ImsysSim(melo_id=MELO)
        assert smgw.deliver("2026-11-01").vollstaendig

    def test_an_expired_gateway_refuses_to_deliver(self):
        smgw = ImsysSim(melo_id=MELO)
        smgw.expire_certificate_in(days=-1)
        with pytest.raises(RuntimeError, match="certificate is expired"):
            smgw.deliver("2026-11-01")

    def test_an_unknown_taf_profile_is_rejected(self):
        with pytest.raises(ValueError, match="unknown TAF profile"):
            ImsysSim(melo_id=MELO, taf="TAF-99")


class TestImsysDelivery:
    """A gateway that cannot deliver in a shape the platform ingests is a stub."""

    def test_a_delivery_renders_as_the_direct_push_payload(self):
        smgw = ImsysSim(melo_id=MELO, today="2026-11-01")
        gang = smgw.deliver("2026-11-01", flat_kwh=1.5)
        push = gang.as_direct_push(sender_mp_id=NB_ID)

        assert len(push["intervals"]) == 96
        assert push["source"] == "DIRECT_PUSH"
        assert push["melo_id"] == MELO
        assert push["intervals"][0]["unit"] == "kWh"

    def test_a_gap_is_an_absent_interval_not_a_zero(self):
        """The whole point of modelling gaps.

        A platform must substitute (Ersatzwertbildung); a zero it was handed is
        a reading it settles against. Filling a gap with 0.0 would make the
        substitution path untestable and bill the customer for nothing.
        """
        gang = ImsysSim(melo_id=MELO).deliver(
            "2026-11-01", flat_kwh=1.5, luecken=[41, 42]
        )
        push = gang.as_direct_push(sender_mp_id=NB_ID)

        assert len(push["intervals"]) == 94
        assert not any(i["value"] == 0 for i in push["intervals"])
        starts = {i["from"] for i in push["intervals"]}
        assert "2026-11-01T09:15:00+00:00" not in starts, "index 41 must be absent"

    def test_the_day_is_a_local_one(self):
        """A Zählerstandsgang covers a Europe/Berlin calendar day, like a curve."""
        gang = ImsysSim(melo_id=MELO).deliver("2026-11-01")
        push = gang.as_direct_push(sender_mp_id=NB_ID)
        assert push["intervals"][0]["from"] == "2026-10-31T23:00:00+00:00"

    def test_the_session_id_is_stable_across_runs(self):
        """It is the idempotency key: a re-submission must be recognised as one."""

        def push() -> dict:
            gang = ImsysSim(melo_id=MELO).deliver("2026-11-01")
            return gang.as_direct_push(sender_mp_id=NB_ID)

        assert push()["session_id"] == push()["session_id"]


class TestZaehlerstandsgangShape:
    """A delivery day is a Europe/Berlin day, and two a year are not 24 hours."""

    @pytest.mark.parametrize(
        ("tag", "mtus"), [("2026-03-29", 92), ("2026-06-21", 96), ("2026-10-25", 100)]
    )
    def test_a_series_is_as_long_as_its_own_day(self, tag, mtus):
        gang = ImsysSim(melo_id=MELO, today=tag).deliver(tag)
        assert gang.mtu_count == mtus
        assert len(gang.as_direct_push(sender_mp_id=NB_ID)["intervals"]) == mtus

    def test_a_96_value_series_on_the_short_day_is_refused(self):
        """Four intervals would otherwise run past midnight into the next day.

        Mid-day, where the series still looks plausible and the settlement is
        quietly wrong.
        """
        smgw = ImsysSim(melo_id=MELO, today="2026-03-29")
        with pytest.raises(ValueError, match="23-hour"):
            smgw.deliver("2026-03-29", werte=[1.0] * 96)

    def test_a_gap_index_outside_the_day_is_refused(self):
        """An index nothing lands on removes no interval, so the gap never exists."""
        with pytest.raises(ValueError, match="lie outside"):
            ImsysSim(melo_id=MELO).deliver("2026-11-01", luecken=[500])

    def test_the_last_interval_ends_at_the_end_of_the_local_day(self):
        gang = ImsysSim(melo_id=MELO, today="2026-10-25").deliver("2026-10-25")
        push = gang.as_direct_push(sender_mp_id=NB_ID)
        assert push["intervals"][-1]["to"] == "2026-10-25T23:00:00+00:00"

    def test_the_marktlokation_is_not_in_the_body(self):
        """It is a path parameter on the endpoints this shape targets."""
        push = (
            ImsysSim(melo_id=MELO)
            .deliver("2026-11-01")
            .as_direct_push(sender_mp_id=NB_ID)
        )
        assert "malo_id" not in push

    def test_values_are_decimal_strings_not_floats(self):
        """Energy is a decimal quantity, and a JSON float carries binary error."""
        push = (
            ImsysSim(melo_id=MELO)
            .deliver("2026-11-01", flat_kwh=0.1)
            .as_direct_push(sender_mp_id=NB_ID)
        )
        assert push["intervals"][0]["value"] == "0.1000"
        assert isinstance(push["intervals"][0]["value"], str)


class TestReadingQuality:
    """A gap and a substituted value are different obligations.

    An Ersatzwert is delivered and billable (§ 60 Abs. 2 MsbG); a FAULTY reading
    is delivered and must not be billed; a gap is not delivered at all. A gateway
    that could only emit unqualified numbers could exercise none of it.
    """

    def test_a_stamped_period_carries_its_quality(self):
        gang = ImsysSim(melo_id=MELO).deliver(
            "2026-11-01", qualitaeten={41: "SUBSTITUTED", 42: "FAULTY"}
        )
        push = gang.as_direct_push(sender_mp_id=NB_ID)
        assert push["intervals"][41]["quality"] == "SUBSTITUTED"
        assert push["intervals"][42]["quality"] == "FAULTY"

    def test_unstamped_periods_carry_no_quality_key(self):
        push = (
            ImsysSim(melo_id=MELO)
            .deliver("2026-11-01", qualitaeten={0: "ESTIMATED"})
            .as_direct_push(sender_mp_id=NB_ID)
        )
        assert "quality" not in push["intervals"][1]

    def test_an_unknown_quality_is_refused(self):
        with pytest.raises(ValueError, match="unknown reading quality"):
            ImsysSim(melo_id=MELO).deliver("2026-11-01", qualitaeten={0: "PERFECT"})

    def test_a_substituted_value_is_present_where_a_gap_is_absent(self):
        substituted = ImsysSim(melo_id=MELO).deliver(
            "2026-11-01", qualitaeten={41: "SUBSTITUTED"}
        )
        gap = ImsysSim(melo_id=MELO).deliver("2026-11-01", luecken=[41])
        assert len(substituted.as_direct_push(sender_mp_id=NB_ID)["intervals"]) == 96
        assert len(gap.as_direct_push(sender_mp_id=NB_ID)["intervals"]) == 95
