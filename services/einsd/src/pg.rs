//! PostgreSQL persistence for `einsd`.
//!
//! Settlement formulas are implemented in the [`eeg-billing`] crate.
//! This module is responsible for:
//! - Plant registration and lifecycle (CRUD on `eeg_anlagen`)
//! - Persisting settlement receipts (`settlement_receipts`)
//! - KWKG hour-limit state tracking (`kwk_strom_kwh_gesamt` column)
//! - EPEX monthly price storage
//!
//! [`eeg-billing`]: eeg_billing

use crate::models::{KWKG_ZUSCHLAG, VERGUETUNG};
use anyhow::Context as _;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// ── EEG/KWKG Anlage ──────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen` and `PUT /api/v1/anlagen/{tr_id}`.
#[derive(Debug, Deserialize)]
pub struct AnlageUpsertRequest {
    pub tr_id: String,
    pub malo_id: String,
    pub melo_id: Option<String>,
    /// EEG law year of commissioning (2000/2004/2009/2012/2017/2021/2023) or `0` for KWKG.
    pub eeg_gesetz: i16,
    /// ISO 8601 commissioning date (Inbetriebnahmedatum).
    pub inbetriebnahme: String,
    /// Installed peak power in kWp (or kW_el for KWKG).
    pub leistung_kwp: Decimal,
    /// Generator type — see schema CHECK constraint for all valid values.
    pub erzeugungsart: String,
    /// EEG feed-in tariff / KWKG KWK-Zuschlag rate in ct/kWh.
    pub verguetungssatz_ct: Decimal,
    /// Which §48 rate column the plant is paid from: `"UEBERSCHUSS"` (default),
    /// `"VOLLEINSPEISUNG"` (§48 Abs. 2a bonus) or `"KWK_ZUSCHLAG"` (KWKG).
    #[serde(default = "default_ueberschuss")]
    pub verguetungsform: String,
    /// Settlement model.
    #[serde(default = "default_verguetung")]
    pub settlement_model: String,
    pub direktvermarktung: Option<bool>,
    /// Anzulegender Wert ct/kWh (Direktvermarktung / Ausschreibungswert).
    pub direktverm_aw_ct: Option<Decimal>,
    /// Direktvermarkter MP-ID.
    pub direktverm_mp_id: Option<String>,
    /// §21 Abs. 3 EEG Mieterstrom surcharge ct/kWh.
    pub mieter_zuschlag_ct: Option<Decimal>,
    /// BNetzA Zuschlag-ID for Ausschreibungsanlagen.
    pub ausschreibungs_zuschlag_id: Option<String>,
    /// §22 EEG 2023 — awarded anzulegender Wert (ct/kWh) from the BNetzA tender.
    #[serde(default)]
    pub zuschlagswert_ct: Option<Decimal>,
    /// Date of the BNetzA award notification (`CCYY-MM-DD`).
    #[serde(default)]
    pub zuschlag_datum: Option<String>,
    /// §39n EEG 2023 — Innovationsausschreibung (fixed market premium).
    #[serde(default)]
    pub ist_innovationsausschreibung: Option<bool>,
    /// §22b EEG 2023 — Bürgerenergiegesellschaft (§3 Nr. 15).
    #[serde(default)]
    pub ist_buergerenergie: Option<bool>,
    // ── Repowering (§3 Nr. 30 i.V.m. §25 EEG 2023) ──────────────────────────
    /// `true` for a Vollrepowering — replacing the generator unit. It is a fresh
    /// Inbetriebnahme (§3 Nr. 30), so §25 restarts from `repowering_datum` and the
    /// §51 regime is re-derived from that date.
    pub ist_repowering: Option<bool>,
    /// Original commissioning date before repowering (for audit trail).
    pub ursprungs_inbetriebnahme: Option<String>,
    /// Date of repowering — new `inbetriebnahme` for Förderungsdauer calculation.
    pub repowering_datum: Option<String>,
    // ── Zusammenlegung (§24 EEG 2023) ───────────────────────────────────────
    /// For merged plants: TR-ID of the parent entity.
    pub parent_tr_id: Option<String>,
    // ── KWKG §§ 7, 8 ─────────────────────────────────────────────────────────
    /// § 6 Abs. 1 KWKG — which class the plant belongs to:
    /// `"NEU" | "MODERNISIERT" | "NACHGERUESTET"`.
    ///
    /// It decides the § 7 Abs. 1 Nr. 5 rate above 2 MW and the § 8 Förderdauer.
    pub kwk_anlagenart: Option<String>,
    /// § 7 KWKG — what the KWK-Strom is used for:
    /// `"NETZ_DER_ALLGEMEINEN_VERSORGUNG"` for Abs. 1, or one of
    /// `"NICHT_EINGESPEIST_BIS100KW" | "NICHT_EINGESPEIST_KUNDENANLAGE" |
    /// "NICHT_EINGESPEIST_STROMKOSTENINTENSIV" |
    /// "NICHT_EINGESPEIST_BRANCHE_ANLAGE2"` for the § 7 Abs. 2/3 ladders, which
    /// pay markedly less.
    pub kwk_verwendung: Option<String>,
    /// § 7 Abs. 1 Satz 2 KWKG — whether the Bundesministerium für Wirtschaft und
    /// Energie published its Angemessenheits-Feststellung in the Bundesanzeiger.
    ///
    /// The 0,5 ct uplift on Nr. 5 lit. a is payable „soweit" it did, so this
    /// defaults to `false` and the uplift is not paid until an operator records
    /// the publication.
    pub kwk_bmwk_feststellung: Option<bool>,
    /// § 8 Abs. 2/3 KWKG — the cost of the Modernisierung or Nachrüstung as a
    /// share of the cost of building the plant new (`0.25` = 25 %).
    ///
    /// It selects the Vollbenutzungsstunden for a modernisierte or nachgerüstete
    /// plant; a neue Anlage does not need it (§ 8 Abs. 1 is a flat 30 000 h).
    pub kwk_kostenanteil: Option<rust_decimal::Decimal>,
    /// § 8 Abs. 2 KWKG — whole years between the plant first taking up
    /// Dauerbetrieb and this Modernisierung.
    ///
    /// Each Nummer of Abs. 2 has its own Karenzzeit (2, 5 and 10 years), so a
    /// Modernisierung inside it buys no Förderdauer however much it cost.
    pub kwk_jahre_seit_dauerbetrieb: Option<u32>,
    /// § 8 Abs. 2 Nr. 1 lit. c KWKG — whether the plant is a
    /// Dampfsammelschienen-KWK-Anlage with more than 50 MW electrical capacity,
    /// which is the only kind the 6 000-hour tier is open to.
    pub kwk_ist_dampfsammelschiene_ueber_50_mw: Option<bool>,
    /// § 8 Abs. 1–3 KWKG — Förderdauer in Vollbenutzungsstunden.
    ///
    /// Derived from `kwk_anlagenart` and `kwk_kostenanteil` when omitted.
    pub kwk_foerderdauer_h: Option<i32>,
    // ── Flexibilitätsprämie (§50 EEG) ───────────────────────────────────────
    /// Registered flex capacity in kW (§50 EEG biomass flex premium).
    pub flex_leistung_kw: Option<Decimal>,
    /// Flexibilitätsprämie rate in ct/kWh.
    pub flex_praemie_ct_kwh: Option<Decimal>, // ── MaStR + Bankverbindung ────────────────────────────────────────────────────────────────
    /// Whether the plant is registered in the Marktstammdatenregister (MaStR).
    ///
    /// When `false`: §52 penalty applies until registration is confirmed.
    /// - EEG 2023 plants: €10/kW/month Pflichtzahlung (§52 Abs. 1 Nr. 11 EEG 2023)
    /// - EEG ≤2021 plants: Vergütung = 0 (old §52/§47 via §100 Übergangsregelung)
    ///
    /// Confirm via `POST /api/v1/anlagen/{tr_id}/mastr-registrierung`.
    #[serde(default = "default_mastr_true")]
    pub mastr_registriert: bool,
    /// MaStR Registrierungsnummer (e.g. `"SEE900000000001"`).
    pub mastr_nummer: Option<String>,
    /// Date of MaStR registration (ISO 8601).
    pub mastr_datum: Option<String>,
    /// The operator. Payout account and § 19 UStG election live on the
    /// `einspeiser` record, because both belong to the person and not to the
    /// installation — see [`crate::pg_einspeiser`].
    ///
    /// **Mandatory.** § 7 Abs. 1 EEG 2023 puts the payment on the
    /// Netzbetreiber, and a plant nobody can be paid for is not a plant this
    /// service can act on — so `eeg_anlagen.einspeiser_id` is `NOT NULL`
    /// behind `fk_anlage_einspeiser`, and [`upsert_anlage`] refuses an id that
    /// names no registered Anlagenbetreiber. Register the operator first
    /// (`PUT /api/v1/einspeiser/{einspeiser_id}`).
    pub einspeiser_id: String,
    pub notes: Option<String>,
    /// §9 EEG — how the plant satisfies the Steuerbarkeit requirement:
    /// `"FERNSTEUERBARKEIT"`, `"LEISTUNGSBEGRENZUNG_60"` (the 60 % cap at the
    /// Netzverknüpfungspunkt, which §9 Abs. 2 Nr. 2 offers below 100 kW) or
    /// `"KEINE"`.
    ///
    /// Defaults to `"KEINE"`, which is a §52 Abs. 1 Nr. 1 violation wherever §9
    /// requires anything — so a compliant plant must say which route it took.
    #[serde(default = "default_sect9_keine")]
    pub sect9_erfuellung: String,
    /// §9 EEG — date the Fernsteuerbarkeit was installed (ISO 8601), where that
    /// is the chosen route.
    pub fernsteuerbarkeit_datum: Option<String>,
    /// §51b EEG 2023 — Biogas Ausschreibungsanlage with slightly-positive price rule.
    ///
    /// When `true`, the anzulegender Wert reduces to **zero** for any billing period
    /// where `epex_avg_ct_kwh ≤ 2 ct/kWh`. §51/§51a Negativpreisregel do NOT apply.
    ///
    /// Only valid for biogas plants (fermentation, excluding biomethane) that received
    /// their AW via BNetzA tender (`ausschreibungs_zuschlag_id` must be set).
    ///
    /// Legal basis: §51b EEG 2023. Default: `false`.
    #[serde(default)]
    pub is_biogas_sect51b: bool,
    /// §3 Nr. 37 EEG 2023 — Pilotwindenergieanlage an Land.
    ///
    /// Every Fassung of §51 exempts these from the Negativpreisregel regardless
    /// of size. It is a BNetzA/FGW certification fact about the turbine, so it
    /// is declared at registration rather than derived from the plant record.
    #[serde(default)]
    pub ist_pilotwindanlage: bool,
    /// §100 EEG — the date the operator declared, in Textform, that §§51 and 51a
    /// shall apply to this Bestandsanlage (ISO 8601).
    ///
    /// The declaration takes effect at the earliest at the end of the calendar
    /// year in which the plant is fitted with an iMSys, so it needs
    /// `imesys_rollout_datum` to start running. From the effective date the plant
    /// is settled under the Solarspitzengesetz §51 regime and its anzulegender
    /// Wert rises by 0,6 ct/kWh.
    pub sect51_optin_erklaert_am: Option<String>,
    /// §36e / §37e / §39e EEG 2023 — the date the BNetzA Zuschlag lapses if the
    /// plant is not commissioned in time (ISO 8601). From that date the plant has
    /// no award to settle against.
    pub zuschlag_erloeschen_datum: Option<String>,

    // ── §§42–44 EEG 2023 — Biomass fuel composition ──────────
    /// Primary fuel type fed into the plant.
    ///
    /// Matches [`eeg_billing::biomasse::BiomassBrennstoff`] DB variants:
    /// `PFLANZLICHE_BIOMASSE | BIOMETHAN_AUS_BIOMASSE | GUELLE | FESTMIST |
    /// HOLZBIOMASSE | KLAERGAS | DEPONIEGAS | GRUBENGAS | BIOABFALL`
    ///
    /// `None` for non-biomass plants (solar, wind, KWKG, hydro …).
    /// When set, the settlement engine enforces the § 39i Abs. 1 Getreide- und
    /// Mais-Höchstanteil and detects § 44 Güllekleinanlage eligibility at every
    /// billing period.
    pub biomasse_hauptbrennstoff: Option<String>,

    /// §44 EEG 2023 — fraction of energy input from liquid/solid manure (0.0–1.0).
    ///
    /// When `biomasse_guelle_anteil ≥ 0.80` **and** `leistung_kwp ≤ 75 kW`,
    /// the plant qualifies as a **Güllekleinanlage** and receives the higher
    /// §44 bonus rate. Use [`eeg_billing::rates::guelle_lookup`] to
    /// look up the applicable gross AW; subtract §53 deduction (−0.2 ct/kWh)
    /// before storing in `verguetungssatz_ct`.
    pub biomasse_guelle_anteil: Option<rust_decimal::Decimal>,

    /// § 39i Abs. 1 EEG 2023 — share of Getreidekorn und Mais in the mass fed to
    /// the Biogaserzeugung over the calendar year (0.0–1.0).
    ///
    /// The cap applies only to a plant holding a Zuschlag and steps down by the
    /// Gebotstermin: 40 % (2023), 35 % (2024 – 24.02.2025), 30 % (26.02.2025 –
    /// 2025), 25 % (2026–2028). Exceeding it leaves no § 19 Abs. 1 claim, and
    /// the period settles as `KeinAnspruch` at EUR 0.
    ///
    /// `None` is treated as 0.00 at settlement time — the cap cannot be breached
    /// by a plant that has not submitted fuel composition data.
    pub biomasse_getreide_mais_anteil: Option<rust_decimal::Decimal>,
}

fn default_verguetung() -> String {
    VERGUETUNG.to_owned()
}

fn default_ueberschuss() -> String {
    "UEBERSCHUSS".to_owned()
}

/// Stored plant record returned by GET endpoints.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AnlageRow {
    pub tr_id: String,
    pub tenant: String,
    pub malo_id: String,
    pub melo_id: Option<String>,
    pub eeg_gesetz: i16,
    pub inbetriebnahme: Date,
    pub leistung_kwp: Decimal,
    pub erzeugungsart: String,
    pub verguetungssatz_ct: Decimal,
    pub verguetungsform: String,
    /// The EEG Förderende, or `NULL` for a plant that has none.
    ///
    /// § 25 Abs. 1 EEG 2023 dates the EEG claim in years. § 8 KWKG does not: it
    /// measures the Zuschlag in Vollbenutzungsstunden (Abs. 1–3) and caps each
    /// calendar year separately (Abs. 4), and both are counters against
    /// generation. A KWK plant therefore has no calendar Förderende and this is
    /// `NULL` for it — `kwk_max_kwh` is what ends its Zuschlag.
    pub foerderendedatum: Option<Date>,
    pub settlement_model: String,
    pub direktvermarktung: bool,
    pub direktverm_aw_ct: Option<Decimal>,
    pub direktverm_mp_id: Option<String>,
    pub mieter_zuschlag_ct: Option<Decimal>,
    pub ausschreibungs_zuschlag_id: Option<String>,
    pub ist_repowering: bool,
    pub ursprungs_inbetriebnahme: Option<Date>,
    pub repowering_datum: Option<Date>,
    pub parent_tr_id: Option<String>,
    pub kwk_anlagenart: Option<String>,
    pub kwk_verwendung: Option<String>,
    /// § 7 Abs. 1 Satz 2 KWKG — whether the BMWK Feststellung is published.
    pub kwk_bmwk_feststellung: bool,
    pub kwk_kostenanteil: Option<Decimal>,
    pub kwk_foerderdauer_h: Option<i32>,
    pub kwk_strom_kwh_gesamt: Option<Decimal>,
    pub kwk_kwh_jahr: Option<Decimal>,
    pub kwk_kwh_jahr_year: Option<i16>,
    pub flex_leistung_kw: Option<Decimal>,
    pub flex_praemie_ct_kwh: Option<Decimal>,
    pub status: String,
    // MaStR + Bankverbindung
    pub mastr_registriert: bool,
    pub mastr_nummer: Option<String>,
    pub mastr_datum: Option<Date>,
    /// The operator behind the plant (`einspeiser.einspeiser_id`).
    pub einspeiser_id: String,
    pub notes: Option<String>,
    // Plant attributes
    pub inbetriebnahme_typ: Option<String>,
    pub wind_guetegrad: Option<Decimal>,
    pub wind_korrekturfaktor: Option<Decimal>,
    // §36h Abs. 2: JSONB Vec<GuetefaktorReeval> (year 6/11/16 re-evaluations)
    pub wind_guetefaktor_reevaluations: Option<serde_json::Value>,
    pub fernsteuerbarkeit_datum: Option<Date>,
    /// How the plant satisfies §9 — `KEINE` | `FERNSTEUERBARKEIT` | `LEISTUNGSBEGRENZUNG_60`.
    pub sect9_erfuellung: String,
    // Settlement lifecycle
    pub settlement_state: Option<String>,
    // §51b EEG 2023 biogas Ausschreibungsanlage
    pub is_biogas_sect51b: bool,
    // Ausschreibung lifecycle (§36e/§37e/§39e Erlöschen des Zuschlags)
    /// §22 EEG 2023 — the awarded anzulegender Wert (ct/kWh).
    pub zuschlagswert_ct: Option<Decimal>,
    /// Date of the BNetzA award notification.
    pub zuschlag_datum: Option<Date>,
    /// §39n EEG 2023 — Innovationsausschreibung.
    pub ist_innovationsausschreibung: bool,
    /// §22b EEG 2023 — Bürgerenergiegesellschaft (§3 Nr. 15).
    pub ist_buergerenergie: bool,
    pub zuschlag_erloeschen_datum: Option<Date>,
    // §52 Abs. 1 Nr. 11 — the one §52 clock einsd owns end to end. Every other
    // breach lives in `eeg_pflichtverstoesse`.
    pub mastr_violation_start: Option<Date>,
    /// § 19 Abs. 3b / 3c EEG 2023 — `KEINE` | `ABGRENZUNG` | `PAUSCHAL`. Anlage 1
    /// Nr. 2 Satz 3 moves a plant claiming either option onto the
    /// Jahresmarktwert whatever its vintage.
    pub speicher_option: String,
    // §21b Abs. 1 Satz 2 — the effective date of the last Veräußerungsform switch
    pub last_veraeusserungsform_switch: Option<Date>,
    // §51a cumulative RAW negative-price quarter-hours (drives effektives_foerderende)
    pub negative_price_qh_gesamt: i64,
    // §24 Erweiterung capacity blocks (migration 0003, JSONB)
    pub capacity_blocks: Option<serde_json::Value>,
    // §§42–44 EEG 2023 biomass fuel composition
    pub biomasse_hauptbrennstoff: Option<String>,
    pub biomasse_guelle_anteil: Option<Decimal>,
    pub biomasse_getreide_mais_anteil: Option<Decimal>,
    // §44b Biogas annual quota tracking
    pub biogas_quota_kwh_ytd: Decimal,
    pub biogas_quota_ytd_year: Option<i16>,
    // §51 Abs. 2 Nr. 1 iMSys rollout datum
    pub imesys_rollout_datum: Option<Date>,
    // §3 Nr. 37: Pilotwindenergieanlage — §51 carve-out under every Fassung
    pub ist_pilotwindanlage: bool,
    // §100: date the Solarspitzengesetz opt-in was declared (Textform to the NB)
    pub sect51_optin_erklaert_am: Option<Date>,
    // §21c notification tracking
    #[serde(with = "time::serde::rfc3339::option")]
    pub veraeusserungsform_notification_sent_at: Option<OffsetDateTime>,
    /// When the 180-day Förderende alert was emitted; NULL until it is.
    #[serde(with = "time::serde::rfc3339::option")]
    pub foerderung_alert_sent_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

fn default_mastr_true() -> bool {
    true
}

fn default_sect9_keine() -> String {
    "KEINE".to_owned()
}

impl AnlageRow {
    /// How this plant satisfies §9, as a typed value.
    ///
    /// An unrecognised token reads as `Keine` — the conservative direction, since
    /// claiming compliance the registry cannot name would suppress a real §52
    /// Abs. 1 Nr. 1 charge.
    #[must_use]
    pub fn sect9_erfuellung(&self) -> eeg_billing::settlement_state::Sect9Erfuellung {
        use eeg_billing::settlement_state::Sect9Erfuellung as S;
        match self.sect9_erfuellung.as_str() {
            "FERNSTEUERBARKEIT" => S::Fernsteuerbarkeit,
            "LEISTUNGSBEGRENZUNG_60" => S::Leistungsbegrenzung60,
            _ => S::Keine,
        }
    }
}

