-- ── vertragd 0001: B2B + B2C Contract & Customer Management ──────────────────
--
-- Data model:
--
--   Kunde (B2C: Haushalt/Gewerbe-SLP, B2B: Unternehmen/RLM)
--   ├── N × KundenIdentitaet  (OIDC users with portal access — 1:1 for B2C, 1:N for B2B)
--   │     e.g. CEO, Accountant, Site Manager each get their own login
--   ├── [B2B] Rahmenvertrag (Master Framework Contract)
--   │   └── N × Versorgungsvertrag  (Individual Supply Contract, one per site/commodity bundle)
--   │         └── N × Vertragskomponente  (per sparte: STROM|GAS|HEMS|...)
--   └── [B2C] Versorgungsvertrag    (single contract, no Rahmenvertrag)
--         └── N × Vertragskomponente
--
-- Key distinctions:
--   1. Kunde is the legal entity (company or person) — not the portal user
--   2. KundenIdentitaet maps OIDC identities to a Kunde (1:N for B2B)
--   3. B2B portal users may have scoped access (e.g. read-only, finance-only, site-specific)
--   4. Rahmenvertrag enables B2B portfolio management
--   5. Versorgungsvertrag is the persistent legal contract
--   6. B2B: 30/60/90 day payment, portfolio pricing, multi-site Lieferbeginn
--   7. Vertragsmanagement: price guarantees, notice periods, auto-renewal

-- ── Kunden ───────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS kunden (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    kunden_nr           TEXT,                          -- operator-assigned customer number
    -- Classification
    kundentyp           TEXT        NOT NULL
                        CHECK (kundentyp IN (
                            'B2C',          -- private household / SLP
                            'B2B_SLP',      -- small business / SLP
                            'B2B_RLM',      -- commercial & industrial / RLM
                            'B2B_HV'        -- high-voltage / directly connected
                        )),
    -- BO4E typed customer profile
    -- rubo4e::current::Geschaeftspartner with marktrolle=Endkunde
    geschaeftspartner   JSONB,
    -- B2B specific
    organisations_id    TEXT,                          -- company/org identifier from ERP
    umsatzsteuer_id     TEXT,                          -- VAT-ID for B2B e-invoicing
    zahlungsziel_tage   INT         NOT NULL DEFAULT 14,
    sepa_erlaubt        BOOLEAN     NOT NULL DEFAULT true, -- false = B2B invoice payment only
    -- Metadata
    erp_kunde_id        TEXT,                          -- CRM idempotency key
    notizen             TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, kunden_nr)
);

CREATE INDEX IF NOT EXISTS kunden_erp      ON kunden (tenant, erp_kunde_id) WHERE erp_kunde_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS kunden_typ      ON kunden (tenant, kundentyp);

-- ── KundenIdentitaeten (Portal Users) ────────────────────────────────────────
-- Maps OIDC identities to a Kunde.
--
-- B2C: exactly one identity per Kunde (1:1)
-- B2B: multiple identities per Kunde (1:N) — e.g.:
--   CEO (rolle=ADMIN)         → full read/write access
--   Buchhalter (rolle=FINANCE) → invoices + account balance only
--   Site Manager (rolle=SITE, site_filter='Werk Nord') → single-site MaLo access
--
-- portald authorization flow:
--   GET /kunden/authenticate?malo_id={malo_id}
--   1. Extract JWT sub from Authorization header
--   2. SELECT kunden_id FROM kunden_identitaeten WHERE oidc_sub=$sub AND tenant=$tenant
--   3. Check that malo_id is in this customer's active_malo_ids
--   4. Optionally check site_filter / rolle scope

CREATE TABLE IF NOT EXISTS kunden_identitaeten (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kunden_id           UUID        NOT NULL REFERENCES kunden(id) ON DELETE CASCADE,
    tenant              TEXT        NOT NULL,
    -- OIDC identity
    oidc_sub            TEXT        NOT NULL,          -- OIDC subject claim
    email               TEXT,
    display_name        TEXT,                          -- human-readable, e.g. "Max Mustermann"
    -- Access control
    rolle               TEXT        NOT NULL DEFAULT 'VOLLZUGRIFF'
                        CHECK (rolle IN (
                            'VOLLZUGRIFF',  -- B2C default: full read access to own data
                            'ADMIN',        -- B2B: full read + self-service actions
                            'FINANZEN',     -- B2B: invoices + balance only
                            'TECHNIK',      -- B2B: meter data + Lastgang only
                            'READONLY'      -- any: read-only, no self-service actions
                        )),
    -- Optional B2B site scoping: if set, this identity can only see MaLos
    -- belonging to Versorgungsverträge with matching standort_bezeichnung.
    -- NULL = no restriction (sees all MaLos under this Kunde).
    standort_filter     TEXT,                          -- e.g. "Werk Nord"
    -- Activation
    aktiv               BOOLEAN     NOT NULL DEFAULT true,
    eingeladen_am       TIMESTAMPTZ,                   -- invitation sent timestamp
    letzter_login       TIMESTAMPTZ,
    -- Metadata
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, oidc_sub)                          -- one Kunde per OIDC identity per tenant
);

