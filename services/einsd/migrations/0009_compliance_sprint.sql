-- ══ Migration 0009: EEG Compliance Sprint 1–3 ════════════════════════════════
--
-- Closes all Critical and High audit gaps:
--
-- [C1] §44b EEG 2023: Biogas annual 45%-cap quota tracking columns
-- [C2] §20 Abs. 2 + Anlage 1: technology-specific Jahresmarktwert table
-- [H7] §51 Abs. 2 EEG 2023: iMSys rollout datum (post-rollout: no kW exemption)
-- [M1] §22 MessZV: settlement_receipts — partial unique index replaces blanket UNIQUE;
--      initial re-runs now snapshot before overwrite (no silent loss)
-- [M3] §42b EEG 2023: GGV (Gemeinschaftliche Gebäudeversorgung) settlement model
--      (Solarpaket I, BGBl I 2024 Nr. 107)
-- [§21c] Veräußerungsform notification tracking
--
-- Legal bases:
--   §44b Abs. 1 EEG 2023 (BGBl. I 2023 Nr. 1)
--   §20 Abs. 2 + Anlage 1 EEG 2023
--   §51 Abs. 2 Nr. 1 EEG 2023
--   §42b EEG 2023 (Solarpaket I, BGBl I 2024 Nr. 107)
--   §22 MessZV (Messzählerverordnung) — 3-year audit trail
--   §21c EEG 2023 (Veräußerungsform-Wechsel notification)
-- ═════════════════════════════════════════════════════════════════════════════

-- ── 1. §44b Biogas annual production quota tracking ──────────────────────────
--
-- For fermentation-Biogas plants > 100 kW (excl. §39 Ausschreibung), EEG payment
-- is capped at 45% of rated capacity × 8760 h/year (§44b Abs. 1 EEG 2023).
--
-- We track the year-to-date Einspeisemenge per plant so that each monthly
-- settlement can compute the remaining eligible kWh:
--   annual_quota_kwh = leistung_kw × 0.45 × 8760
--   eligible_this_month = max(0, annual_quota_kwh − biogas_quota_kwh_ytd)
--
-- The counter resets to 0 each January (when billing_year ≠ biogas_quota_ytd_year).

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS biogas_quota_kwh_ytd  NUMERIC(14, 3) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS biogas_quota_ytd_year SMALLINT;

COMMENT ON COLUMN eeg_anlagen.biogas_quota_kwh_ytd IS
    '§44b Abs. 1 EEG 2023: cumulative Einspeisemenge fed-in during biogas_quota_ytd_year. '
    'Reset to 0 when billing_year differs from biogas_quota_ytd_year. '
    'Annual quota = leistung_kwp × 0.45 × 8760 kWh. '
    'Only meaningful for: erzeugungsart = BIOGAS, leistung_kwp > 100, is_biogas_sect51b = false.';

COMMENT ON COLUMN eeg_anlagen.biogas_quota_ytd_year IS
    '§44b: calendar year the biogas_quota_kwh_ytd counter belongs to. '
    'When NULL or != billing_year, counter is treated as 0 and reset on next settlement.';

-- Fast lookups for §44b quota monitoring
CREATE INDEX IF NOT EXISTS ea_biogas_quota
    ON eeg_anlagen (tenant, biogas_quota_ytd_year)
    WHERE erzeugungsart = 'BIOGAS' AND is_biogas_sect51b = false;

-- ── 2. §20 Abs. 2 + Anlage 1 EEG 2023 — technology-specific Jahresmarktwert ──
--
-- The ÜNB (transmission system operator) publishes monthly technology-specific
-- Marktwert (Jahresmarktwert) per §20 Abs. 2 EEG 2023.  For MarketPremium
-- (Direktvermarktung / Ausschreibung) settlement, this value — NOT the generic
-- EPEX monthly average — must be used as the market reference price.
--
-- Source: netztransparenz.de / ÜNB Marktwert publications.
-- See also: BNetzA Monatsmarktwerte (§22 Anlage 1 EEG 2023).

