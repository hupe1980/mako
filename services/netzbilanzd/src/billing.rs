//! Settlement orchestration — the seam between an HTTP request and the pure
//! `grid-billing` engine.
//!
//! Everything here is a *translation*: a [`crate::request::SettlementRequest`]
//! becomes one of the four `grid-billing` inputs, the engine returns a
//! [`grid_billing::SettlementResult`], and that becomes an
//! [`grid_billing::InvoiceDocument`] rendered as a BO4E `Rechnung`. No money is
//! computed in this file, and none should be: the arithmetic, the legal
//! references and the statutory ceilings all live in the engine, where they are
//! tested against the ordinances rather than against a service's expectations.
//!
//! ## What this module deliberately does not decide
//!
//! - **The Prüfidentifikator.** It comes from `settlement_type.default_pid()`,
//!   not from a literal beside each match arm — a second copy of a mapping the
//!   engine already owns is free to drift from it.
//! - **The Sparte.** It arrives on the request. Hard-coding it would put
//!   StromNEV citations and the three electricity EnFG levies on gas invoices.
//! - **The invoice number.** Allocated by the database (§14 Abs. 4 Nr. 4 UStG).

use std::sync::Arc;

use anyhow::Context as _;
use grid_billing::{
    GasAwhInput, InvoiceDocument, MmmInput, MsbInput, MsbRechnungsempfaenger, NneInput,
    SettlementPeriod, SettlementResult, Sparte, ValidationResult, settle_abschlag, settle_gas_awh,
    settle_mmm, settle_msb, settle_nne, validate_gas_awh_input, validate_mmm_input,
    validate_msb_input,
};
use invoic_checker::{InvoicCheckEngine, check::CheckConfig, tariff::InMemoryPreisblattStore};
use mako_markt::marktd_client::MarktdClient;
use rust_decimal::Decimal;

use crate::request::{BillingPositionRequest, SettlementRequest};

/// A settled position, ready to be persisted as a draft.
pub struct SettledInvoice {
    /// What the recipient actually pays — the gross less any Abschläge.
    pub zu_zahlen_eur: Decimal,
    /// The invoice number this was issued under.
    ///
    /// Carried here rather than read back out of the rendered document: it is a
    /// NOT NULL unique business key, and recovering it from an `Option` field
    /// means deciding what an absent invoice number should become.
    pub rechnungsnummer: String,
    /// What the engine computed.
    pub settlement: SettlementResult,
    /// The rendered BO4E document.
    pub rechnung: rubo4e::current::Rechnung,
    /// The invoice-checker verdict on the rendered document.
    pub report: invoic_checker::CheckReport,
    /// The Prüfidentifikator, derived from the settlement type.
    pub pid: u32,
}

/// Prices and master data a settlement needs but the request did not carry.
///
/// Resolved *before* the engine runs, so the engine stays I/O-free and the
/// resolved values are what gets stored as the settlement input — an audit
/// replays the same numbers rather than re-querying a service whose data has
/// since moved on.
pub struct Resolver<'a> {
    /// `marktd`, for published MMM prices and MaLo master data.
    pub marktd: &'a Arc<MarktdClient>,
    /// Published MMM prices already fetched during this run, keyed by
    /// `(Sparte, year, month)`.
    ///
    /// A monthly MMM sweep settles every MaLo of one Sparte against the *same*
    /// published series, so fetching per position would mean up to a thousand
    /// identical round-trips to `marktd` before the transaction opens. The memo
    /// is per-run and dropped with it, so a later run reads the current series.
    prices: std::collections::HashMap<(Sparte, i32, u8), (Decimal, Decimal)>,
}

impl<'a> Resolver<'a> {
    /// A resolver for one billing run.
    #[must_use]
    pub fn new(marktd: &'a Arc<MarktdClient>) -> Self {
        Self {
            marktd,
            prices: std::collections::HashMap::new(),
        }
    }

    /// Fill in whatever the request left to `netzbilanzd` to look up.
    ///
    /// Returns the position with every auto-fetched field materialised, so the
    /// stored settlement input is self-contained.
    ///
    /// # Errors
    ///
    /// Returns an error when a price or profile the request left open cannot be
    /// resolved.
    pub async fn resolve(&mut self, position: &mut BillingPositionRequest) -> anyhow::Result<()> {
        let SettlementRequest::Mmm(mmm) = &position.settlement else {
            return Ok(());
        };

        // The published series, if this position left it open.
        let needs_prices =
            mmm.mehr_preis_ct_per_kwh.is_none() || mmm.minder_preis_ct_per_kwh.is_none();
        let key = (
            mmm.sparte,
            position.period_from.year(),
            position.period_from.month() as u8,
        );
        let prices = match (needs_prices, self.prices.get(&key).copied()) {
            (false, _) => None,
            (true, Some(hit)) => Some(hit),
            (true, None) => {
                let fetched = self.fetch_prices(key).await?;
                self.prices.insert(key, fetched);
                Some(fetched)
            }
        };

        // The SLP designation is per-MaLo, so it is fetched per position.
        let lastprofil = if mmm.lastprofil.is_none() {
            // Recorded from UTILMD `TM+EM` at supply start. It explains which
            // profile the bilanzierte Menge came from, which is the first thing
            // an LF asks about an MMM saldo.
            self.marktd
                .get_malo(&position.malo_id)
                .await
                .context("fetch bilanzierungsmethode from marktd")?
                .and_then(|f| f.bilanzierungsmethode)
        } else {
            None
        };

        let SettlementRequest::Mmm(mmm) = &mut position.settlement else {
            unreachable!("the variant was matched above and nothing moved it")
        };
        if let Some((mehr, minder)) = prices {
            mmm.mehr_preis_ct_per_kwh.get_or_insert(mehr);
            mmm.minder_preis_ct_per_kwh.get_or_insert(minder);
        }
        if let Some(profil) = lastprofil {
            mmm.lastprofil = Some(profil);
        }
        Ok(())
    }

    /// The published Mehr-/Mindermengen prices for a Sparte and month.
    async fn fetch_prices(
        &self,
        (sparte, year, month): (Sparte, i32, u8),
    ) -> anyhow::Result<(Decimal, Decimal)> {
        match sparte {
            // Gas MMM prices are the Trading Hub Europe monthly series
            // (GaBi Gas 2.1, BK7-24-01-008) — one national market area, so
            // there is nothing operator-specific to configure.
            Sparte::Gas => self
                .marktd
                .get_mmma_gas(year, month, "THE")
                .await
                .context("fetch Gas MMMA prices (THE) from marktd")?
                .map(|r| (r.mehr_ct_kwh, r.minder_ct_kwh))
                .with_context(|| {
                    format!(
                        "Gas MMM {year}-{month:02}: no THE MMMA prices in marktd. \
                         Import them via PUT /api/v1/mmma-preise/gas/{year}/{month}, \
                         or supply mehr_preis_ct_per_kwh and minder_preis_ct_per_kwh."
                    )
                }),
            // The Strom Mehr-/Mindermengenpreise are einheitlich across the
            // German market and published monthly by the BDEW, so the month
            // alone identifies them. There is no operator dimension to
            // configure — an earlier `vnb_mp_id` setting made every Strom MMM
            // settlement refuse until an operator named an ÜNB whose own series
            // was never published.
            //
            // The authority is keyed on the delivery period, not stated flat:
            // § 13 Abs. 3 StromNZV to 31.12.2025, and § 20 Abs. 3 EnWG through
            // the GPKE Festlegung (BK6-24-174) from 01.01.2026, since StromNZV
            // ceased to have effect (Art. 15 Abs. 4 of the Gesetz v. 22.12.2023,
            // BGBl. 2023 I Nr. 405). `grid_billing` resolves it per settlement
            // via `RegulatoryRegime::for_period`; this comment must not
            // contradict it.
            Sparte::Strom => self
                .marktd
                .get_mmm_strom(year, month)
                .await
                .context("fetch Strom Mehr-/Mindermengenpreise from marktd")?
                .map(|r| (r.mehr_ct_kwh, r.minder_ct_kwh))
                .with_context(|| {
                    format!(
                        "Strom MMM {year}-{month:02}: no Mehr-/Mindermengenpreise in marktd. \
                         Import the BDEW publication via \
                         PUT /api/v1/mmm-preise/strom/{year}/{month}, or supply \
                         mehr_preis_ct_per_kwh and minder_preis_ct_per_kwh."
                    )
                }),
        }
    }
}

