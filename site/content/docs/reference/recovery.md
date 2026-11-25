+++
title = "Cold restore of a Lieferant close"
description = "Restore an issued invoice, its credit and a pending delivery into fresh stores and verify when the local sender can resume."
weight = 19
+++

Use `just recovery-drill` to rehearse the decision to resume sending after an acknowledgement is lost. The synthetic Lieferant has issued a €119.00 Schlussrechnung. A paid advance of €161.17 leaves the customer a €42.17 credit. The accounting receiver has accepted the invoice event, but billingd still has it queued. Retrying must preserve one credit and one invoice posting.

## Run the rehearsal

From the repository root on Linux or macOS, use Docker, Python 3.10 or newer and the pinned Rust toolchain. The command builds the fixture binaries, uses the PostgreSQL image pinned in the development Compose file, and creates its own data directories and databases. It can run alongside the demo. The first build and image pull can take longer than the restore.

```bash
just recovery-drill --help
just recovery-drill
just recovery-drill --json > recovery-result.json
```

The default runs a clean restore and all the refusal cases. It exits successfully only if the clean target resumes and every deliberately inconsistent target stays held for its expected reason. To inspect a single refusal, use `just recovery-drill --case skewed --json`; the underlying command exits with status 2. An operational failure exits with status 1. For automation that needs the exact child exit code, invoke `python3 deploy/recovery/drill.py` directly.

## What has to agree

| Owner | At the checkpoint | Before sending resumes |
|---|---|---|
| billingd · PostgreSQL | LF discovery run, issued invoice, stable pending CloudEvent | Verify run and invoice identity, issuer, amounts, event identity and content. |
| accountingd · PostgreSQL | Paid advance, invoice posting, outstanding credit | Verify the committed posting and the credit’s stable identity, amount and status. |
| makod · local SlateDB | LF Anmeldung with a committed event and pending UTILMD | Re-fold the event, verify its process state and compare the pending message and its originating event. The protocol sender stays offline. |
| Local receiver · independent receipt | Identity and content of the accepted financial event | Match the independent receipt before retry. Check the received attempts, business effects and local queue after resuming. |

Source writers finish before capture. The runner closes the engine, freezes the financial databases, records its evidence and removes the source PostgreSQL container. It restores each database from its custom archive into a new target and opens a separate copy of the engine tree. PostgreSQL provides a consistent snapshot for one database per dump. Quiescing writers across both databases establishes the common cut used here. [PostgreSQL 17, SQL Dump §25.1](https://www.postgresql.org/docs/17/backup-dump.html) · [pg_restore, single-transaction option](https://www.postgresql.org/docs/17/app-pgrestore.html)

## Read the verdict

| Case | Operator decision |
|---|---|
| `clean` | The financial sender can retry the original event against the local receiver. Confirm the queue clears and the credit and posting remain single. |
| `missing` / `damaged` | Keep the sender held. Obtain the missing archive or an intact copy that matches the checkpoint. |
| `skewed` | The archive is readable and its digest agrees, but the credit differs from the pre-loss evidence. Recover both owners from the same cut. |
| `engine-skewed` | The local tree is intact, but its process or pending protocol message has another identity. Recover the engine checkpoint belonging to this cut. |
| `destination-changed` | Keep sending held. Reconcile the subscription ID and destination with the receiver that accepted the original delivery. |
| `receipt-missing` | Keep the sender held. Reconcile the original event ID and content with the receiver’s independently retained record. |

Each held result includes `additional_receiver_contacts`. The rehearsal counts completed requests and checks the original listener for queued connections. Any additional contact fails the rehearsal.

The clean result gives elapsed milliseconds to reconciled business reads and to a cleared financial backlog. Compare those measurements with the objective for your own installation. The reported loss count covers the committed synthetic fixture. A repeated exercise can yield different timings and independently generated event IDs.

The profile covers two PostgreSQL databases and local engine storage. For a deployment rehearsal, agree on the acceptable recovery point and the time to resume the first useful business operation before recording the result. [NIST, Recovery Point Objective](https://csrc.nist.gov/glossary/term/recovery_point_objective) · [NIST, Recovery Time Objective](https://csrc.nist.gov/glossary/term/recovery_time_objective)

A production recovery also needs an inventory of its enabled stores, roles, keys, documents, external recipients and delivery evidence. Cloud writer takeover, standby promotion and a live market send each require separate evidence.
