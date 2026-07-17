-- Migration 0008: extend virtual_meter_configs rule_type CHECK constraint
-- to include GgvConstantAllocation and GgvProportionalAllocation
-- introduced by §42b EnWG Solarpaket I implementation.
--
-- The old 'GgvAllocation' variant is kept for backward-compat with any
-- rows that were stored under the old name. New inserts must use either
-- 'GgvConstantAllocation' or 'GgvProportionalAllocation'.

-- PostgreSQL does not support ALTER CHECK in-place; drop and re-add.
ALTER TABLE virtual_meter_configs
    DROP CONSTRAINT IF EXISTS virtual_meter_configs_rule_type_check;

ALTER TABLE virtual_meter_configs
    ADD CONSTRAINT virtual_meter_configs_rule_type_check CHECK (
        rule_type IN (
            'Sum',
            'Residual',
            'PvSelfConsumption',
            -- Legacy (pre-§42b Solarpaket I redesign); no longer produced by engine
            'GgvAllocation',
            -- §42b EnWG constant-fraction allocation (UTILTS CCI+ZG6)
            'GgvConstantAllocation',
            -- §42b EnWG variable consumption-proportional allocation
            'GgvProportionalAllocation'
        )
    );

COMMENT ON COLUMN virtual_meter_configs.rule_type IS
    'Virtual meter aggregation rule type. '
    'Sum: portfolio total. '
    'Residual: §42a EEG net grid withdrawal. '
    'PvSelfConsumption: prosumer self-consumption. '
    'GgvConstantAllocation: §42b EnWG constant fraction (UTILTS CCI+ZG6). '
    'GgvProportionalAllocation: §42b EnWG proportional consumption ratio.';
