"""The WiM and ESA wires — ORDERS, ORDRSP, IFTSTA and QUOTES.

These are the message types that carry the Messstellenbetrieb and
Energieserviceanbieter processes, and together with UTILMD they take the share
of published answer obligations a test can actually answer from half to nearly
all of them.

Two things they get right that a hand-rolled fixture would not: an Antwortcode
travels with the tree that publishes it on **every** wire, and a Bindungsfrist
is a duration rather than a date.
"""

from __future__ import annotations

import pytest

from conftest import ON
from makotest import (
    antwort_obligation,
    assert_edifact_valid,
    build_iftsta,
    build_interchange,
    build_orders,
    build_ordrsp,
    build_quotes,
    validate_edifact,
)
from makotest.plugin import LF_ID, NB_ID

MALO = "51238696012"


def wrap(message: bytes, *, dar: str = "W1", sender: str = NB_ID) -> bytes:
    """Envelope one message.

    `sender` must match the message's own `NAD+MS`: BDEW Allgemeine Festlegungen
    §2.13 requires UNB DE0004 and the NAD to be identical, and the parser
    refuses the interchange outright when they differ.
    """
    receiver = LF_ID if sender == NB_ID else NB_ID
    return build_interchange(
        sender=sender, receiver=receiver, dar=dar, messages=[message], on=ON
    )


class TestOrdersOrdrspRoundTrip:
    def test_the_request_and_its_answer_both_build(self):
        """The pair closes the loop a WiM or ESA test drives."""
        request = wrap(
            build_orders(
                17008, NB_ID, LF_ID, on=ON, document_code="E01", abonnement="Z01"
            ),
            dar="O1",
        )
        assert_edifact_valid(request, on=ON)
        assert validate_edifact(request, ON).messages[0].message_type == "ORDERS"

        answer = wrap(
            build_ordrsp(
                19011,
                LF_ID,
                NB_ID,
                antwort_code="A01",
                antwort_ebd="E_0254",
                on=ON,
                abonnement="Z01",
                line_item=True,
                item_description=True,
            ),
            dar="O2",
            sender=LF_ID,
        )
        assert_edifact_valid(answer, on=ON)
        assert "AJT+A01+E_0254" in answer.decode()

    def test_the_ahb_names_what_a_pid_forbids(self):
        """17008 uses no `LOC`, and the AHB says so rather than ignoring it.

        Worth pinning because a location looks like the obvious thing to put on
        an order: the rule is per Prüfidentifikator, not per message type.
        """
        wire = wrap(
            build_orders(17008, NB_ID, LF_ID, on=ON, document_code="E01", location=MALO)
        )
        report = validate_edifact(wire, ON)
        assert not report.is_valid
        assert "AHB-17008-LOC-N" in {f.rule_id for f in report.errors}

    def test_the_answer_pid_comes_from_the_published_table(self):
        """17008's answer is an ORDRSP, and the table says which."""
        obligation = antwort_obligation(17008)
        assert obligation is not None
        assert obligation.ebd == "E_0254"


class TestIftsta:
    def test_a_status_is_a_category_and_a_reason(self):
        """`SG15 STS` is a pair, not a single code."""
        wire = wrap(
            build_iftsta(
                21042,
                NB_ID,
                LF_ID,
                on=ON,
                status=("Z21", "105"),
                vorgangsnummer="1",
                order_reference="BEST-1",
                vertragsende="20261101",
            ),
            dar="I1",
        )
        assert_edifact_valid(wire, on=ON)
        text = wire.decode()
        assert "STS+Z21+:105" in text
        assert "CNI+1" in text, "the Vorgangsnummer is Muss on a WiM status"

    def test_the_message_is_an_iftsta_the_ahb_checks(self):
        report = validate_edifact(
            wrap(
                build_iftsta(
                    21042, NB_ID, LF_ID, on=ON, status=("Z21", "105"), vorgangsnummer="1"
                )
            ),
            ON,
        )
        assert report.messages[0].message_type == "IFTSTA"
        assert report.messages[0].rules_applied


class TestQuotes:
    def test_an_angebot_states_its_price_and_binding_period(self):
        wire = wrap(
            build_quotes(
                15003,
                NB_ID,
                LF_ID,
                on=ON,
                location=MALO,
                bindungsfrist=("3", "monat"),
                product="9991000000123",
                price="12.34",
                contact=("Max Mustermann", "esa@example.de"),
                currency="EUR",
            ),
            dar="Q1",
        )
        assert_edifact_valid(wire, on=ON)
        text = wire.decode()
        assert "DTM+273:3:802" in text, "a duration — three months"
        assert "PRI+CAL:12.34" in text

    def test_a_bindungsfrist_is_a_duration_not_a_date(self):
        """A `CCYYMMDD` there is a count the receiver reads as no period at all.

        The segment's presence is what separates an Angebot from an Ablehnung,
        so a date silently turns one into the other. The unit is therefore
        named, and an unknown one is refused rather than guessed.
        """
        with pytest.raises(ValueError, match='"monat", "woche" or "tag"'):
            build_quotes(15003, NB_ID, LF_ID, on=ON, bindungsfrist=("20261101", "jahr"))

    def test_the_angebot_is_the_price_basis(self):
        """An ESA has no Preisblatt, so the accepted Angebot is the basis."""
        report = validate_edifact(
            wrap(
                build_quotes(
                    15003,
                    NB_ID,
                    LF_ID,
                    on=ON,
                    location=MALO,
                    bindungsfrist=("3", "monat"),
                    product="9991000000123",
                    price="12.34",
                    contact=("Max Mustermann", "esa@example.de"),
                    currency="EUR",
                )
            ),
            ON,
        )
        assert report.messages[0].message_type == "QUOTES"
        assert report.messages[0].rules_applied


class TestCoverage:
    def test_most_published_obligations_are_now_answerable(self):
        """The measure that drove binding these four types.

        An obligation whose answer message type cannot be built is one no test
        can answer, however well the Frist and the Antwortcode are modelled.
        """
        from makotest import antwort_obligations, message_types_of

        buildable = {
            "UTILMD",
            "MSCONS",
            "REMADV",
            "ORDRSP",
            "ORDERS",
            "IFTSTA",
            "QUOTES",
            "APERAK",
            "CONTRL",
        }
        unanswerable = [
            o.trigger_pid
            for o in antwort_obligations()
            if (
                message_types_of(o.bestaetigung_pid)
                or message_types_of(o.trigger_pid)
                or ["?"]
            )[0]
            not in buildable
        ]
        # The three stragglers are UTILMD by band; their *answer* PIDs carry no
        # compiled AHB rules, so the message type cannot be resolved from them.
        assert sorted(unanswerable) == [55077, 55230, 55557]
