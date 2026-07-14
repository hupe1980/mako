-- ── einsd migration 0004 — settlement state + metering configuration ─────────
--
-- Background:
--   eeg-billing now has:
--   - SettlementPeriodState: per-plant monthly lifecycle state machine
--   - MeterConfiguration: multi-meter Messkonzept topology
--   - ReductionPipeline: §52 Abs. 6 netting, §53c, §54
--
--   These columns enable:
--   1. Per-plant settlement state audit trail (Active/Reduced/Suspended/PostEeg/Ended)
--   2. Multi-MeLo Messkonzept storage (Volleinspeisung vs. Überschuss, §42b GGV)
--   3. §52 Abs. 6 netting configuration per plant
--   4. §54 Ausschreibungsreduzierung history
--
-- Dependencies:
--   0001_initial.sql
--   0002_mastr_bankverbindung.sql
--   0003_anlagenmerkmale.sql

-- ── 1. Monthly settlement state ───────────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS settlement_state TEXT
        NOT NULL DEFAULT 'active'
        CHECK (settlement_state IN (
            'active',       -- Full Vergütung flows normally
            'reduced',      -- §52 penalty / §53b / technical defect (Vergütung reduced but not zero)
            'suspended',    -- §52 EEG ≤2021: VerguetungAufNull (MaStR or DV pflicht)
            'interrupted',  -- Temporary: no data, no EPEX price
            'post_eeg',     -- Förderdauer expired, EPEX spot basis
            'ended'         -- Plant decommissioned or no further billing
        ));

COMMENT ON COLUMN eeg_anlagen.settlement_state IS
    'Current monthly settlement lifecycle state (eeg-billing::settlement_state::SettlementPeriodState). '
    'Updated after each monthly settlement run. '
    'active = full Vergütung; reduced = §52 penalty/§53b in effect; '
    'suspended = §52 EEG ≤2021 Vergütung = 0; '
    'post_eeg = Förderdauer expired, EPEX spot; ended = no further billing.';

-- State transition audit table
CREATE TABLE IF NOT EXISTS settlement_state_transitions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    from_state      TEXT        NOT NULL,
    to_state        TEXT        NOT NULL,
    effective_from  DATE        NOT NULL,   -- first billing period in new state
    reason          TEXT        NOT NULL,   -- StateTransitionReason DB value
    notes           TEXT,
    recorded_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_sst_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS sst_tr_id ON settlement_state_transitions (tr_id, tenant, effective_from DESC);

COMMENT ON TABLE settlement_state_transitions IS
    'Audit log of settlement state changes. '
    'Every transition (e.g. MaStR registered, Fernsteuerbarkeit installed, sanction detected) '
    'creates a row here. Used for regulatory audit and compliance reporting.';

-- ── 2. Metering configuration ─────────────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS meter_config JSONB,
    ADD COLUMN IF NOT EXISTS metering_mode TEXT
        CHECK (metering_mode IN ('SLP', 'RLM', 'IMSYS'));

COMMENT ON COLUMN eeg_anlagen.meter_config IS
    'Full MeterConfiguration as JSONB (eeg-billing::metering::MeterConfiguration). '
    'Contains: mode (SLP/RLM/IMSYS), meter_points (melo_id, typ, obis_code, ...), '
    'is_bidirectional. '
    'NULL = not yet configured (legacy record); use einspeisemessung_melo_id from melo_id column.';

COMMENT ON COLUMN eeg_anlagen.metering_mode IS
    'Primary metering technology for this plant: SLP / RLM / IMSYS. '
    'Denormalized from meter_config.mode for fast query filtering. '
    'iMSys satisfies §9 EEG Fernsteuerbarkeit requirement (§29 MsbG rollout 2025+).';

-- Backfill metering_mode from leistung_kwp (heuristic only — override if known)
UPDATE eeg_anlagen
SET metering_mode = CASE
    WHEN leistung_kwp > 30 THEN 'RLM'
    ELSE 'SLP'
END
WHERE metering_mode IS NULL;

-- ── 3. §52 Abs. 6 netting configuration ──────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS sect52_netting_enabled BOOLEAN NOT NULL DEFAULT true;

COMMENT ON COLUMN eeg_anlagen.sect52_netting_enabled IS
    'Whether the NB exercises §52 Abs. 6 EEG 2023 netting of Pflichtzahlung against Vergütung. '
    'true (default): Pflichtzahlung is deducted from Vergütung disbursement. '
    'false: Vergütung and Pflichtzahlung are settled separately (NB sends invoice).';

