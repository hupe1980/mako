//! BO4E bridge — renders an [`crate::InvoiceDocument`] as a
//! `rubo4e::current::Rechnung`. Feature-gated behind `bo4e` so the core engine
//! stays rubo4e-free: settlements are computed in pure domain types, and only
//! consumers that store or dispatch a grid invoice (netzbilanzd, invoicd) enable
//! the feature. The rendered Rechnung carries:
//!
//! - `rechnungsnummer`, `rechnungsdatum`, `faelligkeitsdatum` — document facts
//! - per-position `mako:calculation_trace` ZusatzAttribut
//! - settlement-level `mako:legal_references` and (when present)
//!   `mako:settlement_warnings` ZusatzAttribute

use crate::{InvoiceDocument, QuantityUnit, SettlementResult};
use rubo4e::current::{
    Betrag, Menge, Mengeneinheit, NetznutzungRechnungsart, NetznutzungRechnungstyp, Preis,
    Rechnung, Rechnungsposition, Rechnungstyp, Zeitraum, ZusatzAttribut,
};

/// Parse the BDEW Artikelnummer that `grid-billing` decided on.
///
/// The decision — which code applies to which position in which settlement — is
/// domain logic and lives in `grid-billing`. This is only the lookup from its
/// codelist name into the BO4E enum, which `rubo4e` derives via `strum`.
///
/// **Important:** NNE Strom positions (PID 31002, NN-Rechnung) do NOT use classic
/// Artikelnummern since BK6-20-160 — for those, `grid-billing` emits no codelist
/// name and the `artikel_id` is populated from the `PreisblattNetznutzung` by
/// the rendering service. Source: BDEW Codeliste Artikelnummern und Artikel-ID
/// v5.6 (valid 01.09.2025).
#[must_use]
pub fn kind_to_artikelnummer(
    kind: crate::BillingPositionKind,
    settlement_type: crate::SettlementType,
) -> Option<rubo4e::current::BdewArtikelnummer> {
    use std::str::FromStr as _;
    kind.artikelnummer(settlement_type)
        .and_then(|name| rubo4e::current::BdewArtikelnummer::from_str(name).ok())
}

/// The BO4E `rechnungstyp` for a settlement, when it is a Netznutzungsrechnung.
///
/// Not every settlement `grid-billing` produces is one. Grid usage (NNE) and
/// Mehr-/Mindermengen are; the rest are separate commercial documents that
/// happen to share this engine:
///
/// - **`MsbRechnung`** (31009) bills Messstellenbetrieb, not network use.
/// - **`GasAwhSperrung`** (31011) is explicitly a *Rechnung sonstige Leistung*.
/// - **`RedispatchKostenblatt`** is a §13a cost sheet toward the ÜNB.
/// - **`DezentraleEinspeisung`** (§18 StromNEV) is a bilateral credit with no
///   Prüfidentifikator at all.
///
/// Typing those as Netznutzungsrechnung would assert something the AHB does not,
/// so they are left untyped rather than approximated.
#[must_use]
pub fn rechnungstyp_for(settlement_type: crate::SettlementType) -> Option<Rechnungstyp> {
    use crate::SettlementType as S;
    matches!(
        settlement_type,
        S::NneStrom | S::NneGas | S::MmmStrom | S::MmmGas | S::MmmSelbstausstellt
    )
    .then_some(Rechnungstyp::Netznutzungsrechnung)
}

/// The BO4E `netznutzungrechnungsart` — who issued the invoice.
///
/// PID 31006 is the Mehrmenge leg issued by the receiving party itself, which is
/// exactly what *Selbstausgestellt* denotes. Everything else in this family is a
/// conventional Handelsrechnung from the network operator.
#[must_use]
pub fn netznutzungrechnungsart_for(
    settlement_type: crate::SettlementType,
) -> Option<NetznutzungRechnungsart> {
    use crate::SettlementType as S;
    match settlement_type {
        S::MmmSelbstausstellt => Some(NetznutzungRechnungsart::Selbstausgestellt),
        S::NneStrom | S::NneGas | S::MmmStrom | S::MmmGas => {
            Some(NetznutzungRechnungsart::Handelsrechnung)
        }
        _ => None,
    }
}

