"""Counterparty simulators."""

from __future__ import annotations

import pytest

from makotest import (
    BikoSim,
    ImsysSim,
    Klaerfall,
    MarktpartnerSim,
    UtilmdTransaction,
    build_interchange,
    build_utilmd,
)

NB_ID = "9900357000004"
LF_ID = "4012345000023"
MELO = "DE00014559929E00856996N5139699L01"

#: The simulators are exercised on FV2025-10-01, matching the S2.1 release the
#: builder emits — validating an S2.1 message against a later profile would be
#: a different question and would give a different answer.
ON = "2025-10-01"


def utilmd(pid: int, *, sender: str = LF_ID, receiver: str = NB_ID) -> bytes:
    """A structurally valid UTILMD interchange carrying `pid`."""
    msg = build_utilmd(
        pruefidentifikator=pid,
        sender=sender,
        receiver=receiver,
        release="S2.1",
        message_ref="MSG-001",
        document_date="20251101",
        transactions=[
            UtilmdTransaction(
                object_type="melo",
                object_id=MELO,
                process_dates=[("163", "20251101")],
                references=[("Z13", str(pid))],
            )
        ],
    )
    return build_interchange(sender=sender, receiver=receiver, dar="UTILMD", messages=[msg])


class TestMarktpartnerSim:
    def test_bestaetigung_uses_the_ahb_answer_pid(self):
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", strict=False, reference_date=ON)
        nb.on(55001).bestaetigung(zuordnungsbeginn="2026-11-01")

        answer = nb.receive(utilmd(55001))
        assert answer is not None
        assert answer["pid"] == 55002, "Bestätigung Anmeldung is 55002"
        assert answer["modus"] == "bestaetigung"
        assert answer["zuordnungsbeginn"] == "2026-11-01"

    def test_ablehnung_uses_the_ahb_answer_pid_and_carries_the_erc(self):
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", strict=False, reference_date=ON)
        nb.on(55001).ablehnung(erc="A06")

        answer = nb.receive(utilmd(55001))
        assert answer["pid"] == 55003, "Ablehnung Anmeldung is 55003"
        assert answer["erc"] == "A06"

    def test_timeout_answers_nothing_at_all(self):
        """The Frist path: no CONTRL either, or the platform sees liveness."""
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", strict=False, reference_date=ON)
        nb.on(55001).timeout()

        assert nb.receive(utilmd(55001)) is None
        assert nb.exchanges[-1].request_pid == 55001

    def test_an_unconfigured_partner_is_silent(self):
        """Forgetting to bind an answer must not fabricate one."""
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", strict=False, reference_date=ON)
        assert nb.receive(utilmd(55001)) is None

    def test_explicit_antwort_bypasses_the_table_for_adversarial_cases(self):
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", strict=False, reference_date=ON)
        nb.on(55001).antwort(pid=55006, modus="bestaetigung")

        answer = nb.receive(utilmd(55001))
        assert answer["pid"] == 55006, "a wrong-PID answer must be producible"

    def test_binding_an_answer_to_a_non_request_pid_is_rejected(self):
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", reference_date=ON)
        with pytest.raises(ValueError, match="no Bestätigung"):
            nb.on(55002).bestaetigung()

    def test_44020_cannot_be_rejected(self):
        """The asymmetric family must not offer an Ablehnung."""
        gnb = MarktpartnerSim(mp_id=NB_ID, rolle="GNB", reference_date=ON)
        gnb.on(44020).bestaetigung()  # allowed
        with pytest.raises(ValueError, match="no Ablehnung"):
            gnb.on(44020).ablehnung()

    def test_chaining_binds_several_pids(self):
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", strict=False, reference_date=ON)
        nb.on(55001).bestaetigung()
        nb.on(55004).ablehnung(erc="A06")
        assert nb.receive(utilmd(55001))["pid"] == 55002
        assert nb.receive(utilmd(55004))["pid"] == 55006

    def test_exchanges_record_what_was_handled(self):
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", strict=False, reference_date=ON)
        nb.on(55001).bestaetigung()
        nb.receive(utilmd(55001))
        nb.receive(utilmd(55004))
        assert [e.request_pid for e in nb.exchanges] == [55001, 55004]

    def test_unvalidated_requests_flags_vacuous_validation(self):
        """A PID with no AHB rules validates having checked nothing.

        The simulator surfaces that rather than letting the test pass silently.
        """
        nb = MarktpartnerSim(mp_id=NB_ID, rolle="NB", strict=False, reference_date=ON)
        nb.receive(utilmd(55001))
        assert nb.unvalidated_requests == [], "55001 has real AHB rules"


