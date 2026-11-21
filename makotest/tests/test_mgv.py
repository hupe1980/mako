"""The Marktgebietsverantwortliche, answering the aggregierte MMM-Rechnung.

Each test pins one answer to the document that states it: the CONTRL every Gas
interchange earns, the APERAK only a Verarbeitbarkeitsfehler earns, and the
REMADV that pays or refuses the invoice — and every answer is itself validated,
because a simulator whose answer the platform would refuse tests nothing.
"""

from __future__ import annotations

import pytest

from conftest import ON, utilmd_interchange
from makotest import MgvSim, MmmAblehnung, assert_edifact_valid, validate_edifact
from makotest.plugin import GAS_NB_ID, MGV_ID
from makotest.simulators import G_0080, MGV_INVOIC_PIDS

#: An aggregierte MMM-Rechnung in the INVOIC AHB 1.0b Kap. 3.1.3 shape. The
#: Positionszeitraum runs Gastag to Gastag — 06:00 Berlin, `0500`/`0400` UTC.
_SEGMENTS = [
    "UNH+{ref}+INVOIC:D:06A:UN:2.8e",
    "BGM+{bgm}+{nummer}+9",
    "DTM+137:202604010800?+00:303",
    "DTM+9:202604010800?+00:303",
    "DTM+155:202602282300?+00:303",
    "DTM+156:202603312200?+00:303",
    "IMD++MMM",
    "RFF+Z13:{pid}",
    "RFF+ACE:MSCONS-2026-03-0001",
    f"NAD+MS+{GAS_NB_ID}::332++Gasnetz Musterstadt GmbH:::::Z02"
    "+Netzweg 1+Musterstadt++12345+DE",
    "RFF+VA:DE123456789",
    f"NAD+MR+{MGV_ID}::332++Marktgebiet GmbH:::::Z02+Am Hub 2+Ratingen++40882+DE",
    "RFF+VA:DE987654321",
    "NAD+ZSH+NK0001",
    "CUX+2:EUR:4",
    "PYT+3",
    "DTM+265:202604222200?+00:303",
    "LIN+1++9990001000611:Z01",
    "QTY+47:1000:KWH",
    "DTM+155:202603010500?+00:303",
    "DTM+156:202604010400?+00:303",
    "MOA+203:30.00",
    "PRI+CAL:0.03",
    "TAX+7+VAT+++:::19+S",
    "UNS+S",
    "MOA+77:35.70",
    "MOA+9:35.70",
    "TAX+7+VAT+++:::19+S",
    "MOA+125:30.00",
    "MOA+161:5.70",
]


def mmm_message(
    pid: int = 31007, *, ref: str = "1", nummer: str = "MMM-2026-03-0001", drop=()
) -> str:
    bgm = "389" if pid == 31008 else "380"
    segments = [
        s.format(ref=ref, bgm=bgm, nummer=nummer, pid=pid)
        for s in _SEGMENTS
        if not any(s.startswith(d) for d in drop)
    ]
    return "'".join(segments) + f"'UNT+{len(segments) + 1}+{ref}'"


def interchange(*messages: str, dar: str = "INV1") -> bytes:
    head = f"UNA:+.? 'UNB+UNOC:3+{GAS_NB_ID}:502+{MGV_ID}:502+260401:0800+{dar}'"
    return (head + "".join(messages) + f"UNZ+{len(messages)}+{dar}'").encode("latin-1")


@pytest.fixture
def mgv() -> MgvSim:
    return MgvSim(mp_id=MGV_ID, reference_date=ON)


class TestTheFixture:
    def test_the_invoice_is_one_the_ahb_really_checks(self):
        """A vacuous pass on the request would make every answer below vacuous."""
        for pid in sorted(MGV_INVOIC_PIDS):
            report = validate_edifact(interchange(mmm_message(pid)), ON)
            assert report.is_valid, [f.rule_id for f in report.errors]
            assert report.messages[0].rules_applied

    def test_the_plugin_fixture_is_an_mgv(self, mgv_sim, makotest_on):
        assert mgv_sim.mp_id == MGV_ID
        assert mgv_sim.reference_date == makotest_on


