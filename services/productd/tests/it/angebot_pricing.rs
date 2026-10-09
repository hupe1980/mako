//! What a quotation may quote, and what it must refuse instead.
//!
//! An Angebot is a binding offer. Every invariant here exists because the
//! alternative is a number the customer signs for and the invoice then
//! contradicts: a tier that does not apply to the quantity, a band that was
//! never priced, a levy from a past year, a gross figure at a rate the product
//! does not carry.
//!
//! These run without a database: the pricing takes the product's stored JSON
//! and the position, and nothing else.

use productd::handlers::{PricingContext, compute_cost_breakdown, grund};
use productd::pg::AngebotPositionInput;
use rust_decimal::{Decimal, dec};

/// A product carrying nothing but the price positions under test.
fn produkt(tarifpreise: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "tarifpreise": tarifpreise })
}

/// One Preisposition of `preistyp` with the given tiers.
fn preisposition(preistyp: &str, staffeln: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "preistyp": preistyp, "preisstaffeln": staffeln })
}

/// One Preisstaffel. `None` bounds are unbounded on that side.
fn staffel(von: Option<i64>, bis: Option<i64>, preis: &str) -> serde_json::Value {
    let mut o = serde_json::Map::new();
    if let Some(v) = von {
        o.insert("staffelgrenzeVon".into(), serde_json::json!(v));
    }
    if let Some(b) = bis {
        o.insert("staffelgrenzeBis".into(), serde_json::json!(b));
    }
    o.insert("preis".into(), serde_json::json!(preis));
    serde_json::Value::Object(o)
}

/// A single untiered price, the shape most products carry.
fn flat(preistyp: &str, preis: &str) -> serde_json::Value {
    preisposition(preistyp, serde_json::json!([staffel(None, None, preis)]))
}

fn position(sparte: &str, jahresverbrauch_kwh: Decimal) -> AngebotPositionInput {
    AngebotPositionInput {
        product_code: "STROM-B2B-12".to_owned(),
        sparte: sparte.to_owned(),
        malo_id: Some("51238696012".to_owned()),
        melo_id: None,
        nb_mp_id: None,
        standort_bezeichnung: None,
        jahresverbrauch_kwh,
        jahresverbrauch_ht_kwh: None,
        jahresverbrauch_nt_kwh: None,
        leistung_kw: None,
        szenario_tag: None,
        nne_arbeitspreis_ct_per_kwh: None,
        nne_grundpreis_eur_per_year: None,
        nne_leistungspreis_eur_per_kw_year: None,
        ka_ct_per_kwh: None,
        // Zero unless a test is about levies, so the supply figure under test
        // is not buried in statutory ones.
        stromsteuer_ct_per_kwh: Some(Decimal::ZERO),
        energiesteuer_gas_ct_per_kwh: None,
        behg_gas_ct_per_kwh: None,
    }
}

/// No CO₂ price and no reverse charge — the default a Strom quotation runs in.
const PLAIN: PricingContext = PricingContext {
    behg_gas_ct_per_kwh: None,
    reverse_charge_13b: false,
};

// ── Preisstaffeln ─────────────────────────────────────────────────────────────

/// The tier that applies to the quantity is the one that prices it.
///
/// Taking the first tier unconditionally quoted 8 000 kWh at the rate of the
/// tier that ends at 2 000 — an over-quote of 640 EUR/a on a binding offer, and
/// one no later invoice would ever reproduce.
#[test]
fn a_tiered_arbeitspreis_is_priced_by_the_quantity() {
    let data = produkt(serde_json::json!([preisposition(
        "ARBEITSPREIS_EINTARIF",
        serde_json::json!([
            staffel(Some(0), Some(2000), "32"),
            staffel(Some(2001), None, "24"),
        ])
    )]));
    let bd = compute_cost_breakdown(&data, &position("STROM", dec!(8000)), None, &PLAIN)
        .expect("the open top tier prices 8 000 kWh");

    assert_eq!(bd.supply_netto_eur, dec!(1920), "8 000 kWh × 24 ct");
    assert_ne!(
        bd.supply_netto_eur,
        dec!(2560),
        "the 0–2 000 tier must not price a quantity it does not cover"
    );
    assert_eq!(bd.arbeitspreis_ct_per_kwh, Some(dec!(24)));
}