/// The BO4E `netznutzungrechnungstyp` — which kind of Netznutzungsrechnung.
///
/// Only the Mehr-/Mindermengen family is derivable from the settlement type. The
/// remaining variants (Turnus-, Monats-, Abschlags-, Abschluss-, Zwischen-,
/// WiM-Rechnung) describe the **billing cadence**, which `SettlementType` does
/// not carry: an NNE settlement is the same computation whether it is billed
/// monthly or annually. Guessing one would put a specific claim about billing
/// rhythm on the wire that nothing in the settlement supports, so NNE is left
/// unset until the cadence is modelled.
#[must_use]
pub fn netznutzungrechnungstyp_for(
    settlement_type: crate::SettlementType,
) -> Option<NetznutzungRechnungstyp> {
    use crate::SettlementType as S;
    matches!(
        settlement_type,
        S::MmmStrom | S::MmmGas | S::MmmSelbstausstellt
    )
    .then_some(NetznutzungRechnungstyp::Mehrmindermengenrechnung)
}

/// Render a settlement, presented as an invoice, into a BO4E `Rechnung`.
///
/// Takes the document rather than the settlement: `rechnungsnummer`,
/// `rechnungsdatum` and `faelligkeitsdatum` are document facts, and the position
/// numbering is assigned here rather than carried through the calculation.
#[must_use]
pub fn into_rechnung(document: &InvoiceDocument) -> Rechnung {
    let invoice = &document.settlement;

    // Typed builders (rubo4e `builder` feature): omitted fields default to `None`,
    // and `setter(into)` accepts the value directly.
    let lz = Zeitraum::builder()
        .startdatum(invoice.period.from())
        .enddatum(invoice.period.to())
        .build();

    let positions: Vec<Rechnungsposition> = document
        .numbered_positions()
        .map(|(number, p)| {
            let einheit = match p.unit {
                QuantityUnit::Kwh => Mengeneinheit::Kwh,
                QuantityUnit::Kw => Mengeneinheit::Kw,
                // Reactive energy/power keep their own units — BO4E v202607
                // `Mengeneinheit` models them directly (KVARH/KVAR), so we no
                // longer collapse them into the kWh/kW buckets and lose fidelity.
                QuantityUnit::Kvarh => Mengeneinheit::Kvarh,
                QuantityUnit::Kvar => Mengeneinheit::Kvar,
                QuantityUnit::Monat => Mengeneinheit::Monat,
            };
            Rechnungsposition::builder()
                .positionsnummer(i64::from(number))
                .positionstext(p.text.clone())
                .artikelnummer(kind_to_artikelnummer(p.kind, invoice.settlement_type))
                // Artikel-ID (omitted) is resolved from the price sheet at
                // rendering time; the settlement states what was charged, not
                // how it is coded.
                .lieferungszeitraum(lz.clone())
                .positions_menge(Menge::builder().wert(p.quantity).einheit(einheit).build())
                .einzelpreis(Preis::builder().wert(p.unit_price_eur.round_dp(6)).build())
                .gesamtpreis(Betrag::builder().wert(p.net_eur.round_dp(5)).build())
                // The calculation trace travels with the position it explains.
                // grid-billing computes why each amount is what it is — the
                // inputs, the applied paragraphs, the tariff source — and that
                // is the only record of it: the engine's output is dropped once
                // this Rechnung is stored. §20 EnWG audits and LF disputes are
                // answered from here.
                .zusatz_attribute(trace_attribute(p))
                .build()
        })
        .collect();

    let settlement_type = invoice.settlement_type;
    let mut rechnung = Rechnung::builder()
        .rechnungsnummer(document.rechnungsnummer.clone())
        .rechnungsdatum(document.invoice_date)
        .faelligkeitsdatum(document.due_date)
        .rechnungsperiode(lz)
        .gesamtnetto(Betrag::builder().wert(invoice.total_eur).build())
        .rechnungspositionen(positions)
        // Every paragraph the settlement rests on, deduplicated across
        // positions, plus any warnings the engine raised.
        .zusatz_attribute(settlement_attributes(invoice))
        .build();

    // Typed after the builder rather than through it: each of these is `None`
    // for a settlement that is not a Netznutzungsrechnung, and the builder's
    // `setter(into)` would coerce an `Option` into `Some(None)`-shaped noise.
    rechnung.rechnungstyp = rechnungstyp_for(settlement_type);
    rechnung.netznutzungrechnungsart = netznutzungrechnungsart_for(settlement_type);
    rechnung.netznutzungrechnungstyp = netznutzungrechnungstyp_for(settlement_type);
    rechnung
}

