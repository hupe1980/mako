//! Real-PostgreSQL guards for `GET /api/v1/malos/{id}/lokationsbuendel`'s
//! read: the bündel is every object the Lokationszuordnung graph joins to the
//! Marktlokation on the Stichtag — in either edge direction, half-open
//! `[valid_from, valid_to)` — each with the Objektcode its own row holds, and
//! the NB's Angaben row covering that day. The Geschäftsdatenanfrage answer
//! is built from it, so an object a wrong window leaves out is one the answer
//! silently omits, and one a closed edge lets in is one it falsely states.
//!
//! ```bash
//! cargo test -p marktd --test it -- lokationsbuendel_integration --include-ignored
//! ```
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use mako_markt::domain::{Lokationstyp, MaloId, MeloId, Sparte};
use mako_markt::error::MdmError;
use mako_markt::lokationsbuendel::{
    BuendelAngaben, NnaPosition, ObisAngabe, Profilverfahren, RegisterAngabe, Wertegranularitaet,
};
use marktd::pg::{PgLokationsbuendelRepository, PgMaloRepository, PgMeloRepository};
use rubo4e::current::{Energierichtung, ZaehlertypSpezifikation};
use sqlx::PgPool;
use time::macros::date;

const SCHEMA: &str = include_str!("../../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const OTHER_TENANT: &str = "9900000000003";
const MALO: &str = "51238696012";
const MELO: &str = "DE0000000000000000000000000000043";
const NELO: &str = "E1234567890";
const TR: &str = "D1234567890";
/// Verbrauch mit einer Messlokation (Standard) and its object codes
/// (Codeliste der Lokationsbündelstrukturen 1.0, Kap. 4.2).
const STRUKTUR: &str = "9992000000026";
const MALO_CODE: &str = "9992000001016";
const MELO_CODE: &str = "9992000001032";
const NELO_CODE: &str = "9992000001256";

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

async fn test_pool() -> Option<(PgPool, PgContainer)> {
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
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

async fn edge(
    pool: &PgPool,
    tenant: &str,
    (von, von_typ): (&str, &str),
    (nach, nach_typ): (&str, &str),
    valid: (Option<time::Date>, Option<time::Date>),
    code: Option<&str>,
) {
    sqlx::query(
        r"INSERT INTO lokationszuordnungen
              (tenant, von_id, von_typ, nach_id, nach_typ, valid_from, valid_to, lokationsbuendelcode)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(tenant)
    .bind(von)
    .bind(von_typ)
    .bind(nach)
    .bind(nach_typ)
    .bind(valid.0)
    .bind(valid.1)
    .bind(code)
    .execute(pool)
    .await
    .expect("seed edge");
}

/// A verbrauchende Marktlokation, its Messlokation behind a NeLo, and a TR
/// whose edge closes on the Stichtag.
async fn seed(pool: &PgPool) {
    let malo: rubo4e::current::Marktlokation = serde_json::from_value(serde_json::json!({
        "_typ": "MARKTLOKATION", "marktlokationsId": MALO,
        "energierichtung": "AUSSP", "lokationsbuendelObjektcode": MALO_CODE,
    }))
    .expect("valid BO4E Marktlokation");
    let malo_id: MaloId = MALO.parse().expect("valid MaLo-ID");
    PgMaloRepository::new(pool.clone())
        .upsert(&malo_id, Sparte::Strom, &malo, vec![], None, "v202607.0.0")
        .await
        .expect("seed malo");
    let melo: rubo4e::current::Messlokation = serde_json::from_value(serde_json::json!({
        "_typ": "MESSLOKATION", "messlokationsId": MELO,
        "lokationsbuendelObjektcode": MELO_CODE,
    }))
    .expect("valid BO4E Messlokation");
    let melo_id: MeloId = MELO.parse().expect("valid MeLo-ID");
    // No parent here: the edges below carry the dates the test is about.
    PgMeloRepository::new(pool.clone(), TENANT)
        .upsert(&melo_id, None, &melo, None, "v202607.0.0")
        .await
        .expect("seed melo");
    sqlx::query(
        r"INSERT INTO nelo (nelo_id, tenant, sparte, nb_mp_id, data)
          VALUES ($1, $2, 'STROM', $2, $3)",
    )
    .bind(NELO)
    .bind(TENANT)
    .bind(serde_json::json!({ "lokationsbuendelObjektcode": NELO_CODE }))
    .execute(pool)
    .await
    .expect("seed nelo");
    sqlx::query(
        r#"INSERT INTO zaehler (zaehler_id, tenant, melo_id, zaehler_typ, data)
           VALUES ('Z-1', $1, $2, 'MODERNE_MESSEINRICHTUNG',
                   '{"_typ":"ZAEHLER","zaehlertypSpezifikation":"MME_STANDARD"}')"#,
    )
    .bind(TENANT)
    .bind(MELO)
    .execute(pool)
    .await
    .expect("seed zaehler");

    // The MeLo PUT writes MELO → MALO, the graph API NELO → MELO: one bündel.
    edge(
        pool,
        TENANT,
        (MELO, "MELO"),
        (MALO, "MALO"),
        (Some(date!(2026 - 01 - 01)), None),
        Some(STRUKTUR),
    )
    .await;
    edge(
        pool,
        TENANT,
        (NELO, "NELO"),
        (MELO, "MELO"),
        (Some(date!(2026 - 12 - 01)), None),
        None,
    )
    .await;
    // Closes on the Stichtag: `valid_to` is the first day it no longer holds.
    edge(
        pool,
        TENANT,
        (TR, "TR"),
        (MALO, "MALO"),
        (None, Some(date!(2026 - 12 - 01))),
        None,
    )
    .await;
    // Another tenant's graph over the same Marktlokation.
    edge(
        pool,
        OTHER_TENANT,
        ("DE0000000000000000000000000000099", "MELO"),
        (MALO, "MALO"),
        (None, None),
        Some("9992000000034"),
    )
    .await;
}

fn angaben(valid_from: time::Date) -> BuendelAngaben {
    BuendelAngaben {
        valid_from,
        valid_to: None,
        uenb_mp_id: Some("9911845000009".to_owned()),
        technische_einrichtungen: Some(false),
        standardlastprofil: Some("H25".to_owned()),
        profilverfahren: Some(Profilverfahren::Synthetisch),
        nna_positionen: vec![NnaPosition {
            artikel_id: "1-02-0-001".to_owned(),
            zu_abschlag_prozent: rust_decimal::Decimal::ZERO,
        }],
        obis: vec![ObisAngabe {
            obis_kennzahl: "1-1:1.8.0".to_owned(),
            verwendungszweck_nb: Some("V01".to_owned()),
            verwendungszweck_lf: Some("V02".to_owned()),
            verwendungszweck_uenb: Some("V15".to_owned()),
            wertegranularitaet: None,
        }],
        register: vec![RegisterAngabe {
            zaehler_id: "Z-1".to_owned(),
            obis_kennzahl: "1-1:1.8.0".to_owned(),
            wertegranularitaet: Wertegranularitaet::Jaehrlich,
        }],
    }
}

/// The bündel of a Stichtag holds every object the graph joins on that day,
/// whichever way its edge points, with each object's own Objektcode.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_buendel_is_the_graph_of_the_stichtag() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    seed(&pool).await;
    let repo = PgLokationsbuendelRepository::new(pool.clone());

    let b = repo
        .find_at(TENANT, MALO, date!(2026 - 12 - 01))
        .await
        .expect("read")
        .expect("the Marktlokation is held");
    assert_eq!(b.energierichtung, Some(Energierichtung::Aussp));
    assert_eq!(b.lokationsbuendelcodes, vec![STRUKTUR.to_owned()]);
    let objekte: Vec<_> = b
        .objekte
        .iter()
        .map(|o| (o.typ, o.id.as_str(), o.objektcode.as_deref()))
        .collect();
    assert_eq!(
        objekte,
        vec![
            (Lokationstyp::Malo, MALO, Some(MALO_CODE)),
            (Lokationstyp::Melo, MELO, Some(MELO_CODE)),
            (Lokationstyp::Nelo, NELO, Some(NELO_CODE)),
        ],
        "the TR's edge closed on the Stichtag; the other tenant's MeLo is not this bündel"
    );
    assert_eq!(b.zuordnungen.len(), 2);
    assert_eq!(b.zaehler.len(), 1);
    assert_eq!(
        b.zaehler[0].zaehlertyp_spezifikation,
        Some(ZaehlertypSpezifikation::MmeStandard)
    );
    assert!(b.angaben.is_none(), "no Angaben row is stored yet");

    // The day before: the NeLo's edge has not opened, the TR's still holds.
    let vortag = repo
        .find_at(TENANT, MALO, date!(2026 - 11 - 30))
        .await
        .expect("read")
        .expect("held");
    let ids: Vec<_> = vortag.objekte.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, vec![MALO, MELO, TR]);

    assert!(
        repo.find_at(TENANT, "41373559241", date!(2026 - 12 - 01))
            .await
            .expect("read")
            .is_none(),
        "an unknown Marktlokation is no bündel"
    );
}

/// The Angaben row covering the Stichtag is the one read; a second row
/// overlapping it is refused, so a Stichtag never has two.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_angaben_of_the_stichtag_are_one_row() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    seed(&pool).await;
    let repo = PgLokationsbuendelRepository::new(pool.clone());
    let row = angaben(date!(2026 - 01 - 01));
    repo.upsert_angaben(TENANT, MALO, &row)
        .await
        .expect("angaben stored");

    let b = repo
        .find_at(TENANT, MALO, date!(2026 - 12 - 01))
        .await
        .expect("read")
        .expect("held");
    assert_eq!(b.angaben.as_ref(), Some(&row));
    let before = repo
        .find_at(TENANT, MALO, date!(2025 - 12 - 31))
        .await
        .expect("read")
        .expect("held");
    assert!(
        before.angaben.is_none(),
        "no row covers a day before valid_from"
    );
    let other = repo
        .find_at(OTHER_TENANT, MALO, date!(2026 - 12 - 01))
        .await
        .expect("read")
        .expect("held");
    assert!(
        other.angaben.is_none(),
        "another tenant's Angaben are not this one's"
    );

    let overlapping = repo
        .upsert_angaben(TENANT, MALO, &angaben(date!(2026 - 06 - 01)))
        .await;
    assert!(
        matches!(overlapping, Err(MdmError::Unprocessable { .. })),
        "got {overlapping:?}"
    );

    let mut falsch = angaben(date!(2027 - 01 - 01));
    falsch.uenb_mp_id = Some("ÜNB".to_owned());
    assert!(matches!(
        repo.upsert_angaben(TENANT, MALO, &falsch).await,
        Err(MdmError::Unprocessable { .. })
    ));
}
