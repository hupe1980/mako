"""The Rust identifier bindings — construction, not just validation.

A random 11-digit string is a valid Marktlokations-ID one time in ten and a
random 16-character string is essentially never a valid EIC. Every identifier
family therefore has a constructor, and these tests pin them to the BDEW and
ENTSO-E reference vectors rather than to whatever the crate happens to compute.
"""

import pytest

from makotest import (
    bilanzierungsgebiet_from_prefix,
    bilanzierungsgebiet_is_valid,
    bilanzkreis_from_prefix,
    bilanzkreis_is_valid,
    eic_from_prefix,
    eic_is_valid,
    eic_type_char,
    malo_check_digit,
    malo_from_base,
    malo_is_valid,
    melo_is_valid,
    mp_id_authority,
    mp_id_check_digit_schemes,
    mp_id_from_base,
    mp_id_is_valid,
    mp_id_unb_qualifier,
    resource_id_from_base,
    resource_id_is_valid,
    resource_id_kinds,
)


class TestMarktlokation:
    def test_the_bdew_reference_vector(self):
        """§8.1 of the BDEW Anwendungshilfe, worked example."""
        assert malo_from_base("4137355924") == "41373559241"
        assert malo_check_digit("4137355924") == 1
        assert malo_is_valid("41373559241")

    def test_completing_a_base_is_the_only_reliable_way_to_get_one(self):
        assert malo_from_base("5123869601") == "51238696012"
        # Every other check digit for the same base must fail — which is why a
        # test that invents eleven digits exercises rejection, not the happy path.
        for digit in range(10):
            candidate = f"5123869601{digit}"
            assert malo_is_valid(candidate) == (candidate == "51238696012")

    @pytest.mark.parametrize(
        "bad", ["", "512386967", "512386967812", "5123869678X", "abcdefghijk"]
    )
    def test_malformed_malos_are_rejected(self, bad):
        assert not malo_is_valid(bad)

    def test_a_zero_vergabestelle_is_not_a_malo(self):
        """Position 1 is the Codevergabestelle and `0` is unissued."""
        with pytest.raises(ValueError):
            malo_from_base("0123456789")


class TestMesslokation:
    def test_melo_is_33_characters(self):
        assert melo_is_valid("DE00014559929E00856996N5139699L01")

    def test_an_11_digit_malo_is_not_a_melo(self):
        assert not melo_is_valid("51238696012")


class TestMarktpartner:
    def test_the_two_check_digit_schemes_are_different_arithmetic(self):
        """§2.3 defines two procedures, and they disagree on almost every base.

        A fixture that picks 13 digits satisfies neither, and every conformant
        counterparty refuses it — which is why this is a constructor and the
        scheme is an argument rather than a guess from the prefix.
        """
        bdew = mp_id_from_base("990035700000", "bdew")
        gln = mp_id_from_base("990035700000", "gln")
        assert bdew == "9900357000003"
        assert bdew != gln
        assert "bdew" in mp_id_check_digit_schemes(bdew)
        assert "gln" in mp_id_check_digit_schemes(gln)

    def test_an_invented_mp_id_satisfies_no_scheme(self):
        """13 digits is not enough, and this is the value fixtures reach for."""
        assert mp_id_is_valid("9900357000004"), "structurally 13 digits"
        assert mp_id_check_digit_schemes("9900357000004") == []

    @pytest.mark.parametrize(
        ("mp_id", "authority", "qualifier"),
        [
            ("9900357000003", "BDEW", "500"),
            ("9800001000004", "DVGW", "502"),
            ("4012345000023", "GS1 GLN", "14"),
        ],
    )
    def test_the_prefix_decides_the_unb_qualifier(self, mp_id, authority, qualifier):
        """The envelope derives DE0007 from the ID; a mismatch is a Syntaxfehler."""
        assert mp_id_authority(mp_id) == authority
        assert mp_id_unb_qualifier(mp_id) == qualifier

    def test_an_unknown_scheme_is_rejected(self):
        with pytest.raises(ValueError, match="unknown check-digit scheme"):
            mp_id_from_base("990035700000", "luhn")


class TestEic:
    def test_the_entsoe_reference_vector(self):
        assert eic_from_prefix("10YDE-EON------") == "10YDE-EON------1"
        assert eic_is_valid("10YDE-EON------1")
        assert not eic_is_valid("10YDE-EON------2")

    def test_bilanzkreis_is_a_party_and_bilanzierungsgebiet_an_area(self):
        """The object-type character is the *only* thing separating them.

        Both are 16 characters, both carry a valid check character, and MSCONS
        SG6 carries both as free text under different LOC qualifiers — so a
        series filed against the wrong one is a misfiling the BIKO cannot tell
        from a correct submission.
        """
        bk = bilanzkreis_from_prefix("11XSWKIEL------")
        bg = bilanzierungsgebiet_from_prefix("11YSWKIEL------")
        assert eic_type_char(bk) == "X"
        assert eic_type_char(bg) == "Y"
        assert bilanzkreis_is_valid(bk) and not bilanzierungsgebiet_is_valid(bk)
        assert bilanzierungsgebiet_is_valid(bg) and not bilanzkreis_is_valid(bg)

    def test_the_wrong_object_type_is_refused_at_construction(self):
        with pytest.raises(ValueError):
            bilanzkreis_from_prefix("11YSWKIEL------")


class TestResourceIds:
    def test_every_family_round_trips_through_its_own_validator(self):
        """NeLo, NeBe and the four Redispatch resources, plus the Paket-ID.

        These are the objects a UTILMD transaction names alongside a MaLo, so a
        test that builds one has to be able to generate one.
        """
        kinds = dict(resource_id_kinds())
        assert set(kinds) == {"nelo", "nebe", "cr", "sg", "sr", "tr", "paket"}
        for kind, prefix in kinds.items():
            base = prefix + "0" * (10 - len(prefix))
            identifier = resource_id_from_base(kind, base)
            assert len(identifier) == 11
            assert resource_id_is_valid(kind, identifier)

    def test_the_codetyp_is_fixed_by_the_document(self):
        """A NeLo-ID starts with `E`; the letter is not a free choice."""
        with pytest.raises(ValueError, match="Codetyp"):
            resource_id_from_base("nelo", "X000000001")

    def test_an_unknown_family_is_rejected(self):
        with pytest.raises(ValueError, match="unknown resource-ID kind"):
            resource_id_from_base("nosuch", "E000000001")