/// Run the engine for one fully-resolved position.
///
/// # Errors
///
/// Returns the engine's [`grid_billing::BillingError`] as context when the
/// inputs do not describe a computable settlement.
pub fn settle(position: &BillingPositionRequest) -> anyhow::Result<SettlementResult> {
    let period = SettlementPeriod::new(position.period_from, position.period_to)
        .context("billing period")?;

    let result = match &position.settlement {
        SettlementRequest::Abschlag(a) => settle_abschlag(&grid_billing::AbschlagInput {
            malo_id: position.malo_id.clone(),
            nb_mp_id: a.nb_mp_id.clone(),
            lf_mp_id: a.lf_mp_id.clone(),
            period,
            sparte: a.sparte,
            betrag_netto_eur: a.betrag_netto_eur,
            grundlage: a.grundlage,
        }),
        SettlementRequest::Nne(nne) => settle_nne(&NneInput {
            malo_id: position.malo_id.clone(),
            nb_mp_id: nne.nb_mp_id.clone(),
            lf_mp_id: nne.lf_mp_id.clone(),
            period,
            sparte: nne.sparte,
            arbeitspreis: nne.arbeitspreis.clone(),
            leistungspreis: nne.leistungspreis,
            grundpreis: nne.grundpreis,
            konzessionsabgabe: nne.konzessionsabgabe,
            blindarbeit: nne.blindarbeit,
            gas_kapazitaet: nne.gas_kapazitaet,
            letztverbrauchergruppe: nne.letztverbrauchergruppe,
            enfg_jahresvorverbrauch_kwh: nne.enfg_jahresvorverbrauch_kwh,
            sect19_umlage_ct_per_kwh: nne.sect19_umlage_ct_per_kwh,
            offshore_umlage_ct_per_kwh: nne.offshore_umlage_ct_per_kwh,
            kwkg_umlage_ct_per_kwh: nne.kwkg_umlage_ct_per_kwh,
            netzebene: nne.netzebene,
            sect19: nne.sect19.clone(),
            jahreshoechstleistung_kw: nne.jahreshoechstleistung_kw,
            jahresarbeit_kwh: nne.jahresarbeit_kwh,
            tariff_sheet_id: nne.tariff_sheet_id.clone(),
        }),
        SettlementRequest::Mmm(mmm) => {
            // Resolved before the engine runs; reaching here without a price is
            // a programming error in the resolver, not a caller error.
            let mehr = mmm
                .mehr_preis_ct_per_kwh
                .context("MMM Mehrmengenpreis was not resolved")?;
            let minder = mmm
                .minder_preis_ct_per_kwh
                .context("MMM Mindermengenpreis was not resolved")?;
            let input = MmmInput {
                malo_id: position.malo_id.clone(),
                nb_mp_id: mmm.nb_mp_id.clone(),
                lf_mp_id: mmm.lf_mp_id.clone(),
                period,
                sparte: mmm.sparte,
                actual_kwh: mmm.gemessen_kwh,
                profil_kwh: mmm.bilanziert_kwh,
                mehr_preis_ct_per_kwh: mehr,
                minder_preis_ct_per_kwh: minder,
                wiederverkaeufer: mmm.wiederverkaeufer,
            };
            refuse_invalid("MMM", &position.malo_id, validate_mmm_input(&input))?;
            settle_mmm(&input)
        }
        SettlementRequest::Msb(msb) => {
            let input = MsbInput {
                malo_id: position.malo_id.clone(),
                msb_mp_id: msb.msb_mp_id.clone(),
                empfaenger: MsbRechnungsempfaenger {
                    rolle: msb.empfaenger_rolle,
                    mp_id: msb.empfaenger_mp_id.clone(),
                },
                period,
                sparte: msb.sparte,
                grundgebuehr_eur_per_month: msb.grundgebuehr_eur_per_month,
                billing_months: msb.billing_months,
                messdienstleistung_eur: msb.messdienstleistung_eur,
                steuereinrichtung_eur_per_month: msb.steuereinrichtung_eur_per_month,
                tagespreise: msb.tagespreise,
                messstellen_kategorie: msb.messstellen_kategorie,
                entgeltschuldner: msb.entgeltschuldner,
            };
            refuse_invalid("MSB", &position.malo_id, validate_msb_input(&input))?;
            settle_msb(&input)
        }
        SettlementRequest::GasAwh(awh) => {
            let input = GasAwhInput {
                malo_id: position.malo_id.clone(),
                nb_mp_id: awh.nb_mp_id.clone(),
                lf_mp_id: awh.lf_mp_id.clone(),
                period,
                awh_positionen: awh.positionen.clone(),
                tariff_sheet_id: awh.tariff_sheet_id.clone(),
            };
            refuse_invalid("AWH", &position.malo_id, validate_gas_awh_input(&input))?;
            settle_gas_awh(&input)
        }
    };

    result.map_err(|e| anyhow::anyhow!("settlement failed for MaLo {}: {e}", position.malo_id))
}

/// Refuse a settlement whose pre-flight validation found an `Error`.
///
/// `grid-billing`'s `validate_mmm_input` / `validate_msb_input` /
/// `validate_gas_awh_input` return a [`ValidationResult`] whose `is_valid` goes
/// false on an `Error`-severity finding — a Grundgebühr with no month count, an
/// AWH position with no rate. Such an input is not billable: the invoice would
/// reach the counterparty carrying the defect the validator already named.
///
/// Warnings below `Error` are left on the result: they are for the operator to
/// read, not for the run to stop on.
fn refuse_invalid(kind: &str, malo_id: &str, v: ValidationResult) -> anyhow::Result<()> {
    if v.is_valid {
        return Ok(());
    }
    let findings = v
        .warnings
        .iter()
        .filter(|w| w.severity == grid_billing::WarningSeverity::Error)
        .map(|w| format!("[{}] {}", w.code, w.message))
        .collect::<Vec<_>>()
        .join("; ");
    anyhow::bail!("{kind} settlement for MaLo {malo_id} is not billable: {findings}")
}

/// Everything about a document that is not the settlement itself.
#[derive(Debug, Default, Clone)]
pub struct DocumentFacts {
    /// The `rechnungsnummer` this corrects, if any.
    pub correction_of: Option<String>,
    /// The billing cadence — `IMD+7081`.
    pub cadence: Option<grid_billing::Rechnungscharakter>,
    /// Abschlagsrechnungen deducted from what is owed.
    pub abschlaege: Vec<grid_billing::Abschlagsverrechnung>,
    /// The Artikel-ID of each charge the settlement leaves to the price sheet,
    /// keyed by position kind ([`crate::request::NneRequest::artikel_ids`]).
    pub artikel_ids: std::collections::BTreeMap<grid_billing::BillingPositionKind, String>,
}

impl DocumentFacts {
    /// The Artikel-IDs a settlement request states.
    #[must_use]
    pub fn artikel_ids_of(
        settlement: &SettlementRequest,
    ) -> std::collections::BTreeMap<grid_billing::BillingPositionKind, String> {
        match settlement {
            SettlementRequest::Nne(n) => n.artikel_ids.clone(),
            SettlementRequest::Msb(m) => m.artikel_ids.clone(),
            _ => std::collections::BTreeMap::new(),
        }
    }
}

/// Present a settlement as an invoice and check the rendered document.
///
/// The check runs on what will actually be sent, not on the settlement: the
/// receiving LF runs the same library over the same BO4E object, so an NB that
/// checks anything else is checking the wrong thing.
#[must_use]
pub fn render_and_check(
    settlement: SettlementResult,
    rechnungsnummer: String,
    invoice_date: time::Date,
    due_date: time::Date,
    facts: DocumentFacts,
) -> SettledInvoice {
    let pid = settlement.settlement_type.default_pid();
    let artikel_ids = facts.artikel_ids;
    let document = InvoiceDocument {
        settlement,
        pid,
        rechnungsnummer,
        correction_of: facts.correction_of,
        invoice_date,
        due_date,
        cadence: facts.cadence,
        abschlaege: facts.abschlaege,
    };
    let mut rechnung = grid_billing::bo4e::into_rechnung(&document);
    // The positions are numbered in the settlement's order, so the n-th
    // Rechnungsposition is the n-th settled position and its kind.
    for (p, (_, settled)) in rechnung
        .rechnungspositionen
        .iter_mut()
        .flatten()
        .zip(document.numbered_positions())
    {
        if let Some(id) = artikel_ids.get(&settled.kind) {
            p.artikel_id = Some(id.clone());
        }
    }
    let report = check(&rechnung, document.settlement.sender_mp_id.as_str(), pid);
    SettledInvoice {
        rechnungsnummer: document.rechnungsnummer,
        // What is actually owed, after any Abschläge — the figure the ledger
        // and the payment run care about.
        zu_zahlen_eur: document.settlement.steuer.brutto_eur()
            - document
                .abschlaege
                .iter()
                .map(|a| a.betrag_brutto_eur)
                .sum::<Decimal>(),
        settlement: document.settlement,
        rechnung,
        report,
        pid,
    }
}

