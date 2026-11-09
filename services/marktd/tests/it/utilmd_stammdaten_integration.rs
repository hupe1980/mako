//! Real-PostgreSQL guards for `GET /api/v1/malos/{id}/utilmd-stammdaten`'s
//! read: every fact of a Marktlokation's UTILMD record comes from the one
//! table or BO4E object that holds it, and a fact nothing holds stays `None`
//! — the Anmeldung EoG refuses on it, so a filled-in default here would put a
//! statement on the wire nobody made.
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! cargo test -p marktd --test it -- utilmd_stammdaten_integration --include-ignored
//! ```
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use mako_markt::domain::{MaloId, MeloId, Sparte};
use mako_markt::error::MdmError;
use mako_markt::utilmd_stammdaten::{
    Abrechnungsvariante, Betriebszustand, MsbGrundlage, MsbRolle, NetznutzungZahler,
    Netznutzungsdaten, Netznutzungsvertrag,
};
use marktd::pg::{
    MsbDetails, PgMaloRepository, PgMeloMsbRepository, PgMeloRepository,
    PgUtilmdStammdatenRepository,
};
use rubo4e::current::{Fallgruppenzuordnung, Gasqualitaet, Netzebene, Prognosegrundlage};
use sqlx::PgPool;
use time::macros::date;

const SCHEMA: &str = include_str!("../../migrations/0001_initial.sql");
const TENANT: &str = "9807317000007";
const MALO: &str = "51238696012";
const MELO: &str = "DE0000000000000000000000000000042";
const MSB: &str = "9807317000007";
const MARKTGEBIET: &str = "37Y005053MH0000R";

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

fn malo_id() -> MaloId {
    MALO.parse().expect("valid MaLo-ID")
}

async fn seed_malo(pool: &PgPool, mit_adresse: bool) {
    let mut body = serde_json::json!({
        "_typ": "MARKTLOKATION",
        "marktlokationsId": MALO,
        "netzebene": "ND",
        "gasqualitaet": "H_GAS",
        "marktgebiet": MARKTGEBIET,
        "endkunde": { "_typ": "GESCHAEFTSPARTNER", "nachname": "Muster", "vorname": "Erika" },
        "zaehlwerke": [
            { "obisKennzahl": "7-20:99.33.17", "konzessionsabgabe": { "satz": "SA" } }
        ],
    });
    if mit_adresse {
        body["lokationsadresse"] = serde_json::json!({
            "strasse": "Musterstr.", "hausnummer": "1", "postleitzahl": "10115",
            "ort": "Berlin", "landescode": "DE",
        });
    }
    let malo: rubo4e::current::Marktlokation =
        serde_json::from_value(body).expect("valid BO4E Marktlokation");
    PgMaloRepository::new(pool.clone())
        .upsert(&malo_id(), Sparte::Gas, &malo, vec![], None, "v202607.0.0")
        .await
        .expect("seed malo");
}

async fn seed_rest(pool: &PgPool) {
    sqlx::query(
        r#"INSERT INTO bilanzierungen (tenant, malo_id, bilanzierungsbeginn, data)
           VALUES ($1, $2, '2026-01-01T00:00:00Z',
                   '{"_typ":"BILANZIERUNG","prognosegrundlage":"WERTE",
                     "fallgruppenzuordnung":"GABI_RLM_MIT_TAGESBAND"}')"#,
    )
    .bind(TENANT)
    .bind(MALO)
    .execute(pool)
    .await
    .expect("seed bilanzierung");

    let melo: rubo4e::current::Messlokation = serde_json::from_value(serde_json::json!({
        "_typ": "MESSLOKATION", "messlokationsId": MELO,
    }))
    .expect("valid BO4E Messlokation");
    let melo_id: MeloId = MELO.parse().expect("valid MeLo-ID");
    PgMeloRepository::new(pool.clone(), TENANT)
        .upsert(&melo_id, Some(&malo_id()), &melo, None, "v202607.0.0")
        .await
        .expect("seed melo");
    PgMeloMsbRepository::new(pool.clone())
        .assign_msb_with(
            TENANT,
            MELO,
            MSB,
            date!(2026 - 01 - 01),
            MsbDetails {
                rolle: Some(MsbRolle::Grundzustaendig),
                grundlage: Some(MsbGrundlage::Vertrag),
                gmsb_mp_id: Some(MSB),
            },
        )
        .await
        .expect("seed msb");

    sqlx::query(
        r#"INSERT INTO zaehler (zaehler_id, tenant, melo_id, zaehler_typ, data)
           VALUES ('Z-GAS-1', $1, $2, 'BALGENGASZAEHLER',
                   '{"_typ":"ZAEHLER","zaehlernummer":"1ABC0012345678",
                     "zaehlertyp":"BALGENGASZAEHLER","zaehlergroesse":"G4",
                     "istFernauslesbar":false}')"#,
    )
    .bind(TENANT)
    .bind(MELO)
    .execute(pool)
    .await
    .expect("seed zaehler");
    sqlx::query(
        r"INSERT INTO zaehler_register
              (zaehler_id, tenant, bezeichnung, zaehlerauspraegung, obis_kennzahl, valid_from)
          VALUES ('Z-GAS-1', $1, 'Gesamt', 'EINZEL', '7-20:99.33.17', '2026-01-01')",
    )
    .bind(TENANT)
    .execute(pool)
    .await
    .expect("seed register");
}

