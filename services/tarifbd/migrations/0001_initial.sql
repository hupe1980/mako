-- ── tarifbd schema — Product & Tariff Catalog ───────────────────────────────
--
-- `products`:
--   Central product register.  Each row is one product definition identified
--   by (lf_mp_id, product_code, valid_from).  `data` stores the full
--   `Tarifpreisblatt` / `Preisblatt` / telco object as validated BO4E JSONB.
--
-- `customer_products`:
--   Maps a MaLo to its active product assignment.  Enables
--   `GET /api/v1/customer/{malo_id}/product` look-up used by `billingd`.
--
-- `epex_prices`:
--   Hourly EPEX Spot day-ahead prices. Required for §41a dynamic tariffs.
--   Import via `PUT /api/v1/epex-prices/{date}`.
--
-- `product_history`:
--   Immutable audit log of every PUT (upsert) on `products`.

-- ── Products ──────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS products (
    id             UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    lf_mp_id       TEXT    NOT NULL,           -- operator/LF BDEW-Codenummer
    product_code   TEXT    NOT NULL,
    category       TEXT    NOT NULL
                   CHECK (category IN ('ENERGY','SERVICE','TELCO','BUNDLE')),
    name           TEXT    NOT NULL,
    sparte         TEXT,                       -- STROM | GAS | WAERME | NULL
    -- BO4E Registeranzahl for energy products: Eintarif | Zweitarif | Mehrtarif
    register_count TEXT,
    -- Customer segment: Haushalt | Gewerbe | Waermepumpe | Ladesaeule | NULL
    kundentyp      TEXT,
    -- §41a: 'epex-spot-day-ahead' → dynamic pricing; NULL → fixed tariff
    dyn_source     TEXT,
    valid_from     DATE,
    valid_to       DATE,
    -- Full BO4E payload: Tarifpreisblatt | Preisblatt | custom schema.
    -- Validated against rubo4e::current on PUT (422 on schema violation).
    data           JSONB   NOT NULL,
    bo4e_version   TEXT    NOT NULL DEFAULT 'v202607.0.0',
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (lf_mp_id, product_code, valid_from)
);

CREATE INDEX IF NOT EXISTS products_lf_cat    ON products (lf_mp_id, category, valid_from DESC NULLS LAST);
CREATE INDEX IF NOT EXISTS products_lf_sparte ON products (lf_mp_id, sparte, kundentyp);
CREATE INDEX IF NOT EXISTS products_dyn       ON products (dyn_source) WHERE dyn_source IS NOT NULL;
CREATE INDEX IF NOT EXISTS products_gin       ON products USING GIN (data jsonb_path_ops);

