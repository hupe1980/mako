//! Real-PostgreSQL guards for the MaLo ↔ MeLo single-write-path invariant and
//! the `lokationsbuendelcode` / `lokationsbuendel_objektcode` typed columns.
//!
//! The MeLo PUT is the only writer of the `melo.malo_id` FK; the repository
//! maintains the corresponding `melo → malo` edge in the temporal
//! `lokationszuordnungen` graph in the same transaction, so FK and graph can
//! never contradict.
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! just test-marktd-db
//! ```

use mako_markt::{
    domain::{MaloId, MeloId, Sparte},
    repository::{
        LokationszuordnungRepository as _, MaloRepository as _, MeloRepository as _,
        MeloStammdatenPatch, NeLoRecord, NeLoRepository as _, NeloStammdatenPatch, TrancheRecord,
        TrancheRepository as _, TrancheStammdatenPatch,
    },
};
use marktd::pg::{
    PgLokationszuordnungRepository, PgMaloRepository, PgMeloRepository, PgNeLoRepository,
    PgTrancheRepository,
};
use rubo4e::current::Lokationstyp;
use sqlx::PgPool;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const MALO_A: &str = "51238696012";
const MALO_B: &str = "10001234558";
const MELO: &str = "DE0001234567890123456789012345678";

async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

fn malo_id(s: &str) -> MaloId {
    s.parse().expect("valid MaLo-ID")
}

/// Build a typed `Messlokation` from a partial BO4E payload.
fn bo4e_melo(mut body: serde_json::Value) -> rubo4e::current::Messlokation {
    body["_typ"] = serde_json::json!("MESSLOKATION");
    serde_json::from_value(body).expect("valid BO4E Messlokation")
}

/// Build a typed `Marktlokation` from a partial BO4E payload.
fn bo4e_malo(mut body: serde_json::Value) -> rubo4e::current::Marktlokation {
    body["_typ"] = serde_json::json!("MARKTLOKATION");
    serde_json::from_value(body).expect("valid BO4E Marktlokation")
}

fn melo_id(s: &str) -> MeloId {
    s.parse().expect("valid MeLo-ID")
}

/// Seed the two parent MaLos the FK references.
async fn seed_malos(pool: &PgPool) {
    let repo = PgMaloRepository::new(pool.clone());
    for id in [MALO_A, MALO_B] {
        repo.upsert(
            &malo_id(id),
            Sparte::Strom,
            &bo4e_malo(serde_json::json!({ "marktlokationsId": id })),
            vec![],
            None,
            "v202607.0.0",
        )
        .await
        .expect("seed malo");
    }
}

/// Open (valid_to IS NULL) melo→malo edges from the MeLo, as (nach_id, valid_from).
async fn open_parent_edges(
    lz: &PgLokationszuordnungRepository,
) -> Vec<(String, Option<time::Date>)> {
    lz.list_edges_from(TENANT, MELO, None)
        .await
        .expect("list edges")
        .into_iter()
        .filter(|e| {
            e.nach_typ == Lokationstyp::Malo
                && e.von_typ == Lokationstyp::Melo
                && e.valid_to.is_none()
        })
        .map(|e| (e.nach_id, e.valid_from))
        .collect()
}