CREATE TABLE IF NOT EXISTS jahresmarktwert_preise (
    billing_year    SMALLINT     NOT NULL,
    billing_month   SMALLINT     NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    -- Matches erzeugungsart values in eeg_anlagen (WIND_ONSHORE, SOLAR_AUFDACH, etc.)
    -- Special value 'DEFAULT' = generic fallback when no technology-specific value available
    erzeugungsart   TEXT         NOT NULL,
    avg_ct_kwh      NUMERIC(8, 4) NOT NULL
                    CHECK (avg_ct_kwh >= -100 AND avg_ct_kwh <= 1000),
    source          TEXT         NOT NULL DEFAULT 'manual',
    imported_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (billing_year, billing_month, erzeugungsart)
);

COMMENT ON TABLE jahresmarktwert_preise IS
    '§20 Abs. 2 + Anlage 1 EEG 2023: technology-specific monthly Marktwert. '
    'Published by ÜNB. For MarketPremium (Direktvermarktung) settlement, this value '
    'replaces the generic EPEX monthly average from epex_monthly_prices. '
    'Lookup order: exact erzeugungsart match → DEFAULT fallback → epex_monthly_prices.';

CREATE INDEX IF NOT EXISTS jmw_period
    ON jahresmarktwert_preise (billing_year DESC, billing_month DESC);

CREATE INDEX IF NOT EXISTS jmw_art_period
    ON jahresmarktwert_preise (erzeugungsart, billing_year DESC, billing_month DESC);

-- API endpoints for import:
--   PUT /api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}
--   GET /api/v1/jahresmarktwert/{year}/{month}

-- ── 3. §51 Abs. 2 Nr. 1 EEG 2023 — iMSys rollout datum ─────────────────────
--
-- §51 Abs. 2 Nr. 1 EEG 2023: "Anlagen mit einer installierten Leistung von weniger
-- als 100 Kilowatt sind von der Pflicht nach Absatz 1 ausgenommen, bis für sie das
-- intelligente Messsystem nach dem Messstellenbetriebsgesetz eingebaut wurde."
--
-- → The <100 kW exemption ends when iMSys is installed.
--   After rollout: ALL EEG 2023 plants are subject to §51 regardless of size.

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS imesys_rollout_datum DATE;

COMMENT ON COLUMN eeg_anlagen.imesys_rollout_datum IS
    '§51 Abs. 2 Nr. 1 EEG 2023: date when an intelligent metering system (iMSys) '
    'per MsbG was installed at this plant. Once set and billing_date >= this value, '
    'the transitional <100 kW exemption from §51 Negativpreisregel is lifted. '
    'NULL = iMSys not yet installed; plant retains <100 kW exemption.';

-- ── 4. §42b EEG 2023 — Gemeinschaftliche Gebäudeversorgung (GGV) ─────────────
--
-- Solarpaket I (BGBl I 2024 Nr. 107, in force 01.01.2024):
-- Multiple occupants of the same building share a solar plant output.
-- Settlement rate = same as §38a Mieterstrom base rate.
-- A Nutzungsplan (JSON allocation table) distributes kWh among MaLos.

ALTER TABLE eeg_anlagen
    DROP CONSTRAINT IF EXISTS eeg_anlagen_settlement_model_check;

ALTER TABLE eeg_anlagen
    ADD CONSTRAINT eeg_anlagen_settlement_model_check
    CHECK (settlement_model IN (
        -- ── Legacy German names (backward compat) ────────────────────────────
        'VERGUETUNG',
        'DIREKTVERMARKTUNG',
        'AUSSCHREIBUNG',
        'POST_EEG_SPOT',
        'EIGENVERBRAUCH',
        'MIETERSTROM',
        'KWKG_ZUSCHLAG',
        'FLEXIBILITAET',
        'FLEXIBILITAET_ZUSCHLAG',
        -- ── Canonical SettlementScheme names (Rust enum) ─────────────────────
        'FEED_IN_TARIFF',
        'MARKET_PREMIUM',
        'TENANT_ELECTRICITY',
        'POST_EEG',
        'KWK_SURCHARGE',
        'FLEXIBILITY_PREMIUM',
        'FLEXIBILITY_SURCHARGE',
        'TEMPORARY_FEED_IN_TARIFF',
        -- ── New schemes (Solarpaket I + §21a EEG 2023) ───────────────────────
        'GGV',                        -- §42b EEG 2023: Gemeinschaftliche Gebäudeversorgung
        'SONSTIGE_DIREKTVERMARKTUNG'  -- §21a EEG 2023: direct third-party sale (no NB payment)
    ));

