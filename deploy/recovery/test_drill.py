"""Regression checks for cold restore refusals and public error output."""

import io
import json
import socket
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import drill
import fixture


class DrillTests(unittest.TestCase):
    def test_a_missing_real_archive_and_a_link_cannot_reach_restore(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            billing = path / "billingd.dump"
            accounting = path / "accountingd.dump"
            billing.write_bytes(b"valid")
            hashes = dict.fromkeys(drill.DATABASES, drill.digest(b"valid"))
            for code in ("store_missing", "archive_digest_mismatch"):
                with self.assertRaises(drill.Refused) as result:
                    drill.checked_archives(path, hashes)
                self.assertEqual(
                    (result.exception.code, result.exception.store),
                    (code, "accountingd"),
                )
                if not accounting.exists():
                    accounting.symlink_to(billing)

    def test_a_connection_to_a_held_receiver_fails_even_when_its_receipt_is_unchanged(
        self,
    ):
        with (
            socket.create_server(("127.0.0.1", 0)) as listener,
            patch.object(fixture, "_listener", listener),
            patch.object(fixture, "_received_contacts", 0),
            patch.object(fixture, "receipt", return_value={"event_id": "same"}),
        ):
            before = fixture.receiver_contacts()
            self.assertEqual(drill.verify_receiver_held(fixture.receipt(), before), 0)
            with (
                socket.create_connection(listener.getsockname()),
                self.assertRaises(drill.Failed) as caught,
            ):
                drill.verify_receiver_held(fixture.receipt(), before)
            self.assertEqual(caught.exception.code, "unexpected_receiver_contact")
            self.assertEqual(fixture.receiver_contacts(), before + 1)

    def test_dependency_failures_leave_no_private_input_in_the_json_report(self):
        public = io.StringIO()
        with (
            patch.object(sys, "argv", ["drill.py", "--json"]),
            patch.object(drill, "drill", side_effect=RuntimeError("private-input")),
            redirect_stdout(public),
        ):
            self.assertEqual(drill.main(), 1)
        self.assertEqual(json.loads(public.getvalue())["status"], "failed")
        self.assertNotIn("private-input", public.getvalue())