pub async fn upsert_anlage(
    pool: &PgPool,
    tenant: &str,
    req: AnlageUpsertRequest,
) -> anyhow::Result<()> {
    // Refuse a registration the settlement could not honestly act on — most of
    // all a Marktprämie model with no anzulegender Wert, which would settle to
    // EUR 0 every month and emit a payout event for it.
    crate::validate::check(&req).map_err(|e| anyhow::anyhow!("{e}"))?;

    use time::format_description::well_known::Iso8601;
    let inbetriebnahme =
        Date::parse(&req.inbetriebnahme, &Iso8601::DEFAULT).context("parse inbetriebnahme")?;

    let repowering_datum = req
        .repowering_datum
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse repowering_datum")?;

    let ursprungs_inbetriebnahme = req
        .ursprungs_inbetriebnahme
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse ursprungs_inbetriebnahme")?;

    let mastr_datum = req
        .mastr_datum
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse mastr_datum")?;

    let sect51_optin_erklaert_am = req
        .sect51_optin_erklaert_am
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse sect51_optin_erklaert_am")?;

    let fernsteuerbarkeit_datum = req
        .fernsteuerbarkeit_datum
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse fernsteuerbarkeit_datum")?;

    let zuschlag_erloeschen_datum = req
        .zuschlag_erloeschen_datum
        .as_deref()
        .map(|s| Date::parse(s, &Iso8601::DEFAULT))
        .transpose()
        .context("parse zuschlag_erloeschen_datum")?;

    let ist_repowering = req.ist_repowering.unwrap_or(false);

    // ── § 8 Abs. 1–3 KWKG — Vollbenutzungsstunden ───────────────────────────
    // Stated explicitly or derived from the Anlagenart and the Kostenanteil.
    let kwk_anlagenart = req
        .kwk_anlagenart
        .as_deref()
        .map(parse_kwk_anlagenart)
        .transpose()?;
    let kwk_vollbenutzungsstunden: Option<i32> = req.kwk_foerderdauer_h.or_else(|| {
        kwk_anlagenart.and_then(|anlagenart| {
            eeg_billing::kwkg::foerderdauer_vollbenutzungsstunden(
                &eeg_billing::KwkFoerderdauerInput {
                    anlagenart,
                    kostenanteil: req.kwk_kostenanteil,
                    jahre_seit_dauerbetrieb: req.kwk_jahre_seit_dauerbetrieb,
                    ist_dampfsammelschiene_ueber_50_mw: req
                        .kwk_ist_dampfsammelschiene_ueber_50_mw
                        .unwrap_or(false),
                },
            )
            .and_then(|h| i32::try_from(h).ok())
        })
    });

    // ── foerderendedatum ────────────────────────────────────────────────────
    //
    // §25 Abs. 1 EEG 2023: 20 years, extended to 31 December of the twentieth
    // year where the anzulegender Wert is *gesetzlich bestimmt*. A plant whose
    // AW came out of a BNetzA tender does not get Satz 2 and ends on the exact
    // anniversary. A Vollrepowering is a fresh Inbetriebnahme (§3 Nr. 30) and
    // restarts the clock.
    //
    // KWKG is a different statute: § 8 caps the Zuschlag of *every* KWK plant in
    // Vollbenutzungsstunden (Abs. 1–3) and caps each calendar year separately
    // („pro Kalenderjahr […] für **bis zu**", Abs. 4). Both are counters against
    // generation, and Abs. 4's is a ceiling on a year rather than a rate the
    // plant is assumed to draw at — a plant running fewer hours than the cap
    // simply takes longer. No calendar date follows from either, so a KWK plant
    // gets none: the column stays NULL and `kwk_max_kwh` ends the Zuschlag.
    let is_ausschreibung = req.ausschreibungs_zuschlag_id.is_some();
    let zuschlag_datum = req
        .zuschlag_datum
        .as_deref()
        .map(|d| Date::parse(d, &time::format_description::well_known::Iso8601::DATE))
        .transpose()
        .map_err(|e| anyhow::anyhow!("zuschlag_datum: {e}"))?;
    let foerderendedatum: Option<Date> = if kwk_vollbenutzungsstunden.is_some() {
        None
    } else if ist_repowering {
        let basis = repowering_datum.unwrap_or(inbetriebnahme);
        Some(
            eeg_billing::foerderendedatum_repowering(basis)
                .context("compute repowering foerderendedatum")?,
        )
    } else if is_ausschreibung {
        Some(
            eeg_billing::foerderendedatum_eeg_ausschreibung(inbetriebnahme)
                .context("compute tender foerderendedatum")?,
        )
    } else {
        Some(
            eeg_billing::foerderendedatum_eeg(inbetriebnahme)
                .context("compute statutory foerderendedatum")?,
        )
    };

    let settlement_model = if req.direktvermarktung.unwrap_or(false) {
        "DIREKTVERMARKTUNG"
    } else {
        &req.settlement_model
    };

    sqlx::query(
        r"INSERT INTO eeg_anlagen (
               tr_id, tenant, malo_id, melo_id, eeg_gesetz, inbetriebnahme,
               leistung_kwp, erzeugungsart, verguetungssatz_ct, verguetungsform, foerderendedatum,
               direktvermarktung, direktverm_aw_ct, direktverm_mp_id,
               settlement_model, mieter_zuschlag_ct, ausschreibungs_zuschlag_id,
               ist_repowering, ursprungs_inbetriebnahme, repowering_datum,
               parent_tr_id,
               kwk_foerderdauer_h, kwk_anlagenart, kwk_verwendung, kwk_kostenanteil,
               kwk_bmwk_feststellung,
               flex_leistung_kw, flex_praemie_ct_kwh,
               mastr_registriert, mastr_nummer, mastr_datum,
               einspeiser_id,
               notes, is_biogas_sect51b, zuschlag_erloeschen_datum,
               biomasse_hauptbrennstoff, biomasse_guelle_anteil, biomasse_getreide_mais_anteil,
               zuschlagswert_ct, zuschlag_datum,
               ist_innovationsausschreibung, ist_buergerenergie, ist_pilotwindanlage,
               sect51_optin_erklaert_am, sect9_erfuellung, fernsteuerbarkeit_datum,
               updated_at
           ) VALUES (
               $1, $2, $3, $4, $5, $6,
               $7, $8, $9, $40, $10,
               $11, $12, $13, $14, $15, $16,
               $17, $18, $19,
               $20,
               $21, $22, $44, $45,
               $46,
               $23, $24,
               $25, $26, $27,
               $28,
               $29, $30, $31,
               $32, $33, $34,
               $35, $36,
               $37, $38, $39,
               $41, $42, $43, now()
           )
           ON CONFLICT (tr_id, tenant) DO UPDATE SET
               malo_id                   = EXCLUDED.malo_id,
               melo_id                   = EXCLUDED.melo_id,
               eeg_gesetz                = EXCLUDED.eeg_gesetz,
               inbetriebnahme            = EXCLUDED.inbetriebnahme,
               leistung_kwp              = EXCLUDED.leistung_kwp,
               erzeugungsart             = EXCLUDED.erzeugungsart,
               verguetungssatz_ct        = EXCLUDED.verguetungssatz_ct,
               verguetungsform           = EXCLUDED.verguetungsform,
               foerderendedatum          = EXCLUDED.foerderendedatum,
               direktvermarktung         = EXCLUDED.direktvermarktung,
               direktverm_aw_ct          = EXCLUDED.direktverm_aw_ct,
               direktverm_mp_id          = EXCLUDED.direktverm_mp_id,
               settlement_model          = EXCLUDED.settlement_model,
               mieter_zuschlag_ct        = EXCLUDED.mieter_zuschlag_ct,
               ausschreibungs_zuschlag_id = EXCLUDED.ausschreibungs_zuschlag_id,
               zuschlagswert_ct          = EXCLUDED.zuschlagswert_ct,
               zuschlag_datum            = EXCLUDED.zuschlag_datum,
               ist_innovationsausschreibung = EXCLUDED.ist_innovationsausschreibung,
               ist_buergerenergie        = EXCLUDED.ist_buergerenergie,
               ist_pilotwindanlage       = EXCLUDED.ist_pilotwindanlage,
               sect51_optin_erklaert_am  = EXCLUDED.sect51_optin_erklaert_am,
               sect9_erfuellung          = EXCLUDED.sect9_erfuellung,
               fernsteuerbarkeit_datum   = EXCLUDED.fernsteuerbarkeit_datum,
               ist_repowering            = EXCLUDED.ist_repowering,
               ursprungs_inbetriebnahme  = EXCLUDED.ursprungs_inbetriebnahme,
               repowering_datum          = EXCLUDED.repowering_datum,
               parent_tr_id              = EXCLUDED.parent_tr_id,
               kwk_foerderdauer_h        = EXCLUDED.kwk_foerderdauer_h,
               kwk_anlagenart            = EXCLUDED.kwk_anlagenart,
               kwk_verwendung            = EXCLUDED.kwk_verwendung,
               kwk_bmwk_feststellung     = EXCLUDED.kwk_bmwk_feststellung,
               kwk_kostenanteil          = EXCLUDED.kwk_kostenanteil,
               flex_leistung_kw          = EXCLUDED.flex_leistung_kw,
               flex_praemie_ct_kwh       = EXCLUDED.flex_praemie_ct_kwh,
               mastr_registriert         = EXCLUDED.mastr_registriert,
               mastr_nummer              = COALESCE(EXCLUDED.mastr_nummer, eeg_anlagen.mastr_nummer),
               mastr_datum               = COALESCE(EXCLUDED.mastr_datum, eeg_anlagen.mastr_datum),
               einspeiser_id             = EXCLUDED.einspeiser_id,
               notes                     = EXCLUDED.notes,
               is_biogas_sect51b         = EXCLUDED.is_biogas_sect51b,
               zuschlag_erloeschen_datum = EXCLUDED.zuschlag_erloeschen_datum,
               biomasse_hauptbrennstoff  = EXCLUDED.biomasse_hauptbrennstoff,
               biomasse_guelle_anteil    = EXCLUDED.biomasse_guelle_anteil,
               biomasse_getreide_mais_anteil = EXCLUDED.biomasse_getreide_mais_anteil,
               updated_at                = now()",
    )
    .bind(&req.tr_id)
    .bind(tenant)
    .bind(&req.malo_id)
    .bind(&req.melo_id)
    .bind(req.eeg_gesetz)
    .bind(inbetriebnahme)
    .bind(req.leistung_kwp)
    .bind(&req.erzeugungsart)
    .bind(req.verguetungssatz_ct)
    .bind(foerderendedatum)
    .bind(req.direktvermarktung.unwrap_or(false))
    .bind(req.direktverm_aw_ct)
    .bind(&req.direktverm_mp_id)
    .bind(settlement_model)
    .bind(req.mieter_zuschlag_ct)
    .bind(&req.ausschreibungs_zuschlag_id)
    .bind(ist_repowering)
    .bind(ursprungs_inbetriebnahme)
    .bind(repowering_datum)
    .bind(&req.parent_tr_id)
    .bind(kwk_vollbenutzungsstunden)
    .bind(&req.kwk_anlagenart)
    .bind(req.flex_leistung_kw)
    .bind(req.flex_praemie_ct_kwh)
    .bind(req.mastr_registriert)
    .bind(&req.mastr_nummer)
    .bind(mastr_datum)
    .bind(&req.einspeiser_id)
    .bind(&req.notes)
    .bind(req.is_biogas_sect51b)
    .bind(zuschlag_erloeschen_datum)
    .bind(&req.biomasse_hauptbrennstoff)
    .bind(req.biomasse_guelle_anteil)
    .bind(req.biomasse_getreide_mais_anteil)
    .bind(req.zuschlagswert_ct)
    .bind(zuschlag_datum)
    .bind(req.ist_innovationsausschreibung.unwrap_or(false))
    .bind(req.ist_buergerenergie.unwrap_or(false))
    .bind(req.ist_pilotwindanlage) // $39
    .bind(&req.verguetungsform) // $40
    .bind(sect51_optin_erklaert_am) // $41
    .bind(&req.sect9_erfuellung) // $42
    .bind(fernsteuerbarkeit_datum) // $43
    .bind(&req.kwk_verwendung) // $44
    .bind(req.kwk_kostenanteil) // $45
    .bind(req.kwk_bmwk_feststellung.unwrap_or(false)) // $46
    .execute(pool)
    .await
    .map_err(|e| {
        // The only foreign key on this table is the operator. Naming the field
        // beats leaking the constraint name to an API client.
        if e.as_database_error()
            .and_then(sqlx::error::DatabaseError::constraint)
            == Some("fk_anlage_einspeiser")
        {
            anyhow::anyhow!(
                "einspeiser_id {:?} is not a registered Anlagenbetreiber",
                req.einspeiser_id
            )
        } else {
            anyhow::Error::new(e)
        }
    })
    .context("upsert eeg_anlage")?;

    // ── Auto-set mastr_violation_start on first registration without MaStR ──
    // §52 Abs. 1 Nr. 11 EEG 2023: penalty accrues from when the NB registers
    // the plant and notes the missing MaStR entry. Set the start date to today
    // (using heute()) only when the column is NULL (not already tracking).
    if !req.mastr_registriert {
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET mastr_violation_start = COALESCE(mastr_violation_start, heute())
              WHERE tr_id = $1 AND tenant = $2 AND mastr_violation_start IS NULL",
        )
        .bind(&req.tr_id)
        .bind(tenant)
        .execute(pool)
        .await
        .context("set mastr_violation_start")?;
    } else {
        // Plant registered with MaStR confirmed: clear any outstanding violation start.
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET mastr_violation_start = NULL
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&req.tr_id)
        .bind(tenant)
        .execute(pool)
        .await
        .context("clear mastr_violation_start")?;
    }
    Ok(())
}

pub async fn fetch_anlage_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    tr_id: &str,
) -> anyhow::Result<Option<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>("SELECT * FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2")
        .bind(tr_id)
        .bind(tenant)
        .fetch_optional(&mut *conn)
        .await
        .context("fetch plant")
}

pub async fn fetch_anlage(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
) -> anyhow::Result<Option<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>("SELECT * FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2")
        .bind(tr_id)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .context("fetch eeg_anlage")
}

#[derive(Debug, Deserialize)]
pub struct AnlagenQuery {
    pub malo_id: Option<String>,
    pub erzeugungsart: Option<String>,
    pub settlement_model: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_anlagen(
    pool: &PgPool,
    tenant: &str,
    q: &AnlagenQuery,
) -> anyhow::Result<Vec<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>(
        r"SELECT * FROM eeg_anlagen
          WHERE tenant = $1
            AND ($2::text IS NULL OR malo_id = $2)
            AND ($3::text IS NULL OR erzeugungsart = $3)
            AND ($4::text IS NULL OR settlement_model = $4)
            AND ($5::text IS NULL OR status = $5)
          ORDER BY foerderendedatum ASC
          LIMIT $6",
    )
    .bind(tenant)
    .bind(&q.malo_id)
    .bind(&q.erzeugungsart)
    .bind(&q.settlement_model)
    .bind(q.status.as_deref().or(Some("aktiv")))
    .bind(q.limit.unwrap_or(200).min(2000))
    .fetch_all(pool)
    .await
    .context("list eeg_anlagen")
}

/// Plants whose `foerderendedatum` is within `horizon_days` of today.
pub async fn list_expiring(
    pool: &PgPool,
    tenant: &str,
    horizon_days: i32,
) -> anyhow::Result<Vec<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>(
        r"SELECT * FROM eeg_anlagen
          WHERE tenant = $1
            AND status = 'aktiv'
            AND foerderendedatum BETWEEN heute() AND heute() + ($2 * INTERVAL '1 day')
          ORDER BY foerderendedatum ASC",
    )
    .bind(tenant)
    .bind(horizon_days)
    .fetch_all(pool)
    .await
    .context("list_expiring")
}

/// Expiring plants that have not been alerted yet.
///
/// The alert worker sweeps every six hours over a 180-day window, so without
/// this filter each expiring plant produced hundreds of identical CloudEvents.
/// `GET /api/v1/anlagen/foerderung-auslaufend` deliberately keeps the unfiltered
/// view — a dashboard wants the whole window, an event stream wants the edge.
pub async fn list_expiring_unalerted(
    pool: &PgPool,
    tenant: &str,
    horizon_days: i32,
) -> anyhow::Result<Vec<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>(
        r"SELECT * FROM eeg_anlagen
          WHERE tenant = $1
            AND status = 'aktiv'
            AND foerderung_alert_sent_at IS NULL
            AND foerderendedatum BETWEEN heute() AND heute() + ($2 * INTERVAL '1 day')
          ORDER BY foerderendedatum ASC",
    )
    .bind(tenant)
    .bind(horizon_days)
    .fetch_all(pool)
    .await
    .context("list_expiring_unalerted")
}

/// Record that the Förderende alert has been emitted for a plant.
pub async fn mark_foerderung_alert_sent(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE eeg_anlagen SET foerderung_alert_sent_at = now() \
         WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(tr_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("mark_foerderung_alert_sent")?;
    Ok(())
}

pub async fn decommission_anlage(pool: &PgPool, tenant: &str, tr_id: &str) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        "UPDATE eeg_anlagen SET status = 'abgemeldet', updated_at = now() \
         WHERE tr_id = $1 AND tenant = $2 AND status = 'aktiv'",
    )
    .bind(tr_id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("decommission_anlage")?;
    Ok(rows.rows_affected() > 0)
}

/// Why and what a correction settlement supersedes (§ 147 AO / GoBD).
#[derive(Debug, Clone)]
pub struct Korrektur {
    /// The receipt being superseded. `None` when the period has none yet — the
    /// correction is still recorded as one, because the operator asked for it.
    pub original_id: Option<uuid::Uuid>,
    /// The statutory reason class, forwarded to the settlement engine so the
    /// audit positions are labelled as a correction.
    pub reason: eeg_billing::scheme::CorrectionReason,
    /// Free-text detail for the audit trail.
    pub detail: Option<String>,
}

impl Korrektur {
    /// The reason as stored in `settlement_receipts.correction_reason`.
    #[must_use]
    pub fn reason_text(&self) -> String {
        match &self.detail {
            Some(d) => format!("{:?}: {d}", self.reason),
            None => format!("{:?}", self.reason),
        }
    }
}

/// Input for a monthly settlement calculation.
pub struct SettleInput {
    pub tr_id: String,
    pub tenant: String,
    /// Marktlokation of the plant — the Anlagenbetreiber, recipient of the §14 UStG
    /// Gutschrift (the Gutschriftverfahren has the NB *issue* the document).
    pub malo_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    pub einspeisemenge_kwh: Option<Decimal>,
    pub epex_avg_ct_kwh: Option<Decimal>,
    pub settlement_model: String,
    pub verguetungssatz_ct: Decimal,
    pub direktverm_aw_ct: Option<Decimal>,
    pub mieter_zuschlag_ct: Option<Decimal>,
    pub flex_praemie_ct_kwh: Option<Decimal>,
    pub kwk_strom_kwh_gesamt: Option<Decimal>,
    pub kwk_max_kwh: Option<Decimal>,
    /// § 8 Abs. 4 KWKG — kWh already paid the Zuschlag on in `kwk_kwh_jahr_year`.
    pub kwk_kwh_jahr: Option<Decimal>,
    /// The calendar year `kwk_kwh_jahr` tracks.
    pub kwk_kwh_jahr_year: Option<i16>,
    /// § 6 Abs. 1 KWKG — the plant's class, as stored.
    pub kwk_anlagenart: Option<String>,
    /// § 7 KWKG — what the KWK-Strom is used for, as stored.
    pub kwk_verwendung: Option<String>,
    /// § 7 Abs. 1 Satz 2 KWKG — whether the BMWK Feststellung is published.
    pub kwk_bmwk_feststellung: bool,
    /// Derived from `mastr_registriert` in `run_settlement` — not set by caller.
    pub sanktion: Option<eeg_billing::SanktionAlt>,
    pub kwh_during_negative_epex: Option<Decimal>,
    /// Plant commissioning date — forwarded to `eeg-billing` for §51 EEG guard.
    pub inbetriebnahme: Option<Date>,
    /// Installed peak power — used for §51 threshold check (≥100 kWp) and auto Managementprämie.
    pub leistung_kwp: Option<Decimal>,
    /// EEG subsidy end date — triggers automatic `FoerderungBeendet` when billing_date > foerderendedatum.
    pub foerderendedatum: Option<Date>,
    /// First day of the billing month — supplied for FoerderungBeendet auto-detection.
    pub billing_date: Option<Date>,
    /// EEG law year (e.g. 2017, 2021, 2023, 0 for KWKG) — determines version-specific
    /// §51 Negativpreisregel threshold and kW exemption.
    pub eeg_gesetz: i16,
    /// Plant technology type for §51 EEG 2017 kW exemption dispatch.
    pub erzeugungsart: String,
    /// Operator's declared VAT status — decides the feed-in Gutschrift USt
    /// (§19 Kleinunternehmer `E`/0 % vs. Regelbesteuerung `S`/19 %). Read from
    /// `einspeiser.ust_status`: the § 19 election is made by the person, so it is
    /// never per plant and never inferred from plant size.
    pub vat_status: eeg_billing::ust::VatStatus,
    /// Whether the plant is registered in MaStR (Marktstammdatenregister).
    ///
    /// - `false` + EEG 2023  → Pflichtzahlung €10/kW/month (§52 Abs. 1 Nr. 11 EEG 2023)
    /// - `false` + EEG ≤2021 → `sanktion = Some(VerguetungAufNull)` (Vergütung = 0, old §47/§52 via §100)
    pub mastr_registriert: bool,
    /// §36h EEG — certified wind onshore Korrekturfaktor from the plant DB record.
    /// Forwarded directly to `eeg-billing` for MarketPremium wind plants.
    pub wind_korrekturfaktor: Option<Decimal>,
    /// §9 EEG — how the plant satisfies the Steuerbarkeit requirement.
    pub sect9_erfuellung: eeg_billing::settlement_state::Sect9Erfuellung,
    /// Whether this is a §51b biogas Ausschreibungsanlage.
    pub is_biogas_sect51b: bool,
    /// §52 Abs. 1 EEG 2023 — every violation this plant is in for the period,
    /// derived by [`crate::sect52::derive_pflichtverstoesse`].
    ///
    /// Ignored for plants under the pre-2023 regime, where a breach reduces the
    /// Vergütung itself rather than charging a separate Pflichtzahlung.
    pub pflichtverstoesse: Vec<eeg_billing::Pflichtverstoss>,
    /// §21 Abs. 1 Satz 1 Nr. 3 — how long the plant has been on the
    /// Ausfallvergütung, including this period. `run_settlement` re-derives the
    /// §52 Abs. 1 violations under its own transaction and needs this to answer
    /// Nr. 5, which no plant column carries.
    pub ausfallverguetung_nutzung: crate::sect52::AusfallverguetungNutzung,
    /// §36e/§37e/§39e EEG 2023 — whether the Zuschlag has lapsed for this period.
    ///
    /// Derived from `zuschlag_erloeschen_datum` against the billing month rather
    /// than stored: the flag it replaced was read by the settlement and written by
    /// nothing, so the branch that stops settling a lapsed award never ran.
    pub award_expired: bool,
    /// BNetzA Zuschlag-ID for an Ausschreibungsanlage.
    pub ausschreibungs_zuschlag_id: Option<String>,
    /// §22 EEG 2023 — the awarded anzulegender Wert (ct/kWh).
    pub zuschlagswert_ct: Option<Decimal>,
    /// Date of the BNetzA award notification.
    pub zuschlag_datum: Option<Date>,
    /// Anlage 1 Nr. 2 Satz 3 EEG 2023 — the plant claims under the § 19
    /// Abs. 3b/3c Abgrenzungs- oder Pauschaloption, which puts it on the
    /// Jahresmarktwert whatever its Inbetriebnahmedatum.
    pub speicher_abgrenzungs_oder_pauschaloption: bool,
    /// §39n EEG 2023 — Innovationsausschreibung.
    pub ist_innovationsausschreibung: bool,
    /// §22b EEG 2023 — Bürgerenergiegesellschaft (§3 Nr. 15).
    pub ist_buergerenergie: bool,
    /// §24 capacity blocks JSONB — deserialized in run_settlement.
    pub capacity_blocks_json: Option<serde_json::Value>,
    /// §13a EnWG (Redispatch 2.0) — kWh curtailed by NB; NB must compensate at AW rate.
    pub einspeisemanagement_kwh: Option<Decimal>,
    /// §51a EEG 2023 — quarter-hours during negative-price periods for Verlängerungsanspruch.
    pub negative_price_quarter_hours: Option<u64>,
    /// § 147 AO / GoBD — set when this run supersedes an earlier receipt.
    ///
    /// When `Some`, `run_settlement` inserts a new row with
    /// `is_correction = true` *beside* the original, which stays live and
    /// unchanged in `settlement_receipts`.
    ///
    /// It writes **no** `settlement_receipt_history` snapshot: a correction
    /// misses the partial unique index and overwrites nothing, so there is no
    /// row about to be lost. Only an *initial* settle snapshots, because its
    /// upsert replaces the existing row in place.
    pub correction: Option<Korrektur>,
    /// §44b Abs. 1 EEG 2023 — Biogas >100kW: eligible kWh for this billing period.
    /// Caller tracks cumulative annual kWh and passes `min(kwh, remaining_annual_quota)`.
    /// `None` = cap does not apply.
    pub biogas_sect44b_eligible_kwh: Option<Decimal>,
    /// Anlage 1 Nr. 3/4 EEG 2023 — technology-specific Jahresmarktwert.
    /// Alternative to `epex_avg_ct_kwh` for MarketPremium. `None` = auto-fetch.
    pub jahresmarktwert_ct_kwh: Option<Decimal>,
    /// §44b: year-to-date Einspeisemenge for the Biogas annual quota (from AnlageRow).
    pub biogas_quota_kwh_ytd: Decimal,
    /// §44b: calendar year the biogas_quota_kwh_ytd tracks (None = never settled).
    pub biogas_quota_ytd_year: Option<i16>,
    /// §51 Abs. 2 Nr. 1 EEG 2023: date iMSys was installed (None = not yet rolled out).
    pub imesys_rollout_datum: Option<Date>,
    /// §3 Nr. 37 EEG 2023: Pilotwindenergieanlage — exempt from §51 at any size.
    pub ist_pilotwindanlage: bool,
    /// §51 Abs. 3 EEG: calendar days of an unreported negative-price period on
    /// the Ausfallvergütung. Zero when the figure was established.
    pub sect51_abs3_unreported_days: u32,
    /// §100 EEG: when the Solarspitzengesetz opt-in takes effect, if it does.
    pub sect51_optin_wirksam_ab: Option<Date>,
    /// §3 EEG 2023: plant lifecycle type (Erstinbetriebnahme / Wiederinbetriebnahme / Repowering …).
    /// Stored as TEXT in `eeg_anlagen.inbetriebnahme_typ`; `None` = Erstinbetriebnahme.
    pub inbetriebnahme_typ: Option<String>,

