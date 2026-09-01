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
-- Regulatory anchors, verified against the primary sources:
--   § 41 Abs. 5 EnWG      Preisänderungsanzeige (2 Wochen; Haushaltskunden 1 Monat)
--                         + fristloses Sonderkündigungsrecht (Satz 4)
--   § 41 Abs. 8 EnWG      Kündigungsbestätigung in Textform beim Lieferantenwechsel
--   § 41b Abs. 1/5 EnWG   Textform; Umzugssonderkündigung mit 6 Wochen
--   § 20 StromGVV/GasGVV  Kündigung der Grundversorgung: 2 Wochen, Textform
--   § 5 Abs. 2/3 GVV      Grundversorgungspreise: 6 Wochen, nur zum Monatsersten
--   § 38 Abs. 4 EnWG      Ersatzversorgung endet spätestens nach 3 Monaten
--   § 309 Nr. 9 BGB       Verbraucher: ≤ 24 Monate, Verlängerung nur unbefristet
--   DSGVO Art. 15/17/20   Auskunft, Löschung, Datenübertragbarkeit
--
-- The deadline arithmetic itself lives in `src/domain.rs`, not in SQL.

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
    -- § 3 Nr. 57 EnWG: a Haushaltskunde buys energy predominantly for household
    -- consumption *or* consumes no more than 10 000 kWh a year for commercial
    -- purposes. It is therefore NOT the same fact as `kundentyp` — a small
    -- business under the threshold is one — and it decides three deadlines
    -- (§ 41 Abs. 5 notice, § 41b Abs. 5 Umzugskündigung, § 309 Nr. 9 BGB caps),
    -- so it is stored explicitly rather than guessed from the segment label.
    haushaltskunde      BOOLEAN     NOT NULL DEFAULT true,
    -- BO4E Geschaeftspartner (marktrolle=Endkunde)
    geschaeftspartner   JSONB,
    -- BO4E Person (B2C natural persons only; NULL = legal entity)
    person              JSONB,
    -- BO4E Zahlungsinformation (IBAN/BIC for SEPA; validated mod-97 on PUT)
    zahlungsinformation JSONB,
    organisations_id    TEXT,           -- company/org identifier from ERP
    umsatzsteuer_id     TEXT,           -- VAT-ID for B2B XRechnung
    zahlungsziel_tage   INTEGER     NOT NULL DEFAULT 14,
    -- § 13b Abs. 2 Nr. 5 lit. b UStG: this customer is a Stromwiederverkäufer
    -- (reseller — in practice evidenced by a USt 1 TH certificate). When true,
    -- billingd invoices the supply net (reverse charge — the recipient owes the
    -- VAT); § 13b is mandatory when its conditions are met, so this is master
    -- data, not a per-invoice choice.
    stromwiederverkaeufer BOOLEAN   NOT NULL DEFAULT false,
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

COMMENT ON COLUMN kunden.haushaltskunde IS
    '§ 3 Nr. 57 EnWG Haushaltskunde. Drives the § 41 Abs. 5 notice period, the '
    '§ 41b Abs. 5 Umzugskündigung and the § 309 Nr. 9 BGB term caps.';

COMMENT ON COLUMN kunden.person IS
    'BO4E Person BO — B2C natural person (vorname, nachname, geburtsdatum, anrede). '
    'NULL = legal entity (B2B). Validated on PUT /kunden/{id}/person.';

COMMENT ON COLUMN kunden.zahlungsinformation IS
    'BO4E Zahlungsinformation COM (IBAN, BIC, Zahlungsart). '
    'IBAN validated via ISO 13616 mod-97 on PUT. NULL = no SEPA mandate.';

CREATE INDEX kunden_typ     ON kunden (tenant, kundentyp);
-- UNIQUE partial index for ON CONFLICT (tenant, erp_kunde_id) DO UPDATE
CREATE UNIQUE INDEX kunden_erp_unique ON kunden (tenant, erp_kunde_id)
    WHERE erp_kunde_id IS NOT NULL;

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

CREATE INDEX identitaeten_kunde ON kunden_identitaeten (kunden_id, tenant) WHERE aktiv = true;
CREATE INDEX identitaeten_email ON kunden_identitaeten (tenant, email) WHERE email IS NOT NULL;

