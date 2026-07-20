//! The quotation must project into a real BO4E `Angebot`, not an ad-hoc shape.
//!
//! A B2B quotation is the natural CPQ/ERP interchange payload, so the test that
//! matters is whether a receiving system can read it back as
//! `rubo4e::current::Angebot` and find the supply point and cost lines.

use rubo4e::current::{Angebot, Angebotsstatus, Mengeneinheit, Waehrungscode};
use rust_decimal_macros::dec;
use tarifbd::bo4e_angebot::{
    ATTR_IST_BASIS, ATTR_LABEL, ATTR_PRODUCT_CODE, ATTR_RABATT_PCT, build_angebot, status_from_str,
};
use tarifbd::handlers::{PositionCostBreakdown, ScenarioCostBreakdown};

fn position(malo: Option<&str>) -> PositionCostBreakdown {
    PositionCostBreakdown {
        product_code: "STROM-B2B-12".to_owned(),
        sparte: "STROM".to_owned(),
        malo_id: malo.map(str::to_owned),
        standort_bezeichnung: Some("Werk Nord".to_owned()),
        jahresverbrauch_kwh: dec!(250000),
        supply_netto_eur: dec!(50000),
        nne_netto_eur: dec!(20000),
        ka_eur: dec!(275),
        levies_eur: dec!(5125),
        total_netto_eur: dec!(75400),
        total_brutto_eur: dec!(89726),
        arbeitspreis_ct_per_kwh: Some(dec!(19.9)),
        grundpreis_eur_per_year: Some(dec!(240)),
    }
}

fn scenario(
    label: &str,
    ist_basis: bool,
    rabatt: Option<rust_decimal::Decimal>,
) -> ScenarioCostBreakdown {
    ScenarioCostBreakdown {
        label: label.to_owned(),
        laufzeit_monate: 24,
        ist_basis,
        variante_index: if ist_basis { None } else { Some(0) },
        rabatt_pct: rabatt,
        jahreskosten_netto_eur: dec!(75400),
        jahreskosten_brutto_eur: dec!(89726),
        ersparnis_vs_basis_eur: None,
        positionen_detail: vec![position(Some("51238696780"))],
    }
}

fn build() -> Angebot {
    build_angebot(
        "AN-2026-0001",
        "VERSANDT",
        time::macros::date!(2026 - 09 - 30),
        Some(time::macros::date!(2027 - 01 - 01)),
        Some("STROM"),
        &[scenario("Basis (24 Monate)", true, None)],
    )
}

/// The whole point: a receiving ERP must be able to deserialize it as BO4E.
#[test]
fn the_quotation_round_trips_as_a_bo4e_angebot() {
    let json = serde_json::to_value(build()).expect("serialises");
    let back: Angebot = serde_json::from_value(json).expect("must round-trip as BO4E Angebot");
    assert_eq!(back.angebotsnummer.as_deref(), Some("AN-2026-0001"));
    assert_eq!(back.sparte, Some(rubo4e::current::Sparte::Strom));
}

/// `gueltig_bis` is BO4E's `bindefrist` — the offer's binding period, which is
/// exactly what the internal field means.
#[test]
fn gueltig_bis_maps_onto_bindefrist() {
    let a = build();
    let bf = a.bindefrist.expect("Bindefrist set");
    assert_eq!(bf.date(), time::macros::date!(2026 - 09 - 30));
}

/// The supply point must land in `lieferstellenangebotsteil` as a real
/// Marktlokation — that is how a receiving system keys the costs.
#[test]
fn the_supply_point_is_a_typed_marktlokation() {
    let a = build();
    let teil = &a.varianten.as_ref().unwrap()[0].teile.as_ref().unwrap()[0];
    let malo = &teil
        .lieferstellenangebotsteil
        .as_ref()
        .expect("Lieferstelle")[0];
    assert_eq!(
        malo.marktlokations_id.as_ref().map(AsRef::as_ref),
        Some("51238696780")
    );
    assert_eq!(malo.sparte, Some(rubo4e::current::Sparte::Strom));
}

/// An invalid MaLo-ID must not produce a Marktlokation carrying a bad key.
#[test]
fn an_invalid_malo_id_yields_no_lieferstelle() {
    let mut s = scenario("Basis", true, None);
    s.positionen_detail = vec![position(Some("51238696781"))]; // wrong check digit
    let a = build_angebot(
        "AN-1",
        "ANGELEGT",
        time::macros::date!(2026 - 09 - 30),
        None,
        Some("STROM"),
        &[s],
    );
    let teil = &a.varianten.as_ref().unwrap()[0].teile.as_ref().unwrap()[0];
    assert!(
        teil.lieferstellenangebotsteil.is_none(),
        "a failed check digit must not become a Marktlokation"
    );
}

