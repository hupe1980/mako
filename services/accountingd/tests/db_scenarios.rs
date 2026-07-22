//! DB-backed financial scenario tests — the money paths that carry real risk.
//!
//! These require a live PostgreSQL. They are `#[ignore]` by default; run with:
//!
//! ```bash
//! export DATABASE_URL="postgres://a:s@localhost:5432/accountingd_test"
//! cargo test -p accountingd --test db_scenarios -- --ignored
//! ```
//!
//! Each test uses a unique tenant so they are isolated on a shared database and
//! provisions the schema via `sqlx::migrate!` (idempotent).

use accountingd::pg;
use sqlx::PgPool;
use time::macros::date;
use uuid::Uuid;

async fn setup() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::migrate!("./migrations").run(&pool).await.ok()?;
    Some(pool)
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// Duplicate CloudEvent delivery must book the receivable exactly once.
#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn duplicate_ce_books_once() {
    let Some(pool) = setup().await else {
        return;
    };
    let tenant = uniq("t");
    let malo = uniq("MALO");
    let acct = pg::upsert_account(&pool, &malo, "LF1", &tenant)
        .await
        .unwrap();
    let ce = uniq("ce");

    let d = date!(2026 - 07 - 01);
    let id1 = pg::write_entry(
        &pool,
        acct,
        &tenant,
        "RECHNUNG",
        13000,
        None,
        Some("de.billing.rechnung.erstellt"),
        Some(&ce),
        d,
        Some("Jahresrechnung"),
    )
    .await
    .unwrap();
    let id2 = pg::write_entry(
        &pool,
        acct,
        &tenant,
        "RECHNUNG",
        13000,
        None,
        Some("de.billing.rechnung.erstellt"),
        Some(&ce),
        d,
        Some("Jahresrechnung"),
    )
    .await
    .unwrap();

    assert!(id1.is_some(), "first delivery books");
    assert!(id2.is_none(), "redelivery is a no-op");

    let acc = pg::fetch_account(&pool, &malo, "LF1", &tenant)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(acc.balance_ct, 13000, "balance reflects one booking only");
}

/// ABSCHLAG advance-payment credits net against the full-cost Rechnung debit.
#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn abschlag_credit_nets_against_rechnung() {
    let Some(pool) = setup().await else {
        return;
    };
    let tenant = uniq("t");
    let malo = uniq("MALO");
    let acct = pg::upsert_account(&pool, &malo, "LF1", &tenant)
        .await
        .unwrap();
    let d = date!(2026 - 01 - 15);

    // 12 monthly advance-payment credits of 100.00 EUR.
    for m in 1..=12 {
        pg::write_entry(
            &pool,
            acct,
            &tenant,
            "ABSCHLAG",
            -10000,
            None,
            Some("de.accounting.abschlag.posted"),
            Some(&format!("abschlag:{malo}:2026-{m:02}")),
            d,
            Some("Abschlag"),
        )
        .await
        .unwrap();
    }
    // Full annual Rechnung of 1300.00 EUR (Nachzahlung 100.00 EUR).
    pg::write_entry(
        &pool,
        acct,
        &tenant,
        "RECHNUNG",
        130000,
        None,
        Some("de.billing.rechnung.erstellt"),
        Some(&uniq("ce")),
        d,
        Some("Jahresrechnung"),
    )
    .await
    .unwrap();

    let acc = pg::fetch_account(&pool, &malo, "LF1", &tenant)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(acc.balance_ct, 10000, "1300 − 1200 = 100 Nachzahlung");

    // The cached balance must equal the recomputed ledger sum.
    let rec = pg::reconcile_balance(&pool, acct, &tenant, false)
        .await
        .unwrap();
    assert_eq!(rec.recomputed_balance_ct, 10000);
    assert_eq!(rec.cached_balance_ct, rec.recomputed_balance_ct, "no drift");
}

/// Every ledger entry writes exactly one balanced (Soll = Haben) journal pair.
#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn journal_is_balanced_double_entry() {
    let Some(pool) = setup().await else {
        return;
    };
    let tenant = uniq("t");
    let malo = uniq("MALO");
    let acct = pg::upsert_account(&pool, &malo, "LF1", &tenant)
        .await
        .unwrap();
    let id = pg::write_entry(
        &pool,
        acct,
        &tenant,
        "RECHNUNG",
        7777,
        None,
        Some("de.billing.rechnung.erstellt"),
        Some(&uniq("ce")),
        date!(2026 - 03 - 01),
        Some("Rechnung"),
    )
    .await
    .unwrap()
    .unwrap();

    let net: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(CASE WHEN side='D' THEN amount_ct ELSE -amount_ct END),0)::bigint \
         FROM journal_lines WHERE ledger_entry_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(net, 0, "Soll = Haben per ledger entry");

    let line_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM journal_lines WHERE ledger_entry_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(line_count, 2, "exactly one D and one C line");
}

/// The deferred balance trigger rejects an unbalanced (Soll ≠ Haben) pair.
#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn unbalanced_journal_pair_is_rejected() {
    let Some(pool) = setup().await else {
        return;
    };
    let tenant = uniq("t");
    let malo = uniq("MALO");
    let acct = pg::upsert_account(&pool, &malo, "LF1", &tenant)
        .await
        .unwrap();
    // A raw ledger row (no auto journal), then a hand-written unbalanced pair.
    let le: Uuid = sqlx::query_scalar(
        "INSERT INTO ledger_entries (account_id, tenant, entry_type, amount_ct, booking_date, value_date)          VALUES ($1,$2,'RECHNUNG',600,'2026-01-01','2026-01-01') RETURNING id",
    )
    .bind(acct)
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .unwrap();

    let res = sqlx::query(
        "INSERT INTO journal_lines (ledger_entry_id, account_id, tenant, side, skr_account, skr_description, amount_ct, booking_date)          VALUES ($1,$2,$3,'D','1400','x',600,'2026-01-01'),($1,$2,$3,'C','4000','y',400,'2026-01-01')",
    )
    .bind(le)
    .bind(acct)
    .bind(&tenant)
    .execute(&pool)
    .await;
    assert!(
        res.is_err(),
        "unbalanced Soll/Haben must be rejected by the deferred constraint trigger"
    );
}
