-- ── einsd schema — Einspeiser Registry + EEG/KWKG Settlement ─────────────────
--
-- Regulatory frameworks:
--   EEG 2000–2023 (§§20–22, §24, §21 Abs. 3, §42b, §44b, §48–54)
--   KWKG 2023 (§7 KWK-Zuschlag)
--   § 147 AO / GoBD (settlement receipts are Buchungsbelege: 8-year retention)
--
-- Tables:
--   einspeiser                — the plant operator: payee, bank details, USt-Status
--   eeg_anlagen               — central plant register (composite PK: tr_id + tenant)
--   settlement_receipts       — monthly settlement audit log (§ 147 AO / GoBD)
--   settlement_period_accruals — per-period contribution to the cumulative counters
--   settlement_state_transitions — state machine audit trail
--   settlement_receipt_history — § 147 AO / GoBD immutable correction snapshots
--   eeg_verguetungssaetze     — EEG/KWKG tariff reference data
--   epex_monthly_prices       — monthly EPEX Spot reference (Marktprämie)
--   epex_spot_prices          — per-¼h/h day-ahead prices (§51 Negativpreisregel)
--   jahresmarktwert_preise    — technology-specific Jahresmarktwert (§20 Abs. 2 EEG)
--   eeg_regionalnachweise     — §53b Regionalnachweis periods (§79a)
--   eeg_stromsteuerbefreiungen — §53c per-kWh Stromsteuerbefreiung
--   eeg_sect54_solar_defekte  — §54 solar first-segment auction defects
--   jahresabrechnungen        — the year reconciled from the stored monthly receipts

-- ── Einspeiser (Anlagenbetreiber) ─────────────────────────────────────────────
--
-- The party behind the plants. `einspeiser_id` is operator-assigned (a customer
-- number, a MaStR Marktakteur-ID, or a UUID the ERP mints) — einsd does not
-- invent identities for parties it did not register.

CREATE TABLE einspeiser (
    einspeiser_id      TEXT        NOT NULL,
    tenant             TEXT        NOT NULL,
    name               TEXT        NOT NULL,
    -- MaStR Marktakteursnummer (`SEE…`/`ABR…`), where the operator has one.
    mastr_akteur_id    TEXT,

    -- § 19 UStG election. A declared property of the operator: plant size only
    -- *suggests* it (`VatStatus::default_for_plant`), it never decides it.
    ust_status         TEXT        NOT NULL DEFAULT 'REGELBESTEUERUNG'
                       CHECK (ust_status IN ('KLEINUNTERNEHMER', 'REGELBESTEUERUNG')),

    -- Payout account for every Gutschrift issued to this operator.
    bank_iban          TEXT,
    bank_bic           TEXT,
    zahlungsempfaenger TEXT,

    version            BIGINT      NOT NULL DEFAULT 1,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (einspeiser_id, tenant)
);

COMMENT ON TABLE einspeiser IS
    'The Anlagenbetreiber behind one or more EEG/KWKG plants. Not a contract: '
    '§ 7 Abs. 1 EEG 2023 forbids conditioning the EEG claim on one.';

CREATE INDEX einspeiser_mastr ON einspeiser (tenant, mastr_akteur_id)
    WHERE mastr_akteur_id IS NOT NULL;

-- ── EEG/KWKG plant register ───────────────────────────────────────────────────

