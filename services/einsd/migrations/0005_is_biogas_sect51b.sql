-- Migration 0005: §51b EEG 2023 biogas Ausschreibungsanlage flag
--
-- §51b EEG 2023: For biogas plants (excluding biomethane) whose AW was
-- determined by BNetzA auction, the AW reduces to ZERO when the EPEX
-- monthly average is ≤ 2 ct/kWh. §51/§51a do NOT apply to these plants.
--
-- This column is explicitly stored (not derived from erzeugungsart + ausschreibungs_zuschlag_id)
-- because biomethane plants are excluded from §51b even if they have a Zuschlag,
-- and the operator can explicitly opt in/out for edge cases.
--
-- Legal basis: §51b EEG 2023 (BGBl. I Nr. 28, 10.01.2023).
-- Source: Clearingstelle EEG|KWKG Working Text 23.12.2025.

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS is_biogas_sect51b BOOLEAN NOT NULL DEFAULT FALSE;

COMMENT ON COLUMN eeg_anlagen.is_biogas_sect51b IS
    '§51b EEG 2023: true for biogas Ausschreibungsanlagen (excl. biomethane) '
    'where AW = 0 when EPEX monthly avg ≤ 2 ct/kWh. §51/§51a do not apply.';