-- ── Rahmenverträge (B2B Framework Contracts) ─────────────────────────────────

CREATE SEQUENCE rahmenvertrag_nr_seq;

CREATE TABLE rahmenvertraege (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kunden_id               UUID        NOT NULL REFERENCES kunden(id),
    tenant                  TEXT        NOT NULL,
    -- § 41 Abs. 1 Nr. 1 EnWG expects the contract to identify itself, and every
    -- invoice, Mahnung and support call quotes this number. Generated, so no
    -- contract can exist without one.
    rahmenvertrag_nr        TEXT        NOT NULL DEFAULT
                                'RV-' || to_char(heute(), 'YYYY') || '-' ||
                                lpad(nextval('rahmenvertrag_nr_seq')::TEXT, 8, '0'),
    status                  TEXT        NOT NULL DEFAULT 'AKTIV'
                            CHECK (status IN ('ENTWURF','AKTIV','GEKÜNDIGT','ABGELAUFEN')),
    gueltig_von             DATE        NOT NULL,
    gueltig_bis             DATE
                            CHECK (gueltig_bis IS NULL OR gueltig_von <= gueltig_bis),
    kuendigungsfrist_monate INTEGER     NOT NULL DEFAULT 3 CHECK (kuendigungsfrist_monate >= 0),
    auto_renewal            BOOLEAN     NOT NULL DEFAULT true,
    renewal_monate          INTEGER     NOT NULL DEFAULT 12 CHECK (renewal_monate >= 0),
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
    UNIQUE (tenant, rahmenvertrag_nr)
);

CREATE INDEX rahmen_kunden ON rahmenvertraege (kunden_id, tenant, status);
CREATE UNIQUE INDEX rahmen_erp_unique ON rahmenvertraege (tenant, erp_rahmenvertrag_id)
    WHERE erp_rahmenvertrag_id IS NOT NULL;

-- ── GGV-Betreiber (§ 42b EnWG) ────────────────────────────────────────────────
--
-- The operator of a Gemeinschaftliche Gebäudeversorgung is the LF's *customer*
-- for the bundled GGV Sammelrechnung — the BG-7 buyer of that document. It is
-- deliberately not a Marktpartner: a GGV-Betreiber has no MP-ID and never
-- appears in MaKo (§ 42b keeps the participants' ordinary supply
-- relationships), so its master data lives here with every other buyer, not in
-- marktd. `ggv_id` is the operator-assigned community identifier billingd and
-- edmd key allocation and billing on; this table is the one place that says
-- who is behind it.

CREATE TABLE ggv_betreiber (
    tenant     TEXT        NOT NULL,
    ggv_id     TEXT        NOT NULL,
    kunden_id  UUID        NOT NULL REFERENCES kunden(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, ggv_id)
);

COMMENT ON TABLE ggv_betreiber IS
    'The § 42b EnWG GGV operator as a Kunde — the BG-7 buyer of the bundled '
    'GGV Sammelrechnung. Keyed by the operator-assigned ggv_id billingd bills on.';

-- ── Versorgungsverträge (Individual Supply Contracts) ─────────────────────────

CREATE SEQUENCE vertrags_nr_seq;

