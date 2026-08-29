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
    // `event_outbox` is created by `mako-service` at service startup rather
    // than by a migration, so the harness must create it too — otherwise any
    // code path that announces a CloudEvent fails here but works in production.
    mako_service::outbox::ensure_schema(&pool).await.ok()?;
    let ledger = PgLedger::connect(&url, TENANT).await.ok()?;
    Some((pool, ledger, container))
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// A realistically shaped MaLo-ID: **11 digits, no separators** (BDEW).
///
/// `uniq("MALO")` produces a hyphenated string, which is fine as an opaque key
/// but wrong for anything that parses the identifier — the free-text payment
/// resolver compares whole normalised tokens, and a real MaLo-ID has nothing to
/// normalise away.
fn uniq_malo() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    let digits: String = hex.chars().filter(char::is_ascii_digit).take(10).collect();
    format!("5{digits:0<10}")
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

/// The advance lifecycle over one year through the real ledger: twelve demands
/// raised, twelve payments received, one annual invoice that bills the gross
/// and discharges the advances it deducted.
///
/// The numbers are the point. An advance booked as a *credit* would leave this
/// year at −1 200,00 EUR and have the Jahresabschluss pay that out by pain.001
/// to a customer who owes 100,00 EUR.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_advance_plan_paid_in_full_leaves_only_the_nachzahlung() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    sqlx::query("UPDATE accounts SET abschlag_ct = 10000 WHERE malo_id = $1 AND tenant = $2")
        .bind(&malo)
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    let account = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap();

    // Twelve Abschlagsforderungen of 100.00 EUR — debits, because an advance is
    // demanded before it is paid.
    for m in 1u8..=12 {
        let periode = date!(2026 - 01 - 01)
            .replace_month(m.try_into().unwrap())
            .unwrap();
        pg::raise_abschlagsforderung(&ledger, &pool, TENANT, &account, periode, periode, periode)
            .await
            .unwrap();
    }
    assert_eq!(
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        120_000,
        "twelve advances demanded and none yet paid is a receivable, not a credit"
    );

    // The register carries the rate each was raised at, and none is deductible
    // yet because none has been received.
    let register = pg::list_abschlag_forderungen(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        date!(2026 - 01 - 01),
        date!(2026 - 12 - 31),
    )
    .await
    .unwrap();
    assert_eq!(register.len(), 12);
    assert!(
        register.iter().all(|a| !a.vereinnahmt()),
        "nothing has been paid yet"
    );
    assert_eq!(register[0].ust_satz, rust_decimal::dec!(0.19));

    // Twelve payments arrive and clear the demands FIFO.
    for m in 1..=12 {
        let d = date!(2026 - 01 - 20)
            .replace_month((m as u8).try_into().unwrap())
            .unwrap();
        pg::post_entry(
            &ledger,
            &pool,
            TENANT,
            &malo,
            "LF1",
            "ZAHLUNG",
            -10_000,
            &format!("pay:{malo}:{m}"),
            None,
            None,
            d,
            d,
            Some("SEPA-Einzug"),
            None,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        0,
        "the plan is paid up"
    );
    let register = pg::list_abschlag_forderungen(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        date!(2026 - 01 - 01),
        date!(2026 - 12 - 31),
    )
    .await
    .unwrap();
    assert!(
        register.iter().all(pg::AbschlagForderung::deductible),
        "every advance is now received and unabsorbed — deductible under § 14 Abs. 5 Satz 2 UStG"
    );

    // The annual invoice: 1 300,00 EUR gross, deducting the 1 200,00 EUR of
    // advances it settles.
    let ce = uniq("ce");
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        130_000,
        &ce,
        Some(&ce),
        None,
        date!(2027 - 01 - 15),
        date!(2027 - 01 - 15),
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();
    pg::verrechne_abschlaege(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        120_000,
        "RE-2027-000001",
        &format!("{ce}:abschlag-verrechnung"),
        date!(2027 - 01 - 15),
    )
    .await
    .unwrap();

    assert_eq!(
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        10_000,
        "1300 gross − 1200 advances discharged = 100.00 Nachzahlung"
    );

    // Every advance is stamped with the invoice that absorbed it, so a second
    // settlement cannot deduct it again.
    let register = pg::list_abschlag_forderungen(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        date!(2026 - 01 - 01),
        date!(2026 - 12 - 31),
    )
    .await
    .unwrap();
    assert!(
        register
            .iter()
            .all(|a| a.verrechnet_mit.as_deref() == Some("RE-2027-000001")),
        "all twelve absorbed by the settling invoice"
    );
    assert!(
        register.iter().all(|a| !a.deductible()),
        "and none of them deductible a second time"
    );

    // The cache matches the authoritative ledger — no drift.
    let rec = pg::reconcile_balance(&ledger, &pool, &malo, "LF1", TENANT, false)
        .await
        .unwrap();
    assert_eq!(rec.recomputed_balance_ct, 10_000);
    assert_eq!(rec.cached_balance_ct, rec.recomputed_balance_ct, "no drift");
    assert!(rec.is_consistent);
}

/// A demanded, unpaid advance is an **open receivable**, so it reaches the
/// § 41f Abs. 3 Verzug and the Mahnwesen. Booked as a credit it would drive
/// `balance_ct` further negative every month, and the dunning worker — which
/// selects on `balance_ct > 0` — would never see the customer at all.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unpaid_advance_is_in_verzug() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    sqlx::query("UPDATE accounts SET abschlag_ct = 12000 WHERE malo_id = $1 AND tenant = $2")
        .bind(&malo)
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    let account = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap();

    for m in 1u8..=3 {
        let periode = date!(2026 - 01 - 01)
            .replace_month(m.try_into().unwrap())
            .unwrap();
        pg::raise_abschlagsforderung(&ledger, &pool, TENANT, &account, periode, periode, periode)
            .await
            .unwrap();
    }

    let verzug = pg::compute_verzug_ct(&ledger, &pool, TENANT, &malo, "LF1")
        .await
        .unwrap();
    assert_eq!(
        verzug, 36_000,
        "three unpaid advances of 120.00 are 360.00 of supply debt in Verzug"
    );
    let acct = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(acct.balance_ct, 36_000, "and the dunning worker can see it");

    // …and the § 41f Abs. 3 gates arm on it: 360.00 clears the Satz-2 floor of
    // 100.00 and is ≥ 2× the 120.00 monthly Abschlag (Satz 1). Three missed
    // advances is the ordinary shape of this case.
    let case_id = pg::create_dunning_case(
        &pool,
        acct.account_id,
        TENANT,
        3,
        verzug,
        date!(2026 - 04 - 01),
    )
    .await
    .unwrap();
    let candidates = pg::list_androhung_candidates(&pool, TENANT, 10_000)
        .await
        .unwrap();
    assert!(
        candidates.iter().any(|c| c.0 == case_id),
        "unpaid advances are supply debt, so the §41f Abs. 3 gates see them"
    );
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

    // The gap below the watermark is closed too. A seal claims its closing
    // balances are exact — every entry booked on or before the period's last
    // day and nothing else — so a date that merely happens to lie in no defined
    // period must not stay bookable underneath it. Otherwise an ordinary
    // booking restates a sealed balance while the seal, its proofs and the
    // whole chain go on verifying byte for byte.
    assert_eq!(
        ledger.sealed_through(),
        Some(date!(2020 - 01 - 31)),
        "the books are closed through the sealed period's last day"
    );
    let into_undefined_gap = pg::post_entry(
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
        // December 2019 — earlier than the seal and covered by no period at all.
        date!(2019 - 12 - 15),
        date!(2019 - 12 - 15),
        None,
        None,
    )
    .await;
    assert!(
        into_undefined_gap.is_err(),
        "a date below the sealed watermark is closed even where no period covers it"
    );

    // Above the watermark the books stay open — a correction books forward.
    pg::post_entry(
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
        date!(2020 - 02 - 03),
        date!(2020 - 02 - 03),
        Some("Korrektur nach der Festschreibung"),
        None,
    )
    .await
    .expect("a correction books into an open period after the seal");
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
        proof.verify(&stored.content_hash, &head),
        "the entry is committed to by the current Merkle head"
    );

    // The proof is evidence about one leaf of one tree size, and nothing else.
    // Verification takes the whole head for that reason: a check against the
    // root alone would let a genuine proof be re-presented as a proof of a
    // different entry in a differently sized log, which is the difference
    // between tamper-evidence and a formality.
    let wrong_size = doubleentry::TreeHead {
        size: head.size + 1,
        root: head.root,
    };
    assert!(
        !proof.verify(&stored.content_hash, &wrong_size),
        "a proof must not verify against a head that states a different size"
    );
}

