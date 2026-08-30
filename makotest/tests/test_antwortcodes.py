"""The Antwortcode catalogue — what a counterparty is entitled to answer with.

A Prüfidentifikator names the message that answers a request; the Antwortcode in
`SG4 STS+E01` says what the answer *states*, and it only means anything inside
the Entscheidungsbaum that publishes it.
"""

from __future__ import annotations

import pytest

from makotest import (
    antwort_code,
    antwort_codes,
    antwort_codes_for_pid,
    antwort_obligation,
    assert_antwort_code,
    entscheidungsbaeume,
)


class TestCatalogue:
    def test_a_code_is_resolved_within_its_tree_and_never_alone(self):
        """`A02` is published by four trees with four unrelated meanings.

        A catalogue keyed on the code alone would have to be wrong for three of
        them, which is why every lookup names the tree.
        """
        anmeldung = antwort_code("E_0622", "A02")
        abmeldung = antwort_code("E_0607", "A02")
        assert anmeldung is not None and abmeldung is not None
        assert anmeldung.bedeutung != abmeldung.bedeutung

    def test_a_code_the_tree_does_not_publish_is_absent(self):
        assert antwort_code("E_0622", "ZZZ") is None

    def test_an_unknown_tree_is_refused_rather_than_reported_empty(self):
        """An empty outcome space and an unknown tree are different answers."""
        with pytest.raises(ValueError, match="no Entscheidungsbaum"):
            antwort_codes("E_9999")

    def test_the_catalogue_names_the_trees_it_carries(self):
        trees = entscheidungsbaeume()
        assert trees == sorted(set(trees))
        assert "E_0622" in trees and "E_0623" in trees


class TestCluster:
    def test_the_cluster_is_a_property_of_the_code(self):
        """Not a boolean the caller supplies — that is the point of binding it."""
        refusal = antwort_code("E_0622", "A06")
        assert refusal.cluster == "ABLEHNUNG"
        assert refusal.ist_zustimmung is False

        agreement = antwort_code("E_0623", "A51")
        assert agreement.cluster == "ZUSTIMMUNG"
        assert agreement.ist_zustimmung is True

    def test_a_tree_off_the_agreement_axis_answers_none(self):
        """`E_0595` states whether data follows, not whether a request was granted.

        Reading its `None` as `False` would report an accepted Bestellung as a
        refusal.
        """
        off_axis = [c for c in antwort_codes("E_0595") if c.ist_zustimmung is None]
        assert off_axis, "E_0595 is off the agreement axis"
        assert {c.cluster for c in off_axis} <= {
            "AENDERUNG_DER_DATEN",
            "KEINE_AENDERUNG_DER_DATEN",
        }


class TestWireCodeliste:
    def test_a_gpke_tree_names_itself_in_de_1131(self):
        assert antwort_code("E_0622", "A06").wire_codeliste == "E_0622"

    def test_a_wim_tree_names_a_codeliste_and_the_cluster_picks_which(self):
        """DE 1131 carries the Codeliste, which for WiM is not the EBD number.

        A Bestätigung and an Ablehnung name *different* lists, so the value
        cannot be derived from the tree alone — and writing the EBD number there
        is a rejected message rather than a cosmetic difference.
        """
        codes = antwort_codes("E_0200")
        zustimmung = {c.wire_codeliste for c in codes if c.ist_zustimmung is True}
        ablehnung = {c.wire_codeliste for c in codes if c.ist_zustimmung is False}
        assert zustimmung == {"S_0090"}
        assert ablehnung == {"S_0054"}


class TestPidJoin:
    def test_an_inbound_pid_reaches_the_tree_its_frist_names(self):
        """The join the BDEW publishes across three separate documents."""
        codes = antwort_codes_for_pid(55001)
        assert codes and {c.tree for c in codes} == {"E_0622"}
        assert antwort_obligation(55001).ebd == "E_0622"

    def test_a_pid_with_no_published_obligation_resolves_to_nothing(self):
        """Unknown, never guessed at."""
        assert antwort_codes_for_pid(44020) == []

    def test_the_named_tree_can_be_one_stage_of_a_chain(self):
        """A GPKE Anmeldung runs the Vorprüfung before the Lieferbeginn decision.

        `E_0622` publishes refusals only; the agreement codes are in `E_0623`.
        Anything that treated the named tree as the whole outcome space would
        conclude a 55001 can never be confirmed.
        """
        vorpruefung = antwort_codes_for_pid(55001)
        assert all(c.ist_zustimmung is False for c in vorpruefung)
        assert any(c.ist_zustimmung is True for c in antwort_codes("E_0623"))


class TestAssertAntwortCode:
    def test_a_published_code_passes(self):
        assert_antwort_code("A06", ebd="E_0622")
        assert_antwort_code("A06", ebd="E_0622", accepted=False)

    def test_a_code_the_tree_does_not_publish_fails_and_names_what_it_does(self):
        with pytest.raises(AssertionError, match="publishes: "):
            assert_antwort_code("A99", ebd="E_0622")

    def test_the_cluster_is_checked_against_the_answer(self):
        with pytest.raises(AssertionError, match="not a Zustimmung"):
            assert_antwort_code("A06", ebd="E_0622", accepted=True)

    def test_asking_about_agreement_off_the_axis_is_the_tests_error(self):
        """`ValueError`: the question does not apply, so no answer could be right."""
        off_axis = next(c for c in antwort_codes("E_0595") if c.ist_zustimmung is None)
        with pytest.raises(ValueError, match="off the agreement axis"):
            assert_antwort_code(off_axis.code, ebd="E_0595", accepted=True)


class TestStrategies:
    def test_a_strategy_draws_only_codes_the_tree_publishes(self):
        from hypothesis import given

        from makotest.strategies import antwort_codes as antwort_code_strategy

        published = {c.code for c in antwort_codes("E_0623") if c.ist_zustimmung}

        @given(code=antwort_code_strategy(ebd="E_0623", accepted=True))
        def check(code: str) -> None:
            assert code in published

        check()

    def test_a_cluster_a_tree_never_reaches_is_refused(self):
        """`E_0622` is the Vorprüfung: there is no agreement to draw."""
        from makotest.strategies import antwort_codes as antwort_code_strategy

        with pytest.raises(ValueError, match="publishes no Antwortcode"):
            antwort_code_strategy(ebd="E_0622", accepted=True)
