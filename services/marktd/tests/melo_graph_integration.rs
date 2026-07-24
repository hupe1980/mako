//! Real-PostgreSQL guards for the MaLo ↔ MeLo single-write-path invariant and
//! the `lokationsbuendelcode` / `lokationsbuendel_objektcode` typed columns.
//!
//! The MeLo PUT is the only writer of the `melo.malo_id` FK; the repository
//! maintains the corresponding `melo → malo` edge in the temporal
//! `lokationszuordnungen` graph in the same transaction, so FK and graph can
//! never contradict.
//!
//! ```bash
//! docker run -d --name marktd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=marktd -p 55438:5432 postgres:17-alpine
//! export MARKTD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55438/marktd"
//! cargo test -p marktd --test melo_graph_integration -- --include-ignored
//! ```

use mako_markt::{
    domain::{MaloId, MeloId, Sparte},
    repository::{LokationszuordnungRepository as _, MaloRepository as _, MeloRepository as _},
};
use marktd::pg::{PgLokationszuordnungRepository, PgMaloRepository, PgMeloRepository};
use sqlx::PgPool;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const MALO_A: &str = "51238696780";
const MALO_B: &str = "10001234567";
const MELO: &str = "DE0001234567890123456789012345678";

async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("MARKTD_TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;
    let schema = format!("melo_graph_{test_name}");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("drop schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    admin.close().await;
    let opts: sqlx::postgres::PgConnectOptions = base.parse().expect("parse url");
    let pool = PgPool::connect_with(opts.options([("search_path", schema.as_str())]))
        .await
        .expect("connect schema");
    let stripped: String = SCHEMA
        .lines()
        .map(|l| l.split_once("--").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    for stmt in stripped.split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        sqlx::query(s)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("schema stmt failed: {e}\n{s}"));
    }
    Some(pool)
}

fn malo_id(s: &str) -> MaloId {
    s.parse().expect("valid MaLo-ID")
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
            serde_json::json!({ "_typ": "MARKTLOKATION", "marktlokationsId": id }),
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
        .filter(|e| e.nach_typ == "malo" && e.von_typ == "melo" && e.valid_to.is_none())
        .map(|e| (e.nach_id, e.valid_from))
        .collect()
}

/// PUT + reparent + unparent: `melo.malo_id` (FK) and the temporal graph agree
/// after every write, the previous edge is closed on reparenting, and repeated
/// PUTs with the same parent do not duplicate edges.
#[tokio::test]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn melo_put_keeps_fk_and_graph_in_agreement() {
    let Some(pool) = test_pool("fk_graph").await else {
        return;
    };
    seed_malos(&pool).await;
    let melo_repo = PgMeloRepository::new(pool.clone(), TENANT);
    let lz = PgLokationszuordnungRepository::new(pool.clone());
    let today = time::OffsetDateTime::now_utc().date();
    let data = serde_json::json!({ "_typ": "MESSLOKATION", "messlokationsId": MELO });

    // 1. Initial PUT with parent A.
    melo_repo
        .upsert(
            &melo_id(MELO),
            Some(&malo_id(MALO_A)),
            data.clone(),
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
            data.clone(),
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
            data.clone(),
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
            data.clone(),
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
        .upsert(&melo_id(MELO), None, data, None, "v202607.0.0")
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
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn melo_put_respects_existing_open_ended_edge() {
    let Some(pool) = test_pool("open_ended").await else {
        return;
    };
    seed_malos(&pool).await;
    let melo_repo = PgMeloRepository::new(pool.clone(), TENANT);
    let lz = PgLokationszuordnungRepository::new(pool.clone());

    // Graph API writes an open-ended (valid_from = NULL) edge first.
    lz.upsert_edge(
        TENANT,
        MELO,
        "melo",
        MALO_A,
        "malo",
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
            serde_json::json!({ "_typ": "MESSLOKATION" }),
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
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn lokationsbuendel_codes_are_extracted_into_typed_columns() {
    let Some(pool) = test_pool("buendel").await else {
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
            serde_json::json!({
                "_typ": "MARKTLOKATION",
                "lokationsbuendelObjektcode": "9992000000125"
            }),
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
            serde_json::json!({
                "_typ": "MESSLOKATION",
                "lokationsbuendelObjektcode": "9992000000125"
            }),
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
        "malo",
        MELO,
        "melo",
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