CREATE TABLE eeg_anlagen (
    tr_id              TEXT        NOT NULL,
    tenant             TEXT        NOT NULL,
    malo_id            TEXT        NOT NULL,
    melo_id            TEXT,

    -- EEG law year (2000|2004|2009|2012|2017|2021|2023) or 0 for KWKG
    eeg_gesetz         SMALLINT    NOT NULL
                       CHECK (eeg_gesetz IN (0, 2000, 2004, 2009, 2012, 2017, 2021, 2023)),

    inbetriebnahme     DATE        NOT NULL,
    -- A capacity of zero or less is not a small plant: it drives the §9 band, the
    -- §52 Pflichtzahlung (10 €/kW — a negative one would *credit* the operator),
    -- the §44b quota and the §51 size test, and each produces nonsense from it.
    leistung_kwp       NUMERIC(12, 3) NOT NULL CHECK (leistung_kwp > 0),

    erzeugungsart      TEXT        NOT NULL CHECK (erzeugungsart IN (
        -- Solar PV — no generic 'SOLAR': the §48 rate depends on the Bauform,
        -- so a plant that does not state one cannot be priced.
        'SOLAR_AUFDACH',    -- roof-mounted (§48 Abs. 1 EEG 2023)
        'SOLAR_FREIFLAECHE', -- ground-mounted (tendering >1 MWp)
        'SOLAR_AGRIPV',     -- §51a Agri-PV (+0.5 ct/kWh premium)
        'SOLAR_MIETERSTROM',-- §21 Abs. 3 building community solar
        'SOLAR_STECKER',    -- Balkonkraftwerk <800 W (simplified)
        -- Wind
        'WIND_ONSHORE',     -- §21 EEG (tendering >750 kW)
        'WIND_OFFSHORE',    -- §§70ff EEG
        -- Biomass / Gas
        'BIOMASSE',         -- generic solid biomass
        'BIOMASSE_HOLZ',    -- wood biomass (§42a EEG restricted)
        'BIOGAS',           -- fermentation biogas
        'BIOMETHAN',        -- upgraded biogas (grid injection)
        'KLAEGAS',          -- sewage gas (§41 EEG)
        'GRUBENGAS',        -- mine gas (§41 EEG)
        'DEPONIEGAS',       -- landfill gas (§41 EEG)
        -- Hydro / Geo
        'WASSERKRAFT',      -- run-of-river hydro (§40 EEG)
        'GEOTHERMIE',       -- geothermal (§45 EEG)
        'GEZEITEN',         -- tidal / wave
        -- CHP
        'KWKG'              -- combined heat & power (KWKG)
    )),

    -- Feed-in tariff fixed at inbetriebnahme for the full Förderungsdauer (§25 EEG)
    verguetungssatz_ct NUMERIC(8, 4) NOT NULL CHECK (verguetungssatz_ct >= 0),
    foerderendedatum   DATE         NOT NULL,

    -- Which §48 rate column the plant is paid from. Überschusseinspeisung and
    -- Volleinspeisung differ by the §48 Abs. 2a bonus — 8,11 vs. 12,91 ct/kWh
    -- for a ≤10 kWp roof plant — so the rate lookup cannot answer without it.
    -- KWK_ZUSCHLAG is the KWKG column.
    verguetungsform    TEXT        NOT NULL DEFAULT 'UEBERSCHUSS' CHECK (verguetungsform IN (
        'UEBERSCHUSS', 'VOLLEINSPEISUNG', 'KWK_ZUSCHLAG'
    )),

    -- Settlement model. One vocabulary, no aliases: an alias set forces every
    -- gate in the service to list both spellings, and the ones that forgot
    -- (the KWKG kWh counter, the KWKG index) silently stopped working for
    -- half the plants.
    settlement_model   TEXT        NOT NULL DEFAULT 'VERGUETUNG' CHECK (settlement_model IN (
        'VERGUETUNG',                -- §21 Abs. 1 Einspeisevergütung
        'AUSFALLVERGUETUNG',         -- §21 Abs. 1 Nr. 2 Ausfallvergütung (−20 %)
        'DIREKTVERMARKTUNG',         -- §20 gleitende Marktprämie
        'AUSSCHREIBUNG',             -- §22 wettbewerblich ermittelte Marktprämie
        'SONSTIGE_DIREKTVERMARKTUNG',-- §21a Direktvermarktung ohne EEG-Zahlung
        'MIETERSTROM',               -- §21 Abs. 3 Mieterstromzuschlag
        'GGV',                       -- §42b EnWG gemeinschaftliche Gebäudeversorgung
        'EIGENVERBRAUCH',            -- keine Netzeinspeisung, keine Zahlung
        'POST_EEG_SPOT',             -- nach Förderende: Marktwert
        'KWKG_ZUSCHLAG',             -- §7 KWKG 2023 KWK-Zuschlag
        'FLEXIBILITAET',             -- §50b Flexibilitätsprämie (Bestandsanlagen)
        'FLEXIBILITAET_ZUSCHLAG'     -- §50a Flexibilitätszuschlag (Neuanlagen)
    )),

    direktvermarktung          BOOLEAN     NOT NULL DEFAULT false,
    -- §20 EEG: Anzulegender Wert in ct/kWh for Marktprämie
    direktverm_aw_ct           NUMERIC(8, 4),
    direktverm_mp_id           TEXT,
    -- §21 Abs. 3 EEG: Mieterstrom surcharge ct/kWh above base tariff
    mieter_zuschlag_ct         NUMERIC(6, 4),
    -- BNetzA Zuschlag-ID from tender result (Ausschreibungsanlagen)
    ausschreibungs_zuschlag_id TEXT,
    -- §22 EEG 2023 (wettbewerbliche Ermittlung): the awarded anzulegender Wert,
    -- distinct from a contracted
    -- Direktvermarktung value. Held separately so an award is never confused
    -- with a bilaterally agreed rate.
    zuschlagswert_ct           NUMERIC(8, 4),
    zuschlag_datum             DATE,
    -- §39n EEG 2023: Innovationsausschreibung pays a fixed market premium.
    ist_innovationsausschreibung BOOLEAN NOT NULL DEFAULT false,
    -- §22b EEG 2023: a Bürgerenergiegesellschaft (§3 Nr. 15) is exempt from
    -- needing a valid Zuschlag, so it settles at the statutory rate even in an
    -- auction-eligible size class.
    ist_buergerenergie         BOOLEAN NOT NULL DEFAULT false,

    -- Repowering (§3 Nr. 30 i.V.m. §25 EEG 2023): a Vollrepowering is a fresh
    -- Inbetriebnahme, so the Förderdauer restarts. §22 is the Ausschreibung
    -- provision and has nothing to do with this.
    ist_repowering             BOOLEAN     NOT NULL DEFAULT false,
    ursprungs_inbetriebnahme   DATE,       -- original commissioning before repowering
    repowering_datum           DATE,

    -- §24 EEG 2023 Zusammenlegung (merged plants)
    parent_tr_id               TEXT,

    -- KWKG: KWK-Zuschlag duration
    kwk_foerderdauer_h         INTEGER,    -- full-load hours (>2 MW: 30,000 h)
    kwk_foerderdauer_years     SMALLINT,   -- years (≤2 MW)
    kwk_strom_kwh_gesamt       NUMERIC(14, 3), -- accumulated kWh for hour-limit tracking

    -- §50 EEG: Flexibilitätsprämie (biomass demand response)
    flex_leistung_kw           NUMERIC(8, 3),
    flex_praemie_ct_kwh        NUMERIC(6, 4),

    -- Plant status. Only the three values the service actually writes:
    -- 'aktiv' on registration, 'abgemeldet' on Abmeldung/Zusammenlegung and
    -- 'foerderung_beendet' when the KWKG limit is reached. Repowering is
    -- recorded by `ist_repowering` + `repowering_datum` — the plant stays
    -- 'aktiv' and keeps settling, so there is no 'repowered' status.
    status                     TEXT        NOT NULL DEFAULT 'aktiv' CHECK (status IN (
                                    'aktiv', 'abgemeldet', 'foerderung_beendet'
                                )),
    notes                      TEXT,

    -- ── MaStR registration ───────────────────────────────────────────────────
    mastr_registriert          BOOLEAN     NOT NULL DEFAULT true,
    mastr_nummer               TEXT,
    mastr_datum                DATE,

    -- The operator. Payout account and § 19 UStG status live on `einspeiser`
    -- because both belong to the person, not the installation — see that
    -- table's comment. Nullable so a plant can be registered before its
    -- operator record exists; `settle` refuses to issue a Gutschrift without
    -- one rather than guessing the VAT.
    -- The Anlagenbetreiber. Mandatory: § 7 Abs. 1 EEG 2023 puts the payment on
    -- the Netzbetreiber, and a plant nobody can be paid for is not a plant this
    -- service can act on.
    einspeiser_id              TEXT        NOT NULL,

    -- ── Plant attributes ────────────────────────────────────
    -- 'Neubau' | 'Repowering' | 'Modernisierung'
    inbetriebnahme_typ         TEXT,
    -- §36h EEG 2023: Wind Standortgütegrad for Korrekturfaktor computation
    wind_guetegrad             NUMERIC(5, 3),
    wind_korrekturfaktor       NUMERIC(6, 5),
    -- §36h Abs. 2 EEG 2023: Standortgüte re-evaluations (year 6/11/16).
    -- JSONB Vec<{wirksam_ab_jahr: 6|11|16, guetefaktor}> — the effective
    -- Korrekturfaktor per billing period is derived (korrekturfaktor_fuer_periode).
    wind_guetefaktor_reevaluations JSONB NOT NULL DEFAULT '[]',
    -- §9 EEG — Steuerbarkeit. The obligation is staged by installed capacity:
    -- from 100 kW only Fernsteuerbarkeit satisfies it (Abs. 2 Nr. 1), the
    -- 25–100 kW band may take the 60-%-Leistungsbegrenzung instead (Nr. 2),
    -- below 25 kW the cap alone is enough (Nr. 3), and a Steckersolargerät under
    -- 2 kW is out of scope (Abs. 1 Satz 2). Recording only the Fernsteuerbarkeit
    -- date made every compliant plant on the 60 % route look like a §52 Abs. 1
    -- Nr. 1 violation at 10 €/kW/month.
    sect9_erfuellung           TEXT        NOT NULL DEFAULT 'KEINE' CHECK (sect9_erfuellung IN (
        'KEINE', 'FERNSTEUERBARKEIT', 'LEISTUNGSBEGRENZUNG_60'
    )),
    -- When the Fernsteuerbarkeit was installed, where that is the chosen route.
    fernsteuerbarkeit_datum    DATE,
    -- §24 Erweiterung: capacity blocks JSONB (Vec<CapacityBlock>)
    capacity_blocks            JSONB,

    -- ── Settlement lifecycle ─────────────────────────────────
    -- Active | Reduced | Suspended | PostEeg | Ended
    settlement_state           TEXT,

    -- ── §51b EEG 2023: biogas Ausschreibungsanlage flag ──────────────────────
    is_biogas_sect51b          BOOLEAN     NOT NULL DEFAULT false,

    -- ── Ausschreibung lifecycle ───────────────────────────────────────────────
    -- Erlöschen von Zuschlägen when the plant is not commissioned in time. The
    -- rule is technology-specific: §36e (Wind an Land), §37e (Solaranlagen des
    -- ersten Segments), §39e (Biomasseanlagen) EEG 2023. Distinct from §35a
    -- Entwertung von Zuschlägen, which is a BNetzA act rather than a deadline.
    -- The date the Zuschlag lapses when the plant is not commissioned in time.
    -- §36e (Wind an Land), §37e (Solar erstes Segment), §39e (Biomasse) EEG 2023.
    -- Distinct from §35a Entwertung, which is a BNetzA act rather than a deadline.
    --
    -- There is deliberately no `award_expired` flag beside it. There was one, and
    -- nothing ever set it — so the settlement branch that answers "the award has
    -- lapsed, nothing left to settle" was unreachable. The date is the fact; the
    -- expiry is derived from it against the billing period.
    zuschlag_erloeschen_datum  DATE,
    -- §52: cumulative violation start dates for Pflichtzahlung
    mastr_violation_start      DATE,
    fernsteuerbarkeit_violation_start DATE,
    -- §21b guard: date of last Veräußerungsform switch (monthly lock)
    last_veraeusserungsform_switch DATE,
    -- §51a: cumulative RAW negative-price quarter-hours (§51 lost intervals).
    -- The Förderende extension rounds this to whole days / the solar
    -- Volllastviertelstunden contingent ONCE over the 20-year total, so the
    -- effective Förderende is derived (via effektives_foerderende), not stored.
    negative_price_qh_gesamt BIGINT NOT NULL DEFAULT 0,


    -- ── §24 EEG 2023: facts that decide Zusammenfassung ──────────────────────
    -- Two plants are deemed one — changing the tariff band and the tender
    -- threshold for the whole Förderdauer — only when all of §24 Abs. 1 Satz 1
    -- holds and none of Sätze 2–5 carves them out. Ownership is deliberately
    -- absent: Satz 1 says "unabhängig von den Eigentumsverhältnissen".

    -- Satz 1 Nr. 1 — Grundstück, Gebäude or Betriebsgelände. NULL = unknown,
    -- which cannot establish a shared site.
    standort_id                TEXT,
    -- Sätze 3/4 — where a solar installation sits.
    solar_montage              TEXT CHECK (solar_montage IN (
        'AN_GEBAEUDE_ODER_LAERMSCHUTZWAND', 'FREIFLAECHE', 'SONSTIGE')),
    -- Satz 4 — building solar behind different points is not one plant.
    netzverknuepfungspunkt     TEXT,
    -- Satz 2 — biogas (not biomethane) from the same Biogaserzeugungsanlage is
    -- fused regardless of Satz 1.
    biogaserzeugungsanlage_id  TEXT,

    -- ── §44b EEG 2023: Biogas annual 45%-cap quota ───────────────────────────
    biogas_quota_kwh_ytd       NUMERIC(14, 3) NOT NULL DEFAULT 0,
    biogas_quota_ytd_year      SMALLINT,

    -- ── §51 EEG: facts the Negativpreisregel turns on ────────────────────────
    -- §51 Abs. 2 Nr. 1: the sub-100-kW exemption runs only "für Zeiträume vor
    -- dem Einbau eines intelligenten Messsystems". NULL = not yet rolled out.
    imesys_rollout_datum       DATE,
    -- §3 Nr. 37: Pilotwindenergieanlage an Land — carved out of §51 under every
    -- Fassung, at any size. A BNetzA/FGW certification fact about the turbine,
    -- so it is declared rather than derived.
    ist_pilotwindanlage        BOOLEAN     NOT NULL DEFAULT false,
    -- §100 EEG: the date the operator declared, in Textform to the NB, that
    -- §§51 and 51a shall apply to this Bestandsanlage. The declaration runs at
    -- the earliest from the end of the calendar year in which the plant is
    -- fitted with an iMSys, so the *effective* date is derived from this and
    -- `imesys_rollout_datum` rather than stored. From then on the plant forgoes
    -- payment during negative prices and its AW rises by 0,6 ct/kWh.
    sect51_optin_erklaert_am   DATE,

    -- When the 180-day Förderende alert was emitted; NULL until it is. The alert
    -- worker sweeps a 180-day window every six hours, so without this it emitted
    -- the same CloudEvent hundreds of times for every expiring plant.
    foerderung_alert_sent_at   TIMESTAMPTZ,


    -- ── §21c notification tracking ───────────────────────────────────────────
    veraeusserungsform_notification_sent_at TIMESTAMPTZ,

    -- ── §§42–44 EEG 2023: Biomass fuel composition ───────────────────────────
    biomasse_hauptbrennstoff   TEXT,
    biomasse_guelle_anteil     NUMERIC(5, 4),
    biomasse_energiepflanzen_anteil NUMERIC(5, 4),

    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (tr_id, tenant),

    -- A plant may be re-pointed at a different operator (sale, succession) but
    -- never at one this tenant does not know: the payout account and the VAT
    -- status are read through this key on every settlement.
    CONSTRAINT fk_anlage_einspeiser FOREIGN KEY (einspeiser_id, tenant)
        REFERENCES einspeiser (einspeiser_id, tenant) ON DELETE RESTRICT
);

