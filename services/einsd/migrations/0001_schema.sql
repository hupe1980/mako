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
--   marktwert_preise          — Monats- und Jahresmarktwert (Anlage 1 Nr. 2–4 EEG)
--   eeg_regionalnachweise     — §53b Regionalnachweis periods (§79a)
--   eeg_stromsteuerbefreiungen — §53c per-kWh Stromsteuerbefreiung
--   eeg_sect54_solar_defekte  — §54 solar first-segment auction defects
--   jahresabrechnungen        — the year reconciled from the stored monthly receipts

-- ── Einspeiser (Anlagenbetreiber) ─────────────────────────────────────────────
--
-- The party behind the plants. `einspeiser_id` is operator-assigned (a customer
-- number, a MaStR Marktakteur-ID, or a UUID the ERP mints) — einsd does not
-- invent identities for parties it did not register.

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
        'BIOMASSE_HOLZ',    -- wood biomass (§§ 42/43 EEG; no separate Holzbiomasse restriction)
        'BIOGAS',           -- fermentation biogas
        'BIOMETHAN',        -- upgraded biogas (grid injection)
        'KLAERGAS',          -- sewage gas (§41 EEG)
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
    -- The EEG Förderende. NULL where the plant has none: § 8 KWKG measures the
    -- Zuschlag in Vollbenutzungsstunden (Abs. 1-3) and caps each calendar year
    -- separately (Abs. 4), so a KWK plant is ended by kwk_foerderdauer_h and
    -- never by a date.
    foerderendedatum   DATE,

    -- Which §48 rate column the plant is paid from. Überschusseinspeisung and
    -- Volleinspeisung differ by the §48 Abs. 2a bonus — 8,11 vs. 12,87 ct/kWh
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
        'AUSFALLVERGUETUNG',         -- §21 Abs. 1 Satz 1 Nr. 3 Ausfallvergütung (−20 %)
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

    -- KWKG §§7, 8: KWK-Zuschlag rate inputs and Förderdauer
    -- §8 measures the Förderdauer of every plant in Vollbenutzungsstunden and
    -- caps each calendar year separately (Abs. 4); it has no capacity band and
    -- no year-based duration, so there is no years column.
    kwk_anlagenart             TEXT CHECK (kwk_anlagenart IN (
        'NEU', 'MODERNISIERT', 'NACHGERUESTET')),
    kwk_verwendung             TEXT CHECK (kwk_verwendung IN (
        'NETZ_DER_ALLGEMEINEN_VERSORGUNG', 'NICHT_EINGESPEIST_BIS100KW',
        'NICHT_EINGESPEIST_KUNDENANLAGE', 'NICHT_EINGESPEIST_STROMKOSTENINTENSIV',
        'NICHT_EINGESPEIST_BRANCHE_ANLAGE2')),
    kwk_kostenanteil           NUMERIC(5, 4),  -- §8 Abs. 2/3 share of Neuerrichtungskosten
    -- §7 Abs. 1 Satz 2: the 0,5 ct uplift on Nr. 5 lit. a is payable only
    -- "soweit das Bundesministerium ... dies im Bundesanzeiger veroeffentlicht
    -- hat". FALSE until an operator records that it did.
    kwk_bmwk_feststellung      BOOLEAN     NOT NULL DEFAULT FALSE,
    kwk_foerderdauer_h         INTEGER,        -- §8 Abs. 1–3 Vollbenutzungsstunden
    kwk_strom_kwh_gesamt       NUMERIC(14, 3), -- lifetime kWh paid (§8 Abs. 1–3)
    kwk_kwh_jahr               NUMERIC(14, 3), -- kWh paid in kwk_kwh_jahr_year (§8 Abs. 4)
    kwk_kwh_jahr_year          SMALLINT,

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

    -- The Anlagenbetreiber. Payout account and § 19 UStG status live on
    -- `einspeiser` because both belong to the person, not the installation —
    -- see that table's comment.
    --
    -- Mandatory: § 7 Abs. 1 EEG 2023 puts the payment on the Netzbetreiber, and
    -- a plant nobody can be paid for is not a plant this service can act on. The
    -- operator is registered first; `fk_anlage_einspeiser` below enforces it.
    einspeiser_id              TEXT        NOT NULL,

    -- ── Plant attributes ────────────────────────────────────
    -- Parsed by `InbetriebnahmeTyp::from_db_str` (eeg-billing), which accepts
    -- exactly: ERSTINBETRIEBNAHME | WIEDERINBETRIEBNAHME | MODERNISIERUNG |
    -- REPOWERING | ZUSAMMENLEGUNG | ERWEITERUNG. Anything else is rejected,
    -- so the spelling here is load-bearing rather than descriptive.
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
    -- §52 Abs. 1 Nr. 11 — when the MaStR breach began. This is the one §52 clock
    -- einsd owns end to end: registration without a MaStR number sets it,
    -- confirmation clears it. Every other Pflichtverstoß is recorded in
    -- `eeg_pflichtverstoesse` (see below), because its start is a fact only the
    -- operator, the Direktvermarkter or a site visit establishes.
    mastr_violation_start      DATE,
    -- § 19 Abs. 3b / 3c EEG 2023 — the Speicher-Abgrenzungs- bzw. Pauschaloption,
    -- i.e. how the förderfähige Anteil of electricity from a Stromspeicher is
    -- determined. Two consequences, both money:
    --
    --   * Anlage 1 Nr. 2 Satz 3 moves a plant claiming under either option onto
    --     the **Jahres**marktwert even when its vintage would give it the
    --     Monatsmarktwert.
    --   * § 20 Satz 2 requires the whole feed-in at that Einspeisestelle to sit
    --     in a separate Bilanz- oder Unterbilanzkreis.
    --
    -- 'KEINE' is the ordinary case: no storage claim is made.
    speicher_option            TEXT        NOT NULL DEFAULT 'KEINE'
                               CHECK (speicher_option IN ('KEINE', 'ABGRENZUNG', 'PAUSCHAL')),
    -- §21b Abs. 1 Satz 2: the effective date of the last Veräußerungsform switch
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
    biomasse_getreide_mais_anteil NUMERIC(5, 4),  -- §39i Abs. 1

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
    'foerderendedatum = inbetriebnahme + 20 years for EEG (reset on repowering), '
    'NULL for a KWKG plant, whose Zuschlag ends on Vollbenutzungsstunden (§ 8 KWKG).';

CREATE INDEX ea_malo_tenant      ON eeg_anlagen (malo_id, tenant);
CREATE INDEX ea_foerderend       ON eeg_anlagen (foerderendedatum, tenant) WHERE status = 'aktiv';
CREATE INDEX ea_settlement       ON eeg_anlagen (settlement_model, tenant) WHERE status = 'aktiv';
CREATE INDEX ea_gesetz_tenant    ON eeg_anlagen (eeg_gesetz, tenant);
CREATE INDEX ea_repowering       ON eeg_anlagen (tenant) WHERE ist_repowering = true;
CREATE INDEX ea_zusammenlegung   ON eeg_anlagen (parent_tr_id, tenant) WHERE parent_tr_id IS NOT NULL;