class TestZahlungsavis:
    @pytest.mark.parametrize("pid", sorted(MGV_INVOIC_PIDS))
    def test_a_sound_invoice_is_acknowledged_and_paid(self, mgv, pid):
        reply = mgv.receive(interchange(mmm_message(pid)))
        assert reply.contrl_positive and reply.aperak is None
        assert_edifact_valid(reply.contrl, on=ON)
        assert reply.remadv_pids == (33001,)
        (remadv,) = reply.remadvs
        assert_edifact_valid(remadv, on=ON)
        text = remadv.decode()
        assert "BGM+481+" in text, "a Zahlungsavis"
        assert "DOC+{}+MMM-2026-03-0001".format("389" if pid == 31008 else "380") in text
        assert "MOA+12:35.70" in text, "ganz oder gar nicht — the whole amount"
        assert mgv.paid and not mgv.abgewiesen

    def test_each_invoice_gets_its_own_remadv_file(self, mgv):
        """A REMADV-Datei carries exactly one REMADV-Nachricht (Kap. 3, Fn. 1)."""
        wire = interchange(
            mmm_message(ref="1", nummer="MMM-1"), mmm_message(ref="2", nummer="MMM-2")
        )
        reply = mgv.receive(wire)
        assert len(reply.remadvs) == 2
        for remadv in reply.remadvs:
            assert len(validate_edifact(remadv, ON).messages) == 1
        assert [r.rechnungsnummer for r in mgv.rechnungen] == ["MMM-1", "MMM-2"]


class TestAbweisung:
    def test_a_refusal_is_a_33002_with_its_g_0080_code(self, mgv):
        mgv.reject_next(MmmAblehnung("Z42"))
        reply = mgv.receive(interchange(mmm_message()))
        assert reply.contrl_positive, "the invoice was received; it is refused"
        assert reply.remadv_pids == (33002,)
        assert reply.antwort_codes == ("Z42",)
        (remadv,) = reply.remadvs
        assert_edifact_valid(remadv, on=ON)
        text = remadv.decode()
        assert "AJT+Z42+G_0080" in text
        assert "MOA+12:0" in text, "a refusal transfers nothing"

    def test_reject_next_is_queued_not_sticky(self, mgv):
        """So the corrected invoice after a refusal can be asserted as paid."""
        mgv.reject_next(MmmAblehnung("53"))
        first = mgv.receive(interchange(mmm_message()))
        second = mgv.receive(interchange(mmm_message(), dar="INV2"))
        assert (first.remadv_pids, second.remadv_pids) == ((33002,), (33001,))
        assert [r.accepted for r in mgv.rechnungen] == [False, True]

    @pytest.mark.parametrize("code", sorted(set(G_0080) - {"28"}))
    def test_every_g_0080_code_renders_a_conformant_refusal(self, mgv, code):
        mgv.reject_next(MmmAblehnung(code))
        (remadv,) = mgv.receive(interchange(mmm_message())).remadvs
        assert_edifact_valid(remadv, on=ON)

    def test_a_code_from_another_list_is_refused(self):
        """`A70` is the Netznutzungs-Summenprüfung in `E_0406`, not an MMM code."""
        with pytest.raises(ValueError, match="G_0080"):
            MmmAblehnung("A70")

    def test_sonstiges_is_refused_because_it_needs_an_erlaeuterung(self):
        with pytest.raises(ValueError, match="FTX"):
            MmmAblehnung("28")


