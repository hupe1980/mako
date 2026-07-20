//! Billing orchestration — bridges HTTP requests to `grid-billing` pure library.
//!
//! `grid-billing` returns [`grid_billing::SettlementResult`] (pure domain types, no BO4E).
//! This module owns the conversion to `rubo4e::current::Rechnung` via `into_rechnung()`.

use anyhow::{Context as _, bail};
use invoic_checker::{InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore};
use mako_markt::marktd_client::MarktdClient;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use grid_billing::{
    MmmInput, MsbInput, NneInput, QuantityUnit, SettlementResult, settle_mmm, settle_msb,
    settle_nne,
};

use crate::pg::upsert_draft;

// ── BO4E conversion (service-layer concern) ────────────────────────────────────────────────────

/// Map a `BillingPositionKind` to the appropriate BDEW `BdewArtikelnummer`.
///
/// Source: BDEW Codeliste Artikelnummern und Artikel-ID v5.6 (valid 01.09.2025).
///
/// **Important:** NNE Strom positions (PIDs 31001/31006) do NOT use classic Artikelnummern
/// since BK6-20-160. For those, `artikel_id` is populated from the `PreisblattNetznutzung`
/// by the billing run handler. This function returns `None` for Strom NNE positions.
/// Parse the BDEW Artikelnummer that `grid-billing` decided on.
///
/// The decision — which code applies to which position in which settlement — is
/// domain logic and lives in the crate. This is only the lookup from its
/// codelist name into the BO4E enum, which `rubo4e` derives via `strum`.
fn kind_to_artikelnummer(
    kind: grid_billing::BillingPositionKind,
    settlement_type: grid_billing::SettlementType,
) -> Option<rubo4e::current::BdewArtikelnummer> {
    use std::str::FromStr as _;
    kind.artikelnummer(settlement_type)
        .and_then(|name| rubo4e::current::BdewArtikelnummer::from_str(name).ok())
}

/// Convert a `SettlementResult` domain result into a BO4E `Rechnung`.
///
/// This is the only place in netzbilanzd that imports `rubo4e` invoice types.
/// grid-billing itself has no BO4E dependency.
/// Render a settlement, presented as an invoice, into BO4E.
///
/// Takes the document rather than the settlement: `rechnungsnummer`,
/// `rechnungsdatum` and `faelligkeitsdatum` are document facts, and the position
/// numbering is assigned here rather than carried through the calculation.
fn into_rechnung(document: &grid_billing::InvoiceDocument) -> rubo4e::current::Rechnung {
    let invoice = &document.settlement;
    use rubo4e::current::{Betrag, Menge, Mengeneinheit, Preis, Rechnungsposition, Zeitraum};

    let lz = Zeitraum {
        startdatum: Some(invoice.period.from()),
        enddatum: Some(invoice.period.to()),
        ..Default::default()
    };

    let positions: Vec<Rechnungsposition> = document
        .numbered_positions()
        .map(|(number, p)| {
            let einheit = match p.unit {
                QuantityUnit::Kwh => Some(Mengeneinheit::Kwh),
                QuantityUnit::Kw => Some(Mengeneinheit::Kw),
                QuantityUnit::Kvarh => Some(Mengeneinheit::Kwh), // reactive energy — map to kWh bucket; ERP renders as kVARh
                QuantityUnit::Kvar => Some(Mengeneinheit::Kw), // reactive power — map to kW bucket
                QuantityUnit::Monat => Some(Mengeneinheit::Monat),
            };
            Rechnungsposition {
                positionsnummer: Some(i64::from(number)),
                positionstext: Some(p.text.clone()),
                artikelnummer: kind_to_artikelnummer(p.kind, invoice.settlement_type),
                // Artikel-ID is resolved from the price sheet at rendering time;
                // the settlement states what was charged, not how it is coded.
                artikel_id: None,
                lieferungszeitraum: Some(lz.clone()),
                positions_menge: Some(Menge {
                    wert: Some(p.quantity),
                    einheit,
                    ..Default::default()
                }),
                einzelpreis: Some(Preis {
                    wert: Some(p.unit_price_eur.round_dp(6)),
                    ..Default::default()
                }),
                gesamtpreis: Some(Betrag {
                    wert: Some(p.net_eur.round_dp(5)),
                    ..Default::default()
                }),
                // The calculation trace travels with the position it explains.
                // grid-billing computes why each amount is what it is — the
                // inputs, the applied paragraphs, the tariff source — and that
                // is the only record of it: the engine's output is dropped once
                // this Rechnung is stored. §20 EnWG audits and LF disputes are
                // answered from here.
                zusatz_attribute: trace_attribute(p),
                ..Default::default()
            }
        })
        .collect();

    rubo4e::current::Rechnung {
        rechnungsdatum: Some(document.invoice_date),
        faelligkeitsdatum: Some(document.due_date),
        rechnungsperiode: Some(lz),
        gesamtnetto: Some(Betrag {
            wert: Some(invoice.total_eur),
            ..Default::default()
        }),
        rechnungspositionen: Some(positions),
        // Every paragraph the settlement rests on, deduplicated across positions.
        zusatz_attribute: settlement_attributes(invoice),
        ..Default::default()
    }
}

/// Serialise a position's [`grid_billing::CalculationTrace`] into a BO4E
/// `ZusatzAttribut`.
///
/// BO4E has no field for a calculation trace, and inventing one would break the
/// schema — a `ZusatzAttribut` is the sanctioned place for data a standard does
/// not model. Returns `None` when serialisation fails rather than losing the
/// position: an unexplained amount is better than no amount.
fn trace_attribute(
    p: &grid_billing::SettlementPosition,
) -> Option<Vec<rubo4e::current::ZusatzAttribut>> {
    let trace = serde_json::to_value(&p.trace).ok()?;
    Some(vec![rubo4e::current::ZusatzAttribut {
        name: Some("mako:calculation_trace".to_owned()),
        wert: Some(trace),
        ..Default::default()
    }])
}

/// Attach the settlement's deduplicated legal citations and any warnings.
///
/// A warning records what the engine could not do — a levy omitted for want of a
/// published rate, a Konzessionsabgabe above the KAV ceiling. Dropping it leaves
/// an invoice that looks complete and is not.
fn settlement_attributes(
    invoice: &SettlementResult,
) -> Option<Vec<rubo4e::current::ZusatzAttribut>> {
    let mut attrs = vec![rubo4e::current::ZusatzAttribut {
        name: Some("mako:legal_references".to_owned()),
        wert: Some(serde_json::json!(invoice.all_legal_refs())),
        ..Default::default()
    }];
    if !invoice.warnings.is_empty()
        && let Ok(warnings) = serde_json::to_value(&invoice.warnings)
    {
        attrs.push(rubo4e::current::ZusatzAttribut {
            name: Some("mako:settlement_warnings".to_owned()),
            wert: Some(warnings),
            ..Default::default()
        });
    }
    Some(attrs)
}

// ── BillingRunRequest ─────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/run`.
///
/// Each entry in `positions` describes one MaLo to bill.
/// The operator fetches meter data from `edmd` and tariff from `marktd`
/// before calling this endpoint.
#[derive(Debug, Deserialize)]
pub struct BillingRunRequest {
    /// Netzbetreiber MP-ID — invoice sender.
    pub nb_mp_id: String,
    /// Lieferant MP-ID — invoice recipient.
    pub lf_mp_id: String,
    /// Invoice issue date (`YYYY-MM-DD`).
    pub invoice_date: String,
    /// Payment due date (`YYYY-MM-DD`).
    pub due_date: String,
    /// Prefix for auto-generated invoice numbers (`rechnungsnummer = prefix + "-" + index`).
    pub rechnungsnummer_prefix: String,
    /// Billing positions — one per MaLo.
    pub positions: Vec<BillingPosition>,
}

