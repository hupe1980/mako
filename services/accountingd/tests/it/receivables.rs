//! The receivables lifecycle against a live PostgreSQL: instalment plans, the
//! § 288 Abs. 5 BGB Pauschale and the Insolvenz state.
//!
//! `#[ignore]`d like `db_scenarios`, whose harness these tests share:
//!
//! ```bash
//! cargo test -p accountingd --test it -- --include-ignored receivables::
//! ```

use accountingd::pg;
use time::macros::date;

use crate::db_scenarios::{TENANT, contains, seed_stufe3, setup, uniq};

/// Mahngebühren and Zahlungsfristen for the dunning runs under test: Stufe 1
/// gives ten days, so a test can tell a configured period from any other.
pub(crate) const STUFEN: pg::Mahnstufen = pg::Mahnstufen {
    gebuehr_ct: [500, 1000, 1500],
    zahlungsfrist_tage: [10, 12, 5],
};

/// Book a payment of `amount_ct` on `malo` (`LF1`) dated today.
async fn pay(
    pool: &sqlx::PgPool,
    ledger: &accountingd::ledger::PgLedger,
    malo: &str,
    amount_ct: i64,
) -> uuid::Uuid {
    let today = mako_fristen::heute();
    pg::post_entry(
        ledger,
        pool,
        TENANT,
        malo,
        "LF1",
        "ZAHLUNG",
        -amount_ct,
        &uniq("pay"),
        None,
        None,
        today,
        today,
        Some("Rate"),
        None,
    )
    .await
    .unwrap()
}

/// A plan with instalments of `rate_ct` from `first_due`, optionally the
/// Ratenzahlung of an Abwendungsvereinbarung lock.
async fn plan_with(
    pool: &sqlx::PgPool,
    malo: &str,
    total_ct: i64,
    rate_ct: i64,
    first_due: time::Date,
    lock: Option<uuid::Uuid>,
) -> anyhow::Result<uuid::Uuid> {
    pg::create_payment_plan(
        pool,
        TENANT,
        pg::CreatePaymentPlanRequest {
            malo_id: malo.to_owned(),
            lf_mp_id: Some("LF1".to_owned()),
            total_ct,
            installment_ct: rate_ct,
            billing_day: 1,
            first_due_date: first_due.to_string(),
            dunning_case_id: None,
            abwendung_lock_id: lock,
            note: None,
            operator_sub: None,
        },
    )
    .await
}

async fn statuses(pool: &sqlx::PgPool, plan: uuid::Uuid) -> (String, Vec<String>) {
    let (p, inst) = pg::get_payment_plan_with_installments(pool, plan, TENANT)
        .await
        .unwrap()
        .unwrap();
    (p.status, inst.into_iter().map(|i| i.status).collect())
}

/// A plan for `total_ct` in one instalment due on `due`, on the account
/// `seed_stufe3` opened (`lf_mp_id` `LF1`).
async fn plan_for(pool: &sqlx::PgPool, malo: &str, total_ct: i64, due: &str) -> uuid::Uuid {
    pg::create_payment_plan(
        pool,
        TENANT,
        pg::CreatePaymentPlanRequest {
            malo_id: malo.to_owned(),
            lf_mp_id: Some("LF1".to_owned()),
            total_ct,
            installment_ct: total_ct,
            billing_day: 1,
            first_due_date: due.to_owned(),
            dunning_case_id: None,
            abwendung_lock_id: None,
            note: None,
            operator_sub: None,
        },
    )
    .await
    .unwrap()
}

