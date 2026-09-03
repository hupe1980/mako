"""Build → validate round-trips, and the format version as an argument.

The load-bearing property: a message `makotest` builds must survive the same AHB
validation the platform applies on ingest. If build and validate ever disagree,
every test written on top of the builder is meaningless.

The sharpest way to make them disagree is to pin a release by hand and validate
on a date where a different one is in force. So the builders take the **send
date** and resolve the release from it.
"""

import pytest

from conftest import MALO, ON, utilmd_interchange
from makotest import (
    UtilmdTransaction,
    assert_edifact_valid,
    build_answer,
    build_aperak,
    build_aperak_for,
    build_contrl,
    build_contrl_for,
    build_interchange,
    build_mscons,
    build_utilmd,
    format_versions,
    release_for,
    releases,
    validate_edifact,
)
from makotest.plugin import LF_ID, NB_ID


class TestReleaseResolution:
    def test_a_dated_build_picks_the_active_release(self):
        wire = utilmd_interchange(on="2026-04-01")
        report = assert_edifact_valid(wire, on="2026-04-01")
        assert report.release == release_for("UTILMD", "2026-04-01", "STROM")

    def test_the_sparte_selects_the_utilmd_track(self):
        """UTILMD is the only type with two parallel tracks on the same date."""
        strom = release_for("UTILMD", ON, "STROM")
        gas = release_for("UTILMD", ON, "GAS")
        assert strom.startswith("S") and gas.startswith("G")

    def test_the_sparte_is_inferred_from_the_pid_band(self):
        """55xxx is Strom and 44xxx is Gas, so it need not be repeated."""
        strom = build_utilmd(55001, LF_ID, NB_ID, on=ON)
        gas = build_utilmd(44001, LF_ID, NB_ID, on=ON)
        assert validate_edifact(strom, ON).release.startswith("S")
        assert validate_edifact(gas, ON).release.startswith("G")

    def test_neither_a_release_nor_a_date_is_refused(self):
        with pytest.raises(ValueError, match="release="):
            build_utilmd(55001, LF_ID, NB_ID)

    def test_a_date_before_every_profile_is_refused_with_a_hint(self):
        with pytest.raises(ValueError, match="format_versions"):
            build_utilmd(55001, LF_ID, NB_ID, on="2000-01-01")

    def test_the_published_versions_are_introspectable(self):
        assert all(v.startswith("FV") for v in format_versions())
        assert "S2.1" in releases("UTILMD")


class TestRoundTrips:
    def test_utilmd_survives_validation(self, anmeldung):
        report = assert_edifact_valid(anmeldung, on=ON)
        assert report.pruefidentifikator == 55001

    def test_mscons_survives_validation(self):
        message = build_mscons(
            13003,
            NB_ID,
            LF_ID,
            metering_point="51238696012",
            intervals=[("79", "1234.567", "KWH", "202604010000+00", "202604010015+00")],
            on=ON,
        )
        report = validate_edifact(message, ON)
        assert report.pruefidentifikator == 13003
        assert report.message_type == "MSCONS"

    def test_builders_emit_a_message_not_an_interchange(self):
        """`build_*` returns UNH..UNT. Sending it needs the UNB/UNZ envelope."""
        message = build_utilmd(55001, LF_ID, NB_ID, on=ON)
        text = message.decode("latin-1")
        assert text.startswith("UNH+") and "UNB+" not in text

    def test_an_interchange_needs_a_transmission_date(self):
        """`000000:0000` parses to no date, and a counterparty cannot process it."""
        message = build_utilmd(55001, LF_ID, NB_ID, on=ON)
        with pytest.raises(ValueError, match="transmission date"):
            build_interchange(sender=LF_ID, receiver=NB_ID, dar="X", messages=[message])

    def test_an_empty_interchange_is_refused(self):
        with pytest.raises(ValueError, match="at least one message"):
            build_interchange(sender=LF_ID, receiver=NB_ID, dar="X", messages=[], on=ON)