-- ── 4. §54 Ausschreibungsreduzierung ─────────────────────────────────────────

CREATE TABLE IF NOT EXISTS sect54_reductions (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id               TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,
    deduction_ct_kwh    NUMERIC(8,5) NOT NULL,
    bnetza_ref          TEXT        NOT NULL,   -- BNetzA notification reference
    effective_from      DATE        NOT NULL,
    effective_until     DATE,                   -- NULL = until further notice
    notes               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_sect54_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS sect54_tr_id ON sect54_reductions (tr_id, tenant, effective_from DESC);

COMMENT ON TABLE sect54_reductions IS
    '§54 EEG 2023 Ausschreibungsreduzierungen per plant. '
    'Each row = one BNetzA notification of AW reduction. '
    'effective_aw = awarded_aw - deduction_ct_kwh (floor 0). '
    'Only for Ausschreibungsanlagen (TariffSource::Auction plants).';

-- ── 5. §53b regional reduction history ───────────────────────────────────────

CREATE TABLE IF NOT EXISTS sect53b_reductions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    grid_area       TEXT        NOT NULL,   -- Netzgebiet identifier
    reduction_ct_kwh NUMERIC(8,5) NOT NULL,
    bnetza_ref      TEXT        NOT NULL,
    effective_from  DATE        NOT NULL,
    effective_until DATE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS sect53b_area ON sect53b_reductions (tenant, grid_area, effective_from DESC);

COMMENT ON TABLE sect53b_reductions IS
    '§53b EEG 2023 regional Grünstromkennzeichnung reductions. '
    'Applied per Netzgebiet (grid area) — not per plant. '
    'The settlement engine queries this table to apply reductions to all plants '
    'in the certified grid area during the settlement run.';

-- ── 6. settlement_receipts: add netting columns ───────────────────────────────

ALTER TABLE settlement_receipts
    ADD COLUMN IF NOT EXISTS pflichtzahlung_eur       NUMERIC(14,5),
    ADD COLUMN IF NOT EXISTS netting_applied          BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS net_vergütung_eur        NUMERIC(14,5),
    ADD COLUMN IF NOT EXISTS residual_pflichtzahlung  NUMERIC(14,5),
    ADD COLUMN IF NOT EXISTS settlement_state         TEXT;

COMMENT ON COLUMN settlement_receipts.pflichtzahlung_eur IS
    '§52 EEG 2023 Pflichtzahlung owed by plant operator for this period (EUR). '
    'NULL when no §52 violations. Separate from settlement_eur (Vergütung not reduced).';

COMMENT ON COLUMN settlement_receipts.netting_applied IS
    'Whether §52 Abs. 6 netting was applied in this period (true = Pflichtzahlung deducted from Vergütung).';

COMMENT ON COLUMN settlement_receipts.net_vergütung_eur IS
    'Net Vergütung disbursed to operator after §52 Abs. 6 netting, §53b, §53c reductions. '
    'NULL when netting_applied = false (gross = net). '
    'Used for accountingd SEPA CT payment amount.';

COMMENT ON COLUMN settlement_receipts.settlement_state IS
    'Settlement state at the time of calculation (copy of eeg_anlagen.settlement_state). '
    'For audit trail: shows which state the plant was in when this receipt was computed.';

-- ── 7. View: settlement summary with netting ─────────────────────────────────

CREATE OR REPLACE VIEW eeg_settlement_summary AS
SELECT
    sr.id,
    sr.tr_id,
    sr.tenant,
    sr.billing_year,
    sr.billing_month,
    sr.einspeisemenge_kwh,
    sr.settlement_eur             AS gross_vergütung_eur,
    sr.pflichtzahlung_eur,
    sr.netting_applied,
    COALESCE(sr.net_vergütung_eur, sr.settlement_eur) AS disbursed_eur,
    sr.residual_pflichtzahlung,
    sr.status,
    sr.settlement_state,
    ea.erzeugungsart,
    ea.leistung_kwp,
    ea.settlement_state           AS current_anlage_state
FROM settlement_receipts sr
LEFT JOIN eeg_anlagen ea ON ea.tr_id = sr.tr_id AND ea.tenant = sr.tenant;

COMMENT ON VIEW eeg_settlement_summary IS
    'Settlement overview with netting. '
    'disbursed_eur = net_vergütung_eur when netting was applied, else gross settlement_eur. '
    'Use for accountingd SEPA CT batch generation.';