/// An account with an `ACTIVE` instalment plan is not walked toward
/// disconnection: the customer is paying under an agreement, and escalating
/// them would disconnect someone who is keeping it. A plan that is no longer
/// `ACTIVE` stops shielding the account.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_active_payment_plan_keeps_the_account_out_of_the_sperr_sequence() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let case = seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    let plan = plan_for(&pool, &malo, 15_000, "2099-01-01").await;

    assert!(
        !contains(
            &pg::list_androhung_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "an ACTIVE plan keeps the case out of the Androhung"
    );
    pg::mark_sperrandrohung(&pool, case, TENANT).await.unwrap();
    assert!(
        !contains(
            &pg::list_ankuendigung_candidates(&pool, TENANT, 0, 10_000)
                .await
                .unwrap(),
            case
        ),
        "…out of the Ankündigung"
    );
    pg::mark_sperrankuendigung(&pool, case, TENANT, date!(2026 - 01 - 08))
        .await
        .unwrap();
    assert!(
        !contains(
            &pg::list_sperrauftrag_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "…and out of the Sperrauftrag"
    );

    pg::cancel_payment_plan(&pool, plan, TENANT, Some("op-1"))
        .await
        .unwrap();
    assert!(
        contains(
            &pg::list_sperrauftrag_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "a cancelled plan no longer shields the account"
    );
}

/// A payment marks the instalments it covers `PAID`, naming the payment; a
/// payment covering part of the next one leaves that one `PENDING`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_payment_marks_the_instalments_it_covers_paid() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    let today = mako_fristen::heute();
    let plan = plan_with(&pool, &malo, 15_000, 5_000, today, None)
        .await
        .unwrap();
    let entry = pay(&pool, &ledger, &malo, 7_000).await;

    pg::run_payment_plans(&pool, TENANT).await.unwrap();

    let (p, inst) = pg::get_payment_plan_with_installments(&pool, plan, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p.status, "ACTIVE");
    assert_eq!(inst[0].status, "PAID", "the instalment due today is paid");
    assert_eq!(inst[0].ledger_entry_id, Some(entry), "…by that payment");
    assert!(inst[0].paid_at.is_some());
    assert_eq!(
        inst[1].status, "PENDING",
        "2 000 of 5 000 does not pay the next"
    );

    pay(&pool, &ledger, &malo, 8_000).await;
    pg::run_payment_plans(&pool, TENANT).await.unwrap();
    assert_eq!(
        statuses(&pool, plan).await,
        ("COMPLETED".to_owned(), vec!["PAID".to_owned(); 3]),
        "the whole plan paid is COMPLETED"
    );
}

/// An instalment past due and unpaid turns `OVERDUE` and its plan `DEFAULTED`;
/// a plan paid on time stays `ACTIVE`; a `CANCELLED` plan is never defaulted.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_missed_instalment_defaults_the_plan_and_a_paid_one_does_not() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let yesterday = mako_fristen::heute() - time::Duration::days(1);

    let missed = uniq("MALO");
    seed_stufe3(&pool, &ledger, &missed, 15_000).await;
    let missed_plan = plan_with(&pool, &missed, 15_000, 5_000, yesterday, None)
        .await
        .unwrap();

    let kept = uniq("MALO");
    seed_stufe3(&pool, &ledger, &kept, 15_000).await;
    let kept_plan = plan_with(&pool, &kept, 15_000, 5_000, yesterday, None)
        .await
        .unwrap();
    pay(&pool, &ledger, &kept, 5_000).await;

    let cancelled = uniq("MALO");
    seed_stufe3(&pool, &ledger, &cancelled, 15_000).await;
    let cancelled_plan = plan_with(&pool, &cancelled, 15_000, 5_000, yesterday, None)
        .await
        .unwrap();
    pg::cancel_payment_plan(&pool, cancelled_plan, TENANT, Some("op-1"))
        .await
        .unwrap();

    pg::run_payment_plans(&pool, TENANT).await.unwrap();

    let (status, inst) = statuses(&pool, missed_plan).await;
    assert_eq!(status, "DEFAULTED");
    assert_eq!(inst, vec!["OVERDUE", "PENDING", "PENDING"]);
    let (status, inst) = statuses(&pool, kept_plan).await;
    assert_eq!(
        status, "ACTIVE",
        "an instalment paid on time keeps the plan"
    );
    assert_eq!(inst[0], "PAID");
    assert_eq!(statuses(&pool, cancelled_plan).await.0, "CANCELLED");
}