class TestDeterminism:
    def test_two_builds_of_the_same_inputs_are_byte_identical(self):
        """Which makes golden-file comparison viable.

        The document date defaults to the send date rather than to today, so a
        rendered message does not change between runs.
        """
        assert utilmd_interchange() == utilmd_interchange()

    def test_the_document_date_follows_the_send_date(self):
        message = build_utilmd(55001, LF_ID, NB_ID, on="2026-04-01")
        assert b"DTM+137:20260401" in message


class TestAcknowledgements:
    def test_an_aperak_mirrors_the_message_it_answers(self, anmeldung):
        """The parties swap and RFF+ACW carries the acknowledged UNH reference.

        Those fields are what correlate an acknowledgement with what it
        acknowledges, so they are derived rather than supplied.
        """
        aperak = build_aperak_for(anmeldung, on=ON, error_code="Z10")
        text = aperak.decode("latin-1")
        assert f"NAD+MS+{NB_ID}" in text
        assert f"NAD+MR+{LF_ID}" in text
        assert "RFF+ACW:MSG-1" in text
        assert "BGM+313" in text, "an error APERAK is a Verarbeitbarkeitsfehlermeldung"

    def test_a_positive_aperak_is_an_anerkennungsmeldung(self, anmeldung):
        assert b"BGM+312" in build_aperak_for(anmeldung, on=ON)

    def test_a_contrl_echoes_the_datenaustauschreferenz(self, anmeldung):
        contrl = build_contrl_for(anmeldung, on=ON, accept=False)
        assert b"REF1" in contrl

    def test_both_acknowledgements_can_be_built_standalone(self):
        assert build_aperak(NB_ID, LF_ID, on=ON).startswith(b"UNH+1+APERAK")
        assert build_contrl(NB_ID, LF_ID, "REF1", on=ON).startswith(b"UNH+1+CONTRL")


class TestBusinessAnswer:
    def test_the_answer_mirrors_the_request(self, anmeldung):
        """Everything correlating the answer with the request is echoed.

        `IDE+24` carries the Vorgangsnummer, which is what correlates the answer
        on the counterparty's side. The Lokation lives in `SG5 LOC`, so it is
        echoed as its own segment rather than riding the Vorgang, and the
        request's RFF references travel with it.
        """
        answer = build_answer(anmeldung, 55002, on=ON, process_dates=[("92", "20260501")])
        text = answer.decode("latin-1")
        assert "BGM+E01+" in text, text
        assert "RFF+Z13:55002" in text, "the answer carries its own Prüfidentifikator"
        assert f"NAD+MS+{NB_ID}" in text, "the request's receiver answers"
        assert f"LOC+Z16+{MALO}" in text, "the Marktlokation rides SG5 LOC"
        assert "IDE+24+" in text, "IDE carries the Vorgangsnummer, not a location"
        assert "RFF+Z13:55001" not in text, "the request's PID is not echoed"
        assert "DTM+92:20260501" in text

    def test_the_answer_validates_on_the_same_format_version(self, anmeldung):
        answer = build_answer(anmeldung, 55002, on=ON)
        report = assert_edifact_valid(answer, on=ON)
        assert report.pruefidentifikator == 55002

    def test_answering_a_non_utilmd_request_is_refused(self):
        mscons = build_mscons(
            13003,
            NB_ID,
            LF_ID,
            metering_point="51238696012",
            intervals=[("79", "1", "KWH", "202604010000+00", "202604010015+00")],
            on=ON,
        )
        wire = build_interchange(
            sender=NB_ID, receiver=LF_ID, dar="M1", messages=[mscons], on=ON
        )
        with pytest.raises(ValueError, match="build_aperak_for"):
            build_answer(wire, 55002, on=ON)