    /// §§42–44 EEG 2023 — Biomass fuel composition for settlement enforcement.
    ///
    /// Derived from the three typed columns in `eeg_anlagen`.
    /// `None` for non-biomass plants (solar, wind, KWKG, hydro …).
    ///
    /// When `Some`, the settlement engine passes this directly to
    /// [`eeg_billing::calculate_settlement`], which enforces:
    /// - **§ 39i Abs. 1**: a bezuschlagte plant over its Getreide-und-Mais
    ///   Höchstanteil → `KeinAnspruch` (EUR 0)
    /// - **§44 Güllekleinanlage**: `ist_guellebonusanlage` recorded in audit positions
    pub biomasse: Option<eeg_billing::biomasse::BiomassSettlementData>,
}

#[derive(Debug, Serialize)]
pub struct SettleResult {
    pub id: Uuid,
    pub tr_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    pub settlement_model: String,
    pub einspeisemenge_kwh: Option<Decimal>,
    pub settlement_eur: Option<Decimal>,
    pub status: String,
    /// §14 UStG Gutschrift number (present only for a billable settlement).
    pub gutschrift_nummer: Option<String>,
    /// USt shown on the Gutschrift (0 for §12 Abs. 3 / §19).
    pub gutschrift_steuer_eur: Option<Decimal>,
    /// Brutto (net + USt) on the Gutschrift.
    pub gutschrift_brutto_eur: Option<Decimal>,
    /// §52 Abs. 2 EEG 2023 — the **cumulative** Pflichtzahlung owed to the
    /// Netzbetreiber as of this period, not this month's increment.
    ///
    /// §52 charges „pro Kilowatt … und Kalendermonat", so the claim grows with
    /// every month a breach subsists and the figure on each receipt is the
    /// running total. Summing two receipts double-counts; the later one
    /// supersedes the earlier. It is a separate claim and is never netted into
    /// `settlement_eur` — §52 Abs. 6 Satz 2 permits the Aufrechnung, but that is
    /// a decision for the ledger, not for the settlement.
    pub pflichtzahlung_kumuliert_eur: Option<Decimal>,
}

// ── §44b quota computation ─────────────────────────────────────────────────────