/// Refuse a draft whose INVOIC could not go out: a fact the AHB requires that
/// neither the settlement nor anything netzbilanzd reads supplies.
///
/// Checked at drafting, before an invoice number is spent, rather than left to
/// makod's refusal at dispatch. The NN-Rechnung needs its **cadence**: `IMD`
/// DE 7081 is Muss on 31002 (INVOIC AHB 1.0b), and the settlement does not
/// carry it ([`grid_billing::bo4e::netznutzungrechnungstyp_for`]). The
/// NN-Rechnung Strom needs two facts more:
///
/// - every charge's **Artikel-ID** — `LIN` DE 7143 admits only `Z09` for a
///   Strom position from 01.01.2023 (INVOIC AHB 1.0b, 31002,
///   `[492] ∧ [59]`), and the settlement names no Artikel-ID
///   ([`grid_billing::bo4e::artikel_code`]);
/// - the **Lieferschein-MSCONS** — `SG1 RFF+ACE` is Muss for a period ending
///   after 01.12.2019 (`[492] ∧ [36] ∧ [50]`, S. 12).
///
/// # Errors
///
/// A sentence naming each missing fact.
#[must_use = "a refusal must reach the caller"]
pub fn refuse_unissuable(
    position: &BillingPositionRequest,
    settled: &SettledInvoice,
) -> anyhow::Result<()> {
    if let SettlementRequest::Msb(m) = &position.settlement {
        return refuse_unissuable_msb(position, m, settled);
    }
    let SettlementRequest::Nne(nne) = &position.settlement else {
        return Ok(());
    };
    let mut fehlt = Vec::new();
    if settled.rechnung.netznutzungrechnungstyp.is_none() {
        fehlt.push(
            "cadence is missing — the NN-Rechnung states its Rechnungstyp (IMD+7081: \
             Monats-, Turnus-, Zwischen- or Abschlussrechnung)"
                .to_owned(),
        );
    }
    if nne.sparte != Sparte::Strom {
        return if fehlt.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(
                "the NN-Rechnung for MaLo {} cannot go out as an INVOIC: {}",
                position.malo_id,
                fehlt.join("; ")
            )
        };
    }
    for (p, (n, sp)) in settled
        .rechnung
        .rechnungspositionen
        .iter()
        .flatten()
        .zip(settled.settlement.positions.iter().enumerate())
    {
        if p.artikel_id.is_none() {
            fehlt.push(format!(
                "position {} ({}) has no Artikel-ID — state it as artikel_ids.{:?}, from the \
                 Netznutzungspreisblatt",
                n + 1,
                sp.text,
                sp.kind
            ));
        }
    }
    let ende_nach_lieferschein_pflicht = position.period_to >= time::macros::date!(2019 - 11 - 30);
    if ende_nach_lieferschein_pflicht && nne.lieferschein_ref.as_deref().is_none_or(str::is_empty) {
        fehlt.push(
            "lieferschein_ref is missing — the NN-Rechnung Strom names the Lieferschein-MSCONS \
             it settles (SG1 RFF+ACE)"
                .to_owned(),
        );
    }
    if fehlt.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "the NN-Rechnung for MaLo {} cannot go out as an INVOIC: {}",
        position.malo_id,
        fehlt.join("; ")
    )
}

/// The MSB-Rechnung's share of [`refuse_unissuable`] (INVOIC AHB 1.0b, 31009):
///
/// - the **Bezugsdokument** to a Lieferant (the QUOTES, `[73] ∧ [72]`) or an
///   ESA (the ORDERS, `[66]`) — `SG1 RFF+ACE`, S. 12;
/// - from 01.01.2024, **day prices** — `PRI` DE 6411 admits only `DAY`
///   (`[12] ∧ [65]`) — and an **Artikel-ID** per position — `LIN` DE 7143
///   only `Z09` (`[65]`, S. 18).
fn refuse_unissuable_msb(
    position: &BillingPositionRequest,
    m: &crate::request::MsbRequest,
    settled: &SettledInvoice,
) -> anyhow::Result<()> {
    use grid_billing::MsbEmpfaengerRolle as R;
    let mut fehlt = Vec::new();
    if matches!(m.empfaenger_rolle, R::Lieferant | R::Energieserviceanbieter)
        && m.bezugsdokument.as_deref().is_none_or(str::is_empty)
    {
        fehlt.push(
            "bezugsdokument is missing — the MSB-Rechnung to a Lieferant names the QUOTES it \
             answers, to an ESA the ORDERS (SG1 RFF+ACE)"
                .to_owned(),
        );
    }
    if position.period_from >= time::macros::date!(2024 - 01 - 01) {
        if m.tagespreise.is_none() {
            fehlt.push(
                "tagespreise is missing — from 01.01.2024 the MSB-Rechnung prices per day \
                 (PRI DAY)"
                    .to_owned(),
            );
        }
        for (p, sp) in settled
            .rechnung
            .rechnungspositionen
            .iter()
            .flatten()
            .zip(&settled.settlement.positions)
        {
            if !p.artikel_id.as_deref().is_some_and(|a| a.contains('-')) {
                fehlt.push(format!(
                    "{} has no Artikel-ID — state it as artikel_ids.{:?} (Codeliste der \
                     Artikelnummern und Artikel-ID 5.6, Kap. 3.5)",
                    sp.text, sp.kind
                ));
            }
        }
    }
    if fehlt.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "the MSB-Rechnung for MaLo {} cannot go out as an INVOIC: {}",
        position.malo_id,
        fehlt.join("; ")
    )
}

pub use grid_billing::bo4e::{Beteiligte, mit_beteiligten};

/// The payload fields of the issuing command that are not the Rechnung —
/// what makod writes to `SG1 RFF+ACE`, `RFF+Z56` and `IMD` (makod: Issuing
/// the INVOIC).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kopfdaten {
    /// `SG1 RFF+ACE`.
    pub zugehoeriges_dokument: Option<String>,
    /// `SG1 RFF+Z56`.
    pub preise_mp_id: Option<String>,
    /// `IMD` DE 7081, where BO4E has no variant for it.
    pub rechnungstyp: Option<&'static str>,
    /// `DTM+203` Ausführungsdatum.
    pub ausfuehrungsdatum: Option<time::Date>,
}

/// The [`Kopfdaten`] of a draft, from its stored settlement input.
///
/// - NN-Rechnung Strom: the Lieferschein, and from a period beginning
///   01.01.2026 the Preise des Netzbetreibers (`[88] ∧ [492]`, INVOIC AHB
///   1.0b S. 13).
/// - MSB-Rechnung: to the NB or the LF `IMD++MSB` „Rechnung für
///   Messstellenbetrieb" (MIG 2.8e 00014, S. 19; 31009 `MSB=X [72] ∨ [76]`),
///   to an ESA `IMD++KON` „Abrechnung von Konfigurationen
///   (Universalbestellprozess)"; and the document it answers.
/// - Rechnung sonstige Leistung: `IMD++Z43` „Rechnung für Sperren und
///   Wiederinbetriebnahme" (MIG 2.8e 00014, S. 19) — the AWH of the
///   Sperrprozess —, the Sperrauftrag's ORDERS and the Ausführungsdatum.
#[must_use]
pub fn kopfdaten(settlement: &SettlementRequest, period_from: time::Date) -> Kopfdaten {
    match settlement {
        SettlementRequest::Nne(n) if n.sparte == Sparte::Strom => Kopfdaten {
            zugehoeriges_dokument: n.lieferschein_ref.clone(),
            preise_mp_id: (period_from >= time::macros::date!(2026 - 01 - 01))
                .then(|| n.preise_mp_id.clone().unwrap_or_else(|| n.nb_mp_id.clone())),
            ..Kopfdaten::default()
        },
        SettlementRequest::GasAwh(a) => Kopfdaten {
            zugehoeriges_dokument: Some(a.auftrag_ref.clone()),
            rechnungstyp: Some("Z43"),
            ausfuehrungsdatum: Some(a.ausfuehrungsdatum),
            ..Kopfdaten::default()
        },
        SettlementRequest::Msb(m) => Kopfdaten {
            zugehoeriges_dokument: m.bezugsdokument.clone(),
            rechnungstyp: Some(match m.empfaenger_rolle {
                grid_billing::MsbEmpfaengerRolle::Energieserviceanbieter => "KON",
                _ => "MSB",
            }),
            ..Kopfdaten::default()
        },
        _ => Kopfdaten::default(),
    }
}

