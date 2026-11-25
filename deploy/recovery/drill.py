#!/usr/bin/env python3
"""Rehearse a cold restore of the synthetic Lieferant close on isolated, disposable stores."""

import argparse
import contextlib
import hashlib
import json
import os
import secrets
import signal
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import engine
import fixture

ROOT = Path(__file__).resolve().parents[2]
DATABASES = ("billingd", "accountingd")
REFUSALS = {
    "missing": "store_missing",
    "damaged": "archive_digest_mismatch",
    "skewed": "business_evidence_mismatch",
    "destination-changed": "delivery_destination_mismatch",
    "engine-skewed": "process_evidence_mismatch",
    "receipt-missing": "receiver_receipt_unconfirmed",
}

NEXT_ACTIONS = {
    "store_missing": "Retrieve the accountingd archive from the recorded cut. Keep sending held.",
    "archive_digest_mismatch": "Retrieve an intact copy and verify its recorded digest. Keep sending held.",
    "delivery_destination_mismatch": "Retrieve the subscription for the recorded recipient. Keep sending held.",
    "business_evidence_mismatch": "Reconcile the affected financial records with the recorded cut. Keep sending held.",
    "process_evidence_mismatch": "Retrieve the makod store containing the recorded event and outbox item. Keep sending held.",
    "receiver_receipt_unconfirmed": "Reacquire the receiver receipt for the recorded event ID and body digest. Keep sending held.",
    "checkpoint_digest_mismatch": "Retrieve the verified checkpoint manifest. Keep sending held.",
}


class Refused(Exception):
    def __init__(self, code, store):
        super().__init__(code)
        self.code = code
        self.store = store


class Failed(Exception):
    def __init__(self, code, action):
        super().__init__(code)
        self.code = code
        self.action = action


@contextlib.contextmanager
def step(code, action):
    try:
        yield
    except (Refused, Failed):
        raise
    except Exception as error:
        raise Failed(code, action) from error


def command(*args, payload=None, env=None):
    result = subprocess.run(
        args,
        check=False,
        input=payload,
        capture_output=True,
        cwd=ROOT,
        env=env,
        timeout=180,
    )
    if result.returncode:
        # Credentials and financial payloads never belong in a drill report.
        raise RuntimeError(f"{args[0]} {args[1]} exited {result.returncode}")
    return result.stdout


def milliseconds(start):
    return round((time.monotonic() - start) * 1000)


class Database:
    def __init__(self, container, password):
        self.container = container
        self.password = password
        self.port = None

    def execute(self, *args, payload=None):
        return command(
            "docker",
            "exec",
            "-i",
            "-u",
            "postgres",
            self.container,
            *args,
            payload=payload,
        )

    def sql(self, database, sql):
        return (
            self.execute(
                "psql",
                "-XqAt",
                "-v",
                "ON_ERROR_STOP=1",
                "-d",
                database,
                payload=sql.encode(),
            )
            .decode()
            .strip()
        )

    def dump(self, database):
        return self.execute("pg_dump", "--format=custom", database)

    def restore(self, database, archive):
        self.sql("postgres", f"CREATE DATABASE {database} TEMPLATE template0")
        self.execute(
            "pg_restore", "--single-transaction", "-d", database, payload=archive
        )

    @contextlib.contextmanager
    def local_client(self):
        # Each local fixture connection reaches only this networkless container's loopback.
        container = self.container

        class Connection(socketserver.BaseRequestHandler):
            def handle(self):
                subprocess.run(
                    ["docker", "exec", "-i", container, "nc", "127.0.0.1", "5432"],
                    check=False,
                    stdin=self.request,
                    stdout=self.request,
                    stderr=subprocess.DEVNULL,
                )

        with socketserver.ThreadingTCPServer(("127.0.0.1", 0), Connection) as server:
            server.daemon_threads = True
            self.port = server.server_address[1]
            thread = threading.Thread(
                target=server.serve_forever, args=(0.05,), daemon=True
            )
            thread.start()
            try:
                yield
            finally:
                server.shutdown()
                thread.join()
                self.port = None

    def freeze(self):
        if self.port is not None:
            raise RuntimeError(
                "the fixture must close its database listener before the cut"
            )
        clients = """
            SELECT count(*) FROM pg_stat_activity WHERE backend_type = 'client backend'
            AND pid <> pg_backend_pid()
        """
        deadline = time.monotonic() + 5
        while self.sql("postgres", clients) != "0":
            if time.monotonic() >= deadline:
                raise RuntimeError("a writer connection remains at the backup cut")
            time.sleep(0.1)
        for database in DATABASES:
            self.sql(
                "postgres",
                f"ALTER DATABASE {database} SET default_transaction_read_only=on",
            )
            if self.sql(database, "SHOW transaction_read_only") != "on":
                raise RuntimeError("the source did not enter read-only mode")


