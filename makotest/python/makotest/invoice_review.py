"""Compare the received ESA invoice with its independently frozen position records."""

import json
import re
from decimal import ROUND_HALF_UP, Decimal, DecimalException, localcontext

from . import _native
from .rehearsal import _require
from .shadow import _fingerprint, _instant, _record
from .shadow_io import ShadowInputError, read_manifest


def _amount(value):
    _require(
        isinstance(value, str)
        and re.fullmatch(r"-?[0-9]{1,18}(?:\.[0-9]{1,5})?", value) is not None,
        "invalid_amount",
    )
    return Decimal(value)


def compare(invoice, reference):
    ref = _record(
        reference,
        "version invoice_fingerprint invoice_number received_at known_order "
        "duplicate_invoice expected_remadv lines",
    )
    _require(type(ref["version"]) is int and ref["version"] == 1, "unsupported_version")
    _require(
        isinstance(invoice, dict)
        and _fingerprint(invoice) == ref["invoice_fingerprint"]
        and isinstance(ref["invoice_number"], str)
        and ref["invoice_number"]
        and invoice.get("rechnungsnummer") == ref["invoice_number"],
        "unpaired_invoice",
    )
    _instant(ref["received_at"])
    _require(
        type(ref["known_order"]) is bool
        and type(ref["duplicate_invoice"]) is bool
        and type(ref["expected_remadv"]) is int
        and ref["expected_remadv"] in (33001, 33003, 33004),
        "invalid_reference",
    )
    positions = invoice.get("rechnungspositionen")
    _require(
        isinstance(positions, list)
        and isinstance(ref["lines"], list)
        and 0 < len(positions) == len(ref["lines"]) <= 10000,
        "incomplete_positions",
    )
    by_id = {p["positionsnummer"]: p for p in positions}
    _require(
        len(by_id) == len(positions)
        and all(type(n) is int and 0 < n <= 65535 for n in by_id),
        "ambiguous_positions",
    )
    prices = {}
    services = []
    rows = []
    with localcontext() as ctx:
        ctx.prec = 64
        cent = Decimal("0.01")
        for item in ref["lines"]:
            line = _record(
                item, "position article unit quantity unit_price performed already_billed"
            )
            n = line["position"]
            _require(
                type(n) is int
                and n in by_id
                and type(line["performed"]) is bool
                and (
                    line["already_billed"] is None
                    or (
                        isinstance(line["already_billed"], str)
                        and bool(line["already_billed"])
                    )
                ),
                "invalid_position",
            )
            pos = by_id.pop(n)
            article = pos["artikelId"]
            _require(
                isinstance(article, str)
                and bool(article)
                and article == line["article"]
                and isinstance(line["unit"], str)
                and bool(line["unit"])
                and pos.get("zeitbezogeneMenge") is None,
                "unpaired_position",
            )
            services.append((n, line["performed"], line["already_billed"]))
            price_record = pos.get("einzelpreis")
            quantity_record = pos.get("positionsMenge")
            net_record = pos.get("gesamtpreis")
            for record in (price_record, quantity_record, net_record):
                _require(record is None or isinstance(record, dict), "invalid_position")
            if (
                line["unit_price"] is None
                or not price_record
                or price_record.get("wert") is None
            ):
                rows.append(
                    {"position": n, "status": "unassessed", "reason": "missing_price"}
                )
                continue
            if (
                line["quantity"] is None
                or not quantity_record
                or quantity_record.get("wert") is None
            ):
                rows.append(
                    {"position": n, "status": "unassessed", "reason": "missing_quantity"}
                )
                continue
            if not net_record or net_record.get("wert") is None:
                rows.append(
                    {"position": n, "status": "unassessed", "reason": "missing_net"}
                )
                continue
            _require(
                quantity_record.get("einheit") == line["unit"]
                and net_record.get("waehrung") == "EUR"
                and price_record.get("einheit", "EUR") == "EUR"
                and price_record.get("bezugswert", line["unit"]) == line["unit"],
                "unpaired_position",
            )
            price = _amount(line["unit_price"])
            _require(prices.setdefault(article, price) == price, "ambiguous_price")
            qty = _amount(line["quantity"])
            expected = (qty * price).quantize(cent, rounding=ROUND_HALF_UP)
            actual = _amount(net_record["wert"]).quantize(cent, rounding=ROUND_HALF_UP)
            different = [
                field
                for field, diff in (
                    ("quantity", _amount(quantity_record["wert"]) != qty),
                    ("unit_price", _amount(price_record["wert"]) != price),
                    ("net", actual != expected),
                )
                if diff
            ]
            rows.append(
                {
                    "position": n,
                    "status": "different" if different else "matched",
                    "expected_net": str(expected),
                    "actual_net": str(actual),
                    "differences": different,
                }
            )
    pid, codes, open_steps = _native.esa_invoice_verdict(
        json.dumps(invoice),
        ref["received_at"],
        ref["known_order"],
        ref["duplicate_invoice"],
        [(k, str(v)) for k, v in prices.items()],
        services,
    )
    missing = any(row["status"] == "unassessed" for row in rows)
    status = (
        "unassessed"
        if missing or pid is None
        else "different"
        if pid != ref["expected_remadv"] or any(r["status"] == "different" for r in rows)
        else "matched"
    )
    classifications = []
    if any(row["status"] == "different" for row in rows):
        classifications.append("wert")
    if pid is not None and pid != ref["expected_remadv"]:
        classifications.append("antwort")
    if status == "unassessed":
        classifications.append("unassessed")
    return {
        "version": 1,
        "status": status,
        "classifications": classifications or ["gleich"],
        "invoice_fingerprint": ref["invoice_fingerprint"],
        "reference_fingerprint": _fingerprint(reference),
        "positions": sorted(rows, key=lambda r: r["position"]),
        "expected_remadv": ref["expected_remadv"],
        "actual_remadv": pid,
        "codes": codes,
        "open_steps": open_steps,
    }


def run(args):
    try:
        result = compare(read_manifest(args.invoice), read_manifest(args.reference))
    except ShadowInputError as error:
        result = {"version": 1, "status": "unassessed", "reason": error.code}
    except (
        KeyError,
        IndexError,
        TypeError,
        ValueError,
        RuntimeError,
        OverflowError,
        DecimalException,
        OSError,
    ):
        result = {"version": 1, "status": "unassessed", "reason": "invalid_input"}
    print(json.dumps(result, sort_keys=True))
    return {"matched": 0, "different": 1, "unassessed": 2}[result["status"]]
