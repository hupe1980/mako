//! Real-PostgreSQL guards on the temporal invariants of the master data.
//!
//! Every "who was responsible on this date" question this hub answers —
//! which Netzbetreiber, which Messstellenbetreiber, which price sheet — is a
//! read that filters rows by a validity window. If two rows can cover the same
//! date, the query has two answers and returns whichever the planner reached
//! first. That is not a query bug: it is a missing constraint, and it is
//! invisible until a settlement is computed against the wrong tariff or an
//! Anmeldung is addressed to the wrong operator.
//!
//! These tests pin that the database refuses the overlap outright.
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! just test-marktd-db
//! ```

use sqlx::PgPool;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");

async fn test_pool() -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

/// Seed a MaLo so the `rollenzuordnungen` foreign key is satisfiable.
async fn seed_malo(pool: &PgPool, malo_id: &str) {
    sqlx::query("INSERT INTO malo (malo_id, sparte, data) VALUES ($1, 'STROM', '{}'::jsonb)")
        .bind(malo_id)
        .execute(pool)
        .await
        .expect("seed malo");
}

#[tokio::test]
async fn a_malo_cannot_have_two_netzbetreiber_on_the_same_day() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let malo = "51238696012";
    seed_malo(&pool, malo).await;

    let insert = |from: &'static str, to: Option<&'static str>, gln: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO rollenzuordnungen
                     (malo_id, zuordnungstyp, rollencodenummer, valid_from, valid_to)
                 VALUES ($1, 'NB', $2, $3::date, $4::date)",
            )
            .bind(malo)
            .bind(gln)
            .bind(from)
            .bind(to)
            .execute(&pool)
            .await
        }
    };

    insert("2025-01-01", Some("2026-01-01"), "9900000000001")
        .await
        .expect("the first assignment is accepted");

    // Abuts exactly: [2025-01-01, 2026-01-01) then [2026-01-01, ∞). Half-open
    // windows must let a successor start on the day the predecessor ends.
    insert("2026-01-01", None, "9900000000002")
        .await
        .expect("an abutting successor is not an overlap");

    // Starts before the open-ended incumbent ends — the case that used to leave
    // `GET /api/v1/malos/{id}` returning two Netzbetreiber for one MaLo.
    let err = insert("2026-06-01", None, "9900000000003")
        .await
        .expect_err("an overlapping assignment must be refused");
    assert!(
        err.to_string().contains("rollenzuordnungen_no_overlap"),
        "expected the exclusion constraint to reject it, got: {err}"
    );

    // A different role in the same window is fine — one NB and one MSB at once
    // is the normal case.
    sqlx::query(
        "INSERT INTO rollenzuordnungen
             (malo_id, zuordnungstyp, rollencodenummer, valid_from, valid_to)
         VALUES ($1, 'MSB', '9900000000004', '2026-06-01'::date, NULL)",
    )
    .bind(malo)
    .execute(&pool)
    .await
    .expect("a different Zuordnungstyp may overlap");
}

#[tokio::test]
async fn a_melo_cannot_have_two_messstellenbetreiber_on_the_same_day() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let melo = "DE0001112223334445556667778889990";
    sqlx::query("INSERT INTO melo (melo_id, data) VALUES ($1, '{}'::jsonb)")
        .bind(melo)
        .execute(&pool)
        .await
        .expect("seed melo");

    sqlx::query(
        "INSERT INTO melo_msb_zuordnungen (tenant, melo_id, msb_mp_id, valid_from, valid_to)
         VALUES ('9900357000004', $1, '9900000000011', '2025-06-01', NULL)",
    )
    .bind(melo)
    .execute(&pool)
    .await
    .expect("the current assignment is accepted");

    // A late-arriving backdated correction that does not close the incumbent
    // would make `find_msb_at` depend on row order — the WiM Teil 2 UC 4.1.1
    // historical Werteanfrage would route to whichever MSB came back first.
    let err = sqlx::query(
        "INSERT INTO melo_msb_zuordnungen (tenant, melo_id, msb_mp_id, valid_from, valid_to)
         VALUES ('9900357000004', $1, '9900000000010', '2024-01-01', NULL)",
    )
    .bind(melo)
    .execute(&pool)
    .await
    .expect_err("an unclosed backdated assignment must be refused");
    assert!(
        err.to_string().contains("melo_msb_no_overlap"),
        "expected the exclusion constraint to reject it, got: {err}"
    );
}