-- GGV Nutzungsplan: JSON array of {malo_id: string, fraction: decimal}
-- Fractions must sum to 1.0. Only relevant when settlement_model = 'GGV'.
ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS ggv_nutzungsplan JSONB;

COMMENT ON COLUMN eeg_anlagen.ggv_nutzungsplan IS
    '§42b EEG 2023 GGV Nutzungsplan: JSON array [{malo_id, fraction}] allocating '
    'plant output among building occupants. sum(fraction) must equal 1.0. '
    'Only used when settlement_model = ''GGV''.';

-- ── 5. §22 MessZV — settlement_receipts: enforce immutable audit trail ────────
--
-- Current issue: run_settlement uses ON CONFLICT DO UPDATE which silently overwrites
-- an initial receipt without leaving a history entry.
--
-- Fix: The unique constraint is made PARTIAL — only one INITIAL receipt per period.
-- Correction receipts (is_correction = true) coexist freely with the original.
-- In run_settlement, any overwrite of an existing initial receipt now FIRST inserts
-- a snapshot into settlement_receipt_history (logic is in Rust code, not SQL).
--
-- IMPORTANT: existing application code must be updated to handle the new insert
-- semantics (ON CONFLICT DO NOTHING + explicit re-insert after snapshot).

-- Drop the old unconditional unique constraint / index
ALTER TABLE settlement_receipts
    DROP CONSTRAINT IF EXISTS settlement_receipts_tr_id_tenant_billing_year_billing_month_key;

DROP INDEX IF EXISTS settlement_receipts_tr_id_tenant_billing_year_billing_month_key;

-- Partial unique: only one non-correction receipt per (plant × period)
CREATE UNIQUE INDEX IF NOT EXISTS sr_unique_initial
    ON settlement_receipts (tr_id, tenant, billing_year, billing_month)
    WHERE is_correction = false;

COMMENT ON INDEX sr_unique_initial IS
    '§22 MessZV: exactly one initial (non-correction) receipt per billing period per plant. '
    'Correction receipts (is_correction = true) are excluded and may accumulate freely '
    'as an immutable audit chain.';

-- ── 6. §21c EEG 2023 — Veräußerungsform-Wechsel notification tracking ─────────
--
-- §21c EEG 2023: operator must notify the NB of any Veräußerungsform switch
-- via the GPKE process (PID 55022/55023) by end of the calendar month of the switch.
-- Failure to notify triggers §52 Abs. 1 Nr. 9 Pflichtzahlung (+1 extra month).
-- Track when the notification was dispatched (CE emitted to GPKE handler in makod).

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS veraeusserungsform_notification_sent_at TIMESTAMPTZ;

COMMENT ON COLUMN eeg_anlagen.veraeusserungsform_notification_sent_at IS
    '§21c EEG 2023: timestamp when the Veräußerungsform switch notification '
    '(de.eeg.veraeusserungsform.gewechselt CloudEvent) was dispatched to the NB. '
    'NULL = no switch notification pending or already processed. '
    'When set and billing_date is in the same month as last_veraeusserungsform_switch, '
    'the §21c deadline is met.';

-- Index for notification monitoring (overdue notifications = potential §52 Nr. 9)
CREATE INDEX IF NOT EXISTS ea_notification_pending
    ON eeg_anlagen (tenant, last_veraeusserungsform_switch)
    WHERE veraeusserungsform_notification_sent_at IS NULL
      AND last_veraeusserungsform_switch IS NOT NULL;