-- ── Product version history ───────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS product_history (
    id             UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    lf_mp_id       TEXT    NOT NULL,
    product_code   TEXT    NOT NULL,
    data           JSONB   NOT NULL,
    bo4e_version   TEXT    NOT NULL DEFAULT 'v202607.0.0',
    changed_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ph_product ON product_history (lf_mp_id, product_code, changed_at DESC);

-- ── Customer → product assignment ─────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS customer_products (
    malo_id        TEXT    NOT NULL,
    lf_mp_id       TEXT    NOT NULL,
    product_code   TEXT    NOT NULL,
    assigned_from  DATE    NOT NULL,
    assigned_to    DATE,                       -- NULL = currently active
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (malo_id, lf_mp_id, assigned_from),
    FOREIGN KEY (lf_mp_id, product_code, assigned_from)
        REFERENCES products (lf_mp_id, product_code, valid_from)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX IF NOT EXISTS cp_malo_lf  ON customer_products (malo_id, lf_mp_id);
CREATE INDEX IF NOT EXISTS cp_active   ON customer_products (malo_id, lf_mp_id)
    WHERE assigned_to IS NULL;

-- ── EPEX Spot day-ahead prices ────────────────────────────────────────────────
-- Hourly day-ahead auction prices (EPEX SPOT DE-LU).
-- Required for §41a EnWG dynamic tariffs.
-- Import daily via `PUT /api/v1/epex-prices/{date}` (24-entry array).
-- Source: ENTSO-E Transparency Platform, netztransparenz.de, or EPEX API.

CREATE TABLE IF NOT EXISTS epex_prices (
    price_date     DATE        NOT NULL,
    hour           SMALLINT    NOT NULL CHECK (hour BETWEEN 0 AND 23),
    -- ct/kWh  (positive = delivery price; negative = surplus grid feed-in)
    avg_ct_kwh     NUMERIC(10, 4) NOT NULL,
    source         TEXT        NOT NULL DEFAULT 'manual',
    imported_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (price_date, hour)
);

CREATE INDEX IF NOT EXISTS epex_date ON epex_prices (price_date DESC);

-- ── tarifbd 0002: Energy-focused product categories (hard cut) ───────────────
--
-- Removes: TELCO (and all TELCO product rows)
-- Renames: ENERGY → STROM, SERVICE → ENERGIEDIENSTLEISTUNG
-- Adds:    GAS, WAERME, SOLAR, EEG, EINSPEISUNG, WAERMEPUMPE, WALLBOX, HEMS, EMOBILITY
--
-- Breaking change: no backward compatibility.

BEGIN;

-- 1. Remove TELCO data (hard cut)
DELETE FROM customer_products
WHERE product_code IN (
    SELECT product_code FROM products WHERE category = 'TELCO'
);
DELETE FROM product_history
WHERE product_code IN (
    SELECT product_code FROM products WHERE category = 'TELCO'
);
DELETE FROM products WHERE category = 'TELCO';

-- 2. Rename existing categories
UPDATE products SET category = 'STROM'                 WHERE category = 'ENERGY';
UPDATE products SET category = 'ENERGIEDIENSTLEISTUNG' WHERE category = 'SERVICE';

-- 3. Widen CHECK constraint for new energy categories
--    (PostgreSQL requires dropping + re-adding the constraint)
ALTER TABLE products DROP CONSTRAINT IF EXISTS products_category_check;
ALTER TABLE products ADD CONSTRAINT products_category_check
    CHECK (category IN (
        'STROM',
        'GAS',
        'WAERME',
        'SOLAR',
        'EEG',
        'EINSPEISUNG',
        'WAERMEPUMPE',
        'WALLBOX',
        'HEMS',
        'EMOBILITY',
        'ENERGIEDIENSTLEISTUNG',
        'BUNDLE'
    ));

-- 4. Update kundentyp domain to include new segments
ALTER TABLE products DROP CONSTRAINT IF EXISTS products_kundentyp_check;
ALTER TABLE products ADD CONSTRAINT products_kundentyp_check
    CHECK (kundentyp IS NULL OR kundentyp IN (
        'Haushalt',
        'Gewerbe',
        'Waermepumpe',
        'Ladesaeule',
        'Einspeiser',
        'HEMS',
        'Gewerbe_RLM'
    ));

-- 5. Add index optimized for category + sparte lookups
CREATE INDEX IF NOT EXISTS products_category_sparte
    ON products (category, sparte, lf_mp_id, valid_from DESC NULLS LAST);

-- 6. Document the new categories
COMMENT ON COLUMN products.category IS
    'Billing calculation template: STROM|GAS|WAERME|SOLAR|EEG|EINSPEISUNG|'
    'WAERMEPUMPE|WALLBOX|HEMS|EMOBILITY|ENERGIEDIENSTLEISTUNG|BUNDLE. '
    'Determines which billingd calculator is invoked. '
    'ALL prices are user-defined in data.tarifpreispositionen — '
    'the engine contains no hardcoded commercial rates.';

COMMIT;

-- 0003_energiemix.sql
--
-- Adds Energiemix + Oekolabel columns to the `products` table.
--
-- §42 EnWG requires Lieferanten to disclose the energy source mix (Energiemix),
-- CO₂ emissions, radioactive waste, and certification labels (Oekolabel) on
-- annual bills and in the customer portal.  These were previously missing from
-- `tarifbd`, meaning `billingd` had no structured Herkunftsnachweis data to
-- attach to generated Rechnungen.
--
-- Design:
--   `energiemix JSONB`   — full `rubo4e::current::Energiemix` COM payload
--                          (camelCase, validated via PUT /energiemix endpoint)
--   `oekolabel TEXT[]`   — array of Oekolabel enum codes extracted from the
--                          certification list; GIN-indexed for @> filter queries
--                          (e.g. "all Ökostrom products with OK_POWER label")
--
-- Both columns are nullable — not every product is green-certified.

ALTER TABLE products
    ADD COLUMN IF NOT EXISTS energiemix  JSONB    DEFAULT NULL,
    ADD COLUMN IF NOT EXISTS oekolabel   TEXT[]   DEFAULT NULL;

-- GIN index for "contains label" queries:
--   SELECT * FROM products WHERE oekolabel @> ARRAY['OK_POWER']
CREATE INDEX IF NOT EXISTS products_oekolabel
    ON products USING GIN (oekolabel)
    WHERE oekolabel IS NOT NULL;

-- Expression index on co2_emission for range/sort queries (extracted from JSONB):
--   SELECT * FROM products ORDER BY (energiemix->>'co2Emission')::numeric
CREATE INDEX IF NOT EXISTS products_co2
    ON products ((energiemix ->> 'co2Emission'))
    WHERE energiemix IS NOT NULL;

-- Also add energiemix to product_history so amendments are auditable.
ALTER TABLE product_history
    ADD COLUMN IF NOT EXISTS energiemix JSONB DEFAULT NULL;

-- tarifbd migration 0004: B2B Angebot (Quotation) workflow
--
-- L4: Formal Angebot (quotation) for C&I / RLM customers.
--
-- Lifecycle: ANGELEGT → VERSANDT → ANGENOMMEN | ABGELEHNT | ABGELAUFEN
--
-- An Angebot contains one or more Varianten (scenarios for comparison)
-- and multiple Positionen (one per commodity/site).  On acceptance the
-- operator receives a `de.angebot.angenommen` CloudEvent and creates
-- the Rahmenvertrag / Versorgungsvertrag via vertragd.

CREATE TABLE IF NOT EXISTS angebote (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    lf_mp_id        TEXT        NOT NULL,

    -- Customer / prospect
    kunden_id       UUID,                               -- NULL = new prospect
    interessent_name TEXT,                               -- free-text for prospects
    contact_email   TEXT,
    contact_phone   TEXT,

    -- Quotation metadata
    angebotsnummer  TEXT        NOT NULL,               -- e.g. ANG-2026-001234
    status          TEXT        NOT NULL DEFAULT 'ANGELEGT'
                    CHECK (status IN (
                        'ANGELEGT',     -- created, not yet sent
                        'VERSANDT',     -- sent to customer
                        'ANGENOMMEN',   -- digitally accepted
                        'ABGELEHNT',    -- declined by customer
                        'ABGELAUFEN'    -- expired (gueltig_bis < today)
                    )),

    -- Contract terms offered
    gueltig_bis         DATE        NOT NULL,
    lieferbeginn        DATE,
    laufzeit_monate     SMALLINT    NOT NULL DEFAULT 12
                        CHECK (laufzeit_monate IN (1,3,6,12,24,36,48,60)),

    -- Positions: array of AngebotPosition objects
    -- Each position covers one commodity/MaLo with per-product pricing.
    positionen      JSONB       NOT NULL DEFAULT '[]',

    -- Varianten: alternative scenarios (different laufzeit, products, or discounts).
    -- Customer picks one variant on acceptance.
    -- Format: [{ "label": "Eintarif 12M", "rabatt_pct": 0, "positionen": [...] }]
    varianten       JSONB       NOT NULL DEFAULT '[]',

    -- Aggregated totals (sum over all positionen, excluding NNE)
    jahreskosten_netto_eur   NUMERIC(16, 2),
    jahreskosten_brutto_eur  NUMERIC(16, 2),

    -- Acceptance state
    gewaehlte_variante  SMALLINT,       -- index into varianten[] (NULL = base offer)
    rahmenvertrag_id    UUID,           -- set after acceptance via vertragd webhook
    accepted_at         TIMESTAMPTZ,
    declined_at         TIMESTAMPTZ,

    -- Audit trail
    erp_angebot_id  TEXT        UNIQUE, -- ERP-side reference for idempotency
    notizen         TEXT,               -- internal sales notes
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (tenant, lf_mp_id, angebotsnummer)
);

CREATE INDEX IF NOT EXISTS angebote_tenant_status ON angebote (tenant, lf_mp_id, status);
CREATE INDEX IF NOT EXISTS angebote_kunden        ON angebote (kunden_id) WHERE kunden_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS angebote_gueltig       ON angebote (gueltig_bis) WHERE status IN ('ANGELEGT','VERSANDT');

COMMENT ON TABLE angebote IS
    'Formal B2B quotation (Angebot) for C&I/RLM customers. '
    'Lifecycle: ANGELEGT → VERSANDT → ANGENOMMEN/ABGELEHNT/ABGELAUFEN. '
    'Acceptance emits de.angebot.angenommen CloudEvent → vertragd creates Rahmenvertrag.';

COMMENT ON COLUMN angebote.positionen IS
    'JSONB array of AngebotPosition: { product_code, sparte, malo_id, '
    'jahresverbrauch_kwh, arbeitspreis_ct_per_kwh, grundpreis_ct_per_day, '
    'jahreskosten_netto_eur, szenario_tag, nne_eur_year? }';

COMMENT ON COLUMN angebote.varianten IS
    'JSONB array of AngebotVariante: alternative scenarios for side-by-side comparison. '
    'Customer selects one on acceptance via gewaehlte_variante index.';
