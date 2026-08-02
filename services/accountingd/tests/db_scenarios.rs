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

// ── §§41f/41g EnWG disconnection sequence ─────────────────────────────────────

/// Seed an unresolved **Mahnstufe-3** case for a fresh account and return its id.
///
/// Also arms the §41f Abs. 3 S. 1 consumption gate by setting the monthly
/// Abschlag to ⅓ of the arrears (so `2× Abschlag = ⅔ arrears ≤ arrears` clears
/// the gate). Tests that exercise the gate itself override `abschlag_ct` /
/// `jahresabschluss_runs` afterwards via [`set_abschlag`] / [`set_annual_bill`].
async fn seed_stufe3(pool: &PgPool, malo: &str, amount_ct: i64) -> Uuid {
    let account_id = pg::upsert_account(pool, malo, "LF1", TENANT).await.unwrap();
    set_abschlag(pool, malo, amount_ct / 3).await;
    pg::create_dunning_case(
        pool,
        account_id,
        TENANT,
        3,
        amount_ct,
        date!(2026 - 01 - 01),
    )
    .await
    .unwrap()
}

/// Set the agreed monthly Abschlag (`accounts.abschlag_ct`) for the account.
async fn set_abschlag(pool: &PgPool, malo: &str, abschlag_ct: i64) {
    sqlx::query("UPDATE accounts SET abschlag_ct = $1 WHERE malo_id = $2 AND tenant = $3")
        .bind(abschlag_ct)
        .bind(malo)
        .bind(TENANT)
        .execute(pool)
        .await
        .unwrap();
}

/// Record an expected annual bill (`jahresabschluss_runs.annual_bill_ct`) for a
/// given billing year, so the §41f Abs. 3 S. 1 fallback (⅙ Jahresrechnung, when
/// no Abschlag is agreed) fires. The candidate query picks the most recent year.
async fn set_annual_bill(pool: &PgPool, malo: &str, billing_year: i16, annual_bill_ct: i64) {
    sqlx::query(
        "INSERT INTO jahresabschluss_runs \
         (tenant, malo_id, billing_year, annual_bill_ct, sum_abschlage_ct, zahlbetrag_ct) \
         VALUES ($1, $2, $3, $4, 0, 0)",
    )
    .bind(TENANT)
    .bind(malo)
    .bind(billing_year)
    .bind(annual_bill_ct)
    .execute(pool)
    .await
    .unwrap();
}

fn contains(cands: &[(Uuid, String, String, i64)], case: Uuid) -> bool {
    cands.iter().any(|(id, ..)| *id == case)
}

/// The full sequence steps forward one phase at a time, each phase query excludes
/// the case once it has advanced (idempotent), and the Fristen gate correctly.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn sperr_sequence_progresses_through_all_phases() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &malo, 15_000).await;
    let threshold = 10_000;

    // ── Phase 1: Sperrandrohung (§41f Abs. 1) ──
    let c = pg::list_androhung_candidates(&pool, TENANT, threshold)
        .await
        .unwrap();
    assert!(
        contains(&c, case),
        "Stufe-3 arrears ≥ threshold is an Androhung candidate"
    );
    pg::mark_sperrandrohung(&pool, case, TENANT).await.unwrap();
    let c = pg::list_androhung_candidates(&pool, TENANT, threshold)
        .await
        .unwrap();
    assert!(
        !contains(&c, case),
        "an already-threatened case is no longer an Androhung candidate (idempotent)"
    );

    // ── Phase 2: Sperrankündigung (§41f Abs. 5), gated by the 4-Wochen Frist ──
    let not_yet = pg::list_ankuendigung_candidates(&pool, TENANT, 28)
        .await
        .unwrap();
    assert!(
        !contains(&not_yet, case),
        "Ankündigung waits out the 4-Wochen Androhungsfrist"
    );
    let due = pg::list_ankuendigung_candidates(&pool, TENANT, 0)
        .await
        .unwrap();
    assert!(
        contains(&due, case),
        "once the Androhungsfrist has elapsed the case is an Ankündigung candidate"
    );
    // Announce with a past planned date so Phase 3 is immediately eligible.
    pg::mark_sperrankuendigung(&pool, case, TENANT, date!(2026 - 01 - 08))
        .await
        .unwrap();
    let after = pg::list_ankuendigung_candidates(&pool, TENANT, 0)
        .await
        .unwrap();
    assert!(
        !contains(&after, case),
        "an already-announced case is no longer an Ankündigung candidate (idempotent)"
    );

    // ── Phase 3: Sperrauftrag (announced date reached) ──
    let sa = pg::list_sperrauftrag_candidates(&pool, TENANT)
        .await
        .unwrap();
    assert!(
        contains(&sa, case),
        "the announced disconnection date has arrived → Sperrauftrag candidate"
    );
    pg::mark_sperrauftrag_dispatched(&pool, case, TENANT, "sperrd:test")
        .await
        .unwrap();
    let sa = pg::list_sperrauftrag_candidates(&pool, TENANT)
        .await
        .unwrap();
    assert!(
        !contains(&sa, case),
        "a dispatched Sperrauftrag is not re-posted (idempotent)"
    );
}

