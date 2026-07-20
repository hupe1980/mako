//! Projection of a priced quotation into the BO4E `Angebot` business object.
//!
//! `tarifbd` emits typed BO4E for its tariff data (`Tarifinfo`,
//! `Tarifpreisblatt`); a B2B quotation is the natural CPQ/ERP interchange
//! payload, so it is emitted the same way rather than as an ad-hoc JSON shape.
//!
//! # Structure
//!
//! BO4E nests one level deeper than the flat internal breakdown, and the extra
//! level carries real meaning:
//!
//! ```text
//! Angebot                     one quotation
//! └── Angebotsvariante        one pricing scenario (12M fix, 24M fix −5 %, …)
//!     └── Angebotsteil        one supply point — carries the Marktlokation
//!         └── Angebotsposition   one cost line (Arbeitspreis, NNE, KA, Steuern)
//! ```
//!
//! The internal `PositionCostBreakdown` conflates the supply point with its
//! cost lines. Splitting them is what makes the payload interchangeable: a
//! receiving ERP reads `lieferstellenangebotsteil` to find the Marktlokation and
//! `positionen` to find what was charged against it.
//!
//! # Fields BO4E has no home for
//!
//! `Angebotsvariante` has no discount or label field, and `Angebotsteil` has no
//! product code. These go into `zusatz_attribute`, which is BO4E's sanctioned
//! extension point — not into a parallel private JSON blob.

use rubo4e::current::{
    Angebot, Angebotsposition, Angebotsstatus, Angebotsteil, Angebotsvariante, Betrag,
    Marktlokation, Menge, Mengeneinheit, Preis, Sparte, Waehrungscode, Waehrungseinheit, Zeitraum,
    ZusatzAttribut,
};
use rust_decimal::Decimal;

use crate::handlers::{PositionCostBreakdown, ScenarioCostBreakdown};

/// `zusatz_attribut` name for the scenario label.
pub const ATTR_LABEL: &str = "mako.angebot.variante.label";
/// `zusatz_attribut` name for the percentage discount applied to the Arbeitspreis.
pub const ATTR_RABATT_PCT: &str = "mako.angebot.variante.rabattProzent";
/// `zusatz_attribut` name marking the base scenario.
pub const ATTR_IST_BASIS: &str = "mako.angebot.variante.istBasis";
/// `zusatz_attribut` name for the internal product code of a supply point.
pub const ATTR_PRODUCT_CODE: &str = "mako.angebot.teil.produktCode";
/// `zusatz_attribut` name for the free-text site label.
pub const ATTR_STANDORT: &str = "mako.angebot.teil.standortBezeichnung";

fn attr(name: &str, value: impl Into<serde_json::Value>) -> ZusatzAttribut {
    ZusatzAttribut {
        name: Some(name.to_owned()),
        wert: Some(value.into()),
        ..Default::default()
    }
}

fn eur(wert: Decimal) -> Betrag {
    Betrag {
        wert: Some(wert),
        waehrung: Some(Waehrungscode::Eur),
        ..Default::default()
    }
}

fn menge(wert: Decimal, einheit: Mengeneinheit) -> Menge {
    Menge {
        wert: Some(wert),
        einheit: Some(einheit),
        ..Default::default()
    }
}

/// A price per unit, e.g. 24.9 ct/kWh.
fn preis(wert: Decimal, einheit: Waehrungseinheit, bezugswert: Decimal) -> Preis {
    Preis {
        wert: Some(wert),
        einheit: Some(einheit),
        bezugswert: Some(bezugswert),
        ..Default::default()
    }
}

fn sparte_from_str(s: &str) -> Option<Sparte> {
    match s.to_uppercase().as_str() {
        "STROM" => Some(Sparte::Strom),
        "GAS" => Some(Sparte::Gas),
        "WAERME" | "WÄRME" => Some(Sparte::Fernwaerme),
        "WASSER" => Some(Sparte::Wasser),
        _ => None,
    }
}

/// One cost line, emitted only when non-zero.
///
/// A zero line is omitted rather than sent as `0.00`: BO4E has no way to say
/// "this levy does not apply here", and a receiving ERP cannot tell an exemption
/// from an unpriced position.
fn position(
    bezeichnung: &str,
    kosten: Decimal,
    menge_kwh: Option<Decimal>,
) -> Option<Angebotsposition> {
    if kosten.is_zero() {
        return None;
    }
    Some(Angebotsposition {
        positionsbezeichnung: Some(bezeichnung.to_owned()),
        positionskosten: Some(eur(kosten)),
        positionsmenge: menge_kwh.map(|k| menge(k, Mengeneinheit::Kwh)),
        ..Default::default()
    })
}