#[tokio::test]
async fn a_netzbetreiber_cannot_publish_two_price_sheets_valid_on_one_day() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };

    sqlx::query(
        "INSERT INTO preisblaetter (nb_mp_id, valid_from, valid_to, data)
         VALUES ('9900000000001', '2026-01-01', '2027-01-01', '{}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("the first sheet is accepted");

    let err = sqlx::query(
        "INSERT INTO preisblaetter (nb_mp_id, valid_from, valid_to, data)
         VALUES ('9900000000001', '2026-07-01', NULL, '{}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect_err("an overlapping price sheet must be refused");
    assert!(
        err.to_string().contains("preisblaetter_no_overlap"),
        "expected the exclusion constraint to reject it, got: {err}"
    );

    // A different NB in the same window is a different tariff, not a conflict.
    sqlx::query(
        "INSERT INTO preisblaetter (nb_mp_id, valid_from, valid_to, data)
         VALUES ('9900000000002', '2026-07-01', NULL, '{}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("another NB's sheet is unaffected");
}

#[tokio::test]
async fn an_open_started_price_sheet_exists_at_most_once_per_party() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };

    sqlx::query(
        "INSERT INTO preisblaetter (nb_mp_id, valid_from, data)
         VALUES ('9900000000001', NULL, '{\"v\":1}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("the open-started sheet is accepted");

    // Under the default NULLS DISTINCT this second insert succeeded, and the
    // point-in-time read (`ORDER BY valid_from DESC NULLS LAST LIMIT 1`) then
    // returned an arbitrary one of the two.
    let err = sqlx::query(
        "INSERT INTO preisblaetter (nb_mp_id, valid_from, data)
         VALUES ('9900000000001', NULL, '{\"v\":2}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect_err("a second open-started sheet must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("preisblaetter_nb_mp_id_valid_from_key")
            || msg.contains("preisblaetter_no_overlap"),
        "expected a uniqueness/overlap rejection, got: {msg}"
    );
}

#[tokio::test]
async fn a_tariff_window_must_be_a_real_interval() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO zaehler_register
             (id, zaehler_id, tenant, bezeichnung, zaehlerauspraegung, einheit, valid_from)
         VALUES ('00000000-0000-0000-0000-000000000001', 'Z1', '9900357000004',
                 'HT', 'HT', 'KWH', '2026-01-01')",
    )
    .execute(&pool)
    .await
    .expect("seed register");

    // An inverted window classifies no reading at all, so HT energy would go
    // missing rather than be mis-priced — a silent shortfall on the invoice.
    let err = sqlx::query(
        "INSERT INTO zaehler_saisons (register_id, saison, wochentage, zeit_von, zeit_bis)
         VALUES ('00000000-0000-0000-0000-000000000001', 'WINTER',
                 ARRAY[1,2,3,4,5]::smallint[], '22:00', '07:00')",
    )
    .execute(&pool)
    .await
    .expect_err("an inverted window must be refused");
    assert!(
        err.to_string().contains("zaehler_saisons_window_ordered"),
        "expected the ordering CHECK to reject it, got: {err}"
    );

    // And a weekday outside ISO 1..=7 is not a weekday.
    let err = sqlx::query(
        "INSERT INTO zaehler_saisons (register_id, saison, wochentage, zeit_von, zeit_bis)
         VALUES ('00000000-0000-0000-0000-000000000001', 'WINTER',
                 ARRAY[0,8]::smallint[], '07:00', '22:00')",
    )
    .execute(&pool)
    .await
    .expect_err("an out-of-range weekday must be refused");
    assert!(
        err.to_string().contains("zaehler_saisons"),
        "expected the weekday CHECK to reject it, got: {err}"
    );
}

#[tokio::test]
async fn the_strom_mehr_mindermengenpreise_are_keyed_by_month_alone() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO mmm_preise_strom (price_month, mehr_ct_kwh, minder_ct_kwh, source)
         VALUES ('2026-07-01', 7.1234, 6.9876, 'bdew-csv')",
    )
    .execute(&pool)
    .await
    .expect("the published month is accepted");

    // § 13 Abs. 3 StromNZV: the prices are *einheitlich*. A second row for the
    // same month is a second nationwide price, which cannot exist — and the old
    // per-`vnb_mp_id` key permitted exactly that, with no rule for choosing.
    let err = sqlx::query(
        "INSERT INTO mmm_preise_strom (price_month, mehr_ct_kwh, minder_ct_kwh, source)
         VALUES ('2026-07-01', 9.0000, 9.0000, 'manual')",
    )
    .execute(&pool)
    .await
    .expect_err("a second price for the same month must be refused");
    assert!(
        err.to_string().contains("mmm_preise_strom_pkey"),
        "expected the primary key to reject it, got: {err}"
    );
}

#[tokio::test]
async fn a_gas_mabis_zaehlpunkt_cannot_be_recorded() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    // MaBiS is the Marktregeln für die Bilanzkreisabrechnung *Strom*; Gas runs
    // under GaBi Gas and has no MaBiS-Zählpunkt. The CHECK admits 'STROM' only,
    // so the attempt fails at the schema rather than storing an assignment that
    // describes nothing.
    let err = sqlx::query(
        "INSERT INTO mabis_zaehlpunkte (bilanzierungsgebiet, tenant, mabis_zp_id, sparte)
         VALUES ('11YAPG4CTRDNZ--P', '9900357000004',
                 'DE0004030099000000000000000012345', 'GAS')",
    )
    .execute(&pool)
    .await
    .expect_err("there is no Sparte dimension on a MaBiS-Zählpunkt");
    assert!(
        err.to_string().contains("sparte"),
        "expected the missing column to be named, got: {err}"
    );
}

#[tokio::test]
async fn the_tariff_zone_resolves_from_typed_weekday_and_wall_clock_columns() {
    use mako_markt::repository::ZaehlzeitRepository as _;
    use marktd::pg::PgZaehlzeitRepository;
    use time::macros::datetime;

    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let tenant = "9900357000004";

    // HT Mon–Fri 07:00–22:00, NT Mon–Fri 22:00–24:00.
    for (id, name, auspraegung, von, bis) in [
        (
            "00000000-0000-0000-0000-0000000000a1",
            "HT",
            "HT",
            "07:00",
            "22:00",
        ),
        (
            "00000000-0000-0000-0000-0000000000a2",
            "NT",
            "NT",
            "22:00",
            "23:59",
        ),
    ] {
        sqlx::query(
            "INSERT INTO zaehler_register
                 (id, zaehler_id, tenant, bezeichnung, zaehlerauspraegung, einheit, valid_from)
             VALUES ($1::uuid, 'Z1', $2, $3, $4, 'KWH', '2026-01-01')",
        )
        .bind(id)
        .bind(tenant)
        .bind(name)
        .bind(auspraegung)
        .execute(&pool)
        .await
        .expect("seed register");

        sqlx::query(
            "INSERT INTO zaehler_saisons (register_id, saison, wochentage, zeit_von, zeit_bis)
             VALUES ($1::uuid, 'GESAMT', ARRAY[1,2,3,4,5]::smallint[], $2::time, $3::time)",
        )
        .bind(id)
        .bind(von)
        .bind(bis)
        .execute(&pool)
        .await
        .expect("seed saison");
    }

    let repo = PgZaehlzeitRepository::new(pool.clone());

    // Thursday 2026-01-08, inside the HT window.
    assert_eq!(
        repo.resolve_tariff_zone("Z1", tenant, datetime!(2026-01-08 09:30))
            .await
            .unwrap()
            .as_deref(),
        Some("HT"),
    );

    // Same Thursday, after the HT window closes. The comparison is on `TIME`
    // values now; as text this only worked while every writer zero-padded.
    assert_eq!(
        repo.resolve_tariff_zone("Z1", tenant, datetime!(2026-01-08 22:30))
            .await
            .unwrap()
            .as_deref(),
        Some("NT"),
    );

    // The boundary is half-open: 07:00 is HT, and 22:00 has already left it.
    assert_eq!(
        repo.resolve_tariff_zone("Z1", tenant, datetime!(2026-01-08 07:00))
            .await
            .unwrap()
            .as_deref(),
        Some("HT"),
    );

    // Saturday 2026-01-10 matches no weekday in the array.
    assert_eq!(
        repo.resolve_tariff_zone("Z1", tenant, datetime!(2026-01-10 09:30))
            .await
            .unwrap(),
        None,
        "a weekday outside the window's `wochentage` must not resolve"
    );
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
