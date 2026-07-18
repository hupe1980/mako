-- ── tarifbd schema — Product & Tariff Catalog ────────────────────────────────
--
-- `products`: central product register with full BO4E Tarifpreisblatt JSONB.
-- `product_history`: immutable version history of every product update.
-- `customer_products`: MaLo → active product assignment (used by billingd).
-- `epex_prices`: hourly EPEX Spot day-ahead prices for §41a dynamic tariffs.
-- `angebote`: formal B2B quotation workflow (C&I / RLM customers).
--
-- All prices are user-defined in data.tarifpreispositionen.
-- tarifbd contains no hardcoded commercial rates.

-- ── Products ──────────────────────────────────────────────────────────────────

CREATE TABLE products (
    id              UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    lf_mp_id        TEXT    NOT NULL,
    product_code    TEXT    NOT NULL,
    -- Billing calculation template — determines which billingd calculator is invoked
    category        TEXT    NOT NULL CHECK (category IN (
                        'STROM', 'GAS', 'WAERME', 'SOLAR', 'EEG', 'EINSPEISUNG',
                        'WAERMEPUMPE', 'WALLBOX', 'HEMS', 'EMOBILITY',
                        'ENERGIEDIENSTLEISTUNG', 'BUNDLE', 'SHARING'
                    )),
    name            TEXT    NOT NULL,
    sparte          TEXT,   -- STROM | GAS | WAERME | NULL
    -- Tariff structure: Eintarif | Zweitarif | Mehrtarif
    register_count  TEXT,
    -- Customer segment; NULL = universal
    kundentyp       TEXT    CHECK (kundentyp IS NULL OR kundentyp IN (
                        'Haushalt', 'Gewerbe', 'Waermepumpe', 'Ladesaeule',
                        'Einspeiser', 'HEMS', 'Gewerbe_RLM'
                    )),
    -- §41a EnWG: only 'epex-spot-day-ahead' is accepted; NULL → fixed tariff
    dyn_source      TEXT    CHECK (dyn_source IS NULL OR dyn_source = 'epex-spot-day-ahead'),
    valid_from      DATE,
    valid_to        DATE,
    -- Full BO4E payload; validated against rubo4e::current on PUT
    data            JSONB   NOT NULL,
    bo4e_version    TEXT    NOT NULL DEFAULT 'v202607.0.0',
    -- DRAFT = staged/preview — invisible to billingd and comparison feed.
    -- PUBLISHED = active for billing, portald, and §42d comparison feed.
    product_status  TEXT    NOT NULL DEFAULT 'PUBLISHED'
                    CHECK (product_status IN ('DRAFT', 'PUBLISHED')),
    -- BO4E Energiemix COM (§42 EnWG energy source mix disclosure)
    energiemix      JSONB,
    -- Certification labels extracted from energiemix for GIN filtering
    oekolabel       TEXT[],
    tenant          TEXT    NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (lf_mp_id, product_code, valid_from)
);

COMMENT ON TABLE products IS
    'Product catalog. ALL prices are user-defined in data.tarifpreispositionen. '
    'category determines which billingd billing engine is invoked.';

COMMENT ON COLUMN products.category IS
    'STROM|GAS|WAERME|SOLAR|EEG|EINSPEISUNG|WAERMEPUMPE|WALLBOX|HEMS|EMOBILITY|'
    'ENERGIEDIENSTLEISTUNG|BUNDLE';

COMMENT ON COLUMN products.energiemix IS
    '§42 EnWG: rubo4e::current::Energiemix COM — CO₂ emissions, energy sources, '
    'radioactive waste, certification labels. Required on annual bills and portal.';

COMMENT ON COLUMN products.oekolabel IS
    'Certification label codes extracted from energiemix for GIN @> filter queries '
    '(e.g. WHERE oekolabel @> ARRAY[''OK_POWER'']).';

