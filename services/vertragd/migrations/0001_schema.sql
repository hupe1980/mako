-- ── vertragd schema — Contract & Customer Management (LF role) ───────────────
--
-- Data model:
--   Kunde (B2C: Haushalt/Gewerbe-SLP, B2B: Unternehmen/RLM)
--   ├── N × KundenIdentitaet  (OIDC portal users; 1:1 for B2C, 1:N for B2B)
--   ├── [B2B] Rahmenvertrag   (master framework contract)
--   │   └── N × Versorgungsvertrag (individual supply contract per site)
--   │         └── N × Vertragskomponente (per commodity: STROM|GAS|HEMS|...)
--   └── [B2C] Versorgungsvertrag (single contract, no Rahmenvertrag)
--         └── N × Vertragskomponente
--
-- Regulatory: §41 EnWG (Preisgarantie, Preisanpassung), GDPR Art. 15/17/20.

-- ── Kunden ────────────────────────────────────────────────────────────────────

CREATE TABLE kunden (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    kunden_nr           TEXT,
    kundentyp           TEXT        NOT NULL CHECK (kundentyp IN (
                            'B2C',          -- private household / SLP
                            'B2B_SLP',      -- small business / SLP
                            'B2B_RLM',      -- commercial & industrial / RLM
                            'B2B_HV'        -- high-voltage / directly connected
                        )),
    -- BO4E Geschaeftspartner (marktrolle=Endkunde)
    geschaeftspartner   JSONB,
    -- BO4E Person (B2C natural persons only; NULL = legal entity)
    person              JSONB,
    -- BO4E Zahlungsinformation (IBAN/BIC for SEPA; validated mod-97 on PUT)
    zahlungsinformation JSONB,
    organisations_id    TEXT,           -- company/org identifier from ERP
    umsatzsteuer_id     TEXT,           -- VAT-ID for B2B XRechnung
    zahlungsziel_tage   INTEGER     NOT NULL DEFAULT 14,
    sepa_erlaubt        BOOLEAN     NOT NULL DEFAULT true,
    erp_kunde_id        TEXT,           -- CRM idempotency key
    notizen             TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, kunden_nr)
);

COMMENT ON TABLE kunden IS
    'Legal entity (B2C person or B2B company). Not the portal user. '
    'KundenIdentitaeten maps OIDC identities to a Kunde.';

COMMENT ON COLUMN kunden.person IS
    'BO4E Person BO — B2C natural person (vorname, nachname, geburtsdatum, anrede). '
    'NULL = legal entity (B2B). Validated on PUT /kunden/{id}/person.';

COMMENT ON COLUMN kunden.zahlungsinformation IS
    'BO4E Zahlungsinformation COM (IBAN, BIC, Zahlungsart). '
    'IBAN validated via ISO 13616 mod-97 on PUT. NULL = no SEPA mandate.';

CREATE INDEX kunden_erp     ON kunden (tenant, erp_kunde_id) WHERE erp_kunde_id IS NOT NULL;
CREATE INDEX kunden_typ     ON kunden (tenant, kundentyp);
-- UNIQUE partial index for ON CONFLICT (tenant, erp_kunde_id) DO UPDATE
CREATE UNIQUE INDEX kunden_erp_unique ON kunden (tenant, erp_kunde_id)
    WHERE erp_kunde_id IS NOT NULL;
CREATE INDEX kunden_person  ON kunden ((person IS NOT NULL)) WHERE person IS NOT NULL;

-- ── KundenIdentitaeten (Portal Users) ────────────────────────────────────────

CREATE TABLE kunden_identitaeten (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kunden_id       UUID        NOT NULL REFERENCES kunden(id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    oidc_sub        TEXT        NOT NULL,
    email           TEXT,
    display_name    TEXT,
    rolle           TEXT        NOT NULL DEFAULT 'VOLLZUGRIFF' CHECK (rolle IN (
                        'VOLLZUGRIFF',  -- B2C default: full read access to own data
                        'ADMIN',        -- B2B: full read + self-service
                        'FINANZEN',     -- B2B: invoices + balance only
                        'TECHNIK',      -- B2B: meter data + Lastgang only
                        'READONLY'      -- any: read-only, no self-service
                    )),
    -- B2B site-scoped access: only sees MaLos matching this standort_bezeichnung
    standort_filter TEXT,
    aktiv           BOOLEAN     NOT NULL DEFAULT true,
    eingeladen_am   TIMESTAMPTZ,
    letzter_login   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, oidc_sub)
);

COMMENT ON TABLE kunden_identitaeten IS
    'OIDC portal user identities mapped to a Kunde. '
    'B2C: 1:1. B2B: 1:N (different roles per employee). '
    'portald authorization: GET /kunden/authenticate?malo_id={malo_id}';