CREATE TABLE versorgungsvertraege (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kunden_id               UUID        NOT NULL REFERENCES kunden(id),
    rahmenvertrag_id        UUID        REFERENCES rahmenvertraege(id),
    tenant                  TEXT        NOT NULL,
    vertrags_nr             TEXT        NOT NULL DEFAULT
                                'VV-' || to_char(heute(), 'YYYY') || '-' ||
                                lpad(nextval('vertrags_nr_seq')::TEXT, 8, '0'),
    -- Which supply regime governs this contract. Every notice period below
    -- branches on it, so it is a stored fact rather than a derived guess.
    vertragsart             TEXT        NOT NULL DEFAULT 'SONDERVERTRAG' CHECK (vertragsart IN (
                                'GRUNDVERSORGUNG',  -- § 36 EnWG, StromGVV/GasGVV apply
                                'ERSATZVERSORGUNG', -- § 38 EnWG, ends after 3 months
                                'SONDERVERTRAG'     -- § 41b EnWG
                            )),
    status                  TEXT        NOT NULL DEFAULT 'ANGELEGT' CHECK (status IN (
                                'ANGELEGT',
                                'IN_BEARBEITUNG',
                                'TEILERFUELLUNG',
                                'AKTIV',
                                'ABGELEHNT',
                                'GEKÜNDIGT',
                                'ABGELAUFEN',
                                'STORNIERT'
                            )),
    vertragsbeginn          DATE        NOT NULL,
    -- NULL = unbefristet. § 309 Nr. 9 lit. b BGB makes this the *only* lawful
    -- shape for a consumer contract after a tacit extension.
    vertragsende            DATE
                        CHECK (vertragsende IS NULL OR vertragsbeginn <= vertragsende),
    kundentyp               TEXT        NOT NULL,
    -- § 41 EnWG Preisgarantie
    preisgarantie_bis       DATE,
    preisgarantie           JSONB,      -- BO4E Preisgarantie COM (synced with preisgarantie_bis)
    kuendigungsfrist_monate INTEGER     NOT NULL DEFAULT 1 CHECK (kuendigungsfrist_monate >= 0),
    -- §40b EnWG: chosen billing cadence. The supplier must offer monthly,
    -- quarterly and semi-annual billing in addition to the annual default;
    -- the customer's choice is a contract fact and drives the billingd
    -- billing-run worker.
    abrechnungszyklus       TEXT        NOT NULL DEFAULT 'JAEHRLICH' CHECK (abrechnungszyklus IN (
                                'MONATLICH','VIERTELJAEHRLICH','HALBJAEHRLICH','JAEHRLICH'
                            )),
    auto_renewal            BOOLEAN     NOT NULL DEFAULT false,
    -- 0 = extend into an unbefristeten Vertrag (§ 309 Nr. 9 lit. b BGB — the
    -- only lawful tacit extension of a consumer contract). > 0 = a further
    -- fixed term, permitted for business customers (§ 310 Abs. 1 BGB).
    renewal_monate          INTEGER     NOT NULL DEFAULT 0 CHECK (renewal_monate >= 0),
    -- The vertragsende whose advance Ankündigung has gone out. Compared against
    -- the current vertragsende, so each notice fires once per term rather than
    -- once per day.
    autoerneuerung_notif_fuer DATE,
    ablauf_notif_fuer         DATE,
    -- ── Kündigung ────────────────────────────────────────────────────────────
    -- The reason decides the notice period (see src/domain.rs), and § 41 Abs. 8
    -- Nr. 2 EnWG obliges the supplier to confirm receipt in Textform — recorded
    -- here so the obligation is auditable rather than assumed.
    kuendigung_grund        TEXT        CHECK (kuendigung_grund IS NULL OR kuendigung_grund IN (
                                'ORDENTLICH','PREISANPASSUNG','UMZUG','LIEFERANTENWECHSEL'
                            )),
    kuendigung_eingang      DATE,
    kuendigung_zum          DATE,
    kuendigungsbestaetigung_am TIMESTAMPTZ,
    bundle_code             TEXT,
    standort_bezeichnung    TEXT,       -- e.g. "Werk Nord" for B2B site identification
    standort_adresse        JSONB,      -- BO4E Adresse of the supply address (PII)
    zahlungsziel_tage       INTEGER,    -- NULL = use kunden.zahlungsziel_tage
    erp_contract_id         TEXT,
    notizen                 TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at            TIMESTAMPTZ,
    UNIQUE (tenant, vertrags_nr)
);

COMMENT ON COLUMN versorgungsvertraege.standort_adresse IS
    'BO4E Adresse of the supply location. Personal data — pseudonymised by the '
    'DSGVO Art. 17 erasure path together with the Kunde''s own master data.';

CREATE INDEX vv_kunden ON versorgungsvertraege (kunden_id, tenant, status);
CREATE INDEX vv_rahmen ON versorgungsvertraege (rahmenvertrag_id) WHERE rahmenvertrag_id IS NOT NULL;
CREATE INDEX vv_status ON versorgungsvertraege (tenant, status)
    WHERE status IN ('ANGELEGT','IN_BEARBEITUNG','TEILERFUELLUNG','AKTIV','GEKÜNDIGT');