CREATE INDEX IF NOT EXISTS identitaeten_sub    ON kunden_identitaeten (tenant, oidc_sub);
CREATE INDEX IF NOT EXISTS identitaeten_kunde  ON kunden_identitaeten (kunden_id, tenant) WHERE aktiv = true;
CREATE INDEX IF NOT EXISTS identitaeten_email  ON kunden_identitaeten (tenant, email) WHERE email IS NOT NULL;

-- ── Rahmenverträge (B2B Framework Contracts) ─────────────────────────────────
-- Master agreement setting shared terms for all individual supply contracts.
-- Primarily used for B2B_RLM and B2B_HV customers with multiple delivery points.

CREATE TABLE IF NOT EXISTS rahmenvertraege (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kunden_id           UUID        NOT NULL REFERENCES kunden(id),
    tenant              TEXT        NOT NULL,
    rahmenvertrag_nr    TEXT,                          -- operator-assigned
    -- BO4E typed (rubo4e::current::Vertrag with vertragsart=RAHMENVERTRAG)
    vertrag             JSONB,
    status              TEXT        NOT NULL DEFAULT 'AKTIV'
                        CHECK (status IN ('ENTWURF','AKTIV','GEKÜNDIGT','ABGELAUFEN')),
    -- Validity
    gueltig_von         DATE        NOT NULL,
    gueltig_bis         DATE,                          -- NULL = open-ended
    -- Framework terms
    kuendigungsfrist_monate     INT NOT NULL DEFAULT 3,
    auto_renewal                BOOLEAN NOT NULL DEFAULT true,
    renewal_monate              INT NOT NULL DEFAULT 12,
    preisanpassungsformel       TEXT,                  -- indexation clause (e.g., §24 StromNEV)
    portfolio_rabatt_prozent    NUMERIC(5,2),          -- volume discount across all sites
    -- Tender / quotation reference
    angebot_id          UUID,                          -- reference to Ausschreibung
    -- Billing
    rechnungsstellung   TEXT NOT NULL DEFAULT 'EINZEL'
                        CHECK (rechnungsstellung IN (
                            'EINZEL',       -- separate invoice per Versorgungsvertrag
                            'SAMMEL',       -- consolidated portfolio invoice
                            'POSITIONEN'    -- one invoice, one position per site
                        )),
    sammelrechnung_intervall TEXT DEFAULT 'MONATLICH'
                        CHECK (sammelrechnung_intervall IN ('MONATLICH','QUARTALSWEISE','JAEHRLICH')),
    -- Metadata
    erp_rahmenvertrag_id TEXT,
    notizen             TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, erp_rahmenvertrag_id)
);

CREATE INDEX IF NOT EXISTS rahmen_kunden   ON rahmenvertraege (kunden_id, tenant, status);
CREATE INDEX IF NOT EXISTS rahmen_status   ON rahmenvertraege (tenant, status) WHERE status = 'AKTIV';

-- ── Versorgungsverträge (Individual Supply Contracts) ─────────────────────────
-- The persistent legal supply contract between LF and customer.
-- B2C: typically 1 per bundle (Strom + Gas + HEMS in one contract)
-- B2B: 1 per delivery location / site, references Rahmenvertrag for shared terms