/// § 147 AO / GoBD: a sealed period's **closing balance per customer** is
/// provable, not merely reported.
///
/// The two proofs have to chain. The balance proof establishes what a handle
/// held; the binding proof establishes whose account that handle was. Checked
/// separately they would still admit the case a verifier actually cares about —
/// a correct balance attributed to the wrong customer.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_sealed_period_proves_the_customer_balance() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    // A dedicated 2019 window, so this seal cannot freeze other tests.
    let d = date!(2019 - 06 - 10);
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        9_900,
        &uniq("ce"),
        None,
        None,
        d,
        d,
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "ZAHLUNG",
        -4_000,
        &uniq("ce"),
        None,
        None,
        d,
        d,
        Some("Teilzahlung"),
        None,
    )
    .await
    .unwrap();

    let period = uniq("seal2019-06");
    ledger
        .seal_period(&period, date!(2019 - 06 - 01), date!(2019 - 06 - 30))
        .await
        .unwrap();

    let proof = ledger
        .prove_period_balance(&period, "LF1", &malo)
        .await
        .expect("a sealed period proves its balances")
        .into_proven()
        .expect("the customer had movement in the period");
    assert!(proof.verify(), "the proof verifies against its own seal");

    let net = proof.balance.balance.debits.to_minor() - proof.balance.balance.credits.to_minor();
    assert_eq!(
        net, 5_900,
        "the proven balance is the ledger net (9 900 invoiced − 4 000 paid)"
    );
    assert_eq!(
        proof.path().to_string(),
        format!("Kontokorrent:LF1:{malo}"),
        "the proven handle is bound to this customer's Kontokorrent"
    );

    // Entries appended after the seal must not move what the seal proves — the
    // trial balance is rebuilt over the log prefix the seal names, not over the
    // current log.
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        7_777,
        &uniq("ce"),
        None,
        None,
        date!(2026 - 03 - 01),
        date!(2026 - 03 - 01),
        Some("nach der Festschreibung"),
        None,
    )
    .await
    .unwrap();
    let again = ledger
        .prove_period_balance(&period, "LF1", &malo)
        .await
        .expect("still provable after later postings")
        .into_proven()
        .expect("still proven");
    assert!(again.verify(), "and still verifies");
    assert_eq!(
        again.balance.balance.debits.to_minor() - again.balance.balance.credits.to_minor(),
        5_900,
        "a later booking cannot change a sealed period's proven balance"
    );

    // Onboarding a new customer grows the account registry. The binding proof
    // has to be built against the registry the seal committed to, not the live
    // one — built against today's larger registry it has a longer path and
    // verifies against nothing, which would make every balance in every closed
    // period unprovable the moment the next customer signs up.
    let newcomer = uniq("MALO");
    pg::upsert_account(&pool, &newcomer, "LF1", TENANT)
        .await
        .unwrap();
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &newcomer,
        "LF1",
        "RECHNUNG",
        1_234,
        &uniq("ce"),
        None,
        None,
        date!(2026 - 03 - 02),
        date!(2026 - 03 - 02),
        Some("Neukunde nach der Festschreibung"),
        None,
    )
    .await
    .unwrap();

    let after_onboarding = ledger
        .prove_period_balance(&period, "LF1", &malo)
        .await
        .expect("a new customer must not break proofs for already-sealed periods")
        .into_proven()
        .expect("still proven after the registry grew");
    assert!(
        after_onboarding.verify(),
        "the binding proof is built against the registry as the seal recorded it"
    );
    assert_eq!(
        after_onboarding.path().to_string(),
        format!("Kontokorrent:LF1:{malo}"),
        "and still names the right customer"
    );

    // The newcomer had no account when the period closed, so there is nothing
    // to prove about them — and a zero-balance proof must not be manufactured.
    // That is an answer about intact books, not a failure, and it is a different
    // answer from "was on the books but had no movement": collapsing the two
    // would invite reading "not a customer yet" as "nothing happened".
    let newcomer_outcome = ledger
        .prove_period_balance(&period, "LF1", &newcomer)
        .await
        .expect("asking about a later customer is a well-formed question");
    assert!(
        matches!(
            newcomer_outcome,
            doubleentry::SealedBalanceOutcome::NotYetRegistered
        ),
        "a customer who did not exist at sealing time is not someone the seal can          speak about at all, which is not the same as having had no movement"
    );
}

/// An account registered before a seal whose bookings all fall **after** the
/// period has no row in its closing balance — and that is not a balance of zero.
///
/// A seal commits to *closing balances*, cumulative as of the period's last day,
/// so a customer who was quiet during the period but active before it still has a
/// row carrying the balance they brought in. The genuinely absent case is the one
/// here: a customer onboarded in August, for books that close June afterwards.
/// They are nameable — the registry had issued their handle by sealing time — and
/// the period still says nothing about them.
///
/// That is a different answer from "was not a customer yet", which the sibling
/// test covers, and collapsing the two would invite reading one as the other.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_account_booked_only_after_the_period_has_no_row() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let later = uniq("MALO");
    let active = uniq("MALO");
    for malo in [&later, &active] {
        pg::upsert_account(&pool, malo, "LF1", TENANT)
            .await
            .unwrap();
    }

    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &active,
        "LF1",
        "RECHNUNG",
        2_000,
        &uniq("ce"),
        None,
        None,
        date!(2023 - 06 - 15),
        date!(2023 - 06 - 15),
        Some("in der Periode"),
        None,
    )
    .await
    .unwrap();
    // Onboarded in August — registered before June is sealed, but with nothing
    // booked on or before 30 June.
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &later,
        "LF1",
        "RECHNUNG",
        1_000,
        &uniq("ce"),
        None,
        None,
        date!(2023 - 08 - 05),
        date!(2023 - 08 - 05),
        Some("Neukunde im August"),
        None,
    )
    .await
    .unwrap();

    ledger
        .seal_period("2023-06", date!(2023 - 06 - 01), date!(2023 - 06 - 30))
        .await
        .unwrap();

    let outcome = ledger
        .prove_period_balance("2023-06", "LF1", &later)
        .await
        .expect("a registered account is a well-formed question");
    assert!(
        matches!(outcome, doubleentry::SealedBalanceOutcome::NoRow),
        "the handle was issued by sealing time, so the account is nameable — it \
         simply has no row in June's closing balance, which is not a proven zero"
    );
    assert!(outcome.is_absent(), "nothing to prove, either way");

    let proven = ledger
        .prove_period_balance("2023-06", "LF1", &active)
        .await
        .unwrap()
        .into_proven()
        .expect("the active account did move");
    assert!(proven.verify());
    assert_eq!(
        proven.balance.balance.debits.to_minor() - proven.balance.balance.credits.to_minor(),
        2_000,
        "and the August invoice is no part of June's closing balance"
    );
}

/// The sealed watermark has to survive a restart.
///
/// It is held in an in-process calendar mirror, rebuilt from the period table on
/// connect. If that rebuild dropped it, a service restart would silently reopen
/// every gap below the last seal — and the first backdated booking after a
/// deploy would restate a sealed closing balance with every proof still
/// verifying.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_sealed_watermark_survives_a_restart() {
    let Some((url, _pg)) = pg_container().await else {
        return;
    };
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    mako_service::outbox::ensure_schema(&pool).await.unwrap();

    let malo = uniq("MALO");
    {
        let ledger = PgLedger::connect(&url, TENANT).await.unwrap();
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
            5_000,
            &uniq("ce"),
            None,
            None,
            date!(2022 - 05 - 10),
            date!(2022 - 05 - 10),
            Some("vor der Festschreibung"),
            None,
        )
        .await
        .unwrap();
        ledger
            .seal_period("2022-05", date!(2022 - 05 - 01), date!(2022 - 05 - 31))
            .await
            .unwrap();
    }

    // Restart: a fresh process against the same database.
    let restarted = PgLedger::connect(&url, TENANT).await.unwrap();
    assert_eq!(
        restarted.sealed_through(),
        Some(date!(2022 - 05 - 31)),
        "the watermark is rebuilt from the period table on connect"
    );
    let backdated = pg::post_entry(
        &restarted,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "KORREKTUR",
        100,
        &uniq("k"),
        None,
        None,
        date!(2022 - 04 - 02),
        date!(2022 - 04 - 02),
        None,
        None,
    )
    .await;
    assert!(
        backdated.is_err(),
        "an undefined month below the watermark stays closed across a restart"
    );
    assert!(
        restarted.verify_seals().await.is_ok(),
        "and the seal chain still verifies"
    );
}

/// A seal commits to the closing balance folded by **booking date**, not by log
/// prefix — and the two differ in the ordinary case.
///
/// Nobody seals January on 31 January. The books close in February, by which
/// time the log already holds February entries, so the prefix standing at the
/// moment of sealing is not the period. Rebuilding the commitment the wrong way
/// reproduces a different root and nothing is provable at all.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_period_sealed_after_later_bookings_still_proves_its_closing_balance() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();

    // January: the period being closed.
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        12_000,
        &uniq("ce"),
        None,
        None,
        date!(2021 - 01 - 15),
        date!(2021 - 01 - 15),
        Some("Januarrechnung"),
        None,
    )
    .await
    .unwrap();

    // February, booked *before* January is sealed — the normal state of affairs
    // when the books close a few weeks in arrears.
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        99_000,
        &uniq("ce"),
        None,
        None,
        date!(2021 - 02 - 10),
        date!(2021 - 02 - 10),
        Some("Februarrechnung — darf nicht in den Januar-Seal fallen"),
        None,
    )
    .await
    .unwrap();

    ledger
        .seal_period("2021-01", date!(2021 - 01 - 01), date!(2021 - 01 - 31))
        .await
        .unwrap();

    let proof = ledger
        .prove_period_balance("2021-01", "LF1", &malo)
        .await
        .expect("the closing balance is rebuilt by booking date, so it matches the seal")
        .into_proven()
        .expect("January had movement");
    assert!(proof.verify(), "and the proof verifies against the seal");
    assert_eq!(
        proof.balance.balance.debits.to_minor() - proof.balance.balance.credits.to_minor(),
        12_000,
        "the January seal proves the January closing balance — the February \
         invoice sitting in the log ahead of it is not part of the period"
    );
}