/// The Rechnung as the INVOIC of its PID states it.
///
/// A Rechnung für Sperren carries the Ausführungsdatum and no Zeitraum: the
/// 31011 column lists the Abrechnungszeitraum (00007/00008) and the
/// positionsbezogene one (00038/00039) only `[53]`, on the
/// Blindarbeitrechnung (INVOIC AHB 1.0b), and a Zeitraum stated beside `Z43`
/// is a segment the column does not admit.
#[must_use]
pub fn fuer_die_invoic(
    mut rechnung: rubo4e::current::Rechnung,
    settlement: &SettlementRequest,
) -> rubo4e::current::Rechnung {
    if matches!(settlement, SettlementRequest::GasAwh(_)) {
        rechnung.rechnungsperiode = None;
        for p in rechnung.rechnungspositionen.iter_mut().flatten() {
            p.lieferungszeitraum = None;
        }
    }
    rechnung
}

/// Run `invoic-checker` over a rendered document.
///
/// The Preisblatt store is empty: `netzbilanzd` is the *issuer*, and the rates
/// it billed are the rates it published, so a tariff cross-check here would
/// compare a price sheet against itself. Stages 0–3 — Storno reference, period
/// validity, arithmetic and total consistency — are the ones that catch an
/// invoice the counterparty would dispute, and they need no store.
///
/// ## Why the outbound check runs at the inbound settings
///
/// [`CheckConfig::default`] is deliberate rather than incidental. This check
/// exists to answer one question — *would the receiving LF dispute this?* — and
/// the LF runs this same library at these same defaults. Tightening the
/// tolerances here would refuse documents the counterparty accepts, and
/// loosening them would send documents it rejects; either way the answer stops
/// being the one the check is for.
///
/// That extends to severity. `TotalMismatch` is a warning rather than a dispute
/// because the recipient treats it as one, and an issuer that blocked on it
/// would hold back invoices the counterparty would have paid. What it does not
/// extend to is silence: every finding is stored on the draft and returned in
/// the drafting response, so a warned document reaches an operator before it
/// reaches a counterparty.
#[must_use]
pub fn check(
    rechnung: &rubo4e::current::Rechnung,
    sender_mp_id: &str,
    pid: u32,
) -> invoic_checker::CheckReport {
    InvoicCheckEngine::check(
        pid,
        sender_mp_id,
        rechnung,
        &InMemoryPreisblattStore::new(),
        &CheckConfig::default(),
    )
}

/// An amount in units of 10⁻⁵ EUR, as stored.
///
/// # Errors
///
/// Returns an error when the amount does not fit an `i64` — at 10⁻⁵ EUR
/// resolution that is roughly ±92 billion EUR, so it means the inputs are
/// nonsense rather than that the invoice is large.
pub fn eur_units(amount_eur: Decimal) -> anyhow::Result<i64> {
    use rust_decimal::prelude::ToPrimitive as _;
    // `checked_mul`, not `*`: `Decimal`'s multiplication *panics* on overflow,
    // and a panic inside a billing run takes down the request rather than
    // refusing the one nonsensical position that caused it.
    amount_eur
        .checked_mul(Decimal::from(100_000_i64))
        .and_then(|units: Decimal| grid_billing::RoundMoney::round_kfm(&units, 0).to_i64())
        .with_context(|| format!("{amount_eur} EUR does not fit an i64 at 10⁻⁵ EUR"))
}

