"""An invoice position and its independently expected REMADV have separate verdicts."""

import json
from pathlib import Path

import pytest

from makotest.cli import main
from makotest.shadow import _fingerprint

FIXTURE = Path(__file__).parent / "fixtures/shadow/settlement"


@pytest.mark.parametrize(
    ("change", "status", "net"),
    [
        ("match", 0, "13.95"),
        ("cent", 1, "13.95"),
        ("round", 1, "13.96"),
        ("missing", 2, None),
        ("unpriced_invoice", 2, None),
        ("no_quantity", 2, None),
        ("invalid_position", 2, None),
        ("revision", 2, None),
        ("float", 2, None),
        ("reply", 1, "13.95"),
    ],
)
def test_invoice_compares_decimal_evidence_and_the_production_remadv(
    tmp_path, capsys, change, status, net
):
    invoice = json.loads((FIXTURE / "invoice.json").read_bytes())
    reference = json.loads((FIXTURE / "reference.json").read_bytes())
    line = reference["lines"][0]
    if change == "cent":
        invoice["rechnungspositionen"][0]["gesamtpreis"]["wert"] = "13.94"
        reference["invoice_fingerprint"] = _fingerprint(invoice)
    elif change == "round":
        line.update(unit_price="13.955", quantity="1")
    elif change == "missing":
        line["unit_price"] = None
    elif change in {"unpriced_invoice", "no_quantity", "invalid_position"}:
        key, value = {
            "unpriced_invoice": ("einzelpreis", None),
            "no_quantity": ("positionsMenge", None),
            "invalid_position": ("einzelpreis", "broken"),
        }[change]
        invoice["rechnungspositionen"][0][key] = value
        reference["invoice_fingerprint"] = _fingerprint(invoice)
    elif change == "revision":
        reference["invoice_fingerprint"] = "0" * 64
    elif change == "float":
        line["unit_price"] = 0.45
    elif change == "reply":
        reference["expected_remadv"] = 33004
    doc, ref = tmp_path / "invoice.json", tmp_path / "reference.json"
    doc.write_text(json.dumps(invoice))
    ref.write_text(json.dumps(reference))
    assert (
        main(["shadow", "invoice", "--invoice", str(doc), "--reference", str(ref)])
        == status
    )
    report = json.loads(capsys.readouterr().out)
    if net is not None:
        assert report["positions"][0]["expected_net"] == net
    if change == "match":
        assert report["actual_remadv"] == 33001 and report["open_steps"] == []
    if change == "cent":
        assert report["positions"][0]["differences"] == ["net"]
    if change == "missing":
        assert report["positions"][0]["reason"] == "missing_price"
        assert report["actual_remadv"] is None
    if change == "unpriced_invoice":
        assert report["positions"][0]["reason"] == "missing_price"
    if change == "no_quantity":
        assert report["positions"][0]["reason"] == "missing_quantity"
    if change == "invalid_position":
        assert report["reason"] == "invalid_position"
    if change == "reply":
        assert (
            report["actual_remadv"] == 33001
            and report["positions"][0]["status"] == "matched"
        )