class TestVerarbeitbarkeitsfehler:
    def test_a_missing_muss_angabe_earns_z29_and_no_remadv(self, mgv):
        """Gas sends the APERAK only on a fault, and never with BGM+312."""
        reply = mgv.receive(interchange(mmm_message(drop=("NAD+ZSH",))))
        assert reply.contrl_positive, "the interchange is syntactically sound"
        assert reply.ercs == ("Z29",)
        assert_edifact_valid(reply.aperak, on=ON)
        assert b"BGM+313" in reply.aperak and b"BGM+312" not in reply.aperak
        assert reply.remadvs == ()

    def test_a_queued_refusal_waits_for_a_processable_invoice(self, mgv):
        mgv.reject_next(MmmAblehnung("Z42"))
        mgv.receive(interchange(mmm_message(drop=("NAD+ZSH",))))
        reply = mgv.receive(interchange(mmm_message(), dar="INV2"))
        assert reply.antwort_codes == ("Z42",)

    def test_a_sound_invoice_beside_a_broken_one_is_still_paid(self, mgv):
        wire = interchange(
            mmm_message(ref="1", nummer="MMM-1", drop=("NAD+ZSH",)),
            mmm_message(ref="2", nummer="MMM-2"),
        )
        reply = mgv.receive(wire)
        assert reply.ercs == ("Z29",)
        assert reply.remadv_pids == (33001,)
        assert b"RFF+ACW:1" in reply.aperak, "the APERAK names the broken UNH"


class TestNotAddressedToAnMgv:
    def test_an_interchange_for_another_mp_id_earns_a_negative_contrl(self, mgv):
        """The Syntaxprüfung checks the recipient MP-ID (CONTRL AHB 1.0 Kap. 2.1)."""
        reply = mgv.receive(utilmd_interchange())
        assert not reply.contrl_positive
        assert reply.aperak is None and reply.remadvs == ()

    def test_a_utilmd_addressed_to_the_mgv_is_refused_with_z31(self, mgv):
        """No MGV processes a Lieferantenwechsel — Z31, and no further check."""
        reply = mgv.receive(utilmd_interchange(receiver=MGV_ID))
        assert reply.contrl_positive
        assert reply.ercs == ("Z31",)
        assert_edifact_valid(reply.aperak, on=ON)
        assert reply.remadvs == ()
        assert "UTILMD" in mgv.rechnungen[-1].refused

    def test_a_netznutzungsrechnung_is_not_paid_as_an_mmm_rechnung(self, mgv):
        """31002 goes to a Lieferant; paying it would assert nothing."""
        wire = interchange(mmm_message().replace("RFF+Z13:31007", "RFF+Z13:31002"))
        reply = mgv.receive(wire)
        assert reply.ercs == ("Z31",)
        assert reply.remadvs == ()
        assert "31002" in mgv.rechnungen[-1].refused

    def test_an_alocat_is_refused_rather_than_acknowledged_unread(self, mgv):
        alocat = "UNH+1+ALOCAT:D:04B:UN:5.11a'BGM+Z01+A1+9'UNT+3+1'"
        with pytest.raises(ValueError, match="ALOCAT"):
            mgv.receive(interchange(alocat))


class TestSyntax:
    def test_a_mig_violation_earns_only_a_negative_contrl(self, mgv):
        """The MIG is the CONTRL's yardstick (CONTRL AHB 1.0 Kap. 2.3.2).

        A refused interchange counts as never received (APERAK AHB 1.1 Kap. 1.4),
        so there is no APERAK and no REMADV beside the CONTRL.
        """
        too_long = "X" * 80
        wire = interchange(mmm_message(nummer=too_long))
        assert {f.rule_origin for f in validate_edifact(wire, ON).errors} == {"mig"}
        reply = mgv.receive(wire)
        assert not reply.contrl_positive
        uci = f"UCI+INV1+{GAS_NB_ID}:502+{MGV_ID}:502+4+".encode()
        assert uci in reply.contrl, "DE0083 4: this level and all below refused"
        assert reply.aperak is None and reply.remadvs == ()

    def test_a_bare_message_is_not_an_interchange(self, mgv):
        with pytest.raises(ValueError, match="interchange"):
            mgv.receive(mmm_message().encode())
