//! Guards `productd`'s SQL `CHECK` lists against the Rust that writes and reads
//! them.
//!
//! Six columns, three kinds of writer, and the interesting half is the **read**
//! side. `build_tarifinfo` maps `kundentyp` onto BO4E, and until this guard
//! existed it answered `Kundentyp::Privat` for four of the seven values the
//! column allows — a Ladesäulen- or Wärmepumpen-Tarif was published as `PRIVAT`,
//! a wrong statement about who may buy it. A catch-all in a decoder is where a
//! `CHECK` list drifts without anything failing.
//!
//! The parser lives in `mako_service::schema_check` because it is the part that
//! goes wrong by *passing*: `epex_prices.mtu_minutes` is an unquoted integer
//! list that a quote-seeking parser reads as empty, and `products.category` was
//! previously read with a `split_once("))")` that a trailing `OR … IS NULL`
//! would defeat.

use mako_service::schema_check::{assert_agrees_in, check_values_in};
use productd::handlers::PRODUCT_CATEGORIES;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// `PRODUCT_CATEGORIES` is the accept list every write path shares.
#[test]
fn the_category_list_matches_the_constant() {
    assert_agrees_in(SCHEMA, "products", "category", PRODUCT_CATEGORIES);
}

/// `product_status` has exactly two states and the handler gates on both.
#[test]
fn product_status_matches_the_handler_gate() {
    assert_agrees_in(
        SCHEMA,
        "products",
        "product_status",
        &["DRAFT", "PUBLISHED"],
    );
}

/// Every `kundentyp` the column allows maps to a BO4E variant, and none of them
/// falls through to a default.
///
/// The forward direction is the one that used to fail silently: four values
/// reached a `_ => Privat` arm. `build_tarifinfo` now answers `None` for an
/// unrecognised value, so a drifted column drops the field instead of
/// misreporting it — and this guard is what keeps the arms and the list equal.
#[test]
fn every_kundentyp_the_column_allows_is_mapped() {
    let listed = check_values_in(SCHEMA, "products", "kundentyp");
    assert_eq!(
        listed.len(),
        7,
        "the column allows seven values: {listed:?}"
    );

    for value in &listed {
        let row = row_with_kundentyp(value);
        let tarifinfo = productd::handlers::build_tarifinfo(&row, "9900000000001");
        assert!(
            tarifinfo.kundentypen.is_some(),
            "`kundentyp` = {value:?} is allowed by the column and maps to no BO4E \
             variant — `build_tarifinfo` drops the field, so the Tarifinfo says nothing \
             about who may buy this product"
        );
    }

    // The other direction: a value the column forbids must not resolve.
    let row = row_with_kundentyp("Grossabnehmer");
    assert!(
        productd::handlers::build_tarifinfo(&row, "9900000000001")
            .kundentypen
            .is_none(),
        "an unrecognised Kundentyp must drop the field, never default to one"
    );
}

/// `Ladesaeule` and `Haushalt` are BO4E variants in their own right.
///
/// Pinned separately because collapsing them into `Privat` is exactly what this
/// file was written to stop, and a future edit could reintroduce it while
/// leaving the mapping total.
#[test]
fn the_two_exact_bo4e_kundentypen_are_not_collapsed() {
    use rubo4e::current::Kundentyp;
    for (value, expected) in [
        ("Haushalt", Kundentyp::Haushalt),
        ("Ladesaeule", Kundentyp::Ladesaeule),
        ("Gewerbe", Kundentyp::Gewerbe),
        ("Gewerbe_RLM", Kundentyp::Gewerbe),
    ] {
        let row = row_with_kundentyp(value);
        let got = productd::handlers::build_tarifinfo(&row, "9900000000001")
            .kundentypen
            .unwrap_or_default();
        assert_eq!(got, vec![expected], "`kundentyp` = {value:?}");
    }
}

/// The BEHG price source round-trips through `Quelle`.
#[test]
fn the_nehs_price_source_matches_quelle() {
    let listed = check_values_in(SCHEMA, "nehs_prices", "source");
    for value in &listed {
        let parsed = productd::behg::Quelle::parse(value).unwrap_or_else(|| {
            panic!("`source` allows {value:?}, which `Quelle::parse` does not accept")
        });
        assert_eq!(
            parsed.as_db(),
            value.as_str(),
            "`source` = {value:?} does not round-trip"
        );
    }
    assert!(
        productd::behg::Quelle::parse("kaffee").is_none(),
        "`Quelle::parse` must refuse a value the column forbids"
    );
}

/// The Angebot lifecycle states all decode to a distinct BO4E Angebotsstatus.
///
/// `status_from_str` ends in `_ => Unknown`, so a drifted value would be
/// published as `UNKNOWN` rather than failing — the same silent shape as the
/// Kundentyp catch-all.
#[test]
fn every_angebot_status_decodes_to_something_other_than_unknown() {
    use rubo4e::current::Angebotsstatus;
    let listed = check_values_in(SCHEMA, "angebote", "status");
    assert_eq!(listed.len(), 5, "five lifecycle states: {listed:?}");
    for value in &listed {
        assert_ne!(
            productd::bo4e_angebot::status_from_str(value),
            Angebotsstatus::Unknown,
            "`angebote.status` allows {value:?}, which decodes to UNKNOWN — the offer's \
             state would go out as unspecified"
        );
    }
    assert_eq!(
        productd::bo4e_angebot::status_from_str("ZURUECKGEZOGEN"),
        Angebotsstatus::Unknown,
        "a value the column forbids must not resolve to a real status"
    );
}

/// An **integer** CHECK list — the shape a quote-seeking parser reads as empty.
///
/// `mtu_minutes` is the day-ahead market time unit: 15 since 01.10.2025, 60 for
/// the historic series. Both must stay accepted, or a stored curve becomes
/// unreadable.
#[test]
fn the_market_time_unit_list_is_read_as_integers() {
    assert_agrees_in(SCHEMA, "epex_prices", "mtu_minutes", &["15", "60"]);
}

// ── Fixture ───────────────────────────────────────────────────────────────────

/// A minimal row that differs only in `kundentyp`.
fn row_with_kundentyp(kundentyp: &str) -> productd::pg::ProductRow {
    productd::pg::ProductRow {
        kundentyp: Some(kundentyp.to_owned()),
        ..minimal_row()
    }
}

fn minimal_row() -> productd::pg::ProductRow {
    productd::pg::ProductRow {
        id: uuid::Uuid::nil(),
        lf_mp_id: "9900000000001".to_owned(),
        product_code: "TEST".to_owned(),
        category: "STROM".to_owned(),
        name: "Testtarif".to_owned(),
        sparte: Some("STROM".to_owned()),
        register_count: None,
        kundentyp: None,
        dyn_source: None,
        valid_from: None,
        valid_to: None,
        data: serde_json::json!({}),
        bo4e_version: "v202607".to_owned(),
        product_status: "PUBLISHED".to_owned(),
        energiemix: None,
        oekolabel: None,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}
