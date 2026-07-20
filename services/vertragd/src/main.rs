//! `vertragd` — B2B + B2C Contract & Customer Management.
//!
//! Manages the full retail contract lifecycle for both B2C (Haushalt/SLP) and
//! B2B (Gewerbe/RLM/HV) customers:
//!
//! ## Data model
//!
//! ```text
//! Kunde (B2C: Person/Haushalt, B2B: Unternehmen/Gewerbe)
//! ├── [B2B] Rahmenvertrag (Master Framework Contract)
//! │    └── N × Versorgungsvertrag  (one per delivery location / site)
//! │          └── N × Vertragskomponente  (STROM|GAS|HEMS|...)
//! └── [B2C] Versorgungsvertrag  (bundle, no Rahmenvertrag)
//!       └── N × Vertragskomponente
//! ```
//!
//! ## Key capabilities
//!
//! - **Kundenverwaltung**: typed `rubo4e::current::Geschaeftspartner` with OIDC sub for
//!   `portald` resource-level authorization (OIDC `sub` → Kunde → MaLo IDs)
//! - **B2B Rahmenverträge**: portfolio pricing, consolidated invoicing, indexation clauses,
//!   multi-site Lieferbeginn orchestration, auto-renewal
//! - **Vertragsmanagement**: notice periods, price guarantees, Tarifwechsel,
//!   Kündigung with coordinated Schlussablesung
//! - **MaKo orchestration**: triggers GPKE/GeLi Gas Lieferbeginn/-ende via `processd`
//! - **Ablesesteuerung**: automatic LIEFERBEGINN/LIEFERENDE reading orders to `edmd`
//! - **Post-confirmation provisioning**: `tarifbd` product assignment + `accountingd`
//!   billing account
//!
//! ## CloudEvents emitted
//!
//! | Event | Trigger |
//! |---|---|
//! | `de.vertrag.aktiv` | All components confirmed, billing can start |
//! | `de.vertrag.teilerfuellung` | First component confirmed |
//! | `de.vertrag.gekuendigt` | Lieferende dispatched |
//! | `de.vertrag.abgeschlossen` | All components ended |
//!
//! Port: `:9780`
//!
//! ## Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/kunden` | Create / upsert customer (idempotent on erp_kunde_id) |
//! | `GET`  | `/api/v1/kunden/{id}` | Get customer + active MaLo IDs |
//! | `GET`  | `/api/v1/kunden/by-sub/{sub}` | portald authorization: OIDC sub → customer + MaLos |
//! | `POST` | `/api/v1/kunden/{id}/rahmenvertraege` | Create B2B framework contract |
//! | `POST` | `/api/v1/kunden/{id}/vertraege` | Create supply contract (B2C or B2B) |
//! | `GET`  | `/api/v1/vertraege` | List open contracts |
//! | `GET`  | `/api/v1/vertraege/{id}` | Get contract + components |
//! | `POST` | `/api/v1/vertraege/{id}/kuendigen` | Terminate contract (Lieferende) |
//! | `POST` | `/api/v1/events` | Inbound CloudEvents from makod / processd |

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post},
};
use mako_service::{health::health_routes, load_config};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;
use vertragd::{config, handlers, mcp_server, pg};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mako_service::init_tracing_from_env("vertragd");
    let cfg: config::VertragdConfig = load_config("vertragd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    // Run migrations automatically on startup.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run migrations")?;

    // Shared HTTP client — avoids TCP handshake overhead per request.
    let http_client: Arc<reqwest::Client> = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .context("build HTTP client")?,
    );

    // ── OIDC/JWT authentication ───────────────────────────────────────────────
    // Disabled in dev (OidcVerifier::disabled) when `oidc` section is absent.
    // All write endpoints require a valid Bearer token in production.
    let ct = mako_service::shutdown::token();
    let oidc = mako_service::oidc::OidcConfig::build_verifier(
        cfg.oidc.as_ref(),
        &http_client,
        &cfg.tenant,
        ct.clone(),
    )
    .await
    .context("OIDC setup")?;
    let mcp_state = Arc::new(mcp_server::VertragdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
    });

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        // Customer management
        .route(
            "/api/v1/kunden",
            get(handlers::list_kunden_handler).post(handlers::post_create_kunde),
        )
        .route(
            "/api/v1/kunden/:id",
            get(handlers::get_kunde).put(handlers::put_update_kunde),
        )
        .route(
            "/api/v1/kunden/by-sub/:sub",
            get(handlers::get_kunde_by_sub),
        )
        .route(
            "/api/v1/kunden/authenticate",
            get(handlers::get_authenticate),
        )
        // Identity management (B2B portal users: 1 company → N logins)
        .route(
            "/api/v1/kunden/:id/identitaeten",
            post(handlers::post_upsert_identitaet).get(handlers::list_kunde_identitaeten),
        )
        .route(
            "/api/v1/kunden/:id/identitaeten/:sub",
            axum::routing::delete(handlers::delete_identitaet),
        )
        // GDPR Art. 15 full data export
        .route(
            "/api/v1/kunden/:id/export",
            get(handlers::get_kunde_gdpr_export),
        )
        // GDPR Art. 17 — right to erasure (anonymize PII, retain contract records)
        .route(
            "/api/v1/kunden/:id/anonymize",
            post(handlers::post_anonymize_kunde),
        )
        // Person sub-object — B2C natural person details (L13 — GDPR Art. 15)
        .route(
            "/api/v1/kunden/:id/person",
            get(handlers::get_person).put(handlers::put_person),
        )
        // Zahlungsinformation typed BO4E REST (IBAN + BIC + SEPA)
        .route(
            "/api/v1/kunden/:id/zahlungsinformation",
            get(handlers::get_zahlungsinformation_kunde)
                .put(handlers::put_zahlungsinformation_kunde),
        )
        // Rahmenverträge — list all (operator CRM) and single fetch
        .route(
            "/api/v1/rahmenvertraege",
            get(handlers::list_rahmenvertraege_handler),
        )
        .route(
            "/api/v1/rahmenvertraege/:id",
            get(handlers::get_rahmenvertrag_handler),
        )
        // Rahmenvertrag MaLo enumeration for Sammelrechnung (L2)
        .route(
            "/api/v1/rahmenvertraege/:id/malos",
            get(handlers::get_rahmenvertrag_malos),
        )
        // Framework + supply contracts
        .route(
            "/api/v1/kunden/:id/rahmenvertraege",
            get(handlers::list_kunde_rahmenvertraege).post(handlers::post_create_rahmenvertrag),
        )
        .route(
            "/api/v1/kunden/:id/vertraege",
            get(handlers::list_kunde_vertraege).post(handlers::post_create_vertrag),
        )
        // Supply contracts (B2C + B2B)
        .route("/api/v1/vertraege", get(handlers::list_vertraege))
        .route(
            "/api/v1/vertraege/by-malo/:malo_id",
            get(handlers::get_vertrag_by_malo),
        )
        // Expiring contracts monitor (§13 GasGVV / §14 StromGVV / §41 EnWG)
        .route(
            "/api/v1/vertraege/expiring",
            get(handlers::list_expiring_vertraege),
        )
        .route("/api/v1/vertraege/:id", get(handlers::get_vertrag))
        .route(
            "/api/v1/vertraege/:id/kuendigen",
            post(handlers::kuendige_vertrag),
        )
        // Stornieren (cancel before AKTIV — ANGELEGT/IN_BEARBEITUNG only)
        .route(
            "/api/v1/vertraege/:id/stornieren",
            post(handlers::stornieren_vertrag),
        )
        // Kündigung Widerruf (GPKE §20 EnWG: LF may withdraw Lieferende before effective date)
        .route(
            "/api/v1/vertraege/:id/widerruf-kuendigung",
            post(handlers::widerruf_kuendigung_handler),
        )
        // B2B Rahmenvertrag cascade Kündigung (terminates all child Versorgungsverträge)
        .route(
            "/api/v1/rahmenvertraege/:id/kuendigen",
            post(handlers::kuendige_rahmenvertrag_handler),
        )
        .route(
            "/api/v1/vertraege/:id/tarifwechsel",
            post(handlers::tarifwechsel_vertrag),
        )
        // Preisgarantie typed BO4E REST resource (guard on tarifwechsel enforces it)
        .route(
            "/api/v1/vertraege/:id/preisgarantie",
            get(handlers::get_preisgarantie).put(handlers::put_preisgarantie),
        )
        // B2B portfolio summary (all active MaLo/Sparte per Kunde)
        .route(
            "/api/v1/kunden/:id/portfolio",
            get(handlers::get_kunde_portfolio),
        )
        // CloudEvent webhook
        .route("/api/v1/events", post(handlers::post_cloud_event))
        // CPQ: de.angebot.angenommen → auto-create Rahmenvertrag + Versorgungsverträge
        .route(
            "/api/v1/webhooks/angebot",
            axum::routing::post(handlers::post_angebot_webhook),
        )
        .merge(mcp_server::router(mcp_state, ct.clone()))
        .layer(Extension(oidc))
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(Arc::clone(&http_client)))
        .layer(Extension(pool.clone()));

    let port = cfg.port.unwrap_or(9780);
    let addr = format!("0.0.0.0:{port}");
    info!(%addr, "vertragd starting");

    // \u2500\u2500 Auto-renewal background worker (§13 GasGVV / §14 StromGVV) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    // Runs daily. Finds AKTIV vertr\u00e4ge with auto_renewal=true whose vertragsende
    // is within the next 30 days, emits a 30-day advance notification CloudEvent
    // (`de.vertrag.autoerneuerung.ankuendigung`), and extends vertragsende
    // by renewal_monate on the vertragsende date itself.
    {
        let pool_ar = pool.clone();
        let cfg_ar = Arc::clone(&cfg);
        let client_ar = Arc::clone(&http_client);
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            loop {
                let today = time::OffsetDateTime::now_utc().date();
                // Phase 1: 30-day advance notification
                if let Ok(due) = pg::find_auto_renewal_due(&pool_ar, &cfg_ar.tenant, 30).await {
                    for row in &due {
                        if let Some(ref url) = cfg_ar.erp_webhook_url {
                            let ce = serde_json::json!({
                                "specversion": "1.0",
                                "type": "de.vertrag.autoerneuerung.ankuendigung",
                                "source": format!("urn:vertragd:lf:{}", cfg_ar.lf_mp_id),
                                "id": uuid::Uuid::new_v4().to_string(),
                                "time": time::OffsetDateTime::now_utc().to_string(),
                                "data": {
                                    "vertrag_id": row.id.to_string(),
                                    "vertrags_nr": row.vertrags_nr,
                                    "kunden_id": row.kunden_id.to_string(),
                                    "vertragsende": row.vertragsende.to_string(),
                                    "renewal_monate": row.renewal_monate,
                                    "regulatory_basis": "§13 GasGVV / §14 StromGVV"
                                }
                            });
                            let _ = client_ar.post(url).json(&ce).send().await;
                        }
                    }
                }
                // Phase 2: Apply renewals due today
                if let Ok(due) = pg::find_auto_renewal_due(&pool_ar, &cfg_ar.tenant, 0).await {
                    for row in &due {
                        if row.vertragsende == today
                            && let Ok(new_end) = time::Date::from_calendar_date(
                                today.year() + row.renewal_monate / 12,
                                today.month(),
                                today.day().clamp(1, 28),
                            )
                        {
                            if let Err(e) = pg::apply_auto_renewal(&pool_ar, row.id, new_end).await
                            {
                                tracing::error!(vertrag_id = %row.id, error = %e, "vertragd: auto-renewal failed");
                            } else {
                                tracing::info!(vertrag_id = %row.id, new_end = %new_end, "vertragd: auto-renewal applied");
                            }
                        }
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
            }
        });
    }
    // Runs daily and:
    //   1. Applies pending Tarifwechsel whose wirksamkeit date has arrived.
    //   2. Emits `de.vertrag.preisaenderung.ankuendigung` CE for Tarifwechsel
    //      whose wirksamkeit is ~42 days away (\u00a741 Abs. 3 EnWG: \u22656 weeks advance notice).
    {
        let pool_bg = pool.clone();
        let cfg_bg = Arc::clone(&cfg);
        let client_bg = Arc::clone(&http_client);
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
            loop {
                let today = time::OffsetDateTime::now_utc().date();

                // Phase 1: Apply due Tarifwechsel
                match pg::find_tarifwechsel_due_today(&pool_bg, &cfg_bg.tenant, today).await {
                    Ok(due) => {
                        for row in &due {
                            if let Err(e) =
                                pg::apply_pending_tarifwechsel(&pool_bg, row.komp_id).await
                            {
                                tracing::error!(
                                    komp_id = %row.komp_id,
                                    error = %e,
                                    "vertragd: apply_pending_tarifwechsel failed"
                                );
                                continue;
                            }
                            tracing::info!(
                                komp_id = %row.komp_id,
                                new_product = %row.pending_product_code,
                                wirksamkeit = %row.pending_wirksamkeit,
                                "vertragd: Tarifwechsel applied"
                            );
                            // Emit tarifwechsel CE
                            if let Some(ref url) = cfg_bg.erp_webhook_url {
                                let ce = serde_json::json!({
                                    "specversion": "1.0",
                                    "type": "de.vertrag.tarifwechsel",
                                    "source": format!("urn:vertragd:lf:{}", cfg_bg.lf_mp_id),
                                    "id": uuid::Uuid::new_v4().to_string(),
                                    "time": time::OffsetDateTime::now_utc().to_string(),
                                    "datacontenttype": "application/json",
                                    "data": {
                                        "komp_id": row.komp_id.to_string(),
                                        "malo_id": row.malo_id,
                                        "new_product_code": row.pending_product_code,
                                        "wirksamkeit": row.pending_wirksamkeit.to_string(),
                                    }
                                });
                                let client = client_bg.clone();
                                let _ = client
                                    .post(url)
                                    .header("Content-Type", "application/cloudevents+json")
                                    .json(&ce)
                                    .send()
                                    .await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "vertragd: find_tarifwechsel_due_today failed");
                    }
                }

                // Phase 2: Emit 6-week advance notifications (\u00a741 Abs. 3 EnWG)
                match pg::find_tarifwechsel_needing_notif(&pool_bg, &cfg_bg.tenant, today).await {
                    Ok(pending) => {
                        for row in &pending {
                            tracing::info!(
                                komp_id = %row.komp_id,
                                wirksamkeit = %row.pending_wirksamkeit,
                                product = %row.pending_product_code,
                                "vertragd: emitting Preisanpassungsbenachrichtigung (§41 Abs. 3 EnWG)"
                            );
                            if let Some(ref url) = cfg_bg.erp_webhook_url {
                                let ce = serde_json::json!({
                                    "specversion": "1.0",
                                    "type": "de.vertrag.preisaenderung.ankuendigung",
                                    "source": format!("urn:vertragd:lf:{}", cfg_bg.lf_mp_id),
                                    "id": uuid::Uuid::new_v4().to_string(),
                                    "time": time::OffsetDateTime::now_utc().to_string(),
                                    "datacontenttype": "application/json",
                                    "data": {
                                        "komp_id": row.komp_id.to_string(),
                                        "malo_id": row.malo_id,
                                        "current_product_code": row.current_product_code,
                                        "new_product_code": row.pending_product_code,
                                        "wirksamkeit": row.pending_wirksamkeit.to_string(),
                                        "days_until_change": 42,
                                        "regulatory_basis": "\u{00a7}41 Abs. 3 EnWG",
                                    }
                                });
                                let client = client_bg.clone();
                                match client
                                    .post(url)
                                    .header("Content-Type", "application/cloudevents+json")
                                    .json(&ce)
                                    .send()
                                    .await
                                {
                                    Ok(_) => {
                                        let _ = pg::mark_preisanpassung_notif_sent(
                                            &pool_bg,
                                            row.komp_id,
                                        )
                                        .await;
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            komp_id = %row.komp_id,
                                            error = %e,
                                            "vertragd: Preisanpassungsbenachrichtigung webhook failed -- will retry"
                                        );
                                    }
                                }
                            } else {
                                // No webhook configured: mark sent anyway so we don't log endlessly.
                                tracing::warn!(
                                    komp_id = %row.komp_id,
                                    "vertragd: Preisanpassungsbenachrichtigung -- no erp_webhook_url configured"
                                );
                                let _ =
                                    pg::mark_preisanpassung_notif_sent(&pool_bg, row.komp_id).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "vertragd: find_tarifwechsel_needing_notif failed");
                    }
                }

                // Run daily; 23h interval is DST-safe.
                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
            }
        });
    }

    // ── §41 EnWG / §13 GasGVV expiry notification worker ─────────────────────
    // Runs daily. Finds contracts expiring within 30 days and emits
    // `de.vertrag.ablauf.ankuendigung` per contract so the ERP can trigger
    // proactive renewal or churn-prevention workflows.
    {
        let pool_exp = pool.clone();
        let cfg_exp = Arc::clone(&cfg);
        let client_exp = Arc::clone(&http_client);
        tokio::spawn(async move {
            // Initial delay: stagger workers to avoid DB contention at startup.
            tokio::time::sleep(tokio::time::Duration::from_secs(45)).await;
            loop {
                match pg::find_expiring_vertraege(&pool_exp, &cfg_exp.tenant, 30).await {
                    Ok(rows) => {
                        if !rows.is_empty() {
                            tracing::info!(
                                count = rows.len(),
                                "vertragd: dispatching expiry notifications"
                            );
                        }
                        for row in &rows {
                            if let Some(ref url) = cfg_exp.erp_webhook_url {
                                let ce = serde_json::json!({
                                    "specversion": "1.0",
                                    "type": "de.vertrag.ablauf.ankuendigung",
                                    "source": format!("urn:vertragd:lf:{}", cfg_exp.lf_mp_id),
                                    "id": uuid::Uuid::new_v4().to_string(),
                                    "time": time::OffsetDateTime::now_utc().to_string(),
                                    "datacontenttype": "application/json",
                                    "data": {
                                        "vertrag_id": row.id.to_string(),
                                        "kunden_id": row.kunden_id.to_string(),
                                        "vertrags_nr": row.vertrags_nr,
                                        "vertragsende": row.vertragsende.map(|d| d.to_string()),
                                        "preisgarantie_bis": row.preisgarantie_bis.map(|d| d.to_string()),
                                        "auto_renewal": row.auto_renewal,
                                        "kundentyp": row.kundentyp,
                                        "standort_bezeichnung": row.standort_bezeichnung,
                                        "regulatory_basis": "§13 GasGVV / §14 StromGVV / §41 EnWG",
                                    }
                                });
                                let body = match serde_json::to_vec(&ce) {
                                    Ok(b) => b,
                                    Err(_) => continue,
                                };
                                let mut req = client_exp
                                    .post(url)
                                    .header("Content-Type", "application/cloudevents+json");
                                if let Some(ref secret) = cfg_exp.erp_hmac_secret {
                                    let sig = format!(
                                        "sha256={}",
                                        mako_service::webhook::hmac_hex(secret.as_bytes(), &body,)
                                    );
                                    req = req.header("X-Mako-Signature", sig);
                                }
                                let _ = req.body(body).send().await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "vertragd: find_expiring_vertraege failed");
                    }
                }
                // 23h interval is DST-safe.
                tokio::time::sleep(tokio::time::Duration::from_secs(23 * 3600)).await;
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    mako_service::shutdown::serve(listener, app, ct).await
}