/// PUT + reparent + unparent: `melo.malo_id` (FK) and the temporal graph agree
/// after every write, the previous edge is closed on reparenting, and repeated
/// PUTs with the same parent do not duplicate edges.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn melo_put_keeps_fk_and_graph_in_agreement() {
    let Some((pool, _pg)) = test_pool("fk_graph").await else {
        return;
    };
    seed_malos(&pool).await;
    let melo_repo = PgMeloRepository::new(pool.clone(), TENANT);
    let lz = PgLokationszuordnungRepository::new(pool.clone());
    let today = time::OffsetDateTime::now_utc().date();
    let data = bo4e_melo(serde_json::json!({ "messlokationsId": MELO }));

    // 1. Initial PUT with parent A.
    melo_repo
        .upsert(
            &melo_id(MELO),
            Some(&malo_id(MALO_A)),
            &data,
            None,
            "v202607.0.0",
        )
        .await
        .expect("initial PUT");
    let rec = melo_repo
        .find(&melo_id(MELO))
        .await
        .unwrap()
        .expect("melo stored");
    assert_eq!(
        rec.malo_id.as_ref().map(ToString::to_string),
        Some(MALO_A.to_owned())
    );
    assert_eq!(
        open_parent_edges(&lz).await,
        vec![(MALO_A.to_owned(), Some(today))],
        "graph must carry exactly one open edge to the FK parent"
    );

    // 2. Idempotent re-PUT with the same parent: no duplicate edge.
    melo_repo
        .upsert(
            &melo_id(MELO),
            Some(&malo_id(MALO_A)),
            &data,
            None,
            "v202607.0.0",
        )
        .await
        .expect("re-PUT");
    assert_eq!(
        open_parent_edges(&lz).await.len(),
        1,
        "re-PUT must not duplicate the edge"
    );

    // 3. Reparent A → B: FK moves, edge to A is closed, edge to B opens.
    melo_repo
        .upsert(
            &melo_id(MELO),
            Some(&malo_id(MALO_B)),
            &data,
            None,
            "v202607.0.0",
        )
        .await
        .expect("reparent PUT");
    let rec = melo_repo
        .find(&melo_id(MELO))
        .await
        .unwrap()
        .expect("melo stored");
    assert_eq!(
        rec.malo_id.as_ref().map(ToString::to_string),
        Some(MALO_B.to_owned())
    );
    assert_eq!(
        open_parent_edges(&lz).await,
        vec![(MALO_B.to_owned(), Some(today))],
        "after reparenting only the new parent edge is open"
    );
    let all_edges = lz.list_edges_from(TENANT, MELO, None).await.unwrap();
    let closed_a = all_edges
        .iter()
        .find(|e| e.nach_id == MALO_A)
        .expect("history to A survives");
    assert_eq!(
        closed_a.valid_to,
        Some(today),
        "previous edge is closed, not deleted"
    );

    // 4. Same-day reparent back to A reopens the closed edge (dated upsert).
    melo_repo
        .upsert(
            &melo_id(MELO),
            Some(&malo_id(MALO_A)),
            &data,
            None,
            "v202607.0.0",
        )
        .await
        .expect("reparent back");
    assert_eq!(
        open_parent_edges(&lz).await,
        vec![(MALO_A.to_owned(), Some(today))]
    );

    // 5. Unparent: FK NULL, no open melo→malo edges remain.
    melo_repo
        .upsert(&melo_id(MELO), None, &data, None, "v202607.0.0")
        .await
        .expect("unparent PUT");
    let rec = melo_repo
        .find(&melo_id(MELO))
        .await
        .unwrap()
        .expect("melo stored");
    assert!(rec.malo_id.is_none(), "FK cleared");
    assert!(
        open_parent_edges(&lz).await.is_empty(),
        "no open parent edge after unparenting"
    );
}

/// The graph write path respects a pre-existing open-ended edge written via the
/// graph API for the same pair — the MeLo PUT does not create a duplicate.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn melo_put_respects_existing_open_ended_edge() {
    let Some((pool, _pg)) = test_pool("open_ended").await else {
        return;
    };
    seed_malos(&pool).await;
    let melo_repo = PgMeloRepository::new(pool.clone(), TENANT);
    let lz = PgLokationszuordnungRepository::new(pool.clone());

    // Graph API writes an open-ended (valid_from = NULL) edge first.
    lz.upsert_edge(
        TENANT,
        MELO,
        Lokationstyp::Melo,
        MALO_A,
        Lokationstyp::Malo,
        None,
        None,
        serde_json::json!({ "_typ": "LOKATIONSZUORDNUNG" }),
    )
    .await
    .expect("graph API edge");

    melo_repo
        .upsert(
            &melo_id(MELO),
            Some(&malo_id(MALO_A)),
            &bo4e_melo(serde_json::json!({})),
            None,
            "v202607.0.0",
        )
        .await
        .expect("PUT");

    let open = open_parent_edges(&lz).await;
    assert_eq!(
        open.len(),
        1,
        "existing open-ended edge is reused, not duplicated"
    );
    assert_eq!(open[0].0, MALO_A);
    assert_eq!(
        open[0].1, None,
        "the open-ended edge keeps its NULL valid_from"
    );
}