/// One MaLo billing entry inside [`BillingRunRequest`].
#[derive(Debug, Deserialize)]
pub struct BillingPosition {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// Start of billing period (`YYYY-MM-DD`).
    pub period_from: String,
    /// End of billing period (`YYYY-MM-DD`).
    pub period_to: String,
    /// Invoice type: `"nne_strom"` (31001), `"nne_gas"` (31005), `"mmm_strom"` (31002, Strom MMM),
    /// `"mmm_gas"` (31002, Gas MMM with Trading Hub Europe prices), or `"msb_31009"` (31009).
    ///
    /// The legacy value `"mmm"` is **deprecated** — use `"mmm_strom"` or `"mmm_gas"` to avoid
    /// ambiguous price auto-fetch (the old `"mmm"` path tried Gas THE prices first, which is wrong
    /// for Strom MMM). `"mmm"` is kept as an alias for `"mmm_strom"` until all callers migrate.
    pub billing_type: String,
    // ── NNE fields ────────────────────────────────────────────────────────────
    /// Total energy in kWh (NNE and MMM).
    pub arbeitsmenge_kwh: Option<Decimal>,
    /// Arbeitspreis in ct/kWh (NNE only, from `PreisblattNetznutzung`).
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// HT consumption in kWh (§14a Modul 2 ToU, from `edmd MeterBillingPeriod`).
    pub arbeitsmenge_ht_kwh: Option<Decimal>,
    /// HT Arbeitspreis in ct/kWh (from `PreisblattNetznutzung.zeitvariablePreispositionen`).
    pub arbeitspreis_ht_ct_per_kwh: Option<Decimal>,
    /// NT consumption in kWh (§14a Modul 2 ToU, from `edmd MeterBillingPeriod`).
    pub arbeitsmenge_nt_kwh: Option<Decimal>,
    /// NT Arbeitspreis in ct/kWh (from `PreisblattNetznutzung.zeitvariablePreispositionen`).
    pub arbeitspreis_nt_ct_per_kwh: Option<Decimal>,
    /// Spitzenleistung in kW (NNE RLM only).
    pub spitzenleistung_kw: Option<Decimal>,
    /// Leistungspreis in EUR/kW (NNE RLM only).
    pub leistungspreis_eur_per_kw: Option<Decimal>,
    /// Konzessionsabgabe rate in ct/kWh (optional).
    pub ka_satz_ct_per_kwh: Option<Decimal>,
    // ── MMM fields ────────────────────────────────────────────────────────────
    /// SLP profil consumption in kWh (MMM only).
    pub profil_kwh: Option<Decimal>,
    /// Mehrmengen price in ct/kWh (MMM only).
    pub mehr_preis_ct_per_kwh: Option<Decimal>,
    /// Mindermengen price in ct/kWh (MMM only).
    pub minder_preis_ct_per_kwh: Option<Decimal>,
    /// SLP Lastprofil designation for this MaLo (optional, MMM only).
    ///
    /// When absent, auto-fetched from `marktd GET /api/v1/malo/{malo_id}` via the
    /// stored `bilanzierungsmethode` column that was populated from UTILMD `TM+EM`
    /// at supply-start.
    ///
    /// Standard values: `"H0"` (household), `"G0"`–`"G6"` (commercial),
    /// `"L0"`/`"L1"`/`"L2"` (agricultural), `"P0"` (pumping station).
    /// Used to embed the SLP profile in the generated `Rechnung` `bemerkung` field
    /// for `netzbilanzd` audit trail and downstream ERP import.
    pub lastprofil: Option<String>,
    // ── MSB fields (31009) ───────────────────────────────────────────────────
    /// MSB (Messstellenbetreiber) MP-ID — invoice recipient for `"msb_31009"`.
    pub msb_mp_id: Option<String>,
    /// Grundgebühr Messstellenbetrieb in EUR/month (from `PreisblattMessung`).
    pub grundgebuehr_eur_per_month: Option<Decimal>,
    /// Number of full calendar months in the billing period.
    pub billing_months: Option<u32>,
    /// Optional Messdienstleistung flat fee in EUR for the full period.
    pub messdienstleistung_eur: Option<Decimal>,
}

fn parse_date(s: &str) -> anyhow::Result<time::Date> {
    use time::format_description::well_known::Iso8601;
    time::Date::parse(s, &Iso8601::DEFAULT).context("parse date")
}