-- Category + LF lookup
CREATE INDEX products_lf_cat      ON products (lf_mp_id, category, valid_from DESC NULLS LAST);
CREATE INDEX products_lf_sparte   ON products (lf_mp_id, sparte, kundentyp);
-- §41a dynamic tariff filter
CREATE INDEX products_dyn         ON products (dyn_source) WHERE dyn_source IS NOT NULL;
-- Full JSONB search (for advanced MCP/portal queries)
CREATE INDEX products_gin         ON products USING GIN (data jsonb_path_ops);
-- Oekolabel GIN for "nur Ökostrom" portal filter
CREATE INDEX products_oekolabel   ON products USING GIN (oekolabel)
    WHERE oekolabel IS NOT NULL;
-- CO₂ emission sort/range queries
CREATE INDEX products_co2         ON products ((energiemix ->> 'co2Emission'))
    WHERE energiemix IS NOT NULL;
-- Category + sparte category filter
CREATE INDEX products_category_sparte ON products (category, sparte, lf_mp_id, valid_from DESC NULLS LAST);
-- Product status filter (admin: list drafts; billingd / comparison feed: published only)
CREATE INDEX products_status      ON products (lf_mp_id, product_status, valid_from DESC NULLS LAST);
-- Comparison portal feed index (covers pagination ORDER BY) — PUBLISHED only
CREATE INDEX products_feed_idx    ON products (lf_mp_id, updated_at DESC, product_code ASC)
    WHERE category IN ('STROM','GAS','WAERME','SOLAR','WAERMEPUMPE','WALLBOX')
      AND product_status = 'PUBLISHED'
      AND (valid_to IS NULL OR valid_to >= CURRENT_DATE);
-- Sparte + kundentyp for portal "show Haushalt Strom tariffs" filter — PUBLISHED only
CREATE INDEX products_feed_sparte_idx ON products (lf_mp_id, sparte, kundentyp, updated_at DESC)
    WHERE category IN ('STROM','GAS','WAERME','SOLAR','WAERMEPUMPE','WALLBOX')
      AND product_status = 'PUBLISHED'
      AND (valid_to IS NULL OR valid_to >= CURRENT_DATE);
-- §41a dynamic tariff portal filter — PUBLISHED only
CREATE INDEX products_feed_dynamic_idx ON products (lf_mp_id, updated_at DESC)
    WHERE dyn_source IS NOT NULL
      AND category IN ('STROM','WAERMEPUMPE','WALLBOX')
      AND product_status = 'PUBLISHED'
      AND (valid_to IS NULL OR valid_to >= CURRENT_DATE);
-- Tenant filter
CREATE INDEX products_tenant      ON products (tenant, lf_mp_id, valid_from DESC NULLS LAST);

-- ── Product version history (immutable) ──────────────────────────────────────