/// The append-only half of the evidence: an auditor who archived a head earlier
/// can prove the journal has only grown since.
///
/// An inclusion proof alone says the ledger is self-consistent *now*, which a
/// ledger rebuilt from scratch would also satisfy.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_journal_proves_it_is_append_only() {
    use doubleentry::storage::LedgerStore;
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let d = date!(2026 - 04 - 01);

    // The archive point must hold entries. Every log is consistent with the
    // empty tree, so a proof taken against size 0 asserts nothing and could not
    // detect a substituted head however it was checked.
    for amount in [5_000_i64, 6_000] {
        pg::post_entry(
            &ledger,
            &pool,
            TENANT,
            &malo,
            "LF1",
            "RECHNUNG",
            amount,
            &uniq("ce"),
            None,
            None,
            d,
            d,
            Some("pre-archive"),
            None,
        )
        .await
        .unwrap();
    }

    let archived = ledger.store().head().await.unwrap();
    assert!(archived.size > 0, "the auditor archived a non-empty log");
    for amount in [1_000_i64, 2_000, 3_000] {
        pg::post_entry(
            &ledger,
            &pool,
            TENANT,
            &malo,
            "LF1",
            "RECHNUNG",
            amount,
            &uniq("ce"),
            None,
            None,
            d,
            d,
            Some("post-archive"),
            None,
        )
        .await
        .unwrap();
    }

    let (proof, then, now) = ledger.prove_append_only(archived.size).await.unwrap();
    assert_eq!(
        then, archived,
        "the head the service reconstructs for that size is the one the auditor archived"
    );
    assert!(now.size >= archived.size + 3, "the log grew");
    assert!(
        proof.verify(&then, &now),
        "the archived log is a prefix of the current one"
    );

    // The same proof must not vouch for a head it was not built against.
    let forged = doubleentry::TreeHead {
        size: now.size,
        root: archived.root,
    };
    assert!(
        !proof.verify(&then, &forged),
        "a consistency proof must not verify against a substituted root"
    );
}

// ── §§41f/41g EnWG disconnection sequence ─────────────────────────────────────

/// Seed an unresolved **Mahnstufe-3** case for a fresh account and return its id.
///
/// Books `amount_ct` as a real `RECHNUNG` debit, because the §41f Abs. 3 gates
/// read the live open receivable — seeding only the case row leaves the account
/// owing nothing and every phase correctly refuses to advance it.
///
/// Also arms the Abs. 3 S. 1 consumption gate by setting the monthly Abschlag to
/// ⅓ of the arrears (so `2× Abschlag = ⅔ arrears ≤ arrears` clears it). Tests
/// that exercise the gate itself override `abschlag_ct` / `jahresabschluss_runs`
/// via [`set_abschlag`] / [`set_annual_bill`].
async fn seed_stufe3(pool: &PgPool, ledger: &PgLedger, malo: &str, amount_ct: i64) -> Uuid {
    let account_id = pg::upsert_account(pool, malo, "LF1", TENANT).await.unwrap();
    pg::post_entry(
        ledger,
        pool,
        TENANT,
        malo,
        "LF1",
        "RECHNUNG",
        amount_ct,
        &uniq("arrears"),
        None,
        None,
        date!(2026 - 01 - 01),
        date!(2026 - 01 - 01),
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();
    set_abschlag(pool, malo, amount_ct / 3).await;
    // The phase queries read `accounts.verzug_ct`. `post_entry` refreshes it,
    // but `set_abschlag` writes the account afterwards, so prime it explicitly.
    pg::refresh_verzug(ledger, pool, TENANT, malo, "LF1")
        .await
        .unwrap();
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
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
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
    let not_yet = pg::list_ankuendigung_candidates(&pool, TENANT, 28, 10_000)
        .await
        .unwrap();
    assert!(
        !contains(&not_yet, case),
        "Ankündigung waits out the 4-Wochen Androhungsfrist"
    );
    let due = pg::list_ankuendigung_candidates(&pool, TENANT, 0, 10_000)
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
    let after = pg::list_ankuendigung_candidates(&pool, TENANT, 0, 10_000)
        .await
        .unwrap();
    assert!(
        !contains(&after, case),
        "an already-announced case is no longer an Ankündigung candidate (idempotent)"
    );

    // ── Phase 3: Sperrauftrag (announced date reached) ──
    let sa = pg::list_sperrauftrag_candidates(&pool, TENANT, 10_000)
        .await
        .unwrap();
    assert!(
        contains(&sa, case),
        "the announced disconnection date has arrived → Sperrauftrag candidate"
    );
    pg::mark_sperrauftrag_dispatched(&pool, case, TENANT, "sperrd:test")
        .await
        .unwrap();
    let sa = pg::list_sperrauftrag_candidates(&pool, TENANT, 10_000)
        .await
        .unwrap();
    assert!(
        !contains(&sa, case),
        "a dispatched Sperrauftrag is not re-posted (idempotent)"
    );
}

/// **A customer who pays must not be dunned further, and must never be
/// disconnected.**
///
/// Without the settlement sweep the escalation chain runs on `due_date` alone,
/// walking a paid-up customer to Mahnstufe 3 — collecting a Mahngebühr at each
/// step, which pushes the settled balance back above zero and makes the next
/// step look justified — and into the disconnection sequence.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn paying_the_bill_stands_the_disconnection_sequence_down() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;

    // Before payment the case is a candidate for the Androhung.
    assert!(
        contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "150 EUR of unpaid supply debt opens the sequence"
    );

    // The customer pays in full.
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "ZAHLUNG",
        -15_000,
        &uniq("payment"),
        None,
        None,
        date!(2026 - 02 - 01),
        date!(2026 - 02 - 01),
        Some("Überweisung"),
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        pg::compute_verzug_ct(&ledger, &pool, TENANT, &malo, "LF1")
            .await
            .unwrap(),
        0,
        "nothing is owed after the payment is matched"
    );
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "a settled account is not an Androhung candidate"
    );

    // The sweep closes the case, so it stops feeding the escalation chain too.
    let closed = pg::settle_paid_dunning_cases(&pool, TENANT).await.unwrap();
    assert!(closed >= 1, "the settled case is resolved");
    let resolved: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT resolved_at FROM dunning_cases WHERE id = $1")
            .bind(case)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(resolved.is_some(), "resolved_at is set");
}

/// Mahngebühren and Verzugszinsen are **Verzugsschaden**, not the supply debt,
/// so they may not push a customer over the § 41f Abs. 3 threshold — otherwise
/// the dunning process manufactures its own justification: a customer 5 EUR
/// short of the floor crosses it on the Stufe-2 fee, charged *because* they were
/// being dunned.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn dunning_fees_do_not_count_toward_the_disconnection_threshold() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    // 95 EUR of supply debt — below the 100 EUR floor of §41f Abs. 3 S. 2.
    let case = seed_stufe3(&pool, &ledger, &malo, 9_500).await;
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "95 EUR is below the 100 EUR floor"
    );

    // Two Mahngebühren of 10 EUR take the *balance* to 115 EUR.
    for n in 1..=2 {
        pg::post_entry(
            &ledger,
            &pool,
            TENANT,
            &malo,
            "LF1",
            "MAHNGEBUEHR",
            1_000,
            &format!("fee-{malo}-{n}"),
            None,
            None,
            date!(2026 - 02 - 01),
            date!(2026 - 02 - 01),
            Some("Mahngebühr"),
            None,
        )
        .await
        .unwrap();
    }
    let acct = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        acct.balance_ct, 11_500,
        "the balance cache does include the fees — it is the amount demanded"
    );
    assert_eq!(
        pg::compute_verzug_ct(&ledger, &pool, TENANT, &malo, "LF1")
            .await
            .unwrap(),
        9_500,
        "…but the §41f Abs. 3 Zahlungsverzug is the supply debt alone"
    );
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "dunning fees must not carry a customer over the disconnection threshold"
    );
}

/// § 41g Abs. 1 S. 11 — a broken Abwendungsvereinbarung lets the sequence resume,
/// but only after a **fresh** § 41f Abs. 5 announcement.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_broken_abwendungsvereinbarung_reopens_the_sequence() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    pg::mark_sperrandrohung(&pool, case, TENANT).await.unwrap();
    pg::mark_sperrankuendigung(&pool, case, TENANT, date!(2026 - 01 - 08))
        .await
        .unwrap();
    let lock = pg::place_dunning_lock(
        &pool,
        case,
        TENANT,
        pg::LockGrund::Abwendungsvereinbarung,
        None,
        None,
        None,
        None,
        Some("op-1"),
    )
    .await
    .unwrap()
    .expect("lock placed");

    // Halted: the announced date has passed, but the agreement bars disconnection.
    assert!(
        !contains(
            &pg::list_sperrauftrag_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "§41g Abs. 1 S. 10 — an accepted agreement bars the disconnection"
    );

    // The customer stops keeping it.
    let account_id = pg::lift_dunning_lock(&pool, lock, TENANT, "vereinbarung_gebrochen")
        .await
        .unwrap()
        .expect("the lock is lifted");
    pg::clear_ankuendigung(&pool, account_id, TENANT)
        .await
        .unwrap();

    // The sequence resumes — but NOT straight to a Sperrauftrag. §41f Abs. 5 has
    // to be observed again, so the case is back at the Ankündigung.
    assert!(
        !contains(
            &pg::list_sperrauftrag_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "the stale announcement must not be reused — Abs. 5 requires a new one"
    );
    assert!(
        contains(
            &pg::list_ankuendigung_candidates(&pool, TENANT, 0, 10_000)
                .await
                .unwrap(),
            case
        ),
        "the case is an Ankündigung candidate again"
    );
}

/// A **Mahnsperre expires**, and the sequence resumes on its own.
///
/// § 41f Abs. 2 makes the Gefahr *auf Verlangen glaubhaft zu machen*: reviewable,
/// and capable of lapsing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_lock_with_an_end_date_stops_halting_when_it_lapses() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;

    // A Schutzbedürftigkeit substantiated by a certificate valid to yesterday.
    let lock = pg::place_dunning_lock(
        &pool,
        case,
        TENANT,
        pg::LockGrund::Schutzbeduerftigkeit,
        None,
        Some("ärztliches Attest"),
        // A certificate that covered 2019 and has since lapsed.
        Some(date!(2019 - 01 - 01)),
        Some(date!(2020 - 01 - 01)),
        Some("op-1"),
    )
    .await
    .unwrap()
    .expect("lock placed");
    assert!(
        contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "a lapsed lock no longer halts the sequence"
    );

    // An open-ended one does, and keeps doing so.
    pg::place_dunning_lock(
        &pool,
        case,
        TENANT,
        pg::LockGrund::Schutzbeduerftigkeit,
        None,
        Some("Folgeattest, unbefristet"),
        None,
        None,
        Some("op-1"),
    )
    .await
    .unwrap()
    .expect("second lock placed");
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "an active lock halts the sequence"
    );

    // …and it is listed for review rather than left to be forgotten.
    let review = pg::list_locks_due_review(&pool, TENANT, 0).await.unwrap();
    assert_eq!(review.len(), 1, "only the open-ended lock is up for review");
    assert_eq!(review[0].valid_to, None);
    // The lapsed one is not a review item — it ended by itself.
    assert!(review.iter().all(|l| l.lock_id != lock));
}