-- ── §52 Abs. 1 EEG 2023 — the Pflichtverstoß register ────────────────────────
--
-- §52 Abs. 1 lists thirteen breaches (Nr. 1–12 with a Nr. 9a between 9 and 10).
-- einsd *derives* four of them from the plant record — Nr. 1 (§9 Steuerbarkeit),
-- Nr. 5 (Ausfallvergütung Höchstdauer, from the receipts), Nr. 9 (§21c
-- notification) and Nr. 11 (MaStR). The other nine turn on facts no register row
-- carries: whether the Direktvermarkter can actually curtail the plant (Nr. 4,
-- §10b Abs. 5 leaves the Nachweis to the two parties), whether a Speicher meets
-- §9 Abs. 5 (Nr. 2), whether the Ist-Einspeisung is measured in
-- viertelstündlicher Auflösung (Nr. 8), whether the Strom was doppelt vermarktet
-- (Nr. 12) — each is a finding, and this table is where a finding is filed.
--
-- It also supplies what the derived four cannot state on their own:
--
--   * `beginn`      — §52 Abs. 2 charges „pro Kalendermonat, in dem ganz oder
--                     zeitweise ein Pflichtverstoß vorliegt", so the month count
--                     runs from here. Without it the charge starts at one month,
--                     which understates a breach that has run for a year.
--   * `behoben_am`  — §52 Abs. 3 Satz 1 Nr. 1 reduces the charge to 2 €/kW
--                     „sobald die entsprechende Pflicht erfüllt wird; diese
--                     Verringerung wirkt **zurück bis zum Beginn**". Months
--                     already settled at 10 € therefore need a § 147 AO
--                     correction, which is what `settlement_receipts.correction_of`
--                     is for.
--   * `technischer_defekt` — §52 Abs. 3 Satz 2 waives the defect month and the
--                     following one for Nr. 1/3/4/8, for breaches after
--                     31.12.2023. The Darlegungs- und Beweislast is the
--                     operator's, so this is recorded rather than inferred.
--
-- One breach may recur, so the history is kept and only the *open* entry per
-- (plant, Nummer) is unique.
CREATE TABLE eeg_pflichtverstoesse (
    id                 UUID        PRIMARY KEY,
    tenant             TEXT        NOT NULL,
    tr_id              TEXT        NOT NULL,
    -- `eeg_billing::SanktionsTyp::as_db_str` — held against the enum by
    -- `tests/schema_code_guard.rs`, so a fourteenth breach cannot land in the
    -- code without landing here.
    typ                TEXT        NOT NULL CHECK (typ IN (
                           'FERNSTEUERBARKEIT_FEHLEND',
                           'SPEICHER_ANFORDERUNG_NICHT_ERFUELLT',
                           'I_MSS_ANFORDERUNG_NICHT_ERFUELLT',
                           'SECT10B_VORGABEN_VERLETZT',
                           'AUSFALLVERGUETUNG_HOECHSTDAUER_UEBERSCHRITTEN',
                           'EINSPEISEVERGUETUNG_UNZULAESSIGE_NUTZUNG',
                           'VERAEUSSERUNGSFORM_WECHSEL_UNGUELTIG',
                           'VERAEUSSERUNGSFORM_NACHWEISPFLICHT_VERLETZT',
                           'ZUORDNUNGS_WECHSEL_NICHT_GEMELDET',
                           'INBETRIEBNAHME_VORGABE_VERLETZT',
                           'VOLLEINSPEISUNGSPFLICHT_VERLETZT',
                           'MASTR_NICHT_REGISTRIERT',
                           'DOPPELVERMARKTUNGSVERBOT_VERLETZT')),
    beginn             DATE        NOT NULL,
    behoben_am         DATE,
    technischer_defekt BOOLEAN     NOT NULL DEFAULT false,
    -- What was found, and by whom. A Pflichtzahlung is a claim against the
    -- operator; „because the ERP said so" is not an audit trail.
    notiz              TEXT,
    erfasst_am         TIMESTAMPTZ NOT NULL DEFAULT now(),
    aktualisiert_am    TIMESTAMPTZ NOT NULL DEFAULT now(),

    FOREIGN KEY (tr_id, tenant) REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE,
    CONSTRAINT ep_behoben_nach_beginn CHECK (behoben_am IS NULL OR behoben_am >= beginn)
);

-- At most one open breach per plant and Nummer; closed ones are history.
CREATE UNIQUE INDEX ep_offen ON eeg_pflichtverstoesse (tenant, tr_id, typ)
    WHERE behoben_am IS NULL;
CREATE INDEX ep_anlage ON eeg_pflichtverstoesse (tenant, tr_id, beginn DESC);

COMMENT ON TABLE eeg_pflichtverstoesse IS
    '§52 Abs. 1 EEG 2023 Pflichtverstöße recorded against a plant. Supplies the '
    'start date, the §52 Abs. 3 Satz 1 Nr. 1 cure and the Abs. 3 Satz 2 defect '
    'waiver for the four breaches einsd derives, and is the trigger for the nine '
    'it cannot derive.';
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
    -- Anlage 1 Nr. 2 Satz 2 EEG 2023: this settlement used a **provisional**
    -- Jahresmarktwert — an ÜNB running estimate rather than the binding figure,
    -- which exists only once the year is over. The month is settled and paid;
    -- it just has to be recomputed when the final figure is published, which is
    -- a § 147 AO correction like any other. False for a Monatsmarktwert, which
    -- is final when published, and for the EPEX fallback.
    marktwert_vorlaeufig        BOOLEAN     NOT NULL DEFAULT false,
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
    -- | 'jahreskontingent_erschoepft' (§ 8 Abs. 4 KWKG: the calendar year's
    -- Vollbenutzungsstunden are used up; the Förderung resumes in January)
    -- | 'sanctioned' | 'kein_anspruch' | 'unknown'
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

-- ── EEG tariff reference table ───────────────────────────────────────────────
-- Statutory Einspeisevergütung by Erzeugungsart, Vergütungsform, capacity band
-- and Inbetriebnahme window. The §§ 40–49 Absenkungen are already folded into
-- the rows, so a lookup is a table read and never a computation.

