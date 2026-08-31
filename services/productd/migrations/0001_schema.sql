-- ── productd schema — Product & Tariff Catalog ────────────────────────────────
--
-- `products`: central product register with full BO4E Tarifpreisblatt JSONB.
-- `product_history`: immutable version history of every product update.
-- The MaLo→product assignment is NOT here: which product a customer is on is a
-- contract fact, agreed under § 41 Abs. 5 EnWG, and lives in `vertragd`.
-- `epex_prices`: hourly EPEX Spot day-ahead prices for §41a dynamic tariffs.
-- `angebote`: formal B2B quotation workflow (C&I / RLM customers).
--
-- All prices are user-defined in data.tarifpreise.
-- productd contains no hardcoded commercial rates.

-- ── heute() — the business date ───────────────────────────────────────────────
--
-- Every date this schema compares against is a German calendar date — the day a
-- Frist runs out, a validity window opens, an obligation falls due.
-- PostgreSQL's own `current_date` answers the *session* time zone's date, which
-- on a UTC server is still yesterday between 23:00 and midnight Berlin time
-- (22:00 in summer). `heute()` states the conversion once, so it holds however
-- the connection was opened. The Rust side reads the same date through
-- `mako_fristen::heute`.
CREATE OR REPLACE FUNCTION heute() RETURNS date
    LANGUAGE sql STABLE
    AS $$ SELECT (now() AT TIME ZONE 'Europe/Berlin')::date $$;

CREATE EXTENSION IF NOT EXISTS btree_gist;

-- ── Products ──────────────────────────────────────────────────────────────────