/// A lock records **why**, and lifting it records why it stopped applying.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_lock_carries_its_ground_and_its_lifting_reason() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    let lock = pg::place_dunning_lock(
        &pool,
        case,
        TENANT,
        pg::LockGrund::Zahlungsaussicht,
        None,
        Some("Kunde hat Ratenzahlung zugesagt"),
        None,
        None,
        Some("clerk-7"),
    )
    .await
    .unwrap()
    .unwrap();

    let locks = pg::list_dunning_locks(&pool, case, TENANT).await.unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].grund, "zahlungsaussicht");
    assert_eq!(
        locks[0].rechtsgrundlage, "§41f Abs. 1 S. 2 EnWG",
        "the citation defaults from the ground"
    );
    assert_eq!(locks[0].created_by.as_deref(), Some("clerk-7"));
    assert!(locks[0].aufgehoben_at.is_none());

    pg::lift_dunning_lock(&pool, lock, TENANT, "zusage nicht eingehalten")
        .await
        .unwrap()
        .expect("lifted");
    let locks = pg::list_dunning_locks(&pool, case, TENANT).await.unwrap();
    assert!(locks[0].aufgehoben_at.is_some());
    assert_eq!(
        locks[0].aufhebung_grund.as_deref(),
        Some("zusage nicht eingehalten"),
        "the history says why it stopped applying, not just that it did"
    );
    // Lifting is once.
    assert!(
        pg::lift_dunning_lock(&pool, lock, TENANT, "nochmal")
            .await
            .unwrap()
            .is_none()
    );
}

/// § 41f Abs. 3 S. 3–5 — a **disputed claim** leaves the Verzug, and the
/// sequence stops by itself when what is left falls below the threshold.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_disputed_claim_leaves_the_disconnection_threshold() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    let account_id = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap()
        .account_id;
    assert!(
        contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "150 EUR of undisputed supply debt opens the sequence"
    );

    // The customer disputes 100 EUR of it, form- und fristgerecht.
    let einwand = pg::record_einwand(
        &pool,
        TENANT,
        account_id,
        pg::EinwandArt::ForderungBestritten,
        10_000,
        None,
        Some("Widerspruch vom 03.03.2026"),
        Some("clerk-7"),
    )
    .await
    .unwrap();
    let verzug = pg::refresh_verzug(&ledger, &pool, TENANT, &malo, "LF1")
        .await
        .unwrap();
    assert_eq!(verzug, 5_000, "the disputed amount leaves the Verzug");
    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "50 EUR undisputed is below the §41f Abs. 3 S. 2 floor — no disconnection"
    );

    // The objection is rejected. The amount is owed after all.
    pg::close_einwand(&pool, einwand, TENANT, "zurueckgewiesen")
        .await
        .unwrap()
        .expect("closed");
    let verzug = pg::refresh_verzug(&ledger, &pool, TENANT, &malo, "LF1")
        .await
        .unwrap();
    assert_eq!(verzug, 15_000);
    assert!(
        contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "a rejected objection puts the amount back in the Verzug"
    );
}

/// An objection larger than the debt leaves nothing owed, not a credit.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_objection_cannot_drive_the_verzug_negative() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    let account_id = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap()
        .account_id;
    pg::record_einwand(
        &pool,
        TENANT,
        account_id,
        pg::EinwandArt::Schlichtung,
        99_000,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        pg::refresh_verzug(&ledger, &pool, TENANT, &malo, "LF1")
            .await
            .unwrap(),
        0,
        "nothing is owed; the customer is not thereby in credit"
    );
}

/// Verzugszinsen book to Zinserträge, not to the Mahngebühren account:
/// § 275 HGB reports *Zinsen und ähnliche Erträge* on their own line.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn verzugszinsen_book_to_their_own_gl_account() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq_malo();
    let account_id = pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        50_000,
        &uniq("inv"),
        None,
        None,
        date!(2026 - 01 - 01),
        date!(2026 - 01 - 01),
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "MAHNGEBUEHR",
        500,
        &uniq("fee"),
        None,
        None,
        date!(2026 - 02 - 01),
        date!(2026 - 02 - 01),
        Some("Mahngebühr"),
        None,
    )
    .await
    .unwrap();
    pg::create_interest_charge(
        &ledger,
        &pool,
        account_id,
        TENANT,
        &malo,
        "LF1",
        None,
        50_000,
        false,
        date!(2026 - 02 - 01),
        date!(2026 - 05 - 01),
    )
    .await
    .unwrap();

    let tb = ledger.trial_balance().await.unwrap();
    let zins = tb
        .iter()
        .find(|l| l.account.contains("Zinsertraege"))
        .expect("Zinsertraege is its own trial-balance line");
    let mahn = tb
        .iter()
        .find(|l| l.account.contains("Mahnerloese"))
        .expect("Mahnerloese line");
    assert!(zins.credits_ct > 0, "the interest landed in Zinsertraege");
    assert_eq!(
        mahn.credits_ct, 500,
        "…and the Mahngebühr account holds only the fee"
    );
}

/// The EPC **36-month dormancy** clock, and that it runs on presentation.
///
/// A mandate not presented for 36 consecutive months must be cancelled by the
/// creditor. The debtor banks do not enforce it, so an untracked creditor learns
/// of it from a rejected batch.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_mandate_unused_for_36_months_stops_being_collectable() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq_malo();
    let mandatsref = uniq("MND");
    pg::create_mandate(
        &pool,
        TENANT,
        None,
        pg::CreateMandateRequest {
            malo_id: malo.clone(),
            lf_mp_id: "LF1".to_owned(),
            iban: "DE89370400440532013000".to_owned(),
            bic: None,
            kontoinhaber: Some("Muster".to_owned()),
            mandatsref: mandatsref.clone(),
            sequence_type: "RCUR".to_owned(),
            scheme: "CORE".to_owned(),
            signed_at: "2020-01-01".to_owned(),
            debtor_address: accountingd::sepa::AddressParts::default(),
        },
    )
    .await
    .unwrap();

    // Never presented: not dormant — that is a FRST mandate waiting to be used.
    assert_eq!(
        pg::list_active_mandates(&pool, TENANT, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        pg::list_dormant_mandates(&pool, TENANT, 0)
            .await
            .unwrap()
            .is_empty(),
        "a mandate that has never been used is not dormant"
    );

    // Presented 37 months ago.
    sqlx::query(
        "UPDATE sepa_mandates SET last_presented_at = now() - INTERVAL '37 months' \
         WHERE tenant = $1 AND mandatsref = $2",
    )
    .bind(TENANT)
    .bind(&mandatsref)
    .execute(&pool)
    .await
    .unwrap();

    assert!(
        pg::list_active_mandates(&pool, TENANT, 100)
            .await
            .unwrap()
            .is_empty(),
        "a dormant mandate must not be collected on"
    );
    assert_eq!(
        pg::list_dormant_mandates(&pool, TENANT, 0)
            .await
            .unwrap()
            .len(),
        1,
        "…and it is surfaced so it can be cancelled"
    );

    // A presentation resets the clock — including one that later bounces, which
    // is why the stamp is on presentation and not on settlement.
    sqlx::query(
        "UPDATE sepa_mandates SET last_presented_at = now() \
         WHERE tenant = $1 AND mandatsref = $2",
    )
    .bind(TENANT)
    .bind(&mandatsref)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        pg::list_active_mandates(&pool, TENANT, 100)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// § 41f Abs. 7 — a disconnected customer who settles is reconnected
/// *unverzüglich*, and without having to ask.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn settling_after_disconnection_orders_the_reconnection() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    pg::mark_sperrauftrag_dispatched(&pool, case, TENANT, "process-1")
        .await
        .unwrap();

    // Still in arrears: nothing to reconnect.
    assert!(
        !contains(
            &pg::list_entsperrauftrag_candidates(&pool, TENANT)
                .await
                .unwrap(),
            case
        ),
        "a customer still in Verzug is not an Entsperrauftrag candidate"
    );

    // They pay.
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "ZAHLUNG",
        -15_000,
        &uniq("payment"),
        None,
        None,
        date!(2026 - 03 - 01),
        date!(2026 - 03 - 01),
        Some("Überweisung"),
        None,
    )
    .await
    .unwrap();
    pg::settle_paid_dunning_cases(&pool, TENANT).await.unwrap();

    assert!(
        contains(
            &pg::list_entsperrauftrag_candidates(&pool, TENANT)
                .await
                .unwrap(),
            case
        ),
        "§41f Abs. 7 — the grounds are gone, so the reconnection is owed unverzüglich"
    );
    pg::mark_entsperrauftrag_dispatched(&pool, case, TENANT, "process-2")
        .await
        .unwrap();
    assert!(
        !contains(
            &pg::list_entsperrauftrag_candidates(&pool, TENANT)
                .await
                .unwrap(),
            case
        ),
        "a dispatched Entsperrauftrag is not re-ordered (idempotent)"
    );
}

