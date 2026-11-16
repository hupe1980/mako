//! The frozen Endkunden-Angebot: what it prices, what it refuses,
//! and how its hash is held.
//!
//! The pricing tests run without a database: [`bepreise`] takes the product,
//! the Netzbetreiber's sheets and the pricing context already read. The schema
//! properties — append-only, the hash held to the bytes, nothing stored on a
//! refused request — need PostgreSQL and are `#[ignore]`d:
//!
//! ```bash
//! just test-productd-db
//! ```
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use productd::endkunden_angebot::{
    Eingaben, EndkundenAngebotAnfrage, KaBlatt, Produktstand, Sparte, bepreise, code, gueltig_bis,
    kanonisch, pruefe_anfrage, sha256_hex,
};
use productd::handlers::{PricingContext, pricing_context_fuer};
use rust_decimal::{Decimal, dec};
use time::macros::date;

const LF: &str = "9900357000004";
const NB: &str = "9900000000003";
/// The canonical valid fixture MaLo.
const MALO: &str = "51238696012";
/// The refusal fixture: a wrong check digit.
const MALO_FALSCH: &str = "51238696782";

fn anfrage(sparte: Sparte, kwh: Decimal, lieferbeginn: time::Date) -> EndkundenAngebotAnfrage {
    EndkundenAngebotAnfrage {
        product_code: "STROM-OEKO-24".to_owned(),
        sparte,
        verbrauch_kwh: kwh,
        malo_id: Some(MALO.to_owned()),
        nb_mp_id: None,
        plz: None,
        gewuenschter_lieferbeginn: lieferbeginn,
    }
}

/// A supply product: 40 ct/Tag Grundpreis, 30 ct/kWh Arbeitspreis.
fn produkt_data() -> serde_json::Value {
    serde_json::json!({ "tarifpreise": [
        { "preistyp": "GRUNDPREIS", "preisstaffeln": [{ "preis": "40" }] },
        { "preistyp": "ARBEITSPREIS_EINTARIF", "preisstaffeln": [{ "preis": "30.00" }] },
    ]})
}

fn position(
    typ: &str,
    einheit: &str,
    preis: &str,
    bezug: Option<&str>,
    zeit: Option<&str>,
) -> serde_json::Value {
    let mut p = serde_json::json!({
        "leistungstyp": typ,
        "preiseinheit": einheit,
        "preisstaffeln": [{ "preis": preis }],
    });
    if let Some(b) = bezug {
        p["bezugsgroesse"] = serde_json::json!(b);
    }
    if let Some(z) = zeit {
        p["zeitbasis"] = serde_json::json!(z);
    }
    p
}

fn netznutzung() -> serde_json::Value {
    serde_json::json!({ "preispositionen": [
        position("ARBEITSPREIS_WIRKARBEIT", "CT", "8.50", Some("KWH"), None),
        position("GRUNDPREIS", "EUR", "60", None, Some("JAHR")),
    ]})
}

fn messung() -> serde_json::Value {
    serde_json::json!({ "preispositionen": [
        position("MESSSTELLENBETRIEB", "EUR", "20", None, Some("JAHR")),
    ]})
}

fn ka(gruppe: Option<&str>) -> KaBlatt {
    KaBlatt {
        kundengruppe_ka: gruppe.map(str::to_owned),
        data: serde_json::json!({ "preispositionen": [
            position("KONZESSIONS_ABGABE", "CT", "1.59", Some("KWH"), None),
        ]}),
    }
}

fn ctx(tag: time::Date) -> PricingContext {
    pricing_context_fuer(tag, None, Err("kein Regime im Test".to_owned()), false)
}

/// Every input present; the test overrides the one it is about.
struct Fixture {
    anfrage: EndkundenAngebotAnfrage,
    produkt: serde_json::Value,
    status: &'static str,
    nne: Option<serde_json::Value>,
    msb: Option<serde_json::Value>,
    ka: Option<KaBlatt>,
    ctx: PricingContext,
}

impl Fixture {
    fn strom(kwh: Decimal, tag: time::Date) -> Self {
        Self {
            anfrage: anfrage(Sparte::Strom, kwh, tag),
            produkt: produkt_data(),
            status: "PUBLISHED",
            nne: Some(netznutzung()),
            msb: Some(messung()),
            ka: Some(ka(Some("Tarifkunden"))),
            ctx: ctx(tag),
        }
    }