CREATE INDEX identitaeten_sub   ON kunden_identitaeten (tenant, oidc_sub);
CREATE INDEX identitaeten_kunde ON kunden_identitaeten (kunden_id, tenant) WHERE aktiv = true;
CREATE INDEX identitaeten_email ON kunden_identitaeten (tenant, email) WHERE email IS NOT NULL;

-- ── Rahmenverträge (B2B Framework Contracts) ─────────────────────────────────

CREATE TABLE rahmenvertraege (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kunden_id               UUID        NOT NULL REFERENCES kunden(id),
    tenant                  TEXT        NOT NULL,
    rahmenvertrag_nr        TEXT,
    vertrag                 JSONB,      -- rubo4e::current::Vertrag (vertragsart=RAHMENVERTRAG)
    status                  TEXT        NOT NULL DEFAULT 'AKTIV'
                            CHECK (status IN ('ENTWURF','AKTIV','GEKÜNDIGT','ABGELAUFEN')),
    gueltig_von             DATE        NOT NULL,
    gueltig_bis             DATE,
    kuendigungsfrist_monate INTEGER     NOT NULL DEFAULT 3,
    auto_renewal            BOOLEAN     NOT NULL DEFAULT true,
    renewal_monate          INTEGER     NOT NULL DEFAULT 12,
    preisanpassungsformel   TEXT,
    portfolio_rabatt_prozent NUMERIC(5, 2),
    angebot_id              UUID,
    rechnungsstellung       TEXT        NOT NULL DEFAULT 'EINZEL'
                            CHECK (rechnungsstellung IN ('EINZEL', 'SAMMEL', 'POSITIONEN')),
    sammelrechnung_intervall TEXT        DEFAULT 'MONATLICH'
                            CHECK (sammelrechnung_intervall IN ('MONATLICH', 'QUARTALSWEISE', 'JAEHRLICH')),
    erp_rahmenvertrag_id    TEXT,
    notizen                 TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, erp_rahmenvertrag_id)
);

CREATE INDEX rahmen_kunden ON rahmenvertraege (kunden_id, tenant, status);
CREATE INDEX rahmen_status ON rahmenvertraege (tenant, status) WHERE status = 'AKTIV';

-- ── Versorgungsverträge (Individual Supply Contracts) ─────────────────────────

CREATE TABLE versorgungsvertraege (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kunden_id               UUID        NOT NULL REFERENCES kunden(id),
    rahmenvertrag_id        UUID        REFERENCES rahmenvertraege(id),
    tenant                  TEXT        NOT NULL,
    vertrags_nr             TEXT,
    vertrag                 JSONB,      -- rubo4e::current::Vertrag (vertragsart=LIEFERVERTRAG)
    status                  TEXT        NOT NULL DEFAULT 'ANGELEGT' CHECK (status IN (
                                'ANGELEGT',
                                'IN_BEARBEITUNG',
                                'TEILERFUELLUNG',
                                'AKTIV',
                                'ÄNDERUNG',
                                'GEKÜNDIGT',
                                'ABGELAUFEN',
                                'STORNIERT'
                            )),
    vertragsbeginn          DATE        NOT NULL,
    vertragsende            DATE,
    kundentyp               TEXT        NOT NULL,
    -- §41 EnWG Preisgarantie
    preisgarantie_bis       DATE,
    preisgarantie           JSONB,      -- BO4E Preisgarantie COM (synced with preisgarantie_bis)
    kuendigungsfrist_monate INTEGER     NOT NULL DEFAULT 1,
    auto_renewal            BOOLEAN     NOT NULL DEFAULT false,
    renewal_monate          INTEGER     NOT NULL DEFAULT 12,
    naechste_moegliche_kuendigung DATE,
    bundle_code             TEXT,
    standort_bezeichnung    TEXT,       -- e.g. "Werk Nord" for B2B site identification
    standort_adresse        JSONB,
    zahlungsziel_tage       INTEGER,    -- NULL = use kunden.zahlungsziel_tage
    erp_contract_id         TEXT,
    notizen                 TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at            TIMESTAMPTZ,
    UNIQUE (tenant, erp_contract_id)
);

CREATE INDEX vv_kunden ON versorgungsvertraege (kunden_id, tenant, status);
CREATE INDEX vv_rahmen ON versorgungsvertraege (rahmenvertrag_id) WHERE rahmenvertrag_id IS NOT NULL;
CREATE INDEX vv_status ON versorgungsvertraege (tenant, status)
    WHERE status IN ('ANGELEGT','IN_BEARBEITUNG','TEILERFUELLUNG','AKTIV','GEKÜNDIGT');

-- ── Vertragskomponenten (Supply positions per commodity) ──────────────────────

