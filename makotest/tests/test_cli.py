"""The command line.

The people debugging a real inbound message are not always the people writing
the tests, so the same answers have to be reachable from a shell. Exit status is
the contract: 0 when the answer is yes, 1 when it is no, 2 when the question was
malformed — a CI gate built on this command is decoration otherwise.
"""

import json

import pytest

from conftest import ON, utilmd_interchange
from makotest.cli import main


def run(capsys, *argv) -> tuple[int, str]:
    code = main(list(argv))
    return code, capsys.readouterr().out


def run_json(capsys, *argv) -> tuple[int, dict]:
    code, out = run(capsys, "--json", *argv)
    return code, json.loads(out)


@pytest.fixture
def anmeldung_file(tmp_path):
    path = tmp_path / "anmeldung.edi"
    path.write_bytes(utilmd_interchange())
    return str(path)


class TestValidate:
    def test_a_valid_interchange_exits_zero(self, capsys, anmeldung_file):
        code, out = run(capsys, "validate", anmeldung_file, "--on", ON)
        assert code == 0
        assert "valid on" in out
        assert "55001" in out

    def test_an_invalid_interchange_exits_one_and_names_the_rule(self, capsys, tmp_path):
        path = tmp_path / "bad.edi"
        path.write_bytes(utilmd_interchange(melo="NOTAMELO"))
        code, out = run(capsys, "validate", str(path), "--on", ON)
        assert code == 1
        assert "INVALID" in out
        assert "SEM-UTILMD-MALO-FORMAT" in out

    def test_a_vacuous_pass_exits_one(self, capsys, tmp_path):
        """`is_valid` is true and nothing was checked, so this is not a pass.

        The exit status has to say so, or a shell gate reports success for a
        message no rule ever looked at.
        """
        path = tmp_path / "vacuous.edi"
        path.write_bytes(utilmd_interchange(pid=56001))
        code, out = run(capsys, "validate", str(path), "--on", ON)
        assert code == 1
        assert "no AHB rules were applied" in out

    def test_the_json_report_carries_the_findings(self, capsys, tmp_path):
        path = tmp_path / "bad.edi"
        path.write_bytes(utilmd_interchange(melo="NOTAMELO"))
        code, payload = run_json(capsys, "validate", str(path), "--on", ON)
        assert code == 1
        assert payload["valid"] is False
        assert payload["envelope"]["sender_qualifier"] == "14"
        rules = {f["rule_id"] for f in payload["messages"][0]["findings"]}
        assert "SEM-UTILMD-MALO-FORMAT" in rules

    def test_a_missing_file_is_a_usage_error_not_a_verdict(self, capsys, tmp_path):
        assert main(["validate", str(tmp_path / "nope.edi"), "--on", ON]) == 2

    def test_the_reference_date_is_required(self, anmeldung_file):
        with pytest.raises(SystemExit):
            main(["validate", anmeldung_file])


class TestFrist:
    def test_a_published_frist_is_reported_with_its_fundstelle(self, capsys):
        code, out = run(capsys, "frist", "55001")
        assert code == 0
        assert "11:00 on the 1. Werktag" in out
        assert "BK6-24-174" in out

    def test_a_received_instant_resolves_the_deadline(self, capsys):
        code, payload = run_json(
            capsys, "frist", "55001", "--received", "2026-03-02T09:00:00Z"
        )
        assert code == 0
        assert payload["due_at"] == "2026-03-03T11:00:00+01:00"

    def test_an_unquantified_pid_exits_one(self, capsys):
        code, out = run(capsys, "frist", "44020")
        assert code == 1
        assert "unknown, not unbounded" in out


class TestIdentify:
    def test_a_valid_malo_is_classified(self, capsys):
        code, payload = run_json(capsys, "id", "51238696012")
        assert code == 0
        assert payload["kinds"] == ["marktlokation"]

    def test_a_marktpartner_id_reports_its_check_digit_schemes(self, capsys):
        code, payload = run_json(capsys, "id", "9900357000003")
        assert code == 0
        assert payload["authority"] == "BDEW"
        assert "bdew" in payload["check_digit_schemes"]

    def test_an_invented_marktpartner_id_says_every_partner_would_refuse_it(self, capsys):
        code, out = run(capsys, "id", "9900357000004")
        assert code == 0, "structurally it is a Marktpartner-ID"
        assert "NEITHER procedure" in out

    def test_an_eic_reports_its_object_type(self, capsys):
        code, payload = run_json(capsys, "id", "11XSWKIEL------G")
        assert code == 0
        assert payload["eic_type"] == "X"

    def test_nonsense_exits_one(self, capsys):
        code, _ = run(capsys, "id", "not-an-identifier")
        assert code == 1


class TestIntrospection:
    def test_pids_are_listed_for_the_active_profile(self, capsys):
        code, payload = run_json(
            capsys, "pids", "UTILMD", "--on", ON, "--sparte", "STROM"
        )
        assert code == 0
        assert payload["release"].startswith("S")
        assert 55001 in payload["pruefidentifikatoren"]
        assert all(55000 <= p <= 55999 for p in payload["pruefidentifikatoren"])

    def test_a_type_with_no_pids_exits_one(self, capsys):
        code, _ = run(capsys, "pids", "CONTRL")
        assert code == 1

    def test_an_unknown_type_is_a_usage_error(self):
        assert main(["pids", "NOSUCH"]) == 2

    def test_the_format_versions_are_listed(self, capsys):
        code, payload = run_json(capsys, "versions")
        assert code == 0
        assert all(v.startswith("FV") for v in payload["format_versions"])
