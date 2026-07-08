-- 0001_initial.sql — invoicd complete schema
--
-- § 22 MessZV / §41 EnWG: INVOIC receipts must be retained for 3 years.
-- Every INVOIC event received by this daemon is written here in one INSERT
-- before any REMADV/COMDIS command is dispatched to makod.
--
-- All timestamps are TIMESTAMPTZ (UTC).

-- ── INVOIC receipt ledger ─────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS invoic_receipts (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Business key: one row per billing process (idempotent ON CONFLICT)
    process_id    UUID        NOT NULL UNIQUE,

    -- BDEW PID that triggered this check (31001 | 31002 | 31005 | 31006 | 31009)
    pid           SMALLINT    NOT NULL,

    -- 'Inbound' (NB/MSB → LF) or 'Outbound' (LF selbstausgestellt)
    direction     TEXT        NOT NULL,

    -- GLN of the NB/MSB that issued the invoice
    sender_mp_id    TEXT        NOT NULL,

    -- Receiver GLN: tenant GLN for Inbound; NB GLN for Outbound
    receiver_gln  TEXT,

    -- Full BO4E Rechnung object as received (schema-pinned via bo4e_version)
    rechnung      JSONB       NOT NULL,

    -- BO4E schema version (e.g. "v202501.0.0"); use for forward-compat dispatch
    bo4e_version  TEXT        NOT NULL DEFAULT 'v202501.0.0',

    -- Plausibility outcome
    outcome       TEXT        NOT NULL CHECK (outcome IN (
                      'Ok',              -- accepted; REMADV 33001 dispatched
                      'AcceptedPartial', -- accepted with remarks; REMADV 33003/33004
                      'Warn',            -- validation warning (tariff tolerance); auto-approved
                      'Dispute',         -- blocked; COMDIS 29001 or REMADV 33002 dispatched
                      'Dispatched',      -- outbound 31006 sent to NB; awaiting REMADV
                      'Paid'             -- outbound 31006 settled by NB REMADV 33001
                  )),

    -- Serialised Vec<invoic_checker::Finding>
    findings      JSONB       NOT NULL DEFAULT '[]',

    -- Zahlungsziel from INVOIC DTM+92 (rubo4e Rechnung.faelligkeitsdatum)
    -- Alert query: pay_by < now() + interval '3 days' AND dispatched_at IS NULL
    pay_by        TIMESTAMPTZ,

    received_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    checked_at    TIMESTAMPTZ,
    dispatched_at TIMESTAMPTZ,

    tenant        TEXT        NOT NULL DEFAULT 'default'
);

-- Fast billing-period queries per counterparty
CREATE INDEX IF NOT EXISTS invoic_receipts_sender_gln_received_at
    ON invoic_receipts (sender_mp_id, received_at);

-- Tenant-scoped dashboard queries
CREATE INDEX IF NOT EXISTS invoic_receipts_tenant_received_at
    ON invoic_receipts (tenant, received_at);

-- Dispute / exception queries (skip the majority of approved rows)
CREATE INDEX IF NOT EXISTS invoic_receipts_outcome
    ON invoic_receipts (outcome)
    WHERE outcome <> 'Ok';

-- Receipts awaiting dispatch retry
CREATE INDEX IF NOT EXISTS invoic_receipts_pending_dispatch
    ON invoic_receipts (received_at)
    WHERE dispatched_at IS NULL;

-- Alert query: receipts approaching Zahlungsziel without a dispatched REMADV
CREATE INDEX IF NOT EXISTS invoic_receipts_pay_by_pending
    ON invoic_receipts (pay_by)
    WHERE pay_by IS NOT NULL
      AND outcome IN ('Ok', 'AcceptedPartial', 'Warn')
      AND dispatched_at IS NULL;

-- Direction filter (ERP dashboard: inbound vs. outbound invoice summary)
CREATE INDEX IF NOT EXISTS invoic_receipts_direction_tenant
    ON invoic_receipts (tenant, direction, received_at DESC);

-- ── Dead-letter queue ─────────────────────────────────────────────────────────
-- Events that fail deserialization or HMAC verification land here.
-- Alert fires when entries are older than 1 h. Never silently drop.

CREATE TABLE IF NOT EXISTS invoic_dlq (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id        TEXT,
    raw_event      JSONB       NOT NULL,
    failure_reason TEXT        NOT NULL,
    failed_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant         TEXT        NOT NULL DEFAULT 'default'
);

CREATE INDEX IF NOT EXISTS invoic_dlq_failed_at
    ON invoic_dlq (failed_at DESC);
