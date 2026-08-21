"""PID introspection and the AHB answer table."""

import pytest

from conftest import ON
from makotest import (
    ablehnung_pid,
    answer_pids,
    assert_answer_pid,
    bestaetigung_pid,
    message_types_of,
    pid_has_ahb_rules,
    pruefidentifikatoren,
)


class TestEnumeration:
    def test_utilmd_has_a_substantial_pid_set(self):
        pids = pruefidentifikatoren("UTILMD")
        assert len(pids) > 50
        assert pids == sorted(set(pids)), "ascending and unique"

    def test_core_gpke_pids_are_present(self):
        pids = pruefidentifikatoren("UTILMD")
        for pid in (55001, 55002, 55003, 55004):
            assert pid in pids

    def test_unassigned_codes_are_not_reported_as_known(self):
        """The stand-in rule pack must not be mistaken for real rules.

        `ahb_rule_pack` returns a one-rule `unknown-pid` pack for a code it does
        not know, so a `rule_count() > 0` test would report every PID as known.
        56xxx is an unassigned band and is the check for that.
        """
        assert not [p for p in pruefidentifikatoren("UTILMD") if 56000 <= p <= 56999]
        assert not pid_has_ahb_rules("UTILMD", 56001)
        assert not pid_has_ahb_rules("UTILMD", 99999)

    def test_dating_the_enumeration_narrows_it_to_the_active_profile(self):
        """A PID retired at the last Formatumstellung is still known to the
        registry through its old profile — but a message carrying it today
        validates vacuously anyway, so a generator should not draw it.
        """
        everything = pruefidentifikatoren("UTILMD", sparte="STROM")
        today = pruefidentifikatoren("UTILMD", on=ON, sparte="STROM")
        assert today, "a live format version carries PIDs"
        assert set(today) <= set(everything)
        assert all(55000 <= p <= 55999 for p in today)

    def test_the_sparte_splits_the_utilmd_band(self):
        assert all(
            44000 <= p <= 44999 for p in pruefidentifikatoren("UTILMD", sparte="GAS")
        )

    def test_contrl_has_no_pruefidentifikatoren(self):
        """CONTRL is a technical ack — the AHB assigns it no PIDs."""
        assert pruefidentifikatoren("CONTRL") == []

    def test_unknown_message_type_is_an_error(self):
        with pytest.raises(ValueError, match="unknown EDIFACT message type"):
            pruefidentifikatoren("NOSUCH")

    def test_message_type_is_case_insensitive(self):
        assert pruefidentifikatoren("utilmd") == pruefidentifikatoren("UTILMD")


class TestMessageTypesOf:
    @pytest.mark.parametrize(
        ("pid", "expected"),
        [
            (55001, ["UTILMD"]),
            (44001, ["UTILMD"]),
            (13025, ["MSCONS"]),
            (17115, ["ORDERS"]),
            (31009, ["INVOIC"]),
            (33001, ["REMADV"]),
        ],
    )
    def test_pid_resolves_against_the_profiles(self, pid, expected):
        assert message_types_of(pid) == expected

    @pytest.mark.parametrize("pid", [29001, 29002])
    def test_29xxx_belongs_to_both_aperak_and_comdis(self, pid):
        """A PID does not identify one message type.

        Both AHBs declare 29001/29002, so any single-name answer is wrong for
        one of them — which is why this returns a list.
        """
        assert message_types_of(pid) == ["APERAK", "COMDIS"]

    def test_unknown_codes_resolve_to_nothing(self):
        assert message_types_of(99999) == []
        assert message_types_of(1) == []


class TestAnswerTable:
    """The answer PIDs a simulated counterparty must reply with.

    These are conformance-tested against the GPKE and GeLi Gas workflows on the
    Rust side, so a value here is the value the platform actually expects.
    """

    @pytest.mark.parametrize(
        ("anfrage", "pair"),
        [
            (55001, (55002, 55003)),
            (55004, (55005, 55006)),
            (55016, (55017, 55018)),
            (44001, (44002, 44003)),
            (44004, (44005, 44006)),
        ],
    )
    def test_regular_triples(self, anfrage, pair):
        assert answer_pids(anfrage) == pair

    def test_55077_rejects_with_55080_not_55079(self):
        """55079 is unassigned, so the Ablehnung breaks the +2 pattern."""
        assert answer_pids(55077) == (55078, 55080)
        assert ablehnung_pid(55077) == 55080

    def test_44020_is_confirmable_but_not_rejectable(self):
        """An asymmetric family: a pair query must not invent an Ablehnung."""
        assert bestaetigung_pid(44020) == 44021
        assert ablehnung_pid(44020) is None
        assert answer_pids(44020) is None

    def test_44019_has_neither_answer(self):
        assert bestaetigung_pid(44019) is None
        assert ablehnung_pid(44019) is None

    def test_answer_pids_are_not_themselves_request_pids(self):
        for anfrage in (55001, 55004, 44001, 44004):
            ok, nok = answer_pids(anfrage)
            assert answer_pids(ok) is None
            assert answer_pids(nok) is None

    def test_non_request_pids_have_no_answer(self):
        assert answer_pids(13025) is None
        assert answer_pids(99999) is None


class TestAssertAnswerPid:
    def test_a_conformant_answer_passes(self):
        assert_answer_pid(55002, anfrage=55001, accepted=True)
        assert_answer_pid(55080, anfrage=55077, accepted=False)

    def test_the_plus_one_guess_fails(self):
        with pytest.raises(AssertionError, match="wrong answer"):
            assert_answer_pid(55079, anfrage=55077, accepted=False)

    def test_an_answer_the_ahb_does_not_define_says_so(self):
        with pytest.raises(ValueError, match="never rejectable"):
            assert_answer_pid(44022, anfrage=44020, accepted=False)