@contextlib.contextmanager
def postgres(image):
    password = secrets.token_hex(24)
    container = (
        command(
            "docker",
            "create",
            "--network",
            "none",
            "--label",
            "de.mako.recovery=synthetic-lf-close",
            "--tmpfs",
            "/var/lib/postgresql/data",
            "--env",
            "POSTGRES_PASSWORD",
            "--env",
            "POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256 --auth-local=peer",
            image,
            env={**os.environ, "POSTGRES_PASSWORD": password},
        )
        .decode()
        .strip()
    )
    try:
        command("docker", "start", container)
        deadline = time.monotonic() + 90
        while subprocess.run(
            [
                "docker",
                "exec",
                container,
                "pg_isready",
                "-U",
                "postgres",
                "-h",
                "127.0.0.1",
            ],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=10,
        ).returncode:
            if time.monotonic() >= deadline:
                raise RuntimeError("PostgreSQL did not start within 90 seconds")
            time.sleep(0.2)
        yield Database(container, password)
    finally:
        command("docker", "rm", "--force", "--volumes", container)


def digest(content):
    return hashlib.sha256(content).hexdigest()


def capture(source, binary, work, cases):
    for database in DATABASES:
        source.sql("postgres", f"CREATE DATABASE {database} TEMPLATE template0")
    with source.local_client():
        fixture.initialize(source)
    source.freeze()
    alternatives = {}
    for scenario, database, change in (
        ("skewed", "accountingd", fixture.skew),
        ("destination-changed", "billingd", fixture.skew_endpoint),
    ):
        if scenario in cases:
            clone = f"{database}_skewed"
            source.sql("postgres", f"CREATE DATABASE {clone} TEMPLATE {database}")
            change(source, clone)
            data = source.dump(clone)
            (work / f"{clone}.dump").write_bytes(data)
            alternatives[database] = digest(data)
    if "engine-skewed" in cases:
        alternate = work / "alternate-engine"
        alternate.mkdir()
        alternatives["makod-local"] = engine.capture(binary, alternate)["tree_sha256"]

    evidence = fixture.snapshot(source)
    manifest = {
        "business": evidence,
        "receiver_receipt": fixture.receipt(),
        "sha256": {},
        "makod-local": engine.capture(binary, work),
    }
    for database in DATABASES:
        data = source.dump(database)
        (work / f"{database}.dump").write_bytes(data)
        manifest["sha256"][database] = digest(data)
    (work / "checkpoint.json").write_text(json.dumps(manifest))
    return digest((work / "checkpoint.json").read_bytes()), alternatives


def read_archive(path, database):
    if path.is_symlink():
        raise Refused("archive_digest_mismatch", database)
    if not path.is_file():
        raise Refused("store_missing", database)
    return path.read_bytes()


def checked_archives(work, hashes):
    archives = {}
    for database in DATABASES:
        path = work / f"{database}.dump"
        if database not in hashes:
            raise Refused("store_missing", database)
        data = read_archive(path, database)
        if digest(data) != hashes[database]:
            raise Refused("archive_digest_mismatch", database)
        archives[database] = data
    return archives


