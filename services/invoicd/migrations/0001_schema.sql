-- ── invoicd schema — INVOIC Plausibility & Settlement ────────────────────────
--
-- §22 MessZV: INVOIC receipts must be retained for 3 years.
-- Every INVOIC event received is persisted here before any REMADV/COMDIS
-- command is dispatched to makod (persist-before-dispatch invariant).
--
-- Supported PIDs: 31001 31002 31005 31006 31007 31008 31009 31011

CREATE TABLE invoic_receipts (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Business key: one row per billing process
    process_id              UUID        NOT NULL UNIQUE,

    -- BDEW PID that triggered this receipt
    pid                     SMALLINT    NOT NULL,
    -- 'inbound' (NB/MSB → LF) | 'outbound' (LF selbstausgestellt 31006)
    direction               TEXT        NOT NULL CHECK (direction IN ('inbound', 'outbound')),

    -- GL/BDEW-Codenummer of the sender
    sender_mp_id            TEXT        NOT NULL,
    -- Receiver GLN (tenant for inbound; NB for outbound 31006)
    receiver_gln            TEXT,

    -- MaLo-ID extracted from rechnung (indexed for payment-lifecycle queries)
    malo_id                 TEXT,

    -- Full rubo4e::current::Rechnung JSONB
    rechnung                JSONB       NOT NULL,
    bo4e_version            TEXT        NOT NULL DEFAULT 'v202607.0.0',

    -- Plausibility outcome
    outcome                 TEXT        NOT NULL CHECK (outcome IN (
                                'Ok',               -- accepted; REMADV 33001 dispatched
                                'AcceptedPartial',  -- accepted with remarks
                                'Warn',             -- validation warning; auto-approved
                                'Dispute',          -- blocked; REMADV 33002 dispatched
                                'Resolved',         -- dispute resolved by operator
                                'Dispatched',       -- outbound 31006 sent; awaiting NB REMADV
                                'Paid'              -- outbound 31006 settled by NB
                            )),

    -- invoic_checker findings (Vec<Finding> as JSONB)
    findings                JSONB       NOT NULL DEFAULT '[]',

    -- Zahlungsziel from INVOIC DTM+92 (rubo4e Rechnung.faelligkeitsdatum)
    pay_by                  TIMESTAMPTZ,

    -- Dispute resolution
    dispute_resolved_at     TIMESTAMPTZ,
    dispute_resolution_note TEXT,

    received_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    checked_at              TIMESTAMPTZ,
    dispatched_at           TIMESTAMPTZ,
    payment_confirmed_at    TIMESTAMPTZ,

    -- ERP outbox delivery tracking (durable at-least-once)
    -- Backoff: 0→30s, 1→5min, 2→30min, 3→2h; dead-lettered at attempt 5
    erp_notified_at         TIMESTAMPTZ,
    erp_attempts            SMALLINT    NOT NULL DEFAULT 0,
    erp_next_attempt_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    tenant                  TEXT        NOT NULL
);

COMMENT ON TABLE invoic_receipts IS
    '§22 MessZV: 3-year INVOIC receipt ledger. Written before REMADV/COMDIS dispatch. '
    'Supports PIDs 31001/31002/31005/31006/31007/31008/31009/31011.';

COMMENT ON COLUMN invoic_receipts.pay_by IS
    'Alert window: receipts with pay_by < now() + 3d AND dispatched_at IS NULL need attention.';

-- Per-counterparty billing history
CREATE INDEX invoic_sender_received   ON invoic_receipts (sender_mp_id, received_at DESC);
-- Tenant-scoped dashboard
CREATE INDEX invoic_tenant_received   ON invoic_receipts (tenant, received_at DESC);
-- MaLo payment-lifecycle
CREATE INDEX invoic_malo_tenant       ON invoic_receipts (tenant, malo_id)
    WHERE malo_id IS NOT NULL;
-- Exception queue (skip the majority of approved rows)
CREATE INDEX invoic_outcome           ON invoic_receipts (outcome)
    WHERE outcome NOT IN ('Ok', 'Paid');
-- Overdue Zahlungsziel without dispatched REMADV
CREATE INDEX invoic_pay_by_pending    ON invoic_receipts (pay_by)
    WHERE pay_by IS NOT NULL
      AND outcome IN ('Ok', 'AcceptedPartial', 'Warn')
      AND dispatched_at IS NULL;
-- Payment confirmed but ERP not yet notified
CREATE INDEX invoic_erp_pending       ON invoic_receipts (erp_next_attempt_at)
    WHERE erp_notified_at IS NULL AND erp_attempts < 5;
-- Direction filter (inbound vs. outbound summary)
CREATE INDEX invoic_direction_tenant  ON invoic_receipts (tenant, direction, received_at DESC);

-- ── Dead-letter queue ─────────────────────────────────────────────────────────
-- Events that fail deserialization or HMAC verification land here.
-- Alert fires when entries are older than 1 hour — never drop silently.

CREATE TABLE invoic_dlq (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id         TEXT,
    raw_event       JSONB       NOT NULL,
    failure_reason  TEXT        NOT NULL,
    failed_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant          TEXT        NOT NULL
);

COMMENT ON TABLE invoic_dlq IS
    'Dead-letter queue for events failing deserialization or HMAC verification. '
    'Alert when entries are older than 1 hour.';

CREATE INDEX invoic_dlq_failed_at ON invoic_dlq (failed_at DESC);
CREATE INDEX invoic_dlq_tenant    ON invoic_dlq (tenant, failed_at DESC);