class TestBikoSim:
    def test_a_conformant_submission_is_accepted(self):
        biko = BikoSim(mp_id="9979999000007", reference_date=ON)
        answer = biko.receive(utilmd(55001), bilanzkreis="11XBK-------1")
        assert answer["ack"] is True
        assert biko.klaerfaelle == []

    def test_reject_next_raises_one_klaerfall_then_returns_to_default(self):
        """Queued, not sticky — so the re-submission after Clearing can pass."""
        biko = BikoSim(mp_id="9979999000007", reference_date=ON)
        biko.reject_next(Klaerfall("summe_weicht_ab", detail="delta 12.4 kWh"))

        first = biko.receive(utilmd(55001), bilanzkreis="11XBK-------1")
        assert first["ack"] is False
        assert first["klaerfall"] == "summe_weicht_ab"
        assert first["detail"] == "delta 12.4 kWh"

        second = biko.receive(utilmd(55001), bilanzkreis="11XBK-------1")
        assert second["ack"] is True, "the re-submission must succeed"
        assert len(biko.klaerfaelle) == 1

    def test_reject_bilanzkreis_is_sticky_for_that_bilanzkreis_only(self):
        biko = BikoSim(mp_id="9979999000007", reference_date=ON)
        biko.reject_bilanzkreis("11XBK-------1", Klaerfall("bilanzkreis_unbekannt"))

        assert biko.receive(utilmd(55001), bilanzkreis="11XBK-------1")["ack"] is False
        assert biko.receive(utilmd(55001), bilanzkreis="11XBK-------1")["ack"] is False
        assert biko.receive(utilmd(55001), bilanzkreis="11XBK-------2")["ack"] is True

    def test_accept_by_default_false_rejects_everything(self):
        biko = BikoSim(mp_id="9979999000007", accept_by_default=False, reference_date=ON)
        assert biko.receive(utilmd(55001))["ack"] is False


class TestImsysSim:
    def test_a_fresh_gateway_has_a_valid_certificate(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        assert smgw.certificate_state == "valid"

    def test_expiry_inside_the_warn_window_reports_expiring(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        smgw.expire_certificate_in(days=5)
        assert smgw.certificate_state == "expiring"

    def test_a_past_expiry_reports_expired(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        smgw.expire_certificate_in(days=-1)
        assert smgw.certificate_state == "expired"

    def test_revocation_is_distinct_from_expiry(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        smgw.revoke_certificate()
        assert smgw.certificate_state == "revoked"

    def test_advancing_the_clock_can_cross_expiry_mid_test(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        smgw.expire_certificate_in(days=10)
        assert smgw.certificate_state == "expiring"
        smgw.advance(days=11)
        assert smgw.certificate_state == "expired"

    def test_an_expired_certificate_yields_no_usable_cls_channel(self):
        """A §14a dispatch must not steer through a dead control path."""
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        smgw.expire_certificate_in(days=-1)
        channel = smgw.open_channel("CLS-1")
        assert channel.open is False
        assert channel.last_error == "certificate expired"

    def test_a_valid_certificate_opens_the_channel(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        assert smgw.open_channel("CLS-1").open is True

    def test_delivery_records_gaps_so_ersatzwertbildung_can_be_tested(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        gang = smgw.deliver("2026-11-01", werte=[1.0] * 96, luecken=[42, 41])
        assert gang.luecken == [41, 42], "sorted"
        assert gang.vollstaendig is False
        assert smgw.letzte_lieferung is gang

    def test_a_complete_series_reports_vollstaendig(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        assert smgw.deliver("2026-11-01", werte=[1.0] * 96).vollstaendig is True

    def test_an_expired_gateway_refuses_to_deliver(self):
        smgw = ImsysSim(melo_id="DE" + "0" * 31, today="2026-11-01")
        smgw.expire_certificate_in(days=-1)
        with pytest.raises(RuntimeError, match="certificate is expired"):
            smgw.deliver("2026-11-01", werte=[1.0] * 96)

    def test_an_unknown_taf_profile_is_rejected(self):
        with pytest.raises(ValueError, match="unknown TAF profile"):
            ImsysSim(melo_id="DE" + "0" * 31, taf="TAF-99")
