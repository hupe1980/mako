//! EoG gap-closure automation (§36/§38 EnWG) — NB role.
//!
//! Closes the statutory fallback-supply loop:
//!
//! 1. **Gap trigger** — `marktd` emits `de.markt.versorgung.gap-detected`
//!    when a MaLo becomes `Unbeliefert` with no announced successor
//!    (Bestätigung Lieferende 55005/44005 without pending switch). This
//!    module looks up the Grundversorger (§36 Abs. 2 Feststellung, marktd
//!    master data) and dispatches `gpke.eog.anmelden` to `makod`, which
//!    sends the UTILMD 55013 Zuordnung (GPKE Teil 2 Kap. 2.3:
//!    "unverzüglich"; retroactive Zuordnungsbeginn allowed).
//! 2. **Activation** — `de.markt.versorgung.eog-begonnen` promotes the
//!    case to `active` and records `eog_art` + `eog_seit`.
//! 3. **§38 timer** — a daily worker scans active `ERSATZVERSORGUNG` cases
//!    against `eog_seit + 3 months` (§38 Abs. 4 S. 1 EnWG — anchored on
//!    the possibly retroactive Zuordnungsbeginn, not on detection). It
//!    warns `warn_days_before_expiry` days ahead and marks expiry;
//!    both emit `de.markt.versorgung.ersatz-auslaufend` CloudEvents to the
//!    configured webhook. `GRUNDVERSORGUNG` cases have no statutory
//!    maximum and never expire.
//!
//! After expiry the market-side follow-up is deliberately operator-driven:
//! for Haushaltskunden the transition into Grundversorgung happens by law
//! **without a market message** (GPKE Teil 2 Kap. 2.3.2.1 — the E/G's
//! billing switches price regime); for non-Haushaltskunden the NB must
//! secure the Bilanzkreis assignment (vertragliche Ersatzbelieferung E06)
//! or interrupt the Anschlussnutzung.

use mako_markt::makod_client::{ForwardCommand, MakodClient};
use mako_markt::marktd_client::MarktdClient;
use sqlx::PgPool;
use tracing::{info, warn};

// ── Config ────────────────────────────────────────────────────────────────────

/// Configuration for the EoG gap-closure automation.
#[derive(Debug, Clone)]
pub struct EogModuleConfig {
    /// When `true`, a detected gap dispatches `gpke.eog.anmelden`
    /// automatically. When `false`, cases are recorded as `detected` for
    /// operator action via `GET /api/v1/eog`.
    pub auto_activate: bool,
    /// SG4 STS Transaktionsgrund for automatic Anmeldungen. Default `ZT6`
    /// (EoG wegen Kündigung durch LF) — the dominant cause on the
    /// 55005/44005 trigger path.
    pub default_transaktionsgrund: String,
    /// Days before the §38 3-month maximum at which the warning fires.
    pub warn_days_before_expiry: u32,
    /// Optional webhook for `de.markt.versorgung.ersatz-auslaufend`
    /// CloudEvents (ERP / operator alerting).
    pub notify_webhook_url: Option<String>,
    /// HMAC-SHA256 secret for signing the `notify_webhook_url` CloudEvents.
    pub notify_webhook_secret: Option<String>,
}

impl Default for EogModuleConfig {
    fn default() -> Self {
        Self {
            auto_activate: false,
            default_transaktionsgrund: "ZT6".to_owned(),
            warn_days_before_expiry: 14,
            notify_webhook_url: None,
            notify_webhook_secret: None,
        }
    }
}

// ── Event handling ────────────────────────────────────────────────────────────