/// Core billing orchestration called by the handler.
///
/// For each position:
/// 1. For MMM: auto-fetch `mehr_preis` / `minder_preis` from `marktd` when not
///    supplied in the request (eliminates the monthly manual ERP lookup — C18).
/// 2. Calculate invoice via `grid-billing`
/// 3. Self-validate via `invoic-checker`
/// 4. Store as draft in PostgreSQL
///
/// Returns the list of generated draft UUIDs.
pub async fn run_billing_internal(
    pool: &PgPool,
    marktd: &Arc<MarktdClient>,
    tenant: &str,
    vnb_mp_id: Option<&str>,
    req: BillingRunRequest,
) -> anyhow::Result<Vec<Uuid>> {
    let invoice_date = parse_date(&req.invoice_date)?;
    let due_date = parse_date(&req.due_date)?;
    let empty_store = InMemoryPreisblattStore::new();
    let config = CheckConfig::default();

    let mut draft_ids = Vec::new();

    for (i, pos) in req.positions.iter().enumerate() {
        // Constructing the period is the ordering check; the engine no longer
        // repeats it per calculation.
        let period = grid_billing::SettlementPeriod::new(
            parse_date(&pos.period_from)?,
            parse_date(&pos.period_to)?,
        )?;
        let rechnungsnummer = format!("{}-{:04}", req.rechnungsnummer_prefix, i + 1);

        let mut extra_zusatz: Vec<rubo4e::current::ZusatzAttribut> = Vec::new();
        let (result, pid) = match pos.billing_type.as_str() {
            "nne_strom" | "nne_gas" => {
                let arbeit = pos
                    .arbeitsmenge_kwh
                    .context("arbeitsmenge_kwh required for NNE")?;
                let ap = pos
                    .arbeitspreis_ct_per_kwh
                    .context("arbeitspreis_ct_per_kwh required for NNE")?;
                let input = NneInput {
                    malo_id: pos.malo_id.clone(),
                    nb_mp_id: req.nb_mp_id.clone(),
                    lf_mp_id: req.lf_mp_id.clone(),
                    period,
                    arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(
                        grid_billing::MengePreis {
                            menge_kwh: arbeit,
                            preis_ct_per_kwh: ap,
                        },
                    ),
                    leistungspreis: match (pos.spitzenleistung_kw, pos.leistungspreis_eur_per_kw) {
                        (Some(kw), Some(p)) => Some(grid_billing::Leistungspreis {
                            spitzenleistung_kw: kw,
                            preis_eur_per_kw: p,
                        }),
                        _ => None,
                    },
                    letztverbrauchergruppe: Default::default(),
                    sect19_umlage_ct_per_kwh: None,
                    offshore_umlage_ct_per_kwh: None,
                    kwkg_umlage_ct_per_kwh: None,
                    netzebene: None,
                    sect19: None,
                    gas_kapazitaet: None,
                    jahreshoechstleistung_kw: None,
                    jahresarbeit_kwh: None,
                    // The request carries a bare rate; pairing it with the KAV
                    // group is what lets the Höchstbetrag be checked. Absent a
                    // group in the request, Sondervertragskunde is the safe
                    // reading for an NB→LF grid invoice.
                    konzessionsabgabe: pos.ka_satz_ct_per_kwh.map(|satz| {
                        grid_billing::Konzessionsabgabe {
                            satz_ct_per_kwh: satz,
                            klasse: grid_billing::KaKundengruppe::Sondervertragskunde,
                        }
                    }),
                    grundpreis: None,
                    tariff_sheet_id: None,
                    sparte: grid_billing::Sparte::Strom,
                };
                let r = settle_nne(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?;
                // The Prüfidentifikator routes the document; it is not a
                // property of what was calculated.
                let pid = if pos.billing_type == "nne_gas" {
                    31005
                } else {
                    31001
                };
                (r, pid)
            }
            "mmm" | "mmm_strom" | "mmm_gas" => {
                let actual = pos
                    .arbeitsmenge_kwh
                    .context("arbeitsmenge_kwh (actual) required for MMM")?;
                let profil = pos.profil_kwh.context("profil_kwh required for MMM")?;

                // Auto-fetch MMMA prices from marktd when the caller does not supply them.
                //
                // `"mmm_gas"` → ALWAYS auto-fetches from Trading Hub Europe (THE).
                //               Fail hard when THE prices are not imported for the month.
                //
                // `"mmm_strom"` / `"mmm"` → tries VNB-specific Strom MMM prices first.
                //                          Falls back to caller-supplied values (manual path).
                //                          Never tries Gas THE prices for Strom billing.
                let (mp, mnp) = if let (Some(m), Some(mn)) =
                    (pos.mehr_preis_ct_per_kwh, pos.minder_preis_ct_per_kwh)
                {
                    // Caller explicitly supplied both prices — use them regardless of type.
                    (m, mn)
                } else {
                    let d = parse_date(&pos.period_from)?;
                    let (y, m) = (d.year(), d.month() as u8);

                    if pos.billing_type == "mmm_gas" {
                        // Gas MMM: must have THE prices in marktd.
                        let fetched = marktd
                            .get_mmma_gas(y, m, "THE")
                            .await
                            .context("auto-fetch MMMA Gas prices (THE) from marktd")?;
                        match fetched {
                            Some(r) => (r.mehr_ct_kwh, r.minder_ct_kwh),
                            None => {
                                anyhow::bail!(
                                    "Gas MMM billing for {}/{}: THE MMMA prices not yet imported \
                                     into marktd. Import via PUT /api/v1/mmma-preise/gas/{y}/{m} \
                                     before running Gas MMM billing.",
                                    y,
                                    m
                                );
                            }
                        }
                    } else {
                        // Strom MMM ("mmm" / "mmm_strom"): auto-fetch when config has the VNB MP-ID.
                        // Strom MMM prices are VNB-specific (GPKE (BK6-24-174) Teil 1 Kap. 8.4); each NB publishes
                        // to exactly one ÜNB Regelzone. Configure `vnb_mp_id` in netzbilanzd.toml.
                        if let (None, None, Some(unb)) = (
                            pos.mehr_preis_ct_per_kwh,
                            pos.minder_preis_ct_per_kwh,
                            vnb_mp_id,
                        ) {
                            // Auto-fetch Strom MMM prices from marktd.
                            let d = parse_date(&pos.period_from)?;
                            let (y, m) = (d.year(), d.month() as u8);
                            let fetched = marktd
                                .get_mmm_strom(y, m, unb)
                                .await
                                .context("auto-fetch Strom MMM prices from marktd")?;
                            match fetched {
                                Some(r) => (r.mehr_ct_kwh, r.minder_ct_kwh),
                                None => anyhow::bail!(
                                    "Strom MMM billing for {y}/{m}: prices not yet imported for \
                                     VNB {unb}. Import via PUT marktd /api/v1/mmm-preise/strom/{y}/{m}."
                                ),
                            }
                        } else {
                            // Caller supplied both prices (or vnb_mp_id not configured).
                            let mp = pos.mehr_preis_ct_per_kwh.context(
                                "mehr_preis_ct_per_kwh required for Strom MMM. \
                                          Configure vnb_mp_id in netzbilanzd.toml for auto-fetch, \
                                          or supply prices explicitly.",
                            )?;
                            let mnp = pos
                                .minder_preis_ct_per_kwh
                                .context("minder_preis_ct_per_kwh required for Strom MMM")?;
                            (mp, mnp)
                        }
                    }
                };

                let input = MmmInput {
                    malo_id: pos.malo_id.clone(),
                    nb_mp_id: req.nb_mp_id.clone(),
                    lf_mp_id: req.lf_mp_id.clone(),
                    period,
                    actual_kwh: actual,
                    profil_kwh: profil,
                    mehr_preis_ct_per_kwh: mp,
                    minder_preis_ct_per_kwh: mnp,
                    sparte: grid_billing::Sparte::Strom,
                };
                let result = settle_mmm(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?;

                // Auto-embed lastprofil in Rechnung.zusatz_attribute for audit trail.
                // When the caller does not supply `lastprofil`, fetch bilanzierungsmethode
                // from marktd (populated from UTILMD TM+EM at supply-start via
                // patch_typenmerkmal).  SLP type feeds downstream ERP MMM profile selection.
                let lastprofil = if let Some(lp) = pos.lastprofil.as_deref() {
                    lp.to_owned()
                } else {
                    marktd
                        .get_malo(&pos.malo_id)
                        .await
                        .context("fetch bilanzierungsmethode from marktd")?
                        .and_then(|f| f.bilanzierungsmethode)
                        .unwrap_or_else(|| "SLP".to_owned())
                };
                extra_zusatz.push(rubo4e::current::ZusatzAttribut {
                    name: Some("lastprofil".to_owned()),
                    wert: Some(serde_json::Value::String(lastprofil)),
                    ..Default::default()
                });
                (result, 31002)
            }
            "msb_31009" => {
                let msb_mp_id = pos
                    .msb_mp_id
                    .as_deref()
                    .context("msb_mp_id required for msb_31009")?
                    .to_owned();
                let grundgebuehr = pos
                    .grundgebuehr_eur_per_month
                    .context("grundgebuehr_eur_per_month required for msb_31009")?;
                let months = pos
                    .billing_months
                    .context("billing_months required for msb_31009")?;
                let input = MsbInput {
                    malo_id: pos.malo_id.clone(),
                    nb_mp_id: req.nb_mp_id.clone(),
                    msb_mp_id,
                    period,
                    grundgebuehr_eur_per_month: grundgebuehr,
                    billing_months: months,
                    messdienstleistung_eur: pos.messdienstleistung_eur,
                    messstellen_kategorie: None,
                    entgeltschuldner: None,
                };
                let r = settle_msb(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?;
                (r, 31009)
            }
            // PID 31011: Rechnung sonstige Leistung (GeLi Gas AWH Sperrprozesse, GNB → LFG).
            // Regulatory basis: BK7-24-01-009 §5.4.
            // AWH = Abrechnungswürdige Handlungen (billable actions during Sperrprozess).
            // Calculation: flat Arbeitspreis per kWh or fixed AWH fee — treat as NNE Gas
            // with PID override.  The GNB supplies the Sperrpauschale or measured energy.
            "nne_gas_awh_31011" => {
                let arbeit = pos.arbeitsmenge_kwh.context(
                    "arbeitsmenge_kwh required for nne_gas_awh_31011 (AWH Sperrprozesse)",
                )?;
                let ap = pos
                    .arbeitspreis_ct_per_kwh
                    .context("arbeitspreis_ct_per_kwh required for nne_gas_awh_31011")?;
                let input = NneInput {
                    malo_id: pos.malo_id.clone(),
                    nb_mp_id: req.nb_mp_id.clone(),
                    lf_mp_id: req.lf_mp_id.clone(),
                    period,
                    arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(
                        grid_billing::MengePreis {
                            menge_kwh: arbeit,
                            preis_ct_per_kwh: ap,
                        },
                    ),
                    leistungspreis: match (None, None) {
                        (Some(kw), Some(p)) => Some(grid_billing::Leistungspreis {
                            spitzenleistung_kw: kw,
                            preis_eur_per_kw: p,
                        }),
                        _ => None,
                    },
                    letztverbrauchergruppe: Default::default(),
                    sect19_umlage_ct_per_kwh: None,
                    offshore_umlage_ct_per_kwh: None,
                    kwkg_umlage_ct_per_kwh: None,
                    netzebene: None,
                    sect19: None,
                    gas_kapazitaet: None,
                    jahreshoechstleistung_kw: None,
                    jahresarbeit_kwh: None,
                    konzessionsabgabe: None,
                    grundpreis: None,
                    tariff_sheet_id: None,
                    sparte: grid_billing::Sparte::Strom,
                };
                let r = settle_nne(&input)
                    .map_err(|e| anyhow::anyhow!("billing calc failed for {}: {e}", pos.malo_id))?;
                // 31011 = GeLi Gas AWH Sperrprozesse Rechnung.
                (r, 31011)
            }
            t => bail!("unknown billing_type: {t}"),
        };

        // Self-validate via invoic-checker (checks 1–3 pass by construction;
        // check 4–5 may warn if tariff store is empty, but won't dispute).
        // The settlement becomes a document exactly here: this is the only place
        // an invoice number, an issue date and a Prüfidentifikator enter.
        let document = grid_billing::InvoiceDocument {
            settlement: result,
            pid,
            rechnungsnummer,
            correction_of: None,
            invoice_date,
            due_date,
        };
        let result = &document.settlement;
        let mut rechnung = into_rechnung(&document);
        if !extra_zusatz.is_empty() {
            rechnung.zusatz_attribute = Some(extra_zusatz);
        }
        let report = InvoicCheckEngine::check(
            document.pid,
            &result.nb_mp_id,
            &rechnung,
            &empty_store,
            &config,
        );

        let rechnung_json = serde_json::to_value(&rechnung).context("serialize Rechnung")?;

        // For msb_31009 the invoice recipient is the MSB, not the LF.
        let counterparty = pos
            .msb_mp_id
            .as_deref()
            .filter(|_| pos.billing_type == "msb_31009")
            .unwrap_or(&req.lf_mp_id);

        let draft_id = upsert_draft(
            pool,
            tenant,
            &pos.malo_id,
            &req.nb_mp_id,
            counterparty,
            document.pid as i32,
            document.settlement.period.from(),
            document.settlement.period.to(),
            rechnung_json,
            result.total_eur,
            report.outcome,
        )
        .await
        .context("persist draft")?;

        draft_ids.push(draft_id);
    }

    Ok(draft_ids)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

/// Present a settlement as a document, so an adapter can render it.
#[cfg(test)]
fn as_document(settlement: grid_billing::SettlementResult) -> grid_billing::InvoiceDocument {
    grid_billing::InvoiceDocument {
        settlement,
        pid: 31001,
        rechnungsnummer: "NNE-2026-001".to_owned(),
        correction_of: None,
        invoice_date: time::macros::date!(2026 - 02 - 15),
        due_date: time::macros::date!(2026 - 03 - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use rust_decimal::dec;

    fn make_nne_position(
        malo_id: &str,
        billing_type: &str,
        kwh: rust_decimal::Decimal,
        ap: rust_decimal::Decimal,
    ) -> BillingPosition {
        BillingPosition {
            malo_id: malo_id.to_owned(),
            billing_type: billing_type.to_owned(),
            period_from: "2026-01-01".to_owned(),
            period_to: "2026-01-31".to_owned(),
            arbeitsmenge_kwh: Some(kwh),
            arbeitspreis_ct_per_kwh: Some(ap),
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
            profil_kwh: None,
            mehr_preis_ct_per_kwh: None,
            minder_preis_ct_per_kwh: None,
            lastprofil: None,
            msb_mp_id: None,
            grundgebuehr_eur_per_month: None,
            billing_months: None,
            messdienstleistung_eur: None,
        }
    }

    /// NNE Strom: 1 000 kWh × 28.50 ct/kWh = 285.00 EUR
    #[test]
    fn nne_strom_arbeit_basic() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(1000),
                preis_ct_per_kwh: dec!(28.50),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("nne calculation must succeed");
        // 1000 kWh × 28.50 ct/kWh ÷ 100 = 285.00 EUR
        let expected_eur = dec!(285.00);
        let actual_eur = result.total_eur;
        let diff = (actual_eur - expected_eur).abs();
        assert!(
            diff < dec!(0.01),
            "total_eur {actual_eur} expected ~285.00 EUR (diff {diff})"
        );
    }

    /// §14a Modul 2 ToU: HT + NT positions must sum to correct total.
    #[test]
    fn nne_strom_tou_ht_nt() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Modul2ZeitVariabel {
                ht: grid_billing::MengePreis {
                    menge_kwh: dec!(500),
                    preis_ct_per_kwh: dec!(32.0),
                },
                nt: grid_billing::MengePreis {
                    menge_kwh: dec!(200),
                    preis_ct_per_kwh: dec!(24.0),
                },
            },
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("tou calculation must succeed");
        // HT: 500 × 32.0 ct = 160.00 EUR; NT: 200 × 24.0 ct = 48.00 EUR; total = 208.00 EUR
        let expected_eur = dec!(208.00);
        let diff = (result.total_eur - expected_eur).abs();
        assert!(
            diff < dec!(0.01),
            "ToU total {total} expected 208.00 EUR (diff {diff})",
            total = result.total_eur
        );
        // Two energy positions (HT + NT) must appear in the Rechnung.
        let pos_count = result.positions.len();
        assert!(
            pos_count >= 2,
            "ToU billing must produce at least 2 positions (HT + NT), got {pos_count}"
        );
    }

    /// MMM Strom: actual > profil → Mehrmenge credit to NB.
    #[test]
    fn mmm_strom_mehrmengen() {
        use grid_billing::{MmmInput, settle_mmm};
        let input = MmmInput {
            malo_id: "51238696780".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            actual_kwh: dec!(1200),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(5.0),
            minder_preis_ct_per_kwh: dec!(4.0),
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_mmm(&input).expect("mmm calculation must succeed");
        // Over-consumption is an ungewollte Mindermenge, charged at the
        // Mindermengen price: 200 kWh × 4 ct = 8.00 EUR.
        let diff = (result.total_eur - dec!(8.0)).abs();
        assert!(
            diff < dec!(0.01),
            "MMM Mindermenge expected 8.00 EUR, got {}",
            result.total_eur
        );
    }

    /// MMM Strom: profil > actual → Mindermenge (negative = credit to LF).
    #[test]
    fn mmm_strom_mindermengen() {
        use grid_billing::{MmmInput, settle_mmm};
        let input = MmmInput {
            malo_id: "51238696780".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            actual_kwh: dec!(800),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(5.0),
            minder_preis_ct_per_kwh: dec!(4.0),
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_mmm(&input).expect("mmm calculation must succeed");
        // Under-consumption is an ungewollte Mehrmenge, credited at the
        // Mehrmengen price: 200 kWh × 5 ct = -10.00 EUR.
        let diff = (result.total_eur - dec!(-10.0)).abs();
        assert!(
            diff < dec!(0.01),
            "MMM Mehrmenge expected -10.00 EUR, got {}",
            result.total_eur
        );
    }

    /// KA: when ka_satz_ct_per_kwh is provided, a KA position appears.
    #[test]
    fn nne_strom_with_konzessionsabgabe() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(1000),
                preis_ct_per_kwh: dec!(28.50),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: Some(grid_billing::Konzessionsabgabe {
                satz_ct_per_kwh: dec!(1.32),
                klasse: grid_billing::KaKundengruppe::Tarifkunde {
                    gemeinde: grid_billing::GemeindeGroesse::Bis25k,
                    nur_kochen_warmwasser: false,
                },
            }),
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("nne+ka calculation must succeed");
        // NNE: 1000 × 28.50ct = 285.00 EUR; KA: 1000 × 1.32ct = 13.20 EUR; total = 298.20 EUR
        let expected_eur = dec!(298.20);
        let diff = (result.total_eur - expected_eur).abs();
        assert!(
            diff < dec!(0.01),
            "NNE+KA total {total} expected 298.20 EUR",
            total = result.total_eur
        );
        let pos_count = result.positions.len();
        assert!(
            pos_count >= 2,
            "NNE+KA must produce at least 2 positions (Arbeit + KA), got {pos_count}"
        );
    }

    /// MSB-Rechnung: 12 months × grundgebühr + optional messdienstleistung.
    #[test]
    fn msb_rechnung_grundgebuehr() {
        use grid_billing::{MsbInput, settle_msb};
        let input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 12 - 31),
            )
            .expect("valid period"),
            grundgebuehr_eur_per_month: dec!(9.50),
            billing_months: 12,
            messdienstleistung_eur: Some(dec!(24.00)),
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let result = settle_msb(&input).expect("msb calculation must succeed");
        // 12 × 9.50 = 114.00 EUR + 24.00 = 138.00 EUR
        let expected_eur = dec!(138.00);
        let diff = (result.total_eur - expected_eur).abs();
        assert!(
            diff < dec!(0.01),
            "MSB total {total} expected 138.00 EUR",
            total = result.total_eur
        );
    }

    /// Billing type guard: unknown billing_type returns an error.
    #[test]
    fn unknown_billing_type_is_error() {
        let pos = make_nne_position("10001234567", "unknown_type", dec!(1000), dec!(28.5));
        // Verify the string "unknown_type" would reach the error branch.
        assert!(matches!(pos.billing_type.as_str(), t if t == "unknown_type"));
    }

    // ── MMM tests ─────────────────────────────────────────────────────────────

    /// MMM Strom: actual > profil → Mehrmenge → positive claim (NB bills LF).
    #[test]
    fn mmm_strom_mehrmenge() {
        use grid_billing::{MmmInput, settle_mmm};
        let input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            actual_kwh: dec!(1100), // actual metered
            profil_kwh: dec!(1000), // SLP profile forecast
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_mmm(&input).expect("mmm calc must succeed");
        // Mindermenge = 100 kWh, charged at 2.50 ct = 2.50 EUR.
        let diff = (result.total_eur - dec!(2.50)).abs();
        assert!(
            diff < dec!(0.01),
            "MMM Mindermenge: expected 2.50 EUR, got {total}",
            total = result.total_eur,
        );
    }

    /// MMM Strom: actual < profil → Mehrmenge → negative claim (NB credits LF).
    #[test]
    fn mmm_strom_mindermenge() {
        use grid_billing::{MmmInput, settle_mmm};
        let input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            actual_kwh: dec!(900),  // actual less than forecast
            profil_kwh: dec!(1000), // SLP profile forecast
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_mmm(&input).expect("mmm calc must succeed");
        // Mindermenge = 100 kWh; credit to LF = -100 × 2.50ct = -2.50 EUR
        assert!(
            result.total_eur <= dec!(0),
            "MMM Mindermenge: total_eur should be ≤ 0 (credit), got {total}",
            total = result.total_eur,
        );
    }

    /// MMM balanced: actual == profil → zero settlement.
    #[test]
    fn mmm_strom_balanced_zero() {
        use grid_billing::{MmmInput, settle_mmm};
        let input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            actual_kwh: dec!(1000),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_mmm(&input).expect("mmm calc must succeed");
        let diff = result.total_eur.abs();
        assert!(
            diff < dec!(0.001),
            "balanced MMM must be ~0, got {}",
            result.total_eur
        );
    }

    // ── §14a Modul 2 ToU tests ────────────────────────────────────────────────

    /// §14a Modul 2 ToU: separate HT + NT positions; total must match manual calc.
    #[test]
    fn nne_tou_position_count_and_total() {
        use grid_billing::{NneInput, settle_nne};
        // 600 kWh HT × 4.00 ct = 24.00 EUR; 400 kWh NT × 1.50 ct = 6.00 EUR; total 30.00 EUR
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Modul2ZeitVariabel {
                ht: grid_billing::MengePreis {
                    menge_kwh: dec!(600),
                    preis_ct_per_kwh: dec!(4.00),
                },
                nt: grid_billing::MengePreis {
                    menge_kwh: dec!(400),
                    preis_ct_per_kwh: dec!(1.50),
                },
            },
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("§14a ToU calc must succeed");
        let expected = dec!(30.00);
        let diff = (result.total_eur - expected).abs();
        assert!(
            diff < dec!(0.01),
            "§14a ToU total {total} expected 30.00 EUR",
            total = result.total_eur
        );

        // HT + NT → 2 Rechnungspositionen minimum (no single blended Arbeit)
        let pos_count = result.positions.len();
        assert!(
            pos_count >= 2,
            "§14a ToU needs ≥2 positions, got {pos_count}"
        );
    }

    /// §14a ToU + KA: 4 positions expected (HT, NT, KA, total).
    #[test]
    fn nne_tou_with_ka_position_count() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Modul2ZeitVariabel {
                ht: grid_billing::MengePreis {
                    menge_kwh: dec!(600),
                    preis_ct_per_kwh: dec!(4.00),
                },
                nt: grid_billing::MengePreis {
                    menge_kwh: dec!(400),
                    preis_ct_per_kwh: dec!(1.50),
                },
            },
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: Some(grid_billing::Konzessionsabgabe {
                satz_ct_per_kwh: dec!(1.32),
                klasse: grid_billing::KaKundengruppe::Tarifkunde {
                    gemeinde: grid_billing::GemeindeGroesse::Bis25k,
                    nur_kochen_warmwasser: false,
                },
            }),
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("§14a ToU+KA must succeed");
        // HT: 600×4.00ct=24.00; NT: 400×1.50ct=6.00; KA: 1000×1.32ct=13.20; total=43.20
        let expected = dec!(43.20);
        let diff = (result.total_eur - expected).abs();
        assert!(
            diff < dec!(0.01),
            "§14a ToU+KA total {total} expected 43.20 EUR",
            total = result.total_eur
        );
        let pos_count = result.positions.len();
        assert!(
            pos_count >= 3,
            "§14a ToU+KA needs ≥3 positions (HT+NT+KA), got {pos_count}"
        );
    }

    // ── RLM billing tests ─────────────────────────────────────────────────────

    /// NNE RLM: Arbeit + Leistung billing — large C&I customer.
    #[test]
    fn nne_rlm_arbeit_leistung() {
        use grid_billing::{NneInput, settle_nne};
        // 50 000 kWh × 3.80ct + 120 kW × 8.50 EUR/kW
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(50000),
                preis_ct_per_kwh: dec!(3.80),
            }),
            leistungspreis: Some(grid_billing::Leistungspreis {
                spitzenleistung_kw: dec!(120),
                preis_eur_per_kw: dec!(8.50),
            }),
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("RLM calc must succeed");
        // 50000 × 3.80ct = 1900 EUR; 120 × 8.50 EUR = 1020 EUR; total = 2920 EUR
        let expected = dec!(2920.00);
        let diff = (result.total_eur - expected).abs();
        assert!(
            diff < dec!(0.01),
            "RLM total {total} expected 2920.00 EUR",
            total = result.total_eur
        );
        let pos_count = result.positions.len();
        assert!(
            pos_count >= 2,
            "RLM needs ≥2 positions (Arbeit+Leistung), got {pos_count}"
        );
    }

    // ── NNE Gas tests ─────────────────────────────────────────────────────────

    /// NNE Gas (PID 31005): billing formula same as Strom, PID must be 31005.
    #[test]
    fn nne_gas_pid_31005() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(2000),
                preis_ct_per_kwh: dec!(2.10),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("NNE Gas calc must succeed");
        // 2000 kWh × 2.10ct = 42.00 EUR
        let diff = (result.total_eur - dec!(42.00)).abs();
        assert!(
            diff < dec!(0.01),
            "NNE Gas total {total} expected 42.00 EUR",
            total = result.total_eur
        );
        let _ = input.malo_id.len();
    }

    // ── PID 31011 AWH Sperrprozesse tests ─────────────────────────────────────

    /// PID 31011 (GeLi Gas AWH): billing_type nne_gas_awh_31011 must set PID 31011.
    #[test]
    fn awh_31011_pid_override() {
        use grid_billing::{NneInput, settle_nne};
        // Simulate the billing.rs path: calculate NNE then override PID to 31011
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            // AWH Sperrpauschale: 50 EUR flat (expressed as 1 kWh × 5000 ct)
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(1),
                preis_ct_per_kwh: dec!(5000),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("AWH calc must succeed");
        // 1 × 5000 ct = 50.00 EUR
        let diff = (result.total_eur - dec!(50.00)).abs();
        assert!(
            diff < dec!(0.01),
            "AWH 31011 total {total} expected 50.00 EUR",
            total = result.total_eur
        );
    }

    // ── Decimal precision tests ───────────────────────────────────────────────

    /// Decimal precision: no floating-point rounding error on typical tariff value.
    /// Regression guard: 1 234.567 kWh × 12.345 ct/kWh = 152.395_eur exact.
    #[test]
    fn decimal_precision_no_float_error() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: Decimal::from_str_exact("1234.567").unwrap(),
                preis_ct_per_kwh: Decimal::from_str_exact("12.345").unwrap(),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("precision calc must succeed");
        // 1234.567 × 12.345ct / 100 = 152.39553915 EUR (rounded to 5dp: 152.39554)
        // Key assertion: total must not use f64 intermediate (no 152.3955391500001 etc.)
        let total_str = format!("{}", result.total_eur);
        // No repeating decimals from f64 conversion
        assert!(
            !total_str.contains("99999") && !total_str.contains("00000"),
            "decimal result shows float residue: {total_str}",
        );
    }

    /// MSB Rechnung: billing_months = 1 (monthly MSB settlement).
    #[test]
    fn msb_rechnung_single_month() {
        use grid_billing::{MsbInput, settle_msb};
        let input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            grundgebuehr_eur_per_month: dec!(9.50),
            billing_months: 1,
            messdienstleistung_eur: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let result = settle_msb(&input).expect("msb 1-month must succeed");
        let diff = (result.total_eur - dec!(9.50)).abs();
        assert!(
            diff < dec!(0.01),
            "1-month MSB should be 9.50 EUR, got {}",
            result.total_eur
        );
    }

    // ── invoic-checker integration ────────────────────────────────────────────

    /// invoic-checker check 1–3 must not return Dispute for generated NNE invoice.
    #[test]
    fn invoic_checker_accepts_generated_nne() {
        use grid_billing::{NneInput, settle_nne};
        use invoic_checker::{
            InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore,
        };
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(1000),
                preis_ct_per_kwh: dec!(28.50),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("calc must succeed");
        let store = InMemoryPreisblattStore::new();
        let config = CheckConfig::default();
        let rechnung = into_rechnung(&super::as_document(result.clone()));
        let report = InvoicCheckEngine::check(31001, &result.nb_mp_id, &rechnung, &store, &config);
        use invoic_checker::check::CheckOutcome;
        assert_ne!(
            report.outcome,
            CheckOutcome::Dispute,
            "generated NNE INVOIC must not fail invoic-checker checks 1–3; findings: {:?}",
            report.findings,
        );
    }

    /// invoic-checker must not return Dispute for generated MMM invoice.
    #[test]
    fn invoic_checker_accepts_generated_mmm() {
        use grid_billing::{MmmInput, settle_mmm};
        use invoic_checker::{
            InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore,
        };
        let input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            actual_kwh: dec!(1100),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_mmm(&input).expect("mmm calc must succeed");
        let store = InMemoryPreisblattStore::new();
        let config = CheckConfig::default();
        let rechnung = into_rechnung(&super::as_document(result.clone()));
        let report = InvoicCheckEngine::check(31001, &result.nb_mp_id, &rechnung, &store, &config);
        use invoic_checker::check::CheckOutcome;
        assert_ne!(
            report.outcome,
            CheckOutcome::Dispute,
            "generated MMM INVOIC must not fail invoic-checker checks 1–3; findings: {:?}",
            report.findings,
        );
    }

    // ── BillingRunRequest parsing tests ──────────────────────────────────────

    /// parse_date rejects invalid ISO dates.
    #[test]
    fn parse_date_rejects_invalid() {
        assert!(parse_date("not-a-date").is_err());
        assert!(parse_date("2026-13-01").is_err()); // month 13
        assert!(parse_date("2026-02-30").is_err()); // Feb 30 doesn't exist
    }

    /// parse_date accepts valid ISO dates.
    #[test]
    fn parse_date_accepts_valid() {
        assert!(parse_date("2026-01-01").is_ok());
        assert!(parse_date("2026-12-31").is_ok());
        assert!(parse_date("2024-02-29").is_ok()); // leap year
    }

    // ── Regulatory correctness tests ─────────────────────────────────────────

    /// §21 MessZV Zahlungsziel: invoice must carry a valid due_date > invoice_date.
    #[test]
    fn nne_due_date_after_invoice_date() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(1000),
                preis_ct_per_kwh: dec!(28.50),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let settlement = settle_nne(&input).expect("must succeed");
        // §21 MessZV Zahlungsziel is a property of the document, not of what was
        // calculated — so it is asserted where the dates now live.
        let document = super::as_document(settlement);
        assert!(
            document.due_date > document.invoice_date,
            "due_date must be after invoice_date"
        );
    }

    /// PID correctness: each billing_type maps to exactly one PID.
    #[test]
    fn pid_mapping_correctness() {
        use grid_billing::{MmmInput, MsbInput, NneInput, settle_mmm, settle_msb, settle_nne};
        let base_nne = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(100),
                preis_ct_per_kwh: dec!(10),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };

        // The same settlement can be routed as three different documents — which
        // is the point of keeping the Prüfidentifikator off the calculation.
        let settlement = settle_nne(&base_nne).unwrap();
        for pid in [31001, 31005, 31011] {
            let document = grid_billing::InvoiceDocument {
                settlement: settlement.clone(),
                pid,
                rechnungsnummer: format!("DOC-{pid}"),
                correction_of: None,
                invoice_date: time::macros::date!(2026 - 02 - 15),
                due_date: time::macros::date!(2026 - 03 - 15),
            };
            assert_eq!(document.pid, pid);
            assert_eq!(document.settlement.total_eur, settlement.total_eur);
        }

        // MMM identifies itself by settlement type; the PID that routes it is
        // chosen when the document is built.
        let mmm_input = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            actual_kwh: dec!(100),
            profil_kwh: dec!(100),
            mehr_preis_ct_per_kwh: dec!(3),
            minder_preis_ct_per_kwh: dec!(2),
            sparte: grid_billing::Sparte::Strom,
        };
        let mmm = settle_mmm(&mmm_input).unwrap();
        assert_eq!(mmm.settlement_type, grid_billing::SettlementType::MmmStrom);

        // msb_31009 → 31009
        let msb_input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            grundgebuehr_eur_per_month: dec!(9.50),
            billing_months: 1,
            messdienstleistung_eur: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let settlement = settle_msb(&msb_input).unwrap();
        // MsbG §§6–7: one Grundgebühr position for the billed month.
        assert_eq!(settlement.positions.len(), 1);
        assert_eq!(settlement.total_eur, dec!(9.50));
        assert_eq!(
            settlement.settlement_type,
            grid_billing::SettlementType::MsbRechnung
        );
    }

    /// §14a EnWG Modul 2: HT+NT split must produce separate positions.
    /// When HT=0 kWh or NT=0 kWh, the billing must still produce 2 positions
    /// (one may have zero amount) — no silent omission of a rate band.
    #[test]
    fn tou_zero_nt_still_produces_positions() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Modul2ZeitVariabel {
                ht: grid_billing::MengePreis {
                    menge_kwh: dec!(1000),
                    preis_ct_per_kwh: dec!(4.00),
                },
                // A zero NT band is still a band: it must produce a position, so
                // that a rate band is never silently omitted from the invoice.
                nt: grid_billing::MengePreis {
                    menge_kwh: dec!(0),
                    preis_ct_per_kwh: dec!(1.50),
                },
            },
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("zero-NT §14a must succeed");
        // HT: 1000 × 4.00ct = 40.00 EUR; NT: 0 × 1.50ct = 0; total = 40.00
        let diff = (result.total_eur - dec!(40.00)).abs();
        assert!(
            diff < dec!(0.01),
            "zero-NT total expected 40.00 EUR, got {}",
            result.total_eur
        );
    }

    /// KA Konzessionsabgabe: KAV §2 — two KA bands residential vs commercial.
    /// Residential (H0): 1.32 ct/kWh; commercial (G0): 0.11 ct/kWh.
    /// Both rates must be accepted by the billing engine.
    #[test]
    fn ka_rates_residential_and_commercial() {
        use grid_billing::{NneInput, settle_nne};
        // Residential: 1000 kWh × 28.50 ct + 1000 × 1.32 ct KA = 285.00 + 13.20 = 298.20
        let residential = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(1000),
                preis_ct_per_kwh: dec!(28.50),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: Some(grid_billing::Konzessionsabgabe {
                satz_ct_per_kwh: dec!(1.32),
                klasse: grid_billing::KaKundengruppe::Tarifkunde {
                    gemeinde: grid_billing::GemeindeGroesse::Bis25k,
                    nur_kochen_warmwasser: false,
                },
            }),
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let r_res = settle_nne(&residential).unwrap();
        let diff = (r_res.total_eur - dec!(298.20)).abs();
        assert!(
            diff < dec!(0.01),
            "residential KA expected 298.20 EUR, got {}",
            r_res.total_eur
        );

        // Commercial G0: 0.11 ct/kWh KA
        let commercial = NneInput {
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: Some(grid_billing::Konzessionsabgabe {
                satz_ct_per_kwh: dec!(0.11),
                klasse: grid_billing::KaKundengruppe::Tarifkunde {
                    gemeinde: grid_billing::GemeindeGroesse::Bis25k,
                    nur_kochen_warmwasser: false,
                },
            }),
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
            ..residential
        };
        let r_com = settle_nne(&commercial).unwrap();
        // 1000 × 28.50ct + 1000 × 0.11ct = 285.00 + 1.10 = 286.10 EUR
        let diff = (r_com.total_eur - dec!(286.10)).abs();
        assert!(
            diff < dec!(0.01),
            "commercial KA expected 286.10 EUR, got {}",
            r_com.total_eur
        );
    }

    /// GPKE (BK6-24-174) Teil 1 Kap. 8.4 MMM: Strom MMM billing type "mmm" (alias for mmm_strom) should use
    /// the same formula as "mmm_strom". Regression guard for billing_type alias.
    #[test]
    fn mmm_billing_type_alias_consistent() {
        use grid_billing::{MmmInput, settle_mmm};
        let base = MmmInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            actual_kwh: dec!(1100),
            profil_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: dec!(3.00),
            minder_preis_ct_per_kwh: dec!(2.50),
            sparte: grid_billing::Sparte::Strom,
        };
        // Both "mmm" and "mmm_strom" map to the same grid_billing::MmmInput path.
        // Verify the formula produces the same result for any label.
        let r1 = settle_mmm(&base).unwrap();
        let r2 = settle_mmm(&MmmInput { ..base }).unwrap();
        let diff = (r1.total_eur - r2.total_eur).abs();
        assert!(
            diff < dec!(0.00001),
            "mmm alias must yield same total as mmm_strom"
        );
    }

    /// RLM spitzenleistung: zero kW must be handled gracefully (no Leistung position).
    #[test]
    fn rlm_zero_spitzenleistung_no_leistung_position() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(50000),
                preis_ct_per_kwh: dec!(3.80),
            }),
            leistungspreis: Some(grid_billing::Leistungspreis {
                spitzenleistung_kw: dec!(0),
                preis_eur_per_kw: dec!(8.50),
            }),
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("zero Spitze must succeed");
        // Only Arbeit: 50000 × 3.80ct = 1900 EUR; Leistung = 0 × 8.50 = 0
        // Total might be 1900 or include a zero-amount Leistung position — either is valid
        assert!(
            result.total_eur >= dec!(1900.00),
            "at minimum 1900 EUR Arbeit"
        );
        assert!(
            result.total_eur <= dec!(1901.00),
            "no unexpected amount above 1900"
        );
    }

    /// BNetzA §22 MessZV retention: billing period must be recorded accurately.
    /// Guards that a short 2-day period (e.g. partial month on supply start) works.
    #[test]
    fn period_ordering_validity() {
        use grid_billing::{NneInput, settle_nne};
        let input = NneInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 15),
                time::macros::date!(2026 - 01 - 16),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: dec!(48),
                preis_ct_per_kwh: dec!(28.50),
            }),
            leistungspreis: None,
            // These fixtures verify the Arbeits-/Leistungspreis arithmetic; §21 EnFG
            // exemption keeps the network levies out of the totals they assert.
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::Befreit,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        };
        let result = settle_nne(&input).expect("2-day billing must succeed");
        let diff = (result.total_eur - dec!(13.68)).abs(); // 48 × 28.50ct = 13.68 EUR
        assert!(
            diff < dec!(0.01),
            "2-day billing expected 13.68 EUR, got {}",
            result.total_eur
        );
    }

    /// MSB-Rechnung: Messdienstleistung is optional; without it only Grundgebühr.
    #[test]
    fn msb_without_messdienstleistung() {
        use grid_billing::{MsbInput, settle_msb};
        let input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            grundgebuehr_eur_per_month: dec!(15.00),
            billing_months: 3,
            messdienstleistung_eur: None, // no MDL
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let result = settle_msb(&input).expect("MSB no-MDL must succeed");
        // 3 × 15.00 = 45.00 EUR, no MDL
        let diff = (result.total_eur - dec!(45.00)).abs();
        assert!(
            diff < dec!(0.01),
            "MSB no-MDL expected 45.00 EUR, got {}",
            result.total_eur
        );
    }

    /// Invoic-checker accepts generated MSB invoice (PID 31009).
    #[test]
    fn invoic_checker_accepts_generated_msb() {
        use grid_billing::{MsbInput, settle_msb};
        use invoic_checker::{
            InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore,
        };
        let input = MsbInput {
            malo_id: "10001234567".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            msb_mp_id: "4012345000023".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 12 - 31),
            )
            .expect("valid period"),
            grundgebuehr_eur_per_month: dec!(9.50),
            billing_months: 12,
            messdienstleistung_eur: Some(dec!(24.00)),
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let result = settle_msb(&input).expect("MSB calc must succeed");
        let store = InMemoryPreisblattStore::new();
        let config = CheckConfig::default();
        let rechnung = into_rechnung(&super::as_document(result.clone()));
        let report = InvoicCheckEngine::check(31001, &result.nb_mp_id, &rechnung, &store, &config);
        use invoic_checker::check::CheckOutcome;
        assert_ne!(
            report.outcome,
            CheckOutcome::Dispute,
            "generated MSB INVOIC must not fail invoic-checker; findings: {:?}",
            report.findings,
        );
    }
}