    fn eingaben(&self) -> Eingaben<'_> {
        Eingaben {
            lf_mp_id: LF,
            anfrage: &self.anfrage,
            malo_id: Some(MALO),
            nb_mp_id: NB,
            produkt: Some(Produktstand {
                product_status: self.status,
                sparte: Some("STROM"),
                data: &self.produkt,
            }),
            netznutzung: self.nne.as_ref(),
            messung: self.msb.as_ref(),
            konzessionsabgabe: self.ka.as_ref(),
            ctx: &self.ctx,
            gueltig_bis: gueltig_bis(date!(2026 - 11 - 15)),
        }
    }
}

// ── What is refused ───────────────────────────────────────────────────────────

/// No Netznutzung sheet for the NB on the Lieferbeginn refuses, naming the NB —
/// never a Netzentgelt of zero, which would underprice every kWh offered.
#[test]
fn a_missing_preisblatt_refuses_naming_the_nb() {
    let mut f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    f.nne = None;
    let err = bepreise(&f.eingaben()).expect_err("an offer without the NB's Netzentgelte");
    assert_eq!(err.code, code::EINGABE_FEHLT);
    let fehlend = err.fehlend();
    assert!(
        fehlend
            .iter()
            .any(|f| f.contains("Preisblatt Netznutzung") && f.contains(NB)),
        "the refusal must name the Netznutzung sheet and the NB: {fehlend:?}"
    );
}

/// A Lieferbeginn in a year the Stromsteuer and Umlage series have not read
/// refuses naming the year and every series — never a neighbouring year's rate.
#[test]
fn a_missing_series_year_refuses_naming_it() {
    let f = Fixture::strom(dec!(3500), date!(2028 - 01 - 01));
    let err = bepreise(&f.eingaben()).expect_err("2028 has no read Stromsteuer");
    assert_eq!(err.code, code::EINGABE_FEHLT);
    let fehlend = err.fehlend();
    for name in [
        "Stromsteuer 2028",
        "KWKG-Umlage 2028",
        "Offshore-Netzumlage 2028",
    ] {
        assert!(
            fehlend.iter().any(|f| f.contains(name)),
            "the refusal must name {name}: {fehlend:?}"
        );
    }
}

/// A MaLo-ID with a wrong check digit is refused by the check that runs before
/// anything is read or stored.
#[test]
fn a_wrong_check_digit_is_refused_before_anything_is_stored() {
    let mut a = anfrage(Sparte::Strom, dec!(3500), date!(2027 - 02 - 01));
    a.malo_id = Some(MALO_FALSCH.to_owned());
    let err = pruefe_anfrage(&a, date!(2026 - 11 - 15)).expect_err("wrong check digit");
    assert_eq!(err.code, code::MALO_ID_UNGUELTIG);

    a.malo_id = Some(MALO.to_owned());
    assert!(pruefe_anfrage(&a, date!(2026 - 11 - 15)).is_ok());
}

#[test]
fn the_nb_source_is_exactly_one() {
    let heute = date!(2026 - 11 - 15);
    let mut a = anfrage(Sparte::Strom, dec!(3500), date!(2027 - 02 - 01));
    a.nb_mp_id = Some(NB.to_owned());
    assert_eq!(
        pruefe_anfrage(&a, heute).unwrap_err().code,
        code::NB_QUELLE_MEHRDEUTIG
    );
    a.malo_id = None;
    a.nb_mp_id = None;
    assert_eq!(
        pruefe_anfrage(&a, heute).unwrap_err().code,
        code::EINGABE_FEHLT
    );
    a.plz = Some("10115".to_owned());
    assert_eq!(
        pruefe_anfrage(&a, heute).unwrap_err().code,
        code::PLZ_NICHT_UNTERSTUETZT
    );
}

#[test]
fn a_lieferbeginn_before_today_is_refused() {
    let a = anfrage(Sparte::Strom, dec!(3500), date!(2026 - 11 - 14));
    assert_eq!(
        pruefe_anfrage(&a, date!(2026 - 11 - 15)).unwrap_err().code,
        code::LIEFERBEGINN_VERGANGEN
    );
}

