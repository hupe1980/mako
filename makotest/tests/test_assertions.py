"""The domain assertions that do not belong to a message or a Frist."""

import pytest

from makotest import (
    assert_bo4e_generation_matches,
    assert_invoice_reconciles,
    bo4e_schema_version,
)


def rechnung(**overrides) -> dict:
    """A Rechnung whose four identities all hold, before `overrides`."""
    return {
        "rechnungspositionen": [
            {
                "teilsummeNetto": {"wert": "39.99"},
                "teilsummeSteuer": {"steuerwert": "7.60"},
            },
            {
                "teilsummeNetto": {"wert": "79.98"},
                "teilsummeSteuer": {"steuerwert": "15.20"},
            },
        ],
        "gesamtnetto": {"wert": "119.97", "waehrung": "EUR"},
        "gesamtsteuer": {"wert": "22.80"},
        "gesamtbrutto": {"wert": "142.77"},
        "vorausgezahlt": {"wert": "100.00"},
        "zuZahlen": {"wert": "42.77"},
        **overrides,
    }


class TestInvoiceReconciliation:
    def test_an_invoice_whose_totals_all_agree_passes(self):
        assert_invoice_reconciles(rechnung())

    def test_positions_that_do_not_add_up_are_reported(self):
        with pytest.raises(AssertionError, match="Σ teilsummeNetto"):
            assert_invoice_reconciles(rechnung(gesamtnetto="100.00"))

    def test_a_wrong_zu_zahlen_is_caught_even_when_the_positions_are_right(self):
        """The identity that decides what gets collected.

        An invoice whose positions add up and whose `zuZahlen` is wrong is the
        defect that reaches a customer: the positions are what a reviewer reads,
        and `zuZahlen` is what the bank takes.
        """
        with pytest.raises(AssertionError, match="zuZahlen"):
            assert_invoice_reconciles(rechnung(zuZahlen="142.77"))

    def test_net_plus_tax_must_equal_gross(self):
        with pytest.raises(AssertionError, match="gesamtbrutto"):
            assert_invoice_reconciles(rechnung(gesamtbrutto="130.00", zuZahlen="30.00"))

    def test_the_vat_lines_must_add_up_too(self):
        with pytest.raises(AssertionError, match="teilsummeSteuer"):
            assert_invoice_reconciles(rechnung(gesamtsteuer="10.00"))

    def test_a_partial_invoice_is_asserted_for_what_it_states(self):
        """Each identity is checked only when both of its sides are present."""
        assert_invoice_reconciles(
            {
                "gesamtnetto": "119.97",
                "rechnungspositionen": [
                    {"teilsummeNetto": "39.99"},
                    {"teilsummeNetto": "79.98"},
                ],
            }
        )

    def test_money_is_compared_as_decimal_not_float(self):
        """A cent is not representable in binary floating point.

        Ten positions of 0.10 sum to 1.0000000000000002 in float arithmetic; the
        assertion must not depend on which side of the tolerance that lands.
        """
        assert_invoice_reconciles(
            {
                "gesamtnetto": "1.00",
                "rechnungspositionen": [{"teilsummeNetto": "0.10"}] * 10,
            },
            tolerance_eur="0",
        )

    def test_a_scalar_and_a_com_money_field_are_both_accepted(self):
        assert_invoice_reconciles(
            {"gesamtnetto": 5, "rechnungspositionen": [{"teilsummeNetto": {"wert": 5}}]}
        )

    def test_an_invoice_with_no_positions_must_have_no_total(self):
        assert_invoice_reconciles({"gesamtnetto": "0.00"})
        with pytest.raises(AssertionError):
            assert_invoice_reconciles({"gesamtnetto": "10.00"})

    def test_a_discount_is_deducted_from_what_is_owed(self):
        assert_invoice_reconciles(
            {
                "gesamtbrutto": "142.77",
                "rabattBrutto": "12.77",
                "zuZahlen": "130.00",
            }
        )


class TestBo4eGeneration:
    def test_the_expected_generation_comes_from_the_bundled_crates(self):
        """Asked of `rubo4e`, never written down — so it cannot drift."""
        assert bo4e_schema_version().startswith("v202607")

    def test_matching_generations_pass_in_either_spelling(self):
        assert_bo4e_generation_matches("202607")
        assert_bo4e_generation_matches("v202607.0.0")

    def test_a_different_generation_is_refused(self):
        with pytest.raises(AssertionError, match="BO4E generation mismatch"):
            assert_bo4e_generation_matches("v202710.0.0")