CREATE TABLE products (
    id              UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    lf_mp_id        TEXT    NOT NULL,
    product_code    TEXT    NOT NULL,
    -- Billing calculation template — determines which billingd calculator is invoked
    category        TEXT    NOT NULL CHECK (category IN (
                        'STROM', 'GAS', 'WAERME', 'WASSER', 'SOLAR', 'EEG',
                        'EINSPEISUNG', 'WAERMEPUMPE', 'WALLBOX', 'HEMS', 'EMOBILITY',
                        'ENERGIEDIENSTLEISTUNG', 'BUNDLE', 'SHARING'
                    )),
    name            TEXT    NOT NULL,
    sparte          TEXT,   -- STROM | GAS | WAERME | WASSER | NULL
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
    bo4e_version    TEXT    NOT NULL DEFAULT '202607.1.0',
    -- DRAFT = staged/preview — invisible to billingd and comparison feed.
    -- PUBLISHED = active for billing, portald, and § 41c comparison feed.
    product_status  TEXT    NOT NULL DEFAULT 'PUBLISHED'
                    CHECK (product_status IN ('DRAFT', 'PUBLISHED')),
    -- BO4E Energiemix COM (§42 EnWG energy source mix disclosure)
    energiemix      JSONB,
    -- Certification labels extracted from energiemix for GIN filtering
    oekolabel       TEXT[],
    tenant          TEXT    NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The identity of a product version. Two things the plain
-- `UNIQUE (lf_mp_id, product_code, valid_from)` got wrong:
--
--   * no tenant — tenant B's PUT of the same product code overwrote tenant A's
--     row, because the upsert's DO UPDATE had no tenant predicate either;
--   * NULLs are distinct under a plain UNIQUE, so every PUT of an open-ended
--     product (valid_from IS NULL) inserted another duplicate instead of
--     updating, after which `fetch_product`'s LIMIT 1 picked among them
--     nondeterministically.
--
-- The COALESCE sentinel gives the open-ended version a single identity it can
-- actually conflict on. It is never read back — `valid_from` stays NULL.
CREATE UNIQUE INDEX products_identity ON products
    (tenant, lf_mp_id, product_code, (COALESCE(valid_from, DATE '0001-01-01')));

-- Two versions of one product in force on the same day is not a state that
-- should exist: `fetch_product`'s `ORDER BY valid_from DESC LIMIT 1` then picks
-- one of them and bills it, with nothing to say which. The identity index above
-- only stops two versions with the same *start*.
--
-- `valid_to` is inclusive here — a product is sellable up to and including that
-- day — so the range is built as `[valid_from, valid_to + 1)`.
ALTER TABLE products
    ADD CONSTRAINT products_no_overlap
    EXCLUDE USING gist (
        tenant       WITH =,
        lf_mp_id     WITH =,
        product_code WITH =,
        daterange(
            COALESCE(valid_from, DATE '0001-01-01'),
            CASE WHEN valid_to IS NULL THEN NULL ELSE valid_to + 1 END,
            '[)'
        ) WITH &&
    );

COMMENT ON TABLE products IS
    'Product catalog. ALL prices are user-defined in data.tarifpreise. '
    'category determines which billingd billing engine is invoked.';

COMMENT ON COLUMN products.category IS
    'STROM|GAS|WAERME|SOLAR|EEG|EINSPEISUNG|WAERMEPUMPE|WALLBOX|HEMS|EMOBILITY|'
    'ENERGIEDIENSTLEISTUNG|BUNDLE|SHARING (14 categories; SHARING = §42c Energy Sharing)';

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
      AND product_status = 'PUBLISHED';
-- Sparte + kundentyp for portal "show Haushalt Strom tariffs" filter — PUBLISHED only
CREATE INDEX products_feed_sparte_idx ON products (lf_mp_id, sparte, kundentyp, updated_at DESC)
    WHERE category IN ('STROM','GAS','WAERME','SOLAR','WAERMEPUMPE','WALLBOX')
      AND product_status = 'PUBLISHED';
-- §41a dynamic tariff portal filter — PUBLISHED only
CREATE INDEX products_feed_dynamic_idx ON products (lf_mp_id, updated_at DESC)
    WHERE dyn_source IS NOT NULL
      AND category IN ('STROM','WAERMEPUMPE','WALLBOX')
      AND product_status = 'PUBLISHED';
-- Tenant filter
CREATE INDEX products_tenant      ON products (tenant, lf_mp_id, valid_from DESC NULLS LAST);

-- ── Product version history (immutable) ──────────────────────────────────────

CREATE TABLE product_history (
    id              UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
    lf_mp_id        TEXT    NOT NULL,
    product_code    TEXT    NOT NULL,
    data            JSONB   NOT NULL,
    energiemix      JSONB,
    bo4e_version    TEXT    NOT NULL DEFAULT '202607.1.0',
    changed_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE product_history IS
    'Immutable audit log of every product PUT. INSERT-only.';

CREATE INDEX ph_product ON product_history (lf_mp_id, product_code, changed_at DESC);

-- ── EPEX Spot day-ahead prices ────────────────────────────────────────────────
-- §41a EnWG: day-ahead auction prices (EPEX SPOT DE-LU).
--
-- The SDAC day-ahead auction settled on 15-minute Market Time Units (MTU) since
-- 2025-10-01 (96 quarter-hours per delivery day; 92/100 on DST days). Prices are
-- keyed on the UTC start instant of the MTU — DST-safe, resolution-agnostic.
-- Legacy 60-minute source data is stored as 60-min rows and expanded to
-- quarter-hours on fetch. Import via PUT /api/v1/epex-prices/{date}.

CREATE TABLE epex_prices (
    mtu_start       TIMESTAMPTZ NOT NULL,   -- UTC start of the market time unit
    price_date      DATE        NOT NULL,   -- local (Europe/Berlin) delivery date
    mtu_minutes     SMALLINT    NOT NULL DEFAULT 15 CHECK (mtu_minutes IN (15, 60)),
    -- ct/kWh (positive = delivery price; negative = surplus grid feed-in)
    avg_ct_kwh      NUMERIC(10, 4) NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'manual',
    imported_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (mtu_start)
);

COMMENT ON TABLE epex_prices IS
    '§41a EnWG: EPEX Spot day-ahead prices for dynamic tariff calculation, keyed '
    'on the 15-minute Market Time Unit start (UTC; SDAC 15-min go-live 2025-10-01). '
    'Import via PUT /api/v1/epex-prices/{date}.';

CREATE INDEX epex_date ON epex_prices (price_date DESC);

-- ── nEHS certificate prices (BEHG CO₂) ────────────────────────────────────────
--
-- Since 2026 nEHS certificates are auctioned (§10 Abs. 1 BEHG: weekly EEX
-- auctions from 01.07.2026 within the §10 Abs. 2 corridor of 55–65 EUR/t,
-- Verkaufsphase at 68 EUR/t). The CO₂ component of Gas/Wärme billing is
-- therefore market-formed; this dated series carries the supplier's
-- acquisition prices (CO2KostAufG §3: pass through the actual CO₂ costs).
CREATE TABLE nehs_prices (
    price_date      DATE           PRIMARY KEY,
    -- EUR per tonne CO₂ (auction clearing price or Verkaufsphase price)
    eur_per_t       NUMERIC(10, 2) NOT NULL CHECK (eur_per_t > 0),
    -- Provenance of the price point:
    --   'auktion'       — EEX weekly auction clearing price
    --   'verkaufsphase' — fixed Verkaufsphase price (68 EUR/t)
    --   'nachkauf'      — supplementary purchase
    --   'manual'        — operator entry
    source          TEXT           NOT NULL DEFAULT 'manual'
                    CHECK (source IN ('auktion', 'verkaufsphase', 'nachkauf', 'manual')),
    imported_at     TIMESTAMPTZ    NOT NULL DEFAULT now()
);

COMMENT ON TABLE nehs_prices IS
    'BEHG/nEHS certificate prices (EUR/t CO₂), dated. Since 2026 auctioned at '
    'EEX; used by billingd to derive the Gas CO₂ component per CO2KostAufG.';

CREATE INDEX nehs_date ON nehs_prices (price_date DESC);

-- ── B2B Angebote (formal quotation workflow) ──────────────────────────────────
-- Lifecycle: ANGELEGT → VERSANDT → ANGENOMMEN | ABGELEHNT | ABGELAUFEN.
-- On ANGENOMMEN: emits de.tarif.angebot.angenommen → vertragd creates Rahmenvertrag.

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
    -- Any positive term. The old whitelist of (1,3,6,12,24,36,48,60) refused
    -- an 18- or 9-month quotation for no reason a customer would recognise;
    -- a B2B term is negotiated, not chosen from a list.
    laufzeit_monate     SMALLINT    NOT NULL DEFAULT 12
                        CHECK (laufzeit_monate > 0 AND laufzeit_monate <= 240),
    -- Array of AngebotPosition: {product_code, sparte, malo_id, jahresverbrauch_kwh, ...}
    positionen          JSONB       NOT NULL DEFAULT '[]',
    -- Alternative scenarios for side-by-side comparison
    varianten           JSONB       NOT NULL DEFAULT '[]',
    jahreskosten_netto_eur  NUMERIC(16, 2),
    jahreskosten_brutto_eur NUMERIC(16, 2),
    -- BO4E `Angebot` business object for the priced quotation.
    --
    -- The CPQ/ERP interchange payload: Angebot → Angebotsvariante (one per
    -- scenario) → Angebotsteil (one per Marktlokation) → Angebotsposition (one
    -- per cost line). Written by GET .../comparison, which is where the
    -- scenarios are priced; '{}' until first priced.
    bo4e                JSONB       NOT NULL DEFAULT '{}',
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
    'Acceptance emits de.tarif.angebot.angenommen → vertragd creates Rahmenvertrag.';

-- Angebotsnummer counter, one row per tenant and year.
--
-- The number was derived as `COUNT(*) + 1` over `angebote`, so two quotations
-- created at the same time read the same count and the second collided on
-- `UNIQUE (tenant, lf_mp_id, angebotsnummer)`. An upsert on this row hands out
-- each number exactly once.
CREATE TABLE angebot_sequenzen (
    tenant        TEXT     NOT NULL,
    jahr          INTEGER  NOT NULL,
    letzte_nummer BIGINT   NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant, jahr)
);

COMMENT ON COLUMN angebote.varianten IS
    'Array of AngebotVariante: alternative scenarios for comparison. '
    'gewaehlte_variante is the index selected by customer on acceptance.';

CREATE INDEX angebote_tenant_status ON angebote (tenant, lf_mp_id, status);
CREATE INDEX angebote_kunden        ON angebote (kunden_id) WHERE kunden_id IS NOT NULL;
CREATE INDEX angebote_gueltig       ON angebote (gueltig_bis)
    WHERE status IN ('ANGELEGT', 'VERSANDT');