/// A defaulted plan that is the Ratenzahlung of an Abwendungsvereinbarung
/// breaks the agreement by itself (§ 41g Abs. 1 S. 11 EnWG): the lock is
/// lifted with `vereinbarung_gebrochen`, the Ankündigung cleared, and the case
/// is an Ankündigung candidate again — with no operator call.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_defaulted_abwendungsvereinbarung_reopens_the_sequence_by_itself() {
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
    let yesterday = mako_fristen::heute() - time::Duration::days(1);
    let plan = plan_with(&pool, &malo, 15_000, 5_000, yesterday, Some(lock))
        .await
        .unwrap();

    pg::run_payment_plans(&pool, TENANT).await.unwrap();

    assert_eq!(statuses(&pool, plan).await.0, "DEFAULTED");
    let (grund,): (Option<String>,) =
        sqlx::query_as("SELECT aufhebung_grund FROM dunning_locks WHERE lock_id = $1")
            .bind(lock)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(grund.as_deref(), Some("vereinbarung_gebrochen"));
    assert!(
        !contains(
            &pg::list_sperrauftrag_candidates(&pool, TENANT, 10_000)
                .await
                .unwrap(),
            case
        ),
        "the stale Ankündigung is not reused"
    );
    assert!(
        contains(
            &pg::list_ankuendigung_candidates(&pool, TENANT, 0, 10_000)
                .await
                .unwrap(),
            case
        ),
        "the case returns to a fresh § 41f Abs. 5 Ankündigung"
    );
    let announced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox WHERE ce_type = $1 AND envelope->>'subject' = $2",
    )
    .bind(mako_events::accounting::ABWENDUNG_GEBROCHEN)
    .bind(&malo)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(announced, 1, "the breach is announced in the same commit");
}

/// The link names an agreement the account actually holds.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_plan_cannot_link_a_lock_it_does_not_hold() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let other = uniq("MALO");
    let other_case = seed_stufe3(&pool, &ledger, &other, 15_000).await;
    let foreign = pg::place_dunning_lock(
        &pool,
        other_case,
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
    .unwrap();
    let malo = uniq("MALO");
    seed_stufe3(&pool, &ledger, &malo, 15_000).await;
    assert!(
        plan_with(
            &pool,
            &malo,
            15_000,
            5_000,
            mako_fristen::heute(),
            Some(foreign)
        )
        .await
        .is_err()
    );
}

/// The automatic run writes the Zahlungsfrist the operator configured for the
/// Stufe onto the Mahnung.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_configured_zahlungsfrist_reaches_the_mahnung() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let malo = uniq("MALO");
    let account_id = pg::upsert_account(&pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let billed = mako_fristen::heute() - time::Duration::days(60);
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        20_000,
        &uniq("ce"),
        None,
        None,
        billed,
        billed,
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();

    let run = pg::run_auto_dunning(&ledger, &pool, TENANT, 14, &STUFEN)
        .await
        .unwrap();
    assert_eq!(run.mahnstufe1_created, 1);
    let due: time::Date = sqlx::query_scalar(
        "SELECT due_date FROM dunning_cases WHERE account_id = $1 AND stufe = 1",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(due, mako_fristen::heute() + time::Duration::days(10));
}

/// An account in Insolvenz (§ 87 InsO) receives no Mahnstufe: automatic
/// dunning neither opens Mahnstufe 1 nor escalates an open case.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_account_in_insolvenz_receives_no_mahnstufe() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let billed = mako_fristen::heute() - time::Duration::days(60);
    let insolvent = |malo: String| {
        let (pool, ledger) = (&pool, &ledger);
        async move {
            let account_id = pg::upsert_account(pool, &malo, "LF1", TENANT)
                .await
                .unwrap();
            pg::post_entry(
                ledger,
                pool,
                TENANT,
                &malo,
                "LF1",
                "RECHNUNG",
                20_000,
                &uniq("ce"),
                None,
                None,
                billed,
                billed,
                Some("Jahresrechnung"),
                None,
            )
            .await
            .unwrap();
            pg::place_account_lock(
                pool,
                account_id,
                TENANT,
                pg::LockGrund::Insolvenz,
                None,
                Some("AG Musterstadt 1 IN 1/26"),
                None,
                None,
                Some("op-1"),
            )
            .await
            .unwrap()
            .expect("the account exists");
            account_id
        }
    };
    let fresh = insolvent(uniq("MALO")).await;
    let escalating = insolvent(uniq("MALO")).await;
    pg::create_dunning_case(
        &pool,
        escalating,
        TENANT,
        1,
        20_000,
        mako_fristen::heute() - time::Duration::days(1),
    )
    .await
    .unwrap();

    pg::run_auto_dunning(&ledger, &pool, TENANT, 14, &STUFEN)
        .await
        .unwrap();

    let stufen: Vec<(uuid::Uuid, i16)> = sqlx::query_as(
        "SELECT account_id, stufe FROM dunning_cases WHERE account_id = ANY($1) ORDER BY stufe",
    )
    .bind(vec![fresh, escalating])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        stufen,
        vec![(escalating, 1)],
        "no Mahnstufe 1 is opened and the open case is not escalated"
    );
}

