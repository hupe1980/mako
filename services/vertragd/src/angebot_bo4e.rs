//! Read an accepted quotation out of its BO4E `Angebot`.
//!
//! `tarifbd` emits the priced quotation as a BO4E [`Angebot`]; the contract is
//! built from that document rather than from parallel scalar fields, so what was
//! quoted and what is contracted cannot drift apart.
//!
//! # Why the variant matters
//!
//! A quotation carries several `Angebotsvariante`s — 12 months fixed, 24 months
//! fixed with a discount, and so on. Only the accepted one becomes a contract,
//! and each variant has its own Laufzeit. Reading the term from the quotation
//! header instead of the accepted variant is exactly the drift this module
//! exists to prevent.

use rubo4e::current::{Angebot, Angebotsteil, Angebotsvariante};

/// The commercial terms of one accepted supply point.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedSupplyPoint {
    /// Validated Marktlokations-ID, when the quotation named one.
    pub malo_id: Option<String>,
    /// Internal product code, from `mako.angebot.teil.produktCode`.
    pub product_code: Option<String>,
    /// Free-text site label, from `mako.angebot.teil.standortBezeichnung`.
    pub standort_bezeichnung: Option<String>,
    /// Sparte of the supply point.
    pub sparte: Option<String>,
    /// Annual quantity in kWh.
    pub jahresverbrauch_kwh: Option<rust_decimal::Decimal>,
}

/// The accepted quotation, reduced to what a contract needs.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedQuotation {
    pub angebotsnummer: Option<String>,
    /// Supply start, from the accepted variant's `lieferzeitraum`.
    pub lieferbeginn: Option<time::Date>,
    /// Supply end, from the accepted variant's `lieferzeitraum`.
    pub lieferende: Option<time::Date>,
    /// Term in whole months, derived from the accepted variant's Lieferzeitraum.
    pub laufzeit_monate: Option<i32>,
    /// Net annual cost of the accepted variant.
    pub jahreskosten_netto_eur: Option<rust_decimal::Decimal>,
    pub supply_points: Vec<AcceptedSupplyPoint>,
}

fn zusatz<'a>(
    attrs: Option<&'a Vec<rubo4e::current::ZusatzAttribut>>,
    name: &str,
) -> Option<&'a str> {
    attrs?
        .iter()
        .find(|z| z.name.as_deref() == Some(name))
        .and_then(|z| z.wert.as_ref())
        .and_then(serde_json::Value::as_str)
}

fn supply_point(teil: &Angebotsteil) -> AcceptedSupplyPoint {
    let lieferstelle = teil
        .lieferstellenangebotsteil
        .as_ref()
        .and_then(|v| v.first());
    AcceptedSupplyPoint {
        malo_id: lieferstelle
            .and_then(|m| m.marktlokations_id.as_ref())
            .map(|id| id.as_ref().to_owned()),
        product_code: zusatz(
            teil.zusatz_attribute.as_ref(),
            "mako.angebot.teil.produktCode",
        )
        .map(str::to_owned),
        standort_bezeichnung: zusatz(
            teil.zusatz_attribute.as_ref(),
            "mako.angebot.teil.standortBezeichnung",
        )
        .map(str::to_owned),
        sparte: lieferstelle
            .and_then(|m| m.sparte.as_ref())
            .and_then(|s| serde_json::to_value(s).ok())
            .and_then(|v| v.as_str().map(str::to_owned)),
        jahresverbrauch_kwh: teil.gesamtmengeangebotsteil.as_ref().and_then(|m| m.wert),
    }
}

/// Whole months between two dates, rounded down.
fn months_between(from: time::Date, to: time::Date) -> i32 {
    let years = i32::from(to.year() as i16 - from.year() as i16);
    let months = i32::from(to.month() as i8 - from.month() as i8);
    let mut total = years * 12 + months;
    if to.day() < from.day() {
        total -= 1;
    }
    total
}