/// Task 5: `lokationsbuendelcode` is extracted from the BO4E edge payload into
/// its typed column and returned by the graph API; the MaLo/MeLo
/// `lokationsbuendelObjektcode` payload fields land in their typed columns.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn lokationsbuendel_codes_are_extracted_into_typed_columns() {
    let Some((pool, _pg)) = test_pool("buendel").await else {
        return;
    };
    let malo_repo = PgMaloRepository::new(pool.clone());
    let melo_repo = PgMeloRepository::new(pool.clone(), TENANT);
    let lz = PgLokationszuordnungRepository::new(pool.clone());

    // MaLo: data.lokationsbuendelObjektcode → typed column.
    malo_repo
        .upsert(
            &malo_id(MALO_A),
            Sparte::Strom,
            &bo4e_malo(serde_json::json!({
                "lokationsbuendelObjektcode": "9992000000125"
            })),
            vec![],
            None,
            "v202607.0.0",
        )
        .await
        .expect("malo upsert");
    let malo = malo_repo
        .find(&malo_id(MALO_A), time::OffsetDateTime::now_utc().date())
        .await
        .unwrap()
        .expect("malo stored");
    assert_eq!(
        malo.lokationsbuendel_objektcode.as_deref(),
        Some("9992000000125")
    );

    // MeLo: data.lokationsbuendelObjektcode → typed column.
    melo_repo
        .upsert(
            &melo_id(MELO),
            Some(&malo_id(MALO_A)),
            &bo4e_melo(serde_json::json!({
                "lokationsbuendelObjektcode": "9992000000125"
            })),
            None,
            "v202607.0.0",
        )
        .await
        .expect("melo upsert");
    let melo = melo_repo
        .find(&melo_id(MELO))
        .await
        .unwrap()
        .expect("melo stored");
    assert_eq!(
        melo.lokationsbuendel_objektcode.as_deref(),
        Some("9992000000125")
    );

    // Edge: data.lokationsbuendelcode → typed column, returned by the graph API.
    lz.upsert_edge(
        TENANT,
        MALO_A,
        Lokationstyp::Malo,
        MELO,
        Lokationstyp::Melo,
        None,
        None,
        serde_json::json!({
            "_typ": "LOKATIONSZUORDNUNG",
            "lokationsbuendelcode": "9992000000125"
        }),
    )
    .await
    .expect("edge upsert");
    let edges = lz.find_graph(TENANT, MALO_A, None).await.expect("graph");
    let edge = edges
        .iter()
        .find(|e| e.von_id == MALO_A && e.nach_id == MELO)
        .expect("edge present");
    assert_eq!(edge.lokationsbuendelcode.as_deref(), Some("9992000000125"));
    // JSONB fidelity: the full payload survives alongside the typed column.
    assert_eq!(
        edge.data
            .get("lokationsbuendelcode")
            .and_then(|v| v.as_str()),
        Some("9992000000125")
    );

    // An edge without the field yields NULL, not an error.
    let melo_edges = lz.list_edges_from(TENANT, MELO, None).await.expect("edges");
    let plain = melo_edges
        .iter()
        .find(|e| e.nach_id == MALO_A)
        .expect("melo-put edge present");
    assert_eq!(plain.lokationsbuendelcode, None);
}

// ── Stammdatenänderung: patch_stammdaten over the typed MaLo columns ──────────