/// An account owing one overdue invoice of 200 EUR, its Verbraucher status
/// recorded as `verbraucher` (`None` = not recorded).
async fn debtor(
    pool: &sqlx::PgPool,
    ledger: &accountingd::ledger::PgLedger,
    verbraucher: Option<bool>,
) -> (String, uuid::Uuid) {
    let malo = uniq("MALO");
    let account_id = pg::upsert_account(pool, &malo, "LF1", TENANT)
        .await
        .unwrap();
    let billed = mako_fristen::heute() - time::Duration::days(60);
    pg::post_entry(
        ledger,
        pool,
        TENANT,
        &malo,
        "LF1",
        "RECHNUNG",
        20_000,
        &uniq("ce"),
        None,
        Some("RE-2026-0001"),
        billed,
        billed,
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();
    pg::set_verbraucher(pool, &malo, "LF1", TENANT, verbraucher)
        .await
        .unwrap();
    (malo, account_id)
}

/// Open Mahnstufe `stufe` on the account with `fee_ct`.
async fn mahnung(
    pool: &sqlx::PgPool,
    ledger: &accountingd::ledger::PgLedger,
    malo: &str,
    account_id: uuid::Uuid,
    stufe: i16,
    fee_ct: i64,
) {
    let today = mako_fristen::heute();
    pg::create_dunning_case_announced(
        pool,
        TENANT,
        account_id,
        malo,
        "LF1",
        stufe,
        20_000,
        today + time::Duration::days(14),
        Some(pg::MahnFee {
            ledger,
            fee_ct,
            on: today,
        }),
    )
    .await
    .unwrap();
}

/// The `VERZUGSPAUSCHALE` postings on `malo`: (claims, Anrechnungen), both as
/// positive sums.
async fn pauschale(pool: &sqlx::PgPool, malo: &str) -> (i64, i64) {
    let amounts: Vec<i64> = sqlx::query_scalar(
        "SELECT (entwurf->>'amount_ct')::bigint FROM booking_drafts \
         WHERE status = 'gebucht' AND entwurf->>'malo_id' = $1 \
           AND entwurf->>'entry_type' = 'VERZUGSPAUSCHALE'",
    )
    .bind(malo)
    .fetch_all(pool)
    .await
    .unwrap();
    (
        amounts.iter().filter(|&&a| a > 0).sum(),
        -amounts.iter().filter(|&&a| a < 0).sum::<i64>(),
    )
}

/// § 288 Abs. 5 S. 1 BGB: one Pauschale per Entgeltforderung against a
/// non-Verbraucher, however many Mahnungen follow; none against a Verbraucher
/// or a customer whose status nobody recorded.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_pauschale_is_claimed_once_per_claim_and_only_from_a_non_verbraucher() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let (gewerbe, gewerbe_id) = debtor(&pool, &ledger, Some(false)).await;
    mahnung(&pool, &ledger, &gewerbe, gewerbe_id, 1, 0).await;
    mahnung(&pool, &ledger, &gewerbe, gewerbe_id, 2, 0).await;
    assert_eq!(
        pauschale(&pool, &gewerbe).await,
        (pg::VERZUGSPAUSCHALE_CT, 0),
        "two Mahnungen, one claim: one Pauschale of 40 EUR"
    );

    for status in [Some(true), None] {
        let (malo, id) = debtor(&pool, &ledger, status).await;
        mahnung(&pool, &ledger, &malo, id, 1, 0).await;
        assert_eq!(pauschale(&pool, &malo).await, (0, 0), "{status:?}");
    }
}

