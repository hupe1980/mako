-- ── invoicd schema — INVOIC plausibility & settlement ────────────────────────
--
-- § 147 AO / § 14b UStG: a received INVOIC is a received invoice — a
-- Buchungsbeleg — and must be retained for 8 years (§ 147 Abs. 3 AO as amended
-- by BEG IV, in force 01.01.2025). Every INVOIC is persisted here **before** the
-- REMADV/COMDIS command is dispatched to makod (persist-before-dispatch).
--
-- Supported PIDs: 31001 31002 31003 31004 31005 31006 31007 31008 31009 31011

CREATE TABLE invoic_receipts (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Business key: one row per billing process.
    process_id              UUID        NOT NULL UNIQUE,

    -- The EDIFACT INVOIC message reference (BGM 1004) — the business key makod
    -- routes an answer command by, so a re-dispatch that has only the receipt
    -- row still needs it. NULL for outbound self-issued documents, which are
    -- identified by `rechnungsnummer` instead.
    invoice_ref             TEXT,
    -- Invoice number of a document this service issued (outbound only).
    rechnungsnummer         TEXT,

    -- BDEW Prüfidentifikator that triggered this receipt.
    pid                     SMALLINT    NOT NULL,
    -- 'inbound' (NB/MSB → LF) | 'outbound' (LF selbstausgestellt, PID 31006)
    direction               TEXT        NOT NULL CHECK (direction IN ('inbound', 'outbound')),

    -- BDEW-Codenummer / GLN of the sender.
    sender_mp_id            TEXT        NOT NULL,
    -- Receiver GLN (this tenant for inbound; the NB for outbound).
    receiver_gln            TEXT,

    -- MaLo-ID extracted from the Rechnung at ingest (indexed; no JSONB scan).
    malo_id                 TEXT,

    -- The BO4E Rechnung exactly as received, with the schema version it was
    -- read under. Server-derived provenance, never a client-supplied value.
    rechnung                JSONB       NOT NULL,
    bo4e_version            TEXT        NOT NULL,

    -- Plausibility outcome.
    outcome                 TEXT        NOT NULL CHECK (outcome IN (
                                'Ok',               -- accepted
                                'AcceptedPartial',  -- accepted with remarks (Storno)
                                'Warn',             -- warnings, auto-approved
                                'Dispute',          -- rejected
                                'Resolved',         -- dispute closed by an operator
                                'Dispatched',       -- outbound sent; awaiting the NB's REMADV
                                'Paid'              -- outbound settled by the NB
                            )),

    -- invoic_checker findings (Vec<Finding> as JSONB).
    findings                JSONB       NOT NULL DEFAULT '[]',

    -- Zahlungsziel from INVOIC DTM+92 (rubo4e Rechnung.faelligkeitsdatum).
    pay_by                  TIMESTAMPTZ,

    -- Dispute resolution.
    dispute_resolved_at     TIMESTAMPTZ,
    dispute_resolution_note TEXT,

    received_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    checked_at              TIMESTAMPTZ,
    dispatched_at           TIMESTAMPTZ,
    payment_confirmed_at    TIMESTAMPTZ,

    -- Set once `de.invoic.payment.overdue` has gone out for this receipt, so a
    -- passed Zahlungsziel is announced once rather than every six hours until
    -- somebody acts on it.
    overdue_notified_at     TIMESTAMPTZ,

    -- ERP notification outbox (durable at-least-once).
    -- Backoff 30 s → 5 min → 30 min → 2 h; dead-lettered at `erp_attempts = 5`.
    erp_notified_at         TIMESTAMPTZ,
    erp_attempts            SMALLINT    NOT NULL DEFAULT 0,
    erp_next_attempt_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    tenant                  TEXT        NOT NULL,

    -- An inbound receipt without its message reference cannot be answered:
    -- there is nothing to route the REMADV by. Refuse the row rather than
    -- discover it when the Zahlungsziel is a day out.
    CONSTRAINT invoic_inbound_has_message_ref CHECK (
        direction <> 'inbound' OR invoice_ref IS NOT NULL
    ),
    -- A dispute resolution has both parts or neither.
    CONSTRAINT invoic_resolution_timed CHECK (
        (outcome = 'Resolved') = (dispute_resolved_at IS NOT NULL)
    )
);