CREATE TABLE eeg_verguetungssaetze (
    id                  SERIAL      PRIMARY KEY,
    erzeugungsart       TEXT        NOT NULL,
    leistung_min_kwp    NUMERIC(10, 3) NOT NULL,
    leistung_max_kwp    NUMERIC(10, 3),         -- NULL = no upper bound
    -- Same vocabulary as eeg_anlagen.verguetungsform, so the lookup can join one
    -- against the other. No KWK_ZUSCHLAG rows are seeded: § 7 KWKG prices per
    -- Leistungsanteil, so a plant's Zuschlag is a blend across the bands its
    -- capacity spans and no single-rate row can state it.
    verguetungsform     TEXT        NOT NULL DEFAULT 'UEBERSCHUSS' CHECK (verguetungsform IN (
        'UEBERSCHUSS', 'VOLLEINSPEISUNG', 'KWK_ZUSCHLAG'
    )),
    billing_start       DATE        NOT NULL,   -- first Inbetriebnahmedatum in the window
    billing_end         DATE,                   -- last Inbetriebnahmedatum; NULL = open-ended
    verguetungssatz_ct  NUMERIC(8, 4) NOT NULL,
    eeg_gesetz          SMALLINT    NOT NULL,
    notes               TEXT,
    -- verguetungsform is part of the key: the §48 Abs. 2a Volleinspeisung rate
    -- and the Überschuss rate of the same band and window are two different
    -- values, and one row cannot hold both.
    UNIQUE (erzeugungsart, verguetungsform, leistung_min_kwp, billing_start),
    CONSTRAINT evs_band_forward CHECK (leistung_max_kwp IS NULL OR leistung_max_kwp > leistung_min_kwp),
    CONSTRAINT evs_period_forward CHECK (billing_end IS NULL OR billing_end >= billing_start)
);

COMMENT ON TABLE eeg_verguetungssaetze IS
    'Statutory EEG Einspeisevergütung reference, generated from eeg_billing::seed. '
    'verguetungssatz_ct is the NET rate: anzulegender Wert less the §53 Abs. 1 deduction. '
    'billing_start/billing_end bound the plant Inbetriebnahmedatum, not the settled month.';

CREATE INDEX evs_lookup ON eeg_verguetungssaetze
    (erzeugungsart, verguetungsform, billing_start, billing_end);