CREATE UNIQUE INDEX vv_erp_unique ON versorgungsvertraege (tenant, erp_contract_id)
    WHERE erp_contract_id IS NOT NULL;

-- ── Messstellenverträge (§ 9, § 10 MsbG) ──────────────────────────────────────
--
-- The contract a Messstellenbetreiber holds with the Anschlussnutzer or
-- Anschlussnehmer for one Messlokation. It lives here for the same reason
-- `aggregatorvertraege` does: it is Contract-context master data — parties,
-- notice period, validity window — that another service reads over HTTP and
-- keeps no copy of. `marktd` holds *market* data (Lokationen, Marktpartner,
-- Zuordnungen); a contract with a customer is not that.
--
-- WiM Strom Teil 1 Kap. 2.1.3 makes the Kündigung MSB a **contract-layer**
-- process between the two Messstellenbetreiber — the Netzbetreiber is not a
-- party and the Kündigung is not constitutive for the Wechsel. Every
-- Prüfschritt of `E_0200` is therefore a question about this row:
--
--   * `Z12` Ablehnung Vertragsbindung → the notice period, and the answer must
--     name the next admissible date. Derived from `kuendigungsfrist_monate` by
--     `domain::kuendigungsfrist`, not stored — one date to keep correct rather
--     than two.
--   * `Z34` Mehrfachkündigung and `Z29` kein Vertragsverhältnis mehr →
--     `kuendigung_zum` / `beendet_am`, plus the Kap. 2.2.3 table for a contract
--     already terminated.
--   * `ZC9` keine Zuordnung möglich → no row at all.
--
-- `kunden_id` is optional: a gMSB serving a Messlokation under its statutory
-- Grundzuständigkeit (§ 3 MsbG) has no contract with a named customer, and a
-- required FK would force a phantom Kunde for every such Messlokation.
CREATE TABLE messstellenvertraege (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant                  TEXT        NOT NULL,
    -- The Messlokation, as marktd names it. Not an FK: vertragd holds no
    -- Lokationsregister, and a contract may be recorded before the MeLo is.
    melo_id                 TEXT        NOT NULL,
    -- The MSB holding the contract — this deployment when it acts as MSBA.
    msb_mp_id               TEXT        NOT NULL,
    kunden_id               UUID        REFERENCES kunden(id),
    vertragsbeginn          DATE        NOT NULL,
    kuendigungsfrist_monate INTEGER     NOT NULL DEFAULT 1
                            CHECK (kuendigungsfrist_monate >= 0),
    -- Set once a Kündigung has taken effect, by an MSBN in the customer's name
    -- or by the customer directly. Kap. 2.2.3: „Ein bereits wirksam gekündigtes
    -- Vertragsverhältnis kann nicht … durch eine schlichte Kündigung zu einem
    -- späteren Zeitpunkt wieder verlängert werden."
    kuendigung_zum          DATE,
    kuendigung_eingang      DATE,
    -- The earliest end the MSBA would still accept on an already terminated
    -- contract — Kap. 2.2.3 „Fall 1". NULL = the Vertragsende cannot move.
    frueher_moeglich        DATE,
    -- Set when the contract has ended; `Z29` rather than `Z12`.
    beendet_am              DATE,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT msv_dates_ordered CHECK (
        (kuendigung_zum IS NULL OR kuendigung_zum >= vertragsbeginn)
        AND (beendet_am IS NULL OR beendet_am >= vertragsbeginn)
    ),
    -- „Fall 1" is only meaningful against an existing Vertragsende and must be
    -- earlier than it — otherwise it is not a *früheres* Vertragsende.
    CONSTRAINT msv_frueher_is_earlier CHECK (
        frueher_moeglich IS NULL
        OR (kuendigung_zum IS NOT NULL AND frueher_moeglich < kuendigung_zum)
    )
);

COMMENT ON TABLE messstellenvertraege IS
    'The Messstellenbetriebsvertrag per Messlokation (§ 9, § 10 MsbG). Read by '
    'processd over HTTP to answer a WiM Kündigung MSB out of E_0200.';