CREATE INDEX eeg_anlagen_einspeiser ON eeg_anlagen (tenant, einspeiser_id);

COMMENT ON TABLE eeg_anlagen IS
    'Central EEG/KWKG plant register. Composite PK (tr_id, tenant) for multi-tenant isolation. '
    'foerderendedatum = inbetriebnahme + 20 years for EEG; reset on repowering.';

CREATE INDEX ea_malo_tenant      ON eeg_anlagen (malo_id, tenant);
CREATE INDEX ea_foerderend       ON eeg_anlagen (foerderendedatum, tenant) WHERE status = 'aktiv';
CREATE INDEX ea_settlement       ON eeg_anlagen (settlement_model, tenant) WHERE status = 'aktiv';
CREATE INDEX ea_gesetz_tenant    ON eeg_anlagen (eeg_gesetz, tenant);
CREATE INDEX ea_repowering       ON eeg_anlagen (tenant) WHERE ist_repowering = true;
CREATE INDEX ea_zusammenlegung   ON eeg_anlagen (parent_tr_id, tenant) WHERE parent_tr_id IS NOT NULL;
CREATE INDEX ea_kwkg             ON eeg_anlagen (tenant) WHERE settlement_model = 'KWKG_ZUSCHLAG';
CREATE INDEX ea_award_erloeschen ON eeg_anlagen (tenant, zuschlag_erloeschen_datum)
    WHERE zuschlag_erloeschen_datum IS NOT NULL;