/// Load the §§53b–54 facts in force on `billing_date` for one plant.
///
/// Only facts are read. Every deduction amount except §53c's is fixed by statute
/// and lives in `eeg_billing::aw_reductions`, so a wrong row cannot invent a
/// reduction the law does not provide for. §53c is the exception the statute
/// makes itself — it ties the cut to "die Höhe der pro Kilowattstunde gewährten
/// Stromsteuerbefreiung" — and the column is CHECK-bounded to the §3 StromStG
/// full rate.
async fn load_aw_reductions(
    conn: &mut sqlx::PgConnection,
    tr_id: &str,
    tenant: &str,
    billing_date: time::Date,
) -> anyhow::Result<eeg_billing::aw_reductions::AwReductionContext> {
    use eeg_billing::aw_reductions::{AwReductionContext, Sect54SolarReduction};

    let regionalnachweis_ausgestellt: bool = sqlx::query_scalar(
        r"SELECT EXISTS (
            SELECT 1 FROM eeg_regionalnachweise
             WHERE tr_id = $1 AND tenant = $2
               AND effective_from <= $3
               AND (effective_until IS NULL OR effective_until >= $3))",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_date)
    .fetch_one(&mut *conn)
    .await
    .context("load §53b Regionalnachweis periods")?;

    let stromsteuerbefreiung_ct_kwh: Option<rust_decimal::Decimal> = sqlx::query_scalar(
        r"SELECT befreiung_ct_kwh FROM eeg_stromsteuerbefreiungen
           WHERE tr_id = $1 AND tenant = $2
             AND effective_from <= $3
             AND (effective_until IS NULL OR effective_until >= $3)
           ORDER BY effective_from DESC LIMIT 1",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_date)
    .fetch_optional(&mut *conn)
    .await
    .context("load §53c Stromsteuerbefreiung")?;

    // Defects of the same period are unioned: each Absatz is independent, and a
    // plant can carry more than one at once.
    let s54: Option<(bool, bool, bool, bool)> = sqlx::query_as(
        r"SELECT COALESCE(bool_or(zahlungsberechtigung_nach_18_monaten), FALSE),
                 COALESCE(bool_or(flurstueck_abweichung), FALSE),
                 COALESCE(bool_or(agri_nutzungsnachweis_fehlt), FALSE),
                 COALESCE(bool_or(landesverordnung_nicht_erfuellt), FALSE)
            FROM eeg_sect54_solar_defekte
           WHERE tr_id = $1 AND tenant = $2
             AND effective_from <= $3
             AND (effective_until IS NULL OR effective_until >= $3)",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_date)
    .fetch_optional(&mut *conn)
    .await
    .context("load §54 solar defects")?;

    // An un-grouped aggregate always returns one row, and `bool_or` over an empty
    // set is NULL — hence the COALESCE. "No matching rows" and "rows that record
    // nothing" therefore both arrive as all-false, and `is_clean` maps both to
    // no §54 context, which is the same answer either way.
    let sect54_solar = s54
        .map(|(a1, a2, a3, a4)| Sect54SolarReduction {
            zahlungsberechtigung_nach_18_monaten: a1,
            flurstueck_abweichung: a2,
            agri_nutzungsnachweis_fehlt: a3,
            landesverordnung_nicht_erfuellt: a4,
        })
        .filter(|s| !s.is_clean());

    Ok(AwReductionContext {
        regionalnachweis_ausgestellt,
        stromsteuerbefreiung_ct_kwh,
        sect54_solar,
    })
}

/// Compute the §44b eligible kWh for a Biogas plant billing period.
///
/// §44b Abs. 1 EEG 2023: fermentation-Biogas plants >100 kW (excl. §39 Ausschreibung)
/// are paid the full rate only for the share of a calendar year's generation whose
/// **Bemessungsleistung** equals 45 % of the installed capacity. Excess kWh receive:
/// - MarketPremium: AW = 0, Marktprämie = 0
/// - FeedInTariff: paid at EPEX Marktwert
///
/// Returns `None` when the cap does not apply to this plant.
/// Returns `Some(eligible_kwh)` = max(0, annual_quota − ytd_before_this_period).
async fn compute_biogas_sect44b_eligible(
    conn: &mut sqlx::PgConnection,
    input: &SettleInput,
) -> anyhow::Result<Option<Decimal>> {
    use rust_decimal::dec;

    // §44b applies only to: fermentation Biogas, >100 kW, not §51b Ausschreibung
    let is_applicable = input.erzeugungsart == "BIOGAS"
        && input.leistung_kwp.is_some_and(|kw| kw > dec!(100))
        && !input.is_biogas_sect51b;

    if !is_applicable {
        return Ok(None);
    }

    // Reset YTD counter when entering a new calendar year
    let ytd = if input.biogas_quota_ytd_year == Some(input.billing_year) {
        input.biogas_quota_kwh_ytd
    } else {
        // New year: reset the counter atomically before settlement
        sqlx::query(
            "UPDATE eeg_anlagen
             SET biogas_quota_kwh_ytd = 0, biogas_quota_ytd_year = $3
             WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(input.billing_year)
        .execute(&mut *conn)
        .await
        .context("reset biogas §44b YTD counter")?;
        Decimal::ZERO
    };

    let leistung_kw = input.leistung_kwp.unwrap_or(Decimal::ZERO);
    // §44b Abs. 1 i.V.m. §3 Nr. 6: the divisor is "die Summe der vollen
    // Zeitstunden des jeweiligen Kalenderjahres abzüglich der vollen Stunden vor
    // der erstmaligen Erzeugung" — not a flat 8 760. A leap year has 8 784, and a
    // plant that first generated during the year is measured against the rest of
    // it, so the flat figure under-credited every leap year and over-credited
    // every plant's first one.
    let annual_quota = eeg_billing::sect44b_jahreskontingent_kwh(
        leistung_kw,
        i32::from(input.billing_year),
        input.inbetriebnahme,
    );
    let remaining = (annual_quota - ytd).max(Decimal::ZERO);
    Ok(Some(remaining))
}

// ── Idempotent accrual of the cumulative counters ────────────────────────────

/// What one billing period contributes to the plant's running totals.
///
/// These counters (§44b quota, §51a Förderende extension, KWKG kWh limit) run
/// over the whole Förderdauer, while `POST /settle` is idempotent. Recording the
/// contribution per period is what lets a re-settle apply the difference instead
/// of the full amount a second time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeriodAccrual {
    /// §51a: raw negative-price quarter-hours.
    pub negative_price_qh: i64,
    /// §44b: kWh charged against the annual Biogas quota.
    pub biogas_kwh: Decimal,
    /// KWKG: kWh charged against the Zuschlag limit.
    pub kwk_kwh: Decimal,
}

impl PeriodAccrual {
    /// What still has to be applied to reach `self` from `previous`.
    #[must_use]
    pub fn delta_from(&self, previous: &Self) -> Self {
        Self {
            negative_price_qh: self.negative_price_qh - previous.negative_price_qh,
            biogas_kwh: self.biogas_kwh - previous.biogas_kwh,
            kwk_kwh: self.kwk_kwh - previous.kwk_kwh,
        }
    }

    /// `true` when nothing needs to be applied.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// The §51a quarter-hours this period has already been credited with.
async fn existing_period_qh(
    conn: &mut sqlx::PgConnection,
    tr_id: &str,
    tenant: &str,
    billing_year: i16,
    billing_month: i16,
) -> anyhow::Result<i64> {
    let qh: Option<i64> = sqlx::query_scalar(
        "SELECT negative_price_qh FROM settlement_period_accruals
          WHERE tr_id = $1 AND tenant = $2 AND billing_year = $3 AND billing_month = $4",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_year)
    .bind(billing_month)
    .fetch_optional(&mut *conn)
    .await
    .context("read prior period §51a accrual")?;
    Ok(qh.unwrap_or(0))
}

/// Record this period's contribution and return what still has to be applied.
///
/// The stored row is the period's *absolute* contribution, so the returned delta
/// is zero for an unchanged re-settle and carries only the change for a
/// correction — including a negative one when a period is re-settled lower.
async fn record_period_accrual(
    conn: &mut sqlx::PgConnection,
    tr_id: &str,
    tenant: &str,
    billing_year: i16,
    billing_month: i16,
    period: &PeriodAccrual,
) -> anyhow::Result<PeriodAccrual> {
    // The row is locked for the rest of the settlement transaction, so two
    // concurrent settles of the same period cannot both read the same baseline.
    let previous: Option<(i64, Decimal, Decimal)> = sqlx::query_as(
        "SELECT negative_price_qh, biogas_kwh, kwk_kwh
           FROM settlement_period_accruals
          WHERE tr_id = $1 AND tenant = $2
            AND billing_year = $3 AND billing_month = $4
          FOR UPDATE",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_year)
    .bind(billing_month)
    .fetch_optional(&mut *conn)
    .await
    .context("read period accrual")?;

    let previous =
        previous.map_or_else(PeriodAccrual::default, |(qh, biogas, kwk)| PeriodAccrual {
            negative_price_qh: qh,
            biogas_kwh: biogas,
            kwk_kwh: kwk,
        });

    sqlx::query(
        "INSERT INTO settlement_period_accruals
             (tr_id, tenant, billing_year, billing_month,
              negative_price_qh, biogas_kwh, kwk_kwh)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (tr_id, tenant, billing_year, billing_month) DO UPDATE
         SET negative_price_qh = EXCLUDED.negative_price_qh,
             biogas_kwh        = EXCLUDED.biogas_kwh,
             kwk_kwh           = EXCLUDED.kwk_kwh,
             updated_at        = now()",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_year)
    .bind(billing_month)
    .bind(period.negative_price_qh)
    .bind(period.biogas_kwh)
    .bind(period.kwk_kwh)
    .execute(&mut *conn)
    .await
    .context("record period accrual")?;

    Ok(period.delta_from(&previous))
}

/// Update the Biogas §44b year-to-date counter after a successful settlement.
async fn update_biogas_quota_ytd(
    conn: &mut sqlx::PgConnection,
    tr_id: &str,
    tenant: &str,
    billing_year: i16,
    kwh_settled: Decimal,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE eeg_anlagen
         SET biogas_quota_kwh_ytd  = GREATEST(biogas_quota_kwh_ytd + $4, 0),
             biogas_quota_ytd_year = $3
         WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_year)
    .bind(kwh_settled)
    .execute(&mut *conn)
    .await
    .context("update biogas §44b YTD counter")?;
    Ok(())
}

// ── KWKG §§ 7, 8 — typed plant attributes ────────────────────────────────────

/// Parse the `kwk_anlagenart` column into the § 6 Abs. 1 KWKG class.
///
/// # Errors
/// A value outside the three the statute knows.
fn parse_kwk_anlagenart(s: &str) -> anyhow::Result<eeg_billing::KwkAnlagenart> {
    use eeg_billing::KwkAnlagenart as A;
    match s {
        "NEU" => Ok(A::Neu),
        "MODERNISIERT" => Ok(A::Modernisiert),
        "NACHGERUESTET" => Ok(A::Nachgeruestet),
        other => Err(anyhow::anyhow!(
            "kwk_anlagenart {other:?} is not one of NEU, MODERNISIERT, NACHGERUESTET"
        )),
    }
}

/// Parse the `kwk_verwendung` column into the § 7 Absatz that prices the plant.
///
/// The column has no default. § 7 Abs. 1 and Abs. 2 price different claims —
/// 8 down to 3,4 ct for KWK-Strom fed into a Netz der allgemeinen Versorgung,
/// 5,41 down to 1 ct for KWK-Strom that is not — and which one a plant is on is
/// a fact about the plant. A NULL column computes no § 7 rate.
fn parse_kwk_verwendung(s: &str) -> anyhow::Result<eeg_billing::KwkVerwendung> {
    use eeg_billing::KwkVerwendung as V;
    match s {
        "NETZ_DER_ALLGEMEINEN_VERSORGUNG" => Ok(V::NetzDerAllgemeinenVersorgung),
        "NICHT_EINGESPEIST_BIS100KW" => Ok(V::NichtEingespeistBis100Kw),
        "NICHT_EINGESPEIST_KUNDENANLAGE" => Ok(V::NichtEingespeistKundenanlage),
        "NICHT_EINGESPEIST_STROMKOSTENINTENSIV" => Ok(V::NichtEingespeistStromkostenintensiv),
        "NICHT_EINGESPEIST_BRANCHE_ANLAGE2" => Ok(V::NichtEingespeistBrancheAnlage2),
        other => Err(anyhow::anyhow!("kwk_verwendung {other:?} is unknown")),
    }
}

// ── §§42–44 EEG 2023 Biomass fuel composition ────────────────────────────────

/// Derive [`eeg_billing::biomasse::BiomassSettlementData`] from the typed columns
/// stored in `eeg_anlagen`.
///
/// Returns `None` when the plant is not a biomass/biogas plant
/// (`biomasse_hauptbrennstoff` is NULL), so the settlement engine skips
/// §43/§44 enforcement entirely for non-biomass plants.
///
/// ## Settlement-engine rules driven by the returned struct
///
/// - **§ 39i Abs. 1 EEG 2023**: a plant holding a Zuschlag whose Getreide- und
///   Mais-Anteil exceeds the Höchstanteil for its Gebotstermin has no § 19
///   Abs. 1 claim, and the settlement returns `KeinAnspruch` (EUR 0).
/// - **§ 44 EEG 2023 Güllekleinanlage**: `ist_guellebonusanlage = true` is
///   recorded in the audit position label. The Güllekleinanlage
///   `verguetungssatz_ct` must be stored at registration time (§ 44 Abs. 1:
///   22 ct up to 75 kW, 19 ct up to 150 kW, less the § 53 Abs. 1 Nr. 1
///   deduction).
///
/// ## NULL fraction semantics
///
/// NULL fractions in the DB are treated as `0.0`:
/// - `biomasse_guelle_anteil IS NULL` → 0.0 Gülle share → no bonus
/// - `biomasse_getreide_mais_anteil IS NULL` → 0.0 share → § 39i is satisfied
///
/// This is the conservative/safe direction: operators who have not yet
/// submitted fuel composition data are neither denied their claim nor granted
/// the Güllekleinanlage bonus until they do.
fn derive_biomasse(anlage: &AnlageRow) -> Option<eeg_billing::biomasse::BiomassSettlementData> {
    use eeg_billing::biomasse::BiomassBrennstoff;

    let hauptbrennstoff_str = anlage.biomasse_hauptbrennstoff.as_deref()?;

    // Parse DB string → typed enum; any unrecognised value → conservatively
    // treat as PflanzlicheBiomasse (no § 44 bonus).
    let hauptbrennstoff = match hauptbrennstoff_str {
        "GUELLE" => BiomassBrennstoff::Guelle,
        "FESTMIST" => BiomassBrennstoff::Festmist,
        "HOLZBIOMASSE" => BiomassBrennstoff::Holzbiomasse,
        "KLAERGAS" => BiomassBrennstoff::Klaergas,
        "DEPONIEGAS" => BiomassBrennstoff::Deponiegas,
        "GRUBENGAS" => BiomassBrennstoff::Grubengas,
        "BIOMETHAN_AUS_BIOMASSE" => BiomassBrennstoff::BiomethanAusBiomasse,
        // PFLANZLICHE_BIOMASSE + BIOABFALL + any unrecognised value
        _ => BiomassBrennstoff::PflanzlicheBiomasse,
    };

    // NULL fractions → 0.0 (conservative: no bonus, and § 39i is satisfied)
    let guelle_anteil = anlage
        .biomasse_guelle_anteil
        .unwrap_or(rust_decimal::Decimal::ZERO);
    let getreide_mais_anteil = anlage
        .biomasse_getreide_mais_anteil
        .unwrap_or(rust_decimal::Decimal::ZERO);

    Some(eeg_billing::biomasse::BiomassSettlementData::new(
        hauptbrennstoff,
        guelle_anteil,
        getreide_mais_anteil,
        anlage.leistung_kwp,
        // § 39i Abs. 1 conditions only a claim „durch einen Zuschlag erworben",
        // and the Höchstanteil steps down by the Gebotstermin the plant was
        // awarded at.
        anlage.zuschlag_datum,
    ))
}

// ── Anlage 1 Marktwert fetch ──────────────────────────────────────────

/// What a Marktwert lookup found, and how binding it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarktwertTreffer {
    /// The figure in ct/kWh.
    pub avg_ct_kwh: Decimal,
    /// Which Anlage-1 series it came from.
    pub serie: eeg_billing::Marktwertserie,
    /// `true` when it is an ÜNB running estimate rather than the published
    /// binding figure — always `false` for a Monatsmarktwert, and for the EPEX
    /// fallback, which is not a Marktwert at all but is at least final.
    pub vorlaeufig: bool,
}

/// Fetch the energieträgerspezifische Marktwert a plant is entitled to
/// (Anlage 1 Nr. 2–4 EEG 2023).
///
/// `serie` is the plant's, not the operator's: [`eeg_billing::marktwertserie`]
/// derives it from the Inbetriebnahme and the Zuschlag date. Asking for the
/// wrong one is how two parties settle the same month at different figures.
///
/// Lookup order **within the requested series**:
/// 1. exact technology match in `marktwert_preise`
/// 2. the `DEFAULT` fallback row
/// 3. the generic EPEX monthly average the caller passes
/// 4. `None` — `PriceMissing`, which the monthly worker retries. That is the
///    honest answer for a post-2023 plant before the ÜNB have published
///    anything: substituting the other series would misprice every kWh.
///
/// # Errors
///
/// Database failures.
pub async fn fetch_marktwert(
    conn: &mut sqlx::PgConnection,
    billing_year: i16,
    billing_month: i16,
    serie: eeg_billing::Marktwertserie,
    erzeugungsart: &str,
    epex_fallback: Option<Decimal>,
) -> anyhow::Result<Option<MarktwertTreffer>> {
    // A Jahresmarktwert has no month, so the predicate differs by series rather
    // than the value bound to it.
    let monat = match serie {
        eeg_billing::Marktwertserie::Monatsmarktwert => Some(billing_month),
        eeg_billing::Marktwertserie::Jahresmarktwert => None,
    };
    let row: Option<(Decimal, bool)> = sqlx::query_as(
        "SELECT avg_ct_kwh, vorlaeufig FROM marktwert_preise
         WHERE billing_year = $1 AND art = $2
           AND billing_month IS NOT DISTINCT FROM $3
           AND erzeugungsart = ANY(ARRAY[$4, 'DEFAULT'])
         ORDER BY (erzeugungsart = $4) DESC
         LIMIT 1",
    )
    .bind(billing_year)
    .bind(serie.as_db_str())
    .bind(monat)
    .bind(erzeugungsart)
    .fetch_optional(&mut *conn)
    .await
    .context("fetch Marktwert")?;

    Ok(row
        .map(|(avg_ct_kwh, vorlaeufig)| MarktwertTreffer {
            avg_ct_kwh,
            serie,
            vorlaeufig,
        })
        .or_else(|| {
            epex_fallback.map(|avg_ct_kwh| MarktwertTreffer {
                avg_ct_kwh,
                serie,
                vorlaeufig: false,
            })
        }))
}

/// Override values callers can supply to `build_settle_input`.
///
/// Fields left `None` use plant-DB or handler-default values.
#[derive(Debug, Default)]
pub struct SettleOverrides {
    /// Explicit Einspeisemenge (overrides edmd auto-fetch).
    pub einspeisemenge_kwh: Option<Decimal>,
    /// Explicit EPEX / Jahresmarktwert ct/kWh (overrides DB lookup).
    pub epex_avg_ct_kwh: Option<Decimal>,
    /// §13a EnWG curtailed kWh for this billing period.
    pub einspeisemanagement_kwh: Option<Decimal>,
    /// §51 EEG — kWh fed in during negative-spot-price intervals this period.
    ///
    /// Drives the Negativpreisregel: the anzulegender Wert for these kWh is
    /// reduced to null (§51 Abs. 1 EEG 2023), version- and threshold-aware via
    /// the engine. `None` (not `Some(0)`) means "no negative-price data supplied"
    /// and leaves the settlement unreduced.
    pub kwh_during_negative_epex: Option<Decimal>,
    /// §51a quarter-hours during negative EPEX for this period.
    pub negative_price_quarter_hours: Option<u64>,
    /// § 147 AO / GoBD: set when this run supersedes an earlier receipt.
    ///
    /// One field rather than a loose id and a loose reason string, because a
    /// correction without a recorded reason is not one the audit trail can use —
    /// settlement receipts are Buchungsbelege with an eight-year retention under
    /// § 147 Abs. 3 AO and have to say what was corrected and why.
    pub correction: Option<Korrektur>,
    /// Anlage 1 technology-specific Marktwert (explicit override).
    pub jahresmarktwert_ct_kwh: Option<Decimal>,
    /// §51 Abs. 3 EEG — calendar days of an unreported negative-price period,
    /// for a plant on the Ausfallvergütung. Zero when the figure is known.
    pub sect51_abs3_unreported_days: u32,
    /// §21 Abs. 1 Satz 1 Nr. 3 — how long the plant has been on the
    /// Ausfallvergütung, including this period. Drives the §52 Abs. 1 Nr. 5
    /// Höchstdauer check; read from the receipts by
    /// [`crate::pg::ausfallverguetung_nutzung`].
    pub ausfallverguetung: crate::sect52::AusfallverguetungNutzung,
}

/// Build a [`SettleInput`] from a plant row and a billing period.
///
/// This single function is the authoritative mapping between the plant DB record
/// and the settlement engine input. All four settlement entry points
/// (single settle, batch settle, correction settle, MCP settle) use this
/// function so that any new field is automatically threaded everywhere.
///
/// `einspeiser` is the plant's operator. The Umsatzsteuerstatus of a Gutschrift
/// is a property of the invoicing party, not of the plant, so it is read from
/// the operator record and never inferred here.
///
/// # Errors
/// Returns an error when the operator carries an Umsatzsteuerstatus this build
/// does not know. Refusing is deliberate: guessing would put two VAT rates on
/// one operator's Gutschriften.
pub fn build_settle_input(
    tenant: &str,
    anlage: &AnlageRow,
    einspeiser: &crate::pg_einspeiser::Einspeiser,
    billing_year: i16,
    billing_month: i16,
    overrides: SettleOverrides,
) -> anyhow::Result<SettleInput> {
    let billing_date = time::Date::from_calendar_date(
        billing_year as i32,
        time::Month::try_from(billing_month as u8).unwrap_or(time::Month::January),
        1,
    )
    .ok();

    // §51a EEG 2023: move the Förderende forward by the accrued negative-price
    // extension so the plant keeps being paid past its statutory 20-year end.
    // Computed from the RAW cumulative QH (rounded once), technology-aware.
    let is_solar = eeg_billing::ErzeugungsArt::from_db_str(&anlage.erzeugungsart)
        .map(eeg_billing::ErzeugungsArt::is_solar)
        .unwrap_or(false);
    let effektives_foerderende = anlage.foerderendedatum.map(|ende| {
        eeg_billing::foerderdauer::effektives_foerderende(
            ende,
            u64::try_from(anlage.negative_price_qh_gesamt).unwrap_or(0),
            is_solar,
        )
        .unwrap_or(ende)
    });

    // §36h Abs. 2 EEG 2023: the Korrekturfaktor in effect for this billing period.
    // A Standortgüte re-evaluation at year 6/11/16 supersedes the initial factor.
    let wind_korrekturfaktor = match (anlage.wind_korrekturfaktor, billing_date) {
        (Some(initial), Some(bd)) => {
            let reevals: Vec<eeg_billing::wind::GuetefaktorReeval> = anlage
                .wind_guetefaktor_reevaluations
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(eeg_billing::wind::korrekturfaktor_fuer_periode(
                anlage.inbetriebnahme,
                bd,
                initial,
                &reevals,
            ))
        }
        _ => anlage.wind_korrekturfaktor,
    };

    // The Umsatzsteuerstatus is the operator's declared status. An unknown token
    // aborts the settlement rather than falling back to a plant-shaped guess.
    let vat_status = einspeiser.vat_status()?;

    Ok(SettleInput {
        tr_id: anlage.tr_id.clone(),
        tenant: tenant.to_owned(),
        malo_id: anlage.malo_id.clone(),
        billing_year,
        billing_month,
        einspeisemenge_kwh: overrides.einspeisemenge_kwh,
        epex_avg_ct_kwh: overrides.epex_avg_ct_kwh,
        settlement_model: anlage.settlement_model.clone(),
        verguetungssatz_ct: anlage.verguetungssatz_ct,
        // For a tender plant the **awarded** value is the anzulegender Wert.
        // The two columns exist so an award is never mistaken for a bilaterally
        // agreed rate, but the settlement only ever read `direktverm_aw_ct` — so a
        // plant registered with the field named after its award (`zuschlagswert_ct`,
        // which is what an operator reaches for) settled at AW = 0 and was paid
        // nothing, every month, as a `calculated` result.
        direktverm_aw_ct: if anlage.settlement_model == crate::models::AUSSCHREIBUNG {
            anlage.zuschlagswert_ct.or(anlage.direktverm_aw_ct)
        } else {
            anlage.direktverm_aw_ct
        },
        mieter_zuschlag_ct: anlage.mieter_zuschlag_ct,
        flex_praemie_ct_kwh: anlage.flex_praemie_ct_kwh,
        kwk_strom_kwh_gesamt: if anlage.settlement_model == KWKG_ZUSCHLAG {
            anlage.kwk_strom_kwh_gesamt
        } else {
            None
        },
        kwk_max_kwh: anlage
            .kwk_foerderdauer_h
            .map(|h| Decimal::from(h) * anlage.leistung_kwp),
        kwk_kwh_jahr: anlage.kwk_kwh_jahr,
        kwk_kwh_jahr_year: anlage.kwk_kwh_jahr_year,
        kwk_anlagenart: anlage.kwk_anlagenart.clone(),
        kwk_verwendung: anlage.kwk_verwendung.clone(),
        kwk_bmwk_feststellung: anlage.kwk_bmwk_feststellung,
        sanktion: None, // derived from mastr_registriert in run_settlement
        mastr_registriert: anlage.mastr_registriert,
        kwh_during_negative_epex: overrides.kwh_during_negative_epex,
        inbetriebnahme: Some(anlage.inbetriebnahme),
        leistung_kwp: Some(anlage.leistung_kwp),
        foerderendedatum: effektives_foerderende,
        billing_date,
        eeg_gesetz: anlage.eeg_gesetz,
        erzeugungsart: anlage.erzeugungsart.clone(),
        vat_status,
        wind_korrekturfaktor,
        sect9_erfuellung: anlage.sect9_erfuellung(),
        is_biogas_sect51b: anlage.is_biogas_sect51b,
        // Left empty on purpose. §52 Abs. 1 is derived from the plant record
        // **and** the `eeg_pflichtverstoesse` register, and this function is
        // synchronous, so it cannot read the register. `run_settlement` fills the
        // field under the same transaction — the way it refreshes every other
        // running total — so a caller cannot forget to and settle a plant at
        // one month's charge for a year-old breach.
        pflichtverstoesse: Vec::new(),
        ausfallverguetung_nutzung: overrides.ausfallverguetung,
        // §36e/§37e/§39e: the award has lapsed once its date has passed.
        award_expired: matches!(
            (anlage.zuschlag_erloeschen_datum, billing_date),
            (Some(erloeschen), Some(bd)) if bd >= erloeschen
        ),
        capacity_blocks_json: anlage.capacity_blocks.clone(),
        einspeisemanagement_kwh: overrides.einspeisemanagement_kwh,
        negative_price_quarter_hours: overrides.negative_price_quarter_hours,
        ausschreibungs_zuschlag_id: anlage.ausschreibungs_zuschlag_id.clone(),
        zuschlagswert_ct: anlage.zuschlagswert_ct,
        zuschlag_datum: anlage.zuschlag_datum,
        // Anlage 1 Nr. 2 Satz 3 — a storage claim under § 19 Abs. 3b/3c moves the
        // plant onto the Jahresmarktwert whatever its Inbetriebnahmedatum.
        speicher_abgrenzungs_oder_pauschaloption: anlage.speicher_option != "KEINE",
        ist_innovationsausschreibung: anlage.ist_innovationsausschreibung,
        ist_buergerenergie: anlage.ist_buergerenergie,
        correction: overrides.correction,
        biogas_sect44b_eligible_kwh: None, // computed by run_settlement from biogas_quota_kwh_ytd
        jahresmarktwert_ct_kwh: overrides.jahresmarktwert_ct_kwh,
        biogas_quota_kwh_ytd: anlage.biogas_quota_kwh_ytd,
        biogas_quota_ytd_year: anlage.biogas_quota_ytd_year,
        imesys_rollout_datum: anlage.imesys_rollout_datum,
        ist_pilotwindanlage: anlage.ist_pilotwindanlage,
        sect51_abs3_unreported_days: overrides.sect51_abs3_unreported_days,
        // §100 EEG: the declaration alone does nothing — it starts running at the
        // turn of the year after the plant's iMSys went in.
        sect51_optin_wirksam_ab: anlage.sect51_optin_erklaert_am.and_then(|erklaert| {
            eeg_billing::negativpreis::optin_wirksam_ab(erklaert, anlage.imesys_rollout_datum)
        }),
        inbetriebnahme_typ: anlage.inbetriebnahme_typ.clone(),
        // §§42–44 EEG 2023: derive biomass fuel composition from the three typed
        // DB columns. `None` for non-biomass plants.
        biomasse: derive_biomasse(anlage),
    })
}

/// The running totals [`refresh_cumulative_counters`] re-reads under the lock.
#[derive(Debug, sqlx::FromRow)]
struct LockedCounters {
    /// § 8 Abs. 1–3 KWKG — kWh already paid the Zuschlag on over the plant's life.
    kwk_strom_kwh_gesamt: Option<Decimal>,
    /// § 8 Abs. 4 KWKG — kWh already paid the Zuschlag on in `kwk_kwh_jahr_year`.
    kwk_kwh_jahr: Option<Decimal>,
    /// The calendar year `kwk_kwh_jahr` tracks.
    kwk_kwh_jahr_year: Option<i16>,
    /// §44b Abs. 1 EEG 2023 — kWh charged against this year's Biogas cap.
    biogas_quota_kwh_ytd: Decimal,
    /// The calendar year `biogas_quota_kwh_ytd` tracks.
    biogas_quota_ytd_year: Option<i16>,
    /// §51a EEG 2023 — raw negative-price quarter-hours over the Förderdauer.
    negative_price_qh_gesamt: i64,
    /// The statutory Förderende, before the §51a extension is applied to it.
    /// `None` for a plant that has none — a KWK plant, § 8 KWKG.
    foerderendedatum: Option<Date>,
}

/// Re-read the plant's cumulative counters under a row lock.
///
/// [`build_settle_input`] is pure and takes a plant row the caller fetched before
/// the settling transaction opened. Three of that row's values are running totals
/// the settlement both *reads* and *writes* — `kwk_strom_kwh_gesamt` (§8 KWKG
/// Vollbenutzungsstunden), `biogas_quota_kwh_ytd` (§44b) and
/// `negative_price_qh_gesamt` (§51a, via the effective Förderende) — so they are
/// re-read here and entitlement is never computed against a stale total.
///
/// This is the transaction's first statement, which makes it the only
/// serialisation point covering *every* settle of one plant: the receipt upsert
/// orders runs for the same month only, and a correction bypasses its partial
/// index.
///
/// A plant row that no longer exists leaves the input untouched — the settlement
/// engine decides what that means.
async fn refresh_cumulative_counters(
    conn: &mut sqlx::PgConnection,
    input: &mut SettleInput,
) -> anyhow::Result<()> {
    let row: Option<LockedCounters> = sqlx::query_as(
        "SELECT kwk_strom_kwh_gesamt, kwk_kwh_jahr, kwk_kwh_jahr_year,
                biogas_quota_kwh_ytd, biogas_quota_ytd_year,
                negative_price_qh_gesamt, foerderendedatum
           FROM eeg_anlagen
          WHERE tr_id = $1 AND tenant = $2
          FOR UPDATE",
    )
    .bind(&input.tr_id)
    .bind(&input.tenant)
    .fetch_optional(&mut *conn)
    .await
    .context("lock plant for settlement")?;

    let Some(fresh) = row else {
        return Ok(());
    };

    if input.settlement_model == KWKG_ZUSCHLAG {
        input.kwk_strom_kwh_gesamt = fresh.kwk_strom_kwh_gesamt;
        input.kwk_kwh_jahr = fresh.kwk_kwh_jahr;
        input.kwk_kwh_jahr_year = fresh.kwk_kwh_jahr_year;
    }
    input.biogas_quota_kwh_ytd = fresh.biogas_quota_kwh_ytd;
    input.biogas_quota_ytd_year = fresh.biogas_quota_ytd_year;

    // §51a: the Förderende the caller derived came from the same stale total, so
    // it is re-derived here rather than carried over.
    let is_solar = eeg_billing::ErzeugungsArt::from_db_str(&input.erzeugungsart)
        .map(eeg_billing::ErzeugungsArt::is_solar)
        .unwrap_or(false);
    input.foerderendedatum = fresh.foerderendedatum.map(|ende| {
        eeg_billing::foerderdauer::effektives_foerderende(
            ende,
            u64::try_from(fresh.negative_price_qh_gesamt).unwrap_or(0),
            is_solar,
        )
        .unwrap_or(ende)
    });

    Ok(())
}

/// Run the settlement calculation and persist the result.
///
/// Delegates all formula logic to the [`eeg_billing`] crate.
///
/// ## §52 EEG 2023 Pflichtzahlungen
///
/// The violations arrive on the input, derived by [`crate::sect52`]. For a plant
/// under the pre-2023 regime they are discarded: there the breach reduces the
/// Vergütung itself (`SanktionAlt`) and no separate Pflichtzahlung exists.
///
/// ## §25/§26 billing_days_fraction
///
/// When the plant was commissioned in the current billing month, the settlement
/// is prorated to the days with entitlement (commissioning day to end of month).
/// Assemble the §14 UStG Gutschrift for a billable EEG settlement.
///
/// Returns `(rechnung_json, gutschrift_nummer, steuer_eur, brutto_eur)`. All `None`
/// when document assembly fails — the failure is logged and the settlement is stored
/// without a document rather than blocked, because the payout obligation still exists.
fn build_gutschrift(
    input: &SettleInput,
    output: &eeg_billing::SettleOutput,
) -> (
    Option<serde_json::Value>,
    Option<String>,
    Option<Decimal>,
    Option<Decimal>,
) {
    use billing::{Currency, DocumentMeta, Period};
    use eeg_billing::gutschrift::settlement_to_gutschrift_with_document;

    let (year, month) = (input.billing_year, input.billing_month);
    // The feed-in Gutschrift VAT is the operator's declared status (§19
    // Kleinunternehmer `E`/0 % or Regelbesteuerung `S`/19 %) — masterdata, not a
    // guess from plant size. §12 Abs. 3 UStG is a hardware-supply rate and never
    // applies to the feed-in.
    let vat = input.vat_status;

    let nummer = format!("GS-EEG-{}-{year:04}-{month:02}", input.tr_id);
    let m = time::Month::try_from(month as u8).unwrap_or(time::Month::January);
    let last_day = m.length(year as i32);
    let period = Period::new(
        format!("{year:04}-{month:02}-01"),
        format!("{year:04}-{month:02}-{last_day:02}"),
    );

    let meta = DocumentMeta {
        invoice_number: nummer.clone(),
        currency: Currency::EUR,
        period_label: format!("{year:04}-{month:02}"),
        period: Some(period),
        issue_date: Some(mako_fristen::heute().to_string()),
        due_date: output.faelligkeitsdatum.map(|d| d.to_string()),
        // `PartyIdentifier` carries an optional ISO 6523 ICD
        // `scheme`. Both values are left scheme-less on purpose: the tenant is a
        // BDEW/DVGW Codenummer (not every one of which is a GLN) and the MaLo-ID
        // is an 11-digit BDEW identifier with no ICD at all. A scheme is
        // optional, and BR-CL-10 only constrains it when present — asserting
        // `"0088"` here would put an unverified claim into a regulated document.
        issuer_id: Some(billing::PartyIdentifier::new(input.tenant.clone())), // NB issues the Gutschrift
        recipient_id: Some(billing::PartyIdentifier::new(input.malo_id.clone())), // Anlagenbetreiber
        ..Default::default()
    };

    match settlement_to_gutschrift_with_document(output, vat, meta) {
        Ok((rechnung, doc)) => {
            // The outbound gate. `eeg-billing` checks its own emissions in
            // tests, but this Gutschrift is assembled from a settlement run's
            // runtime values — the entitlement, the VAT status, the levy
            // layers — and no fixture covers the arithmetic for arbitrary
            // amounts. It is the document the Anlagenbetreiber is paid against.
            //
            // A non-conformant one is dropped rather than stored, which is the
            // same degradation this function already applies to an assembly
            // failure: the settlement is recorded, the document is not, and the
            // warning names what to fix. Storing one whose totals disagree
            // would put a defective payment document into the record instead.
            if let Err(e) = mako_markt::bo4e::ensure_conformant(&rechnung) {
                tracing::warn!(
                    tr_id = %input.tr_id, error = %e,
                    "einsd: assembled Gutschrift is not a valid BO4E document — \
                     settlement stored without a document"
                );
                return (None, None, None, None);
            }
            (
                serde_json::to_value(&rechnung).ok(),
                Some(nummer),
                // BT-110 VAT total (not `tax_total()`, which would also fold in
                // any non-VAT levy layer). A validated single-layer Gutschrift
                // never errs.
                doc.vat_total().ok().map(|a| a.into_decimal()),
                Some(doc.gross_total().into_decimal()),
            )
        }
        Err(e) => {
            tracing::warn!(
                tr_id = %input.tr_id, error = %e,
                "einsd: Gutschrift assembly failed — settlement stored without a document"
            );
            (None, None, None, None)
        }
    }
}

pub async fn run_settlement(
    conn: &mut sqlx::PgConnection,
    input: SettleInput,
) -> anyhow::Result<SettleResult> {
    use eeg_billing::{
        AusschreibungMetadata, SettleInput as EegInput, SettlementScheme, SettlementStatus,
        TariffSource, calculate_settlement,
    };

    // Lock the plant and refresh everything that is a running total before any
    // entitlement is computed from it. This is the transaction's first statement
    // so that concurrent settlements of one plant queue here, in one order.
    let mut input = input;
    refresh_cumulative_counters(&mut *conn, &mut input).await?;

    // The scheme is built AFTER the §54 computation so direktverm_aw_ct_effective
    // is available.

    let eeg_gesetz_enum = eeg_billing::EegGesetz::from_db_year(input.eeg_gesetz)
        .unwrap_or(eeg_billing::EegGesetz::Eeg2023);

    // ── §52 EEG 2023 Pflichtverstöße ─────────────────────────────────────────
    // EEG 2023: separate Pflichtzahlungen, Vergütung keeps flowing.
    // EEG ≤2021 via §100: the old SanktionAlt model reduces the Vergütung itself.
    //
    // The detection lives in `crate::sect52`, which is the one place plant facts
    // become violations — so a rule cannot be detected in one surface and missed
    // in another.
    // ── §52 Abs. 1 — derive from the plant row *and* the register ────────────
    //
    // Read here rather than in `build_settle_input`, which is synchronous: the
    // register carries the start date, the Abs. 3 Satz 1 Nr. 1 cure and the
    // Abs. 3 Satz 2 defect waiver, and every one of those changes the amount.
    let aufzeichnungen = list_pflichtverstoesse(&mut *conn, &input.tenant, &input.tr_id).await?;
    if let Some(anlage) = fetch_anlage_conn(&mut *conn, &input.tenant, &input.tr_id).await? {
        input.pflichtverstoesse = crate::sect52::derive_pflichtverstoesse(
            &anlage,
            &aufzeichnungen,
            crate::sect52::Sect52Context {
                billing_date: input.billing_date.unwrap_or(anlage.inbetriebnahme),
                ausfallverguetung: input.ausfallverguetung_nutzung,
            },
        );
    }

    let (sanktion, pflichtverstoss) =
        if eeg_gesetz_enum.mastr_nichtregistrierung_suspendiert_verguetung() {
            // EEG ≤2021 path: Vergütung reduced to 0 for unregistered plants.
            let sanktion = if input.mastr_registriert {
                input.sanktion
            } else {
                Some(eeg_billing::SanktionAlt::VerguetungAufNull)
            };
            (sanktion, vec![])
        } else {
            (None, input.pflichtverstoesse.clone())
        };

    // ── The eeg-billing library now auto-computes billing_days_fraction from dates ─
    // No local computation needed — pass billing_days_fraction: None and the library
    // will derive it from billing_date, inbetriebnahme, and foerderendedatum.

    // ── Erlöschen des Zuschlags: nothing left to settle ─────────────────────
    // §36e (Wind an Land) / §37e (Solar erstes Segment) / §39e (Biomasse) EEG
    // 2023. Not §35a, which is BNetzA-driven Entwertung, and not §33, which
    // excludes a bid before any award exists.
    if input.award_expired {
        let id = Uuid::new_v4();
        // Every money-bearing column is overwritten, not just the status: an
        // already-settled plant must not keep its old `settlement_eur` and
        // Gutschrift on a row now labelled `foerderung_beendet`.
        sqlx::query(
            r"INSERT INTO settlement_receipts
                  (id, tr_id, tenant, billing_year, billing_month,
                   settlement_model, einspeisemenge_kwh, settlement_eur, status)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
              ON CONFLICT (tr_id, tenant, billing_year, billing_month)
                  WHERE is_correction = false DO UPDATE
              SET settlement_model    = EXCLUDED.settlement_model,
                  einspeisemenge_kwh  = EXCLUDED.einspeisemenge_kwh,
                  settlement_eur      = EXCLUDED.settlement_eur,
                  status              = EXCLUDED.status,
                  pflichtzahlung_eur  = NULL,
                  faelligkeitsdatum   = NULL,
                  verlaengerungsanspruch_qh = 0,
                  positions_json      = NULL,
                  rechnung_json       = NULL,
                  gutschrift_nummer   = NULL,
                  settled_at          = now()",
        )
        .bind(id)
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(input.billing_year)
        .bind(input.billing_month)
        .bind(&input.settlement_model)
        .bind(input.einspeisemenge_kwh)
        .bind(rust_decimal::Decimal::ZERO)
        .bind("foerderung_beendet")
        .execute(&mut *conn)
        .await
        .context("persist expired-award receipt")?;
        return Ok(SettleResult {
            id,
            tr_id: input.tr_id,
            billing_year: input.billing_year,
            billing_month: input.billing_month,
            settlement_model: input.settlement_model,
            einspeisemenge_kwh: input.einspeisemenge_kwh,
            settlement_eur: Some(rust_decimal::Decimal::ZERO),
            status: "foerderung_beendet".to_owned(),
            // Förderung ended → nothing to bill, so no Gutschrift is issued.
            gutschrift_nummer: None,
            gutschrift_steuer_eur: None,
            gutschrift_brutto_eur: None,
            // The award lapsed before the §52 derivation ran, so no claim is
            // stated here either — the receipt's own columns are cleared with it.
            pflichtzahlung_kumuliert_eur: None,
        });
    }

    // ── § 21 Abs. 1 Satz 1 Nr. 1 EEG 2023 — the claim ends at 100 kW ─────────
    //
    // The Einspeisevergütung mit gesetzlich bestimmtem anzulegenden Wert exists
    // only „für Strom aus Anlagen mit einer installierten Leistung von bis zu 100
    // Kilowatt". A larger plant left on `VERGUETUNG` is owed **nothing**: it is
    // not sanctioned — § 52 Abs. 1 Nr. 4 charges a § 10b breach, which only a
    // plant *in* Direktvermarktung can commit — and it is not paid either.
    //
    // Decided here rather than inside `calculate_settlement`, because
    // `SettlementScheme` names a *formula* while the Veräußerungsform a plant is
    // actually assigned to is register data. `settlement_model` is that fact and
    // it lives in this service.
    //
    // It is **not** an early return: § 52 Abs. 1 charges are owed to the
    // Netzbetreiber whether or not the plant has a Vergütungsanspruch, so the
    // engine still runs and only the money side is overridden below.
    //
    // `AUSFALLVERGUETUNG` is deliberately untouched: § 21 Abs. 1 Satz 1 Nr. 3
    // exists *for* plants above the threshold. A plant commissioned before
    // 2016-01-01 answers `None` and is settled as before rather than refused on a
    // threshold mako's regulatory corpus does not carry.
    let kein_anspruch = input.settlement_model == crate::models::VERGUETUNG
        && input
            .leistung_kwp
            .zip(input.inbetriebnahme)
            .and_then(|(kw, ibn)| eeg_billing::direktverm::direktvermarktungspflicht(kw, ibn))
            .unwrap_or(false);

    // ── §24 Abs. 1 EEG 2023 — deserialize CapacityBlocks from JSONB ─────────
    let capacity_blocks: Vec<eeg_billing::CapacityBlock> = input
        .capacity_blocks_json
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // ── §§53b–54 EEG 2023 — facts that cut the anzulegender Wert ─────────────
    // Only the triggering facts are stored; the deductions are statutory and are
    // applied inside the settlement engine, which also owns the ordering against
    // the Marktprämie floor. Amounts are deliberately not read from the database.
    let aw_reductions = if let Some(bd) = input.billing_date {
        load_aw_reductions(&mut *conn, &input.tr_id, &input.tenant, bd).await?
    } else {
        eeg_billing::aw_reductions::AwReductionContext::default()
    };

    // The engine receives the raw awarded AW; §54 is applied there.
    let direktverm_aw_ct_effective = input.direktverm_aw_ct;

    // ── §§ 7, 8 KWKG — Zuschlagssatz und Jahreshöchstbetrag ─────────────────
    // § 7 prices per Leistungsanteil, so a KWK plant's rate is the blended
    // Mischsatz across the bands its capacity spans, and § 7 Abs. 2 pays a
    // different, lower ladder where the KWK-Strom is not fed into a Netz der
    // allgemeinen Versorgung. § 8 Abs. 4 then caps what one calendar year may be
    // paid for, independently of the lifetime Vollbenutzungsstunden.
    let (kwk_zuschlag_ct, kwk_jahres_rest) = if input.settlement_model == KWKG_ZUSCHLAG {
        let leistung_kw = input.leistung_kwp.unwrap_or(Decimal::ZERO);
        let anlagenart = input
            .kwk_anlagenart
            .as_deref()
            .map(parse_kwk_anlagenart)
            .transpose()?
            .unwrap_or(eeg_billing::KwkAnlagenart::Neu);
        // § 7 Abs. 1 and Abs. 2 are different claims resting on different facts:
        // Abs. 1 prices KWK-Strom „der in ein Netz der allgemeinen Versorgung
        // eingespeist wird" at 8 down to 3,4 ct, Abs. 2 prices KWK-Strom that is
        // not at 5,41 down to 1 ct. Which one a plant is on is a fact about the
        // plant, not a default, so an unrecorded Verwendung computes no rate —
        // guessing Abs. 1 would pay up to eight times the Abs. 2 figure to a
        // plant that may have no Abs. 1 claim at all.
        let verwendung = input
            .kwk_verwendung
            .as_deref()
            .map(parse_kwk_verwendung)
            .transpose()?;
        let statutory =
            verwendung
                .zip(input.inbetriebnahme)
                .and_then(|(verwendung, dauerbetrieb)| {
                    eeg_billing::kwkg::zuschlag_ct_kwh(&eeg_billing::KwkZuschlagInput {
                        kwk_leistung_kw: leistung_kw,
                        anlagenart,
                        verwendung,
                        dauerbetrieb,
                        // § 7 Abs. 1 Satz 2 pays the 0,5 ct uplift on Nr. 5 lit. a
                        // only „soweit das Bundesministerium für Wirtschaft und
                        // Energie […] dies im Bundesanzeiger veröffentlicht hat".
                        // The register records whether it did.
                        bmwk_feststellung_veroeffentlicht: input.kwk_bmwk_feststellung,
                    })
                });
        // A KWKAusV award is a figure the plant won, not a § 7 ladder value, so
        // it wins over the computation. Everything else is priced by § 7: a rate
        // seeded on the plant row is a fallback, not an award, and taking it
        // first would leave the Leistungsanteil computation unreachable for
        // every plant that carries one.
        let satz = if let Some(award) = input
            .zuschlagswert_ct
            .filter(|_| input.ausschreibungs_zuschlag_id.is_some())
        {
            award
        } else if let Some(ct) = statutory {
            ct
        } else if input.verguetungssatz_ct > Decimal::ZERO {
            tracing::warn!(
                tr_id = %input.tr_id,
                kwk_verwendung = ?input.kwk_verwendung,
                "einsd: §7 KWKG prices no rate for this plant (kwk_verwendung unrecorded, \
                 §7 Abs. 3 left to a Verordnung, or §35 Abs. 20 Satz 1 keeping it out of \
                 Nr. 5) — settling on the rate stored on the plant row"
            );
            input.verguetungssatz_ct
        } else {
            anyhow::bail!(
                "KWKG plant {}: §7 prices no Zuschlag without a kwk_verwendung, and the plant \
                 carries neither a KWKAusV Zuschlagswert nor a stored verguetungssatz_ct — \
                 settling it would pay 0 ct/kWh silently",
                input.tr_id
            );
        };
        // § 8 Abs. 4: the year's contingent less what the year has already been
        // paid for. A counter from an earlier year has no bearing on this one.
        let bereits = if input.kwk_kwh_jahr_year == Some(input.billing_year) {
            input.kwk_kwh_jahr.unwrap_or(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };
        let rest =
            eeg_billing::kwkg::jahreskontingent_kwh(leistung_kw, i32::from(input.billing_year))
                .map(|kontingent| (kontingent - bereits).max(Decimal::ZERO));
        (satz, rest)
    } else {
        (input.verguetungssatz_ct, None)
    };

    // Build data-bearing SettlementScheme variant now that direktverm_aw_ct_effective is ready.
    // One token per model — the schema CHECK is the same list, so an unknown
    // value here means the schema and this match have drifted apart.
    let (scheme, tariff_source) = match input.settlement_model.as_str() {
        "VERGUETUNG" => (
            SettlementScheme::FeedInTariff {
                verguetungssatz_ct: input.verguetungssatz_ct,
            },
            TariffSource::Statutory,
        ),
        "AUSFALLVERGUETUNG" => (
            SettlementScheme::TemporaryFeedInTariff {
                verguetungssatz_ct: input.verguetungssatz_ct,
            },
            TariffSource::Statutory,
        ),
        "MIETERSTROM" => (
            SettlementScheme::TenantElectricity {
                verguetungssatz_ct: input.verguetungssatz_ct,
                mieter_zuschlag_ct: input.mieter_zuschlag_ct,
            },
            TariffSource::Statutory,
        ),
        "DIREKTVERMARKTUNG" => (
            SettlementScheme::MarketPremium {
                direktverm_aw_ct: direktverm_aw_ct_effective.unwrap_or(rust_decimal::Decimal::ZERO),
                wind_korrekturfaktor: input.wind_korrekturfaktor,
                wind_standort: None,
            },
            TariffSource::Statutory,
        ),
        "AUSSCHREIBUNG" => (
            SettlementScheme::MarketPremium {
                direktverm_aw_ct: direktverm_aw_ct_effective.unwrap_or(rust_decimal::Decimal::ZERO),
                wind_korrekturfaktor: input.wind_korrekturfaktor,
                wind_standort: None,
            },
            TariffSource::Auction(AusschreibungMetadata {
                is_biogas_sect51b: input.is_biogas_sect51b,
                zuschlag_id: input.ausschreibungs_zuschlag_id.clone(),
                award_ct: input.zuschlagswert_ct,
                award_date: input.zuschlag_datum,
                award_expired: input.award_expired,
                innovation_auction: input.ist_innovationsausschreibung,
                is_buergerenergie: input.ist_buergerenergie,
            }),
        ),
        "POST_EEG_SPOT" => (
            SettlementScheme::PostEeg { price_floor: None },
            TariffSource::Statutory,
        ),
        "EIGENVERBRAUCH" => (SettlementScheme::Eigenverbrauch, TariffSource::Statutory),
        "KWKG_ZUSCHLAG" => (
            SettlementScheme::KwkSurcharge {
                // § 7 prices per Leistungsanteil, so the plant's rate is the
                // blended Mischsatz across the bands its capacity spans — and it
                // depends on whether the KWK-Strom is fed into a Netz der
                // allgemeinen Versorgung. A stored rate is honoured, and the
                // statutory computation fills in where none is stored.
                verguetungssatz_ct: kwk_zuschlag_ct,
                kwh_paid_gesamt: input.kwk_strom_kwh_gesamt,
                max_kwh: input.kwk_max_kwh,
                jahres_restkontingent_kwh: kwk_jahres_rest,
            },
            TariffSource::Statutory,
        ),
        "FLEXIBILITAET" => (
            SettlementScheme::FlexibilityPremium {
                verguetungssatz_ct: input.verguetungssatz_ct,
                flex_praemie_ct_kwh: input.flex_praemie_ct_kwh,
            },
            TariffSource::Statutory,
        ),
        "FLEXIBILITAET_ZUSCHLAG" => (
            SettlementScheme::FlexibilitySurcharge {
                rate_eur_per_kw_year: input.verguetungssatz_ct,
            },
            TariffSource::Statutory,
        ),
        // ── §42b EnWG Gemeinschaftliche Gebäudeversorgung ─────────────────
        // GGV plants receive EEG Einspeisevergütung from the NB like any other
        // solar plant. The settlement is against the Einspeisemessung (grid
        // feed-in) at the GGV MaLo, not per-tenant. TenantElectricity is the
        // correct scheme: Vergütungssatz = §21 EEG rate; mieter_zuschlag_ct =
        // None (no Mieterstrom surcharge for the NB→LF EEG flow).
        // The Nutzungsplan allocation among tenants is handled separately in
        // billingd (POST /api/v1/billing/ggv/{ggv_id}).
        "GGV" => (
            SettlementScheme::TenantElectricity {
                verguetungssatz_ct: input.verguetungssatz_ct,
                mieter_zuschlag_ct: input.mieter_zuschlag_ct,
            },
            TariffSource::Statutory,
        ),
        // ── §21a EEG 2023 Sonstige Direktvermarktung ─────────────────────────
        // No NB EEG payment. Records the period for settlement history.
        "SONSTIGE_DIREKTVERMARKTUNG" => (
            SettlementScheme::SonstigeDirektvermarktung,
            TariffSource::Statutory,
        ),
        other => anyhow::bail!("unknown settlement_model: {other}"),
    };

    // ── §44b Abs. 1 EEG 2023 — Biogas annual 45%-cap quota ────────────────────
    // Auto-computed here when the caller did not supply an explicit eligible_kwh.
    // compute_biogas_sect44b_eligible resets the YTD counter when billing_year changed.
    let biogas_sect44b_eligible_kwh = if input.biogas_sect44b_eligible_kwh.is_some() {
        input.biogas_sect44b_eligible_kwh // caller-provided explicit override
    } else {
        compute_biogas_sect44b_eligible(&mut *conn, &input)
            .await
            .context("compute §44b Biogas quota")?
    };

    // ── Anlage 1 Nr. 2–4 EEG 2023 — the energieträgerspezifische Marktwert ───
    //
    // Which series the plant takes is Nr. 2's answer, not the operator's: a
    // plant commissioned or bezuschlagt before 01.01.2023 settles on the
    // **Monats**marktwert (Nr. 3), everything newer on the **Jahres**marktwert
    // (Nr. 4), and a Satz-1 plant claiming under the § 19 Abs. 3b/3c
    // Abgrenzungs- oder Pauschaloption moves onto Nr. 4 as well.
    //
    // Lookup order: caller override → the plant's own series in
    // `marktwert_preise` (exact technology, then DEFAULT) → the generic EPEX
    // monthly average → `PriceMissing`. It never falls back to the *other*
    // series: that is the substitution this split exists to prevent.
    let serie = eeg_billing::marktwertserie(
        input.inbetriebnahme.unwrap_or(mako_fristen::heute()),
        input.zuschlag_datum,
        input.speicher_abgrenzungs_oder_pauschaloption,
    );
    let mut marktwert_vorlaeufig = false;
    let effective_marktwert = if input.jahresmarktwert_ct_kwh.is_some() {
        input.jahresmarktwert_ct_kwh
    } else if matches!(
        input.settlement_model.as_str(),
        "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG"
    ) {
        let treffer = fetch_marktwert(
            &mut *conn,
            input.billing_year,
            input.billing_month,
            serie,
            &input.erzeugungsart,
            input.epex_avg_ct_kwh,
        )
        .await
        .context("fetch Marktwert")?;
        marktwert_vorlaeufig = treffer.is_some_and(|t| t.vorlaeufig);
        treffer.map(|t| t.avg_ct_kwh)
    } else {
        input.epex_avg_ct_kwh
    };

    // ── §51 Abs. 2 Nr. 1 — the <100 kW exemption lapses at the turn of the year ─
    // The exemption covers „Zeiträume vor dem Ablauf des Kalenderjahres, in dem
    // die Anlage mit einem intelligenten Messsystem ausgestattet wird", so it
    // runs to the end of the installation year, not to the installation day.
    let has_imesys = eeg_billing::negativpreis::imesys_befreiung_entfallen(
        input.imesys_rollout_datum,
        input.billing_date,
    );

    let output = calculate_settlement(&EegInput {
        scheme,
        tariff_source,
        einspeisemenge_kwh: input.einspeisemenge_kwh,
        // Anlage 1 Nr. 2–4: the plant's own Marktwert series for DV plants; EPEX for others.
        marktwert_ct_kwh: effective_marktwert,
        sanktion,
        kwh_during_negative_epex: input.kwh_during_negative_epex,
        inbetriebnahme: input.inbetriebnahme,
        leistung_kwp: input.leistung_kwp,
        foerderendedatum: input.foerderendedatum,
        billing_date: input.billing_date,
        // §24 Abs. 1 EEG 2023: pass deserialized capacity blocks
        capacity_blocks,
        pflichtverstoss,
        eeg_gesetz: eeg_gesetz_enum,
        // einsd's eeg_verguetungssaetze stores NET rates (§53 already deducted),
        // so the engine must not deduct §53 again.
        aw_is_gross: false,
        erzeugungsart: eeg_billing::ErzeugungsArt::from_db_str(&input.erzeugungsart).ok(),
        // §13a EnWG (Redispatch 2.0): curtailment compensation (NB must pay for suppressed kWh)
        einspeisemanagement_kwh: input.einspeisemanagement_kwh,
        billing_days_fraction: None, // auto-computed by eeg-billing from billing_date + dates
        // §§53b–54: statutory AW cuts, from the recorded triggering facts.
        aw_reductions,
        // §51a: pass quarter-hours for Verlängerungsanspruch computation
        negative_price_quarter_hours: input.negative_price_quarter_hours,
        // §44b Abs. 1 EEG 2023: computed above from annual quota tracking
        biogas_sect44b_eligible_kwh,
        // §51 Abs. 2 Nr. 1 EEG 2023: the <100 kW exemption lapses at the end of
        // the calendar year the iMSys was fitted in.
        has_imesys,
        // §3 Nr. 37 EEG 2023: Pilotwindenergieanlagen are carved out of §51.
        ist_pilotwindanlage: input.ist_pilotwindanlage,
        // §51 Abs. 3 EEG: the Ausfallvergütung reporting duty.
        sect51_abs3_unreported_days: input.sect51_abs3_unreported_days,
        // §100 EEG: a Bestandsanlage that opted into the Solarspitzengesetz regime.
        sect51_optin_wirksam_ab: input.sect51_optin_wirksam_ab,
        marktwert_kategorie: None,
        // § 147 AO / GoBD: the engine labels the audit positions differently for
        // a correction, so it has to be told it is one rather than inferring a
        // fresh settlement from an identical input.
        settlement_type: match &input.correction {
            Some(k) => eeg_billing::SettlementType::Correction {
                original_id: k.original_id.map(|id| id.to_string()).unwrap_or_default(),
                reason: k.reason,
            },
            None => eeg_billing::SettlementType::Initial,
        },
        // §3 EEG 2023: plant lifecycle type — drives audit labels and Förderdauer semantics
        inbetriebnahme_typ: input
            .inbetriebnahme_typ
            .as_deref()
            .and_then(|s| eeg_billing::InbetriebnahmeTyp::from_db_str(s).ok())
            .unwrap_or_default(),
        // §§ 39i, 42–44 EEG 2023: biomass fuel composition from the plant record.
        // `None` for non-biomass plants; `Some` enforces the § 39i Abs. 1
        // Getreide- und Mais-Höchstanteil and detects § 44 Güllekleinanlage
        // eligibility at every billing period.
        biomasse: input.biomasse.clone(),
    });

    // ── § 21 Abs. 1 Satz 1 Nr. 1 — override the money side, keep the § 52 side ─
    //
    // The engine priced the Einspeisevergütung because that is the formula it was
    // handed. The plant has no claim to it, so the amount is zero and the audit
    // position says why — but the Pflichtzahlung, the Fälligkeitsdatum and the
    // reduced quantity stay exactly as computed: § 52 Abs. 1 is owed to the
    // Netzbetreiber whether or not the plant is paid, and the § 51 arithmetic is
    // what a later correction (after a Veräußerungsformwechsel) starts from.
    let mut output = output;
    if kein_anspruch && output.status == SettlementStatus::Calculated {
        output.status = SettlementStatus::KeinAnspruch;
        output.settlement_eur = Some(rust_decimal::Decimal::ZERO);
        output.positions = vec![eeg_billing::SettlePosition {
            description: format!(
                "§21 Abs. 1 Satz 1 Nr. 1 EEG 2023: kein Anspruch auf die Einspeisevergütung — \
                 installierte Leistung {} kW über 100 kW",
                input.leistung_kwp.unwrap_or_default()
            ),
            legal_basis: "§21 Abs. 1 Satz 1 Nr. 1 EEG 2023".to_owned(),
            kwh: output.eligible_kwh.unwrap_or(rust_decimal::Decimal::ZERO),
            rate_ct_kwh: rust_decimal::Decimal::ZERO,
            eur: rust_decimal::Decimal::ZERO,
        }];
        tracing::warn!(
            tr_id = %input.tr_id,
            leistung_kwp = ?input.leistung_kwp,
            "einsd: §21 Abs. 1 Satz 1 Nr. 1 — plant over 100 kW on VERGUETUNG has no claim; \
             assign a Direktvermarktung (§20) or the Ausfallvergütung (§21 Abs. 1 Satz 1 Nr. 3)"
        );
    }
    let output = output;

    let status = match output.status {
        SettlementStatus::Calculated => "calculated",
        SettlementStatus::NoData => "no_data",
        SettlementStatus::PriceMissing => "price_missing",
        SettlementStatus::FoerderungBeendet => "foerderung_beendet",
        // § 8 Abs. 4 KWKG — this year's Vollbenutzungsstunden are used up and
        // the plant resumes in January. Storing it as `foerderung_beendet` would
        // retire a plant that is still owed most of its Förderung.
        SettlementStatus::JahreskontingentErschoepft => "jahreskontingent_erschoepft",
        SettlementStatus::Sanctioned => "sanctioned",
        SettlementStatus::KeinAnspruch => "kein_anspruch",
        // Forward-compatible: any future status variant stores as "unknown" and does not block
        _ => "unknown",
    };
    let settlement_eur = output.settlement_eur;
    let effective_kwh = output.eligible_kwh;
    let pflichtzahlung_eur = output.pflichtzahlung_eur;
    let faelligkeitsdatum = output.faelligkeitsdatum;
    let verlaengerungsanspruch_qh = output.verlaengerungsanspruch_qh as i64;
    // Use the fraction actually applied by the library (may be auto-computed from dates)
    let billing_days_fraction_stored = output.billing_days_fraction_applied;
    // Serialize positions to JSONB for the § 147 AO / GoBD audit trail
    // (Buchungsbeleg, 8-year retention).
    // Each position: { description, legal_basis, kwh, rate_ct_kwh, eur }
    let positions_json = serde_json::to_value(
        output
            .positions
            .iter()
            .map(|p| {
                serde_json::json!({
                    "description": p.description,
                    "legal_basis": p.legal_basis,
                    "kwh": p.kwh.to_string(),
                    "rate_ct_kwh": p.rate_ct_kwh.to_string(),
                    "eur": p.eur.to_string()
                })
            })
            .collect::<Vec<_>>(),
    )
    .ok();

    // ── §14 UStG Gutschrift (Gutschriftverfahren) ───────────────────────────────
    // For a billable settlement the NB *issues* the settlement document to the
    // Anlagenbetreiber. The amount alone is not a legal document — VAT law requires
    // a Gutschrift with the per-rate breakdown. We build it here so the ledger and
    // the SEPA payout downstream reference an actual document, and persist it in
    // `rechnung_json`.
    //
    // The gate is whether there is a payment to document, not the status: a §52
    // Abs. 2 EEG 2021 plant is paid the Monatsmarktwert and a §52 Abs. 3 one 80 %
    // of its ordinary Vergütung, both under `Sanctioned`, and both are turnover
    // that needs its §14 UStG document. `NoData` and `PriceMissing` have no
    // amount at all and issue none.
    let (rechnung_json, gutschrift_nummer, gutschrift_steuer_eur, gutschrift_brutto_eur) =
        if output.settlement_eur.is_some_and(|e| e > Decimal::ZERO) {
            build_gutschrift(&input, &output)
        } else {
            (None, None, None, None)
        };

    // Proposed id for a first settlement. An upsert that lands on the DO UPDATE
    // branch keeps the row's original id, which is what `RETURNING` yields — the
    // caller must get the id of the receipt that actually exists.
    let id = Uuid::new_v4();

    // ── § 147 AO / GoBD: snapshot the initial receipt before it is overwritten ───
    //
    // Only an **initial** settle overwrites anything: its upsert lands on the
    // partial unique index and replaces the row in place, so whatever that row
    // held has to be preserved before it is lost.
    //
    // A correction does not: it carries `is_correction = true`, misses the index
    // predicate, and is inserted *beside* the original, which stays live and
    // unchanged. Snapshotting it would copy a row nobody is about to lose, and
    // `ON CONFLICT DO NOTHING` would not dedupe the copies — the table's only
    // unique key is its own surrogate `id`.
    let snapshot_id: Option<uuid::Uuid> = if input.correction.is_some() {
        None
    } else {
        sqlx::query_scalar(
            "SELECT id FROM settlement_receipts
             WHERE tr_id = $1 AND tenant = $2
               AND billing_year = $3 AND billing_month = $4
               AND is_correction = false",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(input.billing_year)
        .bind(input.billing_month)
        .fetch_optional(&mut *conn)
        .await
        .context("check existing initial receipt")?
    };

    if let Some(original_id) = snapshot_id {
        sqlx::query(
            r"INSERT INTO settlement_receipt_history
                  (original_id, tr_id, tenant, billing_year, billing_month,
                   settlement_eur, status, settlement_data)
              SELECT id, tr_id, tenant, billing_year, billing_month,
                     settlement_eur, status,
                     to_jsonb(settlement_receipts) AS settlement_data
              FROM settlement_receipts
              WHERE id = $1
              ON CONFLICT DO NOTHING",
        )
        .bind(original_id)
        .execute(&mut *conn)
        .await
        .context("snapshot receipt before overwrite")?;
    }

    let id: Uuid = sqlx::query_scalar(
        r"INSERT INTO settlement_receipts
              (id, tr_id, tenant, billing_year, billing_month,
               settlement_model, einspeisemenge_kwh, settlement_eur, status,
               pflichtzahlung_eur, faelligkeitsdatum,
               verlaengerungsanspruch_qh, billing_days_fraction, positions_json,
               is_correction, correction_of, correction_reason,
               rechnung_json, gutschrift_nummer, marktwert_vorlaeufig)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9,
                  $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
          ON CONFLICT (tr_id, tenant, billing_year, billing_month) WHERE is_correction = false DO UPDATE
          SET settlement_model          = EXCLUDED.settlement_model,
              einspeisemenge_kwh        = EXCLUDED.einspeisemenge_kwh,
              settlement_eur            = EXCLUDED.settlement_eur,
              status                    = EXCLUDED.status,
              pflichtzahlung_eur        = EXCLUDED.pflichtzahlung_eur,
              marktwert_vorlaeufig      = EXCLUDED.marktwert_vorlaeufig,
              faelligkeitsdatum         = EXCLUDED.faelligkeitsdatum,
              verlaengerungsanspruch_qh = EXCLUDED.verlaengerungsanspruch_qh,
              billing_days_fraction     = EXCLUDED.billing_days_fraction,
              positions_json            = EXCLUDED.positions_json,
              is_correction             = EXCLUDED.is_correction,
              correction_of             = EXCLUDED.correction_of,
              correction_reason         = EXCLUDED.correction_reason,
              rechnung_json             = EXCLUDED.rechnung_json,
              gutschrift_nummer         = EXCLUDED.gutschrift_nummer,
              settled_at                = now()
          RETURNING id",
    )
    .bind(id)
    .bind(&input.tr_id)
    .bind(&input.tenant)
    .bind(input.billing_year)
    .bind(input.billing_month)
    .bind(&input.settlement_model)
    .bind(effective_kwh.or(input.einspeisemenge_kwh))
    .bind(settlement_eur)
    .bind(status)
    .bind(pflichtzahlung_eur)
    .bind(faelligkeitsdatum)
    .bind(verlaengerungsanspruch_qh)
    .bind(billing_days_fraction_stored)
    .bind(positions_json)
    .bind(input.correction.is_some())
    .bind(input.correction.as_ref().and_then(|k| k.original_id))
    .bind(input.correction.as_ref().map(Korrektur::reason_text))
    .bind(rechnung_json.clone())
    .bind(gutschrift_nummer.clone())
    .bind(marktwert_vorlaeufig)
    .fetch_one(&mut *conn)
    .await
    .context("persist settlement")?;

    // ── Cumulative counters, accrued once per period ──────────────────────────
    //
    // `POST /settle` is idempotent — the receipt is an upsert — but these
    // counters are running totals over the plant's whole Förderdauer (§44b
    // quota, §51a Förderende, KWKG limit). Each period's absolute contribution
    // is recorded, and only the difference applied: re-settling a month
    // unchanged is a no-op, a correction moves the counters by what changed.
    //
    // A correction carrying no §51 figures means "unchanged", not "this period
    // had none" — writing a zero would hand back the §51a Förderende extension
    // the original settlement earned. Only an explicit figure moves the counter.
    let previous_qh = existing_period_qh(
        &mut *conn,
        &input.tr_id,
        &input.tenant,
        input.billing_year,
        input.billing_month,
    )
    .await?;
    let period = PeriodAccrual {
        negative_price_qh: match input.negative_price_quarter_hours {
            Some(qh) => i64::try_from(qh).unwrap_or(i64::MAX),
            None => previous_qh,
        },
        // §44b: only a settled period consumes quota — NoData / PriceMissing do not.
        biogas_kwh: if matches!(
            output.status,
            SettlementStatus::Calculated | SettlementStatus::FoerderungBeendet
        ) && biogas_sect44b_eligible_kwh.is_some()
        {
            effective_kwh.unwrap_or(Decimal::ZERO).max(Decimal::ZERO)
        } else {
            Decimal::ZERO
        },
        kwk_kwh: if input.settlement_model == KWKG_ZUSCHLAG {
            effective_kwh.unwrap_or(Decimal::ZERO).max(Decimal::ZERO)
        } else {
            Decimal::ZERO
        },
    };
    let delta = record_period_accrual(
        &mut *conn,
        &input.tr_id,
        &input.tenant,
        input.billing_year,
        input.billing_month,
        &period,
    )
    .await?;

    // ── §44b: update Biogas year-to-date production counter ──────────────────
    if delta.biogas_kwh != Decimal::ZERO {
        update_biogas_quota_ytd(
            &mut *conn,
            &input.tr_id,
            &input.tenant,
            input.billing_year,
            delta.biogas_kwh,
        )
        .await
        .context("update biogas §44b YTD")?;
    }

    // ── §51a: accrue the RAW negative-price quarter-hours on the plant ────────
    // The extension rounds up to a full calendar day (or the solar
    // Volllastviertelstunden contingent) **once over the 20-year total**, so the
    // cumulative column stores the raw lost QH — `build_settle_input` converts it
    // to the effective Förderende via `effektives_foerderende`. Accruing the
    // per-month rounded value would over-extend (each partial day rounding up).
    if delta.negative_price_qh != 0 {
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET negative_price_qh_gesamt =
                      GREATEST(COALESCE(negative_price_qh_gesamt, 0) + $3, 0),
                  updated_at = now()
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(delta.negative_price_qh)
        .execute(&mut *conn)
        .await
        .context("accrue negative_price_qh_gesamt")?;
    }

    // ── KWKG: update accumulated kWh + auto-expire when limit reached ────────
    // Guarded on the **delta**, exactly like §44b and §51a above — not on this
    // period's new contribution, which would drop the negative delta of a month
    // corrected down to zero and go on burning the § 8 KWKG
    // Vollbenutzungsstunden against kWh the plant was never paid for.
    //
    // The status follows this settlement's outcome on the same condition, so a
    // plant parked at `foerderung_beendet` by kWh a correction has since removed
    // returns to `aktiv`.
    if input.settlement_model == KWKG_ZUSCHLAG && !delta.kwk_kwh.is_zero() {
        let new_status = if status == "foerderung_beendet" {
            "foerderung_beendet"
        } else {
            "aktiv"
        };
        // The § 8 Abs. 4 year counter is reset whenever the row still tracks an
        // earlier year: the Jahreshöchstbetrag is per Kalenderjahr, so nothing
        // carries over.
        sqlx::query(
            r"UPDATE eeg_anlagen
              SET kwk_strom_kwh_gesamt =
                      GREATEST(COALESCE(kwk_strom_kwh_gesamt, 0) + $3, 0),
                  kwk_kwh_jahr =
                      GREATEST(
                          CASE WHEN kwk_kwh_jahr_year = $5
                               THEN COALESCE(kwk_kwh_jahr, 0) ELSE 0 END + $3,
                          0),
                  kwk_kwh_jahr_year = $5,
                  status = $4,
                  updated_at = now()
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(delta.kwk_kwh)
        .bind(new_status)
        .bind(input.billing_year)
        .execute(&mut *conn)
        .await
        .context("update kwk_strom_kwh_gesamt")?;
    }

    // ── H-2: derive_settlement_state and update plant record ─────────────────
    // §52 EEG 2023 state machine: drive settlement_state from compliance status.
    if let Some(bd) = input.billing_date {
        let new_settlement_state = eeg_billing::settlement_state::derive_settlement_state(
            &eeg_billing::settlement_state::SettlementStateFacts {
                mastr_registriert: input.mastr_registriert,
                sect9_erfuellung: input.sect9_erfuellung,
                leistung_kwp: input.leistung_kwp.unwrap_or(Decimal::ZERO),
                erzeugungsart: eeg_billing::ErzeugungsArt::from_db_str(&input.erzeugungsart).ok(),
                foerderendedatum: input.foerderendedatum,
                billing_date: bd,
                eeg_gesetz_year: eeg_gesetz_enum.to_db_year(),
            },
        );
        // Both CTEs read the same snapshot, so `prev` yields the value from before
        // the update — the state is otherwise overwritten in place and the
        // transition that produced it becomes unrecoverable.
        let previous_state: Option<String> = sqlx::query_scalar(
            r"WITH prev AS (
                  SELECT settlement_state FROM eeg_anlagen
                  WHERE tr_id = $1 AND tenant = $2
                  FOR UPDATE
              ), upd AS (
                  UPDATE eeg_anlagen
                  SET settlement_state = $3, updated_at = now()
                  WHERE tr_id = $1 AND tenant = $2
              )
              SELECT settlement_state FROM prev",
        )
        .bind(&input.tr_id)
        .bind(&input.tenant)
        .bind(new_settlement_state.to_db_str())
        .fetch_optional(&mut *conn)
        .await
        .context("update settlement_state")?
        .flatten();

        // §52 EEG 2023 changes what the operator is owed, so each change of state
        // is recorded with the period that caused it rather than only its result.
        let to_state = new_settlement_state.to_db_str();
        if previous_state.as_deref() != Some(to_state) {
            sqlx::query(
                r"INSERT INTO settlement_state_transitions
                      (tr_id, tenant, from_state, to_state, effective_from, reason, notes)
                  VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(&input.tr_id)
            .bind(&input.tenant)
            .bind(previous_state.as_deref().unwrap_or("unbekannt"))
            .bind(to_state)
            .bind(bd)
            .bind("derived_at_settlement")
            .bind(format!(
                "abgeleitet bei Abrechnung {:04}-{:02}",
                input.billing_year, input.billing_month
            ))
            .execute(&mut *conn)
            .await
            .context("record settlement_state transition")?;
        }
    }

    Ok(SettleResult {
        id,
        tr_id: input.tr_id,
        billing_year: input.billing_year,
        billing_month: input.billing_month,
        settlement_model: input.settlement_model,
        einspeisemenge_kwh: effective_kwh.or(input.einspeisemenge_kwh),
        settlement_eur,
        status: status.to_owned(),
        gutschrift_nummer,
        gutschrift_steuer_eur,
        gutschrift_brutto_eur,
        pflichtzahlung_kumuliert_eur: pflichtzahlung_eur,
    })
}

/// Statuses of a receipt that represent a settlement actually having happened.
///
/// `price_missing` and `no_data` are the opposite: they record that the run
/// found nothing to settle with. Treating them as settled meant a plant settled
/// too early — before the ÜNB Marktwert or the complete edmd data existed —
/// was never picked up again and simply went unpaid.
const SETTLED_STATUSES: [&str; 5] = [
    "calculated",
    "foerderung_beendet",
    "sanctioned",
    // §21 Abs. 1 Satz 1 Nr. 1 — the period is decided, and re-running it will
    // reach the same answer. Only a Veräußerungsformwechsel changes it, and that
    // rewrites the plant rather than the receipt.
    "kein_anspruch",
    // § 8 Abs. 4 KWKG — the calendar year's Jahreskontingent is exhausted, so
    // this month draws no Zuschlag and re-running it reaches the same answer
    // until January refills the contingent. Omitting it did not make the month
    // payable; it made every batch run and the monthly worker pick the plant up
    // again, month after month, to reach the same "nothing" — the retry-forever
    // shape, not a missed payment. A correction that frees the contingent
    // re-settles the month explicitly and does not come through this sweep.
    "jahreskontingent_erschoepft",
];

/// List all active plants that have NOT been settled for `(year, month)` yet.
///
/// Used by the batch settlement endpoint and the monthly auto-settle worker. A
/// plant whose only receipt for the period is `price_missing` or `no_data`
/// counts as unsettled and is retried.
pub async fn list_unsettled(
    pool: &PgPool,
    tenant: &str,
    year: i16,
    month: i16,
) -> anyhow::Result<Vec<AnlageRow>> {
    sqlx::query_as::<_, AnlageRow>(
        r"SELECT a.*
          FROM eeg_anlagen a
          WHERE a.tenant = $1
            AND a.status = 'aktiv'
            AND NOT EXISTS (
                SELECT 1 FROM settlement_receipts s
                WHERE s.tr_id = a.tr_id
                  AND s.tenant = a.tenant
                  AND s.billing_year = $2
                  AND s.billing_month = $3
                  AND s.status = ANY($4)
            )
          ORDER BY a.tr_id",
    )
    .bind(tenant)
    .bind(year)
    .bind(month)
    .bind(SETTLED_STATUSES.as_slice())
    .fetch_all(pool)
    .await
    .context("list_unsettled")
}

/// §24 EEG 2023 — Zusammenlegung: merge a child plant into a parent entity.
///
/// Sets `parent_tr_id` on the child plant and updates its status to `abgemeldet`.
/// The parent plant continues as the active entity.
///
/// ## Legal basis
///
/// §24 EEG 2023: Multiple plants at the same Netzverknüpfungspunkt may be merged
/// into a single entity ("Gesamtanlage") for the purposes of the tariff threshold
/// (§ 21 EEG 2023 power ranges).  After Zusammenlegung:
/// - The child plant's `status → abgemeldet` (historical record preserved).
/// - The parent plant assumes the combined capacity and continues settlement.
/// - `foerderendedatum` of the parent is NOT reset (unlike Repowering).
///
/// Returns `Ok(true)` if the child was found and updated, `Ok(false)` if not found.
/// Load one plant in the terms §24 Abs. 1 asks about.
async fn load_fuer_zusammenfassung(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    require_aktiv: bool,
) -> anyhow::Result<Option<eeg_billing::AnlageFuerZusammenfassung>> {
    use eeg_billing::{ErzeugungsArt, SolarMontage};

    let sql = if require_aktiv {
        "SELECT inbetriebnahme, erzeugungsart, standort_id, solar_montage,
                netzverknuepfungspunkt, biogaserzeugungsanlage_id
           FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2 AND status = 'aktiv'"
    } else {
        "SELECT inbetriebnahme, erzeugungsart, standort_id, solar_montage,
                netzverknuepfungspunkt, biogaserzeugungsanlage_id
           FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2"
    };
    let Some(row) = sqlx::query(sql)
        .bind(tr_id)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .context("fetch plant for §24 Zusammenfassung")?
    else {
        return Ok(None);
    };

    let montage = match row
        .try_get::<Option<String>, _>("solar_montage")?
        .as_deref()
    {
        Some("AN_GEBAEUDE_ODER_LAERMSCHUTZWAND") => SolarMontage::AnGebaeudeOderLaermschutzwand,
        Some("FREIFLAECHE") => SolarMontage::Freiflaeche,
        _ => SolarMontage::Sonstige,
    };
    // A NULL standort_id must not make two plants share a site, so each unknown
    // gets its own value. Fusing on ignorance moves a plant into a tariff band
    // for twenty years.
    let standort_id = row
        .try_get::<Option<String>, _>("standort_id")?
        .unwrap_or_else(|| format!("unbekannt:{tr_id}"));

    Ok(Some(eeg_billing::AnlageFuerZusammenfassung {
        inbetriebnahme: row.try_get("inbetriebnahme")?,
        art: ErzeugungsArt::from_db_str(&row.try_get::<String, _>("erzeugungsart")?)
            .unwrap_or_default(),
        standort_id,
        // Every plant einsd settles has a size-dependent §19 Abs. 1 claim; the
        // size-independent case does not reach this service.
        anspruch_leistungsabhaengig: true,
        montage,
        netzverknuepfungspunkt: row.try_get("netzverknuepfungspunkt")?,
        biogaserzeugungsanlage_id: row.try_get("biogaserzeugungsanlage_id")?,
        // Satz 5 devices are registered as ordinary plants here; a
        // Steckersolargerät below the thresholds is not settled by einsd.
        steckersolar: None,
    }))
}

pub async fn zusammenlegen(
    pool: &PgPool,
    tenant: &str,
    child_tr_id: &str,
    parent_tr_id: &str,
    combined_leistung_kwp: Option<Decimal>,
    unmittelbare_raeumliche_naehe: bool,
) -> anyhow::Result<bool> {
    let Some(child) = load_fuer_zusammenfassung(pool, tenant, child_tr_id, false).await? else {
        return Ok(false);
    };
    let Some(parent) = load_fuer_zusammenfassung(pool, tenant, parent_tr_id, true).await? else {
        anyhow::bail!("parent plant {} not found or not aktiv", parent_tr_id);
    };

    // §24 decides whether these two are one plant. Merging a pair the statute
    // keeps apart moves the survivor into a tariff band and past a tender
    // threshold it never qualified for, for the rest of its Förderdauer — and
    // nothing downstream can tell that apart from a legitimate merge.
    let verdict = eeg_billing::sind_eine_anlage(&child, &parent, unmittelbare_raeumliche_naehe);
    if !verdict.gelten_als_eine_anlage {
        anyhow::bail!(
            "§24 Abs. 1 EEG 2023 does not treat {child_tr_id} and {parent_tr_id} as one plant: {:?}",
            verdict.grund
        );
    }

    // Both writes commit together. Run apart, a failure between them left the
    // child deregistered — it stops settling immediately — while the parent kept
    // the smaller capacity it is now supposed to carry, so the merged plant was
    // billed in the wrong tariff band with no record of why.
    let mut tx = pool.begin().await.context("begin Zusammenlegung")?;

    // Mark child as merged (preserves history, stops future settlements).
    sqlx::query(
        "UPDATE eeg_anlagen SET status = 'abgemeldet', parent_tr_id = $3, updated_at = now()
         WHERE tr_id = $1 AND tenant = $2 AND status = 'aktiv'",
    )
    .bind(child_tr_id)
    .bind(tenant)
    .bind(parent_tr_id)
    .execute(&mut *tx)
    .await
    .context("mark child abgemeldet for Zusammenlegung")?;

    // Optionally update parent's combined capacity.
    if let Some(combined_kwp) = combined_leistung_kwp {
        sqlx::query(
            "UPDATE eeg_anlagen SET leistung_kwp = $3, updated_at = now()
             WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(parent_tr_id)
        .bind(tenant)
        .bind(combined_kwp)
        .execute(&mut *tx)
        .await
        .context("update parent leistung_kwp for Zusammenlegung")?;
    }

    tx.commit().await.context("commit Zusammenlegung")?;
    Ok(true)
}

/// How long the plant has been on the Ausfallvergütung, up to and including
/// `(billing_year, billing_month)`.
///
/// §21 Abs. 1 Satz 1 Nr. 3 EEG caps it at three consecutive calendar months and
/// six calendar months per calendar year; §52 Abs. 1 Nr. 5 charges 10 €/kW per
/// month for exceeding either. Both counts come from the receipts, because they
/// are what records that the plant actually drew the Ausfallvergütung in a month
/// rather than merely being configured for it.
///
/// The current period counts even before its receipt exists — the cap is on the
/// Inanspruchnahme, and the month being settled is one.
///
/// # Errors
/// Propagates the query error.
pub async fn ausfallverguetung_nutzung(
    conn: &mut sqlx::PgConnection,
    tr_id: &str,
    tenant: &str,
    billing_year: i16,
    billing_month: i16,
) -> anyhow::Result<crate::sect52::AusfallverguetungNutzung> {
    let monate: Vec<(i16, i16)> = sqlx::query_as(
        r"SELECT billing_year, billing_month
            FROM settlement_receipts
           WHERE tr_id = $1 AND tenant = $2
             AND settlement_model = $3
             AND status = 'calculated'
             AND (billing_year, billing_month) < ($4, $5)
           ORDER BY billing_year DESC, billing_month DESC
           LIMIT 24",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(crate::models::AUSFALLVERGUETUNG)
    .bind(billing_year)
    .bind(billing_month)
    .fetch_all(&mut *conn)
    .await
    .context("read Ausfallvergütung history")?;

    // Walk backwards from the month before this one; the run ends at the first gap.
    let mut monate_am_stueck = 1;
    let (mut y, mut m) = (billing_year, billing_month);
    for (ry, rm) in &monate {
        let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
        if (*ry, *rm) != (py, pm) {
            break;
        }
        monate_am_stueck += 1;
        (y, m) = (py, pm);
    }

    let monate_im_jahr =
        1 + u32::try_from(monate.iter().filter(|(ry, _)| *ry == billing_year).count())
            .unwrap_or(u32::MAX);

    Ok(crate::sect52::AusfallverguetungNutzung {
        monate_am_stueck,
        monate_im_jahr,
    })
}

pub async fn list_settlement_receipts(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    year: Option<i16>,
    month: Option<i16>,
    limit: i64,
) -> anyhow::Result<Vec<serde_json::Value>> {
    // `$3 IS NULL OR billing_year = $3` rather than a built-up WHERE clause:
    // the two columns are the leading pair of the receipts index and both
    // filters are optional, so one prepared statement serves all four
    // combinations without concatenating SQL.
    let rows = sqlx::query(
        r"SELECT id, tr_id, billing_year, billing_month, settlement_model,
                 einspeisemenge_kwh, settlement_eur, status, settled_at, gutschrift_nummer,
                 is_correction, correction_of, correction_reason
          FROM settlement_receipts
          WHERE tr_id = $1 AND tenant = $2
            AND ($3::smallint IS NULL OR billing_year  = $3)
            AND ($4::smallint IS NULL OR billing_month = $4)
          ORDER BY billing_year DESC, billing_month DESC, settled_at DESC
          LIMIT $5",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(year)
    .bind(month)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_settlement_receipts")?;

    Ok(rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.try_get::<Uuid, _>("id").ok().map(|u| u.to_string()),
                "tr_id": r.try_get::<String, _>("tr_id").ok(),
                "billing_year": r.try_get::<i16, _>("billing_year").ok(),
                "billing_month": r.try_get::<i16, _>("billing_month").ok(),
                "settlement_model": r.try_get::<String, _>("settlement_model").ok(),
                "einspeisemenge_kwh": r.try_get::<Option<Decimal>, _>("einspeisemenge_kwh").ok().flatten(),
                "settlement_eur": r.try_get::<Option<Decimal>, _>("settlement_eur").ok().flatten(),
                "status": r.try_get::<String, _>("status").ok(),
                "settled_at": r.try_get::<OffsetDateTime, _>("settled_at").ok().map(|t| t.to_string()),
                "gutschrift_nummer": r.try_get::<Option<String>, _>("gutschrift_nummer").ok().flatten(),
                // Without these an original and the correction that superseded it
                // are two rows for the same month with no way to tell which is
                // which — the history reads as a double payment.
                "is_correction": r.try_get::<bool, _>("is_correction").ok(),
                "correction_of": r.try_get::<Option<Uuid>, _>("correction_of").ok().flatten().map(|u| u.to_string()),
                "correction_reason": r.try_get::<Option<String>, _>("correction_reason").ok().flatten(),
            })
        })
        .collect())
}

// ── EPEX monthly prices ───────────────────────────────────────────────────────

/// Look up the statutory net Vergütungssatz for a technology, size band,
/// Vergütungsform and **Inbetriebnahmedatum**.
///
/// `verguetungsform` is **not** optional. Überschuss- and Volleinspeisung rates
/// for the same band and window differ by the § 48 Abs. 2a uplift — 8,11 vs.
/// 12,86 ct/kWh for a ≤ 10 kWp roof plant commissioned in the 1 February 2024
/// window — so omitting it would leave the choice to row order.
///
/// Band bounds are **inclusive at the top**: every Staffel in the EEG reads „bis
/// einschließlich einer Leistung von X", so a plant of exactly 10 kWp is a
/// § 48 Abs. 2 Nr. 1 plant and not a Nr. 2 one. The lowest band that still
/// covers the capacity therefore wins.
pub async fn lookup_verguetungssatz(
    pool: &PgPool,
    erzeugungsart: &str,
    verguetungsform: &str,
    leistung_kwp: Decimal,
    inbetriebnahme: &str,
) -> anyhow::Result<Option<Decimal>> {
    use time::format_description::well_known::Iso8601;
    let date = Date::parse(inbetriebnahme, &Iso8601::DEFAULT)
        .context("parse inbetriebnahme for lookup")?;

    let row = sqlx::query(
        r"SELECT verguetungssatz_ct
          FROM eeg_verguetungssaetze
          WHERE erzeugungsart   = $1
            AND verguetungsform = $2
            AND leistung_min_kwp <= $3
            AND (leistung_max_kwp IS NULL OR leistung_max_kwp >= $3)
            AND billing_start <= $4
            AND (billing_end IS NULL OR billing_end >= $4)
          ORDER BY billing_start DESC, leistung_min_kwp ASC
          LIMIT 1",
    )
    .bind(erzeugungsart)
    .bind(verguetungsform)
    .bind(leistung_kwp)
    .bind(date)
    .fetch_optional(pool)
    .await
    .context("lookup_verguetungssatz")?;

    Ok(row.and_then(|r| r.try_get::<Decimal, _>("verguetungssatz_ct").ok()))
}

/// Upsert a technology-specific Jahresmarktwert price (Anlage 1 Nr. 3/4 EEG 2023).
///
/// `erzeugungsart` must match a value from `eeg_anlagen.erzeugungsart` (e.g. `WIND_ONSHORE`,
/// `SOLAR_AUFDACH`) or the special value `DEFAULT` for the generic fallback row.
/// Published by ÜNB at netztransparenz.de.
/// One ÜNB Marktwert row, as a caller states it.
#[derive(Debug, Clone, Copy)]
pub struct MarktwertImport<'a> {
    /// Calendar year the figure belongs to.
    pub year: i16,
    /// Anlage 1 Nr. 3 or Nr. 4.
    pub serie: eeg_billing::Marktwertserie,
    /// The month, for a Monatsmarktwert; `None` for a Jahresmarktwert.
    pub month: Option<i16>,
    /// Plant technology, or `DEFAULT` for the generic fallback row.
    pub erzeugungsart: &'a str,
    /// The figure in ct/kWh.
    pub avg_ct_kwh: Decimal,
    /// An ÜNB running estimate rather than the published binding figure.
    pub vorlaeufig: bool,
    /// Where it came from, for the audit trail.
    pub source: &'a str,
}

/// Store or replace one ÜNB Marktwert.
///
/// # Errors
///
/// Database failures, including the `art`/`billing_month` consistency CHECK.
pub async fn upsert_marktwert(pool: &PgPool, mw: MarktwertImport<'_>) -> anyhow::Result<()> {
    let MarktwertImport {
        year,
        serie,
        month,
        erzeugungsart,
        avg_ct_kwh,
        vorlaeufig,
        source,
    } = mw;
    // A Monatsmarktwert is final when it is published; only the Jahresmarktwert
    // has a running estimate. The CHECK enforces it too — this keeps the caller
    // from having to know.
    let vorlaeufig = vorlaeufig && serie == eeg_billing::Marktwertserie::Jahresmarktwert;
    sqlx::query(
        r"INSERT INTO marktwert_preise
            (billing_year, art, billing_month, erzeugungsart, avg_ct_kwh, vorlaeufig, source)
          VALUES ($1, $2, $3, $4, $5, $6, $7)
          ON CONFLICT (billing_year, art, erzeugungsart, COALESCE(billing_month, 0)) DO UPDATE
          SET avg_ct_kwh   = EXCLUDED.avg_ct_kwh,
              vorlaeufig   = EXCLUDED.vorlaeufig,
              source       = EXCLUDED.source,
              imported_at  = now()",
    )
    .bind(year)
    .bind(serie.as_db_str())
    .bind(month)
    .bind(erzeugungsart)
    .bind(avg_ct_kwh)
    .bind(vorlaeufig)
    .bind(source)
    .execute(pool)
    .await
    .context("upsert_marktwert")?;
    Ok(())
}

/// Fetch one stored Marktwert (exact technology match only — no `DEFAULT`
/// fallback, because this answers „what did we import", not „what applies").
///
/// # Errors
///
/// Database failures.
pub async fn fetch_marktwert_single(
    pool: &PgPool,
    year: i16,
    serie: eeg_billing::Marktwertserie,
    month: Option<i16>,
    erzeugungsart: &str,
) -> anyhow::Result<Option<(Decimal, bool)>> {
    sqlx::query_as(
        "SELECT avg_ct_kwh, vorlaeufig FROM marktwert_preise
          WHERE billing_year = $1 AND art = $2
            AND billing_month IS NOT DISTINCT FROM $3
            AND erzeugungsart = $4",
    )
    .bind(year)
    .bind(serie.as_db_str())
    .bind(month)
    .bind(erzeugungsart)
    .fetch_optional(pool)
    .await
    .context("fetch_marktwert_single")
}

/// Every settled period computed on a **provisional** Jahresmarktwert.
///
/// Anlage 1 Nr. 2 Satz 2 EEG 2023 prices a post-2023 plant's Marktprämie from
/// the Jahresmarktwert, and the binding figure exists only once the year is
/// over. A month settled before that is correct as far as it could be and wrong
/// as soon as the ÜNB publish — this is the list of what to recompute, which
/// `POST /api/v1/anlagen/{tr_id}/settlements/{year}/{month}/correction` then
/// does, one plant at a time.
///
/// # Errors
///
/// Database failures.
pub async fn list_marktwert_nachbewertung(
    pool: &PgPool,
    tenant: &str,
    year: i16,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let rows = sqlx::query(
        r"SELECT tr_id, billing_month, settlement_model, settlement_eur
          FROM settlement_receipts
          WHERE tenant = $1 AND billing_year = $2
            AND marktwert_vorlaeufig
            AND is_correction = false
          ORDER BY tr_id, billing_month",
    )
    .bind(tenant)
    .bind(year)
    .fetch_all(pool)
    .await
    .context("list_marktwert_nachbewertung")?;
    use sqlx::Row as _;
    Ok(rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "tr_id": r.get::<String, _>("tr_id"),
                "billing_month": r.get::<i16, _>("billing_month"),
                "settlement_model": r.get::<String, _>("settlement_model"),
                "settlement_eur": r.get::<Option<Decimal>, _>("settlement_eur"),
            })
        })
        .collect())
}

pub async fn upsert_epex_price(
    pool: &PgPool,
    year: i16,
    month: i16,
    avg_ct_kwh: Decimal,
    source: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r"INSERT INTO epex_monthly_prices (billing_year, billing_month, avg_ct_kwh, source)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (billing_year, billing_month) DO UPDATE
          SET avg_ct_kwh = EXCLUDED.avg_ct_kwh,
              source     = EXCLUDED.source,
              imported_at = now()",
    )
    .bind(year)
    .bind(month)
    .bind(avg_ct_kwh)
    .bind(source)
    .execute(pool)
    .await
    .context("upsert_epex_price")?;
    Ok(())
}