/// Serialise a position's [`crate::CalculationTrace`] into a BO4E
/// `ZusatzAttribut`.
///
/// BO4E has no field for a calculation trace, and inventing one would break the
/// schema — a `ZusatzAttribut` is the sanctioned place for data a standard does
/// not model. Returns `None` when serialisation fails, so the position is still
/// emitted without its trace rather than dropped.
fn trace_attribute(p: &crate::SettlementPosition) -> Option<Vec<ZusatzAttribut>> {
    let trace = serde_json::to_value(&p.trace).ok()?;
    Some(vec![ZusatzAttribut {
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
fn settlement_attributes(invoice: &SettlementResult) -> Vec<ZusatzAttribut> {
    let mut attrs = vec![ZusatzAttribut {
        name: Some("mako:legal_references".to_owned()),
        wert: Some(serde_json::json!(invoice.all_legal_refs())),
        ..Default::default()
    }];
    if !invoice.warnings.is_empty()
        && let Ok(warnings) = serde_json::to_value(&invoice.warnings)
    {
        attrs.push(ZusatzAttribut {
            name: Some("mako:settlement_warnings".to_owned()),
            wert: Some(warnings),
            ..Default::default()
        });
    }
    attrs
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Present a settlement as a document, so the adapter can render it.
    fn as_document(settlement: crate::SettlementResult) -> crate::InvoiceDocument {
        crate::InvoiceDocument {
            settlement,
            pid: 31002,
            rechnungsnummer: "NNE-2026-001".to_owned(),
            correction_of: None,
            invoice_date: time::macros::date!(2026 - 02 - 15),
            due_date: time::macros::date!(2026 - 03 - 15),
        }
    }

    fn sample_nne() -> crate::NneInput {
        crate::NneInput {
            blindarbeit: None,
            malo_id: "51238696012".to_owned(),
            nb_mp_id: "9900357000004".to_owned(),
            lf_mp_id: "9900012345678".to_owned(),
            period: crate::SettlementPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .expect("valid period"),
            arbeitspreis: crate::ArbeitspreisModell::Einheitlich(crate::MengePreis {
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
            sparte: crate::Sparte::Strom,
        }
    }

    /// The calculation trace must survive into the rendered Rechnung.
    ///
    /// grid-billing computes, per position, the inputs it used, the paragraphs
    /// it applied and where the rate came from. That is the only record of it —
    /// a §20 EnWG audit or an LF dispute is answered from here.
    #[test]
    fn the_calculation_trace_reaches_the_rechnung() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let rechnung = into_rechnung(&as_document(settlement));

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

        assert!(trace.get("explanation").is_some(), "{trace}");
        assert!(trace.get("legal_refs").is_some(), "{trace}");
        assert!(trace.get("input_quantity").is_some(), "{trace}");
        assert!(trace.get("gross_eur").is_some(), "{trace}");
    }

    /// The settlement's citations survive too, deduplicated.
    #[test]
    fn the_legal_references_reach_the_rechnung() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        let rechnung = into_rechnung(&as_document(settlement));

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

    /// The two behaviours that drifted apart in the per-service copies must
    /// both hold: the document's `rechnungsnummer` is carried (invoicd had it,
    /// netzbilanzd dropped it) AND the settlement warnings are emitted
    /// (netzbilanzd had them, invoicd dropped them).
    #[test]
    fn rechnungsnummer_and_warnings_are_both_present() {
        let mut settlement = crate::settle_nne(&sample_nne()).expect("settle");
        settlement.warnings.push(crate::SettlementWarning {
            severity: crate::WarningSeverity::Warning,
            code: "TEST_WARNING",
            message: "levy omitted for want of a published rate".to_owned(),
        });
        let rechnung = into_rechnung(&as_document(settlement));

        assert_eq!(
            rechnung.rechnungsnummer.as_deref(),
            Some("NNE-2026-001"),
            "the document's rechnungsnummer must reach the Rechnung"
        );

        let warnings = rechnung
            .zusatz_attribute
            .as_ref()
            .expect("settlement attributes")
            .iter()
            .find(|a| a.name.as_deref() == Some("mako:settlement_warnings"))
            .and_then(|a| a.wert.as_ref())
            .expect("mako:settlement_warnings present");
        let list = warnings.as_array().expect("an array of warnings");
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0].get("code").and_then(|c| c.as_str()),
            Some("TEST_WARNING")
        );
    }

    /// A settlement without warnings emits no empty warnings attribute.
    #[test]
    fn no_warnings_attribute_when_clean() {
        let settlement = crate::settle_nne(&sample_nne()).expect("settle");
        assert!(settlement.warnings.is_empty(), "fixture must be clean");
        let rechnung = into_rechnung(&as_document(settlement));
        let has_warnings = rechnung
            .zusatz_attribute
            .as_ref()
            .expect("settlement attributes")
            .iter()
            .any(|a| a.name.as_deref() == Some("mako:settlement_warnings"));
        assert!(!has_warnings);
    }
}

#[cfg(test)]
mod artikelnummer_bridge_tests {
    use crate::{BillingPositionKind as K, SettlementType as ST};

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

#[cfg(test)]
mod rechnungstyp_tests {
    use super::*;
    use crate::SettlementType as S;

    #[test]
    fn grid_usage_and_mmm_are_netznutzungsrechnungen() {
        for st in [
            S::NneStrom,
            S::NneGas,
            S::MmmStrom,
            S::MmmGas,
            S::MmmSelbstausstellt,
        ] {
            assert_eq!(
                rechnungstyp_for(st),
                Some(Rechnungstyp::Netznutzungsrechnung),
                "{st:?} bills network use and must be typed as such"
            );
        }
    }

    #[test]
    fn non_grid_settlements_are_left_untyped() {
        // Typing these as Netznutzungsrechnung would assert something the AHB
        // does not. `None` is the honest answer, not a gap to fill later.
        for st in [
            S::MsbRechnung,
            S::GasAwhSperrung,
            S::RedispatchKostenblatt,
            S::DezentraleEinspeisung,
        ] {
            assert_eq!(
                rechnungstyp_for(st),
                None,
                "{st:?} is not a Netznutzungsrechnung"
            );
            assert_eq!(netznutzungrechnungsart_for(st), None);
            assert_eq!(netznutzungrechnungstyp_for(st), None);
        }
    }

    #[test]
    fn only_pid_31006_is_self_issued() {
        assert_eq!(
            netznutzungrechnungsart_for(S::MmmSelbstausstellt),
            Some(NetznutzungRechnungsart::Selbstausgestellt)
        );
        for st in [S::NneStrom, S::NneGas, S::MmmStrom, S::MmmGas] {
            assert_eq!(
                netznutzungrechnungsart_for(st),
                Some(NetznutzungRechnungsart::Handelsrechnung),
                "{st:?} is issued by the network operator"
            );
        }
    }

    #[test]
    fn the_cadence_field_is_only_set_where_it_is_known() {
        // Mehr-/Mindermengen has a dedicated code, so it can be stated.
        for st in [S::MmmStrom, S::MmmGas, S::MmmSelbstausstellt] {
            assert_eq!(
                netznutzungrechnungstyp_for(st),
                Some(NetznutzungRechnungstyp::Mehrmindermengenrechnung)
            );
        }
        // NNE has none: Turnus/Monats/Abschlag describe billing rhythm, which a
        // settlement type does not carry. Emitting a guess would put an
        // unsupported claim about billing cadence on the wire.
        for st in [S::NneStrom, S::NneGas] {
            assert_eq!(
                netznutzungrechnungstyp_for(st),
                None,
                "{st:?} has no cadence to state"
            );
        }
    }
}