def candidate_archives(work, directory, checkpoint, alternatives, case):
    # Ordinary files and pre-recorded digests allow even an intact archive from
    # a different cut to reach the independent business reconciliation.
    hashes = checkpoint["sha256"].copy()
    for database in DATABASES:
        path = work / f"{database}.dump"
        if database == "accountingd" and case == "missing":
            continue
        if (database, case) in {
            ("accountingd", "skewed"),
            ("billingd", "destination-changed"),
        }:
            path = work / f"{database}_skewed.dump"
            hashes[database] = alternatives[database]
        data = read_archive(path, database)
        if database == "accountingd" and case == "damaged":
            data = data[: len(data) // 2]
        (directory / f"{database}.dump").write_bytes(data)
    return checked_archives(directory, hashes)


def recover(image, archives, expected, binary, engine_dir, engine_hash, started):
    with postgres(image) as target:
        with step(
            "restore_failed",
            "Verify the PostgreSQL image and retrieve the recorded database archives. Keep sending held.",
        ):
            for database, archive in archives.items():
                target.restore(database, archive)
        observed = fixture.snapshot(target)
        if (
            observed["delivery"]["destination_sha256"]
            != expected["business"]["delivery"]["destination_sha256"]
        ):
            raise Refused("delivery_destination_mismatch", "billingd")
        if observed != expected["business"]:
            differing = sorted(
                key
                for key in expected["business"].keys() | observed.keys()
                if expected["business"].get(key) != observed.get(key)
            )
            raise Refused("business_evidence_mismatch", ", ".join(differing))
        try:
            engine.restore(
                binary,
                engine_dir,
                {**expected["makod-local"], "tree_sha256": engine_hash},
            )
        except engine.ProcessMismatch as error:
            raise Refused("process_evidence_mismatch", "makod-local") from error
        try:
            receiver_receipt = fixture.receipt()
        except (OSError, ValueError) as error:
            raise Refused("receiver_receipt_unconfirmed", "receiver") from error
        if receiver_receipt != expected["receiver_receipt"]:
            raise Refused("receiver_receipt_unconfirmed", "receiver")
        reconciled_ms = milliseconds(started)
        with (
            step(
                "delivery_unconfirmed",
                "Inspect the receiver receipt for the recorded event ID before retrying.",
            ),
            target.local_client(),
        ):
            delivery = fixture.resume(target)
        return {
            "status": "recovered",
            "reconciled_ms": reconciled_ms,
            "backlog_cleared_ms": milliseconds(started),
            "observed_missing_fixture_records": 0,
            "delivery": delivery,
        }


def build_examples():
    """Read the executable paths from Cargo, including cross-target and target-dir settings."""
    expected = {"recovery_store", "restore_delivery"}
    result = subprocess.run(
        [
            "cargo",
            "build",
            "-q",
            "-p",
            "makod",
            "-p",
            "accountingd",
            "--examples",
            "--message-format=json-render-diagnostics",
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        text=True,
        check=True,
    )
    binaries = {}
    for line in result.stdout.splitlines():
        record = json.loads(line)
        if record.get("executable") and record["target"]["name"] in expected:
            binaries[record["target"]["name"]] = Path(record["executable"])
    if binaries.keys() != expected:
        raise RuntimeError("Cargo did not produce both local recovery fixtures")
    return binaries


def verify_receiver_held(receipt_before, contacts_before):
    additional = fixture.receiver_contacts() - contacts_before
    if additional:
        raise Failed(
            "unexpected_receiver_contact",
            "Keep sending held and inspect the local receiver attempts.",
        )
    if fixture.receipt() != receipt_before:
        raise Failed(
            "receiver_receipt_changed",
            "Keep sending held and reconcile the original receipt.",
        )
    return additional


def drill(case):
    image = (
        command(
            "docker", "compose", "-f", "deploy/dev/compose.yaml", "config", "--images"
        )
        .decode()
        .strip()
    )
    try:
        with step(
            "fixture_build_failed",
            "Build the two recovery examples with the pinned Rust toolchain and retry.",
        ):
            binaries = build_examples()
            fixture.prepare(binaries["restore_delivery"])
            binary = binaries["recovery_store"]
        with tempfile.TemporaryDirectory(prefix="mako-close-") as directory:
            work = Path(directory)
            cases = ["clean", *REFUSALS] if case == "all" else [case]
            with (
                step(
                    "source_capture_failed",
                    "Check Docker and the synthetic source fixture. Repeat from a fresh cut.",
                ),
                postgres(image) as source,
            ):
                checkpoint_hash, alternatives = capture(source, binary, work, cases)
                lost_at = time.monotonic()
            # A restore cannot consult the source. Both PostgreSQL data directories
            # are disposable and every effect outside the cut stays with the receiver.
            checkpoint_data = (work / "checkpoint.json").read_bytes()
            if digest(checkpoint_data) != checkpoint_hash:
                raise Refused("checkpoint_digest_mismatch", "checkpoint")
            checkpoint = json.loads(checkpoint_data)
            report = {
                "profile": "synthetic-lf-close",
                "revision": command("git", "rev-parse", "HEAD").decode().strip(),
                "worktree_dirty": bool(command("git", "status", "--porcelain")),
                "postgres_image": image,
                "checkpoint_sha256": checkpoint_hash,
                "checkpoint": checkpoint,
                "cases": [],
            }
            for scenario in cases:
                receipt_before = fixture.receipt()
                contacts_before = fixture.receiver_contacts()
                try:
                    receipt_state = (
                        fixture.receipt_unavailable()
                        if scenario == "receipt-missing"
                        else contextlib.nullcontext()
                    )
                    with (
                        receipt_state,
                        tempfile.TemporaryDirectory(dir=work) as candidate,
                    ):
                        archives = candidate_archives(
                            work, Path(candidate), checkpoint, alternatives, scenario
                        )
                        alternate = scenario == "engine-skewed"
                        answer = recover(
                            image,
                            archives,
                            checkpoint,
                            binary,
                            work / "alternate-engine" if alternate else work,
                            alternatives["makod-local"]
                            if alternate
                            else checkpoint["makod-local"]["tree_sha256"],
                            lost_at if scenario == "clean" else time.monotonic(),
                        )
                except Refused as error:
                    if error.code != REFUSALS.get(scenario):
                        raise
                    additional_contacts = verify_receiver_held(
                        receipt_before, contacts_before
                    )
                    answer = {
                        "status": "held",
                        "reason": error.code,
                        "store": error.store,
                        "receiver_unchanged": True,
                        "additional_receiver_contacts": additional_contacts,
                        "next_action": NEXT_ACTIONS[error.code],
                    }
                if scenario in REFUSALS and answer["status"] != "held":
                    raise RuntimeError(f"{scenario} passed the restore gate")
                report["cases"].append({"case": scenario, **answer})
            return report
    finally:
        fixture.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--json",
        action="store_true",
        help="write the machine-readable report to stdout",
    )
    parser.add_argument(
        "--case",
        choices=["all", "clean", *REFUSALS],
        default="all",
        help="run a single case; a refused restore exits 2",
    )
    args = parser.parse_args()
    try:
        report = drill(args.case)
        code = 2 if args.case in REFUSALS else 0
    except Refused as error:
        report = {
            "status": "held",
            "reason": error.code,
            "store": error.store,
            "next_action": NEXT_ACTIONS[error.code],
        }
        code = 2
    except Failed as error:
        report = {"status": "failed", "reason": error.code, "next_action": error.action}
        code = 1
    except Exception:  # noqa: BLE001 -- the CLI redacts unexpected dependency errors.
        # A dependency exception can carry a private path, DSN or event payload.
        report = {
            "status": "failed",
            "reason": "drill_failed",
            "next_action": "Check Docker and the local fixture build, then repeat the drill.",
        }
        code = 1
    if args.json:
        print(json.dumps(report))
    elif "cases" in report:
        print(f"Lieferant close recovery · {report['revision'][:12]}")
        for case in report["cases"]:
            if case["status"] == "held":
                print(
                    f"{case['case']}: {case['reason']} ({case['store']}). {case['next_action']}"
                )
            else:
                delivery = case["delivery"]
                euro, cents = divmod(delivery["credit_cents"], 100)
                invoice = report["checkpoint"]["business"]["invoice"]["number"]
                print(
                    f"{invoice} · {euro},{cents:02} € credit · {delivery['attempts']} delivery attempts · "
                    f"{delivery['effects']} invoice booking · 1 open credit"
                )
                print(
                    f"Reconciled at {case['reconciled_ms']} ms · financial backlog cleared at "
                    f"{case['backlog_cleared_ms']} ms · 0 missing fixture records · GPKE message remains pending"
                )
    else:
        print(
            f"Recovery {report['status']}: {report['reason']}. {report['next_action']}",
            file=sys.stderr,
        )
    return code


if __name__ == "__main__":
    # SIGTERM must unwind the same temporary stores and containers as Ctrl-C.
    signal.signal(signal.SIGTERM, lambda signum, _frame: sys.exit(128 + signum))
    sys.exit(main())
