//! §§ 41f / 41g EnWG — payment-default disconnection sequence.
//!
//! Since **23.12.2025** (BGBl. 2025 I Nr. 347, umsetzend EU-RL 2024/1711) the
//! disconnection of a Haushaltskunde for non-payment is governed by §§ 41f/41g
//! EnWG — **not** the repealed § 19 StromGVV/GasGVV (which now covers only the
//! illegal-use case). The sequence this module drives:
//!
//! 1. **Sperrandrohung** (§ 41f Abs. 1 S. 1) — threat of disconnection, ≥ 4 Wochen
//!    after the prior Mahnung. Opens the 4-Wochen-Frist.
//! 2. **Sperrankündigung** (§ 41f Abs. 5) — concrete announcement of the
//!    disconnection date, **8 Werktage im Voraus** (briefliche Mitteilung). The
//!    planned date is fixed at `today + 8 Werktage` (BDEW holiday calendar).
//! 3. **Sperrauftrag** — once the announced date has arrived, hand the order to
//!    `sperrd` (`POST /api/v1/sperr-orders`).
//!
//! Each of the first two steps is a **legal act** (a letter the ERP must send),
//! so the state flag and the outbound CloudEvent are committed in one transaction
//! (persist-before-dispatch via the transactional outbox). The sequence halts on
//! any case that has an accepted **Abwendungsvereinbarung** (§ 41g Abs. 1 S. 10)
//! or an **Unverhältnismäßigkeit/Schutzbedürftigkeit** flag (§ 41f Abs. 1 S. 2 /
//! Abs. 2) — those flags are filtered out by every phase query.
//!
//! The governing text is §§ 41f–41g EnWG in the consolidated version of
//! 23.12.2025 (BGBl. 2025 I Nr. 347).

use mako_engine::fristen::{self, HolidayCalendar};
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
}