/// § 288 Abs. 5 S. 3 BGB: the Pauschale is credited against the Mahngebühren
/// of the same Verzug, so the customer pays the larger of the two and never
/// both in full — and the Anrechnung never takes the Pauschale below zero.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_pauschale_and_the_mahngebuehren_are_not_both_charged_in_full() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let (malo, id) = debtor(&pool, &ledger, Some(false)).await;
    mahnung(&pool, &ledger, &malo, id, 1, 500).await;
    mahnung(&pool, &ledger, &malo, id, 2, 1_000).await;
    let (claimed, credited) = pauschale(&pool, &malo).await;
    assert_eq!(claimed, 4_000);
    assert_eq!(credited, 1_500, "both Mahngebühren are credited");
    assert_eq!(
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        20_000 + 4_000,
        "the claim plus max(Pauschale, Mahngebühren)"
    );

    let (malo, id) = debtor(&pool, &ledger, Some(false)).await;
    mahnung(&pool, &ledger, &malo, id, 1, 3_000).await;
    mahnung(&pool, &ledger, &malo, id, 2, 3_000).await;
    assert_eq!(
        pauschale(&pool, &malo).await,
        (4_000, 4_000),
        "the Anrechnung stops at the Pauschale"
    );
    assert_eq!(
        ledger.balance_ct("LF1", &malo).await.unwrap(),
        20_000 + 6_000
    );
}

/// The Pauschale is neither Umsatzerlös nor Zins (§ 275 Abs. 2 Nr. 1 / Nr. 11
/// HGB) and not part of the § 41f Abs. 3 EnWG Zahlungsverzug.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_pauschale_is_neither_revenue_nor_interest_nor_arrears() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let (malo, id) = debtor(&pool, &ledger, Some(false)).await;
    let before = pg::refresh_verzug(&ledger, &pool, TENANT, &malo, "LF1")
        .await
        .unwrap();
    let line = |tb: &[accountingd::ledger::TrialBalanceLine], name: &str| {
        tb.iter()
            .find(|l| l.account.contains(name))
            .map_or(0, |l| l.credits_ct)
    };
    let tb = ledger.trial_balance().await.unwrap();
    let (erloese, zinsen) = (line(&tb, "Energieerloese"), line(&tb, "Zinsertraege"));

    mahnung(&pool, &ledger, &malo, id, 1, 0).await;

    assert_eq!(
        pg::refresh_verzug(&ledger, &pool, TENANT, &malo, "LF1")
            .await
            .unwrap(),
        before,
        "the § 41f Abs. 3 arrears do not move"
    );
    let tb = ledger.trial_balance().await.unwrap();
    assert_eq!(line(&tb, "Energieerloese"), erloese);
    assert_eq!(line(&tb, "Zinsertraege"), zinsen);
    assert_eq!(line(&tb, "Verzugspauschalen"), pg::VERZUGSPAUSCHALE_CT);
}

