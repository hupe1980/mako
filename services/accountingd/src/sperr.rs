//! §§ 41f / 41g EnWG — payment-default disconnection sequence (LF role).
//!
//! Since **23.12.2025** (BGBl. 2025 I Nr. 347, umsetzend EU-RL 2024/1711) the
//! disconnection of a Haushaltskunde for non-payment is governed by §§ 41f/41g
//! EnWG — **not** the repealed § 19 StromGVV/GasGVV, which now covers only the
//! illegal-use case.
//!
//! ## The sequence
//!
//! | Phase | Act | Frist | Norm |
//! |---|---|---|---|
//! | 1 | **Sperrandrohung** — threat of disconnection | ≥ 4 Wochen before the next step | § 41f Abs. 1 S. 1 |
//! | 2 | **Sperrankündigung** — the concrete date, brieflich | **8 Werktage** im Voraus | § 41f Abs. 5 |
//! | 3 | **Sperrauftrag** — ORDERS 17115 to the Netzbetreiber | on the announced date | § 41f Abs. 1 |
//! | 4 | **Entsperrauftrag** — ORDERS 17117, once the grounds are gone | *unverzüglich* | § 41f Abs. 7 |
//!
//! Both notices must state, prominently, the **Grund** of the interruption and the
//! **voraussichtlichen Unterbrechungs- und Wiederherstellungskosten** (§ 41f
//! Abs. 6); the Androhung additionally carries the no-extra-cost avoidance options
//! (§ 41f Abs. 4). Both ride the CloudEvent the ERP turns into the letter, so the
//! content is in the event rather than left for the ERP to invent.
//!
//! ## What halts the sequence
//!
//! * an accepted **Abwendungsvereinbarung** (§ 41g Abs. 1 S. 10) — and if the
//!   customer then breaks it, lifting the lock for `vereinbarung_gebrochen`
//!   applies § 41g Abs. 1 S. 11: the sequence resumes, but the Ankündigung is
//!   cleared so Abs. 5 is re-observed with a **fresh** 8-Werktage announcement;
//! * **Unverhältnismäßigkeit / besondere Schutzbedürftigkeit**
//!   (§ 41f Abs. 1 S. 2 / Abs. 2);
//! * the customer paying. Every phase applies the same § 41f Abs. 3 gates to
//!   `accounts.verzug_ct` — the ledger-derived open supply debt, less
//!   Verzugsschaden, less the Abs. 3 S. 3–5 objections — so a case that settles
//!   between two phases stops advancing (`pg::ABS3_GATES`,
//!   `pg::settle_paid_dunning_cases`);
//! * a **Mahnsperre** in `dunning_locks` — one mechanism for every halt, with a
//!   ground, a citation, a validity period and an operator.
//!
//! ## Why the Sperrauftrag is a market message
//!
//! Phases 3 and 4 dispatch `gpke.sperrung.beauftragen` (**ORDERS 17115**) and
//! `gpke.entsperrung.beauftragen` (**17117**) through `makod`. The Sperrauftrag
//! is a regulated LF→NB message: the NB answers it with ORDRSP 19116/19117 and
//! reports execution with IFTSTA 21039, and the LF's own `gpke-sperrung-lf`
//! process tracks that exchange. An HTTP call into the grid operator's own work
//! queue would produce none of it.
//!
//! The governing text is §§ 41f–41g EnWG in the consolidated version of
//! 23.12.2025 (BGBl. 2025 I Nr. 347).

use std::sync::Arc;

use mako_fristen::{self as fristen, HolidayCalendar};
use mako_markt::makod_client::{ForwardCommand, MakodClient};
use sqlx::PgPool;
use time::OffsetDateTime;

use crate::config::AccountingdConfig;
use crate::pg;

/// Per-run counters for observability.
#[derive(Debug, Default, Clone, Copy)]
pub struct SperrSummary {
    pub androhungen: u64,
    pub ankuendigungen: u64,
    pub sperrauftraege: u64,
    pub entsperrauftraege: u64,
}