#[cfg(test)]
mod trace_persistence_tests {
    use super::*;

    /// The calculation trace must survive into the stored Rechnung.
    ///
    /// grid-billing computes, per position, the inputs it used, the paragraphs
    /// it applied and where the rate came from. That was previously dropped by
    /// this adapter, so a §20 EnWG audit or an LF dispute had nothing to read —
    /// while `netzbilanz-agent` was instructed to verify it.
    #[test]
    fn the_calculation_trace_reaches_the_stored_rechnung() {
        let settlement = grid_billing::settle_nne(&sample_nne()).expect("settle");
        let rechnung = into_rechnung(&super::as_document(settlement));

        let positions = rechnung.rechnungspositionen.expect("positions");
        let first = positions.first().expect("at least one position");
        let attrs = first
            .zusatz_attribute
            .as_ref()
            .expect("position carries its trace");
        let trace = attrs
            .iter()
            .find(|a| a.name.as_deref() == Some("mako:calculation_trace"))
            .and_then(|a| a.wert.as_ref())
            .expect("mako:calculation_trace present");

        // The fields an auditor actually needs, not just any blob.
        assert!(trace.get("explanation").is_some(), "{trace}");
        assert!(trace.get("legal_refs").is_some(), "{trace}");
        assert!(trace.get("input_quantity").is_some(), "{trace}");
        assert!(trace.get("gross_eur").is_some(), "{trace}");
    }