/// Every missing sheet is named at once, so the operator fixes them in one
/// round rather than one refusal at a time.
#[test]
fn every_missing_sheet_is_named_at_once() {
    let mut f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    f.nne = None;
    f.msb = None;
    f.ka = None;
    let fehlend = bepreise(&f.eingaben()).unwrap_err().fehlend();
    for name in [
        "Preisblatt Netznutzung",
        "Preisblatt Messung",
        "Preisblatt Konzessionsabgabe",
    ] {
        assert!(
            fehlend.iter().any(|f| f.contains(name)),
            "{name}: {fehlend:?}"
        );
    }
}

/// A product still in DRAFT is not offered to a consumer.
#[test]
fn a_draft_product_is_not_offered() {
    let mut f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    f.status = "DRAFT";
    let fehlend = bepreise(&f.eingaben()).unwrap_err().fehlend();
    assert!(
        fehlend
            .iter()
            .any(|f| f.contains("veröffentlichtes Produkt")),
        "{fehlend:?}"
    );
}

/// Above 30 000 kWh the Strom Konzessionsabgabe's group depends on the measured
/// demand (§ 2 Abs. 7 KAV), which an offer does not know.
#[test]
fn a_strom_offer_above_thirty_thousand_kwh_refuses_the_ka_group() {
    let f = Fixture::strom(dec!(30001), date!(2027 - 02 - 01));
    let fehlend = bepreise(&f.eingaben()).unwrap_err().fehlend();
    assert!(
        fehlend.iter().any(|f| f.contains("§ 2 Abs. 7 KAV")),
        "{fehlend:?}"
    );
}

/// A sheet carrying two Arbeitspreis positions does not say which one applies.
#[test]
fn two_positions_of_one_kind_refuse_rather_than_pick() {
    let mut f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    f.nne = Some(serde_json::json!({ "preispositionen": [
        position("ARBEITSPREIS_WIRKARBEIT", "CT", "8.50", Some("KWH"), None),
        position("ARBEITSPREIS_WIRKARBEIT", "CT", "6.10", Some("KWH"), None),
        position("GRUNDPREIS", "EUR", "60", None, Some("JAHR")),
    ]}));
    let fehlend = bepreise(&f.eingaben()).unwrap_err().fehlend();
    assert!(
        fehlend.iter().any(|f| f.contains("welche gilt")),
        "{fehlend:?}"
    );
}

/// A Netzentgelt without its unit is not read as ct/kWh.
#[test]
fn a_position_without_its_unit_is_refused() {
    let mut f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    f.nne = Some(serde_json::json!({ "preispositionen": [
        position("ARBEITSPREIS_WIRKARBEIT", "CT", "8.50", None, None),
        position("GRUNDPREIS", "EUR", "60", None, Some("JAHR")),
    ]}));
    let fehlend = bepreise(&f.eingaben()).unwrap_err().fehlend();
    assert!(
        fehlend.iter().any(|f| f.contains("Bezugsgröße")),
        "{fehlend:?}"
    );
}

// ── What is priced ────────────────────────────────────────────────────────────

