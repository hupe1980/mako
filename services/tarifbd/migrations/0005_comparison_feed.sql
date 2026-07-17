-- tarifbd migration 0005: Comparison portal feed optimisations
--
-- Adds a partial index tuned for `GET /api/v1/comparison-feed` queries:
--   - Only energy tariff categories that appear in comparison portals
--     (STROM, GAS, WAERME, SOLAR, WAERMEPUMPE, WALLBOX)
--   - Excludes products with expired validity (valid_to < today)
--   - Covers the pagination ORDER BY (updated_at DESC, product_code ASC)
--
-- Separate from products_lf_cat (covers all categories) so that the planner
-- can choose the smaller partial index for portal feed scans.

CREATE INDEX IF NOT EXISTS products_feed_idx
    ON products (lf_mp_id, updated_at DESC, product_code ASC)
    WHERE category IN ('STROM','GAS','WAERME','SOLAR','WAERMEPUMPE','WALLBOX')
      AND (valid_to IS NULL OR valid_to >= CURRENT_DATE);

-- Sparte + kundentyp partial index for filtered portal scans
-- (e.g. "show only STROM/Haushalt tariffs")
CREATE INDEX IF NOT EXISTS products_feed_sparte_idx
    ON products (lf_mp_id, sparte, kundentyp, updated_at DESC)
    WHERE category IN ('STROM','GAS','WAERME','SOLAR','WAERMEPUMPE','WALLBOX')
      AND (valid_to IS NULL OR valid_to >= CURRENT_DATE);

-- §41a dynamic tariff index (Verivox/Check24 "dynamische Tarife" filter)
CREATE INDEX IF NOT EXISTS products_feed_dynamic_idx
    ON products (lf_mp_id, updated_at DESC)
    WHERE dyn_source IS NOT NULL
      AND category IN ('STROM','WAERMEPUMPE','WALLBOX')
      AND (valid_to IS NULL OR valid_to >= CURRENT_DATE);

-- Oekolabel GIN index for "nur Ökostrom" filter already exists on all products.
-- The existing products_oekolabel GIN index is sufficient for portal label filters.

COMMENT ON INDEX products_feed_idx IS
    'Partial index for GET /api/v1/comparison-feed — covers energy tariff categories '
    'with pagination ordering (updated_at DESC, product_code ASC). '
    'Excludes non-energy categories and expired products for a lean index.';