-- Seed: the §§ 40–49 EEG 2023 Einspeisevergütung series, one row per
-- (Erzeugungsart, Vergütungsform, Leistungsband, Inbetriebnahme-Fenster).
--
-- GENERATED from `eeg_billing::seed::verguetungssatz_rows()`. Edit the crate and
-- regenerate; `tests/verguetungssaetze_seed_guard.rs` fails when the two drift.
--
-- Rates are NET: the anzulegender Wert less the § 53 Abs. 1 deduction (0,4 ct
-- for solar, 0,2 ct for the rest). The window bounds the plant's
-- **Inbetriebnahmedatum**, because every §§ 40–49 Absenkung applies „für die
-- nach diesem Zeitpunkt in Betrieb genommenen Anlagen"; a plant keeps its
-- window's value for its whole Förderdauer.
--
-- Wind is absent: § 22 Abs. 2 makes its claim depend on a BNetzA Zuschlag and
-- § 36h derives the value from it, so awarded values are imported per plant.
-- KWKG is absent too: § 7 prices per Leistungsanteil, so a plant's Zuschlag is a
-- blend across the bands its capacity spans and no single-rate row can state it.
INSERT INTO eeg_verguetungssaetze (erzeugungsart, leistung_min_kwp, leistung_max_kwp, verguetungsform, verguetungssatz_ct, billing_start, billing_end, eeg_gesetz, notes) VALUES
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 8.20, '2023-01-01', '2024-01-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 7.10, '2023-01-01', '2024-01-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.80, '2023-01-01', '2024-01-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 8.11, '2024-02-01', '2024-07-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 7.03, '2024-02-01', '2024-07-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.74, '2024-02-01', '2024-07-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 8.03, '2024-08-01', '2025-01-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.95, '2024-08-01', '2025-01-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.68, '2024-08-01', '2025-01-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.94, '2025-02-01', '2025-07-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.88, '2025-02-01', '2025-07-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.62, '2025-02-01', '2025-07-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.86, '2025-08-01', '2026-01-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.80, '2025-08-01', '2026-01-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.56, '2025-08-01', '2026-01-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.78, '2026-02-01', '2026-07-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.73, '2026-02-01', '2026-07-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.50, '2026-02-01', '2026-07-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.70, '2026-08-01', '2027-01-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.66, '2026-08-01', '2027-01-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.44, '2026-08-01', '2027-01-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.62, '2027-02-01', '2027-07-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.59, '2027-02-01', '2027-07-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.38, '2027-02-01', '2027-07-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.54, '2027-08-01', '2028-01-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.52, '2027-08-01', '2028-01-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.32, '2027-08-01', '2028-01-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.46, '2028-02-01', '2028-07-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.45, '2028-02-01', '2028-07-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.26, '2028-02-01', '2028-07-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.38, '2028-08-01', '2029-01-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.38, '2028-08-01', '2029-01-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.21, '2028-08-01', '2029-01-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.30, '2029-02-01', '2029-07-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.32, '2029-02-01', '2029-07-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.15, '2029-02-01', '2029-07-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.22, '2029-08-01', '2030-01-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.25, '2029-08-01', '2030-01-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.10, '2029-08-01', '2030-01-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.15, '2030-02-01', '2030-07-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.18, '2030-02-01', '2030-07-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 5.04, '2030-02-01', '2030-07-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'UEBERSCHUSS', 7.07, '2030-08-01', '2030-12-31', 2023, '§48 Abs. 2 Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'UEBERSCHUSS', 6.12, '2030-08-01', '2030-12-31', 2023, '§48 Abs. 2 Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 1000, 'UEBERSCHUSS', 4.99, '2030-08-01', '2030-12-31', 2023, '§48 Abs. 2 Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 13.00, '2023-01-01', '2024-01-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.90, '2023-01-01', '2024-01-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.90, '2023-01-01', '2024-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 9.00, '2023-01-01', '2024-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.70, '2023-01-01', '2024-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 12.87, '2024-02-01', '2024-07-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.79, '2024-02-01', '2024-07-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.79, '2024-02-01', '2024-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.91, '2024-02-01', '2024-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.62, '2024-02-01', '2024-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 12.73, '2024-08-01', '2025-01-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.68, '2024-08-01', '2025-01-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.68, '2024-08-01', '2025-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.81, '2024-08-01', '2025-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.54, '2024-08-01', '2025-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 12.60, '2025-02-01', '2025-07-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.56, '2025-02-01', '2025-07-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.56, '2025-02-01', '2025-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.72, '2025-02-01', '2025-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.46, '2025-02-01', '2025-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 12.47, '2025-08-01', '2026-01-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.45, '2025-08-01', '2026-01-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.45, '2025-08-01', '2026-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.63, '2025-08-01', '2026-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.38, '2025-08-01', '2026-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 12.34, '2026-02-01', '2026-07-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.35, '2026-02-01', '2026-07-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.35, '2026-02-01', '2026-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.54, '2026-02-01', '2026-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.30, '2026-02-01', '2026-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 12.22, '2026-08-01', '2027-01-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.24, '2026-08-01', '2027-01-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.24, '2026-08-01', '2027-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.45, '2026-08-01', '2027-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.23, '2026-08-01', '2027-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 12.09, '2027-02-01', '2027-07-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.13, '2027-02-01', '2027-07-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.13, '2027-02-01', '2027-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.36, '2027-02-01', '2027-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.15, '2027-02-01', '2027-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 11.96, '2027-08-01', '2028-01-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 10.03, '2027-08-01', '2028-01-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 10.03, '2027-08-01', '2028-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.27, '2027-08-01', '2028-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.07, '2027-08-01', '2028-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 11.84, '2028-02-01', '2028-07-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 9.92, '2028-02-01', '2028-07-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 9.92, '2028-02-01', '2028-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.19, '2028-02-01', '2028-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 7.00, '2028-02-01', '2028-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 11.72, '2028-08-01', '2029-01-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 9.82, '2028-08-01', '2029-01-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 9.82, '2028-08-01', '2029-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.10, '2028-08-01', '2029-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 6.93, '2028-08-01', '2029-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 11.60, '2029-02-01', '2029-07-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 9.72, '2029-02-01', '2029-07-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 9.72, '2029-02-01', '2029-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 8.02, '2029-02-01', '2029-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 6.85, '2029-02-01', '2029-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 11.48, '2029-08-01', '2030-01-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 9.62, '2029-08-01', '2030-01-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 9.62, '2029-08-01', '2030-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 7.93, '2029-08-01', '2030-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 6.78, '2029-08-01', '2030-01-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 11.36, '2030-02-01', '2030-07-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 9.52, '2030-02-01', '2030-07-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 9.52, '2030-02-01', '2030-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 7.85, '2030-02-01', '2030-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 6.71, '2030-02-01', '2030-07-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('SOLAR_AUFDACH', 0, 10, 'VOLLEINSPEISUNG', 11.24, '2030-08-01', '2030-12-31', 2023, '§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1 EEG 2023'),
('SOLAR_AUFDACH', 10, 40, 'VOLLEINSPEISUNG', 9.42, '2030-08-01', '2030-12-31', 2023, '§48 Abs. 2 Nr. 2 + Abs. 2a Nr. 2 EEG 2023'),
('SOLAR_AUFDACH', 40, 100, 'VOLLEINSPEISUNG', 9.42, '2030-08-01', '2030-12-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 3 EEG 2023'),
('SOLAR_AUFDACH', 100, 400, 'VOLLEINSPEISUNG', 7.77, '2030-08-01', '2030-12-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 4 EEG 2023'),
('SOLAR_AUFDACH', 400, 1000, 'VOLLEINSPEISUNG', 6.64, '2030-08-01', '2030-12-31', 2023, '§48 Abs. 2 Nr. 3 + Abs. 2a Nr. 5 EEG 2023'),
('WASSERKRAFT', 0, 500, 'UEBERSCHUSS', 11.83, '2023-01-01', '2023-12-31', 2023, '§40 Abs. 1 Nr. 1 EEG 2023'),
('WASSERKRAFT', 500, 2000, 'UEBERSCHUSS', 7.73, '2023-01-01', '2023-12-31', 2023, '§40 Abs. 1 Nr. 2 EEG 2023'),
('WASSERKRAFT', 2000, 5000, 'UEBERSCHUSS', 5.87, '2023-01-01', '2023-12-31', 2023, '§40 Abs. 1 Nr. 3 EEG 2023'),
('WASSERKRAFT', 5000, 10000, 'UEBERSCHUSS', 5.12, '2023-01-01', '2023-12-31', 2023, '§40 Abs. 1 Nr. 4 EEG 2023'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS', 4.93, '2023-01-01', '2023-12-31', 2023, '§40 Abs. 1 Nr. 5 EEG 2023'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS', 3.92, '2023-01-01', '2023-12-31', 2023, '§40 Abs. 1 Nr. 6 EEG 2023'),
('WASSERKRAFT', 50000, NULL, 'UEBERSCHUSS', 3.17, '2023-01-01', '2023-12-31', 2023, '§40 Abs. 1 Nr. 7 EEG 2023'),
('WASSERKRAFT', 0, 500, 'UEBERSCHUSS', 11.77, '2024-01-01', '2024-12-31', 2023, '§40 Abs. 1 Nr. 1 EEG 2023'),
('WASSERKRAFT', 500, 2000, 'UEBERSCHUSS', 7.69, '2024-01-01', '2024-12-31', 2023, '§40 Abs. 1 Nr. 2 EEG 2023'),
('WASSERKRAFT', 2000, 5000, 'UEBERSCHUSS', 5.84, '2024-01-01', '2024-12-31', 2023, '§40 Abs. 1 Nr. 3 EEG 2023'),
('WASSERKRAFT', 5000, 10000, 'UEBERSCHUSS', 5.09, '2024-01-01', '2024-12-31', 2023, '§40 Abs. 1 Nr. 4 EEG 2023'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS', 4.90, '2024-01-01', '2024-12-31', 2023, '§40 Abs. 1 Nr. 5 EEG 2023'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS', 3.90, '2024-01-01', '2024-12-31', 2023, '§40 Abs. 1 Nr. 6 EEG 2023'),
('WASSERKRAFT', 50000, NULL, 'UEBERSCHUSS', 3.15, '2024-01-01', '2024-12-31', 2023, '§40 Abs. 1 Nr. 7 EEG 2023'),
('WASSERKRAFT', 0, 500, 'UEBERSCHUSS', 11.71, '2025-01-01', '2025-12-31', 2023, '§40 Abs. 1 Nr. 1 EEG 2023'),
('WASSERKRAFT', 500, 2000, 'UEBERSCHUSS', 7.65, '2025-01-01', '2025-12-31', 2023, '§40 Abs. 1 Nr. 2 EEG 2023'),
('WASSERKRAFT', 2000, 5000, 'UEBERSCHUSS', 5.81, '2025-01-01', '2025-12-31', 2023, '§40 Abs. 1 Nr. 3 EEG 2023'),
('WASSERKRAFT', 5000, 10000, 'UEBERSCHUSS', 5.07, '2025-01-01', '2025-12-31', 2023, '§40 Abs. 1 Nr. 4 EEG 2023'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS', 4.88, '2025-01-01', '2025-12-31', 2023, '§40 Abs. 1 Nr. 5 EEG 2023'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS', 3.88, '2025-01-01', '2025-12-31', 2023, '§40 Abs. 1 Nr. 6 EEG 2023'),
('WASSERKRAFT', 50000, NULL, 'UEBERSCHUSS', 3.14, '2025-01-01', '2025-12-31', 2023, '§40 Abs. 1 Nr. 7 EEG 2023'),
('WASSERKRAFT', 0, 500, 'UEBERSCHUSS', 11.65, '2026-01-01', '2026-12-31', 2023, '§40 Abs. 1 Nr. 1 EEG 2023'),
('WASSERKRAFT', 500, 2000, 'UEBERSCHUSS', 7.61, '2026-01-01', '2026-12-31', 2023, '§40 Abs. 1 Nr. 2 EEG 2023'),
('WASSERKRAFT', 2000, 5000, 'UEBERSCHUSS', 5.78, '2026-01-01', '2026-12-31', 2023, '§40 Abs. 1 Nr. 3 EEG 2023'),
('WASSERKRAFT', 5000, 10000, 'UEBERSCHUSS', 5.04, '2026-01-01', '2026-12-31', 2023, '§40 Abs. 1 Nr. 4 EEG 2023'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS', 4.85, '2026-01-01', '2026-12-31', 2023, '§40 Abs. 1 Nr. 5 EEG 2023'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS', 3.86, '2026-01-01', '2026-12-31', 2023, '§40 Abs. 1 Nr. 6 EEG 2023'),
('WASSERKRAFT', 50000, NULL, 'UEBERSCHUSS', 3.12, '2026-01-01', '2026-12-31', 2023, '§40 Abs. 1 Nr. 7 EEG 2023'),
('WASSERKRAFT', 0, 500, 'UEBERSCHUSS', 11.59, '2027-01-01', '2027-12-31', 2023, '§40 Abs. 1 Nr. 1 EEG 2023'),
('WASSERKRAFT', 500, 2000, 'UEBERSCHUSS', 7.57, '2027-01-01', '2027-12-31', 2023, '§40 Abs. 1 Nr. 2 EEG 2023'),
('WASSERKRAFT', 2000, 5000, 'UEBERSCHUSS', 5.75, '2027-01-01', '2027-12-31', 2023, '§40 Abs. 1 Nr. 3 EEG 2023'),
('WASSERKRAFT', 5000, 10000, 'UEBERSCHUSS', 5.01, '2027-01-01', '2027-12-31', 2023, '§40 Abs. 1 Nr. 4 EEG 2023'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS', 4.83, '2027-01-01', '2027-12-31', 2023, '§40 Abs. 1 Nr. 5 EEG 2023'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS', 3.84, '2027-01-01', '2027-12-31', 2023, '§40 Abs. 1 Nr. 6 EEG 2023'),
('WASSERKRAFT', 50000, NULL, 'UEBERSCHUSS', 3.10, '2027-01-01', '2027-12-31', 2023, '§40 Abs. 1 Nr. 7 EEG 2023'),
('WASSERKRAFT', 0, 500, 'UEBERSCHUSS', 11.53, '2028-01-01', '2028-12-31', 2023, '§40 Abs. 1 Nr. 1 EEG 2023'),
('WASSERKRAFT', 500, 2000, 'UEBERSCHUSS', 7.53, '2028-01-01', '2028-12-31', 2023, '§40 Abs. 1 Nr. 2 EEG 2023'),
('WASSERKRAFT', 2000, 5000, 'UEBERSCHUSS', 5.72, '2028-01-01', '2028-12-31', 2023, '§40 Abs. 1 Nr. 3 EEG 2023'),
('WASSERKRAFT', 5000, 10000, 'UEBERSCHUSS', 4.99, '2028-01-01', '2028-12-31', 2023, '§40 Abs. 1 Nr. 4 EEG 2023'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS', 4.80, '2028-01-01', '2028-12-31', 2023, '§40 Abs. 1 Nr. 5 EEG 2023'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS', 3.82, '2028-01-01', '2028-12-31', 2023, '§40 Abs. 1 Nr. 6 EEG 2023'),
('WASSERKRAFT', 50000, NULL, 'UEBERSCHUSS', 3.09, '2028-01-01', '2028-12-31', 2023, '§40 Abs. 1 Nr. 7 EEG 2023'),
('WASSERKRAFT', 0, 500, 'UEBERSCHUSS', 11.47, '2029-01-01', '2029-12-31', 2023, '§40 Abs. 1 Nr. 1 EEG 2023'),
('WASSERKRAFT', 500, 2000, 'UEBERSCHUSS', 7.50, '2029-01-01', '2029-12-31', 2023, '§40 Abs. 1 Nr. 2 EEG 2023'),
('WASSERKRAFT', 2000, 5000, 'UEBERSCHUSS', 5.69, '2029-01-01', '2029-12-31', 2023, '§40 Abs. 1 Nr. 3 EEG 2023'),
('WASSERKRAFT', 5000, 10000, 'UEBERSCHUSS', 4.96, '2029-01-01', '2029-12-31', 2023, '§40 Abs. 1 Nr. 4 EEG 2023'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS', 4.78, '2029-01-01', '2029-12-31', 2023, '§40 Abs. 1 Nr. 5 EEG 2023'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS', 3.80, '2029-01-01', '2029-12-31', 2023, '§40 Abs. 1 Nr. 6 EEG 2023'),
('WASSERKRAFT', 50000, NULL, 'UEBERSCHUSS', 3.07, '2029-01-01', '2029-12-31', 2023, '§40 Abs. 1 Nr. 7 EEG 2023'),
('WASSERKRAFT', 0, 500, 'UEBERSCHUSS', 11.42, '2030-01-01', '2030-12-31', 2023, '§40 Abs. 1 Nr. 1 EEG 2023'),
('WASSERKRAFT', 500, 2000, 'UEBERSCHUSS', 7.46, '2030-01-01', '2030-12-31', 2023, '§40 Abs. 1 Nr. 2 EEG 2023'),
('WASSERKRAFT', 2000, 5000, 'UEBERSCHUSS', 5.66, '2030-01-01', '2030-12-31', 2023, '§40 Abs. 1 Nr. 3 EEG 2023'),
('WASSERKRAFT', 5000, 10000, 'UEBERSCHUSS', 4.94, '2030-01-01', '2030-12-31', 2023, '§40 Abs. 1 Nr. 4 EEG 2023'),
('WASSERKRAFT', 10000, 20000, 'UEBERSCHUSS', 4.75, '2030-01-01', '2030-12-31', 2023, '§40 Abs. 1 Nr. 5 EEG 2023'),
('WASSERKRAFT', 20000, 50000, 'UEBERSCHUSS', 3.78, '2030-01-01', '2030-12-31', 2023, '§40 Abs. 1 Nr. 6 EEG 2023'),
('WASSERKRAFT', 50000, NULL, 'UEBERSCHUSS', 3.05, '2030-01-01', '2030-12-31', 2023, '§40 Abs. 1 Nr. 7 EEG 2023'),
('DEPONIEGAS', 0, 500, 'UEBERSCHUSS', 7.26, '2023-01-01', '2023-12-31', 2023, '§41 Abs. 1 Nr. 1 EEG 2023'),
('DEPONIEGAS', 500, 5000, 'UEBERSCHUSS', 4.97, '2023-01-01', '2023-12-31', 2023, '§41 Abs. 1 Nr. 2 EEG 2023'),
('DEPONIEGAS', 0, 500, 'UEBERSCHUSS', 7.15, '2024-01-01', '2024-12-31', 2023, '§41 Abs. 1 Nr. 1 EEG 2023'),
('DEPONIEGAS', 500, 5000, 'UEBERSCHUSS', 4.89, '2024-01-01', '2024-12-31', 2023, '§41 Abs. 1 Nr. 2 EEG 2023'),
('DEPONIEGAS', 0, 500, 'UEBERSCHUSS', 7.04, '2025-01-01', '2025-12-31', 2023, '§41 Abs. 1 Nr. 1 EEG 2023'),
('DEPONIEGAS', 500, 5000, 'UEBERSCHUSS', 4.82, '2025-01-01', '2025-12-31', 2023, '§41 Abs. 1 Nr. 2 EEG 2023'),
('DEPONIEGAS', 0, 500, 'UEBERSCHUSS', 6.93, '2026-01-01', '2026-12-31', 2023, '§41 Abs. 1 Nr. 1 EEG 2023'),
('DEPONIEGAS', 500, 5000, 'UEBERSCHUSS', 4.74, '2026-01-01', '2026-12-31', 2023, '§41 Abs. 1 Nr. 2 EEG 2023'),
('DEPONIEGAS', 0, 500, 'UEBERSCHUSS', 6.82, '2027-01-01', '2027-12-31', 2023, '§41 Abs. 1 Nr. 1 EEG 2023'),
('DEPONIEGAS', 500, 5000, 'UEBERSCHUSS', 4.67, '2027-01-01', '2027-12-31', 2023, '§41 Abs. 1 Nr. 2 EEG 2023'),
('DEPONIEGAS', 0, 500, 'UEBERSCHUSS', 6.72, '2028-01-01', '2028-12-31', 2023, '§41 Abs. 1 Nr. 1 EEG 2023'),
('DEPONIEGAS', 500, 5000, 'UEBERSCHUSS', 4.59, '2028-01-01', '2028-12-31', 2023, '§41 Abs. 1 Nr. 2 EEG 2023'),
('DEPONIEGAS', 0, 500, 'UEBERSCHUSS', 6.61, '2029-01-01', '2029-12-31', 2023, '§41 Abs. 1 Nr. 1 EEG 2023'),
('DEPONIEGAS', 500, 5000, 'UEBERSCHUSS', 4.52, '2029-01-01', '2029-12-31', 2023, '§41 Abs. 1 Nr. 2 EEG 2023'),
('DEPONIEGAS', 0, 500, 'UEBERSCHUSS', 6.51, '2030-01-01', '2030-12-31', 2023, '§41 Abs. 1 Nr. 1 EEG 2023'),
('DEPONIEGAS', 500, 5000, 'UEBERSCHUSS', 4.45, '2030-01-01', '2030-12-31', 2023, '§41 Abs. 1 Nr. 2 EEG 2023'),
('KLAERGAS', 0, 500, 'UEBERSCHUSS', 5.73, '2023-01-01', '2023-12-31', 2023, '§41 Abs. 2 Nr. 1 EEG 2023'),
('KLAERGAS', 500, 5000, 'UEBERSCHUSS', 4.97, '2023-01-01', '2023-12-31', 2023, '§41 Abs. 2 Nr. 2 EEG 2023'),
('KLAERGAS', 0, 500, 'UEBERSCHUSS', 5.64, '2024-01-01', '2024-12-31', 2023, '§41 Abs. 2 Nr. 1 EEG 2023'),
('KLAERGAS', 500, 5000, 'UEBERSCHUSS', 4.89, '2024-01-01', '2024-12-31', 2023, '§41 Abs. 2 Nr. 2 EEG 2023'),
('KLAERGAS', 0, 500, 'UEBERSCHUSS', 5.55, '2025-01-01', '2025-12-31', 2023, '§41 Abs. 2 Nr. 1 EEG 2023'),
('KLAERGAS', 500, 5000, 'UEBERSCHUSS', 4.82, '2025-01-01', '2025-12-31', 2023, '§41 Abs. 2 Nr. 2 EEG 2023'),
('KLAERGAS', 0, 500, 'UEBERSCHUSS', 5.47, '2026-01-01', '2026-12-31', 2023, '§41 Abs. 2 Nr. 1 EEG 2023'),
('KLAERGAS', 500, 5000, 'UEBERSCHUSS', 4.74, '2026-01-01', '2026-12-31', 2023, '§41 Abs. 2 Nr. 2 EEG 2023'),
('KLAERGAS', 0, 500, 'UEBERSCHUSS', 5.38, '2027-01-01', '2027-12-31', 2023, '§41 Abs. 2 Nr. 1 EEG 2023'),
('KLAERGAS', 500, 5000, 'UEBERSCHUSS', 4.67, '2027-01-01', '2027-12-31', 2023, '§41 Abs. 2 Nr. 2 EEG 2023'),
('KLAERGAS', 0, 500, 'UEBERSCHUSS', 5.30, '2028-01-01', '2028-12-31', 2023, '§41 Abs. 2 Nr. 1 EEG 2023'),
('KLAERGAS', 500, 5000, 'UEBERSCHUSS', 4.59, '2028-01-01', '2028-12-31', 2023, '§41 Abs. 2 Nr. 2 EEG 2023'),
('KLAERGAS', 0, 500, 'UEBERSCHUSS', 5.22, '2029-01-01', '2029-12-31', 2023, '§41 Abs. 2 Nr. 1 EEG 2023'),
('KLAERGAS', 500, 5000, 'UEBERSCHUSS', 4.52, '2029-01-01', '2029-12-31', 2023, '§41 Abs. 2 Nr. 2 EEG 2023'),
('KLAERGAS', 0, 500, 'UEBERSCHUSS', 5.13, '2030-01-01', '2030-12-31', 2023, '§41 Abs. 2 Nr. 1 EEG 2023'),
('KLAERGAS', 500, 5000, 'UEBERSCHUSS', 4.45, '2030-01-01', '2030-12-31', 2023, '§41 Abs. 2 Nr. 2 EEG 2023'),
('GRUBENGAS', 0, 1000, 'UEBERSCHUSS', 5.78, '2023-01-01', '2023-12-31', 2023, '§41 Abs. 3 Nr. 1 EEG 2023'),
('GRUBENGAS', 1000, 5000, 'UEBERSCHUSS', 3.61, '2023-01-01', '2023-12-31', 2023, '§41 Abs. 3 Nr. 2 EEG 2023'),
('GRUBENGAS', 5000, NULL, 'UEBERSCHUSS', 3.17, '2023-01-01', '2023-12-31', 2023, '§41 Abs. 3 Nr. 3 EEG 2023'),
('GRUBENGAS', 0, 1000, 'UEBERSCHUSS', 5.69, '2024-01-01', '2024-12-31', 2023, '§41 Abs. 3 Nr. 1 EEG 2023'),
('GRUBENGAS', 1000, 5000, 'UEBERSCHUSS', 3.55, '2024-01-01', '2024-12-31', 2023, '§41 Abs. 3 Nr. 2 EEG 2023'),
('GRUBENGAS', 5000, NULL, 'UEBERSCHUSS', 3.12, '2024-01-01', '2024-12-31', 2023, '§41 Abs. 3 Nr. 3 EEG 2023'),
('GRUBENGAS', 0, 1000, 'UEBERSCHUSS', 5.60, '2025-01-01', '2025-12-31', 2023, '§41 Abs. 3 Nr. 1 EEG 2023'),
('GRUBENGAS', 1000, 5000, 'UEBERSCHUSS', 3.50, '2025-01-01', '2025-12-31', 2023, '§41 Abs. 3 Nr. 2 EEG 2023'),
('GRUBENGAS', 5000, NULL, 'UEBERSCHUSS', 3.07, '2025-01-01', '2025-12-31', 2023, '§41 Abs. 3 Nr. 3 EEG 2023'),
('GRUBENGAS', 0, 1000, 'UEBERSCHUSS', 5.51, '2026-01-01', '2026-12-31', 2023, '§41 Abs. 3 Nr. 1 EEG 2023'),
('GRUBENGAS', 1000, 5000, 'UEBERSCHUSS', 3.44, '2026-01-01', '2026-12-31', 2023, '§41 Abs. 3 Nr. 2 EEG 2023'),
('GRUBENGAS', 5000, NULL, 'UEBERSCHUSS', 3.02, '2026-01-01', '2026-12-31', 2023, '§41 Abs. 3 Nr. 3 EEG 2023'),
('GRUBENGAS', 0, 1000, 'UEBERSCHUSS', 5.43, '2027-01-01', '2027-12-31', 2023, '§41 Abs. 3 Nr. 1 EEG 2023'),
('GRUBENGAS', 1000, 5000, 'UEBERSCHUSS', 3.39, '2027-01-01', '2027-12-31', 2023, '§41 Abs. 3 Nr. 2 EEG 2023'),
('GRUBENGAS', 5000, NULL, 'UEBERSCHUSS', 2.97, '2027-01-01', '2027-12-31', 2023, '§41 Abs. 3 Nr. 3 EEG 2023'),
('GRUBENGAS', 0, 1000, 'UEBERSCHUSS', 5.34, '2028-01-01', '2028-12-31', 2023, '§41 Abs. 3 Nr. 1 EEG 2023'),
('GRUBENGAS', 1000, 5000, 'UEBERSCHUSS', 3.33, '2028-01-01', '2028-12-31', 2023, '§41 Abs. 3 Nr. 2 EEG 2023'),
('GRUBENGAS', 5000, NULL, 'UEBERSCHUSS', 2.92, '2028-01-01', '2028-12-31', 2023, '§41 Abs. 3 Nr. 3 EEG 2023'),
('GRUBENGAS', 0, 1000, 'UEBERSCHUSS', 5.26, '2029-01-01', '2029-12-31', 2023, '§41 Abs. 3 Nr. 1 EEG 2023'),
('GRUBENGAS', 1000, 5000, 'UEBERSCHUSS', 3.28, '2029-01-01', '2029-12-31', 2023, '§41 Abs. 3 Nr. 2 EEG 2023'),
('GRUBENGAS', 5000, NULL, 'UEBERSCHUSS', 2.88, '2029-01-01', '2029-12-31', 2023, '§41 Abs. 3 Nr. 3 EEG 2023'),
('GRUBENGAS', 0, 1000, 'UEBERSCHUSS', 5.18, '2030-01-01', '2030-12-31', 2023, '§41 Abs. 3 Nr. 1 EEG 2023'),
('GRUBENGAS', 1000, 5000, 'UEBERSCHUSS', 3.23, '2030-01-01', '2030-12-31', 2023, '§41 Abs. 3 Nr. 2 EEG 2023'),
('GRUBENGAS', 5000, NULL, 'UEBERSCHUSS', 2.83, '2030-01-01', '2030-12-31', 2023, '§41 Abs. 3 Nr. 3 EEG 2023'),
('BIOMASSE', 0, 150, 'UEBERSCHUSS', 12.47, '2023-01-01', '2024-06-30', 2023, '§42 Satz 1 EEG 2023'),
('BIOMASSE', 0, 150, 'UEBERSCHUSS', 12.41, '2024-07-01', '2025-06-30', 2023, '§42 Satz 1 EEG 2023'),
('BIOMASSE', 0, 150, 'UEBERSCHUSS', 12.34, '2025-07-01', '2026-06-30', 2023, '§42 Satz 1 EEG 2023'),
('BIOMASSE', 0, 150, 'UEBERSCHUSS', 12.28, '2026-07-01', '2027-06-30', 2023, '§42 Satz 1 EEG 2023'),
('BIOMASSE', 0, 150, 'UEBERSCHUSS', 12.22, '2027-07-01', '2028-06-30', 2023, '§42 Satz 1 EEG 2023'),
('BIOMASSE', 0, 150, 'UEBERSCHUSS', 12.16, '2028-07-01', '2029-06-30', 2023, '§42 Satz 1 EEG 2023'),
('BIOMASSE', 0, 150, 'UEBERSCHUSS', 12.09, '2029-07-01', '2030-06-30', 2023, '§42 Satz 1 EEG 2023'),
('BIOMASSE', 0, 150, 'UEBERSCHUSS', 12.03, '2030-07-01', '2030-12-31', 2023, '§42 Satz 1 EEG 2023'),
('BIOGAS', 0, 500, 'UEBERSCHUSS', 13.96, '2023-01-01', '2024-06-30', 2023, '§43 Abs. 1 Nr. 1 EEG 2023'),
('BIOGAS', 500, 20000, 'UEBERSCHUSS', 12.21, '2023-01-01', '2024-06-30', 2023, '§43 Abs. 1 Nr. 2 EEG 2023'),
('BIOGAS', 0, 500, 'UEBERSCHUSS', 13.89, '2024-07-01', '2025-06-30', 2023, '§43 Abs. 1 Nr. 1 EEG 2023'),
('BIOGAS', 500, 20000, 'UEBERSCHUSS', 12.15, '2024-07-01', '2025-06-30', 2023, '§43 Abs. 1 Nr. 2 EEG 2023'),
('BIOGAS', 0, 500, 'UEBERSCHUSS', 13.82, '2025-07-01', '2026-06-30', 2023, '§43 Abs. 1 Nr. 1 EEG 2023'),
('BIOGAS', 500, 20000, 'UEBERSCHUSS', 12.09, '2025-07-01', '2026-06-30', 2023, '§43 Abs. 1 Nr. 2 EEG 2023'),
('BIOGAS', 0, 500, 'UEBERSCHUSS', 13.75, '2026-07-01', '2027-06-30', 2023, '§43 Abs. 1 Nr. 1 EEG 2023'),
('BIOGAS', 500, 20000, 'UEBERSCHUSS', 12.02, '2026-07-01', '2027-06-30', 2023, '§43 Abs. 1 Nr. 2 EEG 2023'),
('BIOGAS', 0, 500, 'UEBERSCHUSS', 13.68, '2027-07-01', '2028-06-30', 2023, '§43 Abs. 1 Nr. 1 EEG 2023'),
('BIOGAS', 500, 20000, 'UEBERSCHUSS', 11.96, '2027-07-01', '2028-06-30', 2023, '§43 Abs. 1 Nr. 2 EEG 2023'),
('BIOGAS', 0, 500, 'UEBERSCHUSS', 13.61, '2028-07-01', '2029-06-30', 2023, '§43 Abs. 1 Nr. 1 EEG 2023'),
('BIOGAS', 500, 20000, 'UEBERSCHUSS', 11.90, '2028-07-01', '2029-06-30', 2023, '§43 Abs. 1 Nr. 2 EEG 2023'),
('BIOGAS', 0, 500, 'UEBERSCHUSS', 13.54, '2029-07-01', '2030-06-30', 2023, '§43 Abs. 1 Nr. 1 EEG 2023'),
('BIOGAS', 500, 20000, 'UEBERSCHUSS', 11.84, '2029-07-01', '2030-06-30', 2023, '§43 Abs. 1 Nr. 2 EEG 2023'),
('BIOGAS', 0, 500, 'UEBERSCHUSS', 13.47, '2030-07-01', '2030-12-31', 2023, '§43 Abs. 1 Nr. 1 EEG 2023'),
('BIOGAS', 500, 20000, 'UEBERSCHUSS', 11.78, '2030-07-01', '2030-12-31', 2023, '§43 Abs. 1 Nr. 2 EEG 2023'),
('GEOTHERMIE', 0, NULL, 'UEBERSCHUSS', 25.00, '2023-01-01', '2023-12-31', 2023, '§45 Abs. 1 EEG 2023'),
('GEOTHERMIE', 0, NULL, 'UEBERSCHUSS', 24.87, '2024-01-01', '2024-12-31', 2023, '§45 Abs. 1 EEG 2023'),
('GEOTHERMIE', 0, NULL, 'UEBERSCHUSS', 24.75, '2025-01-01', '2025-12-31', 2023, '§45 Abs. 1 EEG 2023'),
('GEOTHERMIE', 0, NULL, 'UEBERSCHUSS', 24.62, '2026-01-01', '2026-12-31', 2023, '§45 Abs. 1 EEG 2023'),
('GEOTHERMIE', 0, NULL, 'UEBERSCHUSS', 24.50, '2027-01-01', '2027-12-31', 2023, '§45 Abs. 1 EEG 2023'),
('GEOTHERMIE', 0, NULL, 'UEBERSCHUSS', 24.38, '2028-01-01', '2028-12-31', 2023, '§45 Abs. 1 EEG 2023'),
('GEOTHERMIE', 0, NULL, 'UEBERSCHUSS', 24.25, '2029-01-01', '2029-12-31', 2023, '§45 Abs. 1 EEG 2023'),
('GEOTHERMIE', 0, NULL, 'UEBERSCHUSS', 24.13, '2030-01-01', '2030-12-31', 2023, '§45 Abs. 1 EEG 2023');

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