CREATE INDEX ea_mastr_violation  ON eeg_anlagen (tenant, mastr_violation_start) WHERE mastr_violation_start IS NOT NULL;
CREATE INDEX ea_biogas_quota     ON eeg_anlagen (tenant, biogas_quota_ytd_year)
    WHERE erzeugungsart = 'BIOGAS' AND is_biogas_sect51b = false;
CREATE INDEX ea_notification_pending ON eeg_anlagen (tenant, last_veraeusserungsform_switch)
    WHERE veraeusserungsform_notification_sent_at IS NULL AND last_veraeusserungsform_switch IS NOT NULL;

-- ── Monthly settlement receipts ───────────────────────────────────────────────
-- § 147 Abs. 3 AO / GoBD: settlement receipts ground EEG payout bookings —
-- Buchungsbelege, 8-year retention. Written before any CloudEvent dispatch.
-- Correction receipts (is_correction = true) coexist freely with originals.

CREATE TABLE settlement_receipts (
    id                          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id                       TEXT        NOT NULL,
    tenant                      TEXT        NOT NULL,
    billing_year                SMALLINT    NOT NULL,
    billing_month               SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    settlement_model            TEXT        NOT NULL,
    einspeisemenge_kwh          NUMERIC(14, 3),
    settlement_eur              NUMERIC(14, 5),
    -- §26 Abs. 1 EEG: Fälligkeitsdatum = 15th of following month
    faelligkeitsdatum           DATE,
    -- §51a EEG: quarter-hours of Förderzeitraum extension accrued this period
    verlaengerungsanspruch_qh   BIGINT      NOT NULL DEFAULT 0,
    -- §25/§26 EEG: fraction of billing month with entitlement (mid-month events)
    billing_days_fraction       NUMERIC(8, 6),
    -- §52 EEG: separate Pflichtzahlung (penalty) amount
    pflichtzahlung_eur          NUMERIC(14, 5),
    -- § 147 AO / GoBD: itemized position snapshot for audit trail
    positions_json              JSONB,
    -- Full §14 UStG Gutschrift as a rubo4e::current::Rechnung JSONB (Gutschriftverfahren:
    -- the NB issues the settlement document to the Anlagenbetreiber). Populated for a
    -- billable settlement; NULL for NoData / PriceMissing / foerderung_beendet.
    rechnung_json               JSONB,
    -- Human-readable Gutschrift number (GS-EEG-<tr>-<year>-<month>) for lookup/idempotency.
    gutschrift_nummer           TEXT,
    bo4e_version                TEXT        NOT NULL DEFAULT '202607.1.0',
    -- 'calculated' | 'price_missing' | 'no_data' | 'error' | 'foerderung_beendet'
    status                      TEXT        NOT NULL DEFAULT 'calculated',
    -- Active | Reduced | Suspended | PostEeg | Ended (at time of settlement)
    settlement_state            TEXT,
    error_detail                TEXT,

    -- Correction chain support (§ 147 AO / GoBD)
    is_correction               BOOLEAN     NOT NULL DEFAULT false,
    correction_of               UUID,       -- FK to original receipt.id
    correction_reason           TEXT,

    settled_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE settlement_receipts IS
    '§ 147 Abs. 3 AO / GoBD: immutable settlement audit log (Buchungsbelege, 8-year retention). '
    'Correction receipts (is_correction=true) coexist with originals via partial unique index.';

-- Partial unique: exactly one non-correction receipt per plant × period.
-- Upserts must repeat the predicate — ON CONFLICT (cols) WHERE is_correction = false —
-- because Postgres cannot infer a partial index from the column list alone.
CREATE UNIQUE INDEX sr_unique_initial
    ON settlement_receipts (tr_id, tenant, billing_year, billing_month)
    WHERE is_correction = false;

COMMENT ON INDEX sr_unique_initial IS
    '§ 147 AO / GoBD: exactly one initial receipt per billing period per plant. '
    'Correction receipts excluded — they accumulate freely as an immutable audit chain. '
    'Named index supports ON CONFLICT ON CONSTRAINT sr_unique_initial in upsert code.';

CREATE INDEX sr_tr_period       ON settlement_receipts (tr_id, tenant, billing_year DESC, billing_month DESC);
CREATE INDEX sr_faelligkeitsdatum ON settlement_receipts (tenant, faelligkeitsdatum)
    WHERE faelligkeitsdatum IS NOT NULL;
CREATE INDEX sr_correction      ON settlement_receipts (correction_of)
    WHERE is_correction = true;

-- ── Per-period contribution to the cumulative counters ───────────────────────
--
-- The §44b Biogas quota, the §51a Förderende extension and the KWKG kWh limit
-- are running totals over the whole Förderdauer, but `POST /settle` is
-- idempotent and its receipt is an upsert. Without a record of what a period has
-- already contributed, re-running a month added its kWh and quarter-hours a
-- second time — burning quota, over-extending the Förderende and expiring the
-- KWKG limit early.
--
-- One row per plant × period holds the period's *absolute* contribution, so a
-- re-settle applies only the difference (which may be negative).

CREATE TABLE settlement_period_accruals (
    tr_id             TEXT           NOT NULL,
    tenant            TEXT           NOT NULL,
    billing_year      SMALLINT       NOT NULL,
    billing_month     SMALLINT       NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    -- §51a: raw negative-price quarter-hours claimed for this period.
    negative_price_qh BIGINT         NOT NULL DEFAULT 0,
    -- §44b: kWh charged against the annual Biogas quota.
    biogas_kwh        NUMERIC(14, 3) NOT NULL DEFAULT 0,
    -- KWKG: kWh charged against the Zuschlag limit.
    kwk_kwh           NUMERIC(14, 3) NOT NULL DEFAULT 0,
    updated_at        TIMESTAMPTZ    NOT NULL DEFAULT now(),

    PRIMARY KEY (tr_id, tenant, billing_year, billing_month)
);

COMMENT ON TABLE settlement_period_accruals IS
    'What each billing period contributed to the plant-level cumulative counters. '
    'Makes the §44b/§51a/KWKG accrual idempotent across re-settles of the same period.';

-- ── Settlement state audit log ────────────────────────────────────────────────

CREATE TABLE settlement_state_transitions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    from_state      TEXT        NOT NULL,
    to_state        TEXT        NOT NULL,
    effective_from  DATE        NOT NULL,
    reason          TEXT        NOT NULL,
    notes           TEXT,
    recorded_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_sst_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE
);