pub async fn fetch_epex_price(
    pool: &PgPool,
    year: i16,
    month: i16,
) -> anyhow::Result<Option<Decimal>> {
    let row = sqlx::query(
        "SELECT avg_ct_kwh FROM epex_monthly_prices WHERE billing_year = $1 AND billing_month = $2",
    )
    .bind(year)
    .bind(month)
    .fetch_optional(pool)
    .await
    .context("fetch_epex_price")?;
    Ok(row.and_then(|r| r.try_get::<Decimal, _>("avg_ct_kwh").ok()))
}

// ── EPEX Spot per-interval prices (§51 Negativpreisregel) ────────────────────

/// One EPEX day-ahead spot price interval.
#[derive(Debug, Clone)]
pub struct SpotPrice {
    pub delivery_start: time::OffsetDateTime,
    pub resolution_min: i16,
    pub price_ct_kwh: Decimal,
}

/// Bulk-upsert spot prices (one billing month is ~2 976 quarter-hours).
///
/// # Errors
/// Propagates the batched insert error.
pub async fn upsert_spot_prices(
    pool: &PgPool,
    prices: &[SpotPrice],
    source: &str,
) -> anyhow::Result<u64> {
    if prices.is_empty() {
        return Ok(0);
    }
    let starts: Vec<time::OffsetDateTime> = prices.iter().map(|p| p.delivery_start).collect();
    let res: Vec<i16> = prices.iter().map(|p| p.resolution_min).collect();
    let cts: Vec<Decimal> = prices.iter().map(|p| p.price_ct_kwh).collect();
    let n = sqlx::query(
        r"INSERT INTO epex_spot_prices (delivery_start, resolution_min, price_ct_kwh, source)
          SELECT * FROM UNNEST($1::timestamptz[], $2::smallint[], $3::numeric[]) AS t(s, r, c),
                       LATERAL (SELECT $4::text) AS src(source)
          ON CONFLICT (delivery_start) DO UPDATE
          SET resolution_min = EXCLUDED.resolution_min,
              price_ct_kwh   = EXCLUDED.price_ct_kwh,
              source         = EXCLUDED.source,
              imported_at    = now()",
    )
    .bind(&starts)
    .bind(&res)
    .bind(&cts)
    .bind(source)
    .execute(pool)
    .await
    .context("upsert_spot_prices")?
    .rows_affected();
    Ok(n)
}

