"""Financial evidence for the isolated LF close, collected with every sender stopped."""

import base64
import contextlib
import json
import os
import secrets
import select
import socket
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
_recipient = None
_binary = None
_key = None
_listener = None
_received_contacts = 0


def prepare(binary):
    global _recipient, _binary, _key, _listener, _received_contacts
    _received_contacts = 0
    _binary = binary
    _recipient = tempfile.TemporaryDirectory(prefix="mako-drill-recipient-")
    _key = "whsec_" + base64.b64encode(secrets.token_bytes(32)).decode("ascii")
    _listener = socket.create_server(("127.0.0.1", 0))


def close():
    if _listener is not None:
        _listener.close()
    if _recipient is not None:
        _recipient.cleanup()


def _delivery(db, mode):
    global _received_contacts
    result = subprocess.run(
        [_binary, mode, str(db.port), Path(_recipient.name) / "receipt.json"],
        cwd=ROOT,
        stdin=_listener,
        stdout=subprocess.PIPE,
        text=True,
        check=True,
        timeout=60,
        env={
            **os.environ,
            "NO_PROXY": "*",
            "no_proxy": "*",
            "MAKO_DRILL_PASSWORD": db.password,
            "MAKO_DRILL_SIGNING_KEY": _key,
        },
    )
    report = json.loads(result.stdout)
    _received_contacts += report["wire_attempts"]
    return report


def receiver_contacts():
    """Count handled HTTP attempts and queued TCP connections while the receiver is idle."""
    global _received_contacts
    while select.select([_listener], [], [], 0)[0]:
        try:
            connection, _ = _listener.accept()
        except ConnectionAbortedError:
            pass  # The contact reached the listener and its client disconnected.
        else:
            connection.close()
        _received_contacts += 1
    return _received_contacts


def receipt():
    return json.loads((Path(_recipient.name) / "receipt.json").read_text())


@contextlib.contextmanager
def receipt_unavailable():
    path = Path(_recipient.name) / "receipt.json"
    withheld = path.with_suffix(".withheld")
    path.rename(withheld)
    try:
        yield
    finally:
        withheld.rename(path)


def snapshot(db):
    billing = json.loads(
        db.sql(
            "billingd",
            """
      SELECT jsonb_build_object(
        'run', (SELECT jsonb_build_object('id',run_id, 'status',status, 'count',candidate_count,
            'sha256',encode(sha256(convert_to(to_jsonb(r)::text,'UTF8')),'hex'))
            FROM billing_discovery r),
        'invoice_counter', (SELECT last_value FROM invoice_number_series),
        'issuer_run', (SELECT to_jsonb(i) - 'tenant' - 'lf_mp_id' FROM billing_discovery_issuers i),
        'invoice', (SELECT jsonb_build_object('id',id, 'number',rechnungsnummer, 'state',outcome,
            'run_id', jsonb_path_query_first(rechnung_json,
               '$.zusatzAttribute[*] ? (@.name == "mako:billing_run_id").wert'),
            'gross_cents',(total_brutto_eur*100)::bigint, 'due_on',due_on,
            'sha256',encode(sha256(convert_to(to_jsonb(i)::text,'UTF8')),'hex')) FROM billing_records i),
        'delivery', (SELECT jsonb_build_object('id',e.event_id,'record_id',e.envelope#>>'{data,record_id}',
            'delivery_id',d.id,'endpoint_id',w.id,'attempts',d.attempts,'delivered',d.delivered_at IS NOT NULL,
            'dead',d.dead_lettered_at IS NOT NULL,'fanned',e.fanned_out_at IS NOT NULL,
            'destination_sha256',encode(sha256(convert_to(to_jsonb(w)::text,'UTF8')),'hex'),
            'body_sha256',encode(sha256(convert_to(e.envelope::text,'UTF8')),'hex'))
            FROM webhook_deliveries d JOIN webhook_endpoints w ON w.id=d.endpoint_id
            JOIN event_outbox e ON e.event_id=d.event_id),
        'queue_counts',jsonb_build_array((SELECT count(*) FROM event_outbox),
            (SELECT count(*) FROM webhook_endpoints),(SELECT count(*) FROM webhook_deliveries)));
      """,
        )
    )
    accounting = json.loads(
        db.sql(
            "accountingd",
            """
      SELECT jsonb_build_object(
        'credit', (SELECT jsonb_build_object('id',payout_id,'source_id',source_ce_id,'number',rechnungsnummer,
            'amount_cents',amount_ct,'state',status,'due_on',due_on,
            'sha256',encode(sha256(convert_to(to_jsonb(c)::text,'UTF8')),'hex')) FROM credit_payouts c),
        'ledger', jsonb_build_object(
            'balance_cents',(SELECT balance_ct FROM accounts),
            'journal_sha256',(SELECT encode(sha256(convert_to(jsonb_agg(to_jsonb(l) ORDER BY position)::text,'UTF8')),'hex') FROM ledger.leaves l),
            'entries',(SELECT count(*) FROM ledger.entries),
            'postings',(SELECT count(*) FROM ledger.postings),
            'entries_sha256',(SELECT encode(sha256(convert_to(jsonb_agg(to_jsonb(e) ORDER BY position)::text,'UTF8')),'hex') FROM ledger.entries e),
            'postings_sha256',(SELECT encode(sha256(convert_to(jsonb_agg(to_jsonb(p) ORDER BY position,idx)::text,'UTF8')),'hex') FROM ledger.postings p),
            'invoice_bookings',(SELECT count(*) FROM booking_drafts b JOIN credit_payouts c
                ON c.tenant=b.tenant AND c.source_ce_id=b.key WHERE b.status='gebucht'),
            'settled_advances',(SELECT count(*) FROM abschlag_forderungen a JOIN credit_payouts c
                ON c.tenant=a.tenant AND c.lf_mp_id=a.lf_mp_id AND c.malo_id=a.malo_id
                AND a.verrechnet_mit=c.rechnungsnummer)));
      """,
        )
    )
    return {**billing, **accounting}