COMMENT ON TABLE invoic_receipts IS
    '§ 147 AO / § 14b UStG: INVOIC receipts are Buchungsbelege — 8-year retention. '
    'Written before REMADV/COMDIS dispatch (persist-before-dispatch).';

COMMENT ON COLUMN invoic_receipts.invoice_ref IS
    'EDIFACT INVOIC message reference (BGM 1004) — the business key makod routes the answer command by.';

-- Per-counterparty billing history.
CREATE INDEX invoic_sender_received   ON invoic_receipts (sender_mp_id, received_at DESC);
-- Prüfschritt 50 („Rechnungsnummer wurde bereits verwendet", `A05`) of every
-- invoice tree in `mako_pruefung::rechnung`. § 14 Abs. 4 Nr. 4 UStG makes the
-- Rechnungsnummer einmalig **per Rechnungssteller**, so that pair is the key;
-- the partial predicate skips the outbound rows and the ones that never carried
-- a number.
CREATE INDEX invoic_sender_rechnungsnummer
    ON invoic_receipts (tenant, sender_mp_id, rechnungsnummer)
    WHERE rechnungsnummer IS NOT NULL;
-- Tenant-scoped dashboard.
CREATE INDEX invoic_tenant_received   ON invoic_receipts (tenant, received_at DESC);
-- MaLo payment lifecycle.
CREATE INDEX invoic_malo_tenant       ON invoic_receipts (tenant, malo_id)
    WHERE malo_id IS NOT NULL;
-- Exception queue (skips the majority of settled rows).
CREATE INDEX invoic_outcome           ON invoic_receipts (tenant, outcome)
    WHERE outcome NOT IN ('Ok', 'Paid');
-- Zahlungsziel approaching without a dispatched answer.
CREATE INDEX invoic_pay_by_pending    ON invoic_receipts (tenant, pay_by)
    WHERE pay_by IS NOT NULL AND dispatched_at IS NULL;
-- The overdue-payment sweep.
CREATE INDEX invoic_overdue_pending   ON invoic_receipts (tenant, pay_by)
    WHERE pay_by IS NOT NULL
      AND payment_confirmed_at IS NULL
      AND overdue_notified_at IS NULL
      AND dispatched_at IS NOT NULL;
-- The ERP outbox claim (see pg::receipts::claim_erp_pending).
CREATE INDEX invoic_erp_pending       ON invoic_receipts (tenant, erp_next_attempt_at)
    WHERE erp_notified_at IS NULL AND erp_attempts < 5;
-- Inbound vs. outbound summary.
CREATE INDEX invoic_direction_tenant  ON invoic_receipts (tenant, direction, received_at DESC);

-- ── Dead-letter queue ─────────────────────────────────────────────────────────
--
-- An INVOIC that could not be turned into a receipt: an unparseable Rechnung, a
-- payload with no message reference, a Rechnung makod cannot supply. These are
-- Buchungsbelege that arrived and were not processed, so they are kept and
-- alerted on rather than dropped in a log line.

CREATE TABLE invoic_dlq (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- The process the event named, when it had one. Redelivery of the same
    -- event must not pile up rows.
    process_id      UUID,
    pid             SMALLINT,
    malo_id         TEXT,
    raw_event       JSONB       NOT NULL,
    failure_reason  TEXT        NOT NULL,
    failed_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at     TIMESTAMPTZ,
    tenant          TEXT        NOT NULL
);

COMMENT ON TABLE invoic_dlq IS
    'INVOIC events that could not be processed. Alert when unresolved entries are older than 1 hour.';

-- Redelivery of the same failing event updates the row instead of adding one.
CREATE UNIQUE INDEX invoic_dlq_process ON invoic_dlq (tenant, process_id)
    WHERE process_id IS NOT NULL;
CREATE INDEX invoic_dlq_open ON invoic_dlq (tenant, failed_at DESC)
    WHERE resolved_at IS NULL;