/// Render an amount in 10⁻⁵ EUR units as a decimal string.
///
/// Padded to five places, so every amount reported has the same shape and the
/// stored resolution is visible rather than trimmed away.
#[must_use]
pub fn format_eur(units: i64) -> String {
    format!("{:.5}", Decimal::from(units) / Decimal::from(100_000_i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{GasAwhRequest, MmmRequest, NneRequest};
    use grid_billing::{
        ArbeitspreisModell, AwhPositionInput, KaKundengruppe, Konzessionsabgabe, MengePreis,
        SettlementType,
    };
    use rust_decimal::dec;

    fn position(settlement: SettlementRequest) -> BillingPositionRequest {
        BillingPositionRequest {
            malo_id: "51238696012".to_owned(),
            period_from: time::macros::date!(2026 - 01 - 01),
            period_to: time::macros::date!(2026 - 01 - 31),
            cadence: None,
            abschlaege: Vec::new(),
            settlement,
        }
    }

    fn nne(sparte: Sparte) -> BillingPositionRequest {
        position(SettlementRequest::Nne(Box::new(NneRequest {
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            sparte,
            arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: dec!(1000),
                preis_ct_per_kwh: dec!(3.5),
            }),
            leistungspreis: None,
            grundpreis: None,
            konzessionsabgabe: None,
            blindarbeit: None,
            gas_kapazitaet: None,
            letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::default(),
            enfg_jahresvorverbrauch_kwh: None,
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            tariff_sheet_id: None,
            artikel_ids: std::collections::BTreeMap::new(),
            lieferschein_ref: None,
            preise_mp_id: None,
        })))
    }

    /// A Gas NN-Rechnung carries no electricity levies.
    ///
    /// The three EnFG levies (§19 StromNEV, Offshore-Netzumlage, KWKG-Umlage)
    /// ride on the *electricity* Netzentgelt. Settling gas as `Sparte::Strom`
    /// adds roughly 2.95 ct/kWh to a gas invoice — on a 3 000 kWh month, about
    /// 88 EUR with no legal basis whatever.
    #[test]
    fn a_gas_nne_carries_no_electricity_levies() {
        use grid_billing::BillingPositionKind as K;
        let gas = settle(&nne(Sparte::Gas)).expect("settle gas");
        let levies: Vec<_> = gas
            .positions
            .iter()
            .filter(|p| {
                matches!(
                    p.kind,
                    K::Sect19StromNevUmlage | K::OffshoreNetzumlage | K::KwkgUmlage
                )
            })
            .collect();
        assert!(
            levies.is_empty(),
            "gas invoice carries electricity levies: {levies:#?}"
        );
        assert_eq!(gas.settlement_type, SettlementType::NneGas);
        assert_eq!(gas.total_eur, dec!(35.00), "1000 kWh × 3.5 ct");

        // The Strom counterpart does carry them — the guard is the Sparte, not
        // a blanket removal.
        let strom = settle(&nne(Sparte::Strom)).expect("settle strom");
        assert!(
            strom.positions.iter().any(|p| matches!(
                p.kind,
                K::Sect19StromNevUmlage | K::OffshoreNetzumlage | K::KwkgUmlage
            )),
            "a Strom NN-Rechnung must carry the EnFG levies"
        );
    }

    /// NN-Rechnung is PID 31002 for both Sparten, taken from the engine.
    #[test]
    fn the_pid_comes_from_the_settlement_type() {
        for sparte in [Sparte::Strom, Sparte::Gas] {
            let s = settle(&nne(sparte)).expect("settle");
            assert_eq!(s.settlement_type.default_pid(), 31002);
        }
    }

    /// AWH is billed per action under GeLi Gas, not per kWh under StromNEV.
    #[test]
    fn awh_is_billed_per_action_under_geli_gas() {
        let settled = settle(&position(SettlementRequest::GasAwh(GasAwhRequest {
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            positionen: vec![
                AwhPositionInput {
                    beschreibung: "Sperrung Gaszähler".to_owned(),
                    anzahl: 1,
                    preis_eur: dec!(45.00),
                    artikel_id: Some("2-01-7-001".to_owned()),
                },
                AwhPositionInput {
                    beschreibung: "Erfolglose Unterbrechung".to_owned(),
                    anzahl: 2,
                    preis_eur: dec!(20.00),
                    artikel_id: Some("2-01-7-003".to_owned()),
                },
            ],
            tariff_sheet_id: None,
            auftrag_ref: "ORD-2026-0001".to_owned(),
            ausfuehrungsdatum: time::macros::date!(2026 - 01 - 14),
        })))
        .expect("settle AWH");

        assert_eq!(settled.settlement_type.default_pid(), 31011);
        assert_eq!(settled.total_eur, dec!(85.00), "45 + 2 × 20");
        assert_eq!(settled.sparte, Sparte::Gas);

        let refs: Vec<String> = settled
            .all_legal_refs()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(
            refs.iter().any(|r| r.contains("GeLi Gas 3.0")),
            "AWH must cite BK7-24-01-009 §5.4, got {refs:?}"
        );
        assert!(
            !refs.iter().any(|r| r.contains("StromNEV")),
            "a gas Sperrprozess invoice must not cite StromNEV, got {refs:?}"
        );
    }

    /// A Gas MMM settles under GaBi Gas, not under the Strom GPKE chapter.
    #[test]
    fn a_gas_mmm_cites_gabi_gas() {
        let settled = settle(&position(SettlementRequest::Mmm(MmmRequest {
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            sparte: Sparte::Gas,
            gemessen_kwh: dec!(1200),
            bilanziert_kwh: dec!(1000),
            mehr_preis_ct_per_kwh: Some(dec!(5.0)),
            minder_preis_ct_per_kwh: Some(dec!(4.0)),
            lastprofil: None,
            wiederverkaeufer: grid_billing::Wiederverkaeuferstatus::KEINER,
        })))
        .expect("settle gas MMM");

        assert_eq!(settled.settlement_type, SettlementType::MmmGas);
        let refs: Vec<String> = settled
            .all_legal_refs()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(
            refs.iter().any(|r| r.contains("GaBi Gas 2.1")),
            "Gas MMM must cite BK7-24-01-008, got {refs:?}"
        );
        assert!(
            !refs.iter().any(|r| r.contains("BK6-24-174")),
            "Gas MMM must not cite the Strom GPKE decision, got {refs:?}"
        );
    }

    /// A Konzessionsabgabe above the KAV §2 ceiling is reported, not billed silently.
    ///
    /// 1.32 ct/kWh is the Tarifkunden Höchstsatz for a Gemeinde up to 25 000 —
    /// on a Sondervertragskunde, whose ceiling is 0.11, it is twelve times the
    /// lawful maximum. The rate and the group travel together so the ceiling is
    /// checkable at all.
    #[test]
    fn a_konzessionsabgabe_above_the_kav_ceiling_is_reported() {
        let with_ka = |klasse| {
            let mut pos = nne(Sparte::Strom);
            let SettlementRequest::Nne(nne) = &mut pos.settlement else {
                unreachable!()
            };
            nne.konzessionsabgabe = Some(Konzessionsabgabe {
                satz_ct_per_kwh: dec!(1.32),
                klasse,
                niederspannung: None,
                grenzpreis: None,
            });
            settle(&pos).expect("settle")
        };
        let breached = |s: &grid_billing::SettlementResult| {
            s.warnings.iter().any(|w| w.code == "KA_ABOVE_KAV_MAXIMUM")
        };

        let sondervertrag = with_ka(KaKundengruppe::Sondervertragskunde);
        assert!(
            breached(&sondervertrag),
            "1.32 ct/kWh is twelve times the 0.11 Sondervertragskunden ceiling: {:#?}",
            sondervertrag.warnings
        );

        // The same rate on a Tarifkunde in a small Gemeinde *is* the ceiling.
        let tarifkunde = with_ka(KaKundengruppe::Tarifkunde {
            gemeinde: grid_billing::GemeindeGroesse::Bis25k,
            nur_kochen_warmwasser: false,
        });
        assert!(
            !breached(&tarifkunde),
            "1.32 ct/kWh is the lawful Tarifkunden maximum: {:#?}",
            tarifkunde.warnings
        );
    }

    /// §14a Modul 3 produces one position per Tarifstufe, and they are billed.
    #[test]
    fn modul_3_bills_all_three_bands() {
        let mut pos = nne(Sparte::Strom);
        {
            let SettlementRequest::Nne(nne) = &mut pos.settlement else {
                unreachable!()
            };
            nne.arbeitspreis = ArbeitspreisModell::Modul3ZeitVariabel {
                ht: MengePreis {
                    menge_kwh: dec!(500),
                    preis_ct_per_kwh: dec!(32.0),
                },
                st: MengePreis {
                    menge_kwh: dec!(300),
                    preis_ct_per_kwh: dec!(28.0),
                },
                nt: MengePreis {
                    menge_kwh: dec!(200),
                    preis_ct_per_kwh: dec!(24.0),
                },
            };
            // §21 EnFG exemption keeps the levies out of the arithmetic asserted here.
            nne.letztverbrauchergruppe = grid_billing::umlagen::Letztverbrauchergruppe::Befreit;
        }

        let settled = settle(&pos).expect("settle");
        // 500×32 + 300×28 + 200×24 = 160 + 84 + 48 = 292.00 EUR
        assert_eq!(settled.total_eur, dec!(292.00));
        assert_eq!(
            settled.positions.len(),
            3,
            "one position per Tarifstufe: {:#?}",
            settled.positions
        );
    }

    // ── The INVOIC makod sends for a draft ────────────────────────────────────

    fn partner(
        name: &str,
        strasse: &str,
        plz: &str,
        ort: &str,
        ust: &str,
    ) -> rubo4e::current::Geschaeftspartner {
        rubo4e::current::Geschaeftspartner {
            organisationsname: Some(name.to_owned()),
            adresse: Some(rubo4e::current::Adresse {
                strasse: Some(strasse.to_owned()),
                hausnummer: Some("1".to_owned()),
                postleitzahl: Some(plz.to_owned()),
                ort: Some(ort.to_owned()),
                landescode: Some(rubo4e::current::Landescode::De),
                ..Default::default()
            }),
            umsatzsteuer_id: Some(ust.to_owned()),
            ..Default::default()
        }
    }

    /// The parties and the Marktlokation as marktd records them.
    fn beteiligte() -> Beteiligte {
        Beteiligte {
            ersteller: partner(
                "Stadtnetz Musterstadt GmbH",
                "Netzweg",
                "10115",
                "Berlin",
                "DE123456789",
            ),
            empfaenger: partner(
                "Lieferant Beispiel AG",
                "Handelsplatz",
                "50667",
                "Köln",
                "DE987654321",
            ),
            marktlokation: rubo4e::current::Marktlokation {
                lokationsadresse: Some(rubo4e::current::Adresse {
                    strasse: Some("Lindenallee".to_owned()),
                    hausnummer: Some("12".to_owned()),
                    postleitzahl: Some("10115".to_owned()),
                    ort: Some("Berlin".to_owned()),
                    landescode: Some(rubo4e::current::Landescode::De),
                    ..Default::default()
                }),
                ..Default::default()
            },
        }
    }

    /// The message makod builds for `rechnung` under `pid`, with the payload
    /// fields netzbilanzd sends beside it.
    fn invoic(
        pid: u32,
        rechnung: &rubo4e::current::Rechnung,
        kopf: &Kopfdaten,
    ) -> Result<String, makod::orchestrator::invoic_ausgang::InvoicAusgangError> {
        invoic_from(pid, rechnung, kopf, "9900357000004", "9900012345678")
    }

    fn invoic_from(
        pid: u32,
        rechnung: &rubo4e::current::Rechnung,
        kopf: &Kopfdaten,
        sender: &str,
        receiver: &str,
    ) -> Result<String, makod::orchestrator::invoic_ausgang::InvoicAusgangError> {
        use makod::orchestrator::invoic_ausgang as ausgang;
        let profile =
            ausgang::profil(time::macros::date!(2026 - 11 - 06)).expect("INVOIC in force");
        ausgang::rechnung(
            profile,
            rechnung,
            &ausgang::Kopf {
                pid,
                sender,
                receiver,
                message_ref: "INV1",
                buchungsdatum: time::macros::datetime!(2026-02-01 09:00 UTC),
                zugehoeriges_dokument: kopf.zugehoeriges_dokument.as_deref(),
                preise_mp_id: kopf.preise_mp_id.as_deref(),
                rechnungstyp: kopf.rechnungstyp,
                ausfuehrungsdatum: kopf.ausfuehrungsdatum,
            },
        )
        .map(|m| m.nachricht)
    }

    /// An NN-Rechnung Strom for January 2026, with the Artikel-IDs of its
    /// price sheet — Codeliste der Artikelnummern und Artikel-ID 5.6:
    /// `1-02-0-002` Arbeitspreis (Kap. 3.1.2, S. 18), `1-10-1-001` KWKG,
    /// `1-10-2-001` Offshore-Netzumlage (Kap. 3.1.10, S. 41), `1-10-4-001`
    /// § 19 StromNEV (S. 42) — and its Lieferschein.
    fn nne_strom_zum_versand() -> BillingPositionRequest {
        use grid_billing::BillingPositionKind as K;
        let mut position = nne(Sparte::Strom);
        position.cadence = Some(grid_billing::Rechnungscharakter::Monatsrechnung);
        if let SettlementRequest::Nne(n) = &mut position.settlement {
            n.artikel_ids = [
                (K::NneArbeit, "1-02-0-002"),
                (K::KwkgUmlage, "1-10-1-001"),
                (K::OffshoreNetzumlage, "1-10-2-001"),
                (K::Sect19StromNevUmlage, "1-10-4-001"),
            ]
            .into_iter()
            .map(|(k, id)| (k, id.to_owned()))
            .collect();
            n.lieferschein_ref = Some("LS-2026-0001".to_owned());
        }
        position
    }

    fn modul1_januar() -> ArbeitspreisModell {
        ArbeitspreisModell::Modul1Pauschal {
            basis: MengePreis {
                menge_kwh: dec!(1000),
                preis_ct_per_kwh: dec!(3.5),
            },
            pauschale_eur_pro_jahr: dec!(120),
        }
    }

    fn drafted(position: &BillingPositionRequest, nummer: &str) -> SettledInvoice {
        render_and_check(
            settle(position).expect("settle"),
            nummer.to_owned(),
            time::macros::date!(2026 - 02 - 01),
            time::macros::date!(2026 - 03 - 03),
            DocumentFacts {
                cadence: position.cadence,
                artikel_ids: DocumentFacts::artikel_ids_of(&position.settlement),
                ..DocumentFacts::default()
            },
        )
    }

    /// A drafted NN-Rechnung Strom, with the parties and the Marktlokation
    /// dispatch adds, is a 31002 makod sends with no finding: every charge
    /// carries its Artikel-ID as `Z09`, the Lieferschein is `RFF+ACE`, and a
    /// period from 01.01.2026 names the Preise des Netzbetreibers.
    #[test]
    fn a_dispatched_nn_rechnung_strom_is_a_valid_31002() {
        let position = nne_strom_zum_versand();
        let settled = drafted(&position, "NNE-2026-000001");
        refuse_unissuable(&position, &settled).expect("issuable");
        let kopf = kopfdaten(&position.settlement, position.period_from);

        // As drafted, the Rechnung names no party and no Marktlokation.
        let e = invoic(31002, &settled.rechnung, &kopf).expect_err("refused as drafted");
        assert!(e.places().contains(&"00022".to_owned()), "{e}");

        let r = mit_beteiligten(settled.rechnung, &position.malo_id, beteiligte()).expect("MaLo");
        let n = invoic(31002, &r, &kopf).unwrap_or_else(|e| panic!("refused: {e}"));
        assert!(n.contains("LIN+1++1-02-0-002:Z09'"), "{n}");
        assert!(n.contains("RFF+ACE:LS-2026-0001'"), "{n}");
        assert!(n.contains("RFF+Z56:9900357000004'"), "{n}");
        assert!(n.contains("LOC+172+51238696012'"), "{n}");
        assert!(n.contains("IMD++MVR'"), "{n}");
    }

    /// A 1 234,567 kWh NN-Rechnung Strom goes out: every position net is in
    /// cents (`MOA+203` `[930]`) and the Summenteil is their sum.
    #[test]
    fn an_nn_rechnung_strom_of_any_quantity_is_a_valid_31002() {
        let mut position = nne_strom_zum_versand();
        if let SettlementRequest::Nne(n) = &mut position.settlement {
            n.arbeitspreis = ArbeitspreisModell::Einheitlich(MengePreis {
                menge_kwh: dec!(1234.567),
                preis_ct_per_kwh: dec!(3.517),
            });
        }
        let settled = drafted(&position, "NNE-2026-000004");
        let kopf = kopfdaten(&position.settlement, position.period_from);
        let r = mit_beteiligten(settled.rechnung, &position.malo_id, beteiligte()).expect("MaLo");
        invoic(31002, &r, &kopf).unwrap_or_else(|e| panic!("refused: {e}"));
    }

    /// A Gas NN-Rechnung with its monthly Verrechnungspreis goes out: the
    /// months at `QTY+136`, the count `1` `H87` at `QTY+47`.
    #[test]
    fn a_gas_nn_rechnung_with_a_grundpreis_is_a_valid_31002() {
        let mut position = nne(Sparte::Gas);
        position.cadence = Some(grid_billing::Rechnungscharakter::Monatsrechnung);
        if let SettlementRequest::Nne(n) = &mut position.settlement {
            n.nb_mp_id = "9800357000004".to_owned();
            n.lf_mp_id = "9800012345678".to_owned();
            n.grundpreis = Some(grid_billing::Grundpreis {
                eur_per_month: dec!(15),
                months: dec!(1),
            });
        }
        let settled = drafted(&position, "NNE-2026-000005");
        assert!(
            settled.report.outcome != invoic_checker::CheckOutcome::Dispute,
            "{:?}",
            settled.report.findings
        );
        let r = mit_beteiligten(settled.rechnung, &position.malo_id, beteiligte()).expect("MaLo");
        let n = invoic_from(
            31002,
            &r,
            &Kopfdaten::default(),
            "9800357000004",
            "9800012345678",
        )
        .unwrap_or_else(|e| panic!("refused: {e}"));
        assert!(n.contains("QTY+47:1:H87'QTY+136:1:MON'"), "{n}");
        assert!(n.contains("PRI+CAL:15::::MON'"), "{n}");
    }

    /// A drafted Rechnung sonstige Leistung is a 31011 makod sends with no
    /// finding: `IMD++Z43`, the Sperrauftrag's ORDERS at `RFF+ACE`, the
    /// Ausführungsdatum at `DTM+203`, each AWH position with its Artikel-ID —
    /// and no Zeitraum, which the column admits only on a Blindarbeitrechnung.
    #[test]
    fn a_dispatched_rechnung_sonstige_leistung_is_a_valid_31011() {
        let position = position(SettlementRequest::GasAwh(GasAwhRequest {
            nb_mp_id: "9800357000004".to_owned(),
            lf_mp_id: "9800012345678".to_owned(),
            positionen: vec![AwhPositionInput {
                beschreibung: "Unterbrechung der Anschlussnutzung".to_owned(),
                anzahl: 1,
                preis_eur: dec!(60.00),
                artikel_id: Some("2-01-7-001".to_owned()),
            }],
            tariff_sheet_id: None,
            auftrag_ref: "ORD-2026-0001".to_owned(),
            ausfuehrungsdatum: time::macros::date!(2026 - 01 - 14),
        }));
        let settled = drafted(&position, "AWH-2026-000001");
        let kopf = kopfdaten(&position.settlement, position.period_from);
        let r = mit_beteiligten(settled.rechnung, &position.malo_id, beteiligte()).expect("MaLo");
        let r = fuer_die_invoic(r, &position.settlement);
        let n = invoic_from(31011, &r, &kopf, "9800357000004", "9800012345678")
            .unwrap_or_else(|e| panic!("refused: {e}"));
        assert!(n.contains("DTM+203:20260114:102'IMD++Z43'"), "{n}");
        assert!(n.contains("RFF+ACE:ORD-2026-0001'"), "{n}");
        assert!(n.contains("LIN+1++2-01-7-001:Z09'"), "{n}");
    }

    /// A MSB-Rechnung to the Lieferant for January 2026, priced per day: the
    /// Preisobergrenze of a verbrauchende Marktlokation `]6.000 kWh/a;
    /// 10.000 kWh/a]` mit iMS, Artikel-ID `4-02-0-006` (Codeliste 5.6 Kap.
    /// 3.5.1, S. 52), answering the QUOTES of the accepted Angebot.
    fn msb_an_lf() -> BillingPositionRequest {
        use grid_billing::BillingPositionKind as K;
        position(SettlementRequest::Msb(crate::request::MsbRequest {
            msb_mp_id: "9900357000004".to_owned(),
            empfaenger_rolle: grid_billing::MsbEmpfaengerRolle::Lieferant,
            empfaenger_mp_id: "9900012345678".to_owned(),
            sparte: Sparte::Strom,
            grundgebuehr_eur_per_month: Decimal::ZERO,
            billing_months: 0,
            tagespreise: Some(grid_billing::MsbTagespreise {
                grundgebuehr_eur_per_tag: dec!(0.10),
                steuereinrichtung_eur_per_tag: None,
            }),
            artikel_ids: [(K::MsbGrundgebuehr, "4-02-0-006".to_owned())]
                .into_iter()
                .collect(),
            bezugsdokument: Some("QUOTES-2025-0042".to_owned()),
            messdienstleistung_eur: None,
            steuereinrichtung_eur_per_month: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        }))
    }

    /// A drafted MSB-Rechnung to the Lieferant is a 31009 makod sends with no
    /// finding: `IMD++MSB`, the QUOTES at `RFF+ACE`, the Artikel-ID as `Z09`,
    /// 31 days at `QTY+136` and the price per `DAY`.
    #[test]
    fn a_dispatched_msb_rechnung_is_a_valid_31009() {
        let position = msb_an_lf();
        let settled = drafted(&position, "MSB-2026-000001");
        refuse_unissuable(&position, &settled).expect("issuable");
        let kopf = kopfdaten(&position.settlement, position.period_from);
        let r = mit_beteiligten(settled.rechnung, &position.malo_id, beteiligte()).expect("MaLo");
        let n = invoic(31009, &r, &kopf).unwrap_or_else(|e| panic!("refused: {e}"));
        assert!(n.contains("IMD++MSB'"), "{n}");
        assert!(n.contains("RFF+ACE:QUOTES-2025-0042'"), "{n}");
        assert!(
            n.contains("LIN+1++4-02-0-006:Z09'QTY+47:1:H87'QTY+136:31:DAY'"),
            "{n}"
        );
        assert!(n.contains("PRI+CAL:0.1::::DAY'"), "{n}");
    }

    /// Without its QUOTES, its day prices or its Artikel-IDs, a MSB-Rechnung
    /// from 2024 is refused at drafting, each named.
    #[test]
    fn a_msb_rechnung_without_what_the_invoic_needs_is_refused() {
        let mut position = msb_an_lf();
        if let SettlementRequest::Msb(m) = &mut position.settlement {
            m.bezugsdokument = None;
            m.artikel_ids.clear();
            m.tagespreise = None;
            m.grundgebuehr_eur_per_month = dec!(3);
            m.billing_months = 1;
        }
        let settled = drafted(&position, "MSB-2026-000002");
        let e = refuse_unissuable(&position, &settled)
            .expect_err("refused")
            .to_string();
        assert!(e.contains("bezugsdokument"), "{e}");
        assert!(e.contains("tagespreise"), "{e}");
        assert!(e.contains("artikel_ids.MsbGrundgebuehr"), "{e}");
    }

    fn mmm(gemessen: Decimal, bilanziert: Decimal) -> BillingPositionRequest {
        position(SettlementRequest::Mmm(MmmRequest {
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            sparte: Sparte::Strom,
            gemessen_kwh: gemessen,
            bilanziert_kwh: bilanziert,
            mehr_preis_ct_per_kwh: Some(dec!(9.87)),
            minder_preis_ct_per_kwh: Some(dec!(9.12)),
            lastprofil: None,
            wiederverkaeufer: grid_billing::Wiederverkaeuferstatus::KEINER,
        }))
    }

    fn mmm_invoic(position: &BillingPositionRequest, pid: u32) -> String {
        let settled = drafted(position, "MMM-2026-000001");
        assert_eq!(settled.pid, pid);
        let mut b = beteiligte();
        b.marktlokation.energierichtung = Some(rubo4e::current::Energierichtung::Aussp);
        let r = mit_beteiligten(settled.rechnung, &position.malo_id, b).expect("MaLo");
        invoic(pid, &r, &Kopfdaten::default()).unwrap_or_else(|e| panic!("refused: {e}"))
    }

    /// A Mindermenge is the NB's 31005: the quantity in whole kWh, `IMD`
    /// DE 7009 from the Marktlokation's Energierichtung.
    #[test]
    fn a_mindermenge_is_a_valid_31005() {
        let n = mmm_invoic(&mmm(dec!(1234.567), dec!(1000)), 31005);
        assert!(n.contains("IMD++MMM+Z07'"), "{n}");
        assert!(
            n.contains("LIN+1++9990001000756:Z01'QTY+47:235:KWH'"),
            "{n}"
        );
    }

    /// A Mehrmenge is the NB's selbst ausgestellte 31006 to the LF: `BGM+389`,
    /// the LF's Guthaben stated positive, no Korrekturfaktor.
    #[test]
    fn a_mehrmenge_is_a_valid_31006() {
        let n = mmm_invoic(&mmm(dec!(1000), dec!(1234.567)), 31006);
        assert!(n.contains("BGM+389+"), "{n}");
        assert!(n.contains("NAD+MS+9900357000004"), "{n}");
        assert!(
            n.contains("LIN+1++9990001000748:Z01'QTY+47:235:KWH'"),
            "{n}"
        );
        assert!(n.contains("MOA+203:23.19'PRI+CAL:0.0987'"), "{n}");
        assert!(!n.contains("QTY+Z17"), "{n}");
    }

    /// A §14a Modul 1 NN-Rechnung Strom is a 31002 makod sends with no
    /// finding: the pauschale Reduzierung credited for the period's days at
    /// its day rate, Artikel-ID `1-02-0-015` (€/Tag; Codeliste der
    /// Artikelnummern und Artikel-ID 5.6, Kap. 3.1.2, S. 19) — `QTY+136`
    /// `DAY`, the one time unit the 31002 column admits for Strom from 2023.
    #[test]
    fn a_modul_1_nn_rechnung_strom_is_a_valid_31002() {
        use grid_billing::BillingPositionKind as K;
        let mut position = nne_strom_zum_versand();
        if let SettlementRequest::Nne(n) = &mut position.settlement {
            n.arbeitspreis = modul1_januar();
            n.artikel_ids
                .insert(K::NneArbeitModul1, "1-02-0-002".to_owned());
            n.artikel_ids
                .insert(K::NneModul1Pauschale, "1-02-0-015".to_owned());
        }
        let settled = drafted(&position, "NNE-2026-000006");
        let kopf = kopfdaten(&position.settlement, position.period_from);
        let r = mit_beteiligten(settled.rechnung, &position.malo_id, beteiligte()).expect("MaLo");
        let n = invoic(31002, &r, &kopf).unwrap_or_else(|e| panic!("refused: {e}"));
        assert!(
            n.contains("LIN+2++1-02-0-015:Z09'QTY+47:1:H87'QTY+136:31:DAY'"),
            "{n}"
        );
        assert!(
            n.contains("MOA+203:-10.19'PRI+CAL:-0.328767::::DAY'"),
            "{n}"
        );
    }

    /// A drafted Abschlagsrechnung is a 31001 makod sends with no finding: one
    /// position, the Abschlag's BDEW-Artikelnummer as `Z01`.
    #[test]
    fn a_dispatched_abschlagsrechnung_is_a_valid_31001() {
        let position = position(SettlementRequest::Abschlag(
            crate::request::AbschlagRequest {
                nb_mp_id: "9900357000004".to_owned(),
                lf_mp_id: "9900012345678".to_owned(),
                sparte: Sparte::Strom,
                betrag_netto_eur: dec!(50),
                grundlage: grid_billing::AbschlagGrundlage::Vorjahresverbrauch,
            },
        ));
        let settled = drafted(&position, "ABS-2026-000001");
        refuse_unissuable(&position, &settled).expect("issuable");
        let kopf = kopfdaten(&position.settlement, position.period_from);
        assert_eq!(
            kopf,
            Kopfdaten::default(),
            "the 31001 column has no RFF+ACE and no RFF+Z56"
        );
        let r = mit_beteiligten(settled.rechnung, &position.malo_id, beteiligte()).expect("MaLo");
        let n = invoic(31001, &r, &kopf).unwrap_or_else(|e| panic!("refused: {e}"));
        assert!(n.contains("LIN+1++9990001000376:Z01'"), "{n}");
        assert!(n.contains("IMD++ABS'"), "{n}");
    }

    /// An NN-Rechnung Strom without its Artikel-IDs, its Lieferschein or its
    /// cadence is refused at drafting, each named.
    #[test]
    fn an_nn_rechnung_strom_without_what_the_invoic_needs_is_refused() {
        let mut position = nne(Sparte::Strom);
        let settled = drafted(&position, "NNE-2026-000002");
        let e = refuse_unissuable(&position, &settled)
            .expect_err("refused")
            .to_string();
        assert!(e.contains("artikel_ids.NneArbeit"), "{e}");
        assert!(e.contains("lieferschein_ref"), "{e}");
        assert!(e.contains("cadence"), "{e}");

        // A Gas NN-Rechnung keeps its Artikelnummern and needs no Lieferschein.
        position = nne(Sparte::Gas);
        position.cadence = Some(grid_billing::Rechnungscharakter::Monatsrechnung);
        refuse_unissuable(&position, &drafted(&position, "NNE-2026-000003")).expect("issuable");
    }

    /// `RFF+Z56` only from a period beginning 01.01.2026 (`[88]`), and only
    /// on the Strom NN-Rechnung (`[492]`).
    #[test]
    fn the_preise_des_netzbetreibers_follow_the_period_and_the_sparte() {
        let mut position = nne_strom_zum_versand();
        assert!(
            kopfdaten(&position.settlement, position.period_from)
                .preise_mp_id
                .is_some()
        );
        assert_eq!(
            kopfdaten(&position.settlement, time::macros::date!(2025 - 12 - 01)).preise_mp_id,
            None
        );
        position.settlement = nne(Sparte::Gas).settlement;
        assert_eq!(
            kopfdaten(&position.settlement, position.period_from),
            Kopfdaten::default()
        );
    }

    /// A rendered invoice is checked, and a well-formed one passes.
    #[test]
    fn a_rendered_invoice_passes_its_own_check() {
        let settlement = settle(&nne(Sparte::Strom)).expect("settle");
        let settled = render_and_check(
            settlement,
            "NNE-2026-000001".to_owned(),
            time::macros::date!(2026 - 02 - 01),
            time::macros::date!(2026 - 03 - 03),
            DocumentFacts::default(),
        );
        assert_eq!(settled.pid, 31002);
        assert_ne!(
            settled.report.outcome,
            invoic_checker::CheckOutcome::Dispute,
            "{:#?}",
            settled.report.findings
        );
        assert_eq!(
            settled.rechnung.sparte,
            Some(rubo4e::current::Sparte::Strom),
            "the Sparte must reach the wire — PID 31002 does not carry it"
        );
    }

    /// Amounts render from the integer, padded to the stored resolution.
    #[test]
    fn an_amount_renders_at_full_resolution() {
        assert_eq!(format_eur(123_456_000), "1234.56000");
        assert_eq!(format_eur(-1), "-0.00001");
        assert_eq!(format_eur(0), "0.00000");
    }

    /// A settled invoice states its Umsatzsteuer, and its own gate accepts it.
    ///
    /// The gate is `invoic-checker`, which the receiving LF runs on the same
    /// document. An invoice carrying a net figure and nothing else is not a
    /// Rechnung under §14 Abs. 4 Nr. 8 UStG, and is worth no Vorsteuerabzug to
    /// the counterparty.
    #[test]
    fn a_settled_invoice_states_its_tax_and_passes_its_own_gate() {
        let settlement = settle(&nne(Sparte::Strom)).expect("settle");
        let netto = settlement.total_eur;
        assert_eq!(settlement.steuer.satz_prozent, dec!(19));
        assert_eq!(
            settlement.steuer.brutto_eur(),
            netto + settlement.steuer.steuer_eur
        );

        let settled = render_and_check(
            settlement,
            "NNE-2026-000001".to_owned(),
            time::macros::date!(2026 - 02 - 01),
            time::macros::date!(2026 - 03 - 03),
            DocumentFacts::default(),
        );
        assert_ne!(
            settled.report.outcome,
            invoic_checker::CheckOutcome::Dispute,
            "{:#?}",
            settled.report.findings
        );
        assert!(
            settled.rechnung.gesamtsteuer.is_some() && settled.rechnung.gesamtbrutto.is_some(),
            "the tax block must reach the document"
        );
    }

    /// A Gas Mehr-/Mindermenge to a Wiederverkäufer is reverse-charged; the same
    /// settlement to a counterparty without §3g status is taxed at 19 %.
    ///
    /// A Mehr-/Mindermenge is a Lieferung, not a network service, so §13b
    /// Abs. 2 Nr. 5 Buchst. b reaches it — and for gas the recipient's status
    /// alone decides.
    #[test]
    fn a_gas_mmm_follows_the_recipients_wiederverkaeufer_status() {
        let settle_with = |status: grid_billing::Wiederverkaeuferstatus| {
            settle(&position(SettlementRequest::Mmm(MmmRequest {
                nb_mp_id: "9900357000004".to_owned(),
                lf_mp_id: "9900012345678".to_owned(),
                sparte: Sparte::Gas,
                gemessen_kwh: dec!(1200),
                bilanziert_kwh: dec!(1000),
                mehr_preis_ct_per_kwh: Some(dec!(5.0)),
                minder_preis_ct_per_kwh: Some(dec!(4.0)),
                lastprofil: None,
                wiederverkaeufer: status,
            })))
            .expect("settle")
        };

        let taxed = settle_with(grid_billing::Wiederverkaeuferstatus::KEINER);
        assert_eq!(taxed.steuer.satz_prozent, dec!(19));
        assert!(taxed.steuer.steuer_eur > rust_decimal::Decimal::ZERO);

        // For gas the recipient's status alone shifts it — the issuer's does not
        // matter, which is the opposite of the electricity condition.
        let verlagert = settle_with(grid_billing::Wiederverkaeuferstatus {
            leistender: false,
            empfaenger: true,
        });
        assert_eq!(verlagert.steuer.steuer_eur, rust_decimal::Decimal::ZERO);
        assert_eq!(
            verlagert.steuer.hinweis,
            Some("Steuerschuldnerschaft des Leistungsempfängers")
        );
    }

    /// The stored total is exact at 10⁻⁵ EUR, and absurd inputs are refused
    /// rather than silently truncated.
    #[test]
    fn the_stored_total_is_exact() {
        assert_eq!(eur_units(dec!(1234.56)).expect("fits"), 123_456_000);
        assert_eq!(eur_units(dec!(-10.0)).expect("fits"), -1_000_000);
        // ±92 billion EUR at 10⁻⁵ resolution is the i64 boundary. Beyond it the
        // inputs are nonsense, and truncating silently would report a plausible
        // total for an invoice that was never computable.
        assert!(eur_units(Decimal::MAX).is_err());
    }

    /// `grid-billing`'s pre-flight validators were computed and ignored.
    ///
    /// Each returns a `ValidationResult` whose `is_valid` goes false on an
    /// `Error`-severity finding, and the crate's README documents them as the
    /// gate a service runs. No service ran one: `is_valid` was written by the
    /// engine and read only by a unit test, so an MSB settlement with
    /// `billing_months = 0` billed a Grundgebühr of nothing, and a Gas-AWH
    /// settlement with no positions billed an empty invoice — both to a
    /// counterparty, both with the defect already named in the result.
    #[test]
    fn an_input_the_validator_rejects_is_not_billed() {
        use grid_billing::{
            AwhPositionInput, GasAwhInput, MsbInput, MsbRechnungsempfaenger, SettlementPeriod,
            Sparte, validate_gas_awh_input, validate_msb_input,
        };
        use rust_decimal::dec;
        use time::macros::date;

        let period = SettlementPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31)).unwrap();

        // A Messstellenbetrieb invoice over zero months.
        let msb = MsbInput {
            malo_id: "51238696781".to_owned(),
            msb_mp_id: "9900000000002".to_owned(),
            empfaenger: MsbRechnungsempfaenger {
                rolle: grid_billing::MsbEmpfaengerRolle::Lieferant,
                mp_id: "9900000000001".to_owned(),
            },
            period,
            sparte: Sparte::Strom,
            grundgebuehr_eur_per_month: dec!(20),
            billing_months: 0,
            messdienstleistung_eur: None,
            steuereinrichtung_eur_per_month: None,
            tagespreise: None,
            messstellen_kategorie: None,
            entgeltschuldner: None,
        };
        let err = refuse_invalid("MSB", "51238696781", validate_msb_input(&msb))
            .expect_err("zero billing months is not billable");
        assert!(err.to_string().contains("ZERO_BILLING_MONTHS"), "{err}");

        // A Gas-AWH invoice with nothing on it.
        let awh = GasAwhInput {
            malo_id: "51238696781".to_owned(),
            nb_mp_id: "9900000000003".to_owned(),
            lf_mp_id: "9900000000001".to_owned(),
            period,
            awh_positionen: Vec::new(),
            tariff_sheet_id: None,
        };
        let err = refuse_invalid("AWH", "51238696781", validate_gas_awh_input(&awh))
            .expect_err("an empty AWH invoice is not billable");
        assert!(err.to_string().contains("EMPTY_AWH_POSITIONEN"), "{err}");

        // …and a sound input still passes.
        let ok = GasAwhInput {
            awh_positionen: vec![AwhPositionInput {
                beschreibung: "Sperrung Gaszähler".to_owned(),
                anzahl: 1,
                preis_eur: dec!(15),
                artikel_id: Some("2-01-7-001".to_owned()),
            }],
            ..awh
        };
        assert!(refuse_invalid("AWH", "51238696781", validate_gas_awh_input(&ok)).is_ok());
    }
}