/// The Sperrauftrag must not fire before the announced disconnection date
/// (§41f Abs. 5 — 8 Werktage im Voraus).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn sperrauftrag_waits_for_the_announced_date() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &malo, 15_000).await;
    pg::mark_sperrandrohung(&pool, case, TENANT).await.unwrap();
    // Announce a date far in the future.
    pg::mark_sperrankuendigung(&pool, case, TENANT, date!(2099 - 01 - 01))
        .await
        .unwrap();
    let sa = pg::list_sperrauftrag_candidates(&pool, TENANT)
        .await
        .unwrap();
    assert!(
        !contains(&sa, case),
        "a case whose announced date is still in the future is not a Sperrauftrag candidate"
    );
}

/// An accepted Abwendungsvereinbarung (§41g Abs. 1 S. 10) bars disconnection —
/// the case drops out of every phase query.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn abwendungsvereinbarung_halts_the_sequence() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &malo, 15_000).await;
    // A second open Stufe-3 case on the SAME account (auto-dunning creates a fresh
    // case per Mahnstufe, so this is realistic). The agreement covers the supply
    // point, so accepting it must halt both.
    let account_id = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap()
        .account_id;
    let case2 =
        pg::create_dunning_case(&pool, account_id, TENANT, 3, 15_000, date!(2026 - 02 - 01))
            .await
            .unwrap();

    let n = pg::vereinbare_abwendung(&pool, case, TENANT).await.unwrap();
    assert_eq!(n, 2, "the agreement halts every open case of the account");
    for c in [case, case2] {
        assert!(
            !contains(
                &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                    .await
                    .unwrap(),
                c
            ),
            "an Abwendungsvereinbarung removes the account's cases from Phase 1"
        );
    }
    assert!(
        !contains(
            &pg::list_ankuendigung_candidates(&pool, TENANT, 0)
                .await
                .unwrap(),
            case
        ),
        "…and from Phase 2"
    );
}

/// An Unverhältnismäßigkeit/Schutzbedürftigkeit flag (§41f Abs. 1 S. 2 / Abs. 2)
/// halts the sequence.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn unverhaeltnismaessigkeit_halts_the_sequence() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &malo, 15_000).await;
    let n = pg::markiere_unverhaeltnismaessig(&pool, case, TENANT)
        .await
        .unwrap();
    assert_eq!(n, 1, "the open case is flagged");
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "an Unverhältnismäßigkeit flag removes the case from the sequence"
    );
}

/// Below the §41f Abs. 3 S. 2 threshold (arrears < 100 €) the case is not a
/// disconnection candidate.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn below_threshold_is_not_a_candidate() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &malo, 5_000).await; // 50 € < 100 € threshold
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "arrears below the threshold never open the disconnection sequence"
    );
}

/// §41f Abs. 3 S. 1 — arrears above the 100 € floor but **below 2× the monthly
/// Abschlag** do not qualify (the consumption-relative gate is not met).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn below_consumption_gate_is_not_a_candidate() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &malo, 15_000).await; // 150 € > 100 € floor
    // Override the Abschlag to 200 €/month → 2× = 400 € > 150 € arrears.
    set_abschlag(&pool, &malo, 20_000).await;
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "arrears below 2× the monthly Abschlag do not meet §41f Abs. 3 S. 1"
    );
    // Lower the Abschlag to 50 €/month → 2× = 100 € ≤ 150 € arrears → qualifies.
    set_abschlag(&pool, &malo, 5_000).await;
    assert!(
        contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "arrears ≥ 2× the monthly Abschlag meet the consumption gate"
    );
}

/// §41f Abs. 3 S. 1 fallback — *wenn keine Abschläge vereinbart sind* the gate is
/// ⅙ of the expected annual bill (`jahresabschluss_runs.annual_bill_ct`).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn annual_bill_fallback_when_no_abschlag() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &malo, 15_000).await; // 150 € > 100 € floor
    set_abschlag(&pool, &malo, 0).await; // no Abschlag agreed → fall back to annual bill
    // Annual bill 1200 € → ⅙ = 200 € > 150 € arrears → excluded.
    set_annual_bill(&pool, &malo, 2024, 120_000).await;
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "arrears below ⅙ of the expected annual bill do not qualify"
    );
    // A more recent (2025), lower annual bill 600 € → ⅙ = 100 € ≤ 150 € → qualifies.
    // This also proves the query picks the latest billing_year, not the largest bill.
    set_annual_bill(&pool, &malo, 2025, 60_000).await;
    assert!(
        contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "arrears ≥ ⅙ of the most recent expected annual bill qualify"
    );
}

/// With neither an Abschlag nor a prior Jahresrechnung on record the §41f Abs. 3
/// S. 1 gate cannot be established — the case is conservatively excluded even
/// with high arrears (mako never disconnects without a provable consumption basis).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn no_consumption_basis_is_conservatively_excluded() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &malo, 500_000).await; // 5000 € arrears
    set_abschlag(&pool, &malo, 0).await; // no Abschlag, no Jahresabschluss
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "no consumption basis → not a candidate, regardless of the arrears size"
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