/// Project one priced supply point into an [`Angebotsteil`].
fn teil(pos: &PositionCostBreakdown, lieferzeitraum: Option<&Zeitraum>) -> Angebotsteil {
    let mut positionen = Vec::new();
    positionen.extend(position(
        "Energiepreis (Arbeits-, Grund- und Leistungspreis)",
        pos.supply_netto_eur,
        Some(pos.jahresverbrauch_kwh),
    ));
    positionen.extend(position(
        "Netznutzungsentgelt",
        pos.nne_netto_eur,
        Some(pos.jahresverbrauch_kwh),
    ));
    positionen.extend(position(
        "Konzessionsabgabe (KAV §2)",
        pos.ka_eur,
        Some(pos.jahresverbrauch_kwh),
    ));
    positionen.extend(position(
        "Steuern und Umlagen",
        pos.levies_eur,
        Some(pos.jahresverbrauch_kwh),
    ));

    // The Arbeitspreis is the figure a buyer compares on, so it is carried as a
    // typed `Preis` on its own line rather than folded into the supply total.
    if let Some(ap) = pos.arbeitspreis_ct_per_kwh {
        positionen.push(Angebotsposition {
            positionsbezeichnung: Some("Arbeitspreis".to_owned()),
            positionspreis: Some(preis(ap, Waehrungseinheit::Ct, Decimal::ONE)),
            positionsmenge: Some(menge(pos.jahresverbrauch_kwh, Mengeneinheit::Kwh)),
            ..Default::default()
        });
    }
    if let Some(gp) = pos.grundpreis_eur_per_year {
        positionen.push(Angebotsposition {
            positionsbezeichnung: Some("Grundpreis".to_owned()),
            positionspreis: Some(preis(gp, Waehrungseinheit::Eur, Decimal::ONE)),
            positionsmenge: Some(menge(Decimal::ONE, Mengeneinheit::Jahr)),
            ..Default::default()
        });
    }

    let mut zusatz = vec![attr(ATTR_PRODUCT_CODE, pos.product_code.clone())];
    if let Some(ref s) = pos.standort_bezeichnung {
        zusatz.push(attr(ATTR_STANDORT, s.clone()));
    }

    Angebotsteil {
        gesamtkostenangebotsteil: Some(eur(pos.total_netto_eur)),
        gesamtmengeangebotsteil: Some(menge(pos.jahresverbrauch_kwh, Mengeneinheit::Kwh)),
        lieferstellenangebotsteil: pos.malo_id.as_ref().and_then(|id| {
            rubo4e::identifiers::MaloId::new(id).ok().map(|malo| {
                vec![Box::new(Marktlokation {
                    marktlokations_id: Some(malo),
                    sparte: sparte_from_str(&pos.sparte),
                    ..Default::default()
                })]
            })
        }),
        lieferzeitraum: lieferzeitraum.cloned(),
        positionen: Some(positionen),
        zusatz_attribute: Some(zusatz),
        ..Default::default()
    }
}

/// Project one priced scenario into an [`Angebotsvariante`].
fn variante(
    scenario: &ScenarioCostBreakdown,
    status: Angebotsstatus,
    lieferbeginn: Option<time::Date>,
) -> Angebotsvariante {
    // The scenario's own Laufzeit, not the quotation's: comparing a 12- and a
    // 24-month variant is the point of the comparison.
    let lieferzeitraum = lieferbeginn.map(|start| Zeitraum {
        startdatum: Some(start),
        enddatum: start.checked_add(time::Duration::days(
            i64::from(scenario.laufzeit_monate) * 30,
        )),
        ..Default::default()
    });

    let gesamtmenge: Decimal = scenario
        .positionen_detail
        .iter()
        .map(|p| p.jahresverbrauch_kwh)
        .sum();

    let mut zusatz = vec![
        attr(ATTR_LABEL, scenario.label.clone()),
        attr(ATTR_IST_BASIS, scenario.ist_basis),
    ];
    if let Some(r) = scenario.rabatt_pct {
        zusatz.push(attr(ATTR_RABATT_PCT, r.to_string()));
    }

    Angebotsvariante {
        angebotsstatus: Some(status),
        gesamtkosten: Some(eur(scenario.jahreskosten_netto_eur)),
        gesamtmenge: Some(menge(gesamtmenge, Mengeneinheit::Kwh)),
        teile: Some(
            scenario
                .positionen_detail
                .iter()
                .map(|p| teil(p, lieferzeitraum.as_ref()))
                .collect(),
        ),
        ..Default::default()
    }
    .tap_zusatz(zusatz)
}

/// Small helper so the builder above stays a single expression.
trait TapZusatz {
    fn tap_zusatz(self, z: Vec<ZusatzAttribut>) -> Self;
}

impl TapZusatz for Angebotsvariante {
    fn tap_zusatz(mut self, z: Vec<ZusatzAttribut>) -> Self {
        self.zusatz_attribute = Some(z);
        self
    }
}

/// Map the stored lifecycle status onto BO4E [`Angebotsstatus`].
///
/// `ANGELEGT` is *Konzeption*, not *Unverbindlich*: it has not been sent, so it
/// is not yet an offer to the counterparty at all.
#[must_use]
pub fn status_from_str(status: &str) -> Angebotsstatus {
    match status {
        "ANGELEGT" => Angebotsstatus::Konzeption,
        "VERSANDT" => Angebotsstatus::Verbindlich,
        "ANGENOMMEN" => Angebotsstatus::Beauftragt,
        "ABGELEHNT" => Angebotsstatus::Abgelehnt,
        "ABGELAUFEN" => Angebotsstatus::Ungueltig,
        _ => Angebotsstatus::Unknown,
    }
}

/// Build the BO4E [`Angebot`] for a priced quotation.
///
/// `bindefrist` is the quotation's validity date — BO4E's own term for it, so
/// the internal `gueltig_bis` maps onto it directly.
#[must_use]
pub fn build_angebot(
    angebotsnummer: &str,
    status: &str,
    bindefrist: time::Date,
    lieferbeginn: Option<time::Date>,
    sparte: Option<&str>,
    scenarios: &[ScenarioCostBreakdown],
) -> Angebot {
    let bo_status = status_from_str(status);
    Angebot {
        angebotsnummer: Some(angebotsnummer.to_owned()),
        bindefrist: bindefrist
            .with_hms(23, 59, 59)
            .ok()
            .map(|dt| dt.assume_utc()),
        sparte: sparte.and_then(sparte_from_str),
        varianten: Some(
            scenarios
                .iter()
                .map(|s| variante(s, bo_status, lieferbeginn))
                .collect(),
        ),
        ..Default::default()
    }
}