/// A UTILMD Stammdatenänderung (GPKE Teil 4 / GeLi Gas) patches only the typed
/// MaLo columns, leaving the BO4E JSONB payload and the `version` untouched,
/// and no-ops when the MaLo is not yet known.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn patch_stammdaten_updates_typed_columns_only() {
    use mako_markt::repository::MaloStammdatenPatch;

    let Some((pool, _pg)) = test_pool("patch_stammdaten").await else {
        return;
    };
    let repo = PgMaloRepository::new(pool.clone());
    let m = malo_id(MALO_A);

    // No row yet → the change is a no-op, not an error.
    let applied = repo
        .patch_stammdaten(
            &m,
            &MaloStammdatenPatch {
                netzebene: Some("NSP".to_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("patch on absent MaLo");
    assert!(!applied, "no row → no-op");

    // Seed the MaLo with a BO4E payload (version 1).
    repo.upsert(
        &m,
        Sparte::Strom,
        &bo4e_malo(serde_json::json!({
            "marktlokationsId": MALO_A,
            "bilanzierungsmethode": "SLP"
        })),
        vec![],
        None,
        "v202607.0.0",
    )
    .await
    .expect("seed malo");
    let before = repo
        .find(&m, time::OffsetDateTime::now_utc().date())
        .await
        .unwrap()
        .unwrap();

    // Apply a change to several typed columns, incl. the §14a EnWG
    // Fernsteuerbarkeit (UTILMD CCI+7037 Z97/Z96 → bool).
    let applied = repo
        .patch_stammdaten(
            &m,
            &MaloStammdatenPatch {
                netzebene: Some("MSP".to_owned()),
                bilanzierungsmethode: Some("RLM".to_owned()),
                regelzone: Some("10YDE-EON------1".to_owned()),
                fernsteuerbar: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("patch existing MaLo");
    assert!(applied, "row present → applied");

    let after = repo
        .find(&m, time::OffsetDateTime::now_utc().date())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.netzebene.as_deref(), Some("MSP"));
    assert_eq!(after.bilanzierungsmethode.as_deref(), Some("RLM"));
    assert_eq!(after.regelzone.as_deref(), Some("10YDE-EON------1"));
    assert_eq!(
        after.fernsteuerbar,
        Some(true),
        "§14a Fernsteuerbarkeit applied"
    );
    // gasqualitaet was not in the patch → unchanged (COALESCE).
    assert_eq!(after.gasqualitaet, before.gasqualitaet);
    // The optimistic version is NOT bumped by a Stammdatenänderung.
    assert_eq!(after.version, before.version, "version untouched");
}

/// A `LOC+Z17` Stammdatenänderung patches the typed MeLo columns
/// (`netzebene_messung`, `regelzone`) via `MeloRepository::patch_stammdaten`,
/// leaving the JSONB payload and version untouched, and no-ops when unknown.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn melo_patch_stammdaten_updates_typed_columns_only() {
    let Some((pool, _pg)) = test_pool("melo_patch").await else {
        return;
    };
    let repo = PgMeloRepository::new(pool.clone(), TENANT);
    let m = melo_id(MELO);

    // No row yet → no-op.
    let applied = repo
        .patch_stammdaten(
            &m,
            &MeloStammdatenPatch {
                netzebene_messung: Some("MSP".to_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("patch absent MeLo");
    assert!(!applied, "no row → no-op");

    repo.upsert(
        &m,
        None,
        &bo4e_melo(serde_json::json!({ "messlokationsId": MELO })),
        None,
        "v202607.0.0",
    )
    .await
    .expect("seed melo");
    let before = repo.find(&m).await.unwrap().unwrap();

    let applied = repo
        .patch_stammdaten(
            &m,
            &MeloStammdatenPatch {
                netzebene_messung: Some("NSP".to_owned()),
                regelzone: Some("10YDE-EON------1".to_owned()),
            },
        )
        .await
        .expect("patch existing MeLo");
    assert!(applied);

    let after = repo.find(&m).await.unwrap().unwrap();
    assert_eq!(after.netzebene_messung.as_deref(), Some("NSP"));
    assert_eq!(after.regelzone.as_deref(), Some("10YDE-EON------1"));
    assert_eq!(after.version, before.version, "version untouched");
}

/// A `LOC+Z18` Stammdatenänderung patches the typed NeLo Netzebene via
/// `NeLoRepository::patch_stammdaten`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn nelo_patch_stammdaten_updates_netzebene() {
    let Some((pool, _pg)) = test_pool("nelo_patch").await else {
        return;
    };
    let repo = PgNeLoRepository::new(pool.clone());
    const NELO: &str = "11XNELO-EXAMPL1";

    // No row yet → no-op.
    let applied = repo
        .patch_stammdaten(
            NELO,
            TENANT,
            &NeloStammdatenPatch {
                netzebene: Some("HSP".to_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("patch absent NeLo");
    assert!(!applied, "no row → no-op");

    repo.upsert(
        NeLoRecord {
            nelo_id: NELO.to_owned(),
            tenant: TENANT.to_owned(),
            name: None,
            sparte: Sparte::Strom,
            netzebene: Some("MSP".to_owned()),
            nb_mp_id: TENANT.to_owned(),
            steuerkanal: None,
            eigenschaft_msb_lokation: None,
            grundzustaendiger_msb_codenr: None,
            data: serde_json::json!({}),
            version: 0,
            updated_at: time::OffsetDateTime::now_utc(),
        },
        None,
    )
    .await
    .expect("seed nelo");
    let before = repo.find(NELO, TENANT).await.unwrap().unwrap();

    // Patch Netzebene + the §14a Steuerkanal (UTILMD CCI+7059=Z49 ZF3/ZF2).
    let applied = repo
        .patch_stammdaten(
            NELO,
            TENANT,
            &NeloStammdatenPatch {
                netzebene: Some("HSP".to_owned()),
                steuerkanal: Some(true),
            },
        )
        .await
        .expect("patch existing NeLo");
    assert!(applied);

    let after = repo.find(NELO, TENANT).await.unwrap().unwrap();
    assert_eq!(after.netzebene.as_deref(), Some("HSP"));
    assert_eq!(after.steuerkanal, Some(true), "§14a Steuerkanal applied");
    // patch_stammdaten does not bump the optimistic version.
    assert_eq!(after.version, before.version, "version untouched");
}

/// The MeLo Stammdatenänderung's MSB-Zuordnung lands on the dated
/// `melo_msb_zuordnungen` timeline via `MeloMsbRepository::assign_msb` — the path
/// the `MESSLOKATION` apply writes to. A later assignment closes the earlier one.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn melo_msb_zuordnung_from_stammdatenaenderung() {
    use mako_markt::repository::MeloMsbRepository as _;
    use marktd::pg::PgMeloMsbRepository;

    let Some((pool, _pg)) = test_pool("melo_msb").await else {
        return;
    };
    // The melo_msb_zuordnungen FK requires the MeLo row to exist (the apply
    // path degrades to a non-fatal warn when it does not).
    PgMeloRepository::new(pool.clone(), TENANT)
        .upsert(
            &melo_id(MELO),
            None,
            &bo4e_melo(serde_json::json!({ "messlokationsId": MELO })),
            None,
            "v202607.0.0",
        )
        .await
        .expect("seed melo");

    let repo = PgMeloMsbRepository::new(pool.clone());
    let d = |s: &str| {
        time::Date::parse(s, &time::macros::format_description!("[year][month][day]")).unwrap()
    };

    // First MSB assignment effective the Änderungsdatum.
    repo.assign_msb(TENANT, MELO, "9900111111111", d("20260101"))
        .await
        .expect("assign msb 1");
    assert_eq!(
        repo.find_msb_at(TENANT, MELO, d("20260201"))
            .await
            .unwrap()
            .as_deref(),
        Some("9900111111111")
    );

    // A later Stammdatenänderung switches the serving MSB.
    repo.assign_msb(TENANT, MELO, "9900222222222", d("20260601"))
        .await
        .expect("assign msb 2");
    assert_eq!(
        repo.find_msb_at(TENANT, MELO, d("20260701"))
            .await
            .unwrap()
            .as_deref(),
        Some("9900222222222"),
        "current MSB is the latest assignment"
    );
    assert_eq!(
        repo.find_msb_at(TENANT, MELO, d("20260301"))
            .await
            .unwrap()
            .as_deref(),
        Some("9900111111111"),
        "history: the earlier MSB is still resolvable point-in-time"
    );
}

/// A `LOC+Z20` Stammdatenänderung patches the TR Fernschaltbarkeit via
/// `TechnischeRessourceRepository::patch_stammdaten` (UTILMD CAV Z58 + Z06/Z07).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn tr_patch_stammdaten_updates_fernschaltbarkeit() {
    use mako_markt::repository::{
        TechnischeRessourceRepository as _, TechnischeRessourceStammdatenPatch,
    };
    use marktd::pg::PgTechnischeRessourceRepository;

    let Some((pool, _pg)) = test_pool("tr_patch").await else {
        return;
    };
    let repo = PgTechnischeRessourceRepository::new(pool.clone());
    const TR: &str = "TR000000000001";

    // No row yet → no-op.
    let applied = repo
        .patch_stammdaten(
            TR,
            TENANT,
            &TechnischeRessourceStammdatenPatch {
                ist_fernschaltbar: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("patch absent TR");
    assert!(!applied, "no row → no-op");

    repo.upsert_tr(
        TR,
        TENANT,
        Some(MALO_A),
        None,
        Some("STROMERZEUGUNGSART"),
        None,
        Some(false),
        serde_json::json!({ "_typ": "TECHNISCHERESSOURCE" }),
        "v202607.0.0",
    )
    .await
    .expect("seed tr");

    // First patch: only Fernschaltbarkeit — nutzung/verbrauchsart absent → COALESCE keeps them.
    let applied = repo
        .patch_stammdaten(
            TR,
            TENANT,
            &TechnischeRessourceStammdatenPatch {
                ist_fernschaltbar: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("patch existing TR");
    assert!(applied);

    let after = repo.find_tr(TR, TENANT).await.unwrap().unwrap();
    assert_eq!(
        after.ist_fernschaltbar,
        Some(true),
        "Fernschaltbarkeit applied"
    );
    // nutzung was not in the patch → unchanged (COALESCE); verbrauchsart still NULL.
    assert_eq!(after.nutzung.as_deref(), Some("STROMERZEUGUNGSART"));
    assert_eq!(after.verbrauchsart, None);

    // Second patch: apply the BO4E-aligned nutzung + verbrauchsart.
    repo.patch_stammdaten(
        TR,
        TENANT,
        &TechnischeRessourceStammdatenPatch {
            nutzung: Some("STROMVERBRAUCHSART".to_owned()),
            verbrauchsart: Some("E_MOBILITAET".to_owned()),
            ..Default::default()
        },
    )
    .await
    .expect("patch nutzung/verbrauchsart");
    let after = repo.find_tr(TR, TENANT).await.unwrap().unwrap();
    assert_eq!(after.nutzung.as_deref(), Some("STROMVERBRAUCHSART"));
    assert_eq!(after.verbrauchsart.as_deref(), Some("E_MOBILITAET"));
    assert_eq!(
        after.ist_fernschaltbar,
        Some(true),
        "Fernschaltbarkeit retained"
    );
}

/// A `LOC+Z19` SR Stammdatenänderung replaces the contracted
/// `konfigurationsprodukte` (BO4E `Vec<Konfigurationsprodukt>`) via
/// `SteuerbareRessourceRepository::replace_sr_konfigurationsprodukte`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn sr_konfigurationsprodukte_replace() {
    use mako_markt::repository::SteuerbareRessourceRepository as _;
    use marktd::pg::PgSteuerbareRessourceRepository;

    let Some((pool, _pg)) = test_pool("sr_konfig").await else {
        return;
    };
    let repo = PgSteuerbareRessourceRepository::new(pool.clone());
    const SR: &str = "SR000000000001";

    // Unknown SR → no-op (404 signal).
    let applied = repo
        .replace_sr_konfigurationsprodukte(SR, TENANT, serde_json::json!([]))
        .await
        .expect("replace absent SR");
    assert!(!applied, "no row → no-op");

    // Seed the SR with no products.
    repo.upsert_sr(
        SR,
        TENANT,
        Some(MALO_A),
        None,
        serde_json::json!({ "_typ": "STEUERBARERESSOURCE" }),
        "v202607.0.0",
        None,
    )
    .await
    .expect("seed sr");

    // Apply the extracted per-product array (produktcode + marktpartner).
    let kp = serde_json::json!([
        {
            "_typ": "KONFIGURATIONSPRODUKT",
            "produktcode": "PRODUKT_A",
            "marktpartner": { "_typ": "MARKTTEILNEHMER", "rollencodenummer": "9900123456789" },
            "leistungskurvendefinition": "LK001"
        }
    ]);
    let applied = repo
        .replace_sr_konfigurationsprodukte(SR, TENANT, kp.clone())
        .await
        .expect("replace existing SR");
    assert!(applied);

    let after = repo.find_sr(SR, TENANT).await.unwrap().unwrap();
    let stored = after
        .konfigurationsprodukte
        .expect("konfigurationsprodukte set");
    let arr = stored.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["produktcode"], "PRODUKT_A");
    assert_eq!(arr[0]["marktpartner"]["rollencodenummer"], "9900123456789");
}

/// A `LOC+Z21` Stammdatenänderung patches the typed Tranche columns via
/// `TrancheRepository::patch_stammdaten`; the greenfield table round-trips.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn tranche_patch_stammdaten_updates_typed_columns() {
    let Some((pool, _pg)) = test_pool("tranche_patch").await else {
        return;
    };
    let repo = PgTrancheRepository::new(pool.clone());
    let tranche_id = format!("{MALO_A}-T01");

    // No row yet → no-op.
    let applied = repo
        .patch_stammdaten(
            &tranche_id,
            TENANT,
            &TrancheStammdatenPatch {
                netzebene: Some("MSP".to_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("patch absent Tranche");
    assert!(!applied, "no row → no-op");

    repo.upsert(
        TrancheRecord {
            tranche_id: tranche_id.clone(),
            tenant: TENANT.to_owned(),
            malo_id: Some(MALO_A.to_owned()),
            bilanzierungsgebiet: None,
            netzebene: None,
            energierichtung: None,
            data: serde_json::json!({ "_typ": "TRANCHE" }),
            version: 0,
            updated_at: time::OffsetDateTime::now_utc(),
        },
        None,
    )
    .await
    .expect("seed tranche");
    let before = repo.find(&tranche_id, TENANT).await.unwrap().unwrap();

    let applied = repo
        .patch_stammdaten(
            &tranche_id,
            TENANT,
            &TrancheStammdatenPatch {
                bilanzierungsgebiet: Some("11YW-EXAMPLE-BGG".to_owned()),
                netzebene: Some("NSP".to_owned()),
                energierichtung: Some("AUSSP".to_owned()),
            },
        )
        .await
        .expect("patch existing Tranche");
    assert!(applied);

    let after = repo.find(&tranche_id, TENANT).await.unwrap().unwrap();
    assert_eq!(
        after.bilanzierungsgebiet.as_deref(),
        Some("11YW-EXAMPLE-BGG")
    );
    assert_eq!(after.netzebene.as_deref(), Some("NSP"));
    assert_eq!(after.energierichtung.as_deref(), Some("AUSSP"));
    // list_by_malo groups Tranchen under their parent MaLo.
    let listed = repo.list_by_malo(MALO_A, TENANT, 0, 10).await.unwrap();
    assert_eq!(listed.total, 1);
    assert_eq!(before.version, 1);
}
/// The Postgres container guard a test holds until it ends — dropping it removes
/// the container (testcontainers cleans up on `Drop`; no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// Start a fresh throwaway `postgres:17-alpine` and return its URL plus the
/// container guard. `None` when Docker is unavailable (tests skip gracefully).
async fn pg_container() -> Option<(String, PgContainer)> {
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
    Some((url, container))
}
