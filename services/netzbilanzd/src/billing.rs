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
    SettlementPeriod, SettlementResult, Sparte, settle_abschlag, settle_gas_awh, settle_mmm,
    settle_msb, settle_nne,
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
            // German market (§ 13 Abs. 3 StromNZV) and published monthly by the
            // BDEW, so the month alone identifies them. There is no operator
            // dimension to configure — an earlier `vnb_mp_id` setting made
            // every Strom MMM settlement refuse until an operator named an ÜNB
            // whose own series was never published.
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
            settle_mmm(&MmmInput {
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
            })
        }
        SettlementRequest::Msb(msb) => settle_msb(&MsbInput {
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
            messstellen_kategorie: msb.messstellen_kategorie,
            entgeltschuldner: msb.entgeltschuldner,
        }),
        SettlementRequest::GasAwh(awh) => settle_gas_awh(&GasAwhInput {
            malo_id: position.malo_id.clone(),
            nb_mp_id: awh.nb_mp_id.clone(),
            lf_mp_id: awh.lf_mp_id.clone(),
            period,
            awh_positionen: awh.positionen.clone(),
            tariff_sheet_id: awh.tariff_sheet_id.clone(),
        }),
    };

    result.map_err(|e| anyhow::anyhow!("settlement failed for MaLo {}: {e}", position.malo_id))
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
    let rechnung = grid_billing::bo4e::into_rechnung(&document);
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

/// Run `invoic-checker` over a rendered document.
///
/// The Preisblatt store is empty: `netzbilanzd` is the *issuer*, and the rates
/// it billed are the rates it published, so a tariff cross-check here would
/// compare a price sheet against itself. Stages 0–3 — Storno reference, period
/// validity, arithmetic and total consistency — are the ones that catch an
/// invoice the counterparty would dispute, and they need no store.
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
        .and_then(|units: Decimal| units.round().to_i64())
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
            sect19_umlage_ct_per_kwh: None,
            offshore_umlage_ct_per_kwh: None,
            kwkg_umlage_ct_per_kwh: None,
            netzebene: None,
            sect19: None,
            jahreshoechstleistung_kw: None,
            jahresarbeit_kwh: None,
            tariff_sheet_id: None,
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
}
