-- 0002_completed_at.sql
-- Add completed_at column for cycle-time computation in KPI reports.
--
-- Set when state transitions to completed / rejected / cancelled.
-- Backfill existing terminal-state rows from last_event_at (best estimate).

ALTER TABLE process_projections
    ADD COLUMN IF NOT EXISTS completed_at TIMESTAMPTZ;

UPDATE process_projections
    SET completed_at = last_event_at
    WHERE state IN ('completed', 'rejected', 'cancelled')
      AND completed_at IS NULL;

CREATE INDEX IF NOT EXISTS pp_completed_at
    ON process_projections (completed_at)
    WHERE completed_at IS NOT NULL;