CREATE TABLE IF NOT EXISTS versorgungsvertraege (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kunden_id           UUID        NOT NULL REFERENCES kunden(id),
    rahmenvertrag_id    UUID        REFERENCES rahmenvertraege(id), -- NULL for B2C / standalone
    tenant              TEXT        NOT NULL,
    vertrags_nr         TEXT,                          -- operator-assigned
    -- BO4E typed (rubo4e::current::Vertrag with vertragsart=LIEFERVERTRAG)
    vertrag             JSONB,
    status              TEXT        NOT NULL DEFAULT 'ANGELEGT'
                        CHECK (status IN (
                            'ANGELEGT',         -- created, MaKo not yet dispatched
                            'IN_BEARBEITUNG',   -- MaKo Lieferbeginn dispatched
                            'TEILERFUELLUNG',   -- some components confirmed
                            'AKTIV',            -- all components active, billing running
                            'ÄNDERUNG',         -- Tarifwechsel or modification in progress
                            'GEKÜNDIGT',        -- notice given, still supplying
                            'ABGELAUFEN',       -- supply ended, Schlussrechnung produced
                            'STORNIERT'         -- cancelled before activation
                        )),
    -- Contract dates
    vertragsbeginn      DATE        NOT NULL,
    vertragsende        DATE,                          -- NULL = open-ended
    -- Terms
    kundentyp           TEXT        NOT NULL,          -- B2C | B2B_SLP | B2B_RLM | B2B_HV
    preisgarantie_bis   DATE,
    preisgarantie       JSONB,                         -- BO4E Preisgarantie COM (typed; synced with preisgarantie_bis)
    kuendigungsfrist_monate INT NOT NULL DEFAULT 1,
    auto_renewal        BOOLEAN     NOT NULL DEFAULT false,
    renewal_monate      INT         NOT NULL DEFAULT 12,
    naechste_moegliche_kuendigung DATE, -- computed: vertragsbeginn + kuendigungsfrist
    -- Bundle / product reference
    bundle_code         TEXT,                          -- tarifbd BUNDLE product_code
    -- Delivery location (for B2B single-site contracts)
    standort_bezeichnung TEXT,                         -- e.g. "Werk Nord", "Büro Hamburg"
    standort_adresse    JSONB,                         -- BO4E Adresse
    -- Payment (B2B override from Rahmenvertrag or customer default)
    zahlungsziel_tage   INT,                           -- NULL = use kunden.zahlungsziel_tage
    -- Metadata
    erp_contract_id     TEXT,
    notizen             TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at        TIMESTAMPTZ,
    UNIQUE (tenant, erp_contract_id)
);

CREATE INDEX IF NOT EXISTS vv_kunden       ON versorgungsvertraege (kunden_id, tenant, status);
CREATE INDEX IF NOT EXISTS vv_rahmen       ON versorgungsvertraege (rahmenvertrag_id) WHERE rahmenvertrag_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS vv_status       ON versorgungsvertraege (tenant, status) WHERE status IN ('ANGELEGT','IN_BEARBEITUNG','TEILERFUELLUNG','AKTIV','GEKÜNDIGT');
CREATE INDEX IF NOT EXISTS vv_beginn       ON versorgungsvertraege (vertragsbeginn DESC);

-- ── Vertragskomponenten (Supply positions per contract) ───────────────────────
-- One row per commodity/service.
-- STROM/GAS/WAERME/SOLAR/EEG/WAERMEPUMPE/WALLBOX → MaKo Lieferbeginn via processd
-- HEMS/EMOBILITY/ENERGIEDIENSTLEISTUNG → direct fulfillment (no MaKo)

