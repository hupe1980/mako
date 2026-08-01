//! DB-backed financial scenario tests — the money paths that carry real risk.
//!
//! Every money movement now flows through the `doubleentry` ledger; these tests
//! run against a live PostgreSQL (accountingd satellites in `public`, the ledger
//! in the `doubleentry` schema of the same database). They are `#[ignore]` by
//! default and self-manage PostgreSQL via testcontainers (only a Docker daemon is
//! required); they skip gracefully when Docker is unavailable:
//!
//! ```bash
//! just test-accountingd-db
//! ```
//!
//! Each test uses a unique tenant, so the shared `public` satellites are isolated;
//! the ledger is one-per-database, so `TENANT` names it (a fixed test ledger).

use accountingd::ledger::PgLedger;
use accountingd::pg;
use sqlx::PgPool;
use time::macros::date;
use uuid::Uuid;

const TENANT: &str = "accountingd-db-test";

/// Connects the satellite pool (migrated) and the doubleentry ledger.
async fn setup() -> Option<(PgPool, PgLedger, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::migrate!("./migrations").run(&pool).await.ok()?;
    let ledger = PgLedger::connect(&url, TENANT).await.ok()?;
    Some((pool, ledger, container))
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// Duplicate CloudEvent delivery must book the receivable exactly once — the
/// doubleentry idempotency key makes the redelivery a no-op returning the original.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn duplicate_ce_books_once() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let ce = uniq("ce");
    let d = date!(2026 - 07 - 01);

    let id1 = pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        13000,
        &ce,
        Some(&ce),
        None,
        d,
        d,
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();
    let id2 = pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        13000,
        &ce,
        Some(&ce),
        None,
        d,
        d,
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();

    assert_eq!(id1, id2, "redelivery returns the original entry id (no-op)");
    assert_eq!(
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        13000,
        "balance reflects one booking only"
    );
    // The satellite cache agrees.
    let acc = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(acc.balance_ct, 13000, "balance cache reflects one booking");
}

/// ABSCHLAG advance-payment credits net against the full-cost Rechnung debit, and
/// the balance cache stays consistent with the authoritative ledger.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn abschlag_credit_nets_against_rechnung() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let d = date!(2026 - 01 - 15);

    // 12 monthly advance-payment credits of 100.00 EUR.
    for m in 1..=12 {
        let key = format!("abschlag:{malo}:2026-{m:02}");
        pg::post_entry(
            &ledger,
            &pool,
            TENANT,
            &malo,
            "LF1",
            "ABSCHLAG",
            -10000,
            &key,
            None,
            None,
            d,
            d,
            Some("Abschlag"),
            None,
        )
        .await
        .unwrap();
    }
    // Full annual Rechnung of 1300.00 EUR (Nachzahlung 100.00 EUR).
    let ce = uniq("ce");
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        130000,
        &ce,
        Some(&ce),
        None,
        d,
        d,
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        10000,
        "1300 − 1200 = 100 Nachzahlung"
    );

    // The cache matches the authoritative ledger — no drift.
    let rec = pg::reconcile_balance(&ledger, &pool, &malo, "LF1", TENANT, false)
        .await
        .unwrap();
    assert_eq!(rec.recomputed_balance_ct, 10000);
    assert_eq!(rec.cached_balance_ct, rec.recomputed_balance_ct, "no drift");
    assert!(rec.is_consistent);
}

/// A conflicting reuse of an idempotency key (same key, different amount) is
/// refused — never a silent overwrite, never a second entry.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn conflicting_idempotency_key_is_refused() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let key = uniq("k");
    let d = date!(2026 - 05 - 01);

    pg::post_entry(
        &ledger, &pool, TENANT, &malo, "LF1", "RECHNUNG", 5000, &key, None, None, d, d, None, None,
    )
    .await
    .unwrap();
    let conflict = pg::post_entry(
        &ledger, &pool, TENANT, &malo, "LF1", "RECHNUNG", 9999, &key, None, None, d, d, None, None,
    )
    .await;
    assert!(
        conflict.is_err(),
        "same key, different content must be refused"
    );
    assert_eq!(ledger.balance_ct("LF1", &malo).await.unwrap(), 5000);
}