/// Calendar days (German local time) touched by an uninterrupted negative-price
/// period in `[from, to)`.
///
/// §51 Abs. 3 EEG reduces an Ausfallvergütung claim by 5 % for each calendar day
/// on which such a period fell "ganz oder teilweise", so a run that straddles
/// midnight counts both days.
///
/// # Errors
/// Propagates the query error.
pub async fn negative_price_calendar_days(
    pool: &PgPool,
    from: time::OffsetDateTime,
    to: time::OffsetDateTime,
) -> anyhow::Result<u32> {
    let spot = fetch_spot_prices(pool, from, to).await?;
    let tage: std::collections::BTreeSet<time::Date> = spot
        .iter()
        .filter(|(_, price)| price.is_sign_negative())
        .map(|(t, _)| mako_fristen::berlin_date(*t))
        .collect();
    Ok(u32::try_from(tage.len()).unwrap_or(u32::MAX))
}

/// Fetch the spot prices whose delivery start falls in `[from, to)`, ascending,
/// **expanded to the quarter-hour grid**.
///
/// The store permits `resolution_min = 60`, and §51 matches feed-in
/// quarter-hours against a price by their start instant. An hourly row therefore
/// matched only the `:00` quarter, and three quarters of every negative hour
/// went unnoticed — the plant was paid for kWh §51 says it must not be.
///
/// # Errors
/// Propagates the query error.
pub async fn fetch_spot_prices(
    pool: &PgPool,
    from: time::OffsetDateTime,
    to: time::OffsetDateTime,
) -> anyhow::Result<Vec<(time::OffsetDateTime, Decimal)>> {
    let rows = sqlx::query(
        r"SELECT delivery_start, resolution_min, price_ct_kwh FROM epex_spot_prices
          WHERE delivery_start >= $1 AND delivery_start < $2
          ORDER BY delivery_start",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
    .context("fetch_spot_prices")?;

    let mut out: Vec<(time::OffsetDateTime, Decimal)> = Vec::with_capacity(rows.len());
    for r in rows {
        let Ok(start) = r.try_get::<time::OffsetDateTime, _>("delivery_start") else {
            continue;
        };
        let Ok(price) = r.try_get::<Decimal, _>("price_ct_kwh") else {
            continue;
        };
        let resolution = r.try_get::<i16, _>("resolution_min").unwrap_or(15).max(15);
        // The price is constant across the market time unit, so every
        // quarter-hour it covers carries it.
        let slots = i64::from(resolution) / 15;
        for i in 0..slots.max(1) {
            out.push((start + time::Duration::minutes(i * 15), price));
        }
    }
    out.retain(|(t, _)| *t < to);
    Ok(out)
}

// ── §36h Abs. 2 EEG 2023 — Wind Standortgüte re-evaluation ───────────────────

/// Record a §36h Abs. 2 Standortgüte re-evaluation (year 6/11/16) on a wind plant.
///
/// Upserts the re-evaluation into `wind_guetefaktor_reevaluations` (replacing any
/// entry for the same effective year) and reports whether it triggers a
/// reconciliation of the reviewed five-year period (§36h Abs. 2 Satz 2: the
/// recomputed Gütefaktor deviates > 2 pp from the previous one). The effective
/// Korrekturfaktor per billing period is then derived by `build_settle_input`.
///
/// Returns `None` when the plant does not exist, else
/// `Some((reconciliation_required, previous_guetefaktor))`.
///
/// # Errors
/// Propagates query/serialisation errors.
pub async fn record_wind_reevaluation(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    wirksam_ab_jahr: i16,
    guetefaktor: Decimal,
    korrekturfaktor: Option<Decimal>,
) -> anyhow::Result<Option<(bool, Option<Decimal>)>> {
    let Some(row) = sqlx::query(
        "SELECT wind_guetegrad, wind_guetefaktor_reevaluations
           FROM eeg_anlagen WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(tr_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("load wind re-evaluation state")?
    else {
        return Ok(None);
    };

    let guetegrad: Option<Decimal> = row.try_get("wind_guetegrad").ok();
    let existing: serde_json::Value = row
        .try_get("wind_guetefaktor_reevaluations")
        .unwrap_or_else(|_| serde_json::json!([]));
    let mut reevals: Vec<eeg_billing::wind::GuetefaktorReeval> =
        serde_json::from_value(existing).unwrap_or_default();

    // The previous Gütefaktor is the latest prior re-evaluation, else the initial
    // Standortgütegrad measured at commissioning.
    let previous_gf = reevals
        .iter()
        .max_by_key(|r| r.wirksam_ab_jahr)
        .map(|r| r.guetefaktor)
        .or(guetegrad);
    let reconciliation = previous_gf
        .is_some_and(|p| eeg_billing::wind::reevaluation_requires_reconciliation(p, guetefaktor));

    // Upsert: one entry per effective year.
    let jahr = u8::try_from(wirksam_ab_jahr).unwrap_or(0);
    reevals.retain(|r| r.wirksam_ab_jahr != jahr);
    // §36h Abs. 3 Nr. 2: the Netzbetreiber settles on the Gutachten's factor.
    // Südregion is not modelled here, so the fallback takes the Nr. 3 floor.
    reevals.push(eeg_billing::wind::GuetefaktorReeval {
        wirksam_ab_jahr: jahr,
        guetefaktor,
        korrekturfaktor: korrekturfaktor.unwrap_or_else(|| {
            eeg_billing::wind::korrekturfaktor_fuer_guetefaktor(guetefaktor, false)
        }),
    });
    let json = serde_json::to_value(&reevals).context("serialise re-evaluations")?;

    sqlx::query(
        "UPDATE eeg_anlagen SET wind_guetefaktor_reevaluations = $3, updated_at = now()
           WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(json)
    .execute(pool)
    .await
    .context("update wind re-evaluations")?;

    Ok(Some((reconciliation, previous_gf)))
}

// ── Jahresabrechnung ─────────────────────────────────────────────────────────

/// Annual reconciliation over one plant's monthly settlements.
///
/// Derived from `settlement_receipts` rather than recomputed: the monthly runs
/// are what created the payment obligation, so a statement that recalculated
/// from scratch could disagree with what was actually paid.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Jahresabrechnung {
    /// Plant this statement covers.
    pub tr_id: String,
    /// Calendar year.
    pub billing_year: i16,
    /// Total energy fed in across the year.
    pub einspeisemenge_kwh: Decimal,
    /// Total Vergütung across the year.
    pub settlement_eur: Decimal,
    /// §52 EEG 2023 Pflichtzahlungen — a separate claim, never netted into
    /// `settlement_eur`.
    pub pflichtzahlung_eur: Decimal,
    /// How many of the twelve months carry a settlement.
    pub months_settled: i16,
    /// Which of the months the plant was **entitled to** carry no receipt.
    ///
    /// Bounded by the commissioning date and the Förderende: a plant commissioned
    /// in June is not missing January, and demanding twelve made its first year
    /// permanently `vorlaeufig`.
    pub missing_months: Vec<i16>,
    /// §51a quarter-hours accrued toward the Vergütungszeitraum.
    pub verlaengerungsanspruch_qh: i64,
    /// Corrections issued in the year (§ 147 AO / GoBD signal).
    pub correction_count: i16,
    /// `vorlaeufig` until every month is settled.
    pub status: String,
}

/// Build and store the annual reconciliation for one plant and year.
///
/// Each month contributes its **latest** receipt — the correction where one
/// exists, the original otherwise — so the statement equals what was actually
/// paid. Re-running replaces it, so it can be produced provisionally during the
/// year and finalised once the last entitled month is settled.
///
/// # Errors
///
/// Returns an error when the plant is unknown or the database is unreachable.
pub async fn run_jahresabrechnung(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    year: i16,
) -> anyhow::Result<Jahresabrechnung> {
    // The plant's entitlement period bounds which months the year can have.
    let Some(anlage) = fetch_anlage(pool, tenant, tr_id).await? else {
        anyhow::bail!("plant {tr_id} not found");
    };

    // One row per month: the **latest** receipt for it — the correction where one
    // exists, the original otherwise. A correction is a separate row that neither
    // adds to its month nor replaces the original in place, so exactly one of the
    // two is taken and never both.
    //
    // The months and the correction count come from **one** statement, so the
    // count always describes the rows it was summed with.
    let rows: Vec<(i16, Decimal, Decimal, Decimal, i64, i64)> = sqlx::query_as(
        r"WITH periode AS (
              SELECT * FROM settlement_receipts
               WHERE tr_id = $1 AND tenant = $2 AND billing_year = $3
          ), korrekturen AS (
              SELECT count(*) AS n FROM periode WHERE is_correction
          )
          SELECT DISTINCT ON (billing_month)
                 billing_month,
                 COALESCE(einspeisemenge_kwh, 0),
                 COALESCE(settlement_eur, 0),
                 COALESCE(pflichtzahlung_eur, 0),
                 COALESCE(verlaengerungsanspruch_qh, 0),
                 (SELECT n FROM korrekturen)
            FROM periode
           ORDER BY billing_month, is_correction DESC, settled_at DESC",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(year)
    .fetch_all(pool)
    .await
    .context("read settlement receipts for the year")?;

    // No receipts at all means no corrections either — the count rides on the
    // rows, so an empty year yields zero rather than a second query.
    let correction_count: i16 = rows.first().map_or(0, |(_, _, _, _, _, n)| {
        i16::try_from(*n).unwrap_or(i16::MAX)
    });

    let mut settled_months = std::collections::BTreeSet::new();
    let mut einspeisemenge_kwh = Decimal::ZERO;
    let mut settlement_eur = Decimal::ZERO;
    let mut pflichtzahlung_eur = Decimal::ZERO;
    let mut verlaengerungsanspruch_qh: i64 = 0;

    for (month, kwh, eur, pflicht, qh, _) in rows {
        settled_months.insert(month);
        einspeisemenge_kwh += kwh;
        settlement_eur += eur;
        pflichtzahlung_eur += pflicht;
        verlaengerungsanspruch_qh += qh;
    }

    // Only the months the plant was actually entitled to can be missing. A plant
    // commissioned in June has no January receipt and never will, so demanding
    // twelve made its first year permanently `vorlaeufig` with five months
    // reported missing that were never owed — and the same at the Förderende.
    let erster = if anlage.inbetriebnahme.year() == i32::from(year) {
        anlage.inbetriebnahme.month() as i16
    } else if anlage.inbetriebnahme.year() > i32::from(year) {
        13 // commissioned after this year: no month is owed
    } else {
        1
    };
    // A plant with no calendar Förderende — a KWK plant, § 8 KWKG — is owed
    // every month of the year that follows its Inbetriebnahme.
    let letzter = match anlage.foerderendedatum {
        Some(ende) if ende.year() == i32::from(year) => ende.month() as i16,
        Some(ende) if ende.year() < i32::from(year) => 0,
        _ => 12,
    };
    let missing_months: Vec<i16> = (erster..=letzter)
        .filter(|m| !settled_months.contains(m))
        .collect();
    let months_settled = i16::try_from(settled_months.len()).unwrap_or(i16::MAX);
    let status = if missing_months.is_empty() {
        "endgueltig"
    } else {
        "vorlaeufig"
    };

    sqlx::query(
        r"INSERT INTO jahresabrechnungen
              (tr_id, tenant, billing_year, einspeisemenge_kwh, settlement_eur,
               pflichtzahlung_eur, months_settled, missing_months,
               verlaengerungsanspruch_qh, correction_count, status, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now())
          ON CONFLICT (tr_id, tenant, billing_year) DO UPDATE
          SET einspeisemenge_kwh        = EXCLUDED.einspeisemenge_kwh,
              settlement_eur            = EXCLUDED.settlement_eur,
              pflichtzahlung_eur        = EXCLUDED.pflichtzahlung_eur,
              months_settled            = EXCLUDED.months_settled,
              missing_months            = EXCLUDED.missing_months,
              verlaengerungsanspruch_qh = EXCLUDED.verlaengerungsanspruch_qh,
              correction_count          = EXCLUDED.correction_count,
              status                    = EXCLUDED.status,
              updated_at                = now()",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(year)
    .bind(einspeisemenge_kwh)
    .bind(settlement_eur)
    .bind(pflichtzahlung_eur)
    .bind(months_settled)
    .bind(&missing_months)
    .bind(verlaengerungsanspruch_qh)
    .bind(correction_count)
    .bind(status)
    .execute(pool)
    .await
    .context("store Jahresabrechnung")?;

    Ok(Jahresabrechnung {
        tr_id: tr_id.to_owned(),
        billing_year: year,
        einspeisemenge_kwh,
        settlement_eur,
        pflichtzahlung_eur,
        months_settled,
        missing_months,
        verlaengerungsanspruch_qh,
        correction_count,
        status: status.to_owned(),
    })
}

// ── §§53b–54 EEG 2023 — recording and inspecting the facts that cut the AW ───

/// Record a Regionalnachweis period (§53b EEG 2023 i.V.m. §79a).
///
/// No amount is taken: §53b fixes 0,1 ct/kWh, so there is nothing here a
/// data-entry error could use to invent a different deduction.
pub async fn record_regionalnachweis(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    nachweis_ref: &str,
    effective_from: time::Date,
    effective_until: Option<time::Date>,
) -> anyhow::Result<uuid::Uuid> {
    sqlx::query_scalar(
        r"INSERT INTO eeg_regionalnachweise
              (tr_id, tenant, nachweis_ref, effective_from, effective_until)
          VALUES ($1, $2, $3, $4, $5)
          RETURNING id",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(nachweis_ref)
    .bind(effective_from)
    .bind(effective_until)
    .fetch_one(pool)
    .await
    .context("record §53b Regionalnachweis")
}

/// Record a granted Stromsteuerbefreiung (§53c EEG 2023).
///
/// The amount is stored because the statute ties the cut to "die Höhe der pro
/// Kilowattstunde gewährten Stromsteuerbefreiung". The schema caps it at the
/// §3 StromStG full rate.
pub async fn record_stromsteuerbefreiung(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    befreiung_ct_kwh: Decimal,
    rechtsgrundlage: &str,
    effective_from: time::Date,
    effective_until: Option<time::Date>,
) -> anyhow::Result<uuid::Uuid> {
    sqlx::query_scalar(
        r"INSERT INTO eeg_stromsteuerbefreiungen
              (tr_id, tenant, befreiung_ct_kwh, rechtsgrundlage, effective_from, effective_until)
          VALUES ($1, $2, $3, $4, $5, $6)
          RETURNING id",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(befreiung_ct_kwh)
    .bind(rechtsgrundlage)
    .bind(effective_from)
    .bind(effective_until)
    .fetch_one(pool)
    .await
    .context("record §53c Stromsteuerbefreiung")
}

/// Record §54 solar first-segment defects for a period.
#[allow(clippy::too_many_arguments)]
pub async fn record_sect54_defekt(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    defekte: eeg_billing::Sect54SolarReduction,
    bnetza_ref: Option<&str>,
    notes: Option<&str>,
    effective_from: time::Date,
    effective_until: Option<time::Date>,
) -> anyhow::Result<uuid::Uuid> {
    sqlx::query_scalar(
        r"INSERT INTO eeg_sect54_solar_defekte
              (tr_id, tenant, zahlungsberechtigung_nach_18_monaten, flurstueck_abweichung,
               agri_nutzungsnachweis_fehlt, landesverordnung_nicht_erfuellt,
               bnetza_ref, notes, effective_from, effective_until)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
          RETURNING id",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(defekte.zahlungsberechtigung_nach_18_monaten)
    .bind(defekte.flurstueck_abweichung)
    .bind(defekte.agri_nutzungsnachweis_fehlt)
    .bind(defekte.landesverordnung_nicht_erfuellt)
    .bind(bnetza_ref)
    .bind(notes)
    .bind(effective_from)
    .bind(effective_until)
    .fetch_one(pool)
    .await
    .context("record §54 solar defect")
}

/// §54 Abs. 3 Satz 2/3 — close a defect period because the Nachweis arrived.
///
/// The statute makes the 2,5 ct deduction lapse *for the future* once the proof
/// is supplied, and retroactively for the periods it covers. Closing the period
/// is therefore the correct record: deleting the row would erase that the plant
/// was ever short, which the §147 AO audit trail needs.
///
/// Returns `false` when no open row matches.
pub async fn close_sect54_defekt(
    pool: &PgPool,
    tenant: &str,
    id: uuid::Uuid,
    effective_until: time::Date,
) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        r"UPDATE eeg_sect54_solar_defekte
             SET effective_until = $3
           WHERE id = $1 AND tenant = $2 AND effective_until IS NULL",
    )
    .bind(id)
    .bind(tenant)
    .bind(effective_until)
    .execute(pool)
    .await
    .context("close §54 defect period")?
    .rows_affected();
    Ok(rows > 0)
}

/// Everything cutting a plant's anzulegender Wert on a given date, with the
/// statutory amounts spelled out.
///
/// This is the explainability path for the §§53b–54 band: a settlement changes
/// silently when one of these rows exists, so an operator needs to be able to
/// ask what is in force without reading the settlement back.
pub async fn aw_reduktionen_am(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    on: time::Date,
) -> anyhow::Result<serde_json::Value> {
    let mut conn = pool.acquire().await.context("acquire for AW reductions")?;
    let ctx = load_aw_reductions(&mut conn, tr_id, tenant, on).await?;

    let mut cuts = Vec::new();
    if ctx.regionalnachweis_ausgestellt {
        cuts.push(serde_json::json!({
            "paragraph": "§53b EEG 2023",
            "grund": "Regionalnachweis (§79a EEG) ausgestellt",
            "abzug_ct_kwh": eeg_billing::aw_reductions::SECT53B_REGIONALNACHWEIS_CT_KWH,
            "hinweis": "gilt nur bei gesetzlich bestimmtem anzulegendem Wert",
        }));
    }
    if let Some(ct) = ctx.stromsteuerbefreiung_ct_kwh {
        cuts.push(serde_json::json!({
            "paragraph": "§53c EEG 2023",
            "grund": "Stromsteuerbefreiung für durchgeleiteten Strom",
            "abzug_ct_kwh": ct.min(eeg_billing::aw_reductions::STROMSTEUER_VOLLSATZ_CT_KWH),
        }));
    }
    if let Some(s54) = ctx.sect54_solar {
        for (flag, absatz, grund, ct) in [
            (
                s54.zahlungsberechtigung_nach_18_monaten,
                "§54 Abs. 1 EEG 2023",
                "Zahlungsberechtigung erst nach dem 18. Kalendermonat beantragt",
                Some(eeg_billing::aw_reductions::SECT54_ABS1_ABS2_CT_KWH),
            ),
            (
                s54.flurstueck_abweichung,
                "§54 Abs. 2 EEG 2023",
                "Standort weicht von den Gebots-Flurstücken ab",
                Some(eeg_billing::aw_reductions::SECT54_ABS1_ABS2_CT_KWH),
            ),
            (
                s54.agri_nutzungsnachweis_fehlt,
                "§54 Abs. 3 EEG 2023",
                "Nachweis der gleichzeitigen landwirtschaftlichen Nutzung fehlt",
                Some(eeg_billing::aw_reductions::SECT54_ABS3_CT_KWH),
            ),
            (
                s54.landesverordnung_nicht_erfuellt,
                "§54 Abs. 4 EEG 2023",
                "Landesverordnung nach §37c Abs. 2 nicht erfüllt — AW auf null",
                None,
            ),
        ] {
            if flag {
                cuts.push(serde_json::json!({
                    "paragraph": absatz,
                    "grund": grund,
                    "abzug_ct_kwh": ct,
                    "setzt_aw_auf_null": ct.is_none(),
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "tr_id": tr_id,
        "stichtag": on.to_string(),
        "reduktionen": cuts,
        "hinweis": "Alle §§53b–54-Abzüge mindern den anzulegenden Wert vor der \
                    Vergütungsformel; die gleitende Marktprämie ist bei null begrenzt.",
    }))
}

// ── §52 Abs. 1 EEG 2023 — the Pflichtverstoß register ────────────────────────

/// One recorded §52 Abs. 1 Pflichtverstoß.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PflichtverstossRecord {
    /// Row id — stable across a cure, so the history is followable.
    pub id: Uuid,
    /// Which Nummer of §52 Abs. 1.
    pub typ: eeg_billing::SanktionsTyp,
    /// First day the breach subsisted. §52 Abs. 2 counts calendar months from
    /// here.
    pub beginn: Date,
    /// Day the obligation was met. `None` while the breach is open; once set,
    /// §52 Abs. 3 Satz 1 Nr. 1 reduces the charge to 2 €/kW **back to
    /// `beginn`** for the Nummern that admit it.
    pub behoben_am: Option<Date>,
    /// §52 Abs. 3 Satz 2 — the breach was caused by a technical defect, so the
    /// defect month and the following one are waived (Nr. 1/3/4/8, breaches
    /// after 31.12.2023 only). The operator carries the Beweislast.
    pub technischer_defekt: bool,
    /// What was found, and by whom.
    pub notiz: Option<String>,
}

impl PflichtverstossRecord {
    /// Whether this record bears on the given billing month.
    ///
    /// §52 Abs. 2 charges „pro Kalendermonat, in dem ganz oder zeitweise ein
    /// Pflichtverstoß … vorliegt oder andauert", so a breach cured on the 5th
    /// still counts for that month — and one cured before the month began does
    /// not count for it at all.
    #[must_use]
    pub fn gilt_fuer(&self, billing_date: Date) -> bool {
        let monat = (billing_date.year(), billing_date.month());
        let nach_beginn = (self.beginn.year(), self.beginn.month()) <= monat;
        let vor_ende = self
            .behoben_am
            .is_none_or(|b| (b.year(), b.month()) >= monat);
        nach_beginn && vor_ende
    }
}

fn pflichtverstoss_from_row(row: &sqlx::postgres::PgRow) -> Option<PflichtverstossRecord> {
    use sqlx::Row as _;
    Some(PflichtverstossRecord {
        id: row.try_get("id").ok()?,
        typ: eeg_billing::SanktionsTyp::from_db_str(&row.try_get::<String, _>("typ").ok()?)?,
        beginn: row.try_get("beginn").ok()?,
        behoben_am: row.try_get("behoben_am").ok()?,
        technischer_defekt: row.try_get("technischer_defekt").ok()?,
        notiz: row.try_get("notiz").ok()?,
    })
}

/// Every recorded Pflichtverstoß of one plant, newest breach first.
///
/// # Errors
///
/// Database failures. A row whose `typ` no longer parses is skipped rather than
/// failing the read: it cannot be charged, and refusing the whole settlement
/// because of one unknown token would be worse.
pub async fn list_pflichtverstoesse(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    tr_id: &str,
) -> anyhow::Result<Vec<PflichtverstossRecord>> {
    let rows = sqlx::query(
        r"SELECT id, typ, beginn, behoben_am, technischer_defekt, notiz
          FROM eeg_pflichtverstoesse
          WHERE tenant = $1 AND tr_id = $2
          ORDER BY beginn DESC, erfasst_am DESC",
    )
    .bind(tenant)
    .bind(tr_id)
    .fetch_all(&mut *conn)
    .await
    .context("list Pflichtverstöße")?;
    Ok(rows.iter().filter_map(pflichtverstoss_from_row).collect())
}

/// Open a Pflichtverstoß against a plant.
///
/// # Errors
///
/// Database failures, including the partial unique index when one of that
/// Nummer is already open — the caller turns that into a `409`.
pub async fn open_pflichtverstoss(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    tr_id: &str,
    typ: eeg_billing::SanktionsTyp,
    beginn: Date,
    technischer_defekt: bool,
    notiz: Option<&str>,
) -> Result<PflichtverstossRecord, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query(
        r"INSERT INTO eeg_pflichtverstoesse
              (id, tenant, tr_id, typ, beginn, technischer_defekt, notiz)
          VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(tenant)
    .bind(tr_id)
    .bind(typ.as_db_str())
    .bind(beginn)
    .bind(technischer_defekt)
    .bind(notiz)
    .execute(&mut *conn)
    .await?;
    Ok(PflichtverstossRecord {
        id,
        typ,
        beginn,
        behoben_am: None,
        technischer_defekt,
        notiz: notiz.map(ToOwned::to_owned),
    })
}

/// Close the open Pflichtverstoß of that Nummer.
///
/// `Ok(None)` when none is open — the caller answers `404` rather than opening
/// a closed one, because a cure without a breach is a data error.
///
/// # Errors
///
/// Database failures, including the `behoben_am >= beginn` constraint.
pub async fn close_pflichtverstoss(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    tr_id: &str,
    typ: eeg_billing::SanktionsTyp,
    behoben_am: Date,
) -> Result<Option<PflichtverstossRecord>, sqlx::Error> {
    let row = sqlx::query(
        r"UPDATE eeg_pflichtverstoesse
          SET behoben_am = $4, aktualisiert_am = now()
          WHERE tenant = $1 AND tr_id = $2 AND typ = $3 AND behoben_am IS NULL
          RETURNING id, typ, beginn, behoben_am, technischer_defekt, notiz",
    )
    .bind(tenant)
    .bind(tr_id)
    .bind(typ.as_db_str())
    .bind(behoben_am)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.as_ref().and_then(pflichtverstoss_from_row))
}