-- ── Anlage 1 Nr. 2–4 EEG 2023: der energieträgerspezifische Marktwert ───────
--
-- The Marktprämie is `max(0, AW − MW)`, and **which** MW is keyed on the plant's
-- vintage rather than on the operator's preference (Anlage 1 Nr. 2):
--
--   Satz 1 — plants in Betrieb genommen *or* bezuschlagt **before 01.01.2023**
--            use the energieträgerspezifische **Monats**marktwert of Nr. 3.
--   Satz 2 — every other plant uses the **Jahres**marktwert of Nr. 4.
--   Satz 3 — a Satz-1 plant moves onto the Jahresmarktwert as well once it
--            claims under the § 19 Abs. 3b/3c Abgrenzungs- oder Pauschaloption.
--
-- One table held both series before, keyed only by (Jahr, Monat, Erzeugungsart),
-- so whichever figure the operator happened to load was applied to every plant.
-- `art` makes the two distinguishable and the lookup asks for the one the plant
-- is entitled to; a missing figure is `price_missing`, which the monthly worker
-- retries, rather than a silent substitution.
--
-- A **Jahres**marktwert has no month: the ÜNB publish a running estimate during
-- the year and the binding figure after it. `billing_month` is therefore NULL
-- for that series and `vorlaeufig` says which of the two a row is — a settlement
-- computed on a provisional figure is correctable, and the receipt records that
-- it was so the year-end publication can find it.