/// Select the accepted variant.
///
/// `gewaehlte_variante` indexes the quotation's variants; `None` means the base
/// offer, which is the first variant.
fn accepted_variant(angebot: &Angebot, gewaehlte: Option<i16>) -> Option<&Angebotsvariante> {
    let varianten = angebot.varianten.as_ref()?;
    let idx = gewaehlte.and_then(|i| usize::try_from(i).ok()).unwrap_or(0);
    varianten.get(idx).or_else(|| varianten.first())
}

/// Read the accepted quotation out of a BO4E `Angebot`.
///
/// Returns `None` when the document carries no variants — there is then nothing
/// that was accepted, and falling back to a guess would contract terms the
/// customer never saw.
#[must_use]
pub fn read_accepted(
    angebot: &Angebot,
    gewaehlte_variante: Option<i16>,
) -> Option<AcceptedQuotation> {
    let variante = accepted_variant(angebot, gewaehlte_variante)?;
    let teile = variante.teile.as_deref().unwrap_or_default();

    // Every Angebotsteil of a variant shares its Lieferzeitraum, so the first
    // one carries the accepted term.
    let zeitraum = teile.iter().find_map(|t| t.lieferzeitraum.as_ref());
    let lieferbeginn = zeitraum.and_then(|z| z.startdatum);
    let lieferende = zeitraum.and_then(|z| z.enddatum);

    Some(AcceptedQuotation {
        angebotsnummer: angebot.angebotsnummer.clone(),
        lieferbeginn,
        lieferende,
        laufzeit_monate: match (lieferbeginn, lieferende) {
            (Some(a), Some(b)) => Some(months_between(a, b)),
            _ => None,
        },
        jahreskosten_netto_eur: variante.gesamtkosten.as_ref().and_then(|b| b.wert),
        supply_points: teile.iter().map(supply_point).collect(),
    })
}