def initialize(db):
    first = _delivery(db, "seed")
    evidence = snapshot(db)
    run, invoice, delivery = (evidence[k] for k in ("run", "invoice", "delivery"))
    ledger, credit = (evidence[k] for k in ("ledger", "credit"))
    if not (
        first["wire_attempts"] == 1
        and run["id"] == invoice["run_id"] == evidence["issuer_run"]["run_id"]
        and run["status"] == "completed"
        and run["count"] == evidence["issuer_run"]["billed"] == 1
        and invoice["state"] == "dispatched"
        and invoice["gross_cents"] == 11900
        and invoice["id"] == delivery["record_id"]
        and credit["number"] == invoice["number"]
        and delivery["id"]
        == credit["source_id"]
        == receipt()["event_id"]
        == first["event_id"]
        and evidence["queue_counts"] == [1, 1, 1]
        and delivery["fanned"]
        and delivery["endpoint_id"] == receipt()["endpoint_id"]
        and delivery["attempts"] == 1
        and not delivery["delivered"]
        and not delivery["dead"]
        and credit["state"] == "open"
        and credit["amount_cents"] == -ledger["balance_cents"] == 4217
        and ledger["invoice_bookings"] == ledger["settled_advances"] == 1
        and ledger["entries"] >= 4
        and ledger["postings"] >= 8
    ):
        raise RuntimeError(
            "source financial obligations and recipient receipt do not reconcile"
        )


def skew(db, database="accountingd"):
    # A valid independent database generation can still disagree with the issued invoice.
    changed = db.sql(
        database, "UPDATE credit_payouts SET amount_ct=amount_ct+1 RETURNING amount_ct"
    )
    if changed != "4218":
        raise RuntimeError("skew fixture must change exactly one open credit")


def skew_endpoint(db, database):
    changed = db.sql(
        database,
        "UPDATE webhook_endpoints SET url=url || '/unreconciled' RETURNING name",
    )
    if changed != "default":
        raise RuntimeError("the cut must change exactly one destination")


def resume(db):
    before = snapshot(db)
    first_receipt = receipt()
    if (
        before["delivery"]["delivered"]
        or before["delivery"]["dead"]
        or before["delivery"]["attempts"] != 1
    ):
        raise RuntimeError("resume requires the original pending delivery")
    result = _delivery(db, "resume")
    after = snapshot(db)
    expected = {
        **before,
        "delivery": {**before["delivery"], "delivered": True, "attempts": 2},
    }
    if after != expected or receipt() != first_receipt or result["wire_attempts"] != 1:
        raise RuntimeError(
            "restored delivery changed the financial obligations or recipient evidence"
        )
    return {
        "event_id": result["event_id"],
        "attempts": after["delivery"]["attempts"],
        "effects": after["ledger"]["invoice_bookings"],
        "credit_cents": after["credit"]["amount_cents"],
        "acknowledged": True,
    }