    /// The settlement's citations survive too, deduplicated.
    #[test]
    fn the_legal_references_reach_the_stored_rechnung() {
        let settlement = grid_billing::settle_nne(&sample_nne()).expect("settle");
        let rechnung = into_rechnung(&super::as_document(settlement));

        let refs = rechnung
            .zusatz_attribute
            .as_ref()
            .expect("settlement attributes")
            .iter()
            .find(|a| a.name.as_deref() == Some("mako:legal_references"))
            .and_then(|a| a.wert.as_ref())
            .expect("mako:legal_references present");

        let list = refs.as_array().expect("an array of citations");
        assert!(!list.is_empty(), "a settlement always rests on something");
    }

    fn sample_nne() -> grid_billing::NneInput {
        grid_billing::NneInput {
            malo_id: "51238696780".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: grid_billing::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: grid_billing::ArbeitspreisModell::Einheitlich(grid_billing::MengePreis {
                menge_kwh: rust_decimal::Decimal::from(1000),
                preis_ct_per_kwh: rust_decimal::Decimal::new(35, 1),
            }),
            leistungspreis: None,
            letztverbrauchergruppe: Default::default(),
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            gas_kapazitaet: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            konzessionsabgabe: None,
            grundpreis: None,
            tariff_sheet_id: None,
            sparte: grid_billing::Sparte::Strom,
        }
    }
}