/// The Jahreskosten are every component, each from its source, gross.
#[test]
fn an_offer_prices_every_component_with_its_source() {
    use grid_billing::umlagen::{self, Letztverbrauchergruppe as G};
    let f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    let angebot = bepreise(&f.eingaben()).expect("every input present");

    let umlagen = umlagen::sect19_stromnev_ct_per_kwh(2027, G::A).unwrap()
        + umlagen::offshore_netzumlage_ct_per_kwh(2027, G::A).unwrap()
        + umlagen::kwkg_umlage_ct_per_kwh(2027, G::A).unwrap();
    let steuer = grid_billing::stromsteuer_for_year(2027).unwrap();
    let ct = dec!(30.00) + dec!(8.50) + umlagen + dec!(1.59) + steuer;
    // 40 ct/Tag × 365 + NNE 60 + Messstellenbetrieb 20.
    let eur_fix = dec!(146) + dec!(60) + dec!(20);
    let netto = ct * dec!(3500) / dec!(100) + eur_fix;
    let brutto = (netto * dec!(1.19))
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    assert_eq!(angebot.jahreskosten_brutto_eur, brutto);
    assert_eq!(
        angebot.abschlag_monat_brutto_eur,
        (brutto / dec!(12))
            .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
    );
    for art in [
        "ARBEITSPREIS",
        "GRUNDPREIS",
        "NETZENTGELT_ARBEITSPREIS",
        "NETZENTGELT_GRUNDPREIS",
        "MESSSTELLENBETRIEB",
        "KONZESSIONSABGABE",
        "SECT19_STROMNEV_UMLAGE",
        "OFFSHORE_NETZUMLAGE",
        "KWKG_UMLAGE",
        "STROMSTEUER",
        "UMSATZSTEUER",
        "ARBEITSPREIS_GESAMT_BRUTTO",
        "GRUNDPREIS_GESAMT_BRUTTO",
    ] {
        let zeile = angebot
            .preise
            .iter()
            .find(|z| z.art == art)
            .unwrap_or_else(|| panic!("no line {art}"));
        assert!(!zeile.quelle.is_empty(), "{art} names no source");
    }
    assert_eq!(angebot.product_version.len(), 64);
}

/// For Gas no rule picks the Konzessionsabgabe group, so a sheet that applies to
/// one group only does not price the offer.
#[test]
fn a_gas_offer_refuses_a_group_specific_ka_sheet() {
    let tag = date!(2027 - 02 - 01);
    let mut f = Fixture::strom(dec!(12000), tag);
    f.anfrage.sparte = Sparte::Gas;
    f.ka = Some(ka(Some("Sondervertragskunden")));
    let mut e = f.eingaben();
    e.produkt = Some(Produktstand {
        product_status: "PUBLISHED",
        sparte: Some("GAS"),
        data: &f.produkt,
    });
    let fehlend = bepreise(&e).unwrap_err().fehlend();
    assert!(
        fehlend
            .iter()
            .any(|f| f.contains("Kundengruppe dieser Belieferung")),
        "{fehlend:?}"
    );
}

#[test]
fn a_product_of_another_sparte_is_refused() {
    let f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    let mut e = f.eingaben();
    e.produkt = Some(Produktstand {
        product_status: "PUBLISHED",
        sparte: Some("GAS"),
        data: &f.produkt,
    });
    assert_eq!(bepreise(&e).unwrap_err().code, code::SPARTE_ABWEICHEND);
}

/// The offer binds to the end of the fourteenth Berlin day, stated in the
/// Berlin offset.
#[test]
fn the_offer_binds_to_the_end_of_the_fourteenth_day() {
    let bis = gueltig_bis(date!(2026 - 11 - 15));
    assert_eq!(
        bis.format(&time::format_description::well_known::Rfc3339)
            .unwrap(),
        "2026-11-29T23:59:59+01:00"
    );
}

// ── The hash ──────────────────────────────────────────────────────────────────

/// A canonical JSON writer that shares nothing with `serde_jcs`: keys sorted by
/// their UTF-16 code units (RFC 8785 Kap. 3.2.3), no whitespace, strings
/// escaped as JSON. The offer body carries no number, so no number formatting
/// enters.
fn zweiter_serialisierer(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push(':');
                zweiter_serialisierer(&m[*k], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                zweiter_serialisierer(x, out);
            }
            out.push(']');
        }
        serde_json::Value::Number(_) => panic!("the offer body carries no JSON number"),
        other => out.push_str(&serde_json::to_string(other).unwrap()),
    }
}

/// A third party re-serialising the offer with another canonicaliser reproduces
/// `content_sha256` — the property that lets an order be proven outside mako.
#[test]
fn the_hash_is_reproduced_by_a_second_serialiser() {
    let f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    let body = bepreise(&f.eingaben()).unwrap().body().unwrap();
    let bytes = kanonisch(&body).unwrap();

    let mut zweiter = String::new();
    zweiter_serialisierer(&body, &mut zweiter);
    assert_eq!(
        String::from_utf8(bytes.clone()).unwrap(),
        zweiter,
        "two canonical serialisations of the offer differ"
    );
    assert_eq!(sha256_hex(&bytes), sha256_hex(zweiter.as_bytes()));
}