/// § 41f Abs. 4 EnWG — the no-extra-cost avoidance options the Androhung must
/// name, in Textform. The statute lists them; the ERP renders them.
const VERMEIDUNGSMOEGLICHKEITEN: &[&str] = &[
    "Hilfsangebote der Kommune und der Sozialleistungsträger",
    "Vorauszahlungssystem",
    "Energieberatung",
    "Ratenzahlungsplan oder Stundung (Abwendungsvereinbarung)",
    "Sozialleistungen und die zuständige Behörde",
    "Schuldnerberatung",
];

/// Drive the §§ 41f/41g sequence one step forward for every qualifying case.
///
/// Idempotent and safe to run daily: each phase query excludes cases already past
/// that step, halted, or no longer in Verzug, so a re-run never re-issues a letter
/// or double-orders a disconnection.
///
/// # Errors
///
/// Propagates database and ledger errors. A single case whose dispatch fails is
/// logged and skipped; the run continues.
pub async fn run_sperr_sequence(
    pool: &PgPool,
    makod: &Arc<MakodClient>,
    cfg: &AccountingdConfig,
) -> anyhow::Result<SperrSummary> {
    let tenant = &cfg.tenant;
    let threshold = cfg.sperrung_threshold_ct.unwrap_or(10_000);
    let androhung_frist = cfg.sperrandrohung_frist_days.unwrap_or(28);
    let ankuendigung_wt =
        u32::try_from(cfg.sperrankuendigung_frist_werktage.unwrap_or(8)).unwrap_or(8);
    // § 41f Abs. 6 — both notices must state the expected costs. § 41f Abs. 7 S. 2
    // allows them to be a Pauschale, which is what an operator configures here.
    let sperrkosten = cfg.sperrkosten_ct.unwrap_or(0);
    let entsperrkosten = cfg.entsperrkosten_ct.unwrap_or(0);
    let mut summary = SperrSummary::default();

    // The Androhung and the Ankündigung are **legal acts** — letters the ERP has
    // to send, delivered off the emitted CloudEvent. Without a webhook there is
    // no dispatch path, so no case may be advanced toward disconnection: marking
    // the step here would let it reach the Sperrauftrag on schedule while the
    // customer never received the notice the statute requires.
    if cfg.erp_webhook_url.is_none() {
        tracing::warn!(
            "accountingd: no erp_webhook_url — §41f Sperrandrohung/Sperrankündigung cannot be \
             dispatched; the notice phases are paused (no case is advanced toward disconnection)"
        );
    } else {
        // ── Phase 1: Sperrandrohung (§41f Abs. 1) ───────────────────────────
        for (case_id, malo_id, lf_mp_id, verzug_ct) in
            pg::list_androhung_candidates(pool, tenant, threshold).await?
        {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("accountingd", tenant),
                mako_events::accounting::SPERRANDROHUNG,
                &malo_id,
                serde_json::json!({
                    "malo_id": malo_id,
                    "lf_mp_id": lf_mp_id,
                    "amount_due_ct": verzug_ct,
                    "rechtsgrundlage": "§41f Abs. 1 EnWG",
                    "ankuendigung_frist_wochen": 4,
                    // §41f Abs. 6 — Grund und voraussichtliche Kosten, klar und
                    // deutlich. The letter is invalid without both.
                    "grund": "Zahlungsverzug trotz Mahnung (§41f Abs. 1 S. 1 EnWG)",
                    "voraussichtliche_kosten_ct": {
                        "unterbrechung":   sperrkosten,
                        "wiederherstellung": entsperrkosten,
                    },
                    // §41f Abs. 4 — Vermeidungsmöglichkeiten ohne Zusatzkosten.
                    "vermeidungsmoeglichkeiten": VERMEIDUNGSMOEGLICHKEITEN,
                    // §41g Abs. 1 S. 1 — from here the Grundversorgungskunde may
                    // demand an Abwendungsvereinbarung offer.
                    "abwendungsvereinbarung_verlangbar": true,
                }),
            );
            let mut tx = pool.begin().await?;
            pg::mark_sperrandrohung(&mut *tx, case_id, tenant).await?;
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
            tx.commit().await?;
            summary.androhungen += 1;
            tracing::info!(%malo_id, verzug_ct, "accountingd: Sperrandrohung issued (§41f Abs. 1 EnWG)");
        }

        // ── Phase 2: Sperrankündigung (§41f Abs. 5) ─────────────────────────
        // The announced disconnection date is today + 8 Werktage.
        let today = OffsetDateTime::now_utc().date();
        let geplantes_sperrdatum =
            fristen::add_werktage(today, ankuendigung_wt, HolidayCalendar::BdewMaKo);
        for (case_id, malo_id, lf_mp_id, verzug_ct) in
            pg::list_ankuendigung_candidates(pool, tenant, androhung_frist, threshold).await?
        {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("accountingd", tenant),
                mako_events::accounting::SPERRANKUENDIGUNG,
                &malo_id,
                serde_json::json!({
                    "malo_id": malo_id,
                    "lf_mp_id": lf_mp_id,
                    "amount_due_ct": verzug_ct,
                    "rechtsgrundlage": "§41f Abs. 5 EnWG",
                    "geplantes_sperrdatum": geplantes_sperrdatum.to_string(),
                    "zustellung": "brieflich",
                    "grund": "Zahlungsverzug trotz Mahnung und Sperrandrohung (§41f Abs. 1 EnWG)",
                    "voraussichtliche_kosten_ct": {
                        "unterbrechung":   sperrkosten,
                        "wiederherstellung": entsperrkosten,
                    },
                    // §41g Abs. 1 S. 2 — at the latest with the Ankündigung, the
                    // Grundversorger must have offered an Abwendungsvereinbarung.
                    "abwendungsvereinbarung_angebot_faellig": true,
                }),
            );
            let mut tx = pool.begin().await?;
            pg::mark_sperrankuendigung(&mut *tx, case_id, tenant, geplantes_sperrdatum).await?;
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
            tx.commit().await?;
            summary.ankuendigungen += 1;
            tracing::info!(%malo_id, %geplantes_sperrdatum, "accountingd: Sperrankündigung issued (§41f Abs. 5 EnWG, 8 Werktage)");
        }
    }

    // ── Phase 3: Sperrauftrag — ORDERS 17115 to the NB ──────────────────────
    for (case_id, malo_id, lf_mp_id, verzug_ct) in
        pg::list_sperrauftrag_candidates(pool, tenant, threshold).await?
    {
        let cmd = ForwardCommand {
            command: "gpke.sperrung.beauftragen".to_owned(),
            marktrolle: Some("LF".to_owned()),
            malo_id: Some(malo_id.clone()),
            melo_id: None,
            payload: serde_json::json!({
                "grund":         "Zahlungsverzug (§41f EnWG)",
                "amount_due_ct": verzug_ct,
            }),
        };
        // Keyed on the case, so a retry after a lost response is the same order.
        let key = format!("accountingd-sperrauftrag-{case_id}");
        match makod.post_command(&key, &cmd).await {
            Ok(accepted) => {
                let reference = accepted.process_id.to_string();
                // The order is on the market and cannot be withdrawn by leaving a
                // column NULL: the candidate query selects on
                // `sperrauftrag_ce_id IS NULL`, so the mark commits **before** the
                // announcement. A lost announcement is replayable; a second §41f
                // disconnection order is not.
                if let Err(e) =
                    pg::mark_sperrauftrag_dispatched(pool, case_id, tenant, &reference).await
                {
                    tracing::error!(
                        error = %e, %case_id, %malo_id, %reference,
                        "accountingd: ORDERS 17115 dispatched but the case was NOT marked — \
                         the next run would order a second disconnection; mark it by hand"
                    );
                    continue;
                }
                let ce = mako_service::CloudEvent::new(
                    mako_service::source("accountingd", tenant),
                    mako_events::accounting::SPERRAUFTRAG,
                    &malo_id,
                    serde_json::json!({
                        "malo_id":         malo_id,
                        "lf_mp_id":        lf_mp_id,
                        "amount_due_ct":   verzug_ct,
                        "amount_eur":      crate::handlers::format_ct_as_eur(verzug_ct),
                        "rechtsgrundlage": "§41f EnWG",
                        "pid":             17115,
                        "process_id":      reference,
                    }),
                );
                announce(pool, &ce, case_id, &malo_id, "de.accounting.sperrauftrag").await;
                summary.sperrauftraege += 1;
                tracing::info!(%malo_id, verzug_ct, %reference, "accountingd: Sperrauftrag ORDERS 17115 dispatched (§41f EnWG)");
            }
            Err(e) => {
                tracing::warn!(error = %e, %malo_id, "accountingd: makod refused gpke.sperrung.beauftragen");
            }
        }
    }

    // ── Phase 4: Entsperrauftrag — ORDERS 17117 (§41f Abs. 7) ───────────────
    // "unverzüglich, sobald die Gründe entfallen sind": a case that was
    // disconnected and has since been settled must be reconnected without being
    // asked. Nothing did this before — the sequence was one-way, so a customer
    // who paid stayed disconnected until an operator noticed.
    for (case_id, malo_id, lf_mp_id, _) in pg::list_entsperrauftrag_candidates(pool, tenant).await?
    {
        let cmd = ForwardCommand {
            command: "gpke.entsperrung.beauftragen".to_owned(),
            marktrolle: Some("LF".to_owned()),
            malo_id: Some(malo_id.clone()),
            melo_id: None,
            payload: serde_json::json!({
                "grund": "Zahlungsrückstand ausgeglichen (§41f Abs. 7 EnWG)",
            }),
        };
        let key = format!("accountingd-entsperrauftrag-{case_id}");
        match makod.post_command(&key, &cmd).await {
            Ok(accepted) => {
                let reference = accepted.process_id.to_string();
                if let Err(e) =
                    pg::mark_entsperrauftrag_dispatched(pool, case_id, tenant, &reference).await
                {
                    tracing::error!(
                        error = %e, %case_id, %malo_id,
                        "accountingd: ORDERS 17117 dispatched but the case was NOT marked"
                    );
                    continue;
                }
                let ce = mako_service::CloudEvent::new(
                    mako_service::source("accountingd", tenant),
                    mako_events::accounting::ENTSPERRAUFTRAG,
                    &malo_id,
                    serde_json::json!({
                        "malo_id":         malo_id,
                        "lf_mp_id":        lf_mp_id,
                        "rechtsgrundlage": "§41f Abs. 7 EnWG",
                        "pid":             17117,
                        "process_id":      reference,
                    }),
                );
                announce(
                    pool,
                    &ce,
                    case_id,
                    &malo_id,
                    "de.accounting.entsperrauftrag",
                )
                .await;
                summary.entsperrauftraege += 1;
                tracing::info!(%malo_id, %reference, "accountingd: Entsperrauftrag ORDERS 17117 dispatched (§41f Abs. 7 EnWG)");
            }
            Err(e) => {
                tracing::warn!(error = %e, %malo_id, "accountingd: makod refused gpke.entsperrung.beauftragen");
            }
        }
    }

    Ok(summary)
}

/// Enqueue an announcement whose state change has already committed.
///
/// A failure here loses only the announcement, so it is logged at `ERROR` with
/// the case id and can be replayed — it must not roll the state change back.
async fn announce(
    pool: &PgPool,
    ce: &mako_service::CloudEvent,
    case_id: uuid::Uuid,
    malo_id: &str,
    ce_type: &str,
) {
    let res = async {
        let mut tx = pool.begin().await?;
        mako_service::outbox::enqueue(&mut tx, ce).await?;
        tx.commit().await?;
        anyhow::Ok(())
    }
    .await;
    if let Err(e) = res {
        tracing::error!(
            error = %e, %case_id, %malo_id, ce_type,
            "accountingd: order placed and recorded but the CloudEvent was NOT announced — replay it"
        );
    }
}