/// Handle a `de.markt.versorgung.*` CloudEvent. Returns `Ok(true)` when the
/// event was consumed by this module.
#[allow(clippy::too_many_arguments)]
pub async fn handle_versorgung_event(
    event: &serde_json::Value,
    cfg: &EogModuleConfig,
    marktd: &MarktdClient,
    makod: &MakodClient,
    pool: &PgPool,
    tenant: &str,
    own_mp_id: &str,
) -> anyhow::Result<bool> {
    let ce_type = event["type"].as_str().unwrap_or("");
    match ce_type {
        t if t == mako_events::markt::VERSORGUNG_GAP_DETECTED => {
            handle_gap_detected(event, cfg, marktd, makod, pool, tenant, own_mp_id).await?;
            Ok(true)
        }
        t if t == mako_events::markt::VERSORGUNG_EOG_BEGONNEN => {
            handle_eog_begonnen(event, pool, tenant).await?;
            Ok(true)
        }
        t if t == mako_events::markt::VERSORGUNG_CHANGED => {
            handle_versorgung_changed(event, pool, tenant).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// An `angemeldet` case older than this is stuck, not in flight: `makod`
/// answers the 55013/44013 Zuordnung well inside the window, and no timer
/// touches `angemeldet`. Re-detection is allowed to reopen it past this age.
const ANGEMELDET_STALE_HOURS: i64 = 72;

/// Regular supply resumed — close the case.
///
/// This is the only writer of `closed`. Without it the state existed in the
/// CHECK constraint and nothing ever reached it, so the case log kept reporting
/// fallback supply for MaLos that had a Lieferant again.
async fn handle_versorgung_changed(
    event: &serde_json::Value,
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<()> {
    let data = &event["data"];
    let Some(malo_id) = data["malo_id"]
        .as_str()
        .or_else(|| event["marktmaloid"].as_str())
        .or_else(|| event["subject"].as_str())
    else {
        return Ok(());
    };
    // marktd emits the LieferStatus Display form.
    if !data["lieferstatus"]
        .as_str()
        .is_some_and(|s| s.eq_ignore_ascii_case("beliefert"))
    {
        return Ok(());
    }

    let closed = sqlx::query(
        r"UPDATE eog_activations
          SET status = 'closed', updated_at = now()
          WHERE tenant = $1 AND malo_id = $2
            AND status IN ('detected', 'angemeldet', 'active', 'expiring', 'expired')",
    )
    .bind(tenant)
    .bind(malo_id)
    .execute(pool)
    .await?
    .rows_affected();
    if closed > 0 {
        info!(
            malo_id,
            "processd EoG: regular supply resumed — case closed"
        );
    }
    Ok(())
}

/// Gap detected: record the case and (when `auto_activate`) dispatch the
/// UTILMD 55013 Zuordnung to the Grundversorger.
async fn handle_gap_detected(
    event: &serde_json::Value,
    cfg: &EogModuleConfig,
    marktd: &MarktdClient,
    makod: &MakodClient,
    pool: &PgPool,
    tenant: &str,
    own_mp_id: &str,
) -> anyhow::Result<()> {
    let data = &event["data"];
    let Some(malo_id) = data["malo_id"].as_str() else {
        warn!("processd EoG: gap-detected event without malo_id — ignored");
        return Ok(());
    };
    let sparte_str = data["sparte"].as_str().unwrap_or("STROM");
    let sparte: mako_markt::domain::Sparte = sparte_str
        .parse()
        .unwrap_or(mako_markt::domain::Sparte::Strom);
    let nb_mp_id = data["nb_mp_id"].as_str().unwrap_or(own_mp_id);

    // Idempotency: one open case per MaLo. Re-detection reopens a closed case
    // (a MaLo can fall into a gap repeatedly) and a stale `angemeldet` one —
    // nothing else moves that state, so a dispatch whose Zuordnung never landed
    // would otherwise sit there permanently.
    let inserted = sqlx::query(
        r"INSERT INTO eog_activations (tenant, malo_id, sparte, status, detail)
          VALUES ($1, $2, $3, 'detected', $4)
          ON CONFLICT (tenant, malo_id) DO UPDATE
          SET sparte = EXCLUDED.sparte,
              status = 'detected',
              gv_mp_id = NULL, eog_art = NULL, eog_seit = NULL,
              haushaltskunde = NULL, warned_at = NULL, expired_at = NULL,
              detail = EXCLUDED.detail,
              updated_at = now()
          WHERE eog_activations.status IN ('closed', 'expired', 'detected')
             OR (eog_activations.status = 'angemeldet'
                 AND eog_activations.updated_at < now() - make_interval(hours => $5))",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(sparte.to_string())
    .bind(format!("gap detected via PID {}", data["pid"]))
    .bind(i32::try_from(ANGEMELDET_STALE_HOURS).unwrap_or(72))
    .execute(pool)
    .await?
    .rows_affected();
    if inserted == 0 {
        // An angemeldet/active case already exists — at-least-once redelivery.
        return Ok(());
    }

    if !cfg.auto_activate {
        warn!(
            malo_id,
            "processd EoG: supply gap detected — auto_activate off, operator must \
             dispatch gpke.eog.anmelden (§38 EnWG: Zuordnung unverzüglich)"
        );
        return Ok(());
    }
    // Command + PID by Sparte: Strom rides gpke.eog.anmelden (55013), Gas rides
    // geli.eog.anmelden (44013). Both spawn a GNB-initiator EoG process in makod.
    let (eog_command, eog_pid) = match sparte {
        mako_markt::domain::Sparte::Gas => (mako_markt::commands::GELI_EOG_ANMELDEN, 44013),
        mako_markt::domain::Sparte::Strom => (mako_markt::commands::GPKE_EOG_ANMELDEN, 55013),
    };

    // Resolve the Grundversorger (§36 Abs. 2 Feststellung).
    let gv = match marktd.get_grundversorger(nb_mp_id, sparte).await {
        Ok(Some(gv)) => gv,
        Ok(None) => {
            warn!(
                malo_id, nb_mp_id, %sparte,
                "processd EoG: no Grundversorger Feststellung in marktd — case \
                 escalated (PUT /api/v1/grundversorger/{{nb_mp_id}} to fix)"
            );
            return Ok(());
        }
        // Nothing retries a 'detected' case, so acking here strands it forever.
        Err(e) => {
            warn!(malo_id, error = %e, "processd EoG: Grundversorger lookup failed");
            return Err(e.into());
        }
    };

    // Zuordnungsbeginn: day after the recorded Lieferende when known
    // (retroactive per GPKE Teil 2 Kap. 2.3), otherwise today.
    let zuordnungsbeginn = match marktd.get_versorgung(malo_id).await {
        Ok(Some(vs)) => vs
            .lieferende
            .and_then(|d| d.next_day())
            .unwrap_or_else(mako_fristen::heute),
        _ => mako_fristen::heute(),
    };

    let cmd = ForwardCommand {
        marktrolle: None,
        command: eog_command.to_owned(),
        malo_id: Some(malo_id.to_owned()),
        melo_id: None,
        payload: serde_json::json!({
            "malo_id":           malo_id,
            "gv_mp_id":          gv.gv_mp_id,
            "process_date":      zuordnungsbeginn.to_string(),
            "transaktionsgrund": cfg.default_transaktionsgrund,
        }),
    };
    makod
        .post_command(&format!("processd-eog-{tenant}-{malo_id}"), &cmd)
        .await
        .inspect_err(
            |e| warn!(%e, malo_id, eog_command, "processd EoG: EoG anmelden dispatch failed"),
        )?;

    sqlx::query(
        r"UPDATE eog_activations
          SET status = 'angemeldet', gv_mp_id = $3, eog_seit = $4, updated_at = now()
          WHERE tenant = $1 AND malo_id = $2",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(&gv.gv_mp_id)
    .bind(zuordnungsbeginn)
    .execute(pool)
    .await?;

    info!(
        malo_id,
        gv_mp_id = %gv.gv_mp_id,
        %zuordnungsbeginn,
        eog_command,
        eog_pid,
        "processd EoG: dispatched EoG anmelden"
    );
    Ok(())
}

/// EoG began (marktd recorded the fallback supply): promote the case.
async fn handle_eog_begonnen(
    event: &serde_json::Value,
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<()> {
    let data = &event["data"];
    let Some(malo_id) = data["malo_id"].as_str() else {
        return Ok(());
    };
    let sparte = data["sparte"]
        .as_str()
        .and_then(|s| s.parse::<mako_markt::domain::Sparte>().ok())
        .unwrap_or(mako_markt::domain::Sparte::Strom);
    let eog_art = data["eog_art"].as_str().map(|s| {
        // marktd emits the LieferStatus Display form; normalise to the
        // SCREAMING_SNAKE wire labels used in eog_activations.
        match s {
            "Ersatzversorgung" => "ERSATZVERSORGUNG".to_owned(),
            "Grundversorgung" => "GRUNDVERSORGUNG".to_owned(),
            other => other.to_uppercase(),
        }
    });
    // §38 Abs. 4 S. 1 EnWG runs from the Zuordnungsbeginn, which may be
    // retroactive. Anchoring an unusable value on TODAY silently extends the
    // statutory 3-month maximum; the event time is the closest honest floor.
    let event_time = event["time"]
        .as_str()
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
        .unwrap_or_else(time::OffsetDateTime::now_utc)
        .date();
    let eog_seit = match data["eog_seit"].as_str() {
        Some(s) => time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_else(|e| {
                warn!(
                    malo_id, eog_seit = s, error = %e,
                    "processd EoG: unparseable eog_seit — §38 clock anchored on the event time"
                );
                event_time
            }),
        None => {
            warn!(
                malo_id,
                "processd EoG: eog-begonnen without eog_seit — §38 clock anchored on the event time"
            );
            event_time
        }
    };
    let haushaltskunde = data["haushaltskunde"].as_bool();

    sqlx::query(
        r"INSERT INTO eog_activations
              (tenant, malo_id, sparte, status, gv_mp_id, eog_art, eog_seit, haushaltskunde)
          VALUES ($1, $2, $7, 'active', $3, $4, $5, $6)
          ON CONFLICT (tenant, malo_id) DO UPDATE
          SET status = 'active',
              sparte = EXCLUDED.sparte,
              gv_mp_id = COALESCE(EXCLUDED.gv_mp_id, eog_activations.gv_mp_id),
              eog_art = EXCLUDED.eog_art,
              eog_seit = EXCLUDED.eog_seit,
              haushaltskunde = COALESCE(EXCLUDED.haushaltskunde, eog_activations.haushaltskunde),
              updated_at = now()",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(data["gv_mp_id"].as_str())
    .bind(&eog_art)
    .bind(eog_seit)
    .bind(haushaltskunde)
    .bind(sparte.to_string())
    .execute(pool)
    .await?;

    info!(malo_id, ?eog_art, %eog_seit, "processd EoG: fallback supply active");
    Ok(())
}

// ── §38 timer ─────────────────────────────────────────────────────────────────

/// Row surfaced by the §38 timer sweep.
#[derive(Debug, sqlx::FromRow)]
pub struct EogTimerHit {
    pub malo_id: String,
    pub gv_mp_id: Option<String>,
    pub eog_seit: Option<time::Date>,
    pub haushaltskunde: Option<bool>,
}

/// Daily sweep: warn ahead of and mark expiry of the §38 Abs. 4 S. 1 EnWG
/// 3-month maximum. Returns `(warned, expired)` counts.
///
/// `INTERVAL '3 months'` is calendar-month arithmetic — exactly the statute's
/// "drei Monate nach Beginn der Ersatzenergieversorgung".
pub async fn sweep_ersatzversorgung_timer(
    pool: &PgPool,
    cfg: &EogModuleConfig,
    tenant: &str,
    client: &reqwest::Client,
) -> anyhow::Result<(u64, u64)> {
    // Phase 1 — warning window.
    let warned: Vec<EogTimerHit> = sqlx::query_as(
        r"UPDATE eog_activations
          SET status = 'expiring', warned_at = now(), updated_at = now()
          WHERE tenant = $1
            AND status = 'active'
            AND eog_art = 'ERSATZVERSORGUNG'
            AND eog_seit + INTERVAL '3 months' <= heute() + make_interval(days => $2)
          RETURNING malo_id, gv_mp_id, eog_seit, haushaltskunde",
    )
    .bind(tenant)
    .bind(i32::try_from(cfg.warn_days_before_expiry).unwrap_or(14))
    .fetch_all(pool)
    .await?;

    // Phase 2 — expiry.
    let expired: Vec<EogTimerHit> = sqlx::query_as(
        r"UPDATE eog_activations
          SET status = 'expired', expired_at = now(), updated_at = now()
          WHERE tenant = $1
            AND status IN ('active', 'expiring')
            AND eog_art = 'ERSATZVERSORGUNG'
            AND eog_seit + INTERVAL '3 months' <= heute()
          RETURNING malo_id, gv_mp_id, eog_seit, haushaltskunde",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await?;

    for (phase, hits) in [("warning", &warned), ("expired", &expired)] {
        for hit in hits {
            warn!(
                malo_id = %hit.malo_id,
                phase,
                eog_seit = ?hit.eog_seit,
                haushaltskunde = ?hit.haushaltskunde,
                "processd EoG: §38 Abs. 4 EnWG 3-month maximum — {}",
                if phase == "expired" {
                    "Ersatzversorgung ended by law; Haushaltskunde → Grundversorgung \
                     (automatic, no market message); otherwise secure BK assignment \
                     or interrupt (operator action)"
                } else {
                    "approaching"
                }
            );
            notify_ersatz_auslaufend(cfg, client, tenant, hit, phase).await;
        }
    }

    Ok((warned.len() as u64, expired.len() as u64))
}

/// POST a `de.markt.versorgung.ersatz-auslaufend` CloudEvent to the
/// configured webhook (fire-and-forget; failures are logged only).
async fn notify_ersatz_auslaufend(
    cfg: &EogModuleConfig,
    client: &reqwest::Client,
    tenant: &str,
    hit: &EogTimerHit,
    phase: &str,
) {
    let Some(url) = cfg.notify_webhook_url.as_deref() else {
        return;
    };
    let ce = mako_service::CloudEvent::new(
        mako_service::source("processd", tenant),
        mako_events::markt::VERSORGUNG_ERSATZ_AUSLAUFEND,
        &hit.malo_id,
        serde_json::json!({
            "tenant":         tenant,
            "malo_id":        hit.malo_id,
            "gv_mp_id":       hit.gv_mp_id,
            "eog_seit":       hit.eog_seit.map(|d| d.to_string()),
            "haushaltskunde": hit.haushaltskunde,
            "phase":          phase,
            "legal_basis":    "§38 Abs. 4 S. 1 EnWG",
        }),
    );
    if let Err(e) = mako_service::post_ce_with_retry(
        client,
        url,
        &ce,
        cfg.notify_webhook_secret.as_deref().map(str::as_bytes),
    )
    .await
    {
        warn!(error = %e, malo_id = %hit.malo_id, "processd EoG: ersatz-auslaufend webhook failed");
    }
}

// ── REST ──────────────────────────────────────────────────────────────────────

/// Case row for `GET /api/v1/eog`.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct EogCaseRow {
    pub malo_id: String,
    pub sparte: String,
    pub status: String,
    pub gv_mp_id: Option<String>,
    pub eog_art: Option<String>,
    pub eog_seit: Option<time::Date>,
    pub haushaltskunde: Option<bool>,
    pub detail: Option<String>,
}

/// List EoG cases, optionally filtered by status.
pub async fn list_cases(
    pool: &PgPool,
    tenant: &str,
    status: Option<&str>,
) -> anyhow::Result<Vec<EogCaseRow>> {
    let rows = if let Some(status) = status {
        sqlx::query_as(
            r"SELECT malo_id, sparte, status, gv_mp_id, eog_art, eog_seit,
                     haushaltskunde, detail
              FROM eog_activations
              WHERE tenant = $1 AND status = $2
              ORDER BY updated_at DESC LIMIT 500",
        )
        .bind(tenant)
        .bind(status)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            r"SELECT malo_id, sparte, status, gv_mp_id, eog_art, eog_seit,
                     haushaltskunde, detail
              FROM eog_activations
              WHERE tenant = $1
              ORDER BY updated_at DESC LIMIT 500",
        )
        .bind(tenant)
        .fetch_all(pool)
        .await?
    };
    Ok(rows)
}