// ── PostgreSQL ────────────────────────────────────────────────────────────────

const SCHEMA: &str = include_str!("../../migrations/0001_schema.sql");
const TENANT: &str = "9900357000004";

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

async fn test_pool() -> Option<(sqlx::PgPool, PgContainer)> {
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = sqlx::PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

async fn gespeichert(pool: &sqlx::PgPool) -> serde_json::Value {
    let f = Fixture::strom(dec!(3500), date!(2027 - 02 - 01));
    let angebot = bepreise(&f.eingaben()).unwrap();
    productd::endkunden_angebot::speichere(pool, TENANT, &angebot)
        .await
        .expect("insert")
}

async fn anzahl(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM endkunden_angebote")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A stored offer is read back with a hash that recomputes over what is served.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_stored_offer_reads_back_with_its_hash() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let erstellt = gespeichert(&pool).await;
    let id: uuid::Uuid = erstellt["id"].as_str().unwrap().parse().unwrap();
    let gelesen = productd::endkunden_angebot::lade(&pool, TENANT, id)
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(gelesen, erstellt, "GET answers what POST answered");

    let mut body = gelesen.clone();
    let m = body.as_object_mut().unwrap();
    for k in ["id", "content_sha256", "created_at"] {
        m.remove(k);
    }
    assert_eq!(
        sha256_hex(&kanonisch(&body).unwrap()),
        gelesen["content_sha256"].as_str().unwrap()
    );
    assert!(
        productd::endkunden_angebot::lade(&pool, "9900000000001", id)
            .await
            .unwrap()
            .is_none(),
        "another tenant reads nothing"
    );
}

/// An order is proven against these bytes, so the row never changes.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_endkunden_angebot_is_append_only() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    gespeichert(&pool).await;
    for sql in [
        "UPDATE endkunden_angebote SET jahreskosten_brutto_eur = 1",
        "DELETE FROM endkunden_angebote",
        "TRUNCATE endkunden_angebote",
    ] {
        let err = sqlx::query(sql)
            .execute(&pool)
            .await
            .expect_err("an offer row must not change");
        assert!(err.to_string().contains("append-only"), "{sql}: {err}");
    }
    assert_eq!(anzahl(&pool).await, 1);
}

/// The schema holds `content_sha256` to `kanonisch`, whoever writes the row.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_schema_holds_the_hash_to_the_bytes() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let err = sqlx::query(
        "INSERT INTO endkunden_angebote
            (id, tenant, lf_mp_id, product_code, product_version, sparte, nb_mp_id,
             verbrauch_kwh, gewuenschter_lieferbeginn, lieferbeginn_fruehestens, preise,
             jahreskosten_brutto_eur, abschlag_monat_brutto_eur, gueltig_bis, kanonisch,
             content_sha256)
         VALUES (gen_random_uuid(), $1, $1, 'P', repeat('a', 64), 'STROM', $2, 3500,
                 '2027-02-01', '2027-02-01', '[]', 1, 1, now(), '\\x7b7d', repeat('0', 64))",
    )
    .bind(TENANT)
    .bind(NB)
    .execute(&pool)
    .await
    .expect_err("a hash that is not the bytes' must be refused");
    assert!(err.to_string().contains("check constraint"), "{err}");
}

/// The check-digit refusal comes before `marktd` is consulted and before any
/// row is written: no marktd is configured here, and the answer is still the
/// MaLo refusal, with the table empty.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_wrong_check_digit_stores_nothing() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let mut a = anfrage(
        Sparte::Strom,
        dec!(3500),
        mako_fristen::heute() + time::Duration::days(30),
    );
    a.malo_id = Some(MALO_FALSCH.to_owned());
    let err = productd::endkunden_angebot::erstelle(
        &pool,
        TENANT,
        LF,
        &productd::endkunden_angebot::MarktdQuelle::new(None, None, reqwest::Client::new()),
        &productd::billingd::BehgRegimeQuelle::keine(),
        &a,
    )
    .await
    .expect_err("wrong check digit");
    assert_eq!(err.code(), Some(code::MALO_ID_UNGUELTIG));
    assert_eq!(anzahl(&pool).await, 0);
}
