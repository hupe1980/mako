-- 0001_initial_schema.sql
-- Core MDM schema: malo, melo, contracts, lokationszuordnung,
-- subscriptions, process_correlation, partners.
--
-- All timestamps are TIMESTAMPTZ (UTC).
-- Date columns (valid_from/valid_to) are DATE (German local calendar dates,
-- but stored as plain DATE since the business meaning is a calendar date, not
-- a wall-clock instant).

-- ── Marktlokation ─────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS malo (
    malo_id     TEXT        PRIMARY KEY,            -- 11-digit BDEW alternating-weight ID
    sparte      TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    version     BIGINT      NOT NULL DEFAULT 1,
    data        JSONB       NOT NULL,               -- full BO4E MARKTLOKATION
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Lokationszuordnung (role assignments, temporal) ──────────────────────────

CREATE TABLE IF NOT EXISTS lokationszuordnung (
    malo_id          TEXT  NOT NULL REFERENCES malo (malo_id) ON DELETE CASCADE,
    zuordnungstyp    TEXT  NOT NULL,                -- NB | GNB | MSB | GMSB | LF | LFG | …
    rollencodenummer TEXT  NOT NULL,                -- 13-digit BDEW/DVGW GLN
    valid_from       DATE  NOT NULL,
    valid_to         DATE,                          -- NULL = currently valid
    PRIMARY KEY (malo_id, zuordnungstyp, valid_from)
);

CREATE INDEX IF NOT EXISTS lokationszuordnung_malo_id ON lokationszuordnung (malo_id);
CREATE INDEX IF NOT EXISTS lokationszuordnung_rollencodenummer ON lokationszuordnung (rollencodenummer);

-- ── Messlokation ──────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS melo (
    melo_id     TEXT        PRIMARY KEY,            -- DE + 31 alphanumeric chars
    malo_id     TEXT        REFERENCES malo (malo_id) ON DELETE SET NULL,
    version     BIGINT      NOT NULL DEFAULT 1,
    data        JSONB       NOT NULL,               -- full BO4E MESSLOKATION
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS melo_malo_id ON melo (malo_id);

-- ── Contracts ─────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS contracts (
    contract_id  TEXT        PRIMARY KEY,           -- ERP contract number or UUID
    malo_id      TEXT        REFERENCES malo (malo_id) ON DELETE SET NULL,
    sparte       TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    vertragsart  TEXT        NOT NULL,
    version      BIGINT      NOT NULL DEFAULT 1,
    data         JSONB       NOT NULL,              -- full BO4E VERTRAG + _mdm_billing
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS contracts_malo_id ON contracts (malo_id);

-- ── Process correlation index ─────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS process_correlation (
    process_id       UUID        PRIMARY KEY,       -- makod WorkflowId
    workflow_name    TEXT,                          -- e.g. "gpke-supplier-change"
    pid              INTEGER,                       -- BDEW Prüfidentifikator
    malo_id          TEXT,
    melo_id          TEXT,
    contract_id      TEXT,
    erp_contract_id  TEXT,
    erp_order_id     TEXT,                          -- ERP Idempotency-Key from command submission
    edifact_conv_id  UUID,                          -- from makoconvid extension attribute
    marktrolle       TEXT,                          -- canonical role (NB, LF, MSB, UNB, …)
    format_version   TEXT,                          -- e.g. "FV2026-10-01"
    status           TEXT        NOT NULL CHECK (status IN ('RUNNING', 'COMPLETED', 'FAILED')),
    initiated_at     TIMESTAMPTZ NOT NULL,
    completed_at     TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS process_correlation_erp_order_id  ON process_correlation (erp_order_id);
CREATE INDEX IF NOT EXISTS process_correlation_malo_id_status ON process_correlation (malo_id, status);
CREATE INDEX IF NOT EXISTS process_correlation_edifact_conv_id ON process_correlation (edifact_conv_id);

-- ── Webhook subscriptions ─────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS subscriptions (
    subscriber_id  TEXT      PRIMARY KEY,
    webhook_url    TEXT      NOT NULL,
    -- webhook_secret is AES-256-GCM encrypted at rest; stored as base64.
    -- NULL means no HMAC signing for this subscriber.
    webhook_secret TEXT,
    roles          TEXT[]    NOT NULL DEFAULT '{}', -- empty = all roles
    event_types    TEXT[]    NOT NULL DEFAULT '{}', -- empty = all types; supports '*' suffix
    sparten        TEXT[]    NOT NULL DEFAULT '{}', -- empty = all Sparten
    active         BOOLEAN   NOT NULL DEFAULT true,
    version        BIGINT    NOT NULL DEFAULT 1,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Trading partners ──────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS partners (
    gln          TEXT        PRIMARY KEY,           -- 13-digit BDEW/DVGW/GS1 GLN
    display_name TEXT,
    marktrolle   TEXT,
    sparte       TEXT        CHECK (sparte IN ('STROM', 'GAS')),
    channels     JSONB       NOT NULL DEFAULT '[]', -- AS4 endpoints, certificates, etc.
    version      BIGINT      NOT NULL DEFAULT 1,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Idempotency dedup for inbound makod events ────────────────────────────────

CREATE TABLE IF NOT EXISTS processed_events (
    event_id    TEXT        PRIMARY KEY,            -- CloudEvents "id" (UUID v4)
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Purge entries older than 7 days (via scheduled DELETE in background worker).
CREATE INDEX IF NOT EXISTS processed_events_processed_at ON processed_events (processed_at);