/// The Sperrauftrag must not fire before the announced disconnection date
/// (§41f Abs. 5 — 8 Werktage im Voraus).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn sperrauftrag_waits_for_the_announced_date() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    pg::mark_sperrandrohung(&pool, case, TENANT).await.unwrap();
    // Announce a date far in the future.
    pg::mark_sperrankuendigung(&pool, case, TENANT, date!(2099 - 01 - 01))
        .await
        .unwrap();
    let sa = pg::list_sperrauftrag_candidates(&pool, TENANT, 10_000)
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
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
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

    pg::place_dunning_lock(
        &pool,
        case,
        TENANT,
        pg::LockGrund::Abwendungsvereinbarung,
        None,
        None,
        None,
        None,
        Some("op-1"),
    )
    .await
    .unwrap()
    .expect("lock placed");
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
            &pg::list_ankuendigung_candidates(&pool, TENANT, 0, 10_000)
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
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    pg::place_dunning_lock(
        &pool,
        case,
        TENANT,
        pg::LockGrund::Schutzbeduerftigkeit,
        None,
        Some("ärztliches Attest vom 12.02.2026"),
        None,
        None,
        Some("op-1"),
    )
    .await
    .unwrap()
    .expect("lock placed");
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
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 5_000).await; // 50 € < 100 € threshold
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
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await; // 150 € > 100 € floor
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
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await; // 150 € > 100 € floor
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
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 500_000).await; // 5000 € arrears
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

// ── Event announcements ───────────────────────────────────────────────────────

/// Opening a Mahnstufe case must announce `de.accounting.mahnung.issued`.
///
/// The event was declared in the catalog, documented as emitted, and subscribed
/// to by `agentd`'s payment-reconciliation agent — but nothing produced it.
/// `MAHNUNG_ISSUED` appeared in `pg.rs` only as the *correlation string* on the
/// Mahngebühr ledger entry, which is not an emission, so the agent never ran.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn opening_a_dunning_case_announces_the_mahnstufe() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let account_id = pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();

    let case_id = pg::create_dunning_case_announced(
        &pool,
        TENANT,
        account_id,
        &malo,
        "LF1",
        2,
        4_500,
        date!(2026 - 07 - 15),
    )
    .await
    .expect("case created");

    let row: Option<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT ce_type, envelope FROM event_outbox \
         WHERE ce_type = $1 AND envelope->'data'->>'malo_id' = $2",
    )
    .bind(mako_events::accounting::MAHNUNG_ISSUED)
    .bind(&malo)
    .fetch_optional(&pool)
    .await
    .expect("outbox query");

    let (ce_type, envelope) = row.expect(
        "opening a dunning case must enqueue de.accounting.mahnung.issued — \
         agentd triggers on it",
    );
    assert_eq!(ce_type, "de.accounting.mahnung.issued");
    let data = &envelope["data"];
    assert_eq!(data["mahnstufe"], 2);
    assert_eq!(data["amount_due_ct"], 4_500);
    assert_eq!(
        data["amount_eur"], "45.00",
        "agents read amount_eur — it must be the ct value, not a re-scaled one"
    );
    assert_eq!(data["case_id"], case_id.to_string());
}

/// The case row and its announcement must be written in one transaction.
///
/// The outbox is persist-before-dispatch: a case opened without its event would
/// escalate a customer toward disconnection while nothing downstream heard.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_dunning_case_and_its_announcement_are_atomic() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let account_id = pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();

    pg::create_dunning_case_announced(
        &pool,
        TENANT,
        account_id,
        &malo,
        "LF1",
        1,
        1_000,
        date!(2026 - 07 - 15),
    )
    .await
    .expect("case created");

    let cases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dunning_cases WHERE account_id = $1 AND tenant = $2",
    )
    .bind(account_id)
    .bind(TENANT)
    .fetch_one(&pool)
    .await
    .unwrap();
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox \
         WHERE ce_type = $1 AND envelope->'data'->>'malo_id' = $2",
    )
    .bind(mako_events::accounting::MAHNUNG_ISSUED)
    .bind(&malo)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(cases, 1);
    assert_eq!(
        events, cases,
        "one announcement per case, never a case alone"
    );
}

/// A second interest charge for the same period must not duplicate the row.
///
/// The MAHNGEBUEHR ledger entry is idempotent on `interest:{malo}:{from}:{to}`,
/// so a retry left the ledger correct — while `interest_charges` grew a second
/// row and `GET /interest-charges` showed the customer the same Verzugszinsen
/// twice. The table now carries a unique key, and the row, the announcement and
/// the guard commit together.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn interest_for_the_same_period_is_charged_once() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let account_id = pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();

    let first = pg::create_interest_charge(
        &ledger,
        &pool,
        account_id,
        TENANT,
        &malo,
        "LF1",
        Some("RE-2026-001"),
        50_000,
        false,
        date!(2026 - 02 - 01),
        date!(2026 - 03 - 01),
    )
    .await
    .expect("first charge books");

    let second = pg::create_interest_charge(
        &ledger,
        &pool,
        account_id,
        TENANT,
        &malo,
        "LF1",
        Some("RE-2026-001"),
        50_000,
        false,
        date!(2026 - 02 - 01),
        date!(2026 - 03 - 01),
    )
    .await
    .expect("replay must not error");

    assert_eq!(
        first.id, second.id,
        "a replay must return the existing charge, not create a second"
    );

    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM interest_charges WHERE tenant = $1 AND account_id = $2",
    )
    .bind(TENANT)
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows, 1,
        "the customer must not be charged twice for one period"
    );

    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox \
         WHERE ce_type = $1 AND envelope->'data'->>'malo_id' = $2",
    )
    .bind(mako_events::accounting::INTEREST_CHARGED)
    .bind(&malo)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1, "and announced exactly once");
}

/// A re-announced Abschlag drops at the outbox.
///
/// `post_entry` is idempotent, so a second run in the same month is a ledger
/// no-op — but it returns `Ok`, so announcing on that alone re-sent the event
/// every time the 23-hour scheduler drifted across midnight. The CloudEvent id
/// is derived from the same `(MaLo, month)` key the ledger uses, and
/// `outbox::enqueue` is `ON CONFLICT (event_id) DO NOTHING`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_replayed_abschlag_is_announced_once() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let ref_id = format!("ABSCHLAG-{malo}-2026-07");

    for _ in 0..3 {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("accountingd", TENANT),
            mako_events::accounting::ABSCHLAG_POSTED,
            &malo,
            serde_json::json!({ "malo_id": malo, "amount_ct": 12_000 }),
        )
        .with_id(ref_id.clone());
        let mut tx = pool.begin().await.unwrap();
        mako_service::outbox::enqueue(&mut tx, &ce).await.unwrap();
        tx.commit().await.unwrap();
    }

    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox \
         WHERE ce_type = $1 AND envelope->'data'->>'malo_id' = $2",
    )
    .bind(mako_events::accounting::ABSCHLAG_POSTED)
    .bind(&malo)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        events, 1,
        "three scheduler passes in one month must announce the advance payment once"
    );
}

// ── IBAN payment matching ─────────────────────────────────────────────────────