/// BO4E states tier bounds as `0 – 1000, 1001 – 2000` and rules that a quantity
/// between two tiers „rutscht in die obere Zone", so it is priced — not left
/// without a tier at all.
#[test]
fn a_quantity_between_two_tiers_takes_the_upper_zone() {
    let data = produkt(serde_json::json!([preisposition(
        "ARBEITSPREIS_EINTARIF",
        serde_json::json!([
            staffel(Some(0), Some(1000), "30"),
            staffel(Some(1001), Some(2000), "25"),
        ])
    )]));
    let bd = compute_cost_breakdown(&data, &position("STROM", dec!(1000.6)), None, &PLAIN)
        .expect("the upper zone prices it");
    assert_eq!(bd.arbeitspreis_ct_per_kwh, Some(dec!(25)));
}

/// A quantity above every stated tier has no price in the product at all, and
/// the quotation says so instead of inventing one.
#[test]
fn a_quantity_above_every_tier_is_refused() {
    let data = produkt(serde_json::json!([preisposition(
        "ARBEITSPREIS_EINTARIF",
        serde_json::json!([staffel(Some(0), Some(2000), "32")])
    )]));
    let err = compute_cost_breakdown(&data, &position("STROM", dec!(8000)), None, &PLAIN)
        .expect_err("no tier covers 8 000 kWh");
    assert_eq!(err.grund, grund::KEINE_PREISSTAFFEL);
}

// ── Zweitarif ─────────────────────────────────────────────────────────────────

/// HT and NT are each priced against their own volume.
#[test]
fn a_zweitarif_product_prices_both_bands() {
    let data = produkt(serde_json::json!([
        flat("ARBEITSPREIS_HT", "30"),
        flat("ARBEITSPREIS_NT", "20"),
    ]));
    let mut pos = position("STROM", dec!(10000));
    pos.jahresverbrauch_ht_kwh = Some(dec!(6000));
    pos.jahresverbrauch_nt_kwh = Some(dec!(4000));

    let bd = compute_cost_breakdown(&data, &pos, None, &PLAIN).expect("both bands price");
    assert_eq!(
        bd.supply_netto_eur,
        dec!(2600),
        "6 000 × 30 ct + 4 000 × 20 ct"
    );
    assert_ne!(
        bd.supply_netto_eur,
        dec!(3000),
        "the whole volume must not be quoted at HT"
    );
    assert_eq!(bd.arbeitspreis_ht_ct_per_kwh, Some(dec!(30)));
    assert_eq!(bd.arbeitspreis_nt_ct_per_kwh, Some(dec!(20)));
    assert_eq!(
        bd.arbeitspreis_ct_per_kwh,
        Some(dec!(26)),
        "the headline rate is the volume-weighted average, not HT"
    );
}

/// Without the split there is no NT volume to price, so the quotation is
/// refused rather than quoted at HT for the whole year.
#[test]
fn a_zweitarif_product_without_the_split_is_refused() {
    let data = produkt(serde_json::json!([
        flat("ARBEITSPREIS_HT", "30"),
        flat("ARBEITSPREIS_NT", "20"),
    ]));
    let err = compute_cost_breakdown(&data, &position("STROM", dec!(10000)), None, &PLAIN)
        .expect_err("no HT/NT split");
    assert_eq!(err.grund, grund::HT_NT_AUFTEILUNG_FEHLT);
}

/// A split that does not add up to the quoted year describes a different
/// consumption from the one being priced.
#[test]
fn an_ht_nt_split_that_does_not_add_up_is_refused() {
    let data = produkt(serde_json::json!([
        flat("ARBEITSPREIS_HT", "30"),
        flat("ARBEITSPREIS_NT", "20"),
    ]));
    let mut pos = position("STROM", dec!(10000));
    pos.jahresverbrauch_ht_kwh = Some(dec!(6000));
    pos.jahresverbrauch_nt_kwh = Some(dec!(3000));
    let err = compute_cost_breakdown(&data, &pos, None, &PLAIN).expect_err("9 000 ≠ 10 000");
    assert_eq!(err.grund, grund::HT_NT_AUFTEILUNG_FEHLT);
}

