"""The Rust identifier bindings — the check-digit contract."""

import pytest

from makotest import malo_check_digit, malo_from_base, malo_is_valid, melo_is_valid


def test_known_valid_malo_is_accepted():
    # The worked example from the BDEW check-digit definition.
    assert malo_is_valid("51238696012")


def test_check_digit_completes_a_base():
    # The base of the fixture above, so this file states one MaLo consistently:
    # 5·1·2·3·8·6·9·6·0·1 weights to 58, and 60 − 58 = 2.
    assert malo_from_base("5123869601") == "51238696012"
    assert malo_check_digit("5123869601") == 2


def test_wrong_check_digit_is_rejected():
    # Same base, every other check digit must fail — this is the whole point of
    # generating MaLos via `malo_from_base` instead of inventing 11 digits.
    for d in range(10):
        candidate = f"5123869601{d}"
        assert malo_is_valid(candidate) == (candidate == "51238696012")


@pytest.mark.parametrize(
    "bad",
    ["", "512386967", "512386967812", "5123869678X", "abcdefghijk"],
)
def test_malformed_malos_are_rejected(bad):
    assert not malo_is_valid(bad)


def test_melo_is_33_characters():
    # From the MSCONS MIG 2.5 worked example for LOC+172.
    assert melo_is_valid("DE00014559929E00856996N5139699L01")


def test_an_11_digit_malo_is_not_a_melo():
    """MaLo and MeLo are different schemes; 11 digits is never a MeLo."""
    assert not melo_is_valid("51238696012")