/// Registering a SEPA mandate must write `accounts.iban_hash`, because that is
/// the only key CAMT.054 import resolves an account by. It was read but never
/// written, so every incoming payment fell to unmatched and dunning could fire
/// against customers who had already paid.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_mandate_makes_the_account_findable_by_iban() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let key = accountingd::ledger::iban_hash_key("test-secret");
    let iban = "DE89 3704 0044 0532 0130 00";

    pg::create_mandate(
        &pool,
        TENANT,
        Some(&key),
        pg::CreateMandateRequest {
            malo_id: malo.clone(),
            lf_mp_id: "LF1".to_owned(),
            iban: iban.to_owned(),
            bic: None,
            kontoinhaber: Some("Erika Mustermann".to_owned()),
            mandatsref: uniq("MND"),
            sequence_type: "FRST".to_owned(),
            scheme: "CORE".to_owned(),
            signed_at: "2026-01-15".to_owned(),
            debtor_address: accountingd::sepa::AddressParts {
                town: Some("Berlin".to_owned()),
                country: Some("DE".to_owned()),
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();

    // The CAMT.054 lookup, verbatim — a bank statement quotes the IBAN without
    // spaces, so the stored hash must be over the normalised form.
    let hit: Option<(String, String)> = sqlx::query_as(
        "SELECT malo_id, lf_mp_id FROM accounts WHERE iban_hash = $1 AND tenant = $2 LIMIT 1",
    )
    .bind(accountingd::ledger::iban_hash(
        Some(&key),
        "de89370400440532013000",
    ))
    .bind(TENANT)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(
        hit,
        Some((malo.clone(), "LF1".to_owned())),
        "the mandate's IBAN must resolve the account"
    );

    // A different key must not match — the hash is keyed, not a plain digest.
    let other = accountingd::ledger::iban_hash_key("other-secret");
    let miss: Option<String> = sqlx::query_scalar(
        "SELECT malo_id FROM accounts WHERE iban_hash = $1 AND tenant = $2 LIMIT 1",
    )
    .bind(accountingd::ledger::iban_hash(
        Some(&other),
        "DE89370400440532013000",
    ))
    .bind(TENANT)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(miss, None);
}

/// Updating an account's IBAN re-keys the lookup hash along with it — otherwise
/// a customer who changes bank stops matching after the first statement.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn changing_the_iban_rekeys_the_lookup_hash() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let key = accountingd::ledger::iban_hash_key("test-secret");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();

    pg::update_account_tenanted(
        &pool,
        &malo,
        "LF1",
        TENANT,
        Some(&key),
        pg::UpdateAccountRequest {
            iban: Some("DE02120300000000202051".to_owned()),
            mandatsref: None,
            abschlag_ct: None,
            billing_day: None,
            address: Default::default(),
        },
    )
    .await
    .unwrap();

    let stored: Option<String> =
        sqlx::query_scalar("SELECT iban_hash FROM accounts WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo)
            .bind(TENANT)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        Some(accountingd::ledger::iban_hash(
            Some(&key),
            "DE02120300000000202051"
        ))
    );

    // An unrelated field update leaves the hash intact (COALESCE, not overwrite).
    pg::update_account_tenanted(
        &pool,
        &malo,
        "LF1",
        TENANT,
        None,
        pg::UpdateAccountRequest {
            iban: None,
            mandatsref: None,
            abschlag_ct: Some(9_900),
            billing_day: None,
            address: Default::default(),
        },
    )
    .await
    .unwrap();
    let after: Option<String> =
        sqlx::query_scalar("SELECT iban_hash FROM accounts WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo)
            .bind(TENANT)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, stored, "an Abschlag update must not clear the hash");
}

// ── Jahresabschluss settlement ────────────────────────────────────────────────

/// The settlement is the year's whole Kontokorrent movement, so it cannot miss
/// a Buchungsart. Direct payments, chargebacks and the Abschlag pair all reach
/// it — and the figure it produces is by construction the balance it settles.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_settlement_equals_the_balance_it_settles() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let d = date!(2026 - 06 - 15);

    let post = async |kind: &str, amount: i64, key: String| {
        pg::post_entry(
            &ledger,
            &pool,
            TENANT,
            &malo,
            "LF1",
            kind,
            amount,
            &key,
            None,
            None,
            d,
            d,
            Some(kind),
            None,
        )
        .await
        .unwrap();
    };

    // 1200.00 billed gross; 1100.00 demanded as advances and discharged by that
    // invoice; 1100.00 collected, of which 100.00 bounced; 200.00 paid directly.
    post("RECHNUNG", 120_000, uniq("inv")).await;
    post("ABSCHLAG", 110_000, uniq("abs")).await;
    post("ABSCHLAG_VERRECHNUNG", -110_000, uniq("ver")).await;
    post("ZAHLUNG", -110_000, uniq("col")).await;
    post("BANKRUECKLAST", 10_000, uniq("ret")).await;
    post("ZAHLUNG", -20_000, uniq("pay")).await;

    let sums = ledger.year_kind_sums("LF1", &malo, 2026).await.unwrap();
    let s = accountingd::handlers::JahresabschlussSums::from_kind_sums(&sums);

    assert_eq!(s.rechnung_sum, 120_000);
    assert_eq!(s.abschlag_sum, 0, "demanded and then discharged");
    assert_eq!(
        s.zahlung_sum, -120_000,
        "1100 collected + 200 direct − 100 charged back"
    );
    assert_eq!(s.settlement_ct, 0, "ausgeglichen");
    assert_eq!(
        s.settlement_ct,
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        "the settlement must equal the Kontokorrent balance it settles"
    );
    assert_eq!(
        s.rechnung_sum + s.abschlag_sum + s.zahlung_sum + s.verzugsschaden_sum + s.sonstige_sum,
        s.settlement_ct,
        "the buckets partition the total"
    );
}

// ── SEPA collection lifecycle: pain.008 → pain.002 / camt → pain.007 ─────────

/// Build a one-mandate collection run and persist it with its entries.
///
/// Returns `(run_id, entry, malo)`. The mandate carries a structured address so
/// the emitted pain.008 also exercises the `PstlAdr` path end to end.
async fn seed_collection(
    pool: &PgPool,
    collection_date: time::Date,
    amount_ct: i64,
) -> (Uuid, pg::CollectionEntryRow, String) {
    let malo = uniq("MALO");
    let mandatsref = uniq("MND");
    pg::create_mandate(
        pool,
        TENANT,
        None,
        pg::CreateMandateRequest {
            malo_id: malo.clone(),
            lf_mp_id: "LF1".to_owned(),
            iban: "DE89370400440532013000".to_owned(),
            bic: Some("COBADEFFXXX".to_owned()),
            kontoinhaber: Some("Erika Mustermann".to_owned()),
            mandatsref: mandatsref.clone(),
            sequence_type: "RCUR".to_owned(),
            scheme: "CORE".to_owned(),
            signed_at: "2024-01-15".to_owned(),
            debtor_address: accountingd::sepa::AddressParts {
                town: Some("Hamburg".to_owned()),
                country: Some("DE".to_owned()),
                street: Some("Deichstrasse".to_owned()),
                building_number: Some("7".to_owned()),
                post_code: Some("20459".to_owned()),
                country_subdivision: None,
            },
        },
    )
    .await
    .unwrap();

    let mandates = pg::list_active_mandates(pool, TENANT, 100).await.unwrap();
    let mandate = mandates
        .into_iter()
        .find(|m| m.mandatsref == mandatsref)
        .expect("the mandate we just created");
    let creditor_address = accountingd::sepa::AddressParts {
        town: Some("Berlin".to_owned()),
        country: Some("DE".to_owned()),
        ..Default::default()
    };
    let run = accountingd::sepa::build_pain_008(
        &accountingd::sepa::CreditorIdentity {
            iban: "DE89370400440532013000",
            name: "Test Energie GmbH",
            creditor_id: "DE98ZZZ09999999999",
            address: Some(&creditor_address),
        },
        collection_date,
        &[(&mandate, amount_ct)],
        Default::default(),
    )
    .expect("pain.008 builds");
    assert!(
        run.xml.contains("<TwnNm>Hamburg</TwnNm>"),
        "the mandate's structured address reaches the wire"
    );

    let run_id = pg::persist_sepa_collection(pool, TENANT, collection_date, &run)
        .await
        .unwrap();
    let entry = pg::find_collection_entry_by_e2e(pool, TENANT, &mandatsref)
        .await
        .unwrap()
        .expect("the collected entry is persisted");
    (run_id, entry, malo)
}

/// The run row is the archive; the entry rows are what attributes a bank reply.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn collection_run_persists_its_entries() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let d = date!(2026 - 07 - 25);
    let (run_id, entry, malo) = seed_collection(&pool, d, 12_000).await;

    assert_eq!(entry.run_id, run_id);
    assert_eq!(entry.amount_ct, 12_000);
    assert_eq!(entry.status, "SUBMITTED");
    assert_eq!(entry.sequence_type, "RCUR");
    assert_eq!(entry.collection_date, d);
    assert_eq!(entry.msg_id, "DD-2026-07-25");
    assert_eq!(entry.payment_info_id, "DD-2026-07-25-CORE-RCUR");
    assert_eq!(entry.malo_id.as_deref(), Some(malo.as_str()));
    // The IBAN is reached through the mandate, not duplicated onto the entry,
    // so GDPR erasure keeps working from one place.
    assert_eq!(entry.debtor_iban.as_deref(), Some("DE89370400440532013000"));
    assert_eq!(entry.mandate_signed_at, Some(date!(2024 - 01 - 15)));

    let listed = pg::list_collection_entries(&pool, TENANT, Some("SUBMITTED"), Some(&malo), 10)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].entry_id, entry.entry_id);
}

/// Regenerating a run for the same collection date replaces its entries.
///
/// A stale entry from a superseded batch would claim a collection that is not in
/// the file the bank received — and would then be reversible.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn regenerating_a_run_replaces_its_entries() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let d = date!(2026 - 08 - 10);
    let (run_id, entry, _malo) = seed_collection(&pool, d, 5_000).await;

    // A second mandate collected on the same date supersedes the first run.
    let (run_id2, entry2, _malo2) = seed_collection(&pool, d, 7_000).await;
    assert_eq!(run_id, run_id2, "one run per (tenant, collection_date)");

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sepa_collection_entries WHERE run_id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 1, "the superseded batch's entries are gone");
    assert_eq!(entry2.amount_ct, 7_000);
    assert!(
        pg::fetch_collection_entry(&pool, entry.entry_id, TENANT)
            .await
            .unwrap()
            .is_none(),
        "the first run's entry no longer exists"
    );
}

/// A status only advances out of `SUBMITTED` once.
///
/// A settled collection that is later returned is a separate R-transaction; a
/// second pain.002 for an entry that already moved on must not rewrite history.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn collection_status_advances_once() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let (_run_id, entry, _malo) = seed_collection(&pool, date!(2026 - 09 - 05), 9_900).await;

    assert!(
        pg::set_collection_entry_status(&pool, entry.entry_id, "SETTLED", None)
            .await
            .unwrap()
    );
    assert!(
        !pg::set_collection_entry_status(&pool, entry.entry_id, "REJECTED", Some("AC01"))
            .await
            .unwrap(),
        "a settled collection is not re-opened by a second report"
    );
    let after = pg::fetch_collection_entry(&pool, entry.entry_id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.status, "SETTLED");
}