/// Automatic dunning holds back on an account with an `ACTIVE` payment plan or
/// a live Mahnsperre of any ground: it opens no Mahnstufe 1, escalates no open
/// case and charges no Mahngebühr. A `DEFAULTED` plan or a lifted lock
/// re-opens dunning.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_plan_or_a_lock_holds_automatic_dunning_back_until_it_ends() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let today = mako_fristen::heute();
    // An account owing an aged invoice, with an open Mahnstufe-1 case past due.
    let overdue = |malo: String| {
        let (pool, ledger) = (&pool, &ledger);
        async move {
            let account_id = pg::upsert_account(pool, &malo, "LF1", TENANT)
                .await
                .unwrap();
            let billed = today - time::Duration::days(60);
            pg::post_entry(
                ledger,
                pool,
                TENANT,
                &malo,
                "LF1",
                "RECHNUNG",
                20_000,
                &uniq("ce"),
                None,
                None,
                billed,
                billed,
                Some("Jahresrechnung"),
                None,
            )
            .await
            .unwrap();
            let case = pg::create_dunning_case(
                pool,
                account_id,
                TENANT,
                1,
                20_000,
                today - time::Duration::days(1),
            )
            .await
            .unwrap();
            (malo, account_id, case)
        }
    };
    let (planned, planned_id, _) = overdue(uniq("MALO")).await;
    let plan = plan_with(
        &pool,
        &planned,
        20_000,
        20_000,
        today + time::Duration::days(30),
        None,
    )
    .await
    .unwrap();
    let mut locked = Vec::new();
    for grund in [
        pg::LockGrund::Abwendungsvereinbarung,
        pg::LockGrund::Schutzbeduerftigkeit,
        pg::LockGrund::Zahlungsaussicht,
        pg::LockGrund::Insolvenz,
    ] {
        let (malo, id, case) = overdue(uniq("MALO")).await;
        let lock = pg::place_dunning_lock(
            &pool,
            case,
            TENANT,
            grund,
            None,
            None,
            None,
            None,
            Some("op-1"),
        )
        .await
        .unwrap()
        .unwrap();
        locked.push((malo, id, lock));
    }
    // A locked account with no case yet gets no Mahnstufe 1 either.
    let fresh = uniq("MALO");
    let fresh_id = pg::upsert_account(&pool, &fresh, "LF1", TENANT)
        .await
        .unwrap();
    let billed = today - time::Duration::days(60);
    pg::post_entry(
        &ledger,
        &pool,
        TENANT,
        &fresh,
        "LF1",
        "RECHNUNG",
        20_000,
        &uniq("ce"),
        None,
        None,
        billed,
        billed,
        Some("Jahresrechnung"),
        None,
    )
    .await
    .unwrap();
    pg::place_account_lock(
        &pool,
        fresh_id,
        TENANT,
        pg::LockGrund::Schutzbeduerftigkeit,
        None,
        None,
        None,
        None,
        Some("op-1"),
    )
    .await
    .unwrap()
    .unwrap();

    pg::run_auto_dunning(&ledger, &pool, TENANT, 14, &STUFEN)
        .await
        .unwrap();

    let max_stufe = |id: uuid::Uuid| {
        let pool = &pool;
        async move {
            sqlx::query_scalar::<_, Option<i16>>(
                "SELECT max(stufe) FROM dunning_cases WHERE account_id = $1",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
        }
    };
    let fees = |malo: String| {
        let pool = &pool;
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM booking_drafts WHERE entwurf->>'malo_id' = $1 \
                 AND entwurf->>'entry_type' = 'MAHNGEBUEHR'",
            )
            .bind(malo)
            .fetch_one(pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        max_stufe(planned_id).await,
        Some(1),
        "an ACTIVE plan holds Stufe 1"
    );
    assert_eq!(fees(planned.clone()).await, 0, "…and charges no Mahngebühr");
    for (malo, id, _) in &locked {
        assert_eq!(max_stufe(*id).await, Some(1), "a live lock holds the case");
        assert_eq!(fees(malo.clone()).await, 0);
    }
    assert_eq!(
        max_stufe(fresh_id).await,
        None,
        "a live lock opens no Mahnstufe 1"
    );

    // The plan defaults and a lock is lifted: dunning resumes on the next day's run.
    pg::cancel_payment_plan(&pool, plan, TENANT, Some("op-1"))
        .await
        .unwrap();
    pg::lift_dunning_lock(&pool, locked[1].2, TENANT, "entfallen")
        .await
        .unwrap();
    sqlx::query("DELETE FROM auto_dunning_runs WHERE tenant = $1")
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    pg::run_auto_dunning(&ledger, &pool, TENANT, 14, &STUFEN)
        .await
        .unwrap();
    assert_eq!(
        max_stufe(planned_id).await,
        Some(2),
        "a plan that ended re-opens dunning"
    );
    assert_eq!(
        max_stufe(locked[1].1).await,
        Some(2),
        "a lifted lock re-opens dunning"
    );
    assert_eq!(
        max_stufe(locked[0].1).await,
        Some(1),
        "a live lock still holds"
    );
}