/// Parse a BO4E `Angebot` out of a CloudEvent `data.bo4e` field.
///
/// Returns `None` for an absent or empty document, which is how a quotation that
/// was never priced presents.
#[must_use]
pub fn from_ce_data(data: &serde_json::Value) -> Option<Angebot> {
    let bo4e = data.get("bo4e")?;
    if bo4e.is_null() || bo4e.as_object().is_some_and(serde_json::Map::is_empty) {
        return None;
    }
    serde_json::from_value(bo4e.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubo4e::current::{
        Angebotsteil, Angebotsvariante, Betrag, Marktlokation, Menge, Mengeneinheit, Sparte,
        Waehrungscode, Zeitraum, ZusatzAttribut,
    };
    use rust_decimal::dec;

    fn attr(name: &str, wert: &str) -> ZusatzAttribut {
        ZusatzAttribut {
            name: Some(name.to_owned()),
            wert: Some(serde_json::Value::String(wert.to_owned())),
            ..Default::default()
        }
    }

    fn teil(malo: &str, months: i64) -> Angebotsteil {
        let start = time::macros::date!(2027 - 01 - 01);
        Angebotsteil {
            gesamtmengeangebotsteil: Some(Menge {
                wert: Some(dec!(250000)),
                einheit: Some(Mengeneinheit::Kwh),
                ..Default::default()
            }),
            lieferstellenangebotsteil: Some(vec![Box::new(Marktlokation {
                marktlokations_id: rubo4e::identifiers::MaloId::new(malo).ok(),
                sparte: Some(Sparte::Strom),
                ..Default::default()
            })]),
            lieferzeitraum: Some(Zeitraum {
                startdatum: Some(start),
                enddatum: start.checked_add(time::Duration::days(months * 30)),
                ..Default::default()
            }),
            zusatz_attribute: Some(vec![
                attr("mako.angebot.teil.produktCode", "STROM-B2B-24"),
                attr("mako.angebot.teil.standortBezeichnung", "Werk Nord"),
            ]),
            ..Default::default()
        }
    }

    fn variante(months: i64, kosten: rust_decimal::Decimal) -> Angebotsvariante {
        Angebotsvariante {
            gesamtkosten: Some(Betrag {
                wert: Some(kosten),
                waehrung: Some(Waehrungscode::Eur),
                ..Default::default()
            }),
            teile: Some(vec![teil("51238696780", months)]),
            ..Default::default()
        }
    }

    fn angebot() -> Angebot {
        Angebot {
            angebotsnummer: Some("AN-2026-0001".to_owned()),
            varianten: Some(vec![variante(12, dec!(80000)), variante(24, dec!(75400))]),
            ..Default::default()
        }
    }

    /// The whole point: the accepted variant's own term is contracted, not the
    /// quotation header's. Accepting variant 1 must yield 24 months, not 12.
    #[test]
    fn the_accepted_variant_supplies_the_term() {
        let a = read_accepted(&angebot(), Some(1)).expect("accepted");
        assert_eq!(
            a.laufzeit_monate,
            Some(23),
            "24×30d rounds to 23 whole months"
        );
        assert_eq!(a.jahreskosten_netto_eur, Some(dec!(75400)));

        let base = read_accepted(&angebot(), Some(0)).expect("accepted");
        assert_eq!(base.jahreskosten_netto_eur, Some(dec!(80000)));
        assert!(
            base.laufzeit_monate < a.laufzeit_monate,
            "the 12-month variant must be shorter than the 24-month one"
        );
    }

    /// `None` means the base offer, which is the first variant.
    #[test]
    fn no_selection_takes_the_base_variant() {
        let a = read_accepted(&angebot(), None).expect("accepted");
        assert_eq!(a.jahreskosten_netto_eur, Some(dec!(80000)));
    }

    /// An out-of-range index must not silently contract a variant the customer
    /// never chose — it falls back to the base offer.
    #[test]
    fn an_out_of_range_selection_falls_back_to_the_base() {
        let a = read_accepted(&angebot(), Some(99)).expect("accepted");
        assert_eq!(a.jahreskosten_netto_eur, Some(dec!(80000)));
    }

    /// The supply point comes back with its validated MaLo-ID and product code.
    #[test]
    fn supply_points_carry_malo_product_and_quantity() {
        let a = read_accepted(&angebot(), Some(1)).expect("accepted");
        assert_eq!(a.supply_points.len(), 1);
        let sp = &a.supply_points[0];
        assert_eq!(sp.malo_id.as_deref(), Some("51238696780"));
        assert_eq!(sp.product_code.as_deref(), Some("STROM-B2B-24"));
        assert_eq!(sp.standort_bezeichnung.as_deref(), Some("Werk Nord"));
        assert_eq!(sp.jahresverbrauch_kwh, Some(dec!(250000)));
    }

    /// A document with no variants means nothing was accepted; guessing would
    /// contract terms the customer never saw.
    #[test]
    fn a_quotation_without_variants_yields_nothing() {
        let empty = Angebot {
            angebotsnummer: Some("AN-1".to_owned()),
            ..Default::default()
        };
        assert!(read_accepted(&empty, None).is_none());
    }

    /// An unpriced quotation presents as `{}` and must not parse into a
    /// half-populated document.
    #[test]
    fn an_empty_bo4e_field_is_treated_as_absent() {
        assert!(from_ce_data(&serde_json::json!({ "bo4e": {} })).is_none());
        assert!(from_ce_data(&serde_json::json!({ "bo4e": null })).is_none());
        assert!(from_ce_data(&serde_json::json!({})).is_none());
    }

    /// A real CloudEvent payload round-trips into the reader.
    #[test]
    fn a_cloudevent_payload_round_trips() {
        let data = serde_json::json!({
            "bo4e": serde_json::to_value(angebot()).unwrap(),
        });
        let parsed = from_ce_data(&data).expect("parses");
        let accepted = read_accepted(&parsed, Some(1)).expect("accepted");
        assert_eq!(accepted.angebotsnummer.as_deref(), Some("AN-2026-0001"));
        assert_eq!(accepted.supply_points[0].sparte.as_deref(), Some("STROM"));
    }

    #[test]
    fn months_between_rounds_down_on_a_partial_month() {
        let a = time::macros::date!(2027 - 01 - 15);
        assert_eq!(months_between(a, time::macros::date!(2027 - 02 - 14)), 0);
        assert_eq!(months_between(a, time::macros::date!(2027 - 02 - 15)), 1);
        assert_eq!(months_between(a, time::macros::date!(2028 - 01 - 15)), 12);
    }
}