CREATE TABLE vertragskomponenten (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    vertrag_id              UUID        NOT NULL REFERENCES versorgungsvertraege(id) ON DELETE CASCADE,
    tenant                  TEXT        NOT NULL,
    sparte                  TEXT        NOT NULL CHECK (sparte IN (
                                'STROM','GAS','WAERME','SOLAR','EEG','EINSPEISUNG',
                                'WAERMEPUMPE','WALLBOX','HEMS','EMOBILITY','ENERGIEDIENSTLEISTUNG'
                            )),
    malo_id                 TEXT,
    melo_id                 TEXT,
    lf_mp_id                TEXT        NOT NULL,
    nb_mp_id                TEXT,
    product_code            TEXT        NOT NULL,
    lieferbeginn            DATE        NOT NULL,
    lieferende              DATE,
    status                  TEXT        NOT NULL DEFAULT 'ANGELEGT' CHECK (status IN (
                                'ANGELEGT','ANGEMELDET','BESTAETIGT',
                                'AKTIV','BEENDET','ABGELEHNT','STORNIERT'
                            )),
    mako_process_id         TEXT,
    abgelehnt_erc           TEXT,
    abgelehnt_reason        TEXT,
    ablese_auftrag_id       UUID,
    -- §41 Abs. 3 EnWG: planned Tarifwechsel pending approval
    pending_product_code    TEXT,
    pending_wirksamkeit     DATE,
    -- TRUE once the 6-week §41 Abs. 3 price-change notification was dispatched
    preisanpassung_notif_sent BOOLEAN   NOT NULL DEFAULT false,
    fulfillment_data        JSONB,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON COLUMN vertragskomponenten.pending_product_code IS
    'New product_code for a planned Tarifwechsel taking effect on pending_wirksamkeit. '
    'NULL = no pending change.';

COMMENT ON COLUMN vertragskomponenten.preisanpassung_notif_sent IS
    '§41 Abs. 3 EnWG: TRUE once the ≥6-week price-change notification '
    '(de.vertrag.preisaenderung.ankuendigung) was dispatched.';

CREATE INDEX komp_vertrag  ON vertragskomponenten (vertrag_id);
CREATE INDEX komp_malo     ON vertragskomponenten (malo_id) WHERE malo_id IS NOT NULL;
CREATE INDEX komp_status   ON vertragskomponenten (tenant, status, sparte)
    WHERE status IN ('ANGELEGT','ANGEMELDET');
CREATE INDEX komp_prozess  ON vertragskomponenten (mako_process_id)
    WHERE mako_process_id IS NOT NULL;
CREATE INDEX komp_pending_wirksamkeit ON vertragskomponenten (pending_wirksamkeit)
    WHERE pending_wirksamkeit IS NOT NULL;

-- ── CloudEvent inbox (idempotent) ─────────────────────────────────────────────

CREATE TABLE received_events (
    event_id    TEXT        PRIMARY KEY,
    event_type  TEXT        NOT NULL,
    payload     JSONB       NOT NULL,
    processed   BOOLEAN     NOT NULL DEFAULT false,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── GDPR Art. 17 anonymization log (INSERT-only) ──────────────────────────────

CREATE TABLE anonymization_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    kunden_id       UUID        NOT NULL,    -- no FK — kunden row may be deleted
    anonymized_fields TEXT[]    NOT NULL,
    requested_by    TEXT        NOT NULL,
    request_reason  TEXT,
    retention_basis TEXT,
    anonymized_at   TIMESTAMPTZ NOT NULL DEFAULT now()
    -- INSERT-only — rows MUST NOT be updated or deleted
);

COMMENT ON TABLE anonymization_log IS
    'GDPR Art. 17 erasure audit trail. INSERT-only. '
    'Proves compliance per GDPR Art. 5(2) accountability.';

CREATE INDEX anon_log_kunde      ON anonymization_log (kunden_id);
CREATE INDEX anon_log_tenant_time ON anonymization_log (tenant, anonymized_at DESC);

-- ── Preisgarantie override audit trail ───────────────────────────────────────

CREATE TABLE preisgarantie_override_log (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    vertrag_id          UUID        NOT NULL,
    komp_id             UUID        NOT NULL,
    preisgarantie_bis   DATE        NOT NULL,
    wirksamkeit         DATE        NOT NULL,
    old_product_code    TEXT        NOT NULL,
    new_product_code    TEXT        NOT NULL,
    operator_identity   TEXT        NOT NULL,
    override_reason     TEXT,
    overridden_at       TIMESTAMPTZ NOT NULL DEFAULT now()
    -- INSERT-only
);

COMMENT ON TABLE preisgarantie_override_log IS
    'Audit trail for §41 EnWG Preisgarantie bypass (override_preisgarantie=true). '
    'Every override must be justifiable. INSERT-only.';

CREATE INDEX pg_override_vertrag    ON preisgarantie_override_log (vertrag_id);
CREATE INDEX pg_override_tenant_time ON preisgarantie_override_log (tenant, overridden_at DESC);