-- One contract per MSB per Messlokation at any instant: two would let the
-- Kündigung answer depend on row order.
ALTER TABLE messstellenvertraege
    ADD CONSTRAINT msv_no_overlap EXCLUDE USING gist (
        tenant    WITH =,
        melo_id   WITH =,
        msb_mp_id WITH =,
        daterange(vertragsbeginn, COALESCE(beendet_am, kuendigung_zum), '[)') WITH &&
    );

CREATE INDEX msv_lookup ON messstellenvertraege (tenant, melo_id, msb_mp_id);
CREATE INDEX msv_kunde  ON messstellenvertraege (kunden_id) WHERE kunden_id IS NOT NULL;

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
    lieferbeginn            DATE        NOT NULL,
    lieferende              DATE
                        CHECK (lieferende IS NULL OR lieferbeginn <= lieferende),
    status                  TEXT        NOT NULL DEFAULT 'ANGELEGT' CHECK (status IN (
                                'ANGELEGT','ANGEMELDET','BESTAETIGT',
                                'AKTIV','BEENDET','ABGELEHNT','STORNIERT'
                            )),
    mako_process_id         TEXT,
    abgelehnt_erc           TEXT,
    abgelehnt_reason        TEXT,
    -- edmd reading-order id for the Beginn-/Schlussablesung (GPKE
    -- Ablesesteuerung) — the trail from a Schlussrechnung back to its reading.
    ablese_auftrag_id       UUID,
    fulfillment_data        JSONB,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX komp_vertrag  ON vertragskomponenten (vertrag_id);
CREATE INDEX komp_malo     ON vertragskomponenten (tenant, malo_id) WHERE malo_id IS NOT NULL;
CREATE INDEX komp_status   ON vertragskomponenten (tenant, status, sparte)
    WHERE status IN ('ANGELEGT','ANGEMELDET');
CREATE INDEX komp_prozess  ON vertragskomponenten (mako_process_id)
    WHERE mako_process_id IS NOT NULL;

-- ── Produktzuordnung je Komponente (valid-time) ──────────────────────────────
--
-- Which product a supply component is on, over time. This is a **contract**
-- fact: agreeing it is a Tarifwechsel, governed by § 41 Abs. 5 EnWG and by the
-- contract's Preisgarantie, and it is decided here — so it is stored here, once.
--
-- Storing it twice — a current-value `product_code` on the component plus an
-- asynchronously delivered projection in `productd.customer_products` — lets
-- the contract say one product while billing still prices the other, for as
-- long as the projection lags. The slice and the contract change commit in the
-- same transaction, so there is nothing to reconcile.
--
-- ## Half-open ranges
--
-- `[gueltig_von, gueltig_bis)` — `gueltig_bis` is the first day **not** covered,
-- NULL means open-ended. That is what makes consecutive slices tile a billing
-- period exactly: a Tarifwechsel on the 15th ends the old slice and starts the
-- new one on the *same* date, and no day belongs to both.
--
-- ## A future-dated Tarifwechsel is just a future slice
--
-- There is no "pending" state and nothing to apply on the Wirksamkeit date. The
-- slice already says when it starts; a reader asking for a date before it
-- simply does not see it. That removed three columns, a CHECK constraint and a
-- daily worker phase whose only job was to copy one column into another.