/// Drive the full §§41f/41g sequence one step forward for every qualifying case.
///
/// Idempotent and safe to run daily: each phase query excludes cases already
/// advanced past that step (or halted), so re-running never re-issues a letter or
/// double-posts a Sperrauftrag. Returns per-phase counts. A no-op (and `Ok`) when
/// `sperrd_url` is not configured.
pub async fn run_sperr_sequence(
    pool: &PgPool,
    cfg: &AccountingdConfig,
) -> anyhow::Result<SperrSummary> {
    let Some(sperrd_url) = cfg.sperrd_url.as_deref() else {
        return Ok(SperrSummary::default());
    };
    let tenant = &cfg.tenant;
    let threshold = cfg.sperrung_threshold_ct.unwrap_or(10_000);
    let androhung_frist = cfg.sperrandrohung_frist_days.unwrap_or(28);
    let ankuendigung_wt =
        u32::try_from(cfg.sperrankuendigung_frist_werktage.unwrap_or(8)).unwrap_or(8);
    let mut summary = SperrSummary::default();

    // The Androhung and Ankündigung are **legal acts** (letters the ERP must
    // send, delivered off the emitted CloudEvent). Without an ERP webhook there
    // is no dispatch path, so we must NOT advance a case toward disconnection —
    // marking `sperrandrohung_at`/`sperrankuendigung_at` here would let it reach
    // the Sperrauftrag on schedule while the customer never received the legally
    // required notice. Pause both notice phases instead (Phase 3, the handoff to
    // sperrd, needs no ERP and stays active — but has no candidates until Phase 2
    // has run, so the sequence is inert until a webhook is configured).
    if cfg.erp_webhook_url.is_none() {
        tracing::warn!(
            "accountingd: no erp_webhook_url — §41f Sperrandrohung/Sperrankündigung cannot be \
             dispatched; the notice phases are paused (no case is advanced toward disconnection)"
        );
    } else {
        // ── Phase 1: Sperrandrohung (§41f Abs. 1) ───────────────────────────
        for (case_id, malo_id, lf_mp_id, amount_ct) in
            pg::list_androhung_candidates(pool, tenant, threshold).await?
        {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("accountingd", tenant),
                mako_events::accounting::SPERRANDROHUNG,
                &malo_id,
                serde_json::json!({
                    "malo_id": malo_id,
                    "lf_mp_id": lf_mp_id,
                    "amount_due_ct": amount_ct,
                    "rechtsgrundlage": "§41f Abs. 1 EnWG",
                    "ankuendigung_frist_wochen": 4,
                }),
            );
            let mut tx = pool.begin().await?;
            pg::mark_sperrandrohung(&mut *tx, case_id, tenant).await?;
            mako_service::outbox::enqueue(&mut tx, &ce).await?;
            tx.commit().await?;
            summary.androhungen += 1;
            tracing::info!(%malo_id, amount_ct, "accountingd: Sperrandrohung issued (§41f Abs. 1 EnWG)");
        }

        // ── Phase 2: Sperrankündigung (§41f Abs. 5) ─────────────────────────
        // The announced disconnection date is fixed at today + 8 Werktage.
        let today = OffsetDateTime::now_utc().date();
        let geplantes_sperrdatum =
            fristen::add_werktage(today, ankuendigung_wt, HolidayCalendar::BdewMaKo);
        for (case_id, malo_id, lf_mp_id, amount_ct) in
            pg::list_ankuendigung_candidates(pool, tenant, androhung_frist).await?
        {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("accountingd", tenant),
                mako_events::accounting::SPERRANKUENDIGUNG,
                &malo_id,
                serde_json::json!({
                    "malo_id": malo_id,
                    "lf_mp_id": lf_mp_id,
                    "amount_due_ct": amount_ct,
                    "rechtsgrundlage": "§41f Abs. 5 EnWG",
                    "geplantes_sperrdatum": geplantes_sperrdatum.to_string(),
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

    // ── Phase 3: Sperrauftrag → sperrd ──────────────────────────────────────
    let client = mako_service::http::default_client();
    for (case_id, malo_id, lf_mp_id, amount_ct) in
        pg::list_sperrauftrag_candidates(pool, tenant).await?
    {
        let body = serde_json::json!({
            "malo_id": malo_id,
            "lf_mp_id": lf_mp_id,
            "order_type": "sperrung",
        });
        let url = format!("{sperrd_url}/api/v1/sperr-orders");
        match client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                let reference = resp
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_owned))
                    .unwrap_or_else(|| format!("sperrd:{malo_id}"));
                // A real §41f order now exists in sperrd, `create_order` there
                // does not deduplicate, and the candidate query selects on
                // `sperrauftrag_ce_id IS NULL` — so anything that leaves that
                // column NULL makes the next run place a second order.
                //
                // The mark therefore commits on its own, before the
                // announcement. Sharing one transaction would let an outbox
                // failure roll it back; a lost announcement is replayable, a
                // duplicate disconnection is not.
                let ce = mako_service::CloudEvent::new(
                    mako_service::source("accountingd", tenant),
                    mako_events::accounting::SPERRAUFTRAG,
                    &malo_id,
                    serde_json::json!({
                        "malo_id":          malo_id,
                        "lf_mp_id":         lf_mp_id,
                        "amount_due_ct":    amount_ct,
                        "amount_eur":       format!("{:.2}", amount_ct as f64 / 100.0),
                        "rechtsgrundlage":  "§41f EnWG",
                        "sperrd_reference": reference,
                    }),
                );
                let marked =
                    pg::mark_sperrauftrag_dispatched(pool, case_id, tenant, &reference).await;
                if let Err(e) = marked {
                    tracing::warn!(error = %e, "accountingd: mark_sperrauftrag_dispatched failed");
                } else {
                    // The order is placed and recorded; announce it. A failure
                    // here loses only the announcement — logged at ERROR with
                    // the case id so it can be replayed.
                    let announced = async {
                        let mut tx = pool.begin().await?;
                        mako_service::outbox::enqueue(&mut tx, &ce).await?;
                        tx.commit().await?;
                        anyhow::Ok(())
                    }
                    .await;
                    if let Err(e) = announced {
                        tracing::error!(
                            error = %e,
                            %case_id,
                            %malo_id,
                            "accountingd: Sperrauftrag placed and recorded but \
                             de.accounting.sperrauftrag was NOT announced — replay it"
                        );
                    }
                    summary.sperrauftraege += 1;
                    tracing::info!(%malo_id, amount_ct, "accountingd: Sperrauftrag created in sperrd (§41f EnWG)");
                }
            }
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), %malo_id, "accountingd: sperrd rejected Sperrauftrag");
            }
            Err(e) => {
                tracing::warn!(error = %e, %malo_id, "accountingd: sperrd POST failed");
            }
        }
    }

    Ok(summary)
}