/// Auto-clearing: paying an invoice in full removes it from the open-item list
/// (a recorded Zahlungszuordnung), and the Summen- und Saldenliste balances.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn payment_clears_invoice_and_trial_balance_balances() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();

    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        5000,
        &uniq("ce"),
        None,
        None,
        date!(2026 - 06 - 01),
        date!(2026 - 06 - 01),
        Some("Rechnung"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        ledger.open_receivables("LF1", &malo).await.unwrap().len(),
        1,
        "the unpaid invoice is open"
    );

    // Full payment — post_entry auto-clears it against the invoice.
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "ZAHLUNG",
        -5000,
        &uniq("ce"),
        None,
        None,
        date!(2026 - 06 - 10),
        date!(2026 - 06 - 10),
        Some("Zahlung"),
        None,
    )
    .await
    .unwrap();
    assert!(
        ledger
            .open_receivables("LF1", &malo)
            .await
            .unwrap()
            .is_empty(),
        "a fully paid invoice is no longer an open item"
    );

    // The Summen- und Saldenliste balances (every entry is balanced by construction).
    let tb = ledger.trial_balance().await.unwrap();
    let dr: i64 = tb.iter().map(|l| l.debits_ct).sum();
    let cr: i64 = tb.iter().map(|l| l.credits_ct).sum();
    assert_eq!(dr, cr, "Summen- und Saldenliste: Soll = Haben");
}

/// Festschreibung (GoBD / § 146 AO): a period can be sealed, the seal chain
/// verifies, and a backdated booking into the sealed period is then refused.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn sealing_a_period_freezes_it() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    // A dedicated 2020 window so this test's seal cannot freeze other tests.
    let d = date!(2020 - 01 - 10);
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        4200,
        &uniq("ce"),
        None,
        None,
        d,
        d,
        Some("pre-seal"),
        None,
    )
    .await
    .unwrap();

    let period = uniq("seal2020-01");
    let seal = ledger
        .seal_period(&period, date!(2020 - 01 - 01), date!(2020 - 01 - 31))
        .await
        .unwrap();
    assert!(
        seal.entry_count >= 1,
        "the seal covers the period's entries"
    );
    assert!(
        ledger.verify_seals().await.is_ok(),
        "the seal chain verifies"
    );

    // A backdated correction into the sealed period must be refused.
    let backdated = pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "KORREKTUR",
        100,
        &uniq("k"),
        None,
        None,
        date!(2020 - 01 - 15),
        date!(2020 - 01 - 15),
        None,
        None,
    )
    .await;
    assert!(
        backdated.is_err(),
        "a booking into a sealed period must be rejected"
    );
}

/// Tamper-evidence: a recorded entry is provably included under the current head,
/// and the balance is the authoritative ledger net (GoBD Unveränderbarkeit).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn ledger_entry_is_provable() {
    use doubleentry::storage::LedgerStore;
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let ce = uniq("ce");
    let d = date!(2026 - 02 - 01);

    let id = pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        7777,
        &ce,
        Some(&ce),
        None,
        d,
        d,
        Some("Rechnung"),
        None,
    )
    .await
    .unwrap();

    let store = ledger.store();
    let stored = store
        .get(doubleentry::EntryId::from_uuid(id))
        .await
        .unwrap()
        .expect("entry is stored");
    let index = stored.require_index().unwrap();
    let head = store.head().await.unwrap();
    let proof = store.prove_inclusion(index).await.unwrap();
    assert!(
        proof.verify(&stored.content_hash, &head.root),
        "the entry is committed to by the current Merkle head"
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