// ── Leistungspreis ────────────────────────────────────────────────────────────

/// The demand charge dominates an RLM offer; a product that carries one cannot
/// be quoted for a position that states no demand.
#[test]
fn a_leistungspreis_product_without_a_demand_is_refused() {
    let data = produkt(serde_json::json!([
        flat("ARBEITSPREIS_EINTARIF", "12"),
        flat("LEISTUNGSPREIS", "800"),
    ]));
    let err = compute_cost_breakdown(&data, &position("STROM", dec!(500000)), None, &PLAIN)
        .expect_err("no leistung_kw");
    assert_eq!(err.grund, grund::LEISTUNG_FEHLT);
}

/// With a demand it is priced from the tier that covers that demand in kW —
/// not from the tier that covers the consumption in kWh.
#[test]
fn a_leistungspreis_tier_is_selected_by_the_demand() {
    let data = produkt(serde_json::json!([preisposition(
        "LEISTUNGSPREIS",
        serde_json::json!([
            staffel(Some(0), Some(100), "900"),
            staffel(Some(101), None, "700"),
        ])
    )]));
    let mut pos = position("STROM", dec!(500000));
    pos.leistung_kw = Some(dec!(250));
    let bd = compute_cost_breakdown(&data, &pos, None, &PLAIN).expect("the 101+ tier applies");
    assert_eq!(
        bd.supply_netto_eur,
        dec!(21000),
        "700 ct/kW/month × 250 kW × 12"
    );
}

// ── Umsatzsteuer ──────────────────────────────────────────────────────────────

/// The gross figure follows the product's own rate — the same field and scale
/// `billingd` bills from — so the offer and the invoice cannot disagree.
#[test]
fn the_gross_figure_follows_the_products_own_rate() {
    let mut data = produkt(serde_json::json!([flat("ARBEITSPREIS_EINTARIF", "20")]));
    data["mwst_rate_override"] = serde_json::json!("0.07");

    let bd = compute_cost_breakdown(&data, &position("WAERME", dec!(1000)), None, &PLAIN)
        .expect("priced");
    assert_eq!(bd.total_netto_eur, dec!(200));
    assert_eq!(bd.mwst_satz, dec!(0.07));
    assert_eq!(bd.total_brutto_eur, dec!(214));
}

/// Without an override the standard rate of § 12 Abs. 1 UStG applies.
#[test]
fn the_default_rate_is_the_standard_rate() {
    let data = produkt(serde_json::json!([flat("ARBEITSPREIS_EINTARIF", "20")]));
    let bd = compute_cost_breakdown(&data, &position("STROM", dec!(1000)), None, &PLAIN)
        .expect("priced");
    assert_eq!(bd.mwst_satz, dec!(0.19));
    assert_eq!(bd.total_brutto_eur, dec!(238));
}

/// § 13b Abs. 2 Nr. 5 Buchst. b i.V.m. Abs. 5 UStG: the Wiederverkäufer owes
/// the tax, so the quotation states none and gross equals net.
#[test]
fn a_reverse_charge_quotation_states_no_umsatzsteuer() {
    let data = produkt(serde_json::json!([flat("ARBEITSPREIS_EINTARIF", "20")]));
    let ctx = PricingContext {
        behg_gas_ct_per_kwh: None,
        reverse_charge_13b: true,
    };
    let bd =
        compute_cost_breakdown(&data, &position("STROM", dec!(1000)), None, &ctx).expect("priced");
    assert_eq!(bd.mwst_satz, Decimal::ZERO);
    assert_eq!(bd.total_brutto_eur, bd.total_netto_eur);
}

/// The rate is a fraction of the net, never a percentage: `19` would quote
/// twenty times the price, so it is refused rather than interpreted.
#[test]
fn a_percentage_in_the_rate_field_is_refused() {
    let mut data = produkt(serde_json::json!([flat("ARBEITSPREIS_EINTARIF", "20")]));
    data["mwst_rate_override"] = serde_json::json!("19");
    let err = compute_cost_breakdown(&data, &position("STROM", dec!(1000)), None, &PLAIN)
        .expect_err("19 is not a rate");
    assert_eq!(err.grund, grund::MWST_SATZ_UNGUELTIG);
}

