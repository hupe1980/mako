-- netzbilanzd migration 0004: Kostenblatt activation window + dispatch provenance
--
-- Adds three columns to `kostenblatt_records`:
--
-- `activation_start_utc` / `activation_end_utc`
--   Store the exact UTC activation window from the Redispatch ACO/IFTSTA.
--   Required for:
--     - Re-computing `dispatch_kwh` from `edmd` without operator re-input.
--     - Gap detection: activations where no energy data was recorded yet.
--     - Audit trail per BK6-20-061 §4.2 (traceability of Einsatzkosten to
--       the specific dispatch window).
--
-- `dispatch_source`
--   Tracks the provenance of `dispatch_kwh` for §22 MessZV auditability:
--     'lastgang_sum'    — summed from edmd 15-min Lastgang intervals (most precise)
--     'billing_period'  — fallback: `arbeitsmenge_kwh` from edmd billing-period
--                         (used when Lastgang data is absent)
--     'manual_override' — supplied by operator via `dispatch_kwh_override`
--   NULL means the record was inserted manually (pre-migration) without tracking.
--
-- BK6-20-061 §4.2 context:
--   The Kostenblatt must include the actual energy dispatched in the activation
--   window.  For a 15-minute activation, the billing-period monthly aggregate
--   gives the wrong value.  The Lastgang sum over the exact window is correct.

ALTER TABLE kostenblatt_records
    ADD COLUMN IF NOT EXISTS activation_start_utc  TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS activation_end_utc    TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS dispatch_source       TEXT
        CHECK (dispatch_source IN ('lastgang_sum', 'billing_period', 'manual_override'));

-- Partial index for efficient gap detection:
-- "Records that have no energy data yet" = dispatch_kwh = 0 AND dispatch_source IS NULL.
CREATE INDEX IF NOT EXISTS kb_gaps
    ON kostenblatt_records (tenant, period_year, period_month)
    WHERE dispatch_kwh = 0 AND dispatch_source IS NULL AND status = 'pending';

-- Index for finding records by activation window (re-computation queries).
CREATE INDEX IF NOT EXISTS kb_activation_window
    ON kostenblatt_records (activation_start_utc, activation_end_utc)
    WHERE activation_start_utc IS NOT NULL;

COMMENT ON COLUMN kostenblatt_records.activation_start_utc IS
    'UTC start of the Redispatch activation window (from ACO/IFTSTA). '
    'Stored for re-computation from edmd without operator re-input.';

COMMENT ON COLUMN kostenblatt_records.activation_end_utc IS
    'UTC end of the Redispatch activation window. '
    'Together with activation_start_utc uniquely identifies the dispatch event.';

COMMENT ON COLUMN kostenblatt_records.dispatch_source IS
    'Provenance of dispatch_kwh: '
    'lastgang_sum (edmd 15-min intervals — most precise), '
    'billing_period (edmd billing period aggregate — fallback), '
    'manual_override (operator-supplied).';