COMMENT ON TABLE settlement_state_transitions IS
    'Audit log of SettlementPeriodState changes. '
    'INSERT-only. Used for regulatory audit and §52 compliance reporting.';

CREATE INDEX sst_tr_id ON settlement_state_transitions (tr_id, tenant, effective_from DESC);

-- ── § 147 AO / GoBD: correction receipt snapshots ─────────────────────────────────

CREATE TABLE settlement_receipt_history (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    original_id     UUID        NOT NULL,   -- settlement_receipts.id before correction
    snapshot_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL,
    settlement_eur  NUMERIC(14, 5),
    status          TEXT,
    -- Full copy of the pre-correction receipt row for forensic audit
    settlement_data JSONB       NOT NULL
);

COMMENT ON TABLE settlement_receipt_history IS
    '§ 147 AO / GoBD: immutable snapshot taken before each correction upsert. '
    'settlement_data = full receipt JSONB for forensic tracing.';

CREATE INDEX srh_original_id ON settlement_receipt_history (original_id);
CREATE INDEX srh_tr_id       ON settlement_receipt_history (tr_id, tenant, billing_year, billing_month);

-- ── EEG/KWKG tariff reference table ──────────────────────────────────────────
-- Lookup table for statutory feed-in tariff / premium rates.
-- verguetungsform: UEBERSCHUSS | VOLLEINSPEISUNG | KWK_ZUSCHLAG
-- Quarterly degression (§23a EEG) is applied by the eeg-billing degression module.
-- Import updated rates via PUT /api/v1/verguetungssaetze or xtask import.

CREATE TABLE eeg_verguetungssaetze (
    id                  SERIAL      PRIMARY KEY,
    erzeugungsart       TEXT        NOT NULL,
    leistung_min_kwp    NUMERIC(10, 3) NOT NULL,
    leistung_max_kwp    NUMERIC(10, 3),         -- NULL = no upper bound
    verguetungsform     TEXT        NOT NULL DEFAULT 'UEBERSCHUSS' CHECK (verguetungsform IN (
        'UEBERSCHUSS', 'VOLLEINSPEISUNG', 'KWK_ZUSCHLAG'
    )),
    billing_start       DATE        NOT NULL,
    billing_end         DATE,                   -- NULL = currently valid
    verguetungssatz_ct  NUMERIC(8, 4) NOT NULL,
    eeg_gesetz          SMALLINT    NOT NULL,
    notes               TEXT,
    -- verguetungsform is part of the key. Without it the §48 Abs. 2a
    -- Volleinspeisung rates collide with the Überschuss rates of the same band
    -- and start date, and the seed's ON CONFLICT DO NOTHING dropped every one of
    -- them — leaving the Volleinspeisung tariff simply absent from the table.
    UNIQUE (erzeugungsart, verguetungsform, leistung_min_kwp, billing_start),
    CONSTRAINT evs_band_forward CHECK (leistung_max_kwp IS NULL OR leistung_max_kwp > leistung_min_kwp),
    CONSTRAINT evs_period_forward CHECK (billing_end IS NULL OR billing_end >= billing_start)
);

COMMENT ON TABLE eeg_verguetungssaetze IS
    'EEG/KWKG statutory tariff reference. '
    'verguetungssatz_ct = NET rate for EEG (§53 deduction already applied where applicable). '
    'Quarterly degression computed by eeg-billing degression module for post-billing_start quarters.';

CREATE INDEX evs_lookup ON eeg_verguetungssaetze
    (erzeugungsart, verguetungsform, billing_start, billing_end);