fn netznutzung(valid_from: time::Date) -> Netznutzungsdaten {
    Netznutzungsdaten {
        valid_from,
        valid_to: None,
        haushaltskunde: Some(false),
        betriebszustand: Some(Betriebszustand::Regelbetrieb),
        abrechnungstermin: Some("1001".to_owned()),
        naechste_abrechnung_jahr: Some(2027),
        abrechnungsintervall_monate: Some(12),
        abrechnungsvariante: Some(Abrechnungsvariante::ArbeitspreisLeistungspreis),
        gemeinderabatt_prozent: Some(rust_decimal::Decimal::ZERO),
        turnusablesung: Some("05040504".to_owned()),
        netznutzungsvertrag: Some(Netznutzungsvertrag::LieferantNb),
        netznutzung_zahler: Some(NetznutzungZahler::Lieferant),
    }
}

/// The record of a Stichtag assembles every source marktd holds.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_record_assembles_what_marktd_holds() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    seed_malo(&pool, true).await;
    seed_rest(&pool).await;
    let repo = PgUtilmdStammdatenRepository::new(pool.clone());
    repo.upsert_netznutzung(TENANT, MALO, &netznutzung(date!(2026 - 01 - 01)))
        .await
        .expect("netznutzung stored");

    // The Lokationszuordnung a MeLo PUT writes opens today.
    let stichtag = mako_fristen::heute();
    let r = repo
        .find_at(TENANT, MALO, stichtag)
        .await
        .expect("read")
        .expect("the Marktlokation is held");
    assert_eq!(r.sparte, Sparte::Gas);
    assert_eq!(r.netzebene, Some(Netzebene::Nd));
    assert_eq!(r.gasqualitaet, Some(Gasqualitaet::HGas));
    assert_eq!(r.marktgebiet.as_deref(), Some(MARKTGEBIET));
    let a = r.anschrift.as_ref().expect("the Lokationsadresse");
    assert_eq!(a.strasse.as_deref(), Some("Musterstr."));
    assert_eq!(a.landescode.as_deref(), Some("DE"));
    assert_eq!(
        r.kunde.as_ref().and_then(|k| k.nachname.as_deref()),
        Some("Muster")
    );
    assert_eq!(r.zaehlwerke.len(), 1);
    assert_eq!(r.zaehlwerke[0].obis_kennzahl, "7-20:99.33.17");
    assert_eq!(
        r.zaehlwerke[0].konzessionsabgabe_satz,
        Some(rubo4e::current::AbgabeArt::Sa)
    );
    let b = r.bilanzierung.as_ref().expect("the Bilanzierung");
    assert_eq!(b.prognosegrundlage, Some(Prognosegrundlage::Werte));
    assert_eq!(
        b.fallgruppe,
        Some(Fallgruppenzuordnung::GabiRlmMitTagesband)
    );
    assert_eq!(r.netznutzung, Some(netznutzung(date!(2026 - 01 - 01))));
    let [melo] = r.messlokationen.as_slice() else {
        panic!("one Messlokation, got {:?}", r.messlokationen);
    };
    assert_eq!(melo.melo_id, MELO);
    let msb = melo.messstellenbetrieb.as_ref().expect("an MSB");
    assert_eq!(msb.rolle, Some(MsbRolle::Grundzustaendig));
    assert_eq!(msb.gmsb_mp_id.as_deref(), Some(MSB));
    let [z] = melo.zaehler.as_slice() else {
        panic!("one Zähler");
    };
    assert_eq!(z.zaehlernummer.as_deref(), Some("1ABC0012345678"));
    assert_eq!(z.fernauslesbar, Some(false));
    assert_eq!(z.obis_kennzahlen, ["7-20:99.33.17"]);
}

/// A fact nothing holds stays `None`: no Lokationsadresse, no Netznutzungsdaten
/// row covering the Stichtag, no Messlokation — never a default.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_fact_nothing_holds_is_none() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    seed_malo(&pool, false).await;
    let repo = PgUtilmdStammdatenRepository::new(pool.clone());
    repo.upsert_netznutzung(TENANT, MALO, &netznutzung(date!(2027 - 01 - 01)))
        .await
        .expect("a later row");
    let r = repo
        .find_at(TENANT, MALO, date!(2026 - 12 - 01))
        .await
        .expect("read")
        .expect("the Marktlokation is held");
    assert_eq!(r.anschrift, None);
    assert_eq!(r.netznutzung, None, "the row starts after the Stichtag");
    assert_eq!(r.bilanzierung, None);
    assert!(r.messlokationen.is_empty());
    assert!(
        repo.find_at(TENANT, "10001234558", date!(2026 - 12 - 01))
            .await
            .expect("read")
            .is_none(),
        "an unknown Marktlokation is no record"
    );
}