CREATE TABLE product_history (
    id              UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    lf_mp_id        TEXT    NOT NULL,
    product_code    TEXT    NOT NULL,
    data            JSONB   NOT NULL,
    energiemix      JSONB,
    bo4e_version    TEXT    NOT NULL DEFAULT 'v202607.0.0',
    changed_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE product_history IS
    'Immutable audit log of every product PUT. INSERT-only.';

CREATE INDEX ph_product ON product_history (lf_mp_id, product_code, changed_at DESC);

-- ── Customer → product assignment ─────────────────────────────────────────────

CREATE TABLE customer_products (
    malo_id         TEXT    NOT NULL,
    lf_mp_id        TEXT    NOT NULL,
    product_code    TEXT    NOT NULL,
    assigned_from   DATE    NOT NULL,
    assigned_to     DATE,               -- NULL = currently active
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (malo_id, lf_mp_id, assigned_from),
    FOREIGN KEY (lf_mp_id, product_code, assigned_from)
        REFERENCES products (lf_mp_id, product_code, valid_from)
        DEFERRABLE INITIALLY DEFERRED
);

COMMENT ON TABLE customer_products IS
    'MaLo → active product assignment used by billingd for invoice calculation.';

CREATE INDEX cp_malo_lf ON customer_products (malo_id, lf_mp_id);
CREATE INDEX cp_active  ON customer_products (malo_id, lf_mp_id)
    WHERE assigned_to IS NULL;

-- ── EPEX Spot day-ahead prices ────────────────────────────────────────────────
-- §41a EnWG: hourly day-ahead auction prices (EPEX SPOT DE-LU).
-- Import daily via PUT /api/v1/epex-prices/{date} (24-entry array).

CREATE TABLE epex_prices (
    price_date      DATE        NOT NULL,
    hour            SMALLINT    NOT NULL CHECK (hour BETWEEN 0 AND 23),
    -- ct/kWh (positive = delivery price; negative = surplus grid feed-in)
    avg_ct_kwh      NUMERIC(10, 4) NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'manual',
    imported_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (price_date, hour)
);

COMMENT ON TABLE epex_prices IS
    '§41a EnWG: hourly EPEX Spot day-ahead prices for dynamic tariff calculation. '
    'Import via PUT /api/v1/epex-prices/{date} (24-hour array).';

CREATE INDEX epex_date ON epex_prices (price_date DESC);

-- ── B2B Angebote (formal quotation workflow) ──────────────────────────────────
-- Lifecycle: ANGELEGT → VERSANDT → ANGENOMMEN | ABGELEHNT | ABGELAUFEN.
-- On ANGENOMMEN: emits de.angebot.angenommen → vertragd creates Rahmenvertrag.

CREATE TABLE angebote (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    lf_mp_id            TEXT        NOT NULL,
    kunden_id           UUID,                               -- NULL = new prospect
    interessent_name    TEXT,
    contact_email       TEXT,
    contact_phone       TEXT,
    angebotsnummer      TEXT        NOT NULL,
    status              TEXT        NOT NULL DEFAULT 'ANGELEGT'
                        CHECK (status IN (
                            'ANGELEGT',     -- created, not yet sent
                            'VERSANDT',     -- sent to customer
                            'ANGENOMMEN',   -- accepted
                            'ABGELEHNT',    -- declined
                            'ABGELAUFEN'    -- expired (gueltig_bis < today)
                        )),
    gueltig_bis         DATE        NOT NULL,
    lieferbeginn        DATE,
    laufzeit_monate     SMALLINT    NOT NULL DEFAULT 12
                        CHECK (laufzeit_monate IN (1, 3, 6, 12, 24, 36, 48, 60)),
    -- Array of AngebotPosition: {product_code, sparte, malo_id, jahresverbrauch_kwh, ...}
    positionen          JSONB       NOT NULL DEFAULT '[]',
    -- Alternative scenarios for side-by-side comparison
    varianten           JSONB       NOT NULL DEFAULT '[]',
    jahreskosten_netto_eur  NUMERIC(16, 2),
    jahreskosten_brutto_eur NUMERIC(16, 2),
    -- Pre-computed per-variant cost breakdown for GET .../comparison.
    -- Schema: [{label, laufzeit_monate, ist_basis, jahreskosten_netto_eur,
    --           jahreskosten_brutto_eur, rabatt_pct, positionen_detail: [{...}]}]
    -- Populated by POST /angebote and PUT /angebote/{id}; read by GET .../comparison.
    angebot_varianten_enriched JSONB       NOT NULL DEFAULT '[]',
    gewaehlte_variante  SMALLINT,
    rahmenvertrag_id    UUID,
    accepted_at         TIMESTAMPTZ,
    declined_at         TIMESTAMPTZ,
    -- ERP-side reference for idempotency
    erp_angebot_id      TEXT        UNIQUE,
    notizen             TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, lf_mp_id, angebotsnummer)
);

COMMENT ON TABLE angebote IS
    'Formal B2B quotation (Angebot) for C&I/RLM customers. '
    'Acceptance emits de.angebot.angenommen → vertragd creates Rahmenvertrag.';

COMMENT ON COLUMN angebote.varianten IS
    'Array of AngebotVariante: alternative scenarios for comparison. '
    'gewaehlte_variante is the index selected by customer on acceptance.';

CREATE INDEX angebote_tenant_status ON angebote (tenant, lf_mp_id, status);
CREATE INDEX angebote_kunden        ON angebote (kunden_id) WHERE kunden_id IS NOT NULL;
CREATE INDEX angebote_gueltig       ON angebote (gueltig_bis)
    WHERE status IN ('ANGELEGT', 'VERSANDT');
