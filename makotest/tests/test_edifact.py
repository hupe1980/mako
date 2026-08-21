"""Validation through the platform's own AHB engine.

The report covers the whole **interchange**: the envelope's structural integrity
and every message inside it. Validating only the first message of a multi-message
interchange is how a broken second one gets shipped.
"""

import pytest

from conftest import MELO, ON, utilmd_interchange
from makotest import (
    UtilmdTransaction,
    assert_edifact_valid,
    assert_rule_fires,
    assert_rules_applied,
    build_interchange,
    build_utilmd,
    validate_edifact,
)
from makotest.plugin import LF_ID, NB_ID


def _message(
    pid: int, *, melo: str = MELO, message_ref: str = "MSG-1", on: str = ON
) -> bytes:
    """A complete UTILMD message — a UTILMD needs both an RFF and an SG4 IDE."""
    return build_utilmd(
        pid,
        LF_ID,
        NB_ID,
        on=on,
        release="S2.1" if pid > 55999 else None,
        message_ref=message_ref,
        references=[("Z13", str(pid))],
        transactions=[
            UtilmdTransaction(
                "VORGANG-1",
                locations=[("melo", melo)],
                dates=[("92", "20260501")],
                references=[("Z13", str(pid))],
            )
        ],
    )


class TestReport:
    def test_a_valid_interchange_reports_its_message(self, anmeldung):
        report = assert_edifact_valid(anmeldung, on=ON)
        assert report.pruefidentifikator == 55001
        assert report.message_type == "UTILMD"
        assert report.release.startswith("S")
        assert report.rules_applied

    def test_the_envelope_is_reported_with_derived_qualifiers(self, anmeldung):
        """DE0007 comes from the party ID, so it cannot contradict it.

        `4012…` is a GS1 GLN (14) and `99…` a BDEW-Codenummer (500). S002 is
        `[identification, qualifier, reverse routing]`, so the qualifier is the
        second component; reading the third yields the empty routing address.
        """
        envelope = validate_edifact(anmeldung, ON).envelope
        assert envelope.sender_id == LF_ID
        assert envelope.sender_qualifier == "14"
        assert envelope.receiver_id == NB_ID
        assert envelope.receiver_qualifier == "500"
        assert envelope.control_ref == "REF1"
        assert envelope.transmission_date == ON
        assert not envelope.test_indicator
        assert envelope.is_structurally_valid

    def test_a_bare_message_validates_without_an_envelope(self):
        """The builders return UNH..UNT; validating one keeps the loop short."""
        report = validate_edifact(_message(55001), ON)
        assert report.envelope is None
        assert report.pruefidentifikator == 55001

    def test_every_message_of_a_multi_message_interchange_is_validated(self):
        good = _message(55001)
        bad = _message(55001, melo="NOTAMELO", message_ref="2")
        wire = build_interchange(
            sender=LF_ID, receiver=NB_ID, dar="REF2", messages=[good, bad], on=ON
        )
        report = validate_edifact(wire, ON)
        assert len(report.messages) == 2
        assert not report.is_valid, "one bad message invalidates the interchange"
        assert report.messages[0].is_valid and not report.messages[1].is_valid

    def test_a_single_answer_is_refused_for_a_multi_message_interchange(self):
        """One PID cannot speak for two messages, so it raises instead of guessing."""
        message = _message(55001)
        wire = build_interchange(
            sender=LF_ID,
            receiver=NB_ID,
            dar="REF3",
            messages=[message, message],
            on=ON,
        )
        report = validate_edifact(wire, ON)
        with pytest.raises(ValueError, match=r"report\.messages"):
            _ = report.pruefidentifikator


class TestFindings:
    def test_a_bad_location_id_fires_the_semantic_rule(self):
        wire = utilmd_interchange(melo="NOTAMELO")
        assert_rule_fires(wire, "SEM-UTILMD-LOKATIONS-ID", on=ON)

    def test_a_finding_carries_its_position_and_layer(self):
        report = validate_edifact(utilmd_interchange(melo="NOTAMELO"), ON)
        finding = report.errors[0]
        assert finding.rule_origin == "semantic"
        assert finding.position, "a finding must say where it fired"
        assert finding.is_error

    def test_the_layer_separates_syntax_from_application(self):
        """A CONTRL answers `parse`/`directory`; an APERAK answers the rest.

        A simulator picks its acknowledgement from this rather than from the
        message text, so the distinction has to be readable.
        """
        report = validate_edifact(utilmd_interchange(document_code="XXX"), ON)
        assert report.errors, "an unknown BGM code must not pass"
        assert {f.rule_origin for f in report.errors} <= {
            "parse",
            "directory",
            "mig",
            "ahb",
            "semantic",
            "custom",
        }

    def test_assert_rule_fires_reports_what_did_fire(self):
        with pytest.raises(AssertionError, match="expected rule"):
            assert_rule_fires(
                utilmd_interchange(melo="NOTAMELO"), "SEM-NOT-A-REAL-RULE", on=ON
            )


class TestVacuousValidation:
    def test_a_pid_with_no_rules_is_reported_rather_than_passing(self):
        """56xxx is unassigned: the message "validates" having checked nothing.

        This is the failure mode that makes a green suite worthless, so the
        assertion helper refuses it even though `is_valid` is true.
        """
        message = _message(56001)
        wire = build_interchange(
            sender=LF_ID, receiver=NB_ID, dar="REF4", messages=[message], on=ON
        )
        report = validate_edifact(wire, ON)
        assert report.is_valid, "nothing checked it, so nothing rejected it"
        assert not report.rules_applied
        with pytest.raises(AssertionError, match="no AHB rules were applied"):
            assert_edifact_valid(wire, on=ON)
        with pytest.raises(AssertionError, match="cannot fail as written"):
            assert_rules_applied(wire, on=ON)


class TestInput:
    def test_unparseable_input_raises_rather_than_reporting(self):
        with pytest.raises(ValueError, match="parse failed"):
            validate_edifact(b"this is not EDIFACT at all", ON)

    def test_the_reference_date_is_required(self):
        with pytest.raises(TypeError):
            validate_edifact(b"UNB")  # type: ignore[call-arg]

    def test_a_message_valid_on_one_format_version_can_fail_on_another(self):
        """Which is the entire reason the date is an argument.

        FV2026-04-01 expects S2.1 and FV2026-10-01 expects S2.2. Validating one
        release against the other's profile reports the mismatch rather than
        anything about the message.
        """
        wire = utilmd_interchange(on="2026-04-01")
        assert validate_edifact(wire, "2026-04-01").is_valid
        later = validate_edifact(wire, "2026-10-01")
        assert not later.is_valid or later.release != "S2.2"


class TestBareBlobs:
    def test_several_messages_without_an_envelope_are_refused(self):
        """Parsing would silently keep the first and drop the rest."""
        message = _message(55001)
        with pytest.raises(ValueError, match="without a UNB envelope"):
            validate_edifact(message + message, ON)