/// Cost lines become `Angebotsposition`s with typed Betrag/Menge, so the four
/// buckets are separable rather than a single opaque total.
#[test]
fn cost_buckets_become_separate_positions() {
    let a = build();
    let teil = &a.varianten.as_ref().unwrap()[0].teile.as_ref().unwrap()[0];
    let positionen = teil.positionen.as_ref().expect("positions");

    let labels: Vec<&str> = positionen
        .iter()
        .filter_map(|p| p.positionsbezeichnung.as_deref())
        .collect();
    for want in [
        "Energiepreis (Arbeits-, Grund- und Leistungspreis)",
        "Netznutzungsentgelt",
        "Konzessionsabgabe (KAV §2)",
        "Steuern und Umlagen",
        "Arbeitspreis",
        "Grundpreis",
    ] {
        assert!(
            labels.contains(&want),
            "missing position `{want}`: {labels:?}"
        );
    }

    let nne = positionen
        .iter()
        .find(|p| p.positionsbezeichnung.as_deref() == Some("Netznutzungsentgelt"))
        .unwrap();
    let betrag = nne.positionskosten.as_ref().unwrap();
    assert_eq!(betrag.wert, Some(dec!(20000)));
    assert_eq!(betrag.waehrung, Some(Waehrungscode::Eur));
    assert_eq!(
        nne.positionsmenge.as_ref().unwrap().einheit,
        Some(Mengeneinheit::Kwh)
    );
}

/// A zero cost line is omitted: BO4E cannot express "does not apply", and a
/// receiving ERP cannot tell an exemption from an unpriced position.
#[test]
fn zero_cost_lines_are_omitted() {
    let mut s = scenario("Basis", true, None);
    s.positionen_detail[0].ka_eur = rust_decimal::Decimal::ZERO;
    let a = build_angebot(
        "AN-1",
        "ANGELEGT",
        time::macros::date!(2026 - 09 - 30),
        None,
        Some("STROM"),
        &[s],
    );
    let teil = &a.varianten.as_ref().unwrap()[0].teile.as_ref().unwrap()[0];
    let labels: Vec<&str> = teil
        .positionen
        .as_ref()
        .unwrap()
        .iter()
        .filter_map(|p| p.positionsbezeichnung.as_deref())
        .collect();
    assert!(
        !labels.contains(&"Konzessionsabgabe (KAV §2)"),
        "a zero Konzessionsabgabe must not be emitted as 0.00"
    );
}

/// Fields BO4E has no home for go into `zusatz_attribute`, its sanctioned
/// extension point — not into a parallel private blob.
#[test]
fn label_discount_and_product_code_ride_in_zusatz_attribute() {
    let a = build_angebot(
        "AN-1",
        "VERSANDT",
        time::macros::date!(2026 - 09 - 30),
        None,
        Some("STROM"),
        &[scenario("24 Monate −5 %", false, Some(dec!(5.0)))],
    );
    let var = &a.varianten.as_ref().unwrap()[0];
    let names: Vec<&str> = var
        .zusatz_attribute
        .as_ref()
        .unwrap()
        .iter()
        .filter_map(|z| z.name.as_deref())
        .collect();
    assert!(names.contains(&ATTR_LABEL));
    assert!(names.contains(&ATTR_RABATT_PCT));
    assert!(names.contains(&ATTR_IST_BASIS));

    let teil = &var.teile.as_ref().unwrap()[0];
    let teil_names: Vec<&str> = teil
        .zusatz_attribute
        .as_ref()
        .unwrap()
        .iter()
        .filter_map(|z| z.name.as_deref())
        .collect();
    assert!(teil_names.contains(&ATTR_PRODUCT_CODE));
}

/// A quotation that has only been drafted is not yet an offer to the
/// counterparty, so it is Konzeption rather than Unverbindlich.
#[test]
fn lifecycle_status_maps_onto_angebotsstatus() {
    assert_eq!(status_from_str("ANGELEGT"), Angebotsstatus::Konzeption);
    assert_eq!(status_from_str("VERSANDT"), Angebotsstatus::Verbindlich);
    assert_eq!(status_from_str("ANGENOMMEN"), Angebotsstatus::Beauftragt);
    assert_eq!(status_from_str("ABGELEHNT"), Angebotsstatus::Abgelehnt);
    assert_eq!(status_from_str("ABGELAUFEN"), Angebotsstatus::Ungueltig);
}

/// Each scenario carries its own Laufzeit — comparing a 12- and a 24-month
/// variant is the entire point of the comparison endpoint.
#[test]
fn each_variant_carries_its_own_delivery_period() {
    let mut short = scenario("12 Monate", false, None);
    short.laufzeit_monate = 12;
    let a = build_angebot(
        "AN-1",
        "VERSANDT",
        time::macros::date!(2026 - 09 - 30),
        Some(time::macros::date!(2027 - 01 - 01)),
        Some("STROM"),
        &[scenario("24 Monate", true, None), short],
    );
    let varianten = a.varianten.as_ref().unwrap();
    let end = |i: usize| {
        varianten[i].teile.as_ref().unwrap()[0]
            .lieferzeitraum
            .as_ref()
            .unwrap()
            .enddatum
            .unwrap()
    };
    assert!(
        end(0) > end(1),
        "the 24-month variant must end after the 12-month one"
    );
}