CREATE TABLE komponenten_produkte (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant        TEXT        NOT NULL,
    komp_id       UUID        NOT NULL REFERENCES vertragskomponenten(id) ON DELETE CASCADE,
    product_code  TEXT        NOT NULL,
    gueltig_von   DATE        NOT NULL,
    gueltig_bis   DATE,       -- exclusive; NULL = open-ended
    -- § 41 Abs. 5 EnWG: TRUE once the price-change notice announcing this slice
    -- went out. The first slice of a contract announces nothing — the customer
    -- agreed to it — so it is created already marked.
    preisanpassung_notif_sent BOOLEAN NOT NULL DEFAULT false,
    -- § 41 Abs. 5 Satz 1 EnWG — the **Umfang** of the announced change, as the
    -- notice states it: `[{bezeichnung, einheit, bisher, neu}, …]`.
    --
    -- Here rather than resolved from `productd` at notice time, and that is the
    -- whole point. `vertragd` owns which product a Marktlokation is on;
    -- `productd` owns what a product costs, and the two are deliberately not
    -- coupled (see BILLING.md § 3). More importantly, the question a customer
    -- or a Schlichtungsstelle asks afterwards is *"what were we told our new
    -- price would be"*, which is a fact about the notice — one that a catalogue
    -- lookup years later cannot answer, because the catalogue has moved on.
    --
    -- Supplied by whoever schedules the Tarifwechsel: they chose the tariff, so
    -- they hold both price sheets. NULL means the change was scheduled without
    -- them, and the § 41 Abs. 5 notice document is then **not** issued — a
    -- notice that states no Umfang is not a valid Preisänderungsanzeige, and
    -- issuing one would make an invalid notice indistinguishable from a sent
    -- one. The CloudEvent still goes out, so an ERP that composes the letter
    -- itself is unaffected.
    angekuendigte_preise JSONB,
    -- The `outputd` document that communicated the change, and when. A plain
    -- value, not a foreign key: outputd owns the document and keeps it
    -- append-only, which is what keeps the reference resolvable.
    dokument_id   UUID,
    dokument_issued_at TIMESTAMPTZ,
    grund         TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT kp_range_nonempty CHECK (gueltig_bis IS NULL OR gueltig_von < gueltig_bis),
    CONSTRAINT kp_dokument_dated CHECK ((dokument_id IS NULL) = (dokument_issued_at IS NULL))
);

COMMENT ON TABLE komponenten_produkte IS
    'Valid-time product assignment per Vertragskomponente. Half-open '
    '[gueltig_von, gueltig_bis). The single source of truth for which product a '
    'MaLo is billed on; billingd reads the slices covering its billing period.';

-- Two products for one component on one day is not a state that should exist:
-- it either double-bills or picks arbitrarily.
ALTER TABLE komponenten_produkte
    ADD CONSTRAINT kp_no_overlap
    EXCLUDE USING gist (
        komp_id WITH =,
        daterange(gueltig_von, gueltig_bis, '[)') WITH &&
    );

CREATE INDEX kp_komp    ON komponenten_produkte (komp_id, gueltig_von DESC);
CREATE INDEX kp_aktuell ON komponenten_produkte (komp_id) WHERE gueltig_bis IS NULL;
-- The § 41 Abs. 5 notice worker: future slices whose notice is still owed.
CREATE INDEX kp_notif   ON komponenten_produkte (tenant, gueltig_von)
    WHERE preisanpassung_notif_sent = false;

-- ── Outbound task queue (persist-before-dispatch) ─────────────────────────────
--
-- Every side effect vertragd owes another service is an obligation, not a best
-- effort: the NB is waiting for a UTILMD, the customer for a Schlussablesung,
-- billingd for the tariff the contract actually switched to. Firing those from
-- detached tasks meant a restart between the database write and the HTTP call
-- dropped them in silence, with nothing left to retry them.
--
-- The intent is therefore written in the SAME transaction as the contract
-- change, and one worker drains it with backoff and a dead-letter — the same
-- shape as the CloudEvent outbox, for the same reason.

CREATE TABLE outbound_tasks (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant           TEXT        NOT NULL,
    kind             TEXT        NOT NULL CHECK (kind IN (
                         'LIEFERBEGINN',      -- processd  UTILMD Anmeldung
                         'LIEFERENDE',        -- processd  UTILMD Abmeldung
                         'ABLESUNG_BEGINN',   -- edmd      GPKE Beginnablesung
                         'ABLESUNG_ENDE',     -- edmd      GPKE Schlussablesung
                         'ABRECHNUNGSKONTO'   -- accountingd billing account
                     )),
    -- The component the task belongs to; NULL for contract-level tasks.
    komp_id          UUID        REFERENCES vertragskomponenten(id) ON DELETE CASCADE,
    -- Everything the worker needs to perform the call, captured at enqueue time
    -- so a retry re-sends byte-identical content and cannot pick up a later,
    -- unrelated state.
    payload          JSONB       NOT NULL,
    -- Exactly-once enqueue. A repeatable action varies the key by what makes it
    -- distinct ('PRODUKTZUORDNUNG:{komp}:{wirksamkeit}:{code}'); a one-shot one
    -- does not ('LIEFERBEGINN:{komp}'), so an idempotent re-POST of the same
    -- erp_contract_id cannot enqueue a second UTILMD.
    dedupe_key       TEXT        NOT NULL,
    attempts         INTEGER     NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at     TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ,
    last_error       TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, dedupe_key)
);