/// A settled collection can be reversed exactly once, and the reversal restates
/// the original transaction reference from stored data.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn reversal_is_recorded_once_per_collection() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let d = date!(2026 - 10 - 02);
    let (_run_id, entry, malo) = seed_collection(&pool, d, 12_000).await;
    pg::set_collection_entry_status(&pool, entry.entry_id, "SETTLED", None)
        .await
        .unwrap();
    let entry = pg::fetch_collection_entry(&pool, entry.entry_id, TENANT)
        .await
        .unwrap()
        .unwrap();

    let creditor = accountingd::sepa::CreditorIdentity {
        iban: "DE89370400440532013000",
        name: "Test Energie GmbH",
        creditor_id: "DE98ZZZ09999999999",
        address: None,
    };
    let reversal = accountingd::sepa::build_pain_007(
        &creditor,
        &[accountingd::sepa::ReversalRequest {
            original_msg_id: &entry.msg_id,
            original_payment_info_id: &entry.payment_info_id,
            original_end_to_end_id: &entry.end_to_end_id,
            original_amount_ct: entry.amount_ct,
            reversed_amount_ct: None,
            reason: accountingd::sepa::ReversalReason::Am05,
            mandate_ref: &entry.mandatsref,
            mandate_signed_at: entry.mandate_signed_at.unwrap(),
            collection_date: entry.collection_date,
            sequence_type: &entry.sequence_type,
            scheme: &entry.scheme,
            debtor_name: entry.debtor_name.as_deref().unwrap(),
            debtor_iban: entry.debtor_iban.as_deref().unwrap(),
            debtor_bic: entry.debtor_bic.as_deref(),
        }],
        Default::default(),
    )
    .expect("pain.007 builds from stored data alone");
    assert!(reversal.xml.contains(&entry.msg_id));
    assert!(reversal.xml.contains(&entry.mandatsref));

    // The compensating entry re-opens the receivable: the money leaves the bank
    // account again, so what the collection discharged is owed once more.
    let ledger_id = pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "SEPA_STORNO",
        entry.amount_ct,
        &format!("sepa-reversal:{}", entry.entry_id),
        None,
        Some(&entry.end_to_end_id),
        d,
        d,
        Some("pain.007 Storno"),
        None,
    )
    .await
    .unwrap();

    let id = pg::record_sepa_reversal(
        &pool,
        TENANT,
        &entry,
        &reversal,
        entry.amount_ct,
        "AM05",
        Some(ledger_id),
        Some("operator@example.test"),
    )
    .await
    .unwrap();
    assert!(!id.is_nil());

    // The unique index on collection_entry_id is what stops a second request
    // refunding the same collection twice.
    assert!(
        pg::record_sepa_reversal(
            &pool,
            TENANT,
            &entry,
            &reversal,
            entry.amount_ct,
            "AM05",
            Some(ledger_id),
            None,
        )
        .await
        .is_err(),
        "a second reversal of the same collection must be refused"
    );

    assert_eq!(
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        12_000,
        "the reversal re-opens the receivable"
    );
}

/// `bank_import_log` keeps the bank's own batch attribution.
///
/// `Btch/PmtInfId` is what matches a booked collection back to the group that
/// was submitted, without guessing from amounts and dates.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn bank_import_records_the_batch_attribution() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let txn = uniq("acctsvcr");
    pg::record_bank_import(
        &pool,
        TENANT,
        &txn,
        12_000,
        Some("DE89370400440532013000"),
        date!(2026 - 07 - 27),
        None,
        Some("DD-2026-07-25-CORE-RCUR"),
        Some("MND-000123"),
    )
    .await
    .unwrap();

    let (pmt_inf, e2e): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT payment_info_id, end_to_end_id FROM bank_import_log \
         WHERE tenant = $1 AND bank_transaction_id = $2",
    )
    .bind(TENANT)
    .bind(&txn)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pmt_inf.as_deref(), Some("DD-2026-07-25-CORE-RCUR"));
    assert_eq!(e2e.as_deref(), Some("MND-000123"));
}

/// An account's postal address round-trips through the update path.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn account_postal_address_round_trips() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    pg::update_account_tenanted(
        &pool,
        &malo,
        "LF1",
        TENANT,
        None,
        pg::UpdateAccountRequest {
            iban: None,
            mandatsref: None,
            abschlag_ct: None,
            billing_day: None,
            address: accountingd::sepa::AddressParts {
                town: Some("Leipzig".to_owned()),
                country: Some("DE".to_owned()),
                street: Some("Karl-Liebknecht-Strasse".to_owned()),
                building_number: Some("3".to_owned()),
                post_code: Some("04107".to_owned()),
                country_subdivision: Some("SN".to_owned()),
            },
        },
    )
    .await
    .unwrap();

    let acct = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap();
    let address = acct.postal_address();
    assert_eq!(address.town.as_deref(), Some("Leipzig"));
    assert_eq!(address.country.as_deref(), Some("DE"));
    let postal = address
        .to_postal_address()
        .expect("a complete address builds")
        .expect("some address");
    assert_eq!(postal.town_name(), "Leipzig");
    assert_eq!(postal.country(), "DE");

    // An omitted part leaves the stored value alone — the same COALESCE shape
    // every other field on this request has.
    pg::update_account_tenanted(
        &pool,
        &malo,
        "LF1",
        TENANT,
        None,
        pg::UpdateAccountRequest {
            iban: None,
            mandatsref: None,
            abschlag_ct: Some(4_200),
            billing_day: None,
            address: Default::default(),
        },
    )
    .await
    .unwrap();
    let acct = pg::fetch_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(acct.abschlag_ct, 4_200);
    assert_eq!(
        acct.postal_address().town.as_deref(),
        Some("Leipzig"),
        "an unrelated update must not erase the address"
    );
}

// ── Resolving an incoming payment to an account ──────────────────────────────

/// The counterparty IBAN is the strongest evidence and is used first.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn payment_resolves_by_counterparty_iban() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let key = accountingd::ledger::iban_hash_key("test-secret");
    let iban = "DE89370400440532013000";
    let malo = uniq("MALO");
    pg::create_mandate(
        &pool,
        TENANT,
        Some(&key),
        pg::CreateMandateRequest {
            malo_id: malo.clone(),
            lf_mp_id: "LF1".to_owned(),
            iban: iban.to_owned(),
            bic: None,
            kontoinhaber: Some("Erika Mustermann".to_owned()),
            mandatsref: uniq("MND"),
            sequence_type: "FRST".to_owned(),
            scheme: "CORE".to_owned(),
            signed_at: "2026-01-15".to_owned(),
            debtor_address: Default::default(),
        },
    )
    .await
    .unwrap();

    let hit = pg::resolve_account_for_payment(
        &pool,
        TENANT,
        pg::PaymentClues {
            iban_hash: Some(&accountingd::ledger::iban_hash(Some(&key), iban)),
            end_to_end_id: None,
            remittance: None,
        },
    )
    .await
    .unwrap()
    .expect("the mandate's IBAN resolves the account");
    assert_eq!(hit.malo_id, malo);
    assert_eq!(hit.matched_by, "iban");
}

/// A payment from an account nobody has on file still books, when the reference
/// names the customer.
///
/// This is the single biggest reconciliation gap in a retail ledger: a customer
/// paying from a spouse's, an employer's, or a second account produces a
/// transaction with an unknown IBAN. Matching on the IBAN alone loses it, and
/// the receivable stays open against someone who has already paid.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn payment_from_a_stranger_iban_resolves_by_reference() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let key = accountingd::ledger::iban_hash_key("test-secret");
    let malo = uniq_malo();
    let mandatsref = uniq("MND");
    pg::create_mandate(
        &pool,
        TENANT,
        Some(&key),
        pg::CreateMandateRequest {
            malo_id: malo.clone(),
            lf_mp_id: "LF1".to_owned(),
            iban: "DE89370400440532013000".to_owned(),
            bic: None,
            kontoinhaber: Some("Erika Mustermann".to_owned()),
            mandatsref: mandatsref.clone(),
            sequence_type: "RCUR".to_owned(),
            scheme: "CORE".to_owned(),
            signed_at: "2026-01-15".to_owned(),
            debtor_address: Default::default(),
        },
    )
    .await
    .unwrap();

    // The spouse's account — a valid IBAN that hashes to nothing on file.
    let stranger = accountingd::ledger::iban_hash(Some(&key), "DE02120300000000202051");
    let hit = pg::resolve_account_for_payment(
        &pool,
        TENANT,
        pg::PaymentClues {
            iban_hash: Some(&stranger),
            end_to_end_id: None,
            remittance: Some(&format!("Abschlag Strom {mandatsref} Danke")),
        },
    )
    .await
    .unwrap()
    .expect("the Mandatsreferenz in the reference identifies the account");
    assert_eq!(hit.malo_id, malo);
    assert_eq!(hit.matched_by, "remittance_token");

    // The MaLo-ID works the same way.
    let hit = pg::resolve_account_for_payment(
        &pool,
        TENANT,
        pg::PaymentClues {
            iban_hash: Some(&stranger),
            end_to_end_id: None,
            remittance: Some(&format!("Zahlung fuer {malo}")),
        },
    )
    .await
    .unwrap()
    .expect("the MaLo-ID in the reference identifies the account");
    assert_eq!(hit.malo_id, malo);
}

/// The free-text rung matches whole tokens only, never substrings.
///
/// A `LIKE '%…%'` scan would match a Mandatsreferenz that merely happens to be a
/// prefix of another and book a stranger's payment onto a customer's account.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn reference_matching_is_exact_token_not_substring() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let mandatsref = uniq("MND");
    pg::create_mandate(
        &pool,
        TENANT,
        None,
        pg::CreateMandateRequest {
            malo_id: malo.clone(),
            lf_mp_id: "LF1".to_owned(),
            iban: "DE89370400440532013000".to_owned(),
            bic: None,
            kontoinhaber: None,
            mandatsref: mandatsref.clone(),
            sequence_type: "RCUR".to_owned(),
            scheme: "CORE".to_owned(),
            signed_at: "2026-01-15".to_owned(),
            debtor_address: Default::default(),
        },
    )
    .await
    .unwrap();

    // A longer reference that *contains* the stored one as a prefix belongs to
    // somebody else. A substring match would book this onto the wrong account.
    let longer = format!("{mandatsref}9");
    assert!(
        pg::resolve_account_for_payment(
            &pool,
            TENANT,
            pg::PaymentClues {
                iban_hash: None,
                end_to_end_id: None,
                remittance: Some(&format!("Zahlung {longer}")),
            },
        )
        .await
        .unwrap()
        .is_none(),
        "a reference that merely contains the stored one must not match"
    );
}