CREATE TABLE marktwert_preise (
    billing_year    SMALLINT    NOT NULL,
    -- Anlage 1 Nr. 3 = MONATSMARKTWERT, Nr. 4 = JAHRESMARKTWERT.
    art             TEXT        NOT NULL CHECK (art IN ('MONATSMARKTWERT', 'JAHRESMARKTWERT')),
    -- The month for Nr. 3; NULL for Nr. 4, which has none.
    billing_month   SMALLINT    CHECK (billing_month BETWEEN 1 AND 12),
    -- Matches erzeugungsart values; 'DEFAULT' = generic fallback
    erzeugungsart   TEXT        NOT NULL,
    avg_ct_kwh      NUMERIC(8, 4) NOT NULL
                    CHECK (avg_ct_kwh BETWEEN -100 AND 1000),
    -- A running ÜNB estimate rather than the published binding figure. Always
    -- false for a Monatsmarktwert, which is final when it is published.
    vorlaeufig      BOOLEAN     NOT NULL DEFAULT false,
    source          TEXT        NOT NULL DEFAULT 'manual',
    imported_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT mw_monat_passt_zur_art CHECK (
        (art = 'MONATSMARKTWERT' AND billing_month IS NOT NULL)
     OR (art = 'JAHRESMARKTWERT'  AND billing_month IS NULL)),
    CONSTRAINT mw_monatswert_ist_endgueltig CHECK (
        art = 'JAHRESMARKTWERT' OR NOT vorlaeufig)
);

-- `billing_month` is NULL for the annual series, so the key is an index rather
-- than a PRIMARY KEY: NULLs are distinct under a plain unique constraint.
CREATE UNIQUE INDEX mw_key ON marktwert_preise
    (billing_year, art, erzeugungsart, COALESCE(billing_month, 0));
CREATE INDEX mw_period ON marktwert_preise (billing_year DESC, art, erzeugungsart);

COMMENT ON TABLE marktwert_preise IS
    'Anlage 1 Nr. 2–4 EEG 2023: the energieträgerspezifische Marktwert, in both '
    'series the statute defines. Published by ÜNB (netztransparenz.de). Which '
    'series a plant takes follows its Inbetriebnahme/Zuschlag date, never the '
    'operator. Lookup order within a series: exact erzeugungsart → DEFAULT → '
    'epex_monthly_prices.';

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