COMMENT ON TABLE outbound_tasks IS
    'Durable queue for every outbound call vertragd owes processd / edmd / '
    'productd / accountingd. Written in the originating transaction and drained '
    'by one worker with exponential backoff and a dead-letter.';

CREATE INDEX outbound_pending ON outbound_tasks (next_attempt_at)
    WHERE completed_at IS NULL AND dead_lettered_at IS NULL;
CREATE INDEX outbound_dead ON outbound_tasks (tenant, dead_lettered_at)
    WHERE dead_lettered_at IS NOT NULL;

-- ── CloudEvent inbox (idempotent) ─────────────────────────────────────────────

CREATE TABLE received_events (
    event_id    TEXT        PRIMARY KEY,
    event_type  TEXT        NOT NULL,
    payload     JSONB       NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX received_events_age ON received_events (received_at);

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
    'Audit trail for a Preisgarantie bypass (override_preisgarantie=true). '
    'Every override must be justifiable. INSERT-only.';

CREATE INDEX pg_override_vertrag    ON preisgarantie_override_log (vertrag_id);
CREATE INDEX pg_override_tenant_time ON preisgarantie_override_log (tenant, overridden_at DESC);

-- ── Aggregatorverträge (§41e EnWG) ───────────────────────────────────────────
--
-- Contracts between an Aggregator (VPP operator) and the operator of a
-- generation plant or a Letztverbraucher, per §41e EnWG — the German
-- transposition of Art. 17 RL (EU) 2019/944 ("Demand response through
-- aggregation").
--
-- This is Contract-context master data: parties, agreed capacity price, and
-- validity window. `billingd` reads it over HTTP when settling a dispatch; it
-- owns no copy. The dispatch idempotency guard (`vpp_dispatch_ledger`) stays in
-- `billingd` because it references `billing_records`.

CREATE TABLE aggregatorvertraege (
    id                          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant                      TEXT        NOT NULL,
    -- SteuerbareRessource-ID (C…) or NeLo-ID from marktd
    sr_id                       TEXT        NOT NULL,
    -- Operator-assigned VPP portfolio identifier
    vpp_id                      TEXT        NOT NULL,
    malo_id                     TEXT        NOT NULL,
    -- Aggregator market-partner ID (the invoicing party)
    aggregator_mp_id            TEXT        NOT NULL,
    -- Agreed Einsatzkosten in EUR/kWh
    capacity_price_eur_per_kwh  NUMERIC(12, 6) NOT NULL
                                CHECK (capacity_price_eur_per_kwh >= 0),
    vertragsbeginn              DATE        NOT NULL,
    -- Strictly less: the no-overlap constraint below reads the term as the
    -- half-open range [beginn, ende), so beginn = ende is an empty range — a
    -- contract that is never in force, and that overlaps nothing.
    vertragsende                DATE
                                CHECK (vertragsende IS NULL OR vertragsbeginn < vertragsende),
    -- MwSt override; NULL = use the billing default
    mwst_rate_override          NUMERIC(5, 4),
    kunden_id                   UUID        REFERENCES kunden(id),
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, sr_id, vertragsbeginn)
);

COMMENT ON TABLE aggregatorvertraege IS
    '§41e EnWG Aggregatorvertrag (Art. 17 RL (EU) 2019/944 demand response '
    'through aggregation): SR-ID -> agreed capacity price and validity. '
    'Read by billingd when settling de.vpp.dispatch.confirmed.';

CREATE INDEX agg_sr_tenant ON aggregatorvertraege (tenant, sr_id, vertragsbeginn DESC);
CREATE INDEX agg_kunde     ON aggregatorvertraege (kunden_id) WHERE kunden_id IS NOT NULL;

-- Only one Aggregatorvertrag may be active per SR at any instant.
CREATE EXTENSION IF NOT EXISTS btree_gist;
ALTER TABLE aggregatorvertraege
    ADD CONSTRAINT agg_no_overlap
    EXCLUDE USING gist (
        tenant WITH =,
        sr_id  WITH =,
        daterange(vertragsbeginn, vertragsende, '[)') WITH &&
    );