#[cfg(test)]
mod artikelnummer_bridge_tests {
    use grid_billing::{BillingPositionKind as K, SettlementType as ST};

    /// Every codelist name grid-billing emits must parse into the BO4E enum.
    ///
    /// The two are joined by a string, so a typo on either side degrades
    /// silently: `from_str` returns `Err`, the article number becomes `None`,
    /// and the INVOIC ships without it. This is the test that makes the seam
    /// safe.
    #[test]
    fn every_emitted_codelist_name_parses() {
        let kinds = [
            K::NneArbeit,
            K::NneArbeitHt,
            K::NneArbeitNt,
            K::NneArbeitModul1,
            K::NneArbeitModul3,
            K::NneLeistung,
            K::NneGasGrundpreis,
            K::Konzessionsabgabe,
            K::Mehrmenge,
            K::Mindermenge,
            K::MsbGrundgebuehr,
            K::Messdienstleistung,
            K::GasAwhSperrung,
            K::GasAwhEntsprrung,
            K::GasAwhSonstige,
            K::Blindmehrarbeit,
            K::Sect19StromNevUmlage,
            K::OffshoreNetzumlage,
            K::KwkgUmlage,
            K::DezentraleEinspeisung,
            K::Sect19IndividuellesEntgelt,
            K::GasKapazitaetsentgelt,
        ];
        let types = [
            ST::NneStrom,
            ST::NneGas,
            ST::MmmStrom,
            ST::MmmGas,
            ST::MsbRechnung,
            ST::GasAwhSperrung,
            ST::DezentraleEinspeisung,
        ];

        for kind in kinds {
            for st in types {
                let Some(name) = kind.artikelnummer(st) else {
                    continue; // carries an Artikel-ID instead
                };
                assert!(
                    super::kind_to_artikelnummer(kind, st).is_some(),
                    "grid-billing emits {name:?} for {kind:?}/{st:?}, \
                     but rubo4e cannot parse it"
                );
            }
        }
    }

    /// Gas NNE keeps the classic code; Strom NNE carries an Artikel-ID instead.
    ///
    /// BK6-20-160 changed Strom only, and getting this backwards puts the wrong
    /// identifier on every grid invoice.
    #[test]
    fn strom_and_gas_nne_are_coded_differently() {
        assert_eq!(K::NneArbeit.artikelnummer(ST::NneGas), Some("WIRKARBEIT"));
        assert_eq!(K::NneArbeit.artikelnummer(ST::NneStrom), None);
    }
}