/// § 288 BGB reads one fact — the account's recorded Verbraucher status (§ 13
/// BGB), synced from the vertragd contract — for both the rate and the
/// Pauschale: a Verbraucher pays five Prozentpunkte über dem Basiszinssatz
/// (Abs. 1) and no Pauschale, a business customer nine (Abs. 2) and the
/// Pauschale (Abs. 5). An unrecorded status charges neither the Abs. 2 rate
/// nor the Pauschale, because both rest on a fact nobody established.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_rate_and_the_pauschale_follow_the_recorded_verbraucher_status() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    for (status, basis, aufschlag, pauschale_ct) in [
        (Some(true), "§288 Abs. 1 BGB", 5, 0),
        (Some(false), "§288 Abs. 2 BGB", 9, pg::VERZUGSPAUSCHALE_CT),
        (None, "§288 Abs. 1 BGB", 5, 0),
    ] {
        let (malo, id) = debtor(&pool, &ledger, status).await;
        let row = pg::create_interest_charge(
            &ledger,
            &pool,
            id,
            TENANT,
            &malo,
            "LF1",
            None,
            100_000,
            date!(2026 - 02 - 01),
            date!(2026 - 05 - 01),
        )
        .await
        .unwrap();
        assert_eq!(row.legal_basis, basis, "{status:?}");
        // The period lies inside one Basiszinssatz half-year, so one segment.
        assert_eq!(
            row.rate_segments[0]["rate_pct"],
            (row.ecb_base_rate_pct + rust_decimal::Decimal::from(aufschlag))
                .normalize()
                .to_string(),
            "{status:?}: Basiszinssatz + {aufschlag} Prozentpunkte"
        );
        mahnung(&pool, &ledger, &malo, id, 1, 0).await;
        assert_eq!(
            pauschale(&pool, &malo).await,
            (pauschale_ct, 0),
            "{status:?}"
        );
    }
}

/// The account's Verbraucher status is the vertragd contract's: the master-data
/// PUT leaves it alone, and the contract's `null` clears it — a later contract
/// on the MaLo that records no status must not inherit the earlier one's „kein
/// Verbraucher".
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_stated_null_clears_the_verbraucher_status_and_master_data_keeps_it() {
    let Some((pool, ledger, _pg)) = setup().await else {
        return;
    };
    let (malo, id) = debtor(&pool, &ledger, Some(false)).await;
    let status = || async {
        sqlx::query_scalar::<_, Option<bool>>(
            "SELECT verbraucher FROM accounts WHERE account_id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let req: pg::UpdateAccountRequest =
        serde_json::from_value(serde_json::json!({ "abschlag_ct": 5_000 })).unwrap();
    pg::update_account_tenanted(&pool, &malo, "LF1", TENANT, None, req)
        .await
        .unwrap();
    assert_eq!(status().await, Some(false), "master data leaves the status");
    let req: pg::SetVerbraucherRequest =
        serde_json::from_value(serde_json::json!({ "verbraucher": null })).unwrap();
    pg::set_verbraucher(&pool, &malo, "LF1", TENANT, req.verbraucher.flatten())
        .await
        .unwrap();
    assert_eq!(status().await, None, "the contract's null clears it");
}