/// A reference naming two customers is not guessed at.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn ambiguous_reference_resolves_to_nothing() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let a = uniq_malo();
    let b = uniq_malo();
    for malo in [&a, &b] {
        pg::upsert_account(&pool, malo, "LF1", TENANT)
            .await
            .unwrap();
    }
    assert!(
        pg::resolve_account_for_payment(
            &pool,
            TENANT,
            pg::PaymentClues {
                iban_hash: None,
                end_to_end_id: None,
                remittance: Some(&format!("Sammelzahlung {a} {b}")),
            },
        )
        .await
        .unwrap()
        .is_none(),
        "two accounts named means neither can be booked"
    );
}

/// A returned collection is attributed by the EndToEndId the bank echoes back,
/// even when the debtor's stored IBAN has since changed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn return_resolves_by_end_to_end_id() {
    let Some((pool, _ledger, _pg)) = setup().await else {
        return;
    };
    let d = date!(2026 - 11 - 20);
    let (_run_id, entry, malo) = seed_collection(&pool, d, 8_800).await;

    let hit = pg::resolve_account_for_payment(
        &pool,
        TENANT,
        pg::PaymentClues {
            // The bank reported no counterparty IBAN at all.
            iban_hash: None,
            end_to_end_id: Some(&entry.end_to_end_id),
            remittance: None,
        },
    )
    .await
    .unwrap()
    .expect("the collection's EndToEndId identifies the account");
    assert_eq!(hit.malo_id, malo);
    assert_eq!(hit.matched_by, "end_to_end_id");
}

/// Tokenisation keeps whole identifiers and the separator-stripped whole text.
#[test]
fn remittance_tokens_cover_both_spellings() {
    let tokens = pg::remittance_tokens("RF18 5390 0754 7034");
    assert!(
        tokens.contains(&"RF18539007547034".to_owned()),
        "an RF reference keyed in groups must also be looked up whole: {tokens:?}"
    );
    // Fragments under four characters carry no identifying power.
    assert!(!tokens.iter().any(|t| t.len() < 4));

    // The two-word run is what matches a Mandatsreferenz stored as `MND-000123`.
    let tokens = pg::remittance_tokens("Abschlag 07/2026 MND-000123, danke!");
    assert!(tokens.contains(&"MND000123".to_owned()), "{tokens:?}");
    assert!(tokens.contains(&"000123".to_owned()));
    assert!(tokens.contains(&"ABSCHLAG".to_owned()));
    // Runs longer than four words are sentences, not identifiers.
    assert!(!tokens.contains(&"ABSCHLAG072026MND000123DANKE".to_owned()));
}

// ── Jahresabschluss automation (§ 40b Abs. 1 EnWG) ────────────────────────────

/// The annual worker settles exactly the accounts with no settlement for the
/// year, keyed on the absence of a `jahresabschluss_runs` row — the same table
/// the endpoint's idempotency guard writes, so a manual `POST` and the worker
/// cannot disagree about what is outstanding.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_annual_sweep_settles_each_account_once() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let other = uniq("MALO");
    pg::upsert_account(&pool, &other, "LF1", TENANT)
        .await
        .unwrap();

    let due = |year: i16| {
        let pool = pool.clone();
        async move {
            pg::list_jahresabschluss_candidates(&pool, TENANT, year, 100)
                .await
                .unwrap()
                .into_iter()
                .map(|(m, _)| m)
                .collect::<Vec<_>>()
        }
    };
    let before = due(2026).await;
    assert!(before.contains(&malo) && before.contains(&other));

    // Settle one — a balanced year, so no money moves and no IBAN is needed.
    pg::record_jahresabschluss(&pool, TENANT, &malo, 2026, 0, 0, 0, None)
        .await
        .unwrap();

    let after = due(2026).await;
    assert!(
        !after.contains(&malo),
        "a settled account drops out of the candidate set"
    );
    assert!(after.contains(&other), "an unsettled one still is");
    assert!(
        due(2025).await.contains(&malo),
        "the guard is per year — 2025 is still outstanding"
    );

    // An anonymised account is excluded: GDPR Art. 17 destroyed the attribution
    // a settlement would be addressed to.
    sqlx::query("UPDATE accounts SET anonymized_at = now() WHERE malo_id = $1 AND tenant = $2")
        .bind(&other)
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!due(2026).await.contains(&other));

    // And the ledger is untouched by any of this.
    assert_eq!(ledger.balance_ct("LF1", &malo).await.unwrap(), 0);
}

/// The settlement runs end to end against a real ledger: a balanced year books
/// no entry, records its run, and announces itself — whatever the outcome.
///
/// The announcement is the point. `de.accounting.erstattung.faellig` fires only
/// on a refund *and* only with an ERP webhook, so anything downstream of the
/// annual cycle had to poll `jahresabschluss_runs` to notice a Nachzahlung or a
/// balanced year had happened at all.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_committed_settlement_announces_itself() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, TENANT, TENANT)
        .await
        .unwrap();
    let d = date!(2026 - 03 - 01);
    // Billed and paid in full: ausgeglichen, so no money moves and no IBAN is
    // needed — and the refund event does not fire, which is the whole point.
    for (kind, amount, key) in [("RECHNUNG", 120_000, "inv"), ("ZAHLUNG", -120_000, "pay")] {
        pg::post_entry(
            &ledger,
            &pool,
            TENANT,
            &malo,
            TENANT,
            kind,
            amount,
            &uniq(key),
            None,
            None,
            d,
            d,
            Some(kind),
            None,
        )
        .await
        .unwrap();
    }

    // A minimal config: every other field is optional, and a balanced year
    // touches none of the SEPA or makod ones.
    let cfg: std::sync::Arc<accountingd::config::AccountingdConfig> = std::sync::Arc::new(
        serde_json::from_value(serde_json::json!({
            "database": { "url": "unused-by-this-path" },
            "tenant":   TENANT,
        }))
        .expect("a minimal accountingd config deserialises"),
    );
    let q = accountingd::handlers::JahresabschlussQuery {
        lf_mp_id: Some(TENANT.to_owned()),
        year: Some(2026),
        dry_run: Some(false),
    };
    let ledger_arc = std::sync::Arc::new(ledger);
    let body = accountingd::handlers::settle_jahresabschluss(&pool, &ledger_arc, &cfg, &malo, &q)
        .await
        .expect("a balanced year settles");
    assert_eq!(body["action"], "AUSGEGLICHEN");
    assert_eq!(body["settlement_ct"], 0);
    assert_eq!(body["committed"], true);

    let announced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox WHERE ce_type = $1 AND event_id = $2",
    )
    .bind(mako_events::accounting::JAHRESABSCHLUSS_ABGESCHLOSSEN)
    .bind(format!("jahresabschluss:{malo}:2026"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(announced, 1, "every committed settlement announces itself");

    // Idempotent: the second call returns the prior result and enqueues nothing.
    let again = accountingd::handlers::settle_jahresabschluss(&pool, &ledger_arc, &cfg, &malo, &q)
        .await
        .expect("re-run");
    assert_eq!(again["already_settled"], true);
    let announced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox WHERE ce_type = $1 AND event_id = $2",
    )
    .bind(mako_events::accounting::JAHRESABSCHLUSS_ABGESCHLOSSEN)
    .bind(format!("jahresabschluss:{malo}:2026"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(announced, 1, "and does not announce a second time");
}

/// A period statement opens at the balance the period started from.
///
/// § 666 BGB gives the customer an account of the supplier's dealings, and a
/// March Kontoauszug that starts at zero tells them their February balance was
/// nil. The three figures have to close: `eroeffnungssaldo + bewegung =
/// schlusssaldo`, and the closing figure is the period's, not today's.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_period_statement_opens_at_the_prior_balance() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();

    let post = async |amount: i64, on: time::Date| {
        pg::post_entry(
            &ledger,
            &pool,
            TENANT,
            &malo,
            "LF1",
            "RECHNUNG",
            amount,
            &uniq("e"),
            None,
            None,
            on,
            on,
            None,
            None,
        )
        .await
        .unwrap();
    };

    // 300.00 billed in January and February, 150.00 in March, 90.00 in April.
    post(10_000, date!(2026 - 01 - 20)).await;
    post(20_000, date!(2026 - 02 - 10)).await;
    post(15_000, date!(2026 - 03 - 05)).await;
    post(9_000, date!(2026 - 04 - 02)).await;

    let march = pg::list_ledger(
        &ledger,
        "LF1",
        &malo,
        doubleentry::BalanceQuery::between(date!(2026 - 03 - 01), date!(2026 - 03 - 31)),
        500,
    )
    .await
    .unwrap();

    assert_eq!(
        march.lines.len(),
        1,
        "only March's movement belongs on a March statement"
    );
    assert_eq!(
        march.opening_ct, 30_000,
        "the period opens at January + February"
    );
    let bewegung: i64 = march.lines.iter().map(|l| l.signed_ct).sum();
    assert_eq!(bewegung, 15_000);
    assert_eq!(
        march.opening_ct + bewegung,
        45_000,
        "the statement closes at the period's balance, not at April's"
    );
    // …and the running balance the store reports on the last line is that same
    // figure, so the page adds up against its own arithmetic.
    assert_eq!(march.lines[0].running_ct, 45_000);

    // Whole-account: nothing came before, so it opens at zero.
    let all = pg::list_ledger(&ledger, "LF1", &malo, doubleentry::BalanceQuery::all(), 500)
        .await
        .unwrap();
    assert_eq!(all.opening_ct, 0);
    assert_eq!(all.lines.len(), 4);

    // A `limit` that cuts the front folds what it dropped into the opening
    // rather than losing it: the truncated page still adds up.
    let tail = pg::list_ledger(&ledger, "LF1", &malo, doubleentry::BalanceQuery::all(), 2)
        .await
        .unwrap();
    assert_eq!(tail.lines.len(), 2);
    assert_eq!(tail.opening_ct, 30_000, "January + February were cut off");
    let tail_sum: i64 = tail.lines.iter().map(|l| l.signed_ct).sum();
    assert_eq!(tail.opening_ct + tail_sum, 54_000);
}