/// Two Netznutzungsdaten rows covering one date are refused as a client
/// error, so a Stichtag resolves to one record.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn overlapping_netznutzungsdaten_are_refused() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    seed_malo(&pool, true).await;
    let repo = PgUtilmdStammdatenRepository::new(pool.clone());
    repo.upsert_netznutzung(TENANT, MALO, &netznutzung(date!(2026 - 01 - 01)))
        .await
        .expect("first row");
    let err = repo
        .upsert_netznutzung(TENANT, MALO, &netznutzung(date!(2026 - 06 - 01)))
        .await
        .expect_err("an overlapping row is refused");
    assert!(matches!(err, MdmError::Unprocessable { .. }), "{err:?}");
    let mut bad = netznutzung(date!(2028 - 01 - 01));
    bad.abrechnungstermin = Some("1301".to_owned());
    let err = repo
        .upsert_netznutzung(TENANT, MALO, &bad)
        .await
        .expect_err("a month 13 is refused");
    assert!(matches!(err, MdmError::Unprocessable { .. }), "{err:?}");
}

/// The Turnusablesung and the Netznutzungsvertrag with its Zahler are stored
/// and read back on the Stichtag they are valid for, and refused in a shape
/// the UTILMD cannot carry: a `DTM+752` other than `MMDD`/`MMWWMMWW`, the
/// 29.02. the MIG replaces by the 28.02., and a Kunde paying under a Vertrag
/// zwischen Lieferanten und NB (Bedingung `[216]`).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn turnusablesung_and_netznutzungsvertrag_are_held_per_stichtag() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    seed_malo(&pool, true).await;
    let repo = PgUtilmdStammdatenRepository::new(pool.clone());
    let mut first = netznutzung(date!(2026 - 01 - 01));
    first.valid_to = Some(date!(2027 - 01 - 01));
    repo.upsert_netznutzung(TENANT, MALO, &first)
        .await
        .expect("first row");
    let mut second = netznutzung(date!(2027 - 01 - 01));
    second.turnusablesung = Some("0915".to_owned());
    second.netznutzungsvertrag = Some(Netznutzungsvertrag::KundeNb);
    second.netznutzung_zahler = Some(NetznutzungZahler::Kunde);
    repo.upsert_netznutzung(TENANT, MALO, &second)
        .await
        .expect("the next row, starting where the first ends");

    let read = |d: time::Date| {
        let repo = repo.clone();
        async move {
            repo.find_at(TENANT, MALO, d)
                .await
                .expect("read")
                .expect("held")
                .netznutzung
                .expect("a row covers the Stichtag")
        }
    };
    let n = read(date!(2026 - 12 - 31)).await;
    assert_eq!(n.turnusablesung.as_deref(), Some("05040504"));
    assert_eq!(
        n.netznutzungsvertrag,
        Some(Netznutzungsvertrag::LieferantNb)
    );
    assert_eq!(n.netznutzung_zahler, Some(NetznutzungZahler::Lieferant));
    // Half-open: the first row's valid_to is the second row's first day.
    let n = read(date!(2027 - 01 - 01)).await;
    assert_eq!(n.turnusablesung.as_deref(), Some("0915"));
    assert_eq!(n.netznutzungsvertrag, Some(Netznutzungsvertrag::KundeNb));
    assert_eq!(n.netznutzung_zahler, Some(NetznutzungZahler::Kunde));

    for (what, row) in [
        ("29.02.", {
            let mut r = netznutzung(date!(2028 - 01 - 01));
            r.turnusablesung = Some("0229".to_owned());
            r
        }),
        ("a fifth week", {
            let mut r = netznutzung(date!(2028 - 01 - 01));
            r.turnusablesung = Some("05050505".to_owned());
            r
        }),
        ("a Kunde paying under a Lieferant's Vertrag", {
            let mut r = netznutzung(date!(2028 - 01 - 01));
            r.netznutzungsvertrag = Some(Netznutzungsvertrag::LieferantNb);
            r.netznutzung_zahler = Some(NetznutzungZahler::Kunde);
            r
        }),
    ] {
        let mut row = row;
        row.valid_from = date!(2030 - 01 - 01);
        let err = repo
            .upsert_netznutzung(TENANT, MALO, &row)
            .await
            .expect_err(what);
        assert!(
            matches!(err, MdmError::Unprocessable { .. }),
            "{what}: {err:?}"
        );
    }
}