class TestProduktpaket:
    """The Bilanzkreis a Zuordnung assigns, as the `SG8 SEQ+Z79` group."""

    def test_the_whole_group_is_rendered_from_the_eic_alone(self):
        """Four constants the document fixes, so a test does not transcribe them.

        `SEQ+Z79+1`, the Produkt-Code `9991000002082`, `CCI+Z66` and the `ZV4`
        CAV qualifier are all fixed by the AHB. A test supplying them by hand
        would be copying values it has no way to check.
        """
        wire = utilmd_interchange_with_bilanzkreis("11XBK-EEG-----1")
        text = wire.decode()
        assert "SEQ+Z79" in text
        assert "PIA+5+9991000002082:Z11" in text
        assert "CCI+Z66" in text
        assert "CAV+ZV4:::11XBK-EEG-----1" in text
        assert_edifact_valid(wire, on=ON)

    def test_a_vorgang_without_one_carries_no_sg8_at_all(self):
        """Absent, not empty — an empty group is a different message.

        Built on a 55003, which is not one of the six Anwendungsfälle the AHB
        makes the Produktpaket Muss on: a 55001 without one is refused.
        """
        message = build_utilmd(
            55003,
            NB_ID,
            LF_ID,
            on=ON,
            transactions=[UtilmdTransaction("VORGANG-1", locations=[("malo", MALO)])],
        )
        assert "SEQ+" not in message.decode()


def utilmd_interchange_with_bilanzkreis(eic: str) -> bytes:
    message = build_utilmd(
        55001,
        LF_ID,
        NB_ID,
        on=ON,
        message_ref="MSG-1",
        transactions=[
            UtilmdTransaction(
                "VORGANG-1",
                locations=[("malo", MALO)],
                dates=[("92", "20260501")],
                bilanzkreis=eic,
            )
        ],
    )
    return build_interchange(
        sender=LF_ID, receiver=NB_ID, dar="BK1", messages=[message], on=ON
    )


class TestAntwortDritter:
    """`SG4 STS+Z35` — the Altlieferant's own refusal, restated by the NB.

    Muss on an Ablehnung whose Antwortcode is `A50` or `A57` (UTILMD AHB Strom
    Bedingungen `[356]` / `[84]`): both say the LFA refused to release the
    Marktlokation, and GPKE Teil 2 § 2.1.2 Nr. 6 makes the NB state that
    refusal's ground beside its own.
    """

    def test_the_ablehnung_carries_both_statuses(self):
        wire = ablehnung_mit_widerspruch("A50", "A32")
        text = wire.decode()
        assert "STS+E01++A50:E_0623" in text, "the NB's own answer"
        assert "STS+Z35++A32:E_0624" in text, "the LFA's, from its own tree"
        assert_edifact_valid(wire, on=ON)

    def test_the_third_party_code_comes_from_its_own_tree(self):
        """`E_0624` is the Beendigung der Zuordnung the LFA answered from.

        A code resolved against the NB's tree would be a different meaning
        under the same characters, which is why the segment names `E_0624`.
        """
        from makotest import antwort_code

        assert antwort_code("E_0624", "A32") is not None
        assert antwort_code("E_0623", "A32") is None

    def test_an_ablehnung_naming_none_gets_the_third_party_status_filled_in(self):
        """`STS+Z35` is Muss beside `A50` (UTILMD AHB Strom, Bedingung [356]).

        A caller who names no Dritter status gets the column's placeholder:
        the message is completed to its Prüfschablone rather than shipped
        short of a Muss segment.
        """
        wire = ablehnung_mit_widerspruch("A50", None)
        assert "STS+Z35" in wire.decode()
        assert_edifact_valid(wire, on=ON)


def ablehnung_mit_widerspruch(code: str, dritter: str | None) -> bytes:
    message = build_utilmd(
        55003,
        NB_ID,
        LF_ID,
        on=ON,
        message_ref="MSG-1",
        transactions=[
            UtilmdTransaction(
                "VORGANG-1",
                antwort_code=code,
                antwort_ebd="E_0623",
                antwort_dritter=dritter,
            )
        ],
    )
    return build_interchange(
        sender=NB_ID, receiver=LF_ID, dar="W1", messages=[message], on=ON
    )