-- Seed: EEG 2023 + Solarpaket I rates (BGBl. I Nr. 107, 16.05.2024)
-- Operators MUST verify against current BNetzA publications before production use.
INSERT INTO eeg_verguetungssaetze (erzeugungsart, leistung_min_kwp, leistung_max_kwp, verguetungsform, verguetungssatz_ct, billing_start, billing_end, eeg_gesetz, notes) VALUES
-- Solar Aufdach / SOLAR — Überschusseinspeisung (§48 Abs. 1 EEG 2023, Solarpaket I)
('SOLAR_AUFDACH',   0,   10, 'UEBERSCHUSS',   8.11, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1a, ≤10 kWp'),
('SOLAR_AUFDACH',  10,   40, 'UEBERSCHUSS',   7.03, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1b, >10–40 kWp'),
('SOLAR_AUFDACH',  40,  100, 'UEBERSCHUSS',   5.74, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1c, >40–100 kWp'),
('SOLAR_AUFDACH', 100,  400, 'UEBERSCHUSS',   4.30, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1d, >100–400 kWp'),
('SOLAR_AUFDACH', 400, 1000, 'UEBERSCHUSS',   3.14, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1e, >400 kWp–1 MWp'),
-- Solar Aufdach — Volleinspeisung (§48 Abs. 2a EEG 2023, +4.8 ct/kWh bonus)
('SOLAR_AUFDACH',   0,   10, 'VOLLEINSPEISUNG', 12.91, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 2a, ≤10 kWp'),
('SOLAR_AUFDACH',  10,   40, 'VOLLEINSPEISUNG', 10.83, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 2a, >10–40 kWp'),
('SOLAR_AUFDACH',  40,  100, 'VOLLEINSPEISUNG',  9.54, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 2a, >40–100 kWp'),
-- Pre-Solarpaket I rates (EEG 2023 initial, 2023-02–2024-04)
('SOLAR_AUFDACH',   0,  10, 'UEBERSCHUSS', 7.71, '2023-02-01', '2024-04-30', 2023, 'EEG 2023 initial §48 Abs. 1 Nr. 1a'),
('SOLAR_AUFDACH',  10,  40, 'UEBERSCHUSS', 6.69, '2023-02-01', '2024-04-30', 2023, 'EEG 2023 initial §48 Abs. 1 Nr. 1b'),
('SOLAR_AUFDACH',  40, 100, 'UEBERSCHUSS', 5.76, '2023-02-01', '2024-04-30', 2023, 'EEG 2023 initial §48 Abs. 1 Nr. 1c'),
-- EEG 2021 solar
('SOLAR_AUFDACH',   0,  10, 'UEBERSCHUSS', 9.58, '2021-01-01', '2021-12-31', 2021, 'EEG 2021 ≤10 kWp'),
('SOLAR_AUFDACH',  10,  40, 'UEBERSCHUSS', 9.33, '2021-01-01', '2021-12-31', 2021, 'EEG 2021 10–40 kWp'),
-- EEG 2017 solar
('SOLAR_AUFDACH',   0,  10, 'UEBERSCHUSS', 9.87, '2017-04-01', '2020-12-31', 2017, 'EEG 2017 ≤10 kWp avg'),
-- Wind onshore has NO statutory anzulegender Wert. § 22 Abs. 2 Satz 1 EEG 2023:
-- the claim exists "nur, solange und soweit ein von der Bundesnetzagentur
-- erteilter Zuschlag für die Anlage wirksam ist", and § 36h Abs. 1 then derives
-- the value from that Zuschlagswert times the Gütefaktor-Korrekturfaktor. A row
-- here read "EEG 2023 §21 Onshore ≤750 kW, 7.35 ct" — § 21 is Einspeisevergütung
-- und Mieterstromzuschlag, and 750 kW is the *solar* second-segment threshold
-- from § 22 Abs. 3. Awarded values are imported per plant, not seeded.
-- KWKG 2023 (§7 Abs. 1 KWKG 2023)
('KWKG',    0,   50, 'KWK_ZUSCHLAG', 8.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 1, ≤50 kW_el'),
('KWKG',   50,  100, 'KWK_ZUSCHLAG', 6.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 2, >50–100 kW_el'),
('KWKG',  100,  250, 'KWK_ZUSCHLAG', 5.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 3, >100–250 kW_el'),
('KWKG',  250, 2000, 'KWK_ZUSCHLAG', 4.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 4, >250 kW–2 MW_el'),
('KWKG', 2000, NULL, 'KWK_ZUSCHLAG', 3.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 5, >2 MW_el'),
-- Biomasse — § 42 Satz 1 EEG 2023 gives ONE statutory tier (≤150 kW); above it
-- the value is set by tender (§ 22 Abs. 4). §§ 43/44 are separate, higher claims
-- for plants that qualify, not tiers of § 42.
('BIOMASSE',   0, 150, 'UEBERSCHUSS', 12.67, '2023-01-01', NULL, 2023, 'EEG 2023 §42 Satz 1, ≤150 kW Bemessungsleistung'),
('BIOGAS',     0, 500, 'UEBERSCHUSS', 14.16, '2023-01-01', NULL, 2023, 'EEG 2023 §43 Abs. 1 Nr. 1 Bioabfallvergärung, ≤500 kW'),
('BIOGAS',   500, 20000, 'UEBERSCHUSS', 12.41, '2023-01-01', NULL, 2023, 'EEG 2023 §43 Abs. 1 Nr. 2 Bioabfallvergärung, ≤20 MW'),
-- Deponie-, Klär- und Grubengas — § 41 EEG 2023 gives each gas its own ladder.
-- These were seeded as a flat 12.50 / 12.50 / 9.10, which paid Klärgas more than
-- twice its statutory value.
('DEPONIEGAS',    0,  500, 'UEBERSCHUSS', 7.46, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Abs. 1 Nr. 1, ≤500 kW'),
('DEPONIEGAS',  500, 5000, 'UEBERSCHUSS', 5.17, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Abs. 1 Nr. 2, ≤5 MW'),
('KLAEGAS',       0,  500, 'UEBERSCHUSS', 5.93, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Abs. 2 Nr. 1, ≤500 kW'),
('KLAEGAS',     500, 5000, 'UEBERSCHUSS', 5.17, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Abs. 2 Nr. 2, ≤5 MW'),
('GRUBENGAS',     0, 1000, 'UEBERSCHUSS', 5.98, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Abs. 3 Nr. 1, ≤1 MW'),
('GRUBENGAS',  1000, 5000, 'UEBERSCHUSS', 3.81, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Abs. 3 Nr. 2, ≤5 MW'),
('GRUBENGAS',  5000, NULL, 'UEBERSCHUSS', 3.37, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Abs. 3 Nr. 3, >5 MW'),
-- Geothermie — § 45 Abs. 1 EEG 2023, flat. Absent from the seed entirely before.
('GEOTHERMIE',    0, NULL, 'UEBERSCHUSS', 25.20, '2023-01-01', NULL, 2023, 'EEG 2023 §45 Abs. 1, flat'),
-- Wasserkraft — § 40 Abs. 1 EEG 2023, seven tiers by Bemessungsleistung. The
-- seed had three (12.48 / 8.59 / 7.56), none of which is a statutory value.
('WASSERKRAFT',     0,   500, 'UEBERSCHUSS', 12.03, '2023-01-01', NULL, 2023, 'EEG 2023 §40 Abs. 1 Nr. 1, ≤500 kW'),
('WASSERKRAFT',   500,  2000, 'UEBERSCHUSS',  7.93, '2023-01-01', NULL, 2023, 'EEG 2023 §40 Abs. 1 Nr. 2, ≤2 MW'),
('WASSERKRAFT',  2000,  5000, 'UEBERSCHUSS',  6.07, '2023-01-01', NULL, 2023, 'EEG 2023 §40 Abs. 1 Nr. 3, ≤5 MW'),
('WASSERKRAFT',  5000, 10000, 'UEBERSCHUSS',  5.32, '2023-01-01', NULL, 2023, 'EEG 2023 §40 Abs. 1 Nr. 4, ≤10 MW'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS',  5.13, '2023-01-01', NULL, 2023, 'EEG 2023 §40 Abs. 1 Nr. 5, ≤20 MW'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS',  4.12, '2023-01-01', NULL, 2023, 'EEG 2023 §40 Abs. 1 Nr. 6, ≤50 MW'),
('WASSERKRAFT', 50000,  NULL, 'UEBERSCHUSS',  3.37, '2023-01-01', NULL, 2023, 'EEG 2023 §40 Abs. 1 Nr. 7, >50 MW');
-- No ON CONFLICT clause on purpose: it would swallow key collisions, and the §48
-- Abs. 2a Volleinspeisung block collides with the Überschuss rows on
-- (erzeugungsart, leistung_min_kwp, billing_start). A migration that reports
-- success must not leave those rates missing.

-- ── EPEX Spot monthly reference prices ───────────────────────────────────────
-- Required for DIREKTVERMARKTUNG (Marktprämie) and POST_EEG_SPOT settlement.

CREATE TABLE epex_monthly_prices (
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    avg_ct_kwh      NUMERIC(8, 4) NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'manual',
    imported_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (billing_year, billing_month)
);

COMMENT ON TABLE epex_monthly_prices IS
    'Generic monthly EPEX Spot reference. '
    'Used when no technology-specific Jahresmarktwert is available. '
    'Import via PUT /api/v1/epex-monthly/{year}/{month}.';

-- ── EPEX Spot per-interval prices (§51 Negativpreisregel) ────────────────────
-- Quarter-hour (or hourly) day-ahead spot prices. einsd overlays a plant's ¼h
-- feed-in Lastgang (from edmd) against these to derive kwh_during_negative_epex
-- (§51 reduction) and negative_price_quarter_hours (§51a extension) — the
-- version-aware consecutive-hour run logic lives in eeg-billing::negativpreis.
CREATE TABLE epex_spot_prices (
    delivery_start   TIMESTAMPTZ  NOT NULL,      -- interval start, UTC
    resolution_min   SMALLINT     NOT NULL DEFAULT 15 CHECK (resolution_min IN (15, 60)),
    price_ct_kwh     NUMERIC(9, 4) NOT NULL,     -- may be negative
    source           TEXT         NOT NULL DEFAULT 'manual',
    imported_at      TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (delivery_start)
);

-- Range scans over a billing month are the only query pattern.
CREATE INDEX esp_delivery ON epex_spot_prices (delivery_start);

COMMENT ON TABLE epex_spot_prices IS
    'EPEX day-ahead spot prices per ¼h/h interval for §51 Negativpreisregel. '
    'Import via PUT /api/v1/epex-spot (bulk). Negative price → AW reduced to null.';

-- ── §20 Abs. 2 + Anlage 1 EEG 2023: technology-specific Jahresmarktwert ──────

CREATE TABLE jahresmarktwert_preise (
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    -- Matches erzeugungsart values; 'DEFAULT' = generic fallback
    erzeugungsart   TEXT        NOT NULL,
    avg_ct_kwh      NUMERIC(8, 4) NOT NULL
                    CHECK (avg_ct_kwh BETWEEN -100 AND 1000),
    source          TEXT        NOT NULL DEFAULT 'manual',
    imported_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (billing_year, billing_month, erzeugungsart)
);

COMMENT ON TABLE jahresmarktwert_preise IS
    '§20 Abs. 2 + Anlage 1 EEG 2023: technology-specific monthly Marktwert. '
    'Published by ÜNB (netztransparenz.de). '
    'Lookup order: exact erzeugungsart → DEFAULT → epex_monthly_prices.';

CREATE INDEX jmw_period    ON jahresmarktwert_preise (billing_year DESC, billing_month DESC);
CREATE INDEX jmw_art_period ON jahresmarktwert_preise (erzeugungsart, billing_year DESC, billing_month DESC);

-- ── §§53b–54 EEG 2023: reductions of the anzulegender Wert ───────────────────
--
-- Each of these cuts the AW itself, before the settlement formula. That matters
-- for the gleitende Marktprämie, which floors at zero: a euro deduction taken
-- after the floor would drive the settlement negative.
--
-- The amounts are fixed by statute and therefore NOT stored — only the facts
-- that trigger them are. Storing a rate would let a data-entry error produce a
-- deduction the law does not provide for.

-- §53b: "Der anzulegende Wert für Strom, für den dem Anlagenbetreiber ein
-- Regionalnachweis ausgestellt worden ist, verringert sich bei Anlagen, deren
-- anzulegender Wert gesetzlich bestimmt ist, um 0,1 Cent pro Kilowattstunde."
--
-- Per plant and period — a Regionalnachweis is issued to the Anlagenbetreiber
-- for specific electricity (§79a EEG, Herkunfts- und Regionalnachweisregister
-- des Umweltbundesamtes), not to a grid area.

CREATE TABLE eeg_regionalnachweise (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    -- Register reference of the issued Regionalnachweis (§79a EEG).
    nachweis_ref    TEXT        NOT NULL,
    effective_from  DATE        NOT NULL,
    effective_until DATE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_rn_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE,
    CONSTRAINT rn_period_forward CHECK (effective_until IS NULL OR effective_until >= effective_from)
);

COMMENT ON TABLE eeg_regionalnachweise IS
    '§53b EEG 2023: periods for which a Regionalnachweis (§79a) was issued. '
    'The 0,1 ct/kWh deduction is statutory and is applied by the settlement '
    'engine — deliberately not stored here.';

CREATE INDEX rn_tr_id ON eeg_regionalnachweise (tr_id, tenant, effective_from DESC);

-- §53c: "Der anzulegende Wert verringert sich für Strom, der durch ein Netz
-- durchgeleitet wird und der von der Stromsteuer nach dem Stromsteuergesetz
-- befreit ist, um die Höhe der pro Kilowattstunde gewährten Stromsteuerbefreiung."
--
-- The amount IS stored here, because the statute ties it to the exemption
-- actually granted rather than to a fixed rate. It is capped at the full §3
-- StromStG rate (20,50 EUR/MWh = 2,05 ct/kWh) by the settlement engine.

CREATE TABLE eeg_stromsteuerbefreiungen (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    befreiung_ct_kwh NUMERIC(8, 5) NOT NULL,
    -- Which StromStG provision the exemption rests on, e.g. '§9 Abs. 1 Nr. 1'.
    rechtsgrundlage TEXT        NOT NULL,
    effective_from  DATE        NOT NULL,
    effective_until DATE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_sst_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE,
    CONSTRAINT sst_positive CHECK (befreiung_ct_kwh > 0),
    CONSTRAINT sst_max_vollsatz CHECK (befreiung_ct_kwh <= 2.05),
    CONSTRAINT sst_period_forward CHECK (effective_until IS NULL OR effective_until >= effective_from)
);

COMMENT ON TABLE eeg_stromsteuerbefreiungen IS
    '§53c EEG 2023: per-kWh Stromsteuerbefreiung granted for grid-transited '
    'electricity. Capped at the §3 StromStG full rate of 2,05 ct/kWh.';

CREATE INDEX stromsteuer_tr_id ON eeg_stromsteuerbefreiungen (tr_id, tenant, effective_from DESC);

-- §54: Verringerung des Zahlungsanspruchs bei Ausschreibungen für Solaranlagen
-- des ersten Segments. Four independent statutory defects; Abs. 1 and Abs. 2
-- stack, Abs. 4 zeroes the AW outright.
--
-- Abs. 3 Satz 2/3: the deduction lapses for the future once the missing proof
-- is supplied, and retroactively for the periods it covers — so a late Nachweis
-- is recorded by closing the period, not by deleting the row.

CREATE TABLE eeg_sect54_solar_defekte (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    -- Abs. 1 — Zahlungsberechtigung applied for after the 18th calendar month
    -- following public announcement of the Zuschlag. −0,3 ct/kWh.
    zahlungsberechtigung_nach_18_monaten BOOLEAN NOT NULL DEFAULT FALSE,
    -- Abs. 2 — location does not match the Flurstücke named in the bid. −0,3 ct/kWh.
    flurstueck_abweichung                BOOLEAN NOT NULL DEFAULT FALSE,
    -- Abs. 3 — Nachweis of simultaneous crop cultivation / agricultural use
    -- (§37 Abs. 1 Nr. 3, §85c Abs. 1 Satz 4) not supplied. −2,5 ct/kWh.
    agri_nutzungsnachweis_fehlt          BOOLEAN NOT NULL DEFAULT FALSE,
    -- Abs. 4 — Landesverordnung under §37c Abs. 2 not met. AW → 0.
    landesverordnung_nicht_erfuellt      BOOLEAN NOT NULL DEFAULT FALSE,
    bnetza_ref      TEXT,
    notes           TEXT,
    effective_from  DATE        NOT NULL,
    effective_until DATE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_s54_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE,
    CONSTRAINT s54_period_forward CHECK (effective_until IS NULL OR effective_until >= effective_from),
    -- A row that records no defect deducts nothing; it is a data-entry error.
    CONSTRAINT s54_at_least_one_defect CHECK (
        zahlungsberechtigung_nach_18_monaten
        OR flurstueck_abweichung
        OR agri_nutzungsnachweis_fehlt
        OR landesverordnung_nicht_erfuellt
    )
);

COMMENT ON TABLE eeg_sect54_solar_defekte IS
    '§54 EEG 2023 Ausschreibungen für Solaranlagen des ersten Segments: the four '
    'statutory defects per plant and period. Deduction amounts are statutory and '
    'applied by the settlement engine.';

CREATE INDEX s54_tr_id ON eeg_sect54_solar_defekte (tr_id, tenant, effective_from DESC);

-- ── Jahresabrechnung (§25 EEG 2023 / §14 UStG annual reconciliation) ──────────
--
-- The monthly settlements are the payment obligation; this is the annual
-- statement that reconciles them. It is derived from settlement_receipts rather
-- than recomputed, so it can never disagree with what was actually paid, and it
-- records which months are missing rather than quietly summing eleven.
CREATE TABLE jahresabrechnungen (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id                   TEXT        NOT NULL,
    tenant                  TEXT        NOT NULL,
    billing_year            SMALLINT    NOT NULL,
    -- Sum over the year's receipts.
    einspeisemenge_kwh      NUMERIC(18, 3) NOT NULL DEFAULT 0,
    -- Same scale as settlement_receipts.settlement_eur. At NUMERIC(14,2) the
    -- annual total silently rounded each month's 5-decimal amount and no longer
    -- equalled the sum of the receipts it claims to reconcile.
    settlement_eur          NUMERIC(16, 5) NOT NULL DEFAULT 0,
    -- §52 EEG 2023 Pflichtzahlungen, kept apart from the Vergütung: they are a
    -- separate claim and are never netted into the settlement total.
    pflichtzahlung_eur      NUMERIC(16, 5) NOT NULL DEFAULT 0,
    -- Months with a receipt, and the ones without.
    months_settled          SMALLINT    NOT NULL DEFAULT 0,
    missing_months          SMALLINT[]  NOT NULL DEFAULT '{}',
    -- §51a: quarter-hours accrued across the year toward the Vergütungszeitraum.
    verlaengerungsanspruch_qh BIGINT    NOT NULL DEFAULT 0,
    -- Number of corrections issued in the year (§ 147 AO / GoBD audit signal).
    correction_count        SMALLINT    NOT NULL DEFAULT 0,
    status                  TEXT        NOT NULL DEFAULT 'vorlaeufig'
                            CHECK (status IN ('vorlaeufig', 'endgueltig')),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_ja_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE
);

-- One statement per plant and year; re-running replaces it.
CREATE UNIQUE INDEX ja_unique ON jahresabrechnungen (tr_id, tenant, billing_year);
CREATE INDEX ja_year ON jahresabrechnungen (tenant, billing_year);