// ── Steuern und Umlagen ───────────────────────────────────────────────────────

/// § 3 StromStG: 20,50 EUR/MWh.
#[test]
fn the_stromsteuer_default_is_the_statutory_rate() {
    let data = produkt(serde_json::json!([flat("ARBEITSPREIS_EINTARIF", "20")]));
    let mut pos = position("STROM", dec!(1000));
    pos.stromsteuer_ct_per_kwh = None;
    let bd = compute_cost_breakdown(&data, &pos, None, &PLAIN).expect("priced");
    assert_eq!(bd.levies_eur, dec!(20.50), "2,05 ct/kWh × 1 000 kWh");
}

/// A Gas quotation is priced with the CO₂ certificate price of its Stichtag.
/// Certificates have been auctioned since 2026 (§ 10 Abs. 1 BEHG), so a
/// quotation with neither a series price nor a stated one is refused instead of
/// carrying a past year's Festpreis.
#[test]
fn a_gas_position_needs_a_certificate_price() {
    let data = produkt(serde_json::json!([flat("ARBEITSPREIS_EINTARIF", "5")]));
    let err = compute_cost_breakdown(&data, &position("GAS", dec!(1000)), None, &PLAIN)
        .expect_err("no CO₂ price anywhere");
    assert_eq!(err.grund, grund::BEHG_PREIS_FEHLT);

    // 55 EUR/t against the EBeV Erdgas factor of 0,18139464 kg CO₂/kWh_Hs.
    let ctx = PricingContext {
        behg_gas_ct_per_kwh: Some(dec!(0.99767)),
        reverse_charge_13b: false,
    };
    let bd = compute_cost_breakdown(&data, &position("GAS", dec!(1000)), None, &ctx)
        .expect("the series price prices it");
    assert_eq!(
        bd.levies_eur,
        dec!(15.48),
        "§ 2 Abs. 3 Satz 1 Nr. 4 EnergieStG 0,55 ct + BEHG 0,99767 ct, × 1 000 kWh"
    );
}

/// Fernwärme bears neither Strom- nor Energiesteuer on the delivery itself, so
/// nothing statutory is added to it.
#[test]
fn waerme_carries_no_stromsteuer() {
    let data = produkt(serde_json::json!([flat("ARBEITSPREIS_EINTARIF", "12")]));
    let mut pos = position("WAERME", dec!(20000));
    pos.stromsteuer_ct_per_kwh = None;
    let bd = compute_cost_breakdown(&data, &pos, None, &PLAIN).expect("priced");
    assert_eq!(bd.levies_eur, Decimal::ZERO);
}

// ── Rabatt ────────────────────────────────────────────────────────────────────

/// A variant's discount moves the Arbeitspreis and nothing else: the grid fee,
/// the Konzessionsabgabe and the statutory levies are not the supplier's to
/// discount.
#[test]
fn a_discount_touches_only_the_arbeitspreis() {
    let data = produkt(serde_json::json!([
        flat("ARBEITSPREIS_EINTARIF", "20"),
        flat("GRUNDPREIS", "50"),
    ]));
    let mut pos = position("STROM", dec!(1000));
    pos.nne_arbeitspreis_ct_per_kwh = Some(dec!(8));
    pos.stromsteuer_ct_per_kwh = None;

    let bd = compute_cost_breakdown(&data, &pos, Some(dec!(10)), &PLAIN).expect("priced");
    assert_eq!(
        bd.arbeitspreis_ct_per_kwh,
        Some(dec!(18)),
        "20 ct less 10 %"
    );
    assert_eq!(
        bd.supply_netto_eur,
        dec!(362.50),
        "1 000 kWh × 18 ct + 365 days × 50 ct"
    );
    assert_eq!(bd.nne_netto_eur, dec!(80), "the grid fee is not discounted");
    assert_eq!(bd.levies_eur, dec!(20.50), "nor is the Stromsteuer");
}