CREATE TABLE IF NOT EXISTS vertragskomponenten (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    vertrag_id          UUID        NOT NULL REFERENCES versorgungsvertraege(id) ON DELETE CASCADE,
    tenant              TEXT        NOT NULL,
    sparte              TEXT        NOT NULL
                        CHECK (sparte IN (
                            'STROM','GAS','WAERME','SOLAR','EEG','EINSPEISUNG',
                            'WAERMEPUMPE','WALLBOX','HEMS','EMOBILITY','ENERGIEDIENSTLEISTUNG'
                        )),
    -- Delivery point
    malo_id             TEXT,
    melo_id             TEXT,
    lf_mp_id            TEXT        NOT NULL,
    nb_mp_id            TEXT,
    -- Product
    product_code        TEXT        NOT NULL,
    -- Supply period
    lieferbeginn        DATE        NOT NULL,
    lieferende          DATE,
    -- Lifecycle
    status              TEXT        NOT NULL DEFAULT 'ANGELEGT'
                        CHECK (status IN (
                            'ANGELEGT','ANGEMELDET','BESTAETIGT',
                            'AKTIV','BEENDET','ABGELEHNT','STORNIERT'
                        )),
    -- MaKo process tracking
    mako_process_id     TEXT,
    abgelehnt_erc       TEXT,
    abgelehnt_reason    TEXT,
    -- Ablesesteuerung
    ablese_auftrag_id   UUID,
    -- Service-specific data
    fulfillment_data    JSONB,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS komp_vertrag    ON vertragskomponenten (vertrag_id);
CREATE INDEX IF NOT EXISTS komp_malo       ON vertragskomponenten (malo_id) WHERE malo_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS komp_status     ON vertragskomponenten (tenant, status, sparte) WHERE status IN ('ANGELEGT','ANGEMELDET');
CREATE INDEX IF NOT EXISTS komp_prozess    ON vertragskomponenten (mako_process_id) WHERE mako_process_id IS NOT NULL;

-- ── CloudEvent inbox (idempotent) ─────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS received_events (
    event_id    TEXT    PRIMARY KEY,
    event_type  TEXT    NOT NULL,
    payload     JSONB   NOT NULL,
    processed   BOOLEAN NOT NULL DEFAULT false,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- vertragd migration 0002: BO4E Person sub-object for B2C customers
--
-- Adds a typed `person JSONB` column to `kunden` so that natural-person details
-- (vorname, nachname, geburtsdatum, anrede) are stored as `rubo4e::current::Person`
-- camelCase rather than flat columns.
--
-- Regulatory: GDPR Art. 15 right-to-access + correct Anrede for correspondence.
-- NULL = legal entity (B2B Geschäftspartner); NOT NULL = B2C natural person.

ALTER TABLE kunden ADD COLUMN IF NOT EXISTS person JSONB;

COMMENT ON COLUMN kunden.person IS
    'BO4E Person BO — B2C natural person details (vorname, nachname, geburtsdatum, anrede). '
    'NULL = legal entity (B2B). Validated on PUT /kunden/{id}/person via rubo4e::current::Person.';

CREATE INDEX IF NOT EXISTS kunden_person_idx
    ON kunden ((person IS NOT NULL))
    WHERE person IS NOT NULL;

-- vertragd migration 0003: Planned Tarifwechsel tracking
--
-- B13: §41 Abs. 3 EnWG requires the LF to notify customers ≥ 6 weeks before a
--      price increase takes effect.  This migration adds columns to
--      `vertragskomponenten` to track Tarifwechsel that are scheduled for a
--      future `wirksamkeit` date and whether the 6-week notification has been
--      sent.
--
-- Workflow:
--   1. `POST /api/v1/vertraege/{id}/tarifwechsel` with `wirksamkeit > today`
--      → stores pending_product_code + pending_wirksamkeit (does NOT apply yet)
--   2. Background worker (daily):
--      - 42 days before `pending_wirksamkeit`: emit `de.vertrag.preisaenderung.ankuendigung`
--        CE → ERP dispatches customer letter/email; set `preisanpassung_notif_sent = TRUE`
--      - On `pending_wirksamkeit`: apply `product_code = pending_product_code`,
--        clear pending fields, emit `de.vertrag.tarifwechsel`
--
-- If `wirksamkeit ≤ today` the old immediate-apply behaviour is preserved.

ALTER TABLE vertragskomponenten
    ADD COLUMN IF NOT EXISTS pending_product_code TEXT,
    ADD COLUMN IF NOT EXISTS pending_wirksamkeit  DATE,
    ADD COLUMN IF NOT EXISTS preisanpassung_notif_sent BOOLEAN NOT NULL DEFAULT FALSE;

-- Fast lookup: all components with an upcoming planned Tarifwechsel.
CREATE INDEX IF NOT EXISTS komp_pending_wirksamkeit
    ON vertragskomponenten (pending_wirksamkeit)
    WHERE pending_wirksamkeit IS NOT NULL;

COMMENT ON COLUMN vertragskomponenten.pending_product_code IS
    'New product_code that will take effect on pending_wirksamkeit. '
    'NULL = no planned Tarifwechsel.';

COMMENT ON COLUMN vertragskomponenten.pending_wirksamkeit IS
    'Effective date for the planned Tarifwechsel. '
    'Background worker applies the change on this date and emits the 6-week '
    'advance notification (§41 Abs. 3 EnWG).';

COMMENT ON COLUMN vertragskomponenten.preisanpassung_notif_sent IS
    'TRUE = de.vertrag.preisaenderung.ankuendigung CloudEvent was already emitted '
    '≥ 6 weeks before pending_wirksamkeit. Prevents duplicate notifications.';
